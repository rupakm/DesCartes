//! DES-aware rate limiter layer
//!
//! This module provides a DES-aware implementation of rate limiting that uses
//! simulated time instead of real system time. This ensures deterministic
//! behavior in discrete event simulations.
//!
//! ## Key Features
//!
//! - **Token Bucket Algorithm**: Uses a token bucket for rate limiting
//! - **DES Time Integration**: Refills tokens based on simulated time progression
//! - **Burst Support**: Allows burst capacity for handling traffic spikes
//! - **Deterministic**: Behavior is fully deterministic within the simulation
//!
//! ## Differences from `tower::limit::RateLimit`
//!
//! - Uses `SimTime` instead of `std::time::Instant`
//! - Integrates with DES simulation weak references
//! - Token refill is based on simulation time advancement
//! - No background tasks or timers - purely event-driven
//!
//! ## Example
//!
//! ```rust,no_run
//! use des_components::tower::limit::DesRateLimit;
//! use des_components::tower::DesServiceBuilder;
//! use des_core::Simulation;
//! use std::sync::{Arc, Mutex};
//! use std::time::Duration;
//!
//! # fn example() {
//! let simulation = Arc::new(Mutex::new(Simulation::default()));
//! let base_service = DesServiceBuilder::new("rate-limited".to_string())
//!     .build(simulation.clone()).unwrap();
//!
//! // Allow 10 requests per second with a burst of 20
//! let rate_limited_service = DesRateLimit::new(
//!     base_service,
//!     10.0, // 10 requests per second
//!     20,   // burst capacity of 20 requests
//!     Arc::downgrade(&simulation),
//! );
//! # }
//! ```

use des_core::{SimTime, Simulation};
use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use tower::{Layer, Service};

use crate::tower::{ServiceError, SimBody};

/// DES-aware rate limiter layer
///
/// This is the Layer implementation that creates rate-limited services.
#[derive(Clone)]
pub struct DesRateLimitLayer {
    rate: f64, // requests per second
    burst: usize, // burst capacity
    simulation: Weak<Mutex<Simulation>>,
}

impl DesRateLimitLayer {
    /// Create a new rate limit layer
    pub fn new(rate: f64, burst: usize, simulation: Weak<Mutex<Simulation>>) -> Self {
        Self {
            rate,
            burst,
            simulation,
        }
    }
}

impl<S> Layer<S> for DesRateLimitLayer {
    type Service = DesRateLimit<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DesRateLimit::new(inner, self.rate, self.burst, self.simulation.clone())
    }
}

/// DES-aware rate limiter service
#[derive(Clone)]
pub struct DesRateLimit<S> {
    inner: S,
    rate: f64, // requests per second
    burst: usize, // burst capacity
    tokens: Arc<Mutex<f64>>,
    last_refill: Arc<Mutex<SimTime>>,
    simulation: Weak<Mutex<Simulation>>,
}

impl<S> DesRateLimit<S> {
    pub fn new(
        inner: S,
        rate: f64,
        burst: usize,
        simulation: Weak<Mutex<Simulation>>,
    ) -> Self {
        Self {
            inner,
            rate,
            burst,
            tokens: Arc::new(Mutex::new(burst as f64)),
            last_refill: Arc::new(Mutex::new(SimTime::zero())),
            simulation,
        }
    }

    fn try_acquire_token(&self) -> bool {
        if let Some(sim) = self.simulation.upgrade() {
            let simulation = sim.lock().unwrap();
            let current_time = simulation.scheduler.time();
            
            let mut tokens = self.tokens.lock().unwrap();
            let mut last_refill = self.last_refill.lock().unwrap();
            
            // Calculate tokens to add based on elapsed time
            let elapsed = current_time.duration_since(*last_refill);
            let tokens_to_add = elapsed.as_secs_f64() * self.rate;
            
            *tokens = (*tokens + tokens_to_add).min(self.burst as f64);
            *last_refill = current_time;
            
            if *tokens >= 1.0 {
                *tokens -= 1.0;
                true
            } else {
                false
            }
        } else {
            false
        }
    }
}

/// Future for rate limit operations
#[pin_project]
pub struct DesRateLimitFuture<F> {
    #[pin]
    inner: Option<F>,
    immediate_error: Option<ServiceError>,
}

impl<S, ReqBody> Service<Request<ReqBody>> for DesRateLimit<S>
where
    S: Service<Request<ReqBody>, Response = http::Response<SimBody>, Error = ServiceError>,
{
    type Response = S::Response;
    type Error = ServiceError;
    type Future = DesRateLimitFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        if !self.try_acquire_token() {
            return DesRateLimitFuture {
                inner: None,
                immediate_error: Some(ServiceError::Overloaded),
            };
        }

        let inner_future = self.inner.call(req);
        DesRateLimitFuture {
            inner: Some(inner_future),
            immediate_error: None,
        }
    }
}

impl<F> Future for DesRateLimitFuture<F>
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
            inner.poll(cx)
        } else {
            Poll::Ready(Err(ServiceError::Internal("Rate limit future in invalid state".to_string())))
        }
    }
}