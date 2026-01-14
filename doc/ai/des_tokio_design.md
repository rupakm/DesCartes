# Deterministic Async Runtime + `des_tokio` Design

**Status:** Draft (implementation in progress)

## Goals

- Provide deterministic simulation for async Rust on top of the DES scheduler.
- Keep the simulation **single-threaded**.
- Make polling and waking **simple and readable**.
- Provide a Tokio-like facade crate/module: **`des_tokio`**.
  - `des_tokio::task::spawn` requires `Send` futures.
  - `des_tokio::task::spawn_local` exists for `!Send`.
  - `des_tokio::task::JoinHandle` detaches on drop (Tokio-like); cancellation is explicit via `abort()`.
- Provide deterministic, simulated shared-memory concurrency primitives (threads/yield points, async mutex, traced atomics) with opt-in event recording + replay validation.
- Provide Tokio-like time: **`des_tokio::time::Instant`**.
  - `Instant::now()` and time functions panic if runtime not installed.
- Panics in spawned tasks should **abort immediately** (no `catch_unwind`).

## Non-goals (for now)

- Cross-thread scheduling / multi-threaded simulation.
- Full Tokio API parity.
- Full `tonic` transport fidelity (we are OK with RPC-as-message abstraction).

## Related Docs

- `doc/ai/des_event_scheduling_policies.md`
- `doc/ai/des_tokio_concurrency_replay.md`

## Current State (Repository)

- `des-core/src/async_runtime.rs` implements `DesRuntime` as a `Component`.
  - Uses `RuntimeEvent::{Poll, Wake{task_id}, RegisterTimer{..}, TimerTick, Cancel}`.
  - Uses `create_des_waker(...)` → `defer_wake(...)`.
  - Implements `SimSleep` by registering timers with the runtime (no raw scheduler pointer).
- `des-core/src/scheduler.rs` provides scheduler-context TLS and `defer_wake(...)`.

## Design: One Wake Path

**Rule:** all wakeups schedule runtime/component events via `defer_wake(...)`.

- Futures should not call `Scheduler::schedule(...)` directly.
- Runtime time reads should use scheduler context (`scheduler::current_time()`), not a separate TLS.

## Design: Runtime-managed Timers

Replace “one scheduler event per `sleep`” with runtime-owned timers:

- New runtime events:
  - `RegisterTimer { task_id, deadline: SimTime }`
  - `TimerTick`
- Runtime stores timers in `BTreeMap<SimTime, Vec<TaskId>>`.
- Only the earliest deadline schedules a `TimerTick` into the DES scheduler.

## Design: Deterministic Polling

- Maintain a FIFO ready queue.
- Use snapshot polling per cycle: tasks woken while polling are queued for the next cycle.
- Avoid O(n) `ready_queue.contains` by tracking a `queued` flag per task.
- When using `tokio::select!` in `des_tokio` tasks, prefer `biased;` to keep stable polling order (Tokio defaults to randomizing branch poll order for fairness).

## `des_tokio` Surface (MVP)

### `des_tokio::runtime`
- `Runtime::new()`
- `Runtime::install(&mut Simulation) -> InstalledRuntime`
  - Schedules initial poll
  - Installs a thread-local handle so `spawn()` can work during setup

### `des_tokio::task`
- `spawn(F) -> JoinHandle<T>` where `F: Future<Output=T> + Send + 'static`, `T: Send + 'static`
- `spawn_local(F) -> JoinHandle<T>` where `F: Future<Output=T> + 'static`
- `JoinHandle<T>` detaches on drop; cancellation is explicit via `JoinHandle::abort()`.
- Panics abort immediately.

### `des_tokio::time`
- `Instant` (wrapper over `SimTime`) with Tokio-like methods.
- `sleep(Duration)`, `sleep_until(Instant)`, `timeout(Duration, fut)`.

## Implementation Steps

We will implement incrementally, updating this doc after each step.

1. **Step 1 (next):** Simplify `DesRuntime` polling context
   - Remove `CURRENT_TIME` and `CURRENT_SCHEDULER` TLS from `async_runtime.rs`.
   - Time reads use `des_core::scheduler::current_time()`.
   - Timer scheduling remains as-is temporarily.
   - Add/adjust tests if needed.

2. **Step 2:** Replace `SimSleep` with runtime-managed timers
   - Add `RegisterTimer` + `TimerTick`.
   - Implement `BTreeMap` timer storage.
   - Remove raw scheduler pointer scheduling from sleep.
   - Remove temporary scheduler helper once unused.

3. **Step 3:** Ready queue + task storage cleanup
   - Add `queued` flag to avoid O(n) `contains()` checks.
   - Avoid per-cycle Vec allocation by polling a fixed snapshot count.
   - Consider moving tasks to a slab.

4. **Step 4:** Introduce `des_tokio` MVP
   - Add a runtime install/handle mechanism.
   - `Instant` wrapper and time functions.
   - `spawn`/`spawn_local` + `JoinHandle` (detach on drop; explicit `abort`).

5. **Step 5:** Add initial sync primitives
   - `sync::oneshot`, `sync::mpsc`, `sync::Notify`.
   - Deterministic FIFO wake ordering for waiters.

## Step Log

- 2026-01-10: Draft created.
- 2026-01-10: Step 1 completed: removed duplicated time/scheduler TLS from `des-core/src/async_runtime.rs` and routed time reads through `des_core::scheduler::current_time()`.
- 2026-01-10: Step 2 completed: `SimSleep` now emits `RuntimeEvent::RegisterTimer` and the runtime manages timers via a `BTreeMap<SimTime, Vec<TaskId>>` plus a single `RuntimeEvent::TimerTick`. Removed the temporary `schedule_in_context` helper from `des-core/src/scheduler.rs`.
- 2026-01-10: Step 3 completed (part 1): added per-task `queued` flag and changed `poll_ready_tasks` to poll a snapshot count from `VecDeque` (no per-cycle Vec allocation). This removes O(n) `ready_queue.contains` scans and keeps deterministic snapshot semantics.
- 2026-01-10: Step 4 started (part 1): added spawn queues + `RuntimeEvent::Cancel` to support cancellation.
- 2026-01-10: Step 4 progress: created `des-tokio` crate with `runtime::install`, `task::{spawn, spawn_local, JoinHandle}`, and `time::{Instant, sleep, sleep_until}`. Added `des-tokio/tests/basic.rs` covering install requirement, spawn+join, and drop cancellation.
- 2026-01-10: Added `des-tokio/tests/admission_control.rs` demonstrating admission control + workload spikes. Test uses `Executor::timed(...)` and verifies open-loop behavior (arrivals happen before first completion) plus that rejections occur during the spike.
- 2026-01-10: Added `des-tokio/tests/retry_latency.rs` demonstrating load shedding (Busy responses) + deterministic exponential backoff retries and showing that spike median latency exceeds baseline median latency.
- 2026-01-10: Implemented `des_tokio::time::Instant` parity improvements (checked/saturating/strict duration_since) and added `des-tokio/tests/time_parity.rs`.
- 2026-01-10: Implemented `des_tokio::time::timeout` (+ `Elapsed` and `Timeout`) and added `des-tokio/tests/timeout.rs`.
- 2026-01-10: Updated `des_tokio::task::JoinHandle` to Tokio-like detach-on-drop semantics. Added `abort_cancels_task` and updated the old drop-cancels test to `drop_detaches_task`.
- 2026-01-10: Added `des_tokio::sync::oneshot` and `des-tokio/tests/oneshot.rs` covering send/recv, sender drop, and receiver drop.
- 2026-01-10: Added `des_tokio::sync::mpsc` (bounded channel) with `send/try_send` + `recv/try_recv` and deterministic FIFO wake ordering for blocked senders. Added `des-tokio/tests/mpsc.rs`.
- 2026-01-10: Added `des_tokio::sync::Notify` with FIFO waiter order and Tokio-like permit semantics. Fixed `notify_one/notify_waiters` to mint permits when waking existing waiters so `notified().await` completes. Added `des-tokio/tests/notify.rs`.
