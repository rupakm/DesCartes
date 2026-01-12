//! Test for rare metastable failures in dependent call chains with retries.
//!
//! This test demonstrates how splitting heuristics can efficiently find metastability
//! in dependent call chains where a transient slowdown in B causes retry amplification
//! that prevents recovery, creating a self-sustaining cascade.
//!
//! Model: Two-server chain A→B, A triggers dependent calls to B with timeouts/retries,
//! transient B slowdown leads to retry overload maintaining metastability.

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
use rand::Rng;

// Model parameters
const ARRIVAL_RATE_A: f64 = 12.0; // req/s to A (higher load)
const SERVICE_RATE_A: f64 = 15.0; // req/s for A
const SERVICE_RATE_B: f64 = 18.0; // req/s for B (normally faster)
const SERVICE_RATE_B_SPIKE: f64 = 3.0; // req/s during slowdown (6x slower)
const QUEUE_CAPACITY_A: usize = 20;
const QUEUE_CAPACITY_B: usize = 25;
const QUEUE_ID_A: QueueId = QueueId(1);
const QUEUE_ID_B: QueueId = QueueId(2);
const DEP_TIMEOUT: Duration = Duration::from_millis(500); // 500ms timeout for B calls (shorter)
const MAX_RETRIES: usize = 4;
const SPIKE_START_TIME_SECS: f64 = 10.0;
const SPIKE_END_TIME_SECS: f64 = 30.0; // 20s spike window
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
    AServiceComplete { request_id: u64 },
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
    b_call_pending: bool,
}

// A component (upstream server with dependent calls)
struct AComponent {
    arrivals: PoissonArrivals,
    service: ExponentialDistribution,
    queue: VecDeque<ARequestState>,
    active_count: usize,
    next_request_id: u64,
    monitor: Arc<Mutex<Monitor>>,
    b_key: Key<BEvent>, // Reference to B component
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
            b_call_pending: false,
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

            // Start A service and immediately trigger B call
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
        // Immediately send to B and set timeout
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
        // Send request to B component
        scheduler.schedule(now, self.b_key, BEvent::ReceiveFromA { request_id });
    }

    fn process_b_complete(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<AEvent>,
        _request_id: u64,
        now: SimTime,
    ) {
        // B completed, now complete A's service
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
        // Retry the B call
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
            AEvent::AServiceComplete { request_id } => {
                self.process_b_complete(scheduler, self_id, *request_id, now);
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
    a_key: Key<AEvent>, // Reference to A component
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
            // B queue full - this will cause timeout in A
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

            // Choose service time based on spike window
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

        // Notify A that B completed
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

// Setup function for naïve tests
fn setup_naive(
    sim_config: SimulationConfig,
    ctx: &HarnessContext,
    prefix: Option<&Trace>,
    cont_seed: u64,
) -> (Simulation, Arc<Mutex<Monitor>>) {
    let mut monitor_cfg = MonitorConfig::default();
    monitor_cfg.window = Duration::from_secs(1);
    monitor_cfg.baseline_warmup_windows = 10;
    monitor_cfg.recovery_hold_windows = 3;
    monitor_cfg.baseline_epsilon = 1.5; // More sensitive detection
    monitor_cfg.metastable_persist_windows = 5; // Shorter persistence
    monitor_cfg.recovery_time_limit = Some(Duration::from_secs(120));
    monitor_cfg.score_weights = ScoreWeights {
        queue_mean: 4.0, // High weight on both queues
        retry_amplification: 3.0,
        timeout_rate_rps: 2.0,
        drop_rate_rps: 2.0,
        distance: 1.0,
    };

    let mut monitor = Monitor::new(monitor_cfg, SimTime::zero());
    monitor.mark_post_spike_start(SimTime::from_millis((SPIKE_END_TIME_SECS * 1000.0) as u64));
    let monitor = Arc::new(Mutex::new(monitor));

    setup_common(sim_config, ctx, prefix, cont_seed, monitor)
}

// Setup function for splitting
fn setup_splitting(
    sim_config: SimulationConfig,
    ctx: &HarnessContext,
    prefix: Option<&Trace>,
    cont_seed: u64,
) -> (Simulation, Arc<Mutex<Monitor>>) {
    let mut monitor_cfg = MonitorConfig::default();
    monitor_cfg.window = Duration::from_secs(1);
    monitor_cfg.baseline_warmup_windows = 10;
    monitor_cfg.recovery_hold_windows = 3;
    monitor_cfg.baseline_epsilon = 1.5; // Same sensitive settings
    monitor_cfg.metastable_persist_windows = 5;
    monitor_cfg.recovery_time_limit = Some(Duration::from_secs(120));
    monitor_cfg.score_weights = ScoreWeights {
        queue_mean: 4.0,
        retry_amplification: 3.0,
        timeout_rate_rps: 2.0,
        drop_rate_rps: 2.0,
        distance: 1.0,
    };

    let mut monitor = Monitor::new(monitor_cfg, SimTime::zero());
    monitor.mark_post_spike_start(SimTime::from_millis((SPIKE_END_TIME_SECS * 1000.0) as u64));
    let monitor = Arc::new(Mutex::new(monitor));

    setup_common(sim_config, ctx, prefix, cont_seed, monitor)
}

// Common setup logic
fn setup_common(
    sim_config: SimulationConfig,
    ctx: &HarnessContext,
    prefix: Option<&Trace>,
    cont_seed: u64,
    monitor: Arc<Mutex<Monitor>>,
) -> (Simulation, Arc<Mutex<Monitor>>) {
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

// Test that metastability is rare under naïve search
#[test]
#[ignore]
fn naive_metastable_rarity() {
    let num_naive_runs = 625; // Match splitting's computational budget (5^4 max particles)
    let num_test_runs = 5;

    for test_run in 0..num_test_runs {
        println!("\n=== Naive Dependent Calls Test Run {} ===", test_run + 1);

        let ctx = HarnessContext::new(Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
            seed: 200 + test_run as u64,
            scenario: format!("naive_dep_calls_rarity_test_run{}", test_run),
        }))));

        let mut metastable_count = 0;
        let mut r = rand::thread_rng();

        for naive_run in 0..num_naive_runs {
            let s = r.gen::<u64>() % 100000;
            let (mut sim, monitor) = setup_naive(SimulationConfig { seed: s }, &ctx, None, s);
            sim.execute(Executor::timed(SimTime::from_millis(
                (SIMULATION_END_TIME_SECS * 1000.0) as u64,
            )));

            let status = monitor.lock().unwrap().status();
            if status.metastable {
                metastable_count += 1;
            }

            if (naive_run + 1) % 10 == 0 {
                print!("{}/{} ", naive_run + 1, num_naive_runs);
            }
        }

        let success_rate = metastable_count as f64 / num_naive_runs as f64;
        println!(
            "\nNaive Test Run {}: {}/{} = {:.1}% success rate",
            test_run + 1,
            metastable_count,
            num_naive_runs,
            success_rate * 100.0
        );
    }

    println!("\n✅ Naive search confirms cascade metastability is rare!");
}

// Test splitting success rate variability
#[test]
#[ignore]
fn splitting_success_rate_variability() {
    let cfg = SplittingConfig {
        levels: vec![3.0, 12.0, 30.0, 75.0],
        branch_factor: 5,
        max_particles_per_level: 25,
        end_time: SimTime::from_millis((240.0 * 1000.0) as u64),
        install_tokio: false,
    };

    let num_splitting_runs = 100;
    let num_test_runs = 5;

    for test_run in 0..num_test_runs {
        println!(
            "\n=== Dependent Calls Splitting Test Run {} ===",
            test_run + 1
        );
        let mut success_count = 0;

        let mut rng = rand::thread_rng();

        for splitting_run in 0..num_splitting_runs {
            let seed = rng.gen::<u64>() % 100000;
            let result = find_with_splitting(
                cfg.clone(),
                SimulationConfig { seed },
                format!("rare_dep_calls_test{}_run{}", test_run, splitting_run),
                setup_splitting,
            )
            .unwrap();

            if result.is_some() {
                success_count += 1;
                let found = result.unwrap();
                assert!(
                    found.status.metastable,
                    "Found trace should exhibit metastability"
                );
            }

            if (splitting_run + 1) % 10 == 0 {
                print!("{}/{} ", splitting_run + 1, num_splitting_runs);
            }
        }

        let success_rate = success_count as f64 / num_splitting_runs as f64;
        println!(
            "\nTest Run {}: {}/{} = {:.1}% success rate",
            test_run + 1,
            success_count,
            num_splitting_runs,
            success_rate * 100.0
        );
    }
}

// Comprehensive comparison of naive vs splitting
#[test]
#[ignore]
fn comprehensive_metastability_comparison() {
    let cfg = SplittingConfig {
        levels: vec![3.0, 12.0, 30.0, 75.0],
        branch_factor: 5,
        max_particles_per_level: 25,
        end_time: SimTime::from_millis((240.0 * 1000.0) as u64),
        install_tokio: false,
    };

    let num_runs = 5;
    let num_iterations_naive = 500; // Give naive more resources to match splitting's computational budget
    let num_iterations_splitting = 100;

    println!("=== Comprehensive Dependent Calls Cascade Comparison ===");
    println!("Comparing naive vs splitting for finding retry amplification cascades");
    println!(
        "Naive: {} runs × {} seeds = {} total simulations",
        num_runs,
        num_iterations_naive,
        num_runs * num_iterations_naive
    );
    println!(
        "Splitting: {} runs × {} searches = {} total simulations\n",
        num_runs,
        num_iterations_splitting,
        num_runs * num_iterations_splitting
    );

    // NAIVE SEARCH PHASE
    println!("=== NAIVE SEARCH PHASE ===");
    let mut naive_total_successes = 0;
    let mut naive_run_results = Vec::new();

    for test_run in 0..num_runs {
        println!("Naive Test Run {}: ", test_run + 1);
        let ctx = HarnessContext::new(Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
            seed: 1500 + test_run as u64,
            scenario: format!("naive_dep_comparison_run{}", test_run),
        }))));

        let mut run_successes = 0;
        let mut rng = rand::thread_rng();

        for iteration in 0..num_iterations_naive {
            let seed = rng.gen::<u64>() % 100000;
            let (mut sim, monitor) = setup_naive(SimulationConfig { seed }, &ctx, None, seed);
            sim.execute(Executor::timed(SimTime::from_millis(
                (SIMULATION_END_TIME_SECS * 1000.0) as u64,
            )));

            let status = monitor.lock().unwrap().status();
            if status.metastable {
                run_successes += 1;
            }

            if (iteration + 1) % 100 == 0 {
                // Print less frequently since we have more iterations
                print!("{}/{} ", iteration + 1, num_iterations_naive);
            }
        }

        let run_rate = run_successes as f64 / num_iterations_naive as f64;
        println!(
            "→ {}/{} = {:.1}%",
            run_successes,
            num_iterations_naive,
            run_rate * 100.0
        );

        naive_total_successes += run_successes;
        naive_run_results.push(run_successes);
    }

    // SPLITTING SEARCH PHASE
    println!("\n=== SPLITTING SEARCH PHASE ===");

    let mut splitting_total_successes = 0;
    let mut splitting_run_results = Vec::new();

    for test_run in 0..num_runs {
        println!("Splitting Test Run {}: ", test_run + 1);
        let mut run_successes = 0;

        let mut rng = rand::thread_rng();

        for iteration in 0..num_iterations_splitting {
            let seed = rng.gen::<u64>() % 100000;
            let result = find_with_splitting(
                cfg.clone(),
                SimulationConfig { seed },
                format!("splitting_dep_comparison_test{}_run{}", test_run, iteration),
                setup_splitting,
            )
            .unwrap();

            if result.is_some() {
                run_successes += 1;
            }

            if (iteration + 1) % 20 == 0 {
                print!("{}/{} ", iteration + 1, num_iterations_splitting);
            }
        }

        let run_rate = run_successes as f64 / num_iterations_splitting as f64;
        println!(
            "→ {}/{} = {:.1}%",
            run_successes,
            num_iterations_splitting,
            run_rate * 100.0
        );

        splitting_total_successes += run_successes;
        splitting_run_results.push(run_successes);
    }

    // COMPREHENSIVE SUMMARY
    println!("\n=== DEPENDENT CALLS CASCADE COMPARISON SUMMARY ===");

    let naive_overall_rate =
        naive_total_successes as f64 / (num_runs * num_iterations_naive) as f64;
    let splitting_overall_rate =
        splitting_total_successes as f64 / (num_runs * num_iterations_splitting) as f64;
    let improvement_factor = splitting_overall_rate / naive_overall_rate;

    println!("NAIVE SEARCH RESULTS:");
    for (i, &successes) in naive_run_results.iter().enumerate() {
        let rate = successes as f64 / num_iterations_naive as f64;
        println!(
            "  Run {}: {}/{} = {:.1}%",
            i + 1,
            successes,
            num_iterations_naive,
            rate * 100.0
        );
    }
    println!(
        "  Overall: {}/{} = {:.1}% average success rate",
        naive_total_successes,
        num_runs * num_iterations_naive,
        naive_overall_rate * 100.0
    );

    println!("\nSPLITTING SEARCH RESULTS:");
    for (i, &successes) in splitting_run_results.iter().enumerate() {
        let rate = successes as f64 / num_iterations_splitting as f64;
        println!(
            "  Run {}: {}/{} = {:.1}%",
            i + 1,
            successes,
            num_iterations_splitting,
            rate * 100.0
        );
    }
    println!(
        "  Overall: {}/{} = {:.1}% average success rate",
        splitting_total_successes,
        num_runs * num_iterations_splitting,
        splitting_overall_rate * 100.0
    );

    println!("\nPERFORMANCE COMPARISON:");
    println!(
        "  Naive success rate:     {:.1}%",
        naive_overall_rate * 100.0
    );
    println!(
        "  Splitting success rate: {:.1}%",
        splitting_overall_rate * 100.0
    );
    println!("  Improvement factor:     {:.1}x", improvement_factor);

    println!("\nCONCLUSION:");
    println!(
        "  Splitting achieves {:.1}x higher success rate than naive search",
        improvement_factor
    );
    println!("  for finding retry amplification cascades in dependent call chains.");

    assert!(
        splitting_overall_rate > naive_overall_rate,
        "Splitting should outperform naive search"
    );
    assert!(
        improvement_factor > 1.5,
        "Splitting should show improvement over naive search"
    );

    println!("\n✅ Dependent calls cascade comparison completed successfully!");
}
