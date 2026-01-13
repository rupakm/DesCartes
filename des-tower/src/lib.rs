//! Tower middleware integration for DES.
//!
//! This crate provides Tower `Service` adapters and DES-aware middleware layers.
//! It is designed to be used with `des-tokio` (install the runtime on the
//! simulation before executing).

pub mod tower;

pub use tower::*;
