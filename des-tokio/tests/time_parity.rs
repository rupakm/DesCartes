use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::{Execute, Executor, SimTime, Simulation};

#[test]
fn instant_duration_since_panics_when_earlier_is_later() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let did_panic = Arc::new(Mutex::new(false));
    let did_panic2 = did_panic.clone();

    let _h = des_tokio::task::spawn(async move {
        let now = des_tokio::time::Instant::now();
        let later = now + Duration::from_millis(1);

        let result = std::panic::catch_unwind(|| {
            let _ = now.duration_since(later);
        });

        *did_panic2.lock().unwrap() = result.is_err();
    });

    Executor::timed(SimTime::from_duration(Duration::from_millis(10))).execute(&mut sim);
    assert!(*did_panic.lock().unwrap());
}

#[test]
fn instant_checked_add_returns_none_on_overflow() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let out = Arc::new(Mutex::new(None));
    let out2 = out.clone();

    let _h = des_tokio::task::spawn(async move {
        let now = des_tokio::time::Instant::now();
        let huge = Duration::from_secs(u64::MAX);
        *out2.lock().unwrap() = Some(now.checked_add(huge).is_none());
    });

    Executor::timed(SimTime::from_duration(Duration::from_millis(10))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), Some(true));
}

#[test]
fn instant_saturating_duration_since_never_panics() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let out = Arc::new(Mutex::new(None));
    let out2 = out.clone();

    let _h = des_tokio::task::spawn(async move {
        let now = des_tokio::time::Instant::now();
        let later = now + Duration::from_millis(5);
        let d = now.saturating_duration_since(later);
        *out2.lock().unwrap() = Some(d == Duration::ZERO);
    });

    Executor::timed(SimTime::from_duration(Duration::from_millis(10))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), Some(true));
}
