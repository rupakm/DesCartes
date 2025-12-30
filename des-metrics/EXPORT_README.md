# Metrics Export Guide

This guide explains how to export simulation metrics from DesCartes for visualization and analysis.

## Quick Start

```rust
use des_metrics::SimulationMetrics;

let metrics = SimulationMetrics::new();
// ... collect metrics during simulation ...

// Export to JSON (pretty-printed)
metrics.export_json("results/metrics.json", true)?;

// Export to CSV (with labels)
metrics.export_csv("results/metrics.csv", true)?;
```

## Export Formats

### JSON Export

JSON export creates a single file with all metrics in structured format.

**Features:**
- Complete metrics snapshot including counters, gauges, and histograms
- Request tracker statistics (goodput, throughput, retry rates, etc.)
- High-resolution latency statistics
- Pretty-print option for human readability

**Example:**
```rust
// Pretty-printed JSON (readable)
metrics.export_json("results/metrics.json", true)?;

// Compact JSON (smaller file size)
metrics.export_json("results/metrics.json", false)?;
```

**Output structure:**
```json
{
  "snapshot": {
    "counters": [
      {
        "name": "requests_sent",
        "labels": {"component": "client1"},
        "value": 100
      }
    ],
    "gauges": [...],
    "histograms": [...]
  },
  "request_stats": {
    "goodput": 150.5,
    "throughput": 200.0,
    "success_rate": 0.95,
    ...
  },
  "latency_stats": [...]
}
```

### CSV Export

CSV export creates multiple files for different metric types, suitable for spreadsheet analysis and pandas.

**Features:**
- Separate CSV file for each metric type
- Optional label columns
- Easy to load in Excel, pandas, R, etc.

**Example:**
```rust
// Export with labels
metrics.export_csv("results/metrics.csv", true)?;

// Export without labels (simpler format)
metrics.export_csv("results/metrics.csv", false)?;
```

**Generated files:**
- `metrics_counters.csv` - Counter metrics
- `metrics_gauges.csv` - Gauge metrics
- `metrics_histograms.csv` - Histogram statistics (count, mean, p95, p99, etc.)
- `metrics_request_stats.csv` - Request tracker statistics

**Example CSV (counters):**
```csv
metric_name,labels,value
requests_sent,component=client1,100
requests_sent,component=client2,100
responses_success,component=server1,95
```

**Example CSV (histograms):**
```csv
metric_name,labels,count,sum,min,max,mean,median,p95,p99
response_time_ms,component=server1,100,5234.5,10.5,85.2,52.3,50.1,78.5,82.3
```

## Analysis Workflows

### Python with pandas

```python
import pandas as pd
import matplotlib.pyplot as plt

# Load JSON
import json
with open('results/metrics.json') as f:
    data = json.load(f)

# Or load CSV
counters = pd.read_csv('results/metrics_counters.csv')
gauges = pd.read_csv('results/metrics_gauges.csv')
histograms = pd.read_csv('results/metrics_histograms.csv')

# Analyze
print(counters[counters['metric_name'] == 'requests_sent'])
print(histograms[['metric_name', 'mean', 'p95', 'p99']])

# Plot
histograms.plot(x='metric_name', y=['mean', 'p95', 'p99'], kind='bar')
plt.show()
```

### Python with Plotly (interactive)

```python
import plotly.express as px
import pandas as pd

# Load histogram data
df = pd.read_csv('results/metrics_histograms.csv')

# Create interactive bar chart
fig = px.bar(df, x='metric_name', y='mean', error_y='p99',
             title='Latency Statistics')
fig.show()
```

### R

```r
# Load CSV files
counters <- read.csv('results/metrics_counters.csv')
histograms <- read.csv('results/metrics_histograms.csv')

# Analyze
summary(histograms)

# Plot
library(ggplot2)
ggplot(histograms, aes(x=metric_name, y=mean)) +
  geom_bar(stat='identity') +
  geom_errorbar(aes(ymin=p95, ymax=p99))
```

### Excel / Google Sheets

1. Open the CSV files directly in Excel or Google Sheets
2. Use pivot tables to aggregate by labels
3. Create charts for visualization

## Advanced Usage

### Custom Export Using Exporters

For more control, use the exporter classes directly:

```rust
use des_metrics::export::{JsonExporter, CsvExporter, MetricsExporter};

// JSON exporter
let exporter = JsonExporter::new(Path::new("output.json"), true);
exporter.export(&metrics)?;

// CSV exporter
let exporter = CsvExporter::new(Path::new("output.csv"), false);
exporter.export(&metrics)?;
```

### Accessing Metrics Programmatically

Before exporting, you can access metrics directly:

```rust
// Get specific metrics
let counter = metrics.get_counter("requests_sent", &[("component", "client1")]);
let gauge = metrics.get_gauge("queue_depth", &[("component", "server1")]);
let histogram = metrics.get_histogram_stats("response_time_ms", &[("component", "server1")]);

// Get complete snapshot
let snapshot = metrics.get_metrics_snapshot();
println!("Total counters: {}", snapshot.counters.len());
println!("Total gauges: {}", snapshot.gauges.len());

// Get request statistics
use std::time::Duration;
let stats = metrics.get_request_stats(Duration::from_secs(3600));
println!("Goodput: {:.2} req/s", stats.goodput);
println!("Success rate: {:.1}%", stats.success_rate * 100.0);

// Get latency statistics
if let Some(latency) = metrics.get_latency_stats("service_latency") {
    println!("P99 latency: {:.2}ms", latency.p99.as_secs_f64() * 1000.0);
}
```

### Exporting at Different Points

You can export metrics multiple times during a simulation:

```rust
// Export after warmup phase
metrics.export_json("results/warmup_metrics.json", true)?;

// Continue simulation...

// Export after steady state
metrics.export_json("results/steady_state_metrics.json", true)?;

// Compare the two exports to analyze phase differences
```

## Examples

### Complete Example

See `des-components/examples/export_metrics_example.rs` for a complete working example:

```bash
cargo run --example export_metrics_example
```

This example demonstrates:
- Collecting various metric types (counters, gauges, histograms)
- Recording high-resolution latency data
- Exporting to both JSON and CSV
- Accessing and displaying metrics statistics

### Integration with Simulation

```rust
use des_core::{Simulation, SimTime};
use des_metrics::SimulationMetrics;

fn run_simulation() -> Result<(), Box<dyn std::error::Error>> {
    let mut sim = Simulation::new();
    let mut metrics = SimulationMetrics::new();

    // Run simulation
    while sim.now() < SimTime::from_secs(100) {
        // ... simulation logic ...

        // Record metrics
        metrics.increment_counter("events_processed", &[]);
        metrics.record_gauge("queue_size", sim.queue_size() as f64, &[]);
    }

    // Export results
    metrics.export_json("results/simulation_metrics.json", true)?;
    metrics.export_csv("results/simulation_metrics.csv", true)?;

    Ok(())
}
```

## Troubleshooting

### File Not Created

Ensure the output directory exists:
```rust
std::fs::create_dir_all("results")?;
metrics.export_json("results/metrics.json", true)?;
```

### Large File Sizes

For large simulations with many metrics:
- Use compact JSON: `export_json(path, false)`
- Use CSV without labels: `export_csv(path, false)`
- Consider exporting periodically and aggregating offline

### Memory Usage

If collecting too many metrics causes memory issues, use limited storage:
```rust
// Keep only last 1000 completed requests/attempts
let metrics = SimulationMetrics::with_max_completed(1000);
```

## Next Steps

- See `des-viz/DESIGN.md` for visualization strategies
- Explore Python examples in `examples/python/` (coming soon)
- Read the API documentation: `cargo doc --open -p des-metrics`
