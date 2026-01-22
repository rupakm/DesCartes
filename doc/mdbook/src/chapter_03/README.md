# Chapter 3: DES-Components: Components for Server Modeling

The `des-components` crate provides reusable, composable building blocks for simulating distributed systems. This chapter covers all available components, their configuration options, and practical usage patterns.

## Overview

DES-Components offers:

- **Server Components**: Simulated servers with configurable capacity, service times, and queuing
- **Client Components**: Request generators with retry policies and timeout handling  
- **Queue Implementations**: FIFO and priority-based queues with capacity management
- **Retry Policies**: Exponential backoff, token bucket, and success-based adaptive retries
- **Tower Integration**: Full Tower service abstraction support with middleware
- **Builder Pattern**: Fluent APIs for component configuration

## Component Architecture

All components implement the `Component` trait from `des-core`, making them event-driven and composable:

```rust
use des_components::{Server, SimpleClient, FifoQueue};
use des_core::{Simulation, SimTime};
use std::time::Duration;

let mut sim = Simulation::default();

// Create server with queue
let server = Server::with_constant_service_time(
    "web-server".to_string(),
    4, // thread capacity
    Duration::from_millis(100), // service time
).with_queue(Box::new(FifoQueue::bounded(50)));

let server_id = sim.add_component(server);
```

## Key Features

### Service Time Distributions
Components support various service time patterns:
- **Constant**: Fixed processing time
- **Exponential**: Exponentially distributed (M/M/k queues)
- **Request-dependent**: Service time based on payload size, endpoint, etc.

### Comprehensive Metrics
All components automatically collect metrics:
- Request counts, latencies, throughput
- Queue depths, utilization rates
- Success/failure rates, timeout counts

### Thread Safety
Components are designed for single-threaded discrete event simulation but provide thread-safe metrics collection for analysis.

## Chapter Contents

This chapter is organized into the following sections:

1. **[Server Components](server_components.md)** - Server implementations with queuing and capacity management
2. **[Client Components](client_components.md)** - Request generators with retry policies
3. **[Queue Systems](queue_systems.md)** - FIFO and priority queue implementations
4. **[Retry Policies](retry_policies.md)** - Configurable retry strategies for fault tolerance
5. **[Component Composition](component_composition.md)** - Patterns for building complex systems
6. **[Tower Integration](tower_integration.md)** - Using Tower services in simulations

Each section includes working code examples, configuration options, and best practices for building realistic distributed system simulations.