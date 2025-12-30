//! Example demonstrating metrics export functionality
//!
//! This example shows how to:
//! 1. Collect simulation metrics
//! 2. Export to JSON format
//! 3. Export to CSV format
//!
//! Run with: cargo run --example export_metrics_example

use des_core::SimTime;
use des_metrics::SimulationMetrics;
use std::time::Duration;

fn main() {
    println!("=== Metrics Export Example ===\n");

    // Create metrics collector
    let mut metrics = SimulationMetrics::new();
    metrics.set_start_time(SimTime::zero());

    // Simulate collecting various metrics
    println!("Collecting metrics...");

    // Counter metrics
    for _ in 0..100 {
        metrics.increment_counter("requests_sent", &[("component", "client1")]);
        metrics.increment_counter("requests_sent", &[("component", "client2")]);
    }

    for _ in 0..95 {
        metrics.increment_counter("responses_success", &[("component", "server1")]);
    }

    for _ in 0..5 {
        metrics.increment_counter("responses_failure", &[("component", "server1")]);
    }

    // Gauge metrics
    metrics.record_gauge("queue_depth", 12.0, &[("component", "server1")]);
    metrics.record_gauge("active_threads", 8.0, &[("component", "server1")]);
    metrics.record_gauge("utilization", 0.75, &[("component", "server1")]);
    metrics.record_gauge("throughput_rps", 150.5, &[("component", "server1")]);

    // Histogram metrics - simulate response times
    for i in 1..=100 {
        let latency_ms = 10.0 + (i as f64 * 0.5) + (i % 10) as f64;
        metrics.record_histogram(
            "response_time_ms",
            latency_ms,
            &[("component", "server1")],
        );
    }

    // High-resolution latency tracking
    for i in 1..=100 {
        let latency = Duration::from_millis(10 + i % 50);
        metrics.record_latency("service_latency", latency, &[]);
    }

    // Component-specific metrics using helpers
    metrics.record_component_counter("server1", "requests_processed", 100);
    metrics.record_component_gauge("server1", "memory_usage_mb", 512.0);
    metrics.record_component_latency(
        "server1",
        "processing_latency",
        Duration::from_millis(25),
    );

    println!("✓ Collected metrics from simulated workload\n");

    // Export to JSON
    println!("Exporting to JSON...");
    let json_path = "target/example_metrics.json";
    match metrics.export_json(json_path, true) {
        Ok(_) => println!("✓ Exported to: {}", json_path),
        Err(e) => eprintln!("✗ JSON export failed: {}", e),
    }

    // Export to CSV
    println!("\nExporting to CSV...");
    let csv_path = "target/example_metrics.csv";
    match metrics.export_csv(csv_path, true) {
        Ok(_) => {
            println!("✓ Exported to:");
            println!("  - target/example_metrics_counters.csv");
            println!("  - target/example_metrics_gauges.csv");
            println!("  - target/example_metrics_histograms.csv");
            println!("  - target/example_metrics_request_stats.csv");
        }
        Err(e) => eprintln!("✗ CSV export failed: {}", e),
    }

    // Display some statistics
    println!("\n=== Metrics Summary ===");
    let snapshot = metrics.get_metrics_snapshot();
    println!("Counters: {}", snapshot.counters.len());
    println!("Gauges: {}", snapshot.gauges.len());
    println!("Histograms: {}", snapshot.histograms.len());

    // Show latency stats if available
    if let Some(latency_stats) = metrics.get_latency_stats("service_latency") {
        println!("\nService Latency Statistics:");
        println!("  Count: {}", latency_stats.count);
        println!("  Mean: {:.2}ms", latency_stats.mean.as_secs_f64() * 1000.0);
        println!("  P50: {:.2}ms", latency_stats.p50.as_secs_f64() * 1000.0);
        println!("  P95: {:.2}ms", latency_stats.p95.as_secs_f64() * 1000.0);
        println!("  P99: {:.2}ms", latency_stats.p99.as_secs_f64() * 1000.0);
        println!("  P99.9: {:.2}ms", latency_stats.p999.as_secs_f64() * 1000.0);
    }

    println!("\n=== Example Complete ===");
    println!("You can now:");
    println!("1. View the JSON file: cat {}", json_path);
    println!("2. Open CSV files in a spreadsheet application");
    println!("3. Use Python/pandas for analysis:");
    println!("   import pandas as pd");
    println!("   df = pd.read_csv('target/example_metrics_counters.csv')");
    println!("   print(df)");
}
