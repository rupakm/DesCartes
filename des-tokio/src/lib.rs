//! Tokio-like facade for deterministic DES simulation.
//!
//! This crate provides a small subset of Tokio APIs backed by `des-core`'s
//! discrete-event async runtime.

pub mod concurrency;
pub mod runtime;

/// Compute a stable deterministic identifier for concurrency primitives.
///
/// - `stable_id!("domain", "name")` is stable across call sites.
/// - `stable_id!("name")` uses `module_path!()` as the domain.
#[macro_export]
macro_rules! stable_id {
    ($domain:expr, $name:expr) => {{
        const _ID: u64 = descartes_core::randomness::fnv1a64(concat!($domain, "::", $name));
        _ID
    }};
    ($name:expr) => {{
        $crate::stable_id!(module_path!(), $name)
    }};
}

/// Generate a callsite-unique deterministic identifier.
///
/// This mirrors `descartes_core::draw_site!` but returns only the `site_id`.
#[macro_export]
macro_rules! site_id {
    ($tag:expr) => {{
        $crate::stable_id!(
            concat!(module_path!(), "::", file!(), ":", line!(), ":", column!()),
            $tag
        )
    }};
}
pub mod runtime_internal;
pub mod stream;
pub mod sync;
pub mod task;
pub mod thread;
pub mod time;
