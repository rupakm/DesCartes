//! Same-time event frontier policy tests.

use std::sync::{Arc, Mutex};

use des_core::{
    Component, Execute, Executor, Key, Scheduler, SimTime, Simulation, UniformRandomFrontierPolicy,
};

#[derive(Debug, Clone)]
enum LogEvent {
    Push(usize),
}

struct Logger {
    log: Arc<Mutex<Vec<usize>>>,
}

impl Component for Logger {
    type Event = LogEvent;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        _scheduler: &mut Scheduler,
    ) {
        match *event {
            LogEvent::Push(v) => self.log.lock().unwrap().push(v),
        }
    }
}

fn run_with_policy(policy_seed: Option<u64>, n: usize) -> Vec<usize> {
    let mut sim = Simulation::default();
    if let Some(seed) = policy_seed {
        sim.set_frontier_policy(Box::new(UniformRandomFrontierPolicy::new(seed)));
    }

    let log = Arc::new(Mutex::new(Vec::new()));
    let key = sim.add_component(Logger { log: log.clone() });

    for i in 0..n {
        sim.schedule(SimTime::zero(), key, LogEvent::Push(i));
    }

    Executor::timed(SimTime::from_millis(1)).execute(&mut sim);

    let out = log.lock().unwrap().clone();
    out
}

/// Ensures the default policy preserves FIFO ordering.
#[test]
fn fifo_policy_preserves_schedule_order() {
    let out = run_with_policy(None, 200);
    assert_eq!(out, (0..200).collect::<Vec<_>>());
}

/// Ensures the uniform-random policy is deterministic given a seed.
#[test]
fn uniform_random_policy_is_seeded_and_reproducible() {
    let out1 = run_with_policy(Some(123), 200);
    let out2 = run_with_policy(Some(123), 200);
    assert_eq!(out1, out2);

    // Extremely unlikely to match strict FIFO.
    assert_ne!(out1, (0..200).collect::<Vec<_>>());
}
