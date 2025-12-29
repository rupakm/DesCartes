//! Simulation-specific metrics collection using standard metrics ecosystem
//!
//! This module provides a thin wrapper around the standard `metrics` crate,
//! adding simulation-specific functionality while leveraging battle-tested
//! metrics infrastructure.

use crate::request_tracker::{RequestTracker, RequestTrackerStats};
use des_core::SimTime;
use hdrhistogram::Histogram as HdrHistogram;
use metrics::{counter, gauge, histogram};
use std::collections::HashMap;
use std::time::Duration;

/// Simulation-specific metrics collector that wraps the standard metrics ecosystem
#[derive(Debug)]
pub struct SimulationMetrics {
    /// Request and attempt tracking for simulation analysis
    request_tracker: RequestTracker,
    /// High-resolution histograms for latency analysis
    latency_histograms: HashMap<String, HdrHistogram<u64>>,
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
            start_time: SimTime::zero(),
        }
    }

    /// Create a new simulation metrics collector with limited storage
    pub fn with_max_completed(max_completed: usize) -> Self {
        Self {
            request_tracker: RequestTracker::with_max_completed(max_completed),
            latency_histograms: HashMap::new(),
            start_time: SimTime::zero(),
        }
    }

    /// Set the simulation start time for relative timing calculations
    pub fn set_start_time(&mut self, start_time: SimTime) {
        self.start_time = start_time;
    }

    /// Record a counter increment using the standard metrics crate
    pub fn increment_counter(&self, name: &str, labels: &[(&str, &str)]) {
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
    pub fn increment_counter_by(&self, name: &str, value: u64, labels: &[(&str, &str)]) {
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
    pub fn record_gauge(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
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
    pub fn record_histogram(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
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
    pub fn record_duration(&self, name: &str, duration: Duration, labels: &[(&str, &str)]) {
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
    }

    /// Record a component-specific metric with automatic labeling
    pub fn record_component_counter(&self, component: &str, metric: &str, value: u64) {
        self.increment_counter_by(metric, value, &[("component", component)]);
    }

    /// Record a component-specific gauge with automatic labeling
    pub fn record_component_gauge(&self, component: &str, metric: &str, value: f64) {
        self.record_gauge(metric, value, &[("component", component)]);
    }

    /// Record a component-specific latency with automatic labeling
    pub fn record_component_latency(&mut self, component: &str, metric: &str, latency: Duration) {
        self.record_latency(metric, latency, &[("component", component)]);
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
        metrics: &SimulationMetrics,
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
    pub fn record_queue_depth(metrics: &SimulationMetrics, component: &str, depth: usize) {
        metrics.record_gauge(
            "queue_depth",
            depth as f64,
            &[("component", component)],
        );
    }

    /// Record server utilization
    pub fn record_server_utilization(
        metrics: &SimulationMetrics,
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
        metrics: &SimulationMetrics,
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
        let metrics = SimulationMetrics::new();
        
        helpers::record_request_processed(&metrics, "server1", true, Duration::from_millis(100));
        helpers::record_queue_depth(&metrics, "server1", 5);
        helpers::record_server_utilization(&metrics, "server1", 8, 10);
        helpers::record_throughput(&metrics, "server1", 150.5);
    }
}