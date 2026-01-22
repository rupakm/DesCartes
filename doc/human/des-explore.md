# `des-explore`

`des-explore` contains **opt-in** tools for exploring and reproducing behaviors in DES simulations.

## What it provides

- **Trace recording** of key sources of nondeterminism / branching:
  - Same-time scheduler frontier choices
  - Async runtime ready-task choices (when enabled)
  - RNG draws (where modeled)
  - Optional concurrency observation streams
- **Replay** support to reproduce an execution from a trace.
- **Replay validation** utilities (e.g., concurrency event stream validation).
- **Quantitative estimators** (e.g., Monte Carlo, splitting estimators, Wilson confidence intervals).

## Design intent

- Default simulations remain deterministic and FIFO.
- Exploration / randomized scheduling is explicit and seeded.
- Replay should fail with a structured error if behavior diverges from the trace.

See also:
- `doc/ai/des_exploration_design.md`
- `doc/ai/des_event_scheduling_policies.md`
