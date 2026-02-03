# `descartes_tokio` Concurrency Primitives + Record/Replay Validation

This document describes the **deterministic, single-threaded** “simulated shared-memory concurrency” facilities implemented in `descartes_tokio`, and how `des-explore` can **record** and **validate** their behavior during replay.

These primitives are intended for **exploration tooling** (race hunting, schedule perturbation, trace replay), while preserving the project’s core constraints:

- Deterministic FIFO behavior by default.
- No event priorities.
- Opt-in policy experimentation and opt-in Tokio interleaving exploration.
- Schedule exploration (backtracking/policy search/DPOR) is specified separately:
  - `doc/ai/descartes_explore_schedule_exploration_spec.md`

---

## Mental Model

Even though the simulation is single-threaded, `descartes_tokio` tasks can behave like concurrent “threads” because:

- The async runtime polls many tasks over time.
- Wakeups can make other tasks runnable.
- Explicit yield points can be inserted.

Exploration can vary *which* runnable task is polled next (opt-in), producing different interleavings while remaining deterministic given the chosen policies + seeds.

The concurrency primitives in this module do **not** introduce real parallelism. They are deterministic state machines driven by task polling and wakeups.

---

## Provided Primitives

### `descartes_tokio::thread`

- `descartes_tokio::thread::spawn(fut)`
  - Thin wrapper over `descartes_tokio::task::spawn` to provide a `std::thread`-like entry point.
- `descartes_tokio::thread::yield_now().await`
  - Cooperative yield point.
  - Internally, it schedules itself to be polled again (wakes itself once), allowing other ready tasks to run in between.
  - This is the main user-facing knob for “simulated preemption”.

### `descartes_tokio::sync::Mutex<T>`

`descartes_tokio::sync::Mutex<T>` is an async mutex intended to model contention patterns deterministically.

- Default behavior:
  - Deterministic FIFO waiter ordering.
- API:
  - `Mutex::new(value)`
  - `Mutex::lock().await -> MutexGuard<'_, T>`
  - `Mutex::try_lock() -> Option<MutexGuard<'_, T>>`

#### Stable IDs

For record/replay validation, stable identifiers are important so that the **same logical mutex** emits the same `mutex_id` across record and replay runs.

Use one of:

- `Mutex::new_named("my_mutex", value)` (name-based stable id)
- `Mutex::new_with_id(id, value)` (explicit stable id)

`Mutex::new(value)` uses a process-global counter and is deterministic within a run, but not stable across “same scenario in a different closure” unless construction order is identical.

#### Waiter selection policy (opt-in)

When a guard is dropped and there are waiters, the mutex chooses which waiter to wake next via a **tokio-level global policy** installed during runtime installation.

- Default: FIFO (`FifoMutexWaiterPolicy`)
- Opt-in: uniform random (`UniformRandomMutexWaiterPolicy::new(seed)`)

This policy is installed via `descartes_tokio::runtime::TokioInstallConfig { mutex_policy: ... }`.

### `descartes_tokio::sync::AtomicU64`

`descartes_tokio::sync::AtomicU64` wraps `std::sync::atomic::AtomicU64` but emits **traceable atomic operation events**.

- API subset:
  - `load(ordering)`
  - `store(value, ordering)`
  - `fetch_add(delta, ordering)`
  - `compare_exchange(expected, new, success, failure)`

#### Stable IDs

Atomic operations are identified by a `site_id`:

- `AtomicU64::new_named("my_counter", value)` (recommended)
- `AtomicU64::new(site_id, value)`

You can also generate ids via macros:

- `descartes_tokio::stable_id!("domain", "name")` / `descartes_tokio::stable_id!("name")`
- `descartes_tokio::site_id!("tag")` (callsite-unique)

---

## Concurrency Event Stream (Observational)

Concurrency primitives optionally emit `descartes_tokio::concurrency::ConcurrencyEvent`.

Key properties:

- Events are **observational**: they do not affect scheduling by default.
- Each event includes:
  - `task_id: u64` (the currently-polled async task)
  - `time_nanos: Option<u64>` (current simulation time, when available)
  - primitive-specific identifiers (`mutex_id` or `site_id`)

Event kinds include:

- `MutexContended { mutex_id, task_id, time_nanos, waiter_count }`
- `MutexAcquire { mutex_id, task_id, time_nanos }`
- `MutexRelease { mutex_id, task_id, time_nanos }`
- `AtomicLoad/Store/FetchAdd/CompareExchange { site_id, task_id, time_nanos, ordering, ... }`

To capture events, install a recorder implementing:

- `descartes_tokio::concurrency::ConcurrencyRecorder` (`record(&self, event: ConcurrencyEvent)`)

Installation hook:

- `descartes_tokio::runtime::install_with_tokio(sim, TokioInstallConfig { concurrency_recorder: Some(recorder), .. }, configure)`

If no recorder is installed, events are simply ignored.

---

## Record/Replay Integration (`des-explore`)

`des-explore` extends its trace format with:

- `TraceEvent::Concurrency(ConcurrencyTraceEvent)`

A `ConcurrencyTraceEvent` stores:

- `time_nanos: Option<u64>`
- `task_id: u64`
- `event: ConcurrencyEventKind` (a stable, serializable representation)

### Recording

`descartes_explore::concurrency::RecordingConcurrencyRecorder` implements `descartes_tokio::ConcurrencyRecorder` and records `TraceEvent::Concurrency(...)` into the trace.

When using the harness, enable:

- `HarnessConfig.install_tokio = true`
- `HarnessConfig.record_concurrency = true`

### Replay validation

During replay, `des-explore::concurrency::ReplayConcurrencyValidator` compares the emitted event stream against the trace:

- Validation is **strict**: events must match exactly and in order.
- Mismatch produces a structured error (`ReplayConcurrencyError`) surfaced as `HarnessReplayError::Concurrency`.

Harness behavior:

- If the input trace contains concurrency events, replay installs a validator automatically.
- If the input trace contains **no** concurrency events, replay proceeds without validation and emits a warning.

### Why stable IDs matter

Replay validation compares events structurally. That means IDs like `mutex_id` / `site_id` must be stable across record and replay runs.

Recommended practice:

- Use `Mutex::new_named(...)` / `Mutex::new_with_id(...)`.
- Use `AtomicU64::new_named(...)` or `site_id!(...)`.

---

## Relationship to Scheduling Policies

These primitives are designed to work alongside the opt-in scheduling hooks:

- DES same-time frontier ordering (scheduler-level policy)
- Tokio ready-task polling ordering (runtime-level policy)
- Mutex waiter selection ordering (primitive-level policy)

Defaults remain FIFO at all layers unless explicitly configured.
