use std::sync::{Arc, Mutex as StdMutex};

use descartes_core::{Execute, Executor, SimTime, Simulation, SimulationConfig};
use descartes_explore::prelude::*;
use descartes_tokio::stream::StreamExt;

#[test]
fn mpsc_stream_combinators_are_replayable() {
    let scenario = "mpsc_stream_combinators_record_replay".to_string();

    let (trace, recorded_out) = std::thread::spawn({
        let scenario = scenario.clone();
        move || {
            let trace_path = std::env::temp_dir().join(format!(
                "descartes_explore_mpsc_stream_comb_record_{}_{}.json",
                std::process::id(),
                scenario
            ));

            let cfg = HarnessConfig {
                sim_config: SimulationConfig { seed: 1 },
                scenario: scenario.clone(),
                install_tokio: true,
                tokio_ready: Some(HarnessTokioReadyConfig {
                    policy: HarnessTokioReadyPolicy::UniformRandom { seed: 1234 },
                    record_decisions: true,
                }),
                tokio_mutex: None,
                record_concurrency: true,
                frontier: None,
                trace_path: trace_path.clone(),
                trace_format: TraceFormat::Json,
            };

            let out: Arc<StdMutex<Option<Vec<u64>>>> = Arc::new(StdMutex::new(None));
            let out2 = out.clone();

            let ((), trace) = run_recorded(
                cfg,
                move |sim_config, _ctx| Simulation::new(sim_config),
                move |sim, _ctx| {
                    let (tx, rx) = descartes_tokio::sync::mpsc::channel::<u64>(16);

                    let _producer = descartes_tokio::thread::spawn(async move {
                        for i in 0..20u64 {
                            if tx.send(i).await.is_err() {
                                break;
                            }
                            descartes_tokio::thread::yield_now().await;
                        }
                        drop(tx);
                    });

                    let out2 = out2.clone();
                    descartes_tokio::thread::spawn(async move {
                        let v = rx
                            .map(|x| x + 1)
                            .filter(|x| *x % 3 == 0)
                            .take(5)
                            .collect::<Vec<_>>()
                            .await;

                        *out2.lock().unwrap() = Some(v);
                    });

                    Executor::timed(SimTime::from_millis(10)).execute(sim);
                },
            )
            .unwrap();

            std::fs::remove_file(&trace_path).ok();

            let recorded_out = out.lock().unwrap().clone().expect("set");
            (trace, recorded_out)
        }
    })
    .join()
    .unwrap();

    let replayed_out = std::thread::spawn(move || {
        let trace_path = std::env::temp_dir().join(format!(
            "descartes_explore_mpsc_stream_comb_replay_{}_{}.json",
            std::process::id(),
            scenario
        ));

        let cfg = HarnessConfig {
            sim_config: SimulationConfig { seed: 1 },
            scenario,
            install_tokio: true,
            tokio_ready: None,
            tokio_mutex: None,
            record_concurrency: false,
            frontier: None,
            trace_path: trace_path.clone(),
            trace_format: TraceFormat::Json,
        };

        let out: Arc<StdMutex<Option<Vec<u64>>>> = Arc::new(StdMutex::new(None));
        let out2 = out.clone();

        run_replayed(
            cfg,
            &trace,
            move |sim_config, _ctx, _input| Simulation::new(sim_config),
            move |sim, _ctx| {
                let (tx, rx) = descartes_tokio::sync::mpsc::channel::<u64>(16);

                let _producer = descartes_tokio::thread::spawn(async move {
                    for i in 0..20u64 {
                        if tx.send(i).await.is_err() {
                            break;
                        }
                        descartes_tokio::thread::yield_now().await;
                    }
                    drop(tx);
                });

                let out2 = out2.clone();
                descartes_tokio::thread::spawn(async move {
                    let v = rx
                        .map(|x| x + 1)
                        .filter(|x| *x % 3 == 0)
                        .take(5)
                        .collect::<Vec<_>>()
                        .await;

                    *out2.lock().unwrap() = Some(v);
                });

                Executor::timed(SimTime::from_millis(10)).execute(sim);
            },
        )
        .unwrap();

        std::fs::remove_file(&trace_path).ok();

        let out = out.lock().unwrap().clone().expect("set");
        out
    })
    .join()
    .unwrap();

    assert_eq!(recorded_out, replayed_out);
}
