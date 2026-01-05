//! Simulation-specific metrics collection using standard metrics ecosystem
//!
//! This module provides a thin wrapper around the standard `metrics` crate,
//! adding simulation-specific functionality while leveraging battle-tested
//! metrics infrastructure.

use crate::request_tracker::{RequestTracker, RequestTrackerStats};
use des_core::SimTime;
use hdrhistogram::Histogram as HdrHistogram;
use metrics::{counter, gauge, histogram};
use std::collections::{HashMap, BTreeMap};
use std::time::Duration;

/// Key for identifying metrics with labels
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MetricKey {
    name: String,
    labels: BTreeMap<String, String>,
}

impl MetricKey {
    fn new(name: &str, labels: &[(&str, &str)]) -> Self {
        let labels_map = labels.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Self {
            name: name.to_string(),
            labels: labels_map,
        }
    }
    
    #[allow(dead_code)]
    fn simple(name: &str) -> Self {
        Self {
            name: name.to_string(),
            labels: BTreeMap::new(),
        }
    }
}

/// Internal storage for retrievable metrics
#[derive(Debug, Default)]
struct MetricStorage {
    counters: HashMap<MetricKey, u64>,
    gauges: HashMap<MetricKey, f64>,
    histograms: HashMap<MetricKey, Vec<f64>>,
}

/// Statistics for histogram metrics
#[derive(Debug, Clone)]
pub struct HistogramStats {
    pub count: usize,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
}

impl HistogramStats {
    fn from_values(mut values: Vec<f64>) -> Self {
        if values.is_empty() {
            return Self {
                count: 0,
                sum: 0.0,
                min: 0.0,
                max: 0.0,
                mean: 0.0,
                median: 0.0,
                p95: 0.0,
                p99: 0.0,
            };
        }
        
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let count = values.len();
        let sum: f64 = values.iter().sum();
        let mean = sum / count as f64;
        
        let percentile = |p: f64| -> f64 {
            let index = ((count as f64 - 1.0) * p).round() as usize;
            values[index.min(count - 1)]
        };
        
        Self {
            count,
            sum,
            min: values[0],
            max: values[count - 1],
            mean,
            median: percentile(0.5),
            p95: percentile(0.95),
            p99: percentile(0.99),
        }
    }
}

/// Complete snapshot of all metrics
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub counters: Vec<(String, BTreeMap<String, String>, u64)>,
    pub gauges: Vec<(String, BTreeMap<String, String>, f64)>,
    pub histograms: Vec<(String, BTreeMap<String, String>, HistogramStats)>,
    pub timestamp: SimTime,
}

/// Simulation-specific metrics collector that wraps the standard metrics ecosystem
#[derive(Debug)]
pub struct SimulationMetrics {
    /// Request and attempt tracking for simulation analysis
    request_tracker: RequestTracker,
    /// High-resolution histograms for latency analysis
    latency_histograms: HashMap<String, HdrHistogram<u64>>,
    /// Internal storage for retrievable metrics
    storage: MetricStorage,
    /// Simulation start time for relative timing
    start_time: SimTime,
}

impl Default for SimulationMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl SimulationMetrics {
    /// Create a new simulation metrics collector
    pub fn new() -> Self {
        Self {
            request_tracker: RequestTracker::new(),
            latency_histograms: HashMap::new(),
            storage: MetricStorage::default(),
            start_time: SimTime::zero(),
        }
    }

    /// Create a new simulation metrics collector with limited storage
    pub fn with_max_completed(max_completed: usize) -> Self {
        Self {
            request_tracker: RequestTracker::with_max_completed(max_completed),
            latency_histograms: HashMap::new(),
            storage: MetricStorage::default(),
            start_time: SimTime::zero(),
        }
    }

    /// Set the simulation start time for relative timing calculations
    pub fn set_start_time(&mut self, start_time: SimTime) {
        self.start_time = start_time;
    }

    /// Record a counter increment using the standard metrics crate
    pub fn increment_counter(&mut self, name: &str, labels: &[(&str, &str)]) {
        // Store internally for retrieval
        let key = MetricKey::new(name, labels);
        *self.storage.counters.entry(key).or_insert(0) += 1;
        
        // Also record in standard metrics for monitoring
        // Convert everything to owned strings to satisfy the metrics macros
        let name_owned = name.to_string();
        match labels.len() {
            0 => counter!(name_owned).increment(1),
            1 => {
                let k1 = labels[0].0.to_string();
                let v1 = labels[0].1.to_string();
                counter!(name_owned, k1 => v1).increment(1);
            },
            2 => {
                let k1 = labels[0].0.to_string();
                let v1 = labels[0].1.to_string();
                let k2 = labels[1].0.to_string();
                let v2 = labels[1].1.to_string();
                counter!(name_owned, k1 => v1, k2 => v2).increment(1);
            },
            _ => {
                // For more complex cases, build a flattened key
                let key = format!("{}_{}", name, labels.iter().map(|(k, v)| format!("{k}_{v}")).collect::<Vec<_>>().join("_"));
                counter!(key).increment(1);
            }
        }
    }

    /// Record a counter increment with a specific value
    pub fn increment_counter_by(&mut self, name: &str, value: u64, labels: &[(&str, &str)]) {
        // Store internally for retrieval
        let key = MetricKey::new(name, labels);
        *self.storage.counters.entry(key).or_insert(0) += value;
        
        // Also record in standard metrics
        let name_owned = name.to_string();
        match labels.len() {
            0 => counter!(name_owned).increment(value),
            1 => {
                let k1 = labels[0].0.to_string();
                let v1 = labels[0].1.to_string();
                counter!(name_owned, k1 => v1).increment(value);
            },
            2 => {
                let k1 = labels[0].0.to_string();
                let v1 = labels[0].1.to_string();
                let k2 = labels[1].0.to_string();
                let v2 = labels[1].1.to_string();
                counter!(name_owned, k1 => v1, k2 => v2).increment(value);
            },
            _ => {
                let key = format!("{}_{}", name, labels.iter().map(|(k, v)| format!("{k}_{v}")).collect::<Vec<_>>().join("_"));
                counter!(key).increment(value);
            }
        }
    }

    /// Record a gauge value using the standard metrics crate
    pub fn record_gauge(&mut self, name: &str, value: f64, labels: &[(&str, &str)]) {
        // Store internally for retrieval
        let key = MetricKey::new(name, labels);
        self.storage.gauges.insert(key, value);
        
        // Also record in standard metrics
        let name_owned = name.to_string();
        match labels.len() {
            0 => gauge!(name_owned).set(value),
            1 => {
                let k1 = labels[0].0.to_string();
                let v1 = labels[0].1.to_string();
                gauge!(name_owned, k1 => v1).set(value);
            },
            2 => {
                let k1 = labels[0].0.to_string();
                let v1 = labels[0].1.to_string();
                let k2 = labels[1].0.to_string();
                let v2 = labels[1].1.to_string();
                gauge!(name_owned, k1 => v1, k2 => v2).set(value);
            },
            _ => {
                let key = format!("{}_{}", name, labels.iter().map(|(k, v)| format!("{k}_{v}")).collect::<Vec<_>>().join("_"));
                gauge!(key).set(value);
            }
        }
    }

    /// Record a histogram value using the standard metrics crate
    pub fn record_histogram(&mut self, name: &str, value: f64, labels: &[(&str, &str)]) {
        // Store internally for retrieval
        let key = MetricKey::new(name, labels);
        self.storage.histograms.entry(key).or_default().push(value);
        
        // Also record in standard metrics
        let name_owned = name.to_string();
        match labels.len() {
            0 => histogram!(name_owned).record(value),
            1 => {
                let k1 = labels[0].0.to_string();
                let v1 = labels[0].1.to_string();
                histogram!(name_owned, k1 => v1).record(value);
            },
            2 => {
                let k1 = labels[0].0.to_string();
                let v1 = labels[0].1.to_string();
                let k2 = labels[1].0.to_string();
                let v2 = labels[1].1.to_string();
                histogram!(name_owned, k1 => v1, k2 => v2).record(value);
            },
            _ => {
                let key = format!("{}_{}", name, labels.iter().map(|(k, v)| format!("{k}_{v}")).collect::<Vec<_>>().join("_"));
                histogram!(key).record(value);
            }
        }
    }

    /// Record a duration as a histogram (converts to milliseconds)
    pub fn record_duration(&mut self, name: &str, duration: Duration, labels: &[(&str, &str)]) {
        let millis = duration.as_secs_f64() * 1000.0;
        self.record_histogram(name, millis, labels);
    }

    /// Record a latency with high-resolution histogram for detailed analysis
    pub fn record_latency(&mut self, name: &str, latency: Duration, labels: &[(&str, &str)]) {
        // Record in standard metrics for monitoring
        self.record_duration(name, latency, labels);

        // Also record in high-resolution histogram for detailed analysis
        let histogram = self.latency_histograms.entry(name.to_string()).or_insert_with(|| {
            HdrHistogram::new_with_bounds(1, 60_000_000, 3).unwrap() // 1μs to 60s with 3 sig figs
        });

        let micros = latency.as_micros() as u64;
        if let Err(e) = histogram.record(micros) {
            tracing::warn!("Failed to record latency in HDR histogram: {}", e);
        }
    }

    /// Get high-resolution latency statistics for a specific metric
    pub fn get_latency_stats(&self, name: &str) -> Option<LatencyStats> {
        self.latency_histograms.get(name).map(|hist| {
            LatencyStats {
                count: hist.len(),
                min: Duration::from_micros(hist.min()),
                max: Duration::from_micros(hist.max()),
                mean: Duration::from_micros(hist.mean() as u64),
                p50: Duration::from_micros(hist.value_at_quantile(0.5)),
                p95: Duration::from_micros(hist.value_at_quantile(0.95)),
                p99: Duration::from_micros(hist.value_at_quantile(0.99)),
                p999: Duration::from_micros(hist.value_at_quantile(0.999)),
                std_dev: Duration::from_micros(hist.stdev() as u64),
            }
        })
    }

    /// Get the request tracker for simulation-specific analysis
    pub fn request_tracker(&self) -> &RequestTracker {
        &self.request_tracker
    }

    /// Get mutable access to the request tracker
    pub fn request_tracker_mut(&mut self) -> &mut RequestTracker {
        &mut self.request_tracker
    }

    /// Get comprehensive request tracking statistics
    pub fn get_request_stats(&self, time_window: Duration) -> RequestTrackerStats {
        self.request_tracker.get_stats(time_window)
    }

    /// Clear all metrics data
    pub fn clear(&mut self) {
        self.request_tracker.clear();
        self.latency_histograms.clear();
        self.storage = MetricStorage::default();
    }

    // === METRICS RETRIEVAL METHODS ===

    /// Get a counter value by name and labels
    pub fn get_counter(&self, name: &str, labels: &[(&str, &str)]) -> Option<u64> {
        let key = MetricKey::new(name, labels);
        self.storage.counters.get(&key).copied()
    }

    /// Get a counter value by name only (no labels)
    pub fn get_counter_simple(&self, name: &str) -> Option<u64> {
        self.get_counter(name, &[])
    }

    /// Get a gauge value by name and labels
    pub fn get_gauge(&self, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
        let key = MetricKey::new(name, labels);
        self.storage.gauges.get(&key).copied()
    }

    /// Get a gauge value by name only (no labels)
    pub fn get_gauge_simple(&self, name: &str) -> Option<f64> {
        self.get_gauge(name, &[])
    }

    /// Get histogram statistics by name and labels
    pub fn get_histogram_stats(&self, name: &str, labels: &[(&str, &str)]) -> Option<HistogramStats> {
        let key = MetricKey::new(name, labels);
        self.storage.histograms.get(&key).map(|values| {
            HistogramStats::from_values(values.clone())
        })
    }

    /// Get histogram statistics by name only (no labels)
    pub fn get_histogram_stats_simple(&self, name: &str) -> Option<HistogramStats> {
        self.get_histogram_stats(name, &[])
    }

    /// List all counter metrics with their labels and values
    pub fn list_counters(&self) -> Vec<(String, BTreeMap<String, String>, u64)> {
        self.storage.counters.iter()
            .map(|(key, value)| (key.name.clone(), key.labels.clone(), *value))
            .collect()
    }

    /// List all gauge metrics with their labels and values
    pub fn list_gauges(&self) -> Vec<(String, BTreeMap<String, String>, f64)> {
        self.storage.gauges.iter()
            .map(|(key, value)| (key.name.clone(), key.labels.clone(), *value))
            .collect()
    }

    /// List all histogram metric names
    pub fn list_histograms(&self) -> Vec<String> {
        self.storage.histograms.keys()
            .map(|key| key.name.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect()
    }

    /// Get a complete snapshot of all metrics
    pub fn get_metrics_snapshot(&self) -> MetricsSnapshot {
        let counters = self.list_counters();
        let gauges = self.list_gauges();
        let histograms = self.storage.histograms.iter()
            .map(|(key, values)| {
                let stats = HistogramStats::from_values(values.clone());
                (key.name.clone(), key.labels.clone(), stats)
            })
            .collect();

        MetricsSnapshot {
            counters,
            gauges,
            histograms,
            timestamp: SimTime::zero(), // TODO: Use actual simulation time
        }
    }

    /// Reset all stored metric values (but keep structure)
    pub fn reset_metrics(&mut self) {
        self.storage = MetricStorage::default();
    }

    /// Record a component-specific metric with automatic labeling
    pub fn record_component_counter(&mut self, component: &str, metric: &str, value: u64) {
        self.increment_counter_by(metric, value, &[("component", component)]);
    }

    /// Record a component-specific gauge with automatic labeling
    pub fn record_component_gauge(&mut self, component: &str, metric: &str, value: f64) {
        self.record_gauge(metric, value, &[("component", component)]);
    }

    /// Record a component-specific latency with automatic labeling
    pub fn record_component_latency(&mut self, component: &str, metric: &str, latency: Duration) {
        self.record_latency(metric, latency, &[("component", component)]);
    }

    // === EXPORT METHODS ===

    /// Export metrics to JSON format
    ///
    /// # Arguments
    /// * `path` - Output file path
    /// * `pretty` - Whether to pretty-print the JSON (adds whitespace for readability)
    ///
    /// # Example
    /// ```no_run
    /// use des_metrics::SimulationMetrics;
    ///
    /// let metrics = SimulationMetrics::new();
    /// // ... collect metrics ...
    /// metrics.export_json("results/metrics.json", true).unwrap();
    /// ```
    pub fn export_json(&self, path: impl AsRef<std::path::Path>, pretty: bool) -> Result<(), crate::error::MetricsError> {
        crate::export::export_json(self, path, pretty)
    }

    /// Export metrics to CSV format
    ///
    /// Creates multiple CSV files with suffixes for different metric types:
    /// - `{base}_counters.csv` - Counter metrics
    /// - `{base}_gauges.csv` - Gauge metrics
    /// - `{base}_histograms.csv` - Histogram statistics
    /// - `{base}_request_stats.csv` - Request tracker statistics
    ///
    /// # Arguments
    /// * `path` - Base output file path
    /// * `include_labels` - Whether to include label columns in the output
    ///
    /// # Example
    /// ```no_run
    /// use des_metrics::SimulationMetrics;
    ///
    /// let metrics = SimulationMetrics::new();
    /// // ... collect metrics ...
    /// metrics.export_csv("results/metrics.csv", true).unwrap();
    /// // Creates: results/metrics_counters.csv, results/metrics_gauges.csv, etc.
    /// ```
    pub fn export_csv(&self, path: impl AsRef<std::path::Path>, include_labels: bool) -> Result<(), crate::error::MetricsError> {
        crate::export::export_csv(self, path, include_labels)
    }
}

/// High-resolution latency statistics using HdrHistogram
#[derive(Debug, Clone)]
pub struct LatencyStats {
    /// Number of samples
    pub count: u64,
    /// Minimum latency
    pub min: Duration,
    /// Maximum latency
    pub max: Duration,
    /// Mean latency
    pub mean: Duration,
    /// 50th percentile (median)
    pub p50: Duration,
    /// 95th percentile
    pub p95: Duration,
    /// 99th percentile
    pub p99: Duration,
    /// 99.9th percentile
    pub p999: Duration,
    /// Standard deviation
    pub std_dev: Duration,
}

impl std::fmt::Display for LatencyStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Latency Stats: count={}, min={:.2}ms, max={:.2}ms, mean={:.2}ms, p50={:.2}ms, p95={:.2}ms, p99={:.2}ms, p999={:.2}ms",
            self.count,
            self.min.as_secs_f64() * 1000.0,
            self.max.as_secs_f64() * 1000.0,
            self.mean.as_secs_f64() * 1000.0,
            self.p50.as_secs_f64() * 1000.0,
            self.p95.as_secs_f64() * 1000.0,
            self.p99.as_secs_f64() * 1000.0,
            self.p999.as_secs_f64() * 1000.0,
        )
    }
}

/// Setup Prometheus metrics backend for production monitoring
pub fn setup_prometheus_metrics(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    // For now, just install a no-op recorder
    // TODO: Add proper Prometheus setup when metrics-prometheus API is clarified
    tracing::info!("Metrics setup requested on port {} (placeholder implementation)", port);
    Ok(())
}

/// Setup Prometheus metrics backend with custom configuration
pub fn setup_prometheus_metrics_with_config(
    addr: std::net::SocketAddr,
    prefix: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    // For now, just install a no-op recorder
    // TODO: Add proper Prometheus setup when metrics-prometheus API is clarified
    tracing::info!("Metrics setup requested on {} with prefix {:?} (placeholder implementation)", addr, prefix);
    Ok(())
}

/// Helper functions for common simulation metrics
pub mod helpers {
    use super::*;

    /// Record request processing metrics
    pub fn record_request_processed(
        metrics: &mut SimulationMetrics,
        component: &str,
        success: bool,
        latency: Duration,
    ) {
        let status = if success { "success" } else { "error" };
        
        metrics.increment_counter(
            "requests_processed_total",
            &[("component", component), ("status", status)],
        );
        
        if success {
            metrics.record_duration(
                "request_duration_ms",
                latency,
                &[("component", component)],
            );
        }
    }

    /// Record queue depth changes
    pub fn record_queue_depth(metrics: &mut SimulationMetrics, component: &str, depth: usize) {
        metrics.record_gauge(
            "queue_depth",
            depth as f64,
            &[("component", component)],
        );
    }

    /// Record server utilization
    pub fn record_server_utilization(
        metrics: &mut SimulationMetrics,
        component: &str,
        active_requests: usize,
        capacity: usize,
    ) {
        let utilization = if capacity > 0 {
            active_requests as f64 / capacity as f64
        } else {
            0.0
        };

        metrics.record_gauge(
            "server_utilization",
            utilization,
            &[("component", component)],
        );

        metrics.record_gauge(
            "server_active_requests",
            active_requests as f64,
            &[("component", component)],
        );
    }

    /// Record throughput metrics
    pub fn record_throughput(
        metrics: &mut SimulationMetrics,
        component: &str,
        requests_per_second: f64,
    ) {
        metrics.record_gauge(
            "throughput_rps",
            requests_per_second,
            &[("component", component)],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_simulation_metrics_basic() {
        let mut metrics = SimulationMetrics::new();
        
        // Test counter
        metrics.increment_counter("test_counter", &[("component", "test")]);
        metrics.increment_counter_by("test_counter_by", 5, &[("component", "test")]);
        
        // Test gauge
        metrics.record_gauge("test_gauge", 42.0, &[("component", "test")]);
        
        // Test histogram
        metrics.record_histogram("test_histogram", 123.45, &[("component", "test")]);
        
        // Test duration
        metrics.record_duration("test_duration", Duration::from_millis(100), &[("component", "test")]);
        
        // Test retrieval
        assert_eq!(metrics.get_counter("test_counter", &[("component", "test")]), Some(1));
        assert_eq!(metrics.get_counter("test_counter_by", &[("component", "test")]), Some(5));
        assert_eq!(metrics.get_gauge("test_gauge", &[("component", "test")]), Some(42.0));
        
        let histogram_stats = metrics.get_histogram_stats("test_histogram", &[("component", "test")]);
        assert!(histogram_stats.is_some());
        assert_eq!(histogram_stats.unwrap().count, 1);
    }

    #[test]
    fn test_metrics_retrieval() {
        let mut metrics = SimulationMetrics::new();
        
        // Test simple retrieval (no labels)
        metrics.increment_counter("simple_counter", &[]);
        metrics.record_gauge("simple_gauge", std::f64::consts::PI, &[]);
        
        assert_eq!(metrics.get_counter_simple("simple_counter"), Some(1));
        assert_eq!(metrics.get_gauge_simple("simple_gauge"), Some(std::f64::consts::PI));
        
        // Test non-existent metrics
        assert_eq!(metrics.get_counter_simple("nonexistent"), None);
        assert_eq!(metrics.get_gauge_simple("nonexistent"), None);
        
        // Test listing
        let counters = metrics.list_counters();
        assert!(!counters.is_empty());
        
        let gauges = metrics.list_gauges();
        assert!(!gauges.is_empty());
    }

    #[test]
    fn test_latency_recording() {
        let mut metrics = SimulationMetrics::new();
        
        // Record some latencies
        for i in 1..=100 {
            let latency = Duration::from_millis(i);
            metrics.record_latency("test_latency", latency, &[("component", "test")]);
        }
        
        // Get stats
        let stats = metrics.get_latency_stats("test_latency").unwrap();
        assert_eq!(stats.count, 100);
        assert!(stats.min <= stats.p50);
        assert!(stats.p50 <= stats.p95);
        assert!(stats.p95 <= stats.p99);
        assert!(stats.p99 <= stats.p999);
        assert!(stats.p999 <= stats.max);
    }

    #[test]
    fn test_component_helpers() {
        let mut metrics = SimulationMetrics::new();
        
        metrics.record_component_counter("server1", "requests", 10);
        metrics.record_component_gauge("server1", "queue_depth", 5.0);
        metrics.record_component_latency("server1", "latency", Duration::from_millis(50));
        
        let stats = metrics.get_latency_stats("latency").unwrap();
        assert_eq!(stats.count, 1);
    }

    #[test]
    fn test_helper_functions() {
        let mut metrics = SimulationMetrics::new();
        
        helpers::record_request_processed(&mut metrics, "server1", true, Duration::from_millis(100));
        helpers::record_queue_depth(&mut metrics, "server1", 5);
        helpers::record_server_utilization(&mut metrics, "server1", 8, 10);
        helpers::record_throughput(&mut metrics, "server1", 150.5);
        
        // Test that metrics were recorded
        assert!(metrics.get_counter("requests_processed_total", &[("component", "server1"), ("status", "success")]).is_some());
        assert_eq!(metrics.get_gauge("queue_depth", &[("component", "server1")]), Some(5.0));
        assert_eq!(metrics.get_gauge("server_utilization", &[("component", "server1")]), Some(0.8));
        assert_eq!(metrics.get_gauge("throughput_rps", &[("component", "server1")]), Some(150.5));
    }
}