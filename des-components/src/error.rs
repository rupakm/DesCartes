//! Error types for simulation components

use thiserror::Error;

/// Errors related to component operations
#[derive(Debug, Error)]
pub enum ComponentError {
    #[error("Component initialization failed: {0}")]
    InitializationFailed(String),

    #[error("Component shutdown failed: {0}")]
    ShutdownFailed(String),

    #[error("Invalid component configuration: {0}")]
    InvalidConfiguration(String),

    #[error("Component operation failed: {0}")]
    OperationFailed(String),
}

/// Errors related to queue operations
#[derive(Debug, Error)]
pub enum QueueError {
    #[error("Queue is full (capacity: {capacity})")]
    Full { capacity: usize },

    #[error("Queue is empty")]
    Empty,

    #[error("Invalid queue operation: {0}")]
    InvalidOperation(String),
}

/// Errors related to throttle operations
#[derive(Debug, Error)]
pub enum ThrottleError {
    #[error("Insufficient tokens (requested: {requested}, available: {available})")]
    InsufficientTokens { requested: usize, available: usize },

    #[error("Throttle configuration error: {0}")]
    ConfigurationError(String),

    #[error("Throttle operation failed: {0}")]
    OperationFailed(String),
}

/// Errors related to request processing
#[derive(Debug, Error)]
pub enum RequestError {
    #[error("Request timeout after {duration:?}")]
    Timeout { duration: std::time::Duration },

    #[error("Server error: {0}")]
    ServerError(String),

    #[error("Request rejected: {0}")]
    Rejected(String),

    #[error("All retry attempts exhausted (attempts: {attempts})")]
    RetriesExhausted { attempts: usize },

    #[error("Circuit breaker is open")]
    CircuitBreakerOpen,

    #[error("Invalid request: {0}")]
    InvalidRequest(String),
}
