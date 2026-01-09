//! Task system for short-lived operations in DES
//!
//! This module provides a lightweight alternative to Components for operations that:
//! - Execute once and complete (timeouts, callbacks)
//! - Don't need persistent state
//! - Should auto-cleanup after execution
//!
//! Tasks are scheduled through the Scheduler and automatically cleaned up after execution,
//! avoiding the overhead and complexity of full Components for simple operations.

use crate::{Scheduler, SimTime};
use std::any::Any;
use std::fmt;
use std::marker::PhantomData;
use tracing::{debug, info, instrument, trace, warn};
use uuid::Uuid;

/// Unique identifier for tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub Uuid);

impl TaskId {
    /// Create a new task ID
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Task({})", self.0)
    }
}

/// Handle for a scheduled task, allowing cancellation and type-safe result retrieval
#[derive(Debug, Clone, Copy)]
pub struct TaskHandle<T> {
    id: TaskId,
    _marker: PhantomData<T>,
}

impl<T> TaskHandle<T> {
    /// Create a new task handle
    pub(crate) fn new(id: TaskId) -> Self {
        Self {
            id,
            _marker: PhantomData,
        }
    }

    /// Get the task ID
    pub fn id(&self) -> TaskId {
        self.id
    }
}

/// Trait for tasks that can be executed by the scheduler
pub trait Task: 'static {
    /// The type returned by this task
    type Output: 'static;

    /// Execute the task
    fn execute(self, scheduler: &mut Scheduler) -> Self::Output;
}

/// Type-erased task execution trait
pub(crate) trait TaskExecution {
    /// Execute the task and return type-erased result
    fn execute(self: Box<Self>, scheduler: &mut Scheduler) -> Box<dyn Any>;

    /// Get the task ID
    #[allow(dead_code)]
    fn task_id(&self) -> TaskId;
}

/// Wrapper that implements TaskExecution for any Task
pub(crate) struct TaskWrapper<T: Task> {
    task: T,
}

impl<T: Task> TaskWrapper<T> {
    pub fn new(task: T, _id: TaskId) -> Self {
        Self { task }
    }
}

impl<T: Task> TaskExecution for TaskWrapper<T> {
    fn execute(self: Box<Self>, scheduler: &mut Scheduler) -> Box<dyn Any> {
        let result = self.task.execute(scheduler);
        Box::new(result)
    }

    fn task_id(&self) -> TaskId {
        TaskId::new()
    }
}

/// A task that executes a closure
pub struct ClosureTask<F, R> {
    closure: F,
    _marker: PhantomData<R>,
}

impl<F, R> ClosureTask<F, R>
where
    F: FnOnce(&mut Scheduler) -> R + 'static,
    R: 'static,
{
    /// Create a new closure task
    pub fn new(closure: F) -> Self {
        Self {
            closure,
            _marker: PhantomData,
        }
    }
}

impl<F, R> Task for ClosureTask<F, R>
where
    F: FnOnce(&mut Scheduler) -> R + 'static,
    R: 'static,
{
    type Output = R;

    #[instrument(skip(self, scheduler), fields(task_type = "ClosureTask"))]
    fn execute(self, scheduler: &mut Scheduler) -> Self::Output {
        debug!("Executing closure task");
        let result = (self.closure)(scheduler);
        trace!("Closure task completed");
        result
    }
}

/// A task that executes after a timeout
pub struct TimeoutTask<F> {
    callback: F,
}

impl<F> TimeoutTask<F>
where
    F: FnOnce(&mut Scheduler) + 'static,
{
    /// Create a new timeout task
    pub fn new(callback: F) -> Self {
        Self { callback }
    }
}

impl<F> Task for TimeoutTask<F>
where
    F: FnOnce(&mut Scheduler) + 'static,
{
    type Output = ();

    #[instrument(skip(self, scheduler), fields(task_type = "TimeoutTask"))]
    fn execute(self, scheduler: &mut Scheduler) -> Self::Output {
        debug!("Executing timeout task");
        (self.callback)(scheduler);
        trace!("Timeout task completed");
    }
}

/// A task that retries an operation with exponential backoff
pub struct RetryTask<F, R, E> {
    operation: F,
    max_attempts: u32,
    current_attempt: u32,
    base_delay: SimTime,
    _marker: PhantomData<(R, E)>,
}

impl<F, R, E> RetryTask<F, R, E>
where
    F: Fn(&mut Scheduler) -> Result<R, E> + 'static,
    R: 'static,
    E: 'static,
{
    /// Create a new retry task
    pub fn new(operation: F, max_attempts: u32, base_delay: SimTime) -> Self {
        Self {
            operation,
            max_attempts,
            current_attempt: 0,
            base_delay,
            _marker: PhantomData,
        }
    }
}

impl<F, R, E> Task for RetryTask<F, R, E>
where
    F: Fn(&mut Scheduler) -> Result<R, E> + 'static,
    R: 'static,
    E: 'static,
{
    type Output = Result<R, E>;

    #[instrument(skip(self, scheduler), fields(
        task_type = "RetryTask",
        attempt = self.current_attempt + 1,
        max_attempts = self.max_attempts
    ))]
    fn execute(mut self, scheduler: &mut Scheduler) -> Self::Output {
        self.current_attempt += 1;

        debug!("Executing retry task");

        match (self.operation)(scheduler) {
            Ok(result) => {
                info!(attempt = self.current_attempt, "Retry task succeeded");
                Ok(result)
            }
            Err(error) => {
                if self.current_attempt >= self.max_attempts {
                    warn!(
                        attempt = self.current_attempt,
                        max_attempts = self.max_attempts,
                        "Retry task failed - max attempts reached"
                    );
                    Err(error)
                } else {
                    // Schedule retry with exponential backoff
                    let delay = self.base_delay * (2_u64.pow(self.current_attempt - 1));
                    debug!(
                        attempt = self.current_attempt,
                        next_delay = ?delay,
                        "Retry task failed - scheduling retry"
                    );

                    let task_id = TaskId::new();
                    let wrapper = TaskWrapper::new(self, task_id);
                    scheduler.schedule_task_at(
                        scheduler.time() + delay,
                        task_id,
                        Box::new(wrapper),
                    );

                    // Return a placeholder error - the real result will come from the retry
                    // This is a limitation of the current design - we can't easily return
                    // a "pending" state. For now, we'll just continue the retry chain.
                    Err(error)
                }
            }
        }
    }
}

/// A task that executes periodically
#[derive(Clone)]
pub struct PeriodicTask<F> {
    callback: F,
    interval: SimTime,
    remaining_executions: Option<u32>,
}

impl<F> PeriodicTask<F>
where
    F: Fn(&mut Scheduler) + Clone + 'static,
{
    /// Create a new periodic task that runs indefinitely
    pub fn new(callback: F, interval: SimTime) -> Self {
        Self {
            callback,
            interval,
            remaining_executions: None,
        }
    }

    /// Create a new periodic task that runs a limited number of times
    pub fn with_count(callback: F, interval: SimTime, count: u32) -> Self {
        Self {
            callback,
            interval,
            remaining_executions: Some(count),
        }
    }
}

impl<F> Task for PeriodicTask<F>
where
    F: Fn(&mut Scheduler) + Clone + 'static,
{
    type Output = ();

    #[instrument(skip(self, scheduler), fields(
        task_type = "PeriodicTask",
        interval = ?self.interval,
        remaining = ?self.remaining_executions
    ))]
    fn execute(mut self, scheduler: &mut Scheduler) -> Self::Output {
        debug!("Executing periodic task");

        // Execute the callback
        (self.callback)(scheduler);

        // Schedule next execution if needed
        if let Some(remaining) = &mut self.remaining_executions {
            *remaining -= 1;
            if *remaining == 0 {
                info!("Periodic task completed - no more executions");
                return; // No more executions
            }
            debug!(remaining = *remaining, "Periodic task continuing");
        } else {
            trace!("Periodic task continuing indefinitely");
        }

        // Schedule next execution
        let task_id = TaskId::new();
        let interval = self.interval;
        let wrapper = TaskWrapper::new(self, task_id);
        scheduler.schedule_task_at(scheduler.time() + interval, task_id, Box::new(wrapper));

        debug!(
            next_execution_time = ?(scheduler.time() + interval),
            "Scheduled next periodic task execution"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Simulation;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn test_task_id_creation() {
        let id1 = TaskId::new();
        let id2 = TaskId::new();
        assert_ne!(id1, id2);

        let id3 = TaskId::default();
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_task_handle() {
        let id = TaskId::new();
        let handle: TaskHandle<i32> = TaskHandle::new(id);
        assert_eq!(handle.id(), id);
    }

    #[test]
    fn test_closure_task() {
        let executed = Arc::new(Mutex::new(false));
        let executed_clone = executed.clone();

        let task = ClosureTask::new(move |_scheduler| {
            *executed_clone.lock().unwrap() = true;
            42
        });

        let mut scheduler = Scheduler::default();
        let result = task.execute(&mut scheduler);

        assert_eq!(result, 42);
        assert!(*executed.lock().unwrap());
    }

    #[test]
    fn test_timeout_task() {
        let executed = Arc::new(Mutex::new(false));
        let executed_clone = executed.clone();

        let task = TimeoutTask::new(move |_scheduler| {
            *executed_clone.lock().unwrap() = true;
        });

        let mut scheduler = Scheduler::default();
        task.execute(&mut scheduler);

        assert!(*executed.lock().unwrap());
    }

    #[test]
    fn test_periodic_task_with_count() {
        let counter = Arc::new(Mutex::new(0));
        let counter_clone = counter.clone();

        let task = PeriodicTask::with_count(
            move |_scheduler| {
                *counter_clone.lock().unwrap() += 1;
            },
            SimTime::from_duration(Duration::from_millis(100)),
            3,
        );

        let mut scheduler = Scheduler::default();

        // Execute the task - it should schedule itself for the next execution
        task.execute(&mut scheduler);

        // The first execution should have happened
        assert_eq!(*counter.lock().unwrap(), 1);

        // There should be a scheduled event for the next execution
        assert!(scheduler.peek().is_some());
    }

    #[test]
    fn test_retry_task_success() {
        let attempt_count = Arc::new(Mutex::new(0));
        let attempt_count_clone = attempt_count.clone();

        let task = RetryTask::new(
            move |_scheduler| {
                let mut count = attempt_count_clone.lock().unwrap();
                *count += 1;
                if *count >= 2 {
                    Ok(42)
                } else {
                    Err("Not ready yet")
                }
            },
            3,
            SimTime::from_duration(Duration::from_millis(100)),
        );

        let mut scheduler = Scheduler::default();
        let result = task.execute(&mut scheduler);

        // First attempt should fail
        assert!(result.is_err());
        assert_eq!(*attempt_count.lock().unwrap(), 1);

        // Should have scheduled a retry
        assert!(scheduler.peek().is_some());
    }

    #[test]
    fn test_retry_task_max_attempts() {
        let attempt_count = Arc::new(Mutex::new(0));
        let attempt_count_clone = attempt_count.clone();

        let task = RetryTask::new(
            move |_scheduler| -> Result<i32, &'static str> {
                let mut count = attempt_count_clone.lock().unwrap();
                *count += 1;
                Err("Always fails")
            },
            2, // Max 2 attempts
            SimTime::from_duration(Duration::from_millis(100)),
        );

        let mut scheduler = Scheduler::default();
        let result: Result<i32, &'static str> = task.execute(&mut scheduler);

        // Should fail after first attempt
        assert!(result.is_err());
        assert_eq!(*attempt_count.lock().unwrap(), 1);
    }
}
