//! `des-tonic` provides a tonic-shaped (but not wire-compatible) RPC facade for
//! running gRPC-like unary RPC interactions inside a `des-core` discrete event
//! simulation.
//!
//! Design goals:
//! - Close to tonic's `Request`/`Response`/`Status` types.
//! - Deterministic and deadlock-free scheduling under DES.
//! - Unary-only RPCs.
//!
//! This crate does **not** implement HTTP/2 or gRPC framing. Instead it uses the
//! `des-components` simulated transport layer to exchange binary payloads.

mod addr;
pub mod channel;
pub mod router;
pub mod server;
pub mod transport;
mod util;
mod wire;

pub use channel::{Channel, ClientBuilder, ClientEndpoint, InstalledClient};
pub use router::Router;
pub use server::{InstalledServer, ServerBuilder, ServerEndpoint};
pub use transport::Transport;

pub use tonic::{Code, Request, Response, Status};
