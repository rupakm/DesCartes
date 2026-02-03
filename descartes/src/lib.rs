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
pub use des_core as core;

// Re-export optional crates
#[cfg(feature = "components")]
pub use des_components as components;

#[cfg(feature = "metrics")]
pub use des_metrics as metrics;

#[cfg(feature = "viz")]
pub use des_viz as viz;

#[cfg(feature = "tokio")]
pub use des_tokio as tokio;

#[cfg(feature = "explore")]
pub use des_explore as explore;

#[cfg(feature = "tonic")]
pub use des_tonic as tonic;

#[cfg(feature = "tower")]
pub use des_tower as tower;

#[cfg(feature = "axum")]
pub use des_axum as axum;

// Convenience re-exports of commonly used items from core
pub mod prelude {
    //! Commonly used types and traits
    
    pub use des_core::{
        Execute, Executor,
        SimTime,
        Simulation, SimulationConfig,
        Component, Key,
        Task, TaskHandle,
        SchedulerHandle,
    };
    
    #[cfg(feature = "components")]
    pub use des_components::{
        simple_client::SimpleClient,
        server::Server,
        queue::Queue,
    };
    
    #[cfg(feature = "metrics")]
    pub use des_metrics::{
        SimulationRecorder,
        RequestTracker,
        SimulationMetrics,
    };
}
