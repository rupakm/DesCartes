use bytes::Bytes;
use des_components::transport::{
    LatencyConfig, LatencyJitterModel, SharedEndpointRegistry, SimTransport,
};
use des_core::{SimTime, Simulation, SimulationConfig};
use des_explore::harness::{
    run_recorded, run_replayed, HarnessConfig, HarnessFrontierConfig, HarnessFrontierPolicy,
    HarnessTokioReadyConfig, HarnessTokioReadyPolicy,
};
use des_explore::io::TraceFormat;
use des_tonic::{ClientBuilder, Router, ServerBuilder};
use des_tower::{
    DesGlobalConcurrencyLimitLayer, DesRateLimitLayer, DesServiceBuilder, ServiceError, SimBody,
};
use http::{Request as HttpRequest, Response as HttpResponse};
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tonic::{Code, Request, Response, Status};
use tower::{Service, ServiceExt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stats {
    sent: usize,
    ok: usize,
    timeouts: usize,
    retries: usize,
    rejected: usize,
}

#[derive(Debug, Default)]
struct SharedStats {
    sent: AtomicUsize,
    ok: AtomicUsize,
    timeouts: AtomicUsize,
    retries: AtomicUsize,
    rejected: AtomicUsize,
}

impl SharedStats {
    fn snapshot(&self) -> Stats {
        Stats {
            sent: self.sent.load(Ordering::Relaxed),
            ok: self.ok.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
            retries: self.retries.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
        }
    }
}

// -----------------------------------------------------------------------------
// A DES-aware bounded buffer (queue) in front of a Tower service.
//
// This is NOT `tower::buffer::Buffer` (which assumes a real tokio runtime).
// Instead, it queues requests on a deterministic `des_tokio::sync::mpsc` channel
// and processes them via a single local worker task.
// -----------------------------------------------------------------------------

#[derive(Debug)]
struct BufferState {
    capacity: usize,
    queued: AtomicUsize,
    waiters: std::sync::Mutex<Vec<Waker>>,
}

impl BufferState {
    fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            capacity,
            queued: AtomicUsize::new(0),
            waiters: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn try_acquire(&self) -> bool {
        let current = self.queued.load(Ordering::Relaxed);
        if current >= self.capacity {
            return false;
        }
        self.queued
            .compare_exchange_weak(current, current + 1, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }

    fn release(&self) {
        self.queued.fetch_sub(1, Ordering::Relaxed);
        let waiters = {
            let mut waiters = self.waiters.lock().unwrap();
            std::mem::take(&mut *waiters)
        };
        for waker in waiters {
            waker.wake();
        }
    }

    fn register_waker(&self, waker: Waker) {
        self.waiters.lock().unwrap().push(waker);
    }
}

type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

type BufferedItem = (
    HttpRequest<SimBody>,
    des_tokio::sync::oneshot::Sender<Result<HttpResponse<SimBody>, ServiceError>>,
);

struct DesBuffer {
    state: Arc<BufferState>,
    tx: des_tokio::sync::mpsc::Sender<BufferedItem>,

    // Cached permit acquired in poll_ready (Tower contract).
    permit: bool,
}

impl DesBuffer {
    fn new<S>(mut inner: S, capacity: usize) -> Self
    where
        S: Service<HttpRequest<SimBody>, Response = HttpResponse<SimBody>, Error = ServiceError>
            + 'static,
        S::Future: 'static,
    {
        let state = BufferState::new(capacity);
        let (tx, mut rx) = des_tokio::sync::mpsc::channel::<BufferedItem>(capacity);

        // Single worker task: dequeue then call the inner service.
        let state_for_worker = state.clone();
        des_tokio::task::spawn_local(async move {
            while let Some((req, resp_tx)) = rx.recv().await {
                // Slot represents queued capacity, not in-flight.
                state_for_worker.release();

                let result = match inner.ready().await {
                    Ok(svc) => svc.call(req).await,
                    Err(e) => Err(e),
                };

                let _ = resp_tx.send(result);
            }
        });

        Self {
            state,
            tx,
            permit: false,
        }
    }
}

impl Clone for DesBuffer {
    fn clone(&self) -> Self {
        // Clones must not inherit readiness.
        Self {
            state: self.state.clone(),
            tx: self.tx.clone(),
            permit: false,
        }
    }
}

impl Service<HttpRequest<SimBody>> for DesBuffer {
    type Response = HttpResponse<SimBody>;
    type Error = ServiceError;
    type Future = BoxFut<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.permit {
            return Poll::Ready(Ok(()));
        }

        if self.state.try_acquire() {
            self.permit = true;
            Poll::Ready(Ok(()))
        } else {
            self.state.register_waker(cx.waker().clone());
            Poll::Pending
        }
    }

    fn call(&mut self, req: HttpRequest<SimBody>) -> Self::Future {
        let acquired = if self.permit {
            self.permit = false;
            true
        } else {
            self.state.try_acquire()
        };

        if !acquired {
            return Box::pin(async { Err(ServiceError::NotReady) });
        }

        let (tx, rx) = des_tokio::sync::oneshot::channel();

        if let Err(_e) = self.tx.try_send((req, tx)) {
            // Channel full/closed. Release the slot we just acquired.
            self.state.release();
            return Box::pin(async { Err(ServiceError::Overloaded) });
        }

        Box::pin(async move { rx.await.map_err(|_| ServiceError::Cancelled)? })
    }
}

async fn unary_with_timeout_retry(
    stats: Arc<SharedStats>,
    channel: des_tonic::Channel,
    method: &'static str,
    payload: Bytes,
    timeout: Duration,
    max_retries: usize,
    backoff: Duration,
) -> Result<Bytes, Status> {
    let mut attempt = 0usize;

    loop {
        attempt += 1;
        let req = Request::new(payload.clone());
        let result = channel.unary(method, req, Some(timeout)).await;

        match result {
            Ok(resp) => {
                stats.ok.fetch_add(1, Ordering::Relaxed);
                return Ok(resp.into_inner());
            }
            Err(status) if status.code() == Code::DeadlineExceeded && attempt <= max_retries => {
                stats.timeouts.fetch_add(1, Ordering::Relaxed);
                stats.retries.fetch_add(1, Ordering::Relaxed);
                des_tokio::time::sleep(backoff).await;
                continue;
            }
            Err(status) => return Err(status),
        }
    }
}

fn run_open_loop_scenario(sim: &mut Simulation) -> Stats {
    let stats = Arc::new(SharedStats::default());

    // Transport: fixed latency+jitter model with deterministic seed.
    let network = Box::new(LatencyJitterModel::with_seed(
        LatencyConfig::new(Duration::from_millis(10)),
        42,
    ));
    let transport_key = sim.add_component(SimTransport::new(network));
    let endpoint_registry = SharedEndpointRegistry::new();

    // Server-side Tower service stack.
    let base = DesServiceBuilder::new("tower-backend".to_string())
        .thread_capacity(2)
        .service_time(Duration::from_millis(25))
        .build(sim)
        .expect("build DesService");

    let svc = ::tower::ServiceBuilder::new()
        // Admission control policy.
        .layer(DesRateLimitLayer::new(200.0, 200))
        // Global concurrency cap across all clones.
        .layer(DesGlobalConcurrencyLimitLayer::with_max_concurrency(4))
        .service(base);

    // Bounded queue in front of the service.
    let buffered = DesBuffer::new(svc, 32);

    // Expose via des-tonic.
    let mut router = Router::new();
    router.add_unary("/demo.Echo/Echo", move |req: Request<Bytes>| {
        let mut buffered = buffered.clone();
        async move {
            let http_req = HttpRequest::builder()
                .method("POST")
                .uri("/demo.Echo/Echo")
                .body(SimBody::new(req.into_inner()))
                .unwrap();

            let http_resp = buffered.call(http_req).await.map_err(|e| match e {
                ServiceError::NotReady | ServiceError::Overloaded => {
                    Status::resource_exhausted("admission rejected")
                }
                ServiceError::Timeout { .. } => Status::deadline_exceeded("server timeout"),
                other => Status::internal(format!("server error: {other}")),
            })?;

            Ok(Response::new(http_resp.body().data().clone()))
        }
    });

    let _server = ServerBuilder::new(
        "demo.Echo".to_string(),
        "server-1".to_string(),
        transport_key,
        endpoint_registry.clone(),
        sim.scheduler_handle(),
    )
    .name("demo-server")
    .processing_delay(Duration::from_millis(0))
    .add_router(router)
    .install(sim)
    .expect("install server");

    let client = ClientBuilder::new(
        "demo.Echo".to_string(),
        transport_key,
        endpoint_registry,
        sim.scheduler_handle(),
    )
    .client_name("client-1")
    .install(sim)
    .expect("install client");

    let channel = client.channel;

    // Open-loop generator: schedule sends independent of responses.
    // This avoids feedback control and keeps determinism at the scheduler level.
    let inter_arrival = Duration::from_millis(5);
    let total_requests = 200usize;

    let timeout = Duration::from_millis(80);
    let max_retries = 2usize;
    let backoff = Duration::from_millis(10);

    for i in 0..total_requests {
        let at = SimTime::from_duration(inter_arrival * (i as u32));
        let stats_for_send = stats.clone();
        let channel = channel.clone();

        sim.schedule_closure(at, move |_scheduler| {
            stats_for_send.sent.fetch_add(1, Ordering::Relaxed);
            let stats_for_task = stats_for_send.clone();

            des_tokio::task::spawn_local(async move {
                let payload = Bytes::from(format!("req-{i}"));

                let result = unary_with_timeout_retry(
                    stats_for_task.clone(),
                    channel,
                    "/demo.Echo/Echo",
                    payload,
                    timeout,
                    max_retries,
                    backoff,
                )
                .await;

                if let Err(status) = result {
                    if status.code() == Code::ResourceExhausted {
                        stats_for_task.rejected.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        });
    }

    sim.execute(Executor::timed(SimTime::from_duration(
        Duration::from_secs(3),
    )));
    stats.snapshot()
}

fn record_and_replay() -> Result<(), Box<dyn std::error::Error>> {
    // Note: des_tokio installs into TLS and can only be installed once per thread.
    // Run record+replay in fresh threads to keep this example single-process.

    let trace_path = std::env::temp_dir().join("des_tonic_open_loop_tower_retry_trace.json");

    let record_cfg = HarnessConfig {
        sim_config: SimulationConfig { seed: 1 },
        scenario: "des_tonic_open_loop_tower_retry".to_string(),
        install_tokio: true,
        tokio_ready: Some(HarnessTokioReadyConfig {
            policy: HarnessTokioReadyPolicy::Fifo,
            record_decisions: true,
        }),
        tokio_mutex: None,
        record_concurrency: false,
        frontier: Some(HarnessFrontierConfig {
            policy: HarnessFrontierPolicy::Fifo,
            record_decisions: true,
        }),
        trace_path: trace_path.clone(),
        trace_format: TraceFormat::Json,
    };

    let (record_stats, trace) = std::thread::spawn(move || {
        run_recorded(
            record_cfg,
            |sim_config, _ctx| Simulation::new(sim_config),
            |sim, _ctx| run_open_loop_scenario(sim),
        )
        .expect("record run")
    })
    .join()
    .expect("record thread panicked");

    let replay_cfg = HarnessConfig {
        sim_config: SimulationConfig { seed: 1 },
        scenario: "des_tonic_open_loop_tower_retry_replay".to_string(),
        install_tokio: true,
        tokio_ready: None,
        tokio_mutex: None,
        record_concurrency: false,
        frontier: None,
        trace_path,
        trace_format: TraceFormat::Json,
    };

    let (replay_stats, _replay_trace) = std::thread::spawn(move || {
        run_replayed(
            replay_cfg,
            &trace,
            |sim_config, _ctx, _trace| Simulation::new(sim_config),
            |sim, _ctx| run_open_loop_scenario(sim),
        )
        .expect("replay run")
    })
    .join()
    .expect("replay thread panicked");

    assert_eq!(record_stats, replay_stats);

    println!(
        "record/replay matched: sent={} ok={} timeouts={} retries={} rejected={}",
        record_stats.sent,
        record_stats.ok,
        record_stats.timeouts,
        record_stats.retries,
        record_stats.rejected
    );

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    record_and_replay()
}
