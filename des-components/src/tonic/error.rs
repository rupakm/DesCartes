//! Error types for tonic simulation support

use thiserror::Error;

/// Result type for tonic operations
pub type TonicResult<T> = Result<T, TonicError>;

/// Errors that can occur in tonic simulation
#[derive(Debug, Error, Clone)]
pub enum TonicError {
    #[error("Transport error: {0}")]
    Transport(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Invalid message format: {0}")]
    InvalidMessage(String),

    #[error("Service not found: {0}")]
    ServiceNotFound(String),

    #[error("Method not found: {service}/{method}")]
    MethodNotFound { service: String, method: String },

    #[error("Request timeout after {duration:?}")]
    Timeout { duration: std::time::Duration },

    #[error("RPC failed with status {code:?}: {message}")]
    RpcFailed {
        code: crate::tonic::RpcStatusCode,
        message: String,
    },

    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<serde_json::Error> for TonicError {
    fn from(err: serde_json::Error) -> Self {
        TonicError::Serialization(err.to_string())
    }
}
