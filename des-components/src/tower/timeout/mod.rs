//! DES-aware timeout layer

use des_core::{Component, Key, Scheduler, SimTime, Simulation};
use http::Request;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
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

/// Future for timeout operations using DES time
#[pin_project]
pub struct DesTimeoutFuture<F> {
    #[pin]
    inner: F,
    timeout_key: Option<Key<TimeoutEvent>>,
    simulation: Weak<Mutex<Simulation>>,
    timeout_expired: Arc<Mutex<bool>>,
}

/// Event type for timeout components
#[derive(Debug, Clone)]
pub enum TimeoutEvent {
    Expire,
}

/// Component that handles timeout expiration
struct TimeoutComponent {
    expired: Arc<Mutex<bool>>,
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
                *self.expired.lock().unwrap() = true;
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
        let timeout_expired = Arc::new(Mutex::new(false));

        // Schedule timeout in DES
        let timeout_key = if let Some(sim) = self.simulation.upgrade() {
            let mut simulation = sim.lock().unwrap();
            let timeout_component = TimeoutComponent {
                expired: timeout_expired.clone(),
            };
            let key = simulation.add_component(timeout_component);
            
            // Schedule timeout event
            simulation.schedule(
                SimTime::from_duration(self.timeout),
                key,
                TimeoutEvent::Expire,
            );
            
            Some(key)
        } else {
            None
        };

        DesTimeoutFuture {
            inner: inner_future,
            timeout_key,
            simulation: self.simulation.clone(),
            timeout_expired,
        }
    }
}

impl<F> Future for DesTimeoutFuture<F>
where
    F: Future<Output = Result<http::Response<SimBody>, ServiceError>>,
{
    type Output = Result<http::Response<SimBody>, ServiceError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        // Check if timeout expired
        if *this.timeout_expired.lock().unwrap() {
            return Poll::Ready(Err(ServiceError::Internal("Request timeout".to_string())));
        }

        // Poll the inner future
        this.inner.poll(cx)
    }
}