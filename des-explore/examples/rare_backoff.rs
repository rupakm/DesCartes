//! Example demonstrating rare metastability in timeout+retry systems with jittered backoff.
//!
//! This example shows how splitting can efficiently find metastability that occurs rarely
//! due to jittered backoff usually breaking retry synchronization. Run with:
//!
//!     cargo run --example rare_backoff

use std::sync::{Arc, Mutex};
use std::time::Duration;

use descartes_core::dists::{
    ArrivalPattern, ExponentialDistribution, PoissonArrivals, ServiceTimeDistribution,
};
use descartes_core::draw_site;
use descartes_core::{Component, Executor, Key, Scheduler, SimTime, Simulation, SimulationConfig};
use descartes_explore::{
    harness::HarnessContext,
    monitor::{Monitor, MonitorConfig, QueueId, ScoreWeights},
    splitting::{find_with_splitting, SplittingConfig},
    trace::{Trace, TraceMeta, TraceRecorder},
};
use std::collections::VecDeque;

// Model parameters (same as test)
const ARRIVAL_RATE: f64 = 9.2;
const SERVICE_RATE: f64 = 10.0;
const QUEUE_CAPACITY: usize = 25;
const QUEUE_ID: QueueId = QueueId(0);
const TIMEOUT_DURATION: Duration = Duration::from_secs(1);
const MAX_RETRIES: usize = 2;
const BASE_BACKOFF: f64 = 0.8;
const BACKOFF_JITTER_RATE: f64 = 14.0;
const SPIKE_END_TIME_SECS: f64 = 20.0;
const SIMULATION_END_TIME_SECS: f64 = 180.0;

// Events (same as test)
#[derive(Debug, Clone)]
enum QueueEvent {
    Arrival { request_id: u64 },
    ServiceComplete { request_id: u64 },
    Timeout { request_id: u64, attempt: usize },
    Retry { request_id: u64, attempt: usize },
}

#[derive(Debug, Clone)]
struct RequestState {
    id: u64,
    arrival_time: SimTime,
    attempt: usize,
    timeout_deadline: SimTime,
}

struct RetryQueueComponent {
    arrivals: PoissonArrivals,
    service: ExponentialDistribution,
    backoff_jitter: ExponentialDistribution,
    queue: VecDeque<RequestState>,
    active_count: usize,
    next_request_id: u64,
    monitor: Arc<Mutex<Monitor>>,
}

impl RetryQueueComponent {
    fn new(
        arrivals: PoissonArrivals,
        service: ExponentialDistribution,
        backoff_jitter: ExponentialDistribution,
        monitor: Arc<Mutex<Monitor>>,
    ) -> Self {
        Self {
            arrivals,
            service,
            backoff_jitter,
            queue: VecDeque::new(),
            active_count: 0,
            next_request_id: 1,
            monitor,
        }
    }

    fn schedule_next_arrival(&mut self, scheduler: &mut Scheduler, self_key: Key<QueueEvent>) {
        let dt = self.arrivals.next_arrival_time();
        scheduler.schedule(
            scheduler.time() + dt,
            self_key,
            QueueEvent::Arrival {
                request_id: self.next_request_id,
            },
        );
        self.next_request_id += 1;
    }

    fn process_arrival(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<QueueEvent>,
        request_id: u64,
        now: SimTime,
    ) {
        let request = RequestState {
            id: request_id,
            arrival_time: now,
            attempt: 1,
            timeout_deadline: now + TIMEOUT_DURATION,
        };

        if self.queue.len() < QUEUE_CAPACITY {
            self.queue.push_back(request.clone());
            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_queue_len(now, QUEUE_ID, self.queue.len() as u64);
            }
            self.try_start_service(scheduler, self_key, now);
        } else {
            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_drop(now);
        }

        self.schedule_next_arrival(scheduler, self_key);
    }

    fn try_start_service(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<QueueEvent>,
        now: SimTime,
    ) {
        if self.active_count == 0 && !self.queue.is_empty() {
            let request = self.queue.pop_front().unwrap();
            self.active_count = 1;

            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_queue_len(now, QUEUE_ID, self.queue.len() as u64);
            }

            let service_time = self.service.sample();
            scheduler.schedule(
                now + service_time,
                self_key,
                QueueEvent::ServiceComplete {
                    request_id: request.id,
                },
            );
            scheduler.schedule(
                request.timeout_deadline,
                self_key,
                QueueEvent::Timeout {
                    request_id: request.id,
                    attempt: request.attempt,
                },
            );
        }
    }

    fn process_service_complete(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<QueueEvent>,
        request_id: u64,
        now: SimTime,
    ) {
        self.active_count = 0;
        let latency = now - SimTime::zero(); // Simplified

        {
            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_complete(now, latency, true);
        }

        self.try_start_service(scheduler, self_key, now);
    }

    fn process_timeout(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<QueueEvent>,
        request_id: u64,
        attempt: usize,
        now: SimTime,
    ) {
        if attempt < MAX_RETRIES {
            let base_backoff = BASE_BACKOFF * (2.0_f64).powf((attempt - 1) as f64);
            let jitter_sample = self.backoff_jitter.sample().as_secs_f64();
            let jittered_backoff = (jitter_sample * base_backoff).min(base_backoff);

            let retry_time = now + Duration::from_secs_f64(jittered_backoff);

            scheduler.schedule(
                retry_time,
                self_key,
                QueueEvent::Retry {
                    request_id,
                    attempt: attempt + 1,
                },
            );

            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_timeout(now);
            monitor.observe_retry(now);
        } else {
            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_drop(now);
        }
    }

    fn process_retry(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<QueueEvent>,
        request_id: u64,
        attempt: usize,
        now: SimTime,
    ) {
        let request = RequestState {
            id: request_id,
            arrival_time: now,
            attempt,
            timeout_deadline: now + TIMEOUT_DURATION,
        };

        if self.queue.len() < QUEUE_CAPACITY {
            self.queue.push_back(request.clone());

            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_queue_len(now, QUEUE_ID, self.queue.len() as u64);
            }

            self.try_start_service(scheduler, self_key, now);
        } else {
            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_drop(now);
        }
    }
}

impl Component for RetryQueueComponent {
    type Event = QueueEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        let now = scheduler.time();

        match event {
            QueueEvent::Arrival { request_id } => {
                self.process_arrival(scheduler, self_id, *request_id, now);
            }
            QueueEvent::ServiceComplete { request_id } => {
                self.process_service_complete(scheduler, self_id, *request_id, now);
            }
            QueueEvent::Timeout {
                request_id,
                attempt,
            } => {
                self.process_timeout(scheduler, self_id, *request_id, *attempt, now);
            }
            QueueEvent::Retry {
                request_id,
                attempt,
            } => {
                self.process_retry(scheduler, self_id, *request_id, *attempt, now);
            }
        }

        let mut monitor = self.monitor.lock().unwrap();
        monitor.flush_up_to(now);
    }
}

fn setup(
    sim_config: SimulationConfig,
    ctx: &HarnessContext,
    prefix: Option<&Trace>,
    cont_seed: u64,
) -> (Simulation, Arc<Mutex<Monitor>>) {
    let mut monitor_cfg = MonitorConfig::default();
    monitor_cfg.window = Duration::from_secs(1);
    monitor_cfg.baseline_warmup_windows = 10;
    monitor_cfg.recovery_hold_windows = 3;
    monitor_cfg.baseline_epsilon = 3.0;
    monitor_cfg.metastable_persist_windows = 20;
    monitor_cfg.recovery_time_limit = Some(Duration::from_secs(90));
    monitor_cfg.score_weights = ScoreWeights {
        queue_mean: 2.0,
        retry_amplification: 1.0,
        timeout_rate_rps: 1.0,
        drop_rate_rps: 1.0,
        distance: 1.0,
    };

    let mut monitor = Monitor::new(monitor_cfg, SimTime::zero());
    monitor.mark_post_spike_start(SimTime::from_millis((SPIKE_END_TIME_SECS * 1000.0) as u64));
    let monitor = Arc::new(Mutex::new(monitor));

    let provider = ctx.shared_branching_provider(prefix, cont_seed);

    let arrivals = PoissonArrivals::from_config(&sim_config, ARRIVAL_RATE)
        .with_provider(Box::new(provider.clone()), draw_site!("arrivals"));
    let service = ExponentialDistribution::from_config(&sim_config, SERVICE_RATE)
        .with_provider(Box::new(provider.clone()), draw_site!("service"));
    let backoff_jitter = ExponentialDistribution::from_config(&sim_config, BACKOFF_JITTER_RATE)
        .with_provider(Box::new(provider), draw_site!("backoff_jitter"));

    let mut sim = Simulation::new(sim_config);
    let component = RetryQueueComponent::new(arrivals, service, backoff_jitter, monitor.clone());
    let component_key = sim.add_component(component);

    sim.schedule(
        SimTime::zero(),
        component_key,
        QueueEvent::Arrival { request_id: 1 },
    );

    (sim, monitor)
}

fn main() {
    println!("Rare Metastable Backoff Example");
    println!("================================");
    println!();
    println!(
        "Model: Single server (μ={:.1}) with bounded queue (capacity={})",
        SERVICE_RATE, QUEUE_CAPACITY
    );
    println!(
        "       Poisson arrivals (λ={:.1}), 1s timeout, exponential backoff + jitter",
        ARRIVAL_RATE
    );
    println!(
        "       Max {} retries with base backoff {}s",
        MAX_RETRIES, BASE_BACKOFF
    );
    println!();
    println!("Metastability occurs when jitter fails to desynchronize retries, causing");
    println!("persistent queue instability despite near-capacity load.");
    println!();

    // Phase 1: Quick naïve check
    println!("Phase 1: Naïve Search (5 seeds, 60s each)");
    println!("------------------------------------------");

    let ctx = HarnessContext::new(Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
        seed: 999,
        scenario: "naive_check".to_string(),
    }))));

    let mut naive_metastable = 0;
    for seed in 100..105 {
        let (mut sim, monitor) = setup(SimulationConfig { seed }, &ctx, None, seed);
        sim.execute(Executor::timed(SimTime::from_secs(60)));

        let status = monitor.lock().unwrap().status();
        if status.metastable {
            naive_metastable += 1;
            println!("  Seed {}: METASTABLE", seed);
        } else {
            println!("  Seed {}: stable", seed);
        }
    }
    println!("Found metastability in {}/5 naïve runs", naive_metastable);
    println!();

    // Phase 2: Splitting search
    println!("Phase 2: Splitting Search");
    println!("-------------------------");

    let cfg = SplittingConfig {
        levels: vec![15.0, 45.0, 90.0],
        branch_factor: 3,
        max_particles_per_level: 12,
        end_time: SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64),
        install_tokio: false,
    };

    println!(
        "Searching with {} levels, branching factor {}, max {} particles/level",
        cfg.levels.len(),
        cfg.branch_factor,
        cfg.max_particles_per_level
    );

    let start_time = std::time::Instant::now();
    let result = find_with_splitting(
        cfg,
        SimulationConfig { seed: 42 },
        "rare_backoff_example".to_string(),
        setup,
    )
    .unwrap();
    let search_time = start_time.elapsed();

    match result {
        Some(found) => {
            println!("✅ Found metastable trace!");
            println!("   Trace length: {} events", found.trace.events.len());
            println!("   Search time: {:.2}s", search_time.as_secs_f64());
            println!("   Final score: {:.1}", found.status.score);
            println!("   Metastable: {}", found.status.metastable);

            // To analyze timeline, would need to replay the trace
        }
        None => {
            println!("❌ No metastable trace found within budget");
        }
    }

    println!();
    println!("This demonstrates how splitting efficiently finds rare metastability");
    println!("that occurs when backoff jitter fails to prevent retry synchronization.");
}
