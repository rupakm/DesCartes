//! DES-aware timeout layer with proper event scheduling
//!
//! This module provides a timeout implementation that properly integrates with the DES
//! scheduler, using DES components and events to fire timeouts deterministically.

use atomic_waker::AtomicWaker;
use des_core::{Component, Key, Scheduler, SimTime, Simulation};
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

/// Shared state between timeout future and timeout component
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

/// Future for timeout operations using DES events
#[pin_project(PinnedDrop)]
pub struct DesTimeoutFuture<F> {
    #[pin]
    inner: F,
    timeout_key: Option<Key<TimeoutEvent>>,
    simulation: Weak<Mutex<Simulation>>,
    timeout_state: Arc<TimeoutState>,
    completed: bool,
}

/// Event type for timeout components
#[derive(Debug, Clone)]
pub enum TimeoutEvent {
    /// Timeout has expired - fire the timeout
    Expire,
}

/// Component that handles timeout expiration via DES events
struct TimeoutComponent {
    state: Arc<TimeoutState>,
}

impl Component for TimeoutComponent {
    type Event = TimeoutEvent;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        _scheduler: &mut Scheduler,
    ) {
        match event {
            TimeoutEvent::Expire => {
                // Only fire timeout if request hasn't completed yet
                if !self.state.is_completed() {
                    self.state.expire();
                }
            }
        }
    }
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
            timeout_key: None,
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
        if this.timeout_key.is_none() {
            if let Some(sim) = this.simulation.upgrade() {
                if let Ok(mut simulation) = sim.try_lock() {
                    let current_time = simulation.scheduler.time();
                    
                    let timeout_component = TimeoutComponent {
                        state: this.timeout_state.clone(),
                    };
                    let key = simulation.add_component(timeout_component);
                    
                    // Schedule timeout event at current_time + timeout_duration
                    let timeout_time = current_time + SimTime::from_duration(this.timeout_state.timeout_duration);
                    simulation.schedule(
                        timeout_time,
                        key,
                        TimeoutEvent::Expire,
                    );
                    
                    *this.timeout_key = Some(key);
                }
            }
        }

        // Store the waker so timeout component can wake us
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

/// Cleanup timeout component when future is dropped
#[pin_project::pinned_drop]
impl<F> PinnedDrop for DesTimeoutFuture<F> {
    fn drop(self: Pin<&mut Self>) {
        // Mark as completed to prevent timeout from firing
        self.timeout_state.complete();

        // Remove the timeout component from simulation to prevent resource leak
        if let Some(timeout_key) = self.timeout_key {
            if let Some(sim) = self.simulation.upgrade() {
                if let Ok(mut simulation) = sim.try_lock() {
                    // Try to remove the component - it's okay if it fails
                    // (e.g., if simulation is shutting down)
                    let _ = simulation.remove_component::<TimeoutEvent, TimeoutComponent>(timeout_key);
                }
            }
        }
    }
}