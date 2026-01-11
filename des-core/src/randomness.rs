//! Randomness facade for deterministic simulation.
//!
//! This module is intentionally small. It provides:
//! - `DrawSite`: a stable identifier for a sampling location, plus a human tag.
//! - `RandomProvider`: a trait for sampling distributions while optionally logging.
//!
//! The default `des-core` distributions still work without a provider. When a
//! provider is injected, distributions delegate sampling to it (which enables
//! tracing, replay, importance sampling, splitting, etc.).

use std::hash::Hasher;

/// A labeled sampling location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DrawSite {
    pub tag: &'static str,
    pub site_id: u64,
}

impl DrawSite {
    pub const fn new(tag: &'static str, site_id: u64) -> Self {
        Self { tag, site_id }
    }
}

/// Sampling interface that can be swapped for tracing / biasing.
///
/// Note: this is designed to be owned by a distribution/component and used on
/// the simulation thread.
pub trait RandomProvider: Send {
    /// Sample an exponential distribution parameterized by `rate` (events/sec).
    /// Returns a value in seconds.
    fn sample_exp_seconds(&mut self, site: DrawSite, rate: f64) -> f64;
}

/// Const-friendly 64-bit FNV-1a hash.
pub const fn fnv1a64(s: &str) -> u64 {
    let bytes = s.as_bytes();
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        i += 1;
    }
    hash
}

/// Generate a `DrawSite` at the macro expansion site.
///
/// This is meant to keep legacy integration low-friction: you can wrap sampling
/// with a macro and still get per-callsite ids.
#[macro_export]
macro_rules! draw_site {
    ($tag:expr) => {{
        const _SITE_ID: u64 = $crate::randomness::fnv1a64(concat!(
            module_path!(),
            "::",
            file!(),
            ":",
            line!(),
            ":",
            column!(),
            ":",
            $tag,
        ));
        $crate::randomness::DrawSite::new($tag, _SITE_ID)
    }};
}

/// A tiny helper used for tests: compute a non-const site id.
///
/// Not used in the main code path.
pub fn runtime_site_id(tag: &str, label: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(tag.as_bytes());
    h.write(label.as_bytes());
    h.finish()
}
