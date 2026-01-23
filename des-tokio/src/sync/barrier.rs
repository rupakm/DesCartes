use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

use des_core::async_runtime;

#[derive(Debug)]
struct Waiter {
    task_id: u64,
    waker: Waker,
}

#[derive(Debug)]
struct State {
    parties: usize,
    arrived: usize,
    generation: u64,
    waiters: VecDeque<Waiter>,
}

impl State {
    fn new(parties: usize) -> Self {
        Self {
            parties,
            arrived: 0,
            generation: 0,
            waiters: VecDeque::new(),
        }
    }
}

/// Tokio-like `Barrier`.
///
/// Deterministic behavior:
/// - The "leader" is deterministically the last task to arrive for a generation.
/// - Waiter wake order is FIFO.
///
/// Cancellation safety: dropping a `wait()` future removes that task from the
/// current generation.
#[derive(Debug)]
pub struct Barrier {
    state: Mutex<State>,
}

impl Barrier {
    pub fn new(parties: usize) -> Self {
        assert!(parties > 0, "Barrier parties must be > 0");
        Self {
            state: Mutex::new(State::new(parties)),
        }
    }

    pub fn wait(&self) -> Wait<'_> {
        let generation = self.state.lock().unwrap().generation;
        Wait {
            barrier: self,
            generation,
            task_id: None,
            registered: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarrierWaitResult {
    is_leader: bool,
}

impl BarrierWaitResult {
    pub fn is_leader(&self) -> bool {
        self.is_leader
    }
}

pub struct Wait<'a> {
    barrier: &'a Barrier,
    generation: u64,
    task_id: Option<u64>,
    registered: bool,
}

impl Future for Wait<'_> {
    type Output = BarrierWaitResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let task_id = *this.task_id.get_or_insert_with(current_task_id_u64);
        let mut state = this.barrier.state.lock().unwrap();

        if state.generation != this.generation {
            return Poll::Ready(BarrierWaitResult { is_leader: false });
        }

        if this.registered {
            if let Some(waiter) = state.waiters.iter_mut().find(|w| w.task_id == task_id) {
                waiter.waker = cx.waker().clone();
            }
            return Poll::Pending;
        }

        // First poll for this task in the current generation.
        if state.arrived + 1 == state.parties {
            // Release the generation. This task is deterministically the leader.
            state.generation = state.generation.wrapping_add(1);
            state.arrived = 0;
            let wakers: Vec<_> = state.waiters.drain(..).map(|w| w.waker).collect();
            drop(state);

            for w in wakers {
                w.wake();
            }

            return Poll::Ready(BarrierWaitResult { is_leader: true });
        }

        state.arrived += 1;
        state.waiters.push_back(Waiter {
            task_id,
            waker: cx.waker().clone(),
        });
        this.registered = true;
        Poll::Pending
    }
}

impl Drop for Wait<'_> {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }

        let task_id = match self.task_id {
            Some(id) => id,
            None => return,
        };

        let mut state = self.barrier.state.lock().unwrap();
        if state.generation != self.generation {
            return;
        }

        if let Some(pos) = state.waiters.iter().position(|w| w.task_id == task_id) {
            state.waiters.remove(pos);
            state.arrived = state.arrived.saturating_sub(1);
        }
    }
}

fn current_task_id_u64() -> u64 {
    async_runtime::current_task_id()
        .map(|t| t.0)
        .unwrap_or_else(|| panic!("des_tokio::sync::Barrier used outside async runtime polling"))
}
