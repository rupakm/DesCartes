use std::cmp::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::{Component, Key, Scheduler, SimTime, Simulation, SimulationConfig};

use des_explore::harness::HarnessContext;
use des_explore::monitor::{Monitor, MonitorConfig, MonitorStatus, ScoreWeights};
use des_explore::policy::ActionDistribution;
use des_explore::policy_search::{
    search_policy, BaseFrontierPolicy, BaseTokioReadyPolicy, FailurePhase, PolicySearchConfig,
    PolicySearchObjective,
};
use des_explore::schedule_explore::DecisionKey;
use des_explore::stats::wilson_interval;
use des_explore::trace::Trace;

#[derive(Debug, Clone)]
enum Ev {
    A,
    B,
}

/// Minimal schedule-sensitive bug:
/// - processing `A` arms the component
/// - processing `B` first triggers an `observe_drop` ("Bad")
struct FrontierOrderDropBug {
    armed: bool,
    monitor: Arc<Mutex<Monitor>>,
}

impl Component for FrontierOrderDropBug {
    type Event = Ev;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        let now = scheduler.time();
        match event {
            Ev::A => {
                self.armed = true;
            }
            Ev::B => {
                if !self.armed {
                    self.monitor.lock().unwrap().observe_drop(now);
                }
            }
        }
    }
}

fn monitor_for_drops() -> Arc<Mutex<Monitor>> {
    let mut mcfg = MonitorConfig::default();
    mcfg.window = Duration::from_millis(1);
    mcfg.track_latency_quantiles = false;

    // Make the score depend only on drops, so `score > 0` is our "Bad" predicate.
    mcfg.score_weights = ScoreWeights {
        queue_mean: 0.0,
        retry_amplification: 0.0,
        timeout_rate_rps: 0.0,
        drop_rate_rps: 1.0,
        distance: 0.0,
    };

    Arc::new(Mutex::new(Monitor::new(mcfg, SimTime::zero())))
}

fn setup_frontier_bug(
    config: SimulationConfig,
    _ctx: &HarnessContext,
    _prefix: Option<&Trace>,
    _cont_seed: u64,
) -> (Simulation, Arc<Mutex<Monitor>>) {
    let mut sim = Simulation::new(config);
    let monitor = monitor_for_drops();

    let key = sim.add_component(FrontierOrderDropBug {
        armed: false,
        monitor: monitor.clone(),
    });

    // Two same-time events; frontier ordering controls bug.
    sim.schedule_now(key, Ev::A);
    sim.schedule_now(key, Ev::B);

    (sim, monitor)
}

fn hit_bad(status: &MonitorStatus) -> bool {
    status.score > 0.0
}

fn canonical_policy(policy: &des_explore::policy::TablePolicy) -> Vec<(DecisionKey, u64)> {
    let mut out: Vec<(DecisionKey, u64)> = policy
        .keys()
        .filter_map(|k| match policy.get(k) {
            Some(ActionDistribution::Deterministic { chosen_id }) => Some((k.clone(), *chosen_id)),
            _ => None,
        })
        .collect();

    out.sort_by(|(a, _), (b, _)| {
        a.time_nanos
            .cmp(&b.time_nanos)
            .then_with(|| (a.kind as u8).cmp(&(b.kind as u8)))
            .then_with(|| a.ordinal.cmp(&b.ordinal))
            .then_with(|| a.choice_set.cmp(&b.choice_set))
    });

    out
}

fn cmp_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or_else(|| {
        panic!("non-finite float comparison: a={a:?}, b={b:?}");
    })
}

fn assert_opt_f64_eq(a: Option<f64>, b: Option<f64>) {
    match (a, b) {
        (None, None) => {}
        (Some(x), Some(y)) => {
            assert_eq!(cmp_f64(x, y), Ordering::Equal);
        }
        (a, b) => panic!("option mismatch: left={a:?} right={b:?}"),
    }
}

#[test]
fn policy_search_is_deterministic_across_runs() {
    let cfg = PolicySearchConfig {
        base_sim_config: SimulationConfig { seed: 1 },
        scenario: "policy_search_frontier_determinism".to_string(),
        end_time: SimTime::from_millis(1),
        install_tokio: false,
        base_frontier: BaseFrontierPolicy::Fifo,
        base_tokio_ready: BaseTokioReadyPolicy::Fifo,
        budget_policies: 1,
        trials_per_policy: 1,
        holdout_trials: 2,
        confidence: 0.95,
        objective: PolicySearchObjective::PHat,
        max_failure_traces: 8,
        mutation_seed: 123,
    };

    let r1 = search_policy(cfg.clone(), setup_frontier_bug, hit_bad).unwrap();
    let r2 = search_policy(cfg, setup_frontier_bug, hit_bad).unwrap();

    assert_eq!(
        canonical_policy(&r1.best_policy),
        canonical_policy(&r2.best_policy)
    );

    assert_eq!(r1.evaluations.len(), r2.evaluations.len());
    for (a, b) in r1.evaluations.iter().zip(r2.evaluations.iter()) {
        assert_eq!(a.policy_index, b.policy_index);
        assert_eq!(a.successes, b.successes);
        assert_eq!(a.trials, b.trials);
        assert_eq!(cmp_f64(a.p_hat, b.p_hat), Ordering::Equal);
        assert_opt_f64_eq(a.wilson_lb, b.wilson_lb);
        assert_eq!(
            cmp_f64(a.objective_value, b.objective_value),
            Ordering::Equal
        );
    }

    assert_eq!(r1.holdout.trials, r2.holdout.trials);
    assert_eq!(r1.holdout.successes, r2.holdout.successes);
    assert_eq!(cmp_f64(r1.holdout.p_hat, r2.holdout.p_hat), Ordering::Equal);
    assert_opt_f64_eq(r1.holdout.ci_low, r2.holdout.ci_low);
    assert_opt_f64_eq(r1.holdout.ci_high, r2.holdout.ci_high);
}

#[test]
fn policy_search_holdout_uses_independent_seeds_from_search() {
    let cfg = PolicySearchConfig {
        base_sim_config: SimulationConfig { seed: 1 },
        scenario: "policy_search_holdout_seed".to_string(),
        end_time: SimTime::from_millis(1),
        install_tokio: false,
        base_frontier: BaseFrontierPolicy::Fifo,
        base_tokio_ready: BaseTokioReadyPolicy::Fifo,
        budget_policies: 0,
        trials_per_policy: 1,
        holdout_trials: 1,
        confidence: 0.95,
        objective: PolicySearchObjective::PHat,
        max_failure_traces: 8,
        mutation_seed: 123,
    };

    // Force every rollout to be treated as a failure so we capture traces in both phases.
    let report = search_policy(cfg, setup_frontier_bug, |_s| true).unwrap();

    let search_seed = report
        .failures
        .iter()
        .find(|f| matches!(f.phase, FailurePhase::Search))
        .map(|f| f.trace.meta.seed)
        .expect("expected at least one search-phase failure trace");

    let holdout_seed = report
        .failures
        .iter()
        .find(|f| matches!(f.phase, FailurePhase::Holdout))
        .map(|f| f.trace.meta.seed)
        .expect("expected at least one holdout-phase failure trace");

    assert_ne!(
        search_seed, holdout_seed,
        "expected holdout to use independent trial seed base"
    );

    assert_eq!(report.holdout.trials, 1);
    assert_eq!(report.holdout.successes, 1);
}

#[test]
fn policy_search_objective_value_reflects_config_selection() {
    let base = PolicySearchConfig {
        base_sim_config: SimulationConfig { seed: 1 },
        scenario: "policy_search_objective_selection".to_string(),
        end_time: SimTime::from_millis(1),
        install_tokio: false,
        base_frontier: BaseFrontierPolicy::UniformRandom,
        base_tokio_ready: BaseTokioReadyPolicy::Fifo,
        budget_policies: 0,
        trials_per_policy: 2,
        holdout_trials: 1,
        confidence: 0.95,
        objective: PolicySearchObjective::PHat,
        max_failure_traces: 0,
        mutation_seed: 123,
    };

    let p_hat_report = search_policy(base.clone(), setup_frontier_bug, hit_bad).unwrap();
    let wilson_report = search_policy(
        PolicySearchConfig {
            objective: PolicySearchObjective::WilsonLowerBound,
            ..base
        },
        setup_frontier_bug,
        hit_bad,
    )
    .unwrap();

    let p_hat_summary = p_hat_report
        .evaluations
        .first()
        .expect("expected baseline evaluation summary");
    let wilson_summary = wilson_report
        .evaluations
        .first()
        .expect("expected baseline evaluation summary");

    assert_eq!(p_hat_summary.successes, wilson_summary.successes);
    assert_eq!(p_hat_summary.trials, wilson_summary.trials);

    let expected_p_hat = p_hat_summary.successes as f64 / p_hat_summary.trials as f64;
    assert_eq!(
        cmp_f64(p_hat_summary.objective_value, expected_p_hat),
        Ordering::Equal
    );

    let expected_wilson = wilson_interval(wilson_summary.successes, wilson_summary.trials, 0.95)
        .map(|(l, _)| l)
        .unwrap_or(0.0);

    assert_eq!(
        cmp_f64(wilson_summary.objective_value, expected_wilson),
        Ordering::Equal
    );

    // PHat and Wilson should agree on success/trial counts but can differ in objective_value.
    assert_eq!(
        p_hat_summary.successes, wilson_summary.successes,
        "success counts should not depend on objective"
    );
}
