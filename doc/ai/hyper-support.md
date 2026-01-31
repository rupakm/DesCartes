# Hyper 1.x Type Support In DES (Tower-Focused)

Goal: run tower middleware/services inside DES with minimal changes, while keeping the simulator transport model simple (no full HTTP/2 stack).

This plan targets (a): reuse of tower services/middleware that commonly use `http` + `http-body` types (and are often deployed with hyper).

## Non-goals

- Do not attempt to run hyper’s actual networking/runtime (IO) stack inside DES.
- Do not model HTTP/1 or HTTP/2 protocol behavior (connection management, HPACK, stream-level flow control, trailers, etc.).
- Do not aim for drop-in compatibility with tonic’s `tonic::transport::{Server, Channel}` (that requires HTTP/2 + gRPC framing semantics).

## Design Principle

Support the *type boundary* that tower middleware/services compose around:

- `tower::Service<http::Request<B>> -> http::Response<B2>`
- Bodies implement `http_body::Body<Data = bytes::Bytes>`

In hyper 1.x, concrete bodies like `hyper::body::Incoming` are receive-only (produced by hyper’s IO). We should avoid requiring those in service signatures.

## Canonical Simulator Types (Hyper 1.x friendly)

Standardize on the hyper 1.x ecosystem versions:

- `http` 1.x
- `http-body` 1.x
- `bytes` 1.x
- `http-body-util` 0.1

Choose a canonical body type that is constructible and ergonomic in a simulator:

- Unary/full-buffer payloads: `http_body_util::Full<bytes::Bytes>`
- General-purpose: `http_body_util::combinators::BoxBody<bytes::Bytes, std::convert::Infallible>`

Recommended public boundary for simulator-facing APIs:

- `type Body = BoxBody<Bytes, Infallible>`
- `type Request = http::Request<Body>`
- `type Response = http::Response<Body>`

This aligns with how many hyper 1.x era libraries already box bodies for type-erasure.

## Provide a Compatibility Module/Crate

Add a small compatibility crate/module (name TBD, e.g. `des-http` or `des-hyper`) that:

- Re-exports the canonical versions of `http`, `http-body`, `http-body-util`, and `bytes` to prevent version-split drift.
- Provides canonical aliases:
  - `Body`, `Request`, `Response`
- Provides constructors/helpers:
  - `body::empty() -> Body`
  - `body::full(bytes: impl Into<Bytes>) -> Body`
  - `body::boxed<B>(body: B) -> Body` where `B: http_body::Body<Data = Bytes, Error = Infallible> + Send + 'static`
  - Optional: `body::collect<B>(body: B) -> Bytes` for buffering when needed.

The goal is that services only change imports and, at most, their body alias.

## Integrate With DES Tower

There is already a tower integration in this repository (`des-tower`) using `http` request/response types and a simple simulated body.

Next steps for hyper 1.x alignment:

1) Ensure `des-tower` uses `http` 1.x + `http-body` 1.x traits.
2) Make the public-facing body type match the canonical boundary (prefer `BoxBody<Bytes, Infallible>`).
   - Keep the existing single-buffer body (e.g. `SimBody`) as an internal fast path.
   - Provide `From<SimBody> for Body` and helpers to create `SimBody`/`Body` from bytes.
3) Ensure `DesService` and layers implement `tower::Service<http::Request<Body>>`.

## Simulation Semantics (Overload Must Affect Sim-Time)

To avoid “overload is invisible in sim-time”, the service model must include capacity/backpressure:

- Concurrency/worker capacity (e.g. a semaphore limiting active handlers)
- Queueing delay when capacity is exceeded (requests wait for a permit)
- Optional bounded queues with explicit overload behavior (drop/reject)
- Service time distribution (deterministic or sampled)

This belongs in `des-tower` (in-process service simulation) and/or in a networked adapter if modeling endpoints.

## Optional: Networked HTTP-Shaped Messages

If needed later, support “HTTP-shaped” request/response transport without modeling HTTP protocol:

- Serialize `(method, uri, headers, body)` into DES transport messages
- Deliver to a remote component which runs the tower service
- Serialize the response back

This supports multi-node simulations and network latency/loss, while still avoiding full HTTP semantics.

## Version Alignment Risk (Primary Practical Pitfall)

The biggest risk to "seamless" support is dependency version splits:

- hyper 1.x stacks generally use `http` 1.x / `http-body` 1.x.
- older tower/hyper 0.14-era code uses `http` 0.2 / `http-body` 0.4.

If both exist in the graph, you effectively get two incompatible `http::Request` types.
Mitigation:

- Choose and enforce the canonical versions at the simulator boundary.
- Provide adapters only where necessary, but avoid mixing eras in core APIs.

## Phased Implementation Plan

1) Inventory target services’ dependency versions (http/tower/axum/hyper/http-body-util).
2) Introduce `des-http`/`des-hyper` compatibility module with canonical `Body/Request/Response` aliases + constructors.
3) Update/align `des-tower` to the canonical boundary (`Request<Body> -> Response<Body>`).
4) Provide adapters so services can accept any `B: Body<Data=Bytes>` and be boxed.
5) Add examples:
   - A small middleware pipeline (timeout/retry/concurrency limit) using `http::Request<Body>`.
   - Optional: axum router running in-process in DES.

## Open Questions

- Do we need streaming bodies (Tier B), or is full-buffer `Full<Bytes>`/boxed bodies sufficient for most use cases?
- Should the compatibility layer live in `des-tower` (re-exported), or in a separate crate to avoid forcing tower dependency on users?
- What is the primary target stack: tower-only, axum 0.7, or custom services?
