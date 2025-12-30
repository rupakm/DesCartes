# DesCartes Visualization Strategy

## Executive Summary

This document outlines the visualization strategy for the DesCartes discrete-event simulator. Based on analysis of the current metrics system, we recommend a **tiered approach** that starts with foundational data export capabilities and progressively adds visualization layers based on user needs.

**Key Recommendation**: Implement flexible data export first (JSON/CSV), then provide both native Rust visualizations (via plotters) and Python analysis templates to serve different user workflows.

---

## Current State Analysis

### Metrics Infrastructure

The DesCartes simulator has a comprehensive metrics collection system:

**Data Types Collected**:
- **Counters**: Requests sent/received, successes/failures, timeouts, retries
- **Gauges**: Queue depth, active threads, utilization, throughput (RPS)
- **Histograms**: Response time, request duration, service time distributions
- **High-Resolution Latency**: Microsecond-precision using hdrhistogram (p50, p95, p99, p999)
- **Request Tracking**: Complete lifecycle data with attempts, statuses, timing

**Export Readiness**:
- `MetricsSnapshot` struct with serde serialization
- Retrievable via `get_metrics_snapshot()`, `list_counters()`, `list_gauges()`, etc.
- **Gap**: No file export functions implemented yet

**Existing Infrastructure**:
- des-viz crate exists with plotters 0.3 library integrated
- des-metrics provides rich data collection
- Examples show phase-based workload analysis patterns

### Data Characteristics

Simulations generate several types of data ideal for visualization:

1. **Time-Series Data**: Latency, throughput, queue depth over SimTime
2. **Statistical Distributions**: Pre-computed percentiles (p50, p95, p99, p999)
3. **Multi-Dimensional Metrics**: Labeled by component, service, status
4. **Phase-Based Analytics**: Workload phases with aggregate metrics
5. **Rate Metrics**: Goodput, throughput, retry rates, error rates

---

## Visualization Options Analysis

### Option 1: Native Rust Visualization

#### Approach A: Plotters (Static Charts)

**What it is**: Pure Rust plotting library, already integrated in des-viz

**Implementation Pattern**:
```rust
// Export static charts after simulation
fn export_latency_chart(snapshot: &MetricsSnapshot) -> Result<()> {
    let root = BitMapBackend::new("latency.png", (1024, 768)).into_drawing_area();
    // Build time-series chart
}
```

**Pros**:
- ✅ Zero additional dependencies (already integrated)
- ✅ Pure Rust - no foreign language interop
- ✅ Generates PNG, SVG, or HTML output
- ✅ Can embed charts in reports
- ✅ Works with existing data structures

**Cons**:
- ❌ Static visualizations only (no interactivity)
- ❌ Limited styling compared to D3.js/Plotly
- ❌ Manual chart building (more code per visualization)
- ❌ No dashboard/multiple chart orchestration

**Best for**: Automated report generation, CI/CD pipelines, batch analysis

**Effort**: Low (2-3 days for basic charts)

---

#### Approach B: egui + egui_plot (Interactive GUI)

**What it is**: Immediate-mode GUI framework with plotting capabilities

**Implementation Pattern**:
```rust
// Real-time dashboard during simulation
fn run_dashboard(metrics: Arc<Mutex<SimulationMetrics>>) {
    eframe::run_native(/* dashboard app */);
}
```

**Pros**:
- ✅ Interactive real-time visualization
- ✅ Pure Rust ecosystem
- ✅ Can run alongside simulation
- ✅ Cross-platform (native app)

**Cons**:
- ❌ Requires GUI application (not web-based)
- ❌ Steeper learning curve for GUI programming
- ❌ Limited ecosystem compared to web
- ❌ Harder to share visualizations remotely

**Best for**: Real-time monitoring during simulation runs, desktop tools

**Effort**: Medium (1-2 weeks for dashboard)

---

### Option 2: Python Ecosystem

#### Approach A: Export to JSON/CSV + Matplotlib/Seaborn

**What it is**: Export metrics to standard formats, analyze with Python notebooks

**Implementation Pattern**:
```rust
// In des-metrics
impl SimulationMetrics {
    pub fn export_json(&self, path: &Path) -> Result<()> {
        let snapshot = self.get_metrics_snapshot();
        let json = serde_json::to_string_pretty(&snapshot)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn export_csv(&self, path: &Path) -> Result<()> {
        // Export time-series data in CSV format
    }
}
```

```python
# Python analysis script
import pandas as pd
import matplotlib.pyplot as plt
import seaborn as sns

data = pd.read_json('metrics.json')
plt.plot(data['timestamp'], data['latency'])
plt.show()
```

**Pros**:
- ✅ Mature ecosystem (matplotlib, pandas, seaborn, numpy)
- ✅ Jupyter notebooks for interactive exploration
- ✅ Extensive statistical analysis tools (scipy, statsmodels)
- ✅ Easy to create publication-quality figures
- ✅ Low implementation cost (just export functions)
- ✅ Researchers/users likely familiar with Python

**Cons**:
- ❌ Two-language workflow (Rust → Python)
- ❌ Manual data export step required
- ❌ Static visualizations (unless using notebook widgets)
- ❌ Users need Python environment setup

**Best for**: Ad-hoc analysis, research papers, exploratory data analysis

**Effort**: Low (1-2 days for export + example notebooks)

---

#### Approach B: Plotly + Dash (Interactive Web Apps)

**What it is**: Build interactive web dashboards in Python

**Implementation Pattern**:
```python
import plotly.graph_objects as go
import dash
from dash import dcc, html

app = dash.Dash(__name__)
app.layout = html.Div([
    dcc.Graph(id='latency-chart'),
    dcc.Graph(id='throughput-chart'),
])

# Load metrics from exported JSON
# Create interactive plotly charts

if __name__ == '__main__':
    app.run_server()
```

**Pros**:
- ✅ Interactive web-based dashboards
- ✅ Rich interactivity (zoom, pan, hover tooltips)
- ✅ Can combine multiple visualizations
- ✅ Shareable via web browser
- ✅ Plotly has excellent time-series support

**Cons**:
- ❌ Requires Python web server
- ❌ Not real-time (needs data export first)
- ❌ Additional Python dependencies
- ❌ Dash apps can be complex for advanced features

**Best for**: Post-simulation interactive exploration, sharing results with team

**Effort**: Medium (3-5 days for functional dashboard)

---

#### Approach C: Streamlit (Rapid Prototyping)

**What it is**: Build simple data apps with minimal code

**Implementation Pattern**:
```python
import streamlit as st
import pandas as pd
import plotly.express as px

st.title("DesCartes Simulation Metrics")

data = pd.read_json('metrics.json')

st.line_chart(data[['timestamp', 'latency']])
st.metric("Avg Latency", f"{data['latency'].mean():.2f}ms")

fig = px.histogram(data, x='latency', nbins=50)
st.plotly_chart(fig)
```

**Pros**:
- ✅ Extremely rapid development
- ✅ Built-in widgets (sliders, selects, file upload)
- ✅ Auto-reloading on code changes
- ✅ Easy deployment (Streamlit Cloud)
- ✅ Great for quick prototypes

**Cons**:
- ❌ Limited customization compared to Dash
- ❌ Top-down execution model (can be limiting)
- ❌ Performance issues with large datasets

**Best for**: Quick demos, internal tools, rapid iteration

**Effort**: Very Low (1-2 days for basic app)

---

### Option 3: Dedicated Visualization Frameworks

#### Approach A: Grafana + Prometheus/InfluxDB

**What it is**: Industry-standard observability platform

**Architecture**:
```
Rust Simulator → Prometheus Exporter → Prometheus → Grafana Dashboards
              OR
Rust Simulator → InfluxDB Line Protocol → InfluxDB → Grafana
```

**Implementation Pattern**:
```rust
// Prometheus exporter (HTTP endpoint)
use prometheus::{Encoder, TextEncoder, Registry};

impl SimulationMetrics {
    pub fn start_prometheus_server(&self, port: u16) {
        // Expose metrics on /metrics endpoint
    }
}

// OR InfluxDB writer
impl SimulationMetrics {
    pub fn write_to_influxdb(&self, client: &InfluxClient) {
        let point = DataPoint::new("simulation_latency")
            .tag("component", component_name)
            .field("value", latency_ms)
            .timestamp(timestamp);
        client.write(point)?;
    }
}
```

**Pros**:
- ✅ Industry-standard tool (widely known)
- ✅ Beautiful, professional dashboards
- ✅ Real-time monitoring capability
- ✅ Powerful query language (PromQL)
- ✅ Alerting built-in
- ✅ User-created dashboards (no coding required)
- ✅ Time-series data is primary use case
- ✅ Can handle very large datasets

**Cons**:
- ❌ Requires external infrastructure (Prometheus + Grafana servers)
- ❌ Overhead for small/quick simulations
- ❌ Prometheus pull model may not fit discrete-event sims
- ❌ Setup complexity for users
- ❌ Best suited for long-running services

**Best for**: Production monitoring, continuous simulation runs, team dashboards

**Effort**: Medium-High (1 week for integration + setup docs)

---

#### Approach B: Apache Superset (Self-Service BI)

**What it is**: Modern data exploration platform

**Architecture**:
```
Rust Simulator → Export to Database (PostgreSQL) → Superset Queries
```

**Pros**:
- ✅ SQL-based exploration (familiar to many)
- ✅ Rich visualization library
- ✅ User-created dashboards without coding
- ✅ Role-based access control
- ✅ Export to PDF/CSV
- ✅ Good for sharing with non-technical stakeholders

**Cons**:
- ❌ Requires database + Superset server
- ❌ Heavy infrastructure for small projects
- ❌ Complex setup
- ❌ Overkill for single-user scenarios

**Best for**: Multi-user organizations, business intelligence needs

**Effort**: High (2+ weeks)

---

#### Approach C: Observable Framework (Markdown + Data)

**What it is**: Framework for creating data apps with markdown + JavaScript

**Architecture**:
```
Rust Simulator → Export JSON/Parquet → Observable Markdown → Static Site
```

**Implementation Pattern**:
```markdown
# DesCartes Simulation Results

\`\`\`js
const data = FileAttachment("metrics.json").json();
\`\`\`

\`\`\`js
Plot.plot({
  marks: [
    Plot.line(data, {x: "timestamp", y: "latency"})
  ]
})
\`\`\`
```

**Pros**:
- ✅ Reactive, interactive visualizations
- ✅ Observable Plot (D3-based, excellent for time-series)
- ✅ Deployable as static site
- ✅ Markdown-based (easy to document)
- ✅ No server required once built
- ✅ Beautiful, modern aesthetics

**Cons**:
- ❌ Requires JavaScript knowledge
- ❌ Build step required
- ❌ Less mature than other options
- ❌ Limited to web deployment

**Best for**: Public-facing demos, documentation sites, research presentations

**Effort**: Medium (3-5 days)

---

## Decision Matrix

| Approach | Setup Complexity | User Flexibility | Real-time | Effort | Best For |
|----------|-----------------|------------------|-----------|--------|----------|
| **Plotters (Rust)** | ⭐ Very Low | ⭐⭐ Low | ❌ | Low | Automated reports |
| **JSON + Python** | ⭐⭐ Low | ⭐⭐⭐⭐⭐ Very High | ❌ | Very Low | Research, exploration |
| **Streamlit** | ⭐⭐⭐ Medium | ⭐⭐⭐⭐ High | ❌ | Low | Internal dashboards |
| **Dash/Plotly** | ⭐⭐⭐ Medium | ⭐⭐⭐⭐ High | ❌ | Medium | Team collaboration |
| **Grafana** | ⭐⭐⭐⭐ High | ⭐⭐⭐⭐ High | ✅ | High | Production monitoring |
| **egui** | ⭐⭐ Low | ⭐⭐ Low | ✅ | Medium | Desktop real-time |
| **Observable** | ⭐⭐⭐ Medium | ⭐⭐⭐ Medium | ❌ | Medium | Public demos |

---

## Recommended Strategy: Tiered Approach

### Tier 1: Foundation (Priority 1) - Export Layer

**Goal**: Enable any downstream visualization tool through standard data formats

**Implementation**: Add export capabilities to des-metrics

```rust
// des-metrics/src/export/mod.rs
pub trait MetricsExporter {
    fn export(&self, metrics: &SimulationMetrics) -> Result<()>;
}

pub struct JsonExporter {
    path: PathBuf,
    pretty: bool,
}

pub struct CsvExporter {
    path: PathBuf,
    include_labels: bool,
}

pub struct ParquetExporter {  // Optional: For large datasets
    path: PathBuf,
}
```

**Deliverables**:
1. `SimulationMetrics::export_json(path)` - Structured export for programmatic use
2. `SimulationMetrics::export_csv(path)` - Flat tables for spreadsheets/pandas
3. `SimulationMetrics::export_timeseries_csv(path)` - Optimized for time-series plotting
4. Unit tests with example data

**Output Formats**:
- **JSON**: Structured, nested data (counters, gauges, histograms with labels)
- **CSV**: Time-series tables (one row per timestamp, columns for metrics)
- **Parquet** (optional): Columnar format for massive datasets

**Why This First**:
- Decouples visualization from data collection
- Enables any downstream tool choice
- Low implementation cost
- Maximum flexibility for users

**Effort**: 2-3 days

---

### Tier 2: Quick Wins (Priority 2) - Dual Implementation

Implement BOTH native Rust charts AND Python templates to serve different user workflows.

#### 2A: Plotters Static Charts (Rust)

**Goal**: Provide native Rust visualization for CI/CD and automated analysis

**Implementation**: Extend des-viz with chart builders

```rust
// des-viz/src/charts/mod.rs
pub mod latency;
pub mod throughput;
pub mod heatmap;
pub mod percentiles;

pub fn create_latency_plot(snapshot: &MetricsSnapshot, output: &Path) -> Result<()>;
pub fn create_throughput_plot(snapshot: &MetricsSnapshot, output: &Path) -> Result<()>;
pub fn create_utilization_heatmap(snapshot: &MetricsSnapshot, output: &Path) -> Result<()>;
pub fn create_percentile_comparison(snapshot: &MetricsSnapshot, output: &Path) -> Result<()>;

// Convenience: Generate all charts
pub fn generate_report(snapshot: &MetricsSnapshot, output_dir: &Path) -> Result<()>;
```

**Chart Types**:
1. **Latency Over Time**: Line chart showing p50/p95/p99 bands
2. **Throughput Analysis**: Area chart with goodput vs total throughput
3. **Queue Depth Heatmap**: Color-coded visualization of queue dynamics
4. **Percentile Comparison**: Bar chart comparing tail latencies across components
5. **Utilization**: Stacked area showing resource usage

**Output Formats**: PNG, SVG, HTML (with embedded interactive elements if needed)

**Use Cases**:
- CI/CD regression testing reports
- Automated performance analysis
- Batch simulation comparison
- Documentation generation

**Effort**: 3-5 days

---

#### 2B: Python Analysis Template

**Goal**: Provide interactive exploration tools for researchers and data scientists

**Implementation**: Create example Jupyter notebooks and Python scripts

```
examples/python/
├── requirements.txt
├── analyze_simulation.ipynb    # Interactive notebook
├── visualize.py                # Standalone visualization script
└── compare_runs.py             # Multi-run comparison tool
```

**Deliverables**:
1. **Jupyter Notebook** with:
   - Load exported metrics (JSON/CSV)
   - Common visualizations (latency distributions, throughput trends)
   - Statistical analysis (confidence intervals, hypothesis tests)
   - Customization examples

2. **Standalone Script** for command-line usage:
   ```bash
   python visualize.py metrics.json --output report.html
   ```

3. **Comparison Tools**:
   ```python
   compare_runs(['baseline.json', 'optimized.json'])
   # Generates side-by-side comparisons, statistical tests
   ```

**Libraries Used**:
- pandas (data manipulation)
- matplotlib / seaborn (static plots)
- plotly (interactive plots)
- scipy / statsmodels (statistical tests)

**Use Cases**:
- Ad-hoc exploration
- Research analysis
- Custom metrics and derived statistics
- Publication-quality figures

**Effort**: 2 days

---

### Tier 3: Advanced (Choose Based on Needs)

After implementing Tiers 1 and 2, evaluate which advanced option (if any) is needed based on actual usage patterns.

#### Option A: Grafana + Prometheus (Real-Time Monitoring)

**When to Implement**:
- Running continuous/long simulations
- Need real-time monitoring
- Team collaboration required
- Multiple concurrent simulations

**Implementation**:
```rust
// des-metrics/src/exporters/prometheus.rs
pub struct PrometheusExporter {
    registry: Registry,
    server_addr: SocketAddr,
}

impl PrometheusExporter {
    pub async fn start(&self) -> Result<()> {
        // HTTP server exposing /metrics endpoint
    }
}
```

**Effort**: 1 week + infrastructure setup documentation

---

#### Option B: Streamlit App (Interactive Dashboards)

**When to Implement**:
- Want interactive dashboards without JavaScript
- Rapid iteration on visualizations needed
- Internal tool for team use
- No real-time requirements

**Implementation**:
```python
# scripts/dashboard.py
import streamlit as st
import pandas as pd
import plotly.express as px

st.title("DesCartes Simulation Dashboard")

uploaded = st.file_uploader("Upload metrics.json")
if uploaded:
    data = pd.read_json(uploaded)

    st.line_chart(data[['timestamp', 'latency']])

    metric_col1, metric_col2, metric_col3 = st.columns(3)
    metric_col1.metric("Avg Latency", f"{data['latency'].mean():.2f}ms")
    metric_col2.metric("Throughput", f"{data['throughput'].mean():.1f} RPS")
    metric_col3.metric("Success Rate", f"{data['success_rate'].mean():.1%}")
```

**Effort**: 2-3 days

---

#### Option C: Web Dashboard (Public Demos)

**When to Implement**:
- Public-facing demos needed
- Embedded in documentation
- No server requirements (static site)
- Modern aesthetics important

**Tech Stack**:
- Rust + wasm-pack (compile metrics reader to WASM)
- TypeScript + Vite (build system)
- D3.js or Plotly.js (visualizations)

**Effort**: 1-2 weeks

---

## Implementation Roadmap

### Phase 1: Export Foundation (Weeks 1-2)

**Files to Create**:
```
des-metrics/
└── src/
    └── export/
        ├── mod.rs          # Trait definitions
        ├── json.rs         # JSON exporter
        ├── csv.rs          # CSV exporter
        └── timeseries.rs   # Time-series optimized format
```

**API Design**:
```rust
// Simple usage
let metrics = SimulationMetrics::new();
// ... run simulation ...
metrics.export_json("results/metrics.json")?;
metrics.export_csv("results/metrics.csv")?;

// Advanced usage with configuration
let exporter = JsonExporter::new("results/metrics.json")
    .pretty(true)
    .include_timestamp(true);
exporter.export(&metrics)?;
```

**Testing Strategy**:
- Unit tests with synthetic metrics data
- Integration tests with example simulations
- Validation of JSON schema
- Round-trip testing (export → import → verify)

---

### Phase 2: Visualization Layer (Weeks 3-4)

#### Rust Implementation

**Files to Create**:
```
des-viz/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── error.rs
    ├── charts/
    │   ├── mod.rs
    │   ├── latency.rs       # Latency time-series
    │   ├── throughput.rs    # Throughput analysis
    │   ├── percentiles.rs   # Percentile distributions
    │   └── heatmap.rs       # Queue depth heatmap
    └── report.rs            # HTML report generator

examples/
└── rust/
    └── generate_report.rs   # Example usage
```

**Example Usage**:
```rust
use des_viz::charts::*;
use des_metrics::SimulationMetrics;

fn main() -> Result<()> {
    let metrics = SimulationMetrics::load_json("results/metrics.json")?;
    let snapshot = metrics.get_metrics_snapshot();

    // Generate individual charts
    create_latency_plot(&snapshot, "output/latency.png")?;
    create_throughput_plot(&snapshot, "output/throughput.svg")?;

    // Or generate complete report
    generate_report(&snapshot, "output/report")?;
    // Creates: output/report/index.html with all charts

    Ok(())
}
```

---

#### Python Implementation

**Files to Create**:
```
examples/python/
├── requirements.txt
├── README.md
├── analyze_simulation.ipynb
├── visualize.py
└── utils/
    ├── __init__.py
    ├── loader.py      # Load DesCartes JSON/CSV
    ├── charts.py      # Chart generation functions
    └── stats.py       # Statistical analysis helpers
```

**Example Notebook Structure**:
```python
# Cell 1: Setup
import pandas as pd
import matplotlib.pyplot as plt
from utils.loader import load_descartes_metrics

# Cell 2: Load Data
metrics = load_descartes_metrics('../../results/metrics.json')
print(f"Simulation duration: {metrics.duration}s")
print(f"Total requests: {metrics.total_requests}")

# Cell 3: Latency Analysis
metrics.plot_latency_over_time()
metrics.plot_latency_distribution()

# Cell 4: Throughput Analysis
metrics.plot_throughput()
metrics.calculate_goodput_stats()

# Cell 5: Custom Analysis
# User can add their own analysis here
```

---

### Phase 3: Advanced Features (Optional, Weeks 5+)

Implement based on user feedback and requirements. See Tier 3 options above.

---

## Code Structure Recommendation

```
des-metrics/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── simulation_metrics.rs    # Existing
    ├── request_tracker.rs       # Existing
    └── export/                  # NEW
        ├── mod.rs
        ├── json.rs
        ├── csv.rs
        └── timeseries.rs

des-viz/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── error.rs                 # Existing
    └── charts/                  # NEW
        ├── mod.rs
        ├── latency.rs
        ├── throughput.rs
        ├── percentiles.rs
        ├── heatmap.rs
        └── report.rs

des-components/
└── examples/
    ├── generate_report.rs       # NEW: Show des-viz usage
    └── export_metrics.rs        # NEW: Show export usage

examples/
└── python/                      # NEW
    ├── requirements.txt
    ├── README.md
    ├── analyze_simulation.ipynb
    └── visualize.py
```

---

## Key Design Principles

1. **Separation of Concerns**: Export logic lives in des-metrics, visualization in des-viz
2. **No Lock-In**: JSON/CSV exports enable any visualization tool
3. **Progressive Enhancement**: Start simple, add complexity only when needed
4. **User Choice**: Support both Rust and Python workflows
5. **Standard Formats**: Use widely-supported formats (JSON, CSV, PNG, SVG)
6. **Documentation**: Each tier includes usage examples and templates

---

## Success Metrics

### Tier 1 (Export Layer)
- [ ] JSON export validates against schema
- [ ] CSV export loads correctly in pandas
- [ ] Example simulation exports successfully
- [ ] Documentation includes format specifications

### Tier 2 (Visualizations)
- [ ] Plotters generates charts for all metric types
- [ ] Python notebook runs without errors
- [ ] Examples demonstrate common workflows
- [ ] Visual output is clear and informative

### Tier 3 (Advanced)
- [ ] Real-time monitoring works for long simulations (if Grafana)
- [ ] Interactive dashboard responds in < 1s (if Streamlit)
- [ ] Static site builds and deploys (if web dashboard)

---

## Future Considerations

### Potential Enhancements
1. **Streaming Export**: For very long simulations, stream metrics incrementally
2. **Compression**: Optional gzip/zstd compression for large datasets
3. **Aggregation**: Pre-aggregate metrics at different time granularities
4. **Comparison Tools**: Built-in diffing between simulation runs
5. **Annotations**: Support for marking phases/events in visualizations

### Performance Optimization
- Use Parquet for large datasets (columnar format)
- Implement sampling for high-frequency metrics
- Lazy evaluation for chart generation
- Caching of computed statistics

### Accessibility
- Color-blind friendly palettes
- High-contrast mode
- Alt text for generated charts
- Keyboard navigation in interactive dashboards

---

## Questions for Decision

Before implementation, clarify:

1. **Primary Use Case**: Research, production monitoring, or demos?
2. **Data Volume**: Expected simulation duration and metric frequency?
3. **User Base**: Primarily Rust developers, Python data scientists, or both?
4. **Real-Time Requirements**: Need to visualize while simulation runs?
5. **Deployment**: Local analysis or shared dashboards?

These answers will help prioritize which tier 3 option (if any) to implement.

---

## References

- [plotters documentation](https://docs.rs/plotters/)
- [Grafana Time Series Visualization](https://grafana.com/docs/grafana/latest/panels-visualizations/visualizations/time-series/)
- [Streamlit Documentation](https://docs.streamlit.io/)
- [Observable Framework](https://observablehq.com/framework/)
