//! Export functionality for simulation metrics
//!
//! This module provides exporters for different data formats to enable
//! visualization and analysis with external tools.

pub mod csv;
pub mod json;

use crate::error::MetricsError;
use crate::simulation_metrics::SimulationMetrics;
use std::path::Path;

/// Trait for exporting metrics to different formats
pub trait MetricsExporter {
    /// Export metrics to the configured destination
    fn export(&self, metrics: &SimulationMetrics) -> Result<(), MetricsError>;
}

/// Export metrics to JSON format
///
/// # Example
/// ```no_run
/// use descartes_metrics::SimulationMetrics;
/// use descartes_metrics::export::export_json;
///
/// let metrics = SimulationMetrics::new();
/// // ... collect metrics ...
/// export_json(&metrics, "results/metrics.json", true).unwrap();
/// ```
pub fn export_json(
    metrics: &SimulationMetrics,
    path: impl AsRef<Path>,
    pretty: bool,
) -> Result<(), MetricsError> {
    let exporter = json::JsonExporter::new(path.as_ref(), pretty);
    exporter.export(metrics)
}

/// Export metrics to CSV format
///
/// # Example
/// ```no_run
/// use descartes_metrics::SimulationMetrics;
/// use descartes_metrics::export::export_csv;
///
/// let metrics = SimulationMetrics::new();
/// // ... collect metrics ...
/// export_csv(&metrics, "results/metrics.csv", true).unwrap();
/// ```
pub fn export_csv(
    metrics: &SimulationMetrics,
    path: impl AsRef<Path>,
    include_labels: bool,
) -> Result<(), MetricsError> {
    let exporter = csv::CsvExporter::new(path.as_ref(), include_labels);
    exporter.export(metrics)
}
