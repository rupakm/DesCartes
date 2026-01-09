//! Example demonstrating metrics retrieval functionality
//!
//! This example shows how to use the new metrics retrieval API to:
//! - Record various types of metrics (counters, gauges, histograms)
//! - Retrieve metric values for testing and analysis
//! - Use metrics snapshots for comprehensive analysis
//! - Validate simulation behavior through metrics
//!
//! Run with: cargo run --package des-components --example metrics_retrieval_example

use des_components::{SimpleClient, ExponentialBackoffPolicy, ClientEvent};
use des_core::{Simulation, SimTime, Execute, Executor, task::PeriodicTask};
use des_metrics::SimulationMetrics;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Metrics Retrieval Example ===\n");
    
    // Example 1: Basic metrics recording and retrieval
    basic_metrics_example();
    
    // Example 2: SimpleClient with metrics validation
    client_metrics_example();
    
    // Example 3: Comprehensive metrics analysis
    comprehensive_analysis_example();
    
    println!("\n=== All Metrics Examples Completed ===");
    Ok(())
}

fn basic_metrics_example() {
    println!("1. Basic Metrics Recording and Retrieval");
    println!("   Demonstrating counter, gauge, and histogram operations\n");
    
    let mut metrics = SimulationMetrics::new();
    
    // Record some counters
    metrics.increment_counter("requests", &[("service", "api"), ("status", "success")]);
    metrics.increment_counter("requests", &[("service", "api"), ("status", "success")]);
    metrics.increment_counter("requests", &[("service", "api"), ("status", "error")]);
    metrics.increment_counter_by("bytes_sent", 1024, &[("service", "api")]);
    
    // Record some gauges
    metrics.record_gauge("cpu_usage", 75.5, &[("host", "server1")]);
    metrics.record_gauge("memory_usage", 82.3, &[("host", "server1")]);
    metrics.record_gauge("queue_depth", 12.0, &[("service", "api")]);
    
    // Record some histogram values
    for latency in [10.5, 15.2, 8.7, 22.1, 18.9, 12.3, 25.6, 9.8] {
        metrics.record_histogram("response_time", latency, &[("service", "api")]);
    }
    
    // Retrieve and display metrics
    println!("   📊 Counter Values:");
    println!("      - API Success Requests: {}", 
        metrics.get_counter("requests", &[("service", "api"), ("status", "success")]).unwrap_or(0));
    println!("      - API Error Requests: {}", 
        metrics.get_counter("requests", &[("service", "api"), ("status", "error")]).unwrap_or(0));
    println!("      - Bytes Sent: {}", 
        metrics.get_counter("bytes_sent", &[("service", "api")]).unwrap_or(0));
    
    println!("   📊 Gauge Values:");
    println!("      - CPU Usage: {:.1}%", 
        metrics.get_gauge("cpu_usage", &[("host", "server1")]).unwrap_or(0.0));
    println!("      - Memory Usage: {:.1}%", 
        metrics.get_gauge("memory_usage", &[("host", "server1")]).unwrap_or(0.0));
    println!("      - Queue Depth: {}", 
        metrics.get_gauge("queue_depth", &[("service", "api")]).unwrap_or(0.0) as u32);
    
    println!("   📊 Histogram Statistics:");
    if let Some(stats) = metrics.get_histogram_stats("response_time", &[("service", "api")]) {
        println!("      - Count: {}", stats.count);
        println!("      - Min: {:.1}ms", stats.min);
        println!("      - Max: {:.1}ms", stats.max);
        println!("      - Mean: {:.1}ms", stats.mean);
        println!("      - Median: {:.1}ms", stats.median);
        println!("      - P95: {:.1}ms", stats.p95);
        println!("      - P99: {:.1}ms", stats.p99);
    }
    
    println!();
}

fn client_metrics_example() {
    println!("2. SimpleClient Metrics Validation");
    println!("   Using metrics to validate client behavior\n");
    
    let mut sim = Simulation::default();
    
    // Create a server first
    let server = des_components::Server::with_constant_service_time("metrics-server".to_string(), 1, Duration::from_millis(50));
    let server_id = sim.add_component(server);
    
    // Create client with retry policy
    let retry_policy = ExponentialBackoffPolicy::new(3, Duration::from_millis(50));
    let client = SimpleClient::new("metrics-client".to_string(), server_id, Duration::from_millis(200), retry_policy)
        .with_max_requests(5)
        .with_timeout(Duration::from_millis(100));
    
    let client_id = sim.add_component(client);
    
    // Start periodic requests
    let task = PeriodicTask::with_count(
        move |scheduler| {
            scheduler.schedule_now(client_id, ClientEvent::SendRequest);
        },
        SimTime::from_duration(Duration::from_millis(200)),
        5,
    );
    sim.schedule_task(SimTime::zero(), task);
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_secs(3))).execute(&mut sim);
    
    // Retrieve client and analyze metrics
    let client = sim.remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(client_id).unwrap();
    let metrics = client.get_metrics();
    
    println!("   📊 Client Metrics Analysis:");
    println!("      - Requests Sent: {}", client.requests_sent);
    
    // Validate metrics match expected behavior
    let requests_sent = metrics.get_counter("requests_sent", &[("component", "metrics-client")]).unwrap_or(0);
    let total_requests_gauge = metrics.get_gauge("total_requests", &[("component", "metrics-client")]).unwrap_or(0.0);
    
    println!("      - Requests Counter: {}", requests_sent);
    println!("      - Total Requests Gauge: {}", total_requests_gauge as u64);
    
    // Validate consistency
    assert_eq!(client.requests_sent, requests_sent);
    assert_eq!(client.requests_sent as f64, total_requests_gauge);
    
    // Check for retry metrics
    if let Some(attempts_sent) = metrics.get_counter("attempts_sent", &[("component", "metrics-client")]) {
        println!("      - Total Attempts: {}", attempts_sent);
        println!("      - Retry Rate: {:.1}%", 
            ((attempts_sent as f64 - requests_sent as f64) / requests_sent as f64) * 100.0);
    }
    
    if let Some(successes) = metrics.get_counter("responses_success", &[("component", "metrics-client")]) {
        println!("      - Successful Responses: {}", successes);
    }
    
    if let Some(failures) = metrics.get_counter("responses_failure", &[("component", "metrics-client")]) {
        println!("      - Failed Responses: {}", failures);
    }
    
    if let Some(timeouts) = metrics.get_counter("requests_timeout", &[("component", "metrics-client")]) {
        println!("      - Timeouts: {}", timeouts);
    }
    
    println!("   ✅ Metrics validation passed!");
    println!();
}

fn comprehensive_analysis_example() {
    println!("3. Comprehensive Metrics Analysis");
    println!("   Using metrics snapshots for full system analysis\n");
    
    let mut metrics = SimulationMetrics::new();
    
    // Simulate a multi-service system
    let services = ["auth", "api", "database", "cache"];
    let statuses = ["success", "error", "timeout"];
    
    // Record metrics for multiple services
    for service in &services {
        for status in &statuses {
            let count = match *status {
                "success" => 100 + (rand::random::<u32>() % 50),
                "error" => 5 + (rand::random::<u32>() % 10),
                "timeout" => 2 + (rand::random::<u32>() % 5),
                _ => 0,
            };
            
            metrics.increment_counter_by("requests_total", count as u64, &[
                ("service", service),
                ("status", status),
            ]);
        }
        
        // Record service-specific gauges
        metrics.record_gauge("cpu_usage", 50.0 + (rand::random::<f64>() * 40.0), &[("service", service)]);
        metrics.record_gauge("memory_usage", 60.0 + (rand::random::<f64>() * 30.0), &[("service", service)]);
        
        // Record latency histograms
        for _ in 0..20 {
            let latency = 10.0 + (rand::random::<f64>() * 50.0);
            metrics.record_histogram("latency", latency, &[("service", service)]);
        }
    }
    
    // Get comprehensive snapshot
    let snapshot = metrics.get_metrics_snapshot();
    
    println!("   📊 System-Wide Metrics Analysis:");
    println!("      Timestamp: {:?}", snapshot.timestamp);
    println!();
    
    // Analyze counters
    println!("   🔢 Request Counters by Service:");
    for service in &services {
        let mut total_requests = 0u64;
        let mut success_requests = 0u64;
        
        for (name, labels, value) in &snapshot.counters {
            if name == "requests_total" && labels.get("service") == Some(&service.to_string()) {
                total_requests += value;
                if labels.get("status") == Some(&"success".to_string()) {
                    success_requests = *value;
                }
            }
        }
        
        let success_rate = if total_requests > 0 {
            (success_requests as f64 / total_requests as f64) * 100.0
        } else {
            0.0
        };
        
        println!("      - {}: {} total, {} success ({:.1}%)", 
            service, total_requests, success_requests, success_rate);
    }
    println!();
    
    // Analyze gauges
    println!("   📏 Resource Usage by Service:");
    for service in &services {
        let mut cpu_usage = 0.0;
        let mut memory_usage = 0.0;
        
        for (name, labels, value) in &snapshot.gauges {
            if labels.get("service") == Some(&service.to_string()) {
                match name.as_str() {
                    "cpu_usage" => cpu_usage = *value,
                    "memory_usage" => memory_usage = *value,
                    _ => {}
                }
            }
        }
        
        println!("      - {}: CPU {:.1}%, Memory {:.1}%", service, cpu_usage, memory_usage);
    }
    println!();
    
    // Analyze histograms
    println!("   📈 Latency Statistics by Service:");
    for service in &services {
        if let Some(stats) = metrics.get_histogram_stats("latency", &[("service", service)]) {
            println!("      - {}: mean={:.1}ms, p95={:.1}ms, p99={:.1}ms", 
                service, stats.mean, stats.p95, stats.p99);
        }
    }
    println!();
    
    // Summary statistics
    println!("   📋 Summary:");
    println!("      - Total Counter Metrics: {}", snapshot.counters.len());
    println!("      - Total Gauge Metrics: {}", snapshot.gauges.len());
    println!("      - Total Histogram Metrics: {}", snapshot.histograms.len());
    
    // List all unique metric names
    let counter_names: std::collections::HashSet<_> = snapshot.counters.iter().map(|(name, _, _)| name).collect();
    let gauge_names: std::collections::HashSet<_> = snapshot.gauges.iter().map(|(name, _, _)| name).collect();
    let histogram_names: std::collections::HashSet<_> = snapshot.histograms.iter().map(|(name, _, _)| name).collect();
    
    println!("      - Unique Counter Names: {:?}", counter_names);
    println!("      - Unique Gauge Names: {:?}", gauge_names);
    println!("      - Unique Histogram Names: {:?}", histogram_names);
}

/// Helper function to demonstrate metrics API usage patterns
#[allow(dead_code)]
fn demonstrate_metrics_patterns() {
    println!("Metrics API Usage Patterns:");
    println!();
    
    println!("1. Recording Metrics:");
    println!("   ```rust");
    println!("   let mut metrics = SimulationMetrics::new();");
    println!("   ");
    println!("   // Counters (cumulative values)");
    println!("   metrics.increment_counter(\"requests\", &[(\"service\", \"api\")]);");
    println!("   metrics.increment_counter_by(\"bytes\", 1024, &[(\"service\", \"api\")]);");
    println!("   ");
    println!("   // Gauges (point-in-time values)");
    println!("   metrics.record_gauge(\"cpu_usage\", 75.5, &[(\"host\", \"server1\")]);");
    println!("   ");
    println!("   // Histograms (distributions)");
    println!("   metrics.record_histogram(\"latency\", 12.5, &[(\"service\", \"api\")]);");
    println!("   metrics.record_duration(\"response_time\", duration, &[(\"service\", \"api\")]);");
    println!("   ```");
    println!();
    
    println!("2. Retrieving Metrics:");
    println!("   ```rust");
    println!("   // Get specific metric values");
    println!("   let count = metrics.get_counter(\"requests\", &[(\"service\", \"api\")]);");
    println!("   let usage = metrics.get_gauge(\"cpu_usage\", &[(\"host\", \"server1\")]);");
    println!("   let stats = metrics.get_histogram_stats(\"latency\", &[(\"service\", \"api\")]);");
    println!("   ");
    println!("   // Simple retrieval (no labels)");
    println!("   let simple_count = metrics.get_counter_simple(\"total_requests\");");
    println!("   ");
    println!("   // List all metrics");
    println!("   let counters = metrics.list_counters();");
    println!("   let gauges = metrics.list_gauges();");
    println!("   ");
    println!("   // Get complete snapshot");
    println!("   let snapshot = metrics.get_metrics_snapshot();");
    println!("   ```");
    println!();
    
    println!("3. Testing with Metrics:");
    println!("   ```rust");
    println!("   // Validate expected behavior");
    println!("   assert_eq!(metrics.get_counter_simple(\"requests_sent\"), Some(5));");
    println!("   assert!(metrics.get_gauge(\"cpu_usage\", &[(\"host\", \"server1\")]).unwrap() < 100.0);");
    println!("   ");
    println!("   // Check histogram statistics");
    println!("   let stats = metrics.get_histogram_stats_simple(\"latency\").unwrap();");
    println!("   assert!(stats.p95 < Duration::from_millis(100));");
    println!("   ```");
    println!();
    
    println!("Benefits of the new metrics retrieval API:");
    println!("• Test validation: Assert expected metric values in tests");
    println!("• Debugging: Inspect metric values during development");
    println!("• Analysis: Export metrics for external analysis tools");
    println!("• Monitoring: Build custom dashboards and alerts");
    println!("• Simulation validation: Verify simulation behavior matches expectations");
}