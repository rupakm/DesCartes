# `des-tokio`

`des-tokio` is a Tokio-like façade that runs on the deterministic DES runtime in `des-core`.

## Goal

Allow writing async Rust code (tasks, timers, channels, synchronization) that executes against **simulated time** and remains deterministic.

## What it provides (subset)

- **Runtime install hooks**
  - `des_tokio::runtime::install(&mut Simulation)`
  - `des_tokio::runtime::install_with(...)` / `install_with_tokio(...)` for opt-in configuration
- **Tasks**
  - `des_tokio::task::{spawn, spawn_local, JoinHandle}`
  - `JoinSet` for awaiting a set of spawned tasks in completion order
- **Time**
  - `des_tokio::time::{Instant, sleep, sleep_until, timeout}`
  - `interval/interval_at` with `MissedTickBehavior`
- **Synchronization / channels** (deterministic behavior)
  - `sync::mpsc`, `sync::oneshot`, `sync::watch`, `sync::Notify`
  - `sync::Mutex` (async) + traced `sync::AtomicU64`
  - `sync::Semaphore` + `sync::RwLock` (Tokio-like fairness semantics)
- **Simulated shared-memory concurrency tools**
  - `des_tokio::thread::yield_now()` to insert explicit yield points

## Determinism notes

- Everything is single-threaded. “Concurrency” is modeled via task interleavings.
- Defaults are FIFO; exploration hooks (e.g., ready-task policy) are opt-in and seeded.

See also:
- `doc/ai/des_tokio_concurrency_replay.md`
