use std::time::Duration;

pub use des_components::transport::{LatencyConfig, NetworkModel};

use des_components::transport::{LatencyJitterModel, SimpleNetworkModel};

/// Deterministic network model constructors.
///
/// These helpers let callers install common network models without importing
/// `des-components` types directly.
/// Simple deterministic network model with constant latency and packet loss.
pub fn simple(base_latency: Duration, packet_loss_rate: f64, seed: u64) -> Box<dyn NetworkModel> {
    Box::new(SimpleNetworkModel::with_seed(
        base_latency,
        packet_loss_rate,
        seed,
    ))
}

/// Convenience: zero latency, zero packet loss.
pub fn ideal(seed: u64) -> Box<dyn NetworkModel> {
    simple(Duration::ZERO, 0.0, seed)
}

/// Latency model with optional jitter, packet loss, and bandwidth.
pub fn latency_jitter(config: LatencyConfig, seed: u64) -> Box<dyn NetworkModel> {
    Box::new(LatencyJitterModel::with_seed(config, seed))
}

/// Convenience: base latency + jitter factor (fraction of base latency).
pub fn latency_jitter_base(
    base_latency: Duration,
    jitter_factor: f64,
    seed: u64,
) -> Box<dyn NetworkModel> {
    latency_jitter(
        LatencyConfig::new(base_latency).with_jitter(jitter_factor),
        seed,
    )
}
