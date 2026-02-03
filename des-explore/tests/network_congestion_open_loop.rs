use std::sync::{Arc, Mutex};
use std::time::Duration;

use descartes_core::async_runtime::current_sim_time;
use descartes_core::dists::{
    ArrivalPattern, ExponentialDistribution, PoissonArrivals, ServiceTimeDistribution,
};
use descartes_core::{Executor, SimTime, Simulation, SimulationConfig};

#[derive(Debug)]
struct Request {
    id: u64,
    start: SimTime,
    respond_to: descartes_tokio::sync::oneshot::Sender<Response>,
}

#[derive(Debug, Clone)]
struct Response {
    request_id: u64,
}

struct Network {
    congestion_start: SimTime,
    fast_latency: Mutex<ExponentialDistribution>,
    congested_latency: Mutex<ExponentialDistribution>,
}

impl Network {
    fn new(
        cfg: &SimulationConfig,
        congestion_start: SimTime,
        fast_rate_per_sec: f64,
        congested_rate_per_sec: f64,
    ) -> Self {
        Self {
            congestion_start,
            fast_latency: Mutex::new(ExponentialDistribution::from_config(cfg, fast_rate_per_sec)),
            congested_latency: Mutex::new(ExponentialDistribution::from_config(
                cfg,
                congested_rate_per_sec,
            )),
        }
    }

    fn sample_one_way_delay(&self) -> Duration {
        let now = current_sim_time().expect("called inside simulation");
        if now >= self.congestion_start {
            self.congested_latency.lock().unwrap().sample()
        } else {
            self.fast_latency.lock().unwrap().sample()
        }
    }

    async fn send_to_server(
        self: &Arc<Self>,
        req: Request,
        server_tx: descartes_tokio::sync::mpsc::Sender<Request>,
    ) {
        // One-way delay client -> server.
        let d = self.sample_one_way_delay();
        descartes_tokio::time::sleep(d).await;

        // If server is gone, drop the request (and thus the response channel).
        let _ = server_tx.send(req).await;
    }

    async fn send_to_client(
        self: &Arc<Self>,
        resp: Response,
        respond_to: descartes_tokio::sync::oneshot::Sender<Response>,
    ) {
        // One-way delay server -> client.
        let d = self.sample_one_way_delay();
        descartes_tokio::time::sleep(d).await;
        let _ = respond_to.send(resp);
    }
}

#[derive(Debug, Default, Clone)]
struct LatencyStats {
    pre_sum: Duration,
    pre_count: u64,
    post_sum: Duration,
    post_count: u64,
    timed_out: u64,
    succeeded: u64,
}

impl LatencyStats {
    fn record_success(
        &mut self,
        started_at: SimTime,
        finished_at: SimTime,
        congestion_start: SimTime,
    ) {
        let lat: Duration = finished_at - started_at;
        if started_at < congestion_start {
            self.pre_sum += lat;
            self.pre_count += 1;
        } else {
            self.post_sum += lat;
            self.post_count += 1;
        }
        self.succeeded += 1;
    }

    fn record_timeout(&mut self) {
        self.timed_out += 1;
    }

    fn avg_pre(&self) -> Option<Duration> {
        if self.pre_count == 0 {
            None
        } else {
            Some(self.pre_sum / (self.pre_count as u32))
        }
    }

    fn avg_post(&self) -> Option<Duration> {
        if self.post_count == 0 {
            None
        } else {
            Some(self.post_sum / (self.post_count as u32))
        }
    }
}

async fn request_with_retries(
    id: u64,
    started_at: SimTime,
    network: Arc<Network>,
    server_tx: descartes_tokio::sync::mpsc::Sender<Request>,
    timeout: Duration,
    base_backoff: Duration,
    max_retries: usize,
    stats: Arc<Mutex<LatencyStats>>,
) {
    for attempt in 0..=max_retries {
        let (tx, rx) = descartes_tokio::sync::oneshot::channel::<Response>();
        let req = Request {
            id,
            start: started_at,
            respond_to: tx,
        };

        // Send request over the network.
        let net = network.clone();
        let tx_clone = server_tx.clone();
        descartes_tokio::task::spawn(async move {
            net.send_to_server(req, tx_clone).await;
        });

        // Await response with timeout.
        match descartes_tokio::time::timeout(timeout, rx).await {
            Ok(Ok(_resp)) => {
                let end = current_sim_time().expect("in sim");
                stats
                    .lock()
                    .unwrap()
                    .record_success(started_at, end, network.congestion_start);
                return;
            }
            Ok(Err(_closed)) => {
                // Server dropped response channel; treat as timeout/failed.
                stats.lock().unwrap().record_timeout();
            }
            Err(_elapsed) => {
                stats.lock().unwrap().record_timeout();
            }
        }

        if attempt == max_retries {
            return;
        }

        // Exponential backoff: base * 2^attempt.
        let backoff = base_backoff * (1u32 << attempt);
        descartes_tokio::time::sleep(backoff).await;
    }
}

async fn server_task(
    cfg: SimulationConfig,
    network: Arc<Network>,
    mut rx: descartes_tokio::sync::mpsc::Receiver<Request>,
) {
    // Exponential service time distribution.
    // Mean = 1/rate seconds.
    let mut service = ExponentialDistribution::from_config(&cfg, 200.0);

    while let Some(req) = rx.recv().await {
        // Queueing happens in the receiver buffer; service is single-threaded.
        let service_time = service.sample();
        descartes_tokio::time::sleep(service_time).await;

        let resp = Response { request_id: req.id };
        let net = network.clone();
        let respond_to = req.respond_to;
        descartes_tokio::task::spawn(async move {
            net.send_to_client(resp, respond_to).await;
        });
    }
}

#[test]
fn open_loop_client_server_congestion_increases_latency_fifo_default() {
    // Default scheduling remains FIFO (no frontier/tokio policy set).
    let cfg = SimulationConfig { seed: 4242 };
    let mut sim = Simulation::new(cfg);
    descartes_tokio::runtime::install(&mut sim);

    // Congestion starts halfway through.
    let sim_end = SimTime::from_secs(2);
    let congestion_start = SimTime::from_secs(1);

    // Network latency model: exponential.
    // Fast mean ~2ms (rate=500/s), congested mean ~30ms (rate=33.3/s).
    let network = Arc::new(Network::new(&sim.config(), congestion_start, 500.0, 33.3));

    // Server request queue.
    let (server_tx, server_rx) = descartes_tokio::sync::mpsc::channel::<Request>(1024);

    // Start server.
    let cfg_for_server = sim.config().clone();
    let net_for_server = network.clone();
    descartes_tokio::task::spawn(async move {
        server_task(cfg_for_server, net_for_server, server_rx).await;
    });

    let stats: Arc<Mutex<LatencyStats>> = Arc::new(Mutex::new(LatencyStats::default()));

    // Open-loop client generator: Poisson arrivals.
    let cfg_for_client = sim.config().clone();
    let net_for_client = network.clone();
    let tx_for_client = server_tx.clone();
    let stats_for_client = stats.clone();

    descartes_tokio::task::spawn(async move {
        let mut arrivals = PoissonArrivals::from_config(&cfg_for_client, 50.0);

        let mut next_id = 0u64;
        loop {
            let now = current_sim_time().expect("in sim");
            if now >= sim_end {
                break;
            }

            let dt = arrivals.next_arrival_time();
            descartes_tokio::time::sleep(dt).await;

            let start = current_sim_time().expect("in sim");
            let net = net_for_client.clone();
            let tx = tx_for_client.clone();
            let stats = stats_for_client.clone();

            let id = next_id;
            next_id += 1;

            // Open-loop: spawn request task, do not await.
            descartes_tokio::task::spawn(async move {
                request_with_retries(
                    id,
                    start,
                    net,
                    tx,
                    Duration::from_millis(100),
                    Duration::from_millis(20),
                    3,
                    stats,
                )
                .await;
            });
        }
    });

    sim.execute(Executor::timed(sim_end));

    let stats = stats.lock().unwrap().clone();
    let avg_pre = stats
        .avg_pre()
        .expect("expected some pre-congestion successes");
    let avg_post = stats
        .avg_post()
        .expect("expected some post-congestion successes");

    eprintln!(
        "latency stats: pre_avg={:?} (n={}), post_avg={:?} (n={}), succeeded={}, timeouts={} ",
        avg_pre, stats.pre_count, avg_post, stats.post_count, stats.succeeded, stats.timed_out
    );

    // The congestion should measurably increase average latency.
    assert!(
        avg_post > avg_pre * 3,
        "expected post-congestion avg latency >> pre-congestion (pre={:?}, post={:?})",
        avg_pre,
        avg_post
    );
}
