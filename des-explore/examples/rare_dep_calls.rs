//! Example demonstrating rare metastability in dependent call chains with retry amplification.
//!
//! This example shows how splitting can efficiently find metastability in A→B call chains
//! where a transient B slowdown causes retry cascades that prevent recovery. Run with:
//!
//!     cargo run --example rare_dep_calls

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::dists::{
    ArrivalPattern, ExponentialDistribution, PoissonArrivals, ServiceTimeDistribution,
};
use des_core::draw_site;
use des_core::{Component, Executor, Key, Scheduler, SimTime, Simulation, SimulationConfig};
use des_explore::{
    harness::HarnessContext,
    monitor::{Monitor, MonitorConfig, QueueId, ScoreWeights},
    splitting::{find_with_splitting, SplittingConfig},
    trace::{Trace, TraceMeta, TraceRecorder},
};

// Model parameters (same as test)
const ARRIVAL_RATE_A: f64 = 12.0;
const SERVICE_RATE_A: f64 = 15.0;
const SERVICE_RATE_B: f64 = 18.0;
const SERVICE_RATE_B_SPIKE: f64 = 3.0;
const QUEUE_CAPACITY_A: usize = 20;
const QUEUE_CAPACITY_B: usize = 25;
const QUEUE_ID_A: QueueId = QueueId(1);
const QUEUE_ID_B: QueueId = QueueId(2);
const DEP_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_RETRIES: usize = 4;
const SPIKE_START_TIME_SECS: f64 = 10.0;
const SPIKE_END_TIME_SECS: f64 = 30.0;
const SIMULATION_END_TIME_SECS: f64 = 180.0;

// Events for A and B components
#[derive(Debug, Clone)]
enum AEvent {
    Arrival { request_id: u64 },
    StartProcessing { request_id: u64 },
    SendToB { request_id: u64 },
    BComplete { request_id: u64 },
    DepTimeout { request_id: u64, attempt: usize },
    RetryDep { request_id: u64, attempt: usize },
}

#[derive(Debug, Clone)]
enum BEvent {
    ReceiveFromA { request_id: u64 },
    BServiceComplete { request_id: u64 },
}

// Request tracking for A
#[derive(Debug, Clone)]
struct ARequestState {
    id: u64,
    arrival_time: SimTime,
    dep_attempt: usize,
    dep_timeout_deadline: SimTime,
}

// A component (upstream server)
struct AComponent {
    arrivals: PoissonArrivals,
    service: ExponentialDistribution,
    queue: VecDeque<ARequestState>,
    active_count: usize,
    next_request_id: u64,
    monitor: Arc<Mutex<Monitor>>,
    b_key: Key<BEvent>,
}

impl AComponent {
    fn new(
        arrivals: PoissonArrivals,
        service: ExponentialDistribution,
        monitor: Arc<Mutex<Monitor>>,
        b_key: Key<BEvent>,
    ) -> Self {
        Self {
            arrivals,
            service,
            queue: VecDeque::new(),
            active_count: 0,
            next_request_id: 1,
            monitor,
            b_key,
        }
    }

    fn schedule_next_arrival(&mut self, scheduler: &mut Scheduler, self_key: Key<AEvent>) {
        let dt = self.arrivals.next_arrival_time();
        scheduler.schedule(
            scheduler.time() + dt,
            self_key,
            AEvent::Arrival {
                request_id: self.next_request_id,
            },
        );
        self.next_request_id += 1;
    }

    fn process_arrival(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<AEvent>,
        request_id: u64,
        now: SimTime,
    ) {
        let request = ARequestState {
            id: request_id,
            arrival_time: now,
            dep_attempt: 1,
            dep_timeout_deadline: now + DEP_TIMEOUT,
        };

        if self.queue.len() < QUEUE_CAPACITY_A {
            self.queue.push_back(request);

            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_queue_len(now, QUEUE_ID_A, self.queue.len() as u64);
            }

            self.try_start_a_service(scheduler, self_key, now);
        } else {
            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_drop(now);
        }

        self.schedule_next_arrival(scheduler, self_key);
    }

    fn try_start_a_service(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<AEvent>,
        now: SimTime,
    ) {
        if self.active_count == 0 && !self.queue.is_empty() {
            let request = self.queue.pop_front().unwrap();
            self.active_count = 1;

            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_queue_len(now, QUEUE_ID_A, self.queue.len() as u64);
            }

            let service_time = self.service.sample();
            scheduler.schedule(
                now + service_time,
                self_key,
                AEvent::StartProcessing {
                    request_id: request.id,
                },
            );
        }
    }

    fn process_start_processing(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<AEvent>,
        request_id: u64,
        now: SimTime,
    ) {
        scheduler.schedule(now, self_key, AEvent::SendToB { request_id });

        scheduler.schedule(
            now + DEP_TIMEOUT,
            self_key,
            AEvent::DepTimeout {
                request_id,
                attempt: 1,
            },
        );
    }

    fn process_send_to_b(&mut self, scheduler: &mut Scheduler, request_id: u64, now: SimTime) {
        scheduler.schedule(now, self.b_key, BEvent::ReceiveFromA { request_id });
    }

    fn process_b_complete(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<AEvent>,
        request_id: u64,
        now: SimTime,
    ) {
        self.active_count = 0;

        {
            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_queue_len(now, QUEUE_ID_A, self.queue.len() as u64);
        }

        self.try_start_a_service(scheduler, self_key, now);
    }

    fn process_dep_timeout(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<AEvent>,
        request_id: u64,
        attempt: usize,
        now: SimTime,
    ) {
        if attempt < MAX_RETRIES {
            scheduler.schedule(
                now,
                self_key,
                AEvent::RetryDep {
                    request_id,
                    attempt: attempt + 1,
                },
            );

            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_timeout(now);
            monitor.observe_retry(now);
        } else {
            // Max retries exceeded - drop request
            self.active_count = 0;
            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_drop(now);
            }
            self.try_start_a_service(scheduler, self_key, now);
        }
    }

    fn process_retry_dep(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<AEvent>,
        request_id: u64,
        attempt: usize,
        now: SimTime,
    ) {
        scheduler.schedule(now, self_key, AEvent::SendToB { request_id });

        scheduler.schedule(
            now + DEP_TIMEOUT,
            self_key,
            AEvent::DepTimeout {
                request_id,
                attempt,
            },
        );
    }
}

impl Component for AComponent {
    type Event = AEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        let now = scheduler.time();

        match event {
            AEvent::Arrival { request_id } => {
                self.process_arrival(scheduler, self_id, *request_id, now);
            }
            AEvent::StartProcessing { request_id } => {
                self.process_start_processing(scheduler, self_id, *request_id, now);
            }
            AEvent::SendToB { request_id } => {
                self.process_send_to_b(scheduler, *request_id, now);
            }
            AEvent::BComplete { request_id } => {
                self.process_b_complete(scheduler, self_id, *request_id, now);
            }
            AEvent::DepTimeout {
                request_id,
                attempt,
            } => {
                self.process_dep_timeout(scheduler, self_id, *request_id, *attempt, now);
            }
            AEvent::RetryDep {
                request_id,
                attempt,
            } => {
                self.process_retry_dep(scheduler, self_id, *request_id, *attempt, now);
            }
        }

        let mut monitor = self.monitor.lock().unwrap();
        monitor.flush_up_to(now);
    }
}

// B component (downstream server)
struct BComponent {
    service: ExponentialDistribution,
    service_spike: ExponentialDistribution,
    queue: VecDeque<u64>,
    active_count: usize,
    monitor: Arc<Mutex<Monitor>>,
    a_key: Key<AEvent>,
}

impl BComponent {
    fn new(
        service: ExponentialDistribution,
        service_spike: ExponentialDistribution,
        monitor: Arc<Mutex<Monitor>>,
        a_key: Key<AEvent>,
    ) -> Self {
        Self {
            service,
            service_spike,
            queue: VecDeque::new(),
            active_count: 0,
            monitor,
            a_key,
        }
    }

    fn is_in_spike(&self, now: SimTime) -> bool {
        let now_secs = now.as_duration().as_secs_f64();
        now_secs >= SPIKE_START_TIME_SECS && now_secs < SPIKE_END_TIME_SECS
    }

    fn process_receive_from_a(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<BEvent>,
        request_id: u64,
        now: SimTime,
    ) {
        if self.queue.len() < QUEUE_CAPACITY_B {
            self.queue.push_back(request_id);

            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_queue_len(now, QUEUE_ID_B, self.queue.len() as u64);
            }

            self.try_start_b_service(scheduler, self_key, now);
        } else {
            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_drop(now);
        }
    }

    fn try_start_b_service(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<BEvent>,
        now: SimTime,
    ) {
        if self.active_count == 0 && !self.queue.is_empty() {
            let request_id = self.queue.pop_front().unwrap();
            self.active_count = 1;

            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_queue_len(now, QUEUE_ID_B, self.queue.len() as u64);
            }

            let service_time = if self.is_in_spike(now) {
                self.service_spike.sample()
            } else {
                self.service.sample()
            };

            scheduler.schedule(
                now + service_time,
                self_key,
                BEvent::BServiceComplete { request_id },
            );
        }
    }

    fn process_b_service_complete(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<BEvent>,
        request_id: u64,
        now: SimTime,
    ) {
        self.active_count = 0;

        {
            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_queue_len(now, QUEUE_ID_B, self.queue.len() as u64);
        }

        scheduler.schedule(now, self.a_key, AEvent::BComplete { request_id });

        self.try_start_b_service(scheduler, self_key, now);
    }
}

impl Component for BComponent {
    type Event = BEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        let now = scheduler.time();

        match event {
            BEvent::ReceiveFromA { request_id } => {
                self.process_receive_from_a(scheduler, self_id, *request_id, now);
            }
            BEvent::BServiceComplete { request_id } => {
                self.process_b_service_complete(scheduler, self_id, *request_id, now);
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
    monitor_cfg.baseline_epsilon = 1.5;
    monitor_cfg.metastable_persist_windows = 5;
    monitor_cfg.recovery_time_limit = Some(Duration::from_secs(120));
    monitor_cfg.score_weights = ScoreWeights {
        queue_mean: 2.0,
        retry_amplification: 2.0,
        timeout_rate_rps: 2.0,
        drop_rate_rps: 1.0,
        distance: 1.0,
    };

    let mut monitor = Monitor::new(monitor_cfg, SimTime::zero());
    monitor.mark_post_spike_start(SimTime::from_millis((SPIKE_END_TIME_SECS * 1000.0) as u64));
    let monitor = Arc::new(Mutex::new(monitor));

    let provider = ctx.shared_branching_provider(prefix, cont_seed);

    let arrivals_a = PoissonArrivals::from_config(&sim_config, ARRIVAL_RATE_A)
        .with_provider(Box::new(provider.clone()), draw_site!("arrivals_a"));
    let service_a = ExponentialDistribution::from_config(&sim_config, SERVICE_RATE_A)
        .with_provider(Box::new(provider.clone()), draw_site!("service_a"));
    let service_b = ExponentialDistribution::from_config(&sim_config, SERVICE_RATE_B)
        .with_provider(Box::new(provider.clone()), draw_site!("service_b"));
    let service_b_spike = ExponentialDistribution::from_config(&sim_config, SERVICE_RATE_B_SPIKE)
        .with_provider(Box::new(provider), draw_site!("service_b_spike"));

    let mut sim = Simulation::new(sim_config);

    // Add B component first with placeholder key
    let b_component = BComponent::new(service_b, service_b_spike, monitor.clone(), Key::new());
    let b_key = sim.add_component(b_component);

    // Add A component with reference to B
    let a_component = AComponent::new(arrivals_a, service_a, monitor.clone(), b_key);
    let a_key = sim.add_component(a_component);

    // Update B component with reference to A
    if let Some(b_comp) = sim.get_component_mut::<BEvent, BComponent>(b_key) {
        b_comp.a_key = a_key;
    }

    // Schedule first arrival
    sim.schedule(SimTime::zero(), a_key, AEvent::Arrival { request_id: 1 });

    (sim, monitor)
}

fn main() {
    println!("Rare Dependent Calls Cascade Example");
    println!("====================================");
    println!();
    println!(
        "Model: A→B chain, A (μ={:.1}, cap={}) triggers calls to B (μ={:.1}, cap={})",
        SERVICE_RATE_A, QUEUE_CAPACITY_A, SERVICE_RATE_B, QUEUE_CAPACITY_B
    );
    println!(
        "       Poisson arrivals to A (λ={:.1}), {}ms timeout on B calls",
        ARRIVAL_RATE_A,
        DEP_TIMEOUT.as_millis()
    );
    println!(
        "       Max {} retries, B slowdown: {:.1}→{:.1} req/s from {:.1}s to {:.1}s",
        MAX_RETRIES,
        SERVICE_RATE_B,
        SERVICE_RATE_B_SPIKE,
        SPIKE_START_TIME_SECS,
        SPIKE_END_TIME_SECS
    );
    println!();
    println!("Metastability occurs when B slowdown causes timeout cascades that overload");
    println!("B during recovery, creating self-sustaining retry amplification.");
    println!();

    // Phase 1: Quick naïve check
    println!("Phase 1: Naïve Search (5 seeds, 60s each)");
    println!("------------------------------------------");

    let ctx = HarnessContext::new(Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
        seed: 888,
        scenario: "naive_dep_check".to_string(),
    }))));

    let mut naive_metastable = 0;
    for seed in 200..205 {
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
        levels: vec![3.0, 12.0, 30.0, 75.0],
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
        SimulationConfig { seed: 77 },
        "rare_dep_calls_example".to_string(),
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
    println!("in dependent call chains where retry amplification prevents recovery.");
}
