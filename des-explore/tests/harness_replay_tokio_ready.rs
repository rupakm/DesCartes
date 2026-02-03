use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use descartes_core::{Execute, Executor, SimTime, Simulation, SimulationConfig};

use descartes_explore::harness::{
    run_recorded, run_replayed, HarnessConfig, HarnessReplayError, HarnessTokioReadyConfig,
    HarnessTokioReadyPolicy,
};
use descartes_explore::io::TraceFormat;
use descartes_explore::trace::{Trace, TraceEvent};

fn temp_path(suffix: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "descartes_explore_harness_tokio_ready_{}_{}{}",
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
fn harness_replays_tokio_ready_task_decisions() {
    let (trace, recorded_log) = std::thread::spawn(|| {
        let record_path = temp_path(".json");
        let log = Arc::new(Mutex::new(Vec::<usize>::new()));

        let cfg = HarnessConfig {
            sim_config: SimulationConfig { seed: 1 },
            scenario: "harness_tokio_ready_record".to_string(),
            install_tokio: true,
            tokio_ready: Some(HarnessTokioReadyConfig {
                policy: HarnessTokioReadyPolicy::UniformRandom { seed: 123 },
                record_decisions: true,
            }),
            tokio_mutex: None,
            record_concurrency: false,
            frontier: None,
            trace_path: record_path.clone(),
            trace_format: TraceFormat::Json,
        };

        let log_for_run = log.clone();
        let ((), trace) = run_recorded(
            cfg,
            move |sim_config, _ctx| Simulation::new(sim_config),
            move |sim, _ctx| {
                for i in 0..200 {
                    let log = log_for_run.clone();
                    descartes_tokio::task::spawn(async move {
                        log.lock().unwrap().push(i);
                    });
                }

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
            .any(|e| matches!(e, TraceEvent::AsyncRuntimeDecision(_))));

        (trace, recorded_log)
    })
    .join()
    .unwrap();

    let replayed_log = std::thread::spawn(move || {
        let replay_path = temp_path(".json");
        let log = Arc::new(Mutex::new(Vec::<usize>::new()));

        let cfg = HarnessConfig {
            sim_config: SimulationConfig { seed: 1 },
            scenario: "harness_tokio_ready_replay".to_string(),
            install_tokio: true,
            tokio_ready: None,
            tokio_mutex: None,
            record_concurrency: false,
            frontier: None,
            trace_path: replay_path.clone(),
            trace_format: TraceFormat::Json,
        };

        let log_for_run = log.clone();
        let ((), _trace2) = run_replayed(
            cfg,
            &trace,
            move |sim_config, _ctx, _input| Simulation::new(sim_config),
            move |sim, _ctx| {
                for i in 0..200 {
                    let log = log_for_run.clone();
                    descartes_tokio::task::spawn(async move {
                        log.lock().unwrap().push(i);
                    });
                }

                Executor::timed(SimTime::from_millis(1)).execute(sim);
            },
        )
        .unwrap();

        std::fs::remove_file(&replay_path).ok();
        let out = log.lock().unwrap().clone();
        out
    })
    .join()
    .unwrap();

    assert_eq!(recorded_log, replayed_log);
}

#[test]
fn harness_replay_reports_tokio_ready_mismatch() {
    let trace: Trace = std::thread::spawn(|| {
        let record_path = temp_path(".json");

        let cfg = HarnessConfig {
            sim_config: SimulationConfig { seed: 1 },
            scenario: "harness_tokio_ready_mismatch_record".to_string(),
            install_tokio: true,
            tokio_ready: Some(HarnessTokioReadyConfig {
                policy: HarnessTokioReadyPolicy::UniformRandom { seed: 123 },
                record_decisions: true,
            }),
            tokio_mutex: None,
            record_concurrency: false,
            frontier: None,
            trace_path: record_path.clone(),
            trace_format: TraceFormat::Json,
        };

        let ((), trace) = run_recorded(
            cfg,
            move |sim_config, _ctx| Simulation::new(sim_config),
            move |sim, _ctx| {
                for _ in 0..200 {
                    descartes_tokio::task::spawn(async move {
                        // no-op
                    });
                }
                Executor::timed(SimTime::from_millis(1)).execute(sim);
            },
        )
        .unwrap();

        std::fs::remove_file(&record_path).ok();
        trace
    })
    .join()
    .unwrap();

    let err = std::thread::spawn(move || {
        let replay_path = temp_path(".json");

        let cfg2 = HarnessConfig {
            sim_config: SimulationConfig { seed: 1 },
            scenario: "harness_tokio_ready_mismatch_replay".to_string(),
            install_tokio: true,
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
            move |sim_config, _ctx, _input| Simulation::new(sim_config),
            move |sim, _ctx| {
                // Intentional divergence: fewer tasks.
                for _ in 0..199 {
                    descartes_tokio::task::spawn(async move {
                        // no-op
                    });
                }
                Executor::timed(SimTime::from_millis(1)).execute(sim);
            },
        )
        .unwrap_err();

        std::fs::remove_file(&replay_path).ok();
        err
    })
    .join()
    .unwrap();

    match err {
        HarnessReplayError::TokioReady(_) => {
            // ok
        }
        other => panic!("expected tokio ready error, got {other:?}"),
    }
}
