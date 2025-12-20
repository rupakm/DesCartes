//! DES-aware concurrency limiter layer
//!
//! This module provides a DES-aware implementation of tower::limit::ConcurrencyLimit
//! that limits the number of concurrent requests being processed by a service.
//!
//! ## Key Features
//!
//! - **Per-Service Limiting**: Each service instance has its own concurrency limit
//! - **Atomic Tracking**: Uses atomic counters for thread-safe concurrency tracking
//! - **Backpressure**: Properly implements Tower's backpressure via `poll_ready`
//! - **Resource Cleanup**: Ensures concurrency slots are released even if futures are dropped
//!
//! ## Differences from `tower::limit::ConcurrencyLimit`
//!
//! - Designed to work within DES simulation context
//! - Uses `PinnedDrop` for proper cleanup in simulation environment
//! - Integrates with DES waker system for efficient waiting
//!
//! ## Example
//!
//! ```rust,no_run
//! use des_components::tower::limit::DesConcurrencyLimit;
//! use des_components::tower::DesServiceBuilder;
//! use des_core::Simulation;
//! use std::sync::{Arc, Mutex};
//!
//! # fn example() {
//! let simulation = Arc::new(Mutex::new(Simulation::default()));
//! let base_service = DesServiceBuilder::new("concurrent".to_string())
//!     .thread_capacity(100) // Server can handle 100 threads
//!     .build(simulation.clone()).unwrap();
//!
//! // But limit to only 5 concurrent requests at the service layer
//! let limited_service = DesConcurrencyLimit::new(base_service, 5);
//! # }
//! ```
//!
//! ## Use Cases
//!
//! - Limiting concurrent requests to prevent overload
//! - Testing backpressure behavior in distributed systems
//! - Simulating resource-constrained environments
//! - Modeling connection pool limits

use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use tower::{Layer, Service};

use crate::tower::{ServiceError, SimBody};

/// DES-aware concurrency limiter layer
///
/// This is the Layer implementation that creates concurrency-limited services.
#[derive(Clone)]
pub struct DesConcurrencyLimitLayer {
    max_concurrency: usize,
}

impl DesConcurrencyLimitLayer {
    /// Create a new concurrency limit layer
    pub fn new(max_concurrency: usize) -> Self {
        Self { max_concurrency }
    }
}

impl<S> Layer<S> for DesConcurrencyLimitLayer {
    type Service = DesConcurrencyLimit<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DesConcurrencyLimit::new(inner, self.max_concurrency)
    }
}

/// DES-aware concurrency limiter that limits the number of concurrent requests
#[derive(Clone)]
pub struct DesConcurrencyLimit<S> {
    inner: S,
    max_concurrency: usize,
    current_concurrency: Arc<AtomicUsize>,
    waiters: Arc<std::sync::Mutex<Vec<Waker>>>,
}

impl<S> DesConcurrencyLimit<S> {
    /// Create a new concurrency limiter with the specified maximum concurrency
    pub fn new(inner: S, max_concurrency: usize) -> Self {
        Self {
            inner,
            max_concurrency,
            current_concurrency: Arc::new(AtomicUsize::new(0)),
            waiters: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Get the current number of concurrent requests
    pub fn current_concurrency(&self) -> usize {
        self.current_concurrency.load(Ordering::Relaxed)
    }

    /// Get the maximum allowed concurrency
    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    /// Check if we can accept a new request
    fn try_acquire(&self) -> bool {
        let current = self.current_concurrency.load(Ordering::Relaxed);
        if current < self.max_concurrency {
            // Try to increment the counter
            self.current_concurrency.compare_exchange_weak(
                current,
                current + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ).is_ok()
        } else {
            false
        }
    }

    /// Register a waker to be notified when capacity becomes available
    fn register_waker(&self, waker: Waker) {
        let mut waiters = self.waiters.lock().unwrap();
        waiters.push(waker);
    }
}

/// Future for concurrency-limited operations
#[pin_project(PinnedDrop)]
pub struct DesConcurrencyLimitFuture<F> {
    #[pin]
    inner: F,
    concurrency_limiter: Arc<AtomicUsize>,
    waiters: Arc<std::sync::Mutex<Vec<Waker>>>,
    acquired: bool, // Track if we actually acquired a slot
}

impl<F> DesConcurrencyLimitFuture<F> {
    fn new(
        inner: F,
        concurrency_limiter: Arc<AtomicUsize>,
        waiters: Arc<std::sync::Mutex<Vec<Waker>>>,
        acquired: bool,
    ) -> Self {
        Self {
            inner,
            concurrency_limiter,
            waiters,
            acquired,
        }
    }
}

impl<F> Future for DesConcurrencyLimitFuture<F>
where
    F: Future<Output = Result<http::Response<SimBody>, ServiceError>>,
{
    type Output = Result<http::Response<SimBody>, ServiceError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().project();
        
        match this.inner.poll(cx) {
            Poll::Ready(result) => {
                // Release the concurrency slot only if we acquired it
                if *this.acquired {
                    this.concurrency_limiter.fetch_sub(1, Ordering::Relaxed);
                    
                    // Wake up any waiting tasks
                    let mut waiters = this.waiters.lock().unwrap();
                    for waker in waiters.drain(..) {
                        waker.wake();
                    }
                    
                    // Mark as released so PinnedDrop doesn't double-release
                    *this.acquired = false;
                }
                
                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[pin_project::pinned_drop]
impl<F> PinnedDrop for DesConcurrencyLimitFuture<F> {
    fn drop(self: Pin<&mut Self>) {
        // Ensure we release the concurrency slot even if the future is dropped
        // Only if we actually acquired it
        if self.acquired {
            self.concurrency_limiter.fetch_sub(1, Ordering::Relaxed);
            
            // Wake up any waiting tasks
            let mut waiters = self.waiters.lock().unwrap();
            for waker in waiters.drain(..) {
                waker.wake();
            }
        }
    }
}

impl<S, ReqBody> Service<Request<ReqBody>> for DesConcurrencyLimit<S>
where
    S: Service<Request<ReqBody>, Response = http::Response<SimBody>, Error = ServiceError>,
{
    type Response = S::Response;
    type Error = ServiceError;
    type Future = DesConcurrencyLimitFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // First check if the inner service is ready
        match self.inner.poll_ready(cx) {
            Poll::Ready(Ok(())) => {
                // Inner service is ready, now check concurrency limit
                if self.try_acquire() {
                    Poll::Ready(Ok(()))
                } else {
                    // Register waker and return pending
                    self.register_waker(cx.waker().clone());
                    Poll::Pending
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        // If poll_ready returned Ok(()), we should have a slot
        // Don't try to acquire again, just assume we have it
        let inner_future = self.inner.call(req);
        
        DesConcurrencyLimitFuture::new(
            inner_future,
            self.current_concurrency.clone(),
            self.waiters.clone(),
            true, // Assume we have the slot
        )
    }
}