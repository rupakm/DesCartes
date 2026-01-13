//! Determinism guardrail tests for Tower integration.
//!
//! These tests run an identical Tower-backed simulation multiple times and assert
//! identical outcomes. The intent is to catch accidental introduction of
//! non-determinism (ordering/wake behavior) without depending on any particular
//! ordering policy.

use des_core::{Execute, Executor, SimTime, Simulation};
use des_tower::{DesServiceBuilder, SimBody};
use http::{Method, Request};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use tower::ServiceExt;

fn run_tower_simulation() -> Vec<String> {
    // des_tokio installs into TLS and can only be installed once per thread.
    // Run each simulation in a fresh thread to allow multiple runs in one test.
    std::thread::spawn(|| {
        let mut simulation = Simulation::default();
        des_tokio::runtime::install(&mut simulation);

        let service = DesServiceBuilder::new("determinism-server".to_string())
            .thread_capacity(1)
            .service_time(Duration::from_millis(10))
            .build(&mut simulation)
            .expect("build service");

        let responses: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

        // Schedule a small fixed workload with no randomness and no overlapping completions.
        // Requests at 0ms, 20ms, 40ms, 60ms, 80ms; service time is 10ms.
        for i in 0..5u8 {
            let service = service.clone();
            let responses = responses.clone();

            simulation.schedule_closure(SimTime::from_millis((i as u64) * 20), move |_scheduler| {
                let request = Request::builder()
                    .method(Method::POST)
                    .uri("/determinism")
                    .body(SimBody::new(format!("req-{i}").into_bytes()))
                    .unwrap();

                des_tokio::task::spawn_local(async move {
                    let response = service.oneshot(request).await.expect("service call failed");
                    let body = String::from_utf8_lossy(response.body().data()).to_string();
                    responses.borrow_mut().push(body);
                });
            });
        }

        Executor::timed(SimTime::from_millis(200)).execute(&mut simulation);

        let result = responses.borrow().clone();
        assert_eq!(result.len(), 5);
        result
    })
    .join()
    .expect("simulation thread panicked")
}

#[test]
fn deterministic_tower_workload_across_runs() {
    let baseline = run_tower_simulation();

    for _ in 0..20 {
        let next = run_tower_simulation();
        assert_eq!(baseline, next);
    }
}
