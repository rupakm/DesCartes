use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use des_core::{
    EventFrontierPolicy, Executor, FifoFrontierPolicy, SimTime, Simulation, SimulationConfig,
    UniformRandomFrontierPolicy,
};

use crate::frontier::{RecordingFrontierPolicy, ReplayFrontierPolicy};

use crate::io::{write_trace_to_path, TraceFormat, TraceIoConfig, TraceIoError};
use crate::rng::TracingRandomProvider;
use crate::trace::{Trace, TraceMeta, TraceRecorder};

#[derive(Debug, Clone)]
pub struct HarnessConfig {
    pub sim_config: SimulationConfig,
    pub scenario: String,

    /// Whether to call `des_tokio::runtime::install(&mut sim)`.
    pub install_tokio: bool,

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

    pub fn tracing_provider(&self, seed: u64) -> Box<dyn des_core::RandomProvider> {
        Box::new(TracingRandomProvider::new(seed, self.recorder.clone()))
    }

    /// Create a provider that replays `prefix` first, then falls back to fresh RNG.
    pub fn branching_provider(
        &self,
        prefix: Option<&Trace>,
        fallback_seed: u64,
    ) -> Box<dyn des_core::RandomProvider> {
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

    if cfg.install_tokio {
        #[cfg(feature = "tokio")]
        {
            des_tokio::runtime::install(&mut sim);
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

/// Heuristic helper: choose format from file extension.
pub fn format_from_extension(path: impl AsRef<Path>) -> TraceFormat {
    match path.as_ref().extension().and_then(|s| s.to_str()) {
        Some("json") => TraceFormat::Json,
        Some("bin") | Some("postcard") => TraceFormat::Postcard,
        _ => TraceFormat::Json,
    }
}
