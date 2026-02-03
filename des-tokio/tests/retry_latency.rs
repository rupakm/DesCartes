use std::sync::{
    atomic::{AtomicUsize, Ordering as AtomicOrdering},
    Arc, Mutex,
};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use descartes_core::{defer_wake, Component, Execute, Executor, Key, Scheduler, SimTime, Simulation};
use rand::SeedableRng;
use rand_distr::Distribution;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Response {
    Ok,
    Busy,
    Rejected,
}

#[derive(Debug, Clone)]
enum ServerEvent {
    Request { response: ResponseStateHandle },
    Finish { response: ResponseStateHandle },
}

#[derive(Debug, Default)]
struct ResponseState {
    response: Option<Response>,
    waker: Option<Waker>,
}

#[derive(Clone, Debug)]
struct ResponseStateHandle {
    inner: Arc<Mutex<ResponseState>>,
}

impl ResponseStateHandle {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ResponseState::default())),
        }
    }

    fn complete(&self, response: Response) {
        let waker_to_wake = {
            let mut locked = self.inner.lock().unwrap();
            locked.response = Some(response);
            locked.waker.take()
        };

        if let Some(waker) = waker_to_wake {
            waker.wake();
        }
    }
}

struct ResponseFuture {
    inner: Arc<Mutex<ResponseState>>,
}

impl std::future::Future for ResponseFuture {
    type Output = Response;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut locked = self.inner.lock().unwrap();

        if let Some(response) = locked.response.take() {
            Poll::Ready(response)
        } else {
            locked.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[derive(Debug)]
struct LoadSheddingServer {
    capacity: usize,
    service_time: Duration,
    in_flight: usize,
    max_in_flight: usize,
    busy_responses: usize,
    ok_responses: usize,
}

impl LoadSheddingServer {
    fn new(capacity: usize, service_time: Duration) -> Self {
        Self {
            capacity,
            service_time,
            in_flight: 0,
            max_in_flight: 0,
            busy_responses: 0,
            ok_responses: 0,
        }
    }
}

impl Component for LoadSheddingServer {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ServerEvent::Request { response } => {
                if self.in_flight < self.capacity {
                    self.in_flight += 1;
                    self.max_in_flight = self.max_in_flight.max(self.in_flight);
                    self.ok_responses += 1;

                    scheduler.schedule(
                        SimTime::from_duration(self.service_time),
                        self_id,
                        ServerEvent::Finish {
                            response: response.clone(),
                        },
                    );
                } else {
                    self.busy_responses += 1;
                    response.complete(Response::Busy);
                }
            }
            ServerEvent::Finish { response } => {
                self.in_flight -= 1;
                response.complete(Response::Ok);
            }
        }
    }
}

fn exp_backoff(attempt: usize) -> Duration {
    // Deterministic exponential backoff, no jitter.
    // base=10ms, doubles each attempt, capped at 200ms.
    let base_ms = 10u64;
    let max_ms = 200u64;

    let shift = (attempt.saturating_sub(1) as u32).min(16);
    let factor = 1u64 << shift;

    Duration::from_millis(base_ms.saturating_mul(factor).min(max_ms))
}

async fn retrying_request_task_at(
    arrival: SimTime,
    server_key: Key<ServerEvent>,
    spike_start: SimTime,
    baseline_latencies: Arc<Mutex<Vec<Duration>>>,
    spike_latencies: Arc<Mutex<Vec<Duration>>>,
    busy_counter: Arc<AtomicUsize>,
) {
    descartes_core::async_runtime::sim_sleep_until(arrival).await;

    let start = descartes_core::async_runtime::current_sim_time().expect("in scheduler context");

    let mut attempt = 1usize;
    let max_attempts = 10usize;

    loop {
        let response_state = ResponseStateHandle::new();
        let response_future = ResponseFuture {
            inner: response_state.inner.clone(),
        };

        defer_wake(
            server_key,
            ServerEvent::Request {
                response: response_state,
            },
        );

        match response_future.await {
            Response::Ok => {
                let end =
                    descartes_core::async_runtime::current_sim_time().expect("in scheduler context");
                let latency = end - start;

                if arrival < spike_start {
                    baseline_latencies.lock().unwrap().push(latency);
                } else {
                    spike_latencies.lock().unwrap().push(latency);
                }

                break;
            }
            Response::Busy => {
                busy_counter.fetch_add(1, AtomicOrdering::Relaxed);
                if attempt >= max_attempts {
                    // Treat as "give up"; record as spike latency to highlight pathologies.
                    let end =
                        descartes_core::async_runtime::current_sim_time().expect("in scheduler context");
                    let latency = end - start;
                    spike_latencies.lock().unwrap().push(latency);
                    break;
                }

                let delay = exp_backoff(attempt);
                attempt += 1;
                descartes_tokio::time::sleep(delay).await;
            }
            Response::Rejected => {
                // This server variant doesn't reject; treat as a give-up path.
                let end =
                    descartes_core::async_runtime::current_sim_time().expect("in scheduler context");
                let latency = end - start;
                spike_latencies.lock().unwrap().push(latency);
                break;
            }
        }
    }
}

fn median(values: &mut [Duration]) -> Duration {
    values.sort();
    values[values.len() / 2]
}

#[test]
fn retrying_clients_under_spike_increase_latency_distribution() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let server_key = sim.add_component(LoadSheddingServer::new(5, Duration::from_millis(250)));

    // Arrival process
    let mut rng = rand::rngs::StdRng::seed_from_u64(9001);
    let exp = rand_distr::Exp::new(6.0).expect("rate must be positive"); // mean ~166ms

    let baseline_count = 30usize;
    let spike_count = 60usize;

    let spike_start = SimTime::from_duration(Duration::from_secs(5));

    // Build baseline arrivals from t=0
    let mut arrivals: Vec<SimTime> = Vec::new();
    let mut t = SimTime::zero();
    arrivals.push(t);
    for _ in 1..baseline_count {
        let dt = Duration::from_secs_f64(exp.sample(&mut rng));
        t = t + dt;
        arrivals.push(t);
    }

    // Make sure spike starts after baseline region.
    t = spike_start;

    // Spike: many arrivals at the same time.
    for _ in 0..spike_count {
        arrivals.push(t);
    }

    // Add some post-spike baseline arrivals.
    for _ in 0..10usize {
        let dt = Duration::from_secs_f64(exp.sample(&mut rng));
        t = t + dt;
        arrivals.push(t);
    }

    let baseline_latencies: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let spike_latencies: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let busy_counter = Arc::new(AtomicUsize::new(0));

    // Spawn all tasks up-front and keep handles alive.
    let mut handles = Vec::with_capacity(arrivals.len());
    for arrival in arrivals {
        let baseline_latencies = baseline_latencies.clone();
        let spike_latencies = spike_latencies.clone();
        let busy_counter = busy_counter.clone();

        let h = descartes_tokio::task::spawn(async move {
            retrying_request_task_at(
                arrival,
                server_key,
                spike_start,
                baseline_latencies,
                spike_latencies,
                busy_counter,
            )
            .await;
        });
        handles.push(h);
    }

    Executor::timed(SimTime::from_duration(Duration::from_secs(60))).execute(&mut sim);

    // Ensure we actually exercised retries.
    assert!(busy_counter.load(AtomicOrdering::Relaxed) > 0);

    let mut baseline = baseline_latencies.lock().unwrap().clone();
    let mut spike = spike_latencies.lock().unwrap().clone();

    assert!(!baseline.is_empty());
    assert!(!spike.is_empty());

    let baseline_med = median(&mut baseline);
    let spike_med = median(&mut spike);

    assert!(
        spike_med > baseline_med,
        "expected spike median latency > baseline median latency, got baseline={:?} spike={:?}",
        baseline_med,
        spike_med
    );

    // Sanity: server never exceeded capacity.
    let server = sim
        .get_component_mut::<ServerEvent, LoadSheddingServer>(server_key)
        .unwrap();
    assert!(server.max_in_flight <= server.capacity);

    // Keep handles alive until end of test.
    drop(handles);
}

#[derive(Debug)]
struct AdmissionLoadSheddingServer {
    capacity: usize,
    service_time: Duration,

    /// Deterministic admission control: reject every Nth request.
    ///
    /// This provides a stable, non-random rejection path while avoiding
    /// unbounded rejection amplification due to retries.
    reject_every: usize,
    total_requests: usize,

    in_flight: usize,
    max_in_flight: usize,

    busy_responses: usize,
    ok_responses: usize,
    rejected_responses: usize,
}

impl AdmissionLoadSheddingServer {
    fn new(capacity: usize, service_time: Duration, reject_every: usize) -> Self {
        Self {
            capacity,
            service_time,
            reject_every,
            total_requests: 0,
            in_flight: 0,
            max_in_flight: 0,
            busy_responses: 0,
            ok_responses: 0,
            rejected_responses: 0,
        }
    }
}

impl Component for AdmissionLoadSheddingServer {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ServerEvent::Request { response } => {
                self.total_requests += 1;

                if self.reject_every > 0 && (self.total_requests % self.reject_every == 0) {
                    self.rejected_responses += 1;
                    response.complete(Response::Rejected);
                    return;
                }

                if self.in_flight < self.capacity {
                    self.in_flight += 1;
                    self.max_in_flight = self.max_in_flight.max(self.in_flight);
                    self.ok_responses += 1;

                    scheduler.schedule(
                        SimTime::from_duration(self.service_time),
                        self_id,
                        ServerEvent::Finish {
                            response: response.clone(),
                        },
                    );
                } else {
                    self.busy_responses += 1;
                    response.complete(Response::Busy);
                }
            }
            ServerEvent::Finish { response } => {
                self.in_flight -= 1;
                response.complete(Response::Ok);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Ok,
    Timeout,
    Rejected,
}

async fn deadline_retrying_request_task_at(
    arrival: SimTime,
    timeout: Duration,
    server_key: Key<ServerEvent>,
    spike_start: SimTime,
    baseline_success_latencies: Arc<Mutex<Vec<Duration>>>,
    spike_success_latencies: Arc<Mutex<Vec<Duration>>>,
    baseline_outcomes: Arc<Mutex<Vec<Outcome>>>,
    spike_outcomes: Arc<Mutex<Vec<Outcome>>>,
) {
    descartes_core::async_runtime::sim_sleep_until(arrival).await;

    let start = descartes_core::async_runtime::current_sim_time().expect("in scheduler context");
    let deadline = start + timeout;

    let mut attempt = 1usize;

    loop {
        let now = descartes_core::async_runtime::current_sim_time().expect("in scheduler context");
        if now >= deadline {
            if arrival < spike_start {
                baseline_outcomes.lock().unwrap().push(Outcome::Timeout);
            } else {
                spike_outcomes.lock().unwrap().push(Outcome::Timeout);
            }
            return;
        }

        let response_state = ResponseStateHandle::new();
        let response_future = ResponseFuture {
            inner: response_state.inner.clone(),
        };

        defer_wake(
            server_key,
            ServerEvent::Request {
                response: response_state,
            },
        );

        match response_future.await {
            Response::Ok => {
                let end =
                    descartes_core::async_runtime::current_sim_time().expect("in scheduler context");
                let latency = end - start;

                if end > deadline {
                    if arrival < spike_start {
                        baseline_outcomes.lock().unwrap().push(Outcome::Timeout);
                    } else {
                        spike_outcomes.lock().unwrap().push(Outcome::Timeout);
                    }
                    return;
                }

                if arrival < spike_start {
                    baseline_success_latencies.lock().unwrap().push(latency);
                    baseline_outcomes.lock().unwrap().push(Outcome::Ok);
                } else {
                    spike_success_latencies.lock().unwrap().push(latency);
                    spike_outcomes.lock().unwrap().push(Outcome::Ok);
                }
                return;
            }
            Response::Rejected => {
                if arrival < spike_start {
                    baseline_outcomes.lock().unwrap().push(Outcome::Rejected);
                } else {
                    spike_outcomes.lock().unwrap().push(Outcome::Rejected);
                }
                return;
            }
            Response::Busy => {
                let delay = exp_backoff(attempt);
                attempt += 1;

                let now =
                    descartes_core::async_runtime::current_sim_time().expect("in scheduler context");
                if now + delay > deadline {
                    if arrival < spike_start {
                        baseline_outcomes.lock().unwrap().push(Outcome::Timeout);
                    } else {
                        spike_outcomes.lock().unwrap().push(Outcome::Timeout);
                    }
                    return;
                }

                descartes_tokio::time::sleep(delay).await;
            }
        }
    }
}

#[test]
fn admission_rejection_and_deadlines_reduce_success_under_spike() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    // Capacity is small, and admission control deterministically rejects
    // every Nth request (stable, no randomness).
    let server_key = sim.add_component(AdmissionLoadSheddingServer::new(
        2,
        Duration::from_millis(250),
        25,
    ));

    // Arrival process
    let mut rng = rand::rngs::StdRng::seed_from_u64(1337);
    let exp = rand_distr::Exp::new(6.0).expect("rate must be positive");

    let baseline_count = 30usize;
    let spike_count = 80usize;
    let spike_start = SimTime::from_duration(Duration::from_secs(5));

    // Build baseline arrivals up to spike_start
    let mut arrivals: Vec<SimTime> = Vec::new();
    let mut t = SimTime::zero();
    arrivals.push(t);
    for _ in 1..baseline_count {
        let dt = Duration::from_secs_f64(exp.sample(&mut rng));
        t = t + dt;
        arrivals.push(t);
    }

    // Spike at spike_start
    t = spike_start;
    for _ in 0..spike_count {
        arrivals.push(t);
    }

    let baseline_success_latencies: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let spike_success_latencies: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));

    let baseline_outcomes: Arc<Mutex<Vec<Outcome>>> = Arc::new(Mutex::new(Vec::new()));
    let spike_outcomes: Arc<Mutex<Vec<Outcome>>> = Arc::new(Mutex::new(Vec::new()));

    // Deadline is short enough that retries under a spike will exceed it,
    // but long enough that some requests can still succeed.
    // Deadline long enough to allow some retries to succeed, but short enough
    // that a portion of spike traffic times out.
    let timeout = Duration::from_millis(700);

    let mut handles = Vec::with_capacity(arrivals.len());
    for arrival in arrivals {
        let baseline_success_latencies = baseline_success_latencies.clone();
        let spike_success_latencies = spike_success_latencies.clone();
        let baseline_outcomes = baseline_outcomes.clone();
        let spike_outcomes = spike_outcomes.clone();

        let h = descartes_tokio::task::spawn(async move {
            deadline_retrying_request_task_at(
                arrival,
                timeout,
                server_key,
                spike_start,
                baseline_success_latencies,
                spike_success_latencies,
                baseline_outcomes,
                spike_outcomes,
            )
            .await;
        });
        handles.push(h);
    }

    Executor::timed(SimTime::from_duration(Duration::from_secs(60))).execute(&mut sim);

    let baseline = baseline_outcomes.lock().unwrap().clone();
    let spike = spike_outcomes.lock().unwrap().clone();

    assert!(!baseline.is_empty());
    assert!(!spike.is_empty());

    let baseline_ok = baseline.iter().filter(|o| **o == Outcome::Ok).count();
    let spike_ok = spike.iter().filter(|o| **o == Outcome::Ok).count();

    let spike_rejected = spike.iter().filter(|o| **o == Outcome::Rejected).count();
    let spike_timeouts = spike.iter().filter(|o| **o == Outcome::Timeout).count();

    assert!(spike_rejected > 0, "expected some rejects during spike");
    assert!(spike_timeouts > 0, "expected some timeouts during spike");

    assert!(
        spike_ok < baseline_ok,
        "expected fewer successes during spike (baseline_ok={baseline_ok} spike_ok={spike_ok})"
    );

    let mut baseline_lats = baseline_success_latencies.lock().unwrap().clone();
    let mut spike_lats = spike_success_latencies.lock().unwrap().clone();

    assert!(!baseline_lats.is_empty());
    assert!(!spike_lats.is_empty());

    baseline_lats.sort();
    spike_lats.sort();

    let baseline_med = baseline_lats[baseline_lats.len() / 2];
    let spike_med = spike_lats[spike_lats.len() / 2];

    let baseline_p90 = baseline_lats[(baseline_lats.len() * 9) / 10];
    let spike_p90 = spike_lats[(spike_lats.len() * 9) / 10];

    let baseline_max = *baseline_lats.last().unwrap();
    let spike_max = *spike_lats.last().unwrap();

    // The median may stay at service_time if the earliest tasks get admitted immediately.
    // The more robust signal is the tail latency under spikes.
    assert!(
        spike_p90 > baseline_p90 || spike_max > baseline_max,
        "expected spike tail latency > baseline tail latency, baseline_med={:?} spike_med={:?} baseline_p90={:?} spike_p90={:?} baseline_max={:?} spike_max={:?}",
        baseline_med,
        spike_med,
        baseline_p90,
        spike_p90,
        baseline_max,
        spike_max
    );

    let server = sim
        .get_component_mut::<ServerEvent, AdmissionLoadSheddingServer>(server_key)
        .unwrap();
    assert!(server.max_in_flight <= server.capacity);
    assert!(server.rejected_responses > 0);

    drop(handles);
}
