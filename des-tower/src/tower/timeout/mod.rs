//! DES-aware timeout layer.
//!
//! Provides timeout functionality using simulation time via `des-tokio`.
//!
//! Note: this requires `des_tokio::runtime::install(&mut sim)`.

use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tower::{Layer, Service};

use super::{ServiceError, SimBody};

/// DES-aware timeout layer.
#[derive(Clone)]
pub struct DesTimeoutLayer {
    timeout: Duration,
}

impl DesTimeoutLayer {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl<S> Layer<S> for DesTimeoutLayer {
    type Service = DesTimeout<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DesTimeout::new(inner, self.timeout)
    }
}

/// DES-aware timeout service that uses simulated time.
#[derive(Clone)]
pub struct DesTimeout<S> {
    inner: S,
    timeout: Duration,
}

impl<S> DesTimeout<S> {
    pub fn new(inner: S, timeout: Duration) -> Self {
        Self { inner, timeout }
    }
}

#[pin_project]
pub struct DesTimeoutFuture<F> {
    timeout: Duration,
    #[pin]
    inner: des_tokio::time::Timeout<F>,
}

impl<F> Future for DesTimeoutFuture<F>
where
    F: Future<Output = Result<http::Response<SimBody>, ServiceError>>,
{
    type Output = Result<http::Response<SimBody>, ServiceError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        match this.inner.poll(cx) {
            Poll::Ready(Ok(v)) => Poll::Ready(v),
            Poll::Ready(Err(des_tokio::time::Elapsed)) => Poll::Ready(Err(ServiceError::Timeout {
                duration: *this.timeout,
            })),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S, ReqBody> Service<Request<ReqBody>> for DesTimeout<S>
where
    S: Service<Request<ReqBody>, Response = http::Response<SimBody>, Error = ServiceError>,
{
    type Response = S::Response;
    type Error = ServiceError;
    type Future = DesTimeoutFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let inner = self.inner.call(req);
        DesTimeoutFuture {
            timeout: self.timeout,
            inner: des_tokio::time::timeout(self.timeout, inner),
        }
    }
}
