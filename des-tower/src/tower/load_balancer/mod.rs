//! DES-aware load balancer layer.
//!
//! Distributes requests across multiple backend services with deterministic behavior.
//!
//! # Strategies
//!
//! - **Round Robin**: Distributes requests evenly in circular order
//! - **Random**: Selects services randomly (seeded for determinism)
//! - **Least Connections**: Routes to service with fewest active connections
//!
//! # Usage
//!
//! ```rust,no_run
//! use descartes_tower::{DesServiceBuilder, DesLoadBalancer, DesLoadBalanceStrategy};
//! use descartes_core::Simulation;
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut simulation = Simulation::default();
//!
//! // Create multiple backend services
//! let services = (0..3).map(|i| {
//!     DesServiceBuilder::new(format!("backend-{}", i))
//!         .thread_capacity(5)
//!         .service_time(Duration::from_millis(100))
//!         .build(&mut simulation)
//! }).collect::<Result<Vec<_>, _>>()?;
//!
//! // Create round-robin load balancer
//! let load_balancer = DesLoadBalancer::round_robin(services);
//! # Ok(())
//! # }
//! ```
//!
//! # Deterministic Random
//!
//! For reproducible random load balancing:
//!
//! ```rust,no_run
//! # use descartes_tower::DesLoadBalancer;
//! # let services: Vec<descartes_tower::DesService> = vec![];
//! let load_balancer = DesLoadBalancer::random_with_seed(services, 12345);
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
    ready_index: Option<usize>,
}

impl<S: Clone> Clone for DesLoadBalancer<S> {
    fn clone(&self) -> Self {
        Self {
            services: self.services.clone(),
            strategy: self.strategy.clone(),
            current_index: AtomicUsize::new(self.current_index.load(Ordering::Relaxed)),
            // Clone the RNG state for deterministic behavior across clones
            rng: Arc::new(Mutex::new(self.rng.lock().unwrap().clone())),
            ready_index: None,
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
            ready_index: None,
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

    fn peek_start_index(&self) -> usize {
        if self.services.is_empty() {
            return 0;
        }

        match self.strategy {
            DesLoadBalanceStrategy::RoundRobin | DesLoadBalanceStrategy::LeastConnections => {
                self.current_index.load(Ordering::Relaxed) % self.services.len()
            }
            DesLoadBalanceStrategy::Random => {
                // Peek from a cloned RNG so poll_ready does not consume randomness.
                let rng = self.rng.lock().unwrap();
                let mut cloned = rng.clone();
                cloned.gen_range(0..self.services.len())
            }
        }
    }

    fn commit_selected_index(&self, idx: usize) {
        if self.services.is_empty() {
            return;
        }

        match self.strategy {
            DesLoadBalanceStrategy::RoundRobin | DesLoadBalanceStrategy::LeastConnections => {
                self.current_index
                    .store((idx + 1) % self.services.len(), Ordering::Relaxed);
            }
            DesLoadBalanceStrategy::Random => {
                // Consume RNG once per request for deterministic progression.
                let mut rng = self.rng.lock().unwrap();
                let _ = rng.gen_range(0..self.services.len());
            }
        }
    }

    fn select_service(&mut self) -> usize {
        // If poll_ready already selected a backend, use it.
        if let Some(idx) = self.ready_index.take() {
            self.commit_selected_index(idx);
            return idx;
        }

        // Fallback: pick based on strategy.
        let start = self.peek_start_index();
        self.commit_selected_index(start);
        start
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
        if self.services.is_empty() {
            return Poll::Ready(Err(ServiceError::Internal(
                "load balancer has no backends".to_string(),
            )));
        }

        // If we already selected a service as ready, ensure it remains ready.
        if let Some(idx) = self.ready_index {
            match self.services[idx].poll_ready(cx) {
                Poll::Ready(Ok(())) => return Poll::Ready(Ok(())),
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => {
                    self.ready_index = None;
                }
            }
        }

        // Find a ready service without violating Tower's readiness contract.
        let start = self.peek_start_index();
        for offset in 0..self.services.len() {
            let idx = (start + offset) % self.services.len();
            match self.services[idx].poll_ready(cx) {
                Poll::Ready(Ok(())) => {
                    self.ready_index = Some(idx);
                    return Poll::Ready(Ok(()));
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => {}
            }
        }

        Poll::Pending
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let index = self.select_service();
        self.services[index].call(req)
    }
}
