//! Splitting-based bug finder for the retry-storm metastability example.
//!
//! ## What the “search” explores
//!
//! This example uses multilevel splitting to explore *trajectories* of a fixed
//! stochastic model. It does **not** search over arrival-pattern parameters
//! (e.g., different `λ(t)` spike shapes). Instead, it searches over different
//! *random draw sequences* (inter-arrival and service-time samples) that are
//! consistent with the configured distributions.
//!
//! Concretely, each run corresponds to a particular sequence of RNG draws.
//! Splitting saves a replayable trace prefix when the monitor score crosses a
//! level threshold, then branches by replaying that prefix and drawing fresh
//! randomness afterward.
//!
//! ## Why this demo often needs little branching
//!
//! The constants in this file intentionally make metastability likely within the
//! finite horizon, so the first particle may already become metastable. The
//! splitting algorithm becomes more valuable in regimes where metastability is
//! *rare* (smaller/shorter spikes, fewer retries, larger timeouts, higher `μ`,
//! backoff/jitter, etc.).
//!
//! Run:
//! - `cargo run -p des-explore --example find_retry_storm --quiet`

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::dists::{
    ArrivalPattern, ExponentialDistribution, PoissonArrivals, ServiceTimeDistribution,
};
use des_core::{Component, Executor, Key, SimTime, Simulation, SimulationConfig};

use des_explore::harness::HarnessContext;
use des_explore::monitor::{Monitor, MonitorConfig, QueueId};
use des_explore::splitting::{find_with_splitting, SplittingConfig};
use des_explore::trace::{Trace, TraceMeta, TraceRecorder};

const LAMBDA_BASE_RPS: f64 = 9.3;
const MU_RPS: f64 = 10.0;

const QUEUE_CAPACITY: usize = 60;
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RETRIES: u8 = 3;

const SPIKE_START: SimTime = SimTime::from_secs(20);
const SPIKE_END: SimTime = SimTime::from_secs(21);
const LAMBDA_SPIKE_RPS: f64 = 30.0;

const MONITOR_WINDOW: Duration = Duration::from_secs(1);
const REPORT_INTERVAL: Duration = Duration::from_secs(5);
const BACKLOG_QUEUE: QueueId = QueueId(1);

#[derive(Debug, Clone, Copy)]
struct Attempt {
    parent_id: u64,
    attempt_no: u8,
    retries_left: u8,
    arrival_time: SimTime,
    active: bool,
}

#[derive(Debug)]
enum Event {
    ExternalArrival,
    RetryArrival {
        parent_id: u64,
        attempt_no: u8,
        retries_left: u8,
    },
    Timeout {
        attempt_id: u64,
    },
    ServiceComplete {
        attempt_id: u64,
    },
    Report,
}

struct Mm1RetryStorm {
    enable_report: bool,
    monitor: Arc<Mutex<Monitor>>,

    arrivals_base: PoissonArrivals,
    arrivals_spike: PoissonArrivals,
    service: ExponentialDistribution,

    queue: VecDeque<u64>,
    server_busy: bool,

    next_parent_id: u64,
    next_attempt_id: u64,
    attempts: HashMap<u64, Attempt>,

    external_arrivals: u64,
    retry_arrivals: u64,
    dropped: u64,
    timeouts: u64,

    completed_total: u64,
    completed_in_time: u64,
    completed_late: u64,

    total_latency_in_time: Duration,
}

impl Mm1RetryStorm {
    fn in_spike(now: SimTime) -> bool {
        now >= SPIKE_START && now < SPIKE_END
    }

    fn backlog_len(&self) -> u64 {
        self.queue.len() as u64 + u64::from(self.server_busy)
    }

    fn observe_backlog(&self, now: SimTime) {
        self.monitor
            .lock()
            .unwrap()
            .observe_queue_len(now, BACKLOG_QUEUE, self.backlog_len());
    }

    fn schedule_next_external_arrival(
        &mut self,
        self_id: Key<Event>,
        scheduler: &mut des_core::Scheduler,
    ) {
        let now = scheduler.time();
        let inter_arrival = if Self::in_spike(now) {
            self.arrivals_spike.next_arrival_time()
        } else {
            self.arrivals_base.next_arrival_time()
        };

        scheduler.schedule(
            SimTime::from_duration(inter_arrival),
            self_id,
            Event::ExternalArrival,
        );
    }

    fn maybe_start_service(&mut self, self_id: Key<Event>, scheduler: &mut des_core::Scheduler) {
        if self.server_busy {
            return;
        }

        let Some(attempt_id) = self.queue.pop_front() else {
            return;
        };

        let now = scheduler.time();
        self.server_busy = true;
        self.observe_backlog(now);

        let service_time = self.service.sample();
        scheduler.schedule(
            SimTime::from_duration(service_time),
            self_id,
            Event::ServiceComplete { attempt_id },
        );
    }

    fn enqueue_attempt(
        &mut self,
        self_id: Key<Event>,
        scheduler: &mut des_core::Scheduler,
        parent_id: u64,
        attempt_no: u8,
        retries_left: u8,
    ) {
        let now = scheduler.time();

        if self.queue.len() >= QUEUE_CAPACITY {
            self.dropped += 1;
            self.monitor.lock().unwrap().observe_drop(now);
            return;
        }

        let attempt_id = self.next_attempt_id;
        self.next_attempt_id += 1;

        self.attempts.insert(
            attempt_id,
            Attempt {
                parent_id,
                attempt_no,
                retries_left,
                arrival_time: now,
                active: true,
            },
        );

        self.queue.push_back(attempt_id);
        self.observe_backlog(now);

        scheduler.schedule(
            SimTime::from_duration(TIMEOUT),
            self_id,
            Event::Timeout { attempt_id },
        );
        self.maybe_start_service(self_id, scheduler);
    }
}

impl Component for Mm1RetryStorm {
    type Event = Event;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut des_core::Scheduler,
    ) {
        match *event {
            Event::ExternalArrival => {
                self.external_arrivals += 1;

                let parent_id = self.next_parent_id;
                self.next_parent_id += 1;

                self.enqueue_attempt(self_id, scheduler, parent_id, 1, MAX_RETRIES);
                self.schedule_next_external_arrival(self_id, scheduler);
            }
            Event::RetryArrival {
                parent_id,
                attempt_no,
                retries_left,
            } => {
                self.retry_arrivals += 1;
                self.enqueue_attempt(self_id, scheduler, parent_id, attempt_no, retries_left);
            }
            Event::Timeout { attempt_id } => {
                let Some(attempt) = self.attempts.get_mut(&attempt_id) else {
                    return;
                };

                if !attempt.active {
                    return;
                }

                let now = scheduler.time();
                attempt.active = false;
                self.timeouts += 1;
                self.monitor.lock().unwrap().observe_timeout(now);

                if attempt.retries_left > 0 {
                    let next_attempt_no = attempt.attempt_no + 1;
                    let retries_left = attempt.retries_left - 1;

                    self.monitor.lock().unwrap().observe_retry(now);

                    scheduler.schedule_now(
                        self_id,
                        Event::RetryArrival {
                            parent_id: attempt.parent_id,
                            attempt_no: next_attempt_no,
                            retries_left,
                        },
                    );
                }
            }
            Event::ServiceComplete { attempt_id } => {
                let now = scheduler.time();

                self.server_busy = false;
                self.observe_backlog(now);

                self.completed_total += 1;

                if let Some(attempt) = self.attempts.remove(&attempt_id) {
                    let latency = now.duration_since(attempt.arrival_time);
                    self.monitor
                        .lock()
                        .unwrap()
                        .observe_complete(now, latency, attempt.active);

                    if attempt.active {
                        self.completed_in_time += 1;
                        self.total_latency_in_time += latency;
                    } else {
                        self.completed_late += 1;
                    }
                }

                self.maybe_start_service(self_id, scheduler);
            }
            Event::Report => {
                let now = scheduler.time();

                if self.enable_report {
                    let offered = self.external_arrivals + self.retry_arrivals;
                    let amp = if self.completed_in_time == 0 {
                        f64::INFINITY
                    } else {
                        self.retry_arrivals as f64 / self.completed_in_time as f64
                    };

                    let status = {
                        let mut m = self.monitor.lock().unwrap();
                        m.flush_up_to(now);
                        m.status()
                    };

                    let d = status.last_distance.unwrap_or(-1.0);

                    let mut out = std::io::stdout().lock();
                    let _ = writeln!(
                        out,
                        "t={:>6.1}s in_spike={} backlog={:>3} ext={} retry={} offered={} drop={} timeout={} ok={} late={} amp={:.2} D={:.2} rec={} meta={} S={:.2}",
                        now.as_duration().as_secs_f64(),
                        Self::in_spike(now),
                        self.backlog_len(),
                        self.external_arrivals,
                        self.retry_arrivals,
                        offered,
                        self.dropped,
                        self.timeouts,
                        self.completed_in_time,
                        self.completed_late,
                        amp,
                        d,
                        status.recovered,
                        status.metastable,
                        status.score,
                    );
                }

                scheduler.schedule(
                    SimTime::from_duration(REPORT_INTERVAL),
                    self_id,
                    Event::Report,
                );
            }
        }
    }
}

fn build_sim(
    sim_config: SimulationConfig,
    ctx: &HarnessContext,
    prefix: Option<&Trace>,
    cont_seed: u64,
    enable_report: bool,
) -> (Simulation, Arc<Mutex<Monitor>>, Key<Event>) {
    let mut sim = Simulation::new(sim_config);

    let mut monitor_cfg = MonitorConfig::default();
    monitor_cfg.window = MONITOR_WINDOW;
    monitor_cfg.baseline_warmup_windows = 10;
    monitor_cfg.recovery_hold_windows = 3;
    monitor_cfg.baseline_epsilon = 3.0;
    monitor_cfg.metastable_persist_windows = 20;
    monitor_cfg.recovery_time_limit = Some(Duration::from_secs(90));

    let mut monitor = Monitor::new(monitor_cfg, SimTime::zero());
    monitor.mark_post_spike_start(SPIKE_END);

    let monitor = Arc::new(Mutex::new(monitor));

    // One shared provider for all distributions (required for prefix replay).
    let provider = ctx.shared_branching_provider(prefix, cont_seed);

    let arrivals_base = PoissonArrivals::from_config(sim.config(), LAMBDA_BASE_RPS).with_provider(
        Box::new(provider.clone()),
        des_core::draw_site!("arrival_base"),
    );
    let arrivals_spike = PoissonArrivals::from_config(sim.config(), LAMBDA_SPIKE_RPS)
        .with_provider(
            Box::new(provider.clone()),
            des_core::draw_site!("arrival_spike"),
        );
    let service = ExponentialDistribution::from_config(sim.config(), MU_RPS)
        .with_provider(Box::new(provider), des_core::draw_site!("service"));

    let key = sim.add_component(Mm1RetryStorm {
        enable_report,
        monitor: monitor.clone(),
        arrivals_base,
        arrivals_spike,
        service,
        queue: VecDeque::new(),
        server_busy: false,
        next_parent_id: 1,
        next_attempt_id: 1,
        attempts: HashMap::new(),
        external_arrivals: 0,
        retry_arrivals: 0,
        dropped: 0,
        timeouts: 0,
        completed_total: 0,
        completed_in_time: 0,
        completed_late: 0,
        total_latency_in_time: Duration::ZERO,
    });

    sim.schedule_now(key, Event::ExternalArrival);
    if enable_report {
        sim.schedule_now(key, Event::Report);
    }

    (sim, monitor, key)
}

fn setup_for_search(
    sim_config: SimulationConfig,
    ctx: &HarnessContext,
    prefix: Option<&Trace>,
    cont_seed: u64,
) -> (Simulation, Arc<Mutex<Monitor>>) {
    let (sim, monitor, _key) = build_sim(sim_config, ctx, prefix, cont_seed, false);
    (sim, monitor)
}

fn main() {
    let mut cfg = SplittingConfig::default();
    cfg.levels = vec![100.0, 200.0, 300.0];
    cfg.branch_factor = 2;
    cfg.max_particles_per_level = 16;
    cfg.end_time = SimTime::from_secs(150);
    cfg.install_tokio = false;

    let end_time = cfg.end_time;
    let found = find_with_splitting(
        cfg.clone(),
        SimulationConfig { seed: 123 },
        "retry_storm".to_string(),
        setup_for_search,
    )
    .expect("splitting should run");

    match found {
        Some(found) => {
            println!(
                "found metastable trace: metastable={} recovered={} score={:.2} last_D={:?} trace_events={}",
                found.status.metastable,
                found.status.recovered,
                found.status.score,
                found.status.last_distance,
                found.trace.events.len(),
            );

            // Replay and print a trajectory similar to `metastable_retry_storm`.
            let recorder = Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
                seed: 123,
                scenario: "retry_storm_replay".to_string(),
            })));
            let ctx = HarnessContext::new(recorder);

            let (mut sim, _monitor, key) = build_sim(
                SimulationConfig { seed: 123 },
                &ctx,
                Some(&found.trace),
                0,
                true,
            );

            sim.execute(Executor::timed(end_time));

            let sim_state: Mm1RetryStorm = sim.remove_component(key).expect("component exists");

            let avg_latency = if sim_state.completed_in_time == 0 {
                None
            } else {
                Some(sim_state.total_latency_in_time / sim_state.completed_in_time as u32)
            };

            let status = {
                let mut m = sim_state.monitor.lock().unwrap();
                m.flush_up_to(end_time);
                m.status()
            };

            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "\n== Replay Summary ==");
            let _ = writeln!(out, "end_time={:?}", end_time.as_duration());
            let _ = writeln!(out, "external_arrivals={}", sim_state.external_arrivals);
            let _ = writeln!(out, "retry_arrivals={}", sim_state.retry_arrivals);
            let _ = writeln!(out, "dropped={}", sim_state.dropped);
            let _ = writeln!(out, "timeouts={}", sim_state.timeouts);
            let _ = writeln!(out, "completed_total={}", sim_state.completed_total);
            let _ = writeln!(out, "completed_in_time={}", sim_state.completed_in_time);
            let _ = writeln!(out, "completed_late={}", sim_state.completed_late);
            if let Some(lat) = avg_latency {
                let _ = writeln!(out, "avg_latency_success={:?}", lat);
            } else {
                let _ = writeln!(out, "avg_latency_success=N/A");
            }

            let _ = writeln!(out, "monitor_windows_seen={}", status.windows_seen);
            let _ = writeln!(out, "monitor_recovered={}", status.recovered);
            let _ = writeln!(out, "monitor_metastable={}", status.metastable);
            if let Some(dt) = status.recovery_time {
                let _ = writeln!(out, "monitor_recovery_time={:?}", dt);
            } else {
                let _ = writeln!(out, "monitor_recovery_time=N/A");
            }
            if let Some(d) = status.last_distance {
                let _ = writeln!(out, "monitor_last_distance={:.2}", d);
            } else {
                let _ = writeln!(out, "monitor_last_distance=N/A");
            }
            let _ = writeln!(out, "monitor_score={:.2}", status.score);
        }
        None => {
            println!("no metastable trace found within limits");
        }
    }
}
