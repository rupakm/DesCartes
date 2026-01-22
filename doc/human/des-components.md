# `des-components`

`des-components` provides reusable building blocks for modeling distributed systems on top of `des-core`.

## What it provides (examples)

- Servers/clients and queueing models
- Throttles / admission control / rate limiting patterns
- Retry policies and backoff patterns
- Transport/network modeling helpers (used by `des-tonic`)

## How it fits

- Components schedule events into a `des-core::Simulation`.
- Many examples are easiest to write with `des-tokio` installed (async tasks and simulated time).
- Tower/tonic layers build higher-level APIs on these primitives.

See also:
- `des-components/README.md` (component-level details)
