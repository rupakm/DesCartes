//! DES-aware hedge layer for request hedging/duplication.
//!
//! Hedging improves tail latency by sending duplicate requests and using the first
//! response that arrives.
//!
//! # Usage
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesHedge, DesHedgeLayer};
//! use des_core::Simulation;
//! use tower::Layer;
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut simulation = Simulation::default();
//! let scheduler = simulation.scheduler_handle();
//!
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(10)
//!     .service_time(Duration::from_millis(100))
//!     .build(&mut simulation)?;
//!
//! // Hedge after 50ms, maximum 2 additional requests
//! let hedged_service = DesHedgeLayer::new(Duration::from_millis(50), 2, scheduler)
//!     .layer(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! # Current Status
//!
//! The current implementation provides the framework but only processes the primary
//! request. Full hedging (scheduling duplicate requests and racing them) can be
//! added as simulation requirements evolve.

use des_core::SchedulerHandle;
use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
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
    scheduler: SchedulerHandle,
}

impl DesHedgeLayer {
    /// Create a new hedge layer
    pub fn new(
        hedge_delay: Duration,
        max_hedged_requests: usize,
        scheduler: SchedulerHandle,
    ) -> Self {
        Self {
            hedge_delay,
            max_hedged_requests,
            scheduler,
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
            self.scheduler.clone(),
        )
    }
}

/// DES-aware hedge service for request hedging/duplication
#[derive(Clone)]
pub struct DesHedge<S> {
    inner: S,
    hedge_delay: Duration,
    max_hedged_requests: usize,
    scheduler: SchedulerHandle,
}

impl<S> DesHedge<S>
where
    S: Clone,
{
    pub fn new(
        inner: S,
        hedge_delay: Duration,
        max_hedged_requests: usize,
        scheduler: SchedulerHandle,
    ) -> Self {
        Self {
            inner,
            hedge_delay,
            max_hedged_requests,
            scheduler,
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
    _scheduler: SchedulerHandle,
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
            _scheduler: self.scheduler.clone(),
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
