//! Comprehensive search for optimal splitting heuristics in hedging scenario

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::dists::{
    ArrivalPattern, ExponentialDistribution, PoissonArrivals, ServiceTimeDistribution,
};
use des_core::draw_site;
use des_core::{Component, Key, Scheduler, SimTime, Simulation, SimulationConfig};
use des_explore::{
    harness::HarnessContext,
    monitor::{Monitor, MonitorConfig, QueueId, ScoreWeights},
    splitting::{find_with_splitting, SplittingConfig},
    trace::{Trace, TraceMeta, TraceRecorder},
};

// Model parameters (same as hedging test)
const BASE_ARRIVAL_RATE: f64 = 8.0;
const SPIKE_ARRIVAL_RATE: f64 = 12.0;
const SERVICE_RATE: f64 = 12.0;
const QUEUE_CAPACITY: usize = 30;
const QUEUE_ID: QueueId = QueueId(0);
const HEDGE_TIMEOUT: Duration = Duration::from_millis(1200);
const SPIKE_START_TIME_SECS: f64 = 10.0;
const SPIKE_END_TIME_SECS: f64 = 11.0;
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
        if let Some((logical_id, _attempt_num)) = self.find_attempt(attempt_id) {
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
// Test configurations for systematic parameter search
struct SplittingTestConfig {
    name: &'static str,
    config: SplittingConfig,
    score_weights: ScoreWeights,
}

/// Experiment-style optimization sweep for splitting parameters.
///
/// Currently ignored: intended for local exploration, not CI.
#[test]
#[ignore]
fn comprehensive_splitting_optimization() {
    // Define test configurations to try
    let test_configs = vec![
        // Current baseline
        SplittingTestConfig {
            name: "baseline",
            config: SplittingConfig {
                levels: vec![2.0, 6.0, 15.0, 40.0],
                branch_factor: 8,
                max_particles_per_level: 40,
                end_time: SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64),
                install_tokio: false,
            },
            score_weights: ScoreWeights {
                queue_mean: 1.0,
                retry_amplification: 0.5,
                timeout_rate_rps: 1.0,
                drop_rate_rps: 2.0,
                distance: 1.0,
            },
        },
        // More granular levels
        SplittingTestConfig {
            name: "fine_grained_levels",
            config: SplittingConfig {
                levels: vec![1.0, 2.0, 3.0, 4.0, 5.0, 7.0, 10.0, 15.0, 20.0, 30.0],
                branch_factor: 8,
                max_particles_per_level: 40,
                end_time: SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64),
                install_tokio: false,
            },
            score_weights: ScoreWeights {
                queue_mean: 1.0,
                retry_amplification: 0.5,
                timeout_rate_rps: 1.0,
                drop_rate_rps: 2.0,
                distance: 1.0,
            },
        },
        // Emphasize retry amplification
        SplittingTestConfig {
            name: "high_retry_weight",
            config: SplittingConfig {
                levels: vec![2.0, 6.0, 15.0, 40.0],
                branch_factor: 8,
                max_particles_per_level: 40,
                end_time: SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64),
                install_tokio: false,
            },
            score_weights: ScoreWeights {
                queue_mean: 0.5,
                retry_amplification: 3.0, // Much higher weight on retries
                timeout_rate_rps: 1.0,
                drop_rate_rps: 2.0,
                distance: 0.5,
            },
        },
        // Exponential level spacing
        SplittingTestConfig {
            name: "exponential_levels",
            config: SplittingConfig {
                levels: vec![1.5, 3.0, 6.0, 12.0, 24.0, 48.0],
                branch_factor: 8,
                max_particles_per_level: 40,
                end_time: SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64),
                install_tokio: false,
            },
            score_weights: ScoreWeights {
                queue_mean: 1.0,
                retry_amplification: 0.5,
                timeout_rate_rps: 1.0,
                drop_rate_rps: 2.0,
                distance: 1.0,
            },
        },
        // Higher branching factor
        SplittingTestConfig {
            name: "high_branching",
            config: SplittingConfig {
                levels: vec![2.0, 6.0, 15.0, 40.0],
                branch_factor: 12,           // Higher branching
                max_particles_per_level: 30, // Slightly lower particles
                end_time: SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64),
                install_tokio: false,
            },
            score_weights: ScoreWeights {
                queue_mean: 1.0,
                retry_amplification: 0.5,
                timeout_rate_rps: 1.0,
                drop_rate_rps: 2.0,
                distance: 1.0,
            },
        },
        // Focus on distance metric
        SplittingTestConfig {
            name: "distance_focused",
            config: SplittingConfig {
                levels: vec![2.0, 6.0, 15.0, 40.0],
                branch_factor: 8,
                max_particles_per_level: 40,
                end_time: SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64),
                install_tokio: false,
            },
            score_weights: ScoreWeights {
                queue_mean: 0.3,
                retry_amplification: 0.3,
                timeout_rate_rps: 0.3,
                drop_rate_rps: 1.0,
                distance: 3.0, // Much higher weight on distance
            },
        },
        // Very fine grained with high branching
        SplittingTestConfig {
            name: "ultra_fine_grained",
            config: SplittingConfig {
                levels: vec![
                    0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 15.0, 20.0, 25.0,
                ],
                branch_factor: 10,
                max_particles_per_level: 25,
                end_time: SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64),
                install_tokio: false,
            },
            score_weights: ScoreWeights {
                queue_mean: 1.0,
                retry_amplification: 2.0, // Higher retry weight
                timeout_rate_rps: 1.0,
                drop_rate_rps: 2.0,
                distance: 1.0,
            },
        },
    ];

    println!("=== Comprehensive Splitting Optimization for Hedging Scenario ===");
    println!(
        "Testing {} different splitting configurations\n",
        test_configs.len()
    );

    // Run a smaller test for each configuration
    let num_test_runs = 2; // Fast for optimization
    let num_splitting_runs = 20; // Fast for optimization

    for test_config in test_configs {
        println!("--- Testing: {} ---", test_config.name);

        let mut total_successes = 0;

        for test_run in 0..num_test_runs {
            let mut run_successes = 0;

            let ctx = HarnessContext::new(Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
                seed: 400 + test_run as u64,
                scenario: format!("{}_run{}", test_config.name, test_run),
            }))));

            for splitting_run in 0..num_splitting_runs {
                let seed = 1000 + test_run as u64 * 100 + splitting_run as u64;

                // Create monitor with custom score weights
                let mut monitor_cfg = MonitorConfig::default();
                monitor_cfg.window = Duration::from_secs(1);
                monitor_cfg.baseline_warmup_windows = 15;
                monitor_cfg.recovery_hold_windows = 3;
                monitor_cfg.baseline_epsilon = 6.0;
                monitor_cfg.metastable_persist_windows = 15;
                monitor_cfg.recovery_time_limit = Some(Duration::from_secs(120));
                monitor_cfg.score_weights = test_config.score_weights.clone();

                let mut monitor = Monitor::new(monitor_cfg, SimTime::zero());
                monitor.mark_post_spike_start(SimTime::from_millis(
                    (SPIKE_END_TIME_SECS * 1000.0) as u64,
                ));

                let result = find_with_splitting(
                    test_config.config.clone(),
                    SimulationConfig { seed },
                    format!("{}_{}_{}", test_config.name, test_run, splitting_run),
                    |sim_config, ctx, prefix, cont_seed| {
                        setup_common_with_weights(
                            sim_config,
                            ctx,
                            prefix,
                            cont_seed,
                            &test_config.score_weights,
                        )
                    },
                )
                .unwrap();

                if result.is_some() {
                    run_successes += 1;
                }
            }

            let run_rate = run_successes as f64 / num_splitting_runs as f64;
            println!(
                "  Run {}: {}/{} = {:.1}%",
                test_run + 1,
                run_successes,
                num_splitting_runs,
                run_rate * 100.0
            );

            total_successes += run_successes;
        }

        let overall_rate = total_successes as f64 / (num_test_runs * num_splitting_runs) as f64;
        println!(
            "  Overall: {}/{} = {:.1}% success rate\n",
            total_successes,
            num_test_runs * num_splitting_runs,
            overall_rate * 100.0
        );
    }

    println!("✅ Comprehensive splitting optimization completed!");
}

// Helper function to create setup with custom score weights
fn setup_common_with_weights(
    sim_config: SimulationConfig,
    ctx: &HarnessContext,
    prefix: Option<&Trace>,
    cont_seed: u64,
    score_weights: &ScoreWeights,
) -> (Simulation, Arc<Mutex<Monitor>>) {
    let provider = ctx.shared_branching_provider(prefix, cont_seed);

    let base_arrivals = PoissonArrivals::from_config(&sim_config, BASE_ARRIVAL_RATE)
        .with_provider(Box::new(provider.clone()), draw_site!("base_arrivals"));
    let spike_arrivals = PoissonArrivals::from_config(&sim_config, SPIKE_ARRIVAL_RATE)
        .with_provider(Box::new(provider.clone()), draw_site!("spike_arrivals"));
    let service = ExponentialDistribution::from_config(&sim_config, SERVICE_RATE)
        .with_provider(Box::new(provider), draw_site!("service"));

    let mut monitor_cfg = MonitorConfig::default();
    monitor_cfg.window = Duration::from_secs(1);
    monitor_cfg.baseline_warmup_windows = 15;
    monitor_cfg.recovery_hold_windows = 3;
    monitor_cfg.baseline_epsilon = 7.0;
    monitor_cfg.metastable_persist_windows = 18;
    monitor_cfg.recovery_time_limit = Some(Duration::from_secs(120));
    monitor_cfg.score_weights = score_weights.clone();

    let mut monitor = Monitor::new(monitor_cfg, SimTime::zero());
    monitor.mark_post_spike_start(SimTime::from_millis((SPIKE_END_TIME_SECS * 1000.0) as u64));
    let monitor = Arc::new(Mutex::new(monitor));

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
