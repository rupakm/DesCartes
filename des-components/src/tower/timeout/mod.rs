//! DES-aware timeout layer.
//!
//! Provides timeout functionality using simulation time for deterministic behavior.
//!
//! # Usage
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesTimeout, DesTimeoutLayer};
//! use des_core::Simulation;
//! use tower::Layer;
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut simulation = Simulation::default();
//! let scheduler = simulation.scheduler_handle();
//!
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(5)
//!     .service_time(Duration::from_millis(200))
//!     .build(&mut simulation)?;
//!
//! // Add 1 second timeout using the layer pattern
//! let timeout_service = DesTimeoutLayer::new(Duration::from_secs(1), scheduler)
//!     .layer(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! # Behavior
//!
//! - Timeouts are scheduled as DES tasks using simulation time
//! - When a timeout fires before the request completes, `ServiceError::Timeout` is returned
//! - When the request completes first, the timeout is cancelled automatically
//! - Resources are cleaned up via `PinnedDrop` when futures are dropped

use atomic_waker::AtomicWaker;
use des_core::task::TimeoutTask;
use des_core::{SchedulerHandle, SimTime};
use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tower::{Layer, Service};

use super::{ServiceError, SimBody};

/// DES-aware timeout layer
///
/// This is the Layer implementation that creates timeout-enabled services.
#[derive(Clone)]
pub struct DesTimeoutLayer {
    timeout: Duration,
    scheduler: SchedulerHandle,
}

impl DesTimeoutLayer {
    /// Create a new timeout layer
    pub fn new(timeout: Duration, scheduler: SchedulerHandle) -> Self {
        Self { timeout, scheduler }
    }
}

impl<S> Layer<S> for DesTimeoutLayer {
    type Service = DesTimeout<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DesTimeout::new(inner, self.timeout, self.scheduler.clone())
    }
}

/// DES-aware timeout service that uses simulated time
#[derive(Clone)]
pub struct DesTimeout<S> {
    inner: S,
    timeout: Duration,
    scheduler: SchedulerHandle,
}

impl<S> DesTimeout<S> {
    pub fn new(inner: S, timeout: Duration, scheduler: SchedulerHandle) -> Self {
        Self {
            inner,
            timeout,
            scheduler,
        }
    }
}

/// Shared state between timeout future and timeout task
#[derive(Debug)]
struct TimeoutState {
    /// Whether the timeout has expired
    expired: AtomicBool,
    /// Whether the request has completed successfully
    completed: AtomicBool,
    /// Waker to notify the future when timeout expires (lock-free)
    waker: AtomicWaker,
    /// Timeout duration for error reporting
    timeout_duration: Duration,
}

impl TimeoutState {
    fn new(timeout_duration: Duration) -> Arc<Self> {
        Arc::new(Self {
            expired: AtomicBool::new(false),
            completed: AtomicBool::new(false),
            waker: AtomicWaker::new(),
            timeout_duration,
        })
    }

    /// Mark the timeout as expired and wake the future
    fn expire(&self) {
        self.expired.store(true, Ordering::Relaxed);
        self.waker.wake();
    }

    /// Mark the request as completed (prevents timeout from firing)
    fn complete(&self) {
        self.completed.store(true, Ordering::Relaxed);
    }

    /// Check if timeout has expired and request hasn't completed
    fn is_timed_out(&self) -> bool {
        self.expired.load(Ordering::Relaxed) && !self.completed.load(Ordering::Relaxed)
    }

    /// Check if request has completed
    fn is_completed(&self) -> bool {
        self.completed.load(Ordering::Relaxed)
    }

    /// Store a waker to be called when timeout expires (lock-free)
    fn set_waker(&self, waker: &Waker) {
        self.waker.register(waker);
    }
}

/// Future for timeout operations using DES tasks
#[pin_project(PinnedDrop)]
pub struct DesTimeoutFuture<F> {
    #[pin]
    inner: F,
    timeout_scheduled: bool,
    scheduler: SchedulerHandle,
    timeout_state: Arc<TimeoutState>,
    completed: bool,
}

impl<S, ReqBody> Service<Request<ReqBody>> for DesTimeout<S>
where
    S: Service<Request<ReqBody>, Response = http::Response<SimBody>, Error = ServiceError>,
    ReqBody: Clone,
{
    type Response = S::Response;
    type Error = ServiceError;
    type Future = DesTimeoutFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let inner_future = self.inner.call(req);
        let timeout_state = TimeoutState::new(self.timeout);

        // Don't schedule timeout here - we'll do it on first poll
        // This ensures timeout is relative to when request actually starts
        DesTimeoutFuture {
            inner: inner_future,
            timeout_scheduled: false,
            scheduler: self.scheduler.clone(),
            timeout_state,
            completed: false,
        }
    }
}

impl<F> Future for DesTimeoutFuture<F>
where
    F: Future<Output = Result<http::Response<SimBody>, ServiceError>>,
{
    type Output = Result<http::Response<SimBody>, ServiceError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().project();

        // Avoid double-completion
        if *this.completed {
            return Poll::Pending;
        }

        // Schedule timeout on first poll if not already scheduled
        if !*this.timeout_scheduled {
            let timeout_state = this.timeout_state.clone();

            // Create a timeout task that will expire the timeout state
            let timeout_task = TimeoutTask::new(move |_scheduler| {
                // Only fire timeout if request hasn't completed yet
                if !timeout_state.is_completed() {
                    timeout_state.expire();
                }
            });

            // Schedule the timeout task using the scheduler handle
            this.scheduler.schedule_task(
                SimTime::from_duration(this.timeout_state.timeout_duration),
                timeout_task,
            );

            *this.timeout_scheduled = true;
        }

        // Store the waker so timeout task can wake us
        this.timeout_state.set_waker(cx.waker());

        // Poll the inner future first - this is the critical fix
        // We need to check completion before checking timeout
        match this.inner.poll(cx) {
            Poll::Ready(result) => {
                // Mark as completed to prevent timeout from firing
                this.timeout_state.complete();
                *this.completed = true;
                Poll::Ready(result)
            }
            Poll::Pending => {
                // Only check timeout if inner future is still pending
                if this.timeout_state.is_timed_out() {
                    *this.completed = true;
                    Poll::Ready(Err(ServiceError::Timeout {
                        duration: this.timeout_state.timeout_duration,
                    }))
                } else {
                    Poll::Pending
                }
            }
        }
    }
}

/// Cleanup timeout state when future is dropped
#[pin_project::pinned_drop]
impl<F> PinnedDrop for DesTimeoutFuture<F> {
    fn drop(self: Pin<&mut Self>) {
        // Mark as completed to prevent timeout from firing
        // The timeout task will automatically clean up after execution
        self.timeout_state.complete();
    }
}
