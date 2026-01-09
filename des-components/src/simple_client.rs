//! Simple client component using the new des-core Component API
//!
//! This is a minimal example showing how to create components that work
//! with the new Component trait and event-driven simulation system.
//!
//! The client now supports configurable retry policies for handling request failures.

use crate::retry_policy::{ExponentialBackoffPolicy, RetryPolicy};
use crate::ServerEvent;
use des_core::{
    Component, Key, RequestAttempt, RequestAttemptId, RequestId, Response, Scheduler, SimTime,
};
use des_metrics::SimulationMetrics;
use std::collections::HashMap;
use std::time::Duration;

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

/// Simple client component that generates periodic requests with retry support
pub struct SimpleClient<P: RetryPolicy> {
    pub name: String,
    pub server_key: Key<ServerEvent>,
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
    RequestTimeout {
        request_id: RequestId,
        attempt_id: RequestAttemptId,
    },
    /// Time to retry a failed request
    RetryRequest { request_id: RequestId },
}

impl<P: RetryPolicy> SimpleClient<P> {
    /// Create a new simple client with a retry policy
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        request_interval: Duration,
        retry_policy: P,
    ) -> Self {
        Self {
            name,
            server_key,
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
            format!("Request {} from {}", request_id.0, self.name).into_bytes(),
        );

        println!(
            "[{}] [{}] Sending request {} attempt {} (attempt ID {})",
            format_sim_time(scheduler.time()),
            self.name,
            request_id.0,
            attempt_number,
            attempt_id
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
        self.metrics
            .increment_counter("attempts_sent", &[("component", &self.name)]);

        // Send the request to the server
        scheduler.schedule_now(
            self.server_key,
            ServerEvent::ProcessRequest {
                attempt: attempt.clone(),
                client_id: self_id,
            },
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
                "[{}] [{}] Received response for request {} attempt {}: {}",
                format_sim_time(scheduler.time()),
                self.name,
                request_id.0,
                attempt.attempt_number,
                if is_success { "SUCCESS" } else { "FAILURE" }
            );

            // Record response metrics
            if is_success {
                self.metrics
                    .increment_counter("responses_success", &[("component", &self.name)]);

                // Calculate and record response time
                let response_time = scheduler.time().duration_since(attempt.started_at);
                self.metrics.record_histogram(
                    "response_time_ms",
                    response_time.as_millis() as f64,
                    &[("component", &self.name)],
                );
            } else {
                self.metrics
                    .increment_counter("responses_failure", &[("component", &self.name)]);

                // Check if we should retry
                if let Some(retry_delay) = retry_policy.should_retry(&attempt, Some(response)) {
                    println!(
                        "[{}] [{}] Scheduling retry for request {} in {:.0}ms",
                        format_sim_time(scheduler.time()),
                        self.name,
                        request_id.0,
                        retry_delay.as_millis()
                    );

                    // Store the retry policy state for this request
                    self.active_requests
                        .insert(request_id, (attempt, retry_policy));

                    // Schedule the retry
                    scheduler.schedule(
                        SimTime::from_duration(retry_delay),
                        self_id,
                        ClientEvent::RetryRequest { request_id },
                    );

                    self.metrics
                        .increment_counter("retries_scheduled", &[("component", &self.name)]);
                } else {
                    println!(
                        "[{}] [{}] Request {} failed permanently (no more retries)",
                        format_sim_time(scheduler.time()),
                        self.name,
                        request_id.0
                    );
                    self.metrics.increment_counter(
                        "requests_failed_permanently",
                        &[("component", &self.name)],
                    );
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
                    "[{}] [{}] Request {} attempt {} timed out",
                    format_sim_time(scheduler.time()),
                    self.name,
                    request_id.0,
                    attempt.attempt_number
                );

                self.metrics
                    .increment_counter("requests_timeout", &[("component", &self.name)]);

                // Check if we should retry on timeout
                if let Some(retry_delay) = retry_policy.should_retry(attempt, None) {
                    println!(
                        "[{}] [{}] Scheduling retry for timed out request {} in {:.0}ms",
                        format_sim_time(scheduler.time()),
                        self.name,
                        request_id.0,
                        retry_delay.as_millis()
                    );

                    // Schedule the retry
                    scheduler.schedule(
                        SimTime::from_duration(retry_delay),
                        self_id,
                        ClientEvent::RetryRequest { request_id },
                    );

                    self.metrics
                        .increment_counter("retries_scheduled", &[("component", &self.name)]);
                } else {
                    println!(
                        "[{}] [{}] Request {} timed out permanently (no more retries)",
                        format_sim_time(scheduler.time()),
                        self.name,
                        request_id.0
                    );
                    self.active_requests.remove(&request_id);
                    self.metrics.increment_counter(
                        "requests_failed_permanently",
                        &[("component", &self.name)],
                    );
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
            let new_attempt =
                self.send_attempt(request_id, attempt.attempt_number, scheduler, self_id);

            // Store the updated state
            self.active_requests
                .insert(request_id, (new_attempt, retry_policy));
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
                self.active_requests
                    .insert(request_id, (attempt, retry_policy));

                self.requests_sent += 1;
                self.metrics
                    .increment_counter("requests_sent", &[("component", &self.name)]);
                self.metrics.record_gauge(
                    "total_requests",
                    self.requests_sent as f64,
                    &[("component", &self.name)],
                );
            }
            ClientEvent::ResponseReceived { response } => {
                self.handle_response(response, scheduler, self_id);
            }
            ClientEvent::RequestTimeout {
                request_id,
                attempt_id,
            } => {
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
        server_key: Key<ServerEvent>,
        request_interval: Duration,
        max_retries: usize,
        base_delay: Duration,
    ) -> Self {
        let retry_policy = ExponentialBackoffPolicy::new(max_retries, base_delay).with_jitter(true);
        SimpleClient::new(name, server_key, request_interval, retry_policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retry_policy::{
        ExponentialBackoffPolicy, SuccessBasedRetryPolicy, TokenBucketRetryPolicy,
    };
    use crate::Server;
    use des_core::task::PeriodicTask;
    use des_core::{Execute, Executor, Simulation};

    #[test]
    fn test_simple_client_with_retry() {
        let mut sim = Simulation::default();

        // Create a server first
        let server = Server::with_constant_service_time(
            "test-server".to_string(),
            1,
            Duration::from_millis(100),
        );
        let server_id = sim.add_component(server);

        // Create a client with exponential backoff retry policy
        let retry_policy = ExponentialBackoffPolicy::new(3, Duration::from_millis(100));
        let client = SimpleClient::new(
            "test-client".to_string(),
            server_id,
            Duration::from_millis(100),
            retry_policy,
        )
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
        sim.schedule_task(SimTime::zero(), task);

        // Run simulation for 5 seconds to allow for retries
        Executor::timed(SimTime::from_duration(Duration::from_secs(5))).execute(&mut sim);

        // Verify the client sent the expected number of requests
        let client = sim
            .remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(client_id)
            .unwrap();
        assert_eq!(client.requests_sent, 3);

        // Check that some metrics were recorded
        let metrics = client.get_metrics();
        // Now we can retrieve counter values for testing!
        assert!(
            metrics
                .get_counter("requests_sent", &[("component", "test-client")])
                .unwrap_or(0)
                > 0
        );
        assert_eq!(
            metrics.get_counter("requests_sent", &[("component", "test-client")]),
            Some(3)
        );
    }

    #[test]
    fn test_simple_client_exponential_backoff_constructor() {
        let mut sim = Simulation::default();

        // Create a server first
        let server = Server::with_constant_service_time(
            "backoff-server".to_string(),
            1,
            Duration::from_millis(50),
        );
        let server_id = sim.add_component(server);

        // Create a client using the convenience constructor
        let client = SimpleClient::with_exponential_backoff(
            "backoff-client".to_string(),
            server_id,
            Duration::from_millis(50),
            3,                          // max retries
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
        sim.schedule_task(SimTime::zero(), task);

        // Run simulation for 495ms (should send 10 requests: at 0, 50, 100, 150, 200, 250, 300, 350, 400, 450ms)
        Executor::timed(SimTime::from_duration(Duration::from_millis(495))).execute(&mut sim);

        // Verify the client sent requests
        let client = sim
            .remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(client_id)
            .unwrap();
        assert_eq!(client.requests_sent, 10);
    }

    #[test]
    fn test_retry_policy_integration() {
        // Test that different retry policies can be used
        let exponential_policy = ExponentialBackoffPolicy::new(3, Duration::from_millis(100))
            .with_jitter(true)
            .with_max_delay(Duration::from_secs(1));

        let token_bucket_policy =
            TokenBucketRetryPolicy::new(3, 5, 2.0).with_base_delay(Duration::from_millis(50));

        let success_based_policy = SuccessBasedRetryPolicy::new(3, Duration::from_millis(100), 10)
            .with_min_success_rate(0.6)
            .with_failure_multiplier(2.0);

        // Create a simulation to get a proper server key
        let mut sim = Simulation::default();
        let server = Server::with_constant_service_time(
            "test-server".to_string(),
            1,
            Duration::from_millis(100),
        );
        let server_key = sim.add_component(server);

        // Create clients with different policies
        let _client1 = SimpleClient::new(
            "exp-client".to_string(),
            server_key,
            Duration::from_millis(100),
            exponential_policy,
        );
        let _client2 = SimpleClient::new(
            "token-client".to_string(),
            server_key,
            Duration::from_millis(100),
            token_bucket_policy,
        );
        let _client3 = SimpleClient::new(
            "success-client".to_string(),
            server_key,
            Duration::from_millis(100),
            success_based_policy,
        );

        // Test passes if compilation succeeds
    }
}
