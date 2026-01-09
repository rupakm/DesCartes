//! Tonic-compatible RPC client and server implementations for discrete event simulation
//!
//! This module provides tonic-style gRPC client and server facades that work within
//! discrete event simulations. It enables testing of gRPC services with deterministic
//! network behavior while maintaining familiar tonic APIs.

pub mod client;
pub mod server;
pub mod codec;
pub mod error;

pub use client::{SimTonicClient, TonicClientBuilder, TonicClientComponent};
pub use server::{SimTonicServer, TonicServerBuilder, TonicServerComponent};
pub use codec::{ProtobufCodec, RpcCodec, JsonCodec};
pub use error::{TonicError, TonicResult};

use crate::transport::{EndpointId, MessageType, SimTransport, TransportEvent};
use des_core::{Component, Key, Scheduler, SchedulerHandle, SimTime};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

/// RPC method descriptor for routing
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MethodDescriptor {
    /// Service name (e.g., "user.UserService")
    pub service: String,
    /// Method name (e.g., "GetUser")
    pub method: String,
}

impl MethodDescriptor {
    /// Create a new method descriptor
    pub fn new(service: String, method: String) -> Self {
        Self { service, method }
    }

    /// Get the full method name (service/method)
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.service, self.method)
    }
}

/// RPC request with metadata
#[derive(Debug, Clone)]
pub struct RpcRequest {
    /// Method being called
    pub method: MethodDescriptor,
    /// Request payload (serialized protobuf)
    pub payload: Vec<u8>,
    /// Request metadata/headers
    pub metadata: HashMap<String, String>,
    /// Timeout for this request
    pub timeout: Option<std::time::Duration>,
}

impl RpcRequest {
    /// Create a new RPC request
    pub fn new(method: MethodDescriptor, payload: Vec<u8>) -> Self {
        Self {
            method,
            payload,
            metadata: HashMap::new(),
            timeout: None,
        }
    }

    /// Add metadata to the request
    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }

    /// Set request timeout
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// RPC response with metadata
#[derive(Debug, Clone)]
pub struct RpcResponse {
    /// Response payload (serialized protobuf)
    pub payload: Vec<u8>,
    /// Response metadata/headers
    pub metadata: HashMap<String, String>,
    /// Response status
    pub status: RpcStatus,
}

impl RpcResponse {
    /// Create a successful response
    pub fn success(payload: Vec<u8>) -> Self {
        Self {
            payload,
            metadata: HashMap::new(),
            status: RpcStatus::Ok,
        }
    }

    /// Create an error response
    pub fn error(code: RpcStatusCode, message: String) -> Self {
        Self {
            payload: Vec::new(),
            metadata: HashMap::new(),
            status: RpcStatus::Error { code, message },
        }
    }

    /// Add metadata to the response
    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }

    /// Check if response is successful
    pub fn is_success(&self) -> bool {
        matches!(self.status, RpcStatus::Ok)
    }
}

/// RPC status codes (subset of gRPC status codes)
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RpcStatusCode {
    Ok = 0,
    Cancelled = 1,
    Unknown = 2,
    InvalidArgument = 3,
    DeadlineExceeded = 4,
    NotFound = 5,
    AlreadyExists = 6,
    PermissionDenied = 7,
    ResourceExhausted = 8,
    FailedPrecondition = 9,
    Aborted = 10,
    OutOfRange = 11,
    Unimplemented = 12,
    Internal = 13,
    Unavailable = 14,
    DataLoss = 15,
    Unauthenticated = 16,
}

/// RPC response status
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RpcStatus {
    /// Request completed successfully
    Ok,
    /// Request failed with error code and message
    Error {
        code: RpcStatusCode,
        message: String,
    },
}

/// Events for RPC components
#[derive(Debug, Clone)]
pub enum RpcEvent {
    /// RPC request received
    RequestReceived {
        request: RpcRequest,
        response_sender: Arc<Mutex<Option<oneshot::Sender<RpcResponse>>>>,
        client_endpoint: EndpointId,
    },
    /// RPC response received
    ResponseReceived {
        response: RpcResponse,
        correlation_id: String,
    },
    /// RPC request timed out
    RequestTimeout {
        correlation_id: String,
    },
}

/// Trait for RPC service implementations
pub trait RpcService: Send + Sync {
    /// Handle an RPC request
    fn handle_request(
        &mut self,
        request: RpcRequest,
        scheduler_handle: &SchedulerHandle,
    ) -> Result<RpcResponse, TonicError>;

    /// Get the service name
    fn service_name(&self) -> &str;

    /// List supported methods
    fn supported_methods(&self) -> Vec<String>;
}

/// Utility functions for RPC serialization
pub mod utils {
    use super::*;

    /// Serialize RPC request to transport message payload
    pub fn serialize_rpc_request(request: &RpcRequest) -> Vec<u8> {
        // Simple serialization format: method_name\n\nmetadata_json\n\npayload
        let method_name = request.method.full_name();
        let metadata_json = serde_json::to_string(&request.metadata).unwrap_or_default();
        
        let mut result = Vec::new();
        result.extend_from_slice(method_name.as_bytes());
        result.extend_from_slice(b"\n\n");
        result.extend_from_slice(metadata_json.as_bytes());
        result.extend_from_slice(b"\n\n");
        result.extend_from_slice(&request.payload);
        
        result
    }

    /// Deserialize transport message payload to RPC request
    pub fn deserialize_rpc_request(payload: &[u8]) -> Result<RpcRequest, TonicError> {
        let payload_str = String::from_utf8_lossy(payload);
        let parts: Vec<&str> = payload_str.splitn(3, "\n\n").collect();
        
        if parts.len() != 3 {
            return Err(TonicError::InvalidMessage("Invalid RPC request format".to_string()));
        }
        
        // Parse method name
        let method_parts: Vec<&str> = parts[0].splitn(2, '/').collect();
        if method_parts.len() != 2 {
            return Err(TonicError::InvalidMessage("Invalid method name format".to_string()));
        }
        
        let method = MethodDescriptor::new(
            method_parts[0].to_string(),
            method_parts[1].to_string(),
        );
        
        // Parse metadata
        let metadata: HashMap<String, String> = serde_json::from_str(parts[1])
            .unwrap_or_default();
        
        // Extract payload
        let request_payload = parts[2].as_bytes().to_vec();
        
        Ok(RpcRequest {
            method,
            payload: request_payload,
            metadata,
            timeout: None,
        })
    }

    /// Serialize RPC response to transport message payload
    pub fn serialize_rpc_response(response: &RpcResponse) -> Vec<u8> {
        // Simple serialization format: status_json\n\nmetadata_json\n\npayload
        let status_json = serde_json::to_string(&response.status).unwrap_or_default();
        let metadata_json = serde_json::to_string(&response.metadata).unwrap_or_default();
        
        let mut result = Vec::new();
        result.extend_from_slice(status_json.as_bytes());
        result.extend_from_slice(b"\n\n");
        result.extend_from_slice(metadata_json.as_bytes());
        result.extend_from_slice(b"\n\n");
        result.extend_from_slice(&response.payload);
        
        result
    }

    /// Deserialize transport message payload to RPC response
    pub fn deserialize_rpc_response(payload: &[u8]) -> Result<RpcResponse, TonicError> {
        let payload_str = String::from_utf8_lossy(payload);
        let parts: Vec<&str> = payload_str.splitn(3, "\n\n").collect();
        
        if parts.len() != 3 {
            return Err(TonicError::InvalidMessage("Invalid RPC response format".to_string()));
        }
        
        // Parse status
        let status: RpcStatus = serde_json::from_str(parts[0])
            .map_err(|e| TonicError::InvalidMessage(format!("Invalid status format: {}", e)))?;
        
        // Parse metadata
        let metadata: HashMap<String, String> = serde_json::from_str(parts[1])
            .unwrap_or_default();
        
        // Extract payload
        let response_payload = parts[2].as_bytes().to_vec();
        
        Ok(RpcResponse {
            payload: response_payload,
            metadata,
            status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_method_descriptor() {
        let method = MethodDescriptor::new("user.UserService".to_string(), "GetUser".to_string());
        assert_eq!(method.service, "user.UserService");
        assert_eq!(method.method, "GetUser");
        assert_eq!(method.full_name(), "user.UserService/GetUser");
    }

    #[test]
    fn test_rpc_request_serialization() {
        let method = MethodDescriptor::new("test.Service".to_string(), "TestMethod".to_string());
        let request = RpcRequest::new(method, vec![1, 2, 3, 4])
            .with_metadata("key1".to_string(), "value1".to_string())
            .with_metadata("key2".to_string(), "value2".to_string());

        let serialized = utils::serialize_rpc_request(&request);
        let deserialized = utils::deserialize_rpc_request(&serialized).unwrap();

        assert_eq!(deserialized.method.service, "test.Service");
        assert_eq!(deserialized.method.method, "TestMethod");
        assert_eq!(deserialized.payload, vec![1, 2, 3, 4]);
        assert_eq!(deserialized.metadata.get("key1"), Some(&"value1".to_string()));
        assert_eq!(deserialized.metadata.get("key2"), Some(&"value2".to_string()));
    }

    #[test]
    fn test_rpc_response_serialization() {
        let response = RpcResponse::success(vec![5, 6, 7, 8])
            .with_metadata("response_key".to_string(), "response_value".to_string());

        let serialized = utils::serialize_rpc_response(&response);
        let deserialized = utils::deserialize_rpc_response(&serialized).unwrap();

        assert!(deserialized.is_success());
        assert_eq!(deserialized.payload, vec![5, 6, 7, 8]);
        assert_eq!(deserialized.metadata.get("response_key"), Some(&"response_value".to_string()));
    }

    #[test]
    fn test_rpc_error_response() {
        let response = RpcResponse::error(RpcStatusCode::NotFound, "User not found".to_string());

        let serialized = utils::serialize_rpc_response(&response);
        let deserialized = utils::deserialize_rpc_response(&serialized).unwrap();

        assert!(!deserialized.is_success());
        match deserialized.status {
            RpcStatus::Error { code, message } => {
                assert_eq!(code, RpcStatusCode::NotFound);
                assert_eq!(message, "User not found");
            }
            _ => panic!("Expected error status"),
        }
    }
}