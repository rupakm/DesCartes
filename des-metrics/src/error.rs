//! Error types for metrics and logging

use thiserror::Error;

/// Errors related to metrics collection
#[derive(Debug, Error)]
pub enum MetricsError {
    #[error("Metrics backend error: {0}")]
    BackendError(String),

    #[error("Invalid metric: {0}")]
    InvalidMetric(String),

    #[error("Metric not found: {0}")]
    NotFound(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Errors related to logging
#[derive(Debug, Error)]
pub enum LogError {
    #[error("Log backend error: {0}")]
    BackendError(String),

    #[error("Invalid log entry: {0}")]
    InvalidEntry(String),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
}
