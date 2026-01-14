use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use des_core::{Component, Key, Simulation, SimulationConfig};

use des_explore::harness::{
    run_controlled, HarnessConfig, HarnessControl, HarnessFrontierConfig, HarnessFrontierPolicy,
};
use des_explore::io::TraceFormat;
use des_explore::schedule_explore::{DecisionKey, DecisionKind, DecisionScript};
use des_explore::trace::TraceEvent;

static TMP_ID: AtomicUsize = AtomicUsize::new(0);

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
        _scheduler: &mut des_core::Scheduler,
    ) {
    }
}

fn temp_path(suffix: &str) -> PathBuf {
    let n = TMP_ID.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "des_explore_harness_toggles_off_{}_{}{}",
        std::process::id(),
        n,
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
fn run_controlled_frontier_toggle_off_ignores_decision_script() {
    // First, record the baseline choice with recording enabled.
    let trace_path1 = temp_path(".json");
    let cfg1 = HarnessConfig {
        sim_config: SimulationConfig { seed: 1 },
        scenario: "controlled_frontier_toggle_off_discover".to_string(),
        install_tokio: false,
        tokio_ready: None,
        tokio_mutex: None,
        record_concurrency: false,
        frontier: Some(HarnessFrontierConfig {
            policy: HarnessFrontierPolicy::Fifo,
            record_decisions: true,
        }),
        trace_path: trace_path1.clone(),
        trace_format: TraceFormat::Json,
    };
    let cfg_base = cfg1.clone();

    let ((), trace1) = run_controlled(
        cfg1,
        HarnessControl {
            decision_script: None,
            explore_frontier: false,
            explore_tokio_ready: false,
        },
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
        .expect("expected SchedulerDecision when record_decisions=true");

    let chosen = d1.chosen_seq.expect("chosen_seq should be recorded");
    let forced = *d1
        .frontier_seqs
        .iter()
        .find(|&&seq| seq != chosen)
        .expect("expected 2+ frontier seqs");

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

    // Now run again with the script, but keep exploration disabled.
    let trace_path2 = temp_path(".json");
    let cfg2 = HarnessConfig {
        trace_path: trace_path2.clone(),
        scenario: "controlled_frontier_toggle_off_script".to_string(),
        ..cfg_base
    };

    let ((), trace2) = run_controlled(
        cfg2,
        HarnessControl {
            decision_script: Some(Arc::new(script)),
            explore_frontier: false,
            explore_tokio_ready: false,
        },
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
        .expect("expected SchedulerDecision when record_decisions=true");

    // Script should be ignored when explore_frontier=false.
    assert_eq!(d2.chosen_seq, Some(chosen));
    assert_ne!(d2.chosen_seq, Some(forced));
}

#[test]
fn run_controlled_without_exploration_or_recording_emits_no_decisions() {
    let trace_path = temp_path(".json");

    let cfg = HarnessConfig {
        sim_config: SimulationConfig { seed: 1 },
        scenario: "controlled_no_decisions".to_string(),
        install_tokio: false,
        tokio_ready: None,
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
        |sim_config, _ctx, _prefix| setup(sim_config),
        |sim, _ctx| {
            sim.step();
        },
    )
    .unwrap();

    std::fs::remove_file(&trace_path).ok();

    assert!(
        !trace
            .events
            .iter()
            .any(|e| matches!(e, TraceEvent::SchedulerDecision(_))),
        "expected no SchedulerDecision events when not exploring/recording"
    );
}
