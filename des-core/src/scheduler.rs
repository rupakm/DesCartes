use std::any::Any;
use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fmt;
use std::rc::Rc;
use uuid::Uuid;

use crate::{Key, SimTime};
use crate::types::EventId;

/// Entry type stored in the scheduler, including the event value, component key, and the time when
/// it is supposed to occur.
///
/// Besides being stored in the scheduler's internal priority queue,
/// event entries are simply passed to [`crate::Components`] object, which unpacks them, and passes them
/// to the correct component.
#[derive(Debug)]
pub struct EventEntry {
    event_id: EventId,
    time: SimTime,
    pub(crate) component: Uuid,
    inner: Box<dyn Any>,
}

impl EventEntry {
    pub(crate) fn new<E: fmt::Debug + 'static>(
        id: EventId,
        time: SimTime,
        component: Key<E>,
        event: E,
    ) -> Self {
        EventEntry {
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

    #[must_use]
    pub(crate) fn component_idx(&self) -> Uuid {
        self.component
    }

    #[must_use]
    pub(crate) fn time(&self) -> SimTime {
        self.time
    }
}

impl PartialEq for EventEntry {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time
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
        other.time.cmp(&self.time)
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
}

impl Default for Scheduler {
    fn default() -> Self {
        Self {
            next_event_id: 0,
            events: BinaryHeap::default(),
            clock: Rc::new(Cell::new(SimTime::default())),
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
        self.events.push(EventEntry::new(EventId(self.next_event_id), time, component, event));
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
            self.clock.replace(event.time);
        })
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
        let entry = EventEntry {
            event_id: EventId(0),
            time: SimTime::from_duration(Duration::from_secs(1)),
            component: Uuid::now_v7(),
            inner: Box::new(String::from("inner")),
        };
        assert!(entry.downcast::<String>().is_some());
        assert!(entry.downcast::<i32>().is_none());
    }

    #[test]
    fn test_event_entry_cmp() {
        let make_entry = || EventEntry {
            event_id: EventId(0),
            time: SimTime::from_duration(Duration::from_secs(1)),
            component: Uuid::now_v7(),
            inner: Box::new(String::from("inner")),
        };
        assert_eq!(
            EventEntry {
                event_id: EventId(0),
                time: SimTime::from_duration(Duration::from_secs(1)),
                ..make_entry()
            },
            EventEntry {
                time: SimTime::from_duration(Duration::from_secs(1)),
                ..make_entry()
            }
        );
        assert_eq!(
            EventEntry {
                event_id: EventId(0),
                time: SimTime::from_duration(Duration::from_secs(0)),
                ..make_entry()
            }
            .cmp(&EventEntry {
                event_id: EventId(1),
                time: SimTime::from_duration(Duration::from_secs(1)),
                ..make_entry()
            }),
            Ordering::Greater
        );
        assert_eq!(
            EventEntry {
                event_id: EventId(1),
                time: SimTime::from_duration(Duration::from_secs(2)),
                ..make_entry()
            }
            .cmp(&EventEntry {
                event_id: EventId(3),
                time: SimTime::from_duration(Duration::from_secs(1)),
                ..make_entry()
            }),
            Ordering::Less
        );
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
