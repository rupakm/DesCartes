//! Tests that component and task IDs are deterministic under `SimulationConfig.seed`.

use des_core::{SimTime, Simulation, SimulationConfig};

#[test]
fn component_ids_are_deterministic_across_runs() {
    let mut a = Simulation::new(SimulationConfig { seed: 123 });
    let mut b = Simulation::new(SimulationConfig { seed: 123 });

    #[derive(Debug)]
    struct Noop;

    impl des_core::Component for Noop {
        type Event = ();

        fn process_event(
            &mut self,
            _self_id: des_core::Key<Self::Event>,
            _event: &Self::Event,
            _scheduler: &mut des_core::Scheduler,
        ) {
        }
    }

    let k1 = a.add_component(Noop);
    let k2 = a.add_component(Noop);

    let j1 = b.add_component(Noop);
    let j2 = b.add_component(Noop);

    assert_eq!(k1.id(), j1.id());
    assert_eq!(k2.id(), j2.id());
}

#[test]
fn task_ids_are_deterministic_across_runs() {
    fn schedule_two(handle: &des_core::SchedulerHandle) -> (des_core::TaskId, des_core::TaskId) {
        let a = handle.timeout(SimTime::zero(), |_| {}).id();
        let b = handle.timeout(SimTime::zero(), |_| {}).id();
        (a, b)
    }

    let sim1 = Simulation::new(SimulationConfig { seed: 999 });
    let sim2 = Simulation::new(SimulationConfig { seed: 999 });

    let h1 = sim1.scheduler_handle();
    let h2 = sim2.scheduler_handle();

    let (t1a, t1b) = schedule_two(&h1);
    let (t2a, t2b) = schedule_two(&h2);

    assert_eq!(t1a, t2a);
    assert_eq!(t1b, t2b);
}
