//! A `metrics::Recorder` implementation backed by `SimulationMetrics`.
//!
//! This lets other crates (des-tower, des-tonic, des-axum, etc.) emit standard
//! `metrics` counters/gauges/histograms and have them collected into an in-memory
//! `SimulationMetrics` instance.
//!
//! In most simulation harnesses, prefer a *local* recorder to avoid global state:
//!
//! ```rust,no_run
//! # use std::sync::{Arc, Mutex};
//! # use descartes_metrics::{SimulationMetrics, with_simulation_metrics_recorder};
//! let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));
//! with_simulation_metrics_recorder(&metrics, || {
//!     metrics::counter!("requests_total", "service" => "demo").increment(1);
//! });
//! assert_eq!(metrics.lock().unwrap().get_counter("requests_total", &[("service", "demo")]), Some(1));
//! ```

use crate::simulation_metrics::SimulationMetrics;
use metrics::{Counter, Gauge, Histogram, Key, KeyName, Metadata, Recorder, SharedString, Unit};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct SimulationRecorder {
    metrics: Arc<Mutex<SimulationMetrics>>,
}

impl SimulationRecorder {
    pub fn new(metrics: Arc<Mutex<SimulationMetrics>>) -> Self {
        Self { metrics }
    }
}

pub fn with_simulation_metrics_recorder<T>(
    metrics: &Arc<Mutex<SimulationMetrics>>,
    f: impl FnOnce() -> T,
) -> T {
    let recorder = SimulationRecorder::new(metrics.clone());
    metrics::with_local_recorder(&recorder, f)
}

struct CounterHandle {
    metrics: Arc<Mutex<SimulationMetrics>>,
    name: String,
    labels: Vec<(String, String)>,
}

impl metrics::CounterFn for CounterHandle {
    fn increment(&self, value: u64) {
        let mut m = self
            .metrics
            .lock()
            .expect("SimulationMetrics mutex poisoned");
        m.increment_counter_by_owned(&self.name, value, &self.labels);
    }

    fn absolute(&self, value: u64) {
        let mut m = self
            .metrics
            .lock()
            .expect("SimulationMetrics mutex poisoned");
        m.set_counter_absolute_owned(&self.name, value, &self.labels);
    }
}

struct GaugeHandle {
    metrics: Arc<Mutex<SimulationMetrics>>,
    name: String,
    labels: Vec<(String, String)>,
}

impl metrics::GaugeFn for GaugeHandle {
    fn increment(&self, value: f64) {
        let mut m = self
            .metrics
            .lock()
            .expect("SimulationMetrics mutex poisoned");
        m.increment_gauge_owned(&self.name, value, &self.labels);
    }

    fn decrement(&self, value: f64) {
        let mut m = self
            .metrics
            .lock()
            .expect("SimulationMetrics mutex poisoned");
        m.increment_gauge_owned(&self.name, -value, &self.labels);
    }

    fn set(&self, value: f64) {
        let mut m = self
            .metrics
            .lock()
            .expect("SimulationMetrics mutex poisoned");
        m.record_gauge_owned(&self.name, value, &self.labels);
    }
}

struct HistogramHandle {
    metrics: Arc<Mutex<SimulationMetrics>>,
    name: String,
    labels: Vec<(String, String)>,
}

impl metrics::HistogramFn for HistogramHandle {
    fn record(&self, value: f64) {
        let mut m = self
            .metrics
            .lock()
            .expect("SimulationMetrics mutex poisoned");
        m.record_histogram_owned(&self.name, value, &self.labels);
    }
}

fn key_to_owned_parts(key: &Key) -> (String, Vec<(String, String)>) {
    let name = key.name().to_string();
    let labels = key
        .labels()
        .map(|l| (l.key().to_string(), l.value().to_string()))
        .collect::<Vec<_>>();
    (name, labels)
}

impl Recorder for SimulationRecorder {
    fn describe_counter(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn describe_gauge(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn describe_histogram(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn register_counter(&self, key: &Key, _metadata: &Metadata<'_>) -> Counter {
        let (name, labels) = key_to_owned_parts(key);
        Counter::from_arc(Arc::new(CounterHandle {
            metrics: self.metrics.clone(),
            name,
            labels,
        }))
    }

    fn register_gauge(&self, key: &Key, _metadata: &Metadata<'_>) -> Gauge {
        let (name, labels) = key_to_owned_parts(key);
        Gauge::from_arc(Arc::new(GaugeHandle {
            metrics: self.metrics.clone(),
            name,
            labels,
        }))
    }

    fn register_histogram(&self, key: &Key, _metadata: &Metadata<'_>) -> Histogram {
        let (name, labels) = key_to_owned_parts(key);
        Histogram::from_arc(Arc::new(HistogramHandle {
            metrics: self.metrics.clone(),
            name,
            labels,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation_metrics::SimulationMetrics;
    use std::sync::{Arc, Mutex};

    #[test]
    fn recorder_captures_metrics_macros() {
        let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));

        with_simulation_metrics_recorder(&metrics, || {
            metrics::counter!("requests_total", "service" => "demo", "method" => "GET", "status" => "200")
                .increment(2);

            metrics::gauge!("queue_depth", "service" => "demo").set(7.0);
            metrics::histogram!("latency_ms", "service" => "demo").record(12.5);
        });

        let locked = metrics.lock().unwrap();
        assert_eq!(
            locked.get_counter(
                "requests_total",
                &[("method", "GET"), ("service", "demo"), ("status", "200")]
            ),
            Some(2)
        );
        assert_eq!(
            locked.get_gauge("queue_depth", &[("service", "demo")]),
            Some(7.0)
        );

        let hist = locked.get_histogram_stats("latency_ms", &[("service", "demo")]);
        assert!(hist.is_some());
        assert_eq!(hist.unwrap().count, 1);
    }
}
