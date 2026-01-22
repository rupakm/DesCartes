# Core Abstractions with Examples

This section provides detailed documentation and working examples for each of the five core DESCARTES abstractions: `Simulation`, `Component`, `Scheduler`, `SimTime`, and `Task`.

## SimTime: Precise Time Management

`SimTime` represents a point in simulation time with nanosecond precision. It's the foundation of all timing in DESCARTES.

### Basic Usage

```rust
use des_core::SimTime;
use std::time::Duration;

// Create SimTime values
let start = SimTime::zero();                    // Simulation start
let t1 = SimTime::from_millis(500);            // 500 milliseconds
let t2 = SimTime::from_secs(2);                // 2 seconds
let t3 = SimTime::from_duration(Duration::from_micros(1500)); // From Duration

// Time arithmetic
let later = t1 + Duration::from_millis(200);   // 700ms
let elapsed = t2 - t1;                         // Duration: 1.5 seconds
let doubled = t1 * 2;                          // 1 second

// Conversions
let as_duration = t1.as_duration();            // Convert to Duration
let as_nanos = t1.as_nanos();                  // Raw nanoseconds: 500_000_000

// From floating point seconds
let t4 = SimTime::from(1.5);                   // 1.5 seconds
let t5 = SimTime::from(0.000001);              // 1 microsecond

println!("Times: {} {} {} {}", t1, t2, t3, t4);
// Output: Times: 500.000ms 2.000s 1.500ms 1.500s
```

### Time Calculations

```rust
use des_core::SimTime;
use std::time::Duration;

fn calculate_service_metrics() {
    let request_arrival = SimTime::from_millis(100);
    let service_start = SimTime::from_millis(150);
    let service_end = SimTime::from_millis(300);
    
    // Calculate waiting time
    let wait_time = service_start - request_arrival;
    println!("Wait time: {:.2}ms", wait_time.as_secs_f64() * 1000.0);
    
    // Calculate service time
    let service_time = service_end - service_start;
    println!("Service time: {:.2}ms", service_time.as_secs_f64() * 1000.0);
    
    // Calculate total response time
    let response_time = service_end - request_arrival;
    println!("Response time: {:.2}ms", response_time.as_secs_f64() * 1000.0);
    
    // Schedule follow-up at response_time + 1 second
    let follow_up_time = service_end + Duration::from_secs(1);
    println!("Follow-up scheduled for: {}", follow_up_time);
}
```

### Key Properties

- **Precision**: Nanosecond precision for accurate timing
- **Deterministic**: Same inputs always produce same outputs
- **Overflow Safe**: Uses saturating arithmetic to prevent panics
- **Ordered**: Implements `Ord` for use in priority queues

## Component: Stateful Event Processors

Components are the primary building blocks of DESCARTES simulations. They maintain state and process events.

### The Component Trait

```rust
use des_core::{Component, Key, Scheduler, SimTime};

pub trait Component {
    type Event: 'static;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    );
}
```

### Simple Component Example

```rust
use des_core::{Component, Key, Scheduler, SimTime, Simulation, Execute, Executor};
use std::time::Duration;

// Define events for a counter component
#[derive(Debug)]
enum CounterEvent {
    Increment,
    Decrement,
    Reset,
    PrintValue,
}

// Counter component with state
struct Counter {
    name: String,
    value: i32,
    max_value: i32,
}

impl Counter {
    fn new(name: String, max_value: i32) -> Self {
        Self {
            name,
            value: 0,
            max_value,
        }
    }
}

impl Component for Counter {
    type Event = CounterEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            CounterEvent::Increment => {
                if self.value < self.max_value {
                    self.value += 1;
                    println!("[{}] {} incremented to {}", 
                             scheduler.time(), self.name, self.value);
                    
                    // Schedule a print after increment
                    scheduler.schedule(
                        SimTime::from_millis(10),
                        self_id,
                        CounterEvent::PrintValue,
                    );
                } else {
                    println!("[{}] {} at maximum value {}", 
                             scheduler.time(), self.name, self.max_value);
                }
            }
            CounterEvent::Decrement => {
                if self.value > 0 {
                    self.value -= 1;
                    println!("[{}] {} decremented to {}", 
                             scheduler.time(), self.name, self.value);
                } else {
                    println!("[{}] {} already at minimum value 0", 
                             scheduler.time(), self.name);
                }
            }
            CounterEvent::Reset => {
                let old_value = self.value;
                self.value = 0;
                println!("[{}] {} reset from {} to 0", 
                         scheduler.time(), self.name, old_value);
            }
            CounterEvent::PrintValue => {
                println!("[{}] {} current value: {}", 
                         scheduler.time(), self.name, self.value);
            }
        }
    }
}

fn main() {
    let mut simulation = Simulation::default();
    
    // Create and add counter
    let counter = Counter::new("MainCounter".to_string(), 5);
    let counter_key = simulation.add_component(counter);
    
    // Schedule some events
    simulation.schedule(SimTime::from_millis(0), counter_key, CounterEvent::Increment);
    simulation.schedule(SimTime::from_millis(100), counter_key, CounterEvent::Increment);
    simulation.schedule(SimTime::from_millis(200), counter_key, CounterEvent::PrintValue);
    simulation.schedule(SimTime::from_millis(300), counter_key, CounterEvent::Decrement);
    simulation.schedule(SimTime::from_millis(400), counter_key, CounterEvent::Reset);
    
    // Run simulation
    simulation.execute(Executor::timed(SimTime::from_millis(500)));
    
    // Get final state
    let final_counter = simulation.remove_component::<CounterEvent, Counter>(counter_key).unwrap();
    println!("Final counter value: {}", final_counter.value);
}
```

### Inter-Component Communication

```rust
use des_core::{Component, Key, Scheduler, SimTime, Simulation, Execute, Executor};

// Events for a producer component
#[derive(Debug)]
enum ProducerEvent {
    Produce,
}

// Events for a consumer component
#[derive(Debug)]
enum ConsumerEvent {
    Consume { item_id: u32, produced_at: SimTime },
}

// Producer component
struct Producer {
    name: String,
    consumer_key: Key<ConsumerEvent>,
    items_produced: u32,
    production_interval: Duration,
}

impl Producer {
    fn new(name: String, consumer_key: Key<ConsumerEvent>, production_interval: Duration) -> Self {
        Self {
            name,
            consumer_key,
            items_produced: 0,
            production_interval,
        }
    }
}

impl Component for Producer {
    type Event = ProducerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ProducerEvent::Produce => {
                self.items_produced += 1;
                let current_time = scheduler.time();
                
                println!("[{}] {} produced item #{}", 
                         current_time, self.name, self.items_produced);
                
                // Send item to consumer
                scheduler.schedule_now(
                    self.consumer_key,
                    ConsumerEvent::Consume {
                        item_id: self.items_produced,
                        produced_at: current_time,
                    },
                );
                
                // Schedule next production
                scheduler.schedule(
                    SimTime::from_duration(self.production_interval),
                    self_id,
                    ProducerEvent::Produce,
                );
            }
        }
    }
}

// Consumer component
struct Consumer {
    name: String,
    items_consumed: u32,
    total_latency: Duration,
}

impl Consumer {
    fn new(name: String) -> Self {
        Self {
            name,
            items_consumed: 0,
            total_latency: Duration::ZERO,
        }
    }
    
    fn average_latency(&self) -> Duration {
        if self.items_consumed > 0 {
            self.total_latency / self.items_consumed
        } else {
            Duration::ZERO
        }
    }
}

impl Component for Consumer {
    type Event = ConsumerEvent;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ConsumerEvent::Consume { item_id, produced_at } => {
                self.items_consumed += 1;
                let current_time = scheduler.time();
                let latency = current_time - *produced_at;
                self.total_latency += latency;
                
                println!("[{}] {} consumed item #{} (latency: {:.2}ms)", 
                         current_time, self.name, item_id,
                         latency.as_secs_f64() * 1000.0);
            }
        }
    }
}

fn main() {
    let mut simulation = Simulation::default();
    
    // Create consumer first (producer needs its key)
    let consumer = Consumer::new("ItemConsumer".to_string());
    let consumer_key = simulation.add_component(consumer);
    
    // Create producer with reference to consumer
    let producer = Producer::new(
        "ItemProducer".to_string(),
        consumer_key,
        Duration::from_millis(200), // Produce every 200ms
    );
    let producer_key = simulation.add_component(producer);
    
    // Start production
    simulation.schedule(SimTime::zero(), producer_key, ProducerEvent::Produce);
    
    // Run simulation for 1 second
    simulation.execute(Executor::timed(SimTime::from_secs(1)));
    
    // Get final statistics
    let final_consumer = simulation.remove_component::<ConsumerEvent, Consumer>(consumer_key).unwrap();
    println!("Items consumed: {}", final_consumer.items_consumed);
    println!("Average latency: {:.2}ms", final_consumer.average_latency().as_secs_f64() * 1000.0);
}
```

### Component Lifecycle

Components follow a predictable lifecycle:

1. **Creation**: Component is created with initial state
2. **Registration**: Added to simulation with `add_component()`
3. **Event Processing**: Processes events via `process_event()`
4. **State Updates**: Modifies internal state during event processing
5. **Scheduling**: Can schedule future events for self or other components
6. **Removal**: Removed from simulation with `remove_component()`

## Scheduler: Event Ordering and Time Management

The `Scheduler` manages the event queue and advances simulation time. It's accessed through the `scheduler` parameter in `process_event()`.

### Basic Scheduling Operations

```rust
use des_core::{Component, Key, Scheduler, SimTime};

impl Component for MyComponent {
    type Event = MyEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        // Schedule event for later
        scheduler.schedule(
            SimTime::from_millis(500),  // 500ms from now
            self_id,
            MyEvent::DelayedAction,
        );
        
        // Schedule event immediately (same simulation time)
        scheduler.schedule_now(self_id, MyEvent::ImmediateAction);
        
        // Get current simulation time
        let current_time = scheduler.time();
        println!("Processing event at {}", current_time);
        
        // Schedule event at absolute time
        let absolute_time = SimTime::from_secs(10);
        scheduler.schedule(absolute_time, self_id, MyEvent::AbsoluteTime);
    }
}
```

### SchedulerHandle for Thread-Safe Access

```rust
use des_core::{Simulation, SchedulerHandle, SimTime, Key};

fn setup_with_scheduler_handle() {
    let mut simulation = Simulation::default();
    
    // Get thread-safe scheduler handle
    let scheduler_handle: SchedulerHandle = simulation.scheduler_handle();
    
    // Can be cloned and passed around
    let scheduler_clone = scheduler_handle.clone();
    
    // Use in other contexts (like Tower middleware)
    schedule_periodic_task(scheduler_clone);
}

fn schedule_periodic_task(scheduler: SchedulerHandle) {
    // This could be called from anywhere, even other threads
    // scheduler.schedule(SimTime::from_secs(1), component_key, MyEvent::Periodic);
}
```

### Clock References for Time Reading

```rust
use des_core::{Simulation, SimTime};

fn time_reading_example() {
    let simulation = Simulation::default();
    
    // Get lock-free clock reference
    let clock = simulation.clock();
    
    // Read time without locks (very fast)
    let current_time = clock.time();
    println!("Current simulation time: {}", current_time);
    
    // Clock can be cloned and passed around
    let clock_clone = clock.clone();
    monitor_time(clock_clone);
}

fn monitor_time(clock: des_core::ClockRef) {
    // Fast, lock-free time reading
    let time = clock.time();
    println!("Monitored time: {}", time);
}
```

## Simulation: The Main Container

The `Simulation` struct orchestrates the entire simulation, managing components, scheduling, and execution.

### Basic Simulation Setup

```rust
use des_core::{Simulation, SimulationConfig, Execute, Executor, SimTime};

fn basic_simulation_example() {
    // Create with default configuration
    let mut simulation = Simulation::default();
    
    // Or create with custom configuration
    let config = SimulationConfig { seed: 12345 };
    let mut custom_simulation = Simulation::new(config);
    
    // Add components
    let component1 = MyComponent::new("Component1");
    let key1 = simulation.add_component(component1);
    
    let component2 = MyComponent::new("Component2");
    let key2 = simulation.add_component(component2);
    
    // Schedule initial events
    simulation.schedule(SimTime::zero(), key1, MyEvent::Start);
    simulation.schedule(SimTime::from_millis(100), key2, MyEvent::Start);
    
    // Run simulation
    simulation.execute(Executor::timed(SimTime::from_secs(5)));
    
    // Get final state
    let final_component1 = simulation.remove_component::<MyEvent, MyComponent>(key1);
    let final_component2 = simulation.remove_component::<MyEvent, MyComponent>(key2);
    
    println!("Simulation completed");
}
```

### Execution Models

```rust
use des_core::{Simulation, Execute, Executor, SimTime};

fn execution_models_example() {
    let mut simulation = Simulation::default();
    
    // Add components and schedule events...
    
    // 1. Timed execution - run for specific duration
    simulation.execute(Executor::timed(SimTime::from_secs(10)));
    
    // 2. Unbounded execution - run until no more events
    simulation.execute(Executor::unbound());
    
    // 3. Step-by-step execution
    while simulation.step() {
        // Process one event at a time
        println!("Processed event at {}", simulation.time());
        
        // Can add custom logic between steps
        if simulation.time() > SimTime::from_secs(5) {
            break;
        }
    }
    
    // 4. Check for pending events
    if simulation.has_pending_events() {
        println!("Simulation has more events to process");
    }
}
```

### Component Management

```rust
use des_core::{Simulation, Component, Key};

fn component_management_example() {
    let mut simulation = Simulation::default();
    
    // Add component
    let component = MyComponent::new("TestComponent");
    let key = simulation.add_component(component);
    
    // Get mutable access to component (during simulation)
    if let Some(component_ref) = simulation.get_component_mut::<MyEvent, MyComponent>(key) {
        component_ref.update_state();
    }
    
    // Remove component (usually at end)
    let final_component = simulation.remove_component::<MyEvent, MyComponent>(key);
    
    match final_component {
        Some(component) => {
            println!("Retrieved component: {}", component.name());
        }
        None => {
            println!("Component not found");
        }
    }
}
```

## Task: Lightweight Operations

Tasks provide a lightweight alternative to Components for simple, one-time operations.

### The Task Trait

```rust
use des_core::{Task, Scheduler, SimTime};

pub trait Task: 'static {
    type Output: 'static;
    
    fn execute(self, scheduler: &mut Scheduler) -> Self::Output;
}
```

### Simple Task Example

```rust
use des_core::{Task, Scheduler, SimTime, Simulation, TaskHandle};

// Simple cleanup task
struct CleanupTask {
    resource_name: String,
}

impl Task for CleanupTask {
    type Output = bool; // Returns success status

    fn execute(self, scheduler: &mut Scheduler) -> Self::Output {
        println!("[{}] Cleaning up resource: {}", 
                 scheduler.time(), self.resource_name);
        
        // Simulate cleanup work
        true // Success
    }
}

// Timeout task with callback
struct TimeoutTask<F> 
where 
    F: FnOnce(&mut Scheduler) + 'static,
{
    callback: F,
}

impl<F> Task for TimeoutTask<F>
where
    F: FnOnce(&mut Scheduler) + 'static,
{
    type Output = ();

    fn execute(self, scheduler: &mut Scheduler) -> Self::Output {
        (self.callback)(scheduler);
    }
}

fn task_examples() {
    let mut simulation = Simulation::default();
    
    // Schedule cleanup task
    let cleanup_task = CleanupTask {
        resource_name: "DatabaseConnection".to_string(),
    };
    let cleanup_handle = simulation.schedule_task(
        SimTime::from_secs(5),
        cleanup_task,
    );
    
    // Schedule timeout with callback
    let timeout_task = TimeoutTask {
        callback: |scheduler| {
            println!("[{}] Timeout fired!", scheduler.time());
        },
    };
    let timeout_handle = simulation.schedule_task(
        SimTime::from_secs(3),
        timeout_task,
    );
    
    // Run simulation
    simulation.execute(Executor::timed(SimTime::from_secs(6)));
    
    // Get task results
    if let Some(cleanup_result) = simulation.get_task_result(cleanup_handle) {
        println!("Cleanup successful: {}", cleanup_result);
    }
    
    // Timeout task returns (), so no result to check
}
```

### Built-in Task Types

DESCARTES provides several built-in task types:

```rust
use des_core::{Simulation, SimTime, PeriodicTask, TimeoutTask, RetryTask};

fn builtin_tasks_example() {
    let mut simulation = Simulation::default();
    
    // 1. Periodic task - runs multiple times
    let periodic_task = PeriodicTask::new(
        |scheduler| {
            println!("[{}] Periodic task executed", scheduler.time());
        },
        SimTime::from_millis(500), // Every 500ms
    );
    let periodic_handle = simulation.schedule_task(SimTime::zero(), periodic_task);
    
    // 2. Periodic task with limited count
    let limited_periodic = PeriodicTask::with_count(
        |scheduler| {
            println!("[{}] Limited periodic task", scheduler.time());
        },
        SimTime::from_millis(200), // Every 200ms
        5, // Only 5 times
    );
    simulation.schedule_task(SimTime::from_millis(100), limited_periodic);
    
    // 3. Simple timeout
    let timeout_handle = simulation.timeout(
        SimTime::from_secs(2),
        |scheduler| {
            println!("[{}] Timeout callback", scheduler.time());
        },
    );
    
    // 4. Closure task (one-time)
    let closure_handle = simulation.schedule_closure(
        SimTime::from_millis(750),
        |scheduler| {
            println!("[{}] Closure task executed", scheduler.time());
            "Task completed".to_string() // Return value
        },
    );
    
    // Run simulation
    simulation.execute(Executor::timed(SimTime::from_secs(3)));
    
    // Get results
    if let Some(result) = simulation.get_task_result(closure_handle) {
        println!("Closure result: {}", result);
    }
    
    // Cancel tasks if needed
    let cancelled = simulation.cancel_task(timeout_handle);
    println!("Timeout cancelled: {}", cancelled);
}
```

### Task vs Component Comparison

| Aspect | Component | Task |
|--------|-----------|------|
| **Lifetime** | Persistent throughout simulation | One-time execution |
| **State** | Maintains state between events | No persistent state |
| **Events** | Processes multiple event types | Single execution |
| **Memory** | Stays in memory | Cleaned up after execution |
| **Complexity** | Can be complex with multiple methods | Simple, single purpose |
| **Use Cases** | Servers, clients, long-lived entities | Timeouts, callbacks, cleanup |

## Advanced Patterns

### Component State Machines

```rust
use des_core::{Component, Key, Scheduler, SimTime};

#[derive(Debug, PartialEq)]
enum ServerState {
    Idle,
    Processing,
    Overloaded,
    Shutdown,
}

#[derive(Debug)]
enum ServerEvent {
    ProcessRequest { request_id: u32 },
    RequestCompleted { request_id: u32 },
    Overload,
    Shutdown,
}

struct StatefulServer {
    name: String,
    state: ServerState,
    active_requests: u32,
    max_capacity: u32,
}

impl Component for StatefulServer {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match (&self.state, event) {
            (ServerState::Idle, ServerEvent::ProcessRequest { request_id }) => {
                self.state = ServerState::Processing;
                self.active_requests += 1;
                
                println!("[{}] {} processing request {}", 
                         scheduler.time(), self.name, request_id);
                
                // Schedule completion
                scheduler.schedule(
                    SimTime::from_millis(100),
                    self_id,
                    ServerEvent::RequestCompleted { request_id: *request_id },
                );
            }
            (ServerState::Processing, ServerEvent::ProcessRequest { request_id }) => {
                if self.active_requests < self.max_capacity {
                    self.active_requests += 1;
                    
                    // Schedule completion
                    scheduler.schedule(
                        SimTime::from_millis(100),
                        self_id,
                        ServerEvent::RequestCompleted { request_id: *request_id },
                    );
                } else {
                    self.state = ServerState::Overloaded;
                    println!("[{}] {} overloaded, rejecting request {}", 
                             scheduler.time(), self.name, request_id);
                }
            }
            (_, ServerEvent::RequestCompleted { request_id }) => {
                self.active_requests -= 1;
                println!("[{}] {} completed request {}", 
                         scheduler.time(), self.name, request_id);
                
                // Update state based on load
                self.state = if self.active_requests == 0 {
                    ServerState::Idle
                } else if self.active_requests < self.max_capacity {
                    ServerState::Processing
                } else {
                    ServerState::Overloaded
                };
            }
            (_, ServerEvent::Shutdown) => {
                self.state = ServerState::Shutdown;
                println!("[{}] {} shutting down", scheduler.time(), self.name);
            }
            _ => {
                println!("[{}] {} ignoring {:?} in state {:?}", 
                         scheduler.time(), self.name, event, self.state);
            }
        }
    }
}
```

### Error Handling Patterns

```rust
use des_core::{Component, Key, Scheduler, SimTime};

#[derive(Debug)]
enum Result<T, E> {
    Ok(T),
    Err(E),
}

#[derive(Debug)]
enum ProcessingError {
    InvalidInput,
    ResourceUnavailable,
    Timeout,
}

#[derive(Debug)]
enum ProcessorEvent {
    ProcessData { data: String },
    ProcessingResult { result: Result<String, ProcessingError> },
    Retry { attempt: u32, data: String },
}

struct ErrorHandlingProcessor {
    name: String,
    max_retries: u32,
}

impl Component for ErrorHandlingProcessor {
    type Event = ProcessorEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ProcessorEvent::ProcessData { data } => {
                // Simulate processing that might fail
                let result = if data.is_empty() {
                    Result::Err(ProcessingError::InvalidInput)
                } else if data.len() > 100 {
                    Result::Err(ProcessingError::ResourceUnavailable)
                } else {
                    Result::Ok(format!("Processed: {}", data))
                };
                
                // Schedule result
                scheduler.schedule_now(
                    self_id,
                    ProcessorEvent::ProcessingResult { result },
                );
            }
            ProcessorEvent::ProcessingResult { result } => {
                match result {
                    Result::Ok(processed_data) => {
                        println!("[{}] {} success: {}", 
                                 scheduler.time(), self.name, processed_data);
                    }
                    Result::Err(error) => {
                        println!("[{}] {} error: {:?}", 
                                 scheduler.time(), self.name, error);
                        
                        // Could schedule retry logic here
                    }
                }
            }
            ProcessorEvent::Retry { attempt, data } => {
                if *attempt <= self.max_retries {
                    println!("[{}] {} retry attempt {} for: {}", 
                             scheduler.time(), self.name, attempt, data);
                    
                    // Schedule retry after delay
                    scheduler.schedule(
                        SimTime::from_millis(100 * (*attempt as u64)),
                        self_id,
                        ProcessorEvent::ProcessData { data: data.clone() },
                    );
                } else {
                    println!("[{}] {} max retries exceeded for: {}", 
                             scheduler.time(), self.name, data);
                }
            }
        }
    }
}
```

## Best Practices

### 1. Type Safety
```rust
// Use strongly typed events
#[derive(Debug)]
enum ServerEvent { ProcessRequest, Shutdown }

#[derive(Debug)]
enum ClientEvent { SendRequest, ReceiveResponse }

// Keys are typed to prevent mistakes
let server_key: Key<ServerEvent> = simulation.add_component(server);
let client_key: Key<ClientEvent> = simulation.add_component(client);
```

### 2. Deterministic Behavior
```rust
// Always use SimTime, never wall-clock time
let delay = SimTime::from_millis(100);
scheduler.schedule(delay, component_key, MyEvent::Process);

// Use simulation seed for reproducible randomness
let seed = simulation.config().seed;
let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
```

### 3. Resource Management
```rust
// Remove components at simulation end to inspect final state
let final_component = simulation.remove_component::<MyEvent, MyComponent>(key);

// Use tasks for cleanup operations
simulation.schedule_task(SimTime::from_secs(10), CleanupTask::new());
```

### 4. Performance Optimization
```rust
// Use tasks for simple operations
simulation.timeout(SimTime::from_secs(1), |_| println!("Timeout"));

// Use components for stateful, long-lived entities
let server = Server::new("web_server", 100);
let server_key = simulation.add_component(server);
```

## Summary

The five core abstractions work together to provide a powerful foundation for discrete event simulation:

- **`SimTime`**: Precise, deterministic time representation
- **`Component`**: Stateful entities that process events
- **`Scheduler`**: Event ordering and time advancement
- **`Simulation`**: Main container and orchestrator
- **`Task`**: Lightweight operations for simple work

Understanding these abstractions and their interactions is key to building effective simulations with DESCARTES. Each serves a specific purpose and together they provide the flexibility to model complex systems accurately and efficiently.

## Next Steps

Now that you understand the core abstractions:

1. **Practice**: Build components using these patterns
2. **Explore**: Learn about [DES-Components](../chapter_03/README.md) for pre-built components
3. **Optimize**: Use the right abstraction for each use case
4. **Scale**: Combine multiple components for complex systems

The core abstractions are your foundation - master them, and you can build any discrete event simulation!