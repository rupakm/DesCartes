use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::{Execute, Executor, SimTime, Simulation};

#[test]
fn rwlock_allows_multiple_readers() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let lock = Arc::new(des_tokio::sync::RwLock::new(0u64));

    let out: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

    for tag in ["r1", "r2"] {
        let l2 = lock.clone();
        let out2 = out.clone();
        des_tokio::task::spawn(async move {
            let _g = l2.read().await;
            out2.lock().unwrap().push(tag);
            des_tokio::thread::yield_now().await;
        });
    }

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    let v = out.lock().unwrap().clone();
    assert_eq!(v.len(), 2);
    assert!(v.contains(&"r1"));
    assert!(v.contains(&"r2"));
}

#[test]
fn rwlock_is_write_preferring() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let lock = Arc::new(des_tokio::sync::RwLock::new(0u64));

    let out: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

    // Hold an initial reader.
    let l1 = lock.clone();
    let out1 = out.clone();
    des_tokio::task::spawn(async move {
        let _g = l1.read().await;
        out1.lock().unwrap().push("r1");
        des_tokio::thread::yield_now().await;
    });

    // Queue a writer.
    let lw = lock.clone();
    let outw = out.clone();
    des_tokio::task::spawn(async move {
        let mut g = lw.write().await;
        *g += 1;
        outw.lock().unwrap().push("w");
    });

    // Queue another reader after the writer.
    let l2 = lock.clone();
    let out2 = out.clone();
    des_tokio::task::spawn(async move {
        // Ensure the queued writer is observed before attempting the read.
        des_tokio::thread::yield_now().await;
        let _g = l2.read().await;
        out2.lock().unwrap().push("r2");
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);

    // Writer should run before r2.
    assert_eq!(*out.lock().unwrap(), vec!["r1", "w", "r2"]);
}
