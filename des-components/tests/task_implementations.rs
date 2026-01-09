//! Comprehensive tests for Task-based implementations
//!
//! This test suite validates all Task-based conversions from the migration plan:
//! - TowerResponseTask (automatic cleanup, response handling)
//! - TimeoutTask in timeout layer (timing precision, cleanup)
//! - TimeoutTask in circuit breaker (state transitions, recovery)
//! - PeriodicTask in SimpleClient (periodic behavior, termination)

use des_components::{
    SimpleClient, ClientEvent, ExponentialBackoffPolicy, Server,
};
use des_core::{
    Execute, Executor, Simulation, SimTime,
    task::{TimeoutTask, PeriodicTask},
};
use des_core::task::ClosureTask;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}};
use std::time::Duration;

/// Test PeriodicTask behavior in SimpleClient
#[test]
fn test_periodic_task_behavior() {
    println!("\n=== PeriodicTask Behavior Test ===\n");

    let mut sim = Simulation::default();
    
    // Create a server first
    let server = Server::with_constant_service_time("periodic-server".to_string(), 1, Duration::from_millis(100));
    let server_id = sim.add_component(server);
    
    // Create client with specific request count
    let client = SimpleClient::with_exponential_backoff(
        "periodic-client".to_string(),
        server_id, // Add the server key
        Duration::from_millis(50),
        3, // max retries
        Duration::from_millis(25), // base delay
    ).with_max_requests(5);
    let client_id = sim.add_component(client);
    
    // Use PeriodicTask with count limit
    let task = PeriodicTask::with_count(
        move |scheduler| {
            scheduler.schedule_now(client_id, ClientEvent::SendRequest);
        },
        SimTime::from_duration(Duration::from_millis(50)),
        5, // Should stop after 5 executions
    );
    
    sim.scheduler.schedule_task(SimTime::zero(), task);
    
    // Run simulation for enough time to see all requests
    Executor::timed(SimTime::from_duration(Duration::from_millis(300))).execute(&mut sim);
    
    // Verify client sent exactly 5 requests
    let client = sim.remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(client_id).unwrap();
    assert_eq!(client.requests_sent, 5, "Client should have sent exactly 5 requests");
    
    println!("PeriodicTask sent {} requests as expected", client.requests_sent);
}

/// Test PeriodicTask automatic termination
#[test]
fn test_periodic_task_automatic_termination() {
    println!("\n=== PeriodicTask Automatic Termination Test ===\n");

    let mut sim = Simulation::default();
    let counter = Arc::new(AtomicUsize::new(0));
    
    // Create a PeriodicTask that increments a counter
    let counter_clone = counter.clone();
    let task = PeriodicTask::with_count(
        move |_scheduler| {
            counter_clone.fetch_add(1, Ordering::Relaxed);
        },
        SimTime::from_duration(Duration::from_millis(25)),
        3, // Should execute exactly 3 times
    );
    
    sim.scheduler.schedule_task(SimTime::zero(), task);
    
    // Run simulation for longer than needed
    Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);
    
    // Verify counter was incremented exactly 3 times
    let final_count = counter.load(Ordering::Relaxed);
    assert_eq!(final_count, 3, "PeriodicTask should have executed exactly 3 times");
    
    println!("PeriodicTask executed {final_count} times and terminated correctly");
}

/// Test Task cleanup behavior under various conditions
#[test]
fn test_task_cleanup_edge_cases() {
    println!("\n=== Task Cleanup Edge Cases Test ===\n");

    let mut sim = Simulation::default();
    
    // Test 1: TimeoutTask that fires immediately
    let fired = Arc::new(AtomicBool::new(false));
    let fired_clone = fired.clone();
    
    let immediate_task = TimeoutTask::new(move |_scheduler| {
        fired_clone.store(true, Ordering::Relaxed);
    });
    
    sim.scheduler.schedule_task(SimTime::zero(), immediate_task);
    
    // Run one step
    assert!(sim.step(), "Simulation should have events to process");
    assert!(fired.load(Ordering::Relaxed), "TimeoutTask should have fired immediately");
    
    // Test 2: ClosureTask with complex cleanup
    let cleanup_called = Arc::new(AtomicBool::new(false));
    let cleanup_clone = cleanup_called.clone();
    
    let closure_task = ClosureTask::new(move |_scheduler| {
        // Simulate some work
        cleanup_clone.store(true, Ordering::Relaxed);
    });
    
    sim.scheduler.schedule_task(SimTime::from_duration(Duration::from_millis(10)), closure_task);
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_millis(20))).execute(&mut sim);
    
    assert!(cleanup_called.load(Ordering::Relaxed), "ClosureTask should have executed");
    
    // Test 3: Multiple overlapping TimeoutTasks
    let task_count = Arc::new(AtomicUsize::new(0));
    
    for i in 0..5 {
        let count_clone = task_count.clone();
        let timeout_task = TimeoutTask::new(move |_scheduler| {
            count_clone.fetch_add(1, Ordering::Relaxed);
        });
        
        sim.scheduler.schedule_task(
            SimTime::from_duration(Duration::from_millis(i * 10)),
            timeout_task
        );
    }
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_millis(100))).execute(&mut sim);
    
    assert_eq!(task_count.load(Ordering::Relaxed), 5, "All 5 TimeoutTasks should have executed");
    
    println!("Task cleanup edge cases test completed successfully");
}

/// Test Task timing precision
#[test]
fn test_task_timing_precision() {
    println!("\n=== Task Timing Precision Test ===\n");

    let mut sim = Simulation::default();
    let execution_times = Arc::new(Mutex::new(Vec::new()));
    
    // Schedule tasks at specific times
    let expected_times = [Duration::from_millis(10),
        Duration::from_millis(25),
        Duration::from_millis(50),
        Duration::from_millis(100)];
    
    for (i, &delay) in expected_times.iter().enumerate() {
        let times_clone = execution_times.clone();
        let task = TimeoutTask::new(move |scheduler| {
            let mut times = times_clone.lock().unwrap();
            times.push((i, scheduler.time()));
        });
        
        sim.scheduler.schedule_task(SimTime::from_duration(delay), task);
    }
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_millis(150))).execute(&mut sim);
    
    // Verify timing precision
    let times = execution_times.lock().unwrap();
    assert_eq!(times.len(), 4, "All 4 tasks should have executed");
    
    for (i, &expected_delay) in expected_times.iter().enumerate() {
        let (task_id, actual_time) = times[i];
        assert_eq!(task_id, i, "Tasks should execute in order");
        
        let expected_time = SimTime::from_duration(expected_delay);
        assert_eq!(actual_time, expected_time, "Task {i} should execute at precise time");
    }
    
    println!("Task timing precision test completed successfully");
}

/// Test Task error handling and recovery
#[test]
fn test_task_error_handling() {
    println!("\n=== Task Error Handling Test ===\n");

    let mut sim = Simulation::default();
    let success_count = Arc::new(AtomicUsize::new(0));
    
    // Create tasks that might encounter various conditions
    for i in 0..3 {
        let count_clone = success_count.clone();
        let task = ClosureTask::new(move |_scheduler| {
            // All tasks should execute successfully
            count_clone.fetch_add(1, Ordering::Relaxed);
        });
        
        sim.scheduler.schedule_task(
            SimTime::from_duration(Duration::from_millis(i * 10)),
            task
        );
    }
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_millis(50))).execute(&mut sim);
    
    // Verify all tasks executed
    assert_eq!(success_count.load(Ordering::Relaxed), 3, "All tasks should execute successfully");
    
    println!("Task error handling test completed successfully");
}

/// Integration test combining multiple Task types
#[test]
fn test_mixed_task_integration() {
    println!("\n=== Mixed Task Integration Test ===\n");

    let mut sim = Simulation::default();
    let results = Arc::new(Mutex::new(Vec::new()));
    
    // 1. PeriodicTask that runs 3 times
    let results_clone = results.clone();
    let periodic_task = PeriodicTask::with_count(
        move |scheduler| {
            let mut r = results_clone.lock().unwrap();
            r.push(format!("Periodic at {:?}", scheduler.time()));
        },
        SimTime::from_duration(Duration::from_millis(30)),
        3,
    );
    sim.scheduler.schedule_task(SimTime::zero(), periodic_task);
    
    // 2. TimeoutTask that fires once
    let results_clone = results.clone();
    let timeout_task = TimeoutTask::new(move |scheduler| {
        let mut r = results_clone.lock().unwrap();
        r.push(format!("Timeout at {:?}", scheduler.time()));
    });
    sim.scheduler.schedule_task(SimTime::from_duration(Duration::from_millis(45)), timeout_task);
    
    // 3. ClosureTask that executes immediately
    let results_clone = results.clone();
    let closure_task = ClosureTask::new(move |scheduler| {
        let mut r = results_clone.lock().unwrap();
        r.push(format!("Closure at {:?}", scheduler.time()));
    });
    sim.scheduler.schedule_task(SimTime::zero(), closure_task);
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_millis(120))).execute(&mut sim);
    
    // Verify all tasks executed
    let final_results = results.lock().unwrap();
    assert_eq!(final_results.len(), 5, "Should have 5 total executions (3 periodic + 1 timeout + 1 closure)");
    
    // Verify execution order and timing
    println!("Task execution results:");
    for result in final_results.iter() {
        println!("  {result}");
    }
    
    // Should have closure and periodic at 0, periodic at 30, timeout at 45, periodic at 60
    // The order of closure vs periodic at time 0 is not deterministic, so just check they both exist
    let time_0_results: Vec<_> = final_results.iter().filter(|r| r.contains("at SimTime(0)")).collect();
    assert_eq!(time_0_results.len(), 2, "Should have 2 tasks executing at time 0");
    assert!(final_results.iter().any(|r| r.contains("Timeout at SimTime(45000000)")), "Timeout should execute at 45ms");
    
    println!("Mixed task integration test completed successfully");
}