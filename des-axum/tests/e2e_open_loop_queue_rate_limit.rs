use descartes_core::{Execute, Executor, SimTime, Simulation};
use descartes_metrics::{with_simulation_metrics_recorder, SimulationMetrics};
use descartes_core::dists::{ExponentialDistribution, RequestContext, ServiceTimeDistribution};
use descartes_tower::{DesRateLimit, DesServiceBuilder, ServiceError, SimBody};
use http::{Request, Response, StatusCode};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, Exp};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tower::Service;
use axum::response::IntoResponse;
use tower::ServiceExt;

struct WorkHealthServiceTime {
    work: ExponentialDistribution,
    health: Duration,
}

impl WorkHealthServiceTime {
    fn from_sim(sim: &Simulation, work_mean: Duration, health_time: Duration) -> Self {
        let rate = 1.0 / work_mean.as_secs_f64();
        Self {
            work: ExponentialDistribution::from_config(sim.config(), rate),
            health: health_time,
        }
    }
}

impl ServiceTimeDistribution for WorkHealthServiceTime {
    fn sample_service_time(&mut self, request: &RequestContext) -> Duration {
        if request.uri.starts_with("/health") {
            self.health
        } else {
            self.work.sample_service_time(request)
        }
    }

    fn expected_service_time(&self, request: &RequestContext) -> Duration {
        if request.uri.starts_with("/health") {
            self.health
        } else {
            self.work.expected_service_time(request)
        }
    }
}

#[derive(Clone)]
struct BackendClient {
    tx: descartes_tokio::sync::mpsc::Sender<BackendReq>,
}

struct BackendReq {
    path: String,
    resp_tx: descartes_tokio::sync::oneshot::Sender<Result<Response<SimBody>, ServiceError>>,
}

async fn work(
    axum::extract::State(backend): axum::extract::State<BackendClient>,
) -> axum::response::Response {
    handle_path(backend, "/work").await
}

async fn health(
    axum::extract::State(backend): axum::extract::State<BackendClient>,
) -> axum::response::Response {
    handle_path(backend, "/health").await
}

async fn handle_path(backend: BackendClient, path: &str) -> axum::response::Response {
    let route = path.to_string();
    let (resp_tx, resp_rx) = descartes_tokio::sync::oneshot::channel();
    if backend
        .tx
        .send(BackendReq {
            path: path.to_string(),
            resp_tx,
        })
        .await
        .is_err()
    {
        metrics::counter!("server_requests_error_total", "route" => route.clone()).increment(1);
        return (StatusCode::INTERNAL_SERVER_ERROR, "backend down").into_response();
    }

    match resp_rx.await {
        Ok(Ok(_resp)) => {
            metrics::counter!("server_requests_ok_total", "route" => route.clone()).increment(1);
            (StatusCode::OK, "ok").into_response()
        }
        Ok(Err(ServiceError::Overloaded)) => {
            metrics::counter!("server_requests_dropped_total", "route" => route.clone()).increment(1);
            (StatusCode::TOO_MANY_REQUESTS, "dropped").into_response()
        }
        Ok(Err(e)) => {
            metrics::counter!("server_requests_error_total", "route" => route.clone()).increment(1);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("error: {e}")).into_response()
        }
        Err(_) => {
            metrics::counter!("server_requests_error_total", "route" => route.clone()).increment(1);
            (StatusCode::INTERNAL_SERVER_ERROR, "backend cancelled").into_response()
        }
    }
}

// A deterministic, DES-friendly bounded queue in front of an inner service.
//
// - `call()` enqueues immediately, or drops if full.
// - A single worker task drains the queue and calls the inner service.
// - Queue length is recorded as a histogram for a simple mean estimate.
#[derive(Clone)]
struct QueueService<S> {
    tx: descartes_tokio::sync::mpsc::Sender<QueuedRequest>,
    queue_len: Arc<AtomicUsize>,
    _phantom: std::marker::PhantomData<S>,
}

struct QueuedRequest {
    req: Request<SimBody>,
    resp_tx: descartes_tokio::sync::oneshot::Sender<Result<Response<SimBody>, ServiceError>>,
}

impl<S> QueueService<S>
where
    S: Service<Request<SimBody>, Response = Response<SimBody>, Error = ServiceError>
        + Clone
        + 'static,
    S::Future: Future<Output = Result<Response<SimBody>, ServiceError>> + 'static,
{
    fn new(inner: S, bound: usize) -> Self {
        let (tx, mut rx) = descartes_tokio::sync::mpsc::channel::<QueuedRequest>(bound);
        let queue_len = Arc::new(AtomicUsize::new(0));
        let queue_len_worker = queue_len.clone();

        // Single worker to drain the queue.
        descartes_tokio::task::spawn_local(async move {
            while let Some(QueuedRequest { req, resp_tx }) = rx.recv().await {
                queue_len_worker.fetch_sub(1, Ordering::Relaxed);
                let q = queue_len_worker.load(Ordering::Relaxed) as f64;
                metrics::histogram!("server_queue_len").record(q);

                let mut svc = inner.clone();
                descartes_tokio::task::spawn_local(async move {
                    let result = match svc.ready().await {
                        Ok(ready_svc) => ready_svc.call(req).await,
                        Err(e) => Err(e),
                    };
                    let _ = resp_tx.send(result);
                });
            }
        });

        Self {
            tx,
            queue_len,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<S> Service<Request<SimBody>> for QueueService<S>
where
    S: Service<Request<SimBody>, Response = Response<SimBody>, Error = ServiceError> + 'static,
    S::Future: Future<Output = Result<Response<SimBody>, ServiceError>> + 'static,
{
    type Response = Response<SimBody>;
    type Error = ServiceError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<SimBody>) -> Self::Future {
        let (resp_tx, resp_rx) = descartes_tokio::sync::oneshot::channel();
        let msg = QueuedRequest { req, resp_tx };

        match self.tx.try_send(msg) {
            Ok(()) => {
                let q = self.queue_len.fetch_add(1, Ordering::Relaxed) + 1;
                metrics::histogram!("server_queue_len").record(q as f64);
                Box::pin(async move { resp_rx.await.map_err(|_| ServiceError::Cancelled)? })
            }
            Err(_e) => {
                metrics::counter!("server_queue_dropped_total").increment(1);
                Box::pin(async move { Err(ServiceError::Overloaded) })
            }
        }
    }
}

#[test]
fn open_loop_exponential_arrivals_axum_with_descartes_tower_queue_and_rate_limit() {
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));

    with_simulation_metrics_recorder(&metrics, || {
        let mut sim = Simulation::default();
        descartes_tokio::runtime::install(&mut sim);

        let transport = descartes_axum::Transport::install_default(&mut sim);

        // Build a DES-backed tower service representing server-side compute.
        // /work is exponential(mean=80ms), /health is constant(1ms). Both share queue + token bucket.
        let service_time = WorkHealthServiceTime::from_sim(
            &sim,
            Duration::from_millis(80),
            Duration::from_millis(1),
        );
        let base = DesServiceBuilder::new("axum-backend".to_string())
            .thread_capacity(2)
            .service_time_distribution(service_time)
            .build(&mut sim)
            .expect("build des-tower service");

        // Queue in front of compute.
        let queued = QueueService::new(base, 50);

        // Token bucket admission control (drops when no tokens).
        // Example: 8 req/s with burst 16.
        let admitted = DesRateLimit::new(queued, 8.0, 16);

        // Bridge axum handler -> des-tower service through a channel.
        // This keeps axum state `Send + Sync` even though the underlying DES service is not.
        let (tx, mut rx) = descartes_tokio::sync::mpsc::channel::<BackendReq>(1024);
        descartes_tokio::task::spawn_local(async move {
            while let Some(BackendReq { path, resp_tx }) = rx.recv().await {
                let mut svc = admitted.clone();
                let req = Request::builder()
                    .method("GET")
                    .uri(path)
                    .body(SimBody::empty())
                    .expect("build backend request");

                descartes_tokio::task::spawn_local(async move {
                    let result = svc.call(req).await;
                    let _ = resp_tx.send(result);
                });
            }
        });

        let state = BackendClient { tx };
        let app = axum::Router::new()
            .route("/work", axum::routing::get(work))
            .route("/health", axum::routing::get(health))
            .with_state(state);

        transport
            .serve_named(&mut sim, "svc", "svc-1", app)
            .expect("install des-axum server");

        let work_client = Rc::new(
            transport
                .connect(&mut sim, "svc")
                .expect("install work client"),
        );
        let health_client = Rc::new(
            transport
                .connect(&mut sim, "svc")
                .expect("install health client"),
        );

        // Open-loop exponential arrivals.
        let sim_duration = Duration::from_secs(20);
        let work_arrival_rate_rps = 15.0;
        let health_arrival_rate_rps = 2.0;
        let seed = 42u64;
        descartes_tokio::task::spawn_local(async move {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let exp = Exp::new(work_arrival_rate_rps).expect("valid exp rate");
            let start = descartes_tokio::time::Instant::now();

            loop {
                let now = descartes_tokio::time::Instant::now();
                if now.duration_since(start) >= sim_duration {
                    break;
                }

                let dt_secs: f64 = exp.sample(&mut rng);
                let dt = Duration::from_secs_f64(dt_secs.max(0.0));
                descartes_tokio::time::sleep(dt).await;

                metrics::counter!("client_requests_sent_total", "route" => "/work").increment(1);

                let client = Rc::clone(&work_client);
                descartes_tokio::task::spawn_local(async move {
                    let t0 = descartes_tokio::time::Instant::now();
                    let resp = client.get("/work", Some(Duration::from_secs(5))).await;
                    let latency = descartes_tokio::time::Instant::now().duration_since(t0);
                    metrics::histogram!("client_latency_ms", "route" => "/work")
                        .record(latency.as_secs_f64() * 1000.0);

                    match resp {
                        Ok(r) if r.status() == StatusCode::OK => {
                            metrics::counter!("client_requests_ok_total", "route" => "/work")
                                .increment(1);
                        }
                        Ok(_r) => {
                            metrics::counter!("client_requests_dropped_total", "route" => "/work")
                                .increment(1);
                        }
                        Err(_e) => {
                            metrics::counter!("client_requests_error_total", "route" => "/work")
                                .increment(1);
                        }
                    }
                });
            }
        });

        // Health client: open-loop exponential arrivals.
        descartes_tokio::task::spawn_local(async move {
            let mut rng = ChaCha8Rng::seed_from_u64(seed ^ 0xBEEF);
            let exp = Exp::new(health_arrival_rate_rps).expect("valid exp rate");
            let start = descartes_tokio::time::Instant::now();

            loop {
                let now = descartes_tokio::time::Instant::now();
                if now.duration_since(start) >= sim_duration {
                    break;
                }

                let dt_secs: f64 = exp.sample(&mut rng);
                let dt = Duration::from_secs_f64(dt_secs.max(0.0));
                descartes_tokio::time::sleep(dt).await;

                metrics::counter!("client_requests_sent_total", "route" => "/health").increment(1);

                let client = Rc::clone(&health_client);
                descartes_tokio::task::spawn_local(async move {
                    let t0 = descartes_tokio::time::Instant::now();
                    let resp = client.get("/health", Some(Duration::from_secs(5))).await;
                    let latency = descartes_tokio::time::Instant::now().duration_since(t0);
                    metrics::histogram!("client_latency_ms", "route" => "/health")
                        .record(latency.as_secs_f64() * 1000.0);

                    match resp {
                        Ok(r) if r.status() == StatusCode::OK => {
                            metrics::counter!("client_requests_ok_total", "route" => "/health")
                                .increment(1);
                        }
                        Ok(_r) => {
                            metrics::counter!(
                                "client_requests_dropped_total",
                                "route" => "/health"
                            )
                            .increment(1);
                        }
                        Err(_e) => {
                            metrics::counter!("client_requests_error_total", "route" => "/health")
                                .increment(1);
                        }
                    }
                });
            }
        });

        Executor::timed(SimTime::from_duration(sim_duration)).execute(&mut sim);
    });

    let locked = metrics.lock().unwrap();
    let avg_queue_len = locked
        .get_histogram_stats_simple("server_queue_len")
        .map(|s| s.mean)
        .unwrap_or(0.0);

    let routes = ["/work", "/health"];
    for route in routes {
        let sent = locked
            .get_counter("client_requests_sent_total", &[("route", route)])
            .unwrap_or(0) as f64;
        let ok = locked
            .get_counter("client_requests_ok_total", &[("route", route)])
            .unwrap_or(0) as f64;
        let dropped = locked
            .get_counter("client_requests_dropped_total", &[("route", route)])
            .unwrap_or(0) as f64;
        let errors = locked
            .get_counter("client_requests_error_total", &[("route", route)])
            .unwrap_or(0) as f64;

        let avg_latency_ms = locked
            .get_histogram_stats("client_latency_ms", &[("route", route)])
            .map(|s| s.mean)
            .unwrap_or(0.0);

        let completed = ok + dropped + errors;
        let drop_rate = if completed > 0.0 { dropped / completed } else { 0.0 };

        println!(
            "route={route} avg_latency_ms={avg_latency_ms:.3} drop_rate={drop_rate:.3} sent={sent} completed={completed} ok={ok} dropped={dropped} errors={errors}"
        );

        assert!(sent > 0.0);
        assert!(ok > 0.0);
    }

    println!("avg_queue_len={avg_queue_len:.3}");
    assert!(avg_queue_len >= 0.0);
}
