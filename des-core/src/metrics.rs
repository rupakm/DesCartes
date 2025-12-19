//! Metrics collection and reporting for discrete event simulation
//!
//! This module provides a unified interface for collecting metrics from simulation
//! components using the `metrics` crate. It supports counters, gauges, and histograms
//! with proper labeling and timestamping for simulation analysis.

use crate::SimTime;
use metrics::{counter, gauge, histogram, Key, Label};
use std::collections::HashMap;
use std::time::Duration;

/// A metric value with metadata for simulation analysis
#[derive(Debug, Clone)]
pub struct MetricValue {
    /// The metric key (name and labels)
    pub key: String,
    /// The metric type
    pub metric_type: MetricType,
    /// The value of the metric
    pub value: f64,
    /// Simulation time when the metric was recorded
    pub timestamp: SimTime,
    /// Optional labels for the metric
    pub labels: HashMap<String, String>,
}

/// Types of metrics supported
#[derive(Debug, Clone, PartialEq)]
pub enum MetricType {
    /// Monotonically increasing counter
    Counter,
    /// Value that can go up or down
    Gauge,
    /// Distribution of values over time
    Histogram,
}

/// Trait for components that can emit metrics
pub trait MetricEmitter {
    /// Emit all current metrics for this component
    fn emit_metrics(&self, timestamp: SimTime) -> Vec<MetricValue>;
    
    /// Get the component name for metric labeling
    fn component_name(&self) -> &str;
}

/// Helper struct for building metric keys with labels
#[derive(Debug, Clone)]
pub struct MetricBuilder {
    name: String,
    labels: Vec<Label>,
}

impl MetricBuilder {
    /// Create a new metric builder with the given name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            labels: Vec::new(),
        }
    }

    /// Add a label to the metric
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.push(Label::new(key.into(), value.into()));
        self
    }

    /// Add a component label
    pub fn with_component(self, component: impl Into<String>) -> Self {
        self.with_label("component", component)
    }

    /// Build the metric key
    pub fn build(self) -> Key {
        Key::from_parts(self.name, self.labels)
    }

    /// Get the name for MetricValue
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get labels as HashMap for MetricValue
    pub fn labels_map(&self) -> HashMap<String, String> {
        self.labels
            .iter()
            .map(|label| (label.key().to_string(), label.value().to_string()))
            .collect()
    }
}

/// Simulation-specific metrics recorder that tracks values for analysis
#[derive(Debug, Default)]
pub struct SimulationMetrics {
    recorded_metrics: Vec<MetricValue>,
}

impl SimulationMetrics {
    /// Create a new simulation metrics recorder
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a counter increment
    pub fn increment_counter(&mut self, name: impl Into<String>, component: impl Into<String>, timestamp: SimTime) {
        let name_str = name.into();
        let component_str = component.into();
        let builder = MetricBuilder::new(&name_str).with_component(&component_str);
        
        // Increment the global counter using the name directly
        counter!(name_str.clone(), "component" => component_str.clone()).increment(1);
        
        // Record for simulation analysis
        self.recorded_metrics.push(MetricValue {
            key: name_str,
            metric_type: MetricType::Counter,
            value: 1.0,
            timestamp,
            labels: builder.labels_map(),
        });
    }

    /// Record a counter increment with a specific value
    pub fn increment_counter_by(&mut self, name: impl Into<String>, component: impl Into<String>, value: u64, timestamp: SimTime) {
        let name_str = name.into();
        let component_str = component.into();
        let builder = MetricBuilder::new(&name_str).with_component(&component_str);
        
        // Increment the global counter
        counter!(name_str.clone(), "component" => component_str.clone()).increment(value);
        
        // Record for simulation analysis
        self.recorded_metrics.push(MetricValue {
            key: name_str,
            metric_type: MetricType::Counter,
            value: value as f64,
            timestamp,
            labels: builder.labels_map(),
        });
    }

    /// Record a gauge value
    pub fn record_gauge(&mut self, name: impl Into<String>, component: impl Into<String>, value: f64, timestamp: SimTime) {
        let name_str = name.into();
        let component_str = component.into();
        let builder = MetricBuilder::new(&name_str).with_component(&component_str);
        
        // Set the global gauge
        gauge!(name_str.clone(), "component" => component_str.clone()).set(value);
        
        // Record for simulation analysis
        self.recorded_metrics.push(MetricValue {
            key: name_str,
            metric_type: MetricType::Gauge,
            value,
            timestamp,
            labels: builder.labels_map(),
        });
    }

    /// Record a histogram value (typically for latencies, processing times, etc.)
    pub fn record_histogram(&mut self, name: impl Into<String>, component: impl Into<String>, value: f64, timestamp: SimTime) {
        let name_str = name.into();
        let component_str = component.into();
        let builder = MetricBuilder::new(&name_str).with_component(&component_str);
        
        // Record in the global histogram
        histogram!(name_str.clone(), "component" => component_str.clone()).record(value);
        
        // Record for simulation analysis
        self.recorded_metrics.push(MetricValue {
            key: name_str,
            metric_type: MetricType::Histogram,
            value,
            timestamp,
            labels: builder.labels_map(),
        });
    }

    /// Record a duration as a histogram (converts to milliseconds)
    pub fn record_duration(&mut self, name: impl Into<String>, component: impl Into<String>, duration: Duration, timestamp: SimTime) {
        let millis = duration.as_secs_f64() * 1000.0;
        self.record_histogram(name, component, millis, timestamp);
    }

    /// Get all recorded metrics
    pub fn get_metrics(&self) -> &[MetricValue] {
        &self.recorded_metrics
    }

    /// Get metrics for a specific component
    pub fn get_component_metrics(&self, component: &str) -> Vec<&MetricValue> {
        self.recorded_metrics
            .iter()
            .filter(|m| m.labels.get("component").is_some_and(|c| c == component))
            .collect()
    }

    /// Get metrics of a specific type
    pub fn get_metrics_by_type(&self, metric_type: MetricType) -> Vec<&MetricValue> {
        self.recorded_metrics
            .iter()
            .filter(|m| m.metric_type == metric_type)
            .collect()
    }

    /// Clear all recorded metrics
    pub fn clear(&mut self) {
        self.recorded_metrics.clear();
    }

    /// Get summary statistics for the simulation
    pub fn summary(&self) -> MetricsSummary {
        let mut summary = MetricsSummary::default();
        
        for metric in &self.recorded_metrics {
            match metric.metric_type {
                MetricType::Counter => summary.total_counters += 1,
                MetricType::Gauge => summary.total_gauges += 1,
                MetricType::Histogram => summary.total_histograms += 1,
            }
        }
        
        summary.total_metrics = self.recorded_metrics.len();
        summary.components = self.recorded_metrics
            .iter()
            .filter_map(|m| m.labels.get("component"))
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .len();
        
        summary
    }
}

/// Summary statistics for recorded metrics
#[derive(Debug, Default)]
pub struct MetricsSummary {
    pub total_metrics: usize,
    pub total_counters: usize,
    pub total_gauges: usize,
    pub total_histograms: usize,
    pub components: usize,
}

impl std::fmt::Display for MetricsSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Metrics Summary: {} total ({} counters, {} gauges, {} histograms) from {} components",
            self.total_metrics, self.total_counters, self.total_gauges, self.total_histograms, self.components
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_metric_builder() {
        let builder = MetricBuilder::new("test_metric")
            .with_component("test_component")
            .with_label("type", "test");
        
        assert_eq!(builder.name(), "test_metric");
        
        let labels = builder.labels_map();
        assert_eq!(labels.get("component"), Some(&"test_component".to_string()));
        assert_eq!(labels.get("type"), Some(&"test".to_string()));
    }

    #[test]
    fn test_simulation_metrics_counter() {
        let mut metrics = SimulationMetrics::new();
        let timestamp = SimTime::from_duration(Duration::from_secs(1));
        
        metrics.increment_counter("requests", "client", timestamp);
        metrics.increment_counter_by("bytes", "client", 1024, timestamp);
        
        let recorded = metrics.get_metrics();
        assert_eq!(recorded.len(), 2);
        
        assert_eq!(recorded[0].metric_type, MetricType::Counter);
        assert_eq!(recorded[0].value, 1.0);
        assert_eq!(recorded[1].value, 1024.0);
    }

    #[test]
    fn test_simulation_metrics_gauge() {
        let mut metrics = SimulationMetrics::new();
        let timestamp = SimTime::from_duration(Duration::from_secs(1));
        
        metrics.record_gauge("queue_size", "server", 42.0, timestamp);
        
        let recorded = metrics.get_metrics();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].metric_type, MetricType::Gauge);
        assert_eq!(recorded[0].value, 42.0);
    }

    #[test]
    fn test_simulation_metrics_histogram() {
        let mut metrics = SimulationMetrics::new();
        let timestamp = SimTime::from_duration(Duration::from_secs(1));
        
        metrics.record_histogram("latency", "server", 123.45, timestamp);
        metrics.record_duration("processing_time", "server", Duration::from_millis(250), timestamp);
        
        let recorded = metrics.get_metrics();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].metric_type, MetricType::Histogram);
        assert_eq!(recorded[0].value, 123.45);
        assert_eq!(recorded[1].value, 250.0); // Duration converted to millis
    }

    #[test]
    fn test_component_filtering() {
        let mut metrics = SimulationMetrics::new();
        let timestamp = SimTime::from_duration(Duration::from_secs(1));
        
        metrics.increment_counter("requests", "client1", timestamp);
        metrics.increment_counter("requests", "client2", timestamp);
        metrics.record_gauge("queue_size", "server", 10.0, timestamp);
        
        let client1_metrics = metrics.get_component_metrics("client1");
        assert_eq!(client1_metrics.len(), 1);
        
        let server_metrics = metrics.get_component_metrics("server");
        assert_eq!(server_metrics.len(), 1);
        assert_eq!(server_metrics[0].metric_type, MetricType::Gauge);
    }

    #[test]
    fn test_metrics_summary() {
        let mut metrics = SimulationMetrics::new();
        let timestamp = SimTime::from_duration(Duration::from_secs(1));
        
        metrics.increment_counter("requests", "client", timestamp);
        metrics.record_gauge("queue_size", "server", 10.0, timestamp);
        metrics.record_histogram("latency", "server", 50.0, timestamp);
        
        let summary = metrics.summary();
        assert_eq!(summary.total_metrics, 3);
        assert_eq!(summary.total_counters, 1);
        assert_eq!(summary.total_gauges, 1);
        assert_eq!(summary.total_histograms, 1);
        assert_eq!(summary.components, 2);
    }
}