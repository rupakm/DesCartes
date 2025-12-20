//! DES-aware circuit breaker layer

use des_core::{SimTime, Simulation};
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
    Open { opened_at: SimTime },
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
            CircuitBreakerState::Open { opened_at } => {
                // Check if recovery timeout has passed
                if let Some(sim) = self.simulation.upgrade() {
                    let simulation = sim.lock().unwrap();
                    let current_time = simulation.scheduler.time();
                    current_time >= opened_at + SimTime::from_duration(self.recovery_timeout)
                } else {
                    false
                }
            }
        }
    }

    #[allow(dead_code)]
    fn record_failure(&self) {
        let mut state = self.state.lock().unwrap();
        match *state {
            CircuitBreakerState::Closed { failure_count } => {
                let new_count = failure_count + 1;
                if new_count >= self.failure_threshold {
                    if let Some(sim) = self.simulation.upgrade() {
                        let simulation = sim.lock().unwrap();
                        let current_time = simulation.scheduler.time();
                        *state = CircuitBreakerState::Open { opened_at: current_time };
                    }
                } else {
                    *state = CircuitBreakerState::Closed { failure_count: new_count };
                }
            }
            CircuitBreakerState::HalfOpen => {
                if let Some(sim) = self.simulation.upgrade() {
                    let simulation = sim.lock().unwrap();
                    let current_time = simulation.scheduler.time();
                    *state = CircuitBreakerState::Open { opened_at: current_time };
                }
            }
            CircuitBreakerState::Open { .. } => {
                // Already open, no change needed
            }
        }
    }
}

/// Future for circuit breaker operations
#[pin_project]
pub struct DesCircuitBreakerFuture<F> {
    #[pin]
    inner: Option<F>,
    state: Arc<Mutex<CircuitBreakerState>>,
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
                immediate_error: Some(ServiceError::Overloaded),
            };
        }

        // Transition to half-open if we were open and timeout passed
        {
            let mut state = self.state.lock().unwrap();
            if let CircuitBreakerState::Open { opened_at } = *state {
                if let Some(sim) = self.simulation.upgrade() {
                    let simulation = sim.lock().unwrap();
                    let current_time = simulation.scheduler.time();
                    if current_time >= opened_at + SimTime::from_duration(self.recovery_timeout) {
                        *state = CircuitBreakerState::HalfOpen;
                    }
                }
            }
        }

        let inner_future = self.inner.call(req);
        DesCircuitBreakerFuture {
            inner: Some(inner_future),
            state: self.state.clone(),
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
                    // Record success
                    let mut state = this.state.lock().unwrap();
                    *state = CircuitBreakerState::Closed { failure_count: 0 };
                    Poll::Ready(Ok(response))
                }
                Poll::Ready(Err(err)) => {
                    // Record failure
                    let mut state = this.state.lock().unwrap();
                    match *state {
                        CircuitBreakerState::Closed { failure_count } => {
                            *state = CircuitBreakerState::Closed { failure_count: failure_count + 1 };
                        }
                        CircuitBreakerState::HalfOpen => {
                            // Transition back to open on failure
                            *state = CircuitBreakerState::Open { opened_at: SimTime::from_duration(Duration::ZERO) };
                        }
                        CircuitBreakerState::Open { .. } => {
                            // Already open
                        }
                    }
                    Poll::Ready(Err(err))
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            // No inner future, should not happen
            Poll::Ready(Err(ServiceError::Internal("Circuit breaker future in invalid state".to_string())))
        }
    }
}