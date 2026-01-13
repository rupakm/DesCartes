//! Simple test to verify Tower service integration with DES.

use des_core::{Execute, Executor, SimTime, Simulation};
use des_tower::{DesServiceBuilder, SimBody};
use http::{Method, Request};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use tower::ServiceExt;

#[test]
fn tower_service_oneshot_returns_success() {
    let mut simulation = Simulation::default();
    des_tokio::runtime::install(&mut simulation);

    let service = DesServiceBuilder::new("test-server".to_string())
        .thread_capacity(1)
        .service_time(Duration::from_millis(10))
        .build(&mut simulation)
        .expect("build service");

    let done: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let done_clone = done.clone();

    des_tokio::task::spawn_local(async move {
        let request = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .body(SimBody::from_static("test"))
            .unwrap();

        let response = service.oneshot(request).await.expect("request succeeds");
        assert!(response.status().is_success());
        *done_clone.borrow_mut() = true;
    });

    Executor::timed(SimTime::from_duration(Duration::from_millis(100))).execute(&mut simulation);

    assert!(*done.borrow());
}
