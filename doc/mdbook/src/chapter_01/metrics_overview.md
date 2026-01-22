# The Metrics Collection System

DESCARTES includes a comprehensive metrics collection system that helps you understand simulation behavior, validate models, and analyze performance. This overview introduces the metrics architecture and capabilities.

## Why Metrics Matter in Simulation

Simulation without measurement is just expensive computation. Metrics help you:

- **Validate Models**: Ensure your simulation behaves as expected
- **Analyze Performance**: Understand bottlenecks and system behavior
- **Compare Scenarios**: Quantify the impact of different configurations
- **Debug Issues**: Identify problems in simulation logic
- **Generate Reports**: Create visualizations and summaries for stakeholders

## Metrics Architecture Overview

DESCARTES metrics are built on three layers:

```
┌─────────────────────────────────────────────────────────────┐
│                    Visualization Layer                      │
│  Charts, Reports, Dashboards (des-viz + external tools)    │
└─────────────────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────────────────┐
│                     Export Layer                           │
│     JSON, CSV, Prometheus (des-metrics/export)             │
└─────────────────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────────────────┐
│                   Collection Layer                         │
│  Counters, Gauges, Histograms (des-metrics/core)          │
└─────────────────────────────────────────────────────────────┘
```

### Collection Layer: Core Metrics Types

DESCARTES supports three fundamental metric types:

#### Counters
Track things that only increase (requests sent, errors occurred, bytes transferred).

```rust
use des_metrics::SimulationMetrics;

let mut metrics = SimulationMetrics::new();

// Increment by 1
metrics.increment_counter("requests_sent", &[("component", "client")]);

// Increment by specific amount
metrics.increment_counter_by("bytes_sent", 1024, &[("component", "client")]);
```

#### Gauges
Track values that can go up and down (queue depth, active connections, memory usage).

```rust
// Record current queue depth
metrics.record_gauge("queue_depth", 15.0, &[("component", "server")]);

// Record server utilization as percentage
metrics.record_gauge("server_utilization", 0.85, &[("component", "server")]);
```

#### Histograms
Track distributions of values (response times, request sizes, processing delays).

```rust
use std::time::Duration;

// Record response time
metrics.record_histogram("response_time_ms", 125.5, &[("component", "server")]);

// Record duration (automatically converts to milliseconds)
metrics.record_duration("processing_time", Duration::from_millis(50), &[("component", "server")]);

// High-resolution latency tracking
metrics.record_latency("request_latency", Duration::from_micros(1250), &[("component", "server")]);
```

### Labels for Dimensional Metrics

All metrics support **labels** for multi-dimensional analysis:

```rust
// Same metric name, different components
metrics.increment_counter("requests_processed", &[("component", "web_server")]);
metrics.increment_counter("requests_processed", &[("component", "db_server")]);
metrics.increment_counter("requests_processed", &[("component", "cache_server")]);

// Multiple labels for fine-grained analysis
metrics.increment_counter("requests_processed", &[
    ("component", "web_server"),
    ("status", "success"),
    ("method", "GET")
]);
```

## Specialized Simulation Metrics

### Request Tracking

DESCARTES includes specialized request tracking for simulation analysis:

```rust
// Access the built-in request tracker
let request_stats = metrics.get_request_stats(Duration::from_secs(60));
println!("Requests in last 60s: {}", request_stats.total_requests);
println!("Average latency: {:.2}ms", request_stats.avg_latency.as_secs_f64() * 1000.0);
```

### High-Resolution Latency Analysis

For detailed latency analysis, DESCARTES uses HdrHistogram for microsecond precision:

```rust
// Record high-resolution latencies
for i in 0..1000 {
    let latency = Duration::from_micros(1000 + i * 10); // 1ms to 11ms
    metrics.record_latency("api_latency", latency, &[("endpoint", "/users")]);
}

// Get detailed statistics
if let Some(stats) = metrics.get_latency_stats("api_latency") {
    println!("Latency Analysis:");
    println!("  Count: {}", stats.count);
    println!("  Mean: {:.2}ms", stats.mean.as_secs_f64() * 1000.0);
    println!("  P95: {:.2}ms", stats.p95.as_secs_f64() * 1000.0);
    println!("  P99: {:.2}ms", stats.p99.as_secs_f64() * 1000.0);
    println!("  P99.9: {:.2}ms", stats.p999.as_secs_f64() * 1000.0);
}
```

### Time-Series Metrics

For tracking metrics over time, DESCARTES provides time-series collection:

```rust
use des_metrics::MmkTimeSeriesMetrics;
use std::sync::{Arc, Mutex};

// Create time-series collector with 100ms aggregation window
let time_series = Arc::new(Mutex::new(MmkTimeSeriesMetrics::new(
    Duration::from_millis(100),
    0.1, // Exponential moving average alpha
)));

// Record values over time
{
    let mut ts = time_series.lock().unwrap();
    ts.record_latency(SimTime::from_secs(1), 50.0); // 50ms latency at t=1s
    ts.record_queue_size(SimTime::from_secs(1), 10.0); // 10 items in queue
    ts.record_utilization(SimTime::from_secs(1), 0.75); // 75% utilization
}
```

## Export and Analysis

### JSON Export

Export all metrics to JSON for analysis with external tools:

```rust
// Export with pretty formatting
metrics.export_json("results/simulation_metrics.json", true)?;
```

Example output:
```json
{
  "counters": [
    {
      "name": "requests_processed",
      "labels": {"component": "server", "status": "success"},
      "value": 1250
    }
  ],
  "gauges": [
    {
      "name": "queue_depth",
      "labels": {"component": "server"},
      "value": 8.5
    }
  ],
  "histograms": [
    {
      "name": "response_time_ms",
      "labels": {"component": "server"},
      "stats": {
        "count": 1000,
        "mean": 125.5,
        "p95": 245.2,
        "p99": 387.1
      }
    }
  ]
}
```

### CSV Export

Export metrics to CSV files for spreadsheet analysis:

```rust
// Creates multiple CSV files: metrics_counters.csv, metrics_gauges.csv, etc.
metrics.export_csv("results/metrics.csv", true)?;
```

### Prometheus Integration

For production monitoring, DESCARTES integrates with Prometheus:

```rust
use des_metrics::setup_prometheus_metrics;

// Start Prometheus metrics server on port 9090
setup_prometheus_metrics(9090)?;

// Your metrics are now available at http://localhost:9090/metrics
```

## Visualization Capabilities

DESCARTES includes built-in visualization through the `des-viz` crate:

### Time-Series Charts

```rust
use des_viz::charts::time_series::{TimeSeries, create_mmk_time_series_chart};
use plotters::prelude::*;

// Create time series from collected data
let latency_series = TimeSeries::new("Response Time", latency_data, RED);
let queue_series = TimeSeries::new("Queue Depth", queue_data, BLUE);

// Generate chart
create_mmk_time_series_chart(
    &latency_series,
    &queue_series,
    &throughput_series,
    "output/simulation_chart.png",
)?;
```

### Latency Distribution Charts

```rust
use des_viz::charts::latency::create_latency_distribution_chart;

// Create latency distribution chart
create_latency_distribution_chart(
    &latency_data,
    "Response Time Distribution",
    "output/latency_distribution.png",
)?;
```

## Common Metrics Patterns

### Component-Level Metrics

Use consistent labeling for component-level analysis:

```rust
// Helper methods for component metrics
metrics.record_component_counter("web_server", "requests_processed", 1);
metrics.record_component_gauge("web_server", "active_connections", 25.0);
metrics.record_component_latency("web_server", "request_latency", Duration::from_millis(150));
```

### Request Processing Metrics

Track the full request lifecycle:

```rust
use des_metrics::helpers;

// Record successful request processing
helpers::record_request_processed(
    &mut metrics,
    "api_server",
    true, // success
    Duration::from_millis(125),
);

// Record server utilization
helpers::record_server_utilization(
    &mut metrics,
    "api_server",
    8, // active requests
    10, // capacity
);

// Record queue depth
helpers::record_queue_depth(&mut metrics, "api_server", 3);

// Record throughput
helpers::record_throughput(&mut metrics, "api_server", 150.5); // requests/second
```

### Error Tracking

Track different types of errors:

```rust
// Network errors
metrics.increment_counter("errors_total", &[
    ("component", "client"),
    ("type", "network"),
    ("code", "timeout")
]);

// Application errors
metrics.increment_counter("errors_total", &[
    ("component", "server"),
    ("type", "application"),
    ("code", "validation_failed")
]);
```

## Integration with Simulation Components

### Automatic Metrics in Built-in Components

DESCARTES components automatically collect metrics:

```rust
use des_components::{Server, SimpleClient};

// Server automatically tracks:
// - requests_processed_total
// - requests_rejected_total  
// - server_utilization
// - queue_depth
// - processing_time_ms

let server = Server::new("web_server", 10)
    .with_metrics(metrics.clone()); // Enable automatic metrics
```

### Custom Component Metrics

Add metrics to your custom components:

```rust
use des_core::{Component, Scheduler, SimTime};
use des_metrics::SimulationMetrics;
use std::sync::{Arc, Mutex};

struct MyComponent {
    name: String,
    metrics: Arc<Mutex<SimulationMetrics>>,
    requests_processed: u64,
}

impl Component for MyComponent {
    type Event = MyEvent;
    
    fn process_event(&mut self, event: &MyEvent, scheduler: &mut Scheduler) {
        match event {
            MyEvent::ProcessRequest => {
                self.requests_processed += 1;
                
                // Record metrics
                {
                    let mut metrics = self.metrics.lock().unwrap();
                    metrics.increment_counter("requests_processed", &[
                        ("component", &self.name)
                    ]);
                    metrics.record_gauge("total_processed", 
                        self.requests_processed as f64, &[
                        ("component", &self.name)
                    ]);
                }
            }
        }
    }
}
```

## Best Practices

### Metric Naming

Use consistent, descriptive names:

```rust
// Good: Descriptive and consistent
"requests_processed_total"
"response_time_ms"
"queue_depth_current"
"server_utilization_percent"

// Avoid: Vague or inconsistent
"count"
"time"
"stuff"
"data"
```

### Label Usage

Use labels for dimensions, not for high-cardinality data:

```rust
// Good: Low cardinality labels
&[("component", "server"), ("status", "success")]
&[("method", "GET"), ("endpoint", "/api/users")]

// Avoid: High cardinality (creates too many metric series)
&[("user_id", "12345"), ("session_id", "abcdef")]
&[("request_id", "req_789")]
```

### Performance Considerations

- Metrics collection has minimal overhead, but avoid excessive labeling
- Use time-series metrics for trending, not for every data point
- Export metrics periodically, not after every event
- Consider sampling for very high-frequency events

## Next Steps

Now that you understand the metrics system:

1. **Try the examples**: Run simulations with metrics collection enabled
2. **Learn visualization**: Explore the [des-viz documentation](../chapter_04/README.md)
3. **Build dashboards**: Set up Prometheus and Grafana for real-time monitoring
4. **Analyze results**: Use exported data with Python, R, or other analysis tools

The metrics system is designed to be both powerful and easy to use, giving you deep insights into your simulation behavior without requiring complex setup or configuration.