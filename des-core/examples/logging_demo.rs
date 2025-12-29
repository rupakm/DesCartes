//! Demonstration of logging capabilities in des-core
//!
//! This example shows how to use the logging system to debug and monitor
//! discrete event simulations.

use des_core::{
    Component, Executor, Key, Simulation, SimTime,
    init_detailed_simulation_logging,
};
use std::time::Duration;
use tracing::{info, debug};

/// A simple server component that processes requests
#[derive(Debug)]
struct Server {
    name: String,
    requests_processed: u32,
    processing_time: Duration,
}

#[derive(Debug)]
enum ServerEvent {
    ProcessRequest { request_id: u32 },
    RequestCompleted { request_id: u32 },
}

impl Server {
    fn new(name: String, processing_time: Duration) -> Self {
        Self {
            name,
            requests_processed: 0,
            processing_time,
        }
    }
}

impl Component for Server {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut des_core::Scheduler,
    ) {
        match event {
            ServerEvent::ProcessRequest { request_id } => {
                info!(
                    server = %self.name,
                    request_id = request_id,
                    "Starting to process request"
                );
                
                // Schedule completion after processing time
                scheduler.schedule(
                    SimTime::from_duration(self.processing_time),
                    self_id,
                    ServerEvent::RequestCompleted { request_id: *request_id },
                );
            }
            ServerEvent::RequestCompleted { request_id } => {
                self.requests_processed += 1;
                info!(
                    server = %self.name,
                    request_id = request_id,
                    total_processed = self.requests_processed,
                    "Request completed"
                );
            }
        }
    }
}

/// A client component that sends requests
#[derive(Debug)]
struct Client {
    name: String,
    server_key: Key<ServerEvent>,
    requests_to_send: u32,
    requests_sent: u32,
    request_interval: Duration,
}

#[derive(Debug)]
enum ClientEvent {
    SendRequest,
}

impl Client {
    fn new(
        name: String,
        server_key: Key<ServerEvent>,
        requests_to_send: u32,
        request_interval: Duration,
    ) -> Self {
        Self {
            name,
            server_key,
            requests_to_send,
            requests_sent: 0,
            request_interval,
        }
    }
}

impl Component for Client {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut des_core::Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                if self.requests_sent < self.requests_to_send {
                    self.requests_sent += 1;
                    
                    debug!(
                        client = %self.name,
                        request_id = self.requests_sent,
                        "Sending request to server"
                    );
                    
                    // Send request to server
                    scheduler.schedule_now(
                        self.server_key,
                        ServerEvent::ProcessRequest {
                            request_id: self.requests_sent,
                        },
                    );
                    
                    // Schedule next request if more to send
                    if self.requests_sent < self.requests_to_send {
                        scheduler.schedule(
                            SimTime::from_duration(self.request_interval),
                            self_id,
                            ClientEvent::SendRequest,
                        );
                    } else {
                        info!(
                            client = %self.name,
                            total_sent = self.requests_sent,
                            "All requests sent"
                        );
                    }
                }
            }
        }
    }
}

fn main() {
    // ============================================================================
    // LOGGING CONFIGURATION OPTIONS
    // ============================================================================
    
    // Option 1: Detailed logging with pretty formatting (RECOMMENDED FOR DEBUGGING)
    // This shows TRACE, DEBUG, INFO, WARN, and ERROR messages with full context
    init_detailed_simulation_logging();
    
    // Option 2: Set a specific log level (uncomment to try)
    // Available levels: "trace", "debug", "info", "warn", "error"
    // init_simulation_logging_with_level("info");    // Only INFO and above
    // init_simulation_logging_with_level("debug");   // DEBUG and above
    // init_simulation_logging_with_level("trace");   // Everything (very verbose)
    
    // Option 3: Use environment variable for dynamic control
    // Set RUST_LOG=debug before running: RUST_LOG=debug cargo run --example logging_demo
    // This allows you to control logging without recompiling
    
    // Option 4: Use default logging (info level)
    // use des_core::init_simulation_logging;
    // init_simulation_logging();
    
    // ============================================================================
    // WHAT YOU'LL SEE IN THE TERMINAL:
    // - Timestamps for each log entry
    // - Log level (TRACE, DEBUG, INFO, WARN, ERROR)
    // - Module path (where the log came from)
    // - File and line number
    // - Thread ID
    // - Structured fields (component_id, request_id, etc.)
    // - Span context (nested execution context)
    // ============================================================================
    
    info!("Starting logging demonstration");
    
    // Create simulation
    let mut sim = Simulation::default();
    
    // Create server component
    let server = Server::new(
        "WebServer".to_string(),
        Duration::from_millis(50), // 50ms processing time
    );
    let server_key = sim.add_component(server);
    
    // Create client component
    let client = Client::new(
        "WebClient".to_string(),
        server_key,
        5, // Send 5 requests
        Duration::from_millis(100), // Every 100ms
    );
    let client_key = sim.add_component(client);
    
    // Start the client
    sim.schedule(SimTime::zero(), client_key, ClientEvent::SendRequest);
    
    // Run simulation for 1 second
    info!("Starting simulation execution");
    sim.execute(Executor::timed(SimTime::from_secs(1)));
    
    // Get final state
    let final_server = sim.remove_component::<ServerEvent, Server>(server_key).unwrap();
    let final_client = sim.remove_component::<ClientEvent, Client>(client_key).unwrap();
    
    info!(
        server_requests_processed = final_server.requests_processed,
        client_requests_sent = final_client.requests_sent,
        final_time = ?sim.scheduler.time(),
        "Simulation completed"
    );
    
    println!("\n=== Simulation Summary ===");
    println!("Server '{}' processed {} requests", final_server.name, final_server.requests_processed);
    println!("Client '{}' sent {} requests", final_client.name, final_client.requests_sent);
    println!("Final simulation time: {:?}", sim.scheduler.time());
}