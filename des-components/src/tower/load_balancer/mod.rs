//! DES-aware load balancer layer
//!
//! This module provides load balancing capabilities that distribute requests across
//! multiple backend services within the discrete event simulation. The load balancer
//! supports multiple strategies and maintains deterministic behavior for reproducible
//! simulation results.
//!
//! # Load Balancing Strategies
//!
//! ## Round Robin (`DesLoadBalanceStrategy::RoundRobin`)
//! Distributes requests evenly across all available services in a circular fashion.
//! This strategy ensures fair distribution and is deterministic across simulation runs.
//!
//! ## Random (`DesLoadBalanceStrategy::Random`)
//! Selects services randomly using a seeded random number generator for deterministic
//! behavior. This can help distribute load more evenly when services have different
//! processing characteristics.
//!
//! ## Least Connections (`DesLoadBalanceStrategy::LeastConnections`)
//! Routes requests to the service with the fewest active connections. Currently
//! implemented as round-robin but can be extended to track actual connection counts.
//!
//! # Usage Examples
//!
//! ## Basic Load Balancer Setup
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesLoadBalancer, DesLoadBalanceStrategy};
//! use des_core::Simulation;
//! use std::sync::{Arc, Mutex};
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let simulation = Arc::new(Mutex::new(Simulation::default()));
//!
//! // Create multiple backend services
//! let services = (0..3).map(|i| {
//!     DesServiceBuilder::new(format!("backend-{}", i))
//!         .thread_capacity(5)
//!         .service_time(Duration::from_millis(100))
//!         .build(simulation.clone())
//! }).collect::<Result<Vec<_>, _>>()?;
//!
//! // Create round-robin load balancer
//! let load_balancer = DesLoadBalancer::round_robin(services);
//! # Ok(())
//! # }
//! ```
//!
//! ## Using Tower Layer Pattern
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesLoadBalancerLayer, DesLoadBalanceStrategy};
//! use tower::Layer;
//!
//! # fn layer_example() -> Result<(), Box<dyn std::error::Error>> {
//! # let simulation = std::sync::Arc::new(std::sync::Mutex::new(des_core::Simulation::default()));
//! let base_service = DesServiceBuilder::new("backend".to_string())
//!     .thread_capacity(10)
//!     .service_time(std::time::Duration::from_millis(50))
//!     .build(simulation.clone())?;
//!
//! // Apply load balancer layer
//! let load_balanced_service = DesLoadBalancerLayer::round_robin()
//!     .layer(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! ## Deterministic Random Load Balancing
//!
//! ```rust,no_run
//! use des_components::tower::DesLoadBalancer;
//!
//! # fn random_example() {
//! # let services = vec![];
//! // Create random load balancer with specific seed for reproducible results
//! let load_balancer = DesLoadBalancer::random_with_seed(services, 12345);
//! # }
//! ```
//!
//! # Performance Characteristics
//!
//! - **Strategy Overhead**: Minimal computational cost for service selection
//! - **Memory Usage**: O(n) where n is the number of backend services
//! - **Determinism**: All strategies produce reproducible results across simulation runs
//! - **Scalability**: Supports arbitrary number of backend services
//!
//! # Integration with Other Middleware
//!
//! Load balancers can be composed with other Tower middleware:
//!
//! ```rust,no_run
//! use des_components::tower::*;
//! use tower::ServiceBuilder;
//!
//! # fn composition_example() -> Result<(), Box<dyn std::error::Error>> {
//! # let simulation = std::sync::Arc::new(std::sync::Mutex::new(des_core::Simulation::default()));
//! # let services = vec![
//! #     DesServiceBuilder::new("backend-1".to_string()).build(simulation.clone())?,
//! #     DesServiceBuilder::new("backend-2".to_string()).build(simulation.clone())?,
//! # ];
//! let load_balancer = DesLoadBalancer::round_robin(services);
//!
//! let service = ServiceBuilder::new()
//!     .layer(DesRateLimitLayer::new(100.0, 50, std::sync::Arc::downgrade(&simulation)))
//!     .layer(DesConcurrencyLimitLayer::new(10))
//!     .service(load_balancer);
//! # Ok(())
//! # }
//! ```

use http::Request;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tower::{Layer, Service};

use super::{ServiceError, SimBody};

/// DES-aware load balancer layer
///
/// This is the Layer implementation that creates load-balanced services.
#[derive(Clone)]
pub struct DesLoadBalancerLayer {
    strategy: DesLoadBalanceStrategy,
}

impl DesLoadBalancerLayer {
    /// Create a new load balancer layer with the specified strategy
    pub fn new(strategy: DesLoadBalanceStrategy) -> Self {
        Self { strategy }
    }

    /// Create a round-robin load balancer layer
    pub fn round_robin() -> Self {
        Self::new(DesLoadBalanceStrategy::RoundRobin)
    }

    /// Create a random load balancer layer
    pub fn random() -> Self {
        Self::new(DesLoadBalanceStrategy::Random)
    }
}

impl<S> Layer<S> for DesLoadBalancerLayer
where
    S: Clone,
{
    type Service = DesLoadBalancer<S>;

    fn layer(&self, inner: S) -> Self::Service {
        // For Layer trait, we create a single-service load balancer
        // In practice, you'd typically use this with a service discovery mechanism
        DesLoadBalancer::new(vec![inner], self.strategy.clone())
    }
}

/// DES-aware load balancer that distributes requests across multiple services
pub struct DesLoadBalancer<S> {
    services: Vec<S>,
    strategy: DesLoadBalanceStrategy,
    current_index: AtomicUsize,
    /// Deterministic RNG for random load balancing
    rng: Arc<Mutex<ChaCha8Rng>>,
}

impl<S: Clone> Clone for DesLoadBalancer<S> {
    fn clone(&self) -> Self {
        Self {
            services: self.services.clone(),
            strategy: self.strategy.clone(),
            current_index: AtomicUsize::new(self.current_index.load(Ordering::Relaxed)),
            // Clone the RNG state for deterministic behavior across clones
            rng: Arc::new(Mutex::new(
                self.rng.lock().unwrap().clone()
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub enum DesLoadBalanceStrategy {
    RoundRobin,
    Random,
    LeastConnections,
}

impl<S> DesLoadBalancer<S> {
    pub fn new(services: Vec<S>, strategy: DesLoadBalanceStrategy) -> Self {
        Self::with_seed(services, strategy, 42) // Default deterministic seed
    }

    /// Create a new load balancer with a specific seed for deterministic random behavior
    pub fn with_seed(services: Vec<S>, strategy: DesLoadBalanceStrategy, seed: u64) -> Self {
        Self {
            services,
            strategy,
            current_index: AtomicUsize::new(0),
            rng: Arc::new(Mutex::new(ChaCha8Rng::seed_from_u64(seed))),
        }
    }

    pub fn round_robin(services: Vec<S>) -> Self {
        Self::new(services, DesLoadBalanceStrategy::RoundRobin)
    }

    pub fn random(services: Vec<S>) -> Self {
        Self::new(services, DesLoadBalanceStrategy::Random)
    }

    /// Create a random load balancer with a specific seed for deterministic behavior
    pub fn random_with_seed(services: Vec<S>, seed: u64) -> Self {
        Self::with_seed(services, DesLoadBalanceStrategy::Random, seed)
    }

    fn select_service(&self) -> usize {
        match self.strategy {
            DesLoadBalanceStrategy::RoundRobin => {
                let index = self.current_index.fetch_add(1, Ordering::Relaxed);
                index % self.services.len()
            }
            DesLoadBalanceStrategy::Random => {
                // Use deterministic seeded RNG instead of thread_rng
                let mut rng = self.rng.lock().unwrap();
                rng.gen_range(0..self.services.len())
            }
            DesLoadBalanceStrategy::LeastConnections => {
                // For simplicity, use round robin for now
                // In a real implementation, you'd track active connections per service
                let index = self.current_index.fetch_add(1, Ordering::Relaxed);
                index % self.services.len()
            }
        }
    }
}

impl<S, ReqBody> Service<Request<ReqBody>> for DesLoadBalancer<S>
where
    S: Service<Request<ReqBody>, Response = http::Response<SimBody>, Error = ServiceError>,
    ReqBody: Clone,
{
    type Response = S::Response;
    type Error = ServiceError;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Check if any service is ready
        for service in &mut self.services {
            if let Poll::Ready(Ok(())) = service.poll_ready(cx) {
                return Poll::Ready(Ok(()));
            }
        }
        Poll::Pending
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let index = self.select_service();
        self.services[index].call(req)
    }
}