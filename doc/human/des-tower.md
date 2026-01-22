# `des-tower`

`des-tower` integrates the Tower ecosystem with the DES runtime.

## Goal

Let you run realistic Tower stacks (layers/middleware/services) inside deterministic simulations.

## Typical use cases

- Simulate retry/timeout/circuit-breaker behavior under latency/congestion models.
- Evaluate tail latency / admission control policies deterministically.
- Combine with `des-explore` to record/replay “interesting” scheduling/interleaving behaviors.

## Notes

- Any async behavior still runs on `des-tokio` (simulated time).
- Deterministic by default; exploration policies are opt-in.
