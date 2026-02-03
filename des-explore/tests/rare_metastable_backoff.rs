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

use descartes_core::dists::{
    ArrivalPattern, ExponentialDistribution, PoissonArrivals, ServiceTimeDistribution,
};
use descartes_core::draw_site;
use descartes_core::{Component, Key, Scheduler, SimTime, Simulation, SimulationConfig};
use descartes_explore::{
    estimate::{
        estimate_monte_carlo, estimate_with_splitting, MonteCarloConfig, SplittingEstimateConfig,
    },
    harness::HarnessContext,
    monitor::{Monitor, MonitorConfig, QueueId, ScoreWeights},
    trace::Trace,
};

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
/// Estimates `P[status.metastable]` via naïve Monte Carlo.
///
/// This is an *experiment-style* test (ignored by default) meant to quantify how
/// often this model triggers the terminal predicate under random seeds.
#[test]
#[ignore]
fn naive_metastable_rarity() {
    let end_time = SimTime::from_millis((SIMULATION_END_TIME_SECS * 1000.0) as u64);

    let mc_cfg = MonteCarloConfig {
        trials: 100,
        end_time,
        install_tokio: false,
        confidence: 0.95,
    };

    for run in 0..5u64 {
        let base_seed = 145 + run;
        let est = estimate_monte_carlo(
            mc_cfg.clone(),
            SimulationConfig { seed: base_seed },
            format!("backoff_naive_run{}", run),
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

// Phase 2: Estimate probability via multilevel splitting
/// Estimates `P[status.metastable]` via multilevel splitting.
///
/// This is an *experiment-style* test (ignored by default) meant to:
/// - exercise checkpointing + replay
/// - provide a rough estimate of a rare-event probability
#[test]
#[ignore]
fn splitting_success_rate_variability() {
    let cfg = SplittingEstimateConfig {
        levels: vec![2.0, 8.0, 20.0, 50.0],
        particles: 64,
        end_time: SimTime::from_millis((240.0 * 1000.0) as u64),
        install_tokio: false,
        confidence: 0.95,
    };

    for run in 0..5u64 {
        let base_seed = 10_000 + run;
        let est = estimate_with_splitting(
            cfg.clone(),
            SimulationConfig { seed: base_seed },
            format!("backoff_splitting_run{}", run),
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

// Phase 3: Side-by-side estimator comparison
/// Compares a Monte Carlo probability estimate to a multilevel splitting estimate.
///
/// This is an *experiment-style* test (ignored by default) and is primarily a
/// readability + API-usage example.
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
        "backoff_monte_carlo".to_string(),
        setup_naive,
        |s| s.metastable,
    )
    .expect("monte carlo estimate should succeed");

    let split = estimate_with_splitting(
        SplittingEstimateConfig {
            levels: vec![2.0, 8.0, 20.0, 50.0],
            particles: 64,
            end_time: SimTime::from_millis((240.0 * 1000.0) as u64),
            install_tokio: false,
            confidence: 0.95,
        },
        SimulationConfig { seed: 2025 },
        "backoff_splitting".to_string(),
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
