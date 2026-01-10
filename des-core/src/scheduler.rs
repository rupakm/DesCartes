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

use crate::task::{ClosureTask, Task, TaskExecution, TaskHandle, TaskId, TaskWrapper, TimeoutTask};
use crate::types::EventId;
use crate::{Key, SimTime};

thread_local! {
    /// Thread-local pointer to the current scheduler during event processing.
    static CURRENT_SCHEDULER: RefCell<Option<*mut Scheduler>> = const { RefCell::new(None) };

    /// Buffer for wakes that occur outside of scheduler context.
    /// These get drained when scheduler context becomes available.
    static PENDING_WAKES: RefCell<Vec<DeferredWake>> = const { RefCell::new(Vec::new()) };
}

/// Defer an event to be scheduled at the current simulation time.
///
/// This function is designed to be called from wakers during single-threaded
/// event processing. It uses thread-local storage to access the scheduler.
///
/// # Panics
///
/// Panics in debug mode if called outside of event processing (scheduler context).
/// In release mode, buffers the wake to be processed when scheduler context becomes available.
pub fn defer_wake<E: fmt::Debug + 'static>(component: Key<E>, event: E) {
    CURRENT_SCHEDULER.with(|sched| {
        if let Some(ptr) = *sched.borrow() {
            // Safety: The pointer is valid during single-threaded event processing
            let scheduler = unsafe { &mut *ptr };
            scheduler.add_deferred_wake(component, event);
        } else {
            // Out of scheduler context - this can happen in edge cases (future_poller)

            eprintln!(
                "Warning: defer_wake called outside of scheduler context. \
                 Wakers should only be used during event processing in single-threaded simulation. \
                 Event: {:?}",
                event
            );

            {
                // Buffer the wake for later processing
                PENDING_WAKES.with(|pending| {
                    pending
                        .borrow_mut()
                        .push(DeferredWake::new(component, event));
                });
            }
        }
    });
}

/// Drain any pending wakes that were buffered while out of scheduler context.
/// This is called when scheduler context becomes available.
fn drain_pending_wakes(scheduler: &mut Scheduler) {
    PENDING_WAKES.with(|pending| {
        let mut wakes = pending.borrow_mut();
        for wake in wakes.drain(..) {
            wake.execute(scheduler);
        }
    });
}

/// Check if we're currently inside event processing (scheduler context is available)
pub fn in_scheduler_context() -> bool {
    CURRENT_SCHEDULER.with(|sched| sched.borrow().is_some())
}

/// Get the current simulation time from the scheduler context.
/// Returns None if not in scheduler context.
pub fn current_time() -> Option<SimTime> {
    CURRENT_SCHEDULER.with(|sched| {
        sched.borrow().map(|ptr| {
            let scheduler = unsafe { &*ptr };
            scheduler.time()
        })
    })
}

/// Execute a closure with access to the scheduler via thread-local storage.
/// Used internally by the simulation step to enable deferred wakes.
pub(crate) fn set_scheduler_context(scheduler: &mut Scheduler) {
    CURRENT_SCHEDULER.with(|sched| {
        *sched.borrow_mut() = Some(scheduler as *mut Scheduler);
    });

    // Drain any wakes that were buffered while out of context
    drain_pending_wakes(scheduler);
}

pub(crate) fn clear_scheduler_context() {
    CURRENT_SCHEDULER.with(|sched| {
        *sched.borrow_mut() = None;
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
    events: BinaryHeap<EventEntry>,
    clock: Clock,
    // Task management
    pending_tasks: HashMap<TaskId, Box<dyn TaskExecution>>,
    completed_task_results: HashMap<TaskId, Box<dyn Any>>,
    cancelled_tasks: HashSet<TaskId>, // Track cancelled task IDs to prevent time advancement
    // Deferred wakes from wakers during event processing
    deferred_wakes: Vec<DeferredWake>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self {
            next_event_id: 0,
            events: BinaryHeap::default(),
            clock: Arc::new(AtomicU64::new(0)),
            pending_tasks: HashMap::new(),
            completed_task_results: HashMap::new(),
            cancelled_tasks: HashSet::new(),
            deferred_wakes: Vec::new(),
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

    /// Create a SchedulerHandle for this scheduler
    ///
    /// This is used internally by components that need to schedule events
    /// from within event handlers. Note: This creates a new scheduler instance
    /// for isolation purposes in the tonic integration.
    pub fn handle(&self) -> SchedulerHandle {
        // For the tonic integration, we create a new scheduler with the same time
        let mut new_scheduler = Scheduler::default();
        new_scheduler.clock = Arc::clone(&self.clock);
        SchedulerHandle::new(Arc::new(Mutex::new(new_scheduler)))
    }

    /// Returns a reference to the next scheduled event or `None` if none are left.
    pub fn peek(&mut self) -> Option<&EventEntry> {
        self.events.peek()
    }

    /// Removes and returns the next scheduled event or `None` if none are left.
    /// Skips cancelled task events to prevent time advancement for cancelled tasks.
    pub fn pop(&mut self) -> Option<EventEntry> {
        while let Some(event) = self.events.pop() {
            // Check if this is a cancelled task event
            if let EventEntry::Task(ref task_entry) = event {
                if self.cancelled_tasks.contains(&task_entry.task_id) {
                    // Remove from cancelled set and skip this event (don't advance time)
                    self.cancelled_tasks.remove(&task_entry.task_id);
                    continue;
                }
            }

            // Not cancelled - advance time and return this event
            self.clock
                .store(simtime_to_nanos(event.time()), AtomicOrdering::Relaxed);
            return Some(event);
        }

        None
    }

    // Task scheduling methods

    /// Schedule a task to run at a specific time
    pub fn schedule_task<T: Task>(&mut self, delay: SimTime, task: T) -> TaskHandle<T::Output> {
        let task_id = TaskId::new();
        let wrapper = TaskWrapper::new(task, task_id);
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

        if let Some(task) = self.pending_tasks.remove(&task_id) {
            let result = task.execute(self);

            // Don't store results for () tasks since they're typically fire-and-forget
            // (timeouts, periodic callbacks, retry delays, etc.) and never retrieved
            if result.downcast_ref::<()>().is_none() {
                self.completed_task_results.insert(task_id, result);
            }

            true
        } else {
            false
        }
    }

    /// Get the result of a completed task
    pub fn get_task_result<T: 'static>(&mut self, handle: TaskHandle<T>) -> Option<T> {
        self.completed_task_results
            .remove(&handle.id())
            .and_then(|boxed| boxed.downcast::<T>().ok())
            .map(|boxed| *boxed)
    }

    // Deferred wake methods

    /// Add a deferred wake event to be scheduled after the current event completes.
    /// This is called by `defer_wake()` via thread-local storage.
    pub(crate) fn add_deferred_wake<E: fmt::Debug + 'static>(
        &mut self,
        component: Key<E>,
        event: E,
    ) {
        trace!(
            component_id = ?component.id(),
            event_type = std::any::type_name::<E>(),
            "Deferred wake added"
        );
        self.deferred_wakes
            .push(DeferredWake::new(component, event));
    }

    /// Process all deferred wakes, scheduling them at the current time.
    /// This is called automatically at the end of each simulation step.
    pub(crate) fn process_deferred_wakes(&mut self) {
        if self.deferred_wakes.is_empty() {
            return;
        }

        let wakes: Vec<_> = self.deferred_wakes.drain(..).collect();
        trace!(
            count = wakes.len(),
            current_time = ?self.time(),
            "Processing deferred wakes"
        );

        for wake in wakes {
            wake.execute(self);
        }
    }

    /// Check if there are any pending deferred wakes
    pub fn has_deferred_wakes(&self) -> bool {
        !self.deferred_wakes.is_empty()
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
            component: Uuid::now_v7(),
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
            component: Uuid::now_v7(),
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

        let component_a = Key::<EventA>::new_with_id(Uuid::now_v7());
        let component_b = Key::<EventB>::new_with_id(Uuid::now_v7());

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
            Box::new(crate::task::TaskWrapper::new(unit_task, task_id1)),
        );

        // Schedule a task that returns a value
        let task_id2 = TaskId::new();
        let value_task = crate::task::ClosureTask::new(|_| 42);
        scheduler.schedule_task_at(
            SimTime::from_secs(2),
            task_id2,
            Box::new(crate::task::TaskWrapper::new(value_task, task_id2)),
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
            Box::new(crate::task::TaskWrapper::new(task, task_id)),
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
