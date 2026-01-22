# Chapter 7: Advanced Examples

This chapter demonstrates advanced DESCARTES simulation patterns and techniques for modeling complex real-world scenarios. All examples are fully runnable and include comprehensive test coverage.

## What You'll Learn

- Payload-dependent service time distributions
- Client-side admission control strategies  
- Server-side throttling and admission control
- Advanced workload generation patterns
- Custom distribution implementations

## Chapter Contents

1. **[Payload-Dependent Service Times](./payload_dependent_service_times.md)** - Service times that vary based on request characteristics
2. **[Client-Side Admission Control](./client_side_admission_control.md)** - Token buckets and adaptive rate limiting
3. **[Server-Side Throttling](./server_side_throttling.md)** - Server-side admission control and load shedding
4. **[Advanced Workload Patterns](./advanced_workload_patterns.md)** - Complex traffic patterns and stateful clients
5. **[Custom Distributions](./custom_distributions.md)** - Implementing custom service time distributions

## Key Features

All examples in this chapter:

- **Fully Runnable**: Complete, working code that you can run immediately
- **Test Coverage**: Each example includes comprehensive test cases
- **Real-World Scenarios**: Based on actual production system patterns
- **Performance Analysis**: Detailed metrics and visualization
- **Extensible**: Designed to be modified and extended for your use cases

## Running the Examples

Each section includes both documentation and runnable code. You can:

1. **Run individual examples**: Each example can be executed standalone
2. **Run the test suite**: All examples are tested with `cargo test`
3. **Modify and experiment**: Examples are designed to be easily customized

```bash
# Run all advanced examples tests
cargo test --package des-components advanced_examples

# Run a specific example
cargo run --package des-components --example payload_dependent_service_times

# Run with visualization output
cargo run --package des-components --example advanced_workload_patterns -- --visualize
```

## Prerequisites

Before diving into advanced examples, ensure you're familiar with:

- **Chapter 1**: Basic DES concepts and terminology
- **Chapter 2**: Core abstractions (Simulation, Component, Scheduler)
- **Chapter 3**: DES-Components (Server, Client, Queue systems)
- **Chapter 6**: Tower integration patterns

## Example Categories

### Performance Modeling
- Request size-based service times
- Endpoint-specific processing delays
- CPU and memory-dependent distributions

### Admission Control
- Token bucket algorithms
- Adaptive rate limiting
- Success-based backpressure

### Load Management
- Server-side throttling
- Queue-based admission control
- Random early detection (RED)

### Traffic Patterns
- Bursty workloads
- Seasonal traffic variations
- Multi-tenant scenarios

Continue to the first section to explore payload-dependent service time distributions.