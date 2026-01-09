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

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tracing::{debug, instrument, trace, warn};

use crate::scheduler::Scheduler;
use crate::waker::create_des_waker;
use crate::{Component, Key, SimTime};

/// Unique identifier for async tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub u64);

// Thread-local storage for current simulation time and scheduler during polling.
// This allows SimSleep to access the current time and schedule wake events.
thread_local! {
    static CURRENT_TIME: RefCell<Option<SimTime>> = const { RefCell::new(None) };
    static CURRENT_SCHEDULER: RefCell<Option<*mut Scheduler>> = const { RefCell::new(None) };
    static CURRENT_RUNTIME_KEY: RefCell<Option<Key<RuntimeEvent>>> = const { RefCell::new(None) };
    static CURRENT_TASK_ID: RefCell<Option<TaskId>> = const { RefCell::new(None) };
}

/// Get the current simulation time (for use in futures).
pub fn current_sim_time() -> Option<SimTime> {
    CURRENT_TIME.with(|t| *t.borrow())
}

fn set_poll_context(
    time: SimTime,
    scheduler: &mut Scheduler,
    runtime_key: Key<RuntimeEvent>,
    task_id: TaskId,
) {
    CURRENT_TIME.with(|t| *t.borrow_mut() = Some(time));
    CURRENT_SCHEDULER.with(|s| *s.borrow_mut() = Some(scheduler as *mut Scheduler));
    CURRENT_RUNTIME_KEY.with(|k| *k.borrow_mut() = Some(runtime_key));
    CURRENT_TASK_ID.with(|t| *t.borrow_mut() = Some(task_id));
}

fn clear_poll_context() {
    CURRENT_TIME.with(|t| *t.borrow_mut() = None);
    CURRENT_SCHEDULER.with(|s| *s.borrow_mut() = None);
    CURRENT_RUNTIME_KEY.with(|k| *k.borrow_mut() = None);
    CURRENT_TASK_ID.with(|t| *t.borrow_mut() = None);
}

/// Schedule a wake event at a specific time (used by SimSleep).
fn schedule_wake_at(delay: SimTime) {
    CURRENT_SCHEDULER.with(|sched| {
        CURRENT_RUNTIME_KEY.with(|key| {
            CURRENT_TASK_ID.with(|task| {
                if let (Some(sched_ptr), Some(runtime_key), Some(task_id)) =
                    (*sched.borrow(), *key.borrow(), *task.borrow())
                {
                    if !sched_ptr.is_null() {
                        unsafe {
                            (*sched_ptr).schedule(
                                delay,
                                runtime_key,
                                RuntimeEvent::Wake { task_id },
                            );
                        }
                    }
                }
            });
        });
    });
}

/// Events that drive the async runtime.
#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    /// Poll all ready tasks.
    Poll,
    /// Wake a specific task (from timer expiration or external event).
    Wake { task_id: TaskId },
}

/// A suspended async task.
struct Task {
    future: Pin<Box<dyn Future<Output = ()>>>,
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
    next_task_id: u64,
    tasks: HashMap<TaskId, Task>,
    ready_queue: VecDeque<TaskId>,
    /// Runtime's own component key (set on first event).
    runtime_key: Option<Key<RuntimeEvent>>,
}

impl Default for DesRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl DesRuntime {
    /// Create a new async runtime.
    pub fn new() -> Self {
        Self {
            next_task_id: 0,
            tasks: HashMap::new(),
            ready_queue: VecDeque::new(),
            runtime_key: None,
        }
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
        let task_id = TaskId(self.next_task_id);
        self.next_task_id += 1;

        self.tasks.insert(
            task_id,
            Task {
                future: Box::pin(future),
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
    #[instrument(skip(self, scheduler), fields(task_id = ?task_id))]
    fn poll_task(&mut self, task_id: TaskId, scheduler: &mut Scheduler) -> Option<Poll<()>> {
        let runtime_key = self.runtime_key?;
        let task = self.tasks.get_mut(&task_id)?;

        // Create waker using unified waker utility
        let waker = create_des_waker(runtime_key, RuntimeEvent::Wake { task_id });
        let mut cx = Context::from_waker(&waker);

        // Set poll context for SimSleep
        set_poll_context(scheduler.time(), scheduler, runtime_key, task_id);

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

        let ready: Vec<TaskId> = self.ready_queue.drain(..).collect();
        let mut completed = 0;

        for task_id in ready {
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
        if self.tasks.contains_key(&task_id) && !self.ready_queue.contains(&task_id) {
            self.ready_queue.push_back(task_id);
            trace!(?task_id, "Task added to ready queue");
        }
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
            // Schedule a wake event at the target time
            let delay = target - current_time;
            schedule_wake_at(SimTime::from_duration(delay));
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
/// use des_core::async_runtime::sim_sleep;
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
/// use des_core::async_runtime::sim_sleep_until;
/// use des_core::SimTime;
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
