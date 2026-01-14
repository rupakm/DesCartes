# DesCartes DES System: Design Audit + Fix Plan

This document captures a careful audit of the current design/implementation, highlights likely bugs and complications (especially around determinism), and proposes a concrete fix plan. It also outlines a path to support end-to-end `tonic` services while preserving deterministic discrete-event simulation.

## Scope

Audit focus:
- Simulation core: event scheduling, time model, task system, async runtime, waker integration.
- Component ecosystem: server/client/queues and Tower integration.
- Determinism and reproducibility: stable ordering, RNG usage, data structure iteration hazards.

All notes below are based on reading the repo and running `cargo test` (all tests passed at the time of writing).

---

## Codebase Map (High-Level)

### `des-core`
- Simulation container + component registry: `des-core/src/lib.rs`
- Time model: `des-core/src/time.rs` (`SimTime`)
- Event queue + scheduler: `des-core/src/scheduler.rs`
- Task system: `des-core/src/task.rs`
- Async runtime: `des-core/src/async_runtime.rs`
- Waker integration: `des-core/src/waker.rs`
- Distributions: `des-core/src/dists.rs`

### `des-components`
- Core building blocks: `des-components/src/server.rs`, `des-components/src/simple_client.rs`, `des-components/src/queue.rs`
- Tower integration:
  - Base DES service + builder: `des-components/src/tower/service/mod.rs`
  - Middleware layers: `des-components/src/tower/**`

### `des-metrics`, `des-viz`
- Observability: `des-metrics/src/**`
- Reporting/charts: `des-viz/src/**`

---

## Audit Findings: Bugs, Risks, and Complications

### 1) Non-deterministic ordering for same-time events

**What:** The scheduler uses `BinaryHeap<EventEntry>` where ordering is primarily by time. For events at the same `SimTime`, ordering is effectively unspecified.

**Why it matters:** Discrete-event simulation correctness and reproducibility depend on deterministic tie-breaking when multiple events occur at the same timestamp (common for wakers, “schedule-now”, retries, timeouts, etc.). `BinaryHeap` does not provide stable ordering among equal-priority entries.

**Symptoms:**
- Same simulation configuration can produce different traces/outcomes across runs.
- Subtle “heisenbugs” where changing logging or unrelated code changes ordering.

**Fix direction:** Include a deterministic tie-breaker in event ordering, e.g. `(time, sequence)` where `sequence` is a monotonic `EventId` assigned at scheduling time.

### 2) Thread-safety and unsafe scheduler TLS pointer

**What:** `SchedulerHandle` suggests “thread-safe scheduling” (mutex around scheduler), but deferred wake integration uses a raw scheduler pointer stored in thread-local storage and mutated without locking.

**Why it matters:** If any part of the system uses `SchedulerHandle` concurrently (or in the future is extended to do so), this creates a plausible data race/UB hazard: the scheduler can be mutated through safe locked paths and unsafe pointer paths simultaneously.

**Fix direction:** Decide and enforce one of:
- **Single-threaded simulation**: make it explicit, remove/limit `Send + Sync` claims, and ensure all scheduling occurs on the simulation thread.
- **Actually thread-safe scheduling**: rework deferred wake collection to be safe under concurrency (e.g., lock-free queue merged under lock, or always lock scheduler for wake enqueue).

### 3) Waker behavior/documentation mismatch

**What:** `defer_wake` drops wakes when not in scheduler context (no active TLS pointer). Documentation indicates it works inside/outside scheduler context.

**Why it matters:** Futures can hang forever if woken outside a scheduler step.

**Fix direction:** Either:
- Make wakers always safe: when out-of-context, store wake requests somewhere durable to be drained at next step.
- Or formally restrict usage: document that wakers only work during simulation stepping and enforce via runtime checks.

### 4) Task system correctness and resource risks

**Issues:**
- `TaskWrapper` ignores the provided id and returns a new random id every time it is queried.
  - Breaks debugging, logging, and any future features relying on task identity.
- Cancellation removes tasks from `pending_tasks` but does not remove the scheduled task event from the heap.
  - When cancelled tasks pop, time may still advance (even though work is skipped), which can change simulation semantics.
- `completed_task_results` can grow unbounded if results are never retrieved.
  - Many tasks (timeouts, periodic callbacks) naturally produce `()` and will never be queried.

**Fix direction:**
- Store stable ids inside task wrappers.
- Provide true cancellation semantics or ensure cancelled task events don’t advance time.
- Add cleanup/retention policy for task results (especially `()` output), or store results only for handles that will be polled.

### 5) RNG usage undermines reproducibility

**What:** RNGs seeded from entropy (`from_entropy`) and `thread_rng()` are used in distributions and retry jitter.

**Why it matters:** This breaks “same seed → same results” and makes test cases or workloads non-reproducible by default.

**Fix direction:**
- Introduce a simulation-scoped seed and deterministic RNG services.
- Thread or inject RNG into distributions/policies.
- Ensure “random” behavior can still be configured, but defaults should be deterministic.

### 6) Time conversion truncation risk

**What:** `SimTime::from_duration` casts nanoseconds (`u128`) to `u64` without checking overflow.

**Why it matters:** Very large durations could silently truncate.

**Fix direction:** Add range checking (panic or saturate with explicit behavior).

### 7) HashMap iteration and UUID generation hazards

**What:** Components and tasks use UUIDs. If iteration order or id ordering ever leaks into behavior, determinism can degrade.

**Fix direction:** Keep execution semantics independent of map iteration; avoid deriving ordering from UUIDs; prefer stable numeric ids for ordering.

### 8) Tower integration depends heavily on scheduler semantics

**What:** The Tower integration (service builder + response router + waker notification) is structurally sound but relies on deterministic scheduling and correct wake behavior.

**Fix direction:** Fix scheduler tie-breaking and waker semantics first; add determinism tests at the Tower layer (same seed/config produces same metrics trace).

---

## Fix Plan (Concrete, Phased)

### Phase 0: Guardrails + tests (fast feedback)

**Goal:** Lock in determinism expectations and prevent regressions.

- Add a small set of determinism tests:
  - Schedule N same-time events and assert stable ordering.
  - A “tower flow” test that runs the same workload twice and asserts identical key metrics and event ordering (at least counts/latency histograms and request ids).
- Add debug-mode assertions:
  - Detect scheduling “in the past”.
  - Detect `defer_wake` being invoked out of scheduler context (until Phase 2 decides behavior).

**Exit criteria:** new tests pass reliably across repeated local runs.

### Phase 1: Deterministic scheduler ordering

**Goal:** Stable ordering for any set of events.

- Introduce a monotonic event sequence id assigned at schedule time.
- Update scheduler ordering to compare `(SimTime, seq)`.
- Ensure all event types (component events, closures, tasks, runtime poll events) participate in the same ordering scheme.

**Exit criteria:**
- Same-time event ordering tests pass.
- No behavior regressions in existing test suite.

### Phase 2: Waker semantics and safety model

**Goal:** Clear, correct wake behavior that can’t silently drop wakeups.

Choose one of these design paths (recommendation depends on desired future concurrency):

**Option A (Recommended if simulation is single-threaded):**
- Enforce single-threaded simulation stepping.
- Make out-of-context wake scheduling a hard error (panic in debug, ignore in release with log) OR buffer wakes and drain on next step.
- Remove misleading “thread-safe scheduling” claims if they exist.

**Option B (Recommended if you want cross-thread scheduling):**
- Replace unsafe TLS scheduler pointer mutation with a safe wake queue:
  - `DesWaker` pushes into an `Arc<Mutex<Vec<WakeRequest>>>` or lock-free queue.
  - Scheduler drains that queue at defined times (start/end of `step`).

**Exit criteria:**
- No silent wake drops.
- Clear docs on what is supported.

### Phase 3: Task identity, cancellation semantics, and result retention

**Goal:** Tasks are observable and do not subtly distort time.

- Fix task id semantics:
  - Store the task id on creation and return it consistently.
- Rework cancellation:
  - Either remove cancelled tasks from the event queue (hard with `BinaryHeap`) or ensure cancelled task events do not advance time.
  - If the latter: represent task event as “no-op” and skip time advancement when popping it.
- Result retention:
  - Don’t store results for `Output=()` (or store with bounded retention).
  - Add explicit API for keeping results (e.g., `schedule_task_with_result_retention`).

**Exit criteria:**
- Task cancellation does not advance time.
- Long-running simulations do not leak memory due to unused results.

### Phase 4: Deterministic randomness

**Goal:** Reproducible outcomes by default.

- Add `SimulationConfig { seed: u64, … }` (or similar) and store it in `Simulation`.
- Provide a simulation RNG service:
  - For example `SimulationRng(ChaCha8Rng)` and per-component derived streams.
  - Derive per-component/per-request RNG deterministically from `(seed, component_id, request_id)`.
- Update:
  - Distributions in `des-core/src/dists.rs` to accept an injected RNG or to use simulation RNG.
  - Retry jitter in `des-components/src/retry_policy.rs` to use deterministic RNG.

**Exit criteria:**
- Running the same workload with the same seed produces identical outputs.

### Phase 5: Time conversion correctness

**Goal:** Make `SimTime` conversions explicit and safe.

- Add range checks in `SimTime::from_duration` or document saturation behavior.

---

## Supporting `tonic` Services End-to-End (Deterministic DES)

### Target outcome

Let users write business logic in a `tonic` server/client style (or very close), but run the whole system under a deterministic discrete-event simulation (virtual time), including:
- unary RPC latency and timeouts
- retries and load shedding
- backpressure and queueing
- optionally streaming RPCs

### Key seam: keep handlers/middleware, swap transport

The most robust approach is to keep application logic at the `tower::Service` boundary and simulate transport separately.

- `tonic` server-side is fundamentally a Tower service stack.
- The repo already has a strong Tower simulation story (`des-components/src/tower/service/mod.rs` and layers).

**Strategy:**
- Model the transport (HTTP/2-ish) as scheduled message deliveries.
- Keep service handlers/middleware identical between simulation and production.

### Minimal viable end-to-end plan (unary RPC)

1) **Define a simulated channel/transport abstraction**
   - `SimTransport` that takes a request payload and schedules delivery to a server endpoint.
   - Configurable network model: latency/jitter/drop/queue/bandwidth.

2) **Client stub in simulation**
   - Provide a `tonic`-like client API facade:
     - For MVP, it can wrap “request bytes” rather than full h2 framing.
   - In simulation, client call becomes:
     - schedule “request arrives at server” at `t + latency`.
     - server processing time modeled by server component/layers.
     - schedule “response arrives at client” at `t + latency`.

3) **Server side in simulation**
   - Use existing `DesServiceBuilder` / `DesService` to simulate server capacity + service time distribution.
   - Extend `RequestContext` extraction to capture enough information from tonic requests (method/path, headers, body size).

4) **Deadlines and cancellations**
   - Use DES-aware timeout logic (already implemented as Tower layer) to model deadlines.
   - Ensure cancellation/waker semantics work reliably (Phase 2 above is a prerequisite).

5) **Determinism and seeds**
   - All network randomness (jitter, drops) must be driven by the simulation seed.

### Streaming RPC (phase 2 for tonic)

Streaming adds complexity because it introduces long-lived ordered message streams and flow-control.

A practical staged approach:
- **Stage A:** model streaming as a sequence of discrete messages with per-message latency and ordering constraints.
- **Stage B:** model flow-control windows and backpressure as explicit simulated resources (buffer capacity, “in-flight credits”).

### Fidelity dial: “RPC-as-message” vs “HTTP/2-accurate”

You can support two fidelity levels:

- **RPC-as-message (Recommended MVP)**
  - Treat RPC as logical messages with metadata + bytes.
  - Much easier; deterministic; enough for queueing/retry/timeout studies.

- **HTTP/2-accurate (Later)**
  - Simulate h2 stream multiplexing, header compression, window updates.
  - High effort; harder to keep deterministic; only needed for very transport-specific research.

### Extension points for tonic support

- A dedicated `SimNetwork` component that routes messages between endpoints.
- A `SimEndpointRegistry` mapping service names to server keys.
- Integrate with metrics: per-RPC latency, retries, timeouts, drops, queue depths.

---

## Suggested Implementation Order (Practical)

1) Phase 1 (stable scheduler ordering) + determinism tests.
2) Phase 2 (waker semantics) + safety model decision.
3) Phase 4 (seeded RNG) so workload/network models are reproducible.
4) MVP tonic end-to-end on top of Tower boundary (RPC-as-message).
5) Phase 3 (task cancellation/results) to harden long-running simulations.

---

## Notes

This repo is already close to supporting realistic server stacks via Tower. The biggest blockers to “deterministic DES” are stable same-time ordering, clear wake semantics, and seedable RNG.
