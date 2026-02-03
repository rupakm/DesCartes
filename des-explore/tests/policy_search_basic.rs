use std::sync::{Arc, Mutex};
use std::time::Duration;

use descartes_core::{Component, Key, Scheduler, SimTime, Simulation, SimulationConfig};

use descartes_explore::harness::HarnessContext;
use descartes_explore::monitor::{Monitor, MonitorConfig, MonitorStatus, QueueId};
use descartes_explore::policy_search::{
    search_policy, BaseFrontierPolicy, BaseTokioReadyPolicy, PolicySearchConfig,
    PolicySearchObjective,
};
use descartes_explore::trace::Trace;

const Q: QueueId = QueueId(42);

#[derive(Debug, Clone)]
enum Ev {
    Tick,
    A,
    B,
    Lower,
}

struct OrderSensitiveBug {
    armed: bool,
    monitor: Arc<Mutex<Monitor>>,
}

impl Component for OrderSensitiveBug {
    type Event = Ev;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        let now = scheduler.time();
        match event {
            Ev::Tick => {
                // Stable baseline activity: 0 queue, steady throughput.
                self.monitor.lock().unwrap().observe_queue_len(now, Q, 0);
                self.monitor
                    .lock()
                    .unwrap()
                    .observe_complete(now, Duration::from_millis(1), true);
                self.monitor
                    .lock()
                    .unwrap()
                    .observe_complete(now, Duration::from_millis(1), true);

                // Keep steady baseline activity.
                scheduler.schedule(SimTime::from_millis(1), self_id, Ev::Tick);

                if now == SimTime::from_millis(2) {
                    // Two same-time events at t=3.5ms (avoids conflicting with Tick).
                    // FIFO chooses A first (no bug).
                    scheduler.schedule(SimTime::from_micros(1500), self_id, Ev::A);
                    scheduler.schedule(SimTime::from_micros(1500), self_id, Ev::B);
                }

                return;
            }
            Ev::A => {
                self.armed = true;
            }
            Ev::B => {
                if !self.armed {
                    // "Bad" schedule: a queue spike that persists long enough
                    // to show up in monitor window summaries.
                    self.monitor.lock().unwrap().observe_queue_len(now, Q, 1);
                    scheduler.schedule(SimTime::from_millis(2), self_id, Ev::Lower);
                }
            }
            Ev::Lower => {
                self.monitor.lock().unwrap().observe_queue_len(now, Q, 0);
            }
        }
    }
}

fn setup(
    config: SimulationConfig,
    _ctx: &HarnessContext,
    _prefix: Option<&Trace>,
    _cont_seed: u64,
) -> (Simulation, Arc<Mutex<Monitor>>) {
    let mut sim = Simulation::new(config);

    let mut mcfg = MonitorConfig::default();
    mcfg.window = Duration::from_millis(1);
    mcfg.baseline_warmup_windows = 2;
    mcfg.baseline_epsilon = 0.0;
    mcfg.recovery_hold_windows = 0;
    mcfg.metastable_persist_windows = 1;

    let monitor = Arc::new(Mutex::new(Monitor::new(mcfg, SimTime::zero())));
    monitor
        .lock()
        .unwrap()
        .mark_post_spike_start(SimTime::zero());

    let key = sim.add_component(OrderSensitiveBug {
        armed: false,
        monitor: monitor.clone(),
    });

    sim.schedule_now(key, Ev::Tick);

    (sim, monitor)
}

#[test]
fn policy_search_finds_ordering_bug() {
    let cfg = PolicySearchConfig {
        base_sim_config: SimulationConfig { seed: 1 },
        scenario: "policy_search_basic".to_string(),
        end_time: SimTime::from_millis(10),
        install_tokio: false,
        base_frontier: BaseFrontierPolicy::Fifo,
        base_tokio_ready: BaseTokioReadyPolicy::Fifo,
        budget_policies: 10,
        trials_per_policy: 1,
        holdout_trials: 16,
        confidence: 0.95,
        objective: PolicySearchObjective::WilsonLowerBound,
        max_failure_traces: 32,
        mutation_seed: 123,
    };

    let report = search_policy(cfg, setup, |s: &MonitorStatus| s.metastable)
        .expect("policy search should succeed");

    // The FIFO baseline should not find the bug, but the hillclimb should.
    assert!(
        report.best_policy.keys().next().is_some(),
        "expected learned policy table to be non-empty"
    );

    assert!(report.holdout.p_hat > 0.5, "p_hat={}", report.holdout.p_hat);
    assert!(
        report.holdout.ci_low.unwrap_or(0.0) > 0.1,
        "ci_low={:?}",
        report.holdout.ci_low
    );

    assert!(
        !report.failures.is_empty(),
        "expected at least one captured failure trace"
    );
}
