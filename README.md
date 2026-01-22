# DesCartes — Deterministic Discrete-Event Simulation for Rust

DesCartes is a Rust workspace for building **deterministic**, **single-threaded** discrete-event simulations (DES) of distributed and concurrent systems.

The core model is:

- Simulated time advances by processing scheduled events.
- Async tasks (`async`/`await`) run on a simulated runtime (`des_tokio`) backed by the DES scheduler.
- Exploration tooling is **opt-in** and does not affect default FIFO behavior.

## Key Properties

- **Deterministic by default**: given the same inputs/seeds, the simulation is reproducible.
- **Single-threaded execution**: concurrency is modeled via interleaving, not OS threads.
- **No event priorities**: same-time events are tie-broken deterministically (FIFO by default) with optional, seeded policies.
- **Opt-in exploration**: record/replay and randomized scheduling are explicit, seeded, and isolated from default runs.
- **Tokio-like façade**: `des_tokio` provides a subset of Tokio APIs backed by simulated time.

## Workspace Crates

- `des-core`: core simulation engine (scheduler, simulated time, async runtime integration, deterministic IDs, formal reasoning hooks)
- `des-tokio`: Tokio-like façade running on `des-core` (tasks, time, channels, mutex/atomics, Semaphore/RwLock, JoinSet)
- `des-explore`: opt-in exploration tooling (trace record/replay, estimator utilities, replay validation)
- `des-components`: reusable components for distributed systems modeling (queues, servers, throttles, retry policies, etc.)
- `des-tower`: Tower integration for simulating real Tower middleware stacks deterministically
- `des-tonic`: tonic-like RPC façade built on simulated transport models
- `des-metrics`: metrics collection and observability helpers
- `des-viz`: visualization helpers for simulation outputs

Human-oriented overviews live in `doc/human/`.
Design notes and implementation details live in `doc/ai/`.

## Getting Started

```bash
# Build all crates
cargo build

# Run all tests
cargo test

# Build rustdoc for the workspace
cargo doc --workspace --open
```

### A minimal simulation

At the lowest layer (`des-core`), you schedule events and/or tasks, then run an executor.

```rust
use des_core::{Execute, Executor, Simulation, SimTime};
use std::time::Duration;

let mut sim = Simulation::default();

// Run the simulation for 1 simulated second.
Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
```

For async workloads, install `des_tokio` into a simulation and spawn async tasks that use simulated time:

```rust
use des_core::{Execute, Executor, SimTime, Simulation};
use std::time::Duration;

let mut sim = Simulation::default();

des_tokio::runtime::install(&mut sim);

des_tokio::task::spawn(async {
    des_tokio::time::sleep(Duration::from_millis(10)).await;
});

Executor::timed(SimTime::from_duration(Duration::from_millis(20))).execute(&mut sim);
```

## Notes on Determinism

- Default ordering is FIFO at the DES scheduler frontier and within the async runtime.
- Any randomized policy must be explicitly installed and seeded.
- Exploration tooling (`des-explore`) is opt-in; recording/replay is designed to make “interesting” executions reproducible.

## License

This repository is intended to be dual-licensed under **MIT OR Apache-2.0** (see `Cargo.toml`).

- Apache 2.0 text: `LICENSE-APACHE`
