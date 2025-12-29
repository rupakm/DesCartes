//! Simple client component using the new des-core Component API
//!
//! This is a minimal example showing how to create components that work
//! with the new Component trait and event-driven simulation system.
//! 
//! The client now supports configurable retry policies for handling request failures.

use des_core::{Component, Key, Scheduler, SimTime, RequestAttempt, RequestAttemptId, RequestId, Response};
use des_metrics::SimulationMetrics;
use crate::retry_policy::{RetryPolicy, ExponentialBackoffPolicy};
use std::time::Duration;
use std::collections::HashMap;

/// Simple client component that generates periodic requests with retry support
pub struct SimpleClient<P: RetryPolicy> {
    pub name: String,
    pub request_interval: Duration,
    pub requests_sent: u64,
    pub max_requests: Option<u64>,
    pub metrics: SimulationMetrics,
    /// Counter for generating unique request IDs
    pub next_request_id: u64,
    /// Counter for generating unique attempt IDs
    pub next_attempt_id: u64,
    /// Retry policy for handling failed requests
    pub retry_policy: P,
    /// Timeout duration for requests
    pub request_timeout: Duration,
    /// Active requests awaiting responses (request_id -> (attempt, retry_policy_state))
    pub active_requests: HashMap<RequestId, (RequestAttempt, P)>,
}

/// Events that the SimpleClient can handle
#[derive(Debug)]
pub enum ClientEvent {
    /// Time to send the next request
    SendRequest,
    /// Response received from server
    ResponseReceived { response: Response },
    /// Request timed out
    RequestTimeout { request_id: RequestId, attempt_id: RequestAttemptId },
    /// Time to retry a failed request
    RetryRequest { request_id: RequestId },
}

impl<P: RetryPolicy> SimpleClient<P> {
    /// Create a new simple client with a retry policy
    pub fn new(name: String, request_interval: Duration, retry_policy: P) -> Self {
        Self {
            name,
            request_interval,
            requests_sent: 0,
            max_requests: None,
            metrics: SimulationMetrics::new(),
            next_request_id: 1,
            next_attempt_id: 1,
            retry_policy,
            request_timeout: Duration::from_secs(5), // Default 5 second timeout
            active_requests: HashMap::new(),
        }
    }

    /// Set maximum number of requests to send
    pub fn with_max_requests(mut self, max_requests: u64) -> Self {
        self.max_requests = Some(max_requests);
        self
    }

    /// Set request timeout duration
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Get metrics for this client
    pub fn get_metrics(&self) -> &SimulationMetrics {
        &self.metrics
    }

    /// Send a new request attempt
    fn send_attempt(
        &mut self,
        request_id: RequestId,
        attempt_number: usize,
        scheduler: &mut Scheduler,
        self_id: Key<ClientEvent>,
    ) -> RequestAttempt {
        let attempt_id = self.next_attempt_id;
        self.next_attempt_id += 1;

        let attempt = RequestAttempt::new(
            RequestAttemptId(attempt_id),
            request_id,
            attempt_number,
            scheduler.time(),
            vec![], // Empty payload for now
        );

        println!(
            "[{}] Sending request {} attempt {} (attempt ID {}) at {:?}",
            self.name,
            request_id.0,
            attempt_number,
            attempt_id,
            scheduler.time()
        );

        // Schedule timeout for this attempt
        scheduler.schedule(
            SimTime::from_duration(self.request_timeout),
            self_id,
            ClientEvent::RequestTimeout {
                request_id,
                attempt_id: RequestAttemptId(attempt_id),
            },
        );

        // Record metrics
        self.metrics.increment_counter("attempts_sent", &[("component", &self.name)]);

        // In a real implementation, we would send the attempt to a server
        // For now, we'll simulate a response with some probability of failure
        let success_probability = 0.7; // 70% success rate
        let will_succeed = rand::random::<f64>() < success_probability;
        
        let response_delay = if will_succeed {
            Duration::from_millis(50 + (rand::random::<u64>() % 100)) // 50-150ms
        } else {
            Duration::from_millis(200 + (rand::random::<u64>() % 300)) // 200-500ms before failure
        };

        let response = if will_succeed {
            Response::success(
                RequestAttemptId(attempt_id),
                request_id,
                scheduler.time() + SimTime::from_duration(response_delay),
                vec![], // Empty response payload
            )
        } else {
            Response::error(
                RequestAttemptId(attempt_id),
                request_id,
                scheduler.time() + SimTime::from_duration(response_delay),
                500,
                "Simulated server error".to_string(),
            )
        };

        scheduler.schedule(
            SimTime::from_duration(response_delay),
            self_id,
            ClientEvent::ResponseReceived { response },
        );

        attempt
    }

    /// Handle a response and determine if retry is needed
    fn handle_response(
        &mut self,
        response: &Response,
        scheduler: &mut Scheduler,
        self_id: Key<ClientEvent>,
    ) {
        let request_id = response.request_id;
        
        if let Some((attempt, mut retry_policy)) = self.active_requests.remove(&request_id) {
            let is_success = response.is_success();
            
            println!(
                "[{}] Received response for request {} attempt {}: {} at {:?}",
                self.name,
                request_id.0,
                attempt.attempt_number,
                if is_success { "SUCCESS" } else { "FAILURE" },
                scheduler.time()
            );

            // Record response metrics
            if is_success {
                self.metrics.increment_counter("responses_success", &[("component", &self.name)]);
                
                // Calculate and record response time
                let response_time = scheduler.time().duration_since(attempt.started_at);
                self.metrics.record_histogram("response_time_ms", response_time.as_millis() as f64, &[("component", &self.name)]);
            } else {
                self.metrics.increment_counter("responses_failure", &[("component", &self.name)]);
                
                // Check if we should retry
                if let Some(retry_delay) = retry_policy.should_retry(&attempt, Some(response)) {
                    println!(
                        "[{}] Scheduling retry for request {} in {:?}",
                        self.name,
                        request_id.0,
                        retry_delay
                    );
                    
                    // Store the retry policy state for this request
                    self.active_requests.insert(request_id, (attempt, retry_policy));
                    
                    // Schedule the retry
                    scheduler.schedule(
                        SimTime::from_duration(retry_delay),
                        self_id,
                        ClientEvent::RetryRequest { request_id },
                    );
                    
                    self.metrics.increment_counter("retries_scheduled", &[("component", &self.name)]);
                } else {
                    println!(
                        "[{}] Request {} failed permanently (no more retries)",
                        self.name,
                        request_id.0
                    );
                    self.metrics.increment_counter("requests_failed_permanently", &[("component", &self.name)]);
                }
            }
        }
    }

    /// Handle a request timeout
    fn handle_timeout(
        &mut self,
        request_id: RequestId,
        attempt_id: RequestAttemptId,
        scheduler: &mut Scheduler,
        self_id: Key<ClientEvent>,
    ) {
        if let Some((attempt, retry_policy)) = self.active_requests.get_mut(&request_id) {
            // Only handle timeout if this is the current attempt
            if attempt.id == attempt_id {
                println!(
                    "[{}] Request {} attempt {} timed out at {:?}",
                    self.name,
                    request_id.0,
                    attempt.attempt_number,
                    scheduler.time()
                );

                self.metrics.increment_counter("requests_timeout", &[("component", &self.name)]);

                // Check if we should retry on timeout
                if let Some(retry_delay) = retry_policy.should_retry(attempt, None) {
                    println!(
                        "[{}] Scheduling retry for timed out request {} in {:?}",
                        self.name,
                        request_id.0,
                        retry_delay
                    );
                    
                    // Schedule the retry
                    scheduler.schedule(
                        SimTime::from_duration(retry_delay),
                        self_id,
                        ClientEvent::RetryRequest { request_id },
                    );
                    
                    self.metrics.increment_counter("retries_scheduled", &[("component", &self.name)]);
                } else {
                    println!(
                        "[{}] Request {} timed out permanently (no more retries)",
                        self.name,
                        request_id.0
                    );
                    self.active_requests.remove(&request_id);
                    self.metrics.increment_counter("requests_failed_permanently", &[("component", &self.name)]);
                }
            }
        }
    }

    /// Handle a retry request
    fn handle_retry(
        &mut self,
        request_id: RequestId,
        scheduler: &mut Scheduler,
        self_id: Key<ClientEvent>,
    ) {
        if let Some((mut attempt, retry_policy)) = self.active_requests.remove(&request_id) {
            // Create a new attempt for the retry
            attempt.attempt_number += 1;
            let new_attempt = self.send_attempt(request_id, attempt.attempt_number, scheduler, self_id);
            
            // Store the updated state
            self.active_requests.insert(request_id, (new_attempt, retry_policy));
        }
    }
}

impl<P: RetryPolicy> Component for SimpleClient<P> {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                let request_id = RequestId(self.next_request_id);
                self.next_request_id += 1;
                
                // Reset retry policy for new request
                let mut retry_policy = self.retry_policy.clone();
                retry_policy.reset();
                
                // Send the first attempt
                let attempt = self.send_attempt(request_id, 1, scheduler, self_id);
                
                // Store the request state
                self.active_requests.insert(request_id, (attempt, retry_policy));
                
                self.requests_sent += 1;
                self.metrics.increment_counter("requests_sent", &[("component", &self.name)]);
                self.metrics.record_gauge("total_requests", self.requests_sent as f64, &[("component", &self.name)]);
            }
            ClientEvent::ResponseReceived { response } => {
                self.handle_response(response, scheduler, self_id);
            }
            ClientEvent::RequestTimeout { request_id, attempt_id } => {
                self.handle_timeout(*request_id, *attempt_id, scheduler, self_id);
            }
            ClientEvent::RetryRequest { request_id } => {
                self.handle_retry(*request_id, scheduler, self_id);
            }
        }
    }
}

/// Convenience constructors for SimpleClient with specific retry policies
impl SimpleClient<ExponentialBackoffPolicy> {
    /// Create a convenience constructor with exponential backoff
    pub fn with_exponential_backoff(
        name: String, 
        request_interval: Duration,
        max_retries: usize,
        base_delay: Duration,
    ) -> Self {
        let retry_policy = ExponentialBackoffPolicy::new(max_retries, base_delay)
            .with_jitter(true);
        SimpleClient::new(name, request_interval, retry_policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use des_core::{Execute, Executor, Simulation};
    use des_core::task::PeriodicTask;
    use crate::retry_policy::{ExponentialBackoffPolicy, TokenBucketRetryPolicy, SuccessBasedRetryPolicy};

    #[test]
    fn test_simple_client_with_retry() {
        let mut sim = Simulation::default();
        
        // Create a client with exponential backoff retry policy
        let retry_policy = ExponentialBackoffPolicy::new(3, Duration::from_millis(100));
        let client = SimpleClient::new("test-client".to_string(), Duration::from_millis(100), retry_policy)
            .with_max_requests(3)
            .with_timeout(Duration::from_millis(500));
        
        let client_id = sim.add_component(client);
        
        // Start the periodic request generation
        let task = PeriodicTask::with_count(
            move |scheduler| {
                scheduler.schedule_now(client_id, ClientEvent::SendRequest);
            },
            SimTime::from_duration(Duration::from_millis(100)),
            3,
        );
        sim.scheduler.schedule_task(SimTime::zero(), task);
        
        // Run simulation for 5 seconds to allow for retries
        Executor::timed(SimTime::from_duration(Duration::from_secs(5))).execute(&mut sim);
        
        // Verify the client sent the expected number of requests
        let client = sim.remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(client_id).unwrap();
        assert_eq!(client.requests_sent, 3);
        
        // Check that some metrics were recorded
        let _metrics = client.get_metrics();
        // Since we can't retrieve counter values, just check that requests were sent
        assert!(client.requests_sent > 0);
    }

    #[test]
    fn test_simple_client_exponential_backoff_constructor() {
        let mut sim = Simulation::default();
        
        // Create a client using the convenience constructor
        let client = SimpleClient::with_exponential_backoff(
            "backoff-client".to_string(),
            Duration::from_millis(50),
            3, // max retries
            Duration::from_millis(100), // base delay
        );
        
        let client_id = sim.add_component(client);
        
        // Start the periodic request generation (unlimited)
        let task = PeriodicTask::new(
            move |scheduler| {
                scheduler.schedule_now(client_id, ClientEvent::SendRequest);
            },
            SimTime::from_duration(Duration::from_millis(50)),
        );
        sim.scheduler.schedule_task(SimTime::zero(), task);
        
        // Run simulation for 495ms (should send 10 requests: at 0, 50, 100, 150, 200, 250, 300, 350, 400, 450ms)
        Executor::timed(SimTime::from_duration(Duration::from_millis(495))).execute(&mut sim);
        
        // Verify the client sent requests
        let client = sim.remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(client_id).unwrap();
        assert_eq!(client.requests_sent, 10);
    }

    #[test]
    fn test_retry_policy_integration() {
        // Test that different retry policies can be used
        let exponential_policy = ExponentialBackoffPolicy::new(3, Duration::from_millis(100))
            .with_jitter(true)
            .with_max_delay(Duration::from_secs(1));
        
        let token_bucket_policy = TokenBucketRetryPolicy::new(3, 5, 2.0)
            .with_base_delay(Duration::from_millis(50));
        
        let success_based_policy = SuccessBasedRetryPolicy::new(3, Duration::from_millis(100), 10)
            .with_min_success_rate(0.6)
            .with_failure_multiplier(2.0);
        
        // Create clients with different policies
        let _client1 = SimpleClient::new("exp-client".to_string(), Duration::from_millis(100), exponential_policy);
        let _client2 = SimpleClient::new("token-client".to_string(), Duration::from_millis(100), token_bucket_policy);
        let _client3 = SimpleClient::new("success-client".to_string(), Duration::from_millis(100), success_based_policy);
        
        // Test passes if compilation succeeds
    }
}