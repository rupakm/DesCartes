# `des-tonic` Prost Codegen (`descartes_tonic_build`) Design

**Status:** Proposed

This document specifies a minimal prost-based codegen layer that makes `des-tonic` feel closer to tonic’s generated stubs (clients/servers) while keeping the DES-native transport.

This is intentionally *not* gRPC/HTTP2 compatible; it generates tonic-shaped APIs that call into `des-tonic`’s `Channel`/`Router`.

## Goals

- Users write code that looks like tonic examples:
  - `pb::foo_server::Foo` trait with async methods
  - `pb::foo_server::FooServer::new(impl Foo).register(&mut Router)`
  - `pb::foo_client::FooClient::new(Channel)` with typed RPC methods
- Remove boilerplate:
  - no string method paths in user code
  - no manual `prost::Message` encode/decode at call sites
- Keep `des-tonic` transport and determinism properties.

## Non-goals

- Compatibility with tonic/h2 wire protocol
- Full tonic surface area (interceptors, TLS, compression, metadata semantics parity)

## Dependencies

- Add `prost` as a normal dependency of `des-tonic` (and in the generated code).
- Add `prost-build` to `descartes_tonic_build`.
- Optional (recommended): add `async-trait` to generated server traits (or re-export `tonic::async_trait`).

We do **not** require `futures-core::Stream` for v1 stubs. Streaming types are `descartes_tonic::DesStreaming<T>` which already provides `async fn next()`.

## Minimal Runtime API Surface in `des-tonic`

The goal is to keep codegen dumb: generated stubs should mostly be (path, type parameters) wiring.

### 1) `IntoRequest<T>`

Match tonic ergonomics for client methods:

```rust
pub trait IntoRequest<T> {
    fn into_request(self) -> tonic::Request<T>;
}

impl<T> IntoRequest<T> for T {
    fn into_request(self) -> tonic::Request<T> {
        tonic::Request::new(self)
    }
}

impl<T> IntoRequest<T> for tonic::Request<T> {
    fn into_request(self) -> tonic::Request<T> {
        self
    }
}
```

### 2) Typed stream channel helper

Provide a small helper to build server response streams or client request streams:

```rust
pub mod stream {
    pub fn channel<T>(cap: usize) -> (Sender<T>, DesStreaming<T>);

    pub struct Sender<T> { /* wraps descartes_tokio::mpsc::Sender<Result<T, Status>> */ }
    impl<T> Sender<T> {
        pub async fn send(&self, item: T) -> Result<(), ()>;
        pub async fn send_err(&self, status: tonic::Status) -> Result<(), ()>;
        pub fn close(&self);
    }
}
```

This can be implemented on top of `descartes_tokio::sync::mpsc`.

### 3) Prost-typed `Channel` RPCs (exact signatures)

These are the primary entry points the generated client uses.

```rust
impl Channel {
    pub async fn unary_prost<Req, Resp>(
        &self,
        path: &'static str,
        request: tonic::Request<Req>,
        timeout: Option<std::time::Duration>,
    ) -> Result<tonic::Response<Resp>, tonic::Status>
    where
        Req: prost::Message,
        Resp: prost::Message + Default;

    pub async fn server_streaming_prost<Req, Resp>(
        &self,
        path: &'static str,
        request: tonic::Request<Req>,
        timeout: Option<std::time::Duration>,
    ) -> Result<tonic::Response<DesStreaming<Resp>>, tonic::Status>
    where
        Req: prost::Message,
        Resp: prost::Message + Default;

    pub async fn client_streaming_prost<Req, Resp>(
        &self,
        path: &'static str,
        timeout: Option<std::time::Duration>,
    ) -> Result<(stream::Sender<Req>, ClientResponseFuture<Resp>), tonic::Status>
    where
        Req: prost::Message,
        Resp: prost::Message + Default;

    pub async fn bidirectional_streaming_prost<Req, Resp>(
        &self,
        path: &'static str,
        timeout: Option<std::time::Duration>,
    ) -> Result<(stream::Sender<Req>, tonic::Response<DesStreaming<Resp>>), tonic::Status>
    where
        Req: prost::Message,
        Resp: prost::Message + Default;
}
```

Notes:
- `ClientResponseFuture<Resp>` is a boxed future:

```rust
pub type ClientResponseFuture<T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<tonic::Response<T>, tonic::Status>> + 'static>>;
```

- These functions are thin adapters over the existing `Bytes` versions.
- Streaming typed versions encode/decode each message boundary.

### 4) Prost-typed `Router` registration (exact signatures)

These are used by the generated server registration wrapper.

```rust
impl Router {
    pub fn add_unary_prost<S, Req, Resp, Fut>(
        &mut self,
        path: &'static str,
        svc: std::sync::Arc<S>,
        f: fn(std::sync::Arc<S>, tonic::Request<Req>) -> Fut,
    ) -> &mut Self
    where
        Req: prost::Message + Default,
        Resp: prost::Message,
        Fut: std::future::Future<Output = Result<tonic::Response<Resp>, tonic::Status>> + Send + 'static;

    pub fn add_server_streaming_prost<S, Req, Resp, Fut>(
        &mut self,
        path: &'static str,
        svc: std::sync::Arc<S>,
        f: fn(std::sync::Arc<S>, tonic::Request<Req>) -> Fut,
    ) -> &mut Self
    where
        Req: prost::Message + Default,
        Resp: prost::Message,
        Fut: std::future::Future<Output = Result<tonic::Response<DesStreaming<Resp>>, tonic::Status>> + Send + 'static;

    pub fn add_client_streaming_prost<S, Req, Resp, Fut>(
        &mut self,
        path: &'static str,
        svc: std::sync::Arc<S>,
        f: fn(std::sync::Arc<S>, tonic::Request<DesStreaming<Req>>) -> Fut,
    ) -> &mut Self
    where
        Req: prost::Message + Default,
        Resp: prost::Message,
        Fut: std::future::Future<Output = Result<tonic::Response<Resp>, tonic::Status>> + Send + 'static;

    pub fn add_bidi_streaming_prost<S, Req, Resp, Fut>(
        &mut self,
        path: &'static str,
        svc: std::sync::Arc<S>,
        f: fn(std::sync::Arc<S>, tonic::Request<DesStreaming<Req>>) -> Fut,
    ) -> &mut Self
    where
        Req: prost::Message + Default,
        Resp: prost::Message,
        Fut: std::future::Future<Output = Result<tonic::Response<DesStreaming<Resp>>, tonic::Status>> + Send + 'static;
}
```

This shape keeps codegen simple:
- generated `FooServer::register` passes `Arc::new(inner)` and function pointers to each method.

## Generated Code Surface (what `descartes_tonic_build` emits)

For a service `Echo` in package `grpc.examples.echo`:

### `pb::echo_server`

```rust
pub mod echo_server {
    use super::*;

    #[descartes_tonic::async_trait]
    pub trait Echo: Send + Sync + 'static {
        async fn unary_echo(
            &self,
            request: tonic::Request<EchoRequest>,
        ) -> Result<tonic::Response<EchoResponse>, tonic::Status>;

        async fn server_streaming_echo(
            &self,
            request: tonic::Request<EchoRequest>,
        ) -> Result<tonic::Response<descartes_tonic::DesStreaming<EchoResponse>>, tonic::Status>;

        async fn client_streaming_echo(
            &self,
            request: tonic::Request<descartes_tonic::DesStreaming<EchoRequest>>,
        ) -> Result<tonic::Response<EchoResponse>, tonic::Status>;

        async fn bidirectional_streaming_echo(
            &self,
            request: tonic::Request<descartes_tonic::DesStreaming<EchoRequest>>,
        ) -> Result<tonic::Response<descartes_tonic::DesStreaming<EchoResponse>>, tonic::Status>;
    }

    pub struct EchoServer<T> {
        inner: std::sync::Arc<T>,
    }

    impl<T: Echo> EchoServer<T> {
        pub fn new(inner: T) -> Self {
            Self { inner: std::sync::Arc::new(inner) }
        }

        pub fn register(self, router: &mut descartes_tonic::Router) {
            router
                .add_unary_prost(METHOD_UNARY_ECHO, self.inner.clone(), |svc, req| async move {
                    svc.unary_echo(req).await
                })
                .add_server_streaming_prost(METHOD_SERVER_STREAMING_ECHO, self.inner.clone(), |svc, req| async move {
                    svc.server_streaming_echo(req).await
                })
                .add_client_streaming_prost(METHOD_CLIENT_STREAMING_ECHO, self.inner.clone(), |svc, req| async move {
                    svc.client_streaming_echo(req).await
                })
                .add_bidi_streaming_prost(METHOD_BIDI_STREAMING_ECHO, self.inner.clone(), |svc, req| async move {
                    svc.bidirectional_streaming_echo(req).await
                });
        }
    }
}
```

### `pb::echo_client`

```rust
pub mod echo_client {
    use super::*;

    #[derive(Clone)]
    pub struct EchoClient {
        inner: descartes_tonic::Channel,
        timeout: Option<std::time::Duration>,
    }

    impl EchoClient {
        pub fn new(inner: descartes_tonic::Channel) -> Self {
            Self { inner, timeout: None }
        }

        pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
            self.timeout = Some(timeout);
            self
        }

        pub async fn unary_echo(
            &self,
            request: impl descartes_tonic::IntoRequest<EchoRequest>,
        ) -> Result<tonic::Response<EchoResponse>, tonic::Status> {
            self.inner
                .unary_prost(METHOD_UNARY_ECHO, request.into_request(), self.timeout)
                .await
        }

        pub async fn server_streaming_echo(
            &self,
            request: impl descartes_tonic::IntoRequest<EchoRequest>,
        ) -> Result<tonic::Response<descartes_tonic::DesStreaming<EchoResponse>>, tonic::Status> {
            self.inner
                .server_streaming_prost(METHOD_SERVER_STREAMING_ECHO, request.into_request(), self.timeout)
                .await
        }

        pub async fn client_streaming_echo(
            &self,
        ) -> Result<(descartes_tonic::stream::Sender<EchoRequest>, descartes_tonic::ClientResponseFuture<EchoResponse>), tonic::Status> {
            self.inner
                .client_streaming_prost(METHOD_CLIENT_STREAMING_ECHO, self.timeout)
                .await
        }

        pub async fn bidirectional_streaming_echo(
            &self,
        ) -> Result<(descartes_tonic::stream::Sender<EchoRequest>, tonic::Response<descartes_tonic::DesStreaming<EchoResponse>>), tonic::Status> {
            self.inner
                .bidirectional_streaming_prost(METHOD_BIDI_STREAMING_ECHO, self.timeout)
                .await
        }
    }
}
```

### Method path constants

Generated constants so user code never writes strings:

```rust
pub const METHOD_UNARY_ECHO: &str = "/grpc.examples.echo.Echo/UnaryEcho";
pub const METHOD_SERVER_STREAMING_ECHO: &str = "/grpc.examples.echo.Echo/ServerStreamingEcho";
pub const METHOD_CLIENT_STREAMING_ECHO: &str = "/grpc.examples.echo.Echo/ClientStreamingEcho";
pub const METHOD_BIDI_STREAMING_ECHO: &str = "/grpc.examples.echo.Echo/BidirectionalStreamingEcho";
```

## `descartes_tonic_build` crate API

Keep it close to `tonic_build`:

```rust
pub struct Builder {
    out_dir: std::path::PathBuf,
    compile_well_known_types: bool,
}

impl Builder {
    pub fn new() -> Self;
    pub fn out_dir(mut self, out: impl Into<std::path::PathBuf>) -> Self;
    pub fn compile_well_known_types(mut self, yes: bool) -> Self;
    pub fn compile(self, protos: &[impl AsRef<std::path::Path>], includes: &[impl AsRef<std::path::Path>])
        -> Result<(), Box<dyn std::error::Error>>;
}
```

`compile` should:
- run `prost-build` to emit message types
- run a small custom service generator to emit client/server stubs that target `des-tonic`
- emit a module layout compatible with `descartes_tonic::include_proto!("...")`

## Open Questions

- Whether to implement `futures_core::Stream` for `DesStreaming<T>` (optional feature) for closer tonic parity.
- Whether to support client methods that accept an input stream (tonic’s `client.bidi(in_stream)`).
  - A minimal follow-up is to generate both variants:
    - `fn bidi()` returning `(Sender, ResponseStream)`
    - `fn bidi_with_stream(in_stream)` that forwards into `Sender`.
