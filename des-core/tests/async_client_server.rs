//! Demonstrates deterministic async client/server interaction in DES.
//!
//! Note: this test intentionally uses a tiny custom response future (instead of
//! `descartes_tokio::sync::oneshot`) to keep the mechanics explicit.
//!
//! For a comparable Tokio example using oneshot channels, see:
//! - Tokio docs: https://docs.rs/tokio/latest/tokio/sync/oneshot/index.html
//! - Tokio source (oneshot implementation/tests): https://github.com/tokio-rs/tokio/blob/master/tokio/src/sync/oneshot.rs
//!
//! For the DES equivalent oneshot tests, see:
//! - `des-tokio/tests/oneshot.rs`
//!
//! ## What this test demonstrates
//! In a discrete-event simulation, **nothing blocks the OS thread**.
//! Instead, an async task runs until it hits an `.await` on a Future that
//! is not ready, at which point the Future returns `Poll::Pending`.
//!
//! When a Future returns `Poll::Pending`, it stores the current task's `Waker`.
//! The simulation continues stepping and processing other events. Later, when
//! the awaited condition becomes true (e.g. a server response arrives at a
//! future simulated time), the producer triggers the stored `Waker`, which
//! schedules a wake event for the runtime. The runtime then polls the task
//! again and the `.await` completes.
//!
//! ### Timeline in this test
//! - **t=0**: client task sends request to server and awaits response → `Pending`
//! - **t=0**: server receives request and schedules response for **t=1s**
//! - **t=1s**: server sends response and calls the stored waker
//! - **t=1s**: runtime polls the client task again, and it completes

use descartes_core::async_runtime::{current_sim_time, sim_sleep_until, DesRuntime, RuntimeEvent};
use descartes_core::{defer_wake, Component, Execute, Executor, Key, Scheduler, SimTime, Simulation};
use rand::SeedableRng;
use rand_distr::Distribution;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

#[derive(Debug, Clone)]
enum ServerEvent {
    Request { response_state: ResponseStateHandle },
    SendResponse { response_state: ResponseStateHandle },
}

#[derive(Debug)]
struct Server {
    request_count: usize,
    request_times: Arc<Mutex<Vec<SimTime>>>,
}

impl Server {
    fn new(request_times: Arc<Mutex<Vec<SimTime>>>) -> Self {
        Self {
            request_count: 0,
            request_times,
        }
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new(Arc::new(Mutex::new(Vec::new())))
    }
}

impl Component for Server {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ServerEvent::Request { response_state } => {
                self.request_count += 1;
                self.request_times.lock().unwrap().push(scheduler.time());

                // Simulate server work taking 1 second.
                scheduler.schedule(
                    SimTime::from_duration(Duration::from_secs(1)),
                    self_id,
                    ServerEvent::SendResponse {
                        response_state: response_state.clone(),
                    },
                );
            }
            ServerEvent::SendResponse { response_state } => {
                response_state.complete("ok".to_string());
            }
        }
    }
}

#[derive(Debug, Default)]
struct ResponseState {
    response: Option<String>,
    waker: Option<Waker>,
}

#[derive(Clone, Debug)]
struct ResponseStateHandle {
    inner: Arc<Mutex<ResponseState>>,
}

impl ResponseStateHandle {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ResponseState::default())),
        }
    }

    fn future(&self) -> ResponseFuture {
        ResponseFuture {
            inner: self.inner.clone(),
        }
    }

    fn complete(&self, response: String) {
        let waker_to_wake = {
            let mut locked = self.inner.lock().unwrap();
            locked.response = Some(response);
            locked.waker.take()
        };

        // Wake outside of the mutex.
        if let Some(waker) = waker_to_wake {
            waker.wake();
        }
    }
}

struct ResponseFuture {
    inner: Arc<Mutex<ResponseState>>,
}

impl Future for ResponseFuture {
    type Output = String;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut locked = self.inner.lock().unwrap();

        if let Some(response) = locked.response.take() {
            Poll::Ready(response)
        } else {
            locked.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

async fn unary_call(server_key: Key<ServerEvent>, response_state: ResponseStateHandle) -> String {
    // Send request at t=0.
    //
    // Important: scheduling from inside an async task runs during event processing.
    // Using `SchedulerHandle` would attempt to lock the scheduler mutex again and can deadlock.
    // Use `defer_wake(...)` instead, which schedules at the current sim time without locking.
    defer_wake(
        server_key,
        ServerEvent::Request {
            response_state: response_state.clone(),
        },
    );

    // Await the response future. This returns Pending until the server responds.
    response_state.future().await
}

async fn client_task(
    server_key: Key<ServerEvent>,
    response_state: ResponseStateHandle,
    completed: Arc<Mutex<bool>>,
) {
    let response = unary_call(server_key, response_state).await;
    assert_eq!(response, "ok");

    *completed.lock().unwrap() = true;
}

#[test]
fn async_client_awaits_server_response_without_blocking_simulation() {
    let mut sim = Simulation::default();

    // Add server component.
    let request_times = Arc::new(Mutex::new(Vec::new()));
    let server_key = sim.add_component(Server::new(request_times.clone()));

    // Add runtime component.
    let mut runtime = DesRuntime::new();

    let completed = Arc::new(Mutex::new(false));
    let completed_clone = completed.clone();

    let response_state = ResponseStateHandle::new();

    // Spawn the client task using explicit `async fn` and `.await`.
    runtime.spawn(async move {
        client_task(server_key, response_state, completed_clone).await;
    });

    let runtime_key = sim.add_component(runtime);
    sim.schedule(SimTime::zero(), runtime_key, RuntimeEvent::Poll);

    // Run long enough to cover the 1s server delay.
    Executor::timed(SimTime::from_duration(Duration::from_secs(2))).execute(&mut sim);

    assert!(*completed.lock().unwrap());

    // Sanity: server should have seen exactly one request.
    let server = sim
        .get_component_mut::<ServerEvent, Server>(server_key)
        .unwrap();
    assert_eq!(server.request_count, 1);

    // And it should have been received at t=0.
    let times = request_times.lock().unwrap().clone();
    assert_eq!(times.len(), 1);
    assert_eq!(times[0], SimTime::zero());
}

async fn request_task_at(
    arrival: SimTime,
    server_key: Key<ServerEvent>,
    completed_times: Arc<Mutex<Vec<SimTime>>>,
) {
    // Wait until the request's arrival time.
    sim_sleep_until(arrival).await;

    // Send request (non-locking) and await response.
    let response_state = ResponseStateHandle::new();
    defer_wake(
        server_key,
        ServerEvent::Request {
            response_state: response_state.clone(),
        },
    );

    let response = response_state.future().await;
    assert_eq!(response, "ok");

    // Record completion time (should be arrival + 1s in this model).
    let now = current_sim_time().expect("must be in scheduler context while polling");
    completed_times.lock().unwrap().push(now);
}

#[test]
fn open_loop_client_exponential_arrivals_do_not_block_subsequent_requests() {
    // This test demonstrates an *open-loop* client where requests arrive according
    // to an exponential inter-arrival time distribution. The key property is that
    // a slow request (server responds 1s later) does not block later arrivals.
    //
    // We model this by spawning one async task per request, each of which sleeps
    // until its arrival time and then awaits the response. Because each request
    // has its own task, awaiting a response does not prevent other requests from
    // being issued at their scheduled arrival times.

    let mut sim = Simulation::default();

    let request_times = Arc::new(Mutex::new(Vec::new()));
    let server_key = sim.add_component(Server::new(request_times.clone()));

    let mut runtime = DesRuntime::new();

    // Deterministic RNG seed for reproducibility.
    let mut rng = rand::rngs::StdRng::seed_from_u64(12345);
    let exp = rand_distr::Exp::new(10.0).expect("rate must be positive"); // mean 100ms

    let request_count = 30;

    // Generate cumulative arrival times (first request at t=0).
    let mut arrivals = Vec::with_capacity(request_count);
    let mut t = SimTime::zero();
    arrivals.push(t);

    for _ in 1..request_count {
        let interarrival_secs: f64 = exp.sample(&mut rng);
        let interarrival = Duration::from_secs_f64(interarrival_secs);
        t = t + interarrival;
        arrivals.push(t);
    }

    let completed_times: Arc<Mutex<Vec<SimTime>>> = Arc::new(Mutex::new(Vec::new()));

    for arrival in arrivals.iter().copied() {
        let completed_times = completed_times.clone();
        runtime.spawn(async move {
            request_task_at(arrival, server_key, completed_times).await;
        });
    }

    let runtime_key = sim.add_component(runtime);
    sim.schedule(SimTime::zero(), runtime_key, RuntimeEvent::Poll);

    // Run long enough for all requests and responses.
    Executor::timed(SimTime::from_duration(Duration::from_secs(20))).execute(&mut sim);

    let observed_request_times = request_times.lock().unwrap().clone();
    assert_eq!(observed_request_times.len(), request_count);

    // Key property: multiple requests should arrive before the first response
    // could possibly return (server responds after 1s).
    let one_second = SimTime::from_duration(Duration::from_secs(1));

    assert_eq!(observed_request_times[0], SimTime::zero());
    assert!(
        observed_request_times.len() >= 2,
        "expected at least 2 requests"
    );
    assert!(
        observed_request_times[1] < one_second,
        "expected 2nd request arrival < 1s, got {}",
        observed_request_times[1]
    );

    // All requests should complete.
    let completed = completed_times.lock().unwrap().clone();
    assert_eq!(completed.len(), request_count);

    // Since the first arrival is at t=0 and server response time is 1s,
    // the earliest completion cannot be before t=1s.
    let earliest_completion = completed
        .iter()
        .copied()
        .min()
        .expect("at least one completion");
    assert!(
        earliest_completion >= one_second,
        "expected earliest completion >= 1s, got {}",
        earliest_completion
    );

    let requests_before_first_completion = observed_request_times
        .iter()
        .filter(|&&rt| rt < earliest_completion)
        .count();
    assert!(
        requests_before_first_completion >= 2,
        "expected at least 2 arrivals before first completion, got {requests_before_first_completion}"
    );

    // Sanity: server processed all requests.
    let server = sim
        .get_component_mut::<ServerEvent, Server>(server_key)
        .unwrap();
    assert_eq!(server.request_count, request_count);
}
