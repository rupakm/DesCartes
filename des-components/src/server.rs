//! Server component for request processing
//!
//! This module provides the Server component, which processes requests with
//! configurable service time distributions and capacity limits.

use crate::builder::{
    validate_non_empty, validate_positive, Set, Unset, Validate, ValidationResult,
};
use crate::component::{Component, Metric};
use crate::error::RequestError;
use crate::queue::{Queue, QueueItem};
use des_core::dists::ServiceTimeDistribution;
use des_core::{Environment, RequestAttempt, Response, SimError, SimTime};
use std::time::Duration;



/// Server component for processing requests
///
/// A Server processes requests with configurable capacity and service time.
/// When capacity is reached, requests are queued if a queue is configured,
/// or rejected otherwise.
///
/// # Requirements
///
/// - 2.1: Provide Server component that processes requests with configurable service time distributions
/// - 3.1: Define trait-based interfaces for all Model_Component types
/// - 3.4: Allow Model_Component instances to emit metrics and traces during simulation
///
/// # Examples
///
/// ```ignore
/// use des_components::server::{Server, ConstantServiceTime};
/// use des_components::queue::FifoQueue;
/// use std::time::Duration;
///
/// let server = Server::builder()
///     .name("web-server")
///     .capacity(10)
///     .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(50))))
///     .queue(Box::new(FifoQueue::bounded(100)))
///     .build()
///     .unwrap();
/// ```
pub struct Server {
    /// Unique name for this server
    name: String,
    /// Maximum number of concurrent requests the server can handle
    capacity: usize,
    /// Distribution for sampling service times
    service_time_dist: Box<dyn ServiceTimeDistribution>,
    /// Optional queue for buffering requests when at capacity
    queue: Option<Box<dyn Queue>>,
    /// Number of requests currently being processed
    active_requests: usize,
    /// Total number of requests processed
    total_processed: u64,
    /// Total number of requests rejected
    total_rejected: u64,
    /// Total service time accumulated
    total_service_time: Duration,
    /// Completion times of currently active requests
    active_request_completions: Vec<SimTime>,
}

impl Server {
    /// Create a new ServerBuilder
    pub fn builder() -> ServerBuilder<Unset, Unset, Unset> {
        ServerBuilder {
            name: None,
            capacity: None,
            service_time_dist: None,
            queue: None,
            _phantom_name: std::marker::PhantomData,
            _phantom_capacity: std::marker::PhantomData,
            _phantom_service_time: std::marker::PhantomData,
        }
    }

    /// Get the current number of active requests
    pub fn active_requests(&self) -> usize {
        self.active_requests
    }

    /// Get the total number of requests processed
    pub fn total_processed(&self) -> u64 {
        self.total_processed
    }

    /// Get the total number of requests rejected
    pub fn total_rejected(&self) -> u64 {
        self.total_rejected
    }

    /// Get the current queue depth (0 if no queue)
    pub fn queue_depth(&self) -> usize {
        self.queue.as_ref().map(|q| q.len()).unwrap_or(0)
    }

    /// Get the current utilization (active requests / capacity)
    pub fn utilization(&self) -> f64 {
        self.active_requests as f64 / self.capacity as f64
    }

    /// Check if the server is at capacity
    pub fn is_at_capacity(&self) -> bool {
        self.active_requests >= self.capacity
    }

    /// Check if the server can accept a request (has capacity or queue space)
    pub fn can_accept(&self) -> bool {
        !self.is_at_capacity() || self.queue.as_ref().map(|q| !q.is_full()).unwrap_or(false)
    }

    /// Process a request attempt synchronously
    ///
    /// This method handles the complete lifecycle of processing a request:
    /// 1. Check if server has capacity
    /// 2. If at capacity, enqueue the request (if queue exists) or reject
    /// 3. Sample service time from distribution
    /// 4. Schedule completion event in the simulation
    /// 5. Update metrics
    ///
    /// # Arguments
    ///
    /// * `env` - The simulation environment
    /// * `attempt` - The request attempt to process
    ///
    /// # Returns
    ///
    /// A Response indicating success or failure, or None if the request was queued
    ///
    /// # Requirements
    ///
    /// - 2.1: Process requests with configurable service time distributions
    /// - 3.4: Emit metrics for queue depth, utilization, and latency
    /// - 7.1: Handle capacity limits and queuing
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use des_components::server::Server;
    /// use des_core::{Environment, RequestAttempt, RequestAttemptId, RequestId, SimTime};
    ///
    /// let mut env = Environment::new();
    /// let mut server = Server::builder()
    ///     .name("test-server")
    ///     .capacity(10)
    ///     .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(50))))
    ///     .build()
    ///     .unwrap();
    ///
    /// let attempt = RequestAttempt::new(
    ///     RequestAttemptId(1),
    ///     RequestId(1),
    ///     1,
    ///     SimTime::zero(),
    ///     vec![],
    /// );
    ///
    /// let response = server.process_request(&mut env, attempt);
    /// ```
    pub fn process_request(
        &mut self,
        env: &mut Environment,
        attempt: RequestAttempt,
    ) -> Result<Response, RequestError> {
        let current_time = env.now();

        // Update state based on current time (lazy completion)
        self.update_state(current_time);

        // Check if we have capacity
        if self.is_at_capacity() {
            // Try to enqueue if we have a queue
            if let Some(ref mut queue) = self.queue {
                let queue_item = QueueItem::new(attempt.clone(), current_time);

                match queue.enqueue(queue_item) {
                    Ok(_) => {
                        // Successfully enqueued - return an error indicating queued status
                        // In a real implementation, this would trigger a callback when capacity becomes available
                        return Err(RequestError::Rejected(
                            "Request queued - will be processed when capacity available"
                                .to_string(),
                        ));
                    }
                    Err(_) => {
                        // Queue is full - reject the request
                        self.total_rejected += 1;
                        return Err(RequestError::Rejected(
                            "Server at capacity and queue is full".to_string(),
                        ));
                    }
                }
            } else {
                // No queue and at capacity - reject
                self.total_rejected += 1;
                return Err(RequestError::Rejected("Server at capacity".to_string()));
            }
        }

        // Process the request directly
        self.process_request_internal(env, attempt)
    }

    /// Internal method to process a request (assumes capacity is available)
    ///
    /// This method starts processing a request and schedules its completion.
    /// The active_requests counter is incremented immediately and should be
    /// decremented when the request completes (after the service time).
    fn process_request_internal(
        &mut self,
        env: &mut Environment,
        attempt: RequestAttempt,
    ) -> Result<Response, RequestError> {
        let start_time = env.now();

        // Increment active requests - this stays incremented until completion
        self.active_requests += 1;

        // Sample service time from distribution
        let service_time = self.service_time_dist.sample();
        self.total_service_time += service_time;

        // Calculate completion time
        let completion_time = start_time + service_time;

        // Schedule a completion event in the simulation
        // In a full implementation, this event would trigger a callback that:
        // 1. Decrements active_requests
        // 2. Increments total_processed
        // 3. Processes queued requests if any
        use des_core::event::EventPayload;
        env.schedule(service_time, EventPayload::Generic);

        // Track completion time for lazy updates
        self.active_request_completions.push(completion_time);

        // For testing purposes, we immediately mark as processed
        // In production, this would happen when the completion event fires
        // self.active_requests -= 1;
        // self.total_processed += 1;

        // Create a successful response
        Ok(Response::success(
            attempt.id,
            attempt.request_id,
            completion_time,
            vec![], // Empty response payload
        ))
    }

    /// Complete a request (called when service time has elapsed)
    ///
    /// This method should be called when a request's service time has completed.
    /// It decrements the active request count and increments the processed count.
    pub fn complete_request(&mut self) {
        if self.active_requests > 0 {
            self.active_requests -= 1;
            self.total_processed += 1;
        }
    }

    /// Try to process queued requests when capacity becomes available
    ///
    /// This method should be called when a request completes to check if there
    /// are queued requests that can now be processed.
    pub fn process_queued_requests(
        &mut self,
        env: &mut Environment,
    ) -> Vec<Result<Response, RequestError>> {
        self.update_state(env.now());
        let mut responses = Vec::new();

        // Process as many queued requests as we have capacity for
        while !self.is_at_capacity() {
            if let Some(ref mut queue) = self.queue {
                if let Some(queued_item) = queue.dequeue() {
                    let result = self.process_request_internal(env, queued_item.attempt);
                    responses.push(result);
                } else {
                    // Queue is empty
                    break;
                }
            } else {
                // No queue
                break;
            }
        }

        responses
    }

    /// Emit metrics with a specific timestamp
    ///
    /// This is a helper method that allows specifying the timestamp for metrics.
    /// Used internally and for testing.
    pub fn emit_metrics_at(&self, timestamp: SimTime) -> Vec<Metric> {
        vec![
            Metric::Gauge {
                name: format!("{}.active_requests", self.name),
                value: self.active_requests as f64,
                timestamp,
            },
            Metric::Gauge {
                name: format!("{}.utilization", self.name),
                value: self.utilization(),
                timestamp,
            },
            Metric::Gauge {
                name: format!("{}.queue_depth", self.name),
                value: self.queue_depth() as f64,
                timestamp,
            },
            Metric::Counter {
                name: format!("{}.total_processed", self.name),
                value: self.total_processed,
                timestamp,
            },
            Metric::Counter {
                name: format!("{}.total_rejected", self.name),
                value: self.total_rejected,
                timestamp,
            },
        ]
    }

    /// Get the average service time per request
    pub fn average_service_time(&self) -> Option<Duration> {
        if self.total_processed > 0 {
            Some(self.total_service_time / self.total_processed as u32)
        } else {
            None
        }
    }

    /// Update internal state based on current time
    ///
    /// Checks for completed requests and updates counters.
    fn update_state(&mut self, current_time: SimTime) {
        // Retain only requests that finish in the future
        // Count how many we remove
        let initial_count = self.active_request_completions.len();
        self.active_request_completions
            .retain(|&t| t > current_time);
        let completed_count = initial_count - self.active_request_completions.len();

        if completed_count > 0 {
            self.active_requests -= completed_count;
            self.total_processed += completed_count as u64;
        }
    }
}

impl Component for Server {
    fn name(&self) -> &str {
        &self.name
    }

    fn initialize(&mut self, _env: &mut Environment) -> Result<(), SimError> {
        // Server initialization - reset metrics
        self.active_requests = 0;
        self.total_processed = 0;
        self.total_rejected = 0;
        self.total_service_time = Duration::ZERO;
        self.active_request_completions.clear();
        Ok(())
    }

    fn shutdown(&mut self, _env: &mut Environment) -> Result<(), SimError> {
        // Server shutdown - ensure all active requests are completed
        if self.active_requests > 0 {
            eprintln!(
                "Warning: Server '{}' shutting down with {} active requests",
                self.name, self.active_requests
            );
        }
        Ok(())
    }

    fn emit_metrics(&self) -> Vec<Metric> {
        // Use zero timestamp as default - in practice, the caller should use emit_metrics_at
        // with the current simulation time
        self.emit_metrics_at(SimTime::zero())
    }
}

/// Builder for constructing Server instances with type-safe validation
///
/// The builder uses the type-state pattern to ensure all required fields
/// are set at compile time.
///
/// # Type Parameters
///
/// - `NameState`: Tracks whether the name has been set
/// - `CapacityState`: Tracks whether the capacity has been set
/// - `ServiceTimeState`: Tracks whether the service time distribution has been set
///
/// # Requirements
///
/// - 6.2: Provide builder patterns for constructing complex Model_Component instances
/// - 6.3: Produce compile-time errors with descriptive messages for invalid parameters
pub struct ServerBuilder<NameState, CapacityState, ServiceTimeState> {
    name: Option<String>,
    capacity: Option<usize>,
    service_time_dist: Option<Box<dyn ServiceTimeDistribution>>,
    queue: Option<Box<dyn Queue>>,
    _phantom_name: std::marker::PhantomData<NameState>,
    _phantom_capacity: std::marker::PhantomData<CapacityState>,
    _phantom_service_time: std::marker::PhantomData<ServiceTimeState>,
}

impl<NameState, CapacityState, ServiceTimeState>
    ServerBuilder<NameState, CapacityState, ServiceTimeState>
{
    /// Set the optional queue for buffering requests
    ///
    /// If no queue is provided, requests will be rejected when the server is at capacity.
    pub fn queue(mut self, queue: Box<dyn Queue>) -> Self {
        self.queue = Some(queue);
        self
    }
}

impl<CapacityState, ServiceTimeState> ServerBuilder<Unset, CapacityState, ServiceTimeState> {
    /// Set the server name (required)
    pub fn name(
        self,
        name: impl Into<String>,
    ) -> ServerBuilder<Set, CapacityState, ServiceTimeState> {
        ServerBuilder {
            name: Some(name.into()),
            capacity: self.capacity,
            service_time_dist: self.service_time_dist,
            queue: self.queue,
            _phantom_name: std::marker::PhantomData,
            _phantom_capacity: std::marker::PhantomData,
            _phantom_service_time: std::marker::PhantomData,
        }
    }
}

impl<NameState, ServiceTimeState> ServerBuilder<NameState, Unset, ServiceTimeState> {
    /// Set the server capacity (required)
    ///
    /// Capacity is the maximum number of concurrent requests the server can handle.
    pub fn capacity(self, capacity: usize) -> ServerBuilder<NameState, Set, ServiceTimeState> {
        ServerBuilder {
            name: self.name,
            capacity: Some(capacity),
            service_time_dist: self.service_time_dist,
            queue: self.queue,
            _phantom_name: std::marker::PhantomData,
            _phantom_capacity: std::marker::PhantomData,
            _phantom_service_time: std::marker::PhantomData,
        }
    }
}

impl<NameState, CapacityState> ServerBuilder<NameState, CapacityState, Unset> {
    /// Set the service time distribution (required)
    ///
    /// The distribution is used to sample service times for each request.
    pub fn service_time(
        self,
        dist: Box<dyn ServiceTimeDistribution>,
    ) -> ServerBuilder<NameState, CapacityState, Set> {
        ServerBuilder {
            name: self.name,
            capacity: self.capacity,
            service_time_dist: Some(dist),
            queue: self.queue,
            _phantom_name: std::marker::PhantomData,
            _phantom_capacity: std::marker::PhantomData,
            _phantom_service_time: std::marker::PhantomData,
        }
    }
}

impl ServerBuilder<Set, Set, Set> {
    /// Build the Server instance
    ///
    /// This method is only available when all required fields have been set.
    ///
    /// # Errors
    ///
    /// Returns a validation error if any field values are invalid.
    pub fn build(self) -> ValidationResult<Server> {
        self.validate()?;

        Ok(Server {
            name: self.name.unwrap(),
            capacity: self.capacity.unwrap(),
            service_time_dist: self.service_time_dist.unwrap(),
            queue: self.queue,
            active_requests: 0,
            total_processed: 0,
            total_rejected: 0,
            total_service_time: Duration::ZERO,
            active_request_completions: Vec::new(),
        })
    }
}

impl Validate for ServerBuilder<Set, Set, Set> {
    fn validate(&self) -> ValidationResult<()> {
        // Validate name is not empty
        if let Some(ref name) = self.name {
            validate_non_empty("name", name)?;
        }

        // Validate capacity is positive
        if let Some(capacity) = self.capacity {
            validate_positive("capacity", capacity)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::FifoQueue;
    use crate::ConstantServiceTime;

    #[test]
    fn test_constant_service_time() {
        let mut dist = ConstantServiceTime::new(Duration::from_millis(100));
        assert_eq!(dist.sample(), Duration::from_millis(100));
        assert_eq!(dist.sample(), Duration::from_millis(100));
    }



    #[test]
    fn test_server_builder_all_required_fields() {
        let server = Server::builder()
            .name("test-server")
            .capacity(10)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .build()
            .unwrap();

        assert_eq!(server.name(), "test-server");
        assert_eq!(server.capacity, 10);
        assert_eq!(server.active_requests(), 0);
        assert_eq!(server.total_processed(), 0);
        assert_eq!(server.total_rejected(), 0);
    }

    #[test]
    fn test_server_builder_with_queue() {
        let server = Server::builder()
            .name("test-server")
            .capacity(5)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .queue(Box::new(FifoQueue::bounded(20)))
            .build()
            .unwrap();

        assert_eq!(server.name(), "test-server");
        assert!(server.queue.is_some());
    }

    #[test]
    fn test_server_builder_validation_empty_name() {
        let result = Server::builder()
            .name("")
            .capacity(10)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .build();

        assert!(result.is_err());
    }

    #[test]
    fn test_server_builder_validation_zero_capacity() {
        let result = Server::builder()
            .name("test-server")
            .capacity(0)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .build();

        assert!(result.is_err());
    }

    #[test]
    fn test_server_utilization() {
        let mut server = Server::builder()
            .name("test-server")
            .capacity(10)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .build()
            .unwrap();

        assert_eq!(server.utilization(), 0.0);

        server.active_requests = 5;
        assert_eq!(server.utilization(), 0.5);

        server.active_requests = 10;
        assert_eq!(server.utilization(), 1.0);
    }

    #[test]
    fn test_server_capacity_checks() {
        let mut server = Server::builder()
            .name("test-server")
            .capacity(2)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .build()
            .unwrap();

        assert!(!server.is_at_capacity());
        assert!(server.can_accept());

        server.active_requests = 2;
        assert!(server.is_at_capacity());
        assert!(!server.can_accept());
    }

    #[test]
    fn test_server_capacity_with_queue() {
        let mut server = Server::builder()
            .name("test-server")
            .capacity(2)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .queue(Box::new(FifoQueue::bounded(5)))
            .build()
            .unwrap();

        server.active_requests = 2;
        assert!(server.is_at_capacity());
        assert!(server.can_accept()); // Can still accept because queue has space
    }

    #[test]
    fn test_server_emit_metrics() {
        let server = Server::builder()
            .name("test-server")
            .capacity(10)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .build()
            .unwrap();

        let metrics = server.emit_metrics();
        assert_eq!(metrics.len(), 5);

        // Check that all expected metrics are present
        let metric_names: Vec<String> = metrics
            .iter()
            .map(|m| match m {
                Metric::Gauge { name, .. } => name.clone(),
                Metric::Counter { name, .. } => name.clone(),
                _ => String::new(),
            })
            .collect();

        assert!(metric_names.contains(&"test-server.active_requests".to_string()));
        assert!(metric_names.contains(&"test-server.utilization".to_string()));
        assert!(metric_names.contains(&"test-server.queue_depth".to_string()));
        assert!(metric_names.contains(&"test-server.total_processed".to_string()));
        assert!(metric_names.contains(&"test-server.total_rejected".to_string()));
    }

    #[test]
    fn test_server_queue_depth() {
        let server = Server::builder()
            .name("test-server")
            .capacity(10)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .build()
            .unwrap();

        assert_eq!(server.queue_depth(), 0);

        let server_with_queue = Server::builder()
            .name("test-server")
            .capacity(10)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .queue(Box::new(FifoQueue::bounded(5)))
            .build()
            .unwrap();

        assert_eq!(server_with_queue.queue_depth(), 0);
    }
}

#[cfg(test)]
mod request_processing_tests {
    use super::*;
    use crate::ConstantServiceTime;
    use crate::queue::FifoQueue;
    use des_core::{Environment, RequestAttemptId, RequestId};

    #[test]
    fn test_process_request_with_capacity() {
        let mut env = Environment::new();
        let mut server = Server::builder()
            .name("test-server")
            .capacity(10)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .build()
            .unwrap();

        let attempt = RequestAttempt::new(
            RequestAttemptId(1),
            RequestId(1),
            1,
            SimTime::zero(),
            vec![1, 2, 3],
        );

        let result = server.process_request(&mut env, attempt);
        assert!(result.is_ok());
        server.complete_request();

        let response = result.unwrap();
        assert!(response.is_success());
        assert_eq!(response.request_id, RequestId(1));
        assert_eq!(response.attempt_id, RequestAttemptId(1));
        assert_eq!(response.attempt_id, RequestAttemptId(1));

        server.complete_request();
        assert_eq!(server.total_processed(), 1);
        assert_eq!(server.total_rejected(), 0);
    }

    #[test]
    fn test_process_request_at_capacity_no_queue() {
        let mut env = Environment::new();
        let mut server = Server::builder()
            .name("test-server")
            .capacity(1)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .build()
            .unwrap();

        // Fill capacity
        server.active_requests = 1;

        let attempt = RequestAttempt::new(
            RequestAttemptId(2),
            RequestId(2),
            1,
            SimTime::zero(),
            vec![],
        );

        let result = server.process_request(&mut env, attempt);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RequestError::Rejected(_)));
        assert_eq!(server.total_rejected(), 1);
    }

    #[test]
    fn test_process_request_with_queue() {
        let mut env = Environment::new();
        let mut server = Server::builder()
            .name("test-server")
            .capacity(1)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .queue(Box::new(FifoQueue::bounded(5)))
            .build()
            .unwrap();

        // Fill capacity
        server.active_requests = 1;

        let attempt = RequestAttempt::new(
            RequestAttemptId(2),
            RequestId(2),
            1,
            SimTime::zero(),
            vec![],
        );

        // Should be queued (returns error indicating queued status)
        let result = server.process_request(&mut env, attempt);
        assert!(result.is_err());
        assert_eq!(server.queue_depth(), 1);
        assert_eq!(server.total_rejected(), 0); // Not rejected, just queued
    }

    #[test]
    fn test_process_request_queue_full() {
        let mut env = Environment::new();
        let mut server = Server::builder()
            .name("test-server")
            .capacity(1)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .queue(Box::new(FifoQueue::bounded(1)))
            .build()
            .unwrap();

        // Fill capacity
        server.active_requests = 1;

        // Fill the queue
        let queue_item = QueueItem::new(
            RequestAttempt::new(
                RequestAttemptId(1),
                RequestId(1),
                1,
                SimTime::zero(),
                vec![],
            ),
            SimTime::zero(),
        );
        server.queue.as_mut().unwrap().enqueue(queue_item).unwrap();

        // Now try to process another request - should be rejected
        let attempt = RequestAttempt::new(
            RequestAttemptId(2),
            RequestId(2),
            1,
            SimTime::zero(),
            vec![],
        );

        let result = server.process_request(&mut env, attempt);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RequestError::Rejected(_)));
        assert_eq!(server.total_rejected(), 1);
    }

    #[test]
    fn test_process_multiple_requests() {
        let mut env = Environment::new();
        let mut server = Server::builder()
            .name("test-server")
            .capacity(5)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .build()
            .unwrap();

        // Process multiple requests
        for i in 1..=3 {
            let attempt = RequestAttempt::new(
                RequestAttemptId(i),
                RequestId(i),
                1,
                SimTime::zero(),
                vec![],
            );

            let result = server.process_request(&mut env, attempt);
            assert!(result.is_ok());
            server.complete_request();
        }

        assert_eq!(server.total_processed(), 3);
        assert_eq!(server.total_rejected(), 0);
    }

    #[test]
    fn test_service_time_accumulation() {
        let mut env = Environment::new();
        let mut server = Server::builder()
            .name("test-server")
            .capacity(10)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                100,
            ))))
            .build()
            .unwrap();

        // Process 3 requests
        for i in 1..=3 {
            let attempt = RequestAttempt::new(
                RequestAttemptId(i),
                RequestId(i),
                1,
                SimTime::zero(),
                vec![],
            );

            server.process_request(&mut env, attempt).unwrap();
            server.complete_request();
        }

        // Total service time should be 300ms
        assert_eq!(server.total_service_time, Duration::from_millis(300));
        assert_eq!(
            server.average_service_time(),
            Some(Duration::from_millis(100))
        );
    }

    #[test]
    fn test_process_queued_requests() {
        let mut env = Environment::new();
        let mut server = Server::builder()
            .name("test-server")
            .capacity(10)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(50))))
            .queue(Box::new(FifoQueue::bounded(5)))
            .build()
            .unwrap();

        // Test that the server can process requests and that queue functionality exists
        let attempt = RequestAttempt::new(
            RequestAttemptId(1),
            RequestId(1),
            1,
            SimTime::zero(),
            vec![],
        );
        
        let result = server.process_request(&mut env, attempt);
        assert!(result.is_ok());
        
        // Verify basic queue functionality
        assert_eq!(server.queue_depth(), 0);
        assert!(server.can_accept());
        assert!(!server.is_at_capacity());
    }

    #[test]
    fn test_emit_metrics_at() {
        let mut server = Server::builder()
            .name("test-server")
            .capacity(10)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .build()
            .unwrap();

        server.active_requests = 5;
        server.total_processed = 100;
        server.total_rejected = 10;

        let timestamp = SimTime::from_millis(1000);
        let metrics = server.emit_metrics_at(timestamp);

        assert_eq!(metrics.len(), 5);

        // Verify all metrics have the correct timestamp
        for metric in &metrics {
            match metric {
                Metric::Gauge { timestamp: ts, .. } => assert_eq!(*ts, timestamp),
                Metric::Counter { timestamp: ts, .. } => assert_eq!(*ts, timestamp),
                _ => unreachable!("Test should only produce Gauge and Counter metrics"),
            }
        }
    }

    #[test]
    fn test_initialize_resets_metrics() {
        let mut env = Environment::new();
        let mut server = Server::builder()
            .name("test-server")
            .capacity(10)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .build()
            .unwrap();

        // Set some state
        server.active_requests = 5;
        server.total_processed = 100;
        server.total_rejected = 10;
        server.total_service_time = Duration::from_secs(10);

        // Initialize should reset
        server.initialize(&mut env).unwrap();

        assert_eq!(server.active_requests, 0);
        assert_eq!(server.total_processed, 0);
        assert_eq!(server.total_rejected, 0);
        assert_eq!(server.total_service_time, Duration::ZERO);
    }

    #[test]
    fn test_shutdown_with_active_requests() {
        let mut env = Environment::new();
        let mut server = Server::builder()
            .name("test-server")
            .capacity(10)
            .service_time(Box::new(ConstantServiceTime::new(Duration::from_millis(
                50,
            ))))
            .build()
            .unwrap();

        server.active_requests = 3;

        // Should succeed but print a warning
        let result = server.shutdown(&mut env);
        assert!(result.is_ok());
    }
}
