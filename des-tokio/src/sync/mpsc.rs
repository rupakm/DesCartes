use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// Error returned from `Sender::send` when the receiver has been dropped.
#[derive(Debug)]
pub struct SendError<T>(pub T);

/// Error returned from `Sender::try_send`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrySendError {
    Full,
    Closed,
}

/// Error returned from `Receiver::try_recv`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryRecvError {
    Empty,
    Closed,
}

#[derive(Debug)]
struct State<T> {
    capacity: usize,
    queue: VecDeque<T>,

    receiver_alive: bool,
    sender_count: usize,

    receiver_waker: Option<Waker>,

    // Deterministic wake ordering for blocked senders.
    next_waiter_id: u64,
    send_wait_order: VecDeque<u64>,
    send_waiters: HashMap<u64, Waker>,
}

impl<T> State<T> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            queue: VecDeque::with_capacity(capacity),
            receiver_alive: true,
            sender_count: 1,
            receiver_waker: None,
            next_waiter_id: 0,
            send_wait_order: VecDeque::new(),
            send_waiters: HashMap::new(),
        }
    }

    fn wake_receiver(&mut self) {
        if let Some(w) = self.receiver_waker.take() {
            w.wake();
        }
    }

    fn wake_one_sender(&mut self) {
        while let Some(id) = self.send_wait_order.pop_front() {
            if let Some(w) = self.send_waiters.remove(&id) {
                w.wake();
                break;
            }
        }
    }

    fn wake_all_senders(&mut self) {
        let ids: Vec<u64> = self.send_wait_order.drain(..).collect();
        for id in ids {
            if let Some(w) = self.send_waiters.remove(&id) {
                w.wake();
            }
        }

        for (_, w) in self.send_waiters.drain() {
            w.wake();
        }
    }
}

#[derive(Debug)]
pub struct Sender<T> {
    inner: Arc<Mutex<State<T>>>,
}

impl<T> Sender<T> {
    pub async fn send(&self, value: T) -> Result<(), SendError<T>> {
        SendFuture {
            inner: self.inner.clone(),
            value: Some(value),
            waiter_id: None,
        }
        .await
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError> {
        let mut state = self.inner.lock().unwrap();

        if !state.receiver_alive {
            return Err(TrySendError::Closed);
        }

        if state.queue.len() >= state.capacity {
            return Err(TrySendError::Full);
        }

        state.queue.push_back(value);
        state.wake_receiver();
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        let state = self.inner.lock().unwrap();
        !state.receiver_alive
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        let mut state = self.inner.lock().unwrap();
        state.sender_count += 1;
        drop(state);

        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut state = self.inner.lock().unwrap();
        state.sender_count = state.sender_count.saturating_sub(1);
        if state.sender_count == 0 {
            state.wake_receiver();
        }
    }
}

#[derive(Debug)]
pub struct Receiver<T> {
    inner: Arc<Mutex<State<T>>>,
}

impl<T> Receiver<T> {
    pub async fn recv(&mut self) -> Option<T> {
        RecvFuture {
            inner: self.inner.clone(),
        }
        .await
    }

    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        let mut state = self.inner.lock().unwrap();

        if let Some(v) = state.queue.pop_front() {
            state.wake_one_sender();
            return Ok(v);
        }

        if state.sender_count == 0 {
            return Err(TryRecvError::Closed);
        }

        Err(TryRecvError::Empty)
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut state = self.inner.lock().unwrap();
        state.receiver_alive = false;
        state.receiver_waker = None;
        state.wake_all_senders();
    }
}

struct SendFuture<T> {
    inner: Arc<Mutex<State<T>>>,
    value: Option<T>,
    waiter_id: Option<u64>,
}

// We do not rely on pinning invariants for `SendFuture`.
impl<T> Unpin for SendFuture<T> {}

impl<T> Future for SendFuture<T> {
    type Output = Result<(), SendError<T>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let current_waiter_id = self.waiter_id;

        // Check closed/full without mutating `self`.
        {
            let state = self.inner.lock().unwrap();

            if !state.receiver_alive {
                drop(state);
                let v = self.value.take().expect("polled after completion");
                return Poll::Ready(Err(SendError(v)));
            }

            if state.queue.len() >= state.capacity {
                // Register as a waiter.
                drop(state);

                if self.waiter_id.is_none() {
                    let mut state = self.inner.lock().unwrap();
                    let id = state.next_waiter_id;
                    state.next_waiter_id += 1;
                    state.send_wait_order.push_back(id);
                    drop(state);
                    self.waiter_id = Some(id);
                }

                let id = self.waiter_id.expect("set above");
                let mut state = self.inner.lock().unwrap();
                if !state.receiver_alive {
                    drop(state);
                    let v = self.value.take().expect("polled after completion");
                    return Poll::Ready(Err(SendError(v)));
                }
                state.send_waiters.insert(id, cx.waker().clone());
                return Poll::Pending;
            }
        }

        // Enqueue.
        let v = self.value.take().expect("polled after completion");

        let mut state = self.inner.lock().unwrap();
        if !state.receiver_alive {
            drop(state);
            return Poll::Ready(Err(SendError(v)));
        }

        if state.queue.len() >= state.capacity {
            // Lost the race: restore value and register as waiter.
            drop(state);
            self.value = Some(v);

            if self.waiter_id.is_none() {
                let mut state = self.inner.lock().unwrap();
                let id = state.next_waiter_id;
                state.next_waiter_id += 1;
                state.send_wait_order.push_back(id);
                drop(state);
                self.waiter_id = Some(id);
            }

            let id = self.waiter_id.expect("set above");
            let mut state = self.inner.lock().unwrap();
            if !state.receiver_alive {
                drop(state);
                let v = self.value.take().expect("polled after completion");
                return Poll::Ready(Err(SendError(v)));
            }
            state.send_waiters.insert(id, cx.waker().clone());
            return Poll::Pending;
        }

        state.queue.push_back(v);
        state.wake_receiver();

        if let Some(id) = current_waiter_id {
            state.send_waiters.remove(&id);
        }

        drop(state);
        self.waiter_id = None;

        Poll::Ready(Ok(()))
    }
}

impl<T> Drop for SendFuture<T> {
    fn drop(&mut self) {
        if let Some(id) = self.waiter_id.take() {
            let mut state = self.inner.lock().unwrap();
            state.send_waiters.remove(&id);
        }
    }
}

struct RecvFuture<T> {
    inner: Arc<Mutex<State<T>>>,
}

impl<T> Future for RecvFuture<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.inner.lock().unwrap();

        if let Some(v) = state.queue.pop_front() {
            state.wake_one_sender();
            return Poll::Ready(Some(v));
        }

        if state.sender_count == 0 {
            return Poll::Ready(None);
        }

        state.receiver_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    assert!(capacity > 0, "mpsc::channel capacity must be > 0");

    let inner = Arc::new(Mutex::new(State::new(capacity)));
    (
        Sender {
            inner: inner.clone(),
        },
        Receiver { inner },
    )
}
