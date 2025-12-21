use std::any::Any;
use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::fmt;
use std::rc::Rc;
use uuid::Uuid;

use crate::{Key, SimTime};
use crate::types::EventId;
use crate::task::{Task, TaskId, TaskHandle, TaskExecution, TaskWrapper, ClosureTask, TimeoutTask};

/// Entry type stored in the scheduler, including the event value, component key, and the time when
/// it is supposed to occur.
///
/// Besides being stored in the scheduler's internal priority queue,
/// event entries are simply passed to [`crate::Components`] object, which unpacks them, and passes them
/// to the correct component.
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
    time: SimTime,
    pub(crate) task_id: TaskId,
}

impl TaskEventEntry {
    pub(crate) fn new(id: EventId, time: SimTime, task_id: TaskId) -> Self {
        // Note: id parameter kept for API compatibility but not stored
        // as it's not currently used
        let _ = id;
        TaskEventEntry {
            time,
            task_id,
        }
    }
}

impl PartialEq for EventEntry {
    fn eq(&self, other: &Self) -> bool {
        self.time() == other.time()
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
        other.time().cmp(&self.time())
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


type Clock = Rc<Cell<SimTime>>;

/// This struct exposes only immutable access to the simulation clock.
/// The clock itself is owned by the scheduler, while others can obtain `ClockRef`
/// to read the current simulation time.
///
/// # Example
///
/// ```
/// # use des_core::Scheduler;
/// let scheduler = Scheduler::default();
/// let clock_ref = scheduler.clock();
/// assert_eq!(clock_ref.time(), scheduler.time());
/// ```
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
        self.clock.get()
    }
}

/// Scheduler is used to keep the current time and information about the upcoming events.
///
/// See the [crate-level documentation](index.html) for more information.
pub struct Scheduler {
    next_event_id: u64,
    events: BinaryHeap<EventEntry>,
    clock: Clock,
    // Task management
    pending_tasks: HashMap<TaskId, Box<dyn TaskExecution>>,
    completed_task_results: HashMap<TaskId, Box<dyn Any>>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self {
            next_event_id: 0,
            events: BinaryHeap::default(),
            clock: Rc::new(Cell::new(SimTime::default())),
            pending_tasks: HashMap::new(),
            completed_task_results: HashMap::new(),
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
        let time = self.time() + time;
        let component_entry = ComponentEventEntry::new(EventId(self.next_event_id), time, component, event);
        self.events.push(EventEntry::Component(component_entry));
    }

    /// Schedules `event` to be executed for `component` at `self.time()`.
    pub fn schedule_now<E: fmt::Debug + 'static>(&mut self, component: Key<E>, event: E) {
        self.schedule(SimTime::zero(), component, event);
    }

    /// Returns the current simulation time.
    #[must_use]
    pub fn time(&self) -> SimTime {
        self.clock.get()
    }

    /// Returns a structure with immutable access to the simulation time.
    #[must_use]
    pub fn clock(&self) -> ClockRef {
        ClockRef {
            clock: Rc::clone(&self.clock),
        }
    }

    /// Returns a reference to the next scheduled event or `None` if none are left.
    pub fn peek(&mut self) -> Option<&EventEntry> {
        self.events.peek()
    }

    /// Removes and returns the next scheduled event or `None` if none are left.
    pub fn pop(&mut self) -> Option<EventEntry> {
        self.events.pop().inspect(|event| {
            self.clock.replace(event.time());
        })
    }

    // Task scheduling methods

    /// Schedule a task to run at a specific time
    pub fn schedule_task<T: Task>(
        &mut self,
        delay: SimTime,
        task: T,
    ) -> TaskHandle<T::Output> {
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
        let event = TaskEventEntry::new(
            EventId(self.next_event_id),
            time,
            task_id,
        );
        self.events.push(EventEntry::Task(event));
    }

    /// Schedule a closure as a task
    pub fn schedule_closure<F, R>(
        &mut self,
        delay: SimTime,
        closure: F,
    ) -> TaskHandle<R>
    where
        F: FnOnce(&mut Scheduler) -> R + 'static,
        R: 'static,
    {
        let task = ClosureTask::new(closure);
        self.schedule_task(delay, task)
    }

    /// Schedule a timeout callback
    pub fn timeout<F>(
        &mut self,
        delay: SimTime,
        callback: F,
    ) -> TaskHandle<()>
    where
        F: FnOnce(&mut Scheduler) + 'static,
    {
        let task = TimeoutTask::new(callback);
        self.schedule_task(delay, task)
    }

    /// Cancel a scheduled task
    pub fn cancel_task<T>(&mut self, handle: TaskHandle<T>) -> bool {
        self.pending_tasks.remove(&handle.id()).is_some()
    }

    /// Execute a task if it's ready
    pub(crate) fn execute_task(&mut self, task_id: TaskId) -> bool {
        if let Some(task) = self.pending_tasks.remove(&task_id) {
            let result = task.execute(self);
            self.completed_task_results.insert(task_id, result);
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
}

#[cfg(test)]
mod test {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_clock_ref() {
        let time = SimTime::from_duration(Duration::from_secs(1));
        let clock = Clock::new(Cell::new(time));
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
        
        let entry1 = EventEntry::Component(ComponentEventEntry {
            event_id: EventId(0),
            time: SimTime::from_duration(Duration::from_secs(1)),
            ..make_component_entry()
        });
        let entry2 = EventEntry::Component(ComponentEventEntry {
            time: SimTime::from_duration(Duration::from_secs(1)),
            ..make_component_entry()
        });
        assert_eq!(entry1, entry2);
        
        let entry3 = EventEntry::Component(ComponentEventEntry {
            event_id: EventId(0),
            time: SimTime::from_duration(Duration::from_secs(0)),
            ..make_component_entry()
        });
        let entry4 = EventEntry::Component(ComponentEventEntry {
            event_id: EventId(1),
            time: SimTime::from_duration(Duration::from_secs(1)),
            ..make_component_entry()
        });
        assert_eq!(entry3.cmp(&entry4), Ordering::Greater);
        
        let entry5 = EventEntry::Component(ComponentEventEntry {
            event_id: EventId(1),
            time: SimTime::from_duration(Duration::from_secs(2)),
            ..make_component_entry()
        });
        let entry6 = EventEntry::Component(ComponentEventEntry {
            event_id: EventId(3),
            time: SimTime::from_duration(Duration::from_secs(1)),
            ..make_component_entry()
        });
        assert_eq!(entry5.cmp(&entry6), Ordering::Less);
    }

    #[derive(Debug, Clone, Eq, PartialEq)]
    struct EventA;
    #[derive(Debug, Clone, Eq, PartialEq)]
    struct EventB;

    #[test]
    fn test_scheduler() {
        let mut scheduler = Scheduler::default();
        assert_eq!(scheduler.time(), SimTime::from_duration(Duration::new(0, 0)));
        assert_eq!(scheduler.clock().time(), SimTime::from_duration(Duration::new(0, 0)));
        assert!(scheduler.events.is_empty());

        let component_a = Key::<EventA>::new_with_id(Uuid::now_v7());
        let component_b = Key::<EventB>::new_with_id(Uuid::now_v7());

        scheduler.schedule(SimTime::from_duration(Duration::from_secs(1)), component_a, EventA);
        scheduler.schedule_now(component_b, EventB);
        scheduler.schedule(SimTime::from_duration(Duration::from_secs(2)), component_b, EventB);

        assert_eq!(scheduler.time(), SimTime::from_duration(Duration::from_secs(0)));

        let entry = scheduler.pop().unwrap();
        let entry = entry.downcast::<EventB>().unwrap();
        assert_eq!(entry.time, SimTime::from_duration(Duration::from_secs(0)));
        assert_eq!(entry.component_idx, component_b.id);
        assert_eq!(entry.component_key.id, component_b.id);
        assert_eq!(entry.event, &EventB);

        assert_eq!(scheduler.time(), SimTime::from_duration(Duration::from_secs(0)));

        let entry = scheduler.pop().unwrap();
        let entry = entry.downcast::<EventA>().unwrap();
        assert_eq!(entry.time, SimTime::from_duration(Duration::from_secs(1)));
        assert_eq!(entry.component_idx, component_a.id);
        assert_eq!(entry.component_key.id, component_a.id);
        assert_eq!(entry.event, &EventA);

        assert_eq!(scheduler.time(), SimTime::from_duration(Duration::from_secs(1)));
        assert_eq!(scheduler.clock().time(), SimTime::from_duration(Duration::from_secs(1)));

        let entry = scheduler.pop().unwrap();
        let entry = entry.downcast::<EventB>().unwrap();
        assert_eq!(entry.time, SimTime::from_duration(Duration::from_secs(2)));
        assert_eq!(entry.component_idx, component_b.id);
        assert_eq!(entry.component_key.id, component_b.id);
        assert_eq!(entry.event, &EventB);

        assert_eq!(scheduler.time(), SimTime::from_duration(Duration::from_secs(2)));

        assert!(scheduler.pop().is_none());
    }
}
