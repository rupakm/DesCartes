//! Tests for request-dependent service time distributions.

use descartes_core::dists::{ConstantServiceTime, EndpointBasedServiceTime, RequestSizeBasedServiceTime};
use descartes_core::{scheduler, Execute, Executor, SimTime, Simulation};
use descartes_tower::{DesServiceBuilder, SimBody};
use http::{Method, Request};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use tower::ServiceExt;

#[test]
fn request_size_based_service_time_is_applied() {
    let mut simulation = Simulation::default();
    descartes_tokio::runtime::install(&mut simulation);

    // Base time: 10ms, 10μs per byte, max 100ms.
    let size_based_dist = RequestSizeBasedServiceTime::new(
        Duration::from_millis(10),    // base time
        Duration::from_nanos(10_000), // 10μs per byte
        Duration::from_millis(100),   // max time
    );

    let service = DesServiceBuilder::new("size-based-server".to_string())
        .thread_capacity(3)
        .service_time_distribution(size_based_dist)
        .build(&mut simulation)
        .expect("build service");

    let completion_times: Rc<RefCell<Vec<Option<SimTime>>>> = Rc::new(RefCell::new(vec![None; 3]));

    let cases: Vec<(usize, Vec<u8>, Duration)> = vec![
        (0, vec![], Duration::from_millis(10)),
        (1, vec![b'x'; 1000], Duration::from_millis(20)),
        (2, vec![b'y'; 10_000], Duration::from_millis(100)),
    ];

    for (index, body, _expected) in cases.iter().cloned() {
        let service = service.clone();
        let completion_times = completion_times.clone();

        simulation.schedule_closure(SimTime::zero(), move |_scheduler| {
            let request = Request::builder()
                .method(Method::POST)
                .uri("/api/test")
                .body(SimBody::new(body))
                .unwrap();

            descartes_tokio::task::spawn_local(async move {
                let _response = service.oneshot(request).await.expect("request succeeds");
                let now = scheduler::current_time().expect("in scheduler context");
                completion_times.borrow_mut()[index] = Some(now);
            });
        });
    }

    Executor::timed(SimTime::from_duration(Duration::from_millis(250))).execute(&mut simulation);

    let times = completion_times.borrow();

    for (index, _body, expected) in cases {
        let time = times[index].expect("request completed");
        let observed = time.as_duration();

        // descartes_tokio wakes the awaiting task via scheduled events, which can
        // introduce a small deterministic overhead versus the raw service time.
        assert!(observed >= expected);
        assert!(observed <= expected + Duration::from_millis(5));
    }

    let t0 = times[0].unwrap().as_duration();
    let t1 = times[1].unwrap().as_duration();
    let t2 = times[2].unwrap().as_duration();
    assert!(t0 < t1);
    assert!(t1 < t2);
}

#[test]
fn endpoint_based_service_time_is_applied() {
    let mut simulation = Simulation::default();
    descartes_tokio::runtime::install(&mut simulation);

    let mut endpoint_dist = EndpointBasedServiceTime::new(Box::new(ConstantServiceTime::new(
        Duration::from_millis(50),
    )));

    endpoint_dist.add_endpoint(
        "/api/fast".to_string(),
        Box::new(ConstantServiceTime::new(Duration::from_millis(10))),
    );

    endpoint_dist.add_endpoint(
        "/api/slow".to_string(),
        Box::new(ConstantServiceTime::new(Duration::from_millis(30))),
    );

    let service = DesServiceBuilder::new("endpoint-server".to_string())
        .thread_capacity(3)
        .service_time_distribution(endpoint_dist)
        .build(&mut simulation)
        .expect("build service");

    let completion_times: Rc<RefCell<Vec<Option<SimTime>>>> = Rc::new(RefCell::new(vec![None; 3]));

    let cases: Vec<(usize, &'static str, Duration)> = vec![
        (0, "/api/fast", Duration::from_millis(10)),
        (1, "/api/slow", Duration::from_millis(30)),
        (2, "/api/unknown", Duration::from_millis(50)),
    ];

    for (index, uri, _expected) in cases.iter().cloned() {
        let service = service.clone();
        let completion_times = completion_times.clone();

        simulation.schedule_closure(SimTime::zero(), move |_scheduler| {
            let request = Request::builder()
                .method(Method::GET)
                .uri(uri)
                .body(SimBody::empty())
                .unwrap();

            descartes_tokio::task::spawn_local(async move {
                let _response = service.oneshot(request).await.expect("request succeeds");
                let now = scheduler::current_time().expect("in scheduler context");
                completion_times.borrow_mut()[index] = Some(now);
            });
        });
    }

    Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut simulation);

    let times = completion_times.borrow();

    for (index, _uri, expected) in cases {
        let time = times[index].expect("request completed");
        let observed = time.as_duration();
        assert!(observed >= expected);
        assert!(observed <= expected + Duration::from_millis(5));
    }

    let fast = times[0].unwrap().as_duration();
    let slow = times[1].unwrap().as_duration();
    let unknown = times[2].unwrap().as_duration();
    assert!(fast < slow);
    assert!(slow < unknown);
}
