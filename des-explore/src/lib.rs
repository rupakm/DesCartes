//! Systematic exploration tools for DES simulations.
//!
//! This crate is intentionally opt-in: it adds debugging/exploration utilities
//! without changing the default behavior of `des-core`.

pub mod estimate;
pub mod harness;
pub mod io;
pub mod monitor;
pub mod replay_rng;
pub mod rng;
pub mod shared_replay_rng;
pub mod shared_rng;
pub mod splitting;
pub mod stats;
pub mod trace;

/// Prelude for common exploration types.
pub mod prelude {
    pub use crate::estimate::{
        estimate_monte_carlo, estimate_with_splitting, BernoulliEstimate, EstimateError,
        FoundCounterexample, MonteCarloConfig, SplittingEstimate, SplittingEstimateConfig,
    };
    pub use crate::harness::{
        format_from_extension, run_recorded, run_timed_recorded, HarnessConfig, HarnessContext,
        HarnessError,
    };
    pub use crate::io::{
        read_trace_from_path, write_trace_to_path, TraceFormat, TraceIoConfig, TraceIoError,
    };
    pub use crate::monitor::{
        Monitor, MonitorConfig, MonitorStatus, QueueId, ScoreWeights, WindowSummary,
    };
    pub use crate::replay_rng::{ChainedRandomProvider, ReplayRandomProvider};
    pub use crate::rng::TracingRandomProvider;
    pub use crate::shared_replay_rng::SharedChainedRandomProvider;
    pub use crate::shared_rng::SharedTracingRandomProvider;
    pub use crate::splitting::{find_with_splitting, FoundBug, SplittingConfig, SplittingError};
    pub use crate::stats::{batch_means, wilson_interval, BatchMeansEstimate};
    pub use crate::trace::{Trace, TraceRecorder};
}
