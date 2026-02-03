use std::sync::{Arc, Mutex};
use std::time::Duration;

use descartes_core::{Execute, Executor, SimTime, Simulation};

#[test]
fn spawn_panics_without_install() {
    let result = std::panic::catch_unwind(|| {
        let _ = descartes_tokio::task::spawn(async { 1usize });
    });

    assert!(result.is_err());
}

#[test]
fn spawn_and_join() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let out: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
    let out_clone = out.clone();

    let h = descartes_tokio::task::spawn(async {
        descartes_tokio::time::sleep(Duration::from_millis(50)).await;
        7usize
    });

    // JoinHandle drop is detach (Tokio-like). The joiner task will still run
    // even if we don't keep its JoinHandle.
    let _joiner = descartes_tokio::task::spawn(async move {
        let v = h.await.expect("join");
        *out_clone.lock().unwrap() = Some(v);
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), Some(7));
}

#[test]
fn drop_detaches_task() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let flag = Arc::new(Mutex::new(false));
    let flag2 = flag.clone();

    let h = descartes_tokio::task::spawn(async move {
        descartes_tokio::time::sleep(Duration::from_secs(1)).await;
        *flag2.lock().unwrap() = true;
        0usize
    });

    // Dropping the handle detaches the task (Tokio-like).
    drop(h);

    Executor::timed(SimTime::from_duration(Duration::from_secs(2))).execute(&mut sim);
    assert!(*flag.lock().unwrap());
}

#[test]
fn abort_cancels_task() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let flag = Arc::new(Mutex::new(false));
    let flag2 = flag.clone();

    let h = descartes_tokio::task::spawn(async move {
        descartes_tokio::time::sleep(Duration::from_secs(1)).await;
        *flag2.lock().unwrap() = true;
        0usize
    });

    h.abort();

    Executor::timed(SimTime::from_duration(Duration::from_secs(2))).execute(&mut sim);
    assert!(!*flag.lock().unwrap());
}
