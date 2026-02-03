use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use descartes_core::{
    EventFrontierPolicy, Executor, FifoFrontierPolicy, SimTime, Simulation, SimulationConfig,
    UniformRandomFrontierPolicy,
};

use crate::frontier::{
    ExplorationFrontierPolicy, RecordingFrontierPolicy, ReplayFrontierError, ReplayFrontierPolicy,
};
use crate::ready_task::{
    ExplorationReadyTaskPolicy, RecordingReadyTaskPolicy, ReplayReadyTaskError,
    ReplayReadyTaskPolicy,
};
use crate::schedule_explore::DecisionScript;

#[cfg(feature = "tokio")]
use crate::concurrency::{
    RecordingConcurrencyRecorder, ReplayConcurrencyError, ReplayConcurrencyValidator,
    TeeConcurrencyRecorder,
};

use crate::io::{write_trace_to_path, TraceFormat, TraceIoConfig, TraceIoError};
use crate::rng::TracingRandomProvider;
use crate::trace::{Trace, TraceMeta, TraceRecorder};

#[derive(Debug, Clone)]
pub struct HarnessConfig {
    pub sim_config: SimulationConfig,
    pub scenario: String,

    /// Whether to call `descartes_tokio::runtime::install(&mut sim)`.
    pub install_tokio: bool,

    /// Optional async-runtime (tokio-level) ready-task policy configuration.
    ///
    /// When omitted, tokio uses deterministic FIFO polling order.
    pub tokio_ready: Option<HarnessTokioReadyConfig>,

    /// Optional tokio sync mutex waiter policy configuration.
    ///
    /// When omitted, tokio mutexes use deterministic FIFO waiter selection.
    pub tokio_mutex: Option<HarnessTokioMutexConfig>,

    /// Record concurrency primitive observations (mutex/atomic/etc.) into the trace.
    ///
    /// This is opt-in and does not affect execution ordering.
    pub record_concurrency: bool,

    /// Optional same-time frontier policy configuration.
    ///
    /// When omitted, the harness does not touch the simulation's frontier policy
    /// (leaving the default deterministic FIFO behavior intact).
    pub frontier: Option<HarnessFrontierConfig>,

    /// Where to write the trace.
    pub trace_path: PathBuf,

    /// Trace encoding format.
    pub trace_format: TraceFormat,
}

#[derive(Debug, Clone)]
pub enum HarnessFrontierPolicy {
    /// Deterministic FIFO (default in `des-core`).
    Fifo,

    /// Uniform random tie-breaker among same-time frontier entries.
    ///
    /// Deterministic given the provided seed.
    UniformRandom { seed: u64 },
}

#[derive(Debug, Clone)]
pub enum HarnessTokioReadyPolicy {
    Fifo,
    UniformRandom { seed: u64 },
}

#[derive(Debug, Clone)]
pub enum HarnessTokioMutexPolicy {
    Fifo,
    UniformRandom { seed: u64 },
}

#[derive(Debug, Clone)]
pub struct HarnessTokioMutexConfig {
    pub policy: HarnessTokioMutexPolicy,
}

impl Default for HarnessTokioMutexConfig {
    fn default() -> Self {
        Self {
            policy: HarnessTokioMutexPolicy::Fifo,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HarnessTokioReadyConfig {
    pub policy: HarnessTokioReadyPolicy,

    /// Record async-runtime ready-task decisions into the trace.
    pub record_decisions: bool,
}

impl Default for HarnessTokioReadyConfig {
    fn default() -> Self {
        Self {
            policy: HarnessTokioReadyPolicy::Fifo,
            record_decisions: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HarnessFrontierConfig {
    pub policy: HarnessFrontierPolicy,

    /// Record scheduler decisions into the trace.
    ///
    /// This is required if you want to replay randomized frontier choices.
    pub record_decisions: bool,
}

impl Default for HarnessFrontierConfig {
    fn default() -> Self {
        Self {
            policy: HarnessFrontierPolicy::Fifo,
            record_decisions: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("failed writing trace: {0}")]
    TraceIo(#[from] TraceIoError),

    #[error("tokio integration requested but `des-explore` was built without feature `tokio`")]
    TokioFeatureDisabled,

    #[error("tokio configuration requested but install_tokio=false")]
    TokioPolicyWithoutTokio,
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessReplayError {
    #[error(transparent)]
    Harness(#[from] HarnessError),

    #[error("frontier replay mismatch: {0}")]
    Frontier(#[from] ReplayFrontierError),

    #[error("tokio ready-task replay mismatch: {0}")]
    TokioReady(#[from] ReplayReadyTaskError),

    #[cfg(feature = "tokio")]
    #[error("concurrency replay mismatch: {0}")]
    Concurrency(#[from] ReplayConcurrencyError),

    #[error("`cfg.frontier` is not used by `run_replayed`; replay uses the input trace instead")]
    FrontierConfigNotAllowed,

    #[error(
        "`cfg.tokio_ready` is not used by `run_replayed`; replay uses the input trace instead"
    )]
    TokioReadyConfigNotAllowed,

    #[error("simulation panicked during replay: {message}")]
    Panic { message: String },
}

/// Optional "control plane" for v1 schedule exploration.
///
/// When `explore_frontier`/`explore_tokio_ready` are enabled, the harness installs
/// exploration wrappers that:
/// - consult the provided [`DecisionScript`] (if any)
/// - otherwise delegate to the configured base policy
/// - always record realized decisions into the output trace
#[derive(Debug, Clone, Default)]
pub struct HarnessControl {
    pub decision_script: Option<Arc<DecisionScript>>,

    pub explore_frontier: bool,
    pub explore_tokio_ready: bool,
}

/// Context passed to setup/run closures.
///
/// Contains the shared trace recorder, and helper constructors for trace-aware
/// providers.
#[derive(Clone)]
pub struct HarnessContext {
    recorder: Arc<Mutex<TraceRecorder>>,
}

impl HarnessContext {
    pub fn new(recorder: Arc<Mutex<TraceRecorder>>) -> Self {
        Self { recorder }
    }

    pub fn recorder(&self) -> Arc<Mutex<TraceRecorder>> {
        self.recorder.clone()
    }

    pub fn recording_frontier_policy<P: EventFrontierPolicy>(
        &self,
        inner: P,
    ) -> RecordingFrontierPolicy<P> {
        RecordingFrontierPolicy::new(inner, self.recorder.clone())
    }

    pub fn install_recording_frontier_policy<P: EventFrontierPolicy + 'static>(
        &self,
        sim: &mut Simulation,
        inner: P,
    ) {
        sim.set_frontier_policy(Box::new(self.recording_frontier_policy(inner)));
    }

    pub fn install_replay_frontier_policy(
        &self,
        sim: &mut Simulation,
        trace: &Trace,
    ) -> ReplayFrontierPolicy {
        let policy = ReplayFrontierPolicy::from_trace_events(&trace.events);
        sim.set_frontier_policy(Box::new(policy.clone()));
        policy
    }

    pub fn tracing_provider(&self, seed: u64) -> Box<dyn descartes_core::RandomProvider> {
        Box::new(TracingRandomProvider::new(seed, self.recorder.clone()))
    }

    /// Create a provider that replays `prefix` first, then falls back to fresh RNG.
    pub fn branching_provider(
        &self,
        prefix: Option<&Trace>,
        fallback_seed: u64,
    ) -> Box<dyn descartes_core::RandomProvider> {
        match prefix {
            Some(prefix) => Box::new(crate::replay_rng::ChainedRandomProvider::new(
                prefix,
                fallback_seed,
                self.recorder.clone(),
            )),
            None => self.tracing_provider(fallback_seed),
        }
    }

    /// Cloneable branching provider for multi-distribution setups.
    ///
    /// Use this when multiple distributions must share a single globally-ordered
    /// randomness stream (required for prefix replay to work reliably).
    pub fn shared_branching_provider(
        &self,
        prefix: Option<&Trace>,
        fallback_seed: u64,
    ) -> crate::shared_replay_rng::SharedChainedRandomProvider {
        crate::shared_replay_rng::SharedChainedRandomProvider::new(
            prefix,
            fallback_seed,
            self.recorder.clone(),
        )
    }

    pub fn snapshot_trace(&self) -> Trace {
        self.recorder.lock().unwrap().snapshot()
    }
}

/// Runs a simulation, recording a trace to disk, while optionally controlling
/// scheduler decisions via a [`DecisionScript`].
///
/// This is the v1 "run with control" primitive used by schedule exploration:
///
/// - If `control.explore_frontier` is enabled, install [`ExplorationFrontierPolicy`].
/// - If `control.explore_tokio_ready` is enabled, install [`ExplorationReadyTaskPolicy`].
/// - If `prefix` is provided, callers can replay RNG draws via
///   [`HarnessContext::branching_provider`] / [`HarnessContext::shared_branching_provider`].
///
/// The returned trace always contains the realized decisions for any enabled
/// exploration wrappers.
pub fn run_controlled<R>(
    cfg: HarnessConfig,
    control: HarnessControl,
    prefix: Option<&Trace>,
    setup: impl FnOnce(SimulationConfig, &HarnessContext, Option<&Trace>) -> Simulation,
    run: impl FnOnce(&mut Simulation, &HarnessContext) -> R,
) -> Result<(R, Trace), HarnessError> {
    let recorder = Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
        seed: cfg.sim_config.seed,
        scenario: cfg.scenario.clone(),
    })));

    let ctx = HarnessContext::new(recorder);

    let mut sim = setup(cfg.sim_config.clone(), &ctx, prefix);

    if (cfg.tokio_ready.is_some()
        || cfg.tokio_mutex.is_some()
        || cfg.record_concurrency
        || control.explore_tokio_ready)
        && !cfg.install_tokio
    {
        return Err(HarnessError::TokioPolicyWithoutTokio);
    }

    if cfg.install_tokio {
        #[cfg(feature = "tokio")]
        {
            let tokio_ready_cfg = cfg.tokio_ready.clone();
            let tokio_mutex_cfg = cfg.tokio_mutex.clone();
            let record_concurrency = cfg.record_concurrency;
            let explore_tokio_ready = control.explore_tokio_ready;
            let script = control.decision_script.clone();
            let recorder = ctx.recorder();

            let concurrency_recorder = record_concurrency.then(|| {
                Arc::new(RecordingConcurrencyRecorder::new(recorder.clone()))
                    as Arc<dyn descartes_tokio::concurrency::ConcurrencyRecorder>
            });

            let mutex_policy: Option<Box<dyn descartes_tokio::sync::mutex::MutexWaiterPolicy>> =
                tokio_mutex_cfg.map(|cfg| match cfg.policy {
                    HarnessTokioMutexPolicy::Fifo => {
                        Box::new(descartes_tokio::sync::mutex::FifoMutexWaiterPolicy)
                            as Box<dyn descartes_tokio::sync::mutex::MutexWaiterPolicy>
                    }
                    HarnessTokioMutexPolicy::UniformRandom { seed } => Box::new(
                        descartes_tokio::sync::mutex::UniformRandomMutexWaiterPolicy::new(seed),
                    ),
                });

            let tokio_install = descartes_tokio::runtime::TokioInstallConfig {
                mutex_policy,
                concurrency_recorder,
            };

            descartes_tokio::runtime::install_with_tokio(&mut sim, tokio_install, move |runtime| {
                use descartes_core::async_runtime::{
                    FifoReadyTaskPolicy, ReadyTaskPolicy, UniformRandomReadyTaskPolicy,
                };

                if explore_tokio_ready {
                    let base_policy: Box<dyn ReadyTaskPolicy> = match tokio_ready_cfg
                        .as_ref()
                        .map(|cfg| cfg.policy.clone())
                    {
                        None | Some(HarnessTokioReadyPolicy::Fifo) => Box::new(FifoReadyTaskPolicy),
                        Some(HarnessTokioReadyPolicy::UniformRandom { seed }) => {
                            Box::new(UniformRandomReadyTaskPolicy::new(seed))
                        }
                    };

                    runtime.set_ready_task_policy(Box::new(ExplorationReadyTaskPolicy::new(
                        base_policy,
                        script.clone(),
                        recorder.clone(),
                    )));
                    return;
                }

                if let Some(tokio_cfg) = tokio_ready_cfg {
                    let base_policy: Box<dyn ReadyTaskPolicy> = match tokio_cfg.policy {
                        HarnessTokioReadyPolicy::Fifo => Box::new(FifoReadyTaskPolicy),
                        HarnessTokioReadyPolicy::UniformRandom { seed } => {
                            Box::new(UniformRandomReadyTaskPolicy::new(seed))
                        }
                    };

                    if tokio_cfg.record_decisions {
                        runtime.set_ready_task_policy(Box::new(RecordingReadyTaskPolicy::new(
                            base_policy,
                            recorder.clone(),
                        )));
                    } else {
                        runtime.set_ready_task_policy(base_policy);
                    }
                }
            });
        }

        #[cfg(not(feature = "tokio"))]
        {
            return Err(HarnessError::TokioFeatureDisabled);
        }
    }

    if control.explore_frontier {
        let script = control.decision_script.clone();

        let policy: Box<dyn EventFrontierPolicy> =
            match cfg.frontier.clone().map(|c| c.policy) {
                None | Some(HarnessFrontierPolicy::Fifo) => Box::new(
                    ExplorationFrontierPolicy::new(FifoFrontierPolicy, script, ctx.recorder()),
                ),
                Some(HarnessFrontierPolicy::UniformRandom { seed }) => {
                    Box::new(ExplorationFrontierPolicy::new(
                        UniformRandomFrontierPolicy::new(seed),
                        script,
                        ctx.recorder(),
                    ))
                }
            };

        sim.set_frontier_policy(policy);
    } else if let Some(frontier_cfg) = cfg.frontier.clone() {
        let policy: Box<dyn EventFrontierPolicy> =
            match (frontier_cfg.policy, frontier_cfg.record_decisions) {
                (HarnessFrontierPolicy::Fifo, false) => Box::new(FifoFrontierPolicy),
                (HarnessFrontierPolicy::Fifo, true) => Box::new(RecordingFrontierPolicy::new(
                    FifoFrontierPolicy,
                    ctx.recorder(),
                )),
                (HarnessFrontierPolicy::UniformRandom { seed }, false) => {
                    Box::new(UniformRandomFrontierPolicy::new(seed))
                }
                (HarnessFrontierPolicy::UniformRandom { seed }, true) => {
                    Box::new(RecordingFrontierPolicy::new(
                        UniformRandomFrontierPolicy::new(seed),
                        ctx.recorder(),
                    ))
                }
            };

        sim.set_frontier_policy(policy);
    }

    let result = run(&mut sim, &ctx);

    let trace = ctx.snapshot_trace();
    write_trace_to_path(
        &cfg.trace_path,
        &trace,
        TraceIoConfig {
            format: cfg.trace_format,
        },
    )?;

    Ok((result, trace))
}

/// Convenience wrapper: run with `Executor::timed(end_time)`.
pub fn run_timed_controlled(
    cfg: HarnessConfig,
    control: HarnessControl,
    prefix: Option<&Trace>,
    end_time: SimTime,
    setup: impl FnOnce(SimulationConfig, &HarnessContext, Option<&Trace>) -> Simulation,
) -> Result<Trace, HarnessError> {
    let ((), trace) = run_controlled(cfg, control, prefix, setup, move |sim, _ctx| {
        sim.execute(Executor::timed(end_time));
    })?;

    Ok(trace)
}

/// Runs a simulation, recording a trace to disk.
///
/// This is the primary entry point used by exploration tooling. The harness is
/// opt-in and does not change `des-core` behavior by default.
///
/// The `setup` closure builds the simulation and is responsible for wiring any
/// trace-aware randomness providers into distributions/components.
///
/// The `run` closure drives the simulation (typically by calling
/// `sim.execute(Executor::timed(...))`).
pub fn run_recorded<R>(
    cfg: HarnessConfig,
    setup: impl FnOnce(SimulationConfig, &HarnessContext) -> Simulation,
    run: impl FnOnce(&mut Simulation, &HarnessContext) -> R,
) -> Result<(R, Trace), HarnessError> {
    let recorder = Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
        seed: cfg.sim_config.seed,
        scenario: cfg.scenario.clone(),
    })));

    let ctx = HarnessContext::new(recorder);

    let mut sim = setup(cfg.sim_config.clone(), &ctx);

    if (cfg.tokio_ready.is_some() || cfg.tokio_mutex.is_some() || cfg.record_concurrency)
        && !cfg.install_tokio
    {
        return Err(HarnessError::TokioPolicyWithoutTokio);
    }

    if cfg.install_tokio {
        #[cfg(feature = "tokio")]
        {
            let tokio_ready_cfg = cfg.tokio_ready.clone();
            let tokio_mutex_cfg = cfg.tokio_mutex.clone();
            let record_concurrency = cfg.record_concurrency;
            let recorder = ctx.recorder();

            let concurrency_recorder = record_concurrency.then(|| {
                Arc::new(RecordingConcurrencyRecorder::new(recorder.clone()))
                    as Arc<dyn descartes_tokio::concurrency::ConcurrencyRecorder>
            });

            let mutex_policy: Option<Box<dyn descartes_tokio::sync::mutex::MutexWaiterPolicy>> =
                tokio_mutex_cfg.map(|cfg| match cfg.policy {
                    HarnessTokioMutexPolicy::Fifo => {
                        Box::new(descartes_tokio::sync::mutex::FifoMutexWaiterPolicy)
                            as Box<dyn descartes_tokio::sync::mutex::MutexWaiterPolicy>
                    }
                    HarnessTokioMutexPolicy::UniformRandom { seed } => Box::new(
                        descartes_tokio::sync::mutex::UniformRandomMutexWaiterPolicy::new(seed),
                    ),
                });

            let tokio_install = descartes_tokio::runtime::TokioInstallConfig {
                mutex_policy,
                concurrency_recorder,
            };

            descartes_tokio::runtime::install_with_tokio(&mut sim, tokio_install, move |runtime| {
                if let Some(tokio_cfg) = tokio_ready_cfg {
                    use descartes_core::async_runtime::{
                        FifoReadyTaskPolicy, ReadyTaskPolicy, UniformRandomReadyTaskPolicy,
                    };

                    let base_policy: Box<dyn ReadyTaskPolicy> = match tokio_cfg.policy {
                        HarnessTokioReadyPolicy::Fifo => Box::new(FifoReadyTaskPolicy),
                        HarnessTokioReadyPolicy::UniformRandom { seed } => {
                            Box::new(UniformRandomReadyTaskPolicy::new(seed))
                        }
                    };

                    if tokio_cfg.record_decisions {
                        runtime.set_ready_task_policy(Box::new(RecordingReadyTaskPolicy::new(
                            base_policy,
                            recorder.clone(),
                        )));
                    } else {
                        runtime.set_ready_task_policy(base_policy);
                    }
                }
            });
        }

        #[cfg(not(feature = "tokio"))]
        {
            return Err(HarnessError::TokioFeatureDisabled);
        }
    }

    if let Some(frontier_cfg) = cfg.frontier.clone() {
        let policy: Box<dyn EventFrontierPolicy> =
            match (frontier_cfg.policy, frontier_cfg.record_decisions) {
                (HarnessFrontierPolicy::Fifo, false) => Box::new(FifoFrontierPolicy),
                (HarnessFrontierPolicy::Fifo, true) => Box::new(RecordingFrontierPolicy::new(
                    FifoFrontierPolicy,
                    ctx.recorder(),
                )),
                (HarnessFrontierPolicy::UniformRandom { seed }, false) => {
                    Box::new(UniformRandomFrontierPolicy::new(seed))
                }
                (HarnessFrontierPolicy::UniformRandom { seed }, true) => {
                    Box::new(RecordingFrontierPolicy::new(
                        UniformRandomFrontierPolicy::new(seed),
                        ctx.recorder(),
                    ))
                }
            };

        sim.set_frontier_policy(policy);
    }

    let result = run(&mut sim, &ctx);

    let trace = ctx.snapshot_trace();
    write_trace_to_path(
        &cfg.trace_path,
        &trace,
        TraceIoConfig {
            format: cfg.trace_format,
        },
    )?;

    Ok((result, trace))
}

/// Convenience wrapper: run with `Executor::timed(end_time)`.
pub fn run_timed_recorded(
    cfg: HarnessConfig,
    end_time: SimTime,
    setup: impl FnOnce(SimulationConfig, &HarnessContext) -> Simulation,
) -> Result<Trace, HarnessError> {
    let ((), trace) = run_recorded(cfg, setup, move |sim, _ctx| {
        sim.execute(Executor::timed(end_time));
    })?;

    Ok(trace)
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

/// Runs a simulation, replaying scheduler decisions from an input trace.
///
/// This is an opt-in convenience API that installs a [`ReplayFrontierPolicy`] and
/// returns a structured error when the run diverges.
///
/// The `setup` closure receives the input trace so it can also wire RNG replay via
/// [`HarnessContext::branching_provider`] / [`HarnessContext::shared_branching_provider`].
pub fn run_replayed<R>(
    cfg: HarnessConfig,
    input_trace: &Trace,
    setup: impl FnOnce(SimulationConfig, &HarnessContext, &Trace) -> Simulation,
    run: impl FnOnce(&mut Simulation, &HarnessContext) -> R,
) -> Result<(R, Trace), HarnessReplayError> {
    if cfg.frontier.is_some() {
        return Err(HarnessReplayError::FrontierConfigNotAllowed);
    }
    if cfg.tokio_ready.is_some() {
        return Err(HarnessReplayError::TokioReadyConfigNotAllowed);
    }

    let recorder = Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
        seed: cfg.sim_config.seed,
        scenario: cfg.scenario.clone(),
    })));

    let ctx = HarnessContext::new(recorder);

    let mut sim = setup(cfg.sim_config.clone(), &ctx, input_trace);

    let mut replay_tokio_ready: Option<ReplayReadyTaskPolicy> = None;

    #[cfg(feature = "tokio")]
    let mut replay_concurrency: Option<ReplayConcurrencyValidator> = None;

    if cfg.install_tokio {
        #[cfg(feature = "tokio")]
        {
            let ready_policy = ReplayReadyTaskPolicy::from_trace_events(&input_trace.events);
            let ready_policy_for_runtime = ready_policy.clone();

            let concurrency_validator =
                ReplayConcurrencyValidator::from_trace_events(&input_trace.events);
            let has_concurrency_events = concurrency_validator.has_expected_events();

            if !has_concurrency_events {
                let msg =
                    "Replay trace contains no concurrency events; skipping concurrency validation";
                tracing::warn!("{msg}");
                eprintln!("Warning: {msg}");
            }

            let record_out = cfg.record_concurrency.then(|| {
                Arc::new(RecordingConcurrencyRecorder::new(ctx.recorder()))
                    as Arc<dyn descartes_tokio::concurrency::ConcurrencyRecorder>
            });

            let concurrency_recorder: Option<Arc<dyn descartes_tokio::concurrency::ConcurrencyRecorder>> =
                match (has_concurrency_events, record_out) {
                    (false, None) => None,
                    (false, Some(r)) => Some(r),
                    (true, None) => Some(Arc::new(concurrency_validator.clone())),
                    (true, Some(r)) => Some(Arc::new(TeeConcurrencyRecorder::new(vec![
                        Arc::new(concurrency_validator.clone()),
                        r,
                    ]))),
                };

            let mutex_policy: Option<Box<dyn descartes_tokio::sync::mutex::MutexWaiterPolicy>> =
                cfg.tokio_mutex.clone().map(|cfg| match cfg.policy {
                    HarnessTokioMutexPolicy::Fifo => {
                        Box::new(descartes_tokio::sync::mutex::FifoMutexWaiterPolicy)
                            as Box<dyn descartes_tokio::sync::mutex::MutexWaiterPolicy>
                    }
                    HarnessTokioMutexPolicy::UniformRandom { seed } => Box::new(
                        descartes_tokio::sync::mutex::UniformRandomMutexWaiterPolicy::new(seed),
                    ),
                });

            let tokio_install = descartes_tokio::runtime::TokioInstallConfig {
                mutex_policy,
                concurrency_recorder,
            };

            descartes_tokio::runtime::install_with_tokio(&mut sim, tokio_install, move |runtime| {
                runtime.set_ready_task_policy(Box::new(ready_policy_for_runtime));
            });

            replay_tokio_ready = Some(ready_policy);
            replay_concurrency = Some(concurrency_validator);
        }

        #[cfg(not(feature = "tokio"))]
        {
            return Err(HarnessError::TokioFeatureDisabled.into());
        }
    }

    let has_frontier_decisions = input_trace
        .events
        .iter()
        .any(|e| matches!(e, crate::trace::TraceEvent::SchedulerDecision(_)));

    let replay_frontier =
        has_frontier_decisions.then(|| ctx.install_replay_frontier_policy(&mut sim, input_trace));

    if !has_frontier_decisions {
        let msg =
            "Replay trace contains no DES scheduler decisions; leaving default FIFO event ordering";
        tracing::warn!("{msg}");
        eprintln!("Warning: {msg}");
    }

    let result = match catch_unwind(AssertUnwindSafe(|| run(&mut sim, &ctx))) {
        Ok(v) => v,
        Err(payload) => {
            if let Some(replay_tokio_ready) = replay_tokio_ready.as_ref() {
                if let Some(err) = replay_tokio_ready.error() {
                    return Err(err.into());
                }
            }

            if let Some(replay_frontier) = replay_frontier.as_ref() {
                if let Some(err) = replay_frontier.error() {
                    return Err(err.into());
                }
            }

            #[cfg(feature = "tokio")]
            if let Some(replay_concurrency) = replay_concurrency.as_ref() {
                if let Some(err) = replay_concurrency.error() {
                    return Err(err.into());
                }
            }

            return Err(HarnessReplayError::Panic {
                message: panic_message(payload),
            });
        }
    };

    if let Some(replay_tokio_ready) = replay_tokio_ready.as_ref() {
        if let Some(err) = replay_tokio_ready.error() {
            return Err(err.into());
        }
    }

    if let Some(replay_frontier) = replay_frontier.as_ref() {
        if let Some(err) = replay_frontier.error() {
            return Err(err.into());
        }
    }

    #[cfg(feature = "tokio")]
    if let Some(replay_concurrency) = replay_concurrency.as_ref() {
        if let Some(err) = replay_concurrency.error() {
            return Err(err.into());
        }
    }

    let trace = ctx.snapshot_trace();
    write_trace_to_path(
        &cfg.trace_path,
        &trace,
        TraceIoConfig {
            format: cfg.trace_format,
        },
    )
    .map_err(HarnessError::from)?;

    Ok((result, trace))
}

/// Convenience wrapper: replay with `Executor::timed(end_time)`.
pub fn run_timed_replayed(
    cfg: HarnessConfig,
    end_time: SimTime,
    input_trace: &Trace,
    setup: impl FnOnce(SimulationConfig, &HarnessContext, &Trace) -> Simulation,
) -> Result<Trace, HarnessReplayError> {
    let ((), trace) = run_replayed(cfg, input_trace, setup, move |sim, _ctx| {
        sim.execute(Executor::timed(end_time));
    })?;

    Ok(trace)
}

/// Heuristic helper: choose format from file extension.
pub fn format_from_extension(path: impl AsRef<Path>) -> TraceFormat {
    match path.as_ref().extension().and_then(|s| s.to_str()) {
        Some("json") => TraceFormat::Json,
        Some("bin") | Some("postcard") => TraceFormat::Postcard,
        _ => TraceFormat::Json,
    }
}
