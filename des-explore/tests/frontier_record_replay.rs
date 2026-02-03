use std::sync::{Arc, Mutex};

use descartes_core::{
    Component, Execute, Executor, Key, Scheduler, SimTime, Simulation, SimulationConfig,
    UniformRandomFrontierPolicy,
};

use descartes_explore::frontier::{RecordingFrontierPolicy, ReplayFrontierPolicy};
use descartes_explore::trace::{TraceEvent, TraceMeta, TraceRecorder};

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

/// Records scheduler decisions using a randomized policy and then replays them.
#[test]
fn record_and_replay_frontier_decisions() {
    let recorder = Arc::new(Mutex::new(TraceRecorder::new(TraceMeta {
        seed: 1,
        scenario: "frontier_record_replay".to_string(),
    })));

    let log = Arc::new(Mutex::new(Vec::new()));

    let mut sim = Simulation::new(SimulationConfig { seed: 1 });
    sim.set_frontier_policy(Box::new(RecordingFrontierPolicy::new(
        UniformRandomFrontierPolicy::new(999),
        recorder.clone(),
    )));

    let key = sim.add_component(Logger { log: log.clone() });
    for i in 0..200 {
        sim.schedule(SimTime::zero(), key, LogEvent::Push(i));
    }

    Executor::timed(SimTime::from_millis(1)).execute(&mut sim);

    let recorded_log = log.lock().unwrap().clone();
    assert_eq!(recorded_log.len(), 200);

    let trace = recorder.lock().unwrap().snapshot();
    assert!(trace
        .events
        .iter()
        .any(|e| matches!(e, TraceEvent::SchedulerDecision(_))));

    // Replay into a fresh simulation.
    let log2 = Arc::new(Mutex::new(Vec::new()));
    let mut sim2 = Simulation::new(SimulationConfig { seed: 1 });
    sim2.set_frontier_policy(Box::new(ReplayFrontierPolicy::from_trace_events(
        &trace.events,
    )));

    let key2 = sim2.add_component(Logger { log: log2.clone() });
    for i in 0..200 {
        sim2.schedule(SimTime::zero(), key2, LogEvent::Push(i));
    }

    Executor::timed(SimTime::from_millis(1)).execute(&mut sim2);

    let replayed_log = log2.lock().unwrap().clone();
    assert_eq!(recorded_log, replayed_log);
}
