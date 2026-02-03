//! Core discrete event simulation engine.
//!
//! This crate provides the fundamental building blocks for discrete event simulation:
//! time management, event scheduling, task execution, and component-based architecture.
//!
//! # Architecture Overview
//!
//! The simulation is built around two main types:
//!
//! - [`Simulation`]: The main entry point that owns the scheduler and components.
//!   Use this to run simulations, add components, and access simulation state.
//!
//! - [`SchedulerHandle`]: A cloneable handle for scheduling events during simulation.
//!   Pass this to Tower service layers and other components that need to schedule
//!   events without direct access to the simulation.
//!
//! # Basic Usage
//!
//! ```rust,no_run
//! use descartes_core::{Simulation, SimTime, Executor};
//! use std::time::Duration;
//!
//! // Create a simulation
//! let mut simulation = Simulation::default();
//!
//! // Get a scheduler handle for Tower layers or other components
//! let scheduler = simulation.scheduler_handle();
//!
//! // Schedule events, add components, run simulation
//! simulation.execute(Executor::unbound());
//! ```
//!
//! # Scheduling Events
//!
//! For simple scheduling from within the simulation:
//! ```rust,ignore
//! simulation.schedule(SimTime::from_millis(100), component_key, MyEvent::Tick);
//! ```
//!
//! For Tower layers or components that need a cloneable handle:
//! ```rust,ignore
//! let scheduler = simulation.scheduler_handle();
//! scheduler.schedule(SimTime::from_millis(100), component_key, MyEvent::Tick);
//! ```
//!
//! # Time Model
//!
//! All timing uses [`SimTime`], which represents simulation time (not wall-clock time).
//! This ensures deterministic, reproducible behavior across simulation runs.

pub mod async_runtime;
pub mod dists;
pub mod error;
pub mod execute;
pub mod ids;
pub mod logging;
pub mod randomness;
pub mod request;
pub mod scheduler;
pub mod task;
pub mod time;
pub mod types;
pub mod waker;

pub mod formal;

use std::any::Any;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering as AtomicOrdering},
    Arc, Mutex,
};
use tracing::{debug, info, instrument, trace, warn};

//pub use environment::Environment;
pub use error::{EventError, SimError};
// pub use event::{Event, EventPayload, EventScheduler};
// pub use process::{Process, ProcessManager, Delay, EventWaiter, delay, wait_for_event};
pub use execute::{Execute, Executor};
pub use logging::{
    component_span, event_span, init_detailed_simulation_logging, init_simulation_logging,
    init_simulation_logging_with_level, simulation_span, task_span,
};
pub use randomness::{DrawSite, RandomProvider};
pub use request::{
    AttemptStatus, Request, RequestAttempt, RequestAttemptId, RequestId, RequestStatus, Response,
    ResponseStatus,
};
pub use scheduler::{
    current_time, defer_wake, defer_wake_after, in_scheduler_context, ClockRef, EventEntry,
    EventFrontierPolicy, FifoFrontierPolicy, FrontierEvent, FrontierEventKind, FrontierSignature,
    Scheduler, SchedulerHandle, UniformRandomFrontierPolicy,
};
pub use task::{ClosureTask, PeriodicTask, RetryTask, Task, TaskHandle, TaskId, TimeoutTask};
pub use time::SimTime;
pub use types::EventId;
pub use waker::create_des_waker;

pub use formal::{CertificateError, LyapunovError, VerificationError};

use uuid::Uuid;

/// Global configuration for a simulation.
///
/// This currently only exposes a seed used to derive
/// deterministic randomness across components, but can
/// be extended in the future with additional options.
#[derive(Debug, Clone)]
pub struct SimulationConfig {
    /// Global seed for deterministic randomness.
    pub seed: u64,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self { seed: 1 }
    }
}

#[derive(Debug)]
pub struct Key<T> {
    id: Uuid,
    _marker: std::marker::PhantomData<T>,
}

impl<T> Key<T> {
    /// Create a new key with a generated UUID.
    ///
    /// This is deterministic (derived from a process-local counter) so that tests
    /// and simulations are reproducible without relying on wall-clock UUIDs.
    pub fn new() -> Self {
        static NEXT_KEY_ID: AtomicU64 = AtomicU64::new(0);
        let counter = NEXT_KEY_ID.fetch_add(1, AtomicOrdering::Relaxed) + 1;
        let id = crate::ids::deterministic_uuid(0, crate::ids::UUID_DOMAIN_KEY, counter);
        Self::new_with_id(id)
    }

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
    next_component_id: u64,
    id_seed: u64,
}

impl Components {
    #[allow(clippy::missing_panics_doc)]
    /// Process the event on the component given by the event entry.
    pub fn process_event_entry(&mut self, entry: EventEntry, scheduler: &mut Scheduler) {
        match entry {
            EventEntry::Component(component_entry) => {
                if let Some(component) = self.components.get_mut(&component_entry.component) {
                    component
                        .process_event_entry(EventEntry::Component(component_entry), scheduler);
                }
            }
            EventEntry::Task(task_entry) => {
                // Execute the task
                scheduler.execute_task(task_entry.task_id);
            }
        }
    }

    /// Registers a new component with the given ID.
    #[must_use]
    pub fn register_with_id<E: std::fmt::Debug + 'static, C: Component<Event = E> + 'static>(
        &mut self,
        id: Uuid,
        component: C,
    ) -> Key<E> {
        self.components.insert(id, Box::new(component));
        Key::new_with_id(id)
    }

    /// Registers a new component and returns its ID.
    ///
    /// This uses a deterministic ID generator (seed + counter). For simulation-wide
    /// determinism prefer `Simulation::add_component`, which uses the simulation seed.
    #[must_use]
    pub fn register<E: std::fmt::Debug + 'static, C: Component<Event = E> + 'static>(
        &mut self,
        component: C,
    ) -> Key<E> {
        self.next_component_id += 1;
        let id = crate::ids::deterministic_uuid(
            self.id_seed,
            crate::ids::UUID_DOMAIN_COMPONENT,
            self.next_component_id,
        );
        self.register_with_id(id, component)
    }

    pub fn remove<E: 'static, C: Component<Event = E> + 'static>(
        &mut self,
        key: Key<E>,
    ) -> Option<C> {
        self.components.remove(&key.id).and_then(|boxed_trait| {
            // Since ProcessEventEntry extends Any, we can cast the Box
            let boxed_any: Box<dyn std::any::Any> = boxed_trait;
            boxed_any.downcast::<C>().ok().map(|boxed_c| *boxed_c)
        })
    }

    /// Get mutable access to a component
    pub fn get_component_mut<E: 'static, C: Component<Event = E> + 'static>(
        &mut self,
        key: Key<E>,
    ) -> Option<&mut C> {
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
pub struct Simulation {
    /// Event scheduler (wrapped in Arc<Mutex<>> for interior mutability).
    scheduler: Arc<Mutex<Scheduler>>,
    /// Same-time frontier tie-breaking policy.
    frontier_policy: Box<dyn scheduler::EventFrontierPolicy>,
    /// Deterministic component ID counter.
    next_component_id: u64,
    /// Component container.
    pub components: Components,
    /// Global configuration for this simulation instance.
    config: SimulationConfig,
}

impl Default for Simulation {
    fn default() -> Self {
        // Use a well-known default seed for simulations created
        // via `Simulation::default()` so that behavior is
        // reproducible across runs unless overridden.
        Self::new(SimulationConfig { seed: 42 })
    }
}

impl Simulation {
    /// Create a new simulation with the given configuration.
    #[must_use]
    pub fn new(config: SimulationConfig) -> Self {
        Self {
            scheduler: Arc::new(Mutex::new(Scheduler::with_seed(config.seed))),
            frontier_policy: Box::new(scheduler::FifoFrontierPolicy),
            next_component_id: 0,
            components: Components::default(),
            config,
        }
    }

    /// Set the same-time event frontier tie-breaking policy.
    ///
    /// By default, simulations use deterministic FIFO ordering.
    pub fn set_frontier_policy(&mut self, policy: Box<dyn scheduler::EventFrontierPolicy>) {
        self.frontier_policy = policy;
    }

    /// Access the simulation configuration.
    #[must_use]
    pub fn config(&self) -> &SimulationConfig {
        &self.config
    }

    /// Returns a handle for scheduling events during simulation stepping.
    ///
    /// The `SchedulerHandle` can be cloned and passed to Tower service layers
    /// or other components that need to schedule events without locking the
    /// entire simulation.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut simulation = Simulation::default();
    /// let scheduler = simulation.scheduler_handle();
    ///
    /// // Pass to Tower layers
    /// let service = DesServiceBuilder::new("server".to_string())
    ///     .timeout(Duration::from_secs(5), scheduler)
    ///     .build(&mut simulation)?;
    /// ```
    #[must_use]
    pub fn scheduler_handle(&self) -> SchedulerHandle {
        SchedulerHandle::new(Arc::clone(&self.scheduler))
    }

    /// Returns the current simulation time.
    #[must_use]
    pub fn time(&self) -> SimTime {
        self.scheduler.lock().unwrap().time()
    }

    /// Performs one step of the simulation. Returns `true` if there was in fact an event
    /// available to process, and `false` otherwise, which signifies that the simulation
    /// ended.
    pub fn step(&mut self) -> bool {
        // Pop the next event while holding the lock briefly
        let event = {
            let mut scheduler = self.scheduler.lock().unwrap();
            scheduler.pop_with_policy(self.frontier_policy.as_mut())
        };

        event.is_some_and(|event| {
            trace!(
                event_time = ?event.time(),
                event_type = match &event {
                    EventEntry::Component(_) => "Component",
                    EventEntry::Task(_) => "Task",
                },
                "Processing simulation step"
            );

            // Set scheduler context for deferred wakes
            {
                let scheduler = self.scheduler.lock().unwrap();
                scheduler::set_scheduler_context(&scheduler);
            }

            // Process the event - need mutable access to scheduler for task execution
            {
                let mut scheduler = self.scheduler.lock().unwrap();
                self.components.process_event_entry(event, &mut scheduler);
            }

            // Clear scheduler context
            scheduler::clear_scheduler_context();

            // Process any deferred wakes that were registered during event processing
            {
                let mut scheduler = self.scheduler.lock().unwrap();
                scheduler::drain_deferred_wakes(&mut scheduler);
            }

            true
        })
    }

    /// Runs the entire simulation.
    ///
    /// The stopping condition and other execution details depend on the executor used.
    /// See [`Execute`] and [`Executor`] for more details.
    #[instrument(skip(self, executor), fields(
        initial_time = ?self.time()
    ))]
    pub fn execute<E: Execute>(&mut self, executor: E) {
        info!("Starting simulation execution");
        executor.execute(self);
        info!(
            final_time = ?self.time(),
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
        self.next_component_id += 1;
        let id = crate::ids::deterministic_uuid(
            self.config.seed,
            crate::ids::UUID_DOMAIN_COMPONENT,
            self.next_component_id,
        );

        let key = self.components.register_with_id(id, component);
        debug!(component_id = ?key.id(), "Added component to simulation");
        key
    }

    /// Remove a component: usually at the end of the simulation to peek at the state
    #[must_use]
    #[instrument(skip(self), fields(component_id = ?key.id()))]
    pub fn remove_component<E: std::fmt::Debug + 'static, C: Component<Event = E> + 'static>(
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
    pub fn get_component_mut<E: std::fmt::Debug + 'static, C: Component<Event = E> + 'static>(
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
        let mut scheduler = self.scheduler.lock().unwrap();
        scheduler.schedule(time, component, event);
    }

    /// Schedules a new event to be executed right away in component `component`.
    pub fn schedule_now<E: std::fmt::Debug + 'static>(&mut self, component: Key<E>, event: E) {
        let mut scheduler = self.scheduler.lock().unwrap();
        scheduler.schedule(SimTime::zero(), component, event);
    }

    /// Returns the time of the next scheduled event, or None if no events are scheduled.
    pub fn peek_next_event_time(&self) -> Option<SimTime> {
        let mut scheduler = self.scheduler.lock().unwrap();
        scheduler.peek().map(|e| e.time())
    }

    /// Returns a ClockRef for reading the simulation time.
    pub fn clock(&self) -> scheduler::ClockRef {
        let scheduler = self.scheduler.lock().unwrap();
        scheduler.clock()
    }

    /// Schedule a closure as a task
    pub fn schedule_closure<F, R>(&mut self, delay: SimTime, closure: F) -> task::TaskHandle<R>
    where
        F: FnOnce(&mut Scheduler) -> R + 'static,
        R: 'static,
    {
        let mut scheduler = self.scheduler.lock().unwrap();
        scheduler.schedule_closure(delay, closure)
    }

    /// Schedule a timeout callback
    pub fn timeout<F>(&mut self, delay: SimTime, callback: F) -> task::TaskHandle<()>
    where
        F: FnOnce(&mut Scheduler) + 'static,
    {
        let mut scheduler = self.scheduler.lock().unwrap();
        scheduler.timeout(delay, callback)
    }

    /// Schedule a task to run at a specific time
    pub fn schedule_task<T: task::Task>(
        &mut self,
        delay: SimTime,
        task: T,
    ) -> task::TaskHandle<T::Output> {
        let mut scheduler = self.scheduler.lock().unwrap();
        scheduler.schedule_task(delay, task)
    }

    /// Cancel a scheduled task
    pub fn cancel_task<T>(&mut self, handle: task::TaskHandle<T>) -> bool {
        let mut scheduler = self.scheduler.lock().unwrap();
        scheduler.cancel_task(handle)
    }

    /// Get the result of a completed task
    pub fn get_task_result<T: 'static>(&mut self, handle: task::TaskHandle<T>) -> Option<T> {
        let mut scheduler = self.scheduler.lock().unwrap();
        scheduler.get_task_result(handle)
    }

    /// Check if there are pending events
    pub fn has_pending_events(&self) -> bool {
        let mut scheduler = self.scheduler.lock().unwrap();
        scheduler.peek().is_some()
    }
}
