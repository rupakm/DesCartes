//! Integration tests for the Task system

use des_core::{Simulation, Execute, Executor, SimTime, Task, TaskHandle};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn test_task_execution_in_simulation() {
    let mut sim = Simulation::default();
    
    let executed = Arc::new(Mutex::new(false));
    let executed_clone = executed.clone();
    
    // Schedule a task to execute after 100ms
    let _handle: TaskHandle<()> = sim.scheduler.schedule_closure(
        SimTime::from_duration(Duration::from_millis(100)),
        move |_scheduler| {
            *executed_clone.lock().unwrap() = true;
        }
    );
    
    // Run simulation for 200ms
    Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);
    
    // Task should have executed
    assert!(*executed.lock().unwrap());
}

#[test]
fn test_timeout_task_in_simulation() {
    let mut sim = Simulation::default();
    
    let timeout_fired = Arc::new(Mutex::new(false));
    let timeout_clone = timeout_fired.clone();
    
    // Schedule a timeout
    let _handle = sim.scheduler.timeout(
        SimTime::from_duration(Duration::from_millis(50)),
        move |_scheduler| {
            *timeout_clone.lock().unwrap() = true;
        }
    );
    
    // Run simulation for 100ms
    Executor::timed(SimTime::from_duration(Duration::from_millis(100))).execute(&mut sim);
    
    // Timeout should have fired
    assert!(*timeout_fired.lock().unwrap());
}

#[test]
fn test_task_cancellation() {
    let mut sim = Simulation::default();
    
    let executed = Arc::new(Mutex::new(false));
    let executed_clone = executed.clone();
    
    // Schedule a task
    let handle = sim.scheduler.schedule_closure(
        SimTime::from_duration(Duration::from_millis(100)),
        move |_scheduler| {
            *executed_clone.lock().unwrap() = true;
        }
    );
    
    // Cancel the task before it executes
    let cancelled = sim.scheduler.cancel_task(handle);
    assert!(cancelled);
    
    // Run simulation for 200ms
    Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);
    
    // Task should not have executed
    assert!(!*executed.lock().unwrap());
}

#[test]
fn test_multiple_tasks_execution_order() {
    let mut sim = Simulation::default();
    
    let execution_order = Arc::new(Mutex::new(Vec::new()));
    
    // Schedule tasks at different times
    let order1 = execution_order.clone();
    sim.scheduler.schedule_closure(
        SimTime::from_duration(Duration::from_millis(100)),
        move |_scheduler| {
            order1.lock().unwrap().push(1);
        }
    );
    
    let order2 = execution_order.clone();
    sim.scheduler.schedule_closure(
        SimTime::from_duration(Duration::from_millis(50)),
        move |_scheduler| {
            order2.lock().unwrap().push(2);
        }
    );
    
    let order3 = execution_order.clone();
    sim.scheduler.schedule_closure(
        SimTime::from_duration(Duration::from_millis(150)),
        move |_scheduler| {
            order3.lock().unwrap().push(3);
        }
    );
    
    // Run simulation for 200ms
    Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);
    
    // Tasks should execute in time order: 2, 1, 3
    let order = execution_order.lock().unwrap();
    assert_eq!(*order, vec![2, 1, 3]);
}

#[test]
fn test_task_scheduling_from_within_task() {
    let mut sim = Simulation::default();
    
    let execution_count = Arc::new(Mutex::new(0));
    let count_clone = execution_count.clone();
    
    // Schedule a task that schedules another task
    sim.scheduler.schedule_closure(
        SimTime::from_duration(Duration::from_millis(50)),
        move |scheduler| {
            *count_clone.lock().unwrap() += 1;
            
            // Schedule another task from within this task
            let count_inner = count_clone.clone();
            scheduler.schedule_closure(
                SimTime::from_duration(Duration::from_millis(50)),
                move |_scheduler| {
                    *count_inner.lock().unwrap() += 1;
                }
            );
        }
    );
    
    // Run simulation for 200ms
    Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);
    
    // Both tasks should have executed
    assert_eq!(*execution_count.lock().unwrap(), 2);
}

#[test]
fn test_task_with_return_value() {
    let mut sim = Simulation::default();
    
    // Schedule a task that returns a value
    let handle = sim.scheduler.schedule_closure(
        SimTime::from_duration(Duration::from_millis(50)),
        |_scheduler| -> i32 {
            42
        }
    );
    
    // Run simulation for 100ms
    Executor::timed(SimTime::from_duration(Duration::from_millis(100))).execute(&mut sim);
    
    // Get the result
    let result = sim.scheduler.get_task_result(handle);
    assert_eq!(result, Some(42));
}

#[test]
fn test_custom_task_implementation() {
    struct CounterTask {
        count: i32,
    }
    
    impl Task for CounterTask {
        type Output = i32;
        
        fn execute(self, _scheduler: &mut des_core::Scheduler) -> Self::Output {
            self.count * 2
        }
    }
    
    let mut sim = Simulation::default();
    
    let task = CounterTask { count: 21 };
    let handle = sim.scheduler.schedule_task(
        SimTime::from_duration(Duration::from_millis(50)),
        task
    );
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_millis(100))).execute(&mut sim);
    
    // Check result
    let result = sim.scheduler.get_task_result(handle);
    assert_eq!(result, Some(42));
}

#[test]
fn test_tasks_mixed_with_components() {
    use des_core::{Component, Key};
    
    #[derive(Debug)]
    enum TestEvent {
        Ping,
    }
    
    struct TestComponent {
        ping_count: i32,
    }
    
    impl Component for TestComponent {
        type Event = TestEvent;
        
        fn process_event(
            &mut self,
            _self_id: Key<Self::Event>,
            event: &Self::Event,
            scheduler: &mut des_core::Scheduler,
        ) {
            match event {
                TestEvent::Ping => {
                    self.ping_count += 1;
                    
                    // Schedule a task from within the component
                    scheduler.schedule_closure(
                        SimTime::from_duration(Duration::from_millis(25)),
                        |_scheduler| {
                            // Task executed from component
                        }
                    );
                }
            }
        }
    }
    
    let mut sim = Simulation::default();
    
    // Add component
    let component = TestComponent { ping_count: 0 };
    let component_key = sim.add_component(component);
    
    // Schedule component event
    sim.schedule(
        SimTime::from_duration(Duration::from_millis(50)),
        component_key,
        TestEvent::Ping,
    );
    
    let task_executed = Arc::new(Mutex::new(false));
    let task_clone = task_executed.clone();
    
    // Schedule a task
    sim.scheduler.schedule_closure(
        SimTime::from_duration(Duration::from_millis(100)),
        move |_scheduler| {
            *task_clone.lock().unwrap() = true;
        }
    );
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_millis(200))).execute(&mut sim);
    
    // Both component and task should have executed
    let final_component = sim.remove_component::<TestEvent, TestComponent>(component_key).unwrap();
    assert_eq!(final_component.ping_count, 1);
    assert!(*task_executed.lock().unwrap());
}