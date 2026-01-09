//! DES-aware circuit breaker layer.
//!
//! Prevents cascading failures by temporarily blocking requests to failing services.
//!
//! # States
//!
//! - **Closed**: Normal operation, tracking consecutive failures
//! - **Open**: Blocking all requests, waiting for recovery timeout
//! - **Half-Open**: Allowing probe requests to test recovery
//!
//! # Usage
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesCircuitBreaker, DesCircuitBreakerLayer};
//! use des_core::Simulation;
//! use tower::Layer;
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut simulation = Simulation::default();
//! let scheduler = simulation.scheduler_handle();
//!
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(5)
//!     .service_time(Duration::from_millis(100))
//!     .build(&mut simulation)?;
//!
//! // Circuit breaker: open after 3 failures, recover after 30 seconds
//! let protected_service = DesCircuitBreakerLayer::new(3, Duration::from_secs(30), scheduler)
//!     .layer(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! # Behavior
//!
//! - Failures increment a counter; when threshold is reached, circuit opens
//! - In open state, requests immediately return `ServiceError::Overloaded`
//! - After recovery timeout, circuit transitions to half-open for probing
//! - A successful probe closes the circuit; a failed probe reopens it

use des_core::task::TimeoutTask;
use des_core::{SchedulerHandle, SimTime};
use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tower::{Layer, Service};

use super::{ServiceError, SimBody};

/// DES-aware circuit breaker layer
///
/// This is the Layer implementation that creates circuit breaker-enabled services.
#[derive(Clone)]
pub struct DesCircuitBreakerLayer {
    failure_threshold: usize,
    recovery_timeout: Duration,
    scheduler: SchedulerHandle,
}

impl DesCircuitBreakerLayer {
    /// Create a new circuit breaker layer
    pub fn new(
        failure_threshold: usize,
        recovery_timeout: Duration,
        scheduler: SchedulerHandle,
    ) -> Self {
        Self {
            failure_threshold,
            recovery_timeout,
            scheduler,
        }
    }
}

impl<S> Layer<S> for DesCircuitBreakerLayer {
    type Service = DesCircuitBreaker<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DesCircuitBreaker::new(
            inner,
            self.failure_threshold,
            self.recovery_timeout,
            self.scheduler.clone(),
        )
    }
}

/// DES-aware circuit breaker service
#[derive(Clone)]
pub struct DesCircuitBreaker<S> {
    inner: S,
    failure_threshold: usize,
    recovery_timeout: Duration,
    state: Arc<Mutex<CircuitBreakerState>>,
    scheduler: SchedulerHandle,
}

#[derive(Debug, Clone)]
enum CircuitBreakerState {
    Closed { failure_count: usize },
    Open, // No longer need to store opened_at time
    HalfOpen,
}

impl<S> DesCircuitBreaker<S> {
    pub fn new(
        inner: S,
        failure_threshold: usize,
        recovery_timeout: Duration,
        scheduler: SchedulerHandle,
    ) -> Self {
        Self {
            inner,
            failure_threshold,
            recovery_timeout,
            state: Arc::new(Mutex::new(CircuitBreakerState::Closed { failure_count: 0 })),
            scheduler,
        }
    }

    fn should_allow_request(&self) -> bool {
        let state = self.state.lock().unwrap();
        match *state {
            CircuitBreakerState::Closed { .. } => true,
            CircuitBreakerState::HalfOpen => true,
            CircuitBreakerState::Open => false, // Simply reject when open
        }
    }
}

/// Future for circuit breaker operations
#[pin_project]
pub struct DesCircuitBreakerFuture<F> {
    #[pin]
    inner: Option<F>,
    state: Arc<Mutex<CircuitBreakerState>>,
    failure_threshold: usize,
    recovery_timeout: Duration,
    scheduler: SchedulerHandle,
    immediate_error: Option<ServiceError>,
}

impl<S, ReqBody> Service<Request<ReqBody>> for DesCircuitBreaker<S>
where
    S: Service<Request<ReqBody>, Response = http::Response<SimBody>, Error = ServiceError>,
{
    type Response = S::Response;
    type Error = ServiceError;
    type Future = DesCircuitBreakerFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if !self.should_allow_request() {
            return Poll::Ready(Err(ServiceError::Overloaded));
        }
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        if !self.should_allow_request() {
            // Create a future that immediately returns an error
            return DesCircuitBreakerFuture {
                inner: None,
                state: self.state.clone(),
                failure_threshold: self.failure_threshold,
                recovery_timeout: self.recovery_timeout,
                scheduler: self.scheduler.clone(),
                immediate_error: Some(ServiceError::Overloaded),
            };
        }

        let inner_future = self.inner.call(req);
        DesCircuitBreakerFuture {
            inner: Some(inner_future),
            state: self.state.clone(),
            failure_threshold: self.failure_threshold,
            recovery_timeout: self.recovery_timeout,
            scheduler: self.scheduler.clone(),
            immediate_error: None,
        }
    }
}

impl<F> Future for DesCircuitBreakerFuture<F>
where
    F: Future<Output = Result<http::Response<SimBody>, ServiceError>>,
{
    type Output = Result<http::Response<SimBody>, ServiceError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        // Check for immediate error first
        if let Some(error) = this.immediate_error.take() {
            return Poll::Ready(Err(error));
        }

        // Poll the inner future if present
        if let Some(inner) = this.inner.as_mut().as_pin_mut() {
            match inner.poll(cx) {
                Poll::Ready(Ok(response)) => {
                    // Record success - reset failure count
                    let mut state = this.state.lock().unwrap();
                    *state = CircuitBreakerState::Closed { failure_count: 0 };
                    Poll::Ready(Ok(response))
                }
                Poll::Ready(Err(err)) => {
                    // Record failure - increment count and check threshold
                    let mut state = this.state.lock().unwrap();
                    match *state {
                        CircuitBreakerState::Closed { failure_count } => {
                            let new_count = failure_count + 1;
                            if new_count >= *this.failure_threshold {
                                // Transition to Open state and schedule recovery task
                                *state = CircuitBreakerState::Open;

                                // Schedule a timeout task to transition to HalfOpen
                                let state_clone = this.state.clone();
                                let recovery_task = TimeoutTask::new(move |_scheduler| {
                                    // Transition from Open to HalfOpen
                                    let mut state = state_clone.lock().unwrap();
                                    if matches!(*state, CircuitBreakerState::Open) {
                                        *state = CircuitBreakerState::HalfOpen;
                                    }
                                });

                                this.scheduler.schedule_task(
                                    SimTime::from_duration(*this.recovery_timeout),
                                    recovery_task,
                                );
                            } else {
                                // Stay closed but increment failure count
                                *state = CircuitBreakerState::Closed {
                                    failure_count: new_count,
                                };
                            }
                        }
                        CircuitBreakerState::HalfOpen => {
                            // Transition back to open on failure in half-open state
                            *state = CircuitBreakerState::Open;

                            // Schedule another recovery task
                            let state_clone = this.state.clone();
                            let recovery_task = TimeoutTask::new(move |_scheduler| {
                                // Transition from Open to HalfOpen
                                let mut state = state_clone.lock().unwrap();
                                if matches!(*state, CircuitBreakerState::Open) {
                                    *state = CircuitBreakerState::HalfOpen;
                                }
                            });

                            this.scheduler.schedule_task(
                                SimTime::from_duration(*this.recovery_timeout),
                                recovery_task,
                            );
                        }
                        CircuitBreakerState::Open => {
                            // Already open, no state change needed
                        }
                    }
                    Poll::Ready(Err(err))
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            // No inner future, should not happen
            Poll::Ready(Err(ServiceError::CircuitBreakerInvalidState))
        }
    }
}
