#![cfg(feature = "tokio")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use descartes_core::{Executor, SimTime, Simulation, SimulationConfig};

use descartes_explore::harness::{
    run_controlled, HarnessConfig, HarnessControl, HarnessTokioReadyConfig, HarnessTokioReadyPolicy,
};
use descartes_explore::io::TraceFormat;
use descartes_explore::schedule_explore::{DecisionKey, DecisionKind, DecisionScript};
use descartes_explore::trace::TraceEvent;

static TMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_path(suffix: &str) -> PathBuf {
    let n = TMP_ID.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "descartes_explore_harness_tokio_toggles_off_{}_{}{}",
        std::process::id(),
        n,
        suffix
    ));
    p
}

fn spawn_two_ready_tasks() {
    for _ in 0..2 {
        descartes_tokio::task::spawn(async move {
            // Encourage both tasks to be ready at the same time.
            descartes_tokio::thread::yield_now().await;
        });
    }
}

#[test]
fn run_controlled_tokio_ready_toggle_off_ignores_decision_script() {
    // `descartes_tokio::runtime::install*` uses thread-local state, so isolate runs.
    let d1 = std::thread::spawn(|| {
        let trace_path1 = temp_path(".json");
        let cfg1 = HarnessConfig {
            sim_config: SimulationConfig { seed: 7 },
            scenario: "controlled_tokio_ready_toggle_off_discover".to_string(),
            install_tokio: true,
            tokio_ready: Some(HarnessTokioReadyConfig {
                policy: HarnessTokioReadyPolicy::Fifo,
                record_decisions: true,
            }),
            tokio_mutex: None,
            record_concurrency: false,
            frontier: None,
            trace_path: trace_path1.clone(),
            trace_format: TraceFormat::Json,
        };

        let ((), trace1) = run_controlled(
            cfg1,
            HarnessControl {
                decision_script: None,
                explore_frontier: false,
                explore_tokio_ready: false,
            },
            None,
            |sim_config, _ctx, _prefix| Simulation::new(sim_config),
            |sim, _ctx| {
                spawn_two_ready_tasks();
                sim.execute(Executor::timed(SimTime::from_duration(
                    Duration::from_millis(1),
                )));
            },
        )
        .unwrap();

        std::fs::remove_file(&trace_path1).ok();

        trace1
            .events
            .into_iter()
            .find_map(|e| match e {
                TraceEvent::AsyncRuntimeDecision(d) if d.ready_task_ids.len() >= 2 => Some(d),
                _ => None,
            })
            .expect("expected AsyncRuntimeDecision when record_decisions=true")
    })
    .join()
    .unwrap();

    let chosen = d1
        .chosen_task_id
        .expect("chosen_task_id should be recorded");
    let forced = *d1
        .ready_task_ids
        .iter()
        .find(|&&id| id != chosen)
        .expect("expected 2+ ready tasks");

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

    let d2 = std::thread::spawn(move || {
        let trace_path2 = temp_path(".json");
        let cfg2 = HarnessConfig {
            sim_config: SimulationConfig { seed: 7 },
            scenario: "controlled_tokio_ready_toggle_off_script".to_string(),
            install_tokio: true,
            tokio_ready: Some(HarnessTokioReadyConfig {
                policy: HarnessTokioReadyPolicy::Fifo,
                record_decisions: true,
            }),
            tokio_mutex: None,
            record_concurrency: false,
            frontier: None,
            trace_path: trace_path2.clone(),
            trace_format: TraceFormat::Json,
        };

        let ((), trace2) = run_controlled(
            cfg2,
            HarnessControl {
                decision_script: Some(Arc::new(script)),
                explore_frontier: false,
                explore_tokio_ready: false,
            },
            None,
            |sim_config, _ctx, _prefix| Simulation::new(sim_config),
            |sim, _ctx| {
                spawn_two_ready_tasks();
                sim.execute(Executor::timed(SimTime::from_duration(
                    Duration::from_millis(1),
                )));
            },
        )
        .unwrap();

        std::fs::remove_file(&trace_path2).ok();

        trace2
            .events
            .into_iter()
            .find_map(|e| match e {
                TraceEvent::AsyncRuntimeDecision(d) if d.ready_task_ids.len() >= 2 => Some(d),
                _ => None,
            })
            .expect("expected AsyncRuntimeDecision when record_decisions=true")
    })
    .join()
    .unwrap();

    // Script should be ignored when explore_tokio_ready=false.
    assert_eq!(d2.chosen_task_id, Some(chosen));
    assert_ne!(d2.chosen_task_id, Some(forced));
}

#[test]
fn run_controlled_tokio_ready_toggle_off_and_no_recording_emits_no_decisions() {
    std::thread::spawn(|| {
        let trace_path = temp_path(".json");
        let cfg = HarnessConfig {
            sim_config: SimulationConfig { seed: 7 },
            scenario: "controlled_tokio_ready_toggle_off_no_record".to_string(),
            install_tokio: true,
            tokio_ready: Some(HarnessTokioReadyConfig {
                policy: HarnessTokioReadyPolicy::Fifo,
                record_decisions: false,
            }),
            tokio_mutex: None,
            record_concurrency: false,
            frontier: None,
            trace_path: trace_path.clone(),
            trace_format: TraceFormat::Json,
        };

        let ((), trace) = run_controlled(
            cfg,
            HarnessControl {
                decision_script: None,
                explore_frontier: false,
                explore_tokio_ready: false,
            },
            None,
            |sim_config, _ctx, _prefix| Simulation::new(sim_config),
            |sim, _ctx| {
                spawn_two_ready_tasks();
                sim.execute(Executor::timed(SimTime::from_duration(
                    Duration::from_millis(1),
                )));
            },
        )
        .unwrap();

        std::fs::remove_file(&trace_path).ok();

        assert!(
            !trace
                .events
                .iter()
                .any(|e| matches!(e, TraceEvent::AsyncRuntimeDecision(_))),
            "expected no AsyncRuntimeDecision events when not exploring/recording"
        );
    })
    .join()
    .unwrap();
}
