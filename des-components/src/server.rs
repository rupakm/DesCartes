//! Server component with queue and simulated threadpool
//!
//! This module provides a Server component that 
//! includes integrated queue support with simulated thread
//! capacity management.

use crate::queue::{Queue, QueueItem};
use crate::request::{RequestAttempt, RequestAttemptId, RequestId, Response};
use des_core::{Component, Key, Scheduler, SimTime, SimulationMetrics};
use std::time::Duration;
use uuid::Uuid;

/// Events that the unified server can handle
#[derive(Debug, Clone)]
pub enum ServerEvent {
    /// Request attempt received from client
    ProcessRequest { 
        attempt: RequestAttempt, 
        client_id: Key<ClientEvent> 
    },
    /// Request processing completed
    RequestCompleted { 
        attempt_id: RequestAttemptId,
        request_id: RequestId,
        client_id: Key<ClientEvent> 
    },
    /// Check for queued requests to process
    ProcessQueuedRequests,
}

// Re-export ClientEvent for compatibility
pub use crate::simple_client::ClientEvent;

/// Server component with queue and simulated threadpool
///
/// This server combines:
/// - Simulated thread capacity (no actual threads)
/// - Integrated queue for request buffering
/// - Configurable service time
/// - Comprehensive metrics
/// - Easy middleware wrapping support
///
/// The server processes requests up to its thread capacity. When at capacity,
/// requests are queued if a queue is configured, or rejected otherwise.
/// The "threadpool" is simulated - we track capacity but don't use real threads.
pub struct Server {
    /// Server name for identification and metrics
    pub name: String,
    /// Simulated thread capacity (maximum concurrent requests)
    pub thread_capacity: usize,
    /// Current number of active "threads" (simulated)
    pub active_threads: usize,
    /// Service time per request
    pub service_time: Duration,
    /// Optional queue for buffering requests when at capacity
    pub queue: Option<Box<dyn Queue>>,
    /// Total requests processed successfully
    pub requests_processed: u64,
    /// Total requests rejected (no capacity, no queue space)
    pub requests_rejected: u64,
    /// Total requests currently queued
    pub requests_queued: u64,
    /// Metrics collector
    pub metrics: SimulationMetrics,
}

impl Server {
    /// Create a new server
    ///
    /// # Arguments
    /// * `name` - Server name for identification
    /// * `thread_capacity` - Maximum number of simulated concurrent threads
    /// * `service_time` - Time to process each request
    pub fn new(name: String, thread_capacity: usize, service_time: Duration) -> Self {
        Self {
            name,
            thread_capacity,
            active_threads: 0,
            service_time,
            queue: None,
            requests_processed: 0,
            requests_rejected: 0,
            requests_queued: 0,
            metrics: SimulationMetrics::new(),
        }
    }

    /// Add a queue to the server for request buffering
    ///
    /// When the server is at capacity, requests will be queued instead of rejected.
    pub fn with_queue(mut self, queue: Box<dyn Queue>) -> Self {
        self.queue = Some(queue);
        self
    }

    /// Get current thread utilization (0.0 to 1.0)
    pub fn utilization(&self) -> f64 {
        self.active_threads as f64 / self.thread_capacity as f64
    }

    /// Check if server has available thread capacity
    pub fn has_capacity(&self) -> bool {
        self.active_threads < self.thread_capacity
    }

    /// Check if server can accept a request (has capacity or queue space)
    pub fn can_accept_request(&self) -> bool {
        self.has_capacity() || 
        self.queue.as_ref().map(|q| !q.is_full()).unwrap_or(false)
    }

    /// Get current queue depth
    pub fn queue_depth(&self) -> usize {
        self.queue.as_ref().map(|q| q.len()).unwrap_or(0)
    }

    /// Get metrics for this server
    pub fn get_metrics(&self) -> &SimulationMetrics {
        &self.metrics
    }

    /// Process a request attempt if capacity allows, otherwise queue or reject
    fn handle_request(
        &mut self, 
        attempt: RequestAttempt, 
        client_id: Key<ClientEvent>, 
        self_id: Key<ServerEvent>, 
        scheduler: &mut Scheduler
    ) {
        if self.has_capacity() {
            // Process immediately
            self.start_processing_request(attempt, client_id, self_id, scheduler);
        } else if let Some(ref mut queue) = self.queue {
            // Try to queue the request attempt
            let queue_item = QueueItem::with_client_id(attempt.clone(), scheduler.time(), client_id.id());
            
            match queue.enqueue(queue_item) {
                Ok(_) => {
                    self.requests_queued += 1;
                    println!(
                        "[{}] Queued request attempt {} at {:?} (queue depth: {})",
                        self.name, attempt.id, scheduler.time(), queue.len()
                    );
                    
                    // Record metrics
                    self.metrics.increment_counter("requests_queued", &self.name, scheduler.time());
                    self.metrics.record_gauge("queue_depth", &self.name, queue.len() as f64, scheduler.time());
                }
                Err(_) => {
                    // Queue is full - reject
                    self.reject_request(attempt, client_id, scheduler);
                }
            }
        } else {
            // No capacity and no queue - reject
            self.reject_request(attempt, client_id, scheduler);
        }
    }

    /// Start processing a request attempt (allocate a simulated thread)
    fn start_processing_request(
        &mut self,
        attempt: RequestAttempt,
        client_id: Key<ClientEvent>,
        self_id: Key<ServerEvent>,
        scheduler: &mut Scheduler,
    ) {
        println!(
            "[{}] Processing request attempt {} (request {}) at {:?} (threads: {}/{})",
            self.name,
            attempt.id,
            attempt.request_id,
            scheduler.time(),
            self.active_threads + 1,
            self.thread_capacity
        );

        // Allocate a simulated thread
        self.active_threads += 1;

        // Record metrics
        self.metrics.increment_counter("requests_accepted", &self.name, scheduler.time());
        self.metrics.record_gauge("active_threads", &self.name, self.active_threads as f64, scheduler.time());
        self.metrics.record_gauge("utilization", &self.name, self.utilization() * 100.0, scheduler.time());

        // Schedule completion
        scheduler.schedule(
            SimTime::from_duration(self.service_time),
            self_id,
            ServerEvent::RequestCompleted { 
                attempt_id: attempt.id,
                request_id: attempt.request_id,
                client_id 
            },
        );
    }

    /// Complete request processing (free a simulated thread)
    fn complete_request(
        &mut self,
        attempt_id: RequestAttemptId,
        request_id: RequestId,
        client_id: Key<ClientEvent>,
        self_id: Key<ServerEvent>,
        scheduler: &mut Scheduler,
    ) {
        println!(
            "[{}] Completed request attempt {} (request {}) at {:?}",
            self.name, attempt_id, request_id, scheduler.time()
        );

        // Free the simulated thread
        self.active_threads = self.active_threads.saturating_sub(1);
        self.requests_processed += 1;

        // Record metrics
        self.metrics.increment_counter("requests_completed", &self.name, scheduler.time());
        self.metrics.record_gauge("total_processed", &self.name, self.requests_processed as f64, scheduler.time());
        self.metrics.record_gauge("active_threads", &self.name, self.active_threads as f64, scheduler.time());
        self.metrics.record_gauge("utilization", &self.name, self.utilization() * 100.0, scheduler.time());
        self.metrics.record_duration("service_time", &self.name, self.service_time, scheduler.time());

        // Create and send success response to client
        let response = Response::success(
            attempt_id,
            request_id,
            scheduler.time(),
            vec![], // Empty response payload for now
        );

        scheduler.schedule(
            SimTime::from_duration(Duration::from_millis(1)),
            client_id,
            ClientEvent::ResponseReceived { response },
        );

        // Check if we can process queued requests
        if self.has_capacity() && self.queue_depth() > 0 {
            scheduler.schedule(
                SimTime::from_duration(Duration::from_millis(1)),
                self_id,
                ServerEvent::ProcessQueuedRequests,
            );
        }
    }

    /// Reject a request attempt (send failure response)
    fn reject_request(
        &mut self,
        attempt: RequestAttempt,
        client_id: Key<ClientEvent>,
        scheduler: &mut Scheduler,
    ) {
        println!(
            "[{}] Rejecting request attempt {} (request {}) - server overloaded (threads: {}/{}, queue: {})",
            self.name,
            attempt.id,
            attempt.request_id,
            self.active_threads,
            self.thread_capacity,
            self.queue_depth()
        );

        self.requests_rejected += 1;

        // Record metrics
        self.metrics.increment_counter("requests_rejected", &self.name, scheduler.time());

        // Create and send error response
        let response = Response::error(
            attempt.id,
            attempt.request_id,
            scheduler.time(),
            503, // Service Unavailable
            "Server overloaded".to_string(),
        );

        scheduler.schedule(
            SimTime::from_duration(Duration::from_millis(1)),
            client_id,
            ClientEvent::ResponseReceived { response },
        );
    }

    /// Process queued requests when capacity becomes available
    fn process_queued_requests(&mut self, self_id: Key<ServerEvent>, scheduler: &mut Scheduler) {
        while self.has_capacity() {
            // Extract the queued item first to avoid borrowing conflicts
            let queued_item = if let Some(ref mut queue) = self.queue {
                queue.dequeue()
            } else {
                None
            };

            if let Some(queued_item) = queued_item {
                let attempt = queued_item.attempt.clone();
                let queue_time = queued_item.queue_time(scheduler.time());
                
                println!(
                    "[{}] Processing queued request attempt {} (request {}) at {:?} (was queued for {:?})",
                    self.name,
                    attempt.id,
                    attempt.request_id,
                    scheduler.time(),
                    queue_time
                );

                // Reconstruct the client_id from the stored UUID
                if let Some(client_uuid) = queued_item.client_id {
                    let client_id = Key::<ClientEvent>::new_with_id(client_uuid);
                    self.start_processing_request(attempt, client_id, self_id, scheduler);
                } else {
                    // Fallback: complete the request without sending response
                    // This handles legacy queue items that don't have client_id stored
                    println!(
                        "[{}] Warning: Processing queued request attempt {} without client_id",
                        self.name, attempt.id
                    );
                    self.active_threads += 1;
                    scheduler.schedule(
                        SimTime::from_duration(self.service_time),
                        self_id,
                        ServerEvent::RequestCompleted { 
                            attempt_id: attempt.id,
                            request_id: attempt.request_id,
                            client_id: Key::<ClientEvent>::new_with_id(Uuid::nil()) 
                        },
                    );
                }
                
                // Record queue metrics after processing
                let queue_len = self.queue.as_ref().map(|q| q.len()).unwrap_or(0);
                self.metrics.record_gauge("queue_depth", &self.name, queue_len as f64, scheduler.time());
            } else {
                // Queue is empty or no queue
                break;
            }
        }
    }
}

impl Component for Server {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ServerEvent::ProcessRequest { attempt, client_id } => {
                self.handle_request(attempt.clone(), *client_id, self_id, scheduler);
            }
            ServerEvent::RequestCompleted { attempt_id, request_id, client_id } => {
                self.complete_request(*attempt_id, *request_id, *client_id, self_id, scheduler);
            }
            ServerEvent::ProcessQueuedRequests => {
                self.process_queued_requests(self_id, scheduler);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::FifoQueue;
    use des_core::{Execute, Executor, Simulation};

    /// Test client for server testing
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
                ClientEvent::SendRequest => {
                    // TestClient doesn't send requests, only receives responses
                }
            }
        }
    }

    #[test]
    fn test_server_basic() {
        let mut sim = Simulation::default();

        // Create server with 2 thread capacity and 50ms service time
        let server = Server::new("test-server".to_string(), 2, Duration::from_millis(50));
        let server_id = sim.add_component(server);

        // Create test client
        let client = TestClient::new("test-client".to_string());
        let client_id = sim.add_component(client);

        // Create and send a request attempt
        let attempt = RequestAttempt::new(
            RequestAttemptId(1),
            RequestId(1),
            1,
            SimTime::from_duration(Duration::from_millis(10)),
            vec![],
        );

        sim.schedule(
            SimTime::from_duration(Duration::from_millis(10)),
            server_id,
            ServerEvent::ProcessRequest { attempt, client_id },
        );

        // Run simulation
        Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);

        // Verify results
        let server = sim.remove_component::<ServerEvent, Server>(server_id).unwrap();
        let client = sim.remove_component::<ClientEvent, TestClient>(client_id).unwrap();

        assert_eq!(server.requests_processed, 1);
        assert_eq!(server.active_threads, 0);
        assert_eq!(client.responses_received, 1);
        assert_eq!(client.successful_responses, 1);
    }

    #[test]
    fn test_server_capacity_limit() {
        let mut sim = Simulation::default();

        // Create server with 1 thread capacity and 100ms service time
        let server = Server::new("test-server".to_string(), 1, Duration::from_millis(100));
        let server_id = sim.add_component(server);

        // Create test client
        let client = TestClient::new("test-client".to_string());
        let client_id = sim.add_component(client);

        // Create and send two request attempts simultaneously
        let attempt1 = RequestAttempt::new(
            RequestAttemptId(1),
            RequestId(1),
            1,
            SimTime::from_duration(Duration::from_millis(10)),
            vec![],
        );
        let attempt2 = RequestAttempt::new(
            RequestAttemptId(2),
            RequestId(2),
            1,
            SimTime::from_duration(Duration::from_millis(11)),
            vec![],
        );

        sim.schedule(
            SimTime::from_duration(Duration::from_millis(10)),
            server_id,
            ServerEvent::ProcessRequest { attempt: attempt1, client_id },
        );
        sim.schedule(
            SimTime::from_duration(Duration::from_millis(11)),
            server_id,
            ServerEvent::ProcessRequest { attempt: attempt2, client_id },
        );

        // Run simulation
        Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);

        // Verify results
        let server = sim.remove_component::<ServerEvent, Server>(server_id).unwrap();
        let client = sim.remove_component::<ClientEvent, TestClient>(client_id).unwrap();

        assert_eq!(server.requests_processed, 1);
        assert_eq!(server.requests_rejected, 1);
        assert_eq!(client.responses_received, 2);
        assert_eq!(client.successful_responses, 1);
    }

    #[test]
    fn test_server_with_queue() {
        let mut sim = Simulation::default();

        // Create server with queue
        let server = Server::new("test-server".to_string(), 1, Duration::from_millis(50))
            .with_queue(Box::new(FifoQueue::bounded(5)));
        let server_id = sim.add_component(server);

        // Create test client
        let client = TestClient::new("test-client".to_string());
        let client_id = sim.add_component(client);

        // Send three request attempts
        for i in 1..=3 {
            let attempt = RequestAttempt::new(
                RequestAttemptId(i),
                RequestId(i),
                1,
                SimTime::from_duration(Duration::from_millis(10 + i)),
                vec![],
            );
            sim.schedule(
                SimTime::from_duration(Duration::from_millis(10 + i)),
                server_id,
                ServerEvent::ProcessRequest { attempt, client_id },
            );
        }

        // Run simulation
        Executor::timed(SimTime::from_duration(Duration::from_millis(300))).execute(&mut sim);

        // Verify results
        let server = sim.remove_component::<ServerEvent, Server>(server_id).unwrap();

        // With queue, all requests should eventually be processed
        assert_eq!(server.requests_processed, 3);
        assert_eq!(server.requests_rejected, 0);
        assert_eq!(server.active_threads, 0);
    }

    #[test]
    fn test_server_metrics() {
        let mut sim = Simulation::default();

        let server = Server::new("metrics-server".to_string(), 4, Duration::from_millis(30));
        let server_id = sim.add_component(server);

        let client = TestClient::new("metrics-client".to_string());
        let client_id = sim.add_component(client);

        // Send multiple request attempts with enough capacity
        for i in 1..=4 {
            let attempt = RequestAttempt::new(
                RequestAttemptId(i),
                RequestId(i),
                1,
                SimTime::from_duration(Duration::from_millis(10 * i)),
                vec![],
            );
            sim.schedule(
                SimTime::from_duration(Duration::from_millis(10 * i)),
                server_id,
                ServerEvent::ProcessRequest { attempt, client_id },
            );
        }

        // Run simulation
        Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);

        // Check metrics
        let server = sim.remove_component::<ServerEvent, Server>(server_id).unwrap();
        let metrics = server.get_metrics();

        assert_eq!(server.requests_processed, 4);
        assert!(!metrics.get_metrics().is_empty());

        let server_metrics = metrics.get_component_metrics("metrics-server");
        assert!(!server_metrics.is_empty());
    }
}