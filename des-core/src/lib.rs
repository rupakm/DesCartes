//! Core discrete event simulation engine
//!
//! This crate provides the fundamental building blocks for discrete event simulation,
//! including time management, event scheduling, and process execution.

pub mod async_runtime;
pub mod dists;
pub mod error;
pub mod execute;
pub mod metrics;
pub mod scheduler;
pub mod time;
pub mod types;

pub mod formal;

use std::collections::HashMap;
use std::any::Any;

//pub use environment::Environment;
pub use error::{EventError, SimError};
// pub use event::{Event, EventPayload, EventScheduler};
// pub use process::{Process, ProcessManager, Delay, EventWaiter, delay, wait_for_event};
pub use time::SimTime;
pub use types::EventId;
pub use execute::{Executor, Execute};
pub use metrics::{MetricEmitter, MetricValue, MetricType, MetricBuilder, SimulationMetrics, MetricsSummary};
// pub use request::{Request, RequestAttempt, RequestStatus, AttemptStatus, Response, ResponseStatus};
pub use scheduler::{EventEntry, Scheduler};



pub use formal::{LyapunovError, CertificateError, VerificationError};

use uuid::Uuid;

#[derive(Debug)]
pub struct Key<T> {
    id: Uuid,
    _marker: std::marker::PhantomData<T>,
}

impl<T> Key<T> {
    pub fn new_with_id(id: Uuid) -> Self {
        Self {
            id,
            _marker: std::marker::PhantomData,

        }
    }

    /// Get the UUID of this key
    pub fn id(&self) -> Uuid {
        self.id
    }
}

impl<T> Clone for Key<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Key<T> {}


pub trait ProcessEventEntry: Any {
    fn process_event_entry(&mut self, entry: EventEntry, scheduler: &mut Scheduler);
}

pub trait Component: ProcessEventEntry {
    type Event: 'static;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    );

    // should we have a function to dump state?
}

impl<E, C> ProcessEventEntry for C
where
    E: std::fmt::Debug + 'static,
    C: Component<Event = E> + 'static,
{
    fn process_event_entry(&mut self, entry: EventEntry, scheduler: &mut Scheduler) {
        let typed_entry = entry
            .downcast::<E>()
            .expect("Failed to downcast event entry.");
        self.process_event(typed_entry.component_key, typed_entry.event, scheduler);
    }
}

/// Container holding type-erased components.
#[derive(Default)]
pub struct Components {
    components: HashMap<Uuid, Box<dyn ProcessEventEntry>>,
}

impl Components {
    #[allow(clippy::missing_panics_doc)]
    /// Process the event on the component given by the event entry.
    pub fn process_event_entry(
        &mut self,
        entry: EventEntry,
        scheduler: &mut Scheduler,
    ) {
        self.components
            .get_mut(&entry.component_idx())
            .unwrap()
            .process_event_entry(entry, scheduler);
    }

    /// Registers a new component and returns its ID.
    #[must_use]
    pub fn register<E: std::fmt::Debug + 'static, C: Component<Event = E> + 'static>(
        &mut self,
        component: C,
    ) -> Key<E> {
        let id = Uuid::now_v7();
        self.components.insert(id, Box::new(component));
        Key::new_with_id(id)
    }

    pub fn remove<E: 'static, C: Component<Event=E> + 'static>(&mut self, key: Key<E>) -> Option<C> {
        use std::any::Any;
        
        self.components.remove(&key.id).and_then(|boxed_trait| {
            // Since ProcessEventEntry extends Any, we can cast the Box
            let boxed_any: Box<dyn Any> = boxed_trait;
            boxed_any.downcast::<C>().ok().map(|boxed_c| *boxed_c)
        })
    }
}


/// Simulation struct that puts different parts of the simulation together.
///
/// See the [crate-level documentation](index.html) for more information.
#[derive(Default)]
pub struct Simulation {
    /// Event scheduler.
    pub scheduler: Scheduler,
    /// Component container.
    pub components: Components,
}

impl Simulation {
    /// Performs one step of the simulation. Returns `true` if there was in fact an event
    /// available to process, and `false` otherwise, which signifies that the simulation
    /// ended.
    pub fn step(&mut self) -> bool {
        self.scheduler.pop().is_some_and(|event| {
            self.components
                .process_event_entry(event, &mut self.scheduler);
            true
        })
    }

    /// Runs the entire simulation.
    ///
    /// The stopping condition and other execution details depend on the executor used.
    /// See [`Execute`] and [`Executor`] for more details.
    pub fn execute<E: Execute>(&mut self, executor: E) {
        executor.execute(self);
    }

    /// Adds a new component.
    #[must_use]
    pub fn add_component<E: std::fmt::Debug + 'static, C: Component<Event = E> + 'static>(
        &mut self,
        component: C,
    ) -> Key<E> {
        self.components.register(component)
    }

    /// Remove a component: usually at the end of the simulation to peek at the state
    #[must_use]
    pub fn remove_component<E: std::fmt::Debug + 'static, C: Component<Event=E> + 'static>(
        &mut self,
        key: Key<E>,
    ) -> Option<C> {
        self.components.remove(key)
    }

    /// Schedules a new event to be executed at time `time` in component `component`.
    pub fn schedule<E: std::fmt::Debug + 'static>(
        &mut self,
        time: SimTime,
        component: Key<E>,
        event: E,
    ) {
        self.scheduler.schedule(time, component, event);
    }
}
