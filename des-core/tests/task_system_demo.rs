//! Comprehensive demonstration of the Task system working with Components and Async runtime

use descartes_core::{
    async_runtime::{sim_sleep, DesRuntime, RuntimeEvent},
    Component, Execute, Executor, Key, SimTime, Simulation,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Demo showing Tasks, Components, and Async working together
#[test]
fn test_task_component_async_integration() {
    println!("\n=== Task System Integration Demo ===");

    let mut sim = Simulation::default();

    // Shared state to track execution order
    let execution_log = Arc::new(Mutex::new(Vec::new()));

    // 1. Add a Component that uses Tasks
    #[derive(Debug)]
    #[allow(dead_code)]
    enum ServerEvent {
        ProcessRequest { id: u32 },
        RequestTimeout { id: u32 },
    }

    struct TaskUsingServer {
        #[allow(dead_code)]
        name: String,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Component for TaskUsingServer {
        type Event = ServerEvent;

        fn process_event(
            &mut self,
            _self_id: Key<Self::Event>,
            event: &Self::Event,
            scheduler: &mut descartes_core::Scheduler,
        ) {
            match event {
                ServerEvent::ProcessRequest { id } => {
                    self.log
                        .lock()
                        .unwrap()
                        .push(format!("Server: Processing request {id}"));

                    // Schedule a timeout task for this request
                    let log_clone = self.log.clone();
                    let request_id = *id;
                    scheduler.timeout(
                        SimTime::from_duration(Duration::from_millis(100)),
                        move |_scheduler| {
                            log_clone
                                .lock()
                                .unwrap()
                                .push(format!("Task: Request {request_id} timed out"));
                            // Could schedule a timeout event back to the component
                        },
                    );

                    // Schedule completion task
                    let log_clone2 = self.log.clone();
                    scheduler.schedule_closure(
                        SimTime::from_duration(Duration::from_millis(50)),
                        move |_scheduler| {
                            log_clone2
                                .lock()
                                .unwrap()
                                .push(format!("Task: Request {request_id} completed"));
                        },
                    );
                }
                ServerEvent::RequestTimeout { id } => {
                    self.log
                        .lock()
                        .unwrap()
                        .push(format!("Server: Request {id} timed out"));
                }
            }
        }
    }

    let server = TaskUsingServer {
        name: "demo-server".to_string(),
        log: execution_log.clone(),
    };
    let server_id = sim.add_component(server);

    // 2. Add Async Runtime
    let mut runtime = DesRuntime::new();

    // Spawn async tasks
    let log_clone = execution_log.clone();
    runtime.spawn(async move {
        log_clone
            .lock()
            .unwrap()
            .push("Async: Task 1 started".to_string());
        sim_sleep(Duration::from_millis(25)).await;
        log_clone
            .lock()
            .unwrap()
            .push("Async: Task 1 after 25ms".to_string());
        sim_sleep(Duration::from_millis(25)).await;
        log_clone
            .lock()
            .unwrap()
            .push("Async: Task 1 completed".to_string());
    });

    let log_clone2 = execution_log.clone();
    runtime.spawn(async move {
        log_clone2
            .lock()
            .unwrap()
            .push("Async: Task 2 started".to_string());
        sim_sleep(Duration::from_millis(75)).await;
        log_clone2
            .lock()
            .unwrap()
            .push("Async: Task 2 completed".to_string());
    });

    let runtime_id = sim.add_component(runtime);

    // 3. Schedule some events and tasks

    // Schedule component events
    sim.schedule(
        SimTime::from_duration(Duration::from_millis(10)),
        server_id,
        ServerEvent::ProcessRequest { id: 1 },
    );

    sim.schedule(
        SimTime::from_duration(Duration::from_millis(30)),
        server_id,
        ServerEvent::ProcessRequest { id: 2 },
    );

    // Schedule standalone tasks
    let log_clone3 = execution_log.clone();
    sim.schedule_closure(
        SimTime::from_duration(Duration::from_millis(20)),
        move |_scheduler| {
            log_clone3
                .lock()
                .unwrap()
                .push("Task: Standalone task executed".to_string());
        },
    );

    // Schedule periodic task
    let log_clone4 = execution_log.clone();
    let mut counter = 0;
    sim.schedule_closure(
        SimTime::from_duration(Duration::from_millis(40)),
        move |scheduler| {
            counter += 1;
            log_clone4
                .lock()
                .unwrap()
                .push(format!("Task: Periodic task #{counter}"));

            if counter < 3 {
                // Schedule next execution
                let log_clone_inner = log_clone4.clone();
                scheduler.schedule_closure(
                    SimTime::from_duration(Duration::from_millis(30)),
                    move |scheduler_inner| {
                        counter += 1;
                        log_clone_inner
                            .lock()
                            .unwrap()
                            .push(format!("Task: Periodic task #{counter}"));

                        if counter < 3 {
                            // One more execution
                            let log_clone_inner2 = log_clone_inner.clone();
                            scheduler_inner.schedule_closure(
                                SimTime::from_duration(Duration::from_millis(30)),
                                move |_| {
                                    log_clone_inner2
                                        .lock()
                                        .unwrap()
                                        .push("Task: Periodic task #3".to_string());
                                },
                            );
                        }
                    },
                );
            }
        },
    );

    // Start async runtime
    sim.schedule(
        SimTime::from_duration(Duration::from_millis(5)),
        runtime_id,
        RuntimeEvent::Poll,
    );

    // Run simulation
    println!("Running simulation for 200ms...");
    Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);

    // Print execution log
    let final_log = execution_log.lock().unwrap();
    println!("\n=== Execution Log ===");
    for (i, entry) in final_log.iter().enumerate() {
        println!("  {}: {}", i + 1, entry);
    }

    // Verify we got a good mix of different execution types
    let log_str = final_log.join(" ");
    assert!(log_str.contains("Server: Processing request"));
    assert!(log_str.contains("Task: Request") && log_str.contains("completed"));
    assert!(log_str.contains("Async: Task 1 started"));
    assert!(log_str.contains("Async: Task 1 completed"));
    assert!(log_str.contains("Async: Task 2 completed"));
    assert!(log_str.contains("Task: Standalone task executed"));
    assert!(log_str.contains("Task: Periodic task"));

    println!("\n=== Integration Demo Complete ===");
    println!("✓ Components successfully used Tasks for timeouts and callbacks");
    println!("✓ Async runtime worked alongside Components and Tasks");
    println!("✓ Standalone Tasks executed independently");
    println!("✓ All systems integrated seamlessly");
}

/// Demo showing task cancellation
#[test]
fn test_task_cancellation_demo() {
    println!("\n=== Task Cancellation Demo ===");

    let mut sim = Simulation::default();

    let executed = Arc::new(Mutex::new(Vec::new()));

    // Schedule a task that will be cancelled
    let executed_clone = executed.clone();
    let handle1 = sim.schedule_closure(
        SimTime::from_duration(Duration::from_millis(100)),
        move |_scheduler| {
            executed_clone
                .lock()
                .unwrap()
                .push("Task 1: Should not execute".to_string());
        },
    );

    // Schedule a task that will execute
    let executed_clone2 = executed.clone();
    let _handle2 = sim.schedule_closure(
        SimTime::from_duration(Duration::from_millis(50)),
        move |_scheduler| {
            executed_clone2
                .lock()
                .unwrap()
                .push("Task 2: Should execute".to_string());
        },
    );

    // Schedule a task to cancel the first task
    sim.schedule_closure(
        SimTime::from_duration(Duration::from_millis(25)),
        move |scheduler| {
            let cancelled = scheduler.cancel_task(handle1);
            println!("Task cancellation result: {cancelled}");
            assert!(cancelled, "Task should have been successfully cancelled");
        },
    );

    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_millis(150))).execute(&mut sim);

    let final_executed = executed.lock().unwrap();
    println!("Executed tasks: {:?}", *final_executed);

    // Verify only the second task executed
    assert_eq!(final_executed.len(), 1);
    assert_eq!(final_executed[0], "Task 2: Should execute");

    println!("✓ Task cancellation working correctly");
}

/// Demo showing task return values
#[test]
fn test_task_return_values_demo() {
    println!("\n=== Task Return Values Demo ===");

    let mut sim = Simulation::default();

    // Schedule tasks with different return types
    let handle1 = sim.schedule_closure(
        SimTime::from_duration(Duration::from_millis(50)),
        |_scheduler| -> i32 {
            println!("Task 1: Computing result...");
            42
        },
    );

    let handle2 = sim.schedule_closure(
        SimTime::from_duration(Duration::from_millis(75)),
        |_scheduler| -> String {
            println!("Task 2: Computing result...");
            "Hello from task!".to_string()
        },
    );

    let handle3 = sim.schedule_closure(
        SimTime::from_duration(Duration::from_millis(100)),
        |_scheduler| -> Vec<i32> {
            println!("Task 3: Computing result...");
            vec![1, 2, 3, 4, 5]
        },
    );

    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_millis(150))).execute(&mut sim);

    // Retrieve results
    let result1 = sim.get_task_result(handle1);
    let result2 = sim.get_task_result(handle2);
    let result3 = sim.get_task_result(handle3);

    println!("Task 1 result: {result1:?}");
    println!("Task 2 result: {result2:?}");
    println!("Task 3 result: {result3:?}");

    assert_eq!(result1, Some(42));
    assert_eq!(result2, Some("Hello from task!".to_string()));
    assert_eq!(result3, Some(vec![1, 2, 3, 4, 5]));

    println!("✓ Task return values working correctly");
}
