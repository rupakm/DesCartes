use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering as AtomicOrdering},
    Arc, Mutex,
};
use std::task::{Context, Poll, Waker};

use des_core::async_runtime::{RuntimeEvent, TaskId};
use des_core::{defer_wake, Key};

use crate::runtime;

#[derive(Debug)]
pub enum JoinError {
    Cancelled,
}

struct JoinState<T> {
    completed: AtomicBool,
    cancelled: AtomicBool,
    result: Mutex<Option<T>>,
    waker: Mutex<Option<Waker>>,
}

impl<T> JoinState<T> {
    fn new() -> Self {
        Self {
            completed: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            result: Mutex::new(None),
            waker: Mutex::new(None),
        }
    }

    fn complete(&self, value: T) {
        *self.result.lock().unwrap() = Some(value);
        self.completed.store(true, AtomicOrdering::Release);

        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }

    fn set_cancelled(&self) {
        self.cancelled.store(true, AtomicOrdering::Release);
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }
}

pub struct JoinHandle<T> {
    task_id: TaskId,
    runtime_key: Key<RuntimeEvent>,
    state: Arc<JoinState<T>>,
}

impl<T> JoinHandle<T> {
    pub fn abort(&self) {
        self.state.set_cancelled();
        defer_wake(
            self.runtime_key,
            RuntimeEvent::Cancel {
                task_id: self.task_id,
            },
        );
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.state.cancelled.load(AtomicOrdering::Acquire) {
            return Poll::Ready(Err(JoinError::Cancelled));
        }

        if self.state.completed.load(AtomicOrdering::Acquire) {
            let value = self.state.result.lock().unwrap().take().expect("completed");
            return Poll::Ready(Ok(value));
        }

        *self.state.waker.lock().unwrap() = Some(cx.waker().clone());
        Poll::Pending
    }
}

pub fn spawn<F, T>(future: F) -> JoinHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    spawn_inner(future)
}

pub fn spawn_local<F, T>(future: F) -> JoinHandle<T>
where
    F: Future<Output = T> + 'static,
    T: 'static,
{
    spawn_local_inner(future)
}

fn spawn_inner<F, T>(future: F) -> JoinHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let runtime_handle = runtime::installed_handle();
    let runtime_key = runtime_handle.runtime_key();
    let state: Arc<JoinState<T>> = Arc::new(JoinState::new());

    let state_clone = state.clone();

    let task_id = runtime_handle.spawn(async move {
        let result = future.await;
        state_clone.complete(result);
    });

    JoinHandle {
        task_id,
        runtime_key,
        state,
    }
}

fn spawn_local_inner<F, T>(future: F) -> JoinHandle<T>
where
    F: Future<Output = T> + 'static,
    T: 'static,
{
    let local_handle = runtime::installed_local_handle();
    let runtime_handle = runtime::installed_handle();
    let runtime_key = runtime_handle.runtime_key();

    let state: Arc<JoinState<T>> = Arc::new(JoinState::new());
    let state_clone = state.clone();

    let task_id = local_handle.spawn_local(async move {
        let result = future.await;
        state_clone.complete(result);
    });

    JoinHandle {
        task_id,
        runtime_key,
        state,
    }
}
