//! Systematic exploration tools for DES simulations.
//!
//! This crate is intentionally opt-in: it adds debugging/exploration utilities
//! without changing the default behavior of `des-core`.

pub mod concurrency;
pub mod estimate;
pub mod frontier;
pub mod harness;
pub mod io;
pub mod monitor;
pub mod ready_task;
pub mod replay_rng;
pub mod rng;
pub mod shared_replay_rng;
pub mod shared_rng;
pub mod splitting;
pub mod stats;
pub mod trace;

/// Prelude for common exploration types.
pub mod prelude {
    #[cfg(feature = "tokio")]
    pub use crate::concurrency::{
        RecordingConcurrencyRecorder, ReplayConcurrencyError, ReplayConcurrencyValidator,
    };
    pub use crate::estimate::{
        estimate_monte_carlo, estimate_with_splitting, BernoulliEstimate, EstimateError,
        FoundCounterexample, MonteCarloConfig, SplittingEstimate, SplittingEstimateConfig,
    };
    pub use crate::frontier::{RecordingFrontierPolicy, ReplayFrontierError, ReplayFrontierPolicy};
    pub use crate::harness::{
        format_from_extension, run_recorded, run_replayed, run_timed_recorded, run_timed_replayed,
        HarnessConfig, HarnessContext, HarnessError, HarnessFrontierConfig, HarnessFrontierPolicy,
        HarnessReplayError, HarnessTokioMutexConfig, HarnessTokioMutexPolicy,
        HarnessTokioReadyConfig, HarnessTokioReadyPolicy,
    };
    pub use crate::io::{
        read_trace_from_path, write_trace_to_path, TraceFormat, TraceIoConfig, TraceIoError,
    };
    pub use crate::monitor::{
        Monitor, MonitorConfig, MonitorStatus, QueueId, ScoreWeights, WindowSummary,
    };
    pub use crate::ready_task::{
        RecordingReadyTaskPolicy, ReplayReadyTaskError, ReplayReadyTaskPolicy,
    };
    pub use crate::replay_rng::{ChainedRandomProvider, ReplayRandomProvider};
    pub use crate::rng::TracingRandomProvider;
    pub use crate::shared_replay_rng::SharedChainedRandomProvider;
    pub use crate::shared_rng::SharedTracingRandomProvider;
    pub use crate::splitting::{find_with_splitting, FoundBug, SplittingConfig, SplittingError};
    pub use crate::stats::{batch_means, wilson_interval, BatchMeansEstimate};
    pub use crate::trace::{Trace, TraceRecorder};
}
