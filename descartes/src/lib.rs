//! # DesCartes - Discrete Event Simulation Framework
//!
//! DesCartes is a deterministic, replayable, discrete-event simulator for Rust.
//!
//! ## Quick Start
//!
//! ```toml
//! [dependencies]
//! descartes = "0.1"
//! ```
//!
//! ## Feature Flags
//!
//! - `default`: Includes `components` and `metrics`
//! - `full`: All features enabled
//! - `components`: High-level simulation components
//! - `metrics`: Metrics collection and analysis
//! - `viz`: Visualization support
//! - `tokio`: Tokio runtime integration
//! - `explore`: State space exploration
//! - `tonic`: gRPC support via Tonic
//! - `tower`: Tower middleware integration
//! - `axum`: Axum web framework integration
//!
//! ## Examples
//!
//! See the [repository](https://github.com/rupakm/DesCartes) for examples.

// Re-export core (always available)
pub use descartes_core as core;

pub use descartes_components as components;

pub use descartes_metrics as metrics;

#[cfg(feature = "viz")]
pub use descartes_viz as viz;

pub use descartes_tokio as tokio;

pub use descartes_explore as explore;

#[cfg(feature = "tonic")]
pub use descartes_tonic as tonic;

#[cfg(feature = "tower")]
pub use descartes_tower as tower;

#[cfg(feature = "axum")]
pub use descartes_axum as axum;

// Convenience re-exports of commonly used items from core
pub mod prelude {
    //! Commonly used types and traits

    pub use descartes_core::{
        Component, Execute, Executor, Key, SchedulerHandle, SimTime, Simulation, SimulationConfig,
        Task, TaskHandle,
    };

    pub use descartes_components::{queue::Queue, server::Server, simple_client::SimpleClient};

    pub use descartes_metrics::{RequestTracker, SimulationMetrics, SimulationRecorder};
}
