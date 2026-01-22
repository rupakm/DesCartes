//! Transport layer abstraction for simulated RPC communication
//!
//! This module provides the foundation for simulating network transport in discrete event
//! simulations. It supports configurable network models for latency, jitter, packet loss,
//! and bandwidth constraints while maintaining deterministic behavior.

pub mod endpoint_registry;
pub mod network_model;
pub mod sim_transport;

pub use endpoint_registry::{
    EndpointId, EndpointInfo, EndpointRegistry, SharedEndpointRegistry, SimEndpointRegistry,
};
pub use network_model::{LatencyConfig, LatencyJitterModel, NetworkModel, SimpleNetworkModel};
pub use sim_transport::SimTransport;

use des_core::SimTime;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Events that can be processed by transport components
#[derive(Debug, Clone)]
pub enum TransportEvent {
    /// Request to send a message through the transport
    SendMessage { message: TransportMessage },
    /// A message has been delivered to its destination
    MessageDelivered { message: TransportMessage },
    /// A message was dropped due to network conditions
    MessageDropped {
        message: TransportMessage,
        reason: String,
    },
}

/// A message being transported through the simulated network
#[derive(Debug, Clone)]
pub struct TransportMessage {
    /// Unique identifier for this message
    pub id: u64,
    /// Source endpoint
    pub source: EndpointId,
    /// Destination endpoint
    pub destination: EndpointId,
    /// Message payload
    pub payload: Vec<u8>,
    /// Simulation time when message was sent
    pub sent_at: SimTime,
    /// Optional correlation ID for request/response matching
    pub correlation_id: Option<String>,
    /// Message type (request, response, etc.)
    pub message_type: MessageType,
}

/// Type of transport message
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageType {
    /// Unary RPC request
    UnaryRequest,
    /// Unary RPC response
    UnaryResponse,
    /// A framed message for RPC streaming.
    ///
    /// The payload is an opaque stream frame (encoded at a higher layer like `des-tonic`).
    RpcStreamFrame,
    /// Streaming RPC message
    StreamMessage,
    /// Stream close signal
    StreamClose,
}

impl TransportMessage {
    /// Create a new transport message
    pub fn new(
        id: u64,
        source: EndpointId,
        destination: EndpointId,
        payload: Vec<u8>,
        sent_at: SimTime,
        message_type: MessageType,
    ) -> Self {
        Self {
            id,
            source,
            destination,
            payload,
            sent_at,
            correlation_id: None,
            message_type,
        }
    }

    /// Set correlation ID for request/response matching
    pub fn with_correlation_id(mut self, correlation_id: String) -> Self {
        self.correlation_id = Some(correlation_id);
        self
    }

    /// Calculate message size in bytes
    pub fn size(&self) -> usize {
        self.payload.len()
    }

    /// Calculate time since message was sent
    pub fn age(&self, current_time: SimTime) -> Duration {
        current_time.duration_since(self.sent_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_type_rpc_stream_frame_serde_roundtrip() {
        let ty = MessageType::RpcStreamFrame;

        let json = serde_json::to_string(&ty).expect("serialize MessageType");
        let decoded: MessageType = serde_json::from_str(&json).expect("deserialize MessageType");

        assert_eq!(decoded, ty);
        assert_eq!(format!("{ty:?}"), "RpcStreamFrame");
    }
}
