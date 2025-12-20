//! DES-aware implementations of tower::limit modules
//!
//! This module provides DES-aware versions of Tower's limit middleware,
//! including rate limiting, concurrency limiting, and global concurrency limiting.
//! All implementations use simulated time and work within the discrete event simulation.
//!
//! ## Key Differences from Standard Tower Limit Modules
//!
//! ### Rate Limiting (`DesRateLimit`)
//! - Uses DES simulated time instead of real system time
//! - Token bucket refill is based on simulation time progression
//! - Integrates with the DES scheduler for deterministic behavior
//!
//! ### Concurrency Limiting (`DesConcurrencyLimit`)
//! - Limits concurrent requests per service instance
//! - Uses atomic counters for thread-safe concurrency tracking
//! - Properly handles future drops to prevent resource leaks
//!
//! ### Global Concurrency Limiting (`DesGlobalConcurrencyLimit`)
//! - Shares concurrency limits across multiple service instances
//! - Uses shared state that can be cloned across services
//! - Enables system-wide concurrency control in simulations
//!
//! ## Usage Examples
//!
//! ```rust,no_run
//! use des_components::tower::limit::*;
//! use des_components::tower::{DesServiceBuilder, ServiceError, SimBody};
//! use des_core::Simulation;
//! use std::sync::{Arc, Mutex};
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), ServiceError> {
//! let simulation = Arc::new(Mutex::new(Simulation::default()));
//!
//! // Create a base service
//! let base_service = DesServiceBuilder::new("example".to_string())
//!     .thread_capacity(10)
//!     .service_time(Duration::from_millis(100))
//!     .build(simulation.clone())?;
//!
//! // Add rate limiting (5 requests per second, burst of 10)
//! let rate_limited = DesRateLimit::new(
//!     base_service,
//!     5.0,
//!     10,
//!     Arc::downgrade(&simulation),
//! );
//!
//! // Add concurrency limiting (max 3 concurrent requests)
//! let concurrency_limited = DesConcurrencyLimit::new(rate_limited, 3);
//!
//! // For global concurrency limiting across multiple services
//! let global_state = GlobalConcurrencyLimitState::new(5);
//! let globally_limited = DesGlobalConcurrencyLimit::new(
//!     concurrency_limited,
//!     global_state,
//! );
//! # Ok(())
//! # }
//! ```

pub mod rate;
pub mod concurrency;
pub mod global_concurrency;

// Re-export the main types for convenience
pub use rate::{DesRateLimit, DesRateLimitLayer};
pub use concurrency::{DesConcurrencyLimit, DesConcurrencyLimitLayer};
pub use global_concurrency::{DesGlobalConcurrencyLimit, DesGlobalConcurrencyLimitLayer, GlobalConcurrencyLimitState};