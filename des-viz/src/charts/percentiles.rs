//! Percentile comparison charts

use crate::charts::ChartConfig;
use crate::error::VizError;
use des_metrics::simulation_metrics::MetricsSnapshot;
use plotters::prelude::*;
use std::path::Path;

/// Create a percentile comparison chart
///
/// Generates a line chart comparing percentile distributions (p50, p95, p99)
/// across different histogram metrics.
///
/// # Arguments
/// * `snapshot` - Metrics snapshot containing histogram data
/// * `output_path` - Output file path (PNG, SVG, or other supported formats)
///
/// # Example
/// ```no_run
/// use des_metrics::SimulationMetrics;
/// use des_viz::charts::percentiles::create_percentile_chart;
///
/// let metrics = SimulationMetrics::new();
/// let snapshot = metrics.get_metrics_snapshot();
/// create_percentile_chart(&snapshot, "percentiles.png").unwrap();
/// ```
pub fn create_percentile_chart(
    snapshot: &MetricsSnapshot,
    output_path: impl AsRef<Path>,
) -> Result<(), VizError> {
    let config = ChartConfig::new("Latency Percentiles Comparison")
        .x_label("Percentile")
        .y_label("Latency (ms)");

    create_percentile_chart_with_config(snapshot, output_path, config)
}

/// Create a percentile chart with custom configuration
pub fn create_percentile_chart_with_config(
    snapshot: &MetricsSnapshot,
    output_path: impl AsRef<Path>,
    config: ChartConfig,
) -> Result<(), VizError> {
    if snapshot.histograms.is_empty() {
        return Err(VizError::InvalidConfiguration(
            "No histogram data available for percentile chart".to_string(),
        ));
    }

    let output_path = output_path.as_ref();
    let root = BitMapBackend::new(output_path, (config.width, config.height)).into_drawing_area();

    root.fill(&WHITE)
        .map_err(|e| VizError::RenderingError(format!("Failed to fill background: {e}")))?;

    // Extract percentile data
    let percentile_labels = ["P50", "P95", "P99"];

    let mut series_data: Vec<(String, Vec<(usize, f64)>)> = Vec::new();

    for (name, labels, stats) in &snapshot.histograms {
        let label = if labels.is_empty() {
            name.clone()
        } else {
            format!("{}:{}", name, format_labels(labels))
        };

        let data = vec![(0, stats.median), (1, stats.p95), (2, stats.p99)];

        series_data.push((label, data));
    }

    // Calculate max value for y-axis
    let max_value = series_data
        .iter()
        .flat_map(|(_, data)| data.iter().map(|(_, v)| *v))
        .fold(0.0f64, f64::max)
        * 1.1; // Add 10% padding

    // Build chart
    let mut chart = ChartBuilder::on(&root)
        .caption(&config.title, ("sans-serif", 40).into_font())
        .margin(10)
        .x_label_area_size(60)
        .y_label_area_size(60)
        .build_cartesian_2d(0usize..2usize, 0.0..max_value)
        .map_err(|e| VizError::RenderingError(format!("Failed to build chart: {e}")))?;

    chart
        .configure_mesh()
        .x_desc(&config.x_label)
        .y_desc(&config.y_label)
        .x_labels(3)
        .x_label_formatter(&|x| percentile_labels.get(*x).unwrap_or(&"").to_string())
        .draw()
        .map_err(|e| VizError::RenderingError(format!("Failed to configure mesh: {e}")))?;

    // Draw lines for each metric
    let colors = [RED, BLUE, GREEN, MAGENTA, CYAN, YELLOW];

    for (idx, (label, data)) in series_data.iter().enumerate() {
        let color = colors[idx % colors.len()];

        chart
            .draw_series(LineSeries::new(
                data.iter().map(|(x, y)| (*x, *y)),
                color.stroke_width(2),
            ))
            .map_err(|e| VizError::RenderingError(format!("Failed to draw line series: {e}")))?
            .label(truncate_label(label, 25))
            .legend(move |(x, y)| {
                PathElement::new(vec![(x, y), (x + 20, y)], color.stroke_width(3))
            });

        // Draw points
        chart
            .draw_series(
                data.iter()
                    .map(|(x, y)| Circle::new((*x, *y), 4, color.filled())),
            )
            .map_err(|e| VizError::RenderingError(format!("Failed to draw points: {e}")))?;
    }

    // Draw legend
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

/// Format labels map as a string
fn format_labels(labels: &std::collections::BTreeMap<String, String>) -> String {
    labels
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Truncate label to max length
fn truncate_label(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use des_metrics::simulation_metrics::SimulationMetrics;

    #[test]
    fn test_percentile_chart_generation() {
        let mut metrics = SimulationMetrics::new();

        // Add histogram data for multiple metrics
        for i in 1..=100 {
            metrics.record_histogram("latency_a", i as f64, &[]);
            metrics.record_histogram("latency_b", (i * 2) as f64, &[]);
        }

        let snapshot = metrics.get_metrics_snapshot();
        let output_path = std::env::temp_dir().join("test_percentiles.png");

        let result = create_percentile_chart(&snapshot, &output_path);
        assert!(result.is_ok());
        assert!(output_path.exists());

        // Cleanup
        std::fs::remove_file(output_path).ok();
    }
}
