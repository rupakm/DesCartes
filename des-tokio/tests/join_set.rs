use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_core::{Execute, Executor, SimTime, Simulation};

#[test]
fn join_set_returns_results_in_completion_order() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let out: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let out2 = out.clone();

    des_tokio::task::spawn_local(async move {
        let mut set = des_tokio::task::JoinSet::new();

        set.spawn(async {
            des_tokio::time::sleep(Duration::from_millis(30)).await;
            1u64
        });
        set.spawn(async {
            des_tokio::time::sleep(Duration::from_millis(10)).await;
            2u64
        });
        set.spawn(async {
            des_tokio::time::sleep(Duration::from_millis(20)).await;
            3u64
        });

        while let Some(res) = set.join_next().await {
            out2.lock().unwrap().push(res.expect("task ok"));
        }
    });

    Executor::timed(SimTime::from_millis(100)).execute(&mut sim);

    assert_eq!(*out.lock().unwrap(), vec![2, 3, 1]);
}

#[test]
fn join_set_drop_aborts_tasks() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let hit: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let hit2 = hit.clone();

    des_tokio::task::spawn_local(async move {
        let mut set = des_tokio::task::JoinSet::new();

        set.spawn(async move {
            des_tokio::time::sleep(Duration::from_millis(100)).await;
            *hit2.lock().unwrap() = true;
        });

        // Drop the set immediately. Tokio semantics: dropping a JoinSet aborts tasks.
        drop(set);

        // Give plenty of simulated time for the task to run if it was not aborted.
        des_tokio::time::sleep(Duration::from_millis(200)).await;
    });

    Executor::timed(SimTime::from_millis(400)).execute(&mut sim);

    assert_eq!(*hit.lock().unwrap(), false);
}

#[test]
fn join_set_abort_handle_cancels_task() {
    let mut sim = Simulation::default();
    des_tokio::runtime::install(&mut sim);

    let out: Arc<Mutex<Vec<Result<u64, des_tokio::task::JoinError>>>> =
        Arc::new(Mutex::new(Vec::new()));
    let out2 = out.clone();

    des_tokio::task::spawn_local(async move {
        let mut set = des_tokio::task::JoinSet::new();

        let abort = set.spawn(async {
            des_tokio::time::sleep(Duration::from_millis(100)).await;
            9u64
        });

        abort.abort();

        if let Some(res) = set.join_next().await {
            out2.lock().unwrap().push(res);
        }
    });

    Executor::timed(SimTime::from_millis(200)).execute(&mut sim);

    let v = out.lock().unwrap();
    assert_eq!(v.len(), 1);
    assert!(matches!(v[0], Err(des_tokio::task::JoinError::Cancelled)));
}
