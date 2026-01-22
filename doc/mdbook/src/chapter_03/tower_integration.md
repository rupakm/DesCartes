# Tower Integration

DES-Components provides full integration with the Tower service abstraction, allowing you to use Tower middleware and services within discrete event simulations. This enables realistic modeling of HTTP services with middleware layers like timeouts, retries, rate limiting, and circuit breakers.

## Tower Service Abstraction

Tower defines services as asynchronous functions that transform requests into responses:

```rust
use tower::Service;
use http::{Request, Response};

// Tower Service trait (simplified)
pub trait Service<Request> {
    type Response;
    type Error;
    type Future: Future<Output = Result<Self::Response, Self::Error>>;
    
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>>;
    fn call(&mut self, req: Request) -> Self::Future;
}
```

## DesServiceBuilder

The `DesServiceBuilder` provides a drop-in replacement for Tower's `ServiceBuilder` with simulation-aware middleware:

### Basic Service Creation

```rust
use des_components::tower::{DesServiceBuilder, SimBody};
use des_core::Simulation;
use std::time::Duration;

fn create_basic_service() -> Result<(), Box<dyn std::error::Error>> {
    let mut simulation = Simulation::default();
    
    let service = DesServiceBuilder::new("api-server".to_string())
        .thread_capacity(4)
        .service_time(Duration::from_millis(100))
        .build(&mut simulation)?;
    
    // Service is now ready to handle HTTP requests
    Ok(())
}
```

### Service with Middleware Layers

```rust
use des_components::tower::{DesServiceBuilder, SimBody};
use des_components::ExponentialBackoffPolicy;
use des_core::Simulation;
use std::time::Duration;

fn create_layered_service() -> Result<(), Box<dyn std::error::Error>> {
    let mut simulation = Simulation::default();
    let scheduler = simulation.scheduler_handle();
    
    let service = DesServiceBuilder::new("production-api".to_string())
        .thread_capacity(8)
        .service_time(Duration::from_millis(50))
        // Add concurrency limiting
        .concurrency_limit(5)
        // Add rate limiting (100 requests per second, burst of 200)
        .rate_limit(100, Duration::from_secs(1), scheduler.clone())
        // Add timeout (30 seconds)
        .timeout(Duration::from_secs(30), scheduler.clone())
        // Add circuit breaker (5 failures, 60 second recovery)
        .circuit_breaker(5, Duration::from_secs(60), scheduler.clone())
        // Add retry with exponential backoff
        .retry(
            ExponentialBackoffPolicy::new(3, Duration::from_millis(100)),
            scheduler.clone(),
        )
        .build(&mut simulation)?;
    
    Ok(())
}
```

## Available Middleware

### Concurrency Limiting

Limit the number of concurrent requests:

```rust
use des_components::tower::DesConcurrencyLimitLayer;

let service = DesServiceBuilder::new("limited-service".to_string())
    .thread_capacity(10)
    .service_time(Duration::from_millis(100))
    .concurrency_limit(3) // Max 3 concurrent requests
    .build(&mut simulation)?;
```

### Rate Limiting

Control request rate with token bucket algorithm:

```rust
let service = DesServiceBuilder::new("rate-limited-service".to_string())
    .thread_capacity(5)
    .service_time(Duration::from_millis(80))
    .rate_limit(
        50, // 50 requests per second
        Duration::from_secs(1), // per second
        scheduler.clone(),
    )
    .build(&mut simulation)?;
```

### Timeout Middleware

Add request timeouts:

```rust
let service = DesServiceBuilder::new("timeout-service".to_string())
    .thread_capacity(4)
    .service_time(Duration::from_millis(200))
    .timeout(Duration::from_secs(5), scheduler.clone())
    .build(&mut simulation)?;
```

### Circuit Breaker

Implement circuit breaker pattern:

```rust
let service = DesServiceBuilder::new("resilient-service".to_string())
    .thread_capacity(6)
    .service_time(Duration::from_millis(100))
    .circuit_breaker(
        3, // failure threshold
        Duration::from_secs(30), // recovery timeout
        scheduler.clone(),
    )
    .build(&mut simulation)?;
```

### Retry Middleware

Add retry logic with various policies:

```rust
use des_components::ExponentialBackoffPolicy;

let retry_policy = ExponentialBackoffPolicy::new(
    3, // max retries
    Duration::from_millis(100), // base delay
);

let service = DesServiceBuilder::new("retry-service".to_string())
    .thread_capacity(4)
    .service_time(Duration::from_millis(150))
    .retry(retry_policy, scheduler.clone())
    .build(&mut simulation)?;
```

### Global Concurrency Limiting

Share concurrency limits across multiple services:

```rust
use des_components::tower::limit::global_concurrency::GlobalConcurrencyLimitState;

let global_state = GlobalConcurrencyLimitState::new(10); // 10 total across all services

let service1 = DesServiceBuilder::new("service-1".to_string())
    .thread_capacity(8)
    .service_time(Duration::from_millis(100))
    .global_concurrency_limit(global_state.clone())
    .build(&mut simulation)?;

let service2 = DesServiceBuilder::new("service-2".to_string())
    .thread_capacity(8)
    .service_time(Duration::from_millis(120))
    .global_concurrency_limit(global_state.clone())
    .build(&mut simulation)?;
```

### Hedging

Send duplicate requests to improve latency:

```rust
let service = DesServiceBuilder::new("hedge-service".to_string())
    .thread_capacity(6)
    .service_time(Duration::from_millis(200))
    .hedge(Duration::from_millis(100), scheduler.clone()) // Hedge after 100ms
    .build(&mut simulation)?;
```

## Complete Tower Integration Example

Here's a comprehensive example showing Tower service usage in a simulation:

```rust
use des_components::tower::{DesServiceBuilder, SimBody};
use des_components::ExponentialBackoffPolicy;
use des_core::{Simulation, Execute, Executor, SimTime};
use http::{Method, Request, Response, StatusCode};
use std::time::Duration;
use tower::Service;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Tower Integration Example ===\n");
    
    let mut simulation = Simulation::default();
    let scheduler = simulation.scheduler_handle();
    
    // Create a production-ready service with multiple middleware layers
    let mut service = DesServiceBuilder::new("production-api".to_string())
        .thread_capacity(8)
        .service_time(Duration::from_millis(100))
        // Layer 1: Global concurrency limit
        .concurrency_limit(5)
        // Layer 2: Rate limiting
        .rate_limit(50, Duration::from_secs(1), scheduler.clone())
        // Layer 3: Timeout
        .timeout(Duration::from_secs(10), scheduler.clone())
        // Layer 4: Circuit breaker
        .circuit_breaker(3, Duration::from_secs(30), scheduler.clone())
        // Layer 5: Retry
        .retry(
            ExponentialBackoffPolicy::new(2, Duration::from_millis(100)),
            scheduler.clone(),
        )
        .build(&mut simulation)?;
    
    println!("Created service with middleware stack:");
    println!("  - Concurrency limit: 5");
    println!("  - Rate limit: 50 req/s");
    println!("  - Timeout: 10s");
    println!("  - Circuit breaker: 3 failures, 30s recovery");
    println!("  - Retry: 2 attempts, exponential backoff\n");
    
    // Test service readiness
    let waker = create_noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    
    match service.poll_ready(&mut cx) {
        std::task::Poll::Ready(Ok(())) => {
            println!("✓ Service is ready to accept requests");
        }
        std::task::Poll::Ready(Err(e)) => {
            println!("✗ Service error: {:?}", e);
            return Err(e.into());
        }
        std::task::Poll::Pending => {
            println!("⏳ Service is not ready (normal for capacity-limited services)");
        }
    }
    
    // Create test requests
    let requests = vec![
        create_request(Method::GET, "/api/users", ""),
        create_request(Method::POST, "/api/orders", r#"{"item": "laptop", "qty": 1}"#),
        create_request(Method::GET, "/api/health", ""),
        create_request(Method::PUT, "/api/users/123", r#"{"name": "John Doe"}"#),
        create_request(Method::DELETE, "/api/orders/456", ""),
    ];
    
    println!("\nSending {} test requests...", requests.len());
    
    // Send requests through the service
    let mut response_futures = Vec::new();
    for (i, request) in requests.into_iter().enumerate() {
        println!("  → Request {}: {} {}", 
            i + 1, 
            request.method(), 
            request.uri()
        );
        
        // Check if service is ready for this request
        match service.poll_ready(&mut cx) {
            std::task::Poll::Ready(Ok(())) => {
                let response_future = service.call(request);
                response_futures.push(response_future);
            }
            std::task::Poll::Ready(Err(e)) => {
                println!("    ✗ Service error: {:?}", e);
            }
            std::task::Poll::Pending => {
                println!("    ⏳ Service not ready, request queued");
            }
        }
    }
    
    println!("\n✓ All requests submitted to service");
    
    // Run simulation to process requests
    println!("\nRunning simulation...");
    Executor::timed(SimTime::from_duration(Duration::from_secs(30)))
        .execute(&mut simulation);
    
    println!("✓ Simulation completed");
    
    // In a real scenario, you would poll the response futures to get results
    // For this example, we just demonstrate the service creation and request submission
    
    println!("\n=== Tower Integration Complete ===");
    Ok(())
}

fn create_request(method: Method, uri: &str, body: &str) -> Request<SimBody> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(SimBody::from_static(body))
        .unwrap()
}

// Helper function to create a no-op waker for testing
fn create_noop_waker() -> std::task::Waker {
    use std::task::{RawWaker, RawWakerVTable};

    fn noop(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }

    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let raw_waker = RawWaker::new(std::ptr::null(), &VTABLE);
    unsafe { std::task::Waker::from_raw(raw_waker) }
}
```

## SimBody for Request/Response Bodies

DES-Components provides `SimBody` for HTTP request and response bodies in simulations:

```rust
use des_components::tower::SimBody;

// Create bodies from various sources
let empty_body = SimBody::empty();
let static_body = SimBody::from_static("Hello, World!");
let string_body = SimBody::from_string("Dynamic content".to_string());
let bytes_body = SimBody::from_bytes(vec![1, 2, 3, 4]);

// Bodies are lightweight and suitable for simulation use
```

## Custom Middleware

You can create custom middleware for specific simulation needs:

```rust
use tower::{Layer, Service};
use std::task::{Context, Poll};
use std::future::Future;
use std::pin::Pin;

// Custom logging middleware
#[derive(Clone)]
pub struct LoggingLayer;

impl<S> Layer<S> for LoggingLayer {
    type Service = LoggingService<S>;
    
    fn layer(&self, service: S) -> Self::Service {
        LoggingService { inner: service }
    }
}

#[derive(Clone)]
pub struct LoggingService<S> {
    inner: S,
}

impl<S, Request> Service<Request> for LoggingService<S>
where
    S: Service<Request>,
    Request: std::fmt::Debug,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = LoggingFuture<S::Future>;
    
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }
    
    fn call(&mut self, req: Request) -> Self::Future {
        println!("Processing request: {:?}", req);
        LoggingFuture {
            inner: self.inner.call(req),
        }
    }
}

pub struct LoggingFuture<F> {
    inner: F,
}

impl<F> Future for LoggingFuture<F>
where
    F: Future,
{
    type Output = F::Output;
    
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let inner = unsafe { Pin::new_unchecked(&mut this.inner) };
        let result = inner.poll(cx);
        
        if result.is_ready() {
            println!("Request completed");
        }
        
        result
    }
}

// Use custom middleware
let service = DesServiceBuilder::new("logged-service".to_string())
    .thread_capacity(4)
    .service_time(Duration::from_millis(100))
    .layer(LoggingLayer)
    .build(&mut simulation)?;
```

## Load Balancing with Tower

Create load-balanced services using Tower's load balancing utilities:

```rust
use des_components::tower::{DesServiceBuilder, DesLoadBalancer, DesLoadBalanceStrategy};

fn create_load_balanced_services() -> Result<(), Box<dyn std::error::Error>> {
    let mut simulation = Simulation::default();
    
    // Create multiple backend services
    let mut backends = Vec::new();
    for i in 0..3 {
        let service = DesServiceBuilder::new(format!("backend-{}", i))
            .thread_capacity(4)
            .service_time(Duration::from_millis(100))
            .build(&mut simulation)?;
        backends.push(service);
    }
    
    // Create load balancer
    let load_balancer = DesLoadBalancer::new(
        backends,
        DesLoadBalanceStrategy::RoundRobin,
    );
    
    // Wrap with additional middleware
    let service = DesServiceBuilder::new("load-balanced-api".to_string())
        .thread_capacity(12) // Total capacity across backends
        .service_time(Duration::from_millis(100))
        .layer_fn(|_| load_balancer)
        .rate_limit(150, Duration::from_secs(1), simulation.scheduler_handle())
        .build(&mut simulation)?;
    
    Ok(())
}
```

## Best Practices

### Middleware Ordering

Order middleware layers carefully for optimal behavior:

```rust
// Recommended order (outer to inner):
let service = DesServiceBuilder::new("optimized-service".to_string())
    .thread_capacity(8)
    .service_time(Duration::from_millis(100))
    // 1. Rate limiting (reject early)
    .rate_limit(100, Duration::from_secs(1), scheduler.clone())
    // 2. Timeout (prevent hanging requests)
    .timeout(Duration::from_secs(30), scheduler.clone())
    // 3. Circuit breaker (fail fast when service is down)
    .circuit_breaker(5, Duration::from_secs(60), scheduler.clone())
    // 4. Concurrency limiting (control resource usage)
    .concurrency_limit(5)
    // 5. Retry (last resort for failed requests)
    .retry(retry_policy, scheduler.clone())
    .build(&mut simulation)?;
```

### Performance Considerations

- **Concurrency Limits**: Set based on actual resource constraints
- **Rate Limits**: Configure based on expected traffic patterns
- **Timeouts**: Balance between user experience and resource usage
- **Circuit Breakers**: Tune thresholds based on acceptable failure rates

### Monitoring Tower Services

```rust
// Tower services integrate with DES metrics
let service_metrics = simulation.get_metrics();

// Monitor key Tower middleware metrics:
// - Rate limit rejections
// - Timeout occurrences  
// - Circuit breaker state changes
// - Concurrency limit hits
// - Retry attempts and successes
```

### Testing Tower Integration

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_service_with_middleware() {
        let mut simulation = Simulation::default();
        let scheduler = simulation.scheduler_handle();
        
        let service = DesServiceBuilder::new("test-service".to_string())
            .thread_capacity(2)
            .service_time(Duration::from_millis(50))
            .concurrency_limit(1)
            .timeout(Duration::from_millis(100), scheduler)
            .build(&mut simulation)
            .unwrap();
        
        // Test service behavior
        // ...
    }
}
```

Tower integration brings the full power of the Tower ecosystem to discrete event simulations, enabling realistic modeling of HTTP services with production-grade middleware patterns. This allows for accurate performance testing and capacity planning of distributed systems.