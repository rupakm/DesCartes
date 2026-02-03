use descartes_core::{Execute, Executor, SimTime, Simulation};
use descartes_tower::{DesServiceBuilder, SimBody};
use http::{Method, Request};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use tower::ServiceExt;

#[test]
fn tower_service_oneshot_completes_without_deadlock() {
    let mut simulation = Simulation::default();
    descartes_tokio::runtime::install(&mut simulation);

    let service = DesServiceBuilder::new("simple-server".to_string())
        .thread_capacity(1)
        .service_time(Duration::from_millis(10))
        .build(&mut simulation)
        .expect("build service");

    let done: Rc<RefCell<Option<http::Response<SimBody>>>> = Rc::new(RefCell::new(None));
    let done_clone = done.clone();

    descartes_tokio::task::spawn_local(async move {
        descartes_tokio::time::sleep(Duration::from_millis(5)).await;

        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/test")
            .body(SimBody::from_static("Test request body"))
            .unwrap();

        let resp = service.oneshot(req).await.expect("request succeeds");
        *done_clone.borrow_mut() = Some(resp);
    });

    Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut simulation);

    let resp = done.borrow_mut().take().expect("response delivered");
    assert!(resp.status().is_success());
}
