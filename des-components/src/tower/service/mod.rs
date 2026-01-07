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
//!
//! # Usage Example
//!
//! ```rust
//! use des_components::tower::{DesServiceBuilder, ServiceError, SimBody};
//! use des_core::Simulation;
//! use http::{Request, Method};
//! use std::sync::{Arc, Mutex};
//! use std::time::Duration;
//! use tower::Service;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create simulation
//! let simulation = Arc::new(Mutex::new(Simulation::default()));
//!
//! // Build the service
//! let mut service = DesServiceBuilder::new("web-server".to_string())
//!     .thread_capacity(10)
//!     .service_time(Duration::from_millis(50))
//!     .build(simulation.clone())?;
//!
//! // Create an HTTP request
//! let request = Request::builder()
//!     .method(Method::GET)
//!     .uri("/api/users")
//!     .body(SimBody::from_static("request body"))?;
//!
//! // Process the request (returns a Future)
//! let response_future = service.call(request);
//!
//! // In a real application, you would await the future and run the simulation
//! # Ok(())
//! # }
//! ```
//!
//! # Integration with Tower Middleware
//!
//! The service can be composed with Tower layers for advanced functionality:
//!
//! ```rust
//! use des_components::tower::{DesServiceBuilder, DesRateLimitLayer, DesConcurrencyLimitLayer};
//! use tower::ServiceBuilder;
//! use std::time::Duration;
//!
//! # async fn middleware_example() {
//! # let simulation = std::sync::Arc::new(std::sync::Mutex::new(des_core::Simulation::default()));
//! // Create base service
//! let base_service = DesServiceBuilder::new("api-server".to_string())
//!     .thread_capacity(5)
//!     .service_time(Duration::from_millis(100))
//!     .build(simulation.clone())
//!     .unwrap();
//!
//! // Compose with middleware layers
//! let service = ServiceBuilder::new()
//!     .layer(DesRateLimitLayer::new(10.0, 20, std::sync::Arc::downgrade(&simulation)))
//!     .layer(DesConcurrencyLimitLayer::new(3))
//!     .service(base_service);
//! # }
//! ```
//!
//! # Simulation Integration
//!
//! The service integrates with the DES framework by:
//!
//! 1. **Timing Control**: All operations use simulation time instead of wall-clock time
//! 2. **Event Scheduling**: Service processing is scheduled as discrete events
//! 3. **Resource Management**: Thread capacity and queuing are simulated accurately
//! 4. **Deterministic Behavior**: Results are reproducible across simulation runs
//!
//! # Performance Characteristics
//!
//! - **Service Time**: Configurable processing time per request
//! - **Capacity**: Maximum number of concurrent requests
//! - **Queuing**: Automatic backpressure when capacity is exceeded
//! - **Metrics**: Built-in tracking of throughput, latency, and utilization

use crate::{Server, ServerEvent, ClientEvent};
use des_core::{Component, Key, Scheduler, SimTime, Simulation, RequestAttempt, RequestAttemptId, RequestId, Response};
use des_core::task::Task;

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

/// Handle to interact with the DES scheduler from Tower services
#[derive(Clone)]
pub struct SchedulerHandle {
    /// Weak reference to avoid circular dependencies
    simulation: Weak<Mutex<Simulation>>,
    /// Server component key in the simulation
    server_key: Key<ServerEvent>,
    /// Pending response channels
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
    /// Create a new scheduler handle
    pub fn new(
        simulation: Weak<Mutex<Simulation>>,
        server_key: Key<ServerEvent>,
        capacity: usize,
    ) -> Self {
        Self {
            simulation,
            server_key,
            pending_responses: Arc::new(Mutex::new(HashMap::new())),
            next_request_id: Arc::new(AtomicU64::new(1)),
            next_attempt_id: Arc::new(AtomicU64::new(1)),
            current_load: Arc::new(AtomicUsize::new(0)),
            capacity,
            wakers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Check if the service has capacity
    pub fn has_capacity(&self) -> bool {
        self.current_load.load(Ordering::Relaxed) < self.capacity
    }

    /// Schedule a request in the DES
    pub fn schedule_request(
        &self,
        attempt: RequestAttempt,
        response_tx: oneshot::Sender<Response>,
    ) -> Result<(), ServiceError> {
        // Store the response channel
        {
            let mut pending = self.pending_responses.lock().unwrap();
            pending.insert(attempt.id, response_tx);
        }

        // Increment load
        self.current_load.fetch_add(1, Ordering::Relaxed);

        // Create a special client component that will schedule the response task when it receives a response
        let response_bridge = TowerResponseBridge::new(
            attempt.id,
            self.pending_responses.clone(),
            self.current_load.clone(),
            self.wakers.clone(),
        );

        // Schedule in DES
        if let Some(sim) = self.simulation.upgrade() {
            let mut simulation = sim.lock().unwrap();
            let bridge_key = simulation.add_component(response_bridge);
            
            simulation.schedule(
                SimTime::from_duration(Duration::from_millis(1)),
                self.server_key,
                ServerEvent::ProcessRequest {
                    attempt,
                    client_id: bridge_key,
                },
            );
            Ok(())
        } else {
            Err(ServiceError::Internal(
                "Simulation has been dropped".to_string(),
            ))
        }
    }

    /// Register a waker for poll_ready
    pub fn register_waker(&self, waker: Waker) {
        let mut wakers = self.wakers.lock().unwrap();
        wakers.push(waker);
    }
}

/// Bridge component that receives responses and schedules response tasks
/// This component exists temporarily to receive the response event and then schedules
/// a TowerResponseTask to handle the actual response processing
struct TowerResponseBridge {
    attempt_id: RequestAttemptId,
    pending_responses: Arc<Mutex<HashMap<RequestAttemptId, oneshot::Sender<Response>>>>,
    current_load: Arc<AtomicUsize>,
    wakers: Arc<Mutex<Vec<Waker>>>,
}

impl TowerResponseBridge {
    fn new(
        attempt_id: RequestAttemptId,
        pending_responses: Arc<Mutex<HashMap<RequestAttemptId, oneshot::Sender<Response>>>>,
        current_load: Arc<AtomicUsize>,
        wakers: Arc<Mutex<Vec<Waker>>>,
    ) -> Self {
        Self {
            attempt_id,
            pending_responses,
            current_load,
            wakers,
        }
    }
}

impl Component for TowerResponseBridge {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::ResponseReceived { response } => {
                // Schedule a task to handle the response
                let response_task = TowerResponseTask::new(
                    self.attempt_id,
                    response.clone(),
                    self.pending_responses.clone(),
                    self.current_load.clone(),
                    self.wakers.clone(),
                );
                
                // Schedule the task to execute immediately
                scheduler.schedule_task(SimTime::from_duration(Duration::ZERO), response_task);
                
                // This bridge component has served its purpose and can be garbage collected
            }
            ClientEvent::SendRequest => {
                // Response bridges don't send requests
            }
            ClientEvent::RequestTimeout { .. } => {
                // Response bridges don't handle timeouts directly
            }
            ClientEvent::RetryRequest { .. } => {
                // Response bridges don't handle retries directly
            }
        }
    }
}

/// Task that handles a single Tower request response
/// This task exists only to bridge the DES event system back to Tower futures
/// and automatically cleans up after execution
struct TowerResponseTask {
    attempt_id: RequestAttemptId,
    response: Response,
    pending_responses: Arc<Mutex<HashMap<RequestAttemptId, oneshot::Sender<Response>>>>,
    current_load: Arc<AtomicUsize>,
    wakers: Arc<Mutex<Vec<Waker>>>,
}

impl TowerResponseTask {
    fn new(
        attempt_id: RequestAttemptId,
        response: Response,
        pending_responses: Arc<Mutex<HashMap<RequestAttemptId, oneshot::Sender<Response>>>>,
        current_load: Arc<AtomicUsize>,
        wakers: Arc<Mutex<Vec<Waker>>>,
    ) -> Self {
        Self {
            attempt_id,
            response,
            pending_responses,
            current_load,
            wakers,
        }
    }
}

impl Task for TowerResponseTask {
    type Output = ();

    fn execute(self, _scheduler: &mut Scheduler) -> Self::Output {
        // Decrement load
        self.current_load.fetch_sub(1, Ordering::Relaxed);

        // Send response to waiting future
        if let Some(tx) = {
            let mut pending = self.pending_responses.lock().unwrap();
            pending.remove(&self.attempt_id)
        } {
            let _ = tx.send(self.response);
        }

        // Wake up any pending poll_ready calls
        let wakers = {
            let mut wakers = self.wakers.lock().unwrap();
            std::mem::take(&mut *wakers)
        };
        for waker in wakers {
            waker.wake();
        }
        
        // Task automatically cleans up after execution - no manual component removal needed
    }
}

/// Builder for creating DES-backed Tower services with full Tower ServiceBuilder compatibility
/// 
/// This builder provides all the methods available in Tower's ServiceBuilder, adapted for
/// discrete event simulation. It allows you to compose middleware layers and build services
/// that run within the simulation environment.
#[derive(Clone)]
pub struct DesServiceBuilder<L> {
    server_name: String,
    thread_capacity: usize,
    service_time: Duration,
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
            service_time: Duration::from_millis(100),
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

    /// Set the service time per request
    pub fn service_time(mut self, duration: Duration) -> Self {
        self.service_time = duration;
        self
    }

    /// Add a new layer `T` into the [`DesServiceBuilder`].
    ///
    /// This wraps the inner service with the service provided by a user-defined
    /// [`Layer`]. The provided layer must implement the [`Layer`] trait.
    pub fn layer<T>(self, layer: T) -> DesServiceBuilder<tower_layer::Stack<T, L>> {
        DesServiceBuilder {
            server_name: self.server_name,
            thread_capacity: self.thread_capacity,
            service_time: self.service_time,
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
    /// Note: This uses Tower's buffer layer which requires the "buffer" feature.
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
    /// Note: This uses Tower's load shed layer which requires the "load-shed" feature.
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
    /// Note: This uses Tower's filter layer which requires the "filter" feature.
    pub fn filter<P>(
        self,
        predicate: P,
    ) -> DesServiceBuilder<tower_layer::Stack<tower::filter::FilterLayer<P>, L>> {
        self.layer(tower::filter::FilterLayer::new(predicate))
    }

    /// Conditionally reject requests based on an asynchronous `predicate`.
    /// Note: This uses Tower's async filter layer which requires the "filter" feature.
    pub fn filter_async<P>(
        self,
        predicate: P,
    ) -> DesServiceBuilder<tower_layer::Stack<tower::filter::AsyncFilterLayer<P>, L>> {
        self.layer(tower::filter::AsyncFilterLayer::new(predicate))
    }

    /// Map one request type to another.
    /// Note: This uses Tower's util layer which requires the "util" feature.
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
    /// Note: This uses Tower's util layer which requires the "util" feature.
    pub fn map_response<F>(
        self,
        f: F,
    ) -> DesServiceBuilder<tower_layer::Stack<tower::util::MapResponseLayer<F>, L>> {
        self.layer(tower::util::MapResponseLayer::new(f))
    }

    /// Map one error type to another.
    /// Note: This uses Tower's util layer which requires the "util" feature.
    pub fn map_err<F>(self, f: F) -> DesServiceBuilder<tower_layer::Stack<tower::util::MapErrLayer<F>, L>> {
        self.layer(tower::util::MapErrLayer::new(f))
    }

    /// Composes a function that transforms futures produced by the service.
    /// Note: This uses Tower's util layer which requires the "util" feature.
    pub fn map_future<F>(self, f: F) -> DesServiceBuilder<tower_layer::Stack<tower::util::MapFutureLayer<F>, L>> {
        self.layer(tower::util::MapFutureLayer::new(f))
    }

    /// Apply an asynchronous function after the service, regardless of whether the future
    /// succeeds or fails.
    /// Note: This uses Tower's util layer which requires the "util" feature.
    pub fn then<F>(self, f: F) -> DesServiceBuilder<tower_layer::Stack<tower::util::ThenLayer<F>, L>> {
        self.layer(tower::util::ThenLayer::new(f))
    }

    /// Executes a new future after this service's future resolves.
    /// Note: This uses Tower's util layer which requires the "util" feature.
    pub fn and_then<F>(self, f: F) -> DesServiceBuilder<tower_layer::Stack<tower::util::AndThenLayer<F>, L>> {
        self.layer(tower::util::AndThenLayer::new(f))
    }

    /// Maps this service's result type to a different value.
    /// Note: This uses Tower's util layer which requires the "util" feature.
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
        self.layer(super::hedge::DesHedgeLayer::new(delay, 2, simulation)) // Default to 2 hedged requests
    }

    /// Returns the underlying `Layer` implementation.
    pub fn into_inner(self) -> L {
        self.layer
    }

    /// Wrap the service `S` with the middleware provided by this
    /// [`DesServiceBuilder`]'s [`Layer`]'s, returning a new [`Service`].
    pub fn service<S>(&self, service: S) -> L::Service
    where
        L: tower_layer::Layer<S>,
    {
        self.layer.layer(service)
    }

    /// Wrap the async function `F` with the middleware provided by this [`DesServiceBuilder`]'s
    /// [`Layer`]s, returning a new [`Service`].
    /// Note: This uses Tower's util layer which requires the "util" feature.
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

    /// Check that the builder when given a service of type `S` produces a service that implements
    /// `Clone`.
    #[inline]
    pub fn check_service_clone<S>(self) -> Self
    where
        L: tower_layer::Layer<S>,
        L::Service: Clone,
    {
        self
    }

    /// Check that the builder when given a service of type `S` produces a service with the given
    /// request, response, and error types.
    #[inline]
    pub fn check_service<S, T, U, E>(self) -> Self
    where
        L: tower_layer::Layer<S>,
        L::Service: tower::Service<T, Response = U, Error = E>,
    {
        self
    }

    /// Build the base DES service and integrate it with the simulation
    pub fn build(
        self,
        simulation: Arc<Mutex<Simulation>>,
    ) -> Result<L::Service, ServiceError> 
    where
        L: tower_layer::Layer<DesService>,
    {
        let mut sim = simulation.lock().unwrap();

        // Create the DES server
        let server = Server::new(self.server_name.clone(), self.thread_capacity, self.service_time);
        let server_key = sim.add_component(server);

        // Create scheduler handle
        let handle = SchedulerHandle::new(
            Arc::downgrade(&simulation),
            server_key,
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
            .field("service_time", &self.service_time)
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

/// Tower Service implementation backed by DES
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