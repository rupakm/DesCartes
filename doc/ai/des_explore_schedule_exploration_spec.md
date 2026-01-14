# `des-explore` Schedule Exploration Spec (v1 + v2)

**Status:** Proposed (implementation-oriented)

**Source of truth:** This is the authoritative specification for *schedule exploration* (backtracking, policy search, and later DPOR/MCTS) in this workspace.

Related documents in `doc/ai/` provide background and record/replay details:
- `doc/ai/des_event_scheduling_policies.md` (frontier + tokio policy hooks, recording/replay)
- `doc/ai/des_tokio_concurrency_replay.md` (mutex/atomic tracing + replay validation)
- `doc/ai/des_exploration_design.md` (older conceptual overview)

---

## 0) Problem Statement

We want a rollout-based exploration tool that can approximate bounded-horizon, worst-scheduler quantities such as:

- **Worst-scheduler reachability probability** (bounded time):
  - \(\sup_{\sigma} \Pr^{\sigma}(\Diamond_{\le T}\ \text{Bad})\)

Where:
- \(T\) is a fixed simulation horizon.
- Randomness includes **both**:
  - model randomness (arrival/service/network/etc.)
  - scheduler-policy randomness (if the policy is randomized)
- The scheduler \(\sigma\) is **non-anticipative** (may depend on the past, but not future draws).

For v1 we accept:
- **High-confidence lower bounds** + **replayable counterexample traces**.
- No requirement for formal (sound) upper bounds.

---

## 1) Scope

### v1 scope (implement first)

- Decision points explored:
  - DES same-time **frontier ordering** decisions.
  - `des_tokio` **ready-task ordering** decisions.
- Exploration strategies:
  - Backtracking core (rerun + replay prefix + force a deviation).
  - **Policy search** (Algorithm 2): optimize a scheduler policy using rollouts.
- Output:
  - Found counterexample traces (`Trace`) that reproduce failures.
  - Empirical estimate of \(\Pr(\Diamond_{\le T} \text{Bad})\) under the best-found policy, with a **corrected** confidence lower bound.

### v1 non-goals

- DPOR / POR reductions.
- Mutex waiter scheduling exploration.
- Sound upper bounds on \(\sup_{\sigma} \Pr(\cdot)\).
- Full MCTS (Algorithm 1) (scheduled for v2).

### v2 scope (next)

- DPOR enhancements and additional decision points (mutex waiters).
- MCTS (Algorithm 1).
- Better policy families + variance reduction plumbing.

---

## 2) Terminology and Invariants

### 2.1 Rollout

A *rollout* is a single execution from time 0 to horizon \(T\) (or earlier termination), under:
- a scheduler policy \(\pi\) (possibly randomized)
- a model-randomness stream

It produces:
- a boolean outcome `hit_bad`
- optional metrics/reward
- a replayable `Trace` containing the realized decisions and RNG draws

### 2.2 Decision points

Decision points are the only sources of nondeterministic interleavings we control.

For v1 we standardize two kinds:

- `DecisionKind::Frontier`
  - Observation: `(time_nanos, frontier_seqs: Vec<u64>)`
  - Choice: `chosen_seq: u64` (preferred) or `chosen_index`

- `DecisionKind::TokioReady`
  - Observation: `(time_nanos, ready_task_ids: Vec<u64>)`
  - Choice: `chosen_task_id: u64` (preferred) or `chosen_index`

**Invariants required for exploration correctness:**
- Frontier `seq` identifiers must be stable within a run (already true).
- Tokio ready task IDs must be stable within a run (already true).
- Replay must validate that the observed choice set signature matches what the script expects.

---

## 3) Core Building Block: Run-With-Control (Rerun + Replay + Force)

Everything in v1 and v2 is built around a single primitive:

> `run_rollout(control, seeds) -> RolloutResult`

Where `control` can:
- replay a trace prefix (for deterministic re-entry into the same partial execution)
- override the next decision at some decision point(s)
- otherwise delegate to a base policy (FIFO or randomized)

### 3.1 DecisionScript

A `DecisionScript` is a partial function that overrides some decisions.

- Keys match observed decision points by **signature**, not by “decision index”, to be robust:
  - `DecisionKey = { kind, time_nanos, choice_set_ids }`
- Values select an element by stable ID:
  - frontier: `chosen_seq`
  - tokio-ready: `chosen_task_id`

When a script does not specify a decision:
- the installed policy chooses it (FIFO or randomized, depending on configuration).

### 3.2 Control policies (installed into the simulation)

For v1 we implement exploration wrappers that combine:

1) **override** (DecisionScript)
2) **policy** (FIFO or seeded randomized)
3) **recording** (always record realized decisions)

Concretely:
- `ExplorationFrontierPolicy<P>` wraps `P: EventFrontierPolicy`.
- `ExplorationReadyTaskPolicy<P>` wraps `P: ReadyTaskPolicy`.

Each wrapper:
- observes the choice set
- applies forced choice if present
- otherwise delegates to `P`
- records the realized decision to the trace (so counterexamples always replay)

### 3.3 Seeds and randomness accounting

Because the probability is over **both** model and scheduler-policy randomness, each rollout must be seeded for:
- model RNG provider(s)
- scheduler policy RNG (if used)

v1 requirement:
- `RolloutSeed` is deterministically derived from a base seed and trial index.
- For a given trial index, *all candidate policies* should use the same model seed (CRN for variance reduction).
- Scheduler-policy seeds may also be held fixed per trial index (recommended) so policy comparisons are less noisy.

---

## 4) Trace Extraction API (for exploration algorithms)

All higher-level algorithms should operate on a normalized view of decision points.

### 4.1 Normalized decision event

Define:

- `ObservedDecision { key: DecisionKey, chosen: DecisionChoice }`

Implement:
- `fn extract_decisions(trace: &Trace) -> Vec<ObservedDecision>`

This function is pure (no simulation access) and is used by:
- backtracking tree building
- policy learning/updating
- counterexample summarization

---

## 5) v1 Algorithm: Policy Search (Algorithm 2)

### 5.1 Goal

Find a policy \(\pi\) that makes `Bad` likely by \(T\), and report:
- replayable counterexample traces
- a high-confidence lower bound on \(\Pr^{\pi}(\Diamond_{\le T} \text{Bad})\)

This is explicitly *not* a certified supremum over all schedulers.

### 5.2 Policy family (v1)

Implement a **table policy with fallback**:

- `TablePolicy` stores a map:
  - `DecisionKey -> ActionDistribution`
- If a `DecisionKey` is not in the table:
  - fall back to the base policy (FIFO by default)

`ActionDistribution` v1 options:
- deterministic: “always pick chosen_id”
- randomized over IDs (categorical weights)

Rationale:
- This is compatible with backtracking and replay.
- It avoids requiring static state abstraction.
- It incrementally adapts to decision keys that actually occur.

### 5.3 Search loop (v1)

A simple, robust v1 loop is “iterative mutation + selection”:

Inputs:
- `budget_policies`: max number of candidate policies evaluated
- `trials_per_policy`: number of rollouts per policy evaluation
- `horizon T`

Loop:
1) Start from `best_policy = BasePolicy`.
2) For `k in 1..=budget_policies`:
   - Propose `candidate = mutate(best_policy)`.
   - Evaluate `candidate` on `trials_per_policy` rollouts.
   - If `candidate` improves the estimated hit rate (or a lower CI bound), accept it as `best_policy`.
   - Save any `Bad` traces from evaluation as counterexamples.

Mutation operators (v1):
- pick a previously seen `DecisionKey` and flip to a different action
- add a new table entry for a newly observed multi-choice `DecisionKey`
- perturb categorical weights slightly (if using randomized policies)

### 5.4 Estimation + confidence lower bounds

For a fixed policy \(\pi\), estimate:

- \(\hat p = s/n\) where `s` successes in `n` trials.
- Compute a Wilson (or Clopper–Pearson) lower bound `LB(\pi, α)`.

Because policy search is adaptive (“we looked at many policies”), v1 should report a conservative bound using one of:

**Option A (recommended): holdout evaluation**
- Use rollouts during search only for optimization.
- After selecting the best policy, run an independent evaluation of `N_eval` trials.
- Report `LB(best, α)` using only holdout.

**Option B: Bonferroni correction**
- If you evaluate `K` policies and want overall confidence `1-δ`, use `α = δ/K` for each.
- Report `max_i LB(\pi_i, δ/K)`.

v1 should implement Option A (simpler + less pessimistic).

### 5.5 Counterexample artifacts

When `Bad` is reached in a rollout:
- persist the `Trace` (contains realized scheduler decisions + RNG draws)
- also persist:
  - the policy snapshot (serialized)
  - the rollout seed(s)
  - the horizon and scenario label

A counterexample must be replayable via `run_replayed(...)`.

---

## 6) v1 Public API (proposed)

This section describes API surface, not exact names.

### 6.1 Rollout runner

- `struct RolloutConfig { horizon: SimTime, install_tokio: bool, record_trace: bool, ... }`
- `struct RolloutResult { hit_bad: bool, trace: Option<Trace>, decisions: Vec<ObservedDecision>, ... }`

- `fn run_rollout(
    cfg: RolloutConfig,
    policy: &dyn SchedulePolicy,
    seed: RolloutSeed,
    setup: impl FnOnce(SimulationConfig, &HarnessContext, Option<&Trace>, u64) -> Simulation,
    predicate: impl Fn(&Simulation) -> bool,
) -> RolloutResult`

Notes:
- `setup` should use `HarnessContext::shared_branching_provider(...)` so all distributions share one ordered random stream.
- `predicate` should be bounded-horizon and checked during stepping or via monitor.

### 6.2 Policy search

- `struct PolicySearchConfig { horizon: SimTime, budget_policies: usize, trials_per_policy: u64, eval_trials: u64, confidence: f64, ... }`
- `struct PolicySearchReport { best_policy: PolicySnapshot, p_hat: f64, lb: f64, counterexamples: Vec<TraceMeta>, ... }`

- `fn search_policy(cfg: PolicySearchConfig, setup: ..., predicate: ...) -> PolicySearchReport`

---

## 7) v2 Spec (DPOR + MCTS + postponed features)

### 7.1 Add mutex waiter exploration

Extend exploration decision points with:

- `DecisionKind::MutexWaiter`
  - Observation: `(time_nanos, mutex_id, waiters: Vec<(waiter_id, task_id)>)`
  - Choice: `chosen_waiter_id`

Requirements:
- Add a new trace event `TraceEvent::MutexWaiterDecision` (new schema element).
- Provide `RecordingMutexWaiterPolicy` and `ReplayMutexWaiterPolicy` analogous to frontier/tokio.

### 7.2 DPOR enhancements (stateless, rollout-based)

Goal: reduce redundant schedule exploration by avoiding permutations of *independent* actions.

Constraints:
- No static analysis.
- Dependence must be inferred dynamically from runtime observations.

v2 adds **optional semantic instrumentation** to support DPOR:

- Transport ops (recommended): add trace events for `SimTransport` send/deliver actions, including stable endpoint/message IDs.
- Component effect tags (optional): allow components/framework layers to emit “reads/writes/sends/receives” with stable IDs.

With these, define a conservative dependence relation:
- Two actions are dependent if they touch a shared resource ID:
  - same mutex_id
  - same atomic site_id (or explicit var id)
  - same transport endpoint/channel id
  - same component id (fallback)

DPOR algorithm sketch:
- Run a rollout under some schedule choices.
- Build a happens-before relation from observed ordering.
- Identify backtracking points where an alternative ordering of two dependent actions could occur.
- Re-run with a forced deviation at that backtracking point (DecisionScript), recursively.

Deliverable: a “DPOR explorer” that produces a set of schedules representative of equivalence classes, plus counterexample traces when found.

### 7.3 MCTS (Algorithm 1)

Add an adversarial MCTS driver that:
- treats scheduler decision points as action nodes
- uses rollouts to estimate probability of `Bad` by horizon
- supports progressive widening for large choice sets
- uses CRN (shared rollout seeds) to reduce variance when comparing actions

Output:
- best-found (possibly history-dependent) policy
- counterexample traces
- lower-bound estimate via holdout evaluation

### 7.4 Better policy families

Expand `TablePolicy` into:
- mixture policies (e.g., FIFO with probability 1-ε, else table)
- parameterized heuristic policies using frontier metadata (component/task kind, etc.)
- cross-entropy method (CEM) optimization for categorical distributions per decision key

### 7.5 Trace + tooling improvements

- Consistent “decision extraction” across all kinds (frontier, tokio-ready, mutex, DPOR semantic ops).
- CLI/reporting helpers:
  - summarize decision counts and branching factors
  - show which decision keys were influential in counterexamples

---

## 8) Acceptance Criteria

### v1 done when
- We can run `search_policy(...)` over frontier + tokio-ready decisions.
- It produces:
  - replayable counterexample traces when `Bad` is hit
  - a conservative high-confidence lower bound for the best policy (holdout eval)
- Exploration can be toggled independently for:
  - frontier
  - tokio-ready

### v2 done when
- Mutex waiter decisions can be explored and replayed.
- A DPOR explorer can reduce redundant schedules for models with many commuting events.
- MCTS driver exists and can outperform v1 policy search on benchmark scenarios.
