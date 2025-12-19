# DES Async Runtime Design

## Overview

This document describes the design of an async runtime built on top of the discrete event simulation (DES) scheduler. The runtime enables ergonomic async/await syntax while maintaining DES semantics - all timing goes through simulation time, not real time.

## Key Insight

Rust's async/await is a state machine transformation - the executor controls when futures are polled. By building a custom executor that uses `SimTime` and schedules wake events through the DES scheduler, we get async ergonomics with deterministic simulation behavior.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         DES Scheduler                            │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │ Event Queue (ordered by SimTime)                         │    │
│  │  [t=10: TimerFired(1)] [t=20: TimerFired(2)] ...        │    │
│  └─────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│                      DesRuntime Component                        │
│                                                                  │
│  AsyncContext (thread-local during polling):                    │
│  - current_time: SimTime                                        │
│  - pending_timers: Vec<(SimTime, TaskId)>                       │
│  - pending_wakes: Vec<TaskId>                                   │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │
│  │   Task 1     │  │   Task 2     │  │   Task 3     │          │
│  │ Pin<Future>  │  │ Pin<Future>  │  │ Pin<Future>  │          │
│  └──────────────┘  └──────────────┘  └──────────────┘          │
│                                                                  │
│  ready_queue: [TaskId]                                          │
└─────────────────────────────────────────────────────────────────┘
```

## Components

### 1. AsyncContext

Created fresh for each polling cycle. Stored in thread-local during task polling so futures can access it. Contains:

- Current simulation time for futures
- Collection point for timer registrations
- Collection point for immediate wake requests

```rust
pub struct AsyncContext {
    pub current_time: SimTime,
    pub pending_timers: Vec<(SimTime, TaskId)>,
    pub pending_wakes: Vec<TaskId>,
}
```

### 2. Thread-Local Context Access

During polling, the context is stored in thread-local storage. This allows futures to access simulation time and register timers without unsafe pointer manipulation:

```rust
thread_local! {
    static CURRENT_CONTEXT: RefCell<Option<*mut AsyncContext>> = RefCell::new(None);
    static CURRENT_TASK_ID: RefCell<Option<TaskId>> = RefCell::new(None);
}

fn with_async_context<F, R>(context: &mut AsyncContext, task_id: TaskId, f: F) -> R
where F: FnOnce() -> R
{
    // Set thread-locals, run closure, clear thread-locals
}
```

### 3. TaskId

Unique identifier for async tasks.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub u64);
```

### 4. Task

A suspended async task containing a pinned future.

```rust
struct Task {
    future: Pin<Box<dyn Future<Output = ()>>>,
}
```

### 5. RuntimeEvent

Events that drive the async runtime:

```rust
pub enum RuntimeEvent {
    Poll,                              // Poll all ready tasks
    TimerFired { task_id: TaskId },    // A timer fired
    ExternalWake { task_id: TaskId },  // External component woke a task
    Spawn { future: Pin<Box<...>> },   // Spawn a new task
}
```

### 6. DesRuntime

The async runtime component that manages tasks:

```rust
pub struct DesRuntime {
    next_task_id: u64,
    tasks: HashMap<TaskId, Task>,
    ready_queue: VecDeque<TaskId>,
}
```

### 7. SimSleep

A future that completes after simulated time elapses:

```rust
pub struct SimSleep {
    duration: Duration,
    target_time: Option<SimTime>,
    registered: bool,
}
```

## Event Flow

### Spawning a Task

1. User calls `runtime.spawn(future)`
2. Runtime creates Task with new TaskId
3. Runtime adds TaskId to ready_queue
4. User schedules `RuntimeEvent::Poll` at current time

### Polling Tasks

1. DES delivers `RuntimeEvent::Poll` to runtime
2. Runtime creates fresh `AsyncContext`
3. For each task in ready_queue:
   - Set thread-local context and task_id
   - Create waker and poll the future
   - Clear thread-local context
   - If Ready: remove task
   - If Pending: future registered its wake condition
4. Schedule collected timer events to DES scheduler
5. If pending_wakes not empty, schedule another Poll

### Timer Registration (sim_sleep)

1. Future is polled, calls `sim_sleep(duration)`
2. SimSleep reads current_time from thread-local context
3. SimSleep calculates target_time = current_time + duration
4. SimSleep calls `register_timer_internal(target_time, task_id)`
5. Context collects the timer registration
6. SimSleep returns Poll::Pending

### Timer Firing

1. DES delivers `RuntimeEvent::TimerFired { task_id }` at target_time
2. Runtime adds task_id to ready_queue
3. Runtime polls the task
4. SimSleep sees current_time >= target_time, returns Poll::Ready
5. Future continues execution

## Waker Design

The waker carries only the task ID. Context access happens through thread-locals:

```rust
struct WakerData {
    task_id: TaskId,
}
```

When `wake()` is called, it registers the task for immediate wake via the thread-local context.

## Safety Considerations

1. **Thread-local lifetime**: Context pointer is only valid during polling, cleared immediately after
2. **Single-threaded**: DES is single-threaded, so thread-locals are safe
3. **Determinism**: All wake events go through DES scheduler with explicit times
4. **No Send/Sync**: Futures don't need to be Send/Sync

## Usage Example

```rust
use des_core::{Simulation, Executor, SimTime};
use des_core::async_runtime::{DesRuntime, RuntimeEvent, sim_sleep};
use std::time::Duration;

async fn client_process() {
    loop {
        sim_sleep(Duration::from_millis(100)).await;
        println!("Client tick!");
    }
}

fn main() {
    let mut sim = Simulation::default();
    
    let mut runtime = DesRuntime::new();
    runtime.spawn(client_process());
    let runtime_id = sim.add_component(runtime);
    
    // Schedule initial poll
    sim.schedule(SimTime::zero(), runtime_id, RuntimeEvent::Poll);
    
    // Run simulation
    Executor::timed(SimTime::from_secs(10)).execute(&mut sim);
}
```

## Benefits

1. **Ergonomic**: Natural async/await syntax
2. **Deterministic**: All timing through DES scheduler
3. **Composable**: Futures compose naturally
4. **Familiar**: Rust developers already know async/await
5. **Efficient**: State machine transformation, no real threads
6. **Safe**: Thread-locals avoid unsafe pointer manipulation in wakers

## Trade-offs

1. **Thread-locals**: Required for context access, but safe in single-threaded DES
2. **Not truly async**: This is simulated async, not real I/O async
3. **Single runtime**: One DesRuntime per simulation (could be extended)
