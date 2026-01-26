use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use des_core::{Execute, Executor, SimTime, Simulation, SimulationConfig};
use des_tonic::{Request, Response, Router, Transport};
use tonic::Status;

#[derive(Clone, PartialEq, prost::Message)]
struct EchoRequest {
    #[prost(uint32, tag = "1")]
    v: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
struct EchoResponse {
    #[prost(uint32, tag = "1")]
    v: u32,
}

struct Svc;

async fn unary_echo(
    _svc: Arc<Svc>,
    req: Request<EchoRequest>,
) -> Result<Response<EchoResponse>, Status> {
    let v = req.into_inner().v;
    Ok(Response::new(EchoResponse { v: v + 1 }))
}

#[test]
fn unary_prost_roundtrip_encodes_and_decodes() {
    let mut sim = Simulation::new(SimulationConfig { seed: 1 });
    des_tokio::runtime::install(&mut sim);

    let transport = Transport::install_default(&mut sim);

    let mut router = Router::new();
    router.add_unary_prost("/svc.Echo/Unary", Arc::new(Svc), unary_echo);

    let addr: SocketAddr = "127.0.0.1:50080".parse().unwrap();
    let _server = transport
        .serve_socket_addr(&mut sim, "svc.Echo", addr, router)
        .expect("serve");

    let channel = transport
        .connect_socket_addr(&mut sim, "svc.Echo", addr)
        .expect("connect");

    let out: Arc<std::sync::Mutex<Option<u32>>> = Arc::new(std::sync::Mutex::new(None));
    let out2 = out.clone();

    des_tokio::task::spawn_local(async move {
        let resp: Response<EchoResponse> = channel
            .unary_prost::<EchoRequest, EchoResponse>(
                "/svc.Echo/Unary",
                Request::new(EchoRequest { v: 41 }),
                Some(Duration::from_millis(10)),
            )
            .await
            .expect("rpc");
        *out2.lock().unwrap() = Some(resp.into_inner().v);
    });

    Executor::timed(SimTime::from_millis(50)).execute(&mut sim);

    assert_eq!(*out.lock().unwrap(), Some(42));
}
