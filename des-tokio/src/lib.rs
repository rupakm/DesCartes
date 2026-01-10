//! Tokio-like facade for deterministic DES simulation.
//!
//! This crate provides a small subset of Tokio APIs backed by `des-core`'s
//! discrete-event async runtime.

pub mod runtime;
pub mod runtime_internal;
pub mod sync;
pub mod task;
pub mod time;
