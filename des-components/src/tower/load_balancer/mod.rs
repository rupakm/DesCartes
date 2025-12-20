//! DES-aware load balancer layer

use http::Request;
use rand::seq::SliceRandom;
use std::sync::atomic::{AtomicUsize, Ordering};
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
}

impl<S: Clone> Clone for DesLoadBalancer<S> {
    fn clone(&self) -> Self {
        Self {
            services: self.services.clone(),
            strategy: self.strategy.clone(),
            current_index: AtomicUsize::new(self.current_index.load(Ordering::Relaxed)),
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
        Self {
            services,
            strategy,
            current_index: AtomicUsize::new(0),
        }
    }

    pub fn round_robin(services: Vec<S>) -> Self {
        Self::new(services, DesLoadBalanceStrategy::RoundRobin)
    }

    pub fn random(services: Vec<S>) -> Self {
        Self::new(services, DesLoadBalanceStrategy::Random)
    }

    fn select_service(&self) -> usize {
        match self.strategy {
            DesLoadBalanceStrategy::RoundRobin => {
                let index = self.current_index.fetch_add(1, Ordering::Relaxed);
                index % self.services.len()
            }
            DesLoadBalanceStrategy::Random => {
                let mut rng = rand::thread_rng();
                (0..self.services.len()).collect::<Vec<_>>().choose(&mut rng).copied().unwrap_or(0)
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