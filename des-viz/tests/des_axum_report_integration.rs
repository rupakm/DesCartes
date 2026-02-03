use des_core::{Execute, Executor, SimTime, Simulation};
use des_metrics::{with_simulation_metrics_recorder, SimulationMetrics};
use des_viz::report::generate_html_report;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn make_temp_dir(prefix: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("{prefix}_{pid}_{n}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn html_report_works_with_des_axum_metrics_labels() {
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));

    with_simulation_metrics_recorder(&metrics, || {
        let mut sim = Simulation::default();
        des_tokio::runtime::install(&mut sim);
        let transport = des_axum::Transport::install_default(&mut sim);

        let app = axum::Router::new()
            .route(
                "/work",
                axum::routing::get(|| async {
                    des_tokio::time::sleep(Duration::from_millis(10)).await;
                    "ok"
                }),
            )
            .route(
                "/health",
                axum::routing::get(|| async {
                    des_tokio::time::sleep(Duration::from_millis(1)).await;
                    "ok"
                }),
            );

        transport
            .serve_named(&mut sim, "svc", "svc-1", app)
            .expect("install server");

        let client = transport.connect(&mut sim, "svc").expect("install client");

        des_tokio::task::spawn_local(async move {
            for _ in 0..20u32 {
                let start = des_tokio::time::Instant::now();
                let resp = client
                    .get("/work", Some(Duration::from_secs(1)))
                    .await
                    .expect("work response");
                let latency = des_tokio::time::Instant::now().duration_since(start);

                metrics::counter!(
                    "http_requests_total",
                    "route" => "/work",
                    "status" => resp.status().as_str().to_string()
                )
                .increment(1);
                metrics::histogram!("client_latency_ms", "route" => "/work")
                    .record(latency.as_secs_f64() * 1000.0);
            }
        });

        let health_client = transport.connect(&mut sim, "svc").expect("install health client");
        des_tokio::task::spawn_local(async move {
            for _ in 0..50u32 {
                let start = des_tokio::time::Instant::now();
                let resp = health_client
                    .get("/health", Some(Duration::from_secs(1)))
                    .await
                    .expect("health response");
                let latency = des_tokio::time::Instant::now().duration_since(start);

                metrics::counter!(
                    "http_requests_total",
                    "route" => "/health",
                    "status" => resp.status().as_str().to_string()
                )
                .increment(1);
                metrics::histogram!("client_latency_ms", "route" => "/health")
                    .record(latency.as_secs_f64() * 1000.0);
            }
        });

        Executor::timed(SimTime::from_duration(Duration::from_secs(2))).execute(&mut sim);
    });

    let snapshot = metrics.lock().unwrap().get_metrics_snapshot();
    let dir = make_temp_dir("des_viz_axum_report_test");
    let report_path = dir.join("report.html");

    generate_html_report(&snapshot, &report_path).expect("generate report");

    assert!(report_path.exists());
    assert!(dir.join("charts/latency_stats.png").exists());
    assert!(dir.join("charts/percentiles.png").exists());
    assert!(dir.join("charts/throughput.png").exists());

    std::fs::remove_dir_all(dir).ok();
}
