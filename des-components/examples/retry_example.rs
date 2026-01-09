//! Example demonstrating RetryTask usage in des-components
//!
//! This example shows how to use the new DesRetryLayer with Tower services
//! and demonstrates the RetryTask integration for exponential backoff.
//!
//! Run with: cargo run --package des-components --example retry_example

use des_components::tower::{DesServiceBuilder, DesRetryLayer, DesRetryPolicy};
use des_core::{Simulation, SimTime, Scheduler, task::RetryTask, Task};
use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};
use std::time::Duration;
use tower::Layer;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== RetryTask Integration Examples ===\n");
    
    // Example 1: Basic RetryTask usage
    basic_retry_task_example();
    
    // Example 2: DesRetryLayer creation and usage
    retry_layer_example()?;
    
    // Example 3: RetryTask with exponential backoff
    exponential_backoff_example();
    
    println!("\n=== All Retry Examples Completed ===");
    Ok(())
}

fn basic_retry_task_example() {
    println!("1. Basic RetryTask Example");
    println!("   Demonstrating RetryTask with simulated failures\n");
    
    let mut scheduler = Scheduler::default();
    let attempt_count = Arc::new(AtomicUsize::new(0));
    let attempt_count_clone = attempt_count.clone();
    
    // Create a RetryTask that fails twice, then succeeds
    let retry_task = RetryTask::new(
        move |scheduler| -> Result<String, &'static str> {
            let attempts = attempt_count_clone.fetch_add(1, Ordering::Relaxed) + 1;
            println!("   🔄 Attempt {} at {:?}", attempts, scheduler.time());
            
            if attempts < 3 {
                Err("Simulated failure")
            } else {
                Ok("Success!".to_string())
            }
        },
        5, // max attempts
        SimTime::from_duration(Duration::from_millis(100)), // base delay
    );
    
    // Execute the retry task
    let result = retry_task.execute(&mut scheduler);
    
    match result {
        Ok(value) => println!("   ✅ RetryTask succeeded: {value}"),
        Err(e) => println!("   ❌ RetryTask failed: {e}"),
    }
    
    println!("   📊 Total attempts made: {}\n", attempt_count.load(Ordering::Relaxed));
}

fn retry_layer_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("2. DesRetryLayer Example");
    println!("   Creating retry-enabled Tower services\n");
    
    let mut simulation = Simulation::default();
    let scheduler = simulation.scheduler_handle();
    
    // Create a base service
    let base_service = DesServiceBuilder::new("retry-service".to_string())
        .thread_capacity(2)
        .service_time(Duration::from_millis(50))
        .build(&mut simulation)?;
    
    // Create retry layer with policy
    let retry_policy = DesRetryPolicy::new(3); // max attempts
    let retry_layer = DesRetryLayer::new(
        retry_policy,
        scheduler,
    );
    
    // Apply retry layer to service
    let _retry_service = retry_layer.layer(base_service);
    
    println!("   ✅ Created retry-enabled service with:");
    println!("      - Max attempts: 3");
    println!("      - Policy-based retry logic");
    println!("      - DES timing integration\n");
    
    Ok(())
}

fn exponential_backoff_example() {
    println!("3. Exponential Backoff Example");
    println!("   Demonstrating RetryTask backoff timing\n");
    
    let mut scheduler = Scheduler::default();
    let execution_times = Arc::new(Mutex::new(Vec::new()));
    let execution_times_clone = execution_times.clone();
    
    // Create a RetryTask that always fails to show backoff timing
    let retry_task = RetryTask::new(
        move |scheduler| -> Result<(), &'static str> {
            let current_time = scheduler.time();
            execution_times_clone.lock().unwrap().push(current_time);
            println!("   ⏰ Retry attempt at {current_time:?}");
            Err("Always fails")
        },
        4, // max attempts
        SimTime::from_duration(Duration::from_millis(50)), // base delay
    );
    
    // Execute the retry task
    let _result = retry_task.execute(&mut scheduler);
    
    // Show the timing pattern
    let times = execution_times.lock().unwrap();
    println!("   📊 Execution times showing exponential backoff:");
    for (i, time) in times.iter().enumerate() {
        if i == 0 {
            println!("      Attempt {}: {:?} (initial)", i + 1, time);
        } else {
            let delay = *time - times[i - 1];
            println!("      Attempt {}: {:?} (delay: {:?})", i + 1, time, delay);
        }
    }
    
    println!("   ✅ Exponential backoff pattern demonstrated");
}

/// Helper function to demonstrate retry patterns
#[allow(dead_code)]
fn demonstrate_retry_patterns() {
    println!("Retry Patterns Available:");
    println!();
    
    println!("1. RetryTask with exponential backoff:");
    println!("   ```rust");
    println!("   let retry_task = RetryTask::new(");
    println!("       move |scheduler| -> Result<T, E> {{");
    println!("           // Your operation here");
    println!("           attempt_operation()");
    println!("       }},");
    println!("       max_attempts,");
    println!("       base_delay");
    println!("   );");
    println!("   ```");
    println!();
    
    println!("2. DesRetryLayer for Tower services:");
    println!("   ```rust");
    println!("   let retry_policy = DesRetryPolicy::new(3);");
    println!("   let retry_layer = DesRetryLayer::new(");
    println!("       retry_policy,");
    println!("       simulation.scheduler_handle()");
    println!("   );");
    println!("   let retry_service = retry_layer.layer(base_service);");
    println!("   ```");
    println!();
    
    println!("3. Exponential backoff layer:");
    println!("   ```rust");
    println!("   let retry_layer = exponential_backoff_layer(");
    println!("       3, // max attempts");
    println!("       simulation.scheduler_handle()");
    println!("   );");
    println!("   ```");
    println!();
    
    println!("Benefits of Tower Policy approach:");
    println!("• Reuses Tower's retry infrastructure");
    println!("• Policy-based retry decisions");
    println!("• Configurable max attempts");
    println!("• Integration with DES timing");
    println!("• Compatible with Tower middleware stack");
}
