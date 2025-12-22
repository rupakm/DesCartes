//! Integration test showing actual client-server communication
//!
//! This demonstrates how components can communicate with each other
//! by sending events between them.

use des_components::{Server, ServerEvent, ClientEvent};
use des_core::{Component, Execute, Executor, Key, Scheduler, Simulation, SimTime, RequestAttempt, RequestAttemptId, RequestId};
use std::time::Duration;

/// Client that actually sends requests to a specific server
pub struct CommunicatingClient {
    pub name: String,
    pub server_id: Key<ServerEvent>,
    pub request_interval: Duration,
    pub requests_sent: u64,
    pub responses_received: u64,
    pub successful_responses: u64,
    pub max_requests: Option<u64>,
    pub next_request_id: u64,
    pub next_attempt_id: u64,
}

impl CommunicatingClient {
    pub fn new(name: String, server_id: Key<ServerEvent>, request_interval: Duration) -> Self {
        Self {
            name,
            server_id,
            request_interval,
            requests_sent: 0,
            responses_received: 0,
            successful_responses: 0,
            max_requests: None,
            next_request_id: 1,
            next_attempt_id: 1,
        }
    }

    pub fn with_max_requests(mut self, max_requests: u64) -> Self {
        self.max_requests = Some(max_requests);
        self
    }

    fn should_send_request(&self) -> bool {
        if let Some(max) = self.max_requests {
            self.requests_sent < max
        } else {
            true
        }
    }

    fn schedule_next_request(&self, self_id: Key<ClientEvent>, scheduler: &mut Scheduler) {
        if self.should_send_request() {
            scheduler.schedule(
                SimTime::from_duration(self.request_interval),
                self_id,
                ClientEvent::SendRequest,
            );
        }
    }
}

impl Component for CommunicatingClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                let request_id = self.next_request_id;
                let attempt_id = self.next_attempt_id;
                
                println!(
                    "[{}] Sending request {} (attempt {}) to server at {:?}",
                    self.name,
                    request_id,
                    attempt_id,
                    scheduler.time()
                );
                
                // Create a RequestAttempt
                let attempt = RequestAttempt::new(
                    RequestAttemptId(attempt_id),
                    RequestId(request_id),
                    1, // First attempt
                    scheduler.time(),
                    vec![], // Empty payload
                );
                
                self.requests_sent += 1;
                self.next_request_id += 1;
                self.next_attempt_id += 1;
                
                // Send request attempt to the server
                scheduler.schedule(
                    SimTime::from_duration(Duration::from_millis(1)),
                    self.server_id,
                    ServerEvent::ProcessRequest { 
                        attempt,
                        client_id: self_id 
                    },
                );
                
                // Schedule next request
                self.schedule_next_request(self_id, scheduler);
            }
            ClientEvent::ResponseReceived { response } => {
                self.responses_received += 1;
                if response.is_success() {
                    self.successful_responses += 1;
                }
                
                println!(
                    "[{}] Received response #{}: {} at {:?}",
                    self.name,
                    self.responses_received,
                    if response.is_success() { "SUCCESS" } else { "FAILURE" },
                    scheduler.time()
                );
            }
        }
    }
}

#[test]
fn test_actual_client_server_communication() {
    println!("\n=== Actual Client-Server Communication Test ===\n");

    let mut sim = Simulation::default();

    // Create server first
    let server = Server::new("api-server".to_string(), 2, Duration::from_millis(30));
    let server_id = sim.add_component(server);

    // Create client that knows about the server
    let client = CommunicatingClient::new(
        "api-client".to_string(), 
        server_id, 
        Duration::from_millis(100)
    ).with_max_requests(4);
    let client_id = sim.add_component(client);

    // Start the client
    sim.schedule(
        SimTime::from_duration(Duration::from_millis(50)),
        client_id,
        ClientEvent::SendRequest,
    );

    println!("Running simulation with actual client-server communication...\n");

    // Run simulation for 800ms
    Executor::timed(SimTime::from_duration(Duration::from_millis(800))).execute(&mut sim);

    println!("\n=== Simulation Complete ===\n");

    // Check results
    let client = sim.remove_component::<ClientEvent, CommunicatingClient>(client_id).unwrap();
    let server = sim.remove_component::<ServerEvent, Server>(server_id).unwrap();

    println!("Final Results:");
    println!("  Client '{}' sent {} requests", client.name, client.requests_sent);
    println!("  Client '{}' received {} responses ({} successful)", 
             client.name, client.responses_received, client.successful_responses);
    println!("  Server '{}' processed {} requests", server.name, server.requests_processed);
    println!("  Server final load: {}/{}", server.active_threads, server.thread_capacity);

    // Verify actual communication occurred
    assert_eq!(client.requests_sent, 4, "Client should have sent 4 requests");
    assert_eq!(client.responses_received, 4, "Client should have received 4 responses");
    assert_eq!(server.requests_processed, 4, "Server should have processed 4 requests");
    assert_eq!(client.successful_responses, 4, "All responses should be successful with this timing");
    assert_eq!(server.active_threads, 0, "Server should have no pending requests");

    println!("\n=== Test Passed ===\n");
}

#[test]
fn test_server_overload_with_communication() {
    println!("\n=== Server Overload with Communication Test ===\n");

    let mut sim = Simulation::default();

    // Create server with capacity 1 and slow service time
    let server = Server::new("slow-server".to_string(), 1, Duration::from_millis(150));
    let server_id = sim.add_component(server);

    // Create fast client
    let client = CommunicatingClient::new(
        "fast-client".to_string(), 
        server_id, 
        Duration::from_millis(50)
    ).with_max_requests(4);
    let client_id = sim.add_component(client);

    // Start the client
    sim.schedule(
        SimTime::from_duration(Duration::from_millis(25)),
        client_id,
        ClientEvent::SendRequest,
    );

    println!("Running simulation with server overload...\n");

    // Run simulation for 500ms
    Executor::timed(SimTime::from_duration(Duration::from_millis(500))).execute(&mut sim);

    println!("\n=== Simulation Complete ===\n");

    // Check results
    let client = sim.remove_component::<ClientEvent, CommunicatingClient>(client_id).unwrap();
    let server = sim.remove_component::<ServerEvent, Server>(server_id).unwrap();

    println!("Final Results:");
    println!("  Client '{}' sent {} requests", client.name, client.requests_sent);
    println!("  Client '{}' received {} responses ({} successful)", 
             client.name, client.responses_received, client.successful_responses);
    println!("  Server '{}' processed {} requests", server.name, server.requests_processed);
    println!("  Server final load: {}/{}", server.active_threads, server.thread_capacity);

    // Verify overload behavior
    assert_eq!(client.requests_sent, 4, "Client should have sent 4 requests");
    assert_eq!(client.responses_received, 4, "Client should have received 4 responses");
    
    // With fast requests (50ms interval) and slow service (150ms), some should be rejected
    assert!(client.successful_responses < client.responses_received, 
            "Some responses should be failures due to overload");
    
    // Server should have processed fewer requests than total sent due to rejections
    assert!(server.requests_processed <= client.requests_sent, 
            "Server shouldn't process more than client sent");

    println!("\n=== Test Passed ===\n");
}