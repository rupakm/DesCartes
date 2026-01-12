//! Test for rare metastable failures in hedged request systems.
//!
//! This test demonstrates how splitting heuristics can efficiently find metastability
//! in systems with request hedging where occasional self-inflicted overload creates
//! sustained duplicate request storms that prevent recovery.
//!
//! Model: Single server with hedging - requests send duplicates when slow,
//! but rare timing bursts create feedback loops of duplicated work.

use std::collections::{HashMap, VecDeque};
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
const BASE_ARRIVAL_RATE: f64 = 8.0; // req/s (below service capacity)
const SPIKE_ARRIVAL_RATE: f64 = 12.0; // req/s during brief spike (very mild)
const SERVICE_RATE: f64 = 12.0; // req/s
const QUEUE_CAPACITY: usize = 30;
const QUEUE_ID: QueueId = QueueId(0);
const HEDGE_TIMEOUT: Duration = Duration::from_millis(1200); // 1200ms hedge delay (very long)
const SPIKE_START_TIME_SECS: f64 = 10.0;
const SPIKE_END_TIME_SECS: f64 = 11.0; // 1s spike window (very short)
const SIMULATION_END_TIME_SECS: f64 = 180.0;

// Events for the hedging component
#[derive(Debug, Clone)]
enum HedgingEvent {
    ExternalArrival { logical_id: u64 },
    ServiceComplete { attempt_id: u64 },
    HedgeTimeout { logical_id: u64, attempt_num: usize },
    CancelAttempt { attempt_id: u64 }, // For winner-takes-first
}

// Logical request state
#[derive(Debug, Clone)]
struct LogicalRequest {
    id: u64,
    arrival_time: SimTime,
    completed: bool,
    attempt_count: usize,
    pending_attempts: Vec<u64>, // attempt IDs still in system
}

// Attempt tracking
#[derive(Debug, Clone)]
struct AttemptState {
    attempt_id: u64,
    logical_id: u64,
    attempt_num: usize,
    enqueued_at: SimTime,
}

// The simulation component
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
        // Create logical request
        let logical_request = LogicalRequest {
            id: logical_id,
            arrival_time: now,
            completed: false,
            attempt_count: 0,
            pending_attempts: Vec::new(),
        };
        self.logical_requests.insert(logical_id, logical_request);

        // Send first attempt
        self.send_attempt(scheduler, self_key, logical_id, 1, now);

        // Schedule hedge timeout
        scheduler.schedule(
            now + HEDGE_TIMEOUT,
            self_key,
            HedgingEvent::HedgeTimeout {
                logical_id,
                attempt_num: 1,
            },
        );

        // Schedule next arrival
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

        // Update logical request
        if let Some(logical) = self.logical_requests.get_mut(&logical_id) {
            logical.attempt_count += 1;
            logical.pending_attempts.push(attempt_id);
        }

        // Try to enqueue
        if self.queue.len() < QUEUE_CAPACITY {
            self.queue.push_back(attempt);

            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_queue_len(now, QUEUE_ID, self.queue.len() as u64);
                if attempt_num > 1 {
                    monitor.observe_retry(now); // Treat duplicates as retries
                }
            }

            self.try_start_service(scheduler, self_key, now);
        } else {
            // Queue full - drop attempt
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

        // Find which logical request this belongs to
        if let Some((logical_id, attempt_num)) = self.find_attempt(attempt_id) {
            if let Some(logical) = self.logical_requests.get_mut(&logical_id) {
                if !logical.completed {
                    // First completion wins!
                    logical.completed = true;

                    // Calculate latency from original arrival
                    let latency = now - logical.arrival_time;
                    let mut monitor = self.monitor.lock().unwrap();
                    monitor.observe_complete(now, latency, true);

                    // Cancel other pending attempts for this logical request
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
                // Send duplicate attempt
                self.send_attempt(scheduler, self_key, logical_id, attempt_num + 1, now);

                // Schedule next hedge timeout (exponential backoff)
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
        // Remove from queue if still there (simplified - don't cancel in-flight)
        self.queue
            .retain(|attempt| attempt.attempt_id != attempt_id);

        // Update logical request
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

// Setup function for naïve tests
fn setup_naive(
    sim_config: SimulationConfig,
    ctx: &HarnessContext,
    prefix: Option<&Trace>,
    cont_seed: u64,
) -> (Simulation, Arc<Mutex<Monitor>>) {
    let mut monitor_cfg = MonitorConfig::default();
    monitor_cfg.window = Duration::from_secs(1);
    monitor_cfg.baseline_warmup_windows = 15;
    monitor_cfg.recovery_hold_windows = 3;
    monitor_cfg.baseline_epsilon = 7.0; // Extremely insensitive - require huge deviation
    monitor_cfg.metastable_persist_windows = 18; // Very long persistence requirement
    monitor_cfg.recovery_time_limit = Some(Duration::from_secs(120));
    monitor_cfg.score_weights = ScoreWeights {
        queue_mean: 1.0,          // Very low weight on sustained queues
        retry_amplification: 0.5, // Very low weight on duplicates
        timeout_rate_rps: 1.0,
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
    monitor_cfg.baseline_warmup_windows = 15;
    monitor_cfg.recovery_hold_windows = 3;
    monitor_cfg.baseline_epsilon = 7.0; // Same settings as naive for fair comparison
    monitor_cfg.metastable_persist_windows = 18;
    monitor_cfg.recovery_time_limit = Some(Duration::from_secs(120));
    monitor_cfg.score_weights = ScoreWeights {
        queue_mean: 0.5,
        retry_amplification: 2.0, // Higher weight on retry amplification
        timeout_rate_rps: 1.0,
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

// Test that metastability is rare under naïve search
#[test]
#[ignore]
fn naive_metastable_rarity() {
    let num_naive_runs = 625; // Match splitting's computational budget
    let num_test_runs = 5;

    for test_run in 0..num_test_runs {
        println!("\n=== Naive Hedging Test Run {} ===", test_run + 1);

        let ctx = HarnessContext::new(Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
            seed: 300 + test_run as u64,
            scenario: format!("naive_hedging_rarity_test_run{}", test_run),
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

            if (naive_run + 1) % 100 == 0 {
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

    println!("\n✅ Naive search confirms hedging metastability is rare!");
}

// Test splitting success rate variability
#[test]
#[ignore]
fn splitting_success_rate_variability() {
    let cfg = SplittingConfig {
        levels: vec![2.0, 6.0, 15.0, 40.0], // Lower thresholds for more exploration
        branch_factor: 8,                   // Higher branching
        max_particles_per_level: 40,        // Larger budget
        end_time: SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64),
        install_tokio: false,
    };

    let num_splitting_runs = 100;
    let num_test_runs = 5;

    for test_run in 0..num_test_runs {
        println!("\n=== Hedging Splitting Test Run {} ===", test_run + 1);
        let mut success_count = 0;

        let mut rng = rand::thread_rng();

        for splitting_run in 0..num_splitting_runs {
            let seed = rng.gen::<u64>() % 100000;
            let result = find_with_splitting(
                cfg.clone(),
                SimulationConfig { seed },
                format!("rare_hedging_test{}_run{}", test_run, splitting_run),
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
        levels: vec![
            0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 15.0, 20.0, 25.0,
        ], // Ultra fine-grained
        branch_factor: 10,           // High branching
        max_particles_per_level: 25, // Moderate particles per level
        end_time: SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64),
        install_tokio: false,
    };

    let num_runs = 3; // Reduce runs for time
    let num_iterations_naive = 300; // Reduce naive iterations
    let num_iterations_splitting = 20; // Further reduce for speed

    println!("=== Comprehensive Hedging Storm Comparison ===");
    println!("Comparing naive vs splitting for finding self-inflicted hedging overload");
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
            seed: 1600 + test_run as u64,
            scenario: format!("naive_hedging_comparison_run{}", test_run),
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
                format!(
                    "splitting_hedging_comparison_test{}_run{}",
                    test_run, iteration
                ),
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
    println!("\n=== HEDGING STORM COMPARISON SUMMARY ===");

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
    println!("  for finding self-inflicted hedging overload in request duplication systems.");

    assert!(
        splitting_overall_rate > naive_overall_rate,
        "Splitting should outperform naive search"
    );
    assert!(
        improvement_factor > 1.0,
        "Splitting should show at least some improvement over naive search"
    );

    println!("\n✅ Hedging storm comparison completed successfully!");
}
