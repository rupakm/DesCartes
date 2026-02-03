use std::path::PathBuf;
use std::sync::Arc;

use descartes_core::{Component, Key, Simulation, SimulationConfig};
use descartes_explore::harness::{run_controlled, HarnessConfig, HarnessControl};
use descartes_explore::io::TraceFormat;
use descartes_explore::schedule_explore::{DecisionKey, DecisionKind, DecisionScript};
use descartes_explore::trace::TraceEvent;

#[derive(Debug)]
enum Ev {
    Tick,
}

#[derive(Debug)]
struct Noop;

impl Component for Noop {
    type Event = Ev;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        _event: &Self::Event,
        _scheduler: &mut descartes_core::Scheduler,
    ) {
    }
}

fn temp_path(suffix: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "decartess_explore_harness_controlled_{}_{}{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        suffix
    ));
    p
}

fn setup(sim_config: SimulationConfig) -> Simulation {
    let mut sim = Simulation::new(sim_config);

    let a = sim.add_component(Noop);
    let b = sim.add_component(Noop);

    sim.schedule_now(a, Ev::Tick);
    sim.schedule_now(b, Ev::Tick);

    sim
}

#[test]
fn run_controlled_frontier_respects_script_choice() {
    let trace_path1 = temp_path(".json");
    let cfg1 = HarnessConfig {
        sim_config: SimulationConfig { seed: 1 },
        scenario: "controlled_frontier_1".to_string(),
        install_tokio: false,
        tokio_ready: None,
        tokio_mutex: None,
        record_concurrency: false,
        frontier: None,
        trace_path: trace_path1.clone(),
        trace_format: TraceFormat::Json,
    };

    let control1 = HarnessControl {
        decision_script: None,
        explore_frontier: true,
        explore_tokio_ready: false,
    };

    let ((), trace1) = run_controlled(
        cfg1,
        control1,
        None,
        |sim_config, _ctx, _prefix| setup(sim_config),
        |sim, _ctx| {
            sim.step();
        },
    )
    .unwrap();

    std::fs::remove_file(&trace_path1).ok();

    let d1 = trace1
        .events
        .iter()
        .find_map(|e| match e {
            TraceEvent::SchedulerDecision(d) => Some(d.clone()),
            _ => None,
        })
        .expect("expected at least one SchedulerDecision");

    assert!(d1.frontier_seqs.len() >= 2);
    let chosen = d1.chosen_seq.expect("chosen_seq should be recorded");
    let forced = *d1.frontier_seqs.iter().find(|&&seq| seq != chosen).unwrap();

    let mut script = DecisionScript::new();
    script.insert(
        DecisionKey::new(
            DecisionKind::Frontier,
            d1.time_nanos,
            0,
            d1.frontier_seqs.clone(),
        ),
        forced,
    );

    let trace_path2 = temp_path(".json");
    let cfg2 = HarnessConfig {
        sim_config: SimulationConfig { seed: 1 },
        scenario: "controlled_frontier_2".to_string(),
        install_tokio: false,
        tokio_ready: None,
        tokio_mutex: None,
        record_concurrency: false,
        frontier: None,
        trace_path: trace_path2.clone(),
        trace_format: TraceFormat::Json,
    };

    let control2 = HarnessControl {
        decision_script: Some(Arc::new(script)),
        explore_frontier: true,
        explore_tokio_ready: false,
    };

    let ((), trace2) = run_controlled(
        cfg2,
        control2,
        None,
        |sim_config, _ctx, _prefix| setup(sim_config),
        |sim, _ctx| {
            sim.step();
        },
    )
    .unwrap();

    std::fs::remove_file(&trace_path2).ok();

    let d2 = trace2
        .events
        .iter()
        .find_map(|e| match e {
            TraceEvent::SchedulerDecision(d) => Some(d.clone()),
            _ => None,
        })
        .expect("expected at least one SchedulerDecision");

    assert_eq!(d2.chosen_seq, Some(forced));
}

#[test]
fn run_controlled_frontier_falls_back_on_invalid_forced_choice() {
    let trace_path = temp_path(".json");

    let cfg = HarnessConfig {
        sim_config: SimulationConfig { seed: 1 },
        scenario: "controlled_frontier_invalid".to_string(),
        install_tokio: false,
        tokio_ready: None,
        tokio_mutex: None,
        record_concurrency: false,
        frontier: None,
        trace_path: trace_path.clone(),
        trace_format: TraceFormat::Json,
    };

    // Discover the decision key.
    let ((), trace1) = run_controlled(
        HarnessConfig {
            trace_path: temp_path(".json"),
            scenario: "controlled_frontier_discover".to_string(),
            ..cfg.clone()
        },
        HarnessControl {
            decision_script: None,
            explore_frontier: true,
            explore_tokio_ready: false,
        },
        None,
        |sim_config, _ctx, _prefix| setup(sim_config),
        |sim, _ctx| {
            sim.step();
        },
    )
    .unwrap();

    let d1 = trace1
        .events
        .iter()
        .find_map(|e| match e {
            TraceEvent::SchedulerDecision(d) => Some(d.clone()),
            _ => None,
        })
        .expect("expected at least one SchedulerDecision");

    let mut script = DecisionScript::new();
    script.insert(
        DecisionKey::new(
            DecisionKind::Frontier,
            d1.time_nanos,
            0,
            d1.frontier_seqs.clone(),
        ),
        1_000_000_000,
    );

    let (_result, trace2) = run_controlled(
        cfg,
        HarnessControl {
            decision_script: Some(Arc::new(script)),
            explore_frontier: true,
            explore_tokio_ready: false,
        },
        None,
        |sim_config, _ctx, _prefix| setup(sim_config),
        |sim, _ctx| {
            sim.step();
        },
    )
    .unwrap();

    std::fs::remove_file(&trace_path).ok();

    let d2 = trace2
        .events
        .iter()
        .find_map(|e| match e {
            TraceEvent::SchedulerDecision(d) => Some(d.clone()),
            _ => None,
        })
        .expect("expected a SchedulerDecision");

    // Invalid forced choice should not be taken.
    assert_ne!(d2.chosen_seq, Some(1_000_000_000));
}
