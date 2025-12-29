//! Reusable simulation components for distributed systems modeling
//!
//! This crate provides composable building blocks for simulating distributed systems,
//! including servers, clients, queues, throttles, and retry policies.

pub mod builder;
pub mod dists;
// Note: client.rs was removed due to incompatibility with current Component API
// Use simple_client.rs for basic client functionality
pub mod error;

pub mod simple_client;
pub mod server;
pub mod tower;

// Export distribution patterns
pub use dists::{ArrivalPattern, ServiceTimeDistribution, ConstantArrivalPattern, ConstantServiceTime};

pub use simple_client::{SimpleClient, ClientEvent};
pub use server::{Server, ServerEvent};
pub use tower::{DesService, DesServiceBuilder, SchedulerHandle, ServiceError, SimBody};
pub use tower::{DesTimeout, DesLoadBalancer, DesCircuitBreaker, DesLoadBalanceStrategy, DesRateLimit, DesConcurrencyLimit, DesGlobalConcurrencyLimit, DesHedge};
pub use tower::retry::{DesRetryLayer, DesRetryPolicy, exponential_backoff_layer, ExponentialBackoff};

pub use builder::{
    BuilderState, IntoOption, Set, Unset, Validate, ValidationError, ValidationResult,
    validate_non_empty, validate_non_negative, validate_positive, validate_range,
};
pub use error::{ComponentError, QueueError, ThrottleError, RequestError};

pub mod queue;
pub use queue::{FifoQueue, PriorityQueue, Queue, QueueItem};

pub use des_core::{Request, RequestAttempt, RequestId, RequestAttemptId, RequestStatus, AttemptStatus, Response, ResponseStatus};



