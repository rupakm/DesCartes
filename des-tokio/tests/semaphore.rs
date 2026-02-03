use std::sync::{Arc, Mutex};
use std::time::Duration;

use descartes_core::{Execute, Executor, SimTime, Simulation};

#[test]
fn semaphore_is_fifo_fair() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    use std::sync::atomic::{AtomicUsize, Ordering};

    let sem = Arc::new(descartes_tokio::sync::Semaphore::new(1));

    let out: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let turn: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));

    for id in 0..5u64 {
        let sem2 = sem.clone();
        let out2 = out.clone();
        let turn2 = turn.clone();
        descartes_tokio::task::spawn(async move {
            // Ensure tasks attempt acquisition in a deterministic order.
            while turn2.load(Ordering::SeqCst) != id as usize {
                descartes_tokio::thread::yield_now().await;
            }

            let fut = sem2.acquire();
            turn2.store(id as usize + 1, Ordering::SeqCst);

            let _permit = fut.await.expect("acquire");
            out2.lock().unwrap().push(id);

            // Force another poll cycle while holding the permit so others enqueue.
            descartes_tokio::thread::yield_now().await;
        });
    }

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), vec![0, 1, 2, 3, 4]);
}

#[test]
fn semaphore_respects_head_of_line_blocking_for_acquire_many() {
    let mut sim = Simulation::default();
    descartes_tokio::runtime::install(&mut sim);

    // Start with 1 permit.
    let sem = Arc::new(descartes_tokio::sync::Semaphore::new(1));

    let out: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let out_a = out.clone();
    let out_b = out.clone();

    let sem_a = sem.clone();
    descartes_tokio::task::spawn(async move {
        // Request 2 permits; should complete before B even though B only needs 1.
        let _p = sem_a.acquire_many(2).await.expect("acquire_many");
        out_a.lock().unwrap().push("A");
        descartes_tokio::thread::yield_now().await;
    });

    let sem_b = sem.clone();
    descartes_tokio::task::spawn(async move {
        let _p = sem_b.acquire().await.expect("acquire");
        out_b.lock().unwrap().push("B");
    });

    // Add one more permit so A can be satisfied.
    let sem_add = sem.clone();
    descartes_tokio::task::spawn(async move {
        descartes_tokio::thread::yield_now().await;
        sem_add.add_permits(1);
    });

    Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
    assert_eq!(*out.lock().unwrap(), vec!["A", "B"]);
}
