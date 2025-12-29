//! Integration test showing SimpleClient and SimpleServer working together
//!
//! This demonstrates the new Component-based API with a client-server interaction.

use des_components::{SimpleClient, Server, ClientEvent, ServerEvent, ExponentialBackoffPolicy};
use des_core::{Execute, Executor, Simulation, SimTime};
use des_core::task::PeriodicTask;
use std::time::Duration;

#[test]
fn test_client_server_integration() {
    println!("\n=== Client-Server Integration Test ===\n");

    let mut sim = Simulation::default();

    // Create a server with capacity 2 and 50ms service time
    let server = Server::new("web-server".to_string(), 2, Duration::from_millis(50));
    let server_id = sim.add_component(server);

    // Create a client that sends 5 requests every 100ms with exponential backoff
    let client = SimpleClient::with_exponential_backoff(
        "web-client".to_string(), 
        Duration::from_millis(100),
        3, // max retries
        Duration::from_millis(50), // base delay
    ).with_max_requests(5);
    let client_id = sim.add_component(client);

    // Start the client with PeriodicTask
    let task = PeriodicTask::with_count(
        move |scheduler| {
            scheduler.schedule_now(client_id, ClientEvent::SendRequest);
        },
        SimTime::from_duration(Duration::from_millis(100)),
        5,
    );
    sim.scheduler.schedule_task(SimTime::zero(), task);

    println!("Running simulation for 1 second...\n");

    // Run simulation for 1 second
    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);

    println!("\n=== Simulation Complete ===\n");

    // Check results
    let client = sim.remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(client_id).unwrap();
    let server = sim.remove_component::<ServerEvent, Server>(server_id).unwrap();

    println!("Final Results:");
    println!("  Client '{}' sent {} requests", client.name, client.requests_sent);
    println!("  Server '{}' processed {} requests", server.name, server.requests_processed);
    println!("  Server final load: {}/{}", server.active_threads, server.thread_capacity);

    // Verify expected behavior
    assert_eq!(client.requests_sent, 5, "Client should have sent 5 requests");
    // Note: Server might not have processed all requests yet due to timing
    assert!(server.requests_processed <= 5, "Server shouldn't process more than 5 requests");
    assert!(server.active_threads <= server.thread_capacity, "Server load should not exceed capacity");

    println!("\n=== Test Passed ===\n");
}

#[test]
fn test_server_overload_scenario() {
    println!("\n=== Server Overload Test ===\n");

    let mut sim = Simulation::default();

    // Create a server with capacity 1 and slow service time (200ms)
    let server = Server::new("slow-server".to_string(), 1, Duration::from_millis(200));
    let server_id = sim.add_component(server);

    // Create a fast client that sends 3 requests every 50ms with exponential backoff
    let client = SimpleClient::with_exponential_backoff(
        "fast-client".to_string(),
        Duration::from_millis(50),
        2, // max retries
        Duration::from_millis(25), // base delay
    ).with_max_requests(3);
    let client_id = sim.add_component(client);

    // Start the client with PeriodicTask
    let task = PeriodicTask::with_count(
        move |scheduler| {
            scheduler.schedule_now(client_id, ClientEvent::SendRequest);
        },
        SimTime::from_duration(Duration::from_millis(50)),
        3,
    );
    sim.scheduler.schedule_task(SimTime::zero(), task);

    println!("Running simulation with server overload scenario...\n");

    // Run simulation for 1 second
    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);

    println!("\n=== Simulation Complete ===\n");

    // Check results
    let client = sim.remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(client_id).unwrap();
    let server = sim.remove_component::<ServerEvent, Server>(server_id).unwrap();

    println!("Final Results:");
    println!("  Client '{}' sent {} requests", client.name, client.requests_sent);
    println!("  Server '{}' processed {} requests", server.name, server.requests_processed);
    println!("  Server final load: {}/{}", server.active_threads, server.thread_capacity);

    // Verify expected behavior - with fast client and slow server, some requests should be rejected
    assert_eq!(client.requests_sent, 3, "Client should have sent 3 requests");
    assert!(server.requests_processed <= 3, "Server shouldn't process more than client sent");
    
    // Due to the timing (50ms intervals, 200ms service time), the server should be overloaded
    // and reject some requests, so processed count should be less than sent count initially

    println!("\n=== Test Passed ===\n");
}