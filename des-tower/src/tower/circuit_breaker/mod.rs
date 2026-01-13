//! DES-aware circuit breaker layer.
//!
//! Prevents cascading failures by temporarily blocking requests to failing services.
//!
//! This implementation uses `des-tokio` for recovery timers.
//! Note: requires `des_tokio::runtime::install(&mut sim)`.

use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tower::{Layer, Service};

use super::{ServiceError, SimBody};

#[derive(Clone)]
pub struct DesCircuitBreakerLayer {
    failure_threshold: usize,
    recovery_timeout: Duration,
}

impl DesCircuitBreakerLayer {
    pub fn new(failure_threshold: usize, recovery_timeout: Duration) -> Self {
        Self {
            failure_threshold,
            recovery_timeout,
        }
    }
}

impl<S> Layer<S> for DesCircuitBreakerLayer {
    type Service = DesCircuitBreaker<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DesCircuitBreaker::new(inner, self.failure_threshold, self.recovery_timeout)
    }
}

#[derive(Clone)]
pub struct DesCircuitBreaker<S> {
    inner: S,
    failure_threshold: usize,
    recovery_timeout: Duration,
    state: Arc<Mutex<CircuitBreakerState>>,
}

#[derive(Debug, Clone)]
enum CircuitBreakerState {
    Closed { failure_count: usize },
    Open { generation: u64 },
    HalfOpen,
}

impl<S> DesCircuitBreaker<S> {
    pub fn new(inner: S, failure_threshold: usize, recovery_timeout: Duration) -> Self {
        Self {
            inner,
            failure_threshold,
            recovery_timeout,
            state: Arc::new(Mutex::new(CircuitBreakerState::Closed { failure_count: 0 })),
        }
    }

    fn should_allow_request(&self) -> bool {
        let state = self.state.lock().unwrap();
        match *state {
            CircuitBreakerState::Closed { .. } => true,
            CircuitBreakerState::HalfOpen => true,
            CircuitBreakerState::Open { .. } => false,
        }
    }
}

#[pin_project]
pub struct DesCircuitBreakerFuture<F> {
    #[pin]
    inner: Option<F>,
    state: Arc<Mutex<CircuitBreakerState>>,
    failure_threshold: usize,
    recovery_timeout: Duration,
    immediate_error: Option<ServiceError>,
}

impl<S, ReqBody> Service<Request<ReqBody>> for DesCircuitBreaker<S>
where
    S: Service<Request<ReqBody>, Response = http::Response<SimBody>, Error = ServiceError> + Clone,
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
            return DesCircuitBreakerFuture {
                inner: None,
                state: self.state.clone(),
                failure_threshold: self.failure_threshold,
                recovery_timeout: self.recovery_timeout,
                immediate_error: Some(ServiceError::Overloaded),
            };
        }

        let inner_future = self.inner.call(req);
        DesCircuitBreakerFuture {
            inner: Some(inner_future),
            state: self.state.clone(),
            failure_threshold: self.failure_threshold,
            recovery_timeout: self.recovery_timeout,
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

        if let Some(error) = this.immediate_error.take() {
            return Poll::Ready(Err(error));
        }

        let Some(inner) = this.inner.as_mut().as_pin_mut() else {
            return Poll::Ready(Err(ServiceError::CircuitBreakerInvalidState));
        };

        match inner.poll(cx) {
            Poll::Ready(Ok(response)) => {
                let mut state = this.state.lock().unwrap();
                *state = CircuitBreakerState::Closed { failure_count: 0 };
                Poll::Ready(Ok(response))
            }
            Poll::Ready(Err(err)) => {
                // Update state.
                let mut to_schedule = None;
                {
                    let mut state = this.state.lock().unwrap();
                    match *state {
                        CircuitBreakerState::Closed { failure_count } => {
                            let new_count = failure_count + 1;
                            if new_count >= *this.failure_threshold {
                                let generation = match *state {
                                    CircuitBreakerState::Open { generation } => generation + 1,
                                    _ => 1,
                                };
                                *state = CircuitBreakerState::Open { generation };
                                to_schedule = Some(generation);
                            } else {
                                *state = CircuitBreakerState::Closed {
                                    failure_count: new_count,
                                };
                            }
                        }
                        CircuitBreakerState::HalfOpen => {
                            // Probe failed: reopen.
                            let generation = 1;
                            *state = CircuitBreakerState::Open { generation };
                            to_schedule = Some(generation);
                        }
                        CircuitBreakerState::Open { .. } => {
                            // already open
                        }
                    }
                }

                if let Some(generation) = to_schedule {
                    // We can only schedule recovery via the owning service, so re-use
                    // `recovery_timeout` from the future and reschedule based on the shared state.
                    let state = this.state.clone();
                    let recovery_timeout = *this.recovery_timeout;
                    des_tokio::task::spawn_local(async move {
                        des_tokio::time::sleep(recovery_timeout).await;
                        let mut s = state.lock().unwrap();
                        if matches!(*s, CircuitBreakerState::Open { generation: g } if g == generation)
                        {
                            *s = CircuitBreakerState::HalfOpen;
                        }
                    });
                }

                Poll::Ready(Err(err))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
