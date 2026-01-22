use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::{Execute, Executor, SimTime, Simulation};

#[test]
fn interval_first_tick_is_immediate() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let out: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let out2 = out.clone();

    des_tokio::task::spawn(async move {
        let start = des_tokio::time::Instant::now();
        let mut intv = des_tokio::time::interval(Duration::from_millis(10));

        let t0 = intv.tick().await;
        let t1 = intv.tick().await;
        let t2 = intv.tick().await;

        *out2.lock().unwrap() = vec![t0 - start, t1 - start, t2 - start];
    });

    Executor::timed(SimTime::from_millis(30)).execute(&mut sim);

    assert_eq!(
        *out.lock().unwrap(),
        vec![
            Duration::from_millis(0),
            Duration::from_millis(10),
            Duration::from_millis(20)
        ]
    );
}

#[test]
fn interval_missed_tick_behavior_burst_catches_up() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let out: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let out2 = out.clone();

    des_tokio::task::spawn(async move {
        let start = des_tokio::time::Instant::now();
        let mut intv = des_tokio::time::interval(Duration::from_millis(10));
        intv.set_missed_tick_behavior(des_tokio::time::MissedTickBehavior::Burst);

        let t0 = intv.tick().await;
        des_tokio::time::sleep(Duration::from_millis(35)).await;
        let t1 = intv.tick().await;
        let t2 = intv.tick().await;
        let t3 = intv.tick().await;
        let t4 = intv.tick().await;

        *out2.lock().unwrap() = vec![t0 - start, t1 - start, t2 - start, t3 - start, t4 - start];
    });

    Executor::timed(SimTime::from_millis(60)).execute(&mut sim);

    assert_eq!(
        *out.lock().unwrap(),
        vec![
            Duration::from_millis(0),
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(30),
            Duration::from_millis(40)
        ]
    );
}

#[test]
fn interval_missed_tick_behavior_delay_resets_schedule() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let out: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let out2 = out.clone();

    des_tokio::task::spawn(async move {
        let start = des_tokio::time::Instant::now();
        let mut intv = des_tokio::time::interval(Duration::from_millis(10));
        intv.set_missed_tick_behavior(des_tokio::time::MissedTickBehavior::Delay);

        let t0 = intv.tick().await;
        des_tokio::time::sleep(Duration::from_millis(35)).await;
        let t1 = intv.tick().await;
        let t2 = intv.tick().await;
        let t3 = intv.tick().await;

        *out2.lock().unwrap() = vec![t0 - start, t1 - start, t2 - start, t3 - start];
    });

    Executor::timed(SimTime::from_millis(80)).execute(&mut sim);

    // After the long sleep, the next tick is immediate (10ms), but the schedule
    // resets from "now" (35ms), so subsequent ticks happen at 45ms, 55ms, ...
    assert_eq!(
        *out.lock().unwrap(),
        vec![
            Duration::from_millis(0),
            Duration::from_millis(10),
            Duration::from_millis(45),
            Duration::from_millis(55)
        ]
    );
}

#[test]
fn interval_missed_tick_behavior_skip_skips_to_next_multiple() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let out: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let out2 = out.clone();

    des_tokio::task::spawn(async move {
        let start = des_tokio::time::Instant::now();
        let mut intv = des_tokio::time::interval(Duration::from_millis(10));
        intv.set_missed_tick_behavior(des_tokio::time::MissedTickBehavior::Skip);

        let t0 = intv.tick().await;
        des_tokio::time::sleep(Duration::from_millis(35)).await;
        let t1 = intv.tick().await;
        let t2 = intv.tick().await;
        let t3 = intv.tick().await;

        *out2.lock().unwrap() = vec![t0 - start, t1 - start, t2 - start, t3 - start];
    });

    Executor::timed(SimTime::from_millis(80)).execute(&mut sim);

    // The tick at 10ms is missed, so we jump to the next multiple after 35ms: 40ms.
    assert_eq!(
        *out.lock().unwrap(),
        vec![
            Duration::from_millis(0),
            Duration::from_millis(10),
            Duration::from_millis(40),
            Duration::from_millis(50)
        ]
    );
}
