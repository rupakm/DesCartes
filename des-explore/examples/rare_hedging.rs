//! Example demonstrating rare metastability in hedged request systems.
//!
//! This example shows how splitting can efficiently find metastability
//! in systems with request hedging where occasional self-inflicted overload
//! creates sustained duplicate request storms. Run with:
//!
//!     cargo run --example rare_hedging

use std::collections::{HashMap, VecDeque};
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

// Model parameters (same as test)
const BASE_ARRIVAL_RATE: f64 = 8.0;
const SPIKE_ARRIVAL_RATE: f64 = 12.0;
const SERVICE_RATE: f64 = 12.0;
const QUEUE_CAPACITY: usize = 30;
const QUEUE_ID: QueueId = QueueId(0);
const HEDGE_TIMEOUT: Duration = Duration::from_millis(1200);
const SPIKE_START_TIME_SECS: f64 = 10.0;
const SPIKE_END_TIME_SECS: f64 = 11.0;
const SIMULATION_END_TIME_SECS: f64 = 180.0;

// Events (same as test)
#[derive(Debug, Clone)]
enum HedgingEvent {
    ExternalArrival { logical_id: u64 },
    ServiceComplete { attempt_id: u64 },
    HedgeTimeout { logical_id: u64, attempt_num: usize },
    CancelAttempt { attempt_id: u64 },
}

#[derive(Debug, Clone)]
struct LogicalRequest {
    id: u64,
    arrival_time: SimTime,
    completed: bool,
    attempt_count: usize,
    pending_attempts: Vec<u64>,
}

#[derive(Debug, Clone)]
struct AttemptState {
    attempt_id: u64,
    logical_id: u64,
    attempt_num: usize,
    enqueued_at: SimTime,
}

struct HedgingComponent {
    base_arrivals: PoissonArrivals,
    spike_arrivals: PoissonArrivals,
    service: ExponentialDistribution,
    queue: VecDeque<AttemptState>,
    active_count: usize,
    next_logical_id: u64,
    next_attempt_id: u64,
    logical_requests: HashMap<u64, LogicalRequest>,
    monitor: Arc<Mutex<Monitor>>,
}

impl HedgingComponent {
    fn new(
        base_arrivals: PoissonArrivals,
        spike_arrivals: PoissonArrivals,
        service: ExponentialDistribution,
        monitor: Arc<Mutex<Monitor>>,
    ) -> Self {
        Self {
            base_arrivals,
            spike_arrivals,
            service,
            queue: VecDeque::new(),
            active_count: 0,
            next_logical_id: 1,
            next_attempt_id: 1,
            logical_requests: HashMap::new(),
            monitor,
        }
    }

    fn is_in_spike(&self, now: SimTime) -> bool {
        let now_secs = now.as_duration().as_secs_f64();
        now_secs >= SPIKE_START_TIME_SECS && now_secs < SPIKE_END_TIME_SECS
    }

    fn schedule_next_arrival(&mut self, scheduler: &mut Scheduler, self_key: Key<HedgingEvent>) {
        let now = scheduler.time();
        let use_spike = self.is_in_spike(now);

        let dt = if use_spike {
            self.spike_arrivals.next_arrival_time()
        } else {
            self.base_arrivals.next_arrival_time()
        };

        scheduler.schedule(
            now + dt,
            self_key,
            HedgingEvent::ExternalArrival {
                logical_id: self.next_logical_id,
            },
        );
        self.next_logical_id += 1;
    }

    fn process_external_arrival(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<HedgingEvent>,
        logical_id: u64,
        now: SimTime,
    ) {
        let logical_request = LogicalRequest {
            id: logical_id,
            arrival_time: now,
            completed: false,
            attempt_count: 0,
            pending_attempts: Vec::new(),
        };
        self.logical_requests.insert(logical_id, logical_request);

        self.send_attempt(scheduler, self_key, logical_id, 1, now);

        scheduler.schedule(
            now + HEDGE_TIMEOUT,
            self_key,
            HedgingEvent::HedgeTimeout {
                logical_id,
                attempt_num: 1,
            },
        );

        self.schedule_next_arrival(scheduler, self_key);
    }

    fn send_attempt(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<HedgingEvent>,
        logical_id: u64,
        attempt_num: usize,
        now: SimTime,
    ) {
        let attempt_id = self.next_attempt_id;
        self.next_attempt_id += 1;

        let attempt = AttemptState {
            attempt_id,
            logical_id,
            attempt_num,
            enqueued_at: now,
        };

        if let Some(logical) = self.logical_requests.get_mut(&logical_id) {
            logical.attempt_count += 1;
            logical.pending_attempts.push(attempt_id);
        }

        if self.queue.len() < QUEUE_CAPACITY {
            self.queue.push_back(attempt);

            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_queue_len(now, QUEUE_ID, self.queue.len() as u64);
                if attempt_num > 1 {
                    monitor.observe_retry(now);
                }
            }

            self.try_start_service(scheduler, self_key, now);
        } else {
            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_drop(now);
        }
    }

    fn try_start_service(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<HedgingEvent>,
        now: SimTime,
    ) {
        if self.active_count == 0 && !self.queue.is_empty() {
            let attempt = self.queue.pop_front().unwrap();
            self.active_count = 1;

            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_queue_len(now, QUEUE_ID, self.queue.len() as u64);
            }

            let service_time = self.service.sample();
            scheduler.schedule(
                now + service_time,
                self_key,
                HedgingEvent::ServiceComplete {
                    attempt_id: attempt.attempt_id,
                },
            );
        }
    }

    fn process_service_complete(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<HedgingEvent>,
        attempt_id: u64,
        now: SimTime,
    ) {
        self.active_count = 0;

        {
            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_queue_len(now, QUEUE_ID, self.queue.len() as u64);
        }

        if let Some((logical_id, attempt_num)) = self.find_attempt(attempt_id) {
            if let Some(logical) = self.logical_requests.get_mut(&logical_id) {
                if !logical.completed {
                    logical.completed = true;

                    let latency = now - logical.arrival_time;
                    let mut monitor = self.monitor.lock().unwrap();
                    monitor.observe_complete(now, latency, true);

                    for &pending_attempt_id in &logical.pending_attempts {
                        if pending_attempt_id != attempt_id {
                            scheduler.schedule_now(
                                self_key,
                                HedgingEvent::CancelAttempt {
                                    attempt_id: pending_attempt_id,
                                },
                            );
                        }
                    }
                }
            }
        }

        self.try_start_service(scheduler, self_key, now);
    }

    fn process_hedge_timeout(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<HedgingEvent>,
        logical_id: u64,
        attempt_num: usize,
        now: SimTime,
    ) {
        if let Some(logical) = self.logical_requests.get(&logical_id) {
            if !logical.completed {
                self.send_attempt(scheduler, self_key, logical_id, attempt_num + 1, now);

                let next_hedge_delay = HEDGE_TIMEOUT * 2_u32.pow((attempt_num - 1) as u32);
                scheduler.schedule(
                    now + next_hedge_delay,
                    self_key,
                    HedgingEvent::HedgeTimeout {
                        logical_id,
                        attempt_num: attempt_num + 1,
                    },
                );
            }
        }
    }

    fn process_cancel_attempt(
        &mut self,
        _scheduler: &mut Scheduler,
        _self_key: Key<HedgingEvent>,
        attempt_id: u64,
        _now: SimTime,
    ) {
        self.queue
            .retain(|attempt| attempt.attempt_id != attempt_id);

        if let Some((logical_id, _)) = self.find_attempt(attempt_id) {
            if let Some(logical) = self.logical_requests.get_mut(&logical_id) {
                logical.pending_attempts.retain(|&id| id != attempt_id);
            }
        }
    }

    fn find_attempt(&self, attempt_id: u64) -> Option<(u64, usize)> {
        for attempt in &self.queue {
            if attempt.attempt_id == attempt_id {
                return Some((attempt.logical_id, attempt.attempt_num));
            }
        }
        None
    }
}

impl Component for HedgingComponent {
    type Event = HedgingEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        let now = scheduler.time();

        match event {
            HedgingEvent::ExternalArrival { logical_id } => {
                self.process_external_arrival(scheduler, self_id, *logical_id, now);
            }
            HedgingEvent::ServiceComplete { attempt_id } => {
                self.process_service_complete(scheduler, self_id, *attempt_id, now);
            }
            HedgingEvent::HedgeTimeout {
                logical_id,
                attempt_num,
            } => {
                self.process_hedge_timeout(scheduler, self_id, *logical_id, *attempt_num, now);
            }
            HedgingEvent::CancelAttempt { attempt_id } => {
                self.process_cancel_attempt(scheduler, self_id, *attempt_id, now);
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
    monitor_cfg.baseline_warmup_windows = 15;
    monitor_cfg.recovery_hold_windows = 3;
    monitor_cfg.baseline_epsilon = 8.0;
    monitor_cfg.metastable_persist_windows = 20;
    monitor_cfg.recovery_time_limit = Some(Duration::from_secs(120));
    monitor_cfg.score_weights = ScoreWeights {
        queue_mean: 2.0,
        retry_amplification: 3.0,
        timeout_rate_rps: 1.0,
        drop_rate_rps: 1.0,
        distance: 1.0,
    };

    let mut monitor = Monitor::new(monitor_cfg, SimTime::zero());
    monitor.mark_post_spike_start(SimTime::from_millis((SPIKE_END_TIME_SECS * 1000.0) as u64));
    let monitor = Arc::new(Mutex::new(monitor));

    let provider = ctx.shared_branching_provider(prefix, cont_seed);

    let base_arrivals = PoissonArrivals::from_config(&sim_config, BASE_ARRIVAL_RATE)
        .with_provider(Box::new(provider.clone()), draw_site!("base_arrivals"));
    let spike_arrivals = PoissonArrivals::from_config(&sim_config, SPIKE_ARRIVAL_RATE)
        .with_provider(Box::new(provider.clone()), draw_site!("spike_arrivals"));
    let service = ExponentialDistribution::from_config(&sim_config, SERVICE_RATE)
        .with_provider(Box::new(provider), draw_site!("service"));

    let mut sim = Simulation::new(sim_config);
    let component = HedgingComponent::new(base_arrivals, spike_arrivals, service, monitor.clone());
    let component_key = sim.add_component(component);

    sim.schedule(
        SimTime::zero(),
        component_key,
        HedgingEvent::ExternalArrival { logical_id: 1 },
    );

    (sim, monitor)
}

fn main() {
    println!("Rare Hedged Request Storm Example");
    println!("==================================");
    println!();
    println!(
        "Model: Single server (μ={:.1}) with hedging - duplicates on {}ms delay",
        SERVICE_RATE,
        HEDGE_TIMEOUT.as_millis()
    );
    println!(
        "       Poisson arrivals (λ={:.1} base, {:.1} spike from {:.1}s to {:.1}s)",
        BASE_ARRIVAL_RATE, SPIKE_ARRIVAL_RATE, SPIKE_START_TIME_SECS, SPIKE_END_TIME_SECS
    );
    println!(
        "       Bounded queue (capacity={}), winner-takes-first completion",
        QUEUE_CAPACITY
    );
    println!();
    println!("Metastability occurs when hedging creates self-inflicted overload:");
    println!("burst → high latency → hedges → duplicate load → higher latency → more hedges");
    println!();

    // Phase 1: Quick naïve check
    println!("Phase 1: Naïve Search (5 seeds, 60s each)");
    println!("------------------------------------------");

    let ctx = HarnessContext::new(Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
        seed: 999,
        scenario: "naive_hedging_check".to_string(),
    }))));

    let mut naive_metastable = 0;
    for seed in 300..305 {
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
        levels: vec![3.5, 14.0, 35.0, 85.0],
        branch_factor: 5,
        max_particles_per_level: 25,
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
        "rare_hedging_example".to_string(),
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
    println!("in hedged request systems where self-inflicted overload prevents recovery.");
}
