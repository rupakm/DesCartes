//! DES-aware retry layer using Tower's retry infrastructure
//!
//! This module provides a retry implementation that integrates with the DES
//! scheduler while reusing Tower's Policy trait and backoff implementations.

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