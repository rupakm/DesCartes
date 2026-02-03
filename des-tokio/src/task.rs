use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering as AtomicOrdering},
    Arc, Mutex,
};
use std::task::{Context, Poll, Waker};

use descartes_core::async_runtime::{RuntimeEvent, TaskId};
use descartes_core::{defer_wake, Key};

use crate::{runtime, runtime_internal};

#[derive(Debug)]
pub enum JoinError {
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct AbortHandle {
    task_id: TaskId,
    runtime_key: Key<RuntimeEvent>,
    cancelled: Arc<AtomicBool>,
}

impl AbortHandle {
    pub fn abort(&self) {
        self.cancelled.store(true, AtomicOrdering::Release);
        defer_wake(
            self.runtime_key,
            RuntimeEvent::Cancel {
                task_id: self.task_id,
            },
        );
    }
}

struct JoinState<T> {
    completed: AtomicBool,
    cancelled: Arc<AtomicBool>,
    result: Mutex<Option<T>>,
    waker: Mutex<Option<Waker>>,
}

impl<T> JoinState<T> {
    fn new() -> Self {
        Self {
            completed: AtomicBool::new(false),
            cancelled: Arc::new(AtomicBool::new(false)),
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

    pub fn abort_handle(&self) -> AbortHandle {
        AbortHandle {
            task_id: self.task_id,
            runtime_key: self.runtime_key,
            cancelled: self.state.cancelled.clone(),
        }
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

pub struct JoinSet<T> {
    handles: VecDeque<JoinHandle<T>>,
}

impl<T> Default for JoinSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> JoinSet<T> {
    pub fn new() -> Self {
        Self {
            handles: VecDeque::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.handles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    #[track_caller]
    pub fn spawn<F>(&mut self, task: F) -> AbortHandle
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let jh = spawn(task);
        let abort = jh.abort_handle();
        self.handles.push_back(jh);
        abort
    }

    #[track_caller]
    pub fn spawn_local<F>(&mut self, task: F) -> AbortHandle
    where
        F: Future<Output = T> + 'static,
        T: 'static,
    {
        let jh = crate::task::spawn_local(task);
        let abort = jh.abort_handle();
        self.handles.push_back(jh);
        abort
    }

    pub async fn join_next(&mut self) -> Option<Result<T, JoinError>> {
        std::future::poll_fn(|cx| self.poll_join_next(cx)).await
    }

    pub fn poll_join_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<T, JoinError>>> {
        if self.handles.is_empty() {
            return Poll::Ready(None);
        }

        let n = self.handles.len();
        let mut any_pending = false;

        for _ in 0..n {
            let mut handle = self.handles.pop_front().expect("len checked");

            match Pin::new(&mut handle).poll(cx) {
                Poll::Ready(res) => {
                    // Do not reinsert.
                    return Poll::Ready(Some(res));
                }
                Poll::Pending => {
                    any_pending = true;
                    self.handles.push_back(handle);
                }
            }
        }

        if any_pending {
            Poll::Pending
        } else {
            // Should not happen: if there are handles, at least one should be pending.
            Poll::Pending
        }
    }

    pub fn abort_all(&mut self) {
        for jh in &self.handles {
            jh.abort();
        }
    }

    pub fn detach_all(&mut self) {
        self.handles.clear();
    }
}

impl<T> Drop for JoinSet<T> {
    fn drop(&mut self) {
        self.abort_all();
        // Dropping JoinHandles detaches. Cancellation is handled by abort_all.
    }
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

    runtime_internal::ensure_polled(runtime_key);

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

    runtime_internal::ensure_polled(runtime_key);

    JoinHandle {
        task_id,
        runtime_key,
        state,
    }
}
