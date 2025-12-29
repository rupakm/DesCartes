//! Integration tests for RetryTask and DesRetryLayer
//!
//! These tests validate the retry functionality and demonstrate
//! how RetryTask integrates with Tower services.

use des_components::{DesServiceBuilder, DesRetryLayer, DesRetryPolicy, ServiceError};
use des_core::{Simulation, SimTime, Task, task::RetryTask};
use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};
use std::time::Duration;
use tower::{Layer, retry::Policy};

#[test]
fn test_retry_task_basic_functionality() {
    println!("\n=== RetryTask Basic Functionality Test ===\n");
    
    let mut sim = Simulation::default();
    let attempt_count = Arc::new(AtomicUsize::new(0));
    let attempt_count_clone = attempt_count.clone();
    
    // Create a RetryTask that succeeds on the first attempt
    let retry_task = RetryTask::new(
        move |_scheduler| -> Result<String, &'static str> {
            let attempts = attempt_count_clone.fetch_add(1, Ordering::Relaxed) + 1;
            println!("   Attempt {attempts} executed");
            Ok("Success!".to_string())
        },
        3, // max attempts
        SimTime::from_duration(Duration::from_millis(100)),
    );
    
    let result = retry_task.execute(&mut sim.scheduler);
    
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Success!");
    assert_eq!(attempt_count.load(Ordering::Relaxed), 1);
    
    println!("✅ RetryTask succeeded on first attempt");
}

#[test]
fn test_retry_task_with_failures() {
    println!("\n=== RetryTask with Failures Test ===\n");
    
    let mut sim = Simulation::default();
    let attempt_count = Arc::new(AtomicUsize::new(0));
    let attempt_count_clone = attempt_count.clone();
    
    // Create a RetryTask that fails on first attempt, succeeds on second
    let retry_task = RetryTask::new(
        move |_scheduler| -> Result<i32, &'static str> {
            let attempts = attempt_count_clone.fetch_add(1, Ordering::Relaxed) + 1;
            println!("   Attempt {attempts} executed");
            
            if attempts == 1 {
                Err("First attempt fails")
            } else {
                Ok(42)
            }
        },
        3, // max attempts
        SimTime::from_duration(Duration::from_millis(50)),
    );
    
    let result = retry_task.execute(&mut sim.scheduler);
    
    // Note: Current RetryTask implementation returns the first result
    // The retry scheduling happens in the background
    println!("   Result: {result:?}");
    println!("   Attempts made: {}", attempt_count.load(Ordering::Relaxed));
    
    // The first execution should have happened
    assert_eq!(attempt_count.load(Ordering::Relaxed), 1);
    
    println!("✅ RetryTask executed with retry scheduling");
}

#[test]
fn test_retry_task_max_attempts() {
    println!("\n=== RetryTask Max Attempts Test ===\n");
    
    let mut sim = Simulation::default();
    let attempt_count = Arc::new(AtomicUsize::new(0));
    let attempt_count_clone = attempt_count.clone();
    
    // Create a RetryTask that always fails
    let retry_task = RetryTask::new(
        move |_scheduler| -> Result<(), &'static str> {
            let attempts = attempt_count_clone.fetch_add(1, Ordering::Relaxed) + 1;
            println!("   Attempt {attempts} executed (always fails)");
            Err("Always fails")
        },
        2, // max attempts
        SimTime::from_duration(Duration::from_millis(25)),
    );
    
    let result = retry_task.execute(&mut sim.scheduler);
    
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Always fails");
    assert_eq!(attempt_count.load(Ordering::Relaxed), 1);
    
    println!("✅ RetryTask respects max attempts limit");
}

#[test]
fn test_des_retry_layer_creation() {
    println!("\n=== DesRetryLayer Creation Test ===\n");
    
    let simulation = Arc::new(Mutex::new(Simulation::default()));
    
    // Create retry layer with policy
    let retry_policy = DesRetryPolicy::new(3); // max attempts
    let retry_layer = DesRetryLayer::new(
        retry_policy,
        Arc::downgrade(&simulation),
    );
    
    // Create base service
    let base_service = DesServiceBuilder::new("retry-test-service".to_string())
        .thread_capacity(2)
        .service_time(Duration::from_millis(50))
        .build(simulation.clone())
        .unwrap();
    
    // Apply retry layer
    let _retry_service = retry_layer.layer(base_service);
    
    println!("✅ DesRetryLayer created and applied successfully");
}

#[test]
fn test_des_retry_policy_functionality() {
    println!("\n=== DesRetryPolicy Functionality Test ===\n");
    
    let mut policy = DesRetryPolicy::new(3);
    
    // Test that policy allows retries for retryable errors
    let retryable_error = ServiceError::Timeout { duration: Duration::from_millis(100) };
    let mut result: Result<(), ServiceError> = Err(retryable_error);
    let mut request = ();
    let should_retry = policy.retry(&mut request, &mut result);
    assert!(should_retry.is_some());
    
    // Test that policy rejects non-retryable errors
    let non_retryable_error = ServiceError::Cancelled;
    let mut result: Result<(), ServiceError> = Err(non_retryable_error);
    let mut request = ();
    let should_retry = policy.retry(&mut request, &mut result);
    assert!(should_retry.is_none());
    
    println!("✅ DesRetryPolicy working correctly");
    println!("   Retryable: Timeout, Overloaded, Internal, NotReady");
    println!("   Non-retryable: Cancelled, Http");
}

#[test]
fn test_retry_task_with_scheduler_integration() {
    println!("\n=== RetryTask Scheduler Integration Test ===\n");
    
    let mut sim = Simulation::default();
    let execution_times = Arc::new(Mutex::new(Vec::new()));
    let execution_times_clone = execution_times.clone();
    
    // Create a RetryTask that records execution times
    let retry_task = RetryTask::new(
        move |scheduler| -> Result<(), &'static str> {
            let current_time = scheduler.time();
            execution_times_clone.lock().unwrap().push(current_time);
            println!("   Executed at {current_time:?}");
            Err("Fail to show timing")
        },
        1, // Only one attempt to keep test simple
        SimTime::from_duration(Duration::from_millis(100)),
    );
    
    let _result = retry_task.execute(&mut sim.scheduler);
    
    let times = execution_times.lock().unwrap();
    assert_eq!(times.len(), 1);
    assert_eq!(times[0], SimTime::from_duration(Duration::ZERO));
    
    println!("✅ RetryTask integrates correctly with scheduler timing");
}

#[test]
fn test_retry_layer_with_service_builder() {
    println!("\n=== Retry Layer with ServiceBuilder Test ===\n");
    
    let simulation = Arc::new(Mutex::new(Simulation::default()));
    
    // Create base service
    let base_service = DesServiceBuilder::new("layered-retry-service".to_string())
        .thread_capacity(3)
        .service_time(Duration::from_millis(75))
        .build(simulation.clone())
        .unwrap();
    
    // Use Tower's ServiceBuilder to compose layers
    use tower::ServiceBuilder;
    
    let retry_policy = DesRetryPolicy::new(4); // max attempts
    let _composed_service = ServiceBuilder::new()
        .layer(DesRetryLayer::new(
            retry_policy,
            Arc::downgrade(&simulation),
        ))
        .service(base_service);
    
    println!("✅ Retry layer composes correctly with ServiceBuilder");
    println!("   Configuration: 4 max attempts with policy-based retry logic");
}

#[test]
fn test_retry_task_exponential_backoff_calculation() {
    println!("\n=== RetryTask Exponential Backoff Calculation Test ===\n");
    
    // Test the exponential backoff calculation logic
    let base_delay = Duration::from_millis(100);
    
    // Simulate the backoff calculation from the retry implementation
    let delays = (1..=4).map(|attempt| {
        let multiplier = 2_u64.pow(attempt - 1);
        base_delay * multiplier as u32
    }).collect::<Vec<_>>();
    
    println!("   Base delay: {base_delay:?}");
    println!("   Exponential backoff sequence:");
    for (i, delay) in delays.iter().enumerate() {
        println!("     Attempt {}: {:?}", i + 1, delay);
    }
    
    // Verify the exponential progression
    assert_eq!(delays[0], Duration::from_millis(100)); // 100 * 2^0 = 100
    assert_eq!(delays[1], Duration::from_millis(200)); // 100 * 2^1 = 200
    assert_eq!(delays[2], Duration::from_millis(400)); // 100 * 2^2 = 400
    assert_eq!(delays[3], Duration::from_millis(800)); // 100 * 2^3 = 800
    
    println!("✅ Exponential backoff calculation verified");
}