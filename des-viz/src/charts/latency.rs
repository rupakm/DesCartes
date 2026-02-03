//! Latency visualization charts

use crate::charts::util::{metric_label, truncate_label};
use crate::charts::ChartConfig;
use crate::error::VizError;
use descartes_metrics::simulation_metrics::MetricsSnapshot;
use plotters::prelude::*;
use std::path::Path;

/// Create a latency statistics chart
///
/// Generates a grouped bar chart showing latency percentiles (mean, p50, p95, p99)
/// for all histogram metrics in the snapshot.
///
/// # Arguments
/// * `snapshot` - Metrics snapshot containing histogram data
/// * `output_path` - Output file path (PNG, SVG, or other supported formats)
///
/// # Example
/// ```no_run
/// use descartes_metrics::SimulationMetrics;
/// use descartes_viz::charts::latency::create_latency_chart;
///
/// let metrics = SimulationMetrics::new();
/// let snapshot = metrics.get_metrics_snapshot();
/// create_latency_chart(&snapshot, "latency.png").unwrap();
/// ```
pub fn create_latency_chart(
    snapshot: &MetricsSnapshot,
    output_path: impl AsRef<Path>,
) -> Result<(), VizError> {
    let config = ChartConfig::new("Latency Statistics")
        .x_label("Metric")
        .y_label("Latency (ms)");

    create_latency_chart_with_config(snapshot, output_path, config)
}

/// Create a latency chart with custom configuration
pub fn create_latency_chart_with_config(
    snapshot: &MetricsSnapshot,
    output_path: impl AsRef<Path>,
    config: ChartConfig,
) -> Result<(), VizError> {
    if snapshot.histograms.is_empty() {
        return Err(VizError::InvalidConfiguration(
            "No histogram data available for latency chart".to_string(),
        ));
    }

    let output_path = output_path.as_ref();
    let root = BitMapBackend::new(output_path, (config.width, config.height)).into_drawing_area();

    root.fill(&WHITE)
        .map_err(|e| VizError::RenderingError(format!("Failed to fill background: {e}")))?;

    // Extract latency data
    let mut chart_data: Vec<(String, f64, f64, f64, f64)> = Vec::new();
    for (name, labels, stats) in &snapshot.histograms {
        let label = metric_label(name, labels);
        chart_data.push((label, stats.mean, stats.median, stats.p95, stats.p99));
    }

    // Calculate max value for y-axis
    let max_latency = chart_data
        .iter()
        .map(|(_, _, _, _, p99)| *p99)
        .fold(0.0f64, f64::max)
        * 1.1; // Add 10% padding

    // Build chart
    let mut chart = ChartBuilder::on(&root)
        .caption(&config.title, ("sans-serif", 40).into_font())
        .margin(10)
        .x_label_area_size(100)
        .y_label_area_size(60)
        .build_cartesian_2d(0..chart_data.len(), 0.0..max_latency)
        .map_err(|e| VizError::RenderingError(format!("Failed to build chart: {e}")))?;

    chart
        .configure_mesh()
        .x_desc(&config.x_label)
        .y_desc(&config.y_label)
        .x_labels(chart_data.len())
        .x_label_formatter(&|x| {
            chart_data
                .get(*x)
                .map(|(name, _, _, _, _)| truncate_label(name, 15))
                .unwrap_or_default()
        })
        .draw()
        .map_err(|e| VizError::RenderingError(format!("Failed to configure mesh: {e}")))?;

    // Draw bars for each percentile using simplified approach
    let colors = [BLUE, GREEN, YELLOW, RED];
    let labels = ["Mean", "Median", "P95", "P99"];

    for (idx, (_name, mean, median, p95, p99)) in chart_data.iter().enumerate() {
        let values = [*mean, *median, *p95, *p99];

        for (value, color) in values.iter().zip(colors.iter()) {
            // Use a simple filled circle/bar representation
            chart
                .draw_series(std::iter::once(Circle::new(
                    (idx, *value),
                    8,
                    color.filled(),
                )))
                .map_err(|e| VizError::RenderingError(format!("Failed to draw bar: {e}")))?;
        }
    }

    // Draw legend
    for (color, label) in colors.iter().zip(labels.iter()) {
        let color_val = *color;
        chart
            .draw_series(std::iter::once(Circle::new(
                (0, 0.0),
                0,
                color_val.filled(),
            )))
            .map_err(|e| VizError::RenderingError(format!("Failed to draw legend: {e}")))?
            .label(*label)
            .legend(move |(x, y)| Circle::new((x + 5, y), 5, color_val.filled()));
    }

    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()
        .map_err(|e| VizError::RenderingError(format!("Failed to draw legend: {e}")))?;

    root.present()
        .map_err(|e| VizError::ExportFailed(format!("Failed to save chart: {e}")))?;

    Ok(())
}

// formatting helpers live in `crate::charts::util`

#[cfg(test)]
mod tests {
    use super::*;
    use descartes_metrics::simulation_metrics::SimulationMetrics;

    #[test]
    fn test_latency_chart_generation() {
        let mut metrics = SimulationMetrics::new();

        // Add histogram data
        for i in 1..=100 {
            metrics.record_histogram("response_time", i as f64, &[("component", "server1")]);
        }

        let snapshot = metrics.get_metrics_snapshot();
        let output_path = std::env::temp_dir().join("test_latency.png");

        let result = create_latency_chart(&snapshot, &output_path);
        assert!(result.is_ok());
        assert!(output_path.exists());

        // Cleanup
        std::fs::remove_file(output_path).ok();
    }
}
