//! DES-aware hedge layer for request hedging/duplication.
//!
//! The current implementation is a placeholder that forwards the primary request.
//! Full hedging (duplicate requests + racing) can be implemented later using
//! `des-tokio` timers.

use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tower::{Layer, Service};

use super::{ServiceError, SimBody};

#[derive(Clone)]
pub struct DesHedgeLayer {
    hedge_delay: Duration,
    max_hedged_requests: usize,
}

impl DesHedgeLayer {
    pub fn new(hedge_delay: Duration, max_hedged_requests: usize) -> Self {
        Self {
            hedge_delay,
            max_hedged_requests,
        }
    }
}

impl<S> Layer<S> for DesHedgeLayer
where
    S: Clone,
{
    type Service = DesHedge<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DesHedge::new(inner, self.hedge_delay, self.max_hedged_requests)
    }
}

#[derive(Clone)]
pub struct DesHedge<S> {
    inner: S,
    hedge_delay: Duration,
    max_hedged_requests: usize,
}

impl<S> DesHedge<S>
where
    S: Clone,
{
    pub fn new(inner: S, hedge_delay: Duration, max_hedged_requests: usize) -> Self {
        Self {
            inner,
            hedge_delay,
            max_hedged_requests,
        }
    }
}

#[pin_project]
pub struct DesHedgeFuture<F> {
    #[pin]
    primary: F,
    _hedge_delay: Duration,
    _max_hedged: usize,
}

impl<S, ReqBody> Service<Request<ReqBody>> for DesHedge<S>
where
    S: Service<Request<ReqBody>, Response = http::Response<SimBody>, Error = ServiceError> + Clone,
{
    type Response = S::Response;
    type Error = ServiceError;
    type Future = DesHedgeFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        DesHedgeFuture {
            primary: self.inner.call(req),
            _hedge_delay: self.hedge_delay,
            _max_hedged: self.max_hedged_requests,
        }
    }
}

impl<F> Future for DesHedgeFuture<F>
where
    F: Future<Output = Result<http::Response<SimBody>, ServiceError>>,
{
    type Output = Result<http::Response<SimBody>, ServiceError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().primary.poll(cx)
    }
}
