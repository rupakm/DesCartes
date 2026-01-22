use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::{Execute, Executor, SimTime, Simulation};

#[test]
fn watch_receiver_observes_updates() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let (tx, mut rx) = des_tokio::sync::watch::channel(0u64);

    let out: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let out2 = out.clone();

    des_tokio::task::spawn(async move {
        out2.lock().unwrap().push(*rx.borrow());

        rx.changed().await.unwrap();
        out2.lock().unwrap().push(*rx.borrow());

        rx.changed().await.unwrap();
        out2.lock().unwrap().push(*rx.borrow());
    });

    des_tokio::task::spawn(async move {
        des_tokio::time::sleep(Duration::from_millis(5)).await;
        tx.send(10).unwrap();
        des_tokio::time::sleep(Duration::from_millis(5)).await;
        tx.send(20).unwrap();
    });

    Executor::timed(SimTime::from_millis(50)).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), vec![0, 10, 20]);
}

#[test]
fn watch_changed_returns_err_when_sender_dropped() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let (tx, mut rx) = des_tokio::sync::watch::channel(1u64);

    let out: Arc<Mutex<Vec<Result<(), des_tokio::sync::watch::RecvError>>>> =
        Arc::new(Mutex::new(Vec::new()));
    let out2 = out.clone();

    des_tokio::task::spawn_local(async move {
        // Sender drops without any changes.
        drop(tx);
        let res = rx.changed().await;
        out2.lock().unwrap().push(res);
    });

    Executor::timed(SimTime::from_millis(10)).execute(&mut sim);

    let v = out.lock().unwrap();
    assert_eq!(v.len(), 1);
    assert!(v[0].is_err());
}

#[test]
fn watch_clone_receiver_sees_updates() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let (tx, mut rx1) = des_tokio::sync::watch::channel(0u64);
    let mut rx2 = rx1.clone();

    let out: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

    let out1 = out.clone();
    des_tokio::task::spawn(async move {
        rx1.changed().await.unwrap();
        if *rx1.borrow() == 7 {
            out1.lock().unwrap().push("rx1");
        }
    });

    let out2 = out.clone();
    des_tokio::task::spawn(async move {
        rx2.changed().await.unwrap();
        if *rx2.borrow() == 7 {
            out2.lock().unwrap().push("rx2");
        }
    });

    des_tokio::task::spawn(async move {
        des_tokio::time::sleep(Duration::from_millis(5)).await;
        tx.send(7).unwrap();
    });

    Executor::timed(SimTime::from_millis(50)).execute(&mut sim);

    let v = out.lock().unwrap().clone();
    assert_eq!(v.len(), 2);
    assert!(v.contains(&"rx1"));
    assert!(v.contains(&"rx2"));
}
