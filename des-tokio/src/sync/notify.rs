use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

#[derive(Debug, Default)]
struct State {
    permits: u64,
    waiters: VecDeque<Waker>,
}

/// Tokio-like `Notify`.
///
/// Deterministic behavior: waiter wake order is FIFO.
///
/// Note: this implementation prioritizes simplicity and determinism. It does not
/// attempt to de-duplicate wakers or remove cancelled waiters from the internal
/// queue; waking a stale waker is benign in this single-threaded DES runtime.
#[derive(Clone, Debug, Default)]
pub struct Notify {
    inner: Arc<Mutex<State>>,
}

impl Notify {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn notify_one(&self) {
        // `Notified` completes by consuming a permit. If we wake a waiter without
        // producing a permit, the woken task will just re-register its waker.
        let waker = {
            let mut state = self.inner.lock().unwrap();
            state.permits = state.permits.saturating_add(1);
            state.waiters.pop_front()
        };

        if let Some(w) = waker {
            w.wake();
        }
    }

    pub fn notify_waiters(&self) {
        // Wake all currently-waiting tasks. Each waiter needs a permit so that
        // its `Notified` future can complete when re-polled.
        let wakers = {
            let mut state = self.inner.lock().unwrap();
            let wakers: Vec<_> = state.waiters.drain(..).collect();
            state.permits = state
                .permits
                .saturating_add(u64::try_from(wakers.len()).unwrap_or(u64::MAX));
            wakers
        };

        for w in wakers {
            w.wake();
        }
    }

    pub fn notified(&self) -> Notified {
        Notified {
            inner: self.inner.clone(),
        }
    }
}

pub struct Notified {
    inner: Arc<Mutex<State>>,
}

impl Future for Notified {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.inner.lock().unwrap();

        if state.permits > 0 {
            state.permits -= 1;
            return Poll::Ready(());
        }

        state.waiters.push_back(cx.waker().clone());
        Poll::Pending
    }
}
