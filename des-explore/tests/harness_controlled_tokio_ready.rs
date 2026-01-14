#![cfg(feature = "tokio")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use des_core::{Executor, SimTime, Simulation, SimulationConfig};
use des_explore::harness::{run_controlled, HarnessConfig, HarnessControl};
use des_explore::io::TraceFormat;
use des_explore::schedule_explore::{DecisionKey, DecisionKind, DecisionScript};
use des_explore::trace::TraceEvent;

fn temp_path(suffix: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "des_explore_harness_controlled_tokio_{}_{}{}",
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
fn run_controlled_tokio_ready_respects_script_choice() {
    // `des_tokio::runtime::install*` uses thread-local state, so each run must
    // execute on a fresh thread.
    let d1 = std::thread::spawn(|| {
        let ran = Arc::new(AtomicUsize::new(0));

        let trace_path1 = temp_path(".json");
        let cfg1 = HarnessConfig {
            sim_config: SimulationConfig { seed: 7 },
            scenario: "controlled_tokio_ready_1".to_string(),
            install_tokio: true,
            tokio_ready: None,
            tokio_mutex: None,
            record_concurrency: false,
            frontier: None,
            trace_path: trace_path1.clone(),
            trace_format: TraceFormat::Json,
        };

        let control1 = HarnessControl {
            decision_script: None,
            explore_frontier: false,
            explore_tokio_ready: true,
        };

        let ran_for_run = ran.clone();
        let ((), trace1) = run_controlled(
            cfg1,
            control1,
            None,
            |sim_config, _ctx, _prefix| Simulation::new(sim_config),
            move |sim, _ctx| {
                for _ in 0..2 {
                    let ran_task = ran_for_run.clone();
                    des_tokio::task::spawn(async move {
                        des_tokio::thread::yield_now().await;
                        ran_task.fetch_add(1, Ordering::Relaxed);
                    });
                }

                sim.execute(Executor::timed(SimTime::from_duration(
                    Duration::from_millis(1),
                )));
            },
        )
        .unwrap();

        std::fs::remove_file(&trace_path1).ok();
        assert_eq!(ran.load(Ordering::Relaxed), 2);

        trace1
            .events
            .iter()
            .find_map(|e| match e {
                TraceEvent::AsyncRuntimeDecision(d) if d.ready_task_ids.len() >= 2 => {
                    Some(d.clone())
                }
                _ => None,
            })
            .expect("expected an AsyncRuntimeDecision with 2+ ready tasks")
    })
    .join()
    .unwrap();

    let chosen = d1
        .chosen_task_id
        .expect("chosen_task_id should be recorded");
    let forced = *d1.ready_task_ids.iter().find(|&&id| id != chosen).unwrap();

    let mut script = DecisionScript::new();
    script.insert(
        DecisionKey::new(
            DecisionKind::TokioReady,
            d1.time_nanos,
            0,
            d1.ready_task_ids.clone(),
        ),
        forced,
    );

    let trace2 = std::thread::spawn(move || {
        let ran2 = Arc::new(AtomicUsize::new(0));

        let trace_path2 = temp_path(".json");
        let cfg2 = HarnessConfig {
            sim_config: SimulationConfig { seed: 7 },
            scenario: "controlled_tokio_ready_2".to_string(),
            install_tokio: true,
            tokio_ready: None,
            tokio_mutex: None,
            record_concurrency: false,
            frontier: None,
            trace_path: trace_path2.clone(),
            trace_format: TraceFormat::Json,
        };

        let control2 = HarnessControl {
            decision_script: Some(Arc::new(script)),
            explore_frontier: false,
            explore_tokio_ready: true,
        };

        let ran2_for_run = ran2.clone();
        let ((), trace2) = run_controlled(
            cfg2,
            control2,
            None,
            |sim_config, _ctx, _prefix| Simulation::new(sim_config),
            move |sim, _ctx| {
                for _ in 0..2 {
                    let ran_task = ran2_for_run.clone();
                    des_tokio::task::spawn(async move {
                        des_tokio::thread::yield_now().await;
                        ran_task.fetch_add(1, Ordering::Relaxed);
                    });
                }

                sim.execute(Executor::timed(SimTime::from_duration(
                    Duration::from_millis(1),
                )));
            },
        )
        .unwrap();

        std::fs::remove_file(&trace_path2).ok();
        assert_eq!(ran2.load(Ordering::Relaxed), 2);

        trace2
    })
    .join()
    .unwrap();

    let d2 = trace2
        .events
        .iter()
        .find_map(|e| match e {
            TraceEvent::AsyncRuntimeDecision(d) if d.ready_task_ids.len() >= 2 => Some(d.clone()),
            _ => None,
        })
        .expect("expected an AsyncRuntimeDecision with 2+ ready tasks");

    assert_eq!(d2.chosen_task_id, Some(forced));
}
