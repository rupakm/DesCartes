//! Throughput visualization charts

use crate::charts::ChartConfig;
use crate::error::VizError;
use des_metrics::simulation_metrics::MetricsSnapshot;
use plotters::prelude::*;
use std::path::Path;

/// Create a throughput chart
///
/// Generates a bar chart showing throughput-related counters
/// (requests sent, responses received, successes, failures, etc.)
///
/// # Arguments
/// * `snapshot` - Metrics snapshot containing counter data
/// * `output_path` - Output file path (PNG, SVG, or other supported formats)
///
/// # Example
/// ```no_run
/// use des_metrics::SimulationMetrics;
/// use des_viz::charts::throughput::create_throughput_chart;
///
/// let metrics = SimulationMetrics::new();
/// let snapshot = metrics.get_metrics_snapshot();
/// create_throughput_chart(&snapshot, "throughput.png").unwrap();
/// ```
pub fn create_throughput_chart(
    snapshot: &MetricsSnapshot,
    output_path: impl AsRef<Path>,
) -> Result<(), VizError> {
    let config = ChartConfig::new("Throughput Metrics")
        .x_label("Metric")
        .y_label("Count");

    create_throughput_chart_with_config(snapshot, output_path, config)
}

/// Create a throughput chart with custom configuration
pub fn create_throughput_chart_with_config(
    snapshot: &MetricsSnapshot,
    output_path: impl AsRef<Path>,
    config: ChartConfig,
) -> Result<(), VizError> {
    if snapshot.counters.is_empty() {
        return Err(VizError::InvalidConfiguration(
            "No counter data available for throughput chart".to_string(),
        ));
    }

    let output_path = output_path.as_ref();
    let root = BitMapBackend::new(output_path, (config.width, config.height)).into_drawing_area();

    root.fill(&WHITE)
        .map_err(|e| VizError::RenderingError(format!("Failed to fill background: {e}")))?;

    // Extract counter data
    let mut chart_data: Vec<(String, u64)> = Vec::new();
    for (name, labels, value) in &snapshot.counters {
        let label = if labels.is_empty() {
            name.clone()
        } else {
            format!("{}:{}", name, format_labels(labels))
        };
        chart_data.push((label, *value));
    }

    // Sort by value descending
    chart_data.sort_by(|a, b| b.1.cmp(&a.1));

    // Limit to top 20 metrics to keep chart readable
    if chart_data.len() > 20 {
        chart_data.truncate(20);
    }

    let max_value = chart_data.iter().map(|(_, v)| *v).max().unwrap_or(1) as f64 * 1.1; // Add 10% padding

    // Build chart
    let mut chart = ChartBuilder::on(&root)
        .caption(&config.title, ("sans-serif", 40).into_font())
        .margin(10)
        .x_label_area_size(150)
        .y_label_area_size(80)
        .build_cartesian_2d(0..chart_data.len(), 0.0..max_value)
        .map_err(|e| VizError::RenderingError(format!("Failed to build chart: {e}")))?;

    chart
        .configure_mesh()
        .x_desc(&config.x_label)
        .y_desc(&config.y_label)
        .x_labels(chart_data.len())
        .x_label_formatter(&|x| {
            chart_data
                .get(*x)
                .map(|(name, _)| truncate_label(name, 20))
                .unwrap_or_default()
        })
        .y_label_formatter(&|y| format_large_number(*y as u64))
        .draw()
        .map_err(|e| VizError::RenderingError(format!("Failed to configure mesh: {e}")))?;

    // Draw bars
    chart
        .draw_series(chart_data.iter().enumerate().map(|(idx, (_, value))| {
            Rectangle::new([(idx, 0.0), (idx, *value as f64)], BLUE.filled())
        }))
        .map_err(|e| VizError::RenderingError(format!("Failed to draw bars: {e}")))?;

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

/// Format large numbers with K/M/B suffixes
fn format_large_number(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use des_metrics::simulation_metrics::SimulationMetrics;

    #[test]
    fn test_throughput_chart_generation() {
        let mut metrics = SimulationMetrics::new();

        // Add counter data
        for _ in 0..100 {
            metrics.increment_counter("requests_sent", &[("component", "client1")]);
        }
        for _ in 0..95 {
            metrics.increment_counter("responses_success", &[("component", "server1")]);
        }
        for _ in 0..5 {
            metrics.increment_counter("responses_failure", &[("component", "server1")]);
        }

        let snapshot = metrics.get_metrics_snapshot();
        let output_path = std::env::temp_dir().join("test_throughput.png");

        let result = create_throughput_chart(&snapshot, &output_path);
        assert!(result.is_ok());
        assert!(output_path.exists());

        // Cleanup
        std::fs::remove_file(output_path).ok();
    }

    #[test]
    fn test_format_large_number() {
        assert_eq!(format_large_number(500), "500");
        assert_eq!(format_large_number(1_500), "1.5K");
        assert_eq!(format_large_number(2_500_000), "2.5M");
        assert_eq!(format_large_number(3_500_000_000), "3.5B");
    }
}
