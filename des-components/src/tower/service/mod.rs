//! DES-backed Tower Service implementation
//!
//! This module provides a Tower Service implementation that integrates with the discrete
//! event simulation framework. It allows Tower-based services and middleware to run
//! within simulated environments with precise timing control.
//!
//! # Overview
//!
//! The [`DesService`] struct implements the Tower [`Service`] trait, enabling it to be
//! used with Tower's ecosystem of middleware layers like rate limiting, circuit breakers,
//! timeouts, and load balancing. Unlike traditional services that use real network I/O,
//! this service operates within simulation time and uses the DES scheduler for timing.
//!
//! # Key Components
//!
//! - [`DesService`]: The main service implementation that processes HTTP requests
//! - [`DesServiceBuilder`]: Builder pattern for constructing services with validation
//! - [`SchedulerHandle`]: Interface for interacting with the DES scheduler
//! - [`DesServiceFuture`]: Future returned by service calls
//! - [`ResponseRouter`]: Pre-registered component that routes server responses to Tower futures
//!
//! # Architecture
//!
//! The service uses a **pre-registered response router** pattern to avoid deadlocks when
//! Tower services are called from within DES event handlers:
//!
//! 1. When `DesServiceBuilder::build()` is called, a single `ResponseRouter` component is
//!    registered with the simulation
//! 2. When `service.call()` is invoked, the request is scheduled to the server with the
//!    `ResponseRouter` as the client
//! 3. The server processes the request and sends the response to the `ResponseRouter`
//! 4. The `ResponseRouter` looks up the oneshot channel for that request and sends the response
//! 5. The Tower future completes with the actual server response
//!
//! This design ensures correct timing - the Tower future only completes when the server
//! actually finishes processing, regardless of whether the service was called from within
//! or outside of event processing.

use crate::{Server, ServerEvent, ClientEvent};
use des_core::{Component, Key, Scheduler, SimTime, Simulation, RequestAttempt, RequestAttemptId, RequestId, Response};
use des_core::{defer_wake, in_scheduler_context};
use des_core::dists::{ServiceTimeDistribution, ConstantServiceTime, ExponentialDistribution};

use http::Request;
use pin_project::pin_project;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::sync::oneshot;
use tower::Service;
use tower_layer::Layer;

use super::{ServiceError, SimBody, response_to_http, serialize_http_request};

// ============================================================================
// Response Router - Pre-registered component for routing server responses
// ============================================================================

/// Routes server responses to Tower service futures.
///
/// This component is registered ONCE when the service is built, and handles ALL
/// responses for that service. This avoids the need to create a new component
/// per request, which would require locking the simulation during event processing.
pub struct ResponseRouter {
    /// Map from attempt_id to oneshot sender for pending responses
    pending_responses: Arc<Mutex<HashMap<RequestAttemptId, oneshot::Sender<Response>>>>,
    /// Current load tracking (decremented when response is received)
    current_load: Arc<AtomicUsize>,
    /// Wakers for pending poll_ready calls
    wakers: Arc<Mutex<Vec<Waker>>>,
}

impl ResponseRouter {
    /// Create a new response router with shared state
    fn new(
        pending_responses: Arc<Mutex<HashMap<RequestAttemptId, oneshot::Sender<Response>>>>,
        current_load: Arc<AtomicUsize>,
        wakers: Arc<Mutex<Vec<Waker>>>,
    ) -> Self {
        Self {
            pending_responses,
            current_load,
            wakers,
        }
    }
}

impl Component for ResponseRouter {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        _scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::ResponseReceived { response } => {
                // Decrement load
                self.current_load.fetch_sub(1, Ordering::Relaxed);

                // Route response to the correct oneshot channel
                if let Some(tx) = self.pending_responses.lock().unwrap().remove(&response.attempt_id) {
                    let _ = tx.send(response.clone());
                }

                // Wake up any pending poll_ready calls
                let wakers: Vec<Waker> = {
                    let mut wakers = self.wakers.lock().unwrap();
                    std::mem::take(&mut *wakers)
                };
                for waker in wakers {
                    waker.wake();
                }
            }
            ClientEvent::SendRequest => {
                // Response router doesn't send requests
            }
            ClientEvent::RequestTimeout { .. } => {
                // Response router doesn't handle timeouts directly
            }
            ClientEvent::RetryRequest { .. } => {
                // Response router doesn't handle retries directly
            }
        }
    }
}

// ============================================================================
// Scheduler Handle - Interface for scheduling requests
// ============================================================================

/// Handle to interact with the DES scheduler from Tower services.
///
/// This handle uses a pre-registered `ResponseRouter` component to receive server
/// responses, avoiding the need to create components during event processing.
#[derive(Clone)]
pub struct SchedulerHandle {
    /// Weak reference to avoid circular dependencies
    simulation: Weak<Mutex<Simulation>>,
    /// Server component key in the simulation
    server_key: Key<ServerEvent>,
    /// Response router component key (pre-registered)
    router_key: Key<ClientEvent>,
    /// Pending response channels (shared with ResponseRouter)
    pending_responses: Arc<Mutex<HashMap<RequestAttemptId, oneshot::Sender<Response>>>>,
    /// Request ID generator
    next_request_id: Arc<AtomicU64>,
    /// Attempt ID generator
    next_attempt_id: Arc<AtomicU64>,
    /// Current load tracking
    current_load: Arc<AtomicUsize>,
    /// Maximum capacity
    pub capacity: usize,
    /// Wakers for pending poll_ready calls
    wakers: Arc<Mutex<Vec<Waker>>>,
}

impl SchedulerHandle {
    /// Create a new scheduler handle with a pre-registered response router
    pub fn new(
        simulation: Weak<Mutex<Simulation>>,
        server_key: Key<ServerEvent>,
        router_key: Key<ClientEvent>,
        pending_responses: Arc<Mutex<HashMap<RequestAttemptId, oneshot::Sender<Response>>>>,
        current_load: Arc<AtomicUsize>,
        wakers: Arc<Mutex<Vec<Waker>>>,
        capacity: usize,
    ) -> Self {
        Self {
            simulation,
            server_key,
            router_key,
            pending_responses,
            next_request_id: Arc::new(AtomicU64::new(1)),
            next_attempt_id: Arc::new(AtomicU64::new(1)),
            current_load,
            capacity,
            wakers,
        }
    }

    /// Check if the service has capacity
    pub fn has_capacity(&self) -> bool {
        self.current_load.load(Ordering::Relaxed) < self.capacity
    }

    /// Schedule a request in the DES.
    ///
    /// This method works correctly from both inside and outside scheduler context:
    /// - Inside scheduler context: uses `defer_wake` to schedule at end of current step
    /// - Outside scheduler context: locks simulation and schedules directly
    ///
    /// In both cases, the server response will be routed through the pre-registered
    /// `ResponseRouter` component, ensuring correct timing.
    pub fn schedule_request(
        &self,
        attempt: RequestAttempt,
        response_tx: oneshot::Sender<Response>,
    ) -> Result<(), ServiceError> {
        // Increment load immediately
        self.current_load.fetch_add(1, Ordering::Relaxed);

        // Store the response channel in the shared map
        // The ResponseRouter will look this up when the server responds
        {
            let mut pending = self.pending_responses.lock().unwrap();
            pending.insert(attempt.id, response_tx);
        }

        // Schedule the request to the server with the router as the client
        if in_scheduler_context() {
            // Inside event processing - use defer_wake to avoid issues
            // The server_key event will be scheduled at the end of the current step
            defer_wake(self.server_key, ServerEvent::ProcessRequest {
                attempt,
                client_id: self.router_key,
            });
            Ok(())
        } else {
            // Outside event processing - can safely lock the simulation
            if let Some(sim) = self.simulation.upgrade() {
                let mut simulation = sim.lock().unwrap();
                simulation.schedule(
                    SimTime::zero(),
                    self.server_key,
                    ServerEvent::ProcessRequest {
                        attempt,
                        client_id: self.router_key,
                    },
                );
                Ok(())
            } else {
                // Simulation dropped - clean up
                self.current_load.fetch_sub(1, Ordering::Relaxed);
                self.pending_responses.lock().unwrap().remove(&attempt.id);
                Err(ServiceError::Internal("Simulation has been dropped".to_string()))
            }
        }
    }

    /// Register a waker for poll_ready
    pub fn register_waker(&self, waker: Waker) {
        let mut wakers = self.wakers.lock().unwrap();
        wakers.push(waker);
    }
}

// ============================================================================
// Service Builder
// ============================================================================

/// Builder for creating DES-backed Tower services with full Tower ServiceBuilder compatibility.
///
/// This builder provides all the methods available in Tower's ServiceBuilder, adapted for
/// discrete event simulation. It allows you to compose middleware layers and build services
/// that run within the simulation environment.
pub struct DesServiceBuilder<L> {
    server_name: String,
    thread_capacity: usize,
    service_time_distribution: Option<Box<dyn ServiceTimeDistribution>>,
    layer: L,
}

impl Default for DesServiceBuilder<tower_layer::Identity> {
    fn default() -> Self {
        Self::new("default-server".to_string())
    }
}

impl DesServiceBuilder<tower_layer::Identity> {
    /// Create a new builder
    pub const fn new(server_name: String) -> Self {
        Self {
            server_name,
            thread_capacity: 10,
            service_time_distribution: None,
            layer: tower_layer::Identity::new(),
        }
    }
}

impl<L> DesServiceBuilder<L> {
    /// Set the server thread capacity
    pub fn thread_capacity(mut self, capacity: usize) -> Self {
        self.thread_capacity = capacity;
        self
    }

    /// Set the service time distribution
    pub fn service_time_distribution<D: ServiceTimeDistribution + 'static>(mut self, distribution: D) -> Self {
        self.service_time_distribution = Some(Box::new(distribution));
        self
    }

    /// Set constant service time per request (backward compatibility)
    pub fn service_time(self, duration: Duration) -> Self {
        self.service_time_distribution(ConstantServiceTime::new(duration))
    }

    /// Set exponential service time distribution
    pub fn exponential_service_time(self, mean_service_time: Duration) -> Self {
        let rate = 1.0 / mean_service_time.as_secs_f64();
        self.service_time_distribution(ExponentialDistribution::new(rate))
    }

    /// Add a new layer `T` into the [`DesServiceBuilder`].
    pub fn layer<T>(self, layer: T) -> DesServiceBuilder<tower_layer::Stack<T, L>> {
        DesServiceBuilder {
            server_name: self.server_name,
            thread_capacity: self.thread_capacity,
            service_time_distribution: self.service_time_distribution,
            layer: tower_layer::Stack::new(layer, self.layer),
        }
    }

    /// Optionally add a new layer `T` into the [`DesServiceBuilder`].
    pub fn option_layer<T>(
        self,
        layer: Option<T>,
    ) -> DesServiceBuilder<tower_layer::Stack<tower::util::Either<T, tower_layer::Identity>, L>> {
        match layer {
            Some(layer) => self.layer(tower::util::Either::Left(layer)),
            None => self.layer(tower::util::Either::Right(tower_layer::Identity::new())),
        }
    }

    /// Add a [`Layer`] built from a function that accepts a service and returns another service.
    pub fn layer_fn<F>(self, f: F) -> DesServiceBuilder<tower_layer::Stack<tower_layer::LayerFn<F>, L>> {
        self.layer(tower_layer::layer_fn(f))
    }

    /// Buffer requests when the next layer is not ready.
    pub fn buffer<Request>(
        self,
        bound: usize,
    ) -> DesServiceBuilder<tower_layer::Stack<tower::buffer::BufferLayer<Request>, L>> {
        self.layer(tower::buffer::BufferLayer::new(bound))
    }

    /// Limit the max number of in-flight requests.
    pub fn concurrency_limit(
        self,
        max: usize,
    ) -> DesServiceBuilder<tower_layer::Stack<super::limit::concurrency::DesConcurrencyLimitLayer, L>> {
        self.layer(super::limit::concurrency::DesConcurrencyLimitLayer::new(max))
    }

    /// Drop requests when the next layer is unable to respond to requests.
    pub fn load_shed(self) -> DesServiceBuilder<tower_layer::Stack<tower::load_shed::LoadShedLayer, L>> {
        self.layer(tower::load_shed::LoadShedLayer::new())
    }

    /// Limit requests to at most `num` per the given duration.
    pub fn rate_limit(
        self,
        num: u64,
        per: std::time::Duration,
        simulation: std::sync::Weak<std::sync::Mutex<Simulation>>,
    ) -> DesServiceBuilder<tower_layer::Stack<super::limit::rate::DesRateLimitLayer, L>> {
        let rate = num as f64 / per.as_secs_f64();
        self.layer(super::limit::rate::DesRateLimitLayer::new(rate, num as usize, simulation))
    }

    /// Retry failed requests according to the given retry policy.
    pub fn retry<P>(
        self, 
        policy: P,
        simulation: std::sync::Weak<std::sync::Mutex<Simulation>>,
    ) -> DesServiceBuilder<tower_layer::Stack<super::retry::DesRetryLayer<P>, L>> 
    where
        P: Clone,
    {
        self.layer(super::retry::DesRetryLayer::new(policy, simulation))
    }

    /// Fail requests that take longer than `timeout`.
    pub fn timeout(
        self,
        timeout: std::time::Duration,
        simulation: std::sync::Weak<std::sync::Mutex<Simulation>>,
    ) -> DesServiceBuilder<tower_layer::Stack<super::timeout::DesTimeoutLayer, L>> {
        self.layer(super::timeout::DesTimeoutLayer::new(timeout, simulation))
    }

    /// Conditionally reject requests based on `predicate`.
    pub fn filter<P>(
        self,
        predicate: P,
    ) -> DesServiceBuilder<tower_layer::Stack<tower::filter::FilterLayer<P>, L>> {
        self.layer(tower::filter::FilterLayer::new(predicate))
    }

    /// Conditionally reject requests based on an asynchronous `predicate`.
    pub fn filter_async<P>(
        self,
        predicate: P,
    ) -> DesServiceBuilder<tower_layer::Stack<tower::filter::AsyncFilterLayer<P>, L>> {
        self.layer(tower::filter::AsyncFilterLayer::new(predicate))
    }

    /// Map one request type to another.
    pub fn map_request<F, R1, R2>(
        self,
        f: F,
    ) -> DesServiceBuilder<tower_layer::Stack<tower::util::MapRequestLayer<F>, L>>
    where
        F: FnMut(R1) -> R2 + Clone,
    {
        self.layer(tower::util::MapRequestLayer::new(f))
    }

    /// Map one response type to another.
    pub fn map_response<F>(
        self,
        f: F,
    ) -> DesServiceBuilder<tower_layer::Stack<tower::util::MapResponseLayer<F>, L>> {
        self.layer(tower::util::MapResponseLayer::new(f))
    }

    /// Map one error type to another.
    pub fn map_err<F>(self, f: F) -> DesServiceBuilder<tower_layer::Stack<tower::util::MapErrLayer<F>, L>> {
        self.layer(tower::util::MapErrLayer::new(f))
    }

    /// Composes a function that transforms futures produced by the service.
    pub fn map_future<F>(self, f: F) -> DesServiceBuilder<tower_layer::Stack<tower::util::MapFutureLayer<F>, L>> {
        self.layer(tower::util::MapFutureLayer::new(f))
    }

    /// Apply an asynchronous function after the service.
    pub fn then<F>(self, f: F) -> DesServiceBuilder<tower_layer::Stack<tower::util::ThenLayer<F>, L>> {
        self.layer(tower::util::ThenLayer::new(f))
    }

    /// Executes a new future after this service's future resolves.
    pub fn and_then<F>(self, f: F) -> DesServiceBuilder<tower_layer::Stack<tower::util::AndThenLayer<F>, L>> {
        self.layer(tower::util::AndThenLayer::new(f))
    }

    /// Maps this service's result type to a different value.
    pub fn map_result<F>(self, f: F) -> DesServiceBuilder<tower_layer::Stack<tower::util::MapResultLayer<F>, L>> {
        self.layer(tower::util::MapResultLayer::new(f))
    }

    /// Add a circuit breaker that opens after a threshold of failures.
    pub fn circuit_breaker(
        self,
        failure_threshold: usize,
        recovery_timeout: std::time::Duration,
        simulation: std::sync::Weak<std::sync::Mutex<Simulation>>,
    ) -> DesServiceBuilder<tower_layer::Stack<super::circuit_breaker::DesCircuitBreakerLayer, L>> {
        self.layer(super::circuit_breaker::DesCircuitBreakerLayer::new(
            failure_threshold,
            recovery_timeout,
            simulation,
        ))
    }

    /// Add global concurrency limiting across multiple services.
    pub fn global_concurrency_limit(
        self,
        state: std::sync::Arc<super::limit::global_concurrency::GlobalConcurrencyLimitState>,
    ) -> DesServiceBuilder<tower_layer::Stack<super::limit::global_concurrency::DesGlobalConcurrencyLimitLayer, L>> {
        self.layer(super::limit::global_concurrency::DesGlobalConcurrencyLimitLayer::new(state))
    }

    /// Add hedging for request duplication.
    pub fn hedge(
        self,
        delay: std::time::Duration,
        simulation: std::sync::Weak<std::sync::Mutex<Simulation>>,
    ) -> DesServiceBuilder<tower_layer::Stack<super::hedge::DesHedgeLayer, L>> {
        self.layer(super::hedge::DesHedgeLayer::new(delay, 2, simulation))
    }

    /// Returns the underlying `Layer` implementation.
    pub fn into_inner(self) -> L {
        self.layer
    }

    /// Wrap the service `S` with the middleware provided by this builder's layers.
    pub fn service<S>(&self, service: S) -> L::Service
    where
        L: tower_layer::Layer<S>,
    {
        self.layer.layer(service)
    }

    /// Wrap the async function `F` with the middleware provided by this builder's layers.
    pub fn service_fn<F>(self, f: F) -> L::Service
    where
        L: tower_layer::Layer<tower::util::ServiceFn<F>>,
    {
        self.service(tower::util::service_fn(f))
    }

    /// Check that the builder implements `Clone`.
    #[inline]
    pub fn check_clone(self) -> Self
    where
        Self: Clone,
    {
        self
    }

    /// Check that the builder when given a service of type `S` produces a service that implements `Clone`.
    #[inline]
    pub fn check_service_clone<S>(self) -> Self
    where
        L: tower_layer::Layer<S>,
        L::Service: Clone,
    {
        self
    }

    /// Check that the builder when given a service of type `S` produces a service with the given types.
    #[inline]
    pub fn check_service<S, T, U, E>(self) -> Self
    where
        L: tower_layer::Layer<S>,
        L::Service: tower::Service<T, Response = U, Error = E>,
    {
        self
    }

    /// Build the base DES service and integrate it with the simulation.
    ///
    /// This method:
    /// 1. Creates the server component with the configured service time distribution
    /// 2. Creates and registers a `ResponseRouter` component to handle all responses
    /// 3. Creates the `SchedulerHandle` with references to both components
    /// 4. Wraps the base service with any configured middleware layers
    pub fn build(
        self,
        simulation: Arc<Mutex<Simulation>>,
    ) -> Result<L::Service, ServiceError> 
    where
        L: tower_layer::Layer<DesService>,
    {
        let mut sim = simulation.lock().unwrap();

        // Use the configured distribution or default to 100ms constant
        let service_time_distribution = self.service_time_distribution
            .unwrap_or_else(|| Box::new(ConstantServiceTime::new(Duration::from_millis(100))));

        // Create shared state for response routing
        let pending_responses = Arc::new(Mutex::new(HashMap::new()));
        let current_load = Arc::new(AtomicUsize::new(0));
        let wakers = Arc::new(Mutex::new(Vec::new()));

        // Create and register the ResponseRouter component ONCE
        // This component will handle ALL responses for this service
        let router = ResponseRouter::new(
            pending_responses.clone(),
            current_load.clone(),
            wakers.clone(),
        );
        let router_key = sim.add_component(router);

        // Create the DES server
        let server = Server::new(
            self.server_name.clone(), 
            self.thread_capacity, 
            service_time_distribution,
        );
        let server_key = sim.add_component(server);

        // Create scheduler handle with the pre-registered router
        let handle = SchedulerHandle::new(
            Arc::downgrade(&simulation),
            server_key,
            router_key,
            pending_responses,
            current_load,
            wakers,
            self.thread_capacity,
        );

        drop(sim); // Release the lock

        let base_service = DesService::new(handle);
        Ok(self.layer.layer(base_service))
    }
}

impl<L: std::fmt::Debug> std::fmt::Debug for DesServiceBuilder<L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DesServiceBuilder")
            .field("server_name", &self.server_name)
            .field("thread_capacity", &self.thread_capacity)
            .field("has_service_time_distribution", &self.service_time_distribution.is_some())
            .field("layer", &self.layer)
            .finish()
    }
}

impl<S, L> Layer<S> for DesServiceBuilder<L>
where
    L: Layer<S>,
{
    type Service = L::Service;

    fn layer(&self, inner: S) -> Self::Service {
        self.layer.layer(inner)
    }
}

// ============================================================================
// DES Service Implementation
// ============================================================================

/// Tower Service implementation backed by DES.
///
/// This service schedules requests to a simulated server and returns futures
/// that complete when the server finishes processing. The timing is controlled
/// by the DES scheduler, ensuring deterministic and reproducible behavior.
#[derive(Clone)]
pub struct DesService {
    scheduler_handle: SchedulerHandle,
}

impl DesService {
    /// Create a new DES-backed Tower service
    pub fn new(scheduler_handle: SchedulerHandle) -> Self {
        Self { scheduler_handle }
    }
}

/// Future returned by DesService::call
#[pin_project]
pub struct DesServiceFuture {
    #[pin]
    receiver: oneshot::Receiver<Response>,
}

impl Future for DesServiceFuture {
    type Output = Result<http::Response<SimBody>, ServiceError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match this.receiver.poll(cx) {
            Poll::Ready(Ok(response)) => {
                let http_response = response_to_http(response)?;
                Poll::Ready(Ok(http_response))
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(ServiceError::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Service<Request<SimBody>> for DesService {
    type Response = http::Response<SimBody>;
    type Error = ServiceError;
    type Future = DesServiceFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.scheduler_handle.has_capacity() {
            Poll::Ready(Ok(()))
        } else {
            // Register waker to be notified when capacity becomes available
            self.scheduler_handle.register_waker(cx.waker().clone());
            Poll::Pending
        }
    }

    fn call(&mut self, req: Request<SimBody>) -> Self::Future {
        let attempt = http_to_request_attempt(&self.scheduler_handle, req);
        let (tx, rx) = oneshot::channel();

        if self.scheduler_handle.schedule_request(attempt, tx).is_err() {
            // If scheduling fails, create a future that immediately returns an error
            let (error_tx, error_rx) = oneshot::channel();
            let _ = error_tx.send(Response::error(
                RequestAttemptId(0),
                RequestId(0),
                SimTime::from_duration(Duration::ZERO),
                500,
                "Failed to schedule request".to_string(),
            ));
            return DesServiceFuture { receiver: error_rx };
        }

        DesServiceFuture { receiver: rx }
    }
}

/// Convert HTTP request to RequestAttempt
fn http_to_request_attempt(
    handle: &SchedulerHandle,
    req: Request<SimBody>,
) -> RequestAttempt {
    let request_id = handle.next_request_id.fetch_add(1, Ordering::Relaxed);
    let attempt_id = handle.next_attempt_id.fetch_add(1, Ordering::Relaxed);

    // Serialize the HTTP request into payload
    let payload = serialize_http_request(&req);

    RequestAttempt::new(
        RequestAttemptId(attempt_id),
        RequestId(request_id),
        1, // First attempt
        SimTime::from_duration(Duration::ZERO), // Will be set by scheduler
        payload,
    )
}
