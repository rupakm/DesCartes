//! Simulation environment and execution
//!
//! This module provides the Environment struct, which is the central coordinator
//! for simulation execution. It manages time progression, event scheduling, and
//! process execution.

use crate::error::SimError;
use crate::event::{EventPayload, EventScheduler};
use crate::process::{Process, ProcessManager};
use crate::time::SimTime;
use crate::types::{EventId, ProcessId};
use std::pin::Pin;
use std::time::Duration;

/// The simulation environment
///
/// Environment is the central coordinator for discrete event simulation. It manages:
/// - Current simulation time
/// - Event scheduling and execution
/// - Process lifecycle and execution
///
/// # Examples
///
/// ```
/// use des_core::environment::Environment;
/// use des_core::{SimTime, event::EventPayload};
/// use std::time::Duration;
///
/// let mut env = Environment::new();
/// assert_eq!(env.now(), SimTime::zero());
///
/// // Schedule an event
/// env.schedule(Duration::from_millis(100), EventPayload::Generic);
///
/// // Run simulation
/// env.run_until(SimTime::from_millis(200)).unwrap();
/// assert_eq!(env.now(), SimTime::from_millis(200));
/// ```
pub struct Environment {
    /// Current simulation time
    current_time: SimTime,
    /// Event scheduler for managing the event queue
    scheduler: EventScheduler,
    /// Process manager for tracking active processes
    process_manager: ProcessManager,
}

impl Environment {
    /// Create a new simulation environment
    ///
    /// The environment starts at time zero with an empty event queue.
    pub fn new() -> Self {
        Self {
            current_time: SimTime::zero(),
            scheduler: EventScheduler::new(),
            process_manager: ProcessManager::new(),
        }
    }

    /// Get the current simulation time
    ///
    /// # Examples
    ///
    /// ```
    /// use des_core::environment::Environment;
    /// use des_core::SimTime;
    ///
    /// let env = Environment::new();
    /// assert_eq!(env.now(), SimTime::zero());
    /// ```
    pub fn now(&self) -> SimTime {
        self.current_time
    }

    /// Schedule an event to occur after a delay
    ///
    /// The event will be scheduled at `current_time + delay`.
    ///
    /// # Arguments
    ///
    /// * `delay` - Duration from now when the event should occur
    /// * `payload` - Event-specific data
    ///
    /// # Returns
    ///
    /// The unique ID of the scheduled event
    ///
    /// # Examples
    ///
    /// ```
    /// use des_core::environment::Environment;
    /// use des_core::event::EventPayload;
    /// use std::time::Duration;
    ///
    /// let mut env = Environment::new();
    /// let event_id = env.schedule(
    ///     Duration::from_millis(100),
    ///     EventPayload::Generic,
    /// );
    /// ```
    pub fn schedule(&mut self, delay: Duration, payload: EventPayload) -> EventId {
        let event_time = self.current_time + delay;
        self.scheduler.schedule(event_time, payload)
    }

    /// Spawn a new process in the simulation
    ///
    /// The process will be registered and can be polled during simulation execution.
    ///
    /// # Arguments
    ///
    /// * `process` - The process to spawn
    ///
    /// # Returns
    ///
    /// The unique ID of the spawned process
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use des_core::environment::Environment;
    /// use des_core::Process;
    ///
    /// let mut env = Environment::new();
    /// let process_id = env.spawn_process(Box::pin(my_process));
    /// ```
    pub fn spawn_process(&mut self, process: Pin<Box<dyn Process>>) -> ProcessId {
        self.process_manager.register_process(process)
    }

    /// Get a reference to the process manager (for testing)
    #[cfg(test)]
    pub fn process_manager(&self) -> &ProcessManager {
        &self.process_manager
    }

    /// Get a mutable reference to the process manager (for testing)
    #[cfg(test)]
    pub fn process_manager_mut(&mut self) -> &mut ProcessManager {
        &mut self.process_manager
    }

    /// Run the simulation until a specific time
    ///
    /// Processes all events up to and including the specified time.
    /// After execution, the current time will be set to the target time.
    ///
    /// # Arguments
    ///
    /// * `until` - The simulation time to run until
    ///
    /// # Returns
    ///
    /// `Ok(())` if the simulation ran successfully, or an error if something went wrong
    ///
    /// # Examples
    ///
    /// ```
    /// use des_core::environment::Environment;
    /// use des_core::{SimTime, event::EventPayload};
    /// use std::time::Duration;
    ///
    /// let mut env = Environment::new();
    /// env.schedule(Duration::from_millis(50), EventPayload::Generic);
    /// env.schedule(Duration::from_millis(150), EventPayload::Generic);
    ///
    /// env.run_until(SimTime::from_millis(100)).unwrap();
    /// assert_eq!(env.now(), SimTime::from_millis(100));
    /// ```
    pub fn run_until(&mut self, until: SimTime) -> Result<(), SimError> {
        // Process all events up to the target time
        while let Some(next_event) = self.scheduler.peek_next() {
            let event_time = next_event.time();
            
            // Stop if the next event is beyond our target time
            if event_time > until {
                break;
            }
            
            // Pop the event and advance time
            let event = self.scheduler.pop_next()?;
            self.current_time = event.time();
            
            // Wake processes waiting for this event
            self.process_manager.wake_event_waiters(event.id());
            
            // Wake processes waiting for time to advance
            self.process_manager.wake_time_waiters(self.current_time);
        }
        
        // Advance time to the target even if no events remain
        if self.current_time < until {
            self.current_time = until;
            // Wake any processes waiting for this time
            self.process_manager.wake_time_waiters(self.current_time);
        }
        
        Ok(())
    }

    /// Run the simulation until all events are processed
    ///
    /// Processes all events in the queue. After execution, the current time
    /// will be set to the time of the last processed event.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the simulation ran successfully, or an error if something went wrong
    ///
    /// # Examples
    ///
    /// ```
    /// use des_core::environment::Environment;
    /// use des_core::{SimTime, event::EventPayload};
    /// use std::time::Duration;
    ///
    /// let mut env = Environment::new();
    /// env.schedule(Duration::from_millis(100), EventPayload::Generic);
    /// env.schedule(Duration::from_millis(200), EventPayload::Generic);
    ///
    /// env.run().unwrap();
    /// assert_eq!(env.now(), SimTime::from_millis(200));
    /// ```
    pub fn run(&mut self) -> Result<(), SimError> {
        // Process all events in the queue
        while !self.scheduler.is_empty() {
            let event = self.scheduler.pop_next()?;
            self.current_time = event.time();
            
            // Wake processes waiting for this event
            self.process_manager.wake_event_waiters(event.id());
            
            // Wake processes waiting for time to advance
            self.process_manager.wake_time_waiters(self.current_time);
        }
        
        Ok(())
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventPayload;


    #[test]
    fn test_environment_creation() {
        let env = Environment::new();
        assert_eq!(env.now(), SimTime::zero());
    }

    #[test]
    fn test_schedule_event() {
        let mut env = Environment::new();
        
        let event_id = env.schedule(
            Duration::from_millis(100),
            EventPayload::Generic,
        );
        
        // Event should be scheduled but time hasn't advanced yet
        assert_eq!(env.now(), SimTime::zero());
        assert_eq!(event_id.0, 0);
    }

    #[test]
    fn test_run_until() {
        let mut env = Environment::new();
        
        // Schedule events at different times
        env.schedule(Duration::from_millis(50), EventPayload::Generic);
        env.schedule(Duration::from_millis(150), EventPayload::Generic);
        env.schedule(Duration::from_millis(250), EventPayload::Generic);
        
        // Run until 100ms - should process first event only
        env.run_until(SimTime::from_millis(100)).unwrap();
        assert_eq!(env.now(), SimTime::from_millis(100));
        
        // Run until 200ms - should process second event
        env.run_until(SimTime::from_millis(200)).unwrap();
        assert_eq!(env.now(), SimTime::from_millis(200));
        
        // Run until 300ms - should process third event
        env.run_until(SimTime::from_millis(300)).unwrap();
        assert_eq!(env.now(), SimTime::from_millis(300));
    }

    #[test]
    fn test_run_until_no_events() {
        let mut env = Environment::new();
        
        // Run with no events scheduled
        env.run_until(SimTime::from_millis(100)).unwrap();
        assert_eq!(env.now(), SimTime::from_millis(100));
    }

    #[test]
    fn test_run_all_events() {
        let mut env = Environment::new();
        
        // Schedule multiple events
        env.schedule(Duration::from_millis(100), EventPayload::Generic);
        env.schedule(Duration::from_millis(200), EventPayload::Generic);
        env.schedule(Duration::from_millis(300), EventPayload::Generic);
        
        // Run all events
        env.run().unwrap();
        
        // Time should be at the last event
        assert_eq!(env.now(), SimTime::from_millis(300));
    }

    #[test]
    fn test_run_empty_queue() {
        let mut env = Environment::new();
        
        // Run with no events
        env.run().unwrap();
        assert_eq!(env.now(), SimTime::zero());
    }

    #[test]
    fn test_event_ordering() {
        let mut env = Environment::new();
        
        // Schedule events in reverse order
        env.schedule(Duration::from_millis(300), EventPayload::Generic);
        env.schedule(Duration::from_millis(100), EventPayload::Generic);
        env.schedule(Duration::from_millis(200), EventPayload::Generic);
        
        // Run until 150ms - should process only the 100ms event
        env.run_until(SimTime::from_millis(150)).unwrap();
        assert_eq!(env.now(), SimTime::from_millis(150));
        
        // Continue running - should process remaining events in order
        env.run().unwrap();
        assert_eq!(env.now(), SimTime::from_millis(300));
    }

    #[test]
    fn test_multiple_events_same_time() {
        let mut env = Environment::new();
        
        // Schedule multiple events at the same time
        let time = Duration::from_millis(100);
        env.schedule(time, EventPayload::Generic);
        env.schedule(time, EventPayload::Generic);
        env.schedule(time, EventPayload::Generic);
        
        // Run all events
        env.run().unwrap();
        
        // All events should be processed
        assert_eq!(env.now(), SimTime::from_millis(100));
    }

    #[test]
    fn test_schedule_after_time_advance() {
        let mut env = Environment::new();
        
        // Schedule and run first event
        env.schedule(Duration::from_millis(100), EventPayload::Generic);
        env.run_until(SimTime::from_millis(100)).unwrap();
        assert_eq!(env.now(), SimTime::from_millis(100));
        
        // Schedule another event relative to current time
        env.schedule(Duration::from_millis(50), EventPayload::Generic);
        
        // Run the second event
        env.run().unwrap();
        assert_eq!(env.now(), SimTime::from_millis(150));
    }

    #[test]
    fn test_run_until_past_all_events() {
        let mut env = Environment::new();
        
        // Schedule events
        env.schedule(Duration::from_millis(100), EventPayload::Generic);
        env.schedule(Duration::from_millis(200), EventPayload::Generic);
        
        // Run until a time past all events
        env.run_until(SimTime::from_millis(500)).unwrap();
        
        // Time should advance to the target time
        assert_eq!(env.now(), SimTime::from_millis(500));
    }
}

    #[test]
    fn test_spawn_process() {
        use crate::process::Process;
        use std::future::Future;
        use std::pin::Pin;
        use std::task::{Context, Poll};

        struct SimpleProcess {
            name: String,
        }

        impl Future for SimpleProcess {
            type Output = ();

            fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                Poll::Ready(())
            }
        }

        impl Process for SimpleProcess {
            fn name(&self) -> &str {
                &self.name
            }
        }

        let mut env = Environment::new();
        
        let process = SimpleProcess {
            name: "test_process".to_string(),
        };
        
        let process_id = env.spawn_process(Box::pin(process));
        assert_eq!(process_id, ProcessId(0));
        assert_eq!(env.process_manager.active_count(), 1);
    }

    #[test]
    fn test_process_wakeup_on_time_advance() {
        let mut env = Environment::new();
        
        // Register a process waiting for time 100ms
        let process_id = ProcessId(0);
        env.process_manager.wait_for_time(process_id, SimTime::from_millis(100));
        
        // Run until 50ms - process should still be waiting
        env.run_until(SimTime::from_millis(50)).unwrap();
        assert_eq!(env.process_manager.time_waiters_count(), 1);
        
        // Run until 150ms - process should be woken
        env.run_until(SimTime::from_millis(150)).unwrap();
        assert_eq!(env.process_manager.time_waiters_count(), 0);
    }

    #[test]
    fn test_process_wakeup_on_event() {
        let mut env = Environment::new();
        
        // Schedule an event
        let event_id = env.schedule(
            Duration::from_millis(100),
            EventPayload::Generic,
        );
        
        // Register a process waiting for this event
        let process_id = ProcessId(0);
        env.process_manager.wait_for_event(process_id, event_id);
        
        // Verify the waiter is registered
        assert!(env.process_manager.has_event_waiter(event_id));
        
        // Run the simulation - should process the event and wake the process
        env.run().unwrap();
        
        // Verify the waiter was removed (woken up)
        assert!(!env.process_manager.has_event_waiter(event_id));
    }


    /// End-to-end integration tests for process execution
    #[cfg(test)]
    mod integration_tests {
        use super::*;
        use crate::process::Process;
        use std::future::Future;
        use std::pin::Pin;
        use std::task::{Context, Poll};

        struct SimpleProcess {
            name: String,
        }

        impl Future for SimpleProcess {
            type Output = ();

            fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                Poll::Ready(())
            }
        }

        impl Process for SimpleProcess {
            fn name(&self) -> &str {
                &self.name
            }
        }

        struct YieldingProcess {
            name: String,
            yields_remaining: usize,
        }

        impl Future for YieldingProcess {
            type Output = ();

            fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                if self.yields_remaining > 0 {
                    self.yields_remaining -= 1;
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            }
        }

        impl Process for YieldingProcess {
            fn name(&self) -> &str {
                &self.name
            }
        }

        #[test]
        fn test_e2e_spawn_single_process() {
            let mut env = Environment::new();

            let process = SimpleProcess {
                name: "test_process".to_string(),
            };

            let process_id = env.spawn_process(Box::pin(process));
            assert_eq!(process_id, ProcessId(0));
            assert_eq!(env.process_manager().active_count(), 1);
        }

        #[test]
        fn test_e2e_spawn_multiple_processes() {
            let mut env = Environment::new();

            let p1 = SimpleProcess {
                name: "process1".to_string(),
            };
            let p2 = YieldingProcess {
                name: "process2".to_string(),
                yields_remaining: 2,
            };
            let p3 = SimpleProcess {
                name: "process3".to_string(),
            };

            let id1 = env.spawn_process(Box::pin(p1));
            let id2 = env.spawn_process(Box::pin(p2));
            let id3 = env.spawn_process(Box::pin(p3));

            assert_eq!(id1, ProcessId(0));
            assert_eq!(id2, ProcessId(1));
            assert_eq!(id3, ProcessId(2));
            assert_eq!(env.process_manager().active_count(), 3);
        }

        #[test]
        fn test_e2e_process_wakeup_with_events() {
            let mut env = Environment::new();

            // Schedule multiple events at different times
            let event1 = env.schedule(
                Duration::from_millis(100),
                EventPayload::Generic,
            );
            let event2 = env.schedule(
                Duration::from_millis(200),
                EventPayload::Generic,
            );
            let event3 = env.schedule(
                Duration::from_millis(300),
                EventPayload::Generic,
            );

            // Register processes waiting for these events
            env.process_manager_mut()
                .wait_for_event(ProcessId(0), event1);
            env.process_manager_mut()
                .wait_for_event(ProcessId(1), event2);
            env.process_manager_mut()
                .wait_for_event(ProcessId(2), event3);

            // Verify all waiters are registered
            assert!(env.process_manager().has_event_waiter(event1));
            assert!(env.process_manager().has_event_waiter(event2));
            assert!(env.process_manager().has_event_waiter(event3));

            // Run until 150ms - should wake process 0
            env.run_until(SimTime::from_millis(150)).unwrap();
            assert!(!env.process_manager().has_event_waiter(event1));
            assert!(env.process_manager().has_event_waiter(event2));
            assert!(env.process_manager().has_event_waiter(event3));

            // Run until 250ms - should wake process 1
            env.run_until(SimTime::from_millis(250)).unwrap();
            assert!(!env.process_manager().has_event_waiter(event2));
            assert!(env.process_manager().has_event_waiter(event3));

            // Run remaining - should wake process 2
            env.run().unwrap();
            assert!(!env.process_manager().has_event_waiter(event3));
        }

        #[test]
        fn test_e2e_process_wakeup_with_time_delays() {
            let mut env = Environment::new();

            // Register processes waiting for different times
            env.process_manager_mut()
                .wait_for_time(ProcessId(0), SimTime::from_millis(100));
            env.process_manager_mut()
                .wait_for_time(ProcessId(1), SimTime::from_millis(200));
            env.process_manager_mut()
                .wait_for_time(ProcessId(2), SimTime::from_millis(300));

            assert_eq!(env.process_manager().time_waiters_count(), 3);

            // Advance time to 150ms - should wake process 0
            env.run_until(SimTime::from_millis(150)).unwrap();
            assert_eq!(env.process_manager().time_waiters_count(), 2);

            // Advance time to 250ms - should wake process 1
            env.run_until(SimTime::from_millis(250)).unwrap();
            assert_eq!(env.process_manager().time_waiters_count(), 1);

            // Advance time to 350ms - should wake process 2
            env.run_until(SimTime::from_millis(350)).unwrap();
            assert_eq!(env.process_manager().time_waiters_count(), 0);
        }

        #[test]
        fn test_e2e_mixed_events_and_time_delays() {
            let mut env = Environment::new();

            // Schedule some events
            let event1 = env.schedule(
                Duration::from_millis(100),
                EventPayload::Generic,
            );
            let event2 = env.schedule(
                Duration::from_millis(300),
                EventPayload::Generic,
            );

            // Mix of event waiters and time waiters
            env.process_manager_mut()
                .wait_for_event(ProcessId(0), event1);
            env.process_manager_mut()
                .wait_for_time(ProcessId(1), SimTime::from_millis(150));
            env.process_manager_mut()
                .wait_for_time(ProcessId(2), SimTime::from_millis(200));
            env.process_manager_mut()
                .wait_for_event(ProcessId(3), event2);

            // Run until 175ms
            env.run_until(SimTime::from_millis(175)).unwrap();

            // Process 0 should be woken (event at 100ms)
            assert!(!env.process_manager().has_event_waiter(event1));

            // Process 1 should be woken (time at 150ms)
            // Process 2 should still be waiting (time at 200ms)
            assert_eq!(env.process_manager().time_waiters_count(), 1);

            // Process 3 should still be waiting (event at 300ms)
            assert!(env.process_manager().has_event_waiter(event2));

            // Run to completion
            env.run().unwrap();

            // All processes should be woken
            assert_eq!(env.process_manager().time_waiters_count(), 0);
            assert!(!env.process_manager().has_event_waiter(event2));
        }

        #[test]
        fn test_e2e_multiple_processes_same_event() {
            let mut env = Environment::new();

            // Schedule a single event
            let event = env.schedule(
                Duration::from_millis(100),
                EventPayload::Generic,
            );

            // Multiple processes waiting for the same event
            env.process_manager_mut()
                .wait_for_event(ProcessId(0), event);
            env.process_manager_mut()
                .wait_for_event(ProcessId(1), event);
            env.process_manager_mut()
                .wait_for_event(ProcessId(2), event);

            assert!(env.process_manager().has_event_waiter(event));

            // Run the simulation
            env.run().unwrap();

            // All processes should be woken
            assert!(!env.process_manager().has_event_waiter(event));
        }

        #[test]
        fn test_e2e_multiple_processes_same_time() {
            let mut env = Environment::new();

            let target_time = SimTime::from_millis(100);

            // Multiple processes waiting for the same time
            env.process_manager_mut()
                .wait_for_time(ProcessId(0), target_time);
            env.process_manager_mut()
                .wait_for_time(ProcessId(1), target_time);
            env.process_manager_mut()
                .wait_for_time(ProcessId(2), target_time);

            assert_eq!(env.process_manager().time_waiters_count(), 3);

            // Advance time past the target
            env.run_until(SimTime::from_millis(150)).unwrap();

            // All processes should be woken
            assert_eq!(env.process_manager().time_waiters_count(), 0);
        }

        #[test]
        fn test_e2e_simulation_with_multiple_events_same_time() {
            let mut env = Environment::new();

            // Schedule multiple events at the same time
            let time = Duration::from_millis(100);
            let event1 = env.schedule(time, EventPayload::Generic);
            let event2 = env.schedule(time, EventPayload::Generic);
            let event3 = env.schedule(time, EventPayload::Generic);

            // Register processes for each event
            env.process_manager_mut()
                .wait_for_event(ProcessId(0), event1);
            env.process_manager_mut()
                .wait_for_event(ProcessId(1), event2);
            env.process_manager_mut()
                .wait_for_event(ProcessId(2), event3);

            // Run the simulation
            env.run().unwrap();

            // All events should be processed (order determined by sequence)
            assert!(!env.process_manager().has_event_waiter(event1));
            assert!(!env.process_manager().has_event_waiter(event2));
            assert!(!env.process_manager().has_event_waiter(event3));
        }

        #[test]
        fn test_e2e_long_running_simulation() {
            let mut env = Environment::new();

            // Schedule many events over a long time period
            for i in 0..100 {
                env.schedule(
                    Duration::from_millis(i * 10),
                    EventPayload::Generic,
                );
            }

            // Register processes at various time points
            for i in 0..50 {
                env.process_manager_mut()
                    .wait_for_time(ProcessId(i), SimTime::from_millis(i * 20));
            }

            let initial_waiters = env.process_manager().time_waiters_count();
            assert_eq!(initial_waiters, 50);

            // Run the entire simulation
            env.run().unwrap();

            // All time waiters should be woken
            assert_eq!(env.process_manager().time_waiters_count(), 0);

            // Time should have advanced to the last event
            assert_eq!(env.now(), SimTime::from_millis(990));
        }

        #[test]
        fn test_e2e_interleaved_events_and_processes() {
            let mut env = Environment::new();

            // Create a complex scenario with interleaved events and process waits
            let e1 = env.schedule(
                Duration::from_millis(50),
                EventPayload::Generic,
            );
            env.process_manager_mut()
                .wait_for_time(ProcessId(0), SimTime::from_millis(25));

            let e2 = env.schedule(
                Duration::from_millis(100),
                EventPayload::Generic,
            );
            env.process_manager_mut()
                .wait_for_event(ProcessId(1), e1);

            env.process_manager_mut()
                .wait_for_time(ProcessId(2), SimTime::from_millis(75));

            let e3 = env.schedule(
                Duration::from_millis(150),
                EventPayload::Generic,
            );
            env.process_manager_mut()
                .wait_for_event(ProcessId(3), e2);
            env.process_manager_mut()
                .wait_for_event(ProcessId(4), e3);

            // Run step by step
            env.run_until(SimTime::from_millis(30)).unwrap();
            assert_eq!(env.process_manager().time_waiters_count(), 1); // Process 2 still waiting

            env.run_until(SimTime::from_millis(60)).unwrap();
            assert!(!env.process_manager().has_event_waiter(e1)); // Process 1 woken

            env.run_until(SimTime::from_millis(80)).unwrap();
            assert_eq!(env.process_manager().time_waiters_count(), 0); // Process 2 woken

            env.run_until(SimTime::from_millis(110)).unwrap();
            assert!(!env.process_manager().has_event_waiter(e2)); // Process 3 woken

            env.run().unwrap();
            assert!(!env.process_manager().has_event_waiter(e3)); // Process 4 woken
        }

        #[test]
        fn test_e2e_empty_simulation() {
            let mut env = Environment::new();

            // No events, no processes
            env.run().unwrap();

            assert_eq!(env.now(), SimTime::zero());
            assert_eq!(env.process_manager().active_count(), 0);
        }

        #[test]
        fn test_e2e_simulation_with_only_processes() {
            let mut env = Environment::new();

            let p1 = SimpleProcess {
                name: "p1".to_string(),
            };
            let p2 = SimpleProcess {
                name: "p2".to_string(),
            };

            env.spawn_process(Box::pin(p1));
            env.spawn_process(Box::pin(p2));

            assert_eq!(env.process_manager().active_count(), 2);

            // Run with no events scheduled
            env.run().unwrap();

            // Time shouldn't advance
            assert_eq!(env.now(), SimTime::zero());
        }

        #[test]
        fn test_e2e_simulation_time_advancement() {
            let mut env = Environment::new();

            // Schedule events at specific times
            env.schedule(
                Duration::from_millis(100),
                EventPayload::Generic,
            );
            env.schedule(
                Duration::from_millis(200),
                EventPayload::Generic,
            );
            env.schedule(
                Duration::from_millis(300),
                EventPayload::Generic,
            );

            assert_eq!(env.now(), SimTime::zero());

            env.run_until(SimTime::from_millis(150)).unwrap();
            assert_eq!(env.now(), SimTime::from_millis(150));

            env.run_until(SimTime::from_millis(250)).unwrap();
            assert_eq!(env.now(), SimTime::from_millis(250));

            env.run().unwrap();
            assert_eq!(env.now(), SimTime::from_millis(300));
        }

        #[test]
        fn test_e2e_complex_multi_process_scenario() {
            let mut env = Environment::new();

            // Spawn several processes
            for i in 0..5 {
                let process = SimpleProcess {
                    name: format!("process_{}", i),
                };
                env.spawn_process(Box::pin(process));
            }

            assert_eq!(env.process_manager().active_count(), 5);

            // Schedule events at various times
            let e1 = env.schedule(
                Duration::from_millis(50),
                EventPayload::Generic,
            );
            let e2 = env.schedule(
                Duration::from_millis(100),
                EventPayload::Generic,
            );
            let e3 = env.schedule(
                Duration::from_millis(150),
                EventPayload::Generic,
            );

            // Set up process wait conditions
            env.process_manager_mut()
                .wait_for_event(ProcessId(0), e1);
            env.process_manager_mut()
                .wait_for_time(ProcessId(1), SimTime::from_millis(75));
            env.process_manager_mut()
                .wait_for_event(ProcessId(2), e2);
            env.process_manager_mut()
                .wait_for_time(ProcessId(3), SimTime::from_millis(125));
            env.process_manager_mut()
                .wait_for_event(ProcessId(4), e3);

            // Run the simulation
            env.run().unwrap();

            // All processes should have been woken
            assert_eq!(env.process_manager().time_waiters_count(), 0);
            assert!(!env.process_manager().has_event_waiter(e1));
            assert!(!env.process_manager().has_event_waiter(e2));
            assert!(!env.process_manager().has_event_waiter(e3));

            // Time should be at the last event
            assert_eq!(env.now(), SimTime::from_millis(150));
        }
    }
