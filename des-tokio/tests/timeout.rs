use std::sync::{Arc, Mutex};
use std::time::Duration;

use descartes_core::{Execute, Executor, SimTime, Simulation};

#[test]
fn timeout_returns_err_when_deadline_hits_first() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let out = Arc::new(Mutex::new(None));
    let out2 = out.clone();

    let _h = descartes_tokio::task::spawn(async move {
        let res = descartes_tokio::time::timeout(Duration::from_millis(10), async {
            descartes_tokio::time::sleep(Duration::from_millis(50)).await;
            1usize
        })
        .await;

        *out2.lock().unwrap() = Some(res.is_err());
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), Some(true));
}

#[test]
fn timeout_returns_ok_when_future_completes_first() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    let out = Arc::new(Mutex::new(None));
    let out2 = out.clone();

    let _h = descartes_tokio::task::spawn(async move {
        let res = descartes_tokio::time::timeout(Duration::from_millis(50), async {
            descartes_tokio::time::sleep(Duration::from_millis(10)).await;
            7usize
        })
        .await;

        *out2.lock().unwrap() = Some(res.ok());
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), Some(Some(7)));
}
