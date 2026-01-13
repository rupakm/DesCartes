use bytes::Bytes;
use des_components::transport::{
    LatencyConfig, LatencyJitterModel, SharedEndpointRegistry, SimTransport,
};
use des_core::{Execute, Executor, SimTime, Simulation};
use des_tonic::{ClientBuilder, Router, ServerBuilder};
use std::time::Duration;
use tonic::{Request, Response};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sim = Simulation::default();

    // Install deterministic async runtime (required by des-tonic).
    des_tokio::runtime::install(&mut sim);

    // Network + transport.
    let network = Box::new(LatencyJitterModel::with_seed(
        LatencyConfig::new(Duration::from_millis(10)),
        42,
    ));
    let transport_key = sim.add_component(SimTransport::new(network));

    let endpoint_registry = SharedEndpointRegistry::new();

    // Server: register unary handler.
    let mut router = Router::new();
    router.add_unary("/echo.EchoService/Echo", |req: Request<Bytes>| async move {
        Ok(Response::new(req.into_inner()))
    });

    let _server = ServerBuilder::new(
        "echo.EchoService".to_string(),
        "server-1".to_string(),
        transport_key,
        endpoint_registry.clone(),
        sim.scheduler_handle(),
    )
    .name("echo-server")
    .processing_delay(Duration::from_millis(5))
    .add_router(router)
    .install(&mut sim)?;

    let client = ClientBuilder::new(
        "echo.EchoService".to_string(),
        transport_key,
        endpoint_registry,
        sim.scheduler_handle(),
    )
    .client_name("client-1")
    .install(&mut sim)?;

    let channel = client.channel;

    // Spawn a unary call from the async runtime.
    des_tokio::task::spawn_local(async move {
        let req = Request::new(Bytes::from_static(b"hello"));
        let resp = channel
            .unary("/echo.EchoService/Echo", req, Some(Duration::from_secs(1)))
            .await;
        match resp {
            Ok(r) => {
                assert_eq!(r.into_inner(), Bytes::from_static(b"hello"));
            }
            Err(e) => panic!("rpc failed: {e}"),
        }
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);

    Ok(())
}
