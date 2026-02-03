# DES Systematic Exploration: Bug Finder + Quantitative Estimation (with Timing)

**Status:** Draft (conceptual / background)

**Implementation spec:** For the concrete v1/v2 implementation plan (backtracking core, policy search, DPOR, MCTS), see:
- `doc/ai/descartes_explore_schedule_exploration_spec.md`

This document outlines two related tools built on top of the DES scheduler:

1. A **bug finder** that aggressively searches for counterexample traces (rare corner cases), combining **scheduler adversarial search** with **rare-event biased randomness**.
2. A **quantitative estimator** that estimates probabilities/expectations under a chosen scheduler policy, using variance-reduction techniques (importance sampling, splitting) and confidence intervals.

We assume the simulation is single-threaded and deterministic given:
- initial state
- scheduler tie-breaking decisions
- random samples (arrival/service times, jitter, etc.)

---

## 1) System Model: Timed DES as Decisions + Randomness

At any point, the simulator state can be seen as:

- Current simulated time `t_now`
- Scheduled event queue: events with timestamps `t_event`
- Async runtime internal structures (ready queue, timers, waker queues)
- Application state (queues, servers, clients, metrics, etc.)
- RNG state (or a log of draws)

**Time stratification:**
- Let `t_min` be the minimum scheduled timestamp.
- If `t_min > t_now`, time advances deterministically: `t_now := t_min`.
- The **choice frontier** at time `t_now` is `E = { e | t_event(e) == t_now }`.

**Where nondeterminism lives (controllable):**
- Ordering of events within the frontier `E`
- Potentially: ordering of ready tasks polled within the runtime at the same time (if exposed as a choice)
- Potentially: ordering of wake deliveries if not already normalized

**Where probability lives (stochastic):**
- Sampling of arrival/service delays, network delays, jitter, etc.

With general distributions, the underlying mathematical object is closest to a **GSMP / SMDP with nondeterminism**:
- Decisions at event boundaries and within equal-time frontiers
- Random sampling that schedules new future events

---

## 2) Trace Semantics (for replay and debugging)

A single run can be made reproducible by recording:

- **Scheduler choices**: when multiple same-time DES events are available, record which one is selected.
- **Tokio ready-task choices (optional)**: when multiple async tasks are ready in a single runtime poll cycle, record which task is polled next.
- **Random choices**: record each sampled value (and/or its “source label”: arrival time, service time, etc.).
- **Concurrency observations (optional)**: record mutex/atomic events and validate them during replay.
- Optional: record high-level invariants, property monitors, and a compact event log.

This yields a replayable “counterexample trace” with:
- initial seed/config
- list of scheduler decisions
- list of async runtime decisions (if enabled)
- list of random draws (or derived scheduled times)
- optional concurrency event stream

---

## 3) Bug Finder: Two-Loop Exploration (Scheduler Search + Rare-Event Rollouts)

### 3.1 Goal

Find a trace that violates a property (deadline miss, queue overflow, assertion failure, safety invariant violation) with minimal effort even if it is extremely rare under nominal randomness and under the default FIFO scheduler.

### 3.2 Key idea

Use two nested loops:

- **Outer loop**: choose/optimize *scheduler decisions* (nondeterministic interleavings).
- **Inner loop**: for a given schedule-policy prefix, run multiple *stochastic rollouts* with biased randomness (importance sampling / splitting) to reach rare states.

This mirrors adversarial testing in stochastic systems:
- The adversary controls “interleavings”
- The environment produces random delays
- We search for “bad combinations”

### 3.3 Outer loop: scheduling adversary/search

Decision points:
- When the event frontier `E` at time `t_now` has size > 1
- (Optional) when the runtime ready-queue has multiple tasks to poll at the same `t_now`

Search strategies:
- **DFS with backtracking (bounded)**: systematically enumerate schedules up to depth/time bound.
- **Best-first / A\***: prioritize schedule choices that increase a risk score (see below).
- **Bandit/MCTS**: treat schedule choices as actions, use rollouts to estimate which actions lead to failure.
- **Adversarial RL**: learn a scheduler policy that maximizes probability of failure (useful for bug finding; no guarantee of global optimum).

Bounding:
- Max number of scheduler decisions
- Max number of executed events
- Max simulated time horizon
- Stop when property violated

### 3.4 Inner loop: stochastic rollouts (rare-event biased)

For each outer-loop node (schedule prefix), evaluate “how promising” it is by running rollouts with biased randomness:

- If any rollout finds a violation → we have a bug + a trace.
- Otherwise, use rollout outcomes to guide outer-loop search (e.g., which schedule choice increases risk).

**Risk score examples (for guiding search):**
- approaching deadlines: `max(0, now - deadline)` or slack
- queue length / utilization metrics
- number of retries/timeouts
- in-flight requests
- “lateness” in server pipeline stages
- distance-to-bad threshold (domain-specific)

---

## 4) Rare Event Techniques for Bug Finding

Bug finding does not require unbiased probability estimates. This is important: you can be aggressively biased to trigger failures quickly.

### 4.1 “Race focusing” bias (timing alignment)

Many corner cases require *near-coincident* events (timeout vs response, stop vs recv, etc.). To trigger them:
- Bias delays so that two key events land at the same `SimTime` or within a tiny window.

Examples:
- service time ≈ timeout duration
- network delay ≈ deadline slack
- stop signal arrives exactly when server is about to poll recv

Implementation pattern:
- Identify “critical pairs” in the state (e.g., pending deadline D, expected service completion).
- Propose a distribution that increases probability mass near the collision condition.

This is extremely effective for concurrency-like bugs in a timed DES.

### 4.2 Load/burst bias (queue blow-up)

Many failures are caused by transient overload:
- Bias inter-arrival times smaller (burstier)
- Bias service times larger or heavier-tailed
- Bias correlated bursts (e.g., several short inter-arrivals in a row)

Even without exact PDF knowledge, bug finding can use:
- scenario-based “burst injection modes” occasionally overriding sampling

### 4.3 Splitting / cloning (multilevel splitting)

Define an importance function `S(state)` measuring “closeness to failure”.
When a run crosses thresholds (levels), clone it into multiple independent continuations.

Example levels:
- queue length crosses 10, 20, 30
- slack crosses 5ms, 2ms, 1ms
- retry count crosses 1, 2, 3
- latency percentile proxy crosses threshold

Why splitting helps:
- The probability of reaching deep rare regions is tiny; splitting produces many samples conditioned on “we made it this far”.

For bug finding, splitting can be used without strict accounting:
- If any clone fails → you found a failing trace.
- You can prioritize deeper-level clones and stop early.

### 4.4 Cross-entropy style adaptation (for bug finding)

Even for bug finding, adaptive bias helps:
- Run a batch, keep the “most risky” traces (highest `S(state)`).
- Fit new sampling parameters to resemble those traces.
- Repeat until failures appear frequently.

This is often how one quickly discovers “what kind of random draws trigger failure”.

---

## 5) Quantitative Estimator (Fixed Policy) with General Distributions

### 5.1 Goal

Estimate quantities under a specified scheduler policy (e.g., deterministic FIFO) such as:
- `P(deadline_miss)`
- `P(queue_overflow)`
- `E(latency)` or tail probabilities `P(latency > L)`

Since distributions are general, we rely on simulation plus variance reduction.

### 5.2 Baseline: Statistical Model Checking (SMC)

- Run N independent traces under the fixed policy.
- Estimate probability via sample mean.
- Provide confidence intervals (Clopper–Pearson / Wilson / Hoeffding bounds).
- For expectations, use standard CI on the mean (or bootstrap).

This scales well but struggles with extremely rare events.

---

## 6) Importance Sampling (IS) for Estimation (assuming approximate pdf/cdf)

We assume we can approximate `pdf(x)` / `cdf(x)` from data. That enables principled IS:

- True density: `f(x)` (estimated from data)
- Proposal density: `g(x)` (chosen to make failures more common)

For a run with samples `x1, x2, ..., xk`, the likelihood ratio weight is:
- `W = ∏ (f(xi) / g(xi))`

Work in log-space for stability.

Estimator for a failure event `A`:
- `P(A) ≈ (1/N) Σ [ I(run_i ∈ A) * W_i ]`

Key practical techniques for choosing `g`:

### 6.1 Exponential tilting / hazard-rate tilting (when applicable)

If you have a parametric family or can approximate one, tilt to increase tail probability:
- Make service times longer / more variable
- Make inter-arrival times shorter

Even if the true distribution is empirical, you can define `g` as a tilted parametric approximation and compute `f/g` using estimated densities.

### 6.2 Mixture proposals (robust and safer)

Use:
- `g(x) = (1-ε) f(x) + ε h(x)`

where `h(x)` is a “stress” distribution (heavier tail, shorter arrivals, collision-focused).

This avoids catastrophic weight blow-ups when `g` puts low density where `f` is high.

### 6.3 Conditional / “collision-focused” proposals

If failures require near-equality constraints, define `h` to oversample near those conditions (race focusing).

This can dramatically reduce variance when the rare event is essentially a narrow timing manifold.

### 6.4 Adaptive IS via cross-entropy (for estimation)

Iteratively tune proposal parameters to minimize estimator variance:
- Run batch under current `g`
- Keep “elite” runs that are closest to failure (or are failures)
- Update parameters to increase likelihood of elite set
- Repeat and then estimate using final `g` (or a mixture over iterations)

---

## 7) Splitting for Estimation (multilevel / subset simulation)

Splitting can also be used for quantitative estimation (not just bug finding), with proper bookkeeping.

Idea:
- Choose levels `L0 < L1 < ... < Lm` over score `S(state)`
- Estimate `P(reach failure)` as product of conditional probabilities:
  - `P(failure) = P(reach L1) * P(reach L2 | reach L1) * ... * P(failure | reach Lm)`

Implementation can fix the number of trajectories per level (fixed effort) to control variance.

This often works better than IS when:
- You can define a good monotone “progress to failure” score.
- The rare event is not well-captured by a single tilted distribution.

---

## 8) Combining estimation with scheduler nondeterminism (what’s feasible)

With nondeterministic scheduling, the rigorous object is closer to an MDP/SMDP with probabilistic transitions.
Exact worst-case probability over all schedulers is typically infeasible at scale with general distributions.

Feasible approaches:
- **(A) Fixed policy estimation** (rigorous, scalable): estimate under FIFO (or any chosen deterministic policy).
- **(B) Stress-policy estimation** (practical): learn an adversarial scheduler policy (via MCTS/RL) that increases failure rate, then estimate probability under that learned policy.
  - This provides a “plausible worst-case policy” and is very useful for engineering, but not a certified supremum over all schedulers.

---

## 9) Implementation Architecture (conceptual)

### 9.1 Centralize random sampling

All stochastic delays should be drawn via a single “randomness provider” so that we can:
- log draws for replay
- plug in biased sampling (`g`) for IS
- implement splitting / cloning semantics

### 9.2 Controlled scheduling interface

Expose a scheduler API that at each frontier:
- enumerates enabled same-time events
- lets a “strategy” choose the next event

Strategies:
- default deterministic FIFO (current behavior)
- adversarial search strategy (bug finder)
- learned policy (stress scheduler)

### 9.3 Trace + replay

Support:
- record: schedule decisions + random draws
- replay deterministically
- minimal counterexample shrinking (optional): try removing irrelevant choices/draws while preserving failure

### 9.4 Site IDs for Random Sampling

Random draws are tagged with stable site identifiers using the `descartes_core::draw_site!("tag")` macro, which computes a hash from the call site (module path, file, line, column, and user tag). This provides:

- **Automatic stability**: No manual ID management for typical use cases
- **Refactor sensitivity**: Moving code changes the hash, which may break trace replay
- **Explicit override**: Users can pass custom `DrawSite` with explicit IDs when needed

### 9.5 Trace Formats

Traces support multiple serialization formats selectable via configuration:

- **JSON**: Human-readable format for debugging and inspection
- **Postcard**: Compact binary format for storage efficiency and fast I/O

### 9.6 Harness Interface

The exploration harness provides a standardized setup pattern:

- `setup(SimulationConfig) -> Simulation`: User-provided function that initializes the simulation
- Optional `descartes_tokio::runtime::install`: Installs the async runtime for simulations that need it
- Enables reproducible exploration across different scenarios and configurations

---

## 10) Open Questions / Required Inputs

1) What are the primary “bad events”?
   - deadline miss? panic? queue overflow? liveness?
2) What state-derived score `S(state)` best measures “closeness to failure”?
3) Which random variables exist, and can we label them by type (arrival/service/network/etc.)?
4) Are there known “races” to target (timeout vs response, cancellation vs recv, stop vs accept)?
5) Do we need correlation models (bursts) or is IID sampling acceptable initially?

---

## 11) Implementation Plan (Milestones)

This plan aims to add systematic exploration without taking away existing features or code ergonomics.

### Staging and review

Implementation should be delivered in small stages. After each stage, validate the build and review the API/ergonomics before proceeding.

### Guiding constraints

- **No behavior change by default**: existing simulations keep deterministic single-threaded semantics.
- **Exploration is opt-in**: introduced as a separate crate (suggested: `des-explore`) and/or behind feature flags.
- **Reproducibility**: every “found bug” is replayable via recorded scheduler decisions + random draws.
- **Incremental adoption**: tagged randomness and monitors can be added gradually by users.

### Milestone 0: Make tie-breaking explicit (if needed)

- Ensure the DES scheduler has a deterministic, stable tie-breaker for same-`SimTime` events (e.g., `(time, sequence_id)`).
- This is required for meaningful replay and for defining a well-posed “same-time frontier”.

### Milestone 1: Trace recording + replay

- Implement a `Trace` format capturing:
  - scheduler choices among same-time frontiers (when controlled)
  - random draws (tag + site id + sampled value)
  - run metadata (seed, horizon, scenario/spike parameters)
- Add a `TraceReplayer` to deterministically reproduce a run from a user-provided setup function.

### Milestone 2: Tagged randomness provider (opt-in)

- Introduce a centralized sampling abstraction (e.g., `RandomProvider`) that supports:
  - nominal sampling (baseline)
  - biased sampling (proposal distributions)
  - returning `log(f/g)` contributions (for IS estimators)
  - cloning streams (for splitting)
- Add lightweight tagging:
  - `tag`: `arrival`, `service`, `network`, `retry`, `timeout`, ...
  - `site_id`: stable identifier for the draw site
  - optional context map (endpoint, request class, etc.)

### Milestone 3: Windowed observables + metastability monitor

A "monitor" is an exploration-focused analysis layer that produces:
- a **trajectory** of windowed observables (time series),
- online detection of **recovery** and **metastability**, and
- a scalar progress score `S(state)` used by splitting.

#### Relation to `des-metrics`

There is overlap in observables, but the monitor has different requirements:

- `des-metrics` focuses on **observability/reporting** (metrics ecosystem, export).
- The exploration monitor focuses on **online decision-making** (window summaries, baseline distance, `S(state)`), with bounded memory and low overhead across many rollouts.

The recommended design is a monitor in `des-explore` that can optionally reuse:
- `descartes_metrics::RequestTracker` (retry/timeout/latency/throughput stats)
- `descartes_metrics::TimeSeriesCollector` (windowed time series)

while keeping exploration-critical computations (baseline distance, recovery predicates, splitting score) inside `des-explore`.

#### Instrumentation options

- **Option A (recommended): shared monitor handle**
  - Components hold an `Arc<Mutex<Monitor>>` (or `Rc<RefCell<_>>`) and call `monitor.on_*()` directly.
  - Pros: low overhead, no extra scheduled events, best for large numbers of rollouts.
  - Cons: requires explicit instrumentation sites.

- **Option B: monitor as a DES component**
  - Components schedule `MonitorEvent` messages to a monitor component.
  - Pros: pure DES event model, explicit in event log.
  - Cons: extra events can perturb performance and increases per-rollout overhead.

#### Implementation details

- Add a monitor that computes windowed summaries for:
  - queue backlog (mean/max)
  - latency distribution proxies (mean + tail proxy)
  - drop rate
  - retry rate / amplification factor
  - throughput
- Build a baseline distribution from a warmup period.
- Define recovery as “return to baseline distribution” using a configurable distance `D` (default: standardized feature vector + diagonal Mahalanobis).
- Define metastable terminal conditions (e.g., `recovery_time > T_max` or sustained `D > ε`).
- Expose a scalar score `S(state)` derived from recent window summaries.

### Milestone 4: Splitting-based bug finder (inner loop)

- Implement multilevel splitting using a progress score `S(state)` derived from the monitor.
- Focus on barrier-crossing + persistence (metastability): clone trajectories at increasing levels of `S`.
- Output: first counterexample trace + metrics timeline.

### Milestone 5: Outer-loop scheduling exploration (two-loop bug finder)

- Add a controlled executor/driver that, at each same-time frontier, can:
  - enumerate enabled events
  - delegate selection to a strategy
- Strategies:
  - deterministic (current behavior)
  - DFS/backtracking (small models)
  - MCTS / bandit selection with rollout-based scoring
- Combine with Milestone 4 rollouts so the outer loop learns which scheduler choices promote metastability.

### Milestone 6: Quantitative estimation under a fixed policy (prototype)

Implemented in `des-explore`:
- Probability of a terminal property under a fixed policy:
  - naïve Monte Carlo `P(A)` with a confidence interval
  - multilevel splitting probability estimator (product of conditional probabilities)

### Milestone 6.1: Estimation rigor + steady-state metrics (planned)

This milestone tightens the statistical story and expands beyond terminal-event probabilities.

**A) Formalize estimands (what we estimate)**
- Bernoulli properties: `P(A)` where `A` is a terminal predicate over run state.
- Time-averaged / steady-state means: `E[X]` where `X` is a time series (window metrics) or
  a per-request sample stream.
- Tail probabilities: `P(X > x)` for either window metrics or request-level samples.

**B) Metric sampling API (two sources)**
- Window-level metrics (from `Monitor`):
  - Treat each monitor window summary as one sample (e.g., throughput, retry rate, mean queue).
  - Support burn-in by discarding the first `W_warmup` windows.
- Request-level metrics (from the scenario):
  - Provide a simple, explicit mechanism for scenarios to record per-request samples such as
    latency, success/failure, and retry count.
  - Two recommended patterns:
    - store raw samples in a `Vec<f64>` (simplest; fine for small/medium experiments)
    - store streaming summaries (e.g., `hdrhistogram`) for large runs, and separately track tail
      events for `P(latency > L)` style estimands

**C) Long-run averages via independent replications (baseline)**
- Run `R` independent replications under the fixed policy.
- For each replication, discard warmup (time or windows) and compute a replication mean.
- Estimate `E[X]` via the mean of replication means with a t-interval CI.

**D) Long-run averages via batch means / overlapping batch means**
- Run one long replication; discard warmup; split the remaining sample stream into `K` batches.
- Use batch means to form an approximate t-interval CI.
- Guardrails:
  - require `K >= 10` (or similar) and a minimum batch size
  - return “insufficient data” rather than producing unstable CIs

**E) Splitting probability estimator with confidence intervals via repeated runs**
- Keep the splitting estimator definition `P(A) = ∏ p_i` (product of conditional stage probabilities).
- For confidence intervals, prefer repeated *independent* splitting runs:
  - run splitting `M` times with independent base seeds
  - aggregate on `log(p_hat)` (log space) and form a t-interval CI
  - exponentiate back to probability space
- Diagnostics to record per run:
  - per-stage success counts and conditional probabilities
  - detect degenerate stages (`p_i = 0` or `p_i = 1`)
  - recommended target per-stage success probability (classic guidance: ~0.1–0.3)

**F) Calibration / validation suite**
- Add small toy models with known truth to validate estimators and CI behavior:
  - Bernoulli event with known `p` (including a rare `p` case)
  - a simple queueing model with known mean metrics (sanity check)
- Validation should focus on non-flaky sanity checks (CI width trends, rough accuracy), and keep
  heavier coverage-style experiments behind `#[ignore]`.

### Milestone 7: Importance sampling for estimation (pdf/cdf approximated from data)

- Implement proposal distributions `g` per `(tag, site_id)`.
- Default to robust mixture proposals: `g = (1-ε)f + εh`.
- Support adaptive IS (cross-entropy style) to reduce variance.

### Milestone 8: Correlation models (metastability realism)

- Add correlated generators driven by a latent process `Z(t)` (Markov-modulated / semi-Markov regimes).
- Optionally add self-exciting models (Hawkes-like) for retries.
- Ensure likelihood ratio accounting works primarily at the latent-driver level (more stable than weighting many per-request draws).

### Milestone 9: Ergonomics + artifacts

- Provide high-level APIs such as:
  - `BugFinder::run(setup, scenario, predicate)`
  - `Estimator::estimate(setup, scenario, metric)`
- Provide trace artifacts:
  - compact binary/JSON trace
  - CSV time series for observables
  - “shrinker” (optional) to minimize counterexample traces.

### Milestone 10: Search over scenario parameters (future)

In addition to exploring stochastic trajectories (random draw sequences), add an
outer exploration layer that searches over **scenario parameters** such as:
- spike magnitude/duration/shape, capacity reduction shape
- timeout / retry policy parameters (max retries, backoff/jitter)
- routing/admission knobs or other policy controls

Design goals:
- Reuse the same harness/monitor/trace machinery.
- Keep scenario search opt-in and separate from the DES core.

Implementation sketch:
- Represent scenarios as a small param struct (or enum) plus a deterministic
  `apply(&mut Simulation, &Scenario)` step inside the `setup` closure.
- Search strategies:
  - grid/random search for quick wins
  - Bayesian optimization / bandits when evaluations are expensive
  - nested splitting: scenario-search outer loop + splitting inner loop

---

## 12) Current Prototype: Metastable Retry Storm Example

A concrete metastability scenario exists as an executable example:

- `des-core/examples/metastable_retry_storm.rs`

It models an M/M/1 queue with bounded capacity and client timeouts that trigger retries (no backoff). Even after a workload spike ends, retries can maintain high offered load and keep the queue near capacity.
