use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use descartes_core::{
    async_runtime::{FifoReadyTaskPolicy, ReadyTaskPolicy, UniformRandomReadyTaskPolicy},
    EventFrontierPolicy, FifoFrontierPolicy, SimTime, Simulation, SimulationConfig,
    UniformRandomFrontierPolicy,
};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;

use crate::estimate::BernoulliEstimate;
use crate::frontier::ExplorationFrontierPolicy;
use crate::harness::{HarnessContext, HarnessError};
use crate::monitor::{Monitor, MonitorStatus};
use crate::policy::{ActionDistribution, TablePolicy};
use crate::ready_task::ExplorationReadyTaskPolicy;
use crate::schedule_explore::{extract_decisions, DecisionKey, DecisionKind};
use crate::stats::wilson_interval;
use crate::trace::{Trace, TraceMeta, TraceRecorder};

#[derive(Debug, Clone, Copy)]
pub enum PolicySearchObjective {
    PHat,
    WilsonLowerBound,
}

#[derive(Debug, Clone)]
pub enum BaseFrontierPolicy {
    Fifo,
    UniformRandom,
}

#[derive(Debug, Clone)]
pub enum BaseTokioReadyPolicy {
    Fifo,
    UniformRandom,
}

#[derive(Debug, Clone)]
pub struct PolicySearchConfig {
    pub base_sim_config: SimulationConfig,
    pub scenario: String,

    pub end_time: SimTime,
    pub install_tokio: bool,

    /// Base policy used when a decision key is not in the table.
    pub base_frontier: BaseFrontierPolicy,

    /// Base tokio ready-task policy used when a decision key is not in the table.
    pub base_tokio_ready: BaseTokioReadyPolicy,

    /// Number of candidate mutations to evaluate.
    pub budget_policies: usize,

    /// Rollouts per candidate policy during search.
    pub trials_per_policy: u64,

    /// Independent rollouts for holdout evaluation of the best policy.
    pub holdout_trials: u64,

    /// Wilson confidence level (e.g., 0.95).
    pub confidence: f64,

    /// Objective used during search to decide whether to accept a mutation.
    pub objective: PolicySearchObjective,

    /// Maximum number of failure traces retained in the report.
    pub max_failure_traces: usize,

    /// Seed controlling mutation randomness.
    pub mutation_seed: u64,
}

impl Default for PolicySearchConfig {
    fn default() -> Self {
        Self {
            base_sim_config: SimulationConfig { seed: 1 },
            scenario: "policy_search".to_string(),
            end_time: SimTime::from_secs(60),
            install_tokio: true,
            base_frontier: BaseFrontierPolicy::Fifo,
            base_tokio_ready: BaseTokioReadyPolicy::Fifo,
            budget_policies: 100,
            trials_per_policy: 20,
            holdout_trials: 200,
            confidence: 0.95,
            objective: PolicySearchObjective::WilsonLowerBound,
            max_failure_traces: 32,
            mutation_seed: 0xDEC0DED,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolicyEvaluationSummary {
    pub policy_index: usize,
    pub successes: u64,
    pub trials: u64,
    pub p_hat: f64,
    pub wilson_lb: Option<f64>,
    pub objective_value: f64,
}

#[derive(Debug, Clone)]
pub enum FailurePhase {
    Search,
    Holdout,
}

#[derive(Debug, Clone)]
pub struct PolicySearchFailure {
    pub phase: FailurePhase,
    pub policy_index: usize,
    pub trial_index: u64,
    pub trace: Trace,
    pub status: Option<MonitorStatus>,
    pub panic_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PolicySearchReport {
    pub best_policy: TablePolicy,

    /// Summaries for the baseline evaluation (index 0) plus all candidate mutations.
    pub evaluations: Vec<PolicyEvaluationSummary>,

    pub holdout: BernoulliEstimate,

    pub failures: Vec<PolicySearchFailure>,

    /// Decision keys observed during search.
    pub observed_keys: HashSet<DecisionKey>,
}

#[derive(Debug, thiserror::Error)]
pub enum PolicySearchError {
    #[error(transparent)]
    Harness(#[from] HarnessError),

    #[error("trials_per_policy must be > 0")]
    ZeroTrialsPerPolicy,

    #[error("holdout_trials must be > 0")]
    ZeroHoldoutTrials,
}

fn derive_seed(base: u64, i: u64) -> u64 {
    // SplitMix64.
    let mut x = base.wrapping_add(i.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

fn run_until_end_or(
    sim: &mut Simulation,
    monitor: &Arc<Mutex<Monitor>>,
    end_time: SimTime,
    mut stop: impl FnMut(&MonitorStatus) -> bool,
) -> MonitorStatus {
    while sim.peek_next_event_time().is_some() && sim.time() < end_time {
        sim.step();
        let now = sim.time();
        let status = {
            let mut m = monitor.lock().unwrap();
            m.flush_up_to(now);
            m.status()
        };

        if stop(&status) {
            return status;
        }
    }

    let mut m = monitor.lock().unwrap();
    m.flush_up_to(end_time);
    m.status()
}

fn fallback_ready_policy(cfg: &BaseTokioReadyPolicy, seed: u64) -> Box<dyn ReadyTaskPolicy> {
    match cfg {
        BaseTokioReadyPolicy::Fifo => Box::new(FifoReadyTaskPolicy),
        BaseTokioReadyPolicy::UniformRandom => Box::new(UniformRandomReadyTaskPolicy::new(seed)),
    }
}

struct RolloutResult {
    hit_bad: bool,
    status: Option<MonitorStatus>,
    panic_message: Option<String>,
    trace: Trace,
    observed_keys: HashSet<DecisionKey>,
}

fn run_rollout(
    cfg: &PolicySearchConfig,
    policy: &TablePolicy,
    setup: impl Fn(
            SimulationConfig,
            &HarnessContext,
            Option<&Trace>,
            u64,
        ) -> (Simulation, Arc<Mutex<Monitor>>)
        + Copy
        + Send
        + 'static,
    predicate: impl Fn(&MonitorStatus) -> bool + Copy + Send + 'static,
    trial_seed_base: u64,
    trial_index: u64,
) -> Result<RolloutResult, HarnessError> {
    let cfg = cfg.clone();
    let policy = policy.clone();

    std::thread::spawn(move || {
        // Model randomness seed (SimulationConfig.seed and the continuation seed passed to setup).
        let model_seed = derive_seed(trial_seed_base, trial_index);

        // Scheduler/policy RNG seed(s) (part of the probability space).
        let policy_seed = derive_seed(trial_seed_base ^ 0xA0A0_A0A0_A0A0_A0A0, trial_index);
        let frontier_seed = derive_seed(policy_seed ^ 0xF00D_F00D_F00D_F00D, 0);
        let ready_seed = derive_seed(policy_seed ^ 0xBEEF_BEEF_BEEF_BEEF, 0);

        let sim_config = SimulationConfig { seed: model_seed };

        let recorder = Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
            seed: model_seed,
            scenario: cfg.scenario.clone(),
        })));
        let ctx = HarnessContext::new(recorder.clone());

        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let (mut sim, monitor) = setup(sim_config, &ctx, None, model_seed);

            let script = {
                let script = policy.to_decision_script();
                (!script.is_empty()).then(|| Arc::new(script))
            };

            if cfg.install_tokio {
                #[cfg(feature = "tokio")]
                {
                    let recorder_for_policies = ctx.recorder();
                    let script_for_tokio = script.clone();
                    let base_ready = fallback_ready_policy(&cfg.base_tokio_ready, ready_seed);

                    descartes_tokio::runtime::install_with(&mut sim, move |runtime| {
                        runtime.set_ready_task_policy(Box::new(ExplorationReadyTaskPolicy::new(
                            base_ready,
                            script_for_tokio,
                            recorder_for_policies,
                        )));
                    });
                }

                #[cfg(not(feature = "tokio"))]
                {
                    let _ = ready_seed;
                    let _ = frontier_seed;
                    let _ = policy_seed;
                    let _ = &monitor;
                    return Err(HarnessError::TokioFeatureDisabled);
                }
            }

            let frontier_policy: Box<dyn EventFrontierPolicy> = match cfg.base_frontier {
                BaseFrontierPolicy::Fifo => Box::new(ExplorationFrontierPolicy::new(
                    FifoFrontierPolicy,
                    script.clone(),
                    ctx.recorder(),
                )),
                BaseFrontierPolicy::UniformRandom => Box::new(ExplorationFrontierPolicy::new(
                    UniformRandomFrontierPolicy::new(frontier_seed),
                    script.clone(),
                    ctx.recorder(),
                )),
            };

            sim.set_frontier_policy(frontier_policy);

            let status = run_until_end_or(&mut sim, &monitor, cfg.end_time, |s| predicate(s));

            Ok::<_, HarnessError>(status)
        }));

        let trace = ctx.snapshot_trace();
        let observed_keys: HashSet<DecisionKey> = extract_decisions(&trace)
            .into_iter()
            .map(|d| d.key)
            .collect();

        match outcome {
            Ok(Ok(status)) => {
                let hit_bad = predicate(&status);
                Ok(RolloutResult {
                    hit_bad,
                    status: Some(status),
                    panic_message: None,
                    trace,
                    observed_keys,
                })
            }
            Ok(Err(e)) => Err(e),
            Err(payload) => Ok(RolloutResult {
                hit_bad: true,
                status: None,
                panic_message: Some(panic_message(payload)),
                trace,
                observed_keys,
            }),
        }
    })
    .join()
    .expect("rollout thread panicked")
}

fn objective_value(
    objective: PolicySearchObjective,
    successes: u64,
    trials: u64,
    confidence: f64,
) -> f64 {
    match objective {
        PolicySearchObjective::PHat => successes as f64 / trials as f64,
        PolicySearchObjective::WilsonLowerBound => wilson_interval(successes, trials, confidence)
            .map(|(lo, _)| lo)
            .unwrap_or(0.0),
    }
}

fn mutate_one(
    base: &TablePolicy,
    known_keys: &[DecisionKey],
    rng: &mut StdRng,
) -> Option<TablePolicy> {
    let mut multi: Vec<&DecisionKey> = known_keys
        .iter()
        .filter(|k| k.choice_set.len() > 1)
        .collect();

    // Prefer mutating the earliest (smallest-time) decision keys first. This makes
    // the v1 hillclimb much more likely to find schedule-sensitive bugs with small
    // budgets.
    multi.sort_by(|a, b| {
        a.time_nanos
            .cmp(&b.time_nanos)
            .then_with(|| (a.kind as u8).cmp(&(b.kind as u8)))
            .then_with(|| a.ordinal.cmp(&b.ordinal))
            .then_with(|| a.choice_set.len().cmp(&b.choice_set.len()))
            .then_with(|| a.choice_set.cmp(&b.choice_set))
    });

    let min_time = multi.first()?.time_nanos;
    let earliest: Vec<&DecisionKey> = multi
        .into_iter()
        .filter(|k| k.time_nanos == min_time)
        .collect();

    let (tokio_ready, other): (Vec<&DecisionKey>, Vec<&DecisionKey>) = earliest
        .into_iter()
        .partition(|k| k.kind == DecisionKind::TokioReady);

    // Heuristic: prefer mutating tokio-ready nondeterminism when present at the
    // same earliest time. This avoids spending small budgets exploring unrelated
    // frontier reorderings of identical `RuntimeEvent::Poll` events.
    let pool = if !tokio_ready.is_empty() {
        tokio_ready
    } else {
        other
    };

    let key = (*pool.choose(rng)?).clone();

    let current = match base.get(&key) {
        Some(ActionDistribution::Deterministic { chosen_id }) => *chosen_id,
        _ => *key.choice_set.first().unwrap(),
    };

    let other_ids: Vec<u64> = key
        .choice_set
        .iter()
        .copied()
        .filter(|&id| id != current)
        .collect();

    let chosen_id = *other_ids.choose(rng)?;

    let mut next = base.clone();
    next.set_deterministic(key, chosen_id);
    Some(next)
}

struct EvalResult {
    successes: u64,
    trials: u64,
    observed_keys: HashSet<DecisionKey>,
    failures: Vec<PolicySearchFailure>,
}

#[allow(clippy::too_many_arguments)]
fn eval_policy(
    cfg: &PolicySearchConfig,
    policy: &TablePolicy,
    setup: impl Fn(
            SimulationConfig,
            &HarnessContext,
            Option<&Trace>,
            u64,
        ) -> (Simulation, Arc<Mutex<Monitor>>)
        + Copy
        + Send
        + 'static,
    predicate: impl Fn(&MonitorStatus) -> bool + Copy + Send + 'static,
    phase: FailurePhase,
    policy_index: usize,
    trials: u64,
    trial_seed_base: u64,
    max_failure_traces: usize,
) -> Result<(EvalResult, Vec<PolicyEvaluationSummary>), PolicySearchError> {
    let mut successes = 0u64;
    let mut observed_keys = HashSet::new();
    let mut failures: Vec<PolicySearchFailure> = Vec::new();

    for trial_index in 0..trials {
        let rr = run_rollout(cfg, policy, setup, predicate, trial_seed_base, trial_index)?;

        observed_keys.extend(rr.observed_keys);

        if rr.hit_bad {
            successes += 1;
            if failures.len() < max_failure_traces {
                failures.push(PolicySearchFailure {
                    phase: phase.clone(),
                    policy_index,
                    trial_index,
                    trace: rr.trace,
                    status: rr.status,
                    panic_message: rr.panic_message,
                });
            }
        }
    }

    let p_hat = successes as f64 / trials as f64;
    let wilson = wilson_interval(successes, trials, cfg.confidence).map(|(lo, _)| lo);
    let obj = objective_value(cfg.objective, successes, trials, cfg.confidence);

    let summary = PolicyEvaluationSummary {
        policy_index,
        successes,
        trials,
        p_hat,
        wilson_lb: wilson,
        objective_value: obj,
    };

    Ok((
        EvalResult {
            successes,
            trials,
            observed_keys,
            failures,
        },
        vec![summary],
    ))
}

pub fn search_policy(
    cfg: PolicySearchConfig,
    setup: impl Fn(
            SimulationConfig,
            &HarnessContext,
            Option<&Trace>,
            u64,
        ) -> (Simulation, Arc<Mutex<Monitor>>)
        + Copy
        + Send
        + 'static,
    predicate: impl Fn(&MonitorStatus) -> bool + Copy + Send + 'static,
) -> Result<PolicySearchReport, PolicySearchError> {
    if cfg.trials_per_policy == 0 {
        return Err(PolicySearchError::ZeroTrialsPerPolicy);
    }
    if cfg.holdout_trials == 0 {
        return Err(PolicySearchError::ZeroHoldoutTrials);
    }

    let mut best_policy = TablePolicy::new();

    let mut evaluations: Vec<PolicyEvaluationSummary> = Vec::new();
    let mut failures: Vec<PolicySearchFailure> = Vec::new();
    let mut observed_keys: HashSet<DecisionKey> = HashSet::new();

    // Baseline evaluation (index 0).
    let (baseline_eval, baseline_summaries) = eval_policy(
        &cfg,
        &best_policy,
        setup,
        predicate,
        FailurePhase::Search,
        0,
        cfg.trials_per_policy,
        cfg.base_sim_config.seed,
        cfg.max_failure_traces.saturating_sub(failures.len()),
    )?;

    let mut best_obj = objective_value(
        cfg.objective,
        baseline_eval.successes,
        baseline_eval.trials,
        cfg.confidence,
    );

    observed_keys.extend(baseline_eval.observed_keys);
    failures.extend(baseline_eval.failures);
    evaluations.extend(baseline_summaries);

    let mut known_keys: Vec<DecisionKey> = observed_keys.iter().cloned().collect();

    let mut mutation_rng = StdRng::seed_from_u64(cfg.mutation_seed);

    for k in 0..cfg.budget_policies {
        let policy_index = k + 1;

        let Some(candidate) = mutate_one(&best_policy, &known_keys, &mut mutation_rng) else {
            break;
        };

        let (eval, summaries) = eval_policy(
            &cfg,
            &candidate,
            setup,
            predicate,
            FailurePhase::Search,
            policy_index,
            cfg.trials_per_policy,
            cfg.base_sim_config.seed,
            cfg.max_failure_traces.saturating_sub(failures.len()),
        )?;

        observed_keys.extend(eval.observed_keys);
        failures.extend(eval.failures);
        evaluations.extend(summaries);

        known_keys = observed_keys.iter().cloned().collect();

        let obj = objective_value(cfg.objective, eval.successes, eval.trials, cfg.confidence);
        if obj > best_obj {
            best_obj = obj;
            best_policy = candidate;
        }
    }

    // Holdout evaluation: independent seeds (avoid adaptive bias).
    let holdout_seed_base = cfg.base_sim_config.seed ^ 0xC0FF_EE00_C0FF_EE00;

    let (holdout_eval, _summaries) = eval_policy(
        &cfg,
        &best_policy,
        setup,
        predicate,
        FailurePhase::Holdout,
        evaluations.len(),
        cfg.holdout_trials,
        holdout_seed_base,
        cfg.max_failure_traces.saturating_sub(failures.len()),
    )?;

    failures.extend(holdout_eval.failures);

    let p_hat = holdout_eval.successes as f64 / holdout_eval.trials as f64;
    let ci = wilson_interval(holdout_eval.successes, holdout_eval.trials, cfg.confidence);

    let holdout = BernoulliEstimate {
        trials: holdout_eval.trials,
        successes: holdout_eval.successes,
        p_hat,
        ci_low: ci.map(|(l, _)| l),
        ci_high: ci.map(|(_, h)| h),
    };

    Ok(PolicySearchReport {
        best_policy,
        evaluations,
        holdout,
        failures,
        observed_keys,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wilson_objective_uses_lower_bound() {
        let successes = 1;
        let trials = 2;
        let confidence = 0.95;

        let obj = objective_value(
            PolicySearchObjective::WilsonLowerBound,
            successes,
            trials,
            confidence,
        );

        let expected = wilson_interval(successes, trials, confidence)
            .map(|(l, _)| l)
            .unwrap();

        assert!((obj - expected).abs() < 1e-12);
    }
}
