//! Example demonstrating metrics visualization with des-viz
//!
//! This example shows how to:
//! 1. Collect simulation metrics
//! 2. Generate charts using plotters
//! 3. Create an HTML report with embedded visualizations
//!
//! Run with: cargo run --example visualize_metrics_example

use descartes_core::SimTime;
use descartes_metrics::SimulationMetrics;
use descartes_viz::charts::{latency, percentiles, throughput};
use descartes_viz::report::generate_html_report;
use std::time::Duration;

fn main() {
    println!("=== Metrics Visualization Example ===\n");

    // Create metrics collector
    let mut metrics = SimulationMetrics::new();
    metrics.set_start_time(SimTime::zero());

    // Simulate collecting diverse metrics from multiple components
    println!("Collecting metrics from simulated workload...");

    // Component 1: Client metrics
    for _ in 0..150 {
        metrics.increment_counter("requests_sent", &[("component", "client1")]);
    }
    for _ in 0..100 {
        metrics.increment_counter("requests_sent", &[("component", "client2")]);
    }

    // Component 2: Server metrics
    for _ in 0..140 {
        metrics.increment_counter("responses_success", &[("component", "server1")]);
    }
    for _ in 0..10 {
        metrics.increment_counter("responses_failure", &[("component", "server1")]);
    }

    for _ in 0..90 {
        metrics.increment_counter("responses_success", &[("component", "server2")]);
    }
    for _ in 0..5 {
        metrics.increment_counter("responses_failure", &[("component", "server2")]);
    }

    // Gauge metrics for different components
    metrics.record_gauge("queue_depth", 15.0, &[("component", "server1")]);
    metrics.record_gauge("queue_depth", 8.0, &[("component", "server2")]);
    metrics.record_gauge("active_threads", 8.0, &[("component", "server1")]);
    metrics.record_gauge("active_threads", 4.0, &[("component", "server2")]);
    metrics.record_gauge("utilization", 0.80, &[("component", "server1")]);
    metrics.record_gauge("utilization", 0.50, &[("component", "server2")]);
    metrics.record_gauge("throughput_rps", 145.5, &[("component", "server1")]);
    metrics.record_gauge("throughput_rps", 90.2, &[("component", "server2")]);

    // Simulate latency distributions for different services
    println!("Recording latency distributions...");

    // Server 1: Lower latency service
    for i in 1..=100 {
        let latency_ms = 15.0 + (i as f64 * 0.3) + (i % 10) as f64 * 0.5;
        metrics.record_histogram("response_time_ms", latency_ms, &[("component", "server1")]);
    }

    // Server 2: Higher latency service
    for i in 1..=100 {
        let latency_ms = 35.0 + (i as f64 * 0.5) + (i % 15) as f64;
        metrics.record_histogram("response_time_ms", latency_ms, &[("component", "server2")]);
    }

    // API endpoint latencies
    for i in 1..=80 {
        let latency_ms = 8.0 + (i as f64 * 0.2) + (i % 5) as f64 * 0.3;
        metrics.record_histogram("api_latency_ms", latency_ms, &[("endpoint", "/users")]);
    }

    for i in 1..=70 {
        let latency_ms = 12.0 + (i as f64 * 0.25) + (i % 8) as f64 * 0.4;
        metrics.record_histogram("api_latency_ms", latency_ms, &[("endpoint", "/orders")]);
    }

    // High-resolution latency tracking
    for i in 1..=150 {
        let latency = Duration::from_micros(5000 + i * 100 + (i % 20) * 50);
        metrics.record_latency("service_latency", latency, &[]);
    }

    println!(
        "✓ Collected {} counters, {} gauges, {} histograms\n",
        metrics.get_metrics_snapshot().counters.len(),
        metrics.get_metrics_snapshot().gauges.len(),
        metrics.get_metrics_snapshot().histograms.len()
    );

    // Generate individual charts
    println!("Generating individual charts...");
    let snapshot = metrics.get_metrics_snapshot();

    std::fs::create_dir_all("target/viz_output").expect("Failed to create output directory");

    // Latency chart
    match latency::create_latency_chart(&snapshot, "target/viz_output/latency.png") {
        Ok(_) => println!("✓ Created latency chart: target/viz_output/latency.png"),
        Err(e) => eprintln!("✗ Latency chart failed: {}", e),
    }

    // Throughput chart
    match throughput::create_throughput_chart(&snapshot, "target/viz_output/throughput.png") {
        Ok(_) => println!("✓ Created throughput chart: target/viz_output/throughput.png"),
        Err(e) => eprintln!("✗ Throughput chart failed: {}", e),
    }

    // Percentiles chart
    match percentiles::create_percentile_chart(&snapshot, "target/viz_output/percentiles.png") {
        Ok(_) => println!("✓ Created percentiles chart: target/viz_output/percentiles.png"),
        Err(e) => eprintln!("✗ Percentiles chart failed: {}", e),
    }

    // Generate complete HTML report
    println!("\nGenerating HTML report...");
    match generate_html_report(&snapshot, "target/viz_output/report.html") {
        Ok(_) => {
            println!("✓ Created HTML report: target/viz_output/report.html");
            println!("\n📊 Open the report in your browser:");
            println!("   open target/viz_output/report.html");
        }
        Err(e) => eprintln!("✗ HTML report failed: {}", e),
    }

    // Display summary statistics
    println!("\n=== Summary Statistics ===");
    if let Some(latency_stats) = metrics.get_latency_stats("service_latency") {
        println!("Service Latency:");
        println!("  Count: {}", latency_stats.count);
        println!("  Mean: {:.2}ms", latency_stats.mean.as_secs_f64() * 1000.0);
        println!("  P50: {:.2}ms", latency_stats.p50.as_secs_f64() * 1000.0);
        println!("  P95: {:.2}ms", latency_stats.p95.as_secs_f64() * 1000.0);
        println!("  P99: {:.2}ms", latency_stats.p99.as_secs_f64() * 1000.0);
        println!(
            "  P99.9: {:.2}ms",
            latency_stats.p999.as_secs_f64() * 1000.0
        );
    }

    println!("\n=== Example Complete ===");
    println!("Generated files:");
    println!("  - target/viz_output/latency.png");
    println!("  - target/viz_output/throughput.png");
    println!("  - target/viz_output/percentiles.png");
    println!("  - target/viz_output/report.html");
    println!("\nNext steps:");
    println!("  1. Open report.html in your browser");
    println!("  2. View individual PNG charts");
    println!("  3. Export metrics for further analysis (see export_metrics_example)");
}
