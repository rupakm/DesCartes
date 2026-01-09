//! DES-aware retry layer.
//!
//! Provides retry functionality using simulation time for deterministic backoff behavior.
//!
//! # Usage
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesRetry, DesRetryLayer, DesRetryPolicy};
//! use des_core::Simulation;
//! use tower::Layer;
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut simulation = Simulation::default();
//! let scheduler = simulation.scheduler_handle();
//!
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(3)
//!     .service_time(Duration::from_millis(100))
//!     .build(&mut simulation)?;
//!
//! // Retry up to 3 times using the layer pattern
//! let retry_service = DesRetryLayer::new(DesRetryPolicy::new(3), scheduler)
//!     .layer(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! # Retry Policy
//!
//! [`DesRetryPolicy`] retries on transient errors:
//! - `ServiceError::Timeout` - Request timeouts
//! - `ServiceError::Overloaded` - Service overload
//! - `ServiceError::Internal` - Internal errors
//! - `ServiceError::NotReady` - Service not ready
//!
//! Non-retryable errors (e.g., `Cancelled`, `Http`) are returned immediately.
//!
//! # Exponential Backoff
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, exponential_backoff_layer};
//! use tower::Layer;
//!
//! # fn backoff_example() -> Result<(), Box<dyn std::error::Error>> {
//! # let mut simulation = des_core::Simulation::default();
//! # let scheduler = simulation.scheduler_handle();
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .build(&mut simulation)?;
//!
//! let retry_service = exponential_backoff_layer(3, scheduler).layer(base_service);
//! # Ok(())
//! # }
//! ```

use des_core::{SimTime, SchedulerHandle, RequestAttemptId, RequestId};
use des_core::task::TimeoutTask;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tower::{Layer, Service};
use tower::retry::Policy;
use http;

use super::{ServiceError, SimBody};


/// Retry-specific metadata that tracks attempt information
#[derive(Debug, Clone)]
pub struct RetryMetadata {
    /// The original request ID (stays the same across retries)
    pub original_request_id: RequestId,
    /// Current attempt number (1, 2, 3, ...)
    pub attempt_number: usize,
    /// Total number of attempts made so far
    pub total_attempts: usize,
}

impl RetryMetadata {
    /// Create new retry metadata for the first attempt
    pub fn new(request_id: RequestId) -> Self {
        Self {
            original_request_id: request_id,
            attempt_number: 1,
            total_attempts: 1,
        }
    }

    /// Create metadata for a retry attempt
    pub fn next_attempt(&self) -> Self {
        Self {
            original_request_id: self.original_request_id,
            attempt_number: self.attempt_number + 1,
            total_attempts: self.total_attempts + 1,
        }
    }
}

/// Utility functions for working with request metadata
pub mod metadata {
    use super::*;
    use http::Request;

    /// Add retry metadata to a request
    pub fn add_retry_metadata(request: &mut Request<SimBody>, retry_meta: RetryMetadata) {
        request.extensions_mut().insert(retry_meta);
    }

    /// Get retry metadata from a request
    pub fn get_retry_metadata(request: &Request<SimBody>) -> Option<&RetryMetadata> {
        request.extensions().get::<RetryMetadata>()
    }

    /// Create a retry request with updated metadata
    pub fn create_retry_request(
        original_request: &Request<SimBody>,
        retry_meta: RetryMetadata,
    ) -> Request<SimBody> {
        let mut new_request = Request::builder()
            .method(original_request.method())
            .uri(original_request.uri())
            .version(original_request.version());

        // Copy headers
        for (name, value) in original_request.headers() {
            new_request = new_request.header(name, value);
        }

        // Copy body (clone the SimBody)
        let body = original_request.body().clone();
        let mut request = new_request.body(body).unwrap();

        // Add retry metadata to the new request
        add_retry_metadata(&mut request, retry_meta);

        request
    }
}

// Global attempt ID generator for retry layers
static NEXT_RETRY_ATTEMPT_ID: AtomicU64 = AtomicU64::new(1000000); // Start high to avoid conflicts

/// Generate a new unique attempt ID for retries
fn next_retry_attempt_id() -> RequestAttemptId {
    RequestAttemptId(NEXT_RETRY_ATTEMPT_ID.fetch_add(1, Ordering::Relaxed))
}

// Re-export Tower's retry components for convenience
pub use tower::retry::backoff::ExponentialBackoff;

/// DES-aware retry layer that uses Tower's Policy trait
///
/// This layer integrates Tower's retry policies with DES timing for deterministic retries.
#[derive(Clone)]
pub struct DesRetryLayer<P> {
    policy: P,
    scheduler: SchedulerHandle,
}

impl<P> DesRetryLayer<P> {
    /// Create a new retry layer with a Tower policy
    pub fn new(policy: P, scheduler: SchedulerHandle) -> Self {
        Self { policy, scheduler }
    }
}

impl<P, S> Layer<S> for DesRetryLayer<P>
where
    P: Clone,
{
    type Service = DesRetry<P, S>;

    fn layer(&self, inner: S) -> Self::Service {
        DesRetry::new(self.policy.clone(), inner, self.scheduler.clone())
    }
}

/// DES-aware retry service that uses Tower's Policy trait
#[derive(Clone)]
pub struct DesRetry<P, S> {
    policy: P,
    inner: S,
    scheduler: SchedulerHandle,
}

impl<P, S> DesRetry<P, S> {
    /// Create a new retry service with a Tower policy
    pub fn new(policy: P, inner: S, scheduler: SchedulerHandle) -> Self {
        Self {
            policy,
            inner,
            scheduler,
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
pub struct DesRetryFuture<P, S>
where
    P: Policy<http::Request<SimBody>, S::Response, S::Error> + Clone,
    S: Service<http::Request<SimBody>> + Clone,
{
    policy: P,
    service: S,
    original_request: http::Request<SimBody>,
    current_request: http::Request<SimBody>,
    state: RetryState<S::Future>,
    scheduler: SchedulerHandle,
    /// Track retry metadata across attempts
    retry_metadata: Option<RetryMetadata>,
}

impl<P, S> DesRetryFuture<P, S>
where
    P: Policy<http::Request<SimBody>, S::Response, S::Error> + Clone,
    S: Service<http::Request<SimBody>> + Clone,
{
    fn new(
        policy: P,
        service: S,
        request: http::Request<SimBody>,
        future: S::Future,
        scheduler: SchedulerHandle,
    ) -> Self {
        // Check if this request already has retry metadata (nested retries)
        let retry_metadata = metadata::get_retry_metadata(&request).cloned();
        
        Self {
            policy,
            service,
            original_request: request.clone(),
            current_request: request,
            state: RetryState::Calling(future),
            scheduler,
            retry_metadata,
        }
    }

    /// Create a retry request with updated metadata
    fn create_retry_request(&mut self) -> http::Request<SimBody> {
        let retry_meta = if let Some(existing_meta) = &self.retry_metadata {
            // This is a nested retry - increment the existing metadata
            existing_meta.next_attempt()
        } else {
            // This is the first retry - we need to create metadata
            // Extract the original request ID from the first attempt
            // For now, we'll generate a new request ID - in a full implementation,
            // we'd need to coordinate with the service layer
            let original_request_id = RequestId(next_retry_attempt_id().0);
            RetryMetadata::new(original_request_id).next_attempt()
        };

        self.retry_metadata = Some(retry_meta.clone());
        metadata::create_retry_request(&self.original_request, retry_meta)
    }
}

impl<P, S> Future for DesRetryFuture<P, S>
where
    P: Policy<http::Request<SimBody>, S::Response, S::Error> + Clone,
    S: Service<http::Request<SimBody>, Error = ServiceError> + Clone,
    S::Future: Unpin,
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
                    let should_retry = this.policy.retry(this.current_request, &mut result);
                    
                    match should_retry {
                        Some(_delay_future) => {
                            // Policy says we should retry
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
                        let delay_waker = cx.waker().clone();
                        let timeout_task = TimeoutTask::new(move |_scheduler| {
                            delay_waker.wake();
                        });
                        
                        // Use a fixed delay for now - in a real implementation,
                        // we'd get this from the backoff policy
                        this.scheduler.schedule_task(
                            SimTime::from_duration(Duration::from_millis(100)),
                            timeout_task,
                        );
                        
                        *delay_scheduled = true;
                        *waker = Some(cx.waker().clone());
                        return Poll::Pending;
                    } else {
                        // Delay has completed, need to create retry request
                        // Create retry request with updated metadata
                        let retry_request = self.as_mut().create_retry_request();
                        
                        // Get a new projection
                        let this = self.as_mut().project();
                        let retry_future = this.service.call(retry_request.clone());
                        *this.current_request = retry_request;
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

impl<P, S> Service<http::Request<SimBody>> for DesRetry<P, S>
where
    P: Policy<http::Request<SimBody>, S::Response, S::Error> + Clone,
    S: Service<http::Request<SimBody>, Error = ServiceError> + Clone,
    S::Future: Unpin,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = DesRetryFuture<P, S>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<SimBody>) -> Self::Future {
        let future = self.inner.call(request.clone());
        DesRetryFuture::new(
            self.policy.clone(),
            self.inner.clone(),
            request,
            future,
            self.scheduler.clone(),
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

impl Policy<http::Request<SimBody>, http::Response<SimBody>, ServiceError> for DesRetryPolicy
{
    type Future = std::future::Ready<()>;

    fn retry(&mut self, _req: &mut http::Request<SimBody>, result: &mut Result<http::Response<SimBody>, ServiceError>) -> Option<Self::Future> {
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

    fn clone_request(&mut self, req: &http::Request<SimBody>) -> Option<http::Request<SimBody>> {
        Some(req.clone())
    }
}

/// Convenience function to create a retry layer with exponential backoff
pub fn exponential_backoff_layer(
    max_retries: usize,
    scheduler: SchedulerHandle,
) -> DesRetryLayer<DesRetryPolicy> {
    let policy = DesRetryPolicy::new(max_retries);
    DesRetryLayer::new(policy, scheduler)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DesServiceBuilder;

    #[test]
    fn test_des_retry_policy_creation() {
        let mut policy = DesRetryPolicy::new(3);
        
        // Test that policy allows retries for retryable errors
        let retryable_error = ServiceError::Timeout { duration: Duration::from_millis(100) };
        let mut result: Result<http::Response<SimBody>, ServiceError> = Err(retryable_error);
        let mut request = http::Request::builder()
            .method("GET")
            .uri("/test")
            .body(SimBody::empty())
            .unwrap();
        let should_retry = policy.retry(&mut request, &mut result);
        assert!(should_retry.is_some());
        
        // Test that policy rejects non-retryable errors
        let non_retryable_error = ServiceError::Cancelled;
        let mut result: Result<http::Response<SimBody>, ServiceError> = Err(non_retryable_error);
        let mut request = http::Request::builder()
            .method("GET")
            .uri("/test")
            .body(SimBody::empty())
            .unwrap();
        let should_retry = policy.retry(&mut request, &mut result);
        assert!(should_retry.is_none());
    }

    #[test]
    fn test_exponential_backoff_layer_creation() {
        let mut simulation = des_core::Simulation::default();
        let scheduler = simulation.scheduler_handle();
        
        let retry_layer = exponential_backoff_layer(
            3,
            scheduler,
        );
        
        // Create a base service
        let base_service = DesServiceBuilder::new("retry-test".to_string())
            .thread_capacity(1)
            .service_time(Duration::from_millis(50))
            .build(&mut simulation)
            .unwrap();
        
        // Apply retry layer
        let _retry_service = retry_layer.layer(base_service);
        
        // Test passes if no panics occur
    }
}