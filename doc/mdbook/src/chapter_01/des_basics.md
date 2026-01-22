# Basics of Discrete Event Simulation

Discrete Event Simulation (DES) is a powerful modeling technique for systems that evolve over time through a sequence of events. This chapter explains the fundamental concepts you need to understand before diving into DESCARTES implementation.

## What is Discrete Event Simulation?

**Discrete Event Simulation** models systems where the state changes only at specific points in time, called **events**. Unlike continuous simulation (where state changes continuously), DES focuses on the moments when something significant happens.

### Real-World Examples

- **Web Server**: Events include "request arrives", "request processing starts", "request completes"
- **Bank Queue**: Events include "customer arrives", "teller becomes available", "service completes"
- **Network Router**: Events include "packet arrives", "packet forwarded", "link becomes congested"
- **Manufacturing**: Events include "part arrives", "machine starts processing", "machine breaks down"

## Core DES Concepts

### Events

An **event** represents something that happens at a specific point in simulation time that changes the system state.

```rust
// Example: Server events in DESCARTES
#[derive(Debug)]
enum ServerEvent {
    ProcessRequest { request_id: u32 },    // Request arrives
    RequestCompleted { request_id: u32 },  // Processing finishes
    ServerOverloaded,                      // Server reaches capacity
}
```

**Key Properties:**
- Events happen **instantaneously** at a specific time
- Events can **schedule future events**
- Events **change system state**

### Simulation Time vs Wall-Clock Time

This is a crucial distinction that often confuses newcomers:

- **Simulation Time**: The time within your model (e.g., "request processed at 15.3 seconds")
- **Wall-Clock Time**: Real time it takes your computer to run the simulation

```rust
// Simulation time: Process request for 100ms in the model
scheduler.schedule(
    SimTime::from_duration(Duration::from_millis(100)),
    server_key,
    ServerEvent::RequestCompleted { request_id: 1 },
);

// Wall-clock time: This scheduling happens instantly in real time
```

**Example**: A 24-hour simulation of a web server might complete in 2 seconds of wall-clock time, but models a full day of server operation.

### Components

**Components** are the entities in your simulation that process events and maintain state.

```rust
// Example: A server component
struct Server {
    name: String,
    active_requests: u32,
    capacity: u32,
    total_processed: u64,
}

impl Component for Server {
    type Event = ServerEvent;
    
    fn process_event(&mut self, event: &ServerEvent, scheduler: &mut Scheduler) {
        // Handle events and update state
    }
}
```

**Key Properties:**
- Components **maintain state** (counters, queues, configurations)
- Components **process events** that are sent to them
- Components can **schedule future events** for themselves or other components

### Event Scheduling

The **scheduler** manages the timeline of events and ensures they execute in chronological order.

```rust
// Schedule an event to happen 500ms from now
scheduler.schedule(
    SimTime::from_duration(Duration::from_millis(500)),
    target_component,
    SomeEvent::ProcessData,
);

// Schedule an event to happen immediately (same simulation time)
scheduler.schedule_now(
    target_component,
    SomeEvent::UrgentAlert,
);
```

### Processes vs Events

In DES, we often think about **processes** (sequences of related events):

**Process**: "Handle HTTP Request"
1. **Event**: Request arrives → Server queues request
2. **Event**: Server starts processing → Server becomes busy
3. **Event**: Processing completes → Server sends response, becomes available

Each step is a separate event, but together they form a logical process.

## Key DES Terminology

### System State
The current condition of all components in your simulation at a given time.

```rust
// Example system state
struct SystemState {
    server_queue_length: usize,
    active_connections: u32,
    total_requests_processed: u64,
    current_time: SimTime,
}
```

### Event List (Future Event Set)
The scheduler's ordered list of all future events. DESCARTES manages this automatically.

### Simulation Clock
The current simulation time. Advances by jumping from event to event (not continuously).

### Entity
Objects that flow through the system (requests, customers, packets). In DESCARTES, these are often represented as data within events.

### Attributes
Properties of entities or components that affect behavior.

```rust
// Entity attributes (carried in events)
struct Request {
    id: u32,
    size_bytes: usize,
    priority: Priority,
    arrival_time: SimTime,
}

// Component attributes (part of component state)
struct Server {
    processing_speed: f64,  // requests per second
    max_queue_size: usize,
    failure_rate: f64,
}
```

### Activities
Operations that take time to complete. In DESCARTES, these are modeled as:
1. Start event → Begin activity, schedule end event
2. End event → Complete activity, update state

### Resources
Limited-capacity entities that components compete for (CPU cores, network bandwidth, memory).

## Simulation Time Mechanics

### Time Advancement

DES uses **next-event time advancement**:

1. Find the next scheduled event
2. Advance simulation clock to that event's time
3. Process the event (which may schedule more events)
4. Repeat until simulation ends

```rust
// Conceptual view of DESCARTES scheduler
while !event_list.is_empty() && current_time < end_time {
    let next_event = event_list.pop_earliest();
    current_time = next_event.time;
    next_event.component.process_event(next_event.data);
}
```

### No Time Between Events

**Important**: Nothing happens between events. If no events are scheduled between time 10.0 and 15.7, the simulation jumps directly from 10.0 to 15.7.

This is why DES is efficient for modeling systems with sporadic activity.

## DES vs Other Simulation Approaches

### Discrete Event vs Continuous Simulation

| Discrete Event | Continuous |
|----------------|------------|
| State changes at specific events | State changes continuously |
| Time jumps between events | Time advances in small steps |
| Good for: queues, networks, logistics | Good for: physics, chemical processes |
| Example: Web server handling requests | Example: Weather simulation |

### Discrete Event vs Agent-Based Modeling

| Discrete Event | Agent-Based |
|----------------|-------------|
| Focus on events and processes | Focus on individual agents |
| Global event scheduling | Agents act independently |
| Centralized time management | Distributed decision making |
| Good for: system-level behavior | Good for: emergent behavior |

### When to Use DES

DES is ideal when:
- ✅ System state changes at discrete points in time
- ✅ You can identify clear events that drive state changes
- ✅ You want to model queues, delays, and resource contention
- ✅ You need precise timing and ordering of events
- ✅ You're modeling computer systems, networks, or service processes

DES is less suitable when:
- ❌ State changes continuously (use continuous simulation)
- ❌ Spatial relationships are critical (consider agent-based modeling)
- ❌ You need real-time interaction (use real-time simulation)

## Common DES Patterns

### Producer-Consumer

```rust
// Producer generates events at regular intervals
scheduler.schedule(
    SimTime::from_duration(inter_arrival_time),
    producer_key,
    ProducerEvent::GenerateItem,
);

// Consumer processes items when available
scheduler.schedule_now(
    consumer_key,
    ConsumerEvent::ProcessItem { item },
);
```

### Request-Response

```rust
// Client sends request
scheduler.schedule_now(
    server_key,
    ServerEvent::ProcessRequest { request_id, client_key },
);

// Server processes and responds
scheduler.schedule(
    SimTime::from_duration(processing_time),
    client_key,
    ClientEvent::ResponseReceived { response },
);
```

### Resource Allocation

```rust
// Request resource
if resource.is_available() {
    resource.allocate();
    scheduler.schedule_now(self_key, MyEvent::StartWork);
} else {
    resource.add_to_queue(self_key);
}

// Release resource
resource.release();
if let Some(waiting_component) = resource.get_next_waiter() {
    scheduler.schedule_now(waiting_component, TheirEvent::ResourceAvailable);
}
```

## Randomness in DES

Most DES models include randomness to represent uncertainty:

```rust
use rand_distr::{Exp, Distribution};

// Exponential inter-arrival times (Poisson process)
let arrival_dist = Exp::new(arrival_rate).unwrap();
let next_arrival = arrival_dist.sample(&mut rng);

scheduler.schedule(
    SimTime::from_duration(Duration::from_secs_f64(next_arrival)),
    client_key,
    ClientEvent::SendRequest,
);
```

**Common Distributions:**
- **Exponential**: Inter-arrival times, service times
- **Normal**: Processing times with variation
- **Uniform**: Random delays, load balancing
- **Poisson**: Number of arrivals in a time period

## Validation and Verification

### Verification: "Are we building the simulation right?"
- Does the code correctly implement the model?
- Are events processed in the right order?
- Do components behave as specified?

### Validation: "Are we building the right simulation?"
- Does the model represent the real system accurately?
- Are the assumptions reasonable?
- Do results match real-world observations?

## Next Steps

Now that you understand DES fundamentals:

1. **Try the examples**: Run the [Quick Start Guide](quick_start.md) to see these concepts in action
2. **Learn DESCARTES specifics**: Read about [DES-Core abstractions](../chapter_02/README.md)
3. **Explore metrics**: Understand how to [collect and analyze results](metrics_overview.md)
4. **Build models**: Start with [M/M/k queueing examples](../chapter_05/README.md)

The key insight is that DES lets you model complex systems by focusing on the events that matter, allowing you to simulate hours, days, or years of system behavior in seconds of computation time.