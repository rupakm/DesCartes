//! DES-aware retry layer.
//!
//! Provides retry functionality using simulation time via `des-tokio`.
//!
//! Note: this requires `des_tokio::runtime::install(&mut sim)`.

use des_core::{RequestAttemptId, RequestId};
use http;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tower::retry::Policy;
use tower::{Layer, Service};

use super::{ServiceError, SimBody};

/// Retry-specific metadata that tracks attempt information.
#[derive(Debug, Clone)]
pub struct RetryMetadata {
    pub original_request_id: RequestId,
    pub attempt_number: usize,
    pub total_attempts: usize,
}

impl RetryMetadata {
    pub fn new(request_id: RequestId) -> Self {
        Self {
            original_request_id: request_id,
            attempt_number: 1,
            total_attempts: 1,
        }
    }

    pub fn next_attempt(&self) -> Self {
        Self {
            original_request_id: self.original_request_id,
            attempt_number: self.attempt_number + 1,
            total_attempts: self.total_attempts + 1,
        }
    }
}

pub mod metadata {
    use super::*;
    use http::Request;

    pub fn add_retry_metadata(request: &mut Request<SimBody>, retry_meta: RetryMetadata) {
        request.extensions_mut().insert(retry_meta);
    }

    pub fn get_retry_metadata(request: &Request<SimBody>) -> Option<&RetryMetadata> {
        request.extensions().get::<RetryMetadata>()
    }

    pub fn create_retry_request(
        original_request: &Request<SimBody>,
        retry_meta: RetryMetadata,
    ) -> Request<SimBody> {
        let mut new_request = Request::builder()
            .method(original_request.method())
            .uri(original_request.uri())
            .version(original_request.version());

        for (name, value) in original_request.headers() {
            new_request = new_request.header(name, value);
        }

        let body = original_request.body().clone();
        let mut request = new_request.body(body).expect("valid retry request");
        add_retry_metadata(&mut request, retry_meta);
        request
    }
}

static NEXT_RETRY_ATTEMPT_ID: AtomicU64 = AtomicU64::new(1_000_000);

fn next_retry_attempt_id() -> RequestAttemptId {
    RequestAttemptId(NEXT_RETRY_ATTEMPT_ID.fetch_add(1, Ordering::Relaxed))
}

pub use tower::retry::backoff::ExponentialBackoff;

/// DES-aware retry layer that uses Tower's `Policy`.
#[derive(Clone)]
pub struct DesRetryLayer<P> {
    policy: P,
}

impl<P> DesRetryLayer<P> {
    pub fn new(policy: P) -> Self {
        Self { policy }
    }
}

impl<P, S> Layer<S> for DesRetryLayer<P>
where
    P: Clone,
{
    type Service = DesRetry<P, S>;

    fn layer(&self, inner: S) -> Self::Service {
        DesRetry::new(self.policy.clone(), inner)
    }
}

/// DES-aware retry service.
#[derive(Clone)]
pub struct DesRetry<P, S> {
    policy: P,
    inner: S,
}

impl<P, S> DesRetry<P, S> {
    pub fn new(policy: P, inner: S) -> Self {
        Self { policy, inner }
    }
}

enum RetryState<F> {
    Calling(F),
    Checking { error: ServiceError },
    Sleeping { sleep: des_tokio::time::Sleep },
    Done,
}

#[pin_project]
pub struct DesRetryFuture<P, S>
where
    P: Policy<http::Request<SimBody>, S::Response, S::Error> + Clone,
    S: Service<http::Request<SimBody>> + Clone,
{
    policy: P,
    service: S,
    original_request: http::Request<SimBody>,
    current_request: http::Request<SimBody>,
    state: RetryState<S::Future>,
    retry_metadata: Option<RetryMetadata>,
}

impl<P, S> DesRetryFuture<P, S>
where
    P: Policy<http::Request<SimBody>, S::Response, S::Error> + Clone,
    S: Service<http::Request<SimBody>> + Clone,
{
    fn new(policy: P, service: S, request: http::Request<SimBody>, future: S::Future) -> Self {
        let retry_metadata = metadata::get_retry_metadata(&request).cloned();
        Self {
            policy,
            service,
            original_request: request.clone(),
            current_request: request,
            state: RetryState::Calling(future),
            retry_metadata,
        }
    }

    fn create_retry_request(&mut self) -> http::Request<SimBody> {
        let retry_meta = if let Some(existing_meta) = &self.retry_metadata {
            existing_meta.next_attempt()
        } else {
            // We don't currently have access to the service-layer RequestId generator.
            // Use a high-range deterministic id to preserve stable tracking.
            let original_request_id = RequestId(next_retry_attempt_id().0);
            RetryMetadata::new(original_request_id).next_attempt()
        };

        self.retry_metadata = Some(retry_meta.clone());
        metadata::create_retry_request(&self.original_request, retry_meta)
    }
}

impl<P, S> Future for DesRetryFuture<P, S>
where
    P: Policy<http::Request<SimBody>, S::Response, S::Error> + Clone,
    S: Service<http::Request<SimBody>, Error = ServiceError> + Clone,
    S::Future: Unpin,
{
    type Output = Result<S::Response, S::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            let this = self.as_mut().project();

            match this.state {
                RetryState::Calling(future) => match Pin::new(future).poll(cx) {
                    Poll::Ready(Ok(response)) => {
                        *this.state = RetryState::Done;
                        return Poll::Ready(Ok(response));
                    }
                    Poll::Ready(Err(error)) => {
                        *this.state = RetryState::Checking {
                            error: error.clone(),
                        };
                        continue;
                    }
                    Poll::Pending => return Poll::Pending,
                },
                RetryState::Checking { error } => {
                    let error_clone = error.clone();
                    let mut result = Err(error_clone);

                    let should_retry = this.policy.retry(this.current_request, &mut result);
                    match should_retry {
                        Some(_) => {
                            // For now, use a fixed retry delay. We can extend this to respect
                            // Tower's backoff future in a later iteration.
                            *this.state = RetryState::Sleeping {
                                sleep: des_tokio::time::sleep(Duration::from_millis(100)),
                            };
                            continue;
                        }
                        None => {
                            let final_error = error.clone();
                            *this.state = RetryState::Done;
                            return Poll::Ready(Err(final_error));
                        }
                    }
                }
                RetryState::Sleeping { sleep } => match Pin::new(sleep).poll(cx) {
                    Poll::Ready(()) => {
                        let retry_request = self.as_mut().get_mut().create_retry_request();
                        let this = self.as_mut().project();
                        let retry_future = this.service.call(retry_request.clone());
                        *this.current_request = retry_request;
                        *this.state = RetryState::Calling(retry_future);
                        continue;
                    }
                    Poll::Pending => return Poll::Pending,
                },
                RetryState::Done => return Poll::Pending,
            }
        }
    }
}

impl<P, S> Service<http::Request<SimBody>> for DesRetry<P, S>
where
    P: Policy<http::Request<SimBody>, S::Response, S::Error> + Clone,
    S: Service<http::Request<SimBody>, Error = ServiceError> + Clone,
    S::Future: Unpin,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = DesRetryFuture<P, S>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<SimBody>) -> Self::Future {
        let future = self.inner.call(request.clone());
        DesRetryFuture::new(self.policy.clone(), self.inner.clone(), request, future)
    }
}

/// A simple policy that retries on specific errors.
#[derive(Clone)]
pub struct DesRetryPolicy {
    max_retries: usize,
    current_attempts: usize,
}

impl DesRetryPolicy {
    pub fn new(max_retries: usize) -> Self {
        Self {
            max_retries,
            current_attempts: 0,
        }
    }
}

impl Policy<http::Request<SimBody>, http::Response<SimBody>, ServiceError> for DesRetryPolicy {
    type Future = std::future::Ready<()>;

    fn retry(
        &mut self,
        _req: &mut http::Request<SimBody>,
        result: &mut Result<http::Response<SimBody>, ServiceError>,
    ) -> Option<Self::Future> {
        if self.current_attempts >= self.max_retries {
            return None;
        }

        let should_retry = match result {
            Ok(_) => false,
            Err(error) => match error {
                ServiceError::Timeout { .. } => true,
                ServiceError::Overloaded => true,
                ServiceError::Internal(_) => true,
                ServiceError::NotReady => true,
                ServiceError::Cancelled => false,
                ServiceError::Http(_) => false,
                ServiceError::CircuitBreakerInvalidState => true,
                ServiceError::RateLimiterInvalidState => true,
                ServiceError::HttpResponseBuilder { .. } => false,
            },
        };

        if should_retry {
            self.current_attempts += 1;
            Some(std::future::ready(()))
        } else {
            None
        }
    }

    fn clone_request(&mut self, req: &http::Request<SimBody>) -> Option<http::Request<SimBody>> {
        Some(req.clone())
    }
}

pub fn exponential_backoff_layer(max_retries: usize) -> DesRetryLayer<DesRetryPolicy> {
    DesRetryLayer::new(DesRetryPolicy::new(max_retries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_des_retry_policy_creation() {
        let mut policy = DesRetryPolicy::new(3);

        let retryable_error = ServiceError::Timeout {
            duration: Duration::from_millis(100),
        };
        let mut result: Result<http::Response<SimBody>, ServiceError> = Err(retryable_error);
        let mut request = http::Request::builder()
            .method("GET")
            .uri("/test")
            .body(SimBody::empty())
            .unwrap();
        let should_retry = policy.retry(&mut request, &mut result);
        assert!(should_retry.is_some());

        let non_retryable_error = ServiceError::Cancelled;
        let mut result: Result<http::Response<SimBody>, ServiceError> = Err(non_retryable_error);
        let mut request = http::Request::builder()
            .method("GET")
            .uri("/test")
            .body(SimBody::empty())
            .unwrap();
        let should_retry = policy.retry(&mut request, &mut result);
        assert!(should_retry.is_none());
    }
}
