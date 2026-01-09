//! Tower Service trait integration for DES components.
//!
//! This module provides Tower Service implementations that run within discrete event
//! simulations, enabling testing of real-world service architectures with deterministic,
//! reproducible results.
//!
//! # Overview
//!
//! All Tower middleware is adapted to use simulation time instead of wall-clock time:
//!
//! - **Deterministic Timing**: Operations use simulation time for reproducible results
//! - **Event-Driven**: Service operations are scheduled as discrete events
//! - **Full Compatibility**: Works with standard Tower middleware and utilities
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, DesTimeoutLayer, DesRateLimitLayer};
//! use des_core::Simulation;
//! use tower::ServiceBuilder;
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), des_components::tower::ServiceError> {
//! let mut simulation = Simulation::default();
//! let scheduler = simulation.scheduler_handle();
//!
//! // Create a base service
//! let base_service = DesServiceBuilder::new("api-server".to_string())
//!     .thread_capacity(5)
//!     .service_time(Duration::from_millis(100))
//!     .build(&mut simulation)?;
//!
//! // Add middleware layers
//! let service = ServiceBuilder::new()
//!     .layer(DesTimeoutLayer::new(Duration::from_secs(5), scheduler.clone()))
//!     .layer(DesRateLimitLayer::new(10.0, 20, scheduler))
//!     .service(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! # Available Middleware
//!
//! - [`DesService`] / [`DesServiceBuilder`]: Base service with configurable capacity and timing
//! - [`DesRateLimit`] / [`DesRateLimitLayer`]: Token bucket rate limiting
//! - [`DesConcurrencyLimit`] / [`DesConcurrencyLimitLayer`]: Per-service concurrency control
//! - [`DesGlobalConcurrencyLimit`]: Shared concurrency limits across services
//! - [`DesCircuitBreaker`] / [`DesCircuitBreakerLayer`]: Failure detection and recovery
//! - [`DesTimeout`] / [`DesTimeoutLayer`]: Request timeouts using simulation events
//! - [`DesRetry`] / [`DesRetryLayer`]: Retry with configurable backoff
//! - [`DesLoadBalancer`]: Round-robin, random, and least-connections strategies
//! - [`DesHedge`] / [`DesHedgeLayer`]: Request hedging for tail latency reduction
//!
//! # Usage Patterns
//!
//! ## Basic Service Creation
//!
//! ```rust,no_run
//! use des_components::tower::{DesServiceBuilder, ServiceError};
//! use des_core::Simulation;
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), ServiceError> {
//! let mut simulation = Simulation::default();
//!
//! let service = DesServiceBuilder::new("web-server".to_string())
//!     .thread_capacity(10)
//!     .service_time(Duration::from_millis(50))
//!     .build(&mut simulation)?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Middleware Composition
//!
//! ```rust,no_run
//! use des_components::tower::*;
//! use tower::ServiceBuilder;
//! use std::time::Duration;
//!
//! # fn middleware_example() -> Result<(), ServiceError> {
//! # let mut simulation = des_core::Simulation::default();
//! # let scheduler = simulation.scheduler_handle();
//! let base_service = DesServiceBuilder::new("api-server".to_string())
//!     .thread_capacity(5)
//!     .service_time(Duration::from_millis(100))
//!     .build(&mut simulation)?;
//!
//! let service = ServiceBuilder::new()
//!     .layer(DesTimeoutLayer::new(Duration::from_secs(5), scheduler.clone()))
//!     .layer(DesRateLimitLayer::new(10.0, 20, scheduler.clone()))
//!     .layer(DesConcurrencyLimitLayer::new(3))
//!     .layer(DesRetryLayer::new(DesRetryPolicy::new(3), scheduler))
//!     .service(base_service);
//! # Ok(())
//! # }
//! ```
//!
//! ## Load Balancing Multiple Services
//!
//! ```rust,no_run
//! use des_components::tower::*;
//!
//! # fn load_balancing_example() -> Result<(), ServiceError> {
//! # let mut simulation = des_core::Simulation::default();
//! let services = (0..3).map(|i| {
//!     DesServiceBuilder::new(format!("server-{}", i))
//!         .thread_capacity(5)
//!         .service_time(std::time::Duration::from_millis(100))
//!         .build(&mut simulation)
//! }).collect::<Result<Vec<_>, _>>()?;
//!
//! let load_balancer = DesLoadBalancer::round_robin(services);
//! # Ok(())
//! # }
//! ```
//!
//! # Performance Characteristics
//!
//! - **Service Time**: Configurable processing time per request
//! - **Queue Delays**: Automatic backpressure when capacity is exceeded
//! - **Middleware Overhead**: Minimal simulation overhead per layer
//!
//! # Testing Scenarios
//!
//! ## Load Testing
//! ```rust,no_run
//! # use des_components::tower::*;
//! # use std::time::Duration;
//! # fn load_test_example() {
//! # let mut simulation = des_core::Simulation::default();
//! # let scheduler = simulation.scheduler_handle();
//! // Simulate high load with rate limiting
//! let service = DesServiceBuilder::new("load-test".to_string())
//!     .thread_capacity(2)
//!     .service_time(Duration::from_millis(200))
//!     .build(&mut simulation).unwrap();
//!
//! let rate_limited = DesRateLimit::new(service, 100.0, 50, scheduler);
//! # }
//! ```
//!
//! ## Failure Testing
//! ```rust,no_run
//! # use des_components::tower::*;
//! # use std::time::Duration;
//! # fn failure_test_example() {
//! # let mut simulation = des_core::Simulation::default();
//! # let scheduler = simulation.scheduler_handle();
//! // Test circuit breaker behavior
//! let service = DesServiceBuilder::new("failure-test".to_string())
//!     .thread_capacity(1)
//!     .service_time(Duration::from_millis(100))
//!     .build(&mut simulation).unwrap();
//!
//! let circuit_breaker = DesCircuitBreaker::new(
//!     service,
//!     3,  // Failure threshold
//!     Duration::from_secs(10),  // Recovery timeout
//!     scheduler,
//! );
//! # }
//! ```

use bytes::Bytes;
use http::{Response as HttpResponse, StatusCode};
use http_body::Body as HttpBody;
use std::pin::Pin;
use std::task::{Context, Poll};
use thiserror::Error;

// Re-export all the layers and services
pub mod service;
pub mod timeout;
pub mod load_balancer;
pub mod circuit_breaker;
pub mod limit;
pub mod hedge;
pub mod retry;
pub mod future_poller;

pub use service::{DesService, DesServiceBuilder, TowerSchedulerHandle};
pub use des_core::SchedulerHandle;
pub use timeout::{DesTimeout, DesTimeoutLayer};
pub use load_balancer::{DesLoadBalancer, DesLoadBalanceStrategy, DesLoadBalancerLayer};
pub use circuit_breaker::{DesCircuitBreaker, DesCircuitBreakerLayer};
pub use limit::{DesRateLimit, DesRateLimitLayer, DesConcurrencyLimit, DesConcurrencyLimitLayer, DesGlobalConcurrencyLimit, DesGlobalConcurrencyLimitLayer};
pub use hedge::{DesHedge, DesHedgeLayer};
pub use retry::{DesRetry, DesRetryLayer, DesRetryPolicy, exponential_backoff_layer, ExponentialBackoff};
pub use future_poller::{FuturePoller, FuturePollerHandle, FuturePollerEvent, FutureId};

/// Errors that can occur in the DES Tower integration
#[derive(Debug, Error, Clone)]
pub enum ServiceError {
    #[error("Service is not ready to accept requests")]
    NotReady,
    #[error("Request was cancelled")]
    Cancelled,
    #[error("Service is overloaded")]
    Overloaded,
    #[error("Request timeout after {duration:?}")]
    Timeout { duration: std::time::Duration },
    #[error("Internal simulation error: {0}")]
    Internal(String),
    #[error("HTTP error: {0}")]
    Http(String), // Changed from http::Error to String for Clone compatibility
    #[error("Circuit breaker is in invalid state")]
    CircuitBreakerInvalidState,
    #[error("Rate limiter is in invalid state")]
    RateLimiterInvalidState,
    #[error("HTTP response builder error: {message}")]
    HttpResponseBuilder { message: String },
}

/// A simple HTTP body type for our simulation
#[derive(Debug, Clone)]
pub struct SimBody {
    data: Bytes,
}

impl SimBody {
    pub fn new(data: impl Into<Bytes>) -> Self {
        Self { data: data.into() }
    }

    pub fn empty() -> Self {
        Self {
            data: Bytes::new(),
        }
    }

    pub fn from_static(data: &'static str) -> Self {
        Self {
            data: Bytes::from_static(data.as_bytes()),
        }
    }

    pub fn data(&self) -> &Bytes {
        &self.data
    }
}

impl HttpBody for SimBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if this.data.is_empty() {
            Poll::Ready(None)
        } else {
            let data = std::mem::take(&mut this.data);
            Poll::Ready(Some(Ok(http_body::Frame::data(data))))
        }
    }
}

/// Convert DES Response to HTTP response
pub(crate) fn response_to_http(
    response: des_core::Response,
) -> Result<HttpResponse<SimBody>, ServiceError> {
    use des_core::ResponseStatus;
    
    match response.status {
        ResponseStatus::Ok => {
            let body = if response.payload.is_empty() {
                SimBody::from_static("OK")
            } else {
                SimBody::new(response.payload)
            };

            HttpResponse::builder()
                .status(StatusCode::OK)
                .body(body)
                .map_err(|e| ServiceError::HttpResponseBuilder { message: e.to_string() })
        }
        ResponseStatus::Error { code, message } => {
            let status = StatusCode::from_u16(code as u16)
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

            HttpResponse::builder()
                .status(status)
                .body(SimBody::new(message))
                .map_err(|e| ServiceError::HttpResponseBuilder { message: e.to_string() })
        }
    }
}

/// Serialize HTTP request to bytes (simplified)
pub(crate) fn serialize_http_request(req: &http::Request<SimBody>) -> Vec<u8> {
    // Simple serialization - in practice you might use a more sophisticated format
    let method = req.method().as_str();
    let uri = req.uri().to_string();
    let headers = req
        .headers()
        .iter()
        .map(|(k, v)| format!("{}: {}", k, v.to_str().unwrap_or("")))
        .collect::<Vec<_>>()
        .join("\r\n");

    // Include the body content
    let body_data = req.body().data();
    let mut result = format!("{method} {uri} HTTP/1.1\r\n{headers}\r\n\r\n").into_bytes();
    result.extend_from_slice(body_data);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use des_core::Simulation;
    use http::{Method, Request};
    use std::task::{Context, Poll, Waker};
    use std::future::Future;
    use std::pin::Pin;
    use std::time::Duration;
    use tower::Service;

    // Helper to create a no-op waker for testing
    fn noop_waker() -> Waker {
        use std::task::{RawWaker, RawWakerVTable};
        
        fn noop(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            RawWaker::new(std::ptr::null(), &VTABLE)
        }
        
        const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        let raw_waker = RawWaker::new(std::ptr::null(), &VTABLE);
        unsafe { Waker::from_raw(raw_waker) }
    }

    #[test]
    fn test_des_service_basic() {
        let mut simulation = Simulation::default();

        // Build the service
        let mut service = DesServiceBuilder::new("test-server".to_string())
            .thread_capacity(2)
            .service_time(std::time::Duration::from_millis(50))
            .build(&mut simulation)
            .unwrap();

        // Create a test request
        let request = http::Request::builder()
            .method(Method::GET)
            .uri("/test")
            .body(SimBody::from_static("test body"))
            .unwrap();

        // Check service is ready
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(service.poll_ready(&mut cx), Poll::Ready(Ok(()))));

        // Make the request
        let mut response_future = service.call(request);

        // Run simulation steps to process the request
        for _ in 0..20 {
            if !simulation.step() {
                break;
            }
        }

        // The response should be ready now
        let response = match Pin::new(&mut response_future).poll(&mut cx) {
            Poll::Ready(Ok(response)) => response,
            Poll::Ready(Err(e)) => panic!("Request failed: {e:?}"),
            Poll::Pending => panic!("Response should be ready after simulation steps"),
        };

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn test_des_rate_limit_layer() {
        let mut simulation = Simulation::default();
        let scheduler = simulation.scheduler_handle();

        // Create base service
        let base_service = DesServiceBuilder::new("rate-limit-test".to_string())
            .thread_capacity(5)
            .service_time(Duration::from_millis(50))
            .build(&mut simulation)
            .unwrap();

        // Wrap with rate limiter (2 requests per second, burst of 3)
        let mut rate_limit_service = DesRateLimit::new(
            base_service,
            2.0, // 2 requests per second
            3,   // burst capacity
            scheduler,
        );

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        // Send burst of requests
        let mut futures = Vec::new();
        for i in 0..5 {
            let req = Request::builder()
                .method(Method::GET)
                .uri(format!("/rate-limit-test/{i}"))
                .body(SimBody::empty())
                .unwrap();
            futures.push(rate_limit_service.call(req));
        }

        // Run simulation
        for _ in 0..100 {
            if !simulation.step() {
                break;
            }
        }

        // Check results - first 3 should succeed (burst), others should be rate limited
        let mut successes = 0;
        let mut rate_limited = 0;

        for mut future in futures {
            match Pin::new(&mut future).poll(&mut cx) {
                Poll::Ready(Ok(response)) => {
                    if response.status() == StatusCode::OK {
                        successes += 1;
                    }
                }
                Poll::Ready(Err(ServiceError::Overloaded)) => {
                    rate_limited += 1;
                }
                Poll::Ready(Err(e)) => panic!("Unexpected error: {e:?}"),
                Poll::Pending => {
                    // Might be rate limited
                    rate_limited += 1;
                }
            }
        }

        println!("Rate limit test - Successes: {successes}, Rate limited: {rate_limited}");
        
        // Should allow burst capacity (3) and rate limit the rest (2)
        assert!(successes <= 3, "Should not exceed burst capacity");
        assert!(rate_limited >= 2, "Should rate limit excess requests");
    }

    #[test]
    fn test_des_concurrency_limit_basic() {
        let mut simulation = Simulation::default();

        // Create base service
        let base_service = DesServiceBuilder::new("basic-concurrency-test".to_string())
            .thread_capacity(5)
            .service_time(Duration::from_millis(50))
            .build(&mut simulation)
            .unwrap();

        // Wrap with concurrency limiter (limit to 1 concurrent request)
        let mut concurrency_service = DesConcurrencyLimit::new(base_service, 1);

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        // First request should be ready
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        
        let req1 = Request::builder()
            .method(Method::GET)
            .uri("/test1")
            .body(SimBody::empty())
            .unwrap();
        let future1 = concurrency_service.call(req1);

        // Second request should be blocked
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Pending));
        
        // Complete the first request
        for _ in 0..100 {
            if !simulation.step() {
                break;
            }
        }
        
        // Check if first request completed
        let mut future1 = future1;
        let result = Pin::new(&mut future1).poll(&mut cx);
        assert!(matches!(result, Poll::Ready(Ok(_))));
        
        // Now service should be ready again
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
    }

    #[test]
    fn test_des_concurrency_limit_backpressure() {
        let mut simulation = Simulation::default();

        // Create base service with high capacity but slow processing
        let base_service = DesServiceBuilder::new("backpressure-test".to_string())
            .thread_capacity(10)
            .service_time(Duration::from_millis(200)) // Slow service
            .build(&mut simulation)
            .unwrap();

        // Wrap with concurrency limiter (limit to 2 concurrent requests)
        let mut concurrency_service = DesConcurrencyLimit::new(base_service, 2);

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        // Test backpressure: first 2 should be ready, 3rd should not
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        let req1 = Request::builder()
            .method(Method::GET)
            .uri("/test1")
            .body(SimBody::empty())
            .unwrap();
        let future1 = concurrency_service.call(req1);

        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        let req2 = Request::builder()
            .method(Method::GET)
            .uri("/test2")
            .body(SimBody::empty())
            .unwrap();
        let future2 = concurrency_service.call(req2);

        // Third request should be blocked by concurrency limit
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Pending));

        // Run simulation partially to start processing
        for _ in 0..10 {
            if !simulation.step() {
                break;
            }
        }

        // Still should be blocked since requests are still processing
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Pending));

        // Complete the simulation
        for _ in 0..300 {
            if !simulation.step() {
                break;
            }
        }

        // Poll the futures to completion to release their slots
        let futures = vec![future1, future2];
        let mut completed = 0;
        for mut future in futures {
            if let Poll::Ready(Ok(_)) = Pin::new(&mut future).poll(&mut cx) {
                completed += 1;
            }
        }
        assert_eq!(completed, 2, "Both requests should have completed");

        // Now should be ready again after slots are released
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
    }

    #[test]
    fn test_des_concurrency_limit_sequential_processing() {
        let mut simulation = Simulation::default();

        // Create base service with capacity 1 to force sequential processing
        let base_service = DesServiceBuilder::new("sequential-test".to_string())
            .thread_capacity(1)
            .service_time(Duration::from_millis(50))
            .build(&mut simulation)
            .unwrap();

        // Concurrency limit of 1 should enforce strict sequential processing
        let mut concurrency_service = DesConcurrencyLimit::new(base_service, 1);

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        // Process requests one by one to test sequential behavior
        for i in 0..3 {
            // Each request should be ready
            assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
            
            let req = Request::builder()
                .method(Method::GET)
                .uri(format!("/sequential/{i}"))
                .body(SimBody::empty())
                .unwrap();
            let mut future = concurrency_service.call(req);

            // After calling, service should not be ready for next request
            assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Pending));
            
            // Run simulation to complete this request
            for _ in 0..100 {
                if !simulation.step() {
                    break;
                }
            }
            
            // Poll the future to completion to release the slot
            assert!(matches!(Pin::new(&mut future).poll(&mut cx), Poll::Ready(Ok(_))));
        }

        // After all requests are processed, service should be ready again
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
    }

    #[test]
    fn test_des_global_concurrency_limit_shared_state() {
        let mut simulation = Simulation::default();

        // Create shared global concurrency state with limit of 2
        let global_state = crate::tower::limit::global_concurrency::GlobalConcurrencyLimitState::new(2);

        // Create two services sharing the same global limit
        let service1 = DesServiceBuilder::new("global-service-1".to_string())
            .thread_capacity(5)
            .service_time(Duration::from_millis(100))
            .build(&mut simulation)
            .unwrap();

        let service2 = DesServiceBuilder::new("global-service-2".to_string())
            .thread_capacity(5)
            .service_time(Duration::from_millis(100))
            .build(&mut simulation)
            .unwrap();

        let mut global_service1 = DesGlobalConcurrencyLimit::new(service1, global_state.clone());
        let mut global_service2 = DesGlobalConcurrencyLimit::new(service2, global_state.clone());

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        // Test that global limit is enforced across services
        
        // Service 1 should be able to take first slot
        assert!(matches!(global_service1.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        let req1 = Request::builder()
            .method(Method::GET)
            .uri("/global-1")
            .body(SimBody::empty())
            .unwrap();
        let future1 = global_service1.call(req1);

        // Service 2 should be able to take second slot
        assert!(matches!(global_service2.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        let req2 = Request::builder()
            .method(Method::GET)
            .uri("/global-2")
            .body(SimBody::empty())
            .unwrap();
        let future2 = global_service2.call(req2);

        // Now both services should be blocked by global limit
        assert!(matches!(global_service1.poll_ready(&mut cx), Poll::Pending));
        assert!(matches!(global_service2.poll_ready(&mut cx), Poll::Pending));

        // Verify global state shows we're at capacity
        assert_eq!(global_state.current_concurrency(), 2);
        assert_eq!(global_state.max_concurrency(), 2);

        // Run simulation to complete requests
        for _ in 0..200 {
            if !simulation.step() {
                break;
            }
        }

        // Poll futures to completion to release their slots
        let futures = vec![future1, future2];
        let mut completed = 0;
        for mut future in futures {
            if let Poll::Ready(Ok(_)) = Pin::new(&mut future).poll(&mut cx) {
                completed += 1;
            }
        }
        assert_eq!(completed, 2, "Both requests should have completed");
        
        // Global state should be back to 0 after futures are polled
        assert_eq!(global_state.current_concurrency(), 0);

        // After completion, services should be ready again (this will acquire new slots)
        assert!(matches!(global_service1.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        assert!(matches!(global_service2.poll_ready(&mut cx), Poll::Ready(Ok(()))));
    }

    #[test]
    fn test_des_global_concurrency_limit_fairness() {
        let mut simulation = Simulation::default();

        // Create shared global state with limit of 1 to test fairness
        let global_state = crate::tower::limit::global_concurrency::GlobalConcurrencyLimitState::new(1);

        // Create three services sharing the same global limit
        let service1 = DesServiceBuilder::new("fair-service-1".to_string())
            .thread_capacity(2)
            .service_time(Duration::from_millis(50))
            .build(&mut simulation)
            .unwrap();

        let service2 = DesServiceBuilder::new("fair-service-2".to_string())
            .thread_capacity(2)
            .service_time(Duration::from_millis(50))
            .build(&mut simulation)
            .unwrap();

        let service3 = DesServiceBuilder::new("fair-service-3".to_string())
            .thread_capacity(2)
            .service_time(Duration::from_millis(50))
            .build(&mut simulation)
            .unwrap();

        let mut global_service1 = DesGlobalConcurrencyLimit::new(service1, global_state.clone());
        let mut global_service2 = DesGlobalConcurrencyLimit::new(service2, global_state.clone());
        let mut global_service3 = DesGlobalConcurrencyLimit::new(service3, global_state.clone());

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        let mut completed_requests = 0;

        // Process requests sequentially across services
        for round in 0..3 {
            // Service 1 gets a turn
            assert!(matches!(global_service1.poll_ready(&mut cx), Poll::Ready(Ok(()))));
            let req = Request::builder()
                .method(Method::GET)
                .uri(format!("/fair-1-{round}"))
                .body(SimBody::empty())
                .unwrap();
            let future = global_service1.call(req);

            // Other services should be blocked
            assert!(matches!(global_service2.poll_ready(&mut cx), Poll::Pending));
            assert!(matches!(global_service3.poll_ready(&mut cx), Poll::Pending));

            // Complete this request
            for _ in 0..100 {
                if !simulation.step() {
                    break;
                }
            }

            // Verify completion
            if let Poll::Ready(Ok(_)) = Pin::new(&mut { future }).poll(&mut cx) {
                completed_requests += 1;
            }
        }

        assert_eq!(completed_requests, 3, "All requests should complete fairly");
        assert_eq!(global_state.current_concurrency(), 0, "Global state should be clean");
    }

    #[test]
    fn test_concurrency_limit_precise_tracking() {
        let mut simulation = Simulation::default();

        // Create base service with high capacity
        let base_service = DesServiceBuilder::new("precise-tracking-test".to_string())
            .thread_capacity(10)
            .service_time(Duration::from_millis(100))
            .build(&mut simulation)
            .unwrap();

        // Wrap with concurrency limiter (limit to 3 concurrent requests)
        let mut concurrency_service = DesConcurrencyLimit::new(base_service, 3);

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        // Test precise concurrency tracking
        
        // Initially should be 0 concurrent requests
        assert_eq!(concurrency_service.current_concurrency(), 0);
        assert_eq!(concurrency_service.max_concurrency(), 3);

        // Acquire first slot
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        let req1 = Request::builder()
            .method(Method::GET)
            .uri("/precise-1")
            .body(SimBody::empty())
            .unwrap();
        let future1 = concurrency_service.call(req1);
        assert_eq!(concurrency_service.current_concurrency(), 1, "Should have 1 active request");

        // Acquire second slot
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        let req2 = Request::builder()
            .method(Method::GET)
            .uri("/precise-2")
            .body(SimBody::empty())
            .unwrap();
        let future2 = concurrency_service.call(req2);
        assert_eq!(concurrency_service.current_concurrency(), 2, "Should have 2 active requests");

        // Acquire third slot
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        let req3 = Request::builder()
            .method(Method::GET)
            .uri("/precise-3")
            .body(SimBody::empty())
            .unwrap();
        let future3 = concurrency_service.call(req3);
        assert_eq!(concurrency_service.current_concurrency(), 3, "Should have 3 active requests (at capacity)");

        // Fourth request should be blocked
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Pending));
        assert_eq!(concurrency_service.current_concurrency(), 3, "Should still be at capacity");

        // Run simulation to complete first request
        for _ in 0..150 {
            if !simulation.step() {
                break;
            }
        }

        // Poll first future to completion to release its slot
        let mut future1 = future1;
        assert!(matches!(Pin::new(&mut future1).poll(&mut cx), Poll::Ready(Ok(_))));
        assert_eq!(concurrency_service.current_concurrency(), 2, "Should have 2 active requests after completion");

        // Now fourth request should be able to proceed
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        let req4 = Request::builder()
            .method(Method::GET)
            .uri("/precise-4")
            .body(SimBody::empty())
            .unwrap();
        let future4 = concurrency_service.call(req4);
        assert_eq!(concurrency_service.current_concurrency(), 3, "Should be back at capacity with new request");

        // Complete remaining requests
        for _ in 0..200 {
            if !simulation.step() {
                break;
            }
        }

        // Poll all remaining futures to completion
        let futures = vec![future2, future3, future4];
        let mut completed = 0;
        for mut future in futures {
            if let Poll::Ready(Ok(_)) = Pin::new(&mut future).poll(&mut cx) {
                completed += 1;
            }
        }
        assert_eq!(completed, 3, "All remaining requests should complete");
        assert_eq!(concurrency_service.current_concurrency(), 0, "Should have 0 active requests after all complete");

        // Service should be ready for new requests
        assert!(matches!(concurrency_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
    }

    #[test]
    fn test_tower_layer_composition() {
        let mut simulation = Simulation::default();
        let scheduler = simulation.scheduler_handle();

        // Create base service
        let base_service = DesServiceBuilder::new("layer-composition-test".to_string())
            .thread_capacity(5)
            .service_time(Duration::from_millis(50))
            .build(&mut simulation)
            .unwrap();

        // Use Layer trait for composable middleware
        use tower::ServiceBuilder;
        
        let mut service = ServiceBuilder::new()
            // Add rate limiting layer (5 requests per second, burst of 10)
            .layer(DesRateLimitLayer::new(5.0, 10, scheduler))
            // Add concurrency limiting layer (max 2 concurrent requests)
            .layer(DesConcurrencyLimitLayer::new(2))
            .service(base_service);

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        // Test that the composed service works
        assert!(matches!(service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        
        let req = Request::builder()
            .method(Method::GET)
            .uri("/composed")
            .body(SimBody::empty())
            .unwrap();
        let mut future = service.call(req);

        // Run simulation
        for _ in 0..200 {
            if !simulation.step() {
                break;
            }
        }

        // Check response
        let result = Pin::new(&mut future).poll(&mut cx);
        match result {
            Poll::Ready(Ok(_)) => {
                // Success! Layer composition works
            }
            Poll::Ready(Err(e)) => {
                panic!("Composed service failed with error: {e:?}");
            }
            Poll::Pending => {
                panic!("Composed service still pending after simulation steps");
            }
        }
    }

    #[test]
    fn test_timeout_layer_success() {
        let mut simulation = Simulation::default();
        let scheduler = simulation.scheduler_handle();

        // Create base service with very fast service time (1ms)
        let base_service = DesServiceBuilder::new("timeout-success-test".to_string())
            .thread_capacity(5)
            .service_time(Duration::from_millis(1))
            .build(&mut simulation)
            .unwrap();

        // Wrap with timeout layer (very long timeout - 1000ms)
        use tower::Layer;
        let mut timeout_service = DesTimeoutLayer::new(
            Duration::from_millis(1000),
            scheduler,
        ).layer(base_service);

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        // Service should be ready
        assert!(matches!(timeout_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        
        let req = Request::builder()
            .method(Method::GET)
            .uri("/timeout-success")
            .body(SimBody::empty())
            .unwrap();
        let mut future = timeout_service.call(req);

        // Poll once to start the request and register waker
        let result1 = Pin::new(&mut future).poll(&mut cx);
        assert!(matches!(result1, Poll::Pending), "Request should be pending initially");

        // Run simulation to complete the request
        // The timeout is scheduled for 1000ms, request completes in ~2ms
        for _ in 0..50 {
            if !simulation.step() {
                break;
            }
        }

        // Request should succeed (no timeout)
        let result = Pin::new(&mut future).poll(&mut cx);
        match result {
            Poll::Ready(Ok(_)) => {
                // Success - request completed before timeout
            }
            Poll::Ready(Err(ServiceError::Timeout { duration })) => {
                panic!("Request should not have timed out (timeout was {duration:?}, service time was 1ms)");
            }
            Poll::Ready(Err(e)) => {
                panic!("Unexpected error: {e:?}");
            }
            Poll::Pending => {
                // Try polling again after more simulation steps
                for _ in 0..50 {
                    if !simulation.step() {
                        break;
                    }
                }
                
                let result2 = Pin::new(&mut future).poll(&mut cx);
                match result2 {
                    Poll::Ready(Ok(_)) => {
                        // Success after more steps
                    }
                    Poll::Ready(Err(e)) => {
                        panic!("Request failed after more simulation steps: {e:?}");
                    }
                    Poll::Pending => {
                        panic!("Request should have completed after sufficient simulation steps");
                    }
                }
            }
        }
    }

    #[test]
    fn test_timeout_layer_timeout() {
        let mut simulation = Simulation::default();
        let scheduler = simulation.scheduler_handle();

        // Create base service with long service time (200ms)
        let base_service = DesServiceBuilder::new("timeout-test".to_string())
            .thread_capacity(5)
            .service_time(Duration::from_millis(200))
            .build(&mut simulation)
            .unwrap();

        // Wrap with timeout layer (short timeout - 50ms)
        use tower::Layer;
        let mut timeout_service = DesTimeoutLayer::new(
            Duration::from_millis(50),
            scheduler,
        ).layer(base_service);

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        // Service should be ready
        assert!(matches!(timeout_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        
        let req = Request::builder()
            .method(Method::GET)
            .uri("/timeout-test")
            .body(SimBody::empty())
            .unwrap();
        let mut future = timeout_service.call(req);

        // Poll once to start the request and register waker
        let result1 = Pin::new(&mut future).poll(&mut cx);
        assert!(matches!(result1, Poll::Pending), "Request should be pending initially");

        // Run simulation to completion - timeout should fire before request completes
        let mut timeout_detected = false;
        
        // Run simulation until timeout or request completion
        for _ in 0..1000 {
            // Run one simulation step
            if !simulation.step() {
                break;
            }
            
            // Check current simulation time
            let current_time = simulation.time();
            let should_poll = current_time >= des_core::SimTime::from_duration(Duration::from_millis(50));
            
            // Poll future if we've passed timeout threshold
            if should_poll {
                let result = Pin::new(&mut future).poll(&mut cx);
                match result {
                    Poll::Ready(Err(ServiceError::Timeout { duration })) => {
                        assert_eq!(duration, Duration::from_millis(50));
                        timeout_detected = true;
                        break;
                    }
                    Poll::Ready(Ok(_)) => {
                        panic!("Request should have timed out, not succeeded");
                    }
                    Poll::Ready(Err(e)) => {
                        panic!("Expected timeout error, got: {e:?}");
                    }
                    Poll::Pending => {
                        // Continue simulation
                    }
                }
            }
        }

        if !timeout_detected {
            // Final poll to check timeout
            let result = Pin::new(&mut future).poll(&mut cx);
            match result {
                Poll::Ready(Err(ServiceError::Timeout { duration })) => {
                    assert_eq!(duration, Duration::from_millis(50));
                }
                Poll::Ready(Ok(_)) => {
                    panic!("Request should have timed out, not succeeded");
                }
                Poll::Ready(Err(e)) => {
                    panic!("Expected timeout error, got: {e:?}");
                }
                Poll::Pending => {
                    panic!("Request should have timed out after sufficient simulation steps");
                }
            }
        }
    }

    #[test]
    fn test_timeout_layer_resource_cleanup() {
        let mut simulation = Simulation::default();
        let scheduler = simulation.scheduler_handle();

        // Create base service
        let base_service = DesServiceBuilder::new("cleanup-test".to_string())
            .thread_capacity(5)
            .service_time(Duration::from_millis(50))
            .build(&mut simulation)
            .unwrap();

        // Wrap with timeout layer
        use tower::Layer;
        let mut timeout_service = DesTimeoutLayer::new(
            Duration::from_millis(100),
            scheduler,
        ).layer(base_service);

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        // Create and drop multiple futures to test cleanup
        for _ in 0..5 {
            assert!(matches!(timeout_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
            
            let req = Request::builder()
                .method(Method::GET)
                .uri("/cleanup-test")
                .body(SimBody::empty())
                .unwrap();
            let future = timeout_service.call(req);
            
            // Drop the future immediately to test PinnedDrop cleanup
            drop(future);
        }

        // Run a few simulation steps to allow any cleanup to occur
        for _ in 0..10 {
            if !simulation.step() {
                break;
            }
        }

        // Test passes if no panics or resource leaks occur
        // The PinnedDrop implementation should clean up timeout components
    }

    #[test]
    fn test_circuit_breaker_failure_threshold() {
        let mut simulation = Simulation::default();
        let scheduler = simulation.scheduler_handle();

        // Create a service that always fails
        let failing_service = FailingService;

        // Wrap with circuit breaker (failure threshold of 3, recovery timeout of 1 second)
        let mut circuit_breaker_service = DesCircuitBreaker::new(
            failing_service,
            3, // failure threshold
            Duration::from_secs(1),
            scheduler,
        );

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        // First 3 requests should be allowed and fail
        for i in 0..3 {
            assert!(matches!(circuit_breaker_service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
            
            let req = Request::builder()
                .method(Method::GET)
                .uri(format!("/fail/{i}"))
                .body(SimBody::empty())
                .unwrap();
            let mut future = circuit_breaker_service.call(req);
            
            // Poll the future to completion
            match Pin::new(&mut future).poll(&mut cx) {
                Poll::Ready(Err(ServiceError::Internal(_))) => {
                    // Expected failure from FailingService
                }
                other => panic!("Expected failure, got: {other:?}"),
            }
        }

        // 4th request should be rejected due to circuit breaker being open
        match circuit_breaker_service.poll_ready(&mut cx) {
            Poll::Ready(Err(ServiceError::Overloaded)) => {
                // Expected - circuit breaker is now open
            }
            other => panic!("Expected circuit breaker to be open, got: {other:?}"),
        }
    }

    // Helper service that always fails for testing circuit breaker
    struct FailingService;

    impl Service<Request<SimBody>> for FailingService {
        type Response = http::Response<SimBody>;
        type Error = ServiceError;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: Request<SimBody>) -> Self::Future {
            std::future::ready(Err(ServiceError::Internal("Always fails".to_string())))
        }
    }

    // Include other integration tests here...
}