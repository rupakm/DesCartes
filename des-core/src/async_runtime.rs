//! DES-based async runtime.
//!
//! This module provides an async runtime that uses simulation time instead of real time,
//! enabling async/await syntax while maintaining discrete event simulation semantics.
//!
//! # Overview
//!
//! The async runtime allows you to write simulation code using familiar async/await patterns
//! while ensuring all timing operations use simulation time rather than wall-clock time.
//!
//! # Key Components
//!
//! - [`DesRuntime`]: The main runtime component that manages async tasks
//! - [`sim_sleep`]: Sleep for a duration in simulation time
//! - [`sim_sleep_until`]: Sleep until a specific simulation time
//!
//! # Basic Usage
//!
//! ```
//! use descartes_core::{Simulation, Execute, Executor, SimTime};
//! use descartes_core::async_runtime::{DesRuntime, RuntimeEvent, sim_sleep};
//! use std::time::Duration;
//!
//! // Create simulation and runtime
//! let mut sim = Simulation::default();
//! let mut runtime = DesRuntime::new();
//!
//! // Spawn an async task
//! runtime.spawn(async {
//!     sim_sleep(Duration::from_millis(100)).await;
//!     sim_sleep(Duration::from_millis(50)).await;
//! });
//!
//! // Add runtime to simulation and start it
//! let runtime_id = sim.add_component(runtime);
//! sim.schedule(SimTime::zero(), runtime_id, RuntimeEvent::Poll);
//!
//! // Run simulation
//! Executor::timed(SimTime::from_millis(200)).execute(&mut sim);
//! ```

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicU64, Ordering as AtomicOrdering},
    Arc, Mutex,
};
use std::task::{Context, Poll};
use std::time::Duration;
use tracing::{debug, instrument, trace, warn};

use crate::scheduler::Scheduler;
use crate::waker::create_des_waker;
use crate::{Component, Key, SimTime};

/// Unique identifier for async tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub u64);

struct SpawnRequestSend {
    task_id: TaskId,
    future: Pin<Box<dyn Future<Output = ()> + Send>>,
}

struct SpawnRequestLocal {
    task_id: TaskId,
    future: Pin<Box<dyn Future<Output = ()>>>,
}

/// Handle for enqueueing tasks into a runtime installed in a `Simulation`.
///
/// This is used by higher-level facades (e.g. `descartes_tokio`) to spawn tasks without
/// direct mutable access to the runtime component.
#[derive(Clone)]
pub struct DesRuntimeHandle {
    runtime_key: Key<RuntimeEvent>,
    next_task_id: Arc<AtomicU64>,
    spawn_queue: Arc<Mutex<VecDeque<SpawnRequestSend>>>,
}

#[derive(Clone)]
pub struct DesRuntimeLocalHandle {
    runtime_key: Key<RuntimeEvent>,
    next_task_id: Arc<AtomicU64>,
    spawn_queue: Arc<Mutex<VecDeque<SpawnRequestLocal>>>,
}

pub struct InstalledDesRuntime {
    pub runtime_key: Key<RuntimeEvent>,
    pub handle: DesRuntimeHandle,
    pub local_handle: DesRuntimeLocalHandle,
}

/// Install a `DesRuntime` into the simulation and return handles for spawning tasks.
///
/// This schedules an initial `RuntimeEvent::Poll` at `t=0`.
pub fn install(sim: &mut crate::Simulation, runtime: DesRuntime) -> InstalledDesRuntime {
    let next_task_id = runtime.next_task_id.clone();
    let spawn_queue = runtime.spawn_queue.clone();
    let spawn_queue_local = runtime.spawn_queue_local.clone();

    let runtime_key = sim.add_component(runtime);
    sim.schedule(crate::SimTime::zero(), runtime_key, RuntimeEvent::Poll);

    InstalledDesRuntime {
        runtime_key,
        handle: DesRuntimeHandle {
            runtime_key,
            next_task_id: next_task_id.clone(),
            spawn_queue,
        },
        local_handle: DesRuntimeLocalHandle {
            runtime_key,
            next_task_id,
            spawn_queue: spawn_queue_local,
        },
    }
}

impl DesRuntimeHandle {
    pub fn runtime_key(&self) -> Key<RuntimeEvent> {
        self.runtime_key
    }

    /// Enqueue a task to be spawned by the runtime.
    ///
    /// Returns the allocated `TaskId`.
    pub fn spawn<F>(&self, future: F) -> TaskId
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let task_id = TaskId(self.next_task_id.fetch_add(1, AtomicOrdering::Relaxed));

        self.spawn_queue
            .lock()
            .unwrap()
            .push_back(SpawnRequestSend {
                task_id,
                future: Box::pin(future),
            });

        // Ensure the runtime gets polled.
        //
        // If we are outside scheduler context (e.g., spawning tasks during setup
        // before the simulation starts), the initial poll scheduled by `install()`
        // will pick up the queued tasks.
        if crate::scheduler::in_scheduler_context() {
            crate::defer_wake(self.runtime_key, RuntimeEvent::Poll);
        }

        task_id
    }

    /// Cancel a task by ID.
    pub fn cancel(&self, task_id: TaskId) {
        // If the task hasn't been installed into the runtime yet (still in the spawn queue),
        // remove it immediately.
        let mut queue = self.spawn_queue.lock().unwrap();
        if !queue.is_empty() {
            let mut retained = VecDeque::with_capacity(queue.len());
            while let Some(req) = queue.pop_front() {
                if req.task_id != task_id {
                    retained.push_back(req);
                }
            }
            *queue = retained;
        }
        drop(queue);

        if crate::scheduler::in_scheduler_context() {
            crate::defer_wake(self.runtime_key, RuntimeEvent::Cancel { task_id });
        }
    }
}

impl DesRuntimeLocalHandle {
    pub fn runtime_key(&self) -> Key<RuntimeEvent> {
        self.runtime_key
    }

    pub fn spawn_local<F>(&self, future: F) -> TaskId
    where
        F: Future<Output = ()> + 'static,
    {
        let task_id = TaskId(self.next_task_id.fetch_add(1, AtomicOrdering::Relaxed));

        self.spawn_queue
            .lock()
            .unwrap()
            .push_back(SpawnRequestLocal {
                task_id,
                future: Box::pin(future),
            });

        if crate::scheduler::in_scheduler_context() {
            crate::defer_wake(self.runtime_key, RuntimeEvent::Poll);
        }
        task_id
    }

    pub fn cancel(&self, task_id: TaskId) {
        let mut queue = self.spawn_queue.lock().unwrap();
        if !queue.is_empty() {
            let mut retained = VecDeque::with_capacity(queue.len());
            while let Some(req) = queue.pop_front() {
                if req.task_id != task_id {
                    retained.push_back(req);
                }
            }
            *queue = retained;
        }
        drop(queue);

        if crate::scheduler::in_scheduler_context() {
            crate::defer_wake(self.runtime_key, RuntimeEvent::Cancel { task_id });
        }
    }
}

// Thread-local storage for the currently-polling runtime and task.
//
// Time access is provided by the scheduler's own context
// (`descartes_core::scheduler::current_time`) so we avoid
// duplicating TLS or storing raw scheduler pointers here.
thread_local! {
    static CURRENT_RUNTIME_KEY: RefCell<Option<Key<RuntimeEvent>>> = const { RefCell::new(None) };
    static CURRENT_TASK_ID: RefCell<Option<TaskId>> = const { RefCell::new(None) };
}

/// Get the current simulation time (for use in futures).
///
/// Returns `None` if called outside scheduler context.
pub fn current_sim_time() -> Option<SimTime> {
    crate::scheduler::current_time()
}

/// Returns the async runtime key for the currently polled task.
///
/// Returns `None` if called outside async runtime polling.
pub fn current_runtime_key() -> Option<Key<RuntimeEvent>> {
    CURRENT_RUNTIME_KEY.with(|k| *k.borrow())
}

/// Returns the ID of the currently polled async task.
///
/// Returns `None` if called outside async runtime polling.
pub fn current_task_id() -> Option<TaskId> {
    CURRENT_TASK_ID.with(|t| *t.borrow())
}

fn set_poll_context(runtime_key: Key<RuntimeEvent>, task_id: TaskId) {
    CURRENT_RUNTIME_KEY.with(|k| *k.borrow_mut() = Some(runtime_key));
    CURRENT_TASK_ID.with(|t| *t.borrow_mut() = Some(task_id));
}

fn clear_poll_context() {
    CURRENT_RUNTIME_KEY.with(|k| *k.borrow_mut() = None);
    CURRENT_TASK_ID.with(|t| *t.borrow_mut() = None);
}

/// Register a timer for the currently polled task.
///
/// This schedules a `RuntimeEvent::RegisterTimer` event to the runtime via
/// `defer_wake(...)` (no scheduler locking).
fn register_timer_at(deadline: SimTime) {
    CURRENT_RUNTIME_KEY.with(|key| {
        CURRENT_TASK_ID.with(|task| {
            if let (Some(runtime_key), Some(task_id)) = (*key.borrow(), *task.borrow()) {
                crate::defer_wake(
                    runtime_key,
                    RuntimeEvent::RegisterTimer { task_id, deadline },
                );
            }
        });
    });
}

/// Events that drive the async runtime.
#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    /// Poll all ready tasks.
    Poll,
    /// Wake a specific task.
    Wake { task_id: TaskId },
    /// Cancel a task.
    Cancel { task_id: TaskId },
    /// Register a timer for a task to be woken at `deadline`.
    RegisterTimer { task_id: TaskId, deadline: SimTime },
    /// Internal event fired when the next timer deadline is reached.
    TimerTick,
}

/// Policy for choosing which ready async task to poll next.
///
/// This is a separate scheduling layer from the DES event queue. It only affects
/// the order in which ready async tasks are polled within a `DesRuntime` poll
/// cycle.
///
/// The default policy is deterministic FIFO, preserving existing behavior.
pub trait ReadyTaskPolicy {
    fn choose(&mut self, time: SimTime, ready: &[TaskId]) -> usize;
}

impl<T: ReadyTaskPolicy + ?Sized> ReadyTaskPolicy for Box<T> {
    fn choose(&mut self, time: SimTime, ready: &[TaskId]) -> usize {
        (**self).choose(time, ready)
    }
}

/// Deterministic FIFO ready-task policy.
#[derive(Debug, Default, Clone, Copy)]
pub struct FifoReadyTaskPolicy;

impl ReadyTaskPolicy for FifoReadyTaskPolicy {
    fn choose(&mut self, _time: SimTime, _ready: &[TaskId]) -> usize {
        0
    }
}

/// Uniform random tie-breaker among ready async tasks.
///
/// Deterministic given the provided seed.
#[derive(Debug, Clone)]
pub struct UniformRandomReadyTaskPolicy {
    rng: StdRng,
}

impl UniformRandomReadyTaskPolicy {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
        }
    }
}

impl ReadyTaskPolicy for UniformRandomReadyTaskPolicy {
    fn choose(&mut self, _time: SimTime, ready: &[TaskId]) -> usize {
        if ready.is_empty() {
            return 0;
        }
        self.rng.gen_range(0..ready.len())
    }
}

/// Stable signature for a ready-task choice point.
///
/// This is intended for replay and (future) schedule-search tooling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyTaskSignature {
    pub time_nanos: u64,
    pub ready_task_ids: Vec<u64>,
}

impl ReadyTaskSignature {
    pub fn new(time: SimTime, ready: &[TaskId]) -> Self {
        let nanos: u128 = time.as_duration().as_nanos();
        let time_nanos = nanos.try_into().unwrap_or(u64::MAX);
        Self {
            time_nanos,
            ready_task_ids: ready.iter().map(|t| t.0).collect(),
        }
    }
}

/// A suspended async task.
struct Task {
    future: Pin<Box<dyn Future<Output = ()>>>,
    /// Whether the task is currently present in the ready queue.
    queued: bool,
}

/// DES-based async runtime component.
///
/// The `DesRuntime` manages async tasks within a discrete event simulation.
/// It integrates with the DES scheduler to provide precise timing control
/// while maintaining the familiar async/await programming model.
///
/// # Usage Pattern
///
/// 1. Create a runtime with `DesRuntime::new()`
/// 2. Spawn tasks using `runtime.spawn(async_function)`
/// 3. Add the runtime to your simulation as a component
/// 4. Schedule an initial `RuntimeEvent::Poll` to start execution
pub struct DesRuntime {
    next_task_id: Arc<AtomicU64>,
    tasks: HashMap<TaskId, Task>,
    ready_queue: VecDeque<TaskId>,

    ready_task_policy: Box<dyn ReadyTaskPolicy>,

    spawn_queue: Arc<Mutex<VecDeque<SpawnRequestSend>>>,
    spawn_queue_local: Arc<Mutex<VecDeque<SpawnRequestLocal>>>,

    // Timer management
    timers: BTreeMap<SimTime, Vec<TaskId>>,
    next_timer_deadline: Option<SimTime>,

    /// Runtime's own component key (set on first event).
    runtime_key: Option<Key<RuntimeEvent>>,
}

impl Default for DesRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(clippy::arc_with_non_send_sync)]
impl DesRuntime {
    /// Create a new async runtime.
    pub fn new() -> Self {
        Self {
            next_task_id: Arc::new(AtomicU64::new(0)),
            tasks: HashMap::new(),
            ready_queue: VecDeque::new(),

            ready_task_policy: Box::new(FifoReadyTaskPolicy),
            spawn_queue: Arc::new(Mutex::new(VecDeque::new())),
            spawn_queue_local: Arc::new(Mutex::new(VecDeque::new())),
            timers: BTreeMap::new(),
            next_timer_deadline: None,
            runtime_key: None,
        }
    }

    pub fn handle(&self, runtime_key: Key<RuntimeEvent>) -> DesRuntimeHandle {
        DesRuntimeHandle {
            runtime_key,
            next_task_id: self.next_task_id.clone(),
            spawn_queue: self.spawn_queue.clone(),
        }
    }

    pub fn local_handle(&self, runtime_key: Key<RuntimeEvent>) -> DesRuntimeLocalHandle {
        DesRuntimeLocalHandle {
            runtime_key,
            next_task_id: self.next_task_id.clone(),
            spawn_queue: self.spawn_queue_local.clone(),
        }
    }

    /// Set the ready-task polling policy.
    ///
    /// By default, the runtime uses deterministic FIFO ordering.
    pub fn set_ready_task_policy(&mut self, policy: Box<dyn ReadyTaskPolicy>) {
        self.ready_task_policy = policy;
    }

    /// Spawn a new async task.
    ///
    /// The task will be added to the ready queue and polled during the next
    /// `RuntimeEvent::Poll` processing.
    #[instrument(skip(self, future), fields(task_id))]
    pub fn spawn<F>(&mut self, future: F) -> TaskId
    where
        F: Future<Output = ()> + 'static,
    {
        let task_id = TaskId(self.next_task_id.fetch_add(1, AtomicOrdering::Relaxed));

        self.tasks.insert(
            task_id,
            Task {
                future: Box::pin(future),
                queued: true,
            },
        );
        self.ready_queue.push_back(task_id);

        tracing::Span::current().record("task_id", tracing::field::debug(&task_id));
        debug!(
            task_count = self.tasks.len(),
            ready_count = self.ready_queue.len(),
            "Spawned async task"
        );

        task_id
    }

    /// Get the number of active tasks.
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Check if there are any active tasks.
    pub fn has_tasks(&self) -> bool {
        !self.tasks.is_empty()
    }

    /// Poll a single task.
    #[instrument(skip(self, _scheduler), fields(task_id = ?task_id))]
    fn poll_task(&mut self, task_id: TaskId, _scheduler: &mut Scheduler) -> Option<Poll<()>> {
        let runtime_key = self.runtime_key?;
        let task = self.tasks.get_mut(&task_id)?;

        // Create waker using unified waker utility
        let waker = create_des_waker(runtime_key, RuntimeEvent::Wake { task_id });
        let mut cx = Context::from_waker(&waker);

        // Set poll context for SimSleep
        set_poll_context(runtime_key, task_id);

        trace!("Polling async task");
        let result = task.future.as_mut().poll(&mut cx);

        clear_poll_context();

        match result {
            Poll::Ready(()) => debug!("Async task completed"),
            Poll::Pending => trace!("Async task returned Pending"),
        }

        Some(result)
    }

    /// Poll all ready tasks.
    #[instrument(skip(self, scheduler), fields(
        ready_tasks = self.ready_queue.len(),
        total_tasks = self.tasks.len()
    ))]
    fn poll_ready_tasks(&mut self, scheduler: &mut Scheduler) {
        debug!("Polling ready tasks");

        // Drain any externally-enqueued spawn requests.
        while let Some(req) = self.spawn_queue.lock().unwrap().pop_front() {
            self.tasks.insert(
                req.task_id,
                Task {
                    // Coerce `dyn Future + Send` to `dyn Future`.
                    future: req.future,
                    queued: true,
                },
            );
            self.ready_queue.push_back(req.task_id);
        }
        while let Some(req) = self.spawn_queue_local.lock().unwrap().pop_front() {
            self.tasks.insert(
                req.task_id,
                Task {
                    future: req.future,
                    queued: true,
                },
            );
            self.ready_queue.push_back(req.task_id);
        }

        // Snapshot polling: only poll tasks that were ready at the start of this cycle.
        // Tasks woken while polling will be queued for the next cycle.
        let to_poll = self.ready_queue.len();
        let mut poll_set: Vec<TaskId> = Vec::with_capacity(to_poll);
        for _ in 0..to_poll {
            if let Some(task_id) = self.ready_queue.pop_front() {
                poll_set.push(task_id);
            }
        }

        let mut completed = 0;

        while !poll_set.is_empty() {
            let now = scheduler.time();
            let chosen = self.ready_task_policy.choose(now, &poll_set);
            let chosen = chosen.min(poll_set.len().saturating_sub(1));
            let task_id = poll_set.swap_remove(chosen);

            // Mark as no longer queued before polling so a wake during polling can re-queue it.
            if let Some(task) = self.tasks.get_mut(&task_id) {
                task.queued = false;
            } else {
                continue;
            }

            if let Some(Poll::Ready(())) = self.poll_task(task_id, scheduler) {
                self.tasks.remove(&task_id);
                completed += 1;
            }
        }

        debug!(
            completed,
            remaining = self.tasks.len(),
            "Poll cycle complete"
        );
    }

    /// Wake a specific task.
    pub fn wake_task(&mut self, task_id: TaskId) {
        let Some(task) = self.tasks.get_mut(&task_id) else {
            return;
        };

        if task.queued {
            return;
        }

        task.queued = true;
        self.ready_queue.push_back(task_id);
        trace!(?task_id, "Task added to ready queue");
    }

    fn schedule_next_timer_tick(&mut self, scheduler: &mut Scheduler, self_id: Key<RuntimeEvent>) {
        let now = scheduler.time();
        let next_deadline = self.timers.keys().next().copied();

        if next_deadline == self.next_timer_deadline {
            return;
        }

        self.next_timer_deadline = next_deadline;

        if let Some(deadline) = next_deadline {
            let delay = if deadline <= now {
                SimTime::zero()
            } else {
                SimTime::from_duration(deadline - now)
            };

            scheduler.schedule(delay, self_id, RuntimeEvent::TimerTick);
        }
    }

    fn register_timer(
        &mut self,
        task_id: TaskId,
        deadline: SimTime,
        scheduler: &mut Scheduler,
        self_id: Key<RuntimeEvent>,
    ) {
        if !self.tasks.contains_key(&task_id) {
            return;
        }

        self.timers.entry(deadline).or_default().push(task_id);
        self.schedule_next_timer_tick(scheduler, self_id);
    }

    fn handle_timer_tick(&mut self, scheduler: &mut Scheduler, self_id: Key<RuntimeEvent>) {
        let now = scheduler.time();

        // We are about to potentially schedule a new timer tick.
        self.next_timer_deadline = None;

        while let Some((&deadline, _)) = self.timers.iter().next() {
            if deadline > now {
                break;
            }

            let (_deadline, tasks) = self.timers.pop_first().expect("deadline exists");
            for task_id in tasks {
                self.wake_task(task_id);
            }
        }

        self.schedule_next_timer_tick(scheduler, self_id);
        self.poll_ready_tasks(scheduler);
    }

    fn cancel_task(
        &mut self,
        task_id: TaskId,
        scheduler: &mut Scheduler,
        self_id: Key<RuntimeEvent>,
    ) {
        self.tasks.remove(&task_id);

        // Remove from timer vectors.
        let mut empty_deadlines = Vec::new();
        for (deadline, tasks) in self.timers.iter_mut() {
            tasks.retain(|t| *t != task_id);
            if tasks.is_empty() {
                empty_deadlines.push(*deadline);
            }
        }
        for deadline in empty_deadlines {
            self.timers.remove(&deadline);
        }

        self.schedule_next_timer_tick(scheduler, self_id);
    }
}

impl Component for DesRuntime {
    type Event = RuntimeEvent;

    #[instrument(skip(self, scheduler), fields(
        event_type = ?event,
        current_time = ?scheduler.time(),
        task_count = self.tasks.len()
    ))]
    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        // Store runtime key on first event
        if self.runtime_key.is_none() {
            self.runtime_key = Some(self_id);
            debug!("Runtime key initialized");
        }

        match event {
            RuntimeEvent::Poll => {
                debug!("Processing Poll event");
                self.poll_ready_tasks(scheduler);
            }
            RuntimeEvent::Wake { task_id } => {
                debug!(?task_id, "Processing Wake event");
                self.wake_task(*task_id);
                self.poll_ready_tasks(scheduler);
            }
            RuntimeEvent::Cancel { task_id } => {
                debug!(?task_id, "Processing Cancel event");
                self.cancel_task(*task_id, scheduler, self_id);
            }
            RuntimeEvent::RegisterTimer { task_id, deadline } => {
                trace!(?task_id, ?deadline, "Processing RegisterTimer event");
                self.register_timer(*task_id, *deadline, scheduler, self_id);
            }
            RuntimeEvent::TimerTick => {
                trace!(current_time = ?scheduler.time(), "Processing TimerTick event");
                self.handle_timer_tick(scheduler, self_id);
            }
        }

        // Schedule another poll if there are ready tasks
        if !self.ready_queue.is_empty() {
            trace!("Scheduling next poll");
            scheduler.schedule_now(self_id, RuntimeEvent::Poll);
        }
    }
}

/// A future that completes after a simulated delay.
///
/// Use [`sim_sleep`] or [`sim_sleep_until`] to create instances.
pub struct SimSleep {
    target_time: Option<SimTime>,
    duration: Duration,
    timer_scheduled: bool,
}

impl SimSleep {
    /// Create a new SimSleep that will complete after the given duration.
    pub fn new(duration: Duration) -> Self {
        Self {
            target_time: None,
            duration,
            timer_scheduled: false,
        }
    }

    /// Create a SimSleep with a specific target time.
    pub fn until(target_time: SimTime) -> Self {
        Self {
            target_time: Some(target_time),
            duration: Duration::ZERO,
            timer_scheduled: false,
        }
    }
}

impl Future for SimSleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        let current_time = match current_sim_time() {
            Some(t) => t,
            None => return Poll::Pending,
        };

        // Calculate target time on first poll
        if self.target_time.is_none() {
            self.target_time = Some(current_time + SimTime::from_duration(self.duration));
        }

        let target = self.target_time.unwrap();

        if current_time >= target {
            Poll::Ready(())
        } else if !self.timer_scheduled {
            // Register a timer for the task. The runtime will wake this task
            // by emitting `RuntimeEvent::Wake { task_id }` when `target` is reached.
            register_timer_at(target);
            self.timer_scheduled = true;
            Poll::Pending
        } else {
            // Timer already scheduled, just wait
            Poll::Pending
        }
    }
}

/// Sleep for a duration in simulation time.
///
/// # Example
///
/// ```
/// use descartes_core::async_runtime::sim_sleep;
/// use std::time::Duration;
///
/// async fn server_task() {
///     sim_sleep(Duration::from_millis(50)).await;
/// }
/// ```
pub fn sim_sleep(duration: Duration) -> SimSleep {
    SimSleep::new(duration)
}

/// Sleep until a specific simulation time.
///
/// # Example
///
/// ```
/// use descartes_core::async_runtime::sim_sleep_until;
/// use descartes_core::SimTime;
///
/// async fn scheduled_task() {
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
        sim.schedule(SimTime::zero(), runtime_id, RuntimeEvent::Poll);

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
            wake_times_clone.borrow_mut().push(1);
            sim_sleep(Duration::from_millis(50)).await;
            wake_times_clone.borrow_mut().push(2);
            sim_sleep(Duration::from_millis(50)).await;
            wake_times_clone.borrow_mut().push(3);
        });

        let runtime_id = sim.add_component(runtime);
        sim.schedule(SimTime::zero(), runtime_id, RuntimeEvent::Poll);

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
        assert!(r.contains(&"t1-start"));
        assert!(r.contains(&"t2-start"));
        assert!(r.contains(&"t1-50ms"));
        assert!(r.contains(&"t2-100ms"));
        assert!(r.contains(&"t1-150ms"));
    }
}
