use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use des_components::transport::{
    LatencyConfig, LatencyJitterModel, NetworkModel, SimpleNetworkModel,
};
use des_core::async_runtime::current_sim_time;
use des_core::{Execute, Executor, SimTime, Simulation, SimulationConfig};
use des_tonic::{Request, Response, Router, Transport};

fn run_client_server_with_transport(
    cfg: SimulationConfig,
    model: Box<dyn NetworkModel>,
) -> Vec<Duration> {
    let mut sim = Simulation::new(cfg);
    des_tokio::runtime::install(&mut sim);

    let transport = Transport::install(&mut sim, model);

    let mut router = Router::new();
    router.add_unary("/svc.Echo/Echo", |_req: Request<Bytes>| async move {
        Ok(Response::new(Bytes::from_static(b"ok")))
    });

    let addr: SocketAddr = "127.0.0.1:50052".parse().unwrap();
    transport
        .serve(&mut sim, "svc.Echo", addr.to_string(), router)
        .expect("serve");

    let channel = des_tonic::Channel::connect(&mut sim, &transport, "svc.Echo", addr.to_string())
        .expect("connect");

    let latencies: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let lat2 = latencies.clone();

    des_tokio::task::spawn_local(async move {
        for _ in 0..20 {
            let start = current_sim_time().unwrap();
            let _resp = channel
                .unary(
                    "/svc.Echo/Echo",
                    Request::new(Bytes::from_static(b"hi")),
                    Some(Duration::from_secs(1)),
                )
                .await
                .expect("rpc");
            let end = current_sim_time().unwrap();
            lat2.lock().unwrap().push(end - start);
            des_tokio::thread::yield_now().await;
        }
    });

    Executor::timed(SimTime::from_millis(500)).execute(&mut sim);

    let mut out = latencies.lock().unwrap().clone();
    out.sort();
    out
}

fn avg(d: &[Duration]) -> Duration {
    let sum: Duration = d.iter().copied().sum();
    sum / (d.len() as u32)
}

#[test]
fn client_server_can_swap_network_models_without_code_changes() {
    let cfg = SimulationConfig { seed: 7 };

    // Zero-latency baseline.
    let base_seed = cfg.seed ^ 0x1111_2222_3333_4444u64;
    let fast = SimpleNetworkModel::with_seed(Duration::ZERO, 0.0, base_seed);
    let cfg_fast = cfg.clone();
    let fast_lat =
        std::thread::spawn(move || run_client_server_with_transport(cfg_fast, Box::new(fast)))
            .join()
            .unwrap();

    let fast_avg = avg(&fast_lat);

    // Latency + jitter.
    let jitter_cfg = LatencyConfig::new(Duration::from_millis(10)).with_jitter(0.5);
    let jitter = LatencyJitterModel::with_seed(jitter_cfg, cfg.seed ^ 0xABCDEu64);
    let jitter_lat =
        std::thread::spawn(move || run_client_server_with_transport(cfg, Box::new(jitter)))
            .join()
            .unwrap();
    let jitter_avg = avg(&jitter_lat);

    eprintln!("fast_avg={fast_avg:?}, jitter_avg={jitter_avg:?}");

    assert!(
        jitter_avg > fast_avg,
        "expected jitter model to increase average latency"
    );

    // Also sanity-check there is some spread (jitter).
    let jitter_min = jitter_lat.first().copied().unwrap();
    let jitter_max = jitter_lat.last().copied().unwrap();
    assert!(
        jitter_max > jitter_min,
        "expected non-zero jitter spread (min={jitter_min:?}, max={jitter_max:?})"
    );
}
