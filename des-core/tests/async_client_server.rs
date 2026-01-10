//! Demonstrates deterministic async client/server interaction in DES.
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

use des_core::async_runtime::{DesRuntime, RuntimeEvent};
use des_core::{defer_wake, Component, Execute, Executor, Key, Scheduler, SimTime, Simulation};
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

#[derive(Debug, Default)]
struct Server {
    request_count: usize,
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
    let server_key = sim.add_component(Server::default());

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
}
