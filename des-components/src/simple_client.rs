//! Simple client component using the new des-core Component API
//!
//! This is a minimal example showing how to create components that work
//! with the new Component trait and event-driven simulation system.

use crate::request::{RequestAttempt, RequestAttemptId, RequestId, Response};
use des_core::{Component, Key, Scheduler, SimTime, SimulationMetrics};
use std::time::Duration;

/// Simple client component that generates periodic requests
pub struct SimpleClient {
    pub name: String,
    pub request_interval: Duration,
    pub requests_sent: u64,
    pub max_requests: Option<u64>,
    pub metrics: SimulationMetrics,
    /// Counter for generating unique request IDs
    pub next_request_id: u64,
    /// Counter for generating unique attempt IDs
    pub next_attempt_id: u64,
}

/// Events that the SimpleClient can handle
#[derive(Debug)]
pub enum ClientEvent {
    /// Time to send the next request
    SendRequest,
    /// Response received from server
    ResponseReceived { response: Response },
}

impl SimpleClient {
    /// Create a new simple client
    pub fn new(name: String, request_interval: Duration) -> Self {
        Self {
            name,
            request_interval,
            requests_sent: 0,
            max_requests: None,
            metrics: SimulationMetrics::new(),
            next_request_id: 1,
            next_attempt_id: 1,
        }
    }

    /// Set maximum number of requests to send
    pub fn with_max_requests(mut self, max_requests: u64) -> Self {
        self.max_requests = Some(max_requests);
        self
    }

    /// Check if we should send more requests
    fn should_send_request(&self) -> bool {
        if let Some(max) = self.max_requests {
            self.requests_sent < max
        } else {
            true
        }
    }

    /// Schedule the next request
    fn schedule_next_request(&self, self_id: Key<ClientEvent>, scheduler: &mut Scheduler) {
        if self.should_send_request() {
            scheduler.schedule(
                SimTime::from_duration(self.request_interval),
                self_id,
                ClientEvent::SendRequest,
            );
        }
    }

    /// Get metrics for this client
    pub fn get_metrics(&self) -> &SimulationMetrics {
        &self.metrics
    }
}

impl Component for SimpleClient {
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
                    "[{}] Sending request {} (attempt {}) at {:?}",
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
                    vec![], // Empty payload for now
                );
                
                self.requests_sent += 1;
                self.next_request_id += 1;
                self.next_attempt_id += 1;
                
                // Record metrics
                self.metrics.increment_counter("requests_sent", &self.name, scheduler.time());
                self.metrics.record_gauge("total_requests", &self.name, self.requests_sent as f64, scheduler.time());
                
                // Schedule next request if we haven't reached the limit
                self.schedule_next_request(self_id, scheduler);
                
                // In a real implementation, we would send the attempt to a server
                // For now, we'll simulate an immediate successful response
                let response = Response::success(
                    attempt.id,
                    attempt.request_id,
                    scheduler.time() + SimTime::from_duration(Duration::from_millis(10)),
                    vec![], // Empty response payload
                );
                
                scheduler.schedule(
                    SimTime::from_duration(Duration::from_millis(10)),
                    self_id,
                    ClientEvent::ResponseReceived { response },
                );
            }
            ClientEvent::ResponseReceived { response } => {
                println!(
                    "[{}] Received response for attempt {} (request {}): {} at {:?}",
                    self.name,
                    response.attempt_id,
                    response.request_id,
                    if response.is_success() { "SUCCESS" } else { "FAILURE" },
                    scheduler.time()
                );
                
                // Record response metrics
                if response.is_success() {
                    self.metrics.increment_counter("responses_success", &self.name, scheduler.time());
                } else {
                    self.metrics.increment_counter("responses_failure", &self.name, scheduler.time());
                }
                
                // Calculate and record response time
                let response_time = scheduler.time().duration_since(response.completed_at);
                self.metrics.record_histogram("response_time_ms", &self.name, response_time.as_millis() as f64, scheduler.time());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use des_core::{Execute, Executor, Simulation};

    #[test]
    fn test_simple_client() {
        let mut sim = Simulation::default();
        
        // Create a client that sends 3 requests every 100ms
        let client = SimpleClient::new("test-client".to_string(), Duration::from_millis(100))
            .with_max_requests(3);
        
        let client_id = sim.add_component(client);
        
        // Schedule the first request
        sim.schedule(
            SimTime::from_duration(Duration::from_millis(100)),
            client_id,
            ClientEvent::SendRequest,
        );
        
        // Run simulation for 1 second
        Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
        
        // Verify the client sent the expected number of requests
        let client = sim.remove_component::<ClientEvent, SimpleClient>(client_id).unwrap();
        assert_eq!(client.requests_sent, 3);
    }

    #[test]
    fn test_simple_client_unlimited() {
        let mut sim = Simulation::default();
        
        // Create a client without request limit
        let client = SimpleClient::new("unlimited-client".to_string(), Duration::from_millis(50));
        
        let client_id = sim.add_component(client);
        
        // Schedule the first request
        sim.schedule(
            SimTime::from_duration(Duration::from_millis(50)),
            client_id,
            ClientEvent::SendRequest,
        );
        
        // Run simulation for 500ms (should send 10 requests)
        Executor::timed(SimTime::from_duration(Duration::from_millis(500))).execute(&mut sim);
        
        // Verify the client sent requests
        let client = sim.remove_component::<ClientEvent, SimpleClient>(client_id).unwrap();
        assert_eq!(client.requests_sent, 10);
    }
}