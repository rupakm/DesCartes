use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::{Execute, Executor, SimTime, Simulation};

#[test]
fn oneshot_send_and_receive() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let (tx, rx) = des_tokio::sync::oneshot::channel::<usize>();

    let out = Arc::new(Mutex::new(None));
    let out2 = out.clone();

    let _h = des_tokio::task::spawn(async move {
        let v = rx.await.expect("recv");
        *out2.lock().unwrap() = Some(v);
    });

    let _h2 = des_tokio::task::spawn(async move {
        des_tokio::time::sleep(Duration::from_millis(10)).await;
        tx.send(7).expect("send");
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), Some(7));
}

#[test]
fn oneshot_sender_drop_wakes_receiver_with_error() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let (tx, rx) = des_tokio::sync::oneshot::channel::<usize>();
    drop(tx);

    let out = Arc::new(Mutex::new(None));
    let out2 = out.clone();

    let _h = des_tokio::task::spawn(async move {
        *out2.lock().unwrap() = Some(rx.await.is_err());
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), Some(true));
}

#[test]
fn oneshot_receiver_drop_causes_send_to_fail() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let (tx, rx) = des_tokio::sync::oneshot::channel::<usize>();

    // Drop receiver in a task after a short delay.
    let _h = des_tokio::task::spawn(async move {
        des_tokio::time::sleep(Duration::from_millis(10)).await;
        drop(rx);
    });

    let out = Arc::new(Mutex::new(None));
    let out2 = out.clone();

    let _h2 = des_tokio::task::spawn(async move {
        des_tokio::time::sleep(Duration::from_millis(20)).await;
        *out2.lock().unwrap() = Some(tx.send(7).is_err());
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), Some(true));
}
