# DES Event Scheduling Policies (Core + Tokio)

This document describes the design for **deterministic** scheduling/tie-breaking policies in this Rust workspace, across:

- `des-core`: discrete-event simulation scheduler and async runtime
- `des_tokio`: Tokio-like facade built on `des-core::async_runtime`
- `des-explore`: opt-in tracing + record/replay + exploration harness

The intent is to preserve **default deterministic behavior** while enabling **opt-in experimentation** (randomized heuristics, record/replay) without adding event priorities or schedule enumeration.

---

## Goals

1. **Deterministic by default**
   - Default simulations behave exactly as before (FIFO semantics) unless explicitly opted into a different policy.

2. **Two independent choice points**
   - **DES scheduler level**: when multiple events are scheduled at the same simulation time.
   - **Tokio/async runtime level**: when multiple async tasks are ready to be polled during a single runtime poll cycle.

3. **Reproducible policy experimentation**
   - Randomized policies must be deterministic given a seed.
   - Record/replay must reproduce the same schedule choices.

4. **Forward/backward trace compatibility**
   - New trace fields/events must be optional and defaultable.
   - Old traces (without the new events) should still deserialize.

5. **Non-goals (this document)**
   - Event priorities.
   - Multi-threading.
   - Systematic schedule exploration algorithms.
     - See `doc/ai/des_explore_schedule_exploration_spec.md` for backtracking/policy search/DPOR/MCTS.

---

## Terminology

- **Simulation time**: `des_core::SimTime`.
- **Event**: a scheduled component event (`EventEntry::Component`) or a scheduled task event (`EventEntry::Task`) in the DES scheduler.
- **Same-time frontier**: all enabled scheduled events at the current minimum time `t`.
- **Ready async tasks**: tasks in `des_core::async_runtime::DesRuntime` that should be polled in the current poll cycle.
- **Choice point**: a moment where more than one runnable action exists and an ordering decision must be made.

---

## A. DES Scheduler (des-core)

### A.1 Baseline ordering model

The DES scheduler maintains an event queue ordered by:

1. `time` ascending
2. `seq` ascending (FIFO within a time)

Historically, ties were resolved purely by `seq` order.

### A.2 Frontier-based tie-breaking

To enable controlled experimentation, same-time events are treated as a **frontier**:

- When the scheduler is about to pop the next event, it gathers all non-cancelled entries at the minimum time into a frontier.
- A policy chooses one element of the frontier to execute next.
- The remaining frontier elements are reinserted into the queue.
- Simulation time advances once, to the frontier time.

Key API concepts (implemented):

- `des_core::scheduler::EventFrontierPolicy`
  - `choose(time: SimTime, frontier: &[FrontierEvent]) -> usize`
- `des_core::scheduler::FrontierEvent`
  - stable descriptor: `seq`, kind, optional IDs
- `des_core::scheduler::FrontierSignature`
  - stable choice-point signature: `{ time_nanos, frontier_seqs }`

### A.3 Policies

Policies are opt-in via the simulation:

- Default: `FifoFrontierPolicy` (deterministic FIFO; preserves old behavior)
- Opt-in: `UniformRandomFrontierPolicy::new(seed)`

Important invariants:

- **No priorities**: the policy only reorders same-time events, never changes event times.
- **Deterministic by default**: `Simulation` starts with FIFO.
- **Explicit opt-in**: users call `Simulation::set_frontier_policy(...)`.

### A.4 Deterministic IDs

Strict reproducibility requires avoiding wall-clock UUIDs in core execution paths.

`des-core` now derives key IDs deterministically from seeds and counters during normal simulation execution. This makes frontier tracing stable and reproducible.

---

## B. Scheduler Decision Recording & Replay (des-explore)

### B.1 Trace event

The exploration trace can record scheduler frontier choices as `TraceEvent::SchedulerDecision`.

Conceptually:

- **Signature**: `FrontierSignature` (time + seq list)
- **Decision**: chosen element (prefer chosen `seq` over chosen index)

This keeps replay robust even if frontier ordering changes due to unrelated internal refactors.

### B.2 Recording wrapper

`des-explore` provides `RecordingFrontierPolicy<P>`:

- wraps an underlying `EventFrontierPolicy`
- delegates `choose(...)`
- records `TraceEvent::SchedulerDecision(...)` via a shared `TraceRecorder`

### B.3 Replay wrapper

`des-explore` provides `ReplayFrontierPolicy`:

- replays `SchedulerDecision` events from a trace
- validates choice-point signature
- on mismatch:
  - does **not panic**
  - stores a `ReplayFrontierError` retrievable by the caller

### B.4 Harness integration

The harness supports frontier selection and decision recording on recorded runs:

- `HarnessConfig.frontier: Option<HarnessFrontierConfig>`
  - `policy: Fifo | UniformRandom { seed }`
  - `record_decisions: bool`

Replay runs install replay frontier policy from an input trace and return structured errors:

- `run_replayed(...) -> Result<_, HarnessReplayError>`

---

## C. Tokio / Async Runtime Scheduling (des-core + des-tokio)

### C.1 Where tokio nondeterminism comes from

`des_tokio` uses `des_core::async_runtime::DesRuntime`.

Inside a single DES event like `RuntimeEvent::Poll`, the runtime currently polls tasks in **FIFO ready-queue order** (`VecDeque::pop_front`). This is deterministic, but it is a *second* scheduling layer distinct from the DES event queue.

For exploration, we want to make this polling order configurable.

### C.2 Scope of tokio policy (agreed)

The tokio-level policy will apply to:

- **(A)** ordering of which ready async task is polled next during `DesRuntime::poll_ready_tasks`

It will **not** apply to:

- ordering of DES events at the same time (handled by scheduler frontier policy)
- ordering of timer deadline processing (wake ordering) beyond its effect on poll ordering

### C.3 Ready-task policy (implemented)

`des_core::async_runtime::DesRuntime` supports a second, separate policy layer for choosing which ready async task to poll next within a single `RuntimeEvent::Poll` cycle:

- `ReadyTaskPolicy`
  - `choose(time: SimTime, ready: &[async_runtime::TaskId]) -> usize`

Built-in policies (implemented):

- `FifoReadyTaskPolicy` (default; preserves historical behavior)
- `UniformRandomReadyTaskPolicy::new(seed)` (opt-in; deterministic given seed)

Implementation notes:

- `poll_ready_tasks` snapshots the ready set at the start of a cycle.
- Tasks woken during polling are queued for the next cycle (snapshot semantics).

### C.4 Exposing opt-in configuration via des_tokio (implemented)

- Default: `des_tokio::runtime::install(&mut Simulation)` remains FIFO.
- Opt-in configuration:
  - `des_tokio::runtime::install_with(sim, configure: impl FnOnce(&mut DesRuntime))`
  - `des_tokio::runtime::install_with_tokio(sim, TokioInstallConfig { .. }, configure)` (adds tokio-level knobs like mutex waiter policy + concurrency recorder)

This keeps defaults unchanged while enabling exploration tooling (and end users) to inject a different ready-task policy.

---

## D. Tokio Decision Recording & Replay (des-explore)

### D.1 Separate trace event (implemented)

Tokio/async runtime scheduling decisions are recorded separately from scheduler frontier decisions:

- `TraceEvent::AsyncRuntimeDecision(AsyncRuntimeDecision)`

Fields (current trace schema):

- `time_nanos: u64`
- `ready_task_ids: Vec<u64>` (observed ready task IDs)
- `chosen_task_id: Option<u64>` (preferred for replay matching)
- `chosen_index: usize` (fallback)

### D.2 Recording wrapper (implemented)

- `RecordingReadyTaskPolicy<P>` wraps a `ReadyTaskPolicy` and records each decision.

### D.3 Replay wrapper (implemented)

- `ReplayReadyTaskPolicy` replays `AsyncRuntimeDecision` events and validates signatures.
- On mismatch it stores a structured error (no panic).

### D.4 Old traces: FIFO fallback + explicit warning (agreed)

When replaying from a trace that contains **no** `AsyncRuntimeDecision` events:

- Replay behaves as FIFO.
- Emit a warning **once per run** via both:
  - `tracing::warn!(...)`
  - `eprintln!(...)`

This warning should be explicit that:

- tokio/async scheduling decisions were not present in the trace
- replay is using FIFO fallback for ready-task polling
- reproducibility is only guaranteed for scheduler frontier + RNG (if those were recorded)

Important: This fallback applies only when the trace has **zero** async-runtime decision events. If the trace has some decisions but replay runs out (or mismatches), replay should return an error (do not silently fallback).

---

## E. End-to-end replay story

A complete “replay everything” run should be able to reproduce:

1. Scheduler frontier choices (`TraceEvent::SchedulerDecision`) via `ReplayFrontierPolicy`
2. Async runtime ready-task choices (`TraceEvent::AsyncRuntimeDecision`) via `ReplayReadyTaskPolicy`
3. RNG draws (`TraceEvent::RandomDraw`) via `ReplayRandomProvider`/`SharedChainedRandomProvider`
4. (Optional) concurrency observations (`TraceEvent::Concurrency`) via `ReplayConcurrencyValidator`

The harness can provide:

- `run_recorded`: record whichever streams you opted into
- `run_replayed`: install replay policies from an input trace and return structured errors

---

## F. Future extensions (not in scope now)

- Additional heuristics (biased policies, hashing-based policies, etc.)
- Multi-run CI estimation for randomized policy families
- Schedule search algorithms that operate on choice-point signatures
- Trace versioning and schema migrations
