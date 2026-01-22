# `des-core`

`des-core` is the foundation of the workspace: a **deterministic, single-threaded discrete-event simulation engine**.

## What it provides

- **Simulated time** (`SimTime`) with nanosecond precision.
- A **scheduler** that executes events in timestamp order.
  - Same-time tie breaking is **FIFO by default**.
  - Optional policies exist, but must be explicitly installed (seeded) so defaults remain unchanged.
- A deterministic async runtime (`des_core::async_runtime`) that powers `des-tokio`.
- Deterministic ID generation (seeded), avoiding wall-clock UUIDs in core execution paths.
- Optional **formal reasoning hooks** (Lyapunov/certificates), exposed under `des_core::formal`.

## How it fits with the rest

- `des-tokio` installs the `des-core` async runtime into a `Simulation` and exposes Tokio-like APIs.
- `des-explore` adds opt-in record/replay (scheduler and async-runtime decisions, RNG draws, etc.).
- Higher-level crates (components/tower/tonic) ultimately schedule and process events via this scheduler.

## Typical usage

- Build a `Simulation`.
- Add components/tasks/events.
- Run an `Executor` to a time bound or completion.

See also:
- `doc/human/des-tokio.md`
- `doc/ai/des_event_scheduling_policies.md`
