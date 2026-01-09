//! Determinism guardrail tests
//!
//! These tests are intended to detect accidental introduction of
//! non-determinism in event execution order for identical simulations.

use des_core::{Component, Execute, Executor, Key, Scheduler, SimTime, Simulation};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
enum LogEvent {
    Push(usize),
}

struct LoggerComponent {
    log: Arc<Mutex<Vec<usize>>>,
}

impl Component for LoggerComponent {
    type Event = LogEvent;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        _scheduler: &mut Scheduler,
    ) {
        match *event {
            LogEvent::Push(value) => self.log.lock().unwrap().push(value),
        }
    }
}

fn run_same_time_component_events(event_count: usize) -> Vec<usize> {
    let mut sim = Simulation::default();
    let log = Arc::new(Mutex::new(Vec::new()));

    let component = LoggerComponent { log: log.clone() };
    let key = sim.add_component(component);

    for i in 0..event_count {
        // Delay is relative to current time (t=0 here), so all events land at the same timestamp.
        sim.schedule(SimTime::zero(), key, LogEvent::Push(i));
    }

    Executor::timed(SimTime::from_millis(1)).execute(&mut sim);

    let result = log.lock().unwrap().clone();
    assert_eq!(result.len(), event_count);
    result
}

#[test]
fn deterministic_same_time_component_event_order_across_runs() {
    // Run multiple identical simulations and ensure their execution order matches.
    // This avoids locking in a particular ordering policy, while still detecting
    // accidental non-determinism.
    let baseline = run_same_time_component_events(200);

    for _ in 0..50 {
        let next = run_same_time_component_events(200);
        assert_eq!(baseline, next);
    }
}

fn run_mixed_same_time_component_and_task_events(count: usize) -> Vec<String> {
    let mut sim = Simulation::default();
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    #[derive(Debug, Clone)]
    enum MixedEvent {
        Push(String),
    }

    struct MixedLogger {
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Component for MixedLogger {
        type Event = MixedEvent;

        fn process_event(
            &mut self,
            _self_id: Key<Self::Event>,
            event: &Self::Event,
            scheduler: &mut Scheduler,
        ) {
            match event {
                MixedEvent::Push(value) => self.log.lock().unwrap().push(value.clone()),
            }

            // Also schedule a task at the same logical time.
            let log = self.log.clone();
            scheduler.schedule_closure(SimTime::zero(), move |_scheduler| {
                log.lock().unwrap().push("task".to_string());
            });
        }
    }

    let key = sim.add_component(MixedLogger { log: log.clone() });

    for i in 0..count {
        sim.schedule(SimTime::zero(), key, MixedEvent::Push(format!("event-{i}")));
    }

    Executor::timed(SimTime::from_millis(1)).execute(&mut sim);

    let result = log.lock().unwrap().clone();
    assert!(result.len() >= count);
    result
}

#[test]
fn deterministic_mixed_event_and_task_order_across_runs() {
    let baseline = run_mixed_same_time_component_and_task_events(50);

    for _ in 0..25 {
        let next = run_mixed_same_time_component_and_task_events(50);
        assert_eq!(baseline, next);
    }
}
