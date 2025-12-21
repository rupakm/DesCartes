# des-components

Reusable simulation components for distributed systems modeling using the DES Framework.

## Overview

This crate provides composable building blocks for simulating distributed systems, including servers, clients, queues, throttles, and retry policies. It integrates with Tower middleware for realistic service simulation.

## Features

- **Component-based Architecture**: Reusable components with event-driven simulation
- **Task Interface**: Lightweight operations with automatic cleanup
- **Tower Integration**: Real Tower middleware in simulated environments
- **Queue Management**: FIFO and priority queues with capacity limits
- **Rate Limiting**: Token bucket and concurrency limiting
- **Circuit Breakers**: Failure detection and recovery patterns
- **Load Balancing**: Round-robin and random load balancing strategies
- **Comprehensive Testing**: Unit and integration tests for all components

## Quick Start

### Basic Client-Server Simulation

```rust
use des_components::{SimpleClient, Server, ClientEvent, ServerEvent};
use des_core::{Simulation, Execute, Executor, SimTime};
use des_core::task::PeriodicTask;
use std::time::Duration;

// Create simulation
let mut sim = Simulation::default();

// Create server with capacity 2 and 50ms service time
let server = Server::new("web-server".to_string(), 2, Duration::from_millis(50));
let server_id = sim.add_component(server);

// Create client that sends 5 requests every 100ms
let client = SimpleClient::new("web-client".to_string(), Duration::from_millis(100))
    .with_max_requests(5);
let client_id = sim.add_component(client);

// Start periodic request generation using Task interface
let task = PeriodicTask::with_count(
    move |scheduler| {
        scheduler.schedule_now(client_id, ClientEvent::SendRequest);
    },
    SimTime::from_duration(Duration::from_millis(100)),
    5,
);
sim.scheduler.schedule_task(SimTime::zero(), task);

// Run simulation
Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
```

### Tower Service Integration

```rust
use des_components::{DesServiceBuilder, DesTimeoutLayer, DesCircuitBreakerLayer};
use tower::{ServiceBuilder, Service};
use http::Request;
use std::sync::{Arc, Mutex};

// Create simulation
let simulation = Arc::new(Mutex::new(Simulation::default()));

// Build DES-backed service
let service = DesServiceBuilder::new("api-server".to_string())
    .thread_capacity(10)
    .service_time(Duration::from_millis(100))
    .build(simulation.clone())
    .unwrap();

// Add Tower middleware layers
let service = ServiceBuilder::new()
    .layer(DesTimeoutLayer::new(
        Duration::from_millis(500),
        Arc::downgrade(&simulation)
    ))
    .layer(DesCircuitBreakerLayer::new(
        3, // failure threshold
        Duration::from_secs(1), // recovery timeout
        Arc::downgrade(&simulation)
    ))
    .service(service);

// Use the service in async code
// The simulation runs deterministically in the background
```

## Task Interface

The Task interface provides a lightweight alternative to Components for short-lived operations:

### TimeoutTask

```rust
use des_core::task::TimeoutTask;

let timeout_task = TimeoutTask::new(move |scheduler| {
    println!("Timeout fired at {:?}", scheduler.time());
});

scheduler.schedule_task(
    SimTime::from_duration(Duration::from_millis(100)),
    timeout_task
);
```

### PeriodicTask

```rust
use des_core::task::PeriodicTask;

// Run every 50ms, exactly 10 times
let periodic_task = PeriodicTask::with_count(
    move |scheduler| {
        println!("Periodic execution at {:?}", scheduler.time());
    },
    SimTime::from_duration(Duration::from_millis(50)),
    10
);

scheduler.schedule_task(SimTime::zero(), periodic_task);
```

### ClosureTask

```rust
use des_core::task::ClosureTask;

let closure_task = ClosureTask::new(move |scheduler| {
    println!("One-shot execution at {:?}", scheduler.time());
});

scheduler.schedule_task(SimTime::zero(), closure_task);
```

## Components

### Server

Simulates a server with configurable capacity and service time:

```rust
use des_components::{Server, FifoQueue};

let server = Server::new("web-server".to_string(), 5, Duration::from_millis(100))
    .with_queue(Box::new(FifoQueue::bounded(20)));
```

### SimpleClient

Generates requests at regular intervals:

```rust
use des_components::SimpleClient;

let client = SimpleClient::new("client".to_string(), Duration::from_millis(200))
    .with_max_requests(100);
```

### Queues

FIFO and priority queues with capacity management:

```rust
use des_components::{FifoQueue, PriorityQueue};

// FIFO queue with capacity 50
let fifo = FifoQueue::bounded(50);

// Priority queue (lower numbers = higher priority)
let priority = PriorityQueue::bounded(50);
```

## Tower Middleware

### Rate Limiting

```rust
use des_components::DesRateLimit;

let rate_limited_service = DesRateLimit::new(
    base_service,
    10.0, // 10 requests per second
    20,   // burst capacity
    Arc::downgrade(&simulation)
);
```

### Concurrency Limiting

```rust
use des_components::DesConcurrencyLimit;

let concurrency_limited_service = DesConcurrencyLimit::new(
    base_service,
    5 // max 5 concurrent requests
);
```

### Circuit Breaker

```rust
use des_components::DesCircuitBreaker;

let circuit_breaker_service = DesCircuitBreaker::new(
    base_service,
    3, // failure threshold
    Duration::from_secs(30), // recovery timeout
    Arc::downgrade(&simulation)
);
```

### Timeout

```rust
use des_components::DesTimeout;

let timeout_service = DesTimeout::new(
    base_service,
    Duration::from_millis(1000),
    Arc::downgrade(&simulation)
);
```

## Testing

The crate includes comprehensive tests demonstrating usage patterns:

```bash
# Run all tests
cargo test --package des-components

# Run specific test suites
cargo test --package des-components --test task_implementations
cargo test --package des-components --test simple_integration
cargo test --package des-components --test tower_exponential_traffic
```

## Examples

See the `tests/` directory for comprehensive examples:

- `simple_integration.rs`: Basic client-server interactions
- `client_server_communication.rs`: Direct component communication
- `tower_exponential_traffic.rs`: Complex traffic patterns with Tower middleware
- `task_implementations.rs`: Task interface usage and testing

## Architecture

```
des-components/
├── src/
│   ├── lib.rs              # Public API exports
│   ├── simple_client.rs    # Basic client component
│   ├── server.rs           # Server component with queuing
│   ├── queue.rs            # FIFO and priority queues
│   ├── request.rs          # Request/response types
│   ├── builder.rs          # Configuration builders
│   ├── error.rs            # Error types
│   └── tower/              # Tower integration
│       ├── mod.rs          # Tower service exports
│       ├── service/        # DES-backed Tower services
│       ├── timeout/        # Timeout middleware
│       ├── circuit_breaker/ # Circuit breaker middleware
│       ├── limit/          # Rate and concurrency limiting
│       └── load_balancer/  # Load balancing strategies
├── tests/                  # Integration tests
└── TASK_MIGRATION_GUIDE.md # Migration documentation
```

## Contributing

When adding new components or features:

1. Follow the Component trait for stateful components
2. Use the Task interface for short-lived operations
3. Add comprehensive tests in the `tests/` directory
4. Update documentation and examples
5. Run `cargo clippy` to check for lint issues

## License

Licensed under the same terms as the parent project (MIT OR Apache-2.0).