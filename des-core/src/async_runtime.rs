//! DES-based async runtime
//!
//! This module provides an async runtime that uses simulation time instead of real time,
//! enabling ergonomic async/await syntax while maintaining discrete event simulation semantics.
//!
//! # Overview
//!
//! The async runtime allows you to write simulation code using familiar async/await patterns
//! while ensuring all timing operations use simulation time rather than wall-clock time.
//! This is crucial for deterministic, reproducible simulations that can run faster or slower
//! than real time.
//!
//! # Key Components
//!
//! - [`DesRuntime`]: The main runtime component that manages async tasks
//! - [`sim_sleep`]: Sleep for a duration in simulation time
//! - [`sim_sleep_until`]: Sleep until a specific simulation time
//! - [`SimSleep`]: The future returned by sleep functions
//!
//! # Basic Usage
//!
//! ```ignore
//! use des_core::{Simulation, Execute, Executor, SimTime};
//! use des_core::async_runtime::{DesRuntime, RuntimeEvent, sim_sleep};
//! use std::time::Duration;
//!
//! // Create simulation and runtime
//! let mut sim = Simulation::default();
//! let mut runtime = DesRuntime::new();
//!
//! // Spawn an async task
//! runtime.spawn(async {
//!     println!("Task starting");
//!     sim_sleep(Duration::from_millis(100)).await;
//!     println!("100ms of simulation time has passed");
//!     sim_sleep(Duration::from_millis(50)).await;
//!     println!("Task completed after 150ms total");
//! });
//!
//! // Add runtime to simulation and start it
//! let runtime_id = sim.add_component(runtime);
//! sim.schedule(SimTime::zero(), runtime_id, RuntimeEvent::Poll);
//!
//! // Run simulation
//! Executor::timed(SimTime::from_millis(200)).execute(&mut sim);
//! ```
//!
//! # Timer Implementation
//!
//! The runtime uses an event-driven approach for timing:
//!
//! 1. When a `SimSleep` future is polled, it calculates the target wake time
//! 2. It schedules an `ExternalWake` event at that exact simulation time
//! 3. When the event fires, the runtime wakes the sleeping task
//! 4. The task continues execution from where it left off
//!
//! This approach ensures:
//! - **Precision**: Tasks wake at exactly the right simulation time
//! - **Efficiency**: No continuous polling or background threads
//! - **Determinism**: Execution order is fully determined by simulation time
//!
//! # Multiple Tasks
//!
//! The runtime can manage multiple concurrent async tasks:
//!
//! ```ignore
//! // Spawn multiple tasks with different timing
//! runtime.spawn(async {
//!     for i in 1..=3 {
//!         println!("Fast task iteration {}", i);
//!         sim_sleep(Duration::from_millis(30)).await;
//!     }
//! });
//!
//! runtime.spawn(async {
//!     sim_sleep(Duration::from_millis(50)).await;
//!     println!("Slow task completed");
//! });
//! ```
//!
//! Tasks are scheduled and executed based on simulation time, not the order they were spawned.

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
/// This context is established during task polling to provide access to simulation
/// state and coordinate task waking. It's stored in thread-local storage so that
/// futures can access the current simulation time and register wake events.
#[derive(Debug)]
pub struct AsyncContext {
    /// Current simulation time when this context was created
    pub current_time: SimTime,
    /// Tasks that need immediate wake (e.g., from external events or channels)
    pub pending_wakes: Vec<TaskId>,
}

impl AsyncContext {
    /// Create a new async context
    pub fn new(current_time: SimTime) -> Self {
        Self {
            current_time,
            pending_wakes: Vec::new(),
        }
    }

    /// Register an immediate wake for a task
    pub fn register_wake(&mut self, task_id: TaskId) {
        self.pending_wakes.push(task_id);
    }
}

// Thread-local storage for the current async context during polling
// 
// This design allows futures to access simulation state without requiring
// explicit context passing. The context is only valid during task polling
// and provides access to:
// - Current simulation time via current_sim_time()
// - Task identification for wake events
// - Scheduler access for event scheduling
// - Runtime key for ExternalWake events
thread_local! {
    static CURRENT_CONTEXT: RefCell<Option<*mut AsyncContext>> = const { RefCell::new(None) };
    static CURRENT_TASK_ID: RefCell<Option<TaskId>> = const { RefCell::new(None) };
    static CURRENT_SCHEDULER: RefCell<Option<*mut Scheduler>> = const { RefCell::new(None) };
    static CURRENT_RUNTIME_KEY: RefCell<Option<Key<RuntimeEvent>>> = const { RefCell::new(None) };
}

/// Set the current async context for the duration of a closure
fn with_async_context<F, R>(
    context: &mut AsyncContext,
    scheduler: &mut Scheduler,
    task_id: TaskId,
    runtime_key: Option<Key<RuntimeEvent>>,
    f: F,
) -> R
where
    F: FnOnce() -> R,
{
    CURRENT_CONTEXT.with(|ctx| {
        *ctx.borrow_mut() = Some(context as *mut AsyncContext);
    });
    CURRENT_TASK_ID.with(|tid| {
        *tid.borrow_mut() = Some(task_id);
    });
    CURRENT_SCHEDULER.with(|sched| {
        *sched.borrow_mut() = Some(scheduler as *mut Scheduler);
    });
    CURRENT_RUNTIME_KEY.with(|key| {
        *key.borrow_mut() = runtime_key;
    });
    
    let result = f();
    
    CURRENT_CONTEXT.with(|ctx| {
        *ctx.borrow_mut() = None;
    });
    CURRENT_TASK_ID.with(|tid| {
        *tid.borrow_mut() = None;
    });
    CURRENT_SCHEDULER.with(|sched| {
        *sched.borrow_mut() = None;
    });
    CURRENT_RUNTIME_KEY.with(|key| {
        *key.borrow_mut() = None;
    });
    
    result
}

/// Get the current simulation time (for use in futures)
pub fn current_sim_time() -> Option<SimTime> {
    CURRENT_CONTEXT.with(|ctx| {
        ctx.borrow().map(|ptr| unsafe { (*ptr).current_time })
    })
}

/// Get the current task ID (for use in futures)
pub fn current_task_id() -> Option<TaskId> {
    CURRENT_TASK_ID.with(|tid| *tid.borrow())
}

/// Data stored in each waker - includes task ID and runtime key
struct WakerData {
    task_id: TaskId,
    runtime_key: Option<Key<RuntimeEvent>>,
}

impl WakerData {
    fn new(task_id: TaskId, runtime_key: Option<Key<RuntimeEvent>>) -> Self {
        Self { task_id, runtime_key }
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
    let new_data = Box::new(WakerData::new(waker_data.task_id, waker_data.runtime_key));
    RawWaker::new(Box::into_raw(new_data) as *const (), &WAKER_VTABLE)
}

unsafe fn waker_wake(data: *const ()) {
    let waker_data = Box::from_raw(data as *mut WakerData);
    wake_task_internal(waker_data.task_id, waker_data.runtime_key);
}

unsafe fn waker_wake_by_ref(data: *const ()) {
    let waker_data = &*(data as *const WakerData);
    wake_task_internal(waker_data.task_id, waker_data.runtime_key);
}

unsafe fn waker_drop(data: *const ()) {
    drop(Box::from_raw(data as *mut WakerData));
}

/// Wake a task by scheduling an ExternalWake event
///
/// This function implements the core waking mechanism for async tasks.
/// It uses a two-phase approach:
///
/// 1. **Immediate wake**: If called during a polling cycle (when CURRENT_CONTEXT
///    is available), the task is added to pending_wakes for immediate processing
/// 2. **Scheduled wake**: If called outside a polling cycle (e.g., from a timer
///    callback), an ExternalWake event is scheduled to wake the task
///
/// This design ensures tasks can be woken both synchronously (during polling)
/// and asynchronously (from external events or timers).
fn wake_task_internal(task_id: TaskId, runtime_key: Option<Key<RuntimeEvent>>) {
    // First try to register with current context if we're in a polling cycle
    let registered = CURRENT_CONTEXT.with(|ctx| {
        if let Some(ptr) = *ctx.borrow() {
            unsafe {
                (*ptr).register_wake(task_id);
            }
            true
        } else {
            false
        }
    });
    
    if registered {
        return;
    }
    
    // If no current context, schedule an ExternalWake event immediately
    if let Some(runtime_key) = runtime_key {
        CURRENT_SCHEDULER.with(|sched| {
            if let Some(ptr) = *sched.borrow() {
                unsafe {
                    // Schedule the ExternalWake event immediately (at current time)
                    (*ptr).schedule_now(runtime_key, RuntimeEvent::ExternalWake { task_id });
                }
            }
        });
    }
}

/// Create a waker for a task
fn create_waker(task_id: TaskId, runtime_key: Option<Key<RuntimeEvent>>) -> Waker {
    let data = Box::new(WakerData::new(task_id, runtime_key));
    let raw_waker = RawWaker::new(Box::into_raw(data) as *const (), &WAKER_VTABLE);
    unsafe { Waker::from_raw(raw_waker) }
}

/// Events that drive the async runtime
///
/// The runtime operates by processing these events through the DES scheduler.
/// Each event type serves a specific purpose in the async execution model:
///
/// - `Poll`: Processes all ready tasks and schedules more polls if needed
/// - `ExternalWake`: Wakes a specific task (typically from timer expiration)
/// - `Spawn`: Adds a new task to the runtime (for cross-component spawning)
pub enum RuntimeEvent {
    /// Poll all ready tasks
    Poll,
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
/// The `DesRuntime` manages async tasks within a discrete event simulation.
/// It integrates with the DES scheduler to provide precise timing control
/// while maintaining the familiar async/await programming model.
///
/// # Key Features
///
/// - **Event-driven execution**: Tasks only run when they have work to do
/// - **Precise timing**: Sleep operations use exact simulation time
/// - **Multiple task support**: Can manage many concurrent async tasks
/// - **Integration**: Works seamlessly with other DES components
///
/// # Usage Pattern
///
/// 1. Create a runtime with `DesRuntime::new()`
/// 2. Spawn tasks using `runtime.spawn(async_function)`
/// 3. Add the runtime to your simulation as a component
/// 4. Schedule an initial `RuntimeEvent::Poll` to start execution
///
/// # Internal Architecture
///
/// The runtime maintains:
/// - A task registry mapping TaskId to Future instances
/// - A ready queue of tasks that need polling
/// - Waker storage for sleeping tasks
/// - Integration with the DES scheduler for timing events
pub struct DesRuntime {
    next_task_id: u64,
    tasks: HashMap<TaskId, Task>,
    ready_queue: VecDeque<TaskId>,
    /// Store wakers for tasks that are waiting (enables external waking)
    task_wakers: HashMap<TaskId, Waker>,
    /// Runtime's own component key (for scheduling ExternalWake events)
    runtime_key: Option<Key<RuntimeEvent>>,
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
            task_wakers: HashMap::new(),
            runtime_key: None,
        }
    }

    /// Set the runtime key for timer callbacks
    pub fn set_runtime_key(&mut self, key: Key<RuntimeEvent>) {
        self.runtime_key = Some(key);
    }

    /// Spawn a new async task
    ///
    /// The task will be added to the ready queue and polled during the next
    /// `RuntimeEvent::Poll` processing. The task runs until completion or
    /// until it awaits on a future that returns `Poll::Pending`.
    ///
    /// # Returns
    ///
    /// Returns a `TaskId` that uniquely identifies this task. This can be
    /// used for external wake operations or debugging.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut runtime = DesRuntime::new();
    /// let task_id = runtime.spawn(async {
    ///     sim_sleep(Duration::from_millis(100)).await;
    ///     println!("Task completed!");
    /// });
    /// ```
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
    fn poll_task(&mut self, task_id: TaskId, context: &mut AsyncContext, scheduler: &mut Scheduler) -> Option<Poll<()>> {
        let task = self.tasks.get_mut(&task_id)?;

        let waker = create_waker(task_id, self.runtime_key);
        let mut cx = Context::from_waker(&waker);

        // Store the waker for potential timer callbacks
        self.task_wakers.insert(task_id, waker.clone());

        // Set up thread-local context and poll
        let result = with_async_context(context, scheduler, task_id, self.runtime_key, || {
            task.future.as_mut().poll(&mut cx)
        });

        Some(result)
    }

    /// Poll all ready tasks
    ///
    /// This is the core execution loop of the runtime. It:
    /// 1. Drains the ready queue to get all tasks that need polling
    /// 2. Polls each task, providing it with a waker and async context
    /// 3. Removes completed tasks from the runtime
    /// 4. Processes any tasks that were woken during polling
    ///
    /// Tasks that return `Poll::Pending` remain in the runtime and will
    /// be woken later by timer events or external wake calls.
    fn poll_ready_tasks(&mut self, context: &mut AsyncContext, scheduler: &mut Scheduler) {
        // Process ready queue first
        let ready: Vec<TaskId> = self.ready_queue.drain(..).collect();

        for task_id in ready {
            if let Some(poll_result) = self.poll_task(task_id, context, scheduler) {
                match poll_result {
                    Poll::Ready(()) => {
                        // Task completed, remove it
                        self.tasks.remove(&task_id);
                        self.task_wakers.remove(&task_id);
                    }
                    Poll::Pending => {
                        // Task is waiting - timer should have been scheduled if needed
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

    /// Wake a specific task
    pub fn wake_task(&mut self, task_id: TaskId) {
        if self.tasks.contains_key(&task_id) {
            if let Some(waker) = self.task_wakers.get(&task_id) {
                waker.wake_by_ref();
            }
            if !self.ready_queue.contains(&task_id) {
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
        // Store runtime key for waker callbacks
        if self.runtime_key.is_none() {
            self.runtime_key = Some(self_id);
        }
        
        // Create async context for this polling cycle
        let mut context = AsyncContext::new(scheduler.time());

        match event {
            RuntimeEvent::Poll => {
                self.poll_ready_tasks(&mut context, scheduler);
            }
            RuntimeEvent::ExternalWake { task_id } => {
                self.wake_task(*task_id);
                self.poll_ready_tasks(&mut context, scheduler);
            }
            RuntimeEvent::Spawn { future: _ } => {
                // Clone the future - this is a bit awkward but necessary
                // In practice, users would call spawn() directly on the runtime
                // This event is for spawning from other components
                // For now, we'll just trigger a poll
                self.poll_ready_tasks(&mut context, scheduler);
            }
        }

        // Only schedule another poll if there are ready tasks
        // This prevents continuous polling - tasks are woken by ExternalWake events
        if !self.ready_queue.is_empty() {
            scheduler.schedule_now(self_id, RuntimeEvent::Poll);
        }
    }
}

/// A future that completes after a simulated delay
///
/// `SimSleep` is the core timing primitive for async simulation code. Unlike
/// `tokio::time::sleep` or `std::thread::sleep`, this uses simulation time
/// and integrates with the DES scheduler for precise, deterministic timing.
///
/// # How It Works
///
/// When first polled, `SimSleep` calculates the target wake time based on the
/// current simulation time. It then schedules an `ExternalWake` event at that
/// exact time. When the simulation reaches that time, the event fires and
/// wakes the sleeping task.
///
/// This approach ensures:
/// - **Deterministic timing**: Tasks always wake at the exact simulation time
/// - **No busy waiting**: The simulation doesn't waste cycles checking timers
/// - **Precise scheduling**: Multiple tasks can wake at the same simulation time
///
/// # Usage
///
/// Most users should use the convenience functions `sim_sleep()` and 
/// `sim_sleep_until()` rather than constructing `SimSleep` directly.
///
/// ```ignore
/// // Sleep for a duration
/// sim_sleep(Duration::from_millis(100)).await;
///
/// // Sleep until a specific time
/// sim_sleep_until(SimTime::from_secs(5)).await;
/// ```
pub struct SimSleep {
    duration: Duration,
    target_time: Option<SimTime>,
    timer_scheduled: bool,
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
            timer_scheduled: false,
        }
    }

    /// Create a SimSleep with a specific target time
    pub fn until(target_time: SimTime) -> Self {
        Self {
            duration: Duration::ZERO,
            target_time: Some(target_time),
            timer_scheduled: false,
        }
    }
}

impl Future for SimSleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        // Get current time from thread-local context
        let current_time = match current_sim_time() {
            Some(t) => t,
            None => return Poll::Pending, // No context available
        };

        // On first poll, calculate actual target time
        if self.target_time.is_none() {
            self.target_time = Some(current_time + SimTime::from_duration(self.duration));
        }
        
        let target = self.target_time.unwrap();

        // Check if we've reached the target time
        if current_time >= target {
            Poll::Ready(())
        } else if !self.timer_scheduled {
            // Schedule an ExternalWake event at the target time
            // This is the core timing mechanism: instead of polling repeatedly,
            // we schedule a precise wake event in the DES scheduler
            CURRENT_SCHEDULER.with(|sched| {
                CURRENT_RUNTIME_KEY.with(|runtime_key| {
                    if let (Some(sched_ptr), Some(runtime_key)) = (*sched.borrow(), *runtime_key.borrow()) {
                        unsafe {
                            let delay_duration = target - current_time;
                            let delay = SimTime::from_duration(delay_duration);
                            
                            // Get the task ID from current context
                            let task_id = current_task_id().unwrap_or(TaskId(0));
                            
                            // Schedule the ExternalWake event directly at the target time
                            (*sched_ptr).schedule(
                                delay,
                                runtime_key,
                                RuntimeEvent::ExternalWake { task_id }
                            );
                            
                            self.timer_scheduled = true;
                        }
                    }
                });
            });
            Poll::Pending
        } else {
            // Timer already scheduled, just wait for the wake event
            Poll::Pending
        }
    }
}

/// Sleep for a duration in simulation time
///
/// This is the primary way to introduce delays in async simulation code.
/// The task will be suspended and resumed after the specified duration
/// of simulation time has passed.
///
/// # Arguments
///
/// * `duration` - How long to sleep in simulation time
///
/// # Returns
///
/// A `SimSleep` future that completes after the duration has elapsed.
///
/// # Example
///
/// ```ignore
/// use des_core::async_runtime::sim_sleep;
/// use std::time::Duration;
///
/// async fn server_task() {
///     loop {
///         println!("Processing request...");
///         
///         // Simulate 50ms of processing time
///         sim_sleep(Duration::from_millis(50)).await;
///         
///         println!("Request completed");
///         
///         // Wait 100ms before next request
///         sim_sleep(Duration::from_millis(100)).await;
///     }
/// }
/// ```
///
/// # Timing Guarantees
///
/// - The task will wake at exactly `current_time + duration`
/// - Multiple tasks sleeping for the same duration will wake simultaneously
/// - Sleep duration is measured in simulation time, not wall-clock time
pub fn sim_sleep(duration: Duration) -> SimSleep {
    SimSleep::new(duration)
}

/// Sleep until a specific simulation time
///
/// This function allows a task to sleep until the simulation reaches a
/// specific absolute time. This is useful for scheduling events at exact
/// times or implementing periodic behavior.
///
/// # Arguments
///
/// * `target` - The simulation time to wake up at
///
/// # Returns
///
/// A `SimSleep` future that completes when simulation time reaches the target.
///
/// # Example
///
/// ```ignore
/// use des_core::async_runtime::sim_sleep_until;
/// use des_core::SimTime;
///
/// async fn scheduled_task() {
///     // Wake up at exactly 1 second of simulation time
///     sim_sleep_until(SimTime::from_secs(1)).await;
///     println!("It's now 1 second in the simulation");
///     
///     // Wake up at 5.5 seconds
///     sim_sleep_until(SimTime::from_millis(5500)).await;
///     println!("It's now 5.5 seconds in the simulation");
/// }
/// ```
///
/// # Behavior Notes
///
/// - If the target time has already passed, the future completes immediately
/// - Multiple tasks can wake at the same simulation time
/// - The target time is absolute, not relative to when the function is called
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
        assert!(ctx.pending_wakes.is_empty());

        ctx.register_wake(TaskId(2));

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
    fn test_external_wake_debug() {
        println!("\n=== Debug ExternalWake Processing ===");
        
        let mut sim = Simulation::default();
        let mut runtime = DesRuntime::new();

        let completed = Rc::new(RefCell::new(false));
        let completed_clone = completed.clone();

        // Simple task that sleeps once
        runtime.spawn(async move {
            println!("Task: Starting");
            sim_sleep(Duration::from_millis(50)).await;
            println!("Task: After sleep");
            *completed_clone.borrow_mut() = true;
        });

        let runtime_id = sim.add_component(runtime);
        sim.schedule(SimTime::zero(), runtime_id, RuntimeEvent::Poll);

        // Run with detailed logging
        let step_count = Rc::new(RefCell::new(0));
        let step_count_clone = step_count.clone();
        
        Executor::timed(SimTime::from_millis(100))
            .side_effect(move |simulation| {
                let mut count = step_count_clone.borrow_mut();
                *count += 1;
                println!("--- Step {}: Time = {} ---", *count, simulation.scheduler.time());
            })
            .execute(&mut sim);

        println!("Final completed state: {}", *completed.borrow());
        println!("Total steps: {}", step_count.borrow());
        println!("=== Debug Complete ===\n");
        
        assert!(*completed.borrow(), "Task should have completed");
    }

    #[test]
    fn test_async_with_logging() {
        let mut sim = Simulation::default();

        let events = Rc::new(RefCell::new(Vec::new()));
        let events_clone = events.clone();

        let mut runtime = DesRuntime::new();
        
        // Spawn multiple async tasks with different timing
        runtime.spawn(async move {
            events_clone.borrow_mut().push("Task1: Starting".to_string());
            sim_sleep(Duration::from_millis(50)).await;
            events_clone.borrow_mut().push("Task1: After 50ms sleep".to_string());
            sim_sleep(Duration::from_millis(100)).await;
            events_clone.borrow_mut().push("Task1: After 150ms total".to_string());
        });

        let events_clone2 = events.clone();
        runtime.spawn(async move {
            events_clone2.borrow_mut().push("Task2: Starting".to_string());
            sim_sleep(Duration::from_millis(75)).await;
            events_clone2.borrow_mut().push("Task2: After 75ms sleep".to_string());
            sim_sleep(Duration::from_millis(50)).await;
            events_clone2.borrow_mut().push("Task2: After 125ms total".to_string());
        });

        let events_clone3 = events.clone();
        runtime.spawn(async move {
            events_clone3.borrow_mut().push("Task3: Starting".to_string());
            sim_sleep(Duration::from_millis(200)).await;
            events_clone3.borrow_mut().push("Task3: After 200ms sleep".to_string());
        });

        let runtime_id = sim.add_component(runtime);

        // Schedule initial poll
        sim.schedule(SimTime::zero(), runtime_id, RuntimeEvent::Poll);

        // Run simulation with side effects for logging
        let step_count = Rc::new(RefCell::new(0));
        let step_count_clone = step_count.clone();
        let events_for_logging = events.clone();
        
        Executor::timed(SimTime::from_millis(250))
            .side_effect(move |simulation| {
                let mut count = step_count_clone.borrow_mut();
                *count += 1;
                println!("Step {}: Time = {:?}", *count, simulation.scheduler.time());
                
                // Print current events
                let current_events = events_for_logging.borrow();
                if !current_events.is_empty() {
                    println!("  Events so far: {}", current_events.len());
                    for (i, event) in current_events.iter().enumerate() {
                        println!("    {}: {}", i + 1, event);
                    }
                }
                
                // Check runtime status
                if let Some(_runtime) = simulation.components.components.get(&runtime_id.id) {
                    // We can't easily access the runtime internals here due to type erasure
                    // But we can see that events are being processed
                    println!("  Runtime is active");
                }
                
                println!();
            })
            .execute(&mut sim);

        // Verify all tasks completed
        let final_events = events.borrow();
        println!("Final events ({} total):", final_events.len());
        for (i, event) in final_events.iter().enumerate() {
            println!("  {}: {}", i + 1, event);
        }

        // Check that all expected events occurred
        assert!(final_events.iter().any(|e| e.contains("Task1: Starting")));
        assert!(final_events.iter().any(|e| e.contains("Task1: After 50ms sleep")));
        assert!(final_events.iter().any(|e| e.contains("Task1: After 150ms total")));
        
        assert!(final_events.iter().any(|e| e.contains("Task2: Starting")));
        assert!(final_events.iter().any(|e| e.contains("Task2: After 75ms sleep")));
        assert!(final_events.iter().any(|e| e.contains("Task2: After 125ms total")));
        
        assert!(final_events.iter().any(|e| e.contains("Task3: Starting")));
        assert!(final_events.iter().any(|e| e.contains("Task3: After 200ms sleep")));

        println!("Total simulation steps: {}", step_count.borrow());
    }

    #[test]
    fn test_async_client_server_simulation() {
        println!("\n=== Async Client-Server Simulation ===");
        
        let mut sim = Simulation::default();
        let mut runtime = DesRuntime::new();

        // Use separate RefCells to avoid borrow conflicts during side effects
        let requests = Rc::new(RefCell::new(Vec::new()));
        let responses = Rc::new(RefCell::new(Vec::new()));
        let activity_log = Rc::new(RefCell::new(Vec::new()));

        // Spawn server task
        let server_requests = requests.clone();
        let server_responses = responses.clone();
        let server_activity = activity_log.clone();
        runtime.spawn(async move {
            server_activity.borrow_mut().push("Server: Starting up".to_string());
            
            let mut processed = 0;
            loop {
                // Check for new requests every 10ms
                sim_sleep(Duration::from_millis(10)).await;
                
                // Try to get a request (avoid holding the borrow)
                let request = {
                    let mut reqs = server_requests.borrow_mut();
                    reqs.pop()
                };
                
                if let Some(req) = request {
                    server_activity.borrow_mut().push(format!("Server: Processing request: {req}"));
                    
                    // Simulate processing time
                    sim_sleep(Duration::from_millis(30)).await;
                    
                    let response = format!("Response to {req}");
                    server_responses.borrow_mut().push(response.clone());
                    server_activity.borrow_mut().push(format!("Server: Sent response: {response}"));
                    
                    processed += 1;
                    if processed >= 3 {
                        server_activity.borrow_mut().push("Server: Shutting down".to_string());
                        break;
                    }
                }
            }
        });

        // Spawn client task
        let client_requests = requests.clone();
        let client_responses = responses.clone();
        let client_activity = activity_log.clone();
        runtime.spawn(async move {
            client_activity.borrow_mut().push("Client: Starting up".to_string());
            
            for i in 1..=3 {
                // Send request
                let request = format!("Request {i}");
                client_requests.borrow_mut().push(request.clone());
                client_activity.borrow_mut().push(format!("Client: Sent {request}"));
                
                // Wait for response
                loop {
                    sim_sleep(Duration::from_millis(5)).await;
                    let responses_len = client_responses.borrow().len();
                    if responses_len >= i {
                        let response = client_responses.borrow()[i-1].clone();
                        client_activity.borrow_mut().push(format!("Client: Received response: {response}"));
                        break;
                    }
                }
                
                // Wait before next request
                sim_sleep(Duration::from_millis(40)).await;
            }
            
            client_activity.borrow_mut().push("Client: All requests completed".to_string());
        });

        let runtime_id = sim.add_component(runtime);
        sim.schedule(SimTime::zero(), runtime_id, RuntimeEvent::Poll);

        // Run with detailed logging - use separate counters to avoid borrow conflicts
        let step_count = Rc::new(RefCell::new(0));
        let step_count_clone = step_count.clone();
        
        Executor::timed(SimTime::from_millis(300))
            .side_effect(move |simulation| {
                let mut count = step_count_clone.borrow_mut();
                *count += 1;
                
                println!("--- Step {}: Time = {} ---", *count, simulation.scheduler.time());
                
                // Just print step info without accessing shared state during execution
                // This avoids borrow conflicts with async tasks
                println!("  Simulation step executing...");
            })
            .execute(&mut sim);

        // Print all log entries after simulation completes
        let final_log = activity_log.borrow();
        println!("\n=== Full Activity Log ===");
        for (i, entry) in final_log.iter().enumerate() {
            println!("  {}: {}", i + 1, entry);
        }

        // Verify communication worked
        let final_responses = responses.borrow();
        assert_eq!(final_responses.len(), 3);
        assert!(final_responses[0].contains("Response to Request 1"));
        assert!(final_responses[1].contains("Response to Request 2"));
        assert!(final_responses[2].contains("Response to Request 3"));
        
        // Verify all expected log entries exist
        let log_entries = final_log.clone();
        assert!(log_entries.iter().any(|e| e.contains("Server: Starting up")));
        assert!(log_entries.iter().any(|e| e.contains("Client: Starting up")));
        assert!(log_entries.iter().any(|e| e.contains("Server: Processing request: Request 1")));
        assert!(log_entries.iter().any(|e| e.contains("Client: All requests completed")));
        assert!(log_entries.iter().any(|e| e.contains("Server: Shutting down")));
        
        println!("Total simulation steps: {}", step_count.borrow());
        println!("=== Simulation Complete ===\n");
    }
}
