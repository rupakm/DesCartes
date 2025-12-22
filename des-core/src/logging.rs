//! Structured logging for discrete event simulation debugging
//!
//! This module provides comprehensive logging capabilities for the DES framework,
//! making it easy to debug simulation behavior, track event processing, and
//! monitor component interactions.
//!
//! # How to Control Terminal Logging Output
//!
//! ## 1. Use `init_detailed_simulation_logging()` (Recommended for debugging)
//! ```rust
//! use des_core::init_detailed_simulation_logging;
//! init_detailed_simulation_logging();
//! ```
//! - Shows **all** log levels (TRACE, DEBUG, INFO, WARN, ERROR)
//! - Pretty-printed format with colors and indentation
//! - Maximum visibility into simulation behavior
//!
//! ## 2. Use `init_simulation_logging_with_level()` for specific levels
//! ```rust
//! use des_core::init_simulation_logging_with_level;
//! init_simulation_logging_with_level("debug");  // DEBUG and above
//! init_simulation_logging_with_level("info");   // INFO and above (default)
//! init_simulation_logging_with_level("trace");  // Everything (very verbose)
//! ```
//!
//! ## 3. Use Environment Variables (Most flexible)
//! ```bash
//! # Default (info level)
//! cargo run --example logging_demo
//!
//! # Debug level
//! RUST_LOG=debug cargo run --example logging_demo
//!
//! # Trace level (most verbose)
//! RUST_LOG=trace cargo run --example logging_demo
//!
//! # Module-specific logging
//! RUST_LOG=des_core::scheduler=debug cargo run --example logging_demo
//!
//! # Multiple modules
//! RUST_LOG=des_core=debug,your_app=trace cargo run --example logging_demo
//! ```
//!
//! ## 4. What You'll See in Terminal Output:
//! - **Timestamps**: When each log entry occurred
//! - **Log Levels**: TRACE, DEBUG, INFO, WARN, ERROR
//! - **Module Paths**: Which part of the code generated the log
//! - **File/Line Numbers**: Exact location in source code
//! - **Thread IDs**: Which thread generated the log
//! - **Structured Fields**: Key-value pairs (component_id, request_id, etc.)
//! - **Span Context**: Nested execution context showing call hierarchy
//!
//! ## 5. Log Level Guidelines:
//! - **TRACE**: Detailed event processing and state changes (very verbose)
//! - **DEBUG**: Component interactions and scheduling decisions
//! - **INFO**: High-level simulation progress and important events
//! - **WARN**: Potential issues or unusual conditions
//! - **ERROR**: Serious problems that may affect simulation correctness
//!
//! ## 6. Examples:
//! See `examples/logging_demo.rs` and `examples/logging_env_demo.rs` for complete
//! working examples of how to use the logging system effectively.
//!


use crate::{SimTime, EventId};
use std::collections::HashMap;
use tracing::{debug, error, info, trace, warn, Span};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, filter::EnvFilter};

/// Initialize logging for the simulation with sensible defaults
///
/// This sets up structured logging with different levels for different modules:
/// - TRACE: Detailed event processing and state changes
/// - DEBUG: Component interactions and scheduling decisions  
/// - INFO: High-level simulation progress
/// - WARN: Potential issues or unusual conditions
/// - ERROR: Serious problems that may affect simulation correctness
pub fn init_simulation_logging() {
    init_simulation_logging_with_level("info")
}

/// Initialize logging with a specific level
///
/// # Arguments
/// * `level` - Log level: "trace", "debug", "info", "warn", or "error"
///
/// # Example
/// ```rust
/// use des_core::logging::init_simulation_logging_with_level;
/// 
/// // Enable detailed debugging
/// init_simulation_logging_with_level("debug");
/// ```
pub fn init_simulation_logging_with_level(level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            // Default filter with more detailed logging for DES modules
            format!(
                "{}={},des_core::scheduler=debug,des_core::async_runtime=debug,des_components=info",
                env!("CARGO_PKG_NAME"), level
            ).into()
        });

    tracing_subscriber::registry()
        .with(fmt::layer()
            .with_target(true)
            .with_thread_ids(true)
            .with_level(true)
            .with_file(true)
            .with_line_number(true)
        )
        .with(filter)
        .init();

    info!("Simulation logging initialized at level: {}", level);
}

/// Initialize logging with custom configuration for advanced debugging
pub fn init_detailed_simulation_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            // Very detailed logging for debugging
            "trace,des_core=trace,des_components=debug,des_metrics=debug".into()
        });

    tracing_subscriber::registry()
        .with(fmt::layer()
            .with_target(true)
            .with_thread_ids(true)
            .with_level(true)
            .with_file(true)
            .with_line_number(true)
            .pretty() // Pretty-printed output for easier reading
        )
        .with(filter)
        .init();

    info!("Detailed simulation logging initialized");
}

/// Create a span for tracking simulation execution
pub fn simulation_span(name: &str) -> Span {
    tracing::info_span!("simulation", name = name)
}

/// Create a span for tracking component execution
pub fn component_span(component_name: &str, component_id: &str) -> Span {
    tracing::debug_span!("component", 
        name = component_name, 
        id = component_id
    )
}

/// Create a span for tracking event processing
pub fn event_span(event_id: EventId, event_type: &str, time: SimTime) -> Span {
    tracing::trace_span!("event",
        id = ?event_id,
        event_type = event_type,
        time = ?time
    )
}

/// Create a span for tracking task execution
pub fn task_span(task_name: &str, task_id: &str) -> Span {
    tracing::debug_span!("task",
        name = task_name,
        id = task_id
    )
}

/// Logging utilities for common simulation events
pub mod events {
    use super::*;

    /// Log simulation start
    pub fn simulation_started(name: &str, end_time: Option<SimTime>) {
        match end_time {
            Some(end) => info!(
                simulation = name,
                end_time = ?end,
                "Simulation started"
            ),
            None => info!(
                simulation = name,
                "Simulation started (unbounded)"
            ),
        }
    }

    /// Log simulation completion
    pub fn simulation_completed(name: &str, final_time: SimTime, events_processed: u64) {
        info!(
            simulation = name,
            final_time = ?final_time,
            events_processed = events_processed,
            "Simulation completed"
        );
    }

    /// Log event scheduling
    pub fn event_scheduled(event_id: EventId, event_type: &str, time: SimTime, component: &str) {
        trace!(
            event_id = ?event_id,
            event_type = event_type,
            time = ?time,
            component = component,
            "Event scheduled"
        );
    }

    /// Log event processing start
    pub fn event_processing_started(event_id: EventId, event_type: &str, time: SimTime) {
        trace!(
            event_id = ?event_id,
            event_type = event_type,
            time = ?time,
            "Processing event"
        );
    }

    /// Log event processing completion
    pub fn event_processing_completed(event_id: EventId, duration_ns: u64) {
        trace!(
            event_id = ?event_id,
            duration_ns = duration_ns,
            "Event processing completed"
        );
    }

    /// Log component state change
    pub fn component_state_changed(component: &str, old_state: &str, new_state: &str) {
        debug!(
            component = component,
            old_state = old_state,
            new_state = new_state,
            "Component state changed"
        );
    }

    /// Log task spawning
    pub fn task_spawned(task_name: &str, task_id: &str) {
        debug!(
            task_name = task_name,
            task_id = task_id,
            "Task spawned"
        );
    }

    /// Log task completion
    pub fn task_completed(task_name: &str, task_id: &str, success: bool) {
        debug!(
            task_name = task_name,
            task_id = task_id,
            success = success,
            "Task completed"
        );
    }

    /// Log scheduler state
    pub fn scheduler_state(current_time: SimTime, pending_events: usize, next_event_time: Option<SimTime>) {
        trace!(
            current_time = ?current_time,
            pending_events = pending_events,
            next_event_time = ?next_event_time,
            "Scheduler state"
        );
    }

    /// Log performance metrics
    pub fn performance_metrics(
        events_per_second: f64,
        memory_usage_mb: f64,
        simulation_time_ratio: f64,
    ) {
        info!(
            events_per_second = events_per_second,
            memory_usage_mb = memory_usage_mb,
            simulation_time_ratio = simulation_time_ratio,
            "Performance metrics"
        );
    }
}

/// Logging utilities for error conditions and warnings
pub mod diagnostics {
    use super::*;

    /// Log a potential deadlock condition
    pub fn potential_deadlock_detected(component: &str, details: &str) {
        warn!(
            component = component,
            details = details,
            "Potential deadlock detected"
        );
    }

    /// Log excessive event queue growth
    pub fn excessive_queue_growth(queue_size: usize, threshold: usize) {
        warn!(
            queue_size = queue_size,
            threshold = threshold,
            "Event queue growing excessively"
        );
    }

    /// Log slow event processing
    pub fn slow_event_processing(event_type: &str, duration_ms: f64, threshold_ms: f64) {
        warn!(
            event_type = event_type,
            duration_ms = duration_ms,
            threshold_ms = threshold_ms,
            "Slow event processing detected"
        );
    }

    /// Log component errors
    pub fn component_error(component: &str, error: &str, context: &HashMap<String, String>) {
        error!(
            component = component,
            error = error,
            context = ?context,
            "Component error occurred"
        );
    }

    /// Log simulation inconsistency
    pub fn simulation_inconsistency(description: &str, expected: &str, actual: &str) {
        error!(
            description = description,
            expected = expected,
            actual = actual,
            "Simulation inconsistency detected"
        );
    }

    /// Log resource exhaustion
    pub fn resource_exhaustion(resource_type: &str, current: u64, limit: u64) {
        error!(
            resource_type = resource_type,
            current = current,
            limit = limit,
            "Resource exhaustion detected"
        );
    }
}

/// Macros for convenient logging with simulation context
#[macro_export]
macro_rules! sim_trace {
    ($($arg:tt)*) => {
        tracing::trace!($($arg)*)
    };
}

#[macro_export]
macro_rules! sim_debug {
    ($($arg:tt)*) => {
        tracing::debug!($($arg)*)
    };
}

#[macro_export]
macro_rules! sim_info {
    ($($arg:tt)*) => {
        tracing::info!($($arg)*)
    };
}

#[macro_export]
macro_rules! sim_warn {
    ($($arg:tt)*) => {
        tracing::warn!($($arg)*)
    };
}

#[macro_export]
macro_rules! sim_error {
    ($($arg:tt)*) => {
        tracing::error!($($arg)*)
    };
}

/// Helper for logging with simulation time context
#[macro_export]
macro_rules! sim_log_with_time {
    ($level:ident, $time:expr, $($arg:tt)*) => {
        tracing::$level!(time = ?$time, $($arg)*)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logging_initialization() {
        // Test that logging can be initialized without panicking
        init_simulation_logging_with_level("debug");
        
        // Test logging at different levels
        info!("Test info message");
        debug!("Test debug message");
        trace!("Test trace message");
    }

    #[test]
    fn test_span_creation() {
        let _sim_span = simulation_span("test_simulation");
        let _comp_span = component_span("test_component", "comp_1");
        let _event_span = event_span(EventId(1), "TestEvent", SimTime::from_millis(100));
        let _task_span = task_span("test_task", "task_1");
    }

    #[test]
    fn test_event_logging() {
        events::simulation_started("test_sim", Some(SimTime::from_secs(10)));
        events::event_scheduled(EventId(1), "TestEvent", SimTime::from_millis(100), "test_component");
        events::event_processing_started(EventId(1), "TestEvent", SimTime::from_millis(100));
        events::event_processing_completed(EventId(1), 1000);
        events::simulation_completed("test_sim", SimTime::from_secs(5), 100);
    }

    #[test]
    fn test_diagnostic_logging() {
        diagnostics::potential_deadlock_detected("test_component", "No events scheduled");
        diagnostics::excessive_queue_growth(10000, 1000);
        diagnostics::slow_event_processing("SlowEvent", 500.0, 100.0);
        
        let mut context = HashMap::new();
        context.insert("request_id".to_string(), "123".to_string());
        diagnostics::component_error("test_component", "Processing failed", &context);
    }

    #[test]
    fn test_logging_macros() {
        sim_info!("Test info macro");
        sim_debug!("Test debug macro");
        sim_warn!("Test warning macro");
        sim_error!("Test error macro");
        
        let time = SimTime::from_millis(100);
        sim_log_with_time!(info, time, "Test with time context");
    }
}