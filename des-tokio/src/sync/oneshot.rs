use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecvError;

impl std::fmt::Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "channel closed")
    }
}

impl std::error::Error for RecvError {}


#[derive(Debug)]
struct State<T> {
    value: Option<T>,
    waker: Option<Waker>,
    sender_alive: bool,
    receiver_alive: bool,
}

impl<T> State<T> {
    fn new() -> Self {
        Self {
            value: None,
            waker: None,
            sender_alive: true,
            receiver_alive: true,
        }
    }
}

#[derive(Debug)]
pub struct Sender<T> {
    inner: Arc<Mutex<State<T>>>,
}

impl<T> Sender<T> {
    pub fn send(self, value: T) -> Result<(), T> {
        let waker_to_wake = {
            let mut state = self.inner.lock().unwrap();

            if !state.receiver_alive {
                return Err(value);
            }

            state.value = Some(value);
            state.sender_alive = false;
            state.waker.take()
        };

        if let Some(waker) = waker_to_wake {
            waker.wake();
        }

        Ok(())
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let waker_to_wake = {
            let mut state = self.inner.lock().unwrap();
            state.sender_alive = false;
            state.waker.take()
        };

        if let Some(waker) = waker_to_wake {
            waker.wake();
        }
    }
}

#[derive(Debug)]
pub struct Receiver<T> {
    inner: Arc<Mutex<State<T>>>,
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, RecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.inner.lock().unwrap();

        if let Some(value) = state.value.take() {
            return Poll::Ready(Ok(value));
        }

        if !state.sender_alive {
            return Poll::Ready(Err(RecvError));
        }

        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut state = self.inner.lock().unwrap();
        state.receiver_alive = false;
        state.waker = None;
    }
}

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Mutex::new(State::<T>::new()));
    (
        Sender {
            inner: inner.clone(),
        },
        Receiver { inner },
    )
}
