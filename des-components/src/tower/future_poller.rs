//! Future Poller for asynchronous Tower service response handling.
//!
//! This module provides a mechanism for polling Tower service futures without blocking,
//! enabling open-loop client patterns where requests are sent at a rate independent
//! of response times.
//!
//! # Usage
//!
//! ```rust,no_run
//! use des_components::tower::{FuturePollerHandle, FuturePollerEvent};
//! use des_core::{Simulation, SimTime, Execute, Executor};
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut simulation = Simulation::default();
//!
//! // Create future poller handle (shared state)
//! let handle = FuturePollerHandle::new();
//!
//! // Create the component and add to simulation
//! let poller = handle.create_component();
//! let poller_key = simulation.add_component(poller);
//!
//! // Set the key on the handle (enables waker scheduling)
//! handle.set_key(poller_key);
//!
//! // Schedule Initialize event to trigger initial polling
//! simulation.schedule(SimTime::zero(), poller_key, FuturePollerEvent::Initialize);
//!
//! // Run simulation
//! Executor::timed(SimTime::from_millis(100)).execute(&mut simulation);
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use des_core::{Component, Key, Scheduler, create_des_waker, defer_wake};
use http::Response;

use super::{ServiceError, SimBody};

/// Unique identifier for a spawned future
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FutureId(pub u64);

/// Events for the FuturePoller component
#[derive(Debug, Clone)]
pub enum FuturePollerEvent {
    /// Poll a specific future that may be ready
    PollFuture { id: FutureId },
    /// Initialize the poller (sets up self_key)
    Initialize,
}

/// A pending future waiting to be polled
struct PendingFuture {
    /// The future to poll
    future: Pin<Box<dyn Future<Output = Result<Response<SimBody>, ServiceError>>>>,
    /// Callback to invoke when the future completes
    on_complete: Option<Box<dyn FnOnce(Result<Response<SimBody>, ServiceError>)>>,
}

/// Shared state for the FuturePoller
///
/// This allows the handle to spawn futures even after the component
/// has been moved into the simulation.
struct FuturePollerState {
    /// Pending futures waiting for responses
    futures: HashMap<FutureId, PendingFuture>,
    /// Next future ID to assign
    next_id: u64,
    /// This component's key (for scheduling events)
    self_key: Option<Key<FuturePollerEvent>>,
    /// Count of completed futures (for metrics)
    completed_count: u64,
}

impl FuturePollerState {
    fn new() -> Self {
        Self {
            futures: HashMap::new(),
            next_id: 0,
            self_key: None,
            completed_count: 0,
        }
    }
}

/// Handle for spawning futures on a FuturePoller
///
/// This handle can be cloned and used from anywhere to spawn futures,
/// even after the FuturePoller component has been added to the simulation.
#[derive(Clone)]
pub struct FuturePollerHandle {
    state: Arc<Mutex<FuturePollerState>>,
}

impl FuturePollerHandle {
    /// Create a new FuturePollerHandle
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(FuturePollerState::new())),
        }
    }

    /// Create a FuturePoller component from this handle
    ///
    /// The component should be added to the simulation, and then
    /// `set_key()` should be called with the component's key.
    pub fn create_component(&self) -> FuturePoller {
        FuturePoller {
            state: self.state.clone(),
        }
    }

    /// Set the component key for this poller
    ///
    /// This must be called after adding the poller to the simulation,
    /// before spawning any futures.
    pub fn set_key(&self, key: Key<FuturePollerEvent>) {
        self.state.lock().unwrap().self_key = Some(key);
    }

    /// Get the component key if set
    pub fn key(&self) -> Option<Key<FuturePollerEvent>> {
        self.state.lock().unwrap().self_key
    }

    /// Spawn a future to be polled asynchronously
    ///
    /// The future will be polled when it becomes ready, and the callback
    /// will be invoked with the result.
    ///
    /// # Arguments
    ///
    /// * `future` - The Tower service future to poll
    /// * `on_complete` - Callback invoked when the future completes
    ///
    /// # Returns
    ///
    /// A `FutureId` that can be used to track or cancel the future
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let future = service.call(request);
    /// handle.spawn(future, |result| {
    ///     match result {
    ///         Ok(response) => println!("Got response: {:?}", response.status()),
    ///         Err(e) => println!("Request failed: {:?}", e),
    ///     }
    /// });
    /// ```
    pub fn spawn<F, C>(&self, future: F, on_complete: C) -> FutureId
    where
        F: Future<Output = Result<Response<SimBody>, ServiceError>> + 'static,
        C: FnOnce(Result<Response<SimBody>, ServiceError>) + 'static,
    {
        let mut state = self.state.lock().unwrap();
        
        let id = FutureId(state.next_id);
        state.next_id += 1;

        state.futures.insert(
            id,
            PendingFuture {
                future: Box::pin(future),
                on_complete: Some(Box::new(on_complete)),
            },
        );

        // Schedule initial poll using defer_wake if we have a key and are in scheduler context
        if let Some(key) = state.self_key {
            defer_wake(key, FuturePollerEvent::PollFuture { id });
        }

        id
    }

    /// Spawn a future without a completion callback
    ///
    /// Useful when you only care about side effects or metrics collection
    /// that happens within the future itself.
    pub fn spawn_detached<F>(&self, future: F) -> FutureId
    where
        F: Future<Output = Result<Response<SimBody>, ServiceError>> + 'static,
    {
        self.spawn(future, |_| {})
    }

    /// Get the number of pending futures
    pub fn pending_count(&self) -> usize {
        self.state.lock().unwrap().futures.len()
    }

    /// Get the number of completed futures
    pub fn completed_count(&self) -> u64 {
        self.state.lock().unwrap().completed_count
    }

    /// Check if there are any pending futures
    pub fn has_pending(&self) -> bool {
        !self.state.lock().unwrap().futures.is_empty()
    }

    /// Create a DES-aware waker for a future using the unified waker utility.
    fn create_waker(state: &FuturePollerState, future_id: FutureId) -> std::task::Waker {
        if let Some(key) = state.self_key {
            create_des_waker(key, FuturePollerEvent::PollFuture { id: future_id })
        } else {
            // Fallback: create a no-op waker if key not set yet
            // This shouldn't happen in normal usage
            std::task::Waker::noop().clone()
        }
    }

    /// Poll a specific future
    fn poll_future(state: &mut FuturePollerState, id: FutureId) {
        // Check if future exists first
        if !state.futures.contains_key(&id) {
            return; // Future was already completed or cancelled
        }

        // Create DES-aware waker before getting mutable reference to future
        let waker = Self::create_waker(state, id);
        let mut cx = Context::from_waker(&waker);

        // Now get mutable reference and poll
        let Some(pending) = state.futures.get_mut(&id) else {
            return;
        };

        // Poll the future
        match pending.future.as_mut().poll(&mut cx) {
            Poll::Ready(result) => {
                // Future completed - remove and call callback
                if let Some(mut pending) = state.futures.remove(&id) {
                    if let Some(callback) = pending.on_complete.take() {
                        callback(result);
                    }

                    state.completed_count += 1;
                }
            }
            Poll::Pending => {
                // Future not ready - waker will schedule next poll when ready
            }
        }
    }
}

impl Default for FuturePollerHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Component that polls Tower service futures asynchronously
///
/// The `FuturePoller` enables open-loop client patterns by managing pending
/// futures and polling them when they become ready, without blocking the
/// simulation.
///
/// # How It Works
///
/// 1. When a future is spawned via `FuturePollerHandle::spawn()`, it's stored in shared state
/// 2. The initial poll registers a DES-aware waker with the future
/// 3. When the underlying oneshot channel receives data, the waker is triggered
/// 4. The waker schedules a `PollFuture` event at the current simulation time
/// 5. The event handler polls the future, which is now ready
/// 6. The completion callback is invoked with the result
///
/// This ensures futures are polled exactly when they become ready, with no
/// busy-waiting or missed completions.
///
/// # Important
///
/// Use `FuturePollerHandle` to create and interact with the poller. The handle
/// can be cloned and used to spawn futures even after the component is added
/// to the simulation.
pub struct FuturePoller {
    /// Shared state with the handle
    state: Arc<Mutex<FuturePollerState>>,
}

impl Component for FuturePoller {
    type Event = FuturePollerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        _scheduler: &mut Scheduler,
    ) {
        let mut state = self.state.lock().unwrap();
        
        // Store self_key on first event
        if state.self_key.is_none() {
            state.self_key = Some(self_id);
        }

        match event {
            FuturePollerEvent::PollFuture { id } => {
                FuturePollerHandle::poll_future(&mut state, *id);
            }
            FuturePollerEvent::Initialize => {
                // Poll all pending futures that haven't been polled yet
                // This handles the case where futures were spawned before
                // the simulation started running
                let pending_ids: Vec<_> = state.futures.keys().copied().collect();
                for id in pending_ids {
                    FuturePollerHandle::poll_future(&mut state, id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use des_core::{Execute, Executor, SimTime, Simulation};
    use std::cell::RefCell;
    use std::rc::Rc;
    use tokio::sync::oneshot;

    /// Simple future that completes when a oneshot channel receives a value
    struct TestFuture {
        receiver: oneshot::Receiver<String>,
    }

    impl Future for TestFuture {
        type Output = Result<Response<SimBody>, ServiceError>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            match Pin::new(&mut self.receiver).poll(cx) {
                Poll::Ready(Ok(msg)) => {
                    let response = Response::builder()
                        .status(200)
                        .body(SimBody::new(msg.into_bytes()))
                        .unwrap();
                    Poll::Ready(Ok(response))
                }
                Poll::Ready(Err(_)) => Poll::Ready(Err(ServiceError::Cancelled)),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    #[test]
    fn test_future_poller_handle_immediate_completion() {
        // Test that a future that's already ready completes immediately
        let mut simulation = Simulation::default();

        // Create handle
        let handle = FuturePollerHandle::new();

        // Create component and add to simulation
        let poller = handle.create_component();
        let poller_key = simulation.add_component(poller);

        // Set key on handle
        handle.set_key(poller_key);

        // Create a channel and send immediately (future will be ready on first poll)
        let (tx, rx) = oneshot::channel();
        tx.send("Immediate!".to_string()).unwrap();

        let completed = Rc::new(RefCell::new(false));
        let completed_clone = completed.clone();

        // Spawn future using handle
        let future = TestFuture { receiver: rx };
        handle.spawn(future, move |result| {
            assert!(result.is_ok());
            *completed_clone.borrow_mut() = true;
        });

        // Schedule Initialize event to trigger initial polling
        simulation.schedule(SimTime::zero(), poller_key, FuturePollerEvent::Initialize);

        // Run simulation
        Executor::timed(SimTime::from_millis(10)).execute(&mut simulation);

        // Future should have completed
        assert!(*completed.borrow());
    }

    #[test]
    fn test_future_poller_handle_waker_mechanism() {
        // Test that the waker correctly schedules poll events.
        // This test verifies that when a future becomes ready during simulation,
        // the waker properly schedules a poll event using the unified waker.
        
        let mut simulation = Simulation::default();

        // Create handle
        let handle = FuturePollerHandle::new();

        // Create component and add to simulation
        let poller = handle.create_component();
        let poller_key = simulation.add_component(poller);

        // Set key on handle
        handle.set_key(poller_key);

        let (tx, rx) = oneshot::channel();

        let completed = Rc::new(RefCell::new(false));
        let completed_clone = completed.clone();

        // Spawn future
        let future = TestFuture { receiver: rx };
        handle.spawn(future, move |result| {
            assert!(result.is_ok());
            *completed_clone.borrow_mut() = true;
        });

        // Schedule Initialize event to trigger initial polling
        simulation.schedule(SimTime::zero(), poller_key, FuturePollerEvent::Initialize);

        // Run one step - future should be pending after initial poll
        simulation.step();
        assert!(!*completed.borrow());
        assert_eq!(handle.pending_count(), 1);

        // Schedule a task that will send the response during simulation
        // This ensures the waker is triggered while in scheduler context
        let tx_cell = std::cell::RefCell::new(Some(tx));
        simulation.schedule_closure(SimTime::from_millis(5), move |_scheduler| {
            if let Some(tx) = tx_cell.borrow_mut().take() {
                tx.send("Delayed!".to_string()).unwrap();
            }
        });

        // Run simulation - the task will send, waker will defer, poll will complete
        Executor::timed(SimTime::from_millis(100)).execute(&mut simulation);

        // Future should now be completed
        assert!(*completed.borrow());
        assert_eq!(handle.pending_count(), 0);
        assert_eq!(handle.completed_count(), 1);
    }

    #[test]
    fn test_future_poller_handle_detached() {
        let mut simulation = Simulation::default();

        // Create handle
        let handle = FuturePollerHandle::new();

        // Create component and add to simulation
        let poller = handle.create_component();
        let poller_key = simulation.add_component(poller);

        // Set key on handle
        handle.set_key(poller_key);

        // Create channel and send immediately
        let (tx, rx) = oneshot::channel();
        tx.send("Done".to_string()).unwrap();

        // Spawn detached (no callback)
        handle.spawn_detached(TestFuture { receiver: rx });

        assert_eq!(handle.pending_count(), 1);

        // Schedule Initialize event
        simulation.schedule(SimTime::zero(), poller_key, FuturePollerEvent::Initialize);

        Executor::timed(SimTime::from_millis(100)).execute(&mut simulation);

        // Should be completed
        assert_eq!(handle.pending_count(), 0);
        assert_eq!(handle.completed_count(), 1);
    }

    #[test]
    fn test_future_poller_handle_clone() {
        let mut simulation = Simulation::default();

        // Create handle
        let handle = FuturePollerHandle::new();

        // Clone the handle
        let handle2 = handle.clone();

        // Create component and add to simulation
        let poller = handle.create_component();
        let poller_key = simulation.add_component(poller);

        // Set key on original handle
        handle.set_key(poller_key);

        // Verify clone has the key set
        assert!(handle2.key().is_some());

        // Spawn from both handles
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();

        tx1.send("First".to_string()).unwrap();
        tx2.send("Second".to_string()).unwrap();

        handle.spawn_detached(TestFuture { receiver: rx1 });
        handle2.spawn_detached(TestFuture { receiver: rx2 });

        assert_eq!(handle.pending_count(), 2);
        assert_eq!(handle2.pending_count(), 2);

        // Schedule Initialize event
        simulation.schedule(SimTime::zero(), poller_key, FuturePollerEvent::Initialize);

        Executor::timed(SimTime::from_millis(100)).execute(&mut simulation);

        assert_eq!(handle.completed_count(), 2);
        assert_eq!(handle2.completed_count(), 2);
    }

    #[test]
    fn test_future_id() {
        let id1 = FutureId(1);
        let id2 = FutureId(1);
        let id3 = FutureId(2);

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }
}
