//! Tower Service trait integration for DES components
//!
//! This module provides implementations of the Tower Service trait that allow
//! Tower-based services and middleware to run within our discrete event simulation.
//! This enables testing of real-world service architectures under simulated
//! network conditions, failures, and performance characteristics.

use crate::request::{RequestAttempt, RequestAttemptId, RequestId, Response, ResponseStatus};
use crate::server::{Server, ServerEvent, ClientEvent};
use des_core::{Component, Key, Scheduler, SimTime, Simulation};

use bytes::Bytes;
use http::{Request, Response as HttpResponse, StatusCode};
use http_body::Body as HttpBody;
use pin_project::pin_project;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::oneshot;
use tower::Service;

/// Errors that can occur in the DES Tower integration
#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("Service is not ready to accept requests")]
    NotReady,
    #[error("Request was cancelled")]
    Cancelled,
    #[error("Service is overloaded")]
    Overloaded,
    #[error("Internal simulation error: {0}")]
    Internal(String),
    #[error("HTTP error: {0}")]
    Http(#[from] http::Error),
}

/// A simple HTTP body type for our simulation
#[derive(Debug, Clone)]
pub struct SimBody {
    data: Bytes,
}

impl SimBody {
    pub fn new(data: impl Into<Bytes>) -> Self {
        Self { data: data.into() }
    }

    pub fn empty() -> Self {
        Self {
            data: Bytes::new(),
        }
    }

    pub fn from_static(data: &'static str) -> Self {
        Self {
            data: Bytes::from_static(data.as_bytes()),
        }
    }
}

impl HttpBody for SimBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if this.data.is_empty() {
            Poll::Ready(None)
        } else {
            let data = std::mem::take(&mut this.data);
            Poll::Ready(Some(Ok(http_body::Frame::data(data))))
        }
    }
}

/// Handle to interact with the DES scheduler from Tower services
#[derive(Clone)]
pub struct SchedulerHandle {
    /// Weak reference to avoid circular dependencies
    simulation: Weak<Mutex<Simulation>>,
    /// Server component key in the simulation
    server_key: Key<ServerEvent>,
    /// Client key for receiving responses
    client_key: Key<ClientEvent>,
    /// Pending response channels
    pending_responses: Arc<Mutex<HashMap<RequestAttemptId, oneshot::Sender<Response>>>>,
    /// Request ID generator
    next_request_id: Arc<AtomicU64>,
    /// Attempt ID generator
    next_attempt_id: Arc<AtomicU64>,
    /// Current load tracking
    current_load: Arc<AtomicUsize>,
    /// Maximum capacity
    capacity: usize,
    /// Wakers for pending poll_ready calls
    wakers: Arc<Mutex<Vec<Waker>>>,
}

impl SchedulerHandle {
    /// Create a new scheduler handle
    pub fn new(
        simulation: Weak<Mutex<Simulation>>,
        server_key: Key<ServerEvent>,
        client_key: Key<ClientEvent>,
        capacity: usize,
    ) -> Self {
        Self {
            simulation,
            server_key,
            client_key,
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

        // Schedule in DES
        if let Some(sim) = self.simulation.upgrade() {
            let mut simulation = sim.lock().unwrap();
            simulation.schedule(
                SimTime::from_duration(Duration::from_millis(1)),
                self.server_key,
                ServerEvent::ProcessRequest {
                    attempt,
                    client_id: self.client_key,
                },
            );
            Ok(())
        } else {
            Err(ServiceError::Internal(
                "Simulation has been dropped".to_string(),
            ))
        }
    }

    /// Handle a response from the DES
    pub fn handle_response(&self, response: Response) {
        // Decrement load
        self.current_load.fetch_sub(1, Ordering::Relaxed);

        // Send response to waiting future
        if let Some(tx) = {
            let mut pending = self.pending_responses.lock().unwrap();
            pending.remove(&response.attempt_id)
        } {
            let _ = tx.send(response);
        }

        // Wake up any pending poll_ready calls
        let wakers = {
            let mut wakers = self.wakers.lock().unwrap();
            std::mem::take(&mut *wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    /// Register a waker for poll_ready
    pub fn register_waker(&self, waker: Waker) {
        let mut wakers = self.wakers.lock().unwrap();
        wakers.push(waker);
    }
}

/// Tower client component that handles responses and forwards them to the scheduler handle
pub struct TowerClient {
    pub name: String,
    pub scheduler_handle: SchedulerHandle,
}

impl TowerClient {
    pub fn new(name: String, scheduler_handle: SchedulerHandle) -> Self {
        Self {
            name,
            scheduler_handle,
        }
    }
}

impl Component for TowerClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        _scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::ResponseReceived { response } => {
                println!(
                    "[{}] Received response for attempt {} (request {}): {}",
                    self.name,
                    response.attempt_id,
                    response.request_id,
                    if response.is_success() {
                        "SUCCESS"
                    } else {
                        "FAILURE"
                    }
                );
                self.scheduler_handle.handle_response(response.clone());
            }
            ClientEvent::SendRequest => {
                // Tower client doesn't send requests, only receives responses
            }
        }
    }
}

/// Tower Service implementation backed by DES
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
    type Output = Result<HttpResponse<SimBody>, ServiceError>;

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
    type Response = HttpResponse<SimBody>;
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

/// Convert DES Response to HTTP response
fn response_to_http(response: Response) -> Result<HttpResponse<SimBody>, ServiceError> {
    match response.status {
        ResponseStatus::Ok => {
            let body = if response.payload.is_empty() {
                SimBody::from_static("OK")
            } else {
                SimBody::new(response.payload)
            };

            HttpResponse::builder()
                .status(StatusCode::OK)
                .body(body)
                .map_err(ServiceError::Http)
        }
        ResponseStatus::Error { code, message } => {
            let status = StatusCode::from_u16(code as u16)
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

            HttpResponse::builder()
                .status(status)
                .body(SimBody::new(message))
                .map_err(ServiceError::Http)
        }
    }
}

/// Serialize HTTP request to bytes (simplified)
fn serialize_http_request(req: &Request<SimBody>) -> Vec<u8> {
    // Simple serialization - in practice you might use a more sophisticated format
    let method = req.method().as_str();
    let uri = req.uri().to_string();
    let headers = req
        .headers()
        .iter()
        .map(|(k, v)| format!("{}: {}", k, v.to_str().unwrap_or("")))
        .collect::<Vec<_>>()
        .join("\r\n");

    format!("{method} {uri} HTTP/1.1\r\n{headers}\r\n\r\n").into_bytes()
}

/// Builder for creating DES-backed Tower services
pub struct DesServiceBuilder {
    server_name: String,
    thread_capacity: usize,
    service_time: Duration,
}

impl DesServiceBuilder {
    /// Create a new builder
    pub fn new(server_name: String) -> Self {
        Self {
            server_name,
            thread_capacity: 10,
            service_time: Duration::from_millis(100),
        }
    }

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

    /// Build the service and integrate it with the simulation
    pub fn build(
        self,
        simulation: Arc<Mutex<Simulation>>,
    ) -> Result<DesService, ServiceError> {
        let mut sim = simulation.lock().unwrap();

        // Create the DES server
        let server = Server::new(self.server_name.clone(), self.thread_capacity, self.service_time);
        let server_key = sim.add_component(server);

        // Create scheduler handle
        let handle = SchedulerHandle::new(
            Arc::downgrade(&simulation),
            server_key,
            Key::new_with_id(uuid::Uuid::now_v7()), // Temporary key, will be replaced
            self.thread_capacity,
        );

        // Create Tower client
        let client = TowerClient::new(format!("{}-tower-client", self.server_name), handle.clone());
        let client_key = sim.add_component(client);

        // Update the handle with the correct client key
        let handle = SchedulerHandle::new(
            Arc::downgrade(&simulation),
            server_key,
            client_key,
            self.thread_capacity,
        );

        // Update the client with the correct handle
        if let Some(client) = sim.get_component_mut::<ClientEvent, TowerClient>(client_key) {
            client.scheduler_handle = handle.clone();
        }

        drop(sim); // Release the lock

        Ok(DesService::new(handle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use des_core::Simulation;
    use http::Method;
    use std::sync::Arc;
    use tower::Service;

    #[tokio::test]
    async fn test_des_service_basic() {
        let simulation = Arc::new(Mutex::new(Simulation::default()));

        // Build the service
        let mut service = DesServiceBuilder::new("test-server".to_string())
            .thread_capacity(2)
            .service_time(Duration::from_millis(50))
            .build(simulation.clone())
            .unwrap();

        // Create a test request
        let request = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .body(SimBody::from_static("test body"))
            .unwrap();

        // Make the request and run simulation concurrently
        let response_future = service.call(request);
        
        // Run a few simulation steps to process the request
        for _ in 0..10 {
            tokio::task::yield_now().await;
            let mut sim = simulation.lock().unwrap();
            for _ in 0..5 {
                if !sim.step() {
                    break;
                }
            }
        }

        // The response should be ready now
        let response = tokio::time::timeout(Duration::from_millis(100), response_future)
            .await
            .expect("Response should be ready")
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_des_service_with_tower_middleware() {
        use tower::timeout::Timeout;

        let simulation = Arc::new(Mutex::new(Simulation::default()));

        // Build the service with Tower timeout middleware
        let base_service = DesServiceBuilder::new("middleware-server".to_string())
            .thread_capacity(1)
            .service_time(Duration::from_millis(30))
            .build(simulation.clone())
            .unwrap();

        let mut service = Timeout::new(base_service, Duration::from_millis(100));

        // Create a test request
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/test")
            .body(SimBody::from_static("middleware test"))
            .unwrap();

        // Make the request and run simulation concurrently
        let response_future = service.call(request);
        
        // Run simulation steps
        for _ in 0..10 {
            tokio::task::yield_now().await;
            let mut sim = simulation.lock().unwrap();
            for _ in 0..5 {
                if !sim.step() {
                    break;
                }
            }
        }

        // Make the request through middleware
        let response = tokio::time::timeout(Duration::from_millis(100), response_future)
            .await
            .expect("Response should be ready")
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_des_service_capacity_limit() {
        let simulation = Arc::new(Mutex::new(Simulation::default()));

        // Build service with capacity 1 and slow service time
        let mut service = DesServiceBuilder::new("capacity-test".to_string())
            .thread_capacity(1)
            .service_time(Duration::from_millis(100))
            .build(simulation.clone())
            .unwrap();

        // Send first request
        let req1 = Request::builder()
            .method(Method::GET)
            .uri("/test1")
            .body(SimBody::empty())
            .unwrap();

        let future1 = service.call(req1);

        // Wait a bit then send second request
        tokio::time::sleep(Duration::from_millis(20)).await;

        let req2 = Request::builder()
            .method(Method::GET)
            .uri("/test2")
            .body(SimBody::empty())
            .unwrap();

        let future2 = service.call(req2);

        // Run simulation steps to process both requests
        for _ in 0..20 {
            tokio::task::yield_now().await;
            let mut sim = simulation.lock().unwrap();
            for _ in 0..10 {
                if !sim.step() {
                    break;
                }
            }
        }

        // Both should complete, but second should wait for first
        let (resp1, resp2) = tokio::join!(
            tokio::time::timeout(Duration::from_millis(200), future1),
            tokio::time::timeout(Duration::from_millis(200), future2)
        );

        assert!(resp1.is_ok());
        assert!(resp2.is_ok());
    }
}