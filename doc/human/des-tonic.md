# `des-tonic`

`des-tonic` provides a tonic-like RPC façade that runs over simulated transport/network models.

## Goal

Make it easy to build and test client/server RPC behavior inside deterministic simulations.

## Highlights

- A `Transport` façade for installing a network model into a simulation.
- Convenience APIs for serving and connecting with `SocketAddr`-like inputs.
- Ability to swap network models (e.g., zero latency vs jitter) to study latency sensitivity.

## How it fits

- Builds on transport abstractions from `des-components`.
- Uses `des-tokio` for async execution.
- Works well with `des-explore` for record/replay of runs.
