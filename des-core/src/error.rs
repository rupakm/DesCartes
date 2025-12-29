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

    #[error("Task execution failed: {message}")]
    TaskExecutionFailed { message: String },

    #[error("Scheduler error: {message}")]
    SchedulerError { message: String },

    #[error("Time validation error: expected non-negative time")]
    InvalidTime,

    #[error("Component not found with ID: {id}")]
    ComponentNotFound { id: String },
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

    #[error("Event scheduling failed: cannot schedule event in the past")]
    ScheduleInPast,

    #[error("Event type mismatch: expected {expected}, got {actual}")]
    TypeMismatch { expected: String, actual: String },

    #[error("Event processing timeout after {duration:?}")]
    ProcessingTimeout { duration: std::time::Duration },

    #[error("Maximum event queue size exceeded: {current}/{max}")]
    QueueOverflow { current: usize, max: usize },
}
