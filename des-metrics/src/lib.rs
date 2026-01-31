//! Metrics collection and observability for simulations
//!
//! This crate provides comprehensive metrics collection, statistical analysis,
//! and structured logging capabilities for simulation observability.
//!
//! The crate leverages the standard Rust metrics ecosystem while adding
//! simulation-specific functionality like request tracking and high-resolution
//! latency analysis.

pub mod error;
pub mod export;
pub mod mmk_time_series;
pub mod recorder;
pub mod request_tracker;
pub mod simulation_metrics;

// Re-export key types
pub use error::{LogError, MetricsError};
pub use mmk_time_series::{
    ExponentialMovingAverage, MmkTimeSeriesMetrics, TimeSeriesCollector, TimeSeriesPoint,
};
pub use request_tracker::{
    LatencyStats as RequestLatencyStats, RequestTracker, RequestTrackerStats,
};
pub use recorder::{with_simulation_metrics_recorder, SimulationRecorder};
pub use simulation_metrics::{
    setup_prometheus_metrics, setup_prometheus_metrics_with_config, LatencyStats, SimulationMetrics,
};

// Re-export standard metrics for convenience
pub use metrics::{counter, gauge, histogram};
