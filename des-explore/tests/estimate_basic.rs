use std::sync::{Arc, Mutex};
use std::time::Duration;

use descartes_core::dists::{ExponentialDistribution, ServiceTimeDistribution};
use descartes_core::{Component, Key, SimTime, Simulation, SimulationConfig};

use descartes_explore::estimate::{
    estimate_monte_carlo, estimate_with_splitting, MonteCarloConfig, SplittingEstimateConfig,
};
use descartes_explore::harness::HarnessContext;
use descartes_explore::monitor::{Monitor, MonitorConfig, QueueId};
use descartes_explore::trace::Trace;

const Q: QueueId = QueueId(1);

#[derive(Debug)]
enum Ev {
    Tick,
}

struct CoinFlipTerminalEvent {
    flip: ExponentialDistribution,
    decided_bad: Option<bool>,
    monitor: Arc<Mutex<Monitor>>,
    decide_at: SimTime,
}

impl Component for CoinFlipTerminalEvent {
    type Event = Ev;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        _event: &Self::Event,
        scheduler: &mut descartes_core::Scheduler,
    ) {
        let now = scheduler.time();

        // Emit a completion each tick so retry amplification stays finite.
        self.monitor
            .lock()
            .unwrap()
            .observe_complete(now, Duration::from_millis(1), true);

        let bad = if now >= self.decide_at {
            *self.decided_bad.get_or_insert_with(|| {
                // Exp(1) draw: P[X < ln 2] = 0.5.
                self.flip.sample().as_secs_f64() < std::f64::consts::LN_2
            })
        } else {
            false
        };

        let qlen = if bad { 100 } else { 0 };
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
    let flip = ExponentialDistribution::from_config(sim.config(), 1.0)
        .with_provider(provider, descartes_core::draw_site!("flip"));

    let key = sim.add_component(CoinFlipTerminalEvent {
        flip,
        decided_bad: None,
        monitor: monitor.clone(),
        decide_at: SimTime::from_millis(2),
    });

    sim.schedule_now(key, Ev::Tick);

    (sim, monitor)
}

/// Estimates a known-probability terminal property via naïve Monte Carlo.
///
/// This test uses the monitor's `status.metastable` flag as a stand-in for a
/// generic terminal event, to validate the estimator wiring end-to-end.
#[test]
fn monte_carlo_estimates_coinflip() {
    let cfg = MonteCarloConfig {
        trials: 200,
        end_time: SimTime::from_millis(10),
        install_tokio: false,
        confidence: 0.95,
    };

    let est = estimate_monte_carlo(
        cfg,
        SimulationConfig { seed: 1 },
        "coinflip".to_string(),
        setup,
        |s| s.metastable,
    )
    .expect("estimate should succeed");

    // Should be reasonably close to 0.5, but don't make it brittle.
    assert!(est.p_hat > 0.35 && est.p_hat < 0.65, "p_hat={}", est.p_hat);

    if let (Some(lo), Some(hi)) = (est.ci_low, est.ci_high) {
        assert!(lo <= 0.5 && 0.5 <= hi, "ci=[{}, {}]", lo, hi);
    }
}

/// Estimates a known-probability terminal property via multilevel splitting.
///
/// This test uses the monitor's `status.metastable` flag as a stand-in for a
/// generic terminal event, to validate prefix checkpointing + replay.
#[test]
fn splitting_estimates_coinflip() {
    let cfg = SplittingEstimateConfig {
        levels: vec![10.0],
        particles: 64,
        end_time: SimTime::from_millis(10),
        install_tokio: false,
        confidence: 0.95,
    };

    let est = estimate_with_splitting(
        cfg,
        SimulationConfig { seed: 1 },
        "coinflip".to_string(),
        setup,
        |s| s.metastable,
    )
    .expect("splitting estimate should succeed");

    assert!(est.p_hat > 0.35 && est.p_hat < 0.65, "p_hat={}", est.p_hat);
    assert_eq!(est.stage_successes.len(), 2); // one level + terminal
}
