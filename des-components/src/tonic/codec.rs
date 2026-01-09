//! Codec implementations for RPC message serialization/deserialization

use crate::tonic::{TonicError, TonicResult};

/// Trait for encoding and decoding RPC messages
pub trait RpcCodec<T>: Send + Sync {
    /// Encode a message to bytes
    fn encode(&self, message: &T) -> TonicResult<Vec<u8>>;
    
    /// Decode bytes to a message
    fn decode(&self, bytes: &[u8]) -> TonicResult<T>;
    
    /// Get the content type for this codec
    fn content_type(&self) -> &'static str;
}

/// Protobuf codec for messages that implement prost traits
pub struct ProtobufCodec<T> {
    _phantom: std::marker::PhantomData<T>,
}

impl<T> ProtobufCodec<T> {
    /// Create a new protobuf codec
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T> Default for ProtobufCodec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> RpcCodec<T> for ProtobufCodec<T>
where
    T: prost::Message + Default,
{
    fn encode(&self, message: &T) -> TonicResult<Vec<u8>> {
        let mut buf = Vec::new();
        message.encode(&mut buf)
            .map_err(|e| TonicError::Serialization(format!("Protobuf encode error: {}", e)))?;
        Ok(buf)
    }
    
    fn decode(&self, bytes: &[u8]) -> TonicResult<T> {
        T::decode(bytes)
            .map_err(|e| TonicError::Serialization(format!("Protobuf decode error: {}", e)))
    }
    
    fn content_type(&self) -> &'static str {
        "application/grpc+proto"
    }
}

/// JSON codec for messages that implement serde traits
pub struct JsonCodec<T> {
    _phantom: std::marker::PhantomData<T>,
}

impl<T> JsonCodec<T> {
    /// Create a new JSON codec
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T> Default for JsonCodec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> RpcCodec<T> for JsonCodec<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned + Send + Sync,
{
    fn encode(&self, message: &T) -> TonicResult<Vec<u8>> {
        serde_json::to_vec(message)
            .map_err(|e| TonicError::Serialization(format!("JSON encode error: {}", e)))
    }
    
    fn decode(&self, bytes: &[u8]) -> TonicResult<T> {
        serde_json::from_slice(bytes)
            .map_err(|e| TonicError::Serialization(format!("JSON decode error: {}", e)))
    }
    
    fn content_type(&self) -> &'static str {
        "application/grpc+json"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct TestMessage {
        id: u32,
        name: String,
    }

    #[test]
    fn test_json_codec() {
        let codec = JsonCodec::<TestMessage>::new();
        let message = TestMessage {
            id: 42,
            name: "test".to_string(),
        };

        let encoded = codec.encode(&message).unwrap();
        let decoded = codec.decode(&encoded).unwrap();

        assert_eq!(message, decoded);
        assert_eq!(codec.content_type(), "application/grpc+json");
    }

    // Note: Protobuf codec test would require prost-generated types
    // which we'll add when we have actual protobuf definitions
}