use std::sync::{Arc, Mutex};
use std::time::Duration;

use descartes_core::{Execute, Executor, SimTime, Simulation};

use descartes_tokio::stream::StreamExt;

#[test]
fn mpsc_receiver_is_a_stream() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let (tx, mut rx) = descartes_tokio::sync::mpsc::channel::<usize>(4);

    let out = Arc::new(Mutex::new(Vec::new()));
    let out2 = out.clone();

    let _consumer = descartes_tokio::task::spawn(async move {
        while let Some(v) = rx.next().await {
            out2.lock().unwrap().push(v);
            if v == 3 {
                break;
            }
        }
    });

    let _producer = descartes_tokio::task::spawn(async move {
        tx.send(1).await.unwrap();
        tx.send(2).await.unwrap();
        tx.send(3).await.unwrap();
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), vec![1, 2, 3]);
}

#[test]
fn mpsc_stream_ends_when_all_senders_dropped() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let (tx, mut rx) = descartes_tokio::sync::mpsc::channel::<usize>(2);

    let out = Arc::new(Mutex::new(None));
    let out2 = out.clone();

    let _consumer = descartes_tokio::task::spawn(async move {
        drop(tx);
        *out2.lock().unwrap() = Some(rx.next().await.is_none());
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), Some(true));
}
