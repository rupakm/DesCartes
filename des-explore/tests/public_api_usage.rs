//! Public API usage patterns for Milestone 6 estimators.
//!
//! This test file is intentionally "example-like": it uses the `descartes_explore::prelude::*`
//! exports and keeps the setup/predicate closures in the shape we expect downstream
//! users to copy.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use descartes_core::dists::{ExponentialDistribution, ServiceTimeDistribution};
use descartes_core::{Component, Execute, Executor, Key, SimTime, Simulation, SimulationConfig};

use descartes_explore::prelude::*;
use descartes_explore::trace::TraceEvent;

const Q: QueueId = QueueId(1);

#[derive(Debug)]
enum Ev {
    Tick,
}

/// Tiny model with a single RNG draw that determines whether a terminal event occurs.
///
/// We use `MonitorStatus.metastable` as a convenient boolean terminal predicate.
struct BernoulliTerminalModel {
    coin: ExponentialDistribution,
    decided: Option<bool>,
    monitor: Arc<Mutex<Monitor>>,
    decide_at: SimTime,
}

impl Component for BernoulliTerminalModel {
    type Event = Ev;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        _event: &Self::Event,
        scheduler: &mut descartes_core::Scheduler,
    ) {
        let now = scheduler.time();

        // Emit completions so the monitor's retry amplification stays finite.
        self.monitor
            .lock()
            .unwrap()
            .observe_complete(now, Duration::from_millis(1), true);

        let terminal = if now >= self.decide_at {
            *self.decided.get_or_insert_with(|| {
                // Exp(1) draw: P[X < ln 2] = 0.5
                self.coin.sample().as_secs_f64() < std::f64::consts::LN_2
            })
        } else {
            false
        };

        let qlen = if terminal { 100 } else { 0 };
        self.monitor.lock().unwrap().observe_queue_len(now, Q, qlen);

        scheduler.schedule(SimTime::from_millis(1), self_id, Ev::Tick);
    }
}

fn setup(
    config: SimulationConfig,
    ctx: &HarnessContext,
    prefix: Option<&Trace>,
    cont_seed: u64,
) -> (Simulation, Arc<Mutex<Monitor>>) {
    let mut sim = Simulation::new(config);

    let mut mcfg = MonitorConfig::default();
    mcfg.window = Duration::from_millis(1);
    mcfg.baseline_warmup_windows = 2;
    mcfg.recovery_hold_windows = 1;
    mcfg.baseline_epsilon = 2.0;
    mcfg.metastable_persist_windows = 2;
    mcfg.recovery_time_limit = None;

    let monitor = Arc::new(Mutex::new(Monitor::new(mcfg, SimTime::zero())));
    monitor
        .lock()
        .unwrap()
        .mark_post_spike_start(SimTime::zero());

    let provider = ctx.branching_provider(prefix, cont_seed);
    let coin = ExponentialDistribution::from_config(sim.config(), 1.0)
        .with_provider(provider, descartes_core::draw_site!("coin"));

    let key = sim.add_component(BernoulliTerminalModel {
        coin,
        decided: None,
        monitor: monitor.clone(),
        decide_at: SimTime::from_millis(2),
    });

    sim.schedule_now(key, Ev::Tick);

    (sim, monitor)
}

/// Demonstrates naïve Monte Carlo estimation via the public prelude.
#[test]
fn public_monte_carlo_usage_pattern() {
    let cfg = MonteCarloConfig {
        trials: 100,
        end_time: SimTime::from_millis(10),
        install_tokio: false,
        confidence: 0.95,
    };

    let est = estimate_monte_carlo(
        cfg,
        SimulationConfig { seed: 1 },
        "public_monte_carlo".to_string(),
        setup,
        |s| s.metastable,
    )
    .expect("monte carlo estimate should succeed");

    assert!(est.p_hat > 0.25 && est.p_hat < 0.75, "p_hat={}", est.p_hat);
}

/// Demonstrates multilevel splitting estimation via the public prelude.
#[test]
fn public_splitting_usage_pattern() {
    let cfg = SplittingEstimateConfig {
        levels: vec![10.0],
        particles: 32,
        end_time: SimTime::from_millis(10),
        install_tokio: false,
        confidence: 0.95,
    };

    let est = estimate_with_splitting(
        cfg,
        SimulationConfig { seed: 1 },
        "public_splitting".to_string(),
        setup,
        |s| s.metastable,
    )
    .expect("splitting estimate should succeed");

    assert!(est.p_hat > 0.25 && est.p_hat < 0.75, "p_hat={}", est.p_hat);
    assert_eq!(est.stage_successes.len(), 2);
}

/// Demonstrates installing a tokio-level ready-task policy and recording/replaying it.
///
/// This is intentionally written in the style we'd expect downstream users to copy:
/// - uses `descartes_explore::prelude::*`
/// - uses the harness for record/replay
/// - installs tokio via the harness (not by manually calling `descartes_tokio::runtime::install`)
#[test]
fn public_tokio_ready_task_record_replay_usage_pattern() {
    fn temp_path(suffix: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "descartes_explore_public_tokio_ready_{}_{}{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            suffix
        ));
        p
    }

    let (trace, recorded) = std::thread::spawn(|| {
        let record_path = temp_path(".json");
        let log = Arc::new(Mutex::new(Vec::<usize>::new()));

        let cfg = HarnessConfig {
            sim_config: SimulationConfig { seed: 1 },
            scenario: "public_tokio_ready_record".to_string(),
            install_tokio: true,
            tokio_ready: Some(HarnessTokioReadyConfig {
                policy: HarnessTokioReadyPolicy::UniformRandom { seed: 123 },
                record_decisions: true,
            }),
            tokio_mutex: None,
            record_concurrency: false,
            frontier: None,
            trace_path: record_path.clone(),
            trace_format: TraceFormat::Json,
        };

        let log_for_run = log.clone();
        let ((), trace) = run_recorded(
            cfg,
            move |sim_config, _ctx| Simulation::new(sim_config),
            move |sim, _ctx| {
                for i in 0..200 {
                    let log = log_for_run.clone();
                    descartes_tokio::task::spawn(async move {
                        log.lock().unwrap().push(i);
                    });
                }

                Executor::timed(SimTime::from_millis(1)).execute(sim);
            },
        )
        .unwrap();

        std::fs::remove_file(&record_path).ok();

        let recorded = log.lock().unwrap().clone();
        assert_eq!(recorded.len(), 200);
        assert!(trace
            .events
            .iter()
            .any(|e| matches!(e, TraceEvent::AsyncRuntimeDecision(_))));

        (trace, recorded)
    })
    .join()
    .unwrap();

    let replayed = std::thread::spawn(move || {
        let replay_path = temp_path(".json");
        let log = Arc::new(Mutex::new(Vec::<usize>::new()));

        let cfg = HarnessConfig {
            sim_config: SimulationConfig { seed: 1 },
            scenario: "public_tokio_ready_replay".to_string(),
            install_tokio: true,
            tokio_ready: None,
            tokio_mutex: None,
            record_concurrency: false,
            frontier: None,
            trace_path: replay_path.clone(),
            trace_format: TraceFormat::Json,
        };

        let log_for_run = log.clone();
        run_replayed(
            cfg,
            &trace,
            move |sim_config, _ctx, _input| Simulation::new(sim_config),
            move |sim, _ctx| {
                for i in 0..200 {
                    let log = log_for_run.clone();
                    descartes_tokio::task::spawn(async move {
                        log.lock().unwrap().push(i);
                    });
                }

                Executor::timed(SimTime::from_millis(1)).execute(sim);
            },
        )
        .unwrap();

        std::fs::remove_file(&replay_path).ok();

        let out = log.lock().unwrap().clone();
        out
    })
    .join()
    .unwrap();

    assert_eq!(recorded, replayed);
}

/// Demonstrates recording/replaying concurrency events (mutex + atomics).
///
/// The replay run validates that the concurrency event stream matches exactly.
#[test]
fn public_concurrency_record_replay_usage_pattern() {
    fn temp_path(suffix: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "descartes_explore_public_concurrency_{}_{}{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            suffix
        ));
        p
    }

    let (trace, saw_concurrency) = std::thread::spawn(|| {
        let record_path = temp_path(".json");

        let cfg = HarnessConfig {
            sim_config: SimulationConfig { seed: 1 },
            scenario: "public_concurrency_record".to_string(),
            install_tokio: true,
            tokio_ready: Some(HarnessTokioReadyConfig {
                policy: HarnessTokioReadyPolicy::Fifo,
                record_decisions: true,
            }),
            tokio_mutex: None,
            record_concurrency: true,
            frontier: None,
            trace_path: record_path.clone(),
            trace_format: TraceFormat::Json,
        };

        let ((), trace) = run_recorded(
            cfg,
            move |sim_config, _ctx| Simulation::new(sim_config),
            move |sim, _ctx| {
                use std::sync::atomic::Ordering;
                use std::sync::Arc;

                let mutex_id = descartes_tokio::stable_id!("concurrency", "public_mutex");
                let atomic_id = descartes_tokio::stable_id!("concurrency", "public_atomic");

                let m = descartes_tokio::sync::Mutex::new_with_id(mutex_id, 0u64);
                let a = Arc::new(descartes_tokio::sync::atomic::AtomicU64::new(atomic_id, 0));

                for _ in 0..2 {
                    let m1 = m.clone();
                    let a1 = a.clone();
                    descartes_tokio::thread::spawn(async move {
                        let mut g = m1.lock().await;
                        let _ = a1.fetch_add(1, Ordering::SeqCst);
                        *g += 1;
                        drop(g);
                        descartes_tokio::thread::yield_now().await;
                        let _g2 = m1.lock().await;
                        let _ = a1.fetch_add(1, Ordering::SeqCst);
                    });
                }

                Executor::timed(SimTime::from_millis(1)).execute(sim);
            },
        )
        .unwrap();

        std::fs::remove_file(&record_path).ok();

        let saw = trace
            .events
            .iter()
            .any(|e| matches!(e, TraceEvent::Concurrency(_)));

        (trace, saw)
    })
    .join()
    .unwrap();

    assert!(saw_concurrency);

    std::thread::spawn(move || {
        let replay_path = temp_path(".json");

        let cfg = HarnessConfig {
            sim_config: SimulationConfig { seed: 1 },
            scenario: "public_concurrency_replay".to_string(),
            install_tokio: true,
            tokio_ready: None,
            tokio_mutex: None,
            record_concurrency: false,
            frontier: None,
            trace_path: replay_path.clone(),
            trace_format: TraceFormat::Json,
        };

        run_replayed(
            cfg,
            &trace,
            move |sim_config, _ctx, _input| Simulation::new(sim_config),
            move |sim, _ctx| {
                use std::sync::atomic::Ordering;
                use std::sync::Arc;

                let mutex_id = descartes_tokio::stable_id!("concurrency", "public_mutex");
                let atomic_id = descartes_tokio::stable_id!("concurrency", "public_atomic");

                let m = descartes_tokio::sync::Mutex::new_with_id(mutex_id, 0u64);
                let a = Arc::new(descartes_tokio::sync::atomic::AtomicU64::new(atomic_id, 0));

                for _ in 0..2 {
                    let m1 = m.clone();
                    let a1 = a.clone();
                    descartes_tokio::thread::spawn(async move {
                        let mut g = m1.lock().await;
                        let _ = a1.fetch_add(1, Ordering::SeqCst);
                        *g += 1;
                        drop(g);
                        descartes_tokio::thread::yield_now().await;
                        let _g2 = m1.lock().await;
                        let _ = a1.fetch_add(1, Ordering::SeqCst);
                    });
                }

                Executor::timed(SimTime::from_millis(1)).execute(sim);
            },
        )
        .unwrap();

        std::fs::remove_file(&replay_path).ok();
    })
    .join()
    .unwrap();
}
