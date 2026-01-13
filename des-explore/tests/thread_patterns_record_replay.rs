use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex as StdMutex};

use des_core::{Execute, Executor, SimTime, Simulation, SimulationConfig};
use des_explore::prelude::*;
use des_explore::trace::TraceEvent;

#[derive(Debug, Clone, Copy)]
enum Pattern {
    MutexCounter,
    AtomicFetchAddCounter,
    RacyLoadStoreCounter,
    SpinLockCounter,
    TicketLockCounter,
}

fn run_pattern_record_and_replay(pattern: Pattern, schedule_seed: u64) {
    let scenario = format!("thread_patterns_{pattern:?}_{schedule_seed}");

    // Record in one OS thread (des_tokio uses TLS for install).
    let (trace, recorded_result) = std::thread::spawn({
        let scenario = scenario.clone();
        move || {
            let trace_path = std::env::temp_dir().join(format!(
                "des_explore_thread_patterns_record_{}_{}.json",
                std::process::id(),
                scenario
            ));

            let cfg = HarnessConfig {
                sim_config: SimulationConfig { seed: 1 },
                scenario: scenario.clone(),
                install_tokio: true,
                tokio_ready: Some(HarnessTokioReadyConfig {
                    policy: HarnessTokioReadyPolicy::UniformRandom {
                        seed: schedule_seed,
                    },
                    record_decisions: true,
                }),
                tokio_mutex: None,
                record_concurrency: true,
                frontier: None,
                trace_path: trace_path.clone(),
                trace_format: TraceFormat::Json,
            };

            let (result, trace) = run_recorded(
                cfg,
                move |sim_config, _ctx| Simulation::new(sim_config),
                move |sim, _ctx| run_pattern(pattern, sim),
            )
            .unwrap();

            std::fs::remove_file(&trace_path).ok();

            assert!(
                trace
                    .events
                    .iter()
                    .any(|e| matches!(e, TraceEvent::Concurrency(_))),
                "expected concurrency events in trace"
            );

            (trace, result)
        }
    })
    .join()
    .unwrap();

    // Replay in a separate OS thread.
    let replayed_result = std::thread::spawn(move || {
        let trace_path = std::env::temp_dir().join(format!(
            "des_explore_thread_patterns_replay_{}_{}.json",
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

        let (result, _trace2) = run_replayed(
            cfg,
            &trace,
            move |sim_config, _ctx, _input| Simulation::new(sim_config),
            move |sim, _ctx| run_pattern(pattern, sim),
        )
        .unwrap();

        std::fs::remove_file(&trace_path).ok();
        result
    })
    .join()
    .unwrap();

    assert_eq!(
        recorded_result, replayed_result,
        "replay result differs for {pattern:?}"
    );
}

fn run_pattern(pattern: Pattern, sim: &mut Simulation) -> u64 {
    // Each pattern spawns worker tasks and a coordinator task.
    // The coordinator awaits all workers and captures the final state from within
    // the runtime (so we don't need to read mutex state outside polling).
    let result: Arc<StdMutex<Option<u64>>> = Arc::new(StdMutex::new(None));

    match pattern {
        Pattern::MutexCounter => {
            let mutex_id = des_tokio::stable_id!("thread_patterns", "mutex_counter");
            let counter = des_tokio::sync::Mutex::new_with_id(mutex_id, 0u64);

            let tasks = 8;
            let iters = 50;
            let mut handles = Vec::new();

            for _ in 0..tasks {
                let c = counter.clone();
                handles.push(des_tokio::thread::spawn(async move {
                    for _ in 0..iters {
                        let mut g = c.lock().await;
                        *g += 1;
                        drop(g);
                        des_tokio::thread::yield_now().await;
                    }
                }));
            }

            let result_out = result.clone();
            des_tokio::thread::spawn(async move {
                for h in handles {
                    h.await.expect("worker should complete");
                }
                let g = counter.lock().await;
                *result_out.lock().unwrap() = Some(*g);
            });

            Executor::timed(SimTime::from_millis(10)).execute(sim);
        }

        Pattern::AtomicFetchAddCounter => {
            let site_id = des_tokio::stable_id!("thread_patterns", "atomic_fetch_add");
            let counter = Arc::new(des_tokio::sync::AtomicU64::new(site_id, 0));

            let tasks = 8;
            let iters = 100;
            let mut handles = Vec::new();

            for _ in 0..tasks {
                let c = counter.clone();
                handles.push(des_tokio::thread::spawn(async move {
                    for _ in 0..iters {
                        let _ = c.fetch_add(1, Ordering::SeqCst);
                        des_tokio::thread::yield_now().await;
                    }
                }));
            }

            let result_out = result.clone();
            des_tokio::thread::spawn(async move {
                for h in handles {
                    h.await.expect("worker should complete");
                }
                let v = counter.load(Ordering::SeqCst);
                *result_out.lock().unwrap() = Some(v);
            });

            Executor::timed(SimTime::from_millis(10)).execute(sim);
        }

        Pattern::RacyLoadStoreCounter => {
            let site_id = des_tokio::stable_id!("thread_patterns", "atomic_load_store");
            let counter = Arc::new(des_tokio::sync::AtomicU64::new(site_id, 0));

            // Intentionally racy (non-RMW) increment: load + yield + store.
            let tasks = 8;
            let iters = 50;
            let mut handles = Vec::new();

            for _ in 0..tasks {
                let c = counter.clone();
                handles.push(des_tokio::thread::spawn(async move {
                    for _ in 0..iters {
                        let v = c.load(Ordering::SeqCst);
                        des_tokio::thread::yield_now().await;
                        c.store(v + 1, Ordering::SeqCst);
                        des_tokio::thread::yield_now().await;
                    }
                }));
            }

            let result_out = result.clone();
            des_tokio::thread::spawn(async move {
                for h in handles {
                    h.await.expect("worker should complete");
                }
                let v = counter.load(Ordering::SeqCst);
                *result_out.lock().unwrap() = Some(v);
            });

            Executor::timed(SimTime::from_millis(10)).execute(sim);
        }

        Pattern::SpinLockCounter => {
            let lock_id = des_tokio::stable_id!("thread_patterns", "spin_lock");
            let value_id = des_tokio::stable_id!("thread_patterns", "spin_lock_value");

            let lock = Arc::new(des_tokio::sync::AtomicU64::new(lock_id, 0));
            let value = Arc::new(des_tokio::sync::AtomicU64::new(value_id, 0));

            async fn acquire(lock: &des_tokio::sync::AtomicU64) {
                loop {
                    if lock
                        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        return;
                    }
                    des_tokio::thread::yield_now().await;
                }
            }

            async fn release(lock: &des_tokio::sync::AtomicU64) {
                lock.store(0, Ordering::SeqCst);
                des_tokio::thread::yield_now().await;
            }

            let tasks = 6;
            let iters = 40;
            let mut handles = Vec::new();

            for _ in 0..tasks {
                let lock = lock.clone();
                let value = value.clone();
                handles.push(des_tokio::thread::spawn(async move {
                    for _ in 0..iters {
                        acquire(&lock).await;
                        let _ = value.fetch_add(1, Ordering::SeqCst);
                        release(&lock).await;
                    }
                }));
            }

            let result_out = result.clone();
            des_tokio::thread::spawn(async move {
                for h in handles {
                    h.await.expect("worker should complete");
                }
                let v = value.load(Ordering::SeqCst);
                *result_out.lock().unwrap() = Some(v);
            });

            Executor::timed(SimTime::from_millis(10)).execute(sim);
        }

        Pattern::TicketLockCounter => {
            let next_id = des_tokio::stable_id!("thread_patterns", "ticket_next");
            let serving_id = des_tokio::stable_id!("thread_patterns", "ticket_serving");
            let value_id = des_tokio::stable_id!("thread_patterns", "ticket_value");

            let next = Arc::new(des_tokio::sync::AtomicU64::new(next_id, 0));
            let serving = Arc::new(des_tokio::sync::AtomicU64::new(serving_id, 0));
            let value = Arc::new(des_tokio::sync::AtomicU64::new(value_id, 0));

            async fn acquire(
                next: &des_tokio::sync::AtomicU64,
                serving: &des_tokio::sync::AtomicU64,
            ) {
                let my = next.fetch_add(1, Ordering::SeqCst);
                loop {
                    if serving.load(Ordering::SeqCst) == my {
                        return;
                    }
                    des_tokio::thread::yield_now().await;
                }
            }

            async fn release(serving: &des_tokio::sync::AtomicU64) {
                let _ = serving.fetch_add(1, Ordering::SeqCst);
                des_tokio::thread::yield_now().await;
            }

            let tasks = 6;
            let iters = 40;
            let mut handles = Vec::new();

            for _ in 0..tasks {
                let next = next.clone();
                let serving = serving.clone();
                let value = value.clone();
                handles.push(des_tokio::thread::spawn(async move {
                    for _ in 0..iters {
                        acquire(&next, &serving).await;
                        let _ = value.fetch_add(1, Ordering::SeqCst);
                        release(&serving).await;
                    }
                }));
            }

            let result_out = result.clone();
            des_tokio::thread::spawn(async move {
                for h in handles {
                    h.await.expect("worker should complete");
                }
                let v = value.load(Ordering::SeqCst);
                *result_out.lock().unwrap() = Some(v);
            });

            Executor::timed(SimTime::from_millis(10)).execute(sim);
        }
    }

    let out = result
        .lock()
        .unwrap()
        .take()
        .expect("coordinator should set result");
    out
}

#[test]
fn thread_patterns_random_schedule_are_replayable() {
    let patterns = [
        Pattern::MutexCounter,
        Pattern::AtomicFetchAddCounter,
        Pattern::RacyLoadStoreCounter,
        Pattern::SpinLockCounter,
        Pattern::TicketLockCounter,
    ];

    // Run each pattern across multiple deterministic schedule seeds.
    let seeds = [10u64, 11, 12, 13, 14];

    for p in patterns {
        for seed in seeds {
            run_pattern_record_and_replay(p, seed);
        }
    }
}
