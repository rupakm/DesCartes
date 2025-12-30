//! DES-aware circuit breaker layer
//!
//! This module provides circuit breaker functionality that prevents cascading failures
//! by temporarily blocking requests to failing services. The circuit breaker monitors
//! request failures and automatically transitions between different states to protect
//! both the client and the failing service.
//!
//! # Circuit Breaker States
//!
//! ## Closed State
//! - **Normal Operation**: All requests are forwarded to the backend service
//! - **Failure Tracking**: Counts consecutive failures up to the threshold
//! - **Transition**: Moves to Open state when failure threshold is exceeded
//!
//! ## Open State
//! - **Request Blocking**: All requests are immediately rejected with `ServiceError::Overloaded`
//! - **Recovery Timer**: Uses DES timing to schedule transition to Half-Open state
//! - **Protection**: Prevents additional load on the failing service
//!
//! ## Half-Open State
//! - **Probe Requests**: Allows limited requests to test service recovery
//! - **Success Path**: Returns to Closed state on successful request
//! - **Failure Path**: Returns to Open state on failed request
//!
//! # Configuration Parameters
//!
//! ## Failure Threshold
//! The number of consecutive failures required to open the circuit breaker.
//! This should be tuned based on:
//! - Expected failure rate of the service
//! - Tolerance for false positives
//! - Time to detect and respond to failures
//!
//! ## Recovery Timeout
//! The duration to wait in Open state before transitioning to Half-Open.
//! Consider:
//! - Expected time for service recovery
//! - Impact of keeping the circuit open
//! - Frequency of recovery attempts
//!
//! # Usage Examples
//!
//! ## Basic Circuit Breaker Setup
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesCircuitBreaker};
//! use des_core::Simulation;
//! use std::sync::{Arc, Mutex};
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let simulation = Arc::new(Mutex::new(Simulation::default()));
//!
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(5)
//!     .service_time(Duration::from_millis(100))
//!     .build(simulation.clone())?;
//!
//! // Circuit breaker with 3 failure threshold and 30 second recovery
//! let circuit_breaker = DesCircuitBreaker::new(
//!     base_service,
//!     3,                              // failure_threshold
//!     Duration::from_secs(30),        // recovery_timeout
//!     std::sync::Arc::downgrade(&simulation),
//! );
//! # Ok(())
//! # }
//! ```
//!
//! ## Using Tower Layer Pattern
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesCircuitBreakerLayer};
//! use tower::Layer;
//! use std::time::Duration;
//!
//! # fn layer_example() -> Result<(), Box<dyn std::error::Error>> {
//! # let simulation = std::sync::Arc::new(std::sync::Mutex::new(des_core::Simulation::default()));
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(10)
//!     .service_time(Duration::from_millis(50))
//!     .build(simulation.clone())?;
//!
//! // Apply circuit breaker layer
//! let protected_service = DesCircuitBreakerLayer::new(
//!     5,                              // failure_threshold
//!     Duration::from_secs(60),        // recovery_timeout
//!     std::sync::Arc::downgrade(&simulation),
//! ).layer(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! ## Integration with Retry Logic
//!
//! ```rust,no_run
//! use des_components::tower::*;
//! use tower::ServiceBuilder;
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! # fn integration_example() -> Result<(), Box<dyn std::error::Error>> {
//! # let simulation = std::sync::Arc::new(std::sync::Mutex::new(des_core::Simulation::default()));
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(5)
//!     .service_time(Duration::from_millis(200))
//!     .build(simulation.clone())?;
//!
//! // Combine circuit breaker with retry for robust error handling
//! let service = ServiceBuilder::new()
//!     .layer(DesRetryLayer::new(
//!         DesRetryPolicy::new(3),
//!         Arc::downgrade(&simulation),
//!     ))
//!     .layer(DesCircuitBreakerLayer::new(
//!         3,
//!         Duration::from_secs(30),
//!         Arc::downgrade(&simulation),
//!     ))
//!     .service(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! # Failure Detection
//!
//! The circuit breaker considers the following as failures:
//! - `ServiceError::Internal`: Backend service errors
//! - `ServiceError::Timeout`: Request timeouts
//! - `ServiceError::Overloaded`: Service overload conditions
//! - Any other error that indicates service unavailability
//!
//! Successful responses reset the failure count and keep the circuit closed.
//!
//! # Performance Characteristics
//!
//! ## State Transitions
//! - **Closed → Open**: O(1) failure count check
//! - **Open → Half-Open**: Scheduled using DES tasks
//! - **Half-Open → Closed/Open**: O(1) based on single request result
//!
//! ## Memory Usage
//! - **Minimal State**: Only tracks current state and failure count
//! - **Shared State**: Thread-safe using Arc<Mutex<CircuitBreakerState>>
//! - **No Request Buffering**: Rejected requests return immediately
//!
//! ## Timing Accuracy
//! - **Deterministic Recovery**: Uses DES scheduler for precise timing
//! - **Reproducible Behavior**: Identical across simulation runs
//! - **No Wall-Clock Dependency**: All timing based on simulation time
//!
//! # Monitoring and Observability
//!
//! Circuit breaker state changes can be monitored through:
//! - Error patterns in request responses
//! - Service availability metrics
//! - Recovery attempt frequency
//! - Impact on overall system throughput
//!
//! # Best Practices
//!
//! ## Threshold Tuning
//! - Start with conservative thresholds (3-5 failures)
//! - Monitor false positive rates in production-like simulations
//! - Adjust based on service reliability characteristics
//!
//! ## Recovery Timeout
//! - Set based on expected service recovery time
//! - Consider exponential backoff for repeated failures
//! - Balance between availability and protection
//!
//! ## Integration Patterns
//! - Place circuit breakers close to external service calls
//! - Combine with retries for transient failures
//! - Use with load balancers for automatic failover

use des_core::{SimTime, Simulation};
use des_core::task::TimeoutTask;
use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
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
    simulation: Weak<Mutex<Simulation>>,
}

impl DesCircuitBreakerLayer {
    /// Create a new circuit breaker layer
    pub fn new(
        failure_threshold: usize,
        recovery_timeout: Duration,
        simulation: Weak<Mutex<Simulation>>,
    ) -> Self {
        Self {
            failure_threshold,
            recovery_timeout,
            simulation,
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
            self.simulation.clone(),
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
    simulation: Weak<Mutex<Simulation>>,
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
        simulation: Weak<Mutex<Simulation>>,
    ) -> Self {
        Self {
            inner,
            failure_threshold,
            recovery_timeout,
            state: Arc::new(Mutex::new(CircuitBreakerState::Closed { failure_count: 0 })),
            simulation,
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
    simulation: Weak<Mutex<Simulation>>,
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
                simulation: self.simulation.clone(),
                immediate_error: Some(ServiceError::Overloaded),
            };
        }

        let inner_future = self.inner.call(req);
        DesCircuitBreakerFuture {
            inner: Some(inner_future),
            state: self.state.clone(),
            failure_threshold: self.failure_threshold,
            recovery_timeout: self.recovery_timeout,
            simulation: self.simulation.clone(),
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
                                if let Some(sim) = this.simulation.upgrade() {
                                    if let Ok(mut simulation) = sim.try_lock() {
                                        let state_clone = this.state.clone();
                                        let recovery_task = TimeoutTask::new(move |_scheduler| {
                                            // Transition from Open to HalfOpen
                                            let mut state = state_clone.lock().unwrap();
                                            if matches!(*state, CircuitBreakerState::Open) {
                                                *state = CircuitBreakerState::HalfOpen;
                                            }
                                        });
                                        
                                        simulation.scheduler.schedule_task(
                                            SimTime::from_duration(*this.recovery_timeout),
                                            recovery_task,
                                        );
                                    }
                                }
                            } else {
                                // Stay closed but increment failure count
                                *state = CircuitBreakerState::Closed { failure_count: new_count };
                            }
                        }
                        CircuitBreakerState::HalfOpen => {
                            // Transition back to open on failure in half-open state
                            *state = CircuitBreakerState::Open;
                            
                            // Schedule another recovery task
                            if let Some(sim) = this.simulation.upgrade() {
                                if let Ok(mut simulation) = sim.try_lock() {
                                    let state_clone = this.state.clone();
                                    let recovery_task = TimeoutTask::new(move |_scheduler| {
                                        // Transition from Open to HalfOpen
                                        let mut state = state_clone.lock().unwrap();
                                        if matches!(*state, CircuitBreakerState::Open) {
                                            *state = CircuitBreakerState::HalfOpen;
                                        }
                                    });
                                    
                                    simulation.scheduler.schedule_task(
                                        SimTime::from_duration(*this.recovery_timeout),
                                        recovery_task,
                                    );
                                }
                            }
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