//! DES-aware global concurrency limiter layer
//!
//! This module provides a DES-aware implementation similar to tower::limit::GlobalConcurrencyLimit
//! that allows sharing a concurrency limit across multiple service instances.
//!
//! ## Key Features
//!
//! - **Shared State**: Multiple service instances share the same concurrency limit
//! - **System-Wide Limiting**: Enables global resource management across services
//! - **Atomic Operations**: Thread-safe concurrency tracking using atomic counters
//! - **Flexible Configuration**: Can be applied to any subset of services
//!
//! ## Architecture
//!
//! The global concurrency limiter consists of two main components:
//!
//! 1. **`GlobalConcurrencyLimitState`**: Shared state that tracks the global concurrency
//! 2. **`DesGlobalConcurrencyLimit`**: Service wrapper that uses the shared state
//!
//! ## Differences from `tower::limit::GlobalConcurrencyLimit`
//!
//! - Designed for DES simulation environments
//! - Uses `PinnedDrop` for proper resource cleanup
//! - Integrates with DES waker system
//! - Supports cloning state across multiple services
//!
//! ## Example
//!
//! ```rust,no_run
//! use des_components::tower::limit::{DesGlobalConcurrencyLimit, GlobalConcurrencyLimitState};
//! use des_components::tower::DesServiceBuilder;
//! use des_core::Simulation;
//! use std::sync::{Arc, Mutex};
//!
//! # fn example() {
//! let simulation = Arc::new(Mutex::new(Simulation::default()));
//!
//! // Create shared global state with limit of 10
//! let global_state = GlobalConcurrencyLimitState::new(10);
//!
//! // Create multiple services sharing the same global limit
//! let service1 = DesServiceBuilder::new("service1".to_string())
//!     .build(simulation.clone()).unwrap();
//! let service2 = DesServiceBuilder::new("service2".to_string())
//!     .build(simulation.clone()).unwrap();
//!
//! let global_service1 = DesGlobalConcurrencyLimit::new(service1, global_state.clone());
//! let global_service2 = DesGlobalConcurrencyLimit::new(service2, global_state.clone());
//!
//! // Now both services share the same global concurrency limit of 10
//! # }
//! ```
//!
//! ## Use Cases
//!
//! - Modeling system-wide resource limits (e.g., database connections)
//! - Simulating shared infrastructure constraints
//! - Testing distributed system behavior under global limits
//! - Load balancing with global capacity management

use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use tower::{Layer, Service};

use crate::tower::{ServiceError, SimBody};

/// DES-aware global concurrency limiter layer
///
/// This is the Layer implementation that creates globally concurrency-limited services.
#[derive(Clone)]
pub struct DesGlobalConcurrencyLimitLayer {
    state: Arc<GlobalConcurrencyLimitState>,
}

impl DesGlobalConcurrencyLimitLayer {
    /// Create a new global concurrency limit layer with shared state
    pub fn new(state: Arc<GlobalConcurrencyLimitState>) -> Self {
        Self { state }
    }

    /// Create a new global concurrency limit layer with the specified maximum concurrency
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

/// Shared state for global concurrency limiting
#[derive(Debug)]
pub struct GlobalConcurrencyLimitState {
    max_concurrency: usize,
    current_concurrency: AtomicUsize,
    waiters: std::sync::Mutex<Vec<Waker>>,
}

impl GlobalConcurrencyLimitState {
    /// Create a new global concurrency limit state
    pub fn new(max_concurrency: usize) -> Arc<Self> {
        Arc::new(Self {
            max_concurrency,
            current_concurrency: AtomicUsize::new(0),
            waiters: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// Get the current number of concurrent requests across all services
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

    /// Release a concurrency slot and wake up waiters
    fn release(&self) {
        self.current_concurrency.fetch_sub(1, Ordering::Relaxed);
        
        // Wake up any waiting tasks
        let waiters = {
            let mut waiters = self.waiters.lock().unwrap();
            std::mem::take(&mut *waiters)
        };
        for waker in waiters {
            waker.wake();
        }
    }

    /// Register a waker to be notified when capacity becomes available
    fn register_waker(&self, waker: Waker) {
        let mut waiters = self.waiters.lock().unwrap();
        waiters.push(waker);
    }
}

/// DES-aware global concurrency limiter that shares limits across multiple service instances
#[derive(Clone)]
pub struct DesGlobalConcurrencyLimit<S> {
    inner: S,
    state: Arc<GlobalConcurrencyLimitState>,
}

impl<S> DesGlobalConcurrencyLimit<S> {
    /// Create a new global concurrency limiter with shared state
    pub fn new(inner: S, state: Arc<GlobalConcurrencyLimitState>) -> Self {
        Self { inner, state }
    }

    /// Create a new global concurrency limiter with the specified maximum concurrency
    pub fn with_max_concurrency(inner: S, max_concurrency: usize) -> Self {
        let state = GlobalConcurrencyLimitState::new(max_concurrency);
        Self::new(inner, state)
    }

    /// Get the shared state for this limiter
    pub fn state(&self) -> &Arc<GlobalConcurrencyLimitState> {
        &self.state
    }

    /// Get the current number of concurrent requests across all services
    pub fn current_concurrency(&self) -> usize {
        self.state.current_concurrency()
    }

    /// Get the maximum allowed concurrency
    pub fn max_concurrency(&self) -> usize {
        self.state.max_concurrency()
    }
}

/// Future for global concurrency-limited operations
#[pin_project(PinnedDrop)]
pub struct DesGlobalConcurrencyLimitFuture<F> {
    #[pin]
    inner: F,
    state: Arc<GlobalConcurrencyLimitState>,
    acquired: bool, // Track if we actually acquired a slot
}

impl<F> DesGlobalConcurrencyLimitFuture<F> {
    fn new(inner: F, state: Arc<GlobalConcurrencyLimitState>, acquired: bool) -> Self {
        Self { inner, state, acquired }
    }
}

impl<F> Future for DesGlobalConcurrencyLimitFuture<F>
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
                    this.state.release();
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
impl<F> PinnedDrop for DesGlobalConcurrencyLimitFuture<F> {
    fn drop(self: Pin<&mut Self>) {
        // Ensure we release the concurrency slot even if the future is dropped
        // Only if we actually acquired it
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
        // First check if the inner service is ready
        match self.inner.poll_ready(cx) {
            Poll::Ready(Ok(())) => {
                // Inner service is ready, now check global concurrency limit
                if self.state.try_acquire() {
                    Poll::Ready(Ok(()))
                } else {
                    // Register waker and return pending
                    self.state.register_waker(cx.waker().clone());
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
        
        DesGlobalConcurrencyLimitFuture::new(inner_future, self.state.clone(), true)
    }
}