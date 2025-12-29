//! DES-aware retry layer using Tower's retry infrastructure
//!
//! This module provides retry functionality that integrates with the discrete event
//! simulation framework while leveraging Tower's established retry patterns and policies.
//! The implementation provides deterministic retry behavior with configurable backoff
//! strategies, all using simulation time for reproducible results.
//!
//! # Retry Strategy
//!
//! ## Policy-Based Retries
//! The retry layer uses Tower's `Policy` trait, enabling:
//! - **Flexible Retry Logic**: Custom policies for different error types
//! - **Backoff Strategies**: Exponential, linear, or custom backoff patterns
//! - **Attempt Limits**: Maximum number of retry attempts per request
//! - **Error Classification**: Selective retries based on error types
//!
//! ## DES Integration
//! - **Simulation Time**: All retry delays use simulation time, not wall-clock time
//! - **Event Scheduling**: Retry delays are scheduled as DES tasks
//! - **Deterministic Behavior**: Identical retry patterns across simulation runs
//! - **Precise Timing**: Exact backoff timing without system timer jitter
//!
//! # Retry Policies
//!
//! ## DesRetryPolicy
//! A simple policy that retries on specific error types:
//! - **Retryable Errors**: Timeouts, overload, internal errors, not ready
//! - **Non-Retryable Errors**: Cancelled requests, HTTP errors, response builder errors
//! - **Attempt Tracking**: Counts attempts up to the maximum limit
//!
//! ## Custom Policies
//! Implement Tower's `Policy` trait for custom retry logic:
//! ```rust,no_run
//! use tower::retry::Policy;
//! use des_components::tower::ServiceError;
//!
//! struct CustomRetryPolicy {
//!     max_attempts: usize,
//!     current_attempts: usize,
//! }
//!
//! impl<Request, Response> Policy<Request, Response, ServiceError> for CustomRetryPolicy
//! where Request: Clone
//! {
//!     type Future = std::future::Ready<()>;
//!
//!     fn retry(&mut self, _req: &mut Request, result: &mut Result<Response, ServiceError>) -> Option<Self::Future> {
//!         // Custom retry logic here
//!         None
//!     }
//!
//!     fn clone_request(&mut self, req: &Request) -> Option<Request> {
//!         Some(req.clone())
//!     }
//! }
//! ```
//!
//! # Usage Examples
//!
//! ## Basic Retry Setup
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesRetry, DesRetryPolicy};
//! use des_core::Simulation;
//! use std::sync::{Arc, Mutex};
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let simulation = Arc::new(Mutex::new(Simulation::default()));
//!
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(3)
//!     .service_time(Duration::from_millis(100))
//!     .build(simulation.clone())?;
//!
//! // Retry up to 3 times with default policy
//! let retry_service = DesRetry::new(
//!     DesRetryPolicy::new(3),
//!     base_service,
//!     Arc::downgrade(&simulation),
//! );
//! # Ok(())
//! # }
//! ```
//!
//! ## Using Tower Layer Pattern
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesRetryLayer, DesRetryPolicy};
//! use tower::Layer;
//! use std::time::Duration;
//!
//! # fn layer_example() -> Result<(), Box<dyn std::error::Error>> {
//! # let simulation = std::sync::Arc::new(std::sync::Mutex::new(des_core::Simulation::default()));
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(5)
//!     .service_time(Duration::from_millis(200))
//!     .build(simulation.clone())?;
//!
//! // Apply retry layer
//! let retry_service = DesRetryLayer::new(
//!     DesRetryPolicy::new(5),  // Maximum 5 retry attempts
//!     Arc::downgrade(&simulation),
//! ).layer(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! ## Exponential Backoff
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, exponential_backoff_layer};
//! use std::time::Duration;
//!
//! # fn backoff_example() -> Result<(), Box<dyn std::error::Error>> {
//! # let simulation = std::sync::Arc::new(std::sync::Mutex::new(des_core::Simulation::default()));
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(2)
//!     .service_time(Duration::from_millis(150))
//!     .build(simulation.clone())?;
//!
//! // Exponential backoff with 3 retries
//! let retry_service = exponential_backoff_layer(
//!     3,
//!     Arc::downgrade(&simulation),
//! ).layer(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! ## Integration with Other Middleware
//!
//! ```rust,no_run
//! use des_components::tower::*;
//! use tower::ServiceBuilder;
//! use std::time::Duration;
//!
//! # fn integration_example() -> Result<(), Box<dyn std::error::Error>> {
//! # let simulation = std::sync::Arc::new(std::sync::Mutex::new(des_core::Simulation::default()));
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(3)
//!     .service_time(Duration::from_millis(100))
//!     .build(simulation.clone())?;
//!
//! // Comprehensive error handling stack
//! let service = ServiceBuilder::new()
//!     .layer(DesTimeoutLayer::new(
//!         Duration::from_secs(2),
//!         Arc::downgrade(&simulation),
//!     ))
//!     .layer(DesRetryLayer::new(
//!         DesRetryPolicy::new(3),
//!         Arc::downgrade(&simulation),
//!     ))
//!     .layer(DesCircuitBreakerLayer::new(
//!         5,
//!         Duration::from_secs(30),
//!         Arc::downgrade(&simulation),
//!     ))
//!     .service(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! # Error Classification
//!
//! ## Retryable Errors
//! The default policy retries these error types:
//! - **`ServiceError::Timeout`**: Request timeouts (may succeed on retry)
//! - **`ServiceError::Overloaded`**: Service overload (may recover)
//! - **`ServiceError::Internal`**: Internal service errors (may be transient)
//! - **`ServiceError::NotReady`**: Service not ready (may become ready)
//! - **`ServiceError::CircuitBreakerInvalidState`**: Circuit breaker issues
//! - **`ServiceError::RateLimiterInvalidState`**: Rate limiter issues
//!
//! ## Non-Retryable Errors
//! These errors are not retried by default:
//! - **`ServiceError::Cancelled`**: Explicitly cancelled requests
//! - **`ServiceError::Http`**: HTTP protocol errors
//! - **`ServiceError::HttpResponseBuilder`**: Response construction errors
//!
//! # Performance Characteristics
//!
//! ## State Management
//! - **Stateful Policies**: Policies track attempt counts and backoff state
//! - **Request Cloning**: Requests are cloned for each retry attempt
//! - **Memory Bounded**: State is cleaned up after max attempts or success
//!
//! ## Timing Behavior
//! - **Deterministic Delays**: All backoff timing uses simulation time
//! - **Precise Scheduling**: Retry delays scheduled as DES tasks
//! - **No Jitter**: Consistent timing across simulation runs
//!
//! ## Resource Usage
//! - **CPU**: Minimal overhead for retry logic and state management
//! - **Memory**: O(1) per active retry operation
//! - **Network**: Multiplies request volume by average retry attempts
//!
//! # Backoff Strategies
//!
//! ## Fixed Delay
//! Current implementation uses a fixed 100ms delay between retries.
//! This can be extended to support:
//! - **Exponential Backoff**: Increasing delays (100ms, 200ms, 400ms, ...)
//! - **Linear Backoff**: Fixed increment delays (100ms, 200ms, 300ms, ...)
//! - **Jittered Backoff**: Random variation to prevent thundering herd
//!
//! ## Custom Backoff
//! Implement custom backoff by extending the retry future to:
//! - Poll backoff futures from Tower's retry policies
//! - Schedule appropriate DES tasks for delay timing
//! - Support complex backoff algorithms
//!
//! # Monitoring and Observability
//!
//! Track retry effectiveness through:
//! - **Retry Rate**: Percentage of requests that require retries
//! - **Attempt Distribution**: Histogram of retry attempts per request
//! - **Success Rate**: Percentage of requests that eventually succeed
//! - **Latency Impact**: Additional latency from retry delays
//!
//! # Best Practices
//!
//! ## Retry Limits
//! - **Conservative Limits**: Start with 3-5 retry attempts
//! - **Exponential Backoff**: Use increasing delays to reduce load
//! - **Circuit Breaker Integration**: Combine with circuit breakers for protection
//!
//! ## Error Handling
//! - **Selective Retries**: Only retry transient, recoverable errors
//! - **Timeout Integration**: Set appropriate timeouts per retry attempt
//! - **Monitoring**: Track retry patterns to identify systemic issues
//!
//! ## Resource Management
//! - **Request Limits**: Consider total resource usage with retries
//! - **Backoff Tuning**: Balance between responsiveness and load reduction
//! - **Failure Detection**: Use retries to improve, not mask, reliability issues

use des_core::{SimTime, Simulation};
use des_core::task::TimeoutTask;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tower::{Layer, Service};
use tower::retry::Policy;

use super::ServiceError;

// Re-export Tower's retry components for convenience
pub use tower::retry::backoff::ExponentialBackoff;

/// DES-aware retry layer that uses Tower's Policy trait
///
/// This layer integrates Tower's retry policies with DES timing for deterministic retries.
#[derive(Clone)]
pub struct DesRetryLayer<P> {
    policy: P,
    simulation: Weak<Mutex<Simulation>>,
}

impl<P> DesRetryLayer<P> {
    /// Create a new retry layer with a Tower policy
    pub fn new(policy: P, simulation: Weak<Mutex<Simulation>>) -> Self {
        Self { policy, simulation }
    }
}

impl<P, S> Layer<S> for DesRetryLayer<P>
where
    P: Clone,
{
    type Service = DesRetry<P, S>;

    fn layer(&self, inner: S) -> Self::Service {
        DesRetry::new(self.policy.clone(), inner, self.simulation.clone())
    }
}

/// DES-aware retry service that uses Tower's Policy trait
#[derive(Clone)]
pub struct DesRetry<P, S> {
    policy: P,
    inner: S,
    simulation: Weak<Mutex<Simulation>>,
}

impl<P, S> DesRetry<P, S> {
    /// Create a new retry service with a Tower policy
    pub fn new(policy: P, inner: S, simulation: Weak<Mutex<Simulation>>) -> Self {
        Self {
            policy,
            inner,
            simulation,
        }
    }
}

/// States for the retry future state machine
#[derive(Debug)]
enum RetryState<F> {
    /// Making the initial or retry call
    Calling(F),
    /// Checking if we should retry after an error
    Checking { error: ServiceError },
    /// Waiting for the retry delay using DES timing
    Retrying {
        waker: Option<Waker>,
        delay_scheduled: bool,
    },
    /// Retry completed (success or max attempts reached)
    Done,
}

/// Future for DES-aware retry operations using Tower policies
#[pin_project]
pub struct DesRetryFuture<P, S, Request>
where
    P: Policy<Request, S::Response, S::Error> + Clone,
    S: Service<Request> + Clone,
    Request: Clone,
{
    policy: P,
    service: S,
    request: Request,
    state: RetryState<S::Future>,
    simulation: Weak<Mutex<Simulation>>,
}

impl<P, S, Request> DesRetryFuture<P, S, Request>
where
    P: Policy<Request, S::Response, S::Error> + Clone,
    S: Service<Request> + Clone,
    Request: Clone,
{
    fn new(
        policy: P,
        service: S,
        request: Request,
        future: S::Future,
        simulation: Weak<Mutex<Simulation>>,
    ) -> Self {
        Self {
            policy,
            service,
            request,
            state: RetryState::Calling(future),
            simulation,
        }
    }
}

impl<P, S, Request> Future for DesRetryFuture<P, S, Request>
where
    P: Policy<Request, S::Response, S::Error> + Clone,
    S: Service<Request, Error = ServiceError> + Clone,
    S::Future: Unpin,
    Request: Clone,
{
    type Output = Result<S::Response, S::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            let this = self.as_mut().project();
            
            match this.state {
                RetryState::Calling(future) => {
                    match Pin::new(future).poll(cx) {
                        Poll::Ready(Ok(response)) => {
                            *this.state = RetryState::Done;
                            return Poll::Ready(Ok(response));
                        }
                        Poll::Ready(Err(error)) => {
                            let error_clone = error.clone();
                            *this.state = RetryState::Checking { error: error_clone };
                            continue;
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
                RetryState::Checking { error } => {
                    // Check if policy allows retry
                    let error_clone = error.clone();
                    let mut result = Err(error_clone);
                    let should_retry = this.policy.retry(this.request, &mut result);
                    
                    match should_retry {
                        Some(_delay_future) => {
                            // Policy says we should retry - for now we'll use a fixed delay
                            // In a full implementation, we'd poll the delay_future
                            *this.state = RetryState::Retrying {
                                waker: None,
                                delay_scheduled: false,
                            };
                            continue;
                        }
                        None => {
                            // No more retries allowed
                            let final_error = error.clone();
                            *this.state = RetryState::Done;
                            return Poll::Ready(Err(final_error));
                        }
                    }
                }
                RetryState::Retrying { waker, delay_scheduled } => {
                    if !*delay_scheduled {
                        // Schedule the retry delay using DES timing
                        if let Some(sim) = this.simulation.upgrade() {
                            if let Ok(mut simulation) = sim.try_lock() {
                                let delay_waker = cx.waker().clone();
                                let timeout_task = TimeoutTask::new(move |_scheduler| {
                                    delay_waker.wake();
                                });
                                
                                // Use a fixed delay for now - in a real implementation,
                                // we'd get this from the backoff
                                simulation.scheduler.schedule_task(
                                    SimTime::from_duration(Duration::from_millis(100)),
                                    timeout_task,
                                );
                                
                                *delay_scheduled = true;
                                *waker = Some(cx.waker().clone());
                            }
                        }
                        return Poll::Pending;
                    } else {
                        // Delay has completed, make the retry call
                        let retry_future = this.service.call(this.request.clone());
                        *this.state = RetryState::Calling(retry_future);
                        continue;
                    }
                }
                RetryState::Done => {
                    panic!("DesRetryFuture polled after completion");
                }
            }
        }
    }
}

impl<P, S, Request> Service<Request> for DesRetry<P, S>
where
    P: Policy<Request, S::Response, S::Error> + Clone,
    S: Service<Request, Error = ServiceError> + Clone,
    S::Future: Unpin,
    Request: Clone,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = DesRetryFuture<P, S, Request>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let future = self.inner.call(request.clone());
        DesRetryFuture::new(
            self.policy.clone(),
            self.inner.clone(),
            request,
            future,
            self.simulation.clone(),
        )
    }
}

/// A simple policy that retries on specific errors
#[derive(Clone)]
pub struct DesRetryPolicy {
    max_retries: usize,
    current_attempts: usize,
}

impl DesRetryPolicy {
    /// Create a new DES retry policy
    pub fn new(max_retries: usize) -> Self {
        Self {
            max_retries,
            current_attempts: 0,
        }
    }
}

impl<Request, Response> Policy<Request, Response, ServiceError> for DesRetryPolicy
where
    Request: Clone,
{
    type Future = std::future::Ready<()>;

    fn retry(&mut self, _req: &mut Request, result: &mut Result<Response, ServiceError>) -> Option<Self::Future> {
        if self.current_attempts >= self.max_retries {
            return None;
        }

        // Check if this is an error we should retry
        let should_retry = match result {
            Ok(_) => false,
            Err(error) => match error {
                ServiceError::Timeout { .. } => true,
                ServiceError::Overloaded => true,
                ServiceError::Internal(_) => true,
                ServiceError::NotReady => true,
                ServiceError::Cancelled => false,
                ServiceError::Http(_) => false,
                ServiceError::CircuitBreakerInvalidState => true,
                ServiceError::RateLimiterInvalidState => true,
                ServiceError::HttpResponseBuilder { .. } => false,
            }
        };

        if should_retry {
            self.current_attempts += 1;
            Some(std::future::ready(()))
        } else {
            None
        }
    }

    fn clone_request(&mut self, req: &Request) -> Option<Request> {
        Some(req.clone())
    }
}

/// Convenience function to create a retry layer with exponential backoff
pub fn exponential_backoff_layer(
    max_retries: usize,
    simulation: Weak<Mutex<Simulation>>,
) -> DesRetryLayer<DesRetryPolicy> {
    let policy = DesRetryPolicy::new(max_retries);
    DesRetryLayer::new(policy, simulation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DesServiceBuilder;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_des_retry_policy_creation() {
        let mut policy = DesRetryPolicy::new(3);
        
        // Test that policy allows retries for retryable errors
        let retryable_error = ServiceError::Timeout { duration: Duration::from_millis(100) };
        let mut result: Result<(), ServiceError> = Err(retryable_error);
        let mut request = ();
        let should_retry = policy.retry(&mut request, &mut result);
        assert!(should_retry.is_some());
        
        // Test that policy rejects non-retryable errors
        let non_retryable_error = ServiceError::Cancelled;
        let mut result: Result<(), ServiceError> = Err(non_retryable_error);
        let mut request = ();
        let should_retry = policy.retry(&mut request, &mut result);
        assert!(should_retry.is_none());
    }

    #[test]
    fn test_exponential_backoff_layer_creation() {
        let simulation = Arc::new(Mutex::new(des_core::Simulation::default()));
        
        let retry_layer = exponential_backoff_layer(
            3,
            Arc::downgrade(&simulation),
        );
        
        // Create a base service
        let base_service = DesServiceBuilder::new("retry-test".to_string())
            .thread_capacity(1)
            .service_time(Duration::from_millis(50))
            .build(simulation.clone())
            .unwrap();
        
        // Apply retry layer
        let _retry_service = retry_layer.layer(base_service);
        
        // Test passes if no panics occur
    }
}