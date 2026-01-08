//! Core discrete event simulation engine
//!
//! This crate provides the fundamental building blocks for discrete event simulation,
//! including time management, event scheduling, and process execution.

pub mod async_runtime;
pub mod dists;
pub mod error;
pub mod execute;
pub mod logging;
pub mod request;
pub mod scheduler;
pub mod time;
pub mod types;
pub mod task;

pub mod formal;

use std::collections::HashMap;
use std::any::Any;
use tracing::{debug, info, trace, warn, instrument};

//pub use environment::Environment;
pub use error::{EventError, SimError};
// pub use event::{Event, EventPayload, EventScheduler};
// pub use process::{Process, ProcessManager, Delay, EventWaiter, delay, wait_for_event};
pub use time::SimTime;
pub use types::EventId;
pub use execute::{Executor, Execute};
pub use logging::{init_simulation_logging, init_simulation_logging_with_level, init_detailed_simulation_logging, simulation_span, component_span, event_span, task_span};
pub use request::{Request, RequestAttempt, RequestStatus, AttemptStatus, Response, ResponseStatus, RequestId, RequestAttemptId};
pub use scheduler::{EventEntry, Scheduler, defer_wake, in_scheduler_context, current_time};
pub use task::{Task, TaskId, TaskHandle, ClosureTask, TimeoutTask, RetryTask, PeriodicTask};



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
    fn as_any_mut(&mut self) -> &mut dyn Any;
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
        if let EventEntry::Component(component_entry) = entry {
            let typed_entry = component_entry
                .downcast::<E>()
                .expect("Failed to downcast event entry.");
            self.process_event(typed_entry.component_key, typed_entry.event, scheduler);
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
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
        match entry {
            EventEntry::Component(component_entry) => {
                if let Some(component) = self.components.get_mut(&component_entry.component) {
                    component.process_event_entry(EventEntry::Component(component_entry), scheduler);
                }
            }
            EventEntry::Task(task_entry) => {
                // Execute the task
                scheduler.execute_task(task_entry.task_id);
            }
        }
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
        self.components.remove(&key.id).and_then(|boxed_trait| {
            // Since ProcessEventEntry extends Any, we can cast the Box
            let boxed_any: Box<dyn std::any::Any> = boxed_trait;
            boxed_any.downcast::<C>().ok().map(|boxed_c| *boxed_c)
        })
    }

    /// Get mutable access to a component
    pub fn get_component_mut<E: 'static, C: Component<Event=E> + 'static>(&mut self, key: Key<E>) -> Option<&mut C> {
        self.components.get_mut(&key.id).and_then(|boxed_trait| {
            // Cast to Any first, then downcast to the concrete type
            let any_ref = boxed_trait.as_any_mut();
            any_ref.downcast_mut::<C>()
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
            trace!(
                event_time = ?event.time(),
                event_type = match &event {
                    EventEntry::Component(_) => "Component",
                    EventEntry::Task(_) => "Task",
                },
                "Processing simulation step"
            );
            
            // Set scheduler context for deferred wakes
            scheduler::set_scheduler_context(&mut self.scheduler);
            
            // Process the event
            self.components
                .process_event_entry(event, &mut self.scheduler);
            
            // Clear scheduler context
            scheduler::clear_scheduler_context();
            
            // Process any deferred wakes that were registered during event processing
            self.scheduler.process_deferred_wakes();
            
            true
        })
    }

    /// Runs the entire simulation.
    ///
    /// The stopping condition and other execution details depend on the executor used.
    /// See [`Execute`] and [`Executor`] for more details.
    #[instrument(skip(self, executor), fields(
        initial_time = ?self.scheduler.time()
    ))]
    pub fn execute<E: Execute>(&mut self, executor: E) {
        info!("Starting simulation execution");
        executor.execute(self);
        info!(
            final_time = ?self.scheduler.time(),
            "Simulation execution completed"
        );
    }

    /// Adds a new component.
    #[must_use]
    #[instrument(skip(self, component), fields(component_type = std::any::type_name::<C>()))]
    pub fn add_component<E: std::fmt::Debug + 'static, C: Component<Event = E> + 'static>(
        &mut self,
        component: C,
    ) -> Key<E> {
        let key = self.components.register(component);
        debug!(
            component_id = ?key.id(),
            "Added component to simulation"
        );
        key
    }

    /// Remove a component: usually at the end of the simulation to peek at the state
    #[must_use]
    #[instrument(skip(self), fields(component_id = ?key.id()))]
    pub fn remove_component<E: std::fmt::Debug + 'static, C: Component<Event=E> + 'static>(
        &mut self,
        key: Key<E>,
    ) -> Option<C> {
        let result = self.components.remove(key);
        if result.is_some() {
            debug!("Removed component from simulation");
        } else {
            warn!("Attempted to remove non-existent component");
        }
        result
    }

    /// Get mutable access to a component
    pub fn get_component_mut<E: std::fmt::Debug + 'static, C: Component<Event=E> + 'static>(
        &mut self,
        key: Key<E>,
    ) -> Option<&mut C> {
        self.components.get_component_mut(key)
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
