use des_core::{Execute, Executor, SimTime, Simulation};
use des_metrics::{with_simulation_metrics_recorder, SimulationMetrics};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn integrates_with_des_tokio_tasks() {
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));

    with_simulation_metrics_recorder(&metrics, || {
        let mut sim = Simulation::default();
        des_tokio::runtime::install(&mut sim);

        des_tokio::task::spawn_local(async {
            for _ in 0..3u32 {
                metrics::counter!("ticks_total").increment(1);
                des_tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });

        Executor::timed(SimTime::from_millis(100)).execute(&mut sim);
    });

    assert_eq!(
        metrics.lock().unwrap().get_counter_simple("ticks_total"),
        Some(3)
    );
}

#[test]
fn integrates_with_des_axum_request_flow() {
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));

    with_simulation_metrics_recorder(&metrics, || {
        let mut sim = Simulation::default();
        des_tokio::runtime::install(&mut sim);

        let transport = des_axum::Transport::install_default(&mut sim);

        let app = axum::Router::new().route(
            "/hello",
            axum::routing::get(|| async move { "hello" }),
        );

        transport
            .serve_named(&mut sim, "hello", "hello-1", app)
            .expect("install server");

        let client = transport.connect(&mut sim, "hello").expect("install client");

        des_tokio::task::spawn_local(async move {
            for _ in 0..5u32 {
                let start = des_tokio::time::Instant::now();
                let resp = client
                    .get("/hello", Some(Duration::from_secs(1)))
                    .await
                    .expect("http response");
                let elapsed = des_tokio::time::Instant::now().duration_since(start);

                let status = resp.status().as_str().to_string();

                metrics::counter!(
                    "http_requests_total",
                    "service" => "hello",
                    "status" => status
                )
                .increment(1);

                metrics::histogram!("http_client_latency_ms", "service" => "hello")
                    .record(elapsed.as_secs_f64() * 1000.0);
            }
        });

        Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    });

    let locked = metrics.lock().unwrap();
    assert_eq!(
        locked.get_counter(
            "http_requests_total",
            &[("service", "hello"), ("status", "200")]
        ),
        Some(5)
    );

    let hist = locked.get_histogram_stats("http_client_latency_ms", &[("service", "hello")]);
    assert!(hist.is_some());
    assert_eq!(hist.unwrap().count, 5);
}

#[test]
fn integrates_with_des_tonic_unary_flow() {
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));

    with_simulation_metrics_recorder(&metrics, || {
        let mut sim = Simulation::default();
        des_tokio::runtime::install(&mut sim);

        let transport = des_tonic::Transport::install_default(&mut sim);
        let service_name = "echo.EchoService";
        let addr = "127.0.0.1:50051".parse().unwrap();

        let mut router = des_tonic::Router::new();
        router.add_unary(
            "/echo.EchoService/Echo",
            |req: tonic::Request<bytes::Bytes>| async move {
                Ok(tonic::Response::new(req.into_inner()))
            },
        );

        transport
            .serve_socket_addr(&mut sim, service_name, addr, router)
            .expect("install grpc server");

        let channel = transport
            .connect_socket_addr(&mut sim, service_name, addr)
            .expect("connect grpc client");

        des_tokio::task::spawn_local(async move {
            let start = des_tokio::time::Instant::now();
            let resp = channel
                .unary(
                    "/echo.EchoService/Echo",
                    tonic::Request::new(bytes::Bytes::from_static(b"hello")),
                    Some(Duration::from_secs(1)),
                )
                .await;

            let elapsed = des_tokio::time::Instant::now().duration_since(start);
            match resp {
                Ok(r) => {
                    assert_eq!(r.into_inner(), bytes::Bytes::from_static(b"hello"));
                    metrics::counter!("grpc_requests_total", "service" => service_name).increment(1);
                    metrics::histogram!("grpc_client_latency_ms", "service" => service_name)
                        .record(elapsed.as_secs_f64() * 1000.0);
                }
                Err(e) => panic!("rpc failed: {e}"),
            }
        });

        Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    });

    let locked = metrics.lock().unwrap();
    assert_eq!(
        locked.get_counter("grpc_requests_total", &[("service", "echo.EchoService")]),
        Some(1)
    );

    let hist = locked.get_histogram_stats(
        "grpc_client_latency_ms",
        &[("service", "echo.EchoService")],
    );
    assert!(hist.is_some());
    assert_eq!(hist.unwrap().count, 1);
}
