//! JSON export for metrics
//!
//! Exports metrics in structured JSON format suitable for programmatic consumption
//! and visualization tools.

use crate::error::MetricsError;
use crate::export::MetricsExporter;
use crate::simulation_metrics::{HistogramStats, LatencyStats, MetricsSnapshot, SimulationMetrics};
use crate::request_tracker::{LatencyStats as RequestLatencyStats, RequestTrackerStats};
use serde::Serialize;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// JSON exporter for simulation metrics
#[derive(Debug)]
pub struct JsonExporter {
    path: PathBuf,
    pretty: bool,
}

impl JsonExporter {
    /// Create a new JSON exporter
    ///
    /// # Arguments
    /// * `path` - Output file path
    /// * `pretty` - Whether to pretty-print the JSON (adds whitespace for readability)
    pub fn new(path: &Path, pretty: bool) -> Self {
        Self {
            path: path.to_path_buf(),
            pretty,
        }
    }
}

impl MetricsExporter for JsonExporter {
    fn export(&self, metrics: &SimulationMetrics) -> Result<(), MetricsError> {
        // Get snapshot and request stats
        let snapshot = metrics.get_metrics_snapshot();
        let request_stats = metrics.get_request_stats(Duration::from_secs(3600)); // 1 hour window

        // Collect all latency stats
        let mut latency_stats = Vec::new();
        for name in &["latency", "response_time_ms", "request_duration_ms", "service_time"] {
            if let Some(stats) = metrics.get_latency_stats(name) {
                latency_stats.push((name.to_string(), SerializableLatencyStats::from(stats)));
            }
        }

        // Create serializable export structure
        let export_data = ExportData {
            snapshot: SerializableMetricsSnapshot::from(snapshot),
            request_stats: Some(SerializableRequestTrackerStats::from(request_stats)),
            latency_stats,
        };

        // Serialize to JSON
        let json = if self.pretty {
            serde_json::to_string_pretty(&export_data)
        } else {
            serde_json::to_string(&export_data)
        }
        .map_err(|e| MetricsError::ExportError(format!("JSON serialization failed: {e}")))?;

        // Write to file
        let mut file = File::create(&self.path)
            .map_err(|e| MetricsError::ExportError(format!("Failed to create file: {e}")))?;

        file.write_all(json.as_bytes())
            .map_err(|e| MetricsError::ExportError(format!("Failed to write to file: {e}")))?;

        Ok(())
    }
}

/// Complete export data structure
#[derive(Debug, Serialize)]
struct ExportData {
    snapshot: SerializableMetricsSnapshot,
    request_stats: Option<SerializableRequestTrackerStats>,
    latency_stats: Vec<(String, SerializableLatencyStats)>,
}

/// Serializable version of MetricsSnapshot
#[derive(Debug, Serialize)]
struct SerializableMetricsSnapshot {
    counters: Vec<CounterEntry>,
    gauges: Vec<GaugeEntry>,
    histograms: Vec<HistogramEntry>,
    timestamp_secs: f64,
}

impl SerializableMetricsSnapshot {
    fn from(snapshot: MetricsSnapshot) -> Self {
        Self {
            counters: snapshot.counters.into_iter()
                .map(|(name, labels, value)| CounterEntry { name, labels, value })
                .collect(),
            gauges: snapshot.gauges.into_iter()
                .map(|(name, labels, value)| GaugeEntry { name, labels, value })
                .collect(),
            histograms: snapshot.histograms.into_iter()
                .map(|(name, labels, stats)| HistogramEntry {
                    name,
                    labels,
                    stats: SerializableHistogramStats::from(stats),
                })
                .collect(),
            timestamp_secs: snapshot.timestamp.as_duration().as_secs_f64(),
        }
    }
}

#[derive(Debug, Serialize)]
struct CounterEntry {
    name: String,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    labels: std::collections::BTreeMap<String, String>,
    value: u64,
}

#[derive(Debug, Serialize)]
struct GaugeEntry {
    name: String,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    labels: std::collections::BTreeMap<String, String>,
    value: f64,
}

#[derive(Debug, Serialize)]
struct HistogramEntry {
    name: String,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    labels: std::collections::BTreeMap<String, String>,
    stats: SerializableHistogramStats,
}

/// Serializable version of HistogramStats
#[derive(Debug, Serialize)]
struct SerializableHistogramStats {
    count: usize,
    sum: f64,
    min: f64,
    max: f64,
    mean: f64,
    median: f64,
    p95: f64,
    p99: f64,
}

impl SerializableHistogramStats {
    fn from(stats: HistogramStats) -> Self {
        Self {
            count: stats.count,
            sum: stats.sum,
            min: stats.min,
            max: stats.max,
            mean: stats.mean,
            median: stats.median,
            p95: stats.p95,
            p99: stats.p99,
        }
    }
}

/// Serializable version of LatencyStats (from simulation_metrics)
#[derive(Debug, Serialize)]
struct SerializableLatencyStats {
    count: u64,
    min_ms: f64,
    max_ms: f64,
    mean_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    p999_ms: f64,
    std_dev_ms: f64,
}

impl SerializableLatencyStats {
    fn from(stats: LatencyStats) -> Self {
        Self {
            count: stats.count,
            min_ms: stats.min.as_secs_f64() * 1000.0,
            max_ms: stats.max.as_secs_f64() * 1000.0,
            mean_ms: stats.mean.as_secs_f64() * 1000.0,
            p50_ms: stats.p50.as_secs_f64() * 1000.0,
            p95_ms: stats.p95.as_secs_f64() * 1000.0,
            p99_ms: stats.p99.as_secs_f64() * 1000.0,
            p999_ms: stats.p999.as_secs_f64() * 1000.0,
            std_dev_ms: stats.std_dev.as_secs_f64() * 1000.0,
        }
    }
}

/// Serializable version of RequestTrackerStats
#[derive(Debug, Serialize)]
struct SerializableRequestTrackerStats {
    active_requests: usize,
    active_attempts: usize,
    completed_requests: usize,
    completed_attempts: usize,
    goodput: f64,
    throughput: f64,
    retry_rate: f64,
    timeout_rate: f64,
    error_rate: f64,
    success_rate: f64,
    request_latency: SerializableRequestLatencyStats,
    attempt_latency: SerializableRequestLatencyStats,
}

impl SerializableRequestTrackerStats {
    fn from(stats: RequestTrackerStats) -> Self {
        Self {
            active_requests: stats.active_requests,
            active_attempts: stats.active_attempts,
            completed_requests: stats.completed_requests,
            completed_attempts: stats.completed_attempts,
            goodput: stats.goodput,
            throughput: stats.throughput,
            retry_rate: stats.retry_rate,
            timeout_rate: stats.timeout_rate,
            error_rate: stats.error_rate,
            success_rate: stats.success_rate,
            request_latency: SerializableRequestLatencyStats::from(stats.request_latency),
            attempt_latency: SerializableRequestLatencyStats::from(stats.attempt_latency),
        }
    }
}

/// Serializable version of request tracker LatencyStats
#[derive(Debug, Serialize)]
struct SerializableRequestLatencyStats {
    mean_ms: f64,
    median_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    p999_ms: f64,
    min_ms: f64,
    max_ms: f64,
    std_dev_ms: f64,
    count: usize,
}

impl SerializableRequestLatencyStats {
    fn from(stats: RequestLatencyStats) -> Self {
        Self {
            mean_ms: stats.mean.as_secs_f64() * 1000.0,
            median_ms: stats.median.as_secs_f64() * 1000.0,
            p50_ms: stats.p50.as_secs_f64() * 1000.0,
            p95_ms: stats.p95.as_secs_f64() * 1000.0,
            p99_ms: stats.p99.as_secs_f64() * 1000.0,
            p999_ms: stats.p999.as_secs_f64() * 1000.0,
            min_ms: stats.min.as_secs_f64() * 1000.0,
            max_ms: stats.max.as_secs_f64() * 1000.0,
            std_dev_ms: stats.std_dev.as_secs_f64() * 1000.0,
            count: stats.count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_json_export() {
        let mut metrics = SimulationMetrics::new();

        // Add some test data
        metrics.increment_counter("test_counter", &[("component", "test")]);
        metrics.record_gauge("test_gauge", 42.0, &[("component", "test")]);
        metrics.record_histogram("test_histogram", 123.45, &[("component", "test")]);
        metrics.record_latency("latency", Duration::from_millis(50), &[]);

        // Export to JSON
        let temp_file = std::env::temp_dir().join("test_metrics.json");
        let exporter = JsonExporter::new(&temp_file, true);
        let result = exporter.export(&metrics);

        assert!(result.is_ok());
        assert!(temp_file.exists());

        // Verify JSON is valid
        let json_content = std::fs::read_to_string(&temp_file).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_content).unwrap();
        assert!(parsed.is_object());

        // Cleanup
        std::fs::remove_file(temp_file).ok();
    }
}
