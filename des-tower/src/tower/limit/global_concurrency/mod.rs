//! DES-aware global concurrency limiter layer.
//!
//! Shares a concurrency limit across multiple service instances.

use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use tower::{Layer, Service};

use crate::tower::{ServiceError, SimBody};

/// DES-aware global concurrency limiter layer.
#[derive(Clone)]
pub struct DesGlobalConcurrencyLimitLayer {
    state: Arc<GlobalConcurrencyLimitState>,
}

impl DesGlobalConcurrencyLimitLayer {
    pub fn new(state: Arc<GlobalConcurrencyLimitState>) -> Self {
        Self { state }
    }

    pub fn with_max_concurrency(max_concurrency: usize) -> Self {
        let state = GlobalConcurrencyLimitState::new(max_concurrency);
        Self::new(state)
    }
}

impl<S> Layer<S> for DesGlobalConcurrencyLimitLayer {
    type Service = DesGlobalConcurrencyLimit<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DesGlobalConcurrencyLimit::new(inner, self.state.clone())
    }
}

/// Shared state for global concurrency limiting.
#[derive(Debug)]
pub struct GlobalConcurrencyLimitState {
    max_concurrency: usize,
    current_concurrency: AtomicUsize,
    waiters: std::sync::Mutex<Vec<Waker>>,
}

impl GlobalConcurrencyLimitState {
    pub fn new(max_concurrency: usize) -> Arc<Self> {
        Arc::new(Self {
            max_concurrency,
            current_concurrency: AtomicUsize::new(0),
            waiters: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub fn current_concurrency(&self) -> usize {
        self.current_concurrency.load(Ordering::Relaxed)
    }

    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    fn try_acquire(&self) -> bool {
        let current = self.current_concurrency.load(Ordering::Relaxed);
        if current < self.max_concurrency {
            self.current_concurrency
                .compare_exchange_weak(current, current + 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        } else {
            false
        }
    }

    fn release(&self) {
        self.current_concurrency.fetch_sub(1, Ordering::Relaxed);

        let waiters = {
            let mut waiters = self.waiters.lock().unwrap();
            std::mem::take(&mut *waiters)
        };
        for waker in waiters {
            waker.wake();
        }
    }

    fn register_waker(&self, waker: Waker) {
        let mut waiters = self.waiters.lock().unwrap();
        waiters.push(waker);
    }
}

/// DES-aware global concurrency limiter that shares limits across multiple service instances.
///
/// This implementation supports `call()` even if the caller does not strictly follow
/// the "`poll_ready` then `call`" pattern.
pub struct DesGlobalConcurrencyLimit<S> {
    inner: S,
    state: Arc<GlobalConcurrencyLimitState>,
    permit: bool,
}

impl<S: Clone> Clone for DesGlobalConcurrencyLimit<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            state: self.state.clone(),
            permit: false,
        }
    }
}

impl<S> DesGlobalConcurrencyLimit<S> {
    pub fn new(inner: S, state: Arc<GlobalConcurrencyLimitState>) -> Self {
        Self {
            inner,
            state,
            permit: false,
        }
    }

    pub fn with_max_concurrency(inner: S, max_concurrency: usize) -> Self {
        let state = GlobalConcurrencyLimitState::new(max_concurrency);
        Self::new(inner, state)
    }

    pub fn state(&self) -> &Arc<GlobalConcurrencyLimitState> {
        &self.state
    }

    pub fn current_concurrency(&self) -> usize {
        self.state.current_concurrency()
    }

    pub fn max_concurrency(&self) -> usize {
        self.state.max_concurrency()
    }
}

#[pin_project(PinnedDrop)]
pub struct DesGlobalConcurrencyLimitFuture<F> {
    #[pin]
    inner: Option<F>,
    state: Arc<GlobalConcurrencyLimitState>,
    acquired: bool,
    immediate_error: Option<ServiceError>,
}

impl<F> DesGlobalConcurrencyLimitFuture<F> {
    fn new(
        inner: Option<F>,
        state: Arc<GlobalConcurrencyLimitState>,
        acquired: bool,
        immediate_error: Option<ServiceError>,
    ) -> Self {
        Self {
            inner,
            state,
            acquired,
            immediate_error,
        }
    }
}

impl<F> Future for DesGlobalConcurrencyLimitFuture<F>
where
    F: Future<Output = Result<http::Response<SimBody>, ServiceError>>,
{
    type Output = Result<http::Response<SimBody>, ServiceError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.as_mut().project();

        if let Some(error) = this.immediate_error.take() {
            return Poll::Ready(Err(error));
        }

        let Some(inner) = this.inner.as_mut().as_pin_mut() else {
            return Poll::Ready(Err(ServiceError::NotReady));
        };

        match inner.poll(cx) {
            Poll::Ready(result) => {
                if *this.acquired {
                    this.state.release();
                    *this.acquired = false;
                }
                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[pin_project::pinned_drop]
impl<F> PinnedDrop for DesGlobalConcurrencyLimitFuture<F> {
    fn drop(self: Pin<&mut Self>) {
        if self.acquired {
            self.state.release();
        }
    }
}

impl<S, ReqBody> Service<Request<ReqBody>> for DesGlobalConcurrencyLimit<S>
where
    S: Service<Request<ReqBody>, Response = http::Response<SimBody>, Error = ServiceError>,
{
    type Response = S::Response;
    type Error = ServiceError;
    type Future = DesGlobalConcurrencyLimitFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.inner.poll_ready(cx) {
            Poll::Ready(Ok(())) => {
                if self.permit {
                    return Poll::Ready(Ok(()));
                }

                if self.state.try_acquire() {
                    self.permit = true;
                    Poll::Ready(Ok(()))
                } else {
                    self.state.register_waker(cx.waker().clone());
                    Poll::Pending
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let acquired = if self.permit {
            self.permit = false;
            true
        } else {
            self.state.try_acquire()
        };

        if !acquired {
            return DesGlobalConcurrencyLimitFuture::new(
                None,
                self.state.clone(),
                false,
                Some(ServiceError::NotReady),
            );
        }

        let inner_future = self.inner.call(req);
        DesGlobalConcurrencyLimitFuture::new(Some(inner_future), self.state.clone(), true, None)
    }
}
