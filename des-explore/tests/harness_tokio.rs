use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use des_core::dists::{ArrivalPattern, PoissonArrivals};
use des_core::{Component, Executor, Key, SimTime, Simulation, SimulationConfig};
use des_explore::harness::{run_recorded, HarnessConfig};
use des_explore::io::{read_trace_from_path, TraceFormat, TraceIoConfig};
use des_explore::trace::TraceEvent;

#[derive(Debug)]
enum Event {
    Tick,
    Stop,
}

struct Driver {
    arrivals: PoissonArrivals,
    stop_flag: Arc<AtomicBool>,
}

impl Component for Driver {
    type Event = Event;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut des_core::Scheduler,
    ) {
        match event {
            Event::Tick => {
                let dt = self.arrivals.next_arrival_time();
                scheduler.schedule(SimTime::from_duration(dt), self_id, Event::Tick);
            }
            Event::Stop => {
                // no-op
                let _ = self.stop_flag.load(Ordering::Relaxed);
            }
        }
    }
}

fn temp_path(suffix: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "des_explore_harness_{}_{}{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        suffix
    ));
    p
}

/// Smoke test for the harness + tokio integration (JSON encoding).
///
/// Verifies that:
/// - a trace is written to disk and can be read back
/// - at least one RNG draw was recorded
/// - an async task scheduled on `des-tokio` runs to completion
#[test]
fn harness_records_trace_and_runs_tokio_tasks_json() {
    let trace_path = temp_path(".json");
    let ran = Arc::new(AtomicBool::new(false));

    let cfg = HarnessConfig {
        sim_config: SimulationConfig { seed: 7 },
        scenario: "harness_tokio_json".to_string(),
        install_tokio: true,
        trace_path: trace_path.clone(),
        trace_format: TraceFormat::Json,
    };

    let ran2 = ran.clone();
    let ran_for_run = ran.clone();

    let ((), trace) = run_recorded(
        cfg,
        move |sim_config, ctx| {
            let mut sim = Simulation::new(sim_config);

            // Record at least one RNG draw via the provider.
            let provider = ctx.tracing_provider(sim.config().seed ^ 0x1111);
            let arrivals = PoissonArrivals::from_config(sim.config(), 1.0)
                .with_provider(provider, des_core::draw_site!("arrival"));

            let key = sim.add_component(Driver {
                arrivals,
                stop_flag: ran2.clone(),
            });

            sim.schedule_now(key, Event::Tick);
            sim
        },
        move |sim, _ctx| {
            // Exercise des-tokio runtime integration.
            let ran_task = ran_for_run.clone();
            des_tokio::task::spawn(async move {
                des_tokio::time::sleep(Duration::from_millis(1)).await;
                ran_task.store(true, Ordering::Relaxed);
            });

            sim.execute(Executor::timed(SimTime::from_duration(
                Duration::from_millis(10),
            )));
        },
    )
    .unwrap();

    assert!(ran.load(Ordering::Relaxed));

    assert!(trace_path.exists());

    let read_back = read_trace_from_path(
        &trace_path,
        TraceIoConfig {
            format: TraceFormat::Json,
        },
    )
    .unwrap();

    std::fs::remove_file(&trace_path).ok();

    let draw_count = read_back
        .events
        .iter()
        .filter(|e| matches!(e, TraceEvent::RandomDraw(_)))
        .count();

    assert!(draw_count > 0);
    assert_eq!(trace.meta.scenario, "harness_tokio_json");
}

/// Smoke test for the harness + tokio integration (postcard encoding).
///
/// Verifies that we can persist traces in a compact binary format.
#[test]
fn harness_records_trace_and_runs_tokio_tasks_postcard() {
    let trace_path = temp_path(".bin");

    let cfg = HarnessConfig {
        sim_config: SimulationConfig { seed: 9 },
        scenario: "harness_tokio_postcard".to_string(),
        install_tokio: true,
        trace_path: trace_path.clone(),
        trace_format: TraceFormat::Postcard,
    };

    let (_result, _trace) = run_recorded(
        cfg,
        move |sim_config, ctx| {
            let mut sim = Simulation::new(sim_config);

            let provider = ctx.tracing_provider(sim.config().seed ^ 0x2222);
            let arrivals = PoissonArrivals::from_config(sim.config(), 1.0)
                .with_provider(provider, des_core::draw_site!("arrival"));

            let key = sim.add_component(Driver {
                arrivals,
                stop_flag: Arc::new(AtomicBool::new(false)),
            });

            sim.schedule_now(key, Event::Tick);
            sim
        },
        move |sim, _ctx| {
            des_tokio::task::spawn(async move {
                des_tokio::time::sleep(Duration::from_millis(1)).await;
            });
            sim.execute(Executor::timed(SimTime::from_duration(
                Duration::from_millis(10),
            )));
        },
    )
    .unwrap();

    assert!(trace_path.exists());

    let read_back = read_trace_from_path(
        &trace_path,
        TraceIoConfig {
            format: TraceFormat::Postcard,
        },
    )
    .unwrap();

    std::fs::remove_file(&trace_path).ok();

    let draw_count = read_back
        .events
        .iter()
        .filter(|e| matches!(e, TraceEvent::RandomDraw(_)))
        .count();

    assert!(draw_count > 0);
    assert_eq!(read_back.meta.scenario, "harness_tokio_postcard");
}
