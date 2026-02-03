use descartes_core::async_runtime;
use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;
use std::task::Waker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AcquireError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TryAcquireError {
    Closed,
    NoPermits,
}

#[derive(Debug)]
struct Waiter {
    waiter_id: u64,
    //task_id: u64,
    required: usize,
    remaining: usize,
    waker: Waker,
}

#[derive(Debug)]
struct State {
    permits: usize,
    closed: bool,
    next_waiter_id: u64,

    // FIFO queue of outstanding acquisitions.
    queue: VecDeque<Waiter>,

    // Waiters that have been satisfied and removed from the queue, but whose
    // futures have not yet observed completion.
    ready: HashSet<u64>,
}

impl State {
    fn new(permits: usize) -> Self {
        Self {
            permits,
            closed: false,
            next_waiter_id: 0,
            queue: VecDeque::new(),
            ready: HashSet::new(),
        }
    }

    fn add_permits_locked(&mut self, added: usize) -> Vec<Waker> {
        let mut to_wake = Vec::new();

        // Any available permits must be assigned to the head of the queue first.
        // This implements head-of-line blocking and prevents later acquires from
        // bypassing an earlier `acquire_many`.
        let mut available = self
            .permits
            .checked_add(added)
            .expect("permit count overflow");
        self.permits = 0;

        while available > 0 {
            let Some(front) = self.queue.front_mut() else {
                self.permits = available;
                break;
            };

            let give = available.min(front.remaining);
            front.remaining -= give;
            available -= give;

            if front.remaining == 0 {
                let waiter = self.queue.pop_front().expect("front exists");
                self.ready.insert(waiter.waiter_id);
                to_wake.push(waiter.waker);
            } else {
                // Head-of-line blocking: cannot satisfy later waiters until the
                // front waiter is fully satisfied.
                break;
            }
        }

        to_wake
    }

    fn remove_waiter_locked(&mut self, waiter_id: u64) -> Option<Waiter> {
        if let Some(pos) = self.queue.iter().position(|w| w.waiter_id == waiter_id) {
            return self.queue.remove(pos);
        }
        None
    }
}

#[derive(Debug)]
pub(crate) struct Semaphore {
    state: Mutex<State>,
}

impl Semaphore {
    pub(crate) fn new(permits: usize) -> Self {
        Self {
            state: Mutex::new(State::new(permits)),
        }
    }

    pub(crate) fn available_permits(&self) -> usize {
        self.state.lock().unwrap().permits
    }

    pub(crate) fn try_acquire(&self, permits: usize) -> Result<(), TryAcquireError> {
        if permits == 0 {
            return Ok(());
        }

        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(TryAcquireError::Closed);
        }

        // Fairness: do not allow barging ahead of queued waiters.
        if !state.queue.is_empty() {
            return Err(TryAcquireError::NoPermits);
        }

        if state.permits < permits {
            return Err(TryAcquireError::NoPermits);
        }

        state.permits -= permits;
        Ok(())
    }

    pub(crate) fn add_permits(&self, permits: usize) {
        if permits == 0 {
            return;
        }

        let wakers = {
            let mut state = self.state.lock().unwrap();
            state.add_permits_locked(permits)
        };

        for w in wakers {
            w.wake();
        }
    }

    pub(crate) fn close(&self) {
        let wakers = {
            let mut state = self.state.lock().unwrap();
            if state.closed {
                return;
            }
            state.closed = true;

            let mut wakers = Vec::new();
            while let Some(w) = state.queue.pop_front() {
                wakers.push(w.waker);
            }
            wakers
        };

        for w in wakers {
            w.wake();
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }

    pub(crate) fn acquire(&self, permits: usize) -> Acquire<'_> {
        Acquire::new(self, permits)
    }
}

pub(crate) struct Acquire<'a> {
    semaphore: &'a Semaphore,
    permits: usize,
    waiter_id: Option<u64>,
    completed: bool,
}

impl<'a> Acquire<'a> {
    fn new(semaphore: &'a Semaphore, permits: usize) -> Self {
        Self {
            semaphore,
            permits,
            waiter_id: None,
            completed: false,
        }
    }

    fn cancel_locked(&mut self, state: &mut State) -> Vec<Waker> {
        let Some(waiter_id) = self.waiter_id.take() else {
            return Vec::new();
        };

        // If we were satisfied but never observed completion, return all permits.
        if state.ready.remove(&waiter_id) {
            return state.add_permits_locked(self.permits);
        }

        // Otherwise, we were queued. Remove from queue and return any partially
        // acquired permits.
        let Some(waiter) = state.remove_waiter_locked(waiter_id) else {
            return Vec::new();
        };

        let acquired = waiter.required.saturating_sub(waiter.remaining);
        if acquired == 0 {
            return Vec::new();
        }

        state.add_permits_locked(acquired)
    }
}

impl std::future::Future for Acquire<'_> {
    type Output = Result<(), AcquireError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        if self.completed {
            return std::task::Poll::Ready(Ok(()));
        }

        let _task_id = async_runtime::current_task_id()
            .map(|t| t.0)
            .unwrap_or_else(|| panic!("des_tokio semaphore used outside async runtime polling"));

        let mut state = self.semaphore.state.lock().unwrap();
        if state.closed {
            return std::task::Poll::Ready(Err(AcquireError));
        }

        if let Some(waiter_id) = self.waiter_id {
            if state.ready.remove(&waiter_id) {
                self.waiter_id = None;
                self.completed = true;
                return std::task::Poll::Ready(Ok(()));
            }

            if let Some(w) = state.queue.iter_mut().find(|w| w.waiter_id == waiter_id) {
                w.waker = cx.waker().clone();
                return std::task::Poll::Pending;
            }

            // If we were woken, we should either still be queued or appear in
            // `ready`. If neither is true, treat as ready to avoid deadlock.
            self.waiter_id = None;
            self.completed = true;
            return std::task::Poll::Ready(Ok(()));
        }

        if state.queue.is_empty() {
            // Fast-path: acquire immediately if no queue.
            if state.permits >= self.permits {
                state.permits -= self.permits;
                self.completed = true;
                return std::task::Poll::Ready(Ok(()));
            }

            // Fairness/head-of-line: reserve all currently-available permits for
            // this waiter by partially assigning them.
            let available = state.permits;
            state.permits = 0;
            let remaining = self.permits.saturating_sub(available);

            let waiter_id = state.next_waiter_id;
            state.next_waiter_id += 1;
            state.queue.push_back(Waiter {
                waiter_id,
                //task_id,
                required: self.permits,
                remaining,
                waker: cx.waker().clone(),
            });
            self.waiter_id = Some(waiter_id);
            return std::task::Poll::Pending;
        }

        // Enqueue behind existing waiters.
        let waiter_id = state.next_waiter_id;
        state.next_waiter_id += 1;
        state.queue.push_back(Waiter {
            waiter_id,
            //task_id,
            required: self.permits,
            remaining: self.permits,
            waker: cx.waker().clone(),
        });
        self.waiter_id = Some(waiter_id);

        std::task::Poll::Pending
    }
}

impl Drop for Acquire<'_> {
    fn drop(&mut self) {
        if self.completed {
            return;
        }

        let wakers = {
            let mut state = self.semaphore.state.lock().unwrap();
            self.cancel_locked(&mut state)
        };

        for w in wakers {
            w.wake();
        }
    }
}
