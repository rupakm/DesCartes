//! CSV export for metrics
//!
//! Exports metrics in CSV format suitable for spreadsheet analysis and pandas.
//! Creates separate CSV files for counters, gauges, and histograms.

use crate::error::MetricsError;
use crate::export::MetricsExporter;
use crate::simulation_metrics::SimulationMetrics;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// CSV exporter for simulation metrics
///
/// This exporter creates separate CSV files for different metric types:
/// - `{base}_counters.csv` - Counter metrics
/// - `{base}_gauges.csv` - Gauge metrics
/// - `{base}_histograms.csv` - Histogram statistics
/// - `{base}_request_stats.csv` - Request tracker statistics
#[derive(Debug)]
pub struct CsvExporter {
    path: PathBuf,
    include_labels: bool,
}

impl CsvExporter {
    /// Create a new CSV exporter
    ///
    /// # Arguments
    /// * `path` - Base output file path (will create multiple files with suffixes)
    /// * `include_labels` - Whether to include label columns in the output
    pub fn new(path: &Path, include_labels: bool) -> Self {
        Self {
            path: path.to_path_buf(),
            include_labels,
        }
    }

    /// Get the path for a specific CSV file
    fn get_path_for_type(&self, suffix: &str) -> PathBuf {
        let stem = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("metrics");
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        parent.join(format!("{stem}_{suffix}.csv"))
    }
}

impl MetricsExporter for CsvExporter {
    fn export(&self, metrics: &SimulationMetrics) -> Result<(), MetricsError> {
        let snapshot = metrics.get_metrics_snapshot();

        // Export counters
        self.export_counters(&snapshot.counters)?;

        // Export gauges
        self.export_gauges(&snapshot.gauges)?;

        // Export histograms
        self.export_histograms(&snapshot.histograms)?;

        // Export request stats
        self.export_request_stats(metrics)?;

        Ok(())
    }
}

impl CsvExporter {
    fn export_counters(
        &self,
        counters: &[(String, std::collections::BTreeMap<String, String>, u64)],
    ) -> Result<(), MetricsError> {
        let path = self.get_path_for_type("counters");
        let mut file = File::create(&path)
            .map_err(|e| MetricsError::ExportError(format!("Failed to create file: {e}")))?;

        // Write header
        if self.include_labels {
            writeln!(file, "metric_name,labels,value")
        } else {
            writeln!(file, "metric_name,value")
        }
        .map_err(|e| MetricsError::ExportError(format!("Failed to write header: {e}")))?;

        // Write data
        for (name, labels, value) in counters {
            if self.include_labels {
                let labels_str = format_labels(labels);
                writeln!(
                    file,
                    "{},{},{}",
                    escape_csv(name),
                    escape_csv(&labels_str),
                    value
                )
            } else {
                writeln!(file, "{},{}", escape_csv(name), value)
            }
            .map_err(|e| MetricsError::ExportError(format!("Failed to write row: {e}")))?;
        }

        Ok(())
    }

    fn export_gauges(
        &self,
        gauges: &[(String, std::collections::BTreeMap<String, String>, f64)],
    ) -> Result<(), MetricsError> {
        let path = self.get_path_for_type("gauges");
        let mut file = File::create(&path)
            .map_err(|e| MetricsError::ExportError(format!("Failed to create file: {e}")))?;

        // Write header
        if self.include_labels {
            writeln!(file, "metric_name,labels,value")
        } else {
            writeln!(file, "metric_name,value")
        }
        .map_err(|e| MetricsError::ExportError(format!("Failed to write header: {e}")))?;

        // Write data
        for (name, labels, value) in gauges {
            if self.include_labels {
                let labels_str = format_labels(labels);
                writeln!(
                    file,
                    "{},{},{}",
                    escape_csv(name),
                    escape_csv(&labels_str),
                    value
                )
            } else {
                writeln!(file, "{},{}", escape_csv(name), value)
            }
            .map_err(|e| MetricsError::ExportError(format!("Failed to write row: {e}")))?;
        }

        Ok(())
    }

    fn export_histograms(
        &self,
        histograms: &[(
            String,
            std::collections::BTreeMap<String, String>,
            crate::simulation_metrics::HistogramStats,
        )],
    ) -> Result<(), MetricsError> {
        let path = self.get_path_for_type("histograms");
        let mut file = File::create(&path)
            .map_err(|e| MetricsError::ExportError(format!("Failed to create file: {e}")))?;

        // Write header
        if self.include_labels {
            writeln!(
                file,
                "metric_name,labels,count,sum,min,max,mean,median,p95,p99"
            )
        } else {
            writeln!(file, "metric_name,count,sum,min,max,mean,median,p95,p99")
        }
        .map_err(|e| MetricsError::ExportError(format!("Failed to write header: {e}")))?;

        // Write data
        for (name, labels, stats) in histograms {
            if self.include_labels {
                let labels_str = format_labels(labels);
                writeln!(
                    file,
                    "{},{},{},{},{},{},{},{},{},{}",
                    escape_csv(name),
                    escape_csv(&labels_str),
                    stats.count,
                    stats.sum,
                    stats.min,
                    stats.max,
                    stats.mean,
                    stats.median,
                    stats.p95,
                    stats.p99
                )
            } else {
                writeln!(
                    file,
                    "{},{},{},{},{},{},{},{},{}",
                    escape_csv(name),
                    stats.count,
                    stats.sum,
                    stats.min,
                    stats.max,
                    stats.mean,
                    stats.median,
                    stats.p95,
                    stats.p99
                )
            }
            .map_err(|e| MetricsError::ExportError(format!("Failed to write row: {e}")))?;
        }

        Ok(())
    }

    fn export_request_stats(&self, metrics: &SimulationMetrics) -> Result<(), MetricsError> {
        use std::time::Duration;

        let stats = metrics.get_request_stats(Duration::from_secs(3600));
        let path = self.get_path_for_type("request_stats");
        let mut file = File::create(&path)
            .map_err(|e| MetricsError::ExportError(format!("Failed to create file: {e}")))?;

        // Write header
        writeln!(file, "metric,value")
            .map_err(|e| MetricsError::ExportError(format!("Failed to write header: {e}")))?;

        // Write aggregate stats
        writeln!(file, "active_requests,{}", stats.active_requests)?;
        writeln!(file, "active_attempts,{}", stats.active_attempts)?;
        writeln!(file, "completed_requests,{}", stats.completed_requests)?;
        writeln!(file, "completed_attempts,{}", stats.completed_attempts)?;
        writeln!(file, "goodput,{}", stats.goodput)?;
        writeln!(file, "throughput,{}", stats.throughput)?;
        writeln!(file, "retry_rate,{}", stats.retry_rate)?;
        writeln!(file, "timeout_rate,{}", stats.timeout_rate)?;
        writeln!(file, "error_rate,{}", stats.error_rate)?;
        writeln!(file, "success_rate,{}", stats.success_rate)?;

        // Write request latency stats
        writeln!(
            file,
            "request_latency_mean_ms,{}",
            stats.request_latency.mean.as_secs_f64() * 1000.0
        )?;
        writeln!(
            file,
            "request_latency_median_ms,{}",
            stats.request_latency.median.as_secs_f64() * 1000.0
        )?;
        writeln!(
            file,
            "request_latency_p95_ms,{}",
            stats.request_latency.p95.as_secs_f64() * 1000.0
        )?;
        writeln!(
            file,
            "request_latency_p99_ms,{}",
            stats.request_latency.p99.as_secs_f64() * 1000.0
        )?;
        writeln!(
            file,
            "request_latency_p999_ms,{}",
            stats.request_latency.p999.as_secs_f64() * 1000.0
        )?;

        // Write attempt latency stats
        writeln!(
            file,
            "attempt_latency_mean_ms,{}",
            stats.attempt_latency.mean.as_secs_f64() * 1000.0
        )?;
        writeln!(
            file,
            "attempt_latency_median_ms,{}",
            stats.attempt_latency.median.as_secs_f64() * 1000.0
        )?;
        writeln!(
            file,
            "attempt_latency_p95_ms,{}",
            stats.attempt_latency.p95.as_secs_f64() * 1000.0
        )?;
        writeln!(
            file,
            "attempt_latency_p99_ms,{}",
            stats.attempt_latency.p99.as_secs_f64() * 1000.0
        )?;
        writeln!(
            file,
            "attempt_latency_p999_ms,{}",
            stats.attempt_latency.p999.as_secs_f64() * 1000.0
        )?;

        Ok(())
    }
}

/// Format labels as a string (key1=value1,key2=value2)
fn format_labels(labels: &std::collections::BTreeMap<String, String>) -> String {
    if labels.is_empty() {
        return String::new();
    }

    labels
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Escape CSV field (add quotes if needed)
fn escape_csv(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_csv_export() {
        let mut metrics = SimulationMetrics::new();

        // Add some test data
        metrics.increment_counter("test_counter", &[("component", "test")]);
        metrics.record_gauge("test_gauge", 42.0, &[("component", "test")]);
        metrics.record_histogram("test_histogram", 123.45, &[("component", "test")]);

        // Export to CSV
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join("test_metrics.csv");
        let exporter = CsvExporter::new(&temp_file, true);
        let result = exporter.export(&metrics);

        assert!(result.is_ok());

        // Check that files were created
        assert!(temp_dir.join("test_metrics_counters.csv").exists());
        assert!(temp_dir.join("test_metrics_gauges.csv").exists());
        assert!(temp_dir.join("test_metrics_histograms.csv").exists());

        // Cleanup
        std::fs::remove_file(temp_dir.join("test_metrics_counters.csv")).ok();
        std::fs::remove_file(temp_dir.join("test_metrics_gauges.csv")).ok();
        std::fs::remove_file(temp_dir.join("test_metrics_histograms.csv")).ok();
        std::fs::remove_file(temp_dir.join("test_metrics_request_stats.csv")).ok();
    }

    #[test]
    fn test_escape_csv() {
        assert_eq!(escape_csv("simple"), "simple");
        assert_eq!(escape_csv("with,comma"), "\"with,comma\"");
        assert_eq!(escape_csv("with\"quote"), "\"with\"\"quote\"");
    }

    #[test]
    fn test_format_labels() {
        let mut labels = std::collections::BTreeMap::new();
        assert_eq!(format_labels(&labels), "");

        labels.insert("key1".to_string(), "value1".to_string());
        assert_eq!(format_labels(&labels), "key1=value1");

        labels.insert("key2".to_string(), "value2".to_string());
        assert_eq!(format_labels(&labels), "key1=value1,key2=value2");
    }
}
