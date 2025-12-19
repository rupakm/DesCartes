# DES Framework - Discrete Event Simulation for Rust

A discrete event simulation framework for modeling and analyzing distributed systems with formal reasoning capabilities.

## Features

- **SimPy-like API**: Familiar discrete event simulation semantics with async/await support
- **Composable Components**: Reusable building blocks for servers, clients, queues, throttles, and retry policies
- **Formal Verification**: Lyapunov function evaluation and certificate verification for performance guarantees
- **Rich Observability**: Comprehensive metrics collection, statistical analysis, and visualization
- **Type Safety**: Compile-time validation of component configurations using Rust's type system
- **Zero-Cost Abstractions**: High performance without runtime overhead

## Project Structure

This workspace contains the following crates:

- **des-core**: Core simulation engine (time management, event scheduling, process execution, and formal reasoning)
- **des-components**: Reusable simulation components for distributed systems
- **des-metrics**: Metrics collection and observability
- **des-viz**: Visualization library for simulation results

## Getting Started

```bash
# Build all crates
cargo build

# Run tests
cargo test

# Build documentation
cargo doc --open
```

## License

Licensed under:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
