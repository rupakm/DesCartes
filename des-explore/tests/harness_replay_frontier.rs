use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use descartes_core::{
    Component, Execute, Executor, Key, Scheduler, SimTime, Simulation, SimulationConfig,
};

use descartes_explore::harness::{
    run_recorded, run_replayed, HarnessConfig, HarnessFrontierConfig, HarnessFrontierPolicy,
    HarnessReplayError,
};
use descartes_explore::io::TraceFormat;
use descartes_explore::trace::TraceEvent;

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

fn temp_path(suffix: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "descartes_explore_harness_replay_{}_{}{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        suffix
    ));
    p
}

#[test]
fn harness_replays_frontier_decisions() {
    let record_path = temp_path(".json");
    let log = Arc::new(Mutex::new(Vec::new()));

    let cfg = HarnessConfig {
        sim_config: SimulationConfig { seed: 1 },
        scenario: "harness_replay_frontier".to_string(),
        install_tokio: false,
        tokio_ready: None,
        tokio_mutex: None,
        record_concurrency: false,
        frontier: Some(HarnessFrontierConfig {
            policy: HarnessFrontierPolicy::UniformRandom { seed: 999 },
            record_decisions: true,
        }),
        trace_path: record_path.clone(),
        trace_format: TraceFormat::Json,
    };

    let log_for_setup = log.clone();
    let ((), trace) = run_recorded(
        cfg,
        move |sim_config, _ctx| {
            let mut sim = Simulation::new(sim_config);
            let key = sim.add_component(Logger {
                log: log_for_setup.clone(),
            });
            for i in 0..200 {
                sim.schedule(SimTime::zero(), key, LogEvent::Push(i));
            }
            sim
        },
        move |sim, _ctx| {
            Executor::timed(SimTime::from_millis(1)).execute(sim);
        },
    )
    .unwrap();

    std::fs::remove_file(&record_path).ok();

    let recorded_log = log.lock().unwrap().clone();
    assert_eq!(recorded_log.len(), 200);
    assert!(trace
        .events
        .iter()
        .any(|e| matches!(e, TraceEvent::SchedulerDecision(_))));

    let replay_path = temp_path(".json");
    let log2 = Arc::new(Mutex::new(Vec::new()));

    let cfg2 = HarnessConfig {
        sim_config: SimulationConfig { seed: 1 },
        scenario: "harness_replay_frontier_replay".to_string(),
        install_tokio: false,
        tokio_ready: None,
        tokio_mutex: None,
        record_concurrency: false,
        frontier: None,
        trace_path: replay_path.clone(),
        trace_format: TraceFormat::Json,
    };

    let log2_for_setup = log2.clone();
    let ((), _replay_trace) = run_replayed(
        cfg2,
        &trace,
        move |sim_config, _ctx, _input| {
            let mut sim = Simulation::new(sim_config);
            let key = sim.add_component(Logger {
                log: log2_for_setup.clone(),
            });
            for i in 0..200 {
                sim.schedule(SimTime::zero(), key, LogEvent::Push(i));
            }
            sim
        },
        move |sim, _ctx| {
            Executor::timed(SimTime::from_millis(1)).execute(sim);
        },
    )
    .unwrap();

    std::fs::remove_file(&replay_path).ok();

    let replayed_log = log2.lock().unwrap().clone();
    assert_eq!(recorded_log, replayed_log);
}

#[test]
fn harness_replay_reports_frontier_mismatch() {
    let record_path = temp_path(".json");

    let cfg = HarnessConfig {
        sim_config: SimulationConfig { seed: 1 },
        scenario: "harness_replay_frontier_mismatch".to_string(),
        install_tokio: false,
        tokio_ready: None,
        tokio_mutex: None,
        record_concurrency: false,
        frontier: Some(HarnessFrontierConfig {
            policy: HarnessFrontierPolicy::UniformRandom { seed: 999 },
            record_decisions: true,
        }),
        trace_path: record_path.clone(),
        trace_format: TraceFormat::Json,
    };

    let ((), trace) = run_recorded(
        cfg,
        move |sim_config, _ctx| {
            let mut sim = Simulation::new(sim_config);
            let key = sim.add_component(Logger {
                log: Arc::new(Mutex::new(Vec::new())),
            });
            for i in 0..200 {
                sim.schedule(SimTime::zero(), key, LogEvent::Push(i));
            }
            sim
        },
        move |sim, _ctx| {
            Executor::timed(SimTime::from_millis(1)).execute(sim);
        },
    )
    .unwrap();

    std::fs::remove_file(&record_path).ok();

    let replay_path = temp_path(".json");
    let cfg2 = HarnessConfig {
        sim_config: SimulationConfig { seed: 1 },
        scenario: "harness_replay_frontier_mismatch_replay".to_string(),
        install_tokio: false,
        tokio_ready: None,
        tokio_mutex: None,
        record_concurrency: false,
        frontier: None,
        trace_path: replay_path.clone(),
        trace_format: TraceFormat::Json,
    };

    let err = run_replayed(
        cfg2,
        &trace,
        move |sim_config, _ctx, _input| {
            let mut sim = Simulation::new(sim_config);
            let key = sim.add_component(Logger {
                log: Arc::new(Mutex::new(Vec::new())),
            });

            // Intentional divergence: fewer same-time events.
            for i in 0..199 {
                sim.schedule(SimTime::zero(), key, LogEvent::Push(i));
            }
            sim
        },
        move |sim, _ctx| {
            Executor::timed(SimTime::from_millis(1)).execute(sim);
        },
    )
    .unwrap_err();

    std::fs::remove_file(&replay_path).ok();

    match err {
        HarnessReplayError::Frontier(_) => {
            // ok
        }
        other => panic!("expected frontier error, got {other:?}"),
    }
}
