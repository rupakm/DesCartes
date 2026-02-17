use rand::SeedableRng;
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll, Waker};

use descartes_core::async_runtime;
use descartes_core::SimTime;

use crate::concurrency::ConcurrencyEvent;

#[derive(Debug, Clone, Copy)]
pub struct WaiterInfo {
    pub waiter_id: u64,
    pub task_id: u64,
}

pub trait MutexWaiterPolicy: Send {
    fn choose(&mut self, time: SimTime, waiters: &[WaiterInfo]) -> usize;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FifoMutexWaiterPolicy;

impl MutexWaiterPolicy for FifoMutexWaiterPolicy {
    fn choose(&mut self, _time: SimTime, _waiters: &[WaiterInfo]) -> usize {
        0
    }
}

#[derive(Debug, Clone)]
pub struct UniformRandomMutexWaiterPolicy {
    rng: rand::rngs::StdRng,
}

impl UniformRandomMutexWaiterPolicy {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: rand::rngs::StdRng::seed_from_u64(seed),
        }
    }
}

impl MutexWaiterPolicy for UniformRandomMutexWaiterPolicy {
    fn choose(&mut self, _time: SimTime, waiters: &[WaiterInfo]) -> usize {
        if waiters.is_empty() {
            return 0;
        }
        use rand::Rng;
        self.rng.gen_range(0..waiters.len())
    }
}

#[derive(Debug)]
struct Waiter {
    waiter_id: u64,
    task_id: u64,
    waker: Waker,
}

#[derive(Debug)]
struct State {
    locked: bool,
    owner_task_id: Option<u64>,
    next_waiter_id: u64,
    waiters: VecDeque<Waiter>,
}

impl State {
    fn new() -> Self {
        Self {
            locked: false,
            owner_task_id: None,
            next_waiter_id: 0,
            waiters: VecDeque::new(),
        }
    }
}

static NEXT_MUTEX_ID: AtomicU64 = AtomicU64::new(1);

pub struct MutexAsync<T> {
    mutex_id: u64,
    state: StdMutex<State>,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for MutexAsync<T> {}
unsafe impl<T: Send> Sync for MutexAsync<T> {}

impl<T> MutexAsync<T> {
    pub fn new(value: T) -> Self {
        Self::new_with_id(
            NEXT_MUTEX_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            value,
        )
    }

    pub fn new_with_id(mutex_id: u64, value: T) -> Self {
        Self {
            mutex_id,
            state: StdMutex::new(State::new()),
            value: UnsafeCell::new(value),
        }
    }

    pub fn mutex_id(&self) -> u64 {
        self.mutex_id
    }

    pub async fn lock(&self) -> MutexGuard<'_, T> {
        LockFuture { mutex: self }.await
    }

    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        let task_id = current_task_id_u64();
        let time_nanos = crate::runtime::current_time_nanos();

        let mut state = self.state.lock().unwrap();
        if state.locked {
            crate::runtime::record_concurrency_event(ConcurrencyEvent::MutexContended {
                mutex_id: self.mutex_id,
                task_id,
                time_nanos,
                waiter_count: state.waiters.len(),
            });
            return None;
        }

        state.locked = true;
        state.owner_task_id = Some(task_id);
        drop(state);

        crate::runtime::record_concurrency_event(ConcurrencyEvent::MutexAcquire {
            mutex_id: self.mutex_id,
            task_id,
            time_nanos,
        });

        Some(MutexGuard { mutex: self })
    }
}

pub struct MutexGuard<'a, T> {
    mutex: &'a MutexAsync<T>,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // Safety: guard represents exclusive access.
        unsafe { &*self.mutex.value.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: guard represents exclusive access.
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        let task_id = current_task_id_u64();
        let time_nanos = crate::runtime::current_time_nanos();

        crate::runtime::record_concurrency_event(ConcurrencyEvent::MutexRelease {
            mutex_id: self.mutex.mutex_id,
            task_id,
            time_nanos,
        });

        let mut state = self.mutex.state.lock().unwrap();
        state.locked = false;
        state.owner_task_id = None;

        if state.waiters.is_empty() {
            return;
        }

        let now = async_runtime::current_sim_time().unwrap_or_else(SimTime::zero);
        let waiter_infos: Vec<WaiterInfo> = state
            .waiters
            .iter()
            .map(|w| WaiterInfo {
                waiter_id: w.waiter_id,
                task_id: w.task_id,
            })
            .collect();

        let chosen = crate::runtime::choose_mutex_waiter(now, &waiter_infos);
        let chosen = chosen.min(state.waiters.len().saturating_sub(1));

        let waiter = state
            .waiters
            .remove(chosen)
            .expect("waiter index validated");

        waiter.waker.wake();
    }
}

struct LockFuture<'a, T> {
    mutex: &'a MutexAsync<T>,
}

impl<'a, T> std::future::Future for LockFuture<'a, T> {
    type Output = MutexGuard<'a, T>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let task_id = current_task_id_u64();
        let time_nanos = crate::runtime::current_time_nanos();

        let mut state = self.mutex.state.lock().unwrap();
        if !state.locked {
            state.locked = true;
            state.owner_task_id = Some(task_id);
            drop(state);

            crate::runtime::record_concurrency_event(ConcurrencyEvent::MutexAcquire {
                mutex_id: self.mutex.mutex_id,
                task_id,
                time_nanos,
            });

            return Poll::Ready(MutexGuard { mutex: self.mutex });
        }

        // Already locked; enqueue waiter.
        crate::runtime::record_concurrency_event(ConcurrencyEvent::MutexContended {
            mutex_id: self.mutex.mutex_id,
            task_id,
            time_nanos,
            waiter_count: state.waiters.len(),
        });

        let waiter_id = state.next_waiter_id;
        state.next_waiter_id += 1;

        // Replace an existing waiter for the same task (multiple polls).
        if let Some(existing) = state.waiters.iter_mut().find(|w| w.task_id == task_id) {
            existing.waker = cx.waker().clone();
            return Poll::Pending;
        }

        state.waiters.push_back(Waiter {
            waiter_id,
            task_id,
            waker: cx.waker().clone(),
        });

        Poll::Pending
    }
}

fn current_task_id_u64() -> u64 {
    async_runtime::current_task_id()
        .map(|t| t.0)
        .unwrap_or_else(|| panic!("des_tokio::sync::Mutex used outside async runtime polling"))
}

/// Tokio-like mutex handle.
#[derive(Clone)]
pub struct Mutex<T>(Arc<MutexAsync<T>>);

impl<T> std::fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mutex")
            .field("mutex_id", &self.0.mutex_id())
            .finish()
    }
}

impl<T> Mutex<T> {
    pub fn new(value: T) -> Self {
        Self(Arc::new(MutexAsync::new(value)))
    }

    /// Construct a mutex with a stable name-based id.
    ///
    /// This is useful when the same logical mutex is created in multiple places
    /// (e.g. record vs replay closures) and you want event IDs to match.
    pub fn new_named(name: &'static str, value: T) -> Self {
        let mutex_id = descartes_core::randomness::runtime_site_id("des_tokio::sync::Mutex", name);
        Self::new_with_id(mutex_id, value)
    }

    /// Construct a mutex with an explicit stable identifier.
    ///
    /// This is recommended for record/replay workflows that validate concurrency
    /// event streams.
    pub fn new_with_id(mutex_id: u64, value: T) -> Self {
        Self(Arc::new(MutexAsync::new_with_id(mutex_id, value)))
    }

    pub fn mutex_id(&self) -> u64 {
        self.0.mutex_id()
    }

    pub async fn lock(&self) -> MutexGuard<'_, T> {
        self.0.lock().await
    }

    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        self.0.try_lock()
    }
}

/// Convenience constructor.
pub fn mutex<T>(value: T) -> Mutex<T> {
    Mutex::new(value)
}
