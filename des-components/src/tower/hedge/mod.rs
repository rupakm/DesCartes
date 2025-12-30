//! DES-aware hedge layer for request hedging/duplication
//!
//! This module provides request hedging capabilities, also known as request duplication
//! or speculative execution. Hedging improves tail latency by sending duplicate requests
//! to multiple backends and using the first response that arrives.
//!
//! # Hedging Strategy
//!
//! The hedge layer implements a time-based hedging strategy:
//! 1. Send the primary request immediately
//! 2. After a configurable delay, send duplicate requests to other backends
//! 3. Return the first successful response and cancel remaining requests
//! 4. Limit the total number of hedged requests to prevent resource exhaustion
//!
//! # Use Cases
//!
//! ## Tail Latency Reduction
//! Hedging is particularly effective when:
//! - Backend services have variable response times
//! - Occasional slow responses significantly impact user experience
//! - The cost of duplicate requests is acceptable for improved latency
//!
//! ## High Availability
//! Hedging can improve availability by:
//! - Automatically working around slow or failing backends
//! - Reducing the impact of temporary performance degradation
//! - Providing graceful degradation under partial failures
//!
//! # Configuration Parameters
//!
//! ## Hedge Delay
//! The time to wait before sending duplicate requests. This should be tuned based on:
//! - Typical response time distribution of your services
//! - Acceptable latency targets (e.g., 95th percentile)
//! - Resource constraints and duplicate request costs
//!
//! ## Maximum Hedged Requests
//! Limits the total number of duplicate requests to prevent:
//! - Resource exhaustion under high load
//! - Amplification attacks or cascading failures
//! - Excessive backend load from hedging
//!
//! # Usage Examples
//!
//! ## Basic Hedging Setup
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesHedge};
//! use des_core::Simulation;
//! use std::sync::{Arc, Mutex};
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let simulation = Arc::new(Mutex::new(Simulation::default()));
//!
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(10)
//!     .service_time(Duration::from_millis(100))
//!     .build(simulation.clone())?;
//!
//! // Hedge after 50ms, maximum 2 additional requests
//! let hedged_service = DesHedge::new(
//!     base_service,
//!     Duration::from_millis(50),  // hedge_delay
//!     2,                          // max_hedged_requests
//!     Arc::downgrade(&simulation),
//! );
//! # Ok(())
//! # }
//! ```
//!
//! ## Using Tower Layer Pattern
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesHedgeLayer};
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
//! // Apply hedge layer
//! let hedged_service = DesHedgeLayer::new(
//!     Duration::from_millis(100),  // hedge after 100ms
//!     3,                           // maximum 3 hedged requests
//!     std::sync::Arc::downgrade(&simulation),
//! ).layer(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! ## Integration with Load Balancing
//!
//! ```rust,no_run
//! use des_components::tower::*;
//! use tower::ServiceBuilder;
//! use std::time::Duration;
//!
//! # fn integration_example() -> Result<(), Box<dyn std::error::Error>> {
//! # let simulation = std::sync::Arc::new(std::sync::Mutex::new(des_core::Simulation::default()));
//! # let services = vec![
//! #     DesServiceBuilder::new("backend-1".to_string()).build(simulation.clone())?,
//! #     DesServiceBuilder::new("backend-2".to_string()).build(simulation.clone())?,
//! # ];
//! let load_balancer = DesLoadBalancer::round_robin(services);
//!
//! // Combine hedging with load balancing for maximum effectiveness
//! let service = ServiceBuilder::new()
//!     .layer(DesHedgeLayer::new(
//!         Duration::from_millis(75),
//!         2,
//!         std::sync::Arc::downgrade(&simulation),
//!     ))
//!     .service(load_balancer);
//! # Ok(())
//! # }
//! ```
//!
//! # Performance Considerations
//!
//! ## Resource Usage
//! - **CPU**: Minimal overhead for scheduling hedge requests
//! - **Memory**: Small per-request state for tracking hedged requests
//! - **Network**: Multiplies request volume by (1 + max_hedged_requests)
//!
//! ## Tuning Guidelines
//! - Set hedge_delay to ~95th percentile of normal response time
//! - Limit max_hedged_requests to 2-3 to balance latency vs. resource usage
//! - Monitor backend load to ensure hedging doesn't cause overload
//!
//! # Current Implementation Status
//!
//! The current implementation provides the basic framework for hedging but only
//! processes the primary request. A full implementation would:
//! - Schedule hedge requests using DES timing
//! - Track multiple concurrent requests per hedge operation
//! - Cancel remaining requests when the first response arrives
//! - Provide metrics on hedge effectiveness and resource usage
//!
//! This foundation can be extended to provide complete hedging functionality
//! as simulation requirements evolve.

use des_core::Simulation;
use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, Weak};
use std::task::{Context, Poll};
use std::time::Duration;
use tower::{Layer, Service};

use super::{ServiceError, SimBody};

/// DES-aware hedge layer
///
/// This is the Layer implementation that creates hedge-enabled services.
#[derive(Clone)]
pub struct DesHedgeLayer {
    hedge_delay: Duration,
    max_hedged_requests: usize,
    simulation: Weak<Mutex<Simulation>>,
}

impl DesHedgeLayer {
    /// Create a new hedge layer
    pub fn new(
        hedge_delay: Duration,
        max_hedged_requests: usize,
        simulation: Weak<Mutex<Simulation>>,
    ) -> Self {
        Self {
            hedge_delay,
            max_hedged_requests,
            simulation,
        }
    }
}

impl<S> Layer<S> for DesHedgeLayer
where
    S: Clone,
{
    type Service = DesHedge<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DesHedge::new(
            inner,
            self.hedge_delay,
            self.max_hedged_requests,
            self.simulation.clone(),
        )
    }
}

/// DES-aware hedge service for request hedging/duplication
#[derive(Clone)]
pub struct DesHedge<S> {
    inner: S,
    hedge_delay: Duration,
    max_hedged_requests: usize,
    simulation: Weak<Mutex<Simulation>>,
}

impl<S> DesHedge<S>
where
    S: Clone,
{
    pub fn new(
        inner: S,
        hedge_delay: Duration,
        max_hedged_requests: usize,
        simulation: Weak<Mutex<Simulation>>,
    ) -> Self {
        Self {
            inner,
            hedge_delay,
            max_hedged_requests,
            simulation,
        }
    }
}

/// Future for hedge operations
#[pin_project]
pub struct DesHedgeFuture<F> {
    #[pin]
    primary: F,
    hedge_scheduled: bool,
    _hedge_delay: Duration,
    _max_hedged: usize,
    _simulation: Weak<Mutex<Simulation>>,
}

impl<S, ReqBody> Service<Request<ReqBody>> for DesHedge<S>
where
    S: Service<Request<ReqBody>, Response = http::Response<SimBody>, Error = ServiceError> + Clone,
    ReqBody: Clone,
{
    type Response = S::Response;
    type Error = ServiceError;
    type Future = DesHedgeFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let primary_future = self.inner.call(req);
        
        DesHedgeFuture {
            primary: primary_future,
            hedge_scheduled: false,
            _hedge_delay: self.hedge_delay,
            _max_hedged: self.max_hedged_requests,
            _simulation: self.simulation.clone(),
        }
    }
}

impl<F> Future for DesHedgeFuture<F>
where
    F: Future<Output = Result<http::Response<SimBody>, ServiceError>>,
{
    type Output = Result<http::Response<SimBody>, ServiceError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        
        // For now, just poll the primary request
        // In a full implementation, you'd schedule and poll hedge requests
        *this.hedge_scheduled = true; // Mark as scheduled to avoid warnings
        
        this.primary.poll(cx)
    }
}