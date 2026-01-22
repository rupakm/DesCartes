# Server Components

The `Server` component simulates a server with configurable thread capacity, service time distributions, and optional request queuing. It provides realistic modeling of server behavior under load.

## Basic Server Creation

### Constant Service Time

The simplest server configuration uses a fixed service time for all requests:

```rust
use des_components::Server;
use std::time::Duration;

let server = Server::with_constant_service_time(
    "web-server".to_string(),
    4, // thread capacity (max concurrent requests)
    Duration::from_millis(100), // fixed service time
);
```

### Exponential Service Time

For M/M/k queueing systems, use exponential service time distribution:

```rust
let server = Server::with_exponential_service_time(
    "api-server".to_string(),
    8, // thread capacity
    Duration::from_millis(50), // mean service time
);
```

### Custom Service Time Distributions

For advanced scenarios, provide custom service time distributions:

```rust
use des_core::dists::{ServiceTimeDistribution, RequestContext};

// Service time based on request payload size
let server = Server::new(
    "file-server".to_string(),
    2,
    Box::new(RequestSizeBasedServiceTime::new(
        Duration::from_millis(10), // base time
        0.1, // ms per byte
    )),
);
```

## Queue Configuration

### No Queue (Reject When Busy)

By default, servers reject requests when at capacity:

```rust
let server = Server::with_constant_service_time(
    "strict-server".to_string(),
    2,
    Duration::from_millis(200),
);
// Requests are rejected when both threads are busy
```

### FIFO Queue

Add a first-in-first-out queue for request buffering:

```rust
use des_components::{Server, FifoQueue};

let server = Server::with_constant_service_time(
    "buffered-server".to_string(),
    4,
    Duration::from_millis(100),
).with_queue(Box::new(FifoQueue::bounded(20))); // Queue up to 20 requests
```

### Priority Queue

Use priority-based queuing for different request types:

```rust
use des_components::{Server, PriorityQueue};

let server = Server::with_constant_service_time(
    "priority-server".to_string(),
    3,
    Duration::from_millis(150),
).with_queue(Box::new(PriorityQueue::bounded(15)));
```

## Complete Server Example

Here's a comprehensive example showing server configuration and usage:

```rust
use des_components::{Server, ServerEvent, FifoQueue, SimpleClient, ClientEvent};
use des_core::{Simulation, Execute, Executor, SimTime, Component, Key};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sim = Simulation::default();
    
    // Create server with exponential service time and FIFO queue
    let server = Server::with_exponential_service_time(
        "web-api".to_string(),
        6, // 6 concurrent threads
        Duration::from_millis(80), // mean 80ms service time
    ).with_queue(Box::new(FifoQueue::bounded(30))); // 30 request queue
    
    let server_id = sim.add_component(server);
    
    // Create client to generate load
    let client = SimpleClient::with_exponential_backoff(
        "load-generator".to_string(),
        server_id,
        Duration::from_millis(50), // request every 50ms
        3, // max 3 retries
        Duration::from_millis(100), // base retry delay
    );
    
    let client_id = sim.add_component(client);
    
    // Start client
    sim.schedule(
        SimTime::zero(),
        client_id,
        ClientEvent::SendRequest,
    );
    
    // Run simulation for 10 seconds
    Executor::timed(SimTime::from_duration(Duration::from_secs(10)))
        .execute(&mut sim);
    
    // Get final server state
    let final_server = sim.remove_component::<ServerEvent, Server>(server_id)?;
    
    println!("Server processed {} requests", final_server.requests_processed);
    println!("Server rejected {} requests", final_server.requests_rejected);
    println!("Final utilization: {:.1}%", final_server.utilization() * 100.0);
    println!("Final queue depth: {}", final_server.queue_depth());
    
    Ok(())
}
```

## Server Metrics

Servers automatically collect comprehensive metrics:

```rust
let server = Server::with_constant_service_time(
    "monitored-server".to_string(),
    4,
    Duration::from_millis(100),
);

// After running simulation
let metrics = server.get_metrics();

// Access collected metrics
println!("Requests accepted: {}", 
    metrics.get_counter("requests_accepted", &[("component", "monitored-server")])
        .unwrap_or(0));
println!("Current utilization: {:.1}%", 
    metrics.get_gauge("utilization", &[("component", "monitored-server")])
        .unwrap_or(0.0));
```

### Available Metrics

- **Counters**: `requests_accepted`, `requests_completed`, `requests_rejected`, `requests_queued`
- **Gauges**: `active_threads`, `utilization`, `queue_depth`, `total_processed`
- **Histograms**: Service time distributions (when using custom distributions)

## Server Configuration Patterns

### High-Throughput Server

For high-throughput scenarios with minimal queuing:

```rust
let high_throughput_server = Server::with_constant_service_time(
    "fast-api".to_string(),
    16, // High thread capacity
    Duration::from_millis(25), // Fast service time
).with_queue(Box::new(FifoQueue::bounded(5))); // Small queue
```

### Batch Processing Server

For batch processing with longer service times:

```rust
let batch_server = Server::with_constant_service_time(
    "batch-processor".to_string(),
    2, // Limited concurrency
    Duration::from_secs(5), // Long processing time
).with_queue(Box::new(FifoQueue::unbounded())); // Large queue
```

### Load-Balanced Server Pool

Create multiple servers for load balancing:

```rust
let mut servers = Vec::new();
for i in 0..3 {
    let server = Server::with_exponential_service_time(
        format!("server-{}", i),
        4,
        Duration::from_millis(100),
    ).with_queue(Box::new(FifoQueue::bounded(10)));
    
    servers.push(sim.add_component(server));
}
```

## Best Practices

### Capacity Planning

Choose thread capacity based on your simulation goals:
- **CPU-bound**: Thread capacity = CPU cores
- **I/O-bound**: Thread capacity = 2-4x CPU cores  
- **Mixed workload**: Start with CPU cores, adjust based on metrics

### Queue Sizing

Size queues based on expected load patterns:
- **Steady load**: Queue size = 2-3x thread capacity
- **Bursty load**: Queue size = 10-20x thread capacity
- **Strict SLA**: Small queue or no queue for fast rejection

### Service Time Modeling

Choose appropriate service time distributions:
- **Constant**: Simple testing, deterministic behavior
- **Exponential**: Realistic for many server workloads
- **Request-dependent**: When payload size or complexity matters

### Monitoring

Always collect and analyze server metrics:
- Monitor utilization to detect overload
- Track queue depth for capacity planning
- Analyze rejection rates for SLA compliance

## Error Handling

Servers handle various error conditions gracefully:

```rust
// Server automatically handles:
// - Capacity exceeded (queues or rejects)
// - Queue full (rejects with 503 Service Unavailable)
// - Component lifecycle (cleanup on shutdown)

// Access error metrics
let rejection_rate = final_server.requests_rejected as f64 / 
    (final_server.requests_processed + final_server.requests_rejected) as f64;
    
if rejection_rate > 0.05 { // 5% rejection threshold
    println!("Warning: High rejection rate {:.1}%", rejection_rate * 100.0);
}
```

This server component provides the foundation for modeling realistic server behavior in distributed system simulations, with comprehensive metrics and flexible configuration options.