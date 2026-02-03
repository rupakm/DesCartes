//! Chart generation for simulation metrics
//!
//! This module provides chart builders for common visualization patterns
//! using the plotters library.

pub mod latency;
pub mod percentiles;
pub mod throughput;
pub mod time_series;
mod util;

use crate::error::VizError;
use descartes_metrics::simulation_metrics::MetricsSnapshot;
use std::path::Path;

/// Common chart configuration
#[derive(Debug, Clone)]
pub struct ChartConfig {
    /// Chart width in pixels
    pub width: u32,
    /// Chart height in pixels
    pub height: u32,
    /// Chart title
    pub title: String,
    /// X-axis label
    pub x_label: String,
    /// Y-axis label
    pub y_label: String,
}

impl Default for ChartConfig {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 768,
            title: String::new(),
            x_label: String::new(),
            y_label: String::new(),
        }
    }
}

impl ChartConfig {
    /// Create a new chart configuration with title
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Default::default()
        }
    }

    /// Set the chart dimensions
    pub fn dimensions(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Set the x-axis label
    pub fn x_label(mut self, label: impl Into<String>) -> Self {
        self.x_label = label.into();
        self
    }

    /// Set the y-axis label
    pub fn y_label(mut self, label: impl Into<String>) -> Self {
        self.y_label = label.into();
        self
    }
}

/// Generate all standard charts for a metrics snapshot
///
/// Creates a complete set of visualization charts:
/// - Latency statistics
/// - Throughput analysis
/// - Percentile comparisons
///
/// # Arguments
/// * `snapshot` - Metrics snapshot to visualize
/// * `output_dir` - Directory to save charts
///
/// # Example
/// ```no_run
/// use descartes_metrics::SimulationMetrics;
/// use descartes_viz::charts::generate_all_charts;
///
/// let metrics = SimulationMetrics::new();
/// let snapshot = metrics.get_metrics_snapshot();
/// generate_all_charts(&snapshot, "output/charts").unwrap();
/// ```
pub fn generate_all_charts(
    snapshot: &MetricsSnapshot,
    output_dir: impl AsRef<Path>,
) -> Result<(), VizError> {
    let output_dir = output_dir.as_ref();
    std::fs::create_dir_all(output_dir)?;

    // Generate latency chart if histogram data exists
    if !snapshot.histograms.is_empty() {
        let latency_path = output_dir.join("latency_stats.png");
        latency::create_latency_chart(snapshot, &latency_path)?;
    }

    // Generate throughput chart if we have counters
    if !snapshot.counters.is_empty() {
        let throughput_path = output_dir.join("throughput.png");
        throughput::create_throughput_chart(snapshot, &throughput_path)?;
    }

    // Generate percentile comparison chart if histograms exist
    if !snapshot.histograms.is_empty() {
        let percentiles_path = output_dir.join("percentiles.png");
        percentiles::create_percentile_chart(snapshot, &percentiles_path)?;
    }

    Ok(())
}
