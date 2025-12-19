//! Error types for the simulation framework

use thiserror::Error;

/// Top-level error type for simulation operations
#[derive(Debug, Error)]
pub enum SimError {
    #[error("Event error: {0}")]
    Event(#[from] EventError),

    #[error("Process error: {0}")]
    Process(String),

    #[error("Component error: {0}")]
    Component(String),

    #[error("Invalid configuration: {0}")]
    Configuration(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Errors related to event scheduling and handling
#[derive(Debug, Error)]
pub enum EventError {
    #[error("Invalid event time: {0}")]
    InvalidTime(String),

    #[error("Event handler failed: {0}")]
    HandlerFailed(String),

    #[error("Event not found: {0}")]
    NotFound(String),

    #[error("Event queue is empty")]
    EmptyQueue,
}
