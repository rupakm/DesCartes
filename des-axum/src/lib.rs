//! Deterministic simulation wrapper for axum apps.
//!
//! `des-axum` runs an axum `Router` (tower service) inside a DES simulation.
//! It does **not** emulate hyper's socket accept loop; instead it uses
//! `des-components`' simulated transport and endpoint registry.
//!
//! What this crate provides
//! - A `des_axum::Transport` wrapper that installs `SimTransport` and exposes serve/connect APIs.
//! - A server endpoint component that decodes an HTTP-shaped request, calls your axum `Router`,
//!   collects the response body to bytes, and sends an HTTP-shaped response back.
//! - A client endpoint component that sends requests over the simulated network and resolves
//!   responses via correlation IDs.
//!
//! Addressing model
//! - Primary: `(service_name, instance_name)`.
//! - Convenience: `SocketAddr` wrappers that use `addr.to_string()` as `instance_name`.
//!
//! Notes / current limits
//! - This is "axum as a tower service in simulation", not a real socket server.
//! - No HTTP/1 parsing and no hyper accept loop.
//! - Bodies are currently handled as collected bytes (no streaming / websockets yet).
//! - Deterministic time requires installing the DES async runtime (`des_tokio::runtime::install`).
//!
//! Sketch
//! ```rust,no_run
//! # use des_core::{Execute, Executor, SimTime, Simulation};
//! # use std::time::Duration;
//! let mut sim = Simulation::default();
//! des_tokio::runtime::install(&mut sim);
//!
//! let transport = des_axum::Transport::install_default(&mut sim);
//! let app = axum::Router::new().route("/hello", axum::routing::get(|| async { "hello" }));
//!
//! // (A) serve by (service_name, instance_name)
//! des_axum::serve(&transport, &mut sim, "hello", "hello-1", app)?;
//!
//! let client = transport.connect(&mut sim, "hello")?;
//! des_tokio::task::spawn_local(async move {
//!     let _resp = client.get("/hello", Some(Duration::from_secs(1))).await.unwrap();
//! });
//!
//! Executor::timed(SimTime::from_duration(Duration::from_secs(1))).execute(&mut sim);
//! # Ok::<(), des_axum::Error>(())
//! ```

mod addr;
mod client;
mod server;
mod transport;
mod util;
mod wire;

pub use crate::client::{Client, ClientBuilder, InstalledClient};
pub use crate::server::{InstalledServer, ServerBuilder};
pub use crate::transport::Transport;

pub use crate::addr::parse_socket_addr;
pub use crate::wire::{HttpRequestWire, HttpResponseWire};

pub use crate::client::Error;

/// Serve an axum router under `(service_name, instance_name)`.
///
/// This is the primary entrypoint if you're replacing `axum::serve(...)` in a
/// simulation harness.
pub fn serve(
    transport: &Transport,
    sim: &mut des_core::Simulation,
    service_name: impl Into<String>,
    instance_name: impl Into<String>,
    app: axum::Router,
) -> Result<InstalledServer, Error> {
    transport.serve_named(sim, service_name, instance_name, app)
}

/// Convenience wrapper: serve an axum router under `(service_name, instance_name)`.
pub fn serve_named(
    transport: &Transport,
    sim: &mut des_core::Simulation,
    service_name: impl Into<String>,
    instance_name: impl Into<String>,
    app: axum::Router,
) -> Result<InstalledServer, Error> {
    transport.serve_named(sim, service_name, instance_name, app)
}

/// Convenience wrapper: serve an axum router under `service_name` at `addr`.
///
/// This uses `addr.to_string()` as `instance_name`.
pub fn serve_socket_addr(
    transport: &Transport,
    sim: &mut des_core::Simulation,
    service_name: impl Into<String>,
    addr: std::net::SocketAddr,
    app: axum::Router,
) -> Result<InstalledServer, Error> {
    transport.serve_socket_addr(sim, service_name, addr, app)
}

/// Convenience wrapper: serve an axum router under `service_name` at `addr`.
///
/// `addr` may be a plain `SocketAddr` string ("127.0.0.1:3000") or a URI-like
/// string ("http://127.0.0.1:3000").
pub fn serve_addr(
    transport: &Transport,
    sim: &mut des_core::Simulation,
    service_name: impl Into<String>,
    addr: impl AsRef<str>,
    app: axum::Router,
) -> Result<InstalledServer, Error> {
    transport.serve(sim, service_name, addr, app)
}
