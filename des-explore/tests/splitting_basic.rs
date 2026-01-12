use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::dists::{ArrivalPattern, PoissonArrivals};
use des_core::{Component, Key, SimTime, Simulation, SimulationConfig};

use des_explore::harness::HarnessContext;
use des_explore::monitor::{Monitor, MonitorConfig, QueueId};
use des_explore::splitting::{find_with_splitting, SplittingConfig};
use des_explore::trace::Trace;

const Q: QueueId = QueueId(1);

#[derive(Debug)]
enum Ev {
    Tick,
}

struct Toy {
    arrivals: PoissonArrivals,
    monitor: Arc<Mutex<Monitor>>,
    backlog: u64,
}

impl Component for Toy {
    type Event = Ev;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        _event: &Self::Event,
        scheduler: &mut des_core::Scheduler,
    ) {
        let now = scheduler.time();

        // Keep backlog high to trigger persistent off-baseline distance.
        self.backlog = self.backlog.max(10);
        self.monitor
            .lock()
            .unwrap()
            .observe_queue_len(now, Q, self.backlog);

        // Consume at least one RNG draw per tick.
        let dt = self.arrivals.next_arrival_time();
        scheduler.schedule(SimTime::from_duration(dt), self_id, Ev::Tick);
    }
}

fn setup(
    sim_config: SimulationConfig,
    ctx: &HarnessContext,
    prefix: Option<&Trace>,
    cont_seed: u64,
) -> (Simulation, Arc<Mutex<Monitor>>) {
    let mut sim = Simulation::new(sim_config);

    let mut monitor_cfg = MonitorConfig::default();
    monitor_cfg.window = Duration::from_millis(5);
    monitor_cfg.baseline_warmup_windows = 0;
    monitor_cfg.recovery_hold_windows = 1;
    monitor_cfg.baseline_epsilon = 0.1;
    monitor_cfg.metastable_persist_windows = 1;
    monitor_cfg.recovery_time_limit = Some(Duration::from_millis(20));

    let monitor = Arc::new(Mutex::new(Monitor::new(monitor_cfg, SimTime::zero())));
    monitor
        .lock()
        .unwrap()
        .mark_post_spike_start(SimTime::zero());

    let provider = ctx.branching_provider(prefix, cont_seed);

    let arrivals = PoissonArrivals::from_config(sim.config(), 1000.0)
        .with_provider(provider, des_core::draw_site!("arrival"));

    let key = sim.add_component(Toy {
        arrivals,
        monitor: monitor.clone(),
        backlog: 0,
    });

    sim.schedule_now(key, Ev::Tick);

    (sim, monitor)
}

/// Smoke test for multilevel splitting bug finding.
///
/// Asserts that `find_with_splitting` can find a metastable trace in a tiny
/// deterministic toy model.
#[test]
fn splitting_finds_metastable_trace_quickly() {
    let mut cfg = SplittingConfig::default();
    cfg.levels = vec![0.5];
    cfg.branch_factor = 2;
    cfg.max_particles_per_level = 4;
    cfg.end_time = SimTime::from_duration(Duration::from_millis(30));
    cfg.install_tokio = false;

    let found = find_with_splitting(cfg, SimulationConfig { seed: 1 }, "toy".to_string(), setup)
        .expect("splitting run should succeed");

    assert!(found.is_some());
    let found = found.unwrap();
    assert!(found.status.metastable);
    assert!(!found.trace.events.is_empty());
}
