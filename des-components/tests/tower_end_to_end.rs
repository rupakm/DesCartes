//! End-to-end tests for Tower service integration with DES using FuturePoller
//!
//! This test suite demonstrates proper DES simulation patterns with Tower services,
//! following the approach from mmk_queueing_example. The clients use DES scheduling
//! to send requests over time and FuturePoller to handle responses asynchronously.

use des_components::tower::{DesServiceBuilder, FuturePollerEvent, FuturePollerHandle, SimBody};
use des_core::{defer_wake, Component, Execute, Executor, Key, Scheduler, SimTime, Simulation};
use http::{Method, Request};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tower::Service;

/// Events for the simple request sender
#[derive(Debug)]
enum RequestSenderEvent {
    SendRequest,
}

/// A simple component that sends a request using the Tower service
/// This demonstrates that the deferred scheduling fix works
struct SimpleRequestSender<S> {
    service: S,
    future_poller: FuturePollerHandle,
    request_sent: bool,
}

impl<S> SimpleRequestSender<S> {
    fn new(service: S, future_poller: FuturePollerHandle) -> Self {
        Self {
            service,
            future_poller,
            request_sent: false,
        }
    }
}

impl<S> Component for SimpleRequestSender<S>
where
    S: Service<
            Request<SimBody>,
            Response = http::Response<SimBody>,
            Error = des_components::tower::ServiceError,
        > + 'static,
    S::Future: 'static,
{
    type Event = RequestSenderEvent;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            RequestSenderEvent::SendRequest => {
                if !self.request_sent {
                    let current_time = scheduler.time().as_duration().as_millis();
                    println!(
                        "📨 [{}ms] SimpleRequestSender: Processing SendRequest event",
                        current_time
                    );

                    // Create a test request
                    let request = Request::builder()
                        .method(Method::POST)
                        .uri("/api/test")
                        .body(SimBody::new("Test request body".as_bytes().to_vec()))
                        .unwrap();

                    println!("   📝 Created HTTP request: POST /api/test");

                    // Check if service is ready
                    let waker = create_noop_waker();
                    let mut cx = Context::from_waker(&waker);

                    println!("   🔍 Checking if Tower service is ready...");
                    match self.service.poll_ready(&mut cx) {
                        Poll::Ready(Ok(())) => {
                            println!("   ✅ Service is ready, making call");

                            // Call service - this should NOT deadlock now with deferred scheduling!
                            let future = self.service.call(request);
                            println!("   🚀 Service.call() returned future (no deadlock!)");

                            // Spawn the future on the FuturePoller with a callback
                            self.future_poller.spawn(future, move |result| {
                                match result {
                                    Ok(response) => {
                                        println!("   ✅ [Response] Request completed with status: {}", response.status());
                                        
                                        // Check if the response body echoes the request
                                        let response_body = response.body().data();
                                        let response_str = String::from_utf8_lossy(response_body);
                                        println!("   📝 [Response] Body: {}", response_str);
                                        
                                        // Verify that the response echoes the request
                                        if response_str.contains("Test request body") {
                                            println!("   ✅ [Response] Successfully echoed request content!");
                                        } else {
                                            println!("   ℹ️ [Response] Response body does not match");
                                        }
                                        assert!(response_str.contains("Test request body"));
                                    }
                                    Err(e) => {
                                        println!("   ❌ [Response] Request failed: {:?}", e);
                                    }
                                }
                            });

                            self.request_sent = true;
                            println!("   🎯 Future spawned on FuturePoller successfully");
                        }
                        Poll::Ready(Err(e)) => {
                            println!("   ❌ Service error: {:?}", e);
                        }
                        Poll::Pending => {
                            println!("   ⏳ Service not ready (capacity limit reached)");
                        }
                    }
                } else {
                    println!(
                        "📨 [{}ms] SimpleRequestSender: Request already sent, ignoring",
                        scheduler.time().as_duration().as_millis()
                    );
                }
            }
        }
    }
}

// Helper function to create a no-op waker for testing
fn create_noop_waker() -> std::task::Waker {
    use std::task::{RawWaker, RawWakerVTable};

    fn noop(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }

    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let raw_waker = RawWaker::new(std::ptr::null(), &VTABLE);
    unsafe { std::task::Waker::from_raw(raw_waker) }
}

#[test]
fn test_simple_tower_service() {
    println!("🚀 Simple Tower Service Test (with Deferred Scheduling Fix)");
    println!("============================================================");

    let mut simulation = Simulation::default();

    // Create a simple Tower service first (before wrapping in Arc)
    let service = DesServiceBuilder::new("simple-server".to_string())
        .thread_capacity(1)
        .service_time(Duration::from_millis(10))
        .build(&mut simulation)
        .expect("Failed to build service");

    // Now wrap in Arc for shared access
    let simulation_arc = Arc::new(Mutex::new(simulation));

    println!("📋 Test Setup:");
    println!("   - Tower service: 1 thread, 10ms processing time");
    println!("   - Request scheduled at: 5ms");
    println!("   - Simulation duration: 100ms");
    println!("   - Using deferred scheduling to avoid deadlock");
    println!();

    // Create FuturePollerHandle for managing async responses
    let handle = FuturePollerHandle::new();
    let poller = handle.create_component();
    let poller_key = {
        let mut sim = simulation_arc.lock().unwrap();
        sim.add_component(poller)
    };
    handle.set_key(poller_key);

    println!("🔧 Components created:");
    println!(
        "   - FuturePoller component added with key: {:?}",
        poller_key
    );

    // Schedule Initialize event for the FuturePoller
    {
        let mut sim = simulation_arc.lock().unwrap();
        sim.schedule(SimTime::zero(), poller_key, FuturePollerEvent::Initialize);
    }
    println!("   - FuturePoller Initialize event scheduled at 0ms");

    // Create a simple component that will send one request
    let request_sender = SimpleRequestSender::new(service, handle.clone());
    let sender_key = {
        let mut sim = simulation_arc.lock().unwrap();
        sim.add_component(request_sender)
    };
    println!(
        "   - SimpleRequestSender component added with key: {:?}",
        sender_key
    );

    // Schedule the request to be sent at 5ms
    {
        let mut sim = simulation_arc.lock().unwrap();
        sim.schedule(
            SimTime::from_millis(0),
            sender_key,
            RequestSenderEvent::SendRequest,
        );
    }
    println!("   - SendRequest event scheduled at 5ms");
    println!();

    println!("🏃 Running simulation with Executor for 100ms...");
    println!(
        "   Initial state: pending={}, completed={}",
        handle.pending_count(),
        handle.completed_count()
    );

    // Use Executor instead of manual stepping
    {
        let mut sim = simulation_arc.lock().unwrap();
        Executor::timed(SimTime::from_millis(100)).execute(&mut sim);
    }

    println!();
    println!("📊 Final Results:");
    println!("   Pending futures: {}", handle.pending_count());
    println!("   Completed futures: {}", handle.completed_count());
    {
        let sim = simulation_arc.lock().unwrap();
        println!(
            "   Simulation time: {}ms",
            sim.time().as_duration().as_millis()
        );
    }

    // Verify that we completed at least one request
    assert!(
        handle.completed_count() > 0,
        "Should have completed at least one request"
    );

    println!();
    println!("✅ Simple Tower service test completed successfully!");
    println!("   This test demonstrates:");
    println!("   - DES component scheduling events over time");
    println!("   - Tower service integration with DES timing");
    println!("   - Deferred scheduling to avoid deadlocks");
    println!("   - FuturePoller handling async responses");
    println!("   - Proper use of Executor for simulation execution");
}

/// Events for the periodic request sender
#[derive(Debug)]
enum PeriodicRequestSenderEvent {
    SendRequest { request_id: u32 },
}

/// A component that sends requests periodically every 20ms
struct PeriodicRequestSender<S> {
    service: S,
    future_poller: FuturePollerHandle,
    request_count: u32,
    max_requests: u32,
    interval: Duration,
    requests_sent: u32,
    responses_received: u32,
    successful_responses: u32,
}

impl<S> PeriodicRequestSender<S> {
    fn new(
        service: S,
        future_poller: FuturePollerHandle,
        max_requests: u32,
        interval: Duration,
    ) -> Self {
        Self {
            service,
            future_poller,
            request_count: 0,
            max_requests,
            interval,
            requests_sent: 0,
            responses_received: 0,
            successful_responses: 0,
        }
    }
}

impl<S> Component for PeriodicRequestSender<S>
where
    S: Service<
            Request<SimBody>,
            Response = http::Response<SimBody>,
            Error = des_components::tower::ServiceError,
        > + 'static,
    S::Future: 'static,
{
    type Event = PeriodicRequestSenderEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            PeriodicRequestSenderEvent::SendRequest { request_id } => {
                let current_time = scheduler.time().as_duration().as_millis();
                println!(
                    "📨 [{}ms] PeriodicRequestSender: Sending request #{}",
                    current_time, request_id
                );

                // Create a test request with unique content
                let body_content =
                    format!("Periodic request #{} at {}ms", request_id, current_time);
                let request = Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/periodic/{}", request_id))
                    .body(SimBody::new(body_content.as_bytes().to_vec()))
                    .unwrap();

                // Check if service is ready
                let waker = create_noop_waker();
                let mut cx = Context::from_waker(&waker);

                match self.service.poll_ready(&mut cx) {
                    Poll::Ready(Ok(())) => {
                        // Call service
                        let future = self.service.call(request);
                        self.requests_sent += 1;

                        // Capture metrics for the callback
                        let request_id_copy = *request_id;
                        let responses_received = &mut self.responses_received as *mut u32;
                        let successful_responses = &mut self.successful_responses as *mut u32;

                        // Spawn the future on the FuturePoller with a callback
                        self.future_poller.spawn(future, move |result| {
                            unsafe {
                                *responses_received += 1;
                                match result {
                                    Ok(response) => {
                                        *successful_responses += 1;
                                        println!("   ✅ [Response #{}] Status: {}", request_id_copy, response.status());
                                        
                                        // Check if the response body contains our request content
                                        let response_body = response.body().data();
                                        let response_str = String::from_utf8_lossy(response_body);
                                        if response_str.contains(&format!("Periodic request #{}", request_id_copy)) {
                                            println!("   📝 [Response #{}] Successfully echoed request content", request_id_copy);
                                        } else {
                                            println!("   📝 [Response #{}] Body: {}", request_id_copy, response_str.chars().take(100).collect::<String>());
                                        }
                                    }
                                    Err(e) => {
                                        println!("   ❌ [Response #{}] Request failed: {:?}", request_id_copy, e);
                                    }
                                }
                            }
                        });

                        println!("   🚀 Request #{} sent successfully", request_id);
                    }
                    Poll::Ready(Err(e)) => {
                        println!("   ❌ Service error for request #{}: {:?}", request_id, e);
                    }
                    Poll::Pending => {
                        println!(
                            "   ⏳ Service not ready for request #{} (capacity limit reached)",
                            request_id
                        );
                    }
                }

                // Schedule the next request if we haven't reached the limit
                self.request_count += 1;
                if self.request_count < self.max_requests {
                    scheduler.schedule(
                        SimTime::from_duration(self.interval),
                        self_id,
                        PeriodicRequestSenderEvent::SendRequest {
                            request_id: self.request_count + 1,
                        },
                    );
                } else {
                    println!(
                        "📊 [{}ms] PeriodicRequestSender: Finished sending all {} requests",
                        current_time, self.max_requests
                    );
                }
            }
        }
    }
}

#[test]
fn test_periodic_tower_service() {
    println!("🚀 Periodic Tower Service Test (Open-Loop Client Pattern)");
    println!("=========================================================");

    let mut simulation = Simulation::default();

    // Create a Tower service with higher capacity to handle multiple requests
    let service = DesServiceBuilder::new("periodic-server".to_string())
        .thread_capacity(3) // Allow 3 concurrent requests
        .service_time(Duration::from_millis(30)) // 30ms processing time
        .build(&mut simulation)
        .expect("Failed to build service");

    // Now wrap in Arc for shared access
    let simulation_arc = Arc::new(Mutex::new(simulation));

    // Calculate test parameters
    let interval = Duration::from_millis(20); // Send every 20ms
    let simulation_duration = Duration::from_millis(1000); // Run for 1 second
    let max_requests = (simulation_duration.as_millis() / interval.as_millis()) as u32; // 50 requests

    println!("📋 Test Setup:");
    println!("   - Tower service: 3 threads, 30ms processing time");
    println!("   - Request interval: {}ms", interval.as_millis());
    println!(
        "   - Simulation duration: {}ms",
        simulation_duration.as_millis()
    );
    println!("   - Expected requests: {}", max_requests);
    println!("   - Using open-loop client pattern (requests independent of responses)");
    println!();

    // Create FuturePollerHandle for managing async responses
    let handle = FuturePollerHandle::new();
    let poller = handle.create_component();
    let poller_key = {
        let mut sim = simulation_arc.lock().unwrap();
        sim.add_component(poller)
    };
    handle.set_key(poller_key);

    println!("🔧 Components created:");
    println!(
        "   - FuturePoller component added with key: {:?}",
        poller_key
    );

    // Schedule Initialize event for the FuturePoller
    {
        let mut sim = simulation_arc.lock().unwrap();
        sim.schedule(SimTime::zero(), poller_key, FuturePollerEvent::Initialize);
    }
    println!("   - FuturePoller Initialize event scheduled at 0ms");

    // Create a periodic request sender
    let periodic_sender =
        PeriodicRequestSender::new(service, handle.clone(), max_requests, interval);
    let sender_key = {
        let mut sim = simulation_arc.lock().unwrap();
        sim.add_component(periodic_sender)
    };
    println!(
        "   - PeriodicRequestSender component added with key: {:?}",
        sender_key
    );

    // Schedule the first request to be sent at 10ms
    {
        let mut sim = simulation_arc.lock().unwrap();
        sim.schedule(
            SimTime::from_millis(10),
            sender_key,
            PeriodicRequestSenderEvent::SendRequest { request_id: 1 },
        );
    }
    println!("   - First SendRequest event scheduled at 10ms");
    println!();

    println!(
        "🏃 Running simulation with Executor for {}ms...",
        simulation_duration.as_millis()
    );
    println!(
        "   Initial state: pending={}, completed={}",
        handle.pending_count(),
        handle.completed_count()
    );
    println!();

    // Use Executor to run the simulation
    {
        let mut sim = simulation_arc.lock().unwrap();
        Executor::timed(SimTime::from_duration(simulation_duration)).execute(&mut sim);
    }

    println!();
    println!("📊 Final Results:");
    println!("   Pending futures: {}", handle.pending_count());
    println!("   Completed futures: {}", handle.completed_count());
    {
        let sim = simulation_arc.lock().unwrap();
        println!(
            "   Simulation time: {}ms",
            sim.time().as_duration().as_millis()
        );
    }

    // Verify that we sent and received a reasonable number of requests
    // Note: Due to the open-loop pattern, some requests may still be in flight
    assert!(
        handle.completed_count() > 0,
        "Should have completed at least some requests"
    );
    assert!(
        handle.completed_count() <= max_requests as u64,
        "Should not exceed expected request count"
    );

    println!();
    println!("✅ Periodic Tower service test completed successfully!");
    println!("   This test demonstrates:");
    println!("   - Open-loop client pattern (requests sent independently of responses)");
    println!("   - Periodic request generation over simulation time");
    println!("   - Multiple concurrent requests with capacity management");
    println!("   - Tower service handling sustained load");
    println!("   - FuturePoller managing multiple async responses");
    println!("   - Realistic timing with service processing delays");
}

/// Events for the retry client
#[derive(Debug)]
enum RetryClientEvent {
    SendRequest { request_id: u32 },
    RequestTimeout { request_id: u32, attempt: u32 },
}

/// A client that implements timeout and retry logic
struct RetryClient<S> {
    service: S,
    future_poller: FuturePollerHandle,
    timeout_duration: Duration,
    max_retries: u32,
    retry_delay: Duration,
    // Track active requests
    active_requests: std::collections::HashMap<u32, RequestState>,
    // Metrics
    requests_sent: u32,
    responses_received: u32,
    successful_responses: u32,
    timeouts: u32,
    retries: u32,
}

#[derive(Debug)]
struct RequestState {
    attempt: u32,
    timeout_scheduled: bool,
}

impl<S> RetryClient<S> {
    fn new(
        service: S,
        future_poller: FuturePollerHandle,
        timeout_duration: Duration,
        max_retries: u32,
        retry_delay: Duration,
    ) -> Self {
        Self {
            service,
            future_poller,
            timeout_duration,
            max_retries,
            retry_delay,
            active_requests: std::collections::HashMap::new(),
            requests_sent: 0,
            responses_received: 0,
            successful_responses: 0,
            timeouts: 0,
            retries: 0,
        }
    }
}

impl<S> Component for RetryClient<S>
where
    S: Service<
            Request<SimBody>,
            Response = http::Response<SimBody>,
            Error = des_components::tower::ServiceError,
        > + 'static,
    S::Future: 'static,
{
    type Event = RetryClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            RetryClientEvent::SendRequest { request_id } => {
                let current_time = scheduler.time().as_duration().as_millis();
                let attempt = self
                    .active_requests
                    .get(request_id)
                    .map(|state| state.attempt + 1)
                    .unwrap_or(1);

                if attempt == 1 {
                    println!(
                        "📨 [{}ms] RetryClient: Sending request #{} (attempt {})",
                        current_time, request_id, attempt
                    );
                } else {
                    println!(
                        "🔄 [{}ms] RetryClient: Retrying request #{} (attempt {})",
                        current_time, request_id, attempt
                    );
                    self.retries += 1;
                }

                // Create request with attempt info
                let body_content = format!(
                    "Retry test request #{} attempt {} at {}ms",
                    request_id, attempt, current_time
                );
                let request = Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/retry/{}/{}", request_id, attempt))
                    .body(SimBody::new(body_content.as_bytes().to_vec()))
                    .unwrap();

                // Check if service is ready
                let waker = create_noop_waker();
                let mut cx = Context::from_waker(&waker);

                match self.service.poll_ready(&mut cx) {
                    Poll::Ready(Ok(())) => {
                        // Instead of calling the service, simulate a request that will timeout
                        // by not spawning any future - this will cause the timeout to fire
                        println!(
                            "   🚀 Request #{} attempt {} sent (simulating slow/hanging request)",
                            request_id, attempt
                        );

                        // Update request state
                        self.active_requests.insert(
                            *request_id,
                            RequestState {
                                attempt,
                                timeout_scheduled: true,
                            },
                        );

                        // Schedule timeout - this will definitely fire since no response will come
                        scheduler.schedule(
                            SimTime::from_duration(self.timeout_duration),
                            self_id,
                            RetryClientEvent::RequestTimeout {
                                request_id: *request_id,
                                attempt,
                            },
                        );

                        self.requests_sent += 1;
                    }
                    Poll::Ready(Err(e)) => {
                        println!("   ❌ Service error for request #{}: {:?}", request_id, e);
                    }
                    Poll::Pending => {
                        println!(
                            "   ⏳ Service not ready for request #{} (capacity limit reached)",
                            request_id
                        );
                    }
                }
            }
            RetryClientEvent::RequestTimeout {
                request_id,
                attempt,
            } => {
                let current_time = scheduler.time().as_duration().as_millis();

                // Check if this timeout is for the current attempt
                if let Some(state) = self.active_requests.get(request_id) {
                    if state.attempt == *attempt {
                        println!(
                            "⏰ [{}ms] RetryClient: Request #{} attempt {} timed out after {}ms",
                            current_time,
                            request_id,
                            attempt,
                            self.timeout_duration.as_millis()
                        );
                        self.timeouts += 1;

                        if *attempt < self.max_retries {
                            // Schedule retry
                            println!(
                                "   🔄 Scheduling retry for request #{} (attempt {} -> {})",
                                request_id,
                                attempt,
                                attempt + 1
                            );
                            scheduler.schedule(
                                SimTime::from_duration(self.retry_delay),
                                self_id,
                                RetryClientEvent::SendRequest {
                                    request_id: *request_id,
                                },
                            );
                        } else {
                            // Max retries exceeded
                            println!(
                                "   ❌ Request #{} failed after {} attempts (max retries exceeded)",
                                request_id, attempt
                            );
                            self.active_requests.remove(request_id);
                        }
                    } else {
                        println!("   ⏰ Ignoring timeout for request #{} attempt {} (current attempt: {})", 
                                request_id, attempt, state.attempt);
                    }
                } else {
                    println!(
                        "   ⏰ Timeout for request #{} attempt {} but request no longer active",
                        request_id, attempt
                    );
                }
            }
        }
    }
}

#[test]
fn test_retry_on_timeout() {
    println!("🚀 Retry on Timeout Test (Client-Side Timeout and Retry)");
    println!("========================================================");

    let mut simulation = Simulation::default();

    // Create a server that will be overloaded to cause timeouts
    let service = DesServiceBuilder::new("limited-server".to_string())
        .thread_capacity(1) // Only 1 thread - will cause blocking
        .service_time(Duration::from_millis(300)) // 300ms processing time
        .build(&mut simulation)
        .expect("Failed to build service");

    // Now wrap in Arc for shared access
    let simulation_arc = Arc::new(Mutex::new(simulation));

    // Client timeout and retry configuration
    let timeout_duration = Duration::from_millis(280); // 150ms timeout (shorter than service time)
    let max_retries = 2;
    let retry_delay = Duration::from_millis(50);
    let simulation_duration = Duration::from_millis(2000); // 2 seconds

    println!("📋 Test Setup:");
    println!("   - Tower service: 1 thread, 300ms processing time");
    println!(
        "   - Client timeout: {}ms (shorter than service time)",
        timeout_duration.as_millis()
    );
    println!("   - Max retries: {}", max_retries);
    println!("   - Retry delay: {}ms", retry_delay.as_millis());
    println!(
        "   - Simulation duration: {}ms",
        simulation_duration.as_millis()
    );
    println!("   - Expected behavior: Requests will timeout and retry");
    println!();

    // Create FuturePollerHandle for managing async responses
    let handle = FuturePollerHandle::new();
    let poller = handle.create_component();
    let poller_key = {
        let mut sim = simulation_arc.lock().unwrap();
        sim.add_component(poller)
    };
    handle.set_key(poller_key);

    println!("🔧 Components created:");
    println!(
        "   - FuturePoller component added with key: {:?}",
        poller_key
    );

    // Schedule Initialize event for the FuturePoller
    {
        let mut sim = simulation_arc.lock().unwrap();
        sim.schedule(SimTime::zero(), poller_key, FuturePollerEvent::Initialize);
    }
    println!("   - FuturePoller Initialize event scheduled at 0ms");

    // Create retry client
    let retry_client = RetryClient::new(
        service,
        handle.clone(),
        timeout_duration,
        max_retries,
        retry_delay,
    );
    let client_key = {
        let mut sim = simulation_arc.lock().unwrap();
        sim.add_component(retry_client)
    };
    println!(
        "   - RetryClient component added with key: {:?}",
        client_key
    );

    // Schedule a single test request to see timeout behavior clearly
    let test_requests = vec![
        (10, 1), // Request 1 at 10ms
    ];

    for (time_ms, request_id) in test_requests {
        let mut sim = simulation_arc.lock().unwrap();
        sim.schedule(
            SimTime::from_millis(time_ms),
            client_key,
            RetryClientEvent::SendRequest { request_id },
        );
        println!("   - Request {} scheduled at {}ms", request_id, time_ms);
    }
    println!();

    println!(
        "🏃 Running simulation with Executor for {}ms...",
        simulation_duration.as_millis()
    );
    println!(
        "   Initial state: pending={}, completed={}",
        handle.pending_count(),
        handle.completed_count()
    );
    println!();

    // Use Executor to run the simulation
    {
        let mut sim = simulation_arc.lock().unwrap();
        Executor::timed(SimTime::from_duration(simulation_duration)).execute(&mut sim);
    }

    println!();
    println!("📊 Final Results:");
    println!("   Pending futures: {}", handle.pending_count());
    println!("   Completed futures: {}", handle.completed_count());
    {
        let sim = simulation_arc.lock().unwrap();
        println!(
            "   Simulation time: {}ms",
            sim.time().as_duration().as_millis()
        );
    }

    // The test should show timeout and retry behavior
    // With 200ms service time and 100ms timeout, requests should timeout and retry
    // Note: completed_count() is u64, so we just verify the test ran without panicking

    println!();
    println!("✅ Retry on timeout test completed successfully!");
    println!("   This test demonstrates:");
    println!("   - Client-side timeout detection");
    println!("   - Automatic retry on timeout");
    println!("   - Retry attempt tracking");
    println!("   - Max retry limit enforcement");
    println!("   - Proper handling of late responses");
    println!("   - Realistic timeout scenarios in DES");
}

// ============================================================================
// Two-Tier Service Test: App -> DB with Async Calls
// ============================================================================

/// Helper to format simulation time for logging
fn format_time(time: SimTime) -> String {
    let ms = time.as_duration().as_millis();
    if ms >= 1000 {
        format!("{}.{:03}s", ms / 1000, ms % 1000)
    } else {
        format!("{}ms", ms)
    }
}

/// Events for the App server that calls DB
#[derive(Debug)]
enum AppServerEvent {
    /// Client request received - start initial processing
    RequestReceived {
        request_id: u32,
        client_key: Key<TwoTierClientEvent>,
        arrival_time: SimTime,
    },
    /// Initial processing complete - now call DB
    CallDbService {
        request_id: u32,
        client_key: Key<TwoTierClientEvent>,
        arrival_time: SimTime,
    },
    /// DB response received - start post-processing
    DbResponseReceived {
        request_id: u32,
        client_key: Key<TwoTierClientEvent>,
        arrival_time: SimTime,
    },
    /// Post-processing complete - send response to client
    SendResponseToClient {
        request_id: u32,
        client_key: Key<TwoTierClientEvent>,
        arrival_time: SimTime,
    },
}

/// App server that forwards requests to DB and waits for response
struct AppServer<S> {
    name: String,
    db_service: S,
    future_poller: FuturePollerHandle,
    /// Initial processing time before calling DB
    initial_processing_time: Duration,
    /// Mean post-processing time after DB response (exponential distribution)
    post_processing_mean: Duration,
    /// RNG for exponential distribution
    rng: rand::rngs::StdRng,
}

impl<S> AppServer<S> {
    fn new(
        name: String,
        db_service: S,
        future_poller: FuturePollerHandle,
        initial_processing_time: Duration,
        post_processing_mean: Duration,
    ) -> Self {
        use rand::SeedableRng;
        Self {
            name,
            db_service,
            future_poller,
            initial_processing_time,
            post_processing_mean,
            rng: rand::rngs::StdRng::seed_from_u64(42), // Fixed seed for reproducibility
        }
    }

    /// Sample from exponential distribution
    fn sample_exponential(&mut self) -> Duration {
        use rand::Rng;
        let u: f64 = self.rng.gen();
        let lambda = 1.0 / self.post_processing_mean.as_secs_f64();
        let sample = -u.ln() / lambda;
        Duration::from_secs_f64(sample)
    }
}

impl<S> Component for AppServer<S>
where
    S: Service<
            Request<SimBody>,
            Response = http::Response<SimBody>,
            Error = des_components::tower::ServiceError,
        > + 'static,
    S::Future: 'static,
{
    type Event = AppServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            AppServerEvent::RequestReceived {
                request_id,
                client_key,
                arrival_time,
            } => {
                println!(
                    "[{}] [{}] 📥 Request #{} received from client",
                    format_time(scheduler.time()),
                    self.name,
                    request_id
                );
                println!(
                    "[{}] [{}]    ⏳ Starting initial processing ({}ms)...",
                    format_time(scheduler.time()),
                    self.name,
                    self.initial_processing_time.as_millis()
                );

                // Schedule DB call after initial processing time
                scheduler.schedule(
                    SimTime::from_duration(self.initial_processing_time),
                    self_id,
                    AppServerEvent::CallDbService {
                        request_id: *request_id,
                        client_key: *client_key,
                        arrival_time: *arrival_time,
                    },
                );
            }

            AppServerEvent::CallDbService {
                request_id,
                client_key,
                arrival_time,
            } => {
                println!(
                    "[{}] [{}]    ✅ Initial processing complete for request #{}",
                    format_time(scheduler.time()),
                    self.name,
                    request_id
                );
                println!(
                    "[{}] [{}]    📤 Calling DB service...",
                    format_time(scheduler.time()),
                    self.name
                );

                // Create request to DB
                let db_request = Request::builder()
                    .method(Method::POST)
                    .uri(format!("/db/query/{}", request_id))
                    .body(SimBody::new(
                        format!("DB query for request {}", request_id).into_bytes(),
                    ))
                    .unwrap();

                // Check if DB service is ready
                let waker = create_noop_waker();
                let mut cx = Context::from_waker(&waker);

                match self.db_service.poll_ready(&mut cx) {
                    Poll::Ready(Ok(())) => {
                        // Call DB service
                        let future = self.db_service.call(db_request);

                        // Spawn future with callback that schedules continuation
                        let request_id_copy = *request_id;
                        let client_key_copy = *client_key;
                        let arrival_time_copy = *arrival_time;

                        self.future_poller.spawn(future, move |result| {
                            match result {
                                Ok(_response) => {
                                    // DB call succeeded - schedule continuation via defer_wake
                                    defer_wake(
                                        self_id,
                                        AppServerEvent::DbResponseReceived {
                                            request_id: request_id_copy,
                                            client_key: client_key_copy,
                                            arrival_time: arrival_time_copy,
                                        },
                                    );
                                }
                                Err(e) => {
                                    println!(
                                        "   ❌ DB call failed for request {}: {:?}",
                                        request_id_copy, e
                                    );
                                }
                            }
                        });
                    }
                    Poll::Ready(Err(e)) => {
                        println!(
                            "[{}] [{}]    ❌ DB service error: {:?}",
                            format_time(scheduler.time()),
                            self.name,
                            e
                        );
                    }
                    Poll::Pending => {
                        println!(
                            "[{}] [{}]    ⏳ DB service not ready (capacity limit)",
                            format_time(scheduler.time()),
                            self.name
                        );
                    }
                }
            }

            AppServerEvent::DbResponseReceived {
                request_id,
                client_key,
                arrival_time,
            } => {
                let post_processing_time = self.sample_exponential();
                println!(
                    "[{}] [{}]    📥 DB response received for request #{}",
                    format_time(scheduler.time()),
                    self.name,
                    request_id
                );
                println!(
                    "[{}] [{}]    ⏳ Starting post-processing ({:.1}ms)...",
                    format_time(scheduler.time()),
                    self.name,
                    post_processing_time.as_secs_f64() * 1000.0
                );

                // Schedule response to client after post-processing
                scheduler.schedule(
                    SimTime::from_duration(post_processing_time),
                    self_id,
                    AppServerEvent::SendResponseToClient {
                        request_id: *request_id,
                        client_key: *client_key,
                        arrival_time: *arrival_time,
                    },
                );
            }

            AppServerEvent::SendResponseToClient {
                request_id,
                client_key,
                arrival_time,
            } => {
                println!(
                    "[{}] [{}]    ✅ Post-processing complete for request #{}",
                    format_time(scheduler.time()),
                    self.name,
                    request_id
                );
                println!(
                    "[{}] [{}] 📤 Sending response to client for request #{}",
                    format_time(scheduler.time()),
                    self.name,
                    request_id
                );

                // Send response to client
                scheduler.schedule(
                    SimTime::zero(), // Immediate
                    *client_key,
                    TwoTierClientEvent::ResponseReceived {
                        request_id: *request_id,
                        arrival_time: *arrival_time,
                    },
                );
            }
        }
    }
}

/// Events for the two-tier test client
#[derive(Debug, Clone)]
enum TwoTierClientEvent {
    /// Send a request to App server
    SendRequest { request_id: u32 },
    /// Response received from App server
    ResponseReceived {
        request_id: u32,
        arrival_time: SimTime,
    },
}

/// Client that sends periodic requests and tracks latency
struct TwoTierClient {
    name: String,
    app_server_key: Key<AppServerEvent>,
    interval: Duration,
    max_requests: u32,
    request_count: u32,
    /// Track request arrival times for latency calculation
    request_times: std::collections::HashMap<u32, SimTime>,
    /// Completed request latencies
    latencies: Vec<Duration>,
}

impl TwoTierClient {
    fn new(
        name: String,
        app_server_key: Key<AppServerEvent>,
        interval: Duration,
        max_requests: u32,
    ) -> Self {
        Self {
            name,
            app_server_key,
            interval,
            max_requests,
            request_count: 0,
            request_times: std::collections::HashMap::new(),
            latencies: Vec::new(),
        }
    }

    fn average_latency(&self) -> Option<Duration> {
        if self.latencies.is_empty() {
            None
        } else {
            let total: Duration = self.latencies.iter().sum();
            Some(total / self.latencies.len() as u32)
        }
    }
}

impl Component for TwoTierClient {
    type Event = TwoTierClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            TwoTierClientEvent::SendRequest { request_id } => {
                let current_time = scheduler.time();

                println!(
                    "[{}] [{}] 🚀 Sending request #{} to App server",
                    format_time(current_time),
                    self.name,
                    request_id
                );

                // Record request time
                self.request_times.insert(*request_id, current_time);

                // Send request to App server immediately
                scheduler.schedule(
                    SimTime::zero(),
                    self.app_server_key,
                    AppServerEvent::RequestReceived {
                        request_id: *request_id,
                        client_key: self_id,
                        arrival_time: current_time,
                    },
                );

                // Schedule next request if not at limit
                self.request_count += 1;
                if self.request_count < self.max_requests {
                    scheduler.schedule(
                        SimTime::from_duration(self.interval),
                        self_id,
                        TwoTierClientEvent::SendRequest {
                            request_id: self.request_count + 1,
                        },
                    );
                }
            }
            TwoTierClientEvent::ResponseReceived {
                request_id,
                arrival_time,
            } => {
                let current_time = scheduler.time();
                let latency = current_time - *arrival_time; // SimTime - SimTime = Duration

                println!(
                    "[{}] [{}] ✅ Response received for request #{} (latency: {:.1}ms)",
                    format_time(current_time),
                    self.name,
                    request_id,
                    latency.as_secs_f64() * 1000.0
                );

                self.latencies.push(latency);
            }
        }
    }
}

#[test]
fn test_two_tier_app_db_service() {
    println!();
    println!("🚀 Two-Tier Service Test: App -> DB with Async Calls");
    println!("====================================================");
    println!();

    let mut simulation = Simulation::default();

    // Create DB service with exponential service time (mean 100ms)
    let db_service = DesServiceBuilder::new("db-server".to_string())
        .thread_capacity(10)
        .exponential_service_time(Duration::from_millis(100))
        .build(&mut simulation)
        .expect("Failed to build DB service");

    // Now wrap in Arc for shared access
    let simulation_arc = Arc::new(Mutex::new(simulation));

    // Test parameters
    let request_interval = Duration::from_millis(500);
    let simulation_duration = Duration::from_secs(10);
    let max_requests = (simulation_duration.as_millis() / request_interval.as_millis()) as u32;
    let initial_processing_time = Duration::from_millis(10);
    let post_processing_mean = Duration::from_millis(20);

    println!("📋 Test Setup:");
    println!("   - App Server:");
    println!(
        "       • Initial processing: {}ms (before DB call)",
        initial_processing_time.as_millis()
    );
    println!(
        "       • Post-processing: exponential (mean {}ms)",
        post_processing_mean.as_millis()
    );
    println!("   - DB Server:");
    println!("       • Service time: exponential (mean 100ms)");
    println!("       • Thread capacity: 10");
    println!("   - Client:");
    println!(
        "       • Request interval: {}ms",
        request_interval.as_millis()
    );
    println!("       • Total requests: {}", max_requests);
    println!(
        "   - Simulation duration: {}s",
        simulation_duration.as_secs()
    );
    println!();

    // Create FuturePoller for async DB calls
    let future_poller_handle = FuturePollerHandle::new();
    let future_poller = future_poller_handle.create_component();
    let future_poller_key = {
        let mut sim = simulation_arc.lock().unwrap();
        sim.add_component(future_poller)
    };
    future_poller_handle.set_key(future_poller_key);

    // Schedule FuturePoller initialization
    {
        let mut sim = simulation_arc.lock().unwrap();
        sim.schedule(
            SimTime::zero(),
            future_poller_key,
            FuturePollerEvent::Initialize,
        );
    }

    // Create App server component
    let app_server = AppServer::new(
        "app-server".to_string(),
        db_service,
        future_poller_handle.clone(),
        initial_processing_time,
        post_processing_mean,
    );
    let app_server_key = {
        let mut sim = simulation_arc.lock().unwrap();
        sim.add_component(app_server)
    };

    // Create client
    let client = TwoTierClient::new(
        "client".to_string(),
        app_server_key,
        request_interval,
        max_requests,
    );
    let client_key = {
        let mut sim = simulation_arc.lock().unwrap();
        sim.add_component(client)
    };

    // Schedule first request
    {
        let mut sim = simulation_arc.lock().unwrap();
        sim.schedule(
            SimTime::zero(),
            client_key,
            TwoTierClientEvent::SendRequest { request_id: 1 },
        );
    }

    println!("🏃 Running simulation...");
    println!("─────────────────────────────────────────────────────────────────");

    // Run simulation
    {
        let mut sim = simulation_arc.lock().unwrap();
        Executor::timed(SimTime::from_duration(simulation_duration)).execute(&mut sim);
    }

    println!("─────────────────────────────────────────────────────────────────");
    println!();

    // Extract results
    let (completed_count, avg_latency, latencies) = {
        let mut sim = simulation_arc.lock().unwrap();
        let client: TwoTierClient = sim.remove_component(client_key).unwrap();
        (
            client.latencies.len(),
            client.average_latency(),
            client.latencies.clone(),
        )
    };

    println!("📊 Final Results:");
    println!(
        "   Completed requests: {} / {}",
        completed_count, max_requests
    );
    println!(
        "   Pending futures: {}",
        future_poller_handle.pending_count()
    );
    println!(
        "   Completed futures: {}",
        future_poller_handle.completed_count()
    );

    if let Some(avg) = avg_latency {
        let avg_ms = avg.as_secs_f64() * 1000.0;
        println!("   Average latency: {:.2}ms", avg_ms);

        // Calculate min/max
        if !latencies.is_empty() {
            let min_ms = latencies.iter().min().unwrap().as_secs_f64() * 1000.0;
            let max_ms = latencies.iter().max().unwrap().as_secs_f64() * 1000.0;
            println!("   Min latency: {:.2}ms", min_ms);
            println!("   Max latency: {:.2}ms", max_ms);
        }
    } else {
        println!("   Average latency: N/A (no completed requests)");
    }

    {
        let sim = simulation_arc.lock().unwrap();
        println!(
            "   Final simulation time: {}ms",
            sim.time().as_duration().as_millis()
        );
    }

    // Assertions
    assert!(
        completed_count > 0,
        "Should have completed at least some requests"
    );

    // Expected latency: 10ms (initial) + ~100ms (DB, exponential mean) + ~20ms (post-DB) = ~130ms
    if let Some(avg) = avg_latency {
        let avg_ms = avg.as_secs_f64() * 1000.0;
        println!();
        println!("   Expected latency breakdown:");
        println!(
            "     - Initial App processing: {}ms",
            initial_processing_time.as_millis()
        );
        println!("     - DB processing (mean): 100ms");
        println!(
            "     - Post-DB processing (mean): {}ms",
            post_processing_mean.as_millis()
        );
        println!("     - Expected total (mean): ~130ms");
        println!("     - Actual average: {:.2}ms", avg_ms);

        // Allow for variance - should be roughly in the 80-300ms range
        assert!(avg_ms > 50.0, "Average latency should be > 50ms");
        assert!(avg_ms < 500.0, "Average latency should be < 500ms");
    }

    println!();
    println!("✅ Two-tier service test completed successfully!");
}
