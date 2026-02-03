use std::sync::{Arc, Mutex};
use std::time::Duration;

use descartes_core::{Execute, Executor, SimTime, Simulation};

#[test]
fn select_prefers_message_when_it_arrives_first() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let (tx, mut rx) = descartes_tokio::sync::mpsc::channel::<u32>(1);
    let stop = Arc::new(descartes_tokio::sync::notify::Notify::new());

    let out = Arc::new(Mutex::new(0u32));

    let stop_server = stop.clone();
    let out_server = out.clone();
    descartes_tokio::task::spawn(async move {
        // Tokio's `select!` is *not* deterministic by default; it randomizes the
        // polling order for fairness. We opt into `biased;` to preserve stable
        // behavior in the DES runtime.
        tokio::select! {
            biased;
            msg = rx.recv() => {
                if let Some(v) = msg {
                    *out_server.lock().unwrap() = v;
                }
            }
            _ = stop_server.notified() => {
                // Stop wins.
            }
        }
    });

    descartes_tokio::task::spawn(async move {
        descartes_tokio::time::sleep(Duration::from_millis(5)).await;
        let _ = tx.send(42).await;
    });

    let stop_notifier = stop.clone();
    descartes_tokio::task::spawn(async move {
        descartes_tokio::time::sleep(Duration::from_millis(10)).await;
        stop_notifier.notify_one();
    });

    Executor::timed(SimTime::from_duration(Duration::from_millis(50))).execute(&mut sim);

    assert_eq!(*out.lock().unwrap(), 42);
}

#[test]
fn select_prefers_stop_when_it_arrives_first() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let (tx, mut rx) = descartes_tokio::sync::mpsc::channel::<u32>(1);
    let stop = Arc::new(descartes_tokio::sync::notify::Notify::new());

    let out = Arc::new(Mutex::new(0u32));

    let stop_server = stop.clone();
    let out_server = out.clone();
    descartes_tokio::task::spawn(async move {
        tokio::select! {
            biased;
            msg = rx.recv() => {
                if let Some(v) = msg {
                    *out_server.lock().unwrap() = v;
                }
            }
            _ = stop_server.notified() => {
                // Stop wins.
            }
        }
    });

    let stop_notifier = stop.clone();
    descartes_tokio::task::spawn(async move {
        descartes_tokio::time::sleep(Duration::from_millis(5)).await;
        stop_notifier.notify_one();
    });

    descartes_tokio::task::spawn(async move {
        descartes_tokio::time::sleep(Duration::from_millis(10)).await;
        let _ = tx.send(42).await;
    });

    Executor::timed(SimTime::from_duration(Duration::from_millis(50))).execute(&mut sim);

    assert_eq!(*out.lock().unwrap(), 0);
}
