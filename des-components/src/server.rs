//! Server component with queue and simulated threadpool
//!
//! This module provides a Server component that
//! includes integrated queue support with simulated thread
//! capacity management.

use crate::queue::{Queue, QueueItem};
use descartes_core::dists::{RequestContext, ServiceTimeDistribution};
use descartes_core::{
    Component, Key, RequestAttempt, RequestAttemptId, RequestId, Response, Scheduler, SimTime,
};
use descartes_metrics::SimulationMetrics;
use std::time::Duration;
use uuid::Uuid;

/// Helper function to format simulation time as duration since start
fn format_sim_time(sim_time: SimTime) -> String {
    let duration = sim_time.as_duration();
    let total_ms = duration.as_millis();
    let seconds = total_ms / 1000;
    let ms = total_ms % 1000;

    if seconds > 0 {
        format!("{seconds}.{ms:03}s")
    } else {
        format!("{ms}ms")
    }
}

/// Events that the unified server can handle
#[derive(Debug, Clone)]
pub enum ServerEvent {
    /// Request attempt received from client
    ProcessRequest {
        attempt: RequestAttempt,
        client_id: Key<ClientEvent>,
    },
    /// Request processing completed
    RequestCompleted {
        attempt_id: RequestAttemptId,
        request_id: RequestId,
        client_id: Key<ClientEvent>,
        request_payload: Vec<u8>,
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
/// - Configurable service time distribution (can be request-dependent)
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
    /// Service time distribution for calculating processing times
    pub service_time_distribution: Box<dyn ServiceTimeDistribution>,
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
    /// Create a new server with a service time distribution
    ///
    /// # Arguments
    /// * `name` - Server name for identification
    /// * `thread_capacity` - Maximum number of simulated concurrent threads
    /// * `service_time_distribution` - Distribution for calculating service times
    pub fn new(
        name: String,
        thread_capacity: usize,
        service_time_distribution: Box<dyn ServiceTimeDistribution>,
    ) -> Self {
        Self {
            name,
            thread_capacity,
            active_threads: 0,
            service_time_distribution,
            queue: None,
            requests_processed: 0,
            requests_rejected: 0,
            requests_queued: 0,
            metrics: SimulationMetrics::new(),
        }
    }

    /// Create a new server with constant service time (backward compatibility)
    ///
    /// # Arguments
    /// * `name` - Server name for identification
    /// * `thread_capacity` - Maximum number of simulated concurrent threads
    /// * `service_time` - Fixed time to process each request
    pub fn with_constant_service_time(
        name: String,
        thread_capacity: usize,
        service_time: Duration,
    ) -> Self {
        use descartes_core::dists::ConstantServiceTime;
        Self::new(
            name,
            thread_capacity,
            Box::new(ConstantServiceTime::new(service_time)),
        )
    }

    /// Create a new server with exponential service time distribution
    ///
    /// # Arguments
    /// * `name` - Server name for identification
    /// * `thread_capacity` - Maximum number of simulated concurrent threads
    /// * `mean_service_time` - Mean service time for the exponential distribution
    pub fn with_exponential_service_time(
        name: String,
        thread_capacity: usize,
        mean_service_time: Duration,
    ) -> Self {
        use descartes_core::dists::ExponentialDistribution;
        let rate = 1.0 / mean_service_time.as_secs_f64();
        Self::new(
            name,
            thread_capacity,
            Box::new(ExponentialDistribution::new(rate)),
        )
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
        self.has_capacity() || self.queue.as_ref().map(|q| !q.is_full()).unwrap_or(false)
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
        scheduler: &mut Scheduler,
    ) {
        if self.has_capacity() {
            // Process immediately
            self.start_processing_request(attempt, client_id, self_id, scheduler);
        } else if let Some(ref mut queue) = self.queue {
            // Try to queue the request attempt
            let queue_item =
                QueueItem::with_client_id(attempt.clone(), scheduler.time(), client_id.id());

            match queue.enqueue(queue_item) {
                Ok(_) => {
                    self.requests_queued += 1;
                    println!(
                        "[{}] [{}] Queued request attempt {} (queue depth: {})",
                        format_sim_time(scheduler.time()),
                        self.name,
                        attempt.id,
                        queue.len()
                    );

                    // Record metrics
                    self.metrics
                        .increment_counter("requests_queued", &[("component", &self.name)]);
                    self.metrics.record_gauge(
                        "queue_depth",
                        queue.len() as f64,
                        &[("component", &self.name)],
                    );
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
            "[{}] [{}] Processing request attempt {} (request {}) (threads: {}/{})",
            format_sim_time(scheduler.time()),
            self.name,
            attempt.id,
            attempt.request_id,
            self.active_threads + 1,
            self.thread_capacity
        );

        // Allocate a simulated thread
        self.active_threads += 1;

        // Calculate service time based on request context
        let request_context = self.build_request_context(&attempt);
        let service_time = self
            .service_time_distribution
            .sample_service_time(&request_context);

        // Record metrics
        self.metrics
            .increment_counter("requests_accepted", &[("component", &self.name)]);
        self.metrics.record_gauge(
            "active_threads",
            self.active_threads as f64,
            &[("component", &self.name)],
        );
        self.metrics.record_gauge(
            "utilization",
            self.utilization() * 100.0,
            &[("component", &self.name)],
        );

        // Schedule completion
        scheduler.schedule(
            SimTime::from_duration(service_time),
            self_id,
            ServerEvent::RequestCompleted {
                attempt_id: attempt.id,
                request_id: attempt.request_id,
                client_id,
                request_payload: attempt.payload.clone(),
            },
        );
    }

    /// Complete request processing (free a simulated thread)
    fn complete_request(
        &mut self,
        attempt_id: RequestAttemptId,
        request_id: RequestId,
        client_id: Key<ClientEvent>,
        request_payload: Vec<u8>,
        self_id: Key<ServerEvent>,
        scheduler: &mut Scheduler,
    ) {
        println!(
            "[{}] [{}] Completed request attempt {} (request {})",
            format_sim_time(scheduler.time()),
            self.name,
            attempt_id,
            request_id
        );

        // Free the simulated thread
        self.active_threads = self.active_threads.saturating_sub(1);
        self.requests_processed += 1;

        // Record metrics
        self.metrics
            .increment_counter("requests_completed", &[("component", &self.name)]);
        self.metrics.record_gauge(
            "total_processed",
            self.requests_processed as f64,
            &[("component", &self.name)],
        );
        self.metrics.record_gauge(
            "active_threads",
            self.active_threads as f64,
            &[("component", &self.name)],
        );
        self.metrics.record_gauge(
            "utilization",
            self.utilization() * 100.0,
            &[("component", &self.name)],
        );

        // Create and send success response to client - echo the request payload
        let response = Response::success(
            attempt_id,
            request_id,
            scheduler.time(),
            request_payload, // Echo the request payload
        );

        scheduler.schedule(
            SimTime::from_duration(Duration::from_millis(1)),
            client_id,
            ClientEvent::ResponseReceived { response },
        );

        // Check if we can process queued requests
        if self.has_capacity() && self.queue_depth() > 0 {
            scheduler.schedule(
                SimTime::from_duration(Duration::from_millis(0)), // immediately send response
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
            "[{}] [{}] Rejecting request attempt {} (request {}) - server overloaded (threads: {}/{}, queue: {})",
            format_sim_time(scheduler.time()),
            self.name,
            attempt.id,
            attempt.request_id,
            self.active_threads,
            self.thread_capacity,
            self.queue_depth()
        );

        self.requests_rejected += 1;

        // Record metrics
        self.metrics
            .increment_counter("requests_rejected", &[("component", &self.name)]);

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
                    "[{}] [{}] Processing queued request attempt {} (request {}) (was queued for {:.0}ms)",
                    format_sim_time(scheduler.time()),
                    self.name,
                    attempt.id,
                    attempt.request_id,
                    queue_time.as_millis()
                );

                // Reconstruct the client_id from the stored UUID
                if let Some(client_uuid) = queued_item.client_id {
                    let client_id = Key::<ClientEvent>::new_with_id(client_uuid);
                    self.start_processing_request(attempt, client_id, self_id, scheduler);
                } else {
                    // Fallback: complete the request without sending response
                    // This handles legacy queue items that don't have client_id stored
                    println!(
                        "[{}] [{}] Warning: Processing queued request attempt {} without client_id",
                        format_sim_time(scheduler.time()),
                        self.name,
                        attempt.id
                    );
                    self.active_threads += 1;

                    // Calculate service time for this request
                    let request_context = self.build_request_context(&attempt);
                    let service_time = self
                        .service_time_distribution
                        .sample_service_time(&request_context);

                    scheduler.schedule(
                        SimTime::from_duration(service_time),
                        self_id,
                        ServerEvent::RequestCompleted {
                            attempt_id: attempt.id,
                            request_id: attempt.request_id,
                            client_id: Key::<ClientEvent>::new_with_id(Uuid::nil()),
                            request_payload: attempt.payload.clone(),
                        },
                    );
                }

                // Record queue metrics after processing
                let queue_len = self.queue.as_ref().map(|q| q.len()).unwrap_or(0);
                self.metrics.record_gauge(
                    "queue_depth",
                    queue_len as f64,
                    &[("component", &self.name)],
                );
            } else {
                // Queue is empty or no queue
                break;
            }
        }
    }

    /// Build request context from a request attempt
    fn build_request_context(&self, request: &RequestAttempt) -> RequestContext {
        // Parse the serialized HTTP request payload
        self.parse_request_payload(&request.payload)
    }

    /// Parse HTTP request payload into RequestContext
    fn parse_request_payload(&self, payload: &[u8]) -> RequestContext {
        // Parse the HTTP request format created by serialize_http_request
        let payload_str = String::from_utf8_lossy(payload);
        let lines: Vec<&str> = payload_str.split("\r\n").collect();

        if lines.is_empty() {
            return RequestContext::default();
        }

        // Parse request line: "METHOD URI HTTP/1.1"
        let request_line_parts: Vec<&str> = lines[0].split_whitespace().collect();
        let method = request_line_parts.first().unwrap_or(&"GET").to_string();
        let uri = request_line_parts.get(1).unwrap_or(&"/").to_string();

        // Parse headers
        let mut headers = std::collections::HashMap::new();
        let mut body_start = lines.len();

        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.is_empty() {
                body_start = i + 1;
                break;
            }
            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim().to_string();
                let value = line[colon_pos + 1..].trim().to_string();
                headers.insert(key, value);
            }
        }

        // Extract body
        let body = if body_start < lines.len() {
            lines[body_start..].join("\r\n").into_bytes()
        } else {
            Vec::new()
        };

        RequestContext {
            method,
            uri,
            headers,
            body_size: body.len(),
            payload: body,
            client_info: None,
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
            ServerEvent::RequestCompleted {
                attempt_id,
                request_id,
                client_id,
                request_payload,
            } => {
                self.complete_request(
                    *attempt_id,
                    *request_id,
                    *client_id,
                    request_payload.clone(),
                    self_id,
                    scheduler,
                );
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
    use descartes_core::{Execute, Executor, Simulation};

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
                        if response.is_success() {
                            "SUCCESS"
                        } else {
                            "FAILURE"
                        },
                        scheduler.time()
                    );
                }
                ClientEvent::SendRequest => {
                    // TestClient doesn't send requests, only receives responses
                }
                ClientEvent::RequestTimeout { .. } => {
                    // TestClient doesn't handle timeouts
                }
                ClientEvent::RetryRequest { .. } => {
                    // TestClient doesn't handle retries
                }
            }
        }
    }

    #[test]
    fn test_server_basic() {
        let mut sim = Simulation::default();

        // Create server with 2 thread capacity and 50ms service time
        let server = Server::with_constant_service_time(
            "test-server".to_string(),
            2,
            Duration::from_millis(50),
        );
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
        let server = sim
            .remove_component::<ServerEvent, Server>(server_id)
            .unwrap();
        let client = sim
            .remove_component::<ClientEvent, TestClient>(client_id)
            .unwrap();

        assert_eq!(server.requests_processed, 1);
        assert_eq!(server.active_threads, 0);
        assert_eq!(client.responses_received, 1);
        assert_eq!(client.successful_responses, 1);
    }

    #[test]
    fn test_server_capacity_limit() {
        let mut sim = Simulation::default();

        // Create server with 1 thread capacity and 100ms service time
        let server = Server::with_constant_service_time(
            "test-server".to_string(),
            1,
            Duration::from_millis(100),
        );
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
            ServerEvent::ProcessRequest {
                attempt: attempt1,
                client_id,
            },
        );
        sim.schedule(
            SimTime::from_duration(Duration::from_millis(11)),
            server_id,
            ServerEvent::ProcessRequest {
                attempt: attempt2,
                client_id,
            },
        );

        // Run simulation
        Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);

        // Verify results
        let server = sim
            .remove_component::<ServerEvent, Server>(server_id)
            .unwrap();
        let client = sim
            .remove_component::<ClientEvent, TestClient>(client_id)
            .unwrap();

        assert_eq!(server.requests_processed, 1);
        assert_eq!(server.requests_rejected, 1);
        assert_eq!(client.responses_received, 2);
        assert_eq!(client.successful_responses, 1);
    }

    #[test]
    fn test_server_with_queue() {
        let mut sim = Simulation::default();

        // Create server with queue
        let server = Server::with_constant_service_time(
            "test-server".to_string(),
            1,
            Duration::from_millis(50),
        )
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
        let server = sim
            .remove_component::<ServerEvent, Server>(server_id)
            .unwrap();

        // With queue, all requests should eventually be processed
        assert_eq!(server.requests_processed, 3);
        assert_eq!(server.requests_rejected, 0);
        assert_eq!(server.active_threads, 0);
    }

    #[test]
    fn test_server_metrics() {
        let mut sim = Simulation::default();

        let server = Server::with_constant_service_time(
            "metrics-server".to_string(),
            4,
            Duration::from_millis(30),
        );
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

        // Check that server processed requests
        let server = sim
            .remove_component::<ServerEvent, Server>(server_id)
            .unwrap();

        assert_eq!(server.requests_processed, 4);
        // Note: With the new metrics system, metrics are recorded globally
        // and can be accessed through the metrics registry, not through the component
    }
}
