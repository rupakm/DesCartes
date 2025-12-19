use crate::{Simulation, SimTime};

/// Simulation execution trait.
pub trait Execute {
    /// Executes the simulation until some stopping condition is reached.
    /// The condition is implementation-specific.
    fn execute(self, sim: &mut Simulation);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndCondition {
    Time(SimTime),
    NoEvents,
    Steps(usize),
}

/// Executor is used for simple execution of an entire simulation.
///
/// See the crate level documentation for examples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Executor {
    end_condition: EndCondition,
}

impl Executor {
    /// Simulation will end only once there is no available events in the queue.
    #[must_use]
    pub fn unbound() -> Self {
        Self {
            end_condition: EndCondition::NoEvents,
        }
    }

    /// Simulation will be run no longer than the given time.
    /// It may terminate early if no events are available.
    #[must_use]
    pub fn timed(time: SimTime) -> Self {
        Self {
            end_condition: EndCondition::Time(time),
        }
    }

    /// Simulation will execute exactly this many steps, unless we run out of events.
    #[must_use]
    pub fn steps(steps: usize) -> Self {
        Self {
            end_condition: EndCondition::Steps(steps),
        }
    }

    /// Registers a side effect that is called _after_ each simulation step.
    #[must_use]
    pub fn side_effect<F>(self, func: F) -> ExecutorWithSideEffect<F>
    where
        F: Fn(&Simulation),
    {
        ExecutorWithSideEffect {
            end_condition: self.end_condition,
            side_effect: func,
        }
    }
}

impl Execute for Executor {
    fn execute(self, sim: &mut Simulation) {
        run_with(sim, self.end_condition, |_| {});
    }
}

pub struct ExecutorWithSideEffect<F>
where
    F: Fn(&Simulation),
{
    end_condition: EndCondition,
    side_effect: F,
}

impl<F> Execute for ExecutorWithSideEffect<F>
where
    F: Fn(&Simulation),
{
    fn execute(self, sim: &mut Simulation) {
        run_with(sim, self.end_condition, self.side_effect);
    }
}

fn run_with<F>(sim: &mut Simulation, end_condition: EndCondition, side_effect: F)
where
    F: Fn(&Simulation),
{
    let step_fn = |sim: &mut Simulation| {
        let result = sim.step();
        if result {
            side_effect(sim);
        }
        result
    };
    match end_condition {
        EndCondition::Time(time) => execute_until(sim, time, step_fn),
        EndCondition::NoEvents => execute_until_empty(sim, step_fn),
        EndCondition::Steps(steps) => execute_steps(sim, steps, step_fn),
    }
}

fn execute_until_empty<F>(sim: &mut Simulation, step: F)
where
    F: Fn(&mut Simulation) -> bool,
{
    while step(sim) {}
}

fn execute_until<F>(sim: &mut Simulation, time: SimTime, step: F)
where
    F: Fn(&mut Simulation) -> bool,
{
    while sim.scheduler.peek().is_some_and(|e| e.time() <= time) {
        step(sim);
    }
}

fn execute_steps<F>(sim: &mut Simulation, steps: usize, step: F)
where
    F: Fn(&mut Simulation) -> bool,
{
    for _ in 0..steps {
        if !step(sim) {
            break;
        }
    }
}

#[cfg(test)]
mod test {
    use std::time::Duration;
    use super::*;
    use crate::Component;

    struct TestComponent {
        counter: usize,
    }

    #[derive(Debug)]
    struct TestEvent;

    impl Component for TestComponent {
        type Event = TestEvent;

        fn process_event(
            &mut self,
            self_id: crate::Key<Self::Event>,
            _event: &Self::Event,
            scheduler: &mut crate::Scheduler,
        ) {
            //let counter = state.get_mut(self.counter).unwrap();
            
            self.counter += 1;
            if self.counter < 10 {
                scheduler.schedule(SimTime::from_duration(Duration::from_secs(2)), self_id, TestEvent);
            }
        }
    }

    #[test]
    fn test_create_executor() {
        assert_eq!(
            Executor::unbound(),
            Executor {
                end_condition: EndCondition::NoEvents
            }
        );
        assert_eq!(
            Executor::timed(SimTime::from_duration(Duration::default())),
            Executor {
                end_condition: EndCondition::Time(SimTime::from_duration(Duration::default()))
            }
        );
        assert_eq!(
            Executor::steps(7),
            Executor {
                end_condition: EndCondition::Steps(7)
            }
        );
        // Bonus: satisfy codecov on derive
        assert_eq!(&format!("{:?}", TestEvent), "TestEvent");
    }

    #[test]
    fn test_steps() {
        let mut sim = Simulation::default();
        //let counter_key = sim.state.insert(0_usize);
        let component = sim.add_component(TestComponent {
            counter: 0,
        });
        sim.schedule(SimTime::from_duration(Duration::default()), component, TestEvent);
        Executor::steps(10).execute(&mut sim);
        let c: TestComponent = sim.remove_component(component).unwrap();
        assert_eq!(c.counter, 10);
    }

    #[test]
    fn test_steps_stops_before() {
        let mut sim = Simulation::default();
        let component = sim.add_component(TestComponent {
            counter: 0,
        });
        sim.schedule(SimTime::from_duration(Duration::default()), component, TestEvent);
        // After 10 steps there are no events, so it will not execute all 100
        Executor::steps(100).execute(&mut sim);
        let c: TestComponent = sim.remove_component(component).unwrap();
        assert_eq!(c.counter, 10);
    }

    #[test]
    fn test_timed() {
        let mut sim = Simulation::default();
        let component = sim.add_component(TestComponent {
            counter: 0,
        });
        sim.schedule(SimTime::from_duration(Duration::default()), component, TestEvent);
        Executor::timed(SimTime::from_duration(Duration::from_secs(6))).execute(&mut sim);
        let c: TestComponent = sim.remove_component(component).unwrap();
        assert_eq!(c.counter, 4);
        assert_eq!(sim.scheduler.clock().time(), SimTime::from_duration(Duration::from_secs(6)));
    }

    #[test]
    fn test_timed_clock_stops_early() {
        let mut sim = Simulation::default();
        let component = sim.add_component(TestComponent {
            counter: 0,
        });
        sim.schedule(SimTime::from_duration(Duration::default()), component, TestEvent);
        Executor::timed(SimTime::from_duration(Duration::from_secs(5))).execute(&mut sim);
        let c: TestComponent = sim.remove_component(component).unwrap();
        assert_eq!(c.counter, 3);
        assert_eq!(sim.scheduler.clock().time(), SimTime::from_duration(Duration::from_secs(4)));
    }
}
