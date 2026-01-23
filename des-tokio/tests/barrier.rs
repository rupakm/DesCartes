use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::{Execute, Executor, SimTime, Simulation};

#[test]
fn barrier_releases_and_selects_last_arrival_as_leader() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let barrier = Arc::new(des_tokio::sync::Barrier::new(3));
    let turn = Arc::new(AtomicUsize::new(0));
    let out: Arc<Mutex<Vec<(u64, bool)>>> = Arc::new(Mutex::new(Vec::new()));

    for id in 0..3u64 {
        let b = barrier.clone();
        let t = turn.clone();
        let o = out.clone();
        des_tokio::task::spawn(async move {
            while t.load(Ordering::SeqCst) != id as usize {
                des_tokio::thread::yield_now().await;
            }

            let fut = b.wait();
            t.store(id as usize + 1, Ordering::SeqCst);
            let res = fut.await;
            o.lock().unwrap().push((id, res.is_leader()));
        });
    }

    Executor::timed(SimTime::from_duration(Duration::from_millis(100))).execute(&mut sim);

    let out = out.lock().unwrap();
    assert_eq!(out.len(), 3);
    let leaders: Vec<u64> = out.iter().filter(|(_, l)| *l).map(|(id, _)| *id).collect();
    assert_eq!(leaders, vec![2]);
}

#[test]
fn barrier_wait_is_cancel_safe() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let barrier = Arc::new(des_tokio::sync::Barrier::new(2));

    let b_a = barrier.clone();
    let h_a = des_tokio::task::spawn(async move {
        let _ = b_a.wait().await;
    });

    let abort_a = des_tokio::task::spawn(async move {
        des_tokio::time::sleep(Duration::from_millis(1)).await;
        h_a.abort();
    });
    drop(abort_a);

    let b = barrier.clone();
    let b_done = Arc::new(Mutex::new(false));
    let b_done2 = b_done.clone();
    des_tokio::task::spawn(async move {
        des_tokio::time::sleep(Duration::from_millis(20)).await;
        let _ = b.wait().await;
        *b_done2.lock().unwrap() = true;
    });

    let snapshot = Arc::new(Mutex::new(None::<bool>));
    let snapshot2 = snapshot.clone();
    let b_done3 = b_done.clone();
    des_tokio::task::spawn(async move {
        des_tokio::time::sleep(Duration::from_millis(30)).await;
        *snapshot2.lock().unwrap() = Some(*b_done3.lock().unwrap());
    });

    let c = barrier.clone();
    let c_done = Arc::new(Mutex::new(false));
    let c_done2 = c_done.clone();
    des_tokio::task::spawn(async move {
        des_tokio::time::sleep(Duration::from_millis(40)).await;
        let _ = c.wait().await;
        *c_done2.lock().unwrap() = true;
    });

    Executor::timed(SimTime::from_duration(Duration::from_millis(100))).execute(&mut sim);

    assert_eq!(*snapshot.lock().unwrap(), Some(false));
    assert!(*b_done.lock().unwrap());
    assert!(*c_done.lock().unwrap());
}
