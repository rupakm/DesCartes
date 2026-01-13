//! DES-aware limit middleware.
//!
//! Provides rate limiting, concurrency limiting, and global concurrency limiting
//! using simulation time for deterministic behavior.
//!
//! # Usage
//!
//! ```rust,no_run
//! use des_tower::limit::*;
//! use des_tower::{DesServiceBuilder, ServiceError};
//! use des_core::Simulation;
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), ServiceError> {
//! let mut simulation = Simulation::default();
//! des_tokio::runtime::install(&mut simulation);
//!
//! // Create a base service
//! let base_service = DesServiceBuilder::new("example".to_string())
//!     .thread_capacity(10)
//!     .service_time(Duration::from_millis(100))
//!     .build(&mut simulation)?;
//!
//! // Add rate limiting (5 requests per second, burst of 10)
//! let rate_limited = DesRateLimit::new(base_service, 5.0, 10);
//!
//! // Add concurrency limiting (max 3 concurrent requests)
//! let concurrency_limited = DesConcurrencyLimit::new(rate_limited, 3);
//!
//! // For global concurrency limiting across multiple services
//! let global_state = GlobalConcurrencyLimitState::new(5);
//! let globally_limited = DesGlobalConcurrencyLimit::new(concurrency_limited, global_state);
//! # Ok(())
//! # }
//! ```

pub mod concurrency;
pub mod global_concurrency;
pub mod rate;

// Re-export the main types for convenience
pub use concurrency::{DesConcurrencyLimit, DesConcurrencyLimitLayer};
pub use global_concurrency::{
    DesGlobalConcurrencyLimit, DesGlobalConcurrencyLimitLayer, GlobalConcurrencyLimitState,
};
pub use rate::{DesRateLimit, DesRateLimitLayer};
