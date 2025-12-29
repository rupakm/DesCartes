//! DES-aware timeout layer with proper event scheduling
//!
//! This module provides timeout functionality that integrates seamlessly with the
//! discrete event simulation framework. Unlike traditional timeout implementations
//! that rely on system timers, this module uses DES tasks and events to provide
//! deterministic, reproducible timeout behavior.
//!
//! # Timeout Implementation
//!
//! ## DES Integration
//! - **Event-Based Timing**: Timeouts are scheduled as DES tasks, not system timers
//! - **Simulation Time**: All timeouts use simulation time, not wall-clock time
//! - **Deterministic Behavior**: Identical timeout behavior across simulation runs
//! - **Precise Control**: Exact timeout timing without system timer jitter
//!
//! ## Timeout Lifecycle
//! 1. **Request Start**: Timeout task is scheduled when request begins
//! 2. **Concurrent Execution**: Request processing and timeout run in parallel
//! 3. **Race Condition**: First completion (success or timeout) wins
//! 4. **Cleanup**: Losing operation is automatically cleaned up
//!
//! # Configuration
//!
//! ## Timeout Duration
//! The maximum time to wait for a request to complete. Consider:
//! - **Service Characteristics**: Typical response time distribution
//! - **User Experience**: Acceptable latency for your application
//! - **Resource Usage**: Balance between responsiveness and resource consumption
//!
//! # Usage Examples
//!
//! ## Basic Timeout Setup
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesTimeout};
//! use des_core::Simulation;
//! use std::sync::{Arc, Mutex};
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let simulation = Arc::new(Mutex::new(Simulation::default()));
//!
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(5)
//!     .service_time(Duration::from_millis(200))
//!     .build(simulation.clone())?;
//!
//! // Add 1 second timeout
//! let timeout_service = DesTimeout::new(
//!     base_service,
//!     Duration::from_secs(1),
//!     Arc::downgrade(&simulation),
//! );
//! # Ok(())
//! # }
//! ```
//!
//! ## Using Tower Layer Pattern
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesTimeoutLayer};
//! use tower::Layer;
//! use std::time::Duration;
//!
//! # fn layer_example() -> Result<(), Box<dyn std::error::Error>> {
//! # let simulation = std::sync::Arc::new(std::sync::Mutex::new(des_core::Simulation::default()));
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(10)
//!     .service_time(Duration::from_millis(100))
//!     .build(simulation.clone())?;
//!
//! // Apply timeout layer
//! let timeout_service = DesTimeoutLayer::new(
//!     Duration::from_millis(500),
//!     Arc::downgrade(&simulation),
//! ).layer(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! ## Timeout with Retry Logic
//!
//! ```rust,no_run
//! use des_components::tower::*;
//! use tower::ServiceBuilder;
//! use std::time::Duration;
//!
//! # fn retry_example() -> Result<(), Box<dyn std::error::Error>> {
//! # let simulation = std::sync::Arc::new(std::sync::Mutex::new(des_core::Simulation::default()));
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(3)
//!     .service_time(Duration::from_millis(300))
//!     .build(simulation.clone())?;
//!
//! // Combine timeout with retry for robust error handling
//! let service = ServiceBuilder::new()
//!     .layer(DesRetryLayer::new(
//!         DesRetryPolicy::new(3),
//!         Arc::downgrade(&simulation),
//!     ))
//!     .layer(DesTimeoutLayer::new(
//!         Duration::from_millis(400),  // Timeout per attempt
//!         Arc::downgrade(&simulation),
//!     ))
//!     .service(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! # Error Handling
//!
//! ## Timeout Errors
//! When a timeout occurs, the service returns `ServiceError::Timeout` containing:
//! - **Duration**: The configured timeout duration
//! - **Context**: Clear indication that the request timed out
//!
//! ## Request Completion Race
//! The implementation handles the race between request completion and timeout:
//! - **Success First**: Normal response is returned, timeout is cancelled
//! - **Timeout First**: Timeout error is returned, request is cancelled
//! - **Atomic State**: Uses atomic operations to prevent double-completion
//!
//! # Performance Characteristics
//!
//! ## Memory Usage
//! - **Per-Request State**: Minimal shared state between request and timeout
//! - **Atomic Operations**: Lock-free coordination using atomic primitives
//! - **Automatic Cleanup**: Resources are cleaned up when futures are dropped
//!
//! ## Timing Accuracy
//! - **DES Precision**: Timeouts fire at exact simulation time
//! - **No Jitter**: Deterministic timing without system timer variations
//! - **Reproducible**: Identical behavior across simulation runs
//!
//! ## Scalability
//! - **O(1) Operations**: Constant time timeout scheduling and cancellation
//! - **Concurrent Requests**: Supports arbitrary number of concurrent timeouts
//! - **Resource Bounded**: Timeout tasks are automatically cleaned up
//!
//! # Advanced Usage
//!
//! ## Dynamic Timeout Configuration
//! ```rust,no_run
//! # use des_components::tower::*;
//! # use std::time::Duration;
//! # fn dynamic_example() {
//! # let simulation = std::sync::Arc::new(std::sync::Mutex::new(des_core::Simulation::default()));
//! // Different timeouts for different request types
//! let fast_timeout = DesTimeoutLayer::new(
//!     Duration::from_millis(100),
//!     Arc::downgrade(&simulation),
//! );
//!
//! let slow_timeout = DesTimeoutLayer::new(
//!     Duration::from_secs(5),
//!     Arc::downgrade(&simulation),
//! );
//! # }
//! ```
//!
//! ## Timeout Monitoring
//! Monitor timeout effectiveness through:
//! - **Timeout Rate**: Percentage of requests that timeout
//! - **Response Time Distribution**: P95, P99 latencies vs. timeout threshold
//! - **Resource Utilization**: Impact of timeouts on system capacity
//!
//! # Implementation Details
//!
//! ## Thread Safety
//! - **AtomicWaker**: Lock-free waker registration for timeout notifications
//! - **Atomic State**: Thread-safe coordination between request and timeout
//! - **Weak References**: Prevents circular dependencies with simulation
//!
//! ## Resource Management
//! - **PinnedDrop**: Automatic cleanup when futures are dropped
//! - **Task Lifecycle**: Timeout tasks are automatically cleaned up after execution
//! - **Memory Safety**: No resource leaks even with cancelled requests
//!
//! ## Integration Points
//! - **DES Scheduler**: Uses TimeoutTask for precise event scheduling
//! - **Tower Ecosystem**: Compatible with all standard Tower middleware
//! - **HTTP Bodies**: Works with SimBody and other HTTP body types

use atomic_waker::AtomicWaker;
use des_core::{SimTime, Simulation};
use des_core::task::TimeoutTask;
use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
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
    simulation: Weak<Mutex<Simulation>>,
}

impl DesTimeoutLayer {
    /// Create a new timeout layer
    pub fn new(timeout: Duration, simulation: Weak<Mutex<Simulation>>) -> Self {
        Self { timeout, simulation }
    }
}

impl<S> Layer<S> for DesTimeoutLayer {
    type Service = DesTimeout<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DesTimeout::new(inner, self.timeout, self.simulation.clone())
    }
}

/// DES-aware timeout service that uses simulated time
#[derive(Clone)]
pub struct DesTimeout<S> {
    inner: S,
    timeout: Duration,
    simulation: Weak<Mutex<Simulation>>,
}

impl<S> DesTimeout<S> {
    pub fn new(inner: S, timeout: Duration, simulation: Weak<Mutex<Simulation>>) -> Self {
        Self {
            inner,
            timeout,
            simulation,
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
    simulation: Weak<Mutex<Simulation>>,
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
            simulation: self.simulation.clone(),
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
            if let Some(sim) = this.simulation.upgrade() {
                if let Ok(mut simulation) = sim.try_lock() {
                    let timeout_state = this.timeout_state.clone();
                    
                    // Create a timeout task that will expire the timeout state
                    let timeout_task = TimeoutTask::new(move |_scheduler| {
                        // Only fire timeout if request hasn't completed yet
                        if !timeout_state.is_completed() {
                            timeout_state.expire();
                        }
                    });
                    
                    // Schedule the timeout task
                    simulation.scheduler.schedule_task(
                        SimTime::from_duration(this.timeout_state.timeout_duration),
                        timeout_task,
                    );
                    
                    *this.timeout_scheduled = true;
                }
            }
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