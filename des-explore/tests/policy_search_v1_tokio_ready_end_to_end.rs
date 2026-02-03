use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use descartes_core::{Component, Key, SimTime, Simulation, SimulationConfig};

use descartes_explore::monitor::{Monitor, MonitorConfig, MonitorStatus, ScoreWeights};
use descartes_explore::policy_search::{
    search_policy, BaseFrontierPolicy, BaseTokioReadyPolicy, PolicySearchConfig,
    PolicySearchObjective,
};
use descartes_explore::trace::Trace;

#[derive(Debug)]
enum Ev {
    Start,
}

/// A minimal schedule-sensitive bug:
///
/// - `init` sets a shared state to 1.
/// - `check` observes state==0 and records a `drop` ("Bad").
///
/// Which task is polled first at time 0 is controlled by the tokio ready-task policy.
struct TokioReadyOrderBug {
    monitor: Arc<Mutex<Monitor>>,
}

impl Component for TokioReadyOrderBug {
    type Event = Ev;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        _scheduler: &mut descartes_core::Scheduler,
    ) {
        match event {
            Ev::Start => {
                let state = Arc::new(AtomicUsize::new(0));

                // Spawn order matters: baseline FIFO polls init first.
                let state_init = state.clone();
                descartes_tokio::task::spawn(async move {
                    state_init.store(1, Ordering::SeqCst);
                });

                let state_check = state.clone();
                let monitor = self.monitor.clone();
                descartes_tokio::task::spawn(async move {
                    let now =
                        descartes_core::async_runtime::current_sim_time().unwrap_or_else(SimTime::zero);

                    if state_check.load(Ordering::SeqCst) == 0 {
                        monitor.lock().unwrap().observe_drop(now);
                    }

                    // Ensure the monitor window flushes (advance simulated time).
                    descartes_tokio::time::sleep(Duration::from_millis(1)).await;
                });
            }
        }
    }
}

fn setup(
    config: SimulationConfig,
    _ctx: &descartes_explore::harness::HarnessContext,
    _prefix: Option<&Trace>,
    _cont_seed: u64,
) -> (Simulation, Arc<Mutex<Monitor>>) {
    let mut sim = Simulation::new(config);

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

    let monitor = Arc::new(Mutex::new(Monitor::new(mcfg, SimTime::zero())));

    let key = sim.add_component(TokioReadyOrderBug {
        monitor: monitor.clone(),
    });
    sim.schedule_now(key, Ev::Start);

    (sim, monitor)
}

fn hit_bad(status: &MonitorStatus) -> bool {
    status.score > 0.0
}

#[test]
fn policy_search_v1_tokio_ready_finds_counterexample_or_beats_fifo() {
    let cfg = PolicySearchConfig {
        base_sim_config: SimulationConfig { seed: 1 },
        scenario: "policy_search_tokio_ready_order".to_string(),
        end_time: SimTime::from_millis(1),
        install_tokio: true,
        base_frontier: BaseFrontierPolicy::Fifo,
        base_tokio_ready: BaseTokioReadyPolicy::Fifo,
        budget_policies: 4,
        trials_per_policy: 1,
        holdout_trials: 5,
        confidence: 0.95,
        objective: PolicySearchObjective::PHat,
        max_failure_traces: 8,
        mutation_seed: 0xC0FFEE,
    };

    let report = search_policy(cfg, setup, hit_bad).expect("policy search should succeed");

    let baseline_p_hat = report.evaluations.first().map(|s| s.p_hat).unwrap_or(0.0);

    assert!(
        !report.failures.is_empty() || report.holdout.p_hat > baseline_p_hat,
        "expected at least one counterexample or p_hat improvement (baseline_p_hat={}, holdout_p_hat={})",
        baseline_p_hat,
        report.holdout.p_hat
    );
}
