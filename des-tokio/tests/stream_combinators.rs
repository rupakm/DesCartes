use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::{Execute, Executor, SimTime, Simulation};
use des_tokio::stream::StreamExt;

#[test]
fn stream_combinators_map_filter_take_collect() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let (tx, rx) = des_tokio::sync::mpsc::channel::<u64>(16);

    let out: Arc<Mutex<Option<Vec<u64>>>> = Arc::new(Mutex::new(None));
    let out2 = out.clone();

    let _producer = des_tokio::task::spawn(async move {
        for i in 0..10u64 {
            tx.send(i).await.unwrap();
        }
        drop(tx);
    });

    let mut rx = rx;
    let _consumer = des_tokio::task::spawn(async move {
        let v = rx
            .map(|x| x * 2)
            .filter(|x| *x % 4 == 0)
            .take(3)
            .collect::<Vec<_>>()
            .await;

        *out2.lock().unwrap() = Some(v);
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), Some(vec![0, 4, 8]));
}

#[test]
fn stream_combinators_fold_sum() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let (tx, rx) = des_tokio::sync::mpsc::channel::<u64>(16);

    let out: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
    let out2 = out.clone();

    let _producer = des_tokio::task::spawn(async move {
        for i in 1..=10u64 {
            tx.send(i).await.unwrap();
        }
        drop(tx);
    });

    let mut rx = rx;
    let _consumer = des_tokio::task::spawn(async move {
        let sum = rx.take(10).fold(0u64, |acc, x| acc + x).await;
        *out2.lock().unwrap() = Some(sum);
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), Some(55));
}
