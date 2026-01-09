//! Examples demonstrating Task interface usage in des-components
//!
//! Run with: cargo run --package des-components --example task_usage

use des_components::{ClientEvent, ExponentialBackoffPolicy, SimpleClient};
use des_core::{
    task::{ClosureTask, PeriodicTask, TimeoutTask},
    Execute, Executor, SimTime, Simulation,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

fn main() {
    println!("=== Task Interface Usage Examples ===\n");

    // Example 1: Basic TimeoutTask
    timeout_task_example();

    // Example 2: PeriodicTask with count limit
    periodic_task_example();

    // Example 3: ClosureTask for immediate execution
    closure_task_example();

    // Example 4: Task coordination
    task_coordination_example();

    // Example 5: Real-world usage with SimpleClient
    client_with_tasks_example();
}

fn timeout_task_example() {
    println!("1. TimeoutTask Example");
    println!("   Creating a task that executes after 100ms delay\n");

    let mut sim = Simulation::default();

    let executed = Arc::new(AtomicUsize::new(0));
    let executed_clone = executed.clone();

    // Create a timeout task
    let timeout_task = TimeoutTask::new(move |scheduler| {
        executed_clone.fetch_add(1, Ordering::Relaxed);
        println!("   ⏰ TimeoutTask executed at {:?}", scheduler.time());
    });

    // Schedule the task to execute after 100ms
    sim.schedule_task(
        SimTime::from_duration(Duration::from_millis(100)),
        timeout_task,
    );

    // Run simulation for 150ms
    Executor::timed(SimTime::from_duration(Duration::from_millis(150))).execute(&mut sim);

    println!(
        "   ✅ Task executed {} times\n",
        executed.load(Ordering::Relaxed)
    );
}

fn periodic_task_example() {
    println!("2. PeriodicTask Example");
    println!("   Creating a task that executes every 50ms, exactly 3 times\n");

    let mut sim = Simulation::default();

    let execution_count = Arc::new(AtomicUsize::new(0));
    let count_clone = execution_count.clone();

    // Create a periodic task with count limit
    let periodic_task = PeriodicTask::with_count(
        move |scheduler| {
            let count = count_clone.fetch_add(1, Ordering::Relaxed) + 1;
            println!(
                "   🔄 PeriodicTask execution #{} at {:?}",
                count,
                scheduler.time()
            );
        },
        SimTime::from_duration(Duration::from_millis(50)),
        3, // Execute exactly 3 times
    );

    // Schedule the task to start immediately
    sim.schedule_task(SimTime::zero(), periodic_task);

    // Run simulation for 200ms (enough for 3 executions)
    Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);

    println!(
        "   ✅ Task executed {} times (should be 3)\n",
        execution_count.load(Ordering::Relaxed)
    );
}

fn closure_task_example() {
    println!("3. ClosureTask Example");
    println!("   Creating a simple one-shot task for immediate execution\n");

    let mut sim = Simulation::default();

    let message = "Hello from ClosureTask!";

    // Create a closure task
    let closure_task = ClosureTask::new(move |scheduler| {
        println!("   💬 {} (executed at {:?})", message, scheduler.time());
    });

    // Schedule for immediate execution
    sim.schedule_task(SimTime::zero(), closure_task);

    // Run one simulation step
    sim.step();

    println!("   ✅ ClosureTask completed\n");
}

fn task_coordination_example() {
    println!("4. Task Coordination Example");
    println!("   Demonstrating multiple tasks working together\n");

    let mut sim = Simulation::default();

    let shared_counter = Arc::new(Mutex::new(0));

    // Task 1: Increment counter every 30ms
    let counter1 = shared_counter.clone();
    let incrementer = PeriodicTask::with_count(
        move |scheduler| {
            let mut count = counter1.lock().unwrap();
            *count += 1;
            println!(
                "   📈 Incrementer: count = {} at {:?}",
                *count,
                scheduler.time()
            );
        },
        SimTime::from_duration(Duration::from_millis(30)),
        4,
    );

    // Task 2: Report counter value after 100ms
    let counter2 = shared_counter.clone();
    let reporter = TimeoutTask::new(move |scheduler| {
        let count = counter2.lock().unwrap();
        println!(
            "   📊 Reporter: final count = {} at {:?}",
            *count,
            scheduler.time()
        );
    });

    // Task 3: Reset counter after 50ms
    let counter3 = shared_counter.clone();
    let resetter = TimeoutTask::new(move |scheduler| {
        let mut count = counter3.lock().unwrap();
        let old_count = *count;
        *count = 0;
        println!(
            "   🔄 Resetter: reset count from {} to 0 at {:?}",
            old_count,
            scheduler.time()
        );
    });

    // Schedule all tasks
    sim.schedule_task(SimTime::zero(), incrementer);
    sim.schedule_task(SimTime::from_duration(Duration::from_millis(50)), resetter);
    sim.schedule_task(SimTime::from_duration(Duration::from_millis(100)), reporter);

    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_millis(150))).execute(&mut sim);

    println!("   ✅ Task coordination completed\n");
}

fn client_with_tasks_example() {
    println!("5. Real-world Example: SimpleClient with PeriodicTask");
    println!("   Using PeriodicTask to drive client request generation\n");

    let mut sim = Simulation::default();

    // Create a server first
    let server = des_components::Server::with_constant_service_time(
        "example-server".to_string(),
        1,
        Duration::from_millis(50),
    );
    let server_id = sim.add_component(server);

    // Create a client with exponential backoff retry policy
    let client = SimpleClient::with_exponential_backoff(
        "example-client".to_string(),
        server_id,
        Duration::from_millis(75),
        2,                         // max retries
        Duration::from_millis(25), // base delay
    )
    .with_max_requests(4);
    let client_id = sim.add_component(client);

    // Use PeriodicTask to generate requests
    let request_task = PeriodicTask::with_count(
        move |scheduler| {
            println!("   📤 Triggering client request at {:?}", scheduler.time());
            scheduler.schedule_now(client_id, ClientEvent::SendRequest);
        },
        SimTime::from_duration(Duration::from_millis(75)),
        4, // Generate 4 requests
    );

    // Schedule the periodic task
    sim.schedule_task(SimTime::zero(), request_task);

    // Add a monitoring task that reports progress
    let monitor_task = PeriodicTask::with_count(
        move |scheduler| {
            println!("   📊 Monitor check at {:?}", scheduler.time());
        },
        SimTime::from_duration(Duration::from_millis(100)),
        3,
    );
    sim.schedule_task(SimTime::zero(), monitor_task);

    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_millis(350))).execute(&mut sim);

    // Check final client state
    let final_client = sim
        .remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(client_id)
        .unwrap();
    println!(
        "   ✅ Client sent {} requests (expected 4)",
        final_client.requests_sent
    );

    println!("\n=== All Examples Completed ===");
}
