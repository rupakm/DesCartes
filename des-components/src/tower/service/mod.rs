//! DES-backed Tower Service implementation

use crate::request::{RequestAttempt, RequestAttemptId, RequestId, Response};
use crate::server::{Server, ServerEvent, ClientEvent};
use des_core::{Component, Key, Scheduler, SimTime, Simulation};
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
            self.thread_capacity,
        );

        drop(sim); // Release the lock

        Ok(DesService::new(handle))
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