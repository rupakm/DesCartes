//! Simple server component using the new des-core Component API
//!
//! This is a minimal example showing how to create a server component that
//! processes requests and sends responses using the new event-driven system.

use des_core::{Component, Key, Scheduler, SimTime, SimulationMetrics};
use std::time::Duration;

/// Simple server component that processes requests
pub struct SimpleServer {
    pub name: String,
    pub service_time: Duration,
    pub requests_processed: u64,
    pub capacity: usize,
    pub current_load: usize,
    pub metrics: SimulationMetrics,
}

/// Events that the SimpleServer can handle
#[derive(Debug)]
pub enum ServerEvent {
    /// Request received from client
    ProcessRequest { request_id: u64, client_id: Key<ClientEvent> },
    /// Request processing completed
    RequestCompleted { request_id: u64, client_id: Key<ClientEvent> },
}

// Re-export ClientEvent from simple_client for compatibility
pub use crate::simple_client::ClientEvent;

impl SimpleServer {
    /// Create a new simple server
    pub fn new(name: String, service_time: Duration, capacity: usize) -> Self {
        Self {
            name,
            service_time,
            requests_processed: 0,
            capacity,
            current_load: 0,
            metrics: SimulationMetrics::new(),
        }
    }

    /// Get metrics for this server
    pub fn get_metrics(&self) -> &SimulationMetrics {
        &self.metrics
    }

    /// Check if the server can accept more requests
    fn can_accept_request(&self) -> bool {
        self.current_load < self.capacity
    }

    /// Process a request if capacity allows
    fn process_request(&mut self, request_id: u64, client_id: Key<ClientEvent>, self_id: Key<ServerEvent>, scheduler: &mut Scheduler) {
        if self.can_accept_request() {
            println!(
                "[{}] Processing request #{} at {:?} (load: {}/{})",
                self.name,
                request_id,
                scheduler.time(),
                self.current_load + 1,
                self.capacity
            );
            
            self.current_load += 1;
            
            // Record metrics for accepted request
            self.metrics.increment_counter("requests_accepted", &self.name, scheduler.time());
            self.metrics.record_gauge("current_load", &self.name, self.current_load as f64, scheduler.time());
            self.metrics.record_gauge("utilization", &self.name, (self.current_load as f64 / self.capacity as f64) * 100.0, scheduler.time());
            
            // Schedule completion of request processing
            scheduler.schedule(
                SimTime::from_duration(self.service_time),
                self_id,
                ServerEvent::RequestCompleted { request_id, client_id },
            );
        } else {
            println!(
                "[{}] Rejecting request #{} - server at capacity ({}/{})",
                self.name,
                request_id,
                self.current_load,
                self.capacity
            );
            
            // Record metrics for rejected request
            self.metrics.increment_counter("requests_rejected", &self.name, scheduler.time());
            
            // Send immediate rejection response
            scheduler.schedule(
                SimTime::from_duration(Duration::from_millis(1)),
                client_id,
                ClientEvent::ResponseReceived { success: false },
            );
        }
    }

    /// Complete request processing
    fn complete_request(&mut self, request_id: u64, client_id: Key<ClientEvent>, scheduler: &mut Scheduler) {
        println!(
            "[{}] Completed request #{} at {:?}",
            self.name,
            request_id,
            scheduler.time()
        );
        
        self.requests_processed += 1;
        self.current_load = self.current_load.saturating_sub(1);
        
        // Record metrics for completed request
        self.metrics.increment_counter("requests_completed", &self.name, scheduler.time());
        self.metrics.record_gauge("total_processed", &self.name, self.requests_processed as f64, scheduler.time());
        self.metrics.record_gauge("current_load", &self.name, self.current_load as f64, scheduler.time());
        self.metrics.record_gauge("utilization", &self.name, (self.current_load as f64 / self.capacity as f64) * 100.0, scheduler.time());
        
        // Record service time as histogram
        self.metrics.record_duration("service_time", &self.name, self.service_time, scheduler.time());
        
        // Send success response to client
        scheduler.schedule(
            SimTime::from_duration(Duration::from_millis(1)),
            client_id,
            ClientEvent::ResponseReceived { success: true },
        );
    }
}

impl Component for SimpleServer {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ServerEvent::ProcessRequest { request_id, client_id } => {
                self.process_request(*request_id, *client_id, self_id, scheduler);
            }
            ServerEvent::RequestCompleted { request_id, client_id } => {
                self.complete_request(*request_id, *client_id, scheduler);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use des_core::{Execute, Executor, Simulation};

    /// Simple client for testing the server
    pub struct TestClient {
        pub name: String,
        pub responses_received: u64,
        pub successful_responses: u64,
    }

    impl TestClient {
        pub fn new(name: String) -> Self {
            Self {
                name,
                responses_received: 0,
                successful_responses: 0,
            }
        }
    }

    impl Component for TestClient {
        type Event = ClientEvent;

        fn process_event(
            &mut self,
            _self_id: Key<Self::Event>,
            event: &Self::Event,
            scheduler: &mut Scheduler,
        ) {
            match event {
                ClientEvent::ResponseReceived { success } => {
                    self.responses_received += 1;
                    if *success {
                        self.successful_responses += 1;
                    }
                    println!(
                        "[{}] Received response #{}: {} at {:?}",
                        self.name,
                        self.responses_received,
                        if *success { "SUCCESS" } else { "FAILURE" },
                        scheduler.time()
                    );
                }
                ClientEvent::SendRequest => {
                    // TestClient doesn't send requests, only receives responses
                    // This case should not occur in normal operation
                }
            }
        }
    }

    #[test]
    fn test_simple_server_basic() {
        let mut sim = Simulation::default();
        
        // Create server with capacity 2 and 50ms service time
        let server = SimpleServer::new("test-server".to_string(), Duration::from_millis(50), 2);
        let server_id = sim.add_component(server);
        
        // Create test client
        let client = TestClient::new("test-client".to_string());
        let client_id = sim.add_component(client);
        
        // Send a request to the server
        sim.schedule(
            SimTime::from_duration(Duration::from_millis(10)),
            server_id,
            ServerEvent::ProcessRequest { request_id: 1, client_id },
        );
        
        // Run simulation for 200ms
        Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);
        
        // Verify the server processed the request
        let server = sim.remove_component::<ServerEvent, SimpleServer>(server_id).unwrap();
        assert_eq!(server.requests_processed, 1);
        assert_eq!(server.current_load, 0);
        
        // Verify the client received a response
        let client = sim.remove_component::<ClientEvent, TestClient>(client_id).unwrap();
        assert_eq!(client.responses_received, 1);
        assert_eq!(client.successful_responses, 1);
    }

    #[test]
    fn test_simple_server_capacity_limit() {
        let mut sim = Simulation::default();
        
        // Create server with capacity 1 and 100ms service time
        let server = SimpleServer::new("test-server".to_string(), Duration::from_millis(100), 1);
        let server_id = sim.add_component(server);
        
        // Create test client
        let client = TestClient::new("test-client".to_string());
        let client_id = sim.add_component(client);
        
        // Send two requests simultaneously
        sim.schedule(
            SimTime::from_duration(Duration::from_millis(10)),
            server_id,
            ServerEvent::ProcessRequest { request_id: 1, client_id },
        );
        sim.schedule(
            SimTime::from_duration(Duration::from_millis(11)),
            server_id,
            ServerEvent::ProcessRequest { request_id: 2, client_id },
        );
        
        // Run simulation for 200ms
        Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);
        
        // Verify the server processed only one request and rejected the other
        let server = sim.remove_component::<ServerEvent, SimpleServer>(server_id).unwrap();
        assert_eq!(server.requests_processed, 1);
        assert_eq!(server.current_load, 0);
        
        // Verify the client received two responses (one success, one failure)
        let client = sim.remove_component::<ClientEvent, TestClient>(client_id).unwrap();
        assert_eq!(client.responses_received, 2);
        assert_eq!(client.successful_responses, 1);
    }

    #[test]
    fn test_simple_server_metrics() {
        let mut sim = Simulation::default();
        
        // Create server with capacity 2 and 50ms service time
        let server = SimpleServer::new("metrics-server".to_string(), Duration::from_millis(50), 2);
        let server_id = sim.add_component(server);
        
        // Create test client
        let client = TestClient::new("metrics-client".to_string());
        let client_id = sim.add_component(client);
        
        // Send three requests: two should be accepted, one rejected due to capacity
        sim.schedule(
            SimTime::from_duration(Duration::from_millis(10)),
            server_id,
            ServerEvent::ProcessRequest { request_id: 1, client_id },
        );
        sim.schedule(
            SimTime::from_duration(Duration::from_millis(15)),
            server_id,
            ServerEvent::ProcessRequest { request_id: 2, client_id },
        );
        sim.schedule(
            SimTime::from_duration(Duration::from_millis(20)),
            server_id,
            ServerEvent::ProcessRequest { request_id: 3, client_id },
        );
        
        // Run simulation for 200ms
        Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);
        
        // Get the server and check metrics
        let server = sim.remove_component::<ServerEvent, SimpleServer>(server_id).unwrap();
        let metrics = server.get_metrics();
        
        // Verify basic server state
        assert_eq!(server.requests_processed, 2);
        assert_eq!(server.current_load, 0);
        
        // Check that metrics were recorded
        let all_metrics = metrics.get_metrics();
        assert!(!all_metrics.is_empty(), "Metrics should have been recorded");
        
        // Check for specific metric types
        let server_metrics = metrics.get_component_metrics("metrics-server");
        assert!(!server_metrics.is_empty(), "Server-specific metrics should exist");
        
        // Verify we have the expected metric types
        let counter_metrics = metrics.get_metrics_by_type(des_core::MetricType::Counter);
        let gauge_metrics = metrics.get_metrics_by_type(des_core::MetricType::Gauge);
        let histogram_metrics = metrics.get_metrics_by_type(des_core::MetricType::Histogram);
        
        assert!(!counter_metrics.is_empty(), "Should have counter metrics");
        assert!(!gauge_metrics.is_empty(), "Should have gauge metrics");
        assert!(!histogram_metrics.is_empty(), "Should have histogram metrics");
        
        // Print metrics summary for debugging
        let summary = metrics.summary();
        println!("Metrics summary: {}", summary);
        
        // Verify the client received responses
        let client = sim.remove_component::<ClientEvent, TestClient>(client_id).unwrap();
        assert_eq!(client.responses_received, 3);
        assert_eq!(client.successful_responses, 2);
    }
}