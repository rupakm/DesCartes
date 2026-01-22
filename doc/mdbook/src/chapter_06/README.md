# Chapter 6: Tower Integration

This chapter covers DESCARTES' integration with the Tower ecosystem, enabling you to use familiar Tower middleware patterns in discrete event simulations with deterministic, reproducible behavior.

## What You'll Learn

- How Tower services work within DES simulations
- Available middleware layers and their configurations
- Building multi-tier service architectures
- Performance considerations and best practices

## Chapter Contents

1. **[Tower Service Abstraction](./tower_services.md)** - Understanding Tower integration with simulation time
2. **[Middleware Examples](./middleware_examples.md)** - Timeout, retry, rate limiting, and circuit breaker patterns
3. **[Multi-Tier Architectures](./multi_tier_architectures.md)** - Building complex service topologies

## Quick Overview

DESCARTES provides Tower-compatible middleware that uses simulation time instead of wall-clock time:

```rust
use des_components::tower::{DesServiceBuilder, DesTimeoutLayer, DesRateLimitLayer};
use des_core::Simulation;
use tower::ServiceBuilder;
use std::time::Duration;

let mut simulation = Simulation::default();
let scheduler = simulation.scheduler_handle();

// Create a base service
let base_service = DesServiceBuilder::new("api-server".to_string())
    .thread_capacity(5)
    .service_time(Duration::from_millis(100))
    .build(&mut simulation)?;

// Add middleware layers using familiar Tower patterns
let service = ServiceBuilder::new()
    .layer(DesTimeoutLayer::new(Duration::from_secs(5), scheduler.clone()))
    .layer(DesRateLimitLayer::new(10.0, 20, scheduler))
    .service(base_service);
```

## Key Benefits

- **Deterministic Behavior**: All timing uses simulation time for reproducible results
- **Full Tower Compatibility**: Works with standard Tower middleware and utilities
- **Event-Driven**: Operations are scheduled as discrete events in the simulation
- **Performance Testing**: Test real-world service architectures with controlled conditions

Continue to the next section to learn about Tower service abstractions in detail.