//! `des-tonic` provides a tonic-shaped (but not wire-compatible) RPC facade for
//! running gRPC-like unary RPC interactions inside a `des-core` discrete event
//! simulation.
//!
//! Design goals:
//! - Close to tonic's `Request`/`Response`/`Status` types.
//! - Deterministic and deadlock-free scheduling under DES.
//! - Unary + streaming RPCs.
//!
//! This crate does **not** implement HTTP/2 or gRPC framing. Instead it uses the
//! `des-components` simulated transport layer to exchange binary payloads.

#![allow(clippy::result_large_err)]

mod addr;
pub mod channel;
pub mod network;
mod request;
pub mod router;
pub mod server;
pub mod stream;
pub mod transport;
mod util;
mod wire;

pub use channel::{Channel, ClientBuilder, ClientEndpoint, InstalledClient};
pub use router::Router;
pub use server::{InstalledServer, ServerBuilder, ServerEndpoint};
pub use stream::{DesStreamSender, DesStreaming};
pub use transport::Transport;

pub use request::IntoRequest;

pub type ClientResponseFuture<T> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<tonic::Response<T>, tonic::Status>> + 'static>,
>;

pub use network::NetworkModel;

pub use tonic::{Code, Request, Response, Status};

pub use async_trait::async_trait;

#[macro_export]
macro_rules! include_proto {
    ($package:tt) => {
        include!(concat!(env!("OUT_DIR"), "/", $package, ".rs"));
    };
}
