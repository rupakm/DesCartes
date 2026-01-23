# `des-tonic` Streaming RPC Design (Server / Client / Bidi)

**Status:** Proposed

This document specifies how to add *streaming RPC* support to `des-tonic` while preserving the project’s core constraints:
- deterministic, deadlock-avoiding behavior under DES
- no HTTP/2 / gRPC wire compatibility
- uses `des-components::transport` for delivery and `des-tokio` for async tasks

This design intentionally prefers keeping transport generic and putting stream semantics into `des-tonic`’s wire framing.

## Relationship to Existing Docs

- `doc/ai/tonic_audit.md` is a historical audit of an older DES tonic implementation and is **not** a spec.
- This document is the *source of truth* for `des-tonic` streaming design.

---

## 1) Goals and Non-Goals

### Goals

- Support three RPC patterns:
  1) unary (existing)
  2) server streaming: `Request<Bytes> -> Stream<Bytes>`
  3) client streaming: `Stream<Bytes> -> Response<Bytes>`
  4) bidirectional streaming: `Stream<Bytes> -> Stream<Bytes>`

- Deterministic behavior:
  - per-stream in-order delivery as observed by the application (even if the network delivers out-of-order)
  - deterministic local scheduling given `des-explore` record/replay / exploration

- Backpressure:
  - prevent unbounded queue growth per stream
  - do not require OS-tokio primitives

- Replay/debuggability:
  - each stream has a stable `stream_id`
  - frames are traceable and correlate to a specific RPC and direction

### Non-goals (initial cut)

- gRPC semantics (headers/trailers compression, HTTP/2 flow control)
- full tonic API compatibility (e.g., `tonic::Streaming<T>` exact behavior)
- cross-process network transport

---

## 2) High-Level Architecture

### Existing unary architecture

- Client sends `UnaryRequestWire` to server via `TransportMessage` (payload bytes)
- Server replies with `UnaryResponseWire` correlated by `TransportMessage.correlation_id`
- Client `Channel` maps `correlation_id -> oneshot` in `pending`

### Streaming extension approach

**Key design choice:** do not modify `des-components::transport::TransportMessage` shape.

Instead:
- introduce a `StreamFrameWire` encoding inside `des-tonic` (`des-tonic/src/wire.rs`)
- send streaming frames as one transport message per frame
- correlate frames using:
  - `TransportMessage.correlation_id` set to a stable `stream_id`
  - plus fields inside `StreamFrameWire` (direction, seq, flags, status)

Transport may need only minimal changes:
- add `MessageType::RpcStreamFrame` in `des-components::transport::MessageType`

---

## 3) Wire Framing

### 3.1 MessageType

Add a new transport message type:

- `MessageType::RpcStreamFrame`

Unary remains unchanged.

### 3.2 StreamFrameWire

All streaming patterns (server/client/bidi) use the same wire frame.

```rust
enum StreamFrameKind {
    // First frame on a stream. Carries method and initial metadata.
    Open,

    // Payload message.
    Data,

    // Sender indicates it will send no more Data frames.
    // Includes final Status for server->client direction.
    Close,

    // Optional: immediate cancellation.
    Cancel,
}

enum StreamDirection {
    ClientToServer,
    ServerToClient,
}

struct StreamFrameWire {
    stream_id: String,
    direction: StreamDirection,
    seq: u64,
    kind: StreamFrameKind,

    // Present for Open.
    method: Option<String>,
    metadata: Vec<(String, String)>,

    // Present for Data.
    payload: Bytes,

    // Present for Close (server->client).
    status_ok: bool,
    status_code: tonic::Code,
    status_message: String,
}
```

Notes:
- `seq` is per-(stream_id, direction) monotonic.
- `Open` always uses `seq=0`.
- `Data` begins at `seq=1`.
- `Close` is the terminal frame; `Cancel` is an early terminal frame.

### 3.3 Ordering policy

The receiver must present frames to the application in `seq` order.

If network delivers out-of-order:
- buffer frames in a `BTreeMap<u64, Frame>` until `next_expected_seq` is present
- then drain in-order

---

## 4) Stream Lifecycle State Machines

We define a unified per-direction state machine.

### 4.1 Per-direction stream state (generic)

For a given stream and direction (C->S or S->C):

States:
- `Idle`: no frames seen
- `Open`: Open has been processed
- `HalfClosed`: Close/Cancel has been processed; no more Data expected

Transitions:
- `Idle --Open--> Open`
- `Open --Data--> Open`
- `Open --Close/Cancel--> HalfClosed`
- Any state receiving an invalid frame (e.g., Data before Open) -> treat as protocol error and Cancel the stream.

### 4.2 Full stream state (bi-directional)

Track both directions:

```rust
struct StreamState {
    stream_id: String,
    method: String,

    client_to_server: DirectionState,
    server_to_client: DirectionState,

    // Ordering
    next_c2s: u64,
    next_s2c: u64,
    reorder_c2s: BTreeMap<u64, Bytes>,
    reorder_s2c: BTreeMap<u64, Bytes>,

    // Application pipes
    inbound_tx: des_tokio::sync::mpsc::Sender<Bytes>,
    inbound_rx: des_tokio::sync::mpsc::Receiver<Bytes>,

    outbound_tx: des_tokio::sync::mpsc::Sender<Bytes>,
    outbound_rx: des_tokio::sync::mpsc::Receiver<Bytes>,

    // Final status (server->client)
    final_status: Option<tonic::Status>,
}
```

The stream is fully closed when:
- C->S is HalfClosed AND S->C is HalfClosed

### 4.3 Cleanup and timeouts

For every stream state, maintain:
- `last_activity_time: SimTime`
- `idle_timeout: Duration`

Cleanup rules:
- On each received frame, update `last_activity_time`
- A periodic cleanup task (or per-stream timer) drops streams that are idle beyond timeout
- Dropping cleans up pending maps and closes inbound/outbound channels

### 4.4 Cancellation

Cancellation sources:
- local drop of sender/receiver handle
- explicit user cancel
- timeout
- protocol violation

Cancellation action:
- enqueue a `Cancel` frame for the opposite side
- mark both directions half-closed locally
- close local channels

---

## 5) Client-Side Design

### 5.1 Channel API

Add methods:

- Server streaming:
  - `server_streaming(path, Request<Bytes>, timeout) -> Response<DesStreaming<Bytes>>`

- Client streaming:
  - `client_streaming(path, timeout) -> (DesStreamSender<Bytes>, ResponseFuture<Bytes>)`

- Bidi:
  - `bidirectional_streaming(path, timeout) -> (DesStreamSender<Bytes>, DesStreaming<Bytes>)`

Where:
- `DesStreaming<T>` implements `Stream<Item = Result<T, Status>>` backed by `des_tokio::sync::mpsc::Receiver`.
- `DesStreamSender<T>` provides `send(T).await` and `close().await`.

### 5.2 Client state

Extend `Channel`/`ClientEndpoint` with:

- `pending_unary: HashMap<correlation_id, oneshot>` (existing)
- `pending_streams: HashMap<stream_id, ClientStreamState>`

ClientStreamState includes:
- the receiving side for server->client frames
- optional oneshot for final response (client-streaming)
- sequence counters and reorder buffers

### 5.3 Client: server-streaming flow

1) allocate `stream_id`
2) send `Open` (C->S) with method + metadata + request payload embedded either:
   - as `Open.payload` (if you want “unary request then stream response”), OR
   - as `Open` followed by one `Data` (preferred for uniformity)
3) create local receiver for S->C `Data` frames
4) return `DesStreaming` to user
5) on `Close` (S->C), close stream receiver and surface final `Status`

### 5.4 Client: client-streaming flow

1) allocate `stream_id`
2) send `Open` (C->S)
3) return `DesStreamSender` to user
4) when user calls `close()`, send `Close` (C->S)
5) await server final `Close` (S->C) carrying status + payload
6) fulfill response future

### 5.5 Client: bidi flow

Combination:
- open stream
- user sends `Data` frames via sender
- user receives `Data` frames via `DesStreaming`
- `close()` triggers client half-close; server may still stream responses

---

## 6) Server-Side Design

### 6.1 Router handler types

Extend router with streaming handlers:

```rust
type ServerStreamingHandler = Arc<
  dyn Fn(Request<Bytes>) -> Pin<Box<dyn Future<Output = Result<DesStreaming<Bytes>, Status>> + Send>>
    + Send
    + Sync
>;

type ClientStreamingHandler = Arc<
  dyn Fn(DesStreaming<Bytes>) -> Pin<Box<dyn Future<Output = Result<Response<Bytes>, Status>> + Send>>
    + Send
    + Sync
>;

type BidiStreamingHandler = Arc<
  dyn Fn(DesStreaming<Bytes>) -> Pin<Box<dyn Future<Output = Result<DesStreaming<Bytes>, Status>> + Send>>
    + Send
    + Sync
>;
```

(Exact signature can vary; key is that inbound streams are represented by a `DesStreaming` fed by frames.)

### 6.2 Server stream map

`ServerEndpoint` maintains:
- `streams: HashMap<stream_id, ServerStreamState>`

Where each state stores:
- inbound channel sender (fed by C->S DATA frames)
- outbound channel receiver (drained to produce S->C DATA frames)
- seq/reorder for both directions
- final status tracking

### 6.3 Server message handling algorithm

On receiving a transport message of type `RpcStreamFrame`:

1) decode `StreamFrameWire`
2) lookup `streams[stream_id]` or create on Open
3) update `last_activity_time`
4) process per direction:

**On Open (C->S):**
- validate method exists in router for the requested streaming kind
- create `ServerStreamState`
- create inbound/outbound bounded channels (`des_tokio::sync::mpsc::channel(cap)`)
- spawn an async task that runs the handler:
  - server-streaming: invoke handler with `Request<Bytes>` and get `DesStreaming<Bytes>` (outbound)
  - client-streaming: invoke handler with inbound stream and await a single `Response<Bytes>`
  - bidi: invoke handler with inbound stream and get outbound stream
- spawn a second async task that forwards outbound items to transport as S->C `Data` frames
- record the handler task so Cancel/timeout can abort it

**On Data (C->S):**
- reorder buffer by seq
- when in-order available, forward payload into inbound channel
- if inbound channel is full:
  - apply backpressure by pausing draining (preferred) or dropping with `resource_exhausted`
  - initial implementation can block the handler side only; receiver side continues buffering up to a cap

**On Close (C->S):**
- mark C->S half-closed
- close inbound channel (so handler sees end-of-stream)

**On Cancel (C->S):**
- abort handler
- close inbound/outbound channels
- optionally send Cancel back (S->C) if not already closed

### 6.4 Server completion

When the handler completes:
- server-streaming/bidi:
  - when outbound stream ends, send S->C `Close` with `Status::ok`.
- client-streaming:
  - send one S->C `Data` (response payload) then S->C `Close` with status.

(Alternate: encode response payload directly in `Close` for client-streaming; either is fine.)

---

## 7) Backpressure (v1)

We need a deterministic and deadlock-avoiding approach.

v1 pragmatic approach:
- Use bounded `des_tokio::sync::mpsc` for inbound and outbound per stream.
- Set caps (configurable):
  - `inbound_capacity_messages`
  - `outbound_capacity_messages`
- If the remote side violates backpressure (sends too many frames):
  - buffer in reorder map only up to a cap; if exceeded, `Cancel` the stream with `resource_exhausted`.

This is not HTTP/2 flow control, but it prevents unbounded memory/event growth.

---

## 8) Determinism & `des-explore` compatibility

- Stream IDs must be deterministic:
  - re-use the channel’s monotonic counter + endpoint ID, similar to unary correlation_id.
- Seq numbers are deterministic:
  - assigned by the sending side’s `AtomicU64` per stream direction.
- All concurrency primitives used are `des_tokio` primitives.
- Exploration hooks:
  - schedule decisions that affect frame delivery (frontier and tokio-ready) are already controllable via `des-explore`.

---

## 9) Is client-streaming “complicated”?

With this design, **client-streaming is not substantially harder than bidi** once the common frame/state machinery exists.

- Server streaming requires server->client DATA + Close.
- Client streaming requires client->server DATA + Close and a single server response.
- Bidi requires both directions simultaneously.

So if you implement the generic `StreamFrameWire` + stream state machine, adding client-streaming is mostly:
- handler signature
- server-side completion semantics (single response)

The real complexity lives in the shared parts:
- stream lifecycle (Open/Data/Close/Cancel)
- timeouts/cleanup
- backpressure

---

## 10) Implementation Plan (incremental, file-by-file)

This section is the concrete execution plan for implementing streaming in `des-tonic`.

### 10.1 Milestone 0: Prep and invariants

- **Invariant:** Unary RPC behavior is unchanged.
- **Invariant:** Streaming uses `des_tokio::*` primitives only.
- **Invariant:** Do not change `des-components::transport::TransportMessage` shape.
- **Determinism:** `stream_id` generation must be deterministic and locally unique.

Deliverable:
- A small internal helper for deterministic stream IDs:
  - `fn next_stream_id(&self) -> String` similar to `Channel::next_correlation_id()`.

### 10.2 Milestone 1: Minimal transport plumbing (one new MessageType)

**Goal:** allow the network to distinguish unary vs streaming frames.

Changes:
- `des-components/src/transport/mod.rs`
  - Extend `MessageType` with `RpcStreamFrame`.
  - Ensure any `match` statements remain exhaustive.

No other transport changes.

Acceptance:
- `cargo test -p des-components` and `cargo test -p des-tonic` still pass.

### 10.3 Milestone 2: Wire format for stream frames

**Goal:** encode/decode streaming frames without touching transport structs.

Changes:
- `des-tonic/src/wire.rs`
  - Add:
    - `StreamFrameKind` (Open/Data/Close/Cancel)
    - `StreamDirection` (ClientToServer/ServerToClient)
    - `StreamFrameWire` structure
  - Add:
    - `encode_stream_frame(&StreamFrameWire) -> Bytes`
    - `decode_stream_frame(Bytes) -> Result<StreamFrameWire, Status>`

Rules:
- `Open` must carry `method`.
- `Close` must carry `Status` (at least for S->C). For C->S Close, `Status` may be unused.
- `payload` is empty for `Open/Close/Cancel` unless explicitly needed.

Tests:
- `des-tonic/src/wire.rs` unit tests:
  - round-trip for each kind
  - corrupted/truncated bytes -> `Status::internal`

### 10.4 Milestone 3: Public stream types (client/server API boundary)

**Goal:** introduce stream sender/receiver types that are simulation-friendly.

Changes:
- Add new module `des-tonic/src/stream.rs` (or `channel/stream.rs`):
  - `pub struct DesStreaming<T>`: wrapper over `des_tokio::sync::mpsc::Receiver<Result<T, Status>>`
  - `pub struct DesStreamSender<T>`: wrapper over `des_tokio::sync::mpsc::Sender<T>` plus `close()`
  - Implement `futures_core::Stream` for `DesStreaming` (if already in deps); otherwise provide `async fn next(&mut self)`.

Notes:
- Prefer `des_tokio::sync::mpsc` to avoid OS-tokio.
- Keep the types `Send` where possible, but allow `spawn_local` usage.

Acceptance:
- No networking yet; these are pure async primitives.

### 10.5 Milestone 4: Router registration + handler contracts

**Goal:** extend the server routing table.

Changes:
- `des-tonic/src/router.rs`
  - Extend `Router`:
    - `add_server_streaming(path, handler)`
    - `add_client_streaming(path, handler)`
    - `add_bidi_streaming(path, handler)`
  - Either:
    - keep 3 maps, or
    - use a single enum dispatch map (recommended if you anticipate growth)

Handler contracts (v1):
- Server streaming: `Fn(Request<Bytes>) -> Future<Result<DesStreaming<Bytes>, Status>>`
- Client streaming: `Fn(DesStreaming<Bytes>) -> Future<Result<Response<Bytes>, Status>>`
- Bidi: `Fn(DesStreaming<Bytes>) -> Future<Result<DesStreaming<Bytes>, Status>>`

Acceptance:
- Pure compile-time changes + basic unit tests (e.g., list methods).

### 10.6 Milestone 5: Client-side stream plumbing

**Goal:** allow `Channel` to open streams and receive frames.

Changes:
- `des-tonic/src/channel.rs`
  - Add `pending_streams: Arc<Mutex<HashMap<String, ClientStreamState>>>`
  - Define `ClientStreamState` containing:
    - in-order assembly for S->C frames: `next_s2c`, `reorder_s2c: BTreeMap<u64, Bytes>`
    - `inbound_tx: mpsc::Sender<Result<Bytes, Status>>` used to feed `DesStreaming`
    - optional `final_response_tx: oneshot::Sender<Result<Response<Bytes>, Status>>` for client-streaming
    - `last_activity: SimTime`

- `des-tonic/src/channel.rs` API additions:
  - `server_streaming(path, Request<Bytes>, timeout) -> Result<Response<DesStreaming<Bytes>>, Status>`
  - `client_streaming(path, timeout) -> Result<(DesStreamSender<Bytes>, ResponseFuture<Bytes>), Status>`
  - `bidirectional_streaming(path, timeout) -> Result<(DesStreamSender<Bytes>, DesStreaming<Bytes>), Status>`

Implementation notes:
- Implement a shared internal helper:
  - `fn send_stream_frame(&self, target: EndpointId, frame: StreamFrameWire)`
- `server_streaming` should:
  - allocate stream
  - send C->S `Open` + one C->S `Data` carrying request payload
  - return S->C `DesStreaming`

- `client_streaming` should:
  - allocate stream
  - send C->S `Open`
  - return sender for subsequent C->S `Data`
  - return future that resolves when S->C Close arrives with response

Timeout:
- v1: treat timeout as client-side: if response stream does not produce any item (or Close) before deadline, cancel.

### 10.7 Milestone 6: ClientEndpoint dispatch of stream frames

**Goal:** make the client endpoint deliver incoming `RpcStreamFrame` messages into the right stream state.

Changes:
- `des-tonic/src/channel.rs` (ClientEndpoint component)
  - Extend `process_event` to handle `MessageType::RpcStreamFrame`.
  - Lookup `pending_streams[stream_id]` (stream_id from `correlation_id` or frame).
  - Apply ordering logic:
    - buffer out-of-order
    - deliver in-order to `inbound_tx`
  - On `Close`:
    - complete `final_response_tx` for client-streaming (if present)
    - close `inbound_tx`
    - remove stream from map

Acceptance tests:
- Integration test in `des-tonic/tests/`:
  - server-streaming method produces N items; client receives exactly N in order.

### 10.8 Milestone 7: ServerEndpoint stream handling

**Goal:** server can accept stream opens, feed inbound data into handlers, and forward outbound items as frames.

Changes:
- `des-tonic/src/server.rs`
  - Add `streams: HashMap<stream_id, ServerStreamState>`
  - On `RpcStreamFrame::Open`:
    - validate handler exists
    - create inbound/outbound bounded channels
    - spawn handler task:
      - server streaming: handler gets request payload (from first Data) and returns outbound stream
      - client streaming/bidi: handler gets inbound stream
    - spawn a forwarder task to send S->C `Data` frames for outbound items
  - On C->S `Data`:
    - in-order assembly and push into inbound channel
  - On C->S `Close`:
    - close inbound channel
  - On `Cancel`:
    - abort handler/forwarder tasks
    - cleanup

Acceptance tests:
- `server_streaming_end_to_end`
- `client_streaming_end_to_end`

### 10.9 Milestone 8: Backpressure and bounded buffering

**Goal:** prevent unbounded accumulation.

v1 policy:
- reorder buffers capped (e.g., `max_reorder_frames`), exceeded => Cancel with `resource_exhausted`.
- inbound/outbound mpsc bounded; `send().await` applies backpressure to producer tasks.

Add config knobs:
- `ServerBuilder::stream_config(StreamConfig { ... })`
- `ClientBuilder::stream_config(StreamConfig { ... })`

### 10.10 Milestone 9: Examples and replay/debug friendliness

Add examples under `des-tonic/examples/`:
- `server_streaming_counter.rs`
- `client_streaming_sum.rs`
- `bidi_chat.rs` (minimal)

Add a `des-explore` record/replay test (optional but recommended):
- record a streaming run with randomized scheduling
- replay it and assert identical received sequence

---

## 11) Suggested order of feature delivery

If you want the smoothest incremental path:

1) server streaming
2) client streaming
3) bidi

Reason: client streaming exercises inbound stream assembly and handler EOF, but avoids full-duplex complexities.
