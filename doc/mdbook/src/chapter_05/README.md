# Chapter 5: M/M/k Queue Examples

This chapter provides comprehensive examples of M/M/k queueing systems using DESCARTES. You'll learn to build, analyze, and optimize queueing systems from simple M/M/1 queues to complex multi-server systems with retry policies and admission control.

## What You'll Learn

By the end of this chapter, you'll be able to:

- Implement classic M/M/1 and M/M/k queueing systems
- Add timeout and retry policies to improve reliability
- Implement admission control mechanisms
- Analyze queueing performance with theoretical validation
- Generate visualizations and performance reports

## Chapter Contents

### [Basic M/M/1 and M/M/k Queues](basic_queues.md)
Learn to implement fundamental queueing systems:
- M/M/1 queue with exponential arrivals and service times
- M/M/k queue with multiple servers
- Performance analysis and theoretical validation
- Queue capacity management and overflow handling

### [Clients with Timeouts and Retry Policies](retry_policies.md)
Add reliability mechanisms to your queueing systems:
- Client timeout configuration
- Fixed retry policies
- Exponential backoff with jitter
- Performance impact analysis

### [Clients with Admission Control](admission_control.md)
Implement sophisticated traffic management:
- Token bucket admission control
- Rate limiting strategies
- Success-based and time-based token filling
- Load shedding and graceful degradation

## Prerequisites

Before starting this chapter, you should:
- Complete [Chapter 1: Basics of DES Simulation](../chapter_01/README.md)
- Understand [DES-Core abstractions](../chapter_02/README.md)
- Be familiar with [DES-Components](../chapter_03/README.md)
- Know how to use [metrics collection](../chapter_04/README.md)

## M/M/k Queueing Theory Primer

### Notation
- **M/M/k**: Markovian arrivals / Markovian service / k servers
- **λ (lambda)**: Arrival rate (requests per second)
- **μ (mu)**: Service rate per server (requests per second)
- **ρ (rho)**: Utilization = λ / (k × μ)
- **k**: Number of servers

### Key Concepts

**Poisson Arrivals**: Requests arrive according to a Poisson process with exponential inter-arrival times.

**Exponential Service**: Each server processes requests with exponentially distributed service times.

**System Stability**: The system is stable when ρ < 1, meaning arrival rate is less than total service capacity.

**Little's Law**: L = λW, where L is average number in system, λ is arrival rate, W is average time in system.

## Running the Examples

All examples in this chapter can be run using:

```bash
# Run the comprehensive M/M/k examples
cargo run --package des-components --example mmk_queueing_example

# Run specific retry policy examples  
cargo run --package des-components --example client_retry_example

# Run end-to-end workload examples
cargo run --package des-components --example end_to_end_workload_example
```

## Example Output

When you run the examples, you'll see output like:

```
=== M/M/k Queueing System Results ===
Configuration:
  λ (arrival rate): 5.00 req/s
  μ (service rate): 8.00 req/s per server
  k (servers): 1
  Queue capacity: 50
  Theoretical ρ: 0.625 (stable)

Performance Metrics:
  Requests sent: 10
  Requests completed: 10
  Requests dropped: 0
  Success rate: 100.00%
  Throughput: 5.00 req/s
  Server utilization: 62.50%

Latency Metrics:
  Average response time: 125.45 ms
  P95 response time: 245.20 ms
  P99 response time: 387.10 ms
  Average queue depth: 2.15
```

## Theoretical Validation

Each example includes theoretical validation to help you understand queueing theory:

- **Utilization Check**: Verify ρ = λ/(k×μ) matches observed server utilization
- **Little's Law**: Confirm L = λW relationship holds
- **Stability Analysis**: Ensure stable systems behave as expected
- **Performance Bounds**: Compare observed metrics to theoretical limits

## Visualization and Analysis

The examples generate time-series charts showing:
- Average latency over time
- Queue depth evolution
- Server utilization patterns
- Timeout and retry rates

Charts are saved to `target/mmk_charts/` and can be viewed with any image viewer.

## Common Patterns

### Basic Queue Setup
```rust
use des_components::{Server, FifoQueue};
use des_core::{Simulation, SimTime};

// Create M/M/k server with exponential service times
let server = Server::with_exponential_service_time(
    "web_server".to_string(),
    k, // number of servers
    Duration::from_secs_f64(1.0 / mu), // mean service time
).with_queue(Box::new(FifoQueue::bounded(queue_capacity)));
```

### Exponential Arrival Client
```rust
use rand_distr::{Exp, Distribution};

// Generate exponential inter-arrival times
let arrival_dist = Exp::new(lambda).unwrap();
let next_arrival = arrival_dist.sample(&mut rng);
scheduler.schedule(
    SimTime::from_duration(Duration::from_secs_f64(next_arrival)),
    client_key,
    ClientEvent::SendRequest,
);
```

### Metrics Collection
```rust
use des_metrics::SimulationMetrics;

// Track key queueing metrics
metrics.increment_counter("requests_sent", &[("component", "client")]);
metrics.record_gauge("queue_depth", queue_size as f64, &[("component", "server")]);
metrics.record_latency("response_time", latency, &[("component", "server")]);
```

## Next Steps

After completing this chapter:
- Explore [Tower Integration](../chapter_06/README.md) for middleware patterns
- Learn [Advanced Examples](../chapter_07/README.md) for complex scenarios
- Study [API Reference](../reference/api.md) for detailed documentation

Ready to build your first queueing system? Start with [Basic M/M/1 and M/M/k Queues](basic_queues.md)!