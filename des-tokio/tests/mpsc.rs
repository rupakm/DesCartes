use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::{Execute, Executor, SimTime, Simulation};

#[test]
fn mpsc_send_recv_fifo() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let (tx, mut rx) = des_tokio::sync::mpsc::channel::<usize>(4);

    let out = Arc::new(Mutex::new(Vec::new()));
    let out2 = out.clone();

    let _consumer = des_tokio::task::spawn(async move {
        while let Some(v) = rx.recv().await {
            out2.lock().unwrap().push(v);
            if v == 3 {
                break;
            }
        }
    });

    let _producer = des_tokio::task::spawn(async move {
        tx.send(1).await.unwrap();
        tx.send(2).await.unwrap();
        tx.send(3).await.unwrap();
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), vec![1, 2, 3]);
}

#[test]
fn mpsc_backpressure_blocks_sender_until_recv() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let (tx, mut rx) = des_tokio::sync::mpsc::channel::<usize>(1);

    let sent = Arc::new(Mutex::new(Vec::new()));
    let sent2 = sent.clone();

    let _producer = des_tokio::task::spawn(async move {
        tx.send(1).await.unwrap();
        sent2.lock().unwrap().push(1);

        // This send must block until receiver consumes the first item.
        tx.send(2).await.unwrap();
        sent2.lock().unwrap().push(2);
    });

    let received = Arc::new(Mutex::new(Vec::new()));
    let received2 = received.clone();

    let _consumer = des_tokio::task::spawn(async move {
        // Wait a bit before reading, to ensure second send would block.
        des_tokio::time::sleep(Duration::from_millis(50)).await;

        if let Some(v) = rx.recv().await {
            received2.lock().unwrap().push(v);
        }

        if let Some(v) = rx.recv().await {
            received2.lock().unwrap().push(v);
        }
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);

    assert_eq!(*sent.lock().unwrap(), vec![1, 2]);
    assert_eq!(*received.lock().unwrap(), vec![1, 2]);
}

#[test]
fn mpsc_recv_returns_none_when_all_senders_dropped() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let (tx, mut rx) = des_tokio::sync::mpsc::channel::<usize>(2);

    let out = Arc::new(Mutex::new(None));
    let out2 = out.clone();

    let _consumer = des_tokio::task::spawn(async move {
        // Drop sender immediately; recv should return None.
        drop(tx);
        *out2.lock().unwrap() = Some(rx.recv().await.is_none());
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), Some(true));
}

#[test]
fn mpsc_send_errors_when_receiver_dropped() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let (tx, rx) = des_tokio::sync::mpsc::channel::<usize>(2);
    drop(rx);

    let out = Arc::new(Mutex::new(None));
    let out2 = out.clone();

    let _producer = des_tokio::task::spawn(async move {
        *out2.lock().unwrap() = Some(tx.send(1).await.is_err());
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), Some(true));
}
