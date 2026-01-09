//! Determinism guardrail tests for Tower integration
//!
//! These tests run an identical Tower-backed simulation multiple times and assert
//! identical outcomes. The intent is to catch accidental introduction of
//! non-determinism (ordering/wake behavior) without depending on any particular
//! ordering policy.

use des_components::tower::{DesServiceBuilder, FuturePollerEvent, FuturePollerHandle, SimBody};
use des_core::{Execute, Executor, SimTime, Simulation};
use http::{Method, Request};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::Duration;
use tower::Service;

fn noop_waker() -> Waker {
    fn noop(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);

    let raw = RawWaker::new(std::ptr::null(), &VTABLE);
    unsafe { Waker::from_raw(raw) }
}

fn run_tower_simulation() -> Vec<String> {
    let mut simulation = Simulation::default();

    let service = DesServiceBuilder::new("determinism-server".to_string())
        .thread_capacity(1)
        .service_time(Duration::from_millis(10))
        .build(&mut simulation)
        .expect("Failed to build service");

    let service: Arc<Mutex<_>> = Arc::new(Mutex::new(service));

    let responses: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // FuturePoller to drive response futures.
    let poller_handle = FuturePollerHandle::new();
    let poller = poller_handle.create_component();
    let poller_key = simulation.add_component(poller);
    poller_handle.set_key(poller_key);

    simulation.schedule(SimTime::zero(), poller_key, FuturePollerEvent::Initialize);

    // Schedule a small fixed workload with no randomness and no overlapping completions.
    // Requests at 0ms, 20ms, 40ms, 60ms, 80ms; service time is 10ms.
    for i in 0..5u8 {
        let service = service.clone();
        let poller_handle = poller_handle.clone();
        let responses = responses.clone();

        simulation.schedule_closure(SimTime::from_millis((i as u64) * 20), move |_scheduler| {
            let request = Request::builder()
                .method(Method::POST)
                .uri("/determinism")
                .body(SimBody::new(format!("req-{i}").into_bytes()))
                .unwrap();

            let waker = noop_waker();
            let mut cx = Context::from_waker(&waker);

            let mut locked = service.lock().unwrap();
            match locked.poll_ready(&mut cx) {
                Poll::Ready(Ok(())) => {
                    let fut = locked.call(request);
                    poller_handle.spawn(fut, move |result| {
                        let response = result.expect("service call failed");
                        let body = String::from_utf8_lossy(response.body().data()).to_string();
                        responses.lock().unwrap().push(body);
                    });
                }
                Poll::Ready(Err(e)) => panic!("Service returned error from poll_ready: {e:?}"),
                Poll::Pending => panic!("Service not ready for deterministic workload"),
            }
        });
    }

    Executor::timed(SimTime::from_millis(200)).execute(&mut simulation);

    let result = responses.lock().unwrap().clone();
    assert_eq!(result.len(), 5);
    result
}

#[test]
fn deterministic_tower_workload_across_runs() {
    let baseline = run_tower_simulation();

    for _ in 0..20 {
        let next = run_tower_simulation();
        assert_eq!(baseline, next);
    }
}
