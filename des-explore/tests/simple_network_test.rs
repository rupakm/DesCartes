use std::sync::{Arc, Mutex};

use descartes_core::{Component, Executor, Key, Scheduler, SimTime, Simulation, SimulationConfig};
use descartes_explore::monitor::{Monitor, MonitorConfig};

/// Basic smoke test that a minimal simulation executes.
///
/// This is intentionally simple: it exercises the core DES execution path and
/// ensures monitor construction doesn't panic.
#[test]
fn basic_network_congestion_test() {
    struct TestComponent {
        _monitor: Arc<Mutex<Monitor>>,
    }

    impl Component for TestComponent {
        type Event = ();

        fn process_event(
            &mut self,
            _self_id: Key<Self::Event>,
            _event: &Self::Event,
            _scheduler: &mut Scheduler,
        ) {
        }
    }

    let monitor = Arc::new(Mutex::new(Monitor::new(
        MonitorConfig::default(),
        SimTime::zero(),
    )));

    let mut sim = Simulation::new(SimulationConfig { seed: 1000 });
    let key = sim.add_component(TestComponent { _monitor: monitor });

    // Schedule a single no-op event and run briefly.
    sim.schedule_now(key, ());
    sim.execute(Executor::timed(SimTime::from_secs(1)));
}
