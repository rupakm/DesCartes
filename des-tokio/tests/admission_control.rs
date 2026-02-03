use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use descartes_core::{defer_wake, Component, Execute, Executor, Key, Scheduler, SimTime, Simulation};
use rand::SeedableRng;
use rand_distr::Distribution;

#[derive(Debug, Clone)]
enum ServerEvent {
    Request { response: ResponseStateHandle },
    Finish { response: ResponseStateHandle },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Response {
    Ok,
    Rejected,
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
struct AdmissionServer {
    capacity: usize,
    service_time: Duration,
    in_flight: usize,
    max_in_flight: usize,
    accepted: usize,
    rejected: usize,
    request_times: Arc<Mutex<Vec<SimTime>>>,
}

impl AdmissionServer {
    fn new(
        capacity: usize,
        service_time: Duration,
        request_times: Arc<Mutex<Vec<SimTime>>>,
    ) -> Self {
        Self {
            capacity,
            service_time,
            in_flight: 0,
            max_in_flight: 0,
            accepted: 0,
            rejected: 0,
            request_times,
        }
    }
}

impl Component for AdmissionServer {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ServerEvent::Request { response } => {
                self.request_times.lock().unwrap().push(scheduler.time());

                if self.in_flight < self.capacity {
                    self.in_flight += 1;
                    self.max_in_flight = self.max_in_flight.max(self.in_flight);
                    self.accepted += 1;

                    scheduler.schedule(
                        SimTime::from_duration(self.service_time),
                        self_id,
                        ServerEvent::Finish {
                            response: response.clone(),
                        },
                    );
                } else {
                    self.rejected += 1;
                    response.complete(Response::Rejected);
                }
            }
            ServerEvent::Finish { response } => {
                self.in_flight -= 1;
                response.complete(Response::Ok);
            }
        }
    }
}

async fn request_task_at(
    arrival: SimTime,
    server_key: Key<ServerEvent>,
    completed: Arc<Mutex<Vec<(SimTime, Response)>>>,
) {
    descartes_core::async_runtime::sim_sleep_until(arrival).await;

    let response_state = ResponseStateHandle::new();
    let response_future = ResponseFuture {
        inner: response_state.inner.clone(),
    };

    defer_wake(
        server_key,
        ServerEvent::Request {
            response: response_state.clone(),
        },
    );

    let response = response_future.await;
    let now = descartes_core::async_runtime::current_sim_time().expect("must be in scheduler context");
    completed.lock().unwrap().push((now, response));
}

#[test]
fn admission_control_rejects_during_spike_and_does_not_block_open_loop() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let request_times = Arc::new(Mutex::new(Vec::new()));
    let server_key = sim.add_component(AdmissionServer::new(
        5,
        Duration::from_millis(250),
        request_times.clone(),
    ));

    let completed: Arc<Mutex<Vec<(SimTime, Response)>>> = Arc::new(Mutex::new(Vec::new()));

    // Baseline exponential arrivals.
    let mut rng = rand::rngs::StdRng::seed_from_u64(4242);
    let exp = rand_distr::Exp::new(8.0).expect("rate must be positive");

    // Precompute arrival times and spawn all request tasks upfront.
    // This models an open-loop client: request generation is independent of response time.
    let mut arrivals: Vec<SimTime> = Vec::new();
    let mut t = SimTime::zero();

    // First 10 baseline arrivals
    arrivals.push(t);
    for _ in 1..10u32 {
        let dt = Duration::from_secs_f64(exp.sample(&mut rng));
        t = t + dt;
        arrivals.push(t);
    }

    // Spike: 50 immediate arrivals at the same time `t`
    for _ in 0..50u32 {
        arrivals.push(t);
    }

    // More baseline arrivals (10)
    for _ in 0..10u32 {
        let dt = Duration::from_secs_f64(exp.sample(&mut rng));
        t = t + dt;
        arrivals.push(t);
    }

    // Spawn all request tasks and keep the JoinHandles alive so they are not cancelled.
    let mut handles = Vec::with_capacity(arrivals.len());
    for arrival in arrivals {
        let completed = completed.clone();
        let h = descartes_tokio::task::spawn(async move {
            request_task_at(arrival, server_key, completed).await;
        });
        handles.push(h);
    }

    // Run the simulation.
    Executor::timed(SimTime::from_duration(Duration::from_secs(30))).execute(&mut sim);

    // Assertions:
    // - The server must have observed multiple arrivals before the first completion.
    let observed = request_times.lock().unwrap().clone();
    assert!(
        observed.len() >= 5,
        "expected at least 5 requests to reach server, got {}",
        observed.len()
    );

    let completed_values = completed.lock().unwrap().clone();
    assert!(!completed_values.is_empty());

    let first_completion = completed_values.iter().map(|(t, _)| *t).min().unwrap();
    let arrivals_before_first_completion =
        observed.iter().filter(|&&t| t < first_completion).count();
    assert!(
        arrivals_before_first_completion >= 2,
        "expected >=2 arrivals before first completion, got {arrivals_before_first_completion}"
    );

    // - During the spike, admission control should reject at least some requests.
    let rejected = completed_values
        .iter()
        .filter(|(_, r)| *r == Response::Rejected)
        .count();
    assert!(rejected > 0, "expected some rejections during spike");

    // - Sanity check the server never exceeded capacity.
    let server = sim
        .get_component_mut::<ServerEvent, AdmissionServer>(server_key)
        .unwrap();
    assert!(server.max_in_flight <= server.capacity);
    assert!(server.rejected > 0);
}
