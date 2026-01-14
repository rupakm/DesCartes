//! Event scheduling and time management for discrete event simulation.
//!
//! This module provides the core scheduling infrastructure:
//!
//! - [`Scheduler`]: The internal event queue and time management (owned by [`Simulation`]).
//! - [`SchedulerHandle`]: A cloneable handle for scheduling events during simulation.
//! - [`ClockRef`]: A lightweight, lock-free reference for reading simulation time.
//!
//! # Scheduling Events
//!
//! Use [`SchedulerHandle`] to schedule events from Tower layers or other components:
//!
//! ```rust,ignore
//! let scheduler = simulation.scheduler_handle();
//!
//! // Schedule an event 100ms from now
//! scheduler.schedule(SimTime::from_millis(100), component_key, MyEvent::Tick);
//!
//! // Schedule immediately
//! scheduler.schedule_now(component_key, MyEvent::Poll);
//!
//! // Schedule a task
//! let handle = scheduler.timeout(SimTime::from_secs(5), |_| {
//!     println!("Timeout fired!");
//! });
//! ```
//!
//! # Reading Time
//!
//! For lock-free time reading, use [`ClockRef`]:
//!
//! ```rust,ignore
//! let clock = simulation.clock();
//! let current_time = clock.time();  // No lock required
//! ```
//!
//! # Deferred Wakes
//!
//! The [`defer_wake`] function allows wakers to schedule events during event processing
//! without deadlocking. This is used internally by async futures.
//!
//! [`Simulation`]: crate::Simulation

use std::any::Any;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fmt;
use std::sync::{
    atomic::{AtomicU64, Ordering as AtomicOrdering},
    Arc, Mutex,
};
use tracing::{debug, trace};
use uuid::Uuid;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::task::{ClosureTask, Task, TaskExecution, TaskHandle, TaskId, TaskWrapper, TimeoutTask};
use crate::types::EventId;
use crate::{Key, SimTime};

thread_local! {
    /// Clock reference available while processing an event.
    static CURRENT_CLOCK: RefCell<Option<ClockRef>> = const { RefCell::new(None) };

    /// Deferred events scheduled by wakers during event processing.
    ///
    /// These are drained (and scheduled at the current simulation time) after the
    /// current event finishes processing.
    static DEFERRED_WAKES: RefCell<Vec<DeferredWake>> = const { RefCell::new(Vec::new()) };
}

/// Defer an event to be scheduled at the current simulation time.
///
/// This is used by async wakers and other logic that needs to schedule work from
/// within event processing without re-locking the scheduler mutex.
///
/// Note: This function does not access the scheduler directly. Deferred wakes are
/// drained by `Simulation::step()` after the current event is processed.
pub fn defer_wake<E: fmt::Debug + 'static>(component: Key<E>, event: E) {
    DEFERRED_WAKES.with(|buf| {
        buf.borrow_mut().push(DeferredWake::new(component, event));
    });
}

/// Drain all deferred wakes into the scheduler.
///
/// This is called by `Simulation::step()` after processing each event.
pub(crate) fn drain_deferred_wakes(scheduler: &mut Scheduler) {
    DEFERRED_WAKES.with(|buf| {
        let mut wakes = buf.borrow_mut();
        for wake in wakes.drain(..) {
            wake.execute(scheduler);
        }
    });
}

/// Check if we're currently inside event processing (scheduler context is available).
pub fn in_scheduler_context() -> bool {
    CURRENT_CLOCK.with(|clock| clock.borrow().is_some())
}

/// Get the current simulation time from scheduler context.
pub fn current_time() -> Option<SimTime> {
    CURRENT_CLOCK.with(|clock| clock.borrow().as_ref().map(ClockRef::time))
}

/// Set scheduler context for the duration of a single event.
pub(crate) fn set_scheduler_context(scheduler: &Scheduler) {
    CURRENT_CLOCK.with(|clock| {
        *clock.borrow_mut() = Some(scheduler.clock());
    });
}

pub(crate) fn clear_scheduler_context() {
    CURRENT_CLOCK.with(|clock| {
        *clock.borrow_mut() = None;
    });
}

// ============================================================================
// Deferred Wake Storage
// ============================================================================

/// A type-erased deferred wake event.
struct DeferredWake {
    /// Closure that schedules the event when called with the scheduler
    schedule_fn: Box<dyn FnOnce(&mut Scheduler)>,
}

impl DeferredWake {
    fn new<E: fmt::Debug + 'static>(component: Key<E>, event: E) -> Self {
        Self {
            schedule_fn: Box::new(move |scheduler: &mut Scheduler| {
                scheduler.schedule_now(component, event);
            }),
        }
    }

    fn execute(self, scheduler: &mut Scheduler) {
        (self.schedule_fn)(scheduler);
    }
}

/// Policy for choosing among same-time frontier entries.
///
/// When multiple events are scheduled at the same simulation time, the scheduler
/// forms a frontier of enabled events. The policy decides which frontier entry
/// should run next.
///
/// The default policy is deterministic FIFO (by event sequence number) to
/// preserve existing behavior.
pub trait EventFrontierPolicy {
    fn choose(&mut self, time: SimTime, frontier: &[FrontierEvent]) -> usize;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontierEventKind {
    Component,
    Task,
}

/// Lightweight, stable descriptor for an event in the same-time frontier.
///
/// This intentionally does not expose event payloads. It is designed for
/// tie-breaking and traceability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrontierEvent {
    pub seq: u64,
    pub kind: FrontierEventKind,
    pub component_id: Option<Uuid>,
    pub task_id: Option<TaskId>,
}

/// Stable signature for a same-time frontier choice point.
///
/// This is intended for replay and (future) schedule-search tooling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontierSignature {
    pub time_nanos: u64,
    pub frontier_seqs: Vec<u64>,
}

impl FrontierSignature {
    pub fn new(time: SimTime, frontier: &[FrontierEvent]) -> Self {
        Self {
            time_nanos: simtime_to_nanos(time),
            frontier_seqs: frontier.iter().map(|e| e.seq).collect(),
        }
    }
}

/// Deterministic FIFO frontier policy (by `seq`).
#[derive(Debug, Default, Clone, Copy)]
pub struct FifoFrontierPolicy;

impl EventFrontierPolicy for FifoFrontierPolicy {
    fn choose(&mut self, _time: SimTime, frontier: &[FrontierEvent]) -> usize {
        frontier
            .iter()
            .enumerate()
            .min_by_key(|(_, e)| e.seq)
            .map(|(i, _)| i)
            .unwrap_or(0)
    }
}

/// Uniform random tie-breaker among frontier entries.
///
/// Deterministic given the provided seed.
#[derive(Debug, Clone)]
pub struct UniformRandomFrontierPolicy {
    rng: StdRng,
}

impl UniformRandomFrontierPolicy {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
        }
    }
}

impl EventFrontierPolicy for UniformRandomFrontierPolicy {
    fn choose(&mut self, _time: SimTime, frontier: &[FrontierEvent]) -> usize {
        if frontier.is_empty() {
            return 0;
        }
        self.rng.gen_range(0..frontier.len())
    }
}

/// Entry type stored in the scheduler's event queue.
///
/// Events are passed to [`Components`](crate::Components) for dispatch to the
/// appropriate component or task.
#[derive(Debug)]
pub enum EventEntry {
    Component(ComponentEventEntry),
    Task(TaskEventEntry),
}

impl EventEntry {
    #[allow(dead_code)]
    pub(crate) fn component_idx(&self) -> Option<Uuid> {
        match self {
            EventEntry::Component(entry) => Some(entry.component),
            EventEntry::Task(_) => None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn task_id(&self) -> Option<TaskId> {
        match self {
            EventEntry::Component(_) => None,
            EventEntry::Task(entry) => Some(entry.task_id),
        }
    }

    pub(crate) fn time(&self) -> SimTime {
        match self {
            EventEntry::Component(entry) => entry.time,
            EventEntry::Task(entry) => entry.time,
        }
    }

    pub(crate) fn seq(&self) -> u64 {
        match self {
            EventEntry::Component(entry) => entry.event_id.0,
            EventEntry::Task(entry) => entry.event_id.0,
        }
    }

    pub(crate) fn to_frontier_event(&self) -> FrontierEvent {
        match self {
            EventEntry::Component(entry) => FrontierEvent {
                seq: self.seq(),
                kind: FrontierEventKind::Component,
                component_id: Some(entry.component),
                task_id: None,
            },
            EventEntry::Task(entry) => FrontierEvent {
                seq: self.seq(),
                kind: FrontierEventKind::Task,
                component_id: None,
                task_id: Some(entry.task_id),
            },
        }
    }

    /// Tries to downcast the event entry to one holding an event of type `E`.
    /// If fails, returns `None`.
    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn downcast<E: fmt::Debug + 'static>(&self) -> Option<EventEntryTyped<'_, E>> {
        match self {
            EventEntry::Component(entry) => entry.downcast(),
            EventEntry::Task(_) => None,
        }
    }
}

#[derive(Debug)]
pub struct ComponentEventEntry {
    event_id: EventId,
    time: SimTime,
    pub(crate) component: Uuid,
    inner: Box<dyn Any>,
}

impl ComponentEventEntry {
    pub(crate) fn new<E: fmt::Debug + 'static>(
        id: EventId,
        time: SimTime,
        component: Key<E>,
        event: E,
    ) -> Self {
        ComponentEventEntry {
            event_id: id,
            time,
            component: component.id,
            inner: Box::new(event),
        }
    }

    /// Tries to downcast the event entry to one holding an event of type `E`.
    /// If fails, returns `None`.
    #[must_use]
    pub(crate) fn downcast<E: fmt::Debug + 'static>(&self) -> Option<EventEntryTyped<'_, E>> {
        self.inner.downcast_ref::<E>().map(|event| EventEntryTyped {
            id: self.event_id,
            time: self.time,
            component_key: Key::new_with_id(self.component),
            component_idx: self.component,
            event,
        })
    }
}

#[derive(Debug)]
pub struct TaskEventEntry {
    event_id: EventId,
    time: SimTime,
    pub(crate) task_id: TaskId,
}

impl TaskEventEntry {
    pub(crate) fn new(event_id: EventId, time: SimTime, task_id: TaskId) -> Self {
        TaskEventEntry {
            event_id,
            time,
            task_id,
        }
    }
}

impl PartialEq for EventEntry {
    fn eq(&self, other: &Self) -> bool {
        self.time() == other.time() && self.seq() == other.seq()
    }
}

impl Eq for EventEntry {}

impl PartialOrd for EventEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EventEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse the ordering for min-heap behavior in BinaryHeap
        other
            .time()
            .cmp(&self.time())
            .then_with(|| other.seq().cmp(&self.seq()))
    }
}

#[derive(Debug)]
pub struct EventEntryTyped<'e, E: fmt::Debug> {
    pub id: EventId,
    pub time: SimTime,
    pub component_key: Key<E>,
    pub component_idx: Uuid,
    pub event: &'e E,
}

type Clock = Arc<AtomicU64>;

/// Convert SimTime to/from atomic storage (nanoseconds as u64)
fn simtime_to_nanos(time: SimTime) -> u64 {
    time.as_duration().as_nanos() as u64
}

fn nanos_to_simtime(nanos: u64) -> SimTime {
    SimTime::from_duration(std::time::Duration::from_nanos(nanos))
}

/// A lightweight, lock-free reference for reading simulation time.
///
/// Obtain a `ClockRef` from [`Simulation::clock()`](crate::Simulation::clock) or
/// [`SchedulerHandle::clock()`]. Multiple `ClockRef` instances can read the time
/// concurrently without synchronization.
///
/// # Example
///
/// ```rust
/// # use des_core::Scheduler;
/// let scheduler = Scheduler::default();
/// let clock = scheduler.clock();
/// assert_eq!(clock.time(), scheduler.time());
/// ```
#[derive(Clone)]
pub struct ClockRef {
    clock: Clock,
}

impl From<Clock> for ClockRef {
    fn from(clock: Clock) -> Self {
        Self { clock }
    }
}

impl ClockRef {
    /// Return the current simulation time.
    #[must_use]
    pub fn time(&self) -> SimTime {
        nanos_to_simtime(self.clock.load(AtomicOrdering::Relaxed))
    }
}

// ============================================================================
// SchedulerHandle - Thread-safe handle for scheduling events
// ============================================================================

/// A cloneable handle for scheduling events during simulation.
///
/// `SchedulerHandle` provides a way to schedule events without direct access to
/// the [`Simulation`](crate::Simulation). This is the primary interface for Tower
/// service layers and other components that need to schedule events during
/// single-threaded simulation stepping.
///
/// # Example
///
/// ```rust,ignore
/// let mut simulation = Simulation::default();
/// let scheduler = simulation.scheduler_handle();
///
/// // Schedule events
/// scheduler.schedule(SimTime::from_millis(100), component_key, MyEvent::Tick);
/// scheduler.schedule_now(component_key, MyEvent::Poll);
///
/// // Schedule tasks
/// scheduler.timeout(SimTime::from_secs(5), |_| println!("Timeout!"));
/// ```
#[derive(Clone)]
pub struct SchedulerHandle {
    scheduler: Arc<Mutex<Scheduler>>,
}

impl SchedulerHandle {
    /// Create a new SchedulerHandle wrapping the given scheduler
    pub(crate) fn new(scheduler: Arc<Mutex<Scheduler>>) -> Self {
        Self { scheduler }
    }

    /// Schedule an event to be executed at `time` from now for `component`.
    pub fn schedule<E: fmt::Debug + 'static>(&self, time: SimTime, component: Key<E>, event: E) {
        let mut scheduler = self.scheduler.lock().unwrap();
        scheduler.schedule(time, component, event);
    }

    /// Schedule an event to be executed immediately for `component`.
    pub fn schedule_now<E: fmt::Debug + 'static>(&self, component: Key<E>, event: E) {
        self.schedule(SimTime::zero(), component, event);
    }

    /// Returns the current simulation time.
    #[must_use]
    pub fn time(&self) -> SimTime {
        let scheduler = self.scheduler.lock().unwrap();
        scheduler.time()
    }

    /// Returns a ClockRef for reading the simulation time without locking.
    #[must_use]
    pub fn clock(&self) -> ClockRef {
        let scheduler = self.scheduler.lock().unwrap();
        scheduler.clock()
    }

    /// Get a weak reference to the underlying scheduler.
    /// This is useful for components that need to check if the simulation is still alive.
    pub fn downgrade(&self) -> std::sync::Weak<Mutex<Scheduler>> {
        Arc::downgrade(&self.scheduler)
    }

    /// Schedule a task to run at a specific time
    pub fn schedule_task<T: Task>(&self, delay: SimTime, task: T) -> TaskHandle<T::Output> {
        let mut scheduler = self.scheduler.lock().unwrap();
        scheduler.schedule_task(delay, task)
    }

    /// Schedule a timeout callback
    pub fn timeout<F>(&self, delay: SimTime, callback: F) -> TaskHandle<()>
    where
        F: FnOnce(&mut Scheduler) + 'static,
    {
        let mut scheduler = self.scheduler.lock().unwrap();
        scheduler.timeout(delay, callback)
    }
}

/// The internal event scheduler (owned by [`Simulation`](crate::Simulation)).
///
/// Most users should interact with the scheduler through [`SchedulerHandle`] or
/// the methods on [`Simulation`](crate::Simulation) rather than directly.
pub struct Scheduler {
    next_event_id: u64,
    id_seed: u64,
    next_task_id: u64,
    events: BinaryHeap<EventEntry>,
    clock: Clock,
    // Task management
    pending_tasks: HashMap<TaskId, Box<dyn TaskExecution>>,
    completed_task_results: HashMap<TaskId, Box<dyn Any>>,
    cancelled_tasks: HashSet<TaskId>, // Track cancelled task IDs to prevent time advancement
    executing_task_id: Option<TaskId>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::with_seed(0)
    }
}

impl Scheduler {
    /// Create a scheduler with a deterministic ID seed.
    pub fn with_seed(id_seed: u64) -> Self {
        Self {
            next_event_id: 0,
            id_seed,
            next_task_id: 0,
            events: BinaryHeap::default(),
            clock: Arc::new(AtomicU64::new(0)),
            pending_tasks: HashMap::new(),
            completed_task_results: HashMap::new(),
            cancelled_tasks: HashSet::new(),
            executing_task_id: None,
        }
    }
}

impl Scheduler {
    /// Schedules `event` to be executed for `component` at `self.time() + time`.
    pub fn schedule<E: fmt::Debug + 'static>(
        &mut self,
        time: SimTime,
        component: Key<E>,
        event: E,
    ) {
        self.next_event_id += 1;
        let absolute_time = self.time() + time;
        let event_id = EventId(self.next_event_id);

        // Log event scheduling
        trace!(
            event_id = ?event_id,
            event_type = std::any::type_name::<E>(),
            scheduled_time = ?absolute_time,
            current_time = ?self.time(),
            component_id = ?component.id(),
            "Event scheduled"
        );

        let component_entry = ComponentEventEntry::new(event_id, absolute_time, component, event);
        self.events.push(EventEntry::Component(component_entry));

        // Log scheduler state periodically
        if self.next_event_id % 1000 == 0 {
            debug!(
                current_time = ?self.time(),
                pending_events = self.events.len(),
                total_events_scheduled = self.next_event_id,
                "Scheduler state update"
            );
        }
    }

    /// Schedules `event` to be executed for `component` at `self.time()`.
    pub fn schedule_now<E: fmt::Debug + 'static>(&mut self, component: Key<E>, event: E) {
        self.schedule(SimTime::zero(), component, event);
    }

    /// Returns the current simulation time.
    #[must_use]
    pub fn time(&self) -> SimTime {
        nanos_to_simtime(self.clock.load(AtomicOrdering::Relaxed))
    }

    /// Returns a structure with immutable access to the simulation time.
    #[must_use]
    pub fn clock(&self) -> ClockRef {
        ClockRef {
            clock: Arc::clone(&self.clock),
        }
    }

    /// Returns a reference to the next scheduled event or `None` if none are left.
    pub fn peek(&mut self) -> Option<&EventEntry> {
        self.events.peek()
    }

    /// Removes and returns the next scheduled event or `None` if none are left.
    /// Skips cancelled task events to prevent time advancement for cancelled tasks.
    pub fn pop(&mut self) -> Option<EventEntry> {
        while let Some(event) = self.events.pop() {
            if self.is_cancelled_task_event(&event) {
                continue;
            }

            // Not cancelled - advance time and return this event
            self.clock
                .store(simtime_to_nanos(event.time()), AtomicOrdering::Relaxed);
            return Some(event);
        }

        None
    }

    /// Removes and returns the next scheduled event using a frontier policy.
    ///
    /// When multiple events are scheduled at the same simulation time, this method
    /// forms a frontier of enabled events and delegates tie-breaking to `policy`.
    /// Cancelled task events are skipped.
    pub fn pop_with_policy(&mut self, policy: &mut dyn EventFrontierPolicy) -> Option<EventEntry> {
        let first = loop {
            let event = self.events.pop()?;
            if self.is_cancelled_task_event(&event) {
                continue;
            }
            break event;
        };

        let frontier_time = first.time();
        let mut frontier_entries: Vec<EventEntry> = vec![first];

        while let Some(peek) = self.events.peek() {
            if peek.time() != frontier_time {
                break;
            }

            let event = self.events.pop().expect("peeked value exists");
            if self.is_cancelled_task_event(&event) {
                continue;
            }
            frontier_entries.push(event);
        }

        let chosen_index = if frontier_entries.len() <= 1 {
            0
        } else {
            let frontier_desc: Vec<FrontierEvent> = frontier_entries
                .iter()
                .map(EventEntry::to_frontier_event)
                .collect();
            policy.choose(frontier_time, &frontier_desc)
        };

        let chosen_index = chosen_index.min(frontier_entries.len().saturating_sub(1));
        let chosen = frontier_entries.swap_remove(chosen_index);

        for remaining in frontier_entries {
            self.events.push(remaining);
        }

        self.clock
            .store(simtime_to_nanos(frontier_time), AtomicOrdering::Relaxed);

        Some(chosen)
    }

    fn is_cancelled_task_event(&mut self, event: &EventEntry) -> bool {
        if let EventEntry::Task(task_entry) = event {
            if self.cancelled_tasks.remove(&task_entry.task_id) {
                return true;
            }
        }
        false
    }

    // Task scheduling methods

    /// Schedule a task to run at a specific time
    pub fn schedule_task<T: Task>(&mut self, delay: SimTime, task: T) -> TaskHandle<T::Output> {
        self.next_task_id += 1;
        let task_uuid = crate::ids::deterministic_uuid(
            self.id_seed,
            crate::ids::UUID_DOMAIN_TASK,
            self.next_task_id,
        );
        let task_id = TaskId(task_uuid);

        let wrapper = TaskWrapper::new(task);
        let time = self.time() + delay;

        self.schedule_task_at(time, task_id, Box::new(wrapper));
        TaskHandle::new(task_id)
    }

    /// Schedule a task to run at a specific absolute time
    pub(crate) fn schedule_task_at(
        &mut self,
        time: SimTime,
        task_id: TaskId,
        task: Box<dyn TaskExecution>,
    ) {
        self.pending_tasks.insert(task_id, task);

        // Create a special event entry for task execution
        self.next_event_id += 1;
        let event = TaskEventEntry::new(EventId(self.next_event_id), time, task_id);
        self.events.push(EventEntry::Task(event));
    }

    /// Schedule a closure as a task
    pub fn schedule_closure<F, R>(&mut self, delay: SimTime, closure: F) -> TaskHandle<R>
    where
        F: FnOnce(&mut Scheduler) -> R + 'static,
        R: 'static,
    {
        let task = ClosureTask::new(closure);
        self.schedule_task(delay, task)
    }

    /// Schedule a timeout callback
    pub fn timeout<F>(&mut self, delay: SimTime, callback: F) -> TaskHandle<()>
    where
        F: FnOnce(&mut Scheduler) + 'static,
    {
        let task = TimeoutTask::new(callback);
        self.schedule_task(delay, task)
    }

    /// Return the currently executing task ID, if any.
    ///
    /// This is primarily intended for task implementations that need to reschedule
    /// themselves under the same ID (e.g. periodic or retry tasks).
    pub(crate) fn executing_task_id(&self) -> Option<TaskId> {
        self.executing_task_id
    }

    /// Cancel a scheduled task
    pub fn cancel_task<T>(&mut self, handle: TaskHandle<T>) -> bool {
        let task_id = handle.id();
        let was_pending = self.pending_tasks.remove(&task_id).is_some();

        // Mark as cancelled so the event gets skipped (preventing time advancement)
        if was_pending {
            self.cancelled_tasks.insert(task_id);
        }

        was_pending
    }

    /// Execute a task if it's ready
    pub(crate) fn execute_task(&mut self, task_id: TaskId) -> bool {
        // Safety check: should not be called for cancelled tasks since pop() skips them
        if self.cancelled_tasks.contains(&task_id) {
            self.cancelled_tasks.remove(&task_id);
            return false;
        }

        let Some(task) = self.pending_tasks.remove(&task_id) else {
            return false;
        };

        // Set the current executing task ID so tasks can reschedule themselves
        // under the same ID (e.g. RetryTask / PeriodicTask).
        let previous_executing_task_id = self.executing_task_id;
        self.executing_task_id = Some(task_id);

        let result = task.execute(self);

        self.executing_task_id = previous_executing_task_id;

        // If the task re-scheduled itself under the same ID, treat it as still pending
        // and do not store an intermediate result.
        if self.pending_tasks.contains_key(&task_id) {
            return true;
        }

        // Don't store results for () tasks since they're typically fire-and-forget
        // (timeouts, periodic callbacks, retry delays, etc.) and never retrieved.
        if result.downcast_ref::<()>().is_none() {
            self.completed_task_results.insert(task_id, result);
        }

        true
    }

    /// Get the result of a completed task
    pub fn get_task_result<T: 'static>(&mut self, handle: TaskHandle<T>) -> Option<T> {
        self.completed_task_results
            .remove(&handle.id())
            .and_then(|boxed| boxed.downcast::<T>().ok())
            .map(|boxed| *boxed)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_clock_ref() {
        let time = SimTime::from_duration(Duration::from_secs(1));
        let clock: Clock = Arc::new(AtomicU64::new(simtime_to_nanos(time)));
        let clock_ref = ClockRef::from(clock);
        assert_eq!(clock_ref.time(), time);
    }

    #[test]
    fn test_event_entry_downcast() {
        let component_entry = ComponentEventEntry {
            event_id: EventId(0),
            time: SimTime::from_duration(Duration::from_secs(1)),
            component: Uuid::from_u128(1),
            inner: Box::new(String::from("inner")),
        };
        let entry = EventEntry::Component(component_entry);
        assert!(entry.downcast::<String>().is_some());
        assert!(entry.downcast::<i32>().is_none());
    }

    #[test]
    fn test_event_entry_cmp() {
        let make_component_entry = || ComponentEventEntry {
            event_id: EventId(0),
            time: SimTime::from_duration(Duration::from_secs(1)),
            component: Uuid::from_u128(2),
            inner: Box::new(String::from("inner")),
        };

        // Same time and seq should be equal
        let entry1 = EventEntry::Component(ComponentEventEntry {
            event_id: EventId(1),
            time: SimTime::from_duration(Duration::from_secs(1)),
            ..make_component_entry()
        });
        let entry2 = EventEntry::Component(ComponentEventEntry {
            event_id: EventId(1),
            time: SimTime::from_duration(Duration::from_secs(1)),
            ..make_component_entry()
        });
        assert_eq!(entry1, entry2);

        // Same time, different seq: seq decides ordering
        let entry3 = EventEntry::Component(ComponentEventEntry {
            event_id: EventId(2),
            time: SimTime::from_duration(Duration::from_secs(1)),
            ..make_component_entry()
        });
        assert_eq!(entry1.cmp(&entry3), Ordering::Greater); // seq 1 < seq 2, so entry1 (seq 1) comes first

        // Different times: time decides
        let entry4 = EventEntry::Component(ComponentEventEntry {
            event_id: EventId(0),
            time: SimTime::from_duration(Duration::from_secs(0)),
            ..make_component_entry()
        });
        let entry5 = EventEntry::Component(ComponentEventEntry {
            event_id: EventId(1),
            time: SimTime::from_duration(Duration::from_secs(1)),
            ..make_component_entry()
        });
        assert_eq!(entry4.cmp(&entry5), Ordering::Greater); // time 0 < time 1, so entry4 comes first

        let entry6 = EventEntry::Component(ComponentEventEntry {
            event_id: EventId(1),
            time: SimTime::from_duration(Duration::from_secs(2)),
            ..make_component_entry()
        });
        let entry7 = EventEntry::Component(ComponentEventEntry {
            event_id: EventId(3),
            time: SimTime::from_duration(Duration::from_secs(1)),
            ..make_component_entry()
        });
        assert_eq!(entry6.cmp(&entry7), Ordering::Less); // time 2 > time 1, so entry7 comes first
    }

    #[derive(Debug, Clone, Eq, PartialEq)]
    struct EventA;
    #[derive(Debug, Clone, Eq, PartialEq)]
    struct EventB;

    #[test]
    fn test_scheduler() {
        let mut scheduler = Scheduler::default();
        assert_eq!(
            scheduler.time(),
            SimTime::from_duration(Duration::new(0, 0))
        );
        assert_eq!(
            scheduler.clock().time(),
            SimTime::from_duration(Duration::new(0, 0))
        );
        assert!(scheduler.events.is_empty());

        let component_a = Key::<EventA>::new_with_id(Uuid::from_u128(3));
        let component_b = Key::<EventB>::new_with_id(Uuid::from_u128(4));

        scheduler.schedule(
            SimTime::from_duration(Duration::from_secs(1)),
            component_a,
            EventA,
        );
        scheduler.schedule_now(component_b, EventB);
        scheduler.schedule(
            SimTime::from_duration(Duration::from_secs(2)),
            component_b,
            EventB,
        );

        assert_eq!(
            scheduler.time(),
            SimTime::from_duration(Duration::from_secs(0))
        );

        let entry = scheduler.pop().unwrap();
        let entry = entry.downcast::<EventB>().unwrap();
        assert_eq!(entry.time, SimTime::from_duration(Duration::from_secs(0)));
        assert_eq!(entry.component_idx, component_b.id);
        assert_eq!(entry.component_key.id, component_b.id);
        assert_eq!(entry.event, &EventB);

        assert_eq!(
            scheduler.time(),
            SimTime::from_duration(Duration::from_secs(0))
        );

        let entry = scheduler.pop().unwrap();
        let entry = entry.downcast::<EventA>().unwrap();
        assert_eq!(entry.time, SimTime::from_duration(Duration::from_secs(1)));
        assert_eq!(entry.component_idx, component_a.id);
        assert_eq!(entry.component_key.id, component_a.id);
        assert_eq!(entry.event, &EventA);

        assert_eq!(
            scheduler.time(),
            SimTime::from_duration(Duration::from_secs(1))
        );
        assert_eq!(
            scheduler.clock().time(),
            SimTime::from_duration(Duration::from_secs(1))
        );

        let entry = scheduler.pop().unwrap();
        let entry = entry.downcast::<EventB>().unwrap();
        assert_eq!(entry.time, SimTime::from_duration(Duration::from_secs(2)));
        assert_eq!(entry.component_idx, component_b.id);
        assert_eq!(entry.component_key.id, component_b.id);
        assert_eq!(entry.event, &EventB);

        assert_eq!(
            scheduler.time(),
            SimTime::from_duration(Duration::from_secs(2))
        );

        assert!(scheduler.pop().is_none());
    }

    #[test]
    fn test_unit_task_results_not_stored() {
        let mut scheduler = Scheduler::default();

        // Schedule a task that returns ()
        let task_id1 = TaskId::new();
        let unit_task = crate::task::TimeoutTask::new(|_| {});
        scheduler.schedule_task_at(
            SimTime::from_secs(1),
            task_id1,
            Box::new(crate::task::TaskWrapper::new(unit_task)),
        );

        // Schedule a task that returns a value
        let task_id2 = TaskId::new();
        let value_task = crate::task::ClosureTask::new(|_| 42);
        scheduler.schedule_task_at(
            SimTime::from_secs(2),
            task_id2,
            Box::new(crate::task::TaskWrapper::new(value_task)),
        );

        // Execute both tasks
        assert!(scheduler.execute_task(task_id1)); // () result - should not be stored
        assert!(scheduler.execute_task(task_id2)); // i32 result - should be stored

        // Check that only the value task result is stored
        assert!(scheduler.completed_task_results.contains_key(&task_id2));
        assert!(!scheduler.completed_task_results.contains_key(&task_id1));

        // Verify we can retrieve the value result
        let handle = crate::task::TaskHandle::<i32>::new(task_id2);
        let result = scheduler.get_task_result(handle);
        assert_eq!(result, Some(42));
    }

    #[test]
    fn test_cancelled_task_does_not_advance_time() {
        let mut scheduler = Scheduler::default();

        // Schedule a task at t=10
        let task_id = TaskId::new();
        let task = crate::task::ClosureTask::new(|_| 42);
        scheduler.schedule_task_at(
            SimTime::from_secs(10),
            task_id,
            Box::new(crate::task::TaskWrapper::new(task)),
        );

        // Cancel the task before it executes
        let handle = crate::task::TaskHandle::<i32>::new(task_id);
        let cancelled = scheduler.cancel_task(handle);
        assert!(cancelled);

        // Verify time is still 0
        assert_eq!(scheduler.time(), SimTime::zero());

        // Pop should skip the cancelled task and return None (no more events)
        let popped = scheduler.pop();
        assert!(popped.is_none());

        // Time should still be 0 (not advanced to 10)
        assert_eq!(scheduler.time(), SimTime::zero());
    }

    #[test]
    fn test_stable_same_time_event_ordering() {
        // Test that same-time events are processed in a stable, deterministic order
        // across multiple runs. This covers component events and task events.
        use crate::{Component, Execute, Executor, Key, SimTime, Simulation};
        use std::sync::{Arc, Mutex};

        #[derive(Debug, Clone)]
        enum TestEvent {
            ComponentEvent(usize),
        }

        #[derive(Debug)]
        struct OrderingTestComponent {
            log: Arc<Mutex<Vec<String>>>,
        }

        impl Component for OrderingTestComponent {
            type Event = TestEvent;

            fn process_event(
                &mut self,
                _self_id: Key<Self::Event>,
                event: &Self::Event,
                scheduler: &mut crate::Scheduler,
            ) {
                match *event {
                    TestEvent::ComponentEvent(id) => {
                        self.log.lock().unwrap().push(format!("component-{}", id));

                        // Schedule a task at the same time
                        let log = self.log.clone();
                        scheduler.schedule_closure(SimTime::zero(), move |_scheduler| {
                            log.lock().unwrap().push("task".to_string());
                        });
                    }
                }
            }
        }

        let log = Arc::new(Mutex::new(Vec::new()));
        let component = OrderingTestComponent { log: log.clone() };
        let mut sim = Simulation::default();
        let key = sim.add_component(component);

        // Schedule 3 component events at the same time (t=0)
        for i in 0..3 {
            sim.schedule(SimTime::zero(), key, TestEvent::ComponentEvent(i));
        }

        // Run for a short time
        Executor::timed(SimTime::from_millis(1)).execute(&mut sim);

        let result = log.lock().unwrap().clone();
        assert_eq!(result.len(), 6); // 3 components + 3 tasks

        // Run the same scenario multiple times and verify identical ordering
        for _ in 0..10 {
            let log2 = Arc::new(Mutex::new(Vec::new()));
            let component2 = OrderingTestComponent { log: log2.clone() };
            let mut sim2 = Simulation::default();
            let key2 = sim2.add_component(component2);

            for i in 0..3 {
                sim2.schedule(SimTime::zero(), key2, TestEvent::ComponentEvent(i));
            }

            Executor::timed(SimTime::from_millis(1)).execute(&mut sim2);
            let result2 = log2.lock().unwrap().clone();
            assert_eq!(result, result2);
        }
    }
}
