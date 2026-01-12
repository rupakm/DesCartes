//! Public API usage patterns for Milestone 6 estimators.
//!
//! This test file is intentionally "example-like": it uses the `des_explore::prelude::*`
//! exports and keeps the setup/predicate closures in the shape we expect downstream
//! users to copy.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::dists::{ExponentialDistribution, ServiceTimeDistribution};
use des_core::{Component, Key, SimTime, Simulation, SimulationConfig};

use des_explore::prelude::*;

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
        scheduler: &mut des_core::Scheduler,
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
        .with_provider(provider, des_core::draw_site!("coin"));

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
