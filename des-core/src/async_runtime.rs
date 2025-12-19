//! DES-based async runtime
//!
//! This module provides an async runtime that uses simulation time instead of real time,
//! enabling ergonomic async/await syntax while maintaining DES semantics.
//!
//! # Overview
//!
//! The runtime is implemented as a DES component that manages async tasks. All timing
//! operations go through the DES scheduler, ensuring deterministic simulation behavior.
//!
//! # Example
//!
//! ```ignore
//! use des_core::{Simulation, Executor, SimTime};
//! use des_core::async_runtime::{DesRuntime, RuntimeEvent, sim_sleep};
//! use std::time::Duration;
//!
//! async fn my_process() {
//!     loop {
//!         sim_sleep(Duration::from_millis(100)).await;
//!         println!("Tick!");
//!     }
//! }
//!
//! let mut sim = Simulation::default();
//! let mut runtime = DesRuntime::new();
//! let runtime_id = sim.add_component(runtime);
//!
//! // Spawn would be done via an event or direct access
//! Executor::timed(SimTime::from_secs(1)).execute(&mut sim);
//! ```

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::Duration;

use crate::scheduler::Scheduler;
use crate::{Component, Key, SimTime};

/// Unique identifier for async tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub u64);

/// Context for async task execution
/// 
/// This is stored in thread-local during polling so futures can access it.
#[derive(Debug)]
pub struct AsyncContext {
    /// Current simulation time
    pub current_time: SimTime,
    /// Timers registered by futures during this poll cycle
    pub pending_timers: Vec<(SimTime, TaskId)>,
    /// Tasks that need immediate wake (e.g., from channel send)
    pub pending_wakes: Vec<TaskId>,
}

impl AsyncContext {
    /// Create a new async context
    pub fn new(current_time: SimTime) -> Self {
        Self {
            current_time,
            pending_timers: Vec::new(),
            pending_wakes: Vec::new(),
        }
    }

    /// Register a timer to wake a task at a specific simulation time
    pub fn register_timer(&mut self, wake_at: SimTime, task_id: TaskId) {
        self.pending_timers.push((wake_at, task_id));
    }

    /// Register an immediate wake for a task
    pub fn register_wake(&mut self, task_id: TaskId) {
        self.pending_wakes.push(task_id);
    }
}

// Thread-local storage for the current async context during polling
// This avoids unsafe pointer manipulation in wakers
thread_local! {
    static CURRENT_CONTEXT: RefCell<Option<*mut AsyncContext>> = const { RefCell::new(None) };
    static CURRENT_TASK_ID: RefCell<Option<TaskId>> = const { RefCell::new(None) };
}

/// Set the current async context for the duration of a closure
fn with_async_context<F, R>(context: &mut AsyncContext, task_id: TaskId, f: F) -> R
where
    F: FnOnce() -> R,
{
    CURRENT_CONTEXT.with(|ctx| {
        *ctx.borrow_mut() = Some(context as *mut AsyncContext);
    });
    CURRENT_TASK_ID.with(|tid| {
        *tid.borrow_mut() = Some(task_id);
    });
    
    let result = f();
    
    CURRENT_CONTEXT.with(|ctx| {
        *ctx.borrow_mut() = None;
    });
    CURRENT_TASK_ID.with(|tid| {
        *tid.borrow_mut() = None;
    });
    
    result
}

/// Get the current simulation time (for use in futures)
pub fn current_sim_time() -> Option<SimTime> {
    CURRENT_CONTEXT.with(|ctx| {
        ctx.borrow().map(|ptr| unsafe { (*ptr).current_time })
    })
}

/// Register a timer with the current context (for use in futures)
fn register_timer_internal(wake_at: SimTime, task_id: TaskId) {
    CURRENT_CONTEXT.with(|ctx| {
        if let Some(ptr) = *ctx.borrow() {
            unsafe {
                (*ptr).register_timer(wake_at, task_id);
            }
        }
    });
}

/// Get the current task ID (for use in futures)
fn current_task_id() -> Option<TaskId> {
    CURRENT_TASK_ID.with(|tid| *tid.borrow())
}

/// Data stored in each waker - just the task ID
struct WakerData {
    task_id: TaskId,
}

impl WakerData {
    fn new(task_id: TaskId) -> Self {
        Self { task_id }
    }
}

// Waker vtable for our custom waker
static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    waker_clone,
    waker_wake,
    waker_wake_by_ref,
    waker_drop,
);

unsafe fn waker_clone(data: *const ()) -> RawWaker {
    let waker_data = &*(data as *const WakerData);
    let new_data = Box::new(WakerData::new(waker_data.task_id));
    RawWaker::new(Box::into_raw(new_data) as *const (), &WAKER_VTABLE)
}

unsafe fn waker_wake(data: *const ()) {
    let waker_data = Box::from_raw(data as *mut WakerData);
    // Register wake with current context if available
    CURRENT_CONTEXT.with(|ctx| {
        if let Some(ptr) = *ctx.borrow() {
            (*ptr).register_wake(waker_data.task_id);
        }
    });
}

unsafe fn waker_wake_by_ref(data: *const ()) {
    let waker_data = &*(data as *const WakerData);
    CURRENT_CONTEXT.with(|ctx| {
        if let Some(ptr) = *ctx.borrow() {
            (*ptr).register_wake(waker_data.task_id);
        }
    });
}

unsafe fn waker_drop(data: *const ()) {
    drop(Box::from_raw(data as *mut WakerData));
}

/// Create a waker for a task
fn create_waker(task_id: TaskId) -> Waker {
    let data = Box::new(WakerData::new(task_id));
    let raw_waker = RawWaker::new(Box::into_raw(data) as *const (), &WAKER_VTABLE);
    unsafe { Waker::from_raw(raw_waker) }
}

/// Events that drive the async runtime
pub enum RuntimeEvent {
    /// Poll all ready tasks
    Poll,
    /// A timer fired, wake specific task
    TimerFired {
        /// The task to wake
        task_id: TaskId,
    },
    /// External component woke a task (e.g., message received)
    ExternalWake {
        /// The task to wake
        task_id: TaskId,
    },
    /// Spawn a new task (boxed to avoid generic in enum)
    Spawn {
        /// The future to spawn, boxed and pinned
        future: Pin<Box<dyn Future<Output = ()>>>,
    },
}

impl std::fmt::Debug for RuntimeEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeEvent::Poll => write!(f, "Poll"),
            RuntimeEvent::TimerFired { task_id } => {
                f.debug_struct("TimerFired").field("task_id", task_id).finish()
            }
            RuntimeEvent::ExternalWake { task_id } => {
                f.debug_struct("ExternalWake").field("task_id", task_id).finish()
            }
            RuntimeEvent::Spawn { .. } => {
                f.debug_struct("Spawn").field("future", &"<future>").finish()
            }
        }
    }
}

/// A suspended async task
struct Task {
    future: Pin<Box<dyn Future<Output = ()>>>,
}

/// DES-based async runtime component
///
/// Manages async tasks and integrates with the DES scheduler for timing.
pub struct DesRuntime {
    next_task_id: u64,
    tasks: HashMap<TaskId, Task>,
    ready_queue: VecDeque<TaskId>,
}

impl Default for DesRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl DesRuntime {
    /// Create a new async runtime
    pub fn new() -> Self {
        Self {
            next_task_id: 0,
            tasks: HashMap::new(),
            ready_queue: VecDeque::new(),
        }
    }

    /// Spawn a new async task
    ///
    /// Returns the TaskId of the spawned task. The task will be polled
    /// when the next Poll event is processed.
    pub fn spawn<F>(&mut self, future: F) -> TaskId
    where
        F: Future<Output = ()> + 'static,
    {
        self.spawn_boxed(Box::pin(future))
    }

    /// Spawn a boxed future
    pub fn spawn_boxed(&mut self, future: Pin<Box<dyn Future<Output = ()>>>) -> TaskId {
        let task_id = TaskId(self.next_task_id);
        self.next_task_id += 1;

        let task = Task { future };

        self.tasks.insert(task_id, task);
        self.ready_queue.push_back(task_id);
        task_id
    }

    /// Get the number of active tasks
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Check if there are any active tasks
    pub fn has_tasks(&self) -> bool {
        !self.tasks.is_empty()
    }

    /// Poll a single task
    fn poll_task(&mut self, task_id: TaskId, context: &mut AsyncContext) -> Option<Poll<()>> {
        let task = self.tasks.get_mut(&task_id)?;

        let waker = create_waker(task_id);
        let mut cx = Context::from_waker(&waker);

        // Set up thread-local context and poll
        let result = with_async_context(context, task_id, || {
            task.future.as_mut().poll(&mut cx)
        });

        Some(result)
    }

    /// Poll all ready tasks
    fn poll_ready_tasks(&mut self, context: &mut AsyncContext) {
        // Process ready queue - note we drain it to avoid borrowing issues
        let ready: Vec<TaskId> = self.ready_queue.drain(..).collect();

        for task_id in ready {
            if let Some(poll_result) = self.poll_task(task_id, context) {
                match poll_result {
                    Poll::Ready(()) => {
                        // Task completed, remove it
                        self.tasks.remove(&task_id);
                    }
                    Poll::Pending => {
                        // Task is waiting - it should have registered a timer or wake
                    }
                }
            }
        }

        // Add any tasks that were woken during polling back to ready queue
        for task_id in context.pending_wakes.drain(..) {
            if self.tasks.contains_key(&task_id) && !self.ready_queue.contains(&task_id) {
                self.ready_queue.push_back(task_id);
            }
        }
    }
}

impl Component for DesRuntime {
    type Event = RuntimeEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        // Create async context for this polling cycle
        let mut context = AsyncContext::new(scheduler.time());

        match event {
            RuntimeEvent::Poll => {
                self.poll_ready_tasks(&mut context);
            }
            RuntimeEvent::TimerFired { task_id } => {
                if self.tasks.contains_key(task_id) {
                    self.ready_queue.push_back(*task_id);
                }
                self.poll_ready_tasks(&mut context);
            }
            RuntimeEvent::ExternalWake { task_id } => {
                if self.tasks.contains_key(task_id) {
                    self.ready_queue.push_back(*task_id);
                }
                self.poll_ready_tasks(&mut context);
            }
            RuntimeEvent::Spawn { future: _ } => {
                // Clone the future - this is a bit awkward but necessary
                // In practice, users would call spawn() directly on the runtime
                // This event is for spawning from other components
                // For now, we'll just trigger a poll
                self.poll_ready_tasks(&mut context);
            }
        }

        // Schedule timer events collected during polling
        for (wake_time, task_id) in context.pending_timers {
            if wake_time <= scheduler.time() {
                // Timer already expired, wake immediately
                scheduler.schedule_now(self_id, RuntimeEvent::TimerFired { task_id });
            } else {
                // Schedule for future
                let delay = wake_time - scheduler.time();
                scheduler.schedule(
                    SimTime::from_duration(delay),
                    self_id,
                    RuntimeEvent::TimerFired { task_id },
                );
            }
        }

        // If there are still ready tasks, schedule another poll
        if !self.ready_queue.is_empty() {
            scheduler.schedule_now(self_id, RuntimeEvent::Poll);
        }
    }
}

/// A future that completes after a simulated delay
///
/// This is the primary timing primitive for async code in the DES runtime.
/// Unlike `tokio::time::sleep`, this uses simulation time.
pub struct SimSleep {
    duration: Duration,
    target_time: Option<SimTime>,
    registered: bool,
}

impl SimSleep {
    /// Create a new SimSleep that will complete after the given duration
    ///
    /// Note: The actual target time is calculated when first polled,
    /// based on the current simulation time at that moment.
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            target_time: None,
            registered: false,
        }
    }

    /// Create a SimSleep with a specific target time
    pub fn until(target_time: SimTime) -> Self {
        Self {
            duration: Duration::ZERO,
            target_time: Some(target_time),
            registered: false,
        }
    }
}

impl Future for SimSleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        // Get current time and task ID from thread-local context
        let current_time = match current_sim_time() {
            Some(t) => t,
            None => return Poll::Pending, // No context available
        };
        
        let task_id = match current_task_id() {
            Some(id) => id,
            None => return Poll::Pending,
        };

        // On first poll, calculate actual target time
        if self.target_time.is_none() {
            self.target_time = Some(current_time + self.duration);
        }
        
        let target = self.target_time.unwrap();

        // Check if we've reached the target time
        if current_time >= target {
            Poll::Ready(())
        } else {
            if !self.registered {
                // Register timer with the context
                register_timer_internal(target, task_id);
                self.registered = true;
            }
            Poll::Pending
        }
    }
}

/// Sleep for a duration in simulation time
///
/// This is the primary way to introduce delays in async simulation code.
///
/// # Example
///
/// ```ignore
/// use des_core::async_runtime::sim_sleep;
/// use std::time::Duration;
///
/// async fn my_task() {
///     // Wait for 100ms of simulation time
///     sim_sleep(Duration::from_millis(100)).await;
///     println!("100ms passed in simulation!");
/// }
/// ```
pub fn sim_sleep(duration: Duration) -> SimSleep {
    SimSleep::new(duration)
}

/// Sleep until a specific simulation time
///
/// # Example
///
/// ```ignore
/// use des_core::async_runtime::sim_sleep_until;
/// use des_core::SimTime;
///
/// async fn my_task() {
///     // Wait until simulation time reaches 1 second
///     sim_sleep_until(SimTime::from_secs(1)).await;
/// }
/// ```
pub fn sim_sleep_until(target: SimTime) -> SimSleep {
    SimSleep::until(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Execute, Executor, Simulation};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn test_task_id() {
        let id1 = TaskId(1);
        let id2 = TaskId(1);
        let id3 = TaskId(2);

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_async_context() {
        let mut ctx = AsyncContext::new(SimTime::from_millis(100));
        assert_eq!(ctx.current_time, SimTime::from_millis(100));
        assert!(ctx.pending_timers.is_empty());
        assert!(ctx.pending_wakes.is_empty());

        ctx.register_timer(SimTime::from_millis(200), TaskId(1));
        ctx.register_wake(TaskId(2));

        assert_eq!(ctx.pending_timers.len(), 1);
        assert_eq!(ctx.pending_wakes.len(), 1);
    }

    #[test]
    fn test_runtime_spawn() {
        let mut runtime = DesRuntime::new();
        assert_eq!(runtime.task_count(), 0);
        assert!(!runtime.has_tasks());

        let task_id = runtime.spawn(async {});
        assert_eq!(task_id, TaskId(0));
        assert_eq!(runtime.task_count(), 1);
        assert!(runtime.has_tasks());

        let task_id2 = runtime.spawn(async {});
        assert_eq!(task_id2, TaskId(1));
        assert_eq!(runtime.task_count(), 2);
    }

    #[test]
    fn test_simple_task_completion() {
        let mut sim = Simulation::default();

        let completed = Rc::new(RefCell::new(false));
        let completed_clone = completed.clone();

        let mut runtime = DesRuntime::new();
        runtime.spawn(async move {
            *completed_clone.borrow_mut() = true;
        });

        let runtime_id = sim.add_component(runtime);

        // Schedule initial poll
        sim.schedule(SimTime::zero(), runtime_id, RuntimeEvent::Poll);

        // Run simulation
        Executor::timed(SimTime::from_millis(100)).execute(&mut sim);

        assert!(*completed.borrow());
    }

    #[test]
    fn test_sim_sleep() {
        let mut sim = Simulation::default();

        let wake_times = Rc::new(RefCell::new(Vec::new()));
        let wake_times_clone = wake_times.clone();

        let mut runtime = DesRuntime::new();
        runtime.spawn(async move {
            // Record time at each wake
            // Note: We can't easily get current time in the async block
            // This test mainly verifies the task runs multiple times
            wake_times_clone.borrow_mut().push(1);
            sim_sleep(Duration::from_millis(50)).await;
            wake_times_clone.borrow_mut().push(2);
            sim_sleep(Duration::from_millis(50)).await;
            wake_times_clone.borrow_mut().push(3);
        });

        let runtime_id = sim.add_component(runtime);

        // Schedule initial poll
        sim.schedule(SimTime::zero(), runtime_id, RuntimeEvent::Poll);

        // Run simulation for 200ms
        Executor::timed(SimTime::from_millis(200)).execute(&mut sim);

        let times = wake_times.borrow();
        assert_eq!(*times, vec![1, 2, 3]);
    }

    #[test]
    fn test_multiple_tasks() {
        let mut sim = Simulation::default();

        let results = Rc::new(RefCell::new(Vec::new()));
        let results1 = results.clone();
        let results2 = results.clone();

        let mut runtime = DesRuntime::new();

        // Task 1: wakes at 50ms and 150ms
        runtime.spawn(async move {
            results1.borrow_mut().push("t1-start");
            sim_sleep(Duration::from_millis(50)).await;
            results1.borrow_mut().push("t1-50ms");
            sim_sleep(Duration::from_millis(100)).await;
            results1.borrow_mut().push("t1-150ms");
        });

        // Task 2: wakes at 100ms
        runtime.spawn(async move {
            results2.borrow_mut().push("t2-start");
            sim_sleep(Duration::from_millis(100)).await;
            results2.borrow_mut().push("t2-100ms");
        });

        let runtime_id = sim.add_component(runtime);
        sim.schedule(SimTime::zero(), runtime_id, RuntimeEvent::Poll);

        Executor::timed(SimTime::from_millis(200)).execute(&mut sim);

        let r = results.borrow();
        // Both tasks start immediately
        assert!(r.contains(&"t1-start"));
        assert!(r.contains(&"t2-start"));
        // Task 1 wakes at 50ms
        assert!(r.contains(&"t1-50ms"));
        // Task 2 wakes at 100ms
        assert!(r.contains(&"t2-100ms"));
        // Task 1 wakes at 150ms
        assert!(r.contains(&"t1-150ms"));
    }

    #[test]
    fn test_external_wake() {
        let mut sim = Simulation::default();

        let woken = Rc::new(RefCell::new(false));
        let woken_clone = woken.clone();

        let mut runtime = DesRuntime::new();
        let task_id = runtime.spawn(async move {
            // This task will be woken externally
            // In a real scenario, it would await on a channel or similar
            *woken_clone.borrow_mut() = true;
        });

        let runtime_id = sim.add_component(runtime);

        // Schedule external wake at 50ms
        sim.schedule(
            SimTime::from_millis(50),
            runtime_id,
            RuntimeEvent::ExternalWake { task_id },
        );

        Executor::timed(SimTime::from_millis(100)).execute(&mut sim);

        assert!(*woken.borrow());
    }
}
