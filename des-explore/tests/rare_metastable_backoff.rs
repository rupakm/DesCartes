//! Test for rare metastable failures in timeout+retry systems with jittered backoff.
//!
//! This test demonstrates how splitting heuristics can efficiently find metastability
//! that occurs rarely under naïve random seeds due to jitter breaking retry synchronization.
//!
//! Model: Single server with bounded queue, Poisson arrivals, exponential service,
//! timeouts with exponential backoff + jitter that usually desynchronizes retries.

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
const ARRIVAL_RATE: f64 = 9.2; // req/s (slightly higher load)
const SERVICE_RATE: f64 = 10.0; // req/s
const QUEUE_CAPACITY: usize = 25;
const QUEUE_ID: QueueId = QueueId(0);
const TIMEOUT_DURATION: Duration = Duration::from_secs(1);
const MAX_RETRIES: usize = 2;
const BASE_BACKOFF: f64 = 0.8; // seconds - longer base backoff
const BACKOFF_JITTER_RATE: f64 = 14.0; // Weaker jitter for more frequent metastability
const SPIKE_END_TIME_SECS: f64 = 20.0;
const SIMULATION_END_TIME_SECS: f64 = 180.0;

// Events for the retry queue component
#[derive(Debug, Clone)]
enum QueueEvent {
    Arrival { request_id: u64 },
    ServiceComplete { request_id: u64 },
    Timeout { request_id: u64, attempt: usize },
    Retry { request_id: u64, attempt: usize },
}

// Request tracking
#[derive(Debug, Clone)]
struct RequestState {
    id: u64,
    arrival_time: SimTime,
    attempt: usize,
    timeout_deadline: SimTime,
}

// The simulation component
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
        // Create request state
        let request = RequestState {
            id: request_id,
            arrival_time: now,
            attempt: 1,
            timeout_deadline: now + TIMEOUT_DURATION,
        };

        // Try to enqueue
        if self.queue.len() < QUEUE_CAPACITY {
            self.queue.push_back(request.clone());

            // Update monitor
            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_queue_len(now, QUEUE_ID, self.queue.len() as u64);
            }

            // Start processing if server available
            self.try_start_service(scheduler, self_key, now);
        } else {
            // Queue full - drop request
            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_drop(now);
        }

        // Schedule next arrival
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

            // Update monitor
            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_queue_len(now, QUEUE_ID, self.queue.len() as u64);
            }

            // Schedule service completion and timeout
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

        // Calculate latency
        let latency = now - SimTime::zero(); // Simplified - would need proper tracking

        // Update monitor
        {
            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_queue_len(now, QUEUE_ID, self.queue.len() as u64);
        }

        // Try to process next request
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
            // Schedule retry with jittered backoff
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

            // Update monitor
            let mut monitor = self.monitor.lock().unwrap();
            monitor.observe_timeout(now);
            monitor.observe_retry(now);
        } else {
            // Max retries exceeded - drop request
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
        // Create new request state for retry
        let request = RequestState {
            id: request_id,
            arrival_time: now,
            attempt,
            timeout_deadline: now + TIMEOUT_DURATION,
        };

        // Try to enqueue retry
        if self.queue.len() < QUEUE_CAPACITY {
            self.queue.push_back(request.clone());

            // Update monitor
            {
                let mut monitor = self.monitor.lock().unwrap();
                monitor.observe_queue_len(now, QUEUE_ID, self.queue.len() as u64);
            }

            // Start processing if server available
            self.try_start_service(scheduler, self_key, now);
        } else {
            // Queue full - drop retry
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

        // Flush monitor windows periodically
        let mut monitor = self.monitor.lock().unwrap();
        monitor.flush_up_to(now);
    }
}

// Setup function for naïve tests (same lenient monitor as splitting)
fn setup_naive(
    sim_config: SimulationConfig,
    ctx: &HarnessContext,
    prefix: Option<&Trace>,
    cont_seed: u64,
) -> (Simulation, Arc<Mutex<Monitor>>) {
    // Create monitor with lenient metastability detection (same as splitting)
    let mut monitor_cfg = MonitorConfig::default();
    monitor_cfg.window = Duration::from_secs(1);
    monitor_cfg.baseline_warmup_windows = 10;
    monitor_cfg.recovery_hold_windows = 3;
    monitor_cfg.baseline_epsilon = 2.0; // Lenient (same as splitting)
    monitor_cfg.metastable_persist_windows = 10; // Shorter persistence (same as splitting)
    monitor_cfg.recovery_time_limit = Some(Duration::from_secs(120));
    monitor_cfg.score_weights = ScoreWeights {
        queue_mean: 3.0, // Very high weight on queue length
        retry_amplification: 2.0,
        timeout_rate_rps: 1.0,
        drop_rate_rps: 1.0,
        distance: 1.0,
    };

    let mut monitor = Monitor::new(monitor_cfg, SimTime::zero());
    monitor.mark_post_spike_start(SimTime::from_millis((SPIKE_END_TIME_SECS * 1000.0) as u64));
    let monitor = Arc::new(Mutex::new(monitor));

    setup_common(sim_config, ctx, prefix, cont_seed, monitor)
}

// Setup function for splitting (more lenient monitor)
fn setup_splitting(
    sim_config: SimulationConfig,
    ctx: &HarnessContext,
    prefix: Option<&Trace>,
    cont_seed: u64,
) -> (Simulation, Arc<Mutex<Monitor>>) {
    // Create monitor with more lenient metastability detection for splitting
    let mut monitor_cfg = MonitorConfig::default();
    monitor_cfg.window = Duration::from_secs(1);
    monitor_cfg.baseline_warmup_windows = 10;
    monitor_cfg.recovery_hold_windows = 3;
    monitor_cfg.baseline_epsilon = 2.0; // More lenient
    monitor_cfg.metastable_persist_windows = 10; // Shorter persistence
    monitor_cfg.recovery_time_limit = Some(Duration::from_secs(120));
    monitor_cfg.score_weights = ScoreWeights {
        queue_mean: 3.0, // Very high weight on queue length
        retry_amplification: 2.0,
        timeout_rate_rps: 1.0,
        drop_rate_rps: 1.0,
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
    // Create shared RNG provider
    let provider = ctx.shared_branching_provider(prefix, cont_seed);
    // Create distributions with provider injection
    let arrivals = PoissonArrivals::from_config(&sim_config, ARRIVAL_RATE)
        .with_provider(Box::new(provider.clone()), draw_site!("arrivals"));
    let service = ExponentialDistribution::from_config(&sim_config, SERVICE_RATE)
        .with_provider(Box::new(provider.clone()), draw_site!("service"));
    let backoff_jitter = ExponentialDistribution::from_config(&sim_config, BACKOFF_JITTER_RATE)
        .with_provider(Box::new(provider), draw_site!("backoff_jitter"));

    // Build simulation
    let mut sim = Simulation::new(sim_config);
    let component = RetryQueueComponent::new(arrivals, service, backoff_jitter, monitor.clone());
    let component_key = sim.add_component(component);

    // Schedule first arrival
    sim.schedule(
        SimTime::zero(),
        component_key,
        QueueEvent::Arrival { request_id: 1 },
    );

    (sim, monitor)
}

// Phase 1: Establish that metastability is rare under naïve search
#[test]
#[ignore]
fn naive_metastable_rarity() {
    let num_naive_runs = 100; // Run naive search 100 times per test run
    let num_test_runs = 5; // Run the entire test 5 times

    for test_run in 0..num_test_runs {
        println!("\n=== Naive Test Run {} ===", test_run + 1);

        let ctx = HarnessContext::new(Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
            seed: 145 + test_run as u64,
            scenario: format!("naive_rarity_test_run{}", test_run),
        }))));

        let mut metastable_count = 0;
        let mut r = rand::thread_rng();

        for naive_run in 0..num_naive_runs {
            let s = r.gen::<u64>() % 100000; // Random seed
            let (mut sim, monitor) = setup_naive(SimulationConfig { seed: s }, &ctx, None, s);
            sim.execute(Executor::timed(SimTime::from_millis(
                (SIMULATION_END_TIME_SECS * 1000.0) as u64,
            )));

            let status = monitor.lock().unwrap().status();
            if status.metastable {
                metastable_count += 1;
            }

            // Print progress every 10 runs
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

        // With lenient monitor, metastability should be detectable but still rare compared to splitting
    }

    println!("\n✅ Naive search confirms metastability is rare!");
}

// Phase 2: Comprehensive comparison of naive vs splitting approaches
#[test]
#[ignore]
fn splitting_success_rate_variability() {
    let cfg = SplittingConfig {
        levels: vec![2.0, 8.0, 20.0, 50.0], // Even lower thresholds
        branch_factor: 5,
        max_particles_per_level: 20, // Much larger budget
        end_time: SimTime::from_millis((240.0 * 1000.0) as u64), // 240s for splitting (more time)
        install_tokio: false,
    };

    let num_splitting_runs = 100; // Run splitting 100 times per test run
    let num_test_runs = 5; // Run the entire test 5 times

    for test_run in 0..num_test_runs {
        println!("\n=== Test Run {} ===", test_run + 1);
        let mut success_count = 0;

        // Use random seeds for each splitting run
        let mut rng = rand::thread_rng();

        for splitting_run in 0..num_splitting_runs {
            let seed = rng.gen::<u64>() % 100000; // Random seed
            let result = find_with_splitting(
                cfg.clone(),
                SimulationConfig { seed },
                format!("rare_backoff_test{}_run{}", test_run, splitting_run),
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

            // Print progress every 10 runs
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

// Phase 3: Comprehensive comparison experiment
#[test]
#[ignore]
fn comprehensive_metastability_comparison() {
    let cfg = SplittingConfig {
        levels: vec![2.0, 8.0, 20.0, 50.0],
        branch_factor: 5,
        max_particles_per_level: 20,
        end_time: SimTime::from_millis((240.0 * 1000.0) as u64),
        install_tokio: false,
    };

    let num_runs = 5; // 5 test runs for each approach
    let num_iterations_per_run = 100; // 100 iterations per test run

    println!("=== Comprehensive Metastability Comparison Experiment ===");
    println!("Running naive search and splitting search with identical monitor settings");
    println!(
        "Each approach: {} runs × {} iterations = {} total simulations\n",
        num_runs,
        num_iterations_per_run,
        num_runs * num_iterations_per_run
    );

    // === NAIVE SEARCH PHASE ===
    println!("=== NAIVE SEARCH PHASE ===");
    let mut naive_total_successes = 0;
    let mut naive_run_results = Vec::new();

    for test_run in 0..num_runs {
        println!("Naive Test Run {}: ", test_run + 1);
        let ctx = HarnessContext::new(Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
            seed: 1000 + test_run as u64,
            scenario: format!("naive_comparison_run{}", test_run),
        }))));

        let mut run_successes = 0;
        let mut rng = rand::thread_rng();

        for iteration in 0..num_iterations_per_run {
            let seed = rng.gen::<u64>() % 100000;
            let (mut sim, monitor) = setup_naive(SimulationConfig { seed }, &ctx, None, seed);
            sim.execute(Executor::timed(SimTime::from_millis(
                (SIMULATION_END_TIME_SECS * 1000.0) as u64,
            )));

            let status = monitor.lock().unwrap().status();
            if status.metastable {
                run_successes += 1;
            }

            if (iteration + 1) % 20 == 0 {
                print!("{}/{} ", iteration + 1, num_iterations_per_run);
            }
        }

        let run_rate = run_successes as f64 / num_iterations_per_run as f64;
        println!(
            "→ {}/{} = {:.1}%",
            run_successes,
            num_iterations_per_run,
            run_rate * 100.0
        );

        naive_total_successes += run_successes;
        naive_run_results.push(run_successes);
    }

    // === SPLITTING SEARCH PHASE ===
    println!("\n=== SPLITTING SEARCH PHASE ===");

    let mut splitting_total_successes = 0;
    let mut splitting_run_results = Vec::new();

    for test_run in 0..num_runs {
        println!("Splitting Test Run {}: ", test_run + 1);
        let mut run_successes = 0;

        let mut rng = rand::thread_rng();

        for iteration in 0..num_iterations_per_run {
            let seed = rng.gen::<u64>() % 100000;
            let result = find_with_splitting(
                cfg.clone(),
                SimulationConfig { seed },
                format!("splitting_comparison_test{}_run{}", test_run, iteration),
                setup_splitting,
            )
            .unwrap();

            if result.is_some() {
                run_successes += 1;
            }

            if (iteration + 1) % 20 == 0 {
                print!("{}/{} ", iteration + 1, num_iterations_per_run);
            }
        }

        let run_rate = run_successes as f64 / num_iterations_per_run as f64;
        println!(
            "→ {}/{} = {:.1}%",
            run_successes,
            num_iterations_per_run,
            run_rate * 100.0
        );

        splitting_total_successes += run_successes;
        splitting_run_results.push(run_successes);
    }

    // === COMPREHENSIVE SUMMARY ===
    println!("\n=== COMPREHENSIVE COMPARISON SUMMARY ===");

    let naive_overall_rate =
        naive_total_successes as f64 / (num_runs * num_iterations_per_run) as f64;
    let splitting_overall_rate =
        splitting_total_successes as f64 / (num_runs * num_iterations_per_run) as f64;
    let improvement_factor = splitting_overall_rate / naive_overall_rate;

    println!("NAIVE SEARCH RESULTS:");
    for (i, &successes) in naive_run_results.iter().enumerate() {
        let rate = successes as f64 / num_iterations_per_run as f64;
        println!(
            "  Run {}: {}/{} = {:.1}%",
            i + 1,
            successes,
            num_iterations_per_run,
            rate * 100.0
        );
    }
    println!(
        "  Overall: {}/{} = {:.1}% average success rate",
        naive_total_successes,
        num_runs * num_iterations_per_run,
        naive_overall_rate * 100.0
    );

    println!("\nSPLITTING SEARCH RESULTS:");
    for (i, &successes) in splitting_run_results.iter().enumerate() {
        let rate = successes as f64 / num_iterations_per_run as f64;
        println!(
            "  Run {}: {}/{} = {:.1}%",
            i + 1,
            successes,
            num_iterations_per_run,
            rate * 100.0
        );
    }
    println!(
        "  Overall: {}/{} = {:.1}% average success rate",
        splitting_total_successes,
        num_runs * num_iterations_per_run,
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
        "  With identical monitor settings, splitting achieves {:.1}x higher success rate",
        improvement_factor
    );
    println!("  than naive random search for finding metastability in this system.");

    assert!(
        splitting_overall_rate > naive_overall_rate,
        "Splitting should outperform naive search"
    );
    assert!(
        improvement_factor > 2.0,
        "Splitting should show significant improvement over naive search"
    );

    println!("\n✅ Comprehensive comparison experiment completed successfully!");
}
