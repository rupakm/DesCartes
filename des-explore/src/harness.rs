use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use des_core::{Executor, SimTime, Simulation, SimulationConfig};

use crate::io::{write_trace_to_path, TraceFormat, TraceIoConfig, TraceIoError};
use crate::rng::TracingRandomProvider;
use crate::trace::{Trace, TraceMeta, TraceRecorder};

#[derive(Debug, Clone)]
pub struct HarnessConfig {
    pub sim_config: SimulationConfig,
    pub scenario: String,

    /// Whether to call `des_tokio::runtime::install(&mut sim)`.
    pub install_tokio: bool,

    /// Where to write the trace.
    pub trace_path: PathBuf,

    /// Trace encoding format.
    pub trace_format: TraceFormat,
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
