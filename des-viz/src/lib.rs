//! Visualization library for simulation metrics
//!
//! This crate provides lightweight plotting and visualization capabilities
//! for analyzing simulation results using the plotters library.
//!
//! # Features
//!
//! - **Static Charts**: Generate PNG/SVG charts for latency, throughput, and percentiles
//! - **HTML Reports**: Complete HTML reports with embedded charts and metrics tables
//! - **Customizable**: Flexible chart configuration and styling
//!
//! # Example
//!
//! ```no_run
//! use des_metrics::SimulationMetrics;
//! use des_viz::report::generate_html_report;
//!
//! let metrics = SimulationMetrics::new();
//! // ... collect metrics ...
//!
//! let snapshot = metrics.get_metrics_snapshot();
//! generate_html_report(&snapshot, "report.html").unwrap();
//! ```

pub mod charts;
pub mod error;
pub mod report;

pub use error::VizError;
