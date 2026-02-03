//! HTML report generation

use crate::charts;
use crate::error::VizError;
use descartes_metrics::simulation_metrics::MetricsSnapshot;
use std::fs;
use std::path::Path;

/// Generate a complete HTML report with embedded charts
///
/// Creates an HTML file with:
/// - Summary statistics
/// - Embedded PNG charts
/// - Formatted metrics tables
///
/// # Arguments
/// * `snapshot` - Metrics snapshot to visualize
/// * `output_path` - Output HTML file path
///
/// # Example
/// ```no_run
/// use descartes_metrics::SimulationMetrics;
/// use descartes_viz::report::generate_html_report;
///
/// let metrics = SimulationMetrics::new();
/// let snapshot = metrics.get_metrics_snapshot();
/// generate_html_report(&snapshot, "report.html").unwrap();
/// ```
pub fn generate_html_report(
    snapshot: &MetricsSnapshot,
    output_path: impl AsRef<Path>,
) -> Result<(), VizError> {
    let output_path = output_path.as_ref();
    let report_dir = output_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(report_dir)?;

    // Generate charts in a subdirectory
    let charts_dir = report_dir.join("charts");
    fs::create_dir_all(&charts_dir)?;

    charts::generate_all_charts(snapshot, &charts_dir)?;

    // Build HTML content
    let html = build_html_content(snapshot, &charts_dir)?;

    // Write HTML file
    fs::write(output_path, html)?;

    Ok(())
}

/// Build HTML content for the report
fn build_html_content(snapshot: &MetricsSnapshot, charts_dir: &Path) -> Result<String, VizError> {
    let mut html = String::new();

    // HTML header
    html.push_str(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Simulation Metrics Report</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Arial, sans-serif;
            max-width: 1400px;
            margin: 0 auto;
            padding: 20px;
            background-color: #f5f5f5;
        }
        h1 {
            color: #333;
            border-bottom: 3px solid #4CAF50;
            padding-bottom: 10px;
        }
        h2 {
            color: #555;
            margin-top: 30px;
            border-bottom: 2px solid #ddd;
            padding-bottom: 5px;
        }
        .summary {
            background: white;
            padding: 20px;
            border-radius: 8px;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
            margin-bottom: 20px;
        }
        .summary-grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 15px;
            margin-top: 15px;
        }
        .summary-item {
            background: #f9f9f9;
            padding: 15px;
            border-radius: 4px;
            border-left: 4px solid #4CAF50;
        }
        .summary-label {
            font-size: 14px;
            color: #666;
            margin-bottom: 5px;
        }
        .summary-value {
            font-size: 24px;
            font-weight: bold;
            color: #333;
        }
        .chart {
            background: white;
            padding: 20px;
            border-radius: 8px;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
            margin-bottom: 30px;
        }
        .chart img {
            max-width: 100%;
            height: auto;
            display: block;
            margin: 0 auto;
        }
        table {
            width: 100%;
            border-collapse: collapse;
            background: white;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
            margin-bottom: 20px;
        }
        th {
            background: #4CAF50;
            color: white;
            padding: 12px;
            text-align: left;
        }
        td {
            padding: 10px 12px;
            border-bottom: 1px solid #ddd;
        }
        tr:hover {
            background: #f5f5f5;
        }
        .timestamp {
            color: #666;
            font-size: 14px;
            margin-bottom: 20px;
        }
    </style>
</head>
<body>
    <h1>📊 Simulation Metrics Report</h1>
"#,
    );

    // Timestamp
    html.push_str(&format!(
        r#"    <div class="timestamp">Generated at simulation time: {:.3}s</div>
"#,
        snapshot.timestamp.as_duration().as_secs_f64()
    ));

    // Summary section
    html.push_str(
        r#"    <div class="summary">
        <h2>Summary</h2>
        <div class="summary-grid">
"#,
    );

    html.push_str(&format!(
        r#"            <div class="summary-item">
                <div class="summary-label">Total Counters</div>
                <div class="summary-value">{}</div>
            </div>
            <div class="summary-item">
                <div class="summary-label">Total Gauges</div>
                <div class="summary-value">{}</div>
            </div>
            <div class="summary-item">
                <div class="summary-label">Total Histograms</div>
                <div class="summary-value">{}</div>
            </div>
"#,
        snapshot.counters.len(),
        snapshot.gauges.len(),
        snapshot.histograms.len()
    ));

    html.push_str(
        r#"        </div>
    </div>
"#,
    );

    // Charts section
    html.push_str(
        r#"    <h2>Visualizations</h2>
"#,
    );

    // Embed charts if they exist
    let chart_files = [
        ("latency_stats.png", "Latency Statistics"),
        ("throughput.png", "Throughput Metrics"),
        ("percentiles.png", "Percentile Comparison"),
    ];

    for (filename, title) in &chart_files {
        let chart_path = charts_dir.join(filename);
        if chart_path.exists() {
            let rel_path = format!("charts/{filename}");
            html.push_str(&format!(
                r#"    <div class="chart">
        <h3>{title}</h3>
        <img src="{rel_path}" alt="{title}">
    </div>
"#
            ));
        }
    }

    // Metrics tables
    if !snapshot.counters.is_empty() {
        html.push_str(
            r#"    <h2>Counter Metrics</h2>
    <table>
        <tr>
            <th>Metric Name</th>
            <th>Labels</th>
            <th>Value</th>
        </tr>
"#,
        );

        for (name, labels, value) in &snapshot.counters {
            let labels_str = format_labels_html(labels);
            html.push_str(&format!(
                r#"        <tr>
            <td>{}</td>
            <td>{}</td>
            <td>{}</td>
        </tr>
"#,
                html_escape(name),
                labels_str,
                value
            ));
        }

        html.push_str(
            r#"    </table>
"#,
        );
    }

    if !snapshot.gauges.is_empty() {
        html.push_str(
            r#"    <h2>Gauge Metrics</h2>
    <table>
        <tr>
            <th>Metric Name</th>
            <th>Labels</th>
            <th>Value</th>
        </tr>
"#,
        );

        for (name, labels, value) in &snapshot.gauges {
            let labels_str = format_labels_html(labels);
            html.push_str(&format!(
                r#"        <tr>
            <td>{}</td>
            <td>{}</td>
            <td>{:.2}</td>
        </tr>
"#,
                html_escape(name),
                labels_str,
                value
            ));
        }

        html.push_str(
            r#"    </table>
"#,
        );
    }

    if !snapshot.histograms.is_empty() {
        html.push_str(
            r#"    <h2>Histogram Metrics</h2>
    <table>
        <tr>
            <th>Metric Name</th>
            <th>Labels</th>
            <th>Count</th>
            <th>Mean</th>
            <th>Median</th>
            <th>P95</th>
            <th>P99</th>
        </tr>
"#,
        );

        for (name, labels, stats) in &snapshot.histograms {
            let labels_str = format_labels_html(labels);
            html.push_str(&format!(
                r#"        <tr>
            <td>{}</td>
            <td>{}</td>
            <td>{}</td>
            <td>{:.2}</td>
            <td>{:.2}</td>
            <td>{:.2}</td>
            <td>{:.2}</td>
        </tr>
"#,
                html_escape(name),
                labels_str,
                stats.count,
                stats.mean,
                stats.median,
                stats.p95,
                stats.p99
            ));
        }

        html.push_str(
            r#"    </table>
"#,
        );
    }

    // HTML footer
    html.push_str(
        r#"</body>
</html>
"#,
    );

    Ok(html)
}

/// Format labels for HTML display
fn format_labels_html(labels: &std::collections::BTreeMap<String, String>) -> String {
    if labels.is_empty() {
        return String::from("-");
    }

    labels
        .iter()
        .map(|(k, v)| format!("{}={}", html_escape(k), html_escape(v)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Escape HTML special characters
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use descartes_metrics::simulation_metrics::SimulationMetrics;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn make_temp_dir(prefix: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("{prefix}_{pid}_{n}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn test_html_report_generation() {
        let mut metrics = SimulationMetrics::new();

        // Add test data
        metrics.increment_counter("test_counter", &[("component", "test")]);
        metrics.record_gauge("test_gauge", 42.0, &[("component", "test")]);
        metrics.record_histogram("test_histogram", 123.45, &[("component", "test")]);

        let snapshot = metrics.get_metrics_snapshot();
        let dir = make_temp_dir("descartes_viz_report_test");
        let output_path = dir.join("report.html");

        let result = generate_html_report(&snapshot, &output_path);
        assert!(result.is_ok());
        assert!(output_path.exists());

        // Verify HTML content
        let content = fs::read_to_string(&output_path).expect("read generated report");
        assert!(content.contains("<!DOCTYPE html>"));
        assert!(content.contains("Simulation Metrics Report"));
        assert!(content.contains("test_counter"));

        // Cleanup
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("hello"), "hello");
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a & b"), "a &amp; b");
    }
}
