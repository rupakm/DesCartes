use std::sync::{Arc, Mutex as StdMutex};

use des_core::{Execute, Executor, SimTime, Simulation, SimulationConfig};
use des_explore::prelude::*;
use des_tokio::stream::StreamExt;

#[test]
fn mpsc_stream_is_replayable_under_random_scheduling() {
    let scenario = "mpsc_stream_record_replay".to_string();

    let (trace, recorded_out) = std::thread::spawn({
        let scenario = scenario.clone();
        move || {
            let trace_path = std::env::temp_dir().join(format!(
                "des_explore_mpsc_stream_record_{}_{}.json",
                std::process::id(),
                scenario
            ));

            let cfg = HarnessConfig {
                sim_config: SimulationConfig { seed: 1 },
                scenario: scenario.clone(),
                install_tokio: true,
                tokio_ready: Some(HarnessTokioReadyConfig {
                    policy: HarnessTokioReadyPolicy::UniformRandom { seed: 77 },
                    record_decisions: true,
                }),
                tokio_mutex: None,
                record_concurrency: false,
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
                    let (tx, mut rx) = des_tokio::sync::mpsc::channel::<u64>(8);

                    let producer = des_tokio::thread::spawn(async move {
                        for i in 0..10u64 {
                            tx.send(i).await.unwrap();
                            des_tokio::thread::yield_now().await;
                        }
                        drop(tx);
                    });

                    let consumer = des_tokio::thread::spawn(async move {
                        let mut v = Vec::new();
                        while let Some(x) = rx.next().await {
                            v.push(x);
                            des_tokio::thread::yield_now().await;
                        }
                        v
                    });

                    let out2 = out2.clone();
                    des_tokio::thread::spawn(async move {
                        producer.await.expect("producer");
                        let v = consumer.await.expect("consumer");
                        *out2.lock().unwrap() = Some(v);
                    });

                    Executor::timed(SimTime::from_millis(5)).execute(sim);
                },
            )
            .unwrap();

            std::fs::remove_file(&trace_path).ok();

            let recorded_out = out.lock().unwrap().clone().expect("set");
            assert_eq!(recorded_out, (0u64..10).collect::<Vec<_>>());

            (trace, recorded_out)
        }
    })
    .join()
    .unwrap();

    let replayed_out = std::thread::spawn(move || {
        let trace_path = std::env::temp_dir().join(format!(
            "des_explore_mpsc_stream_replay_{}_{}.json",
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
                let (tx, mut rx) = des_tokio::sync::mpsc::channel::<u64>(8);

                let producer = des_tokio::thread::spawn(async move {
                    for i in 0..10u64 {
                        tx.send(i).await.unwrap();
                        des_tokio::thread::yield_now().await;
                    }
                    drop(tx);
                });

                let consumer = des_tokio::thread::spawn(async move {
                    let mut v = Vec::new();
                    while let Some(x) = rx.next().await {
                        v.push(x);
                        des_tokio::thread::yield_now().await;
                    }
                    v
                });

                let out2 = out2.clone();
                des_tokio::thread::spawn(async move {
                    producer.await.expect("producer");
                    let v = consumer.await.expect("consumer");
                    *out2.lock().unwrap() = Some(v);
                });

                Executor::timed(SimTime::from_millis(5)).execute(sim);
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
