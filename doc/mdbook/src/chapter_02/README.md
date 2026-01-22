# Chapter 2: DES-Core - The Core Abstractions

This chapter provides a comprehensive guide to the core abstractions that power DESCARTES. Understanding these fundamentals is essential for building effective discrete event simulations.

## What You'll Learn

By the end of this chapter, you'll understand:

- The five core abstractions: `Simulation`, `Component`, `Scheduler`, `SimTime`, and `Task`
- How components process events and maintain state
- The component lifecycle and event processing model
- How the task system complements components for short-lived operations
- Thread safety and concurrent access patterns

## Core Architecture Overview

DESCARTES is built around five fundamental abstractions:

```
┌─────────────────────────────────────────────────────────────┐
│                        Simulation                           │
│  ┌─────────────────┐    ┌─────────────────┐                │
│  │   Components    │    │    Scheduler    │                │
│  │                 │    │                 │                │
│  │  ┌───────────┐  │    │  ┌───────────┐  │                │
│  │  │Component A│  │    │  │Event Queue│  │                │
│  │  │Component B│  │    │  │Task Queue │  │                │
│  │  │Component C│  │    │  │SimTime    │  │                │
│  │  └───────────┘  │    │  └───────────┘  │                │
│  └─────────────────┘    └─────────────────┘                │
└─────────────────────────────────────────────────────────────┘
```

### The Five Core Abstractions

1. **`Simulation`**: The main container that orchestrates everything
2. **`Component`**: Stateful entities that process events
3. **`Scheduler`**: Manages event ordering and time advancement
4. **`SimTime`**: Represents simulation time with nanosecond precision
5. **`Task`**: Lightweight operations for short-lived work

## Prerequisites

Before diving into this chapter:
- Complete [Chapter 1: Basics of DES Simulation](../chapter_01/README.md)
- Understand basic Rust concepts (traits, generics, ownership)
- Familiarity with event-driven programming concepts

## Chapter Contents

This chapter covers each core abstraction in detail:

### [Simulation: The Main Container](#simulation)
- Creating and configuring simulations
- Adding and managing components
- Execution models and lifecycle
- Thread-safe scheduler handles

### [Component: Stateful Event Processors](#component)
- The Component trait and event processing
- Component lifecycle and state management
- Event types and type safety
- Inter-component communication

### [Scheduler: Event Ordering and Time](#scheduler)
- Event scheduling and time advancement
- Priority queues and deterministic ordering
- Deferred wakes and async integration
- Clock references for time reading

### [SimTime: Precise Time Management](#simtime)
- Nanosecond precision time representation
- Time arithmetic and conversions
- Simulation vs wall-clock time
- Time-based calculations

### [Task: Lightweight Operations](#task)
- Task trait and execution model
- Comparison with Components
- Built-in task types (timeouts, periodic, retry)
- Task handles and result retrieval

## Learning Path

**For beginners:**
1. Start with [SimTime](#simtime) to understand time representation
2. Learn [Component](#component) basics with simple examples
3. Understand [Simulation](#simulation) as the orchestrator
4. Explore [Scheduler](#scheduler) for advanced scheduling
5. Use [Task](#task) for lightweight operations

**For experienced developers:**
1. Review [Component](#component) for DESCARTES-specific patterns
2. Focus on [Scheduler](#scheduler) for advanced scheduling features
3. Understand [Task](#task) system for performance optimization
4. Study thread safety patterns in [Simulation](#simulation)

## Example: Putting It All Together

Here's a complete example showing all core abstractions working together:

```rust
use des_core::{
    Component, Simulation, SimTime, Scheduler, Task, TaskHandle,
    Execute, Executor, Key
};
use std::time::Duration;

// Define events for our component
#[derive(Debug)]
enum TimerEvent {
    Start,
    Tick,
    Stop,
}

// A simple timer component
struct Timer {
    name: String,
    tick_count: u32,
    max_ticks: u32,
    tick_interval: Duration,
}

impl Timer {
    fn new(name: String, max_ticks: u32, tick_interval: Duration) -> Self {
        Self {
            name,
            tick_count: 0,
            max_ticks,
            tick_interval,
        }
    }
}

impl Component for Timer {
    type Event = TimerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            TimerEvent::Start => {
                println!("[{}] Timer {} starting", 
                         scheduler.time(), self.name);
                
                // Schedule first tick
                scheduler.schedule(
                    SimTime::from_duration(self.tick_interval),
                    self_id,
                    TimerEvent::Tick,
                );
            }
            TimerEvent::Tick => {
                self.tick_count += 1;
                println!("[{}] Timer {} tick #{}", 
                         scheduler.time(), self.name, self.tick_count);
                
                if self.tick_count < self.max_ticks {
                    // Schedule next tick
                    scheduler.schedule(
                        SimTime::from_duration(self.tick_interval),
                        self_id,
                        TimerEvent::Tick,
                    );
                } else {
                    // Schedule stop event
                    scheduler.schedule_now(self_id, TimerEvent::Stop);
                }
            }
            TimerEvent::Stop => {
                println!("[{}] Timer {} stopped after {} ticks", 
                         scheduler.time(), self.name, self.tick_count);
            }
        }
    }
}

// A simple task for cleanup
struct CleanupTask {
    message: String,
}

impl Task for CleanupTask {
    type Output = ();

    fn execute(self, scheduler: &mut Scheduler) -> Self::Output {
        println!("[{}] Cleanup: {}", scheduler.time(), self.message);
    }
}

fn main() {
    // Create simulation with default configuration
    let mut simulation = Simulation::default();
    
    // Create and add timer component
    let timer = Timer::new(
        "MainTimer".to_string(),
        5, // 5 ticks
        Duration::from_millis(500), // Every 500ms
    );
    let timer_key = simulation.add_component(timer);
    
    // Start the timer
    simulation.schedule(SimTime::zero(), timer_key, TimerEvent::Start);
    
    // Schedule a cleanup task for later
    let cleanup_task = CleanupTask {
        message: "Simulation completed".to_string(),
    };
    let _cleanup_handle = simulation.schedule_task(
        SimTime::from_secs(3),
        cleanup_task,
    );
    
    // Run the simulation for 4 seconds
    println!("🚀 Starting simulation...");
    simulation.execute(Executor::timed(SimTime::from_secs(4)));
    
    // Get final state
    let final_timer = simulation.remove_component::<TimerEvent, Timer>(timer_key).unwrap();
    println!("✅ Simulation completed. Final tick count: {}", final_timer.tick_count);
}
```

This example demonstrates:
- **`Simulation`**: Creates and orchestrates the entire simulation
- **`Component`**: The `Timer` processes events and maintains state
- **`SimTime`**: Used for scheduling and time calculations
- **`Scheduler`**: Manages event ordering (accessed via `scheduler` parameter)
- **`Task`**: The `CleanupTask` performs a simple operation

## Key Design Principles

### Type Safety
DESCARTES uses Rust's type system to prevent common simulation errors:

```rust
// Events are strongly typed per component
enum ServerEvent { ProcessRequest, Shutdown }
enum ClientEvent { SendRequest, ReceiveResponse }

// Keys are typed to prevent sending wrong events
let server_key: Key<ServerEvent> = simulation.add_component(server);
let client_key: Key<ClientEvent> = simulation.add_component(client);

// This won't compile - type mismatch!
// simulation.schedule(SimTime::zero(), server_key, ClientEvent::SendRequest);
```

### Deterministic Execution
All randomness and timing is controlled by simulation time:

```rust
// Always use SimTime, never std::time::Instant
let delay = SimTime::from_millis(100);
scheduler.schedule(delay, component_key, MyEvent::Process);

// Use simulation-controlled randomness
let seed = simulation.config().seed;
let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
```

### Memory Safety
Components are owned by the simulation and accessed safely:

```rust
// Components are stored in type-erased containers
let key = simulation.add_component(my_component);

// Safe removal at simulation end
let final_component = simulation.remove_component::<MyEvent, MyComponent>(key);
```

## Performance Considerations

### Component vs Task Trade-offs

**Use Components when:**
- You need persistent state
- Multiple event types
- Complex lifecycle management
- Inter-component communication

**Use Tasks when:**
- One-time operations
- Simple callbacks
- Timeouts and delays
- No persistent state needed

### Memory Usage

```rust
// Components stay in memory for simulation lifetime
let server = Server::new("web_server", 100); // Stays allocated
let server_key = simulation.add_component(server);

// Tasks are cleaned up after execution
simulation.schedule_task(SimTime::from_secs(1), || {
    println!("This task will be cleaned up after execution");
});
```

## Thread Safety

DESCARTES provides thread-safe access patterns:

```rust
// Get a thread-safe scheduler handle
let scheduler_handle = simulation.scheduler_handle();

// Can be cloned and passed to other threads/components
let scheduler_clone = scheduler_handle.clone();

// Thread-safe scheduling
scheduler_handle.schedule(SimTime::from_millis(100), component_key, MyEvent::Tick);

// Lock-free time reading
let clock = simulation.clock();
let current_time = clock.time(); // No locks required
```

## Next Steps

Now that you understand the core abstractions:

1. **Practice**: Modify the example above to add more components
2. **Explore**: Read [Chapter 3: DES-Components](../chapter_03/README.md) for pre-built components
3. **Build**: Create your own components using these patterns
4. **Optimize**: Use tasks for simple operations to improve performance

The core abstractions provide a solid foundation for building any discrete event simulation. Master these concepts, and you'll be able to model complex systems effectively with DESCARTES.

---

*Ready to dive deeper? Let's explore each abstraction in detail...*