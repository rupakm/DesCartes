use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// Error returned from [`Sender::send`].
#[derive(Debug)]
pub struct SendError<T>(pub T);

/// Error returned from [`Receiver::changed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecvError;

#[derive(Debug)]
struct State<T> {
    sender_alive: bool,
    version: u64,
    value: Arc<T>,

    next_receiver_id: u64,
    receiver_alive: HashMap<u64, bool>,

    // Receiver wakers keyed by receiver id.
    receiver_wakers: HashMap<u64, Waker>,

    // Deterministic wake ordering (FIFO by receiver creation order).
    receiver_order: Vec<u64>,
}

impl<T> State<T> {
    fn wake_all(&mut self) {
        // Wake in deterministic receiver creation order.
        let ids: Vec<u64> = self.receiver_order.clone();
        for id in ids {
            if !self.receiver_alive.get(&id).copied().unwrap_or(false) {
                continue;
            }
            if let Some(w) = self.receiver_wakers.remove(&id) {
                w.wake();
            }
        }
    }

    fn remove_receiver(&mut self, id: u64) {
        self.receiver_alive.remove(&id);
        self.receiver_wakers.remove(&id);
        if let Some(pos) = self.receiver_order.iter().position(|x| *x == id) {
            self.receiver_order.remove(pos);
        }
    }

    fn add_receiver(&mut self) -> u64 {
        let id = self.next_receiver_id;
        self.next_receiver_id += 1;
        self.receiver_alive.insert(id, true);
        self.receiver_order.push(id);
        id
    }
}

/// Creates a new watch channel, returning the sender and receiver.
///
/// The receiver initially observes the provided value.
pub fn channel<T>(value: T) -> (Sender<T>, Receiver<T>) {
    let state = Arc::new(Mutex::new(State {
        sender_alive: true,
        version: 0,
        value: Arc::new(value),
        next_receiver_id: 0,
        receiver_alive: HashMap::new(),
        receiver_wakers: HashMap::new(),
        receiver_order: Vec::new(),
    }));

    let id = {
        let mut st = state.lock().unwrap();
        st.add_receiver()
    };

    (
        Sender {
            inner: state.clone(),
        },
        Receiver {
            inner: state,
            id,
            seen_version: 0,
        },
    )
}

#[derive(Debug)]
pub struct Sender<T> {
    inner: Arc<Mutex<State<T>>>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Sender<T> {
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let mut st = self.inner.lock().unwrap();
        if !st.sender_alive {
            return Err(SendError(value));
        }

        st.value = Arc::new(value);
        st.version = st.version.saturating_add(1);
        st.wake_all();
        Ok(())
    }

    pub fn borrow(&self) -> WatchRef<T> {
        let st = self.inner.lock().unwrap();
        WatchRef {
            value: st.value.clone(),
        }
    }

    pub fn subscribe(&self) -> Receiver<T> {
        let mut st = self.inner.lock().unwrap();
        let id = st.add_receiver();
        let seen_version = st.version;
        Receiver {
            inner: self.inner.clone(),
            id,
            seen_version,
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut st = self.inner.lock().unwrap();
        if !st.sender_alive {
            return;
        }
        st.sender_alive = false;
        st.wake_all();
    }
}

#[derive(Debug)]
pub struct Receiver<T> {
    inner: Arc<Mutex<State<T>>>,
    id: u64,
    seen_version: u64,
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        let mut st = self.inner.lock().unwrap();
        let id = st.add_receiver();
        let seen_version = st.version;
        Self {
            inner: self.inner.clone(),
            id,
            seen_version,
        }
    }
}

impl<T> Receiver<T> {
    pub fn borrow(&self) -> WatchRef<T> {
        let st = self.inner.lock().unwrap();
        WatchRef {
            value: st.value.clone(),
        }
    }

    pub fn borrow_and_update(&mut self) -> WatchRef<T> {
        let st = self.inner.lock().unwrap();
        self.seen_version = st.version;
        WatchRef {
            value: st.value.clone(),
        }
    }

    pub fn has_changed(&self) -> Result<bool, RecvError> {
        let st = self.inner.lock().unwrap();
        if st.version != self.seen_version {
            return Ok(true);
        }
        if !st.sender_alive {
            return Err(RecvError);
        }
        Ok(false)
    }

    pub async fn changed(&mut self) -> Result<(), RecvError> {
        Changed { rx: self }.await
    }

    fn poll_changed(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), RecvError>> {
        let mut st = self.inner.lock().unwrap();

        if st.version != self.seen_version {
            self.seen_version = st.version;
            return Poll::Ready(Ok(()));
        }

        if !st.sender_alive {
            return Poll::Ready(Err(RecvError));
        }

        st.receiver_wakers.insert(self.id, cx.waker().clone());
        Poll::Pending
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut st = self.inner.lock().unwrap();
        st.remove_receiver(self.id);
    }
}

pub struct WatchRef<T> {
    value: Arc<T>,
}

impl<T> std::ops::Deref for WatchRef<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

struct Changed<'a, T> {
    rx: &'a mut Receiver<T>,
}

impl<T> Future for Changed<'_, T> {
    type Output = Result<(), RecvError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.rx.poll_changed(cx)
    }
}
