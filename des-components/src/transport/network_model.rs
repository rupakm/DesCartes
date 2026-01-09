//! Network models for simulating transport characteristics
//!
//! This module provides various network models that can simulate different
//! network conditions including latency, jitter, packet loss, and bandwidth limits.

use crate::transport::{EndpointId, TransportMessage};
use des_core::SimTime;
use rand::Rng;
use rand_distr::{Distribution, Normal};
use std::collections::HashMap;
use std::time::Duration;

/// Trait for modeling network characteristics between endpoints
pub trait NetworkModel: Send + Sync {
    /// Calculate the latency for a message between two endpoints
    fn calculate_latency(&mut self, from: EndpointId, to: EndpointId, message: &TransportMessage) -> Duration;
    
    /// Determine if a message should be dropped
    fn should_drop_message(&mut self, from: EndpointId, to: EndpointId, message: &TransportMessage) -> bool;
    
    /// Calculate bandwidth delay based on message size
    fn calculate_bandwidth_delay(&mut self, from: EndpointId, to: EndpointId, message: &TransportMessage) -> Duration;
    
    /// Reset any internal state (useful for deterministic testing)
    fn reset(&mut self);
}

/// Simple network model with configurable latency and packet loss
pub struct SimpleNetworkModel {
    /// Base latency between any two endpoints
    pub base_latency: Duration,
    /// Packet loss probability (0.0 to 1.0)
    pub packet_loss_rate: f64,
    /// Random number generator for deterministic behavior
    rng: rand_chacha::ChaCha8Rng,
}

impl SimpleNetworkModel {
    /// Create a new simple network model
    pub fn new(base_latency: Duration, packet_loss_rate: f64) -> Self {
        use rand::SeedableRng;
        Self {
            base_latency,
            packet_loss_rate,
            rng: rand_chacha::ChaCha8Rng::from_entropy(),
        }
    }

    /// Create with a specific seed for deterministic behavior
    pub fn with_seed(base_latency: Duration, packet_loss_rate: f64, seed: u64) -> Self {
        use rand::SeedableRng;
        Self {
            base_latency,
            packet_loss_rate,
            rng: rand_chacha::ChaCha8Rng::seed_from_u64(seed),
        }
    }
}

impl NetworkModel for SimpleNetworkModel {
    fn calculate_latency(&mut self, _from: EndpointId, _to: EndpointId, _message: &TransportMessage) -> Duration {
        self.base_latency
    }

    fn should_drop_message(&mut self, _from: EndpointId, _to: EndpointId, _message: &TransportMessage) -> bool {
        self.rng.gen::<f64>() < self.packet_loss_rate
    }

    fn calculate_bandwidth_delay(&mut self, _from: EndpointId, _to: EndpointId, _message: &TransportMessage) -> Duration {
        Duration::ZERO // No bandwidth limits in simple model
    }

    fn reset(&mut self) {
        use rand::SeedableRng;
        self.rng = rand_chacha::ChaCha8Rng::from_entropy();
    }
}

/// Network model with latency, jitter, and per-endpoint characteristics
pub struct LatencyJitterModel {
    /// Per-endpoint pair latency configuration
    latency_config: HashMap<(EndpointId, EndpointId), LatencyConfig>,
    /// Default latency configuration
    default_config: LatencyConfig,
    /// Random number generator
    rng: rand_chacha::ChaCha8Rng,
}

/// Configuration for latency between two endpoints
#[derive(Debug, Clone)]
pub struct LatencyConfig {
    /// Base latency
    pub base_latency: Duration,
    /// Jitter standard deviation (as fraction of base latency)
    pub jitter_factor: f64,
    /// Packet loss rate (0.0 to 1.0)
    pub packet_loss_rate: f64,
    /// Bandwidth in bytes per second (0 = unlimited)
    pub bandwidth_bps: u64,
}

impl LatencyConfig {
    /// Create a new latency configuration
    pub fn new(base_latency: Duration) -> Self {
        Self {
            base_latency,
            jitter_factor: 0.0,
            packet_loss_rate: 0.0,
            bandwidth_bps: 0,
        }
    }

    /// Add jitter as a fraction of base latency
    pub fn with_jitter(mut self, jitter_factor: f64) -> Self {
        self.jitter_factor = jitter_factor;
        self
    }

    /// Add packet loss
    pub fn with_packet_loss(mut self, packet_loss_rate: f64) -> Self {
        self.packet_loss_rate = packet_loss_rate;
        self
    }

    /// Add bandwidth limit
    pub fn with_bandwidth(mut self, bandwidth_bps: u64) -> Self {
        self.bandwidth_bps = bandwidth_bps;
        self
    }
}

impl LatencyJitterModel {
    /// Create a new latency/jitter model with default configuration
    pub fn new(default_config: LatencyConfig) -> Self {
        use rand::SeedableRng;
        Self {
            latency_config: HashMap::new(),
            default_config,
            rng: rand_chacha::ChaCha8Rng::from_entropy(),
        }
    }

    /// Create with a specific seed for deterministic behavior
    pub fn with_seed(default_config: LatencyConfig, seed: u64) -> Self {
        use rand::SeedableRng;
        Self {
            latency_config: HashMap::new(),
            default_config,
            rng: rand_chacha::ChaCha8Rng::seed_from_u64(seed),
        }
    }

    /// Set latency configuration for a specific endpoint pair
    pub fn set_latency(&mut self, from: EndpointId, to: EndpointId, config: LatencyConfig) {
        self.latency_config.insert((from, to), config);
    }

    /// Get latency configuration for an endpoint pair
    fn get_config(&self, from: EndpointId, to: EndpointId) -> &LatencyConfig {
        self.latency_config.get(&(from, to)).unwrap_or(&self.default_config)
    }
}

impl NetworkModel for LatencyJitterModel {
    fn calculate_latency(&mut self, from: EndpointId, to: EndpointId, _message: &TransportMessage) -> Duration {
        let config = self.get_config(from, to);
        let base_ms = config.base_latency.as_millis() as f64;
        
        if config.jitter_factor > 0.0 {
            let jitter_std = base_ms * config.jitter_factor;
            let normal = Normal::new(base_ms, jitter_std).unwrap_or_else(|_| Normal::new(base_ms, 1.0).unwrap());
            let latency_ms = normal.sample(&mut self.rng).max(0.0);
            Duration::from_millis(latency_ms as u64)
        } else {
            config.base_latency
        }
    }

    fn should_drop_message(&mut self, from: EndpointId, to: EndpointId, _message: &TransportMessage) -> bool {
        let packet_loss_rate = self.get_config(from, to).packet_loss_rate;
        self.rng.gen::<f64>() < packet_loss_rate
    }

    fn calculate_bandwidth_delay(&mut self, from: EndpointId, to: EndpointId, message: &TransportMessage) -> Duration {
        let config = self.get_config(from, to);
        if config.bandwidth_bps > 0 {
            let bytes = message.size() as u64;
            let seconds = bytes as f64 / config.bandwidth_bps as f64;
            Duration::from_secs_f64(seconds)
        } else {
            Duration::ZERO
        }
    }

    fn reset(&mut self) {
        use rand::SeedableRng;
        self.rng = rand_chacha::ChaCha8Rng::from_entropy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_network_model() {
        let mut model = SimpleNetworkModel::with_seed(
            Duration::from_millis(100),
            0.1, // 10% packet loss
            42,  // deterministic seed
        );

        let endpoint1 = EndpointId::new("service1".to_string());
        let endpoint2 = EndpointId::new("service2".to_string());
        
        let message = TransportMessage::new(
            1,
            endpoint1,
            endpoint2,
            vec![1, 2, 3],
            SimTime::zero(),
            crate::transport::MessageType::UnaryRequest,
        );

        // Test latency calculation
        let latency = model.calculate_latency(endpoint1, endpoint2, &message);
        assert_eq!(latency, Duration::from_millis(100));

        // Test packet loss (with deterministic seed, should be consistent)
        let dropped = model.should_drop_message(endpoint1, endpoint2, &message);
        // With seed 42, first call should not drop (this is deterministic)
        assert!(!dropped);
    }

    #[test]
    fn test_latency_jitter_model() {
        let default_config = LatencyConfig::new(Duration::from_millis(50))
            .with_jitter(0.2)
            .with_packet_loss(0.05);

        let mut model = LatencyJitterModel::with_seed(default_config, 123);

        let endpoint1 = EndpointId::new("service1".to_string());
        let endpoint2 = EndpointId::new("service2".to_string());
        
        let message = TransportMessage::new(
            1,
            endpoint1,
            endpoint2,
            vec![1, 2, 3],
            SimTime::zero(),
            crate::transport::MessageType::UnaryRequest,
        );

        // Test latency with jitter
        let latency1 = model.calculate_latency(endpoint1, endpoint2, &message);
        let latency2 = model.calculate_latency(endpoint1, endpoint2, &message);
        
        // With jitter, latencies should potentially be different
        // (though with small sample size they might be the same)
        println!("Latency 1: {:?}, Latency 2: {:?}", latency1, latency2);
        
        // Both should be reasonable values around 50ms
        assert!(latency1.as_millis() > 0);
        assert!(latency2.as_millis() > 0);
        assert!(latency1.as_millis() < 200); // Should not be too far from base
        assert!(latency2.as_millis() < 200);
    }

    #[test]
    fn test_bandwidth_delay() {
        let config = LatencyConfig::new(Duration::from_millis(10))
            .with_bandwidth(1000); // 1000 bytes per second

        let mut model = LatencyJitterModel::with_seed(config, 456);

        let endpoint1 = EndpointId::new("service1".to_string());
        let endpoint2 = EndpointId::new("service2".to_string());
        
        // 500 byte message
        let message = TransportMessage::new(
            1,
            endpoint1,
            endpoint2,
            vec![0; 500],
            SimTime::zero(),
            crate::transport::MessageType::UnaryRequest,
        );

        let bandwidth_delay = model.calculate_bandwidth_delay(endpoint1, endpoint2, &message);
        
        // 500 bytes at 1000 bps = 0.5 seconds
        assert_eq!(bandwidth_delay, Duration::from_millis(500));
    }
}