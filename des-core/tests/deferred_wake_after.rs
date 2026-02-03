use descartes_core::{
    defer_wake_after, Component, Execute, Executor, Key, Scheduler, SimTime, Simulation,
};

#[derive(Debug, Clone, Copy)]
enum Ev {
    Start,
    Tick,
}

#[derive(Default)]
struct Driver {
    saw_tick: bool,
}

impl Component for Driver {
    type Event = Ev;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        _s: &mut Scheduler,
    ) {
        match event {
            Ev::Start => {
                // This runs under scheduler context (Simulation::step). Scheduling via
                // SchedulerHandle would deadlock; defer_wake_after must work.
                defer_wake_after(SimTime::from_millis(5), self_id, Ev::Tick);
            }
            Ev::Tick => {
                self.saw_tick = true;
            }
        }
    }
}

#[test]
fn defer_wake_after_schedules_with_delay_without_locking() {
    let mut sim = Simulation::default();
    let key = sim.add_component(Driver::default());
    sim.schedule_now(key, Ev::Start);

    Executor::timed(SimTime::from_millis(4)).execute(&mut sim);
    assert!(!sim.get_component_mut::<Ev, Driver>(key).unwrap().saw_tick);

    Executor::timed(SimTime::from_millis(10)).execute(&mut sim);
    assert!(sim.get_component_mut::<Ev, Driver>(key).unwrap().saw_tick);
}
