use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use des_core::{Execute, Executor, SimTime, Simulation, SimulationConfig};
use des_tonic::{Request, Response, Router, Transport};

#[test]
fn socket_addr_api_connects_and_rejects_duplicate_bind() {
    let mut sim = Simulation::new(SimulationConfig { seed: 1 });
    des_tokio::runtime::install(&mut sim);

    let transport = Transport::install_default(&mut sim);

    let mut router = Router::new();
    router.add_unary("/svc.Echo/Echo", |_req: Request<Bytes>| async move {
        Ok(Response::new(Bytes::from_static(b"ok")))
    });

    let addr: SocketAddr = "127.0.0.1:50051".parse().unwrap();

    let _server = transport
        .serve_socket_addr(&mut sim, "svc.Echo", addr, router.clone())
        .expect("serve");

    let dup = transport.serve(&mut sim, "svc.Echo", "http://127.0.0.1:50051", router);
    assert!(matches!(dup, Err(s) if s.code() == tonic::Code::AlreadyExists));

    let channel = transport
        .connect_socket_addr(&mut sim, "svc.Echo", addr)
        .expect("connect");

    let out: std::sync::Arc<std::sync::Mutex<Option<Vec<u8>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let out2 = out.clone();

    des_tokio::task::spawn_local(async move {
        let resp = channel
            .unary(
                "/svc.Echo/Echo",
                Request::new(Bytes::from_static(b"hi")),
                Some(Duration::from_millis(10)),
            )
            .await
            .expect("rpc");

        *out2.lock().unwrap() = Some(resp.into_inner().to_vec());
    });

    Executor::timed(SimTime::from_millis(50)).execute(&mut sim);

    assert_eq!(*out.lock().unwrap(), Some(b"ok".to_vec()));
}
