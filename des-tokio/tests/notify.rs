use std::sync::{Arc, Mutex};
use std::time::Duration;

use descartes_core::{Execute, Executor, SimTime, Simulation};

#[test]
fn notify_one_wakes_one_waiter() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let notify = Arc::new(descartes_tokio::sync::notify::Notify::new());

    let out = Arc::new(Mutex::new(0usize));

    let n1 = notify.clone();
    let out1 = out.clone();
    let _w1 = descartes_tokio::task::spawn(async move {
        n1.notified().await;
        *out1.lock().unwrap() += 1;
    });

    let n2 = notify.clone();
    let out2 = out.clone();
    let _w2 = descartes_tokio::task::spawn(async move {
        n2.notified().await;
        *out2.lock().unwrap() += 1;
    });

    let n3 = notify.clone();
    let _notifier = descartes_tokio::task::spawn(async move {
        descartes_tokio::time::sleep(Duration::from_millis(10)).await;
        n3.notify_one();
    });

    Executor::timed(SimTime::from_duration(Duration::from_millis(100))).execute(&mut sim);

    assert_eq!(*out.lock().unwrap(), 1);
}

#[test]
fn notify_waiters_wakes_all() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let notify = Arc::new(descartes_tokio::sync::notify::Notify::new());

    let out = Arc::new(Mutex::new(0usize));

    for _ in 0..3 {
        let n = notify.clone();
        let outn = out.clone();
        descartes_tokio::task::spawn(async move {
            n.notified().await;
            *outn.lock().unwrap() += 1;
        });
    }

    let n = notify.clone();
    descartes_tokio::task::spawn(async move {
        descartes_tokio::time::sleep(Duration::from_millis(10)).await;
        n.notify_waiters();
    });

    Executor::timed(SimTime::from_duration(Duration::from_millis(100))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), 3);
}

#[test]
fn notify_one_stores_permit_when_no_waiter() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let notify = Arc::new(descartes_tokio::sync::notify::Notify::new());
    notify.notify_one();

    let out = Arc::new(Mutex::new(false));
    let out2 = out.clone();

    let n = notify.clone();
    descartes_tokio::task::spawn(async move {
        n.notified().await;
        *out2.lock().unwrap() = true;
    });

    Executor::timed(SimTime::from_duration(Duration::from_millis(50))).execute(&mut sim);
    assert!(*out.lock().unwrap());
}
