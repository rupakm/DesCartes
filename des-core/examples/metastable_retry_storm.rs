//! Metastable retry storm in an M/M/1 queue.
//!
//! This example models a single-server queue with:
//! - Poisson arrivals (M)
//! - exponential service times (M)
//! - a single server (1)
//! - bounded queue capacity
//! - client-side timeouts that trigger retries (no backoff)
//!
//! Even after an external workload spike ends, retries can keep the effective
//! offered load high by creating additional work for the server (including work
//! that arrives after the client has already timed out).

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::dists::{
    ArrivalPattern, ExponentialDistribution, PoissonArrivals, ServiceTimeDistribution,
};
use des_core::{Component, Executor, Key, SimTime, Simulation, SimulationConfig};
use des_explore::monitor::{Monitor, MonitorConfig, QueueId};

const LAMBDA_BASE_RPS: f64 = 9.3;
const MU_RPS: f64 = 10.0;

const QUEUE_CAPACITY: usize = 100;
const TIMEOUT: Duration = Duration::from_secs(4);
const MAX_RETRIES: u8 = 3;

const SPIKE_START: SimTime = SimTime::from_secs(120);
const SPIKE_END: SimTime = SimTime::from_secs(125);
const LAMBDA_SPIKE_RPS: f64 = 30.0;

const REPORT_INTERVAL: Duration = Duration::from_secs(5);
const MONITOR_WINDOW: Duration = Duration::from_secs(1);
const SIM_END: SimTime = SimTime::from_secs(400);

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
    monitor: Arc<Mutex<Monitor>>,

    // Distributions
    arrivals_base: PoissonArrivals,
    arrivals_spike: PoissonArrivals,
    service: ExponentialDistribution,

    // System state
    queue: VecDeque<u64>,
    server_busy: bool,

    // Attempt bookkeeping
    next_parent_id: u64,
    next_attempt_id: u64,
    attempts: HashMap<u64, Attempt>,

    // Counters
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
    fn new(config: &SimulationConfig) -> Self {
        let mut monitor_cfg = MonitorConfig::default();
        monitor_cfg.window = MONITOR_WINDOW;
        monitor_cfg.baseline_warmup_windows = 10;
        monitor_cfg.recovery_hold_windows = 3;
        monitor_cfg.baseline_epsilon = 3.0;
        monitor_cfg.metastable_persist_windows = 20;
        monitor_cfg.recovery_time_limit = Some(Duration::from_secs(60));

        let mut monitor = Monitor::new(monitor_cfg, SimTime::zero());
        monitor.mark_post_spike_start(SPIKE_END);

        Self {
            monitor: Arc::new(Mutex::new(monitor)),
            arrivals_base: PoissonArrivals::from_config(config, LAMBDA_BASE_RPS),
            arrivals_spike: PoissonArrivals::from_config(config, LAMBDA_SPIKE_RPS),
            service: ExponentialDistribution::from_config(config, MU_RPS),

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
        }
    }

    fn in_spike(now: SimTime) -> bool {
        now >= SPIKE_START && now < SPIKE_END
    }

    fn backlog_len(&self) -> u64 {
        self.queue.len() as u64 + u64::from(self.server_busy)
    }

    fn observe_backlog(&self, now: SimTime) {
        let mut monitor = self.monitor.lock().unwrap();
        monitor.observe_queue_len(now, BACKLOG_QUEUE, self.backlog_len());
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

        // Client-side timeout.
        scheduler.schedule(
            SimTime::from_duration(TIMEOUT),
            self_id,
            Event::Timeout { attempt_id },
        );

        self.maybe_start_service(self_id, scheduler);
    }

    fn report(&self, now: SimTime) {
        let offered = self.external_arrivals + self.retry_arrivals;
        let amp = if self.completed_in_time == 0 {
            f64::INFINITY
        } else {
            self.retry_arrivals as f64 / self.completed_in_time as f64
        };

        let status = {
            let mut monitor = self.monitor.lock().unwrap();
            monitor.flush_up_to(now);
            monitor.status()
        };

        let backlog = self.backlog_len();
        let d = status.last_distance.unwrap_or(-1.0);

        let mut out = std::io::stdout().lock();
        let _ = writeln!(
            out,
            "t={:>6.1}s in_spike={} backlog={:>3} ext={} retry={} offered={} drop={} timeout={} ok={} late={} amp={:.2} D={:.2} rec={} meta={} S={:.2}",
            now.as_duration().as_secs_f64(),
            Self::in_spike(now),
            backlog,
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
                self.report(scheduler.time());
                scheduler.schedule(
                    SimTime::from_duration(REPORT_INTERVAL),
                    self_id,
                    Event::Report,
                );
            }
        }
    }
}

fn main() {
    let mut sim = Simulation::new(SimulationConfig { seed: 13 });

    let key = sim.add_component(Mm1RetryStorm::new(sim.config()));

    // Kick off arrivals and periodic reporting.
    sim.schedule_now(key, Event::ExternalArrival);
    sim.schedule(SimTime::zero(), key, Event::Report);

    sim.execute(Executor::timed(SIM_END));

    let sim_state: Mm1RetryStorm = sim.remove_component(key).expect("component should exist");

    let avg_latency = if sim_state.completed_in_time == 0 {
        None
    } else {
        Some(sim_state.total_latency_in_time / sim_state.completed_in_time as u32)
    };

    let mut out = std::io::stdout().lock();

    let _ = writeln!(out, "\n== Summary ==");
    let _ = writeln!(out, "base_lambda_rps={LAMBDA_BASE_RPS} mu_rps={MU_RPS}");
    let _ = writeln!(
        out,
        "spike=[{:.1}s,{:.1}s) spike_lambda_rps={LAMBDA_SPIKE_RPS}",
        SPIKE_START.as_duration().as_secs_f64(),
        SPIKE_END.as_duration().as_secs_f64()
    );
    let _ = writeln!(
        out,
        "queue_capacity={QUEUE_CAPACITY} timeout={:?} max_retries={MAX_RETRIES}",
        TIMEOUT
    );
    let _ = writeln!(out, "external_arrivals={}", sim_state.external_arrivals);
    let _ = writeln!(out, "retry_arrivals={}", sim_state.retry_arrivals);
    let _ = writeln!(out, "dropped={}", sim_state.dropped);
    let _ = writeln!(out, "timeouts={}", sim_state.timeouts);
    let _ = writeln!(out, "completed_total={}", sim_state.completed_total);
    let _ = writeln!(out, "completed_in_time={}", sim_state.completed_in_time);
    let _ = writeln!(out, "completed_late={}", sim_state.completed_late);
    let _ = writeln!(out, "final_queue_len={}", sim_state.queue.len());
    if let Some(lat) = avg_latency {
        let _ = writeln!(out, "avg_latency_success={:?}", lat);
    } else {
        let _ = writeln!(out, "avg_latency_success=N/A");
    }

    // Flush the monitor and report recovery/metastability signals.
    let status = {
        let mut monitor = sim_state.monitor.lock().unwrap();
        monitor.flush_up_to(SIM_END);
        monitor.status()
    };

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

    let _ = writeln!(
        out,
        "\nInterpretation tip: if `q` stays high and `amp` (retry_arrivals / completed_in_time) \
         stays elevated well after the spike, you are seeing retry-driven metastability."
    );
}
