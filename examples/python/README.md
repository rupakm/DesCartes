# Python Analysis Tools for DesCartes

This directory contains Python tools and templates for analyzing metrics exported from DesCartes simulations.

## Setup

### Install Dependencies

```bash
cd examples/python
pip install -r requirements.txt
```

Or using a virtual environment:

```bash
python -m venv venv
source venv/bin/activate  # On Windows: venv\\Scripts\\activate
pip install -r requirements.txt
```

## Tools

### 1. `visualize.py` - Metrics Visualization

Generate visualizations from exported metrics.

**Basic Usage:**
```bash
python visualize.py path/to/metrics.json
```

**Options:**
```bash
# Specify output directory
python visualize.py metrics.json --output my_plots/

# Load from CSV format
python visualize.py metrics.csv --format csv

# Print summary only (no plots)
python visualize.py metrics.json --summary
```

**Python API:**
```python
from visualize import load_metrics, plot_latency_distribution

# Load metrics
metrics = load_metrics('metrics.json')

# Generate individual plots
plot_latency_distribution(metrics, 'latency.png')
plot_throughput(metrics, 'throughput.png')
plot_percentile_comparison(metrics, 'percentiles.png')
plot_request_stats(metrics, 'request_stats.png')

# Generate all plots
from visualize import generate_all_plots
generate_all_plots(metrics, 'output/')
```

**Generated Plots:**
- `latency_distribution.png` - Latency percentiles (mean, P50, P95, P99)
- `throughput.png` - Counter metrics sorted by value
- `percentile_comparison.png` - P50/P95/P99 comparison across metrics
- `request_stats.png` - Request tracker statistics dashboard

### 2. `compare_runs.py` - Multi-Run Comparison

Compare metrics from multiple simulation runs to identify performance changes.

**Basic Usage:**
```bash
python compare_runs.py baseline.json optimized.json
```

**With Custom Labels:**
```bash
python compare_runs.py run1.json run2.json run3.json \\
    --labels "Baseline" "Optimized v1" "Optimized v2"
```

**Options:**
```bash
# Specify output directory
python compare_runs.py baseline.json optimized.json --output comparison_results/

# Compare CSV exports
python compare_runs.py baseline.csv optimized.csv --format csv
```

**Generated Outputs:**
- `latency_comparison.png` - Mean and P99 latency comparison
- `throughput_comparison.png` - Goodput and throughput comparison
- `success_rate_comparison.png` - Success/error/timeout rate comparison
- Statistical comparison printed to console

### 3. Jupyter Notebooks

Interactive analysis notebooks (coming soon):
- `analyze_simulation.ipynb` - Template for exploratory analysis
- `compare_simulations.ipynb` - Interactive multi-run comparison

## Examples

### Example 1: Analyze Single Run

```bash
# Run a simulation and export metrics
cd ../..
cargo run --example export_metrics_example

# Visualize results
cd examples/python
python visualize.py ../../target/example_metrics.json --output viz_output/
```

### Example 2: Compare Two Runs

```bash
# Run baseline simulation
cargo run --example export_metrics_example
cp target/example_metrics.json target/baseline.json

# Make changes and run optimized version
# ... modify simulation ...
cargo run --example export_metrics_example
cp target/example_metrics.json target/optimized.json

# Compare runs
cd examples/python
python compare_runs.py \\
    ../../target/baseline.json \\
    ../../target/optimized.json \\
    --labels "Baseline" "Optimized"
```

### Example 3: Custom Analysis

```python
import pandas as pd
from visualize import load_metrics
import matplotlib.pyplot as plt

# Load metrics
metrics = load_metrics('metrics.json')

# Extract histogram data
if 'snapshot' in metrics:
    histograms = pd.DataFrame(metrics['snapshot']['histograms'])
else:
    histograms = pd.read_csv('metrics_histograms.csv')

# Custom analysis
latency_metrics = histograms[histograms['name'].str.contains('latency')]

# Calculate tail latency ratio (P99/P50)
if 'stats' in histograms.columns:
    histograms['tail_ratio'] = histograms['stats'].apply(
        lambda s: s['p99'] / s['median'] if s['median'] > 0 else 0
    )
else:
    histograms['tail_ratio'] = histograms['p99'] / histograms['median']

print("Tail Latency Ratios:")
print(histograms[['name', 'tail_ratio']].sort_values('tail_ratio', ascending=False))

# Plot custom metric
plt.figure(figsize=(12, 6))
plt.bar(range(len(histograms)), histograms['tail_ratio'])
plt.xticks(range(len(histograms)), histograms['name'], rotation=45, ha='right')
plt.ylabel('P99/P50 Ratio')
plt.title('Tail Latency Analysis')
plt.tight_layout()
plt.savefig('tail_latency_analysis.png', dpi=300)
```

## Data Formats

### JSON Format

Exported with `metrics.export_json()`:
```json
{
  "snapshot": {
    "counters": [
      {"name": "requests_sent", "labels": {"component": "client1"}, "value": 100}
    ],
    "gauges": [...],
    "histograms": [
      {
        "name": "response_time_ms",
        "labels": {"component": "server1"},
        "stats": {
          "count": 100,
          "mean": 25.3,
          "median": 24.1,
          "p95": 38.2,
          "p99": 42.5
        }
      }
    ]
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

### CSV Format

Exported with `metrics.export_csv()` creates multiple files:

**counters.csv:**
```csv
metric_name,labels,value
requests_sent,component=client1,100
```

**histograms.csv:**
```csv
metric_name,labels,count,sum,min,max,mean,median,p95,p99
response_time_ms,component=server1,100,2530.0,10.5,45.2,25.3,24.1,38.2,42.5
```

## Advanced Usage

### Statistical Analysis

```python
import pandas as pd
from scipy import stats

# Load multiple runs
baseline = pd.read_csv('baseline_histograms.csv')
optimized = pd.read_csv('optimized_histograms.csv')

# Perform t-test on mean latencies
baseline_means = baseline['mean']
optimized_means = optimized['mean']

t_stat, p_value = stats.ttest_ind(baseline_means, optimized_means)
print(f"T-statistic: {t_stat:.3f}, p-value: {p_value:.4f}")

if p_value < 0.05:
    print("Statistically significant difference!")
```

### Interactive Dashboards with Streamlit

Create `dashboard.py`:
```python
import streamlit as st
from visualize import load_metrics, plot_latency_distribution

st.title("DesCartes Simulation Dashboard")

uploaded_file = st.file_uploader("Upload metrics.json", type=['json'])
if uploaded_file:
    metrics = load_metrics(uploaded_file)

    st.header("Summary")
    st.metric("Total Counters", len(metrics['snapshot']['counters']))
    st.metric("Total Gauges", len(metrics['snapshot']['gauges']))

    st.header("Latency Distribution")
    plot_latency_distribution(metrics)
    st.pyplot()
```

Run with:
```bash
streamlit run dashboard.py
```

## Tips

1. **Large Datasets**: For very large simulations, use CSV format and load only needed columns:
   ```python
   df = pd.read_csv('metrics_histograms.csv', usecols=['metric_name', 'mean', 'p99'])
   ```

2. **Memory Efficiency**: Process metrics in chunks if memory is limited:
   ```python
   for chunk in pd.read_csv('large_metrics.csv', chunksize=1000):
       # Process chunk
       ...
   ```

3. **Custom Plots**: Use seaborn for advanced statistical visualizations:
   ```python
   import seaborn as sns
   sns.violinplot(data=df, x='component', y='latency')
   ```

## Troubleshooting

**Issue**: `ModuleNotFoundError: No module named 'plotly'`
**Solution**: Install dependencies: `pip install -r requirements.txt`

**Issue**: Plots not displaying
**Solution**: If running in a script, ensure you call `plt.show()` or save to file

**Issue**: JSON parsing error
**Solution**: Verify the metrics file was exported correctly and is valid JSON

## Next Steps

- See `../python/notebooks/` for Jupyter notebook templates (coming soon)
- Check `des-metrics/EXPORT_README.md` for export format details
- View `des-viz/DESIGN.md` for visualization strategy overview
