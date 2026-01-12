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
use des_core::{Component, Key, Scheduler, SimTime, Simulation, SimulationConfig};
use des_explore::{
    estimate::{
        estimate_monte_carlo, estimate_with_splitting, MonteCarloConfig, SplittingEstimateConfig,
    },
    harness::HarnessContext,
    monitor::{Monitor, MonitorConfig, QueueId, ScoreWeights},
    trace::Trace,
};

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

// Experiment-style tests using the estimator APIs.

/// Estimates `P[status.metastable]` via naïve Monte Carlo.
///
/// This is an *experiment-style* test (ignored by default) meant to quantify how
/// often the hedging model triggers the terminal predicate under random seeds.
#[test]
#[ignore]
fn naive_metastable_rarity() {
    let end_time = SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64);

    let mc_cfg = MonteCarloConfig {
        trials: 625,
        end_time,
        install_tokio: false,
        confidence: 0.95,
    };

    for run in 0..5u64 {
        let base_seed = 300 + run;
        let est = estimate_monte_carlo(
            mc_cfg.clone(),
            SimulationConfig { seed: base_seed },
            format!("hedging_naive_run{}", run),
            setup_naive,
            |s| s.metastable,
        )
        .expect("monte carlo estimate should succeed");

        println!(
            "naive run {}: p_hat={:.4} ({} / {}), ci=[{:.4}, {:.4}]",
            run + 1,
            est.p_hat,
            est.successes,
            est.trials,
            est.ci_low.unwrap_or(f64::NAN),
            est.ci_high.unwrap_or(f64::NAN)
        );
    }
}

/// Estimates `P[status.metastable]` via multilevel splitting.
///
/// This is an *experiment-style* test (ignored by default) meant to exercise
/// prefix checkpointing + replay.
#[test]
#[ignore]
fn splitting_success_rate_variability() {
    let cfg = SplittingEstimateConfig {
        levels: vec![2.0, 6.0, 15.0, 40.0],
        particles: 64,
        end_time: SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64),
        install_tokio: false,
        confidence: 0.95,
    };

    for run in 0..5u64 {
        let base_seed = 10_000 + run;
        let est = estimate_with_splitting(
            cfg.clone(),
            SimulationConfig { seed: base_seed },
            format!("hedging_splitting_run{}", run),
            setup_splitting,
            |s| s.metastable,
        )
        .expect("splitting estimate should succeed");

        println!(
            "splitting run {}: p_hat={:.6}, ci=[{:.6}, {:.6}], stage_probs={:?}",
            run + 1,
            est.p_hat,
            est.ci_low.unwrap_or(f64::NAN),
            est.ci_high.unwrap_or(f64::NAN),
            est.stage_probs
        );
    }
}

/// Side-by-side Monte Carlo vs splitting probability estimates.
#[test]
#[ignore]
fn comprehensive_metastability_comparison() {
    let end_time = SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64);

    let mc = estimate_monte_carlo(
        MonteCarloConfig {
            trials: 1_000,
            end_time,
            install_tokio: false,
            confidence: 0.95,
        },
        SimulationConfig { seed: 2025 },
        "hedging_monte_carlo".to_string(),
        setup_naive,
        |s| s.metastable,
    )
    .expect("monte carlo estimate should succeed");

    let split = estimate_with_splitting(
        SplittingEstimateConfig {
            levels: vec![2.0, 6.0, 15.0, 40.0],
            particles: 64,
            end_time: SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64),
            install_tokio: false,
            confidence: 0.95,
        },
        SimulationConfig { seed: 2025 },
        "hedging_splitting".to_string(),
        setup_splitting,
        |s| s.metastable,
    )
    .expect("splitting estimate should succeed");

    println!(
        "monte carlo: p_hat={:.6} ({} / {}), ci=[{:.6}, {:.6}]",
        mc.p_hat,
        mc.successes,
        mc.trials,
        mc.ci_low.unwrap_or(f64::NAN),
        mc.ci_high.unwrap_or(f64::NAN)
    );
    println!(
        "splitting:   p_hat={:.6}, ci=[{:.6}, {:.6}], stage_probs={:?}",
        split.p_hat,
        split.ci_low.unwrap_or(f64::NAN),
        split.ci_high.unwrap_or(f64::NAN),
        split.stage_probs
    );
}
