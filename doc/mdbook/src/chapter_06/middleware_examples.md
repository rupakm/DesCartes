# Middleware Examples

This section provides comprehensive examples of Tower middleware available in DESCARTES, showing how to configure and use timeout, retry, rate limiting, circuit breaker, and load balancing middleware.

## Timeout Middleware

The `DesTimeoutLayer` provides request timeouts using simulation time.

### Basic Timeout Configuration

```rust
use des_components::tower::{DesServiceBuilder, DesTimeoutLayer};
use tower::Layer;
use std::time::Duration;

let mut simulation = Simulation::default();
let scheduler = simulation.scheduler_handle();

let base_service = DesServiceBuilder::new("api-server".to_string())
    .thread_capacity(5)
    .service_time(Duration::from_millis(200))  // Slow service
    .build(&mut simulation)?;

// Add 1-second timeout
let timeout_service = DesTimeoutLayer::new(Duration::from_secs(1), scheduler)
    .layer(base_service);
```

### Timeout Behavior

```rust
use http::{Request, Method};
use des_components::tower::{SimBody, ServiceError};

let request = Request::builder()
    .method(Method::GET)
    .uri("/slow-endpoint")
    .body(SimBody::empty())?;

let mut response_future = timeout_service.call(request);

// Run simulation
for _ in 0..2000 {  // Run long enough for timeout to fire
    if !simulation.step() {
        break;
    }
}

match response_future.await {
    Ok(response) => println!("Request completed: {}", response.status()),
    Err(ServiceError::Timeout { duration }) => {
        println!("Request timed out after {:?}", duration);
    }
    Err(e) => println!("Other error: {}", e),
}
```

### Timeout with ServiceBuilder

```rust
use tower::ServiceBuilder;

let service = ServiceBuilder::new()
    .layer(DesTimeoutLayer::new(Duration::from_secs(5), scheduler))
    .service(base_service);
```

## Retry Middleware

The `DesRetryLayer` provides configurable retry logic with exponential backoff.

### Basic Retry Configuration

```rust
use des_components::tower::{DesRetryLayer, DesRetryPolicy};

let retry_policy = DesRetryPolicy::new(3);  // Retry up to 3 times
let retry_service = DesRetryLayer::new(retry_policy, scheduler.clone())
    .layer(base_service);
```

### Exponential Backoff

```rust
use des_components::tower::exponential_backoff_layer;

// Convenience function for exponential backoff
let retry_service = exponential_backoff_layer(3, scheduler.clone())
    .layer(base_service);
```

### Retry Policy Behavior

The retry policy automatically retries on these errors:
- `ServiceError::Timeout` - Request timeouts
- `ServiceError::Overloaded` - Service overload
- `ServiceError::Internal` - Internal errors
- `ServiceError::NotReady` - Service not ready

Non-retryable errors (like `Cancelled` or `Http`) are returned immediately.

### Custom Retry Logic

```rust
use tower::retry::Policy;
use http::{Request, Response};

// Create a custom retry policy
struct CustomRetryPolicy {
    max_retries: usize,
    attempts: usize,
}

impl Policy<Request<SimBody>, Response<SimBody>, ServiceError> for CustomRetryPolicy {
    type Future = std::future::Ready<()>;

    fn retry(
        &mut self,
        _req: &mut Request<SimBody>,
        result: &mut Result<Response<SimBody>, ServiceError>,
    ) -> Option<Self::Future> {
        if self.attempts >= self.max_retries {
            return None;
        }

        let should_retry = match result {
            Err(ServiceError::Timeout { .. }) => true,
            Err(ServiceError::Overloaded) => true,
            _ => false,
        };

        if should_retry {
            self.attempts += 1;
            Some(std::future::ready(()))
        } else {
            None
        }
    }

    fn clone_request(&mut self, req: &Request<SimBody>) -> Option<Request<SimBody>> {
        Some(req.clone())
    }
}
```

## Rate Limiting Middleware

The `DesRateLimitLayer` implements token bucket rate limiting using simulation time.

### Basic Rate Limiting

```rust
use des_components::tower::DesRateLimitLayer;

// Allow 10 requests per second with burst capacity of 20
let rate_limit_service = DesRateLimitLayer::new(
    10.0,  // 10 requests per second
    20,    // burst capacity
    scheduler.clone()
).layer(base_service);
```

### Rate Limiting Behavior

```rust
// Send burst of requests to test rate limiting
let mut futures = Vec::new();

for i in 0..25 {  // Send more than burst capacity
    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/request/{}", i))
        .body(SimBody::empty())?;
    
    futures.push(rate_limit_service.call(request));
}

// Run simulation
for _ in 0..1000 {
    if !simulation.step() {
        break;
    }
}

// Check results
let mut successful = 0;
let mut rate_limited = 0;

for future in futures {
    match future.await {
        Ok(_) => successful += 1,
        Err(ServiceError::Overloaded) => rate_limited += 1,
        Err(e) => println!("Unexpected error: {}", e),
    }
}

println!("Successful: {}, Rate limited: {}", successful, rate_limited);
```

### Dynamic Rate Limiting

```rust
// Different rate limits for different service tiers
let premium_service = DesRateLimitLayer::new(100.0, 200, scheduler.clone())
    .layer(base_service.clone());

let basic_service = DesRateLimitLayer::new(10.0, 20, scheduler.clone())
    .layer(base_service);
```

## Circuit Breaker Middleware

The `DesCircuitBreakerLayer` prevents cascading failures by temporarily blocking requests to failing services.

### Basic Circuit Breaker

```rust
use des_components::tower::DesCircuitBreakerLayer;

// Open circuit after 3 failures, recover after 30 seconds
let circuit_breaker_service = DesCircuitBreakerLayer::new(
    3,                              // failure threshold
    Duration::from_secs(30),        // recovery timeout
    scheduler.clone()
).layer(base_service);
```

### Circuit Breaker States

The circuit breaker has three states:

1. **Closed**: Normal operation, tracking failures
2. **Open**: Blocking all requests, waiting for recovery
3. **Half-Open**: Allowing probe requests to test recovery

### Testing Circuit Breaker Behavior

```rust
// Create a service that fails frequently
let failing_service = DesServiceBuilder::new("failing-service".to_string())
    .thread_capacity(1)
    .service_time(Duration::from_millis(100))
    .build(&mut simulation)?;

let protected_service = DesCircuitBreakerLayer::new(3, Duration::from_secs(10), scheduler)
    .layer(failing_service);

// Send requests that will trigger failures
for i in 0..5 {
    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("/failing-endpoint/{}", i))
        .body(SimBody::empty())?;

    match protected_service.call(request).await {
        Ok(_) => println!("Request {} succeeded", i),
        Err(ServiceError::Overloaded) => println!("Request {} blocked by circuit breaker", i),
        Err(e) => println!("Request {} failed: {}", i, e),
    }
}
```

## Concurrency Limiting

Control the number of concurrent requests to a service.

### Per-Service Concurrency Limits

```rust
use des_components::tower::DesConcurrencyLimitLayer;

// Limit to 3 concurrent requests
let concurrency_limited_service = DesConcurrencyLimitLayer::new(3)
    .layer(base_service);
```

### Global Concurrency Limits

Share concurrency limits across multiple services:

```rust
use des_components::tower::limit::global_concurrency::GlobalConcurrencyLimitState;
use des_components::tower::DesGlobalConcurrencyLimitLayer;

// Create shared global limit
let global_state = GlobalConcurrencyLimitState::new(10);

// Apply to multiple services
let service1 = DesGlobalConcurrencyLimitLayer::new(global_state.clone())
    .layer(base_service1);

let service2 = DesGlobalConcurrencyLimitLayer::new(global_state.clone())
    .layer(base_service2);
```

## Load Balancing

Distribute requests across multiple backend services.

### Round-Robin Load Balancing

```rust
use des_components::tower::{DesLoadBalancer, DesLoadBalanceStrategy};

// Create multiple backend services
let backends = (0..3).map(|i| {
    DesServiceBuilder::new(format!("backend-{}", i))
        .thread_capacity(5)
        .service_time(Duration::from_millis(100))
        .build(&mut simulation)
}).collect::<Result<Vec<_>, _>>()?;

// Create round-robin load balancer
let load_balancer = DesLoadBalancer::round_robin(backends);
```

### Random Load Balancing

```rust
// Random load balancing with deterministic seed
let load_balancer = DesLoadBalancer::random_with_seed(backends, 12345);
```

### Load Balancer Strategies

```rust
// Explicit strategy configuration
let load_balancer = DesLoadBalancer::new(
    backends,
    DesLoadBalanceStrategy::RoundRobin
);
```

## Middleware Composition

Combine multiple middleware layers for comprehensive service protection.

### Complete Middleware Stack

```rust
use tower::ServiceBuilder;

let protected_service = ServiceBuilder::new()
    // Outermost: Timeout (5 seconds)
    .layer(DesTimeoutLayer::new(Duration::from_secs(5), scheduler.clone()))
    
    // Rate limiting (100 req/sec, burst 200)
    .layer(DesRateLimitLayer::new(100.0, 200, scheduler.clone()))
    
    // Circuit breaker (5 failures, 30s recovery)
    .layer(DesCircuitBreakerLayer::new(5, Duration::from_secs(30), scheduler.clone()))
    
    // Retry (3 attempts with exponential backoff)
    .layer(exponential_backoff_layer(3, scheduler.clone()))
    
    // Concurrency limiting (10 concurrent requests)
    .layer(DesConcurrencyLimitLayer::new(10))
    
    // Innermost: Base service
    .service(base_service);
```

### Layer Ordering Considerations

The order of layers matters:

1. **Timeout** (outermost) - Applies to entire request including retries
2. **Rate Limiting** - Prevents excessive load
3. **Circuit Breaker** - Fails fast when service is down
4. **Retry** - Handles transient failures
5. **Concurrency Limiting** - Controls resource usage
6. **Base Service** (innermost) - Actual service implementation

### Reusable Middleware Stacks

```rust
// Create reusable middleware configuration
fn create_resilient_service_stack(scheduler: SchedulerHandle) -> impl Layer<DesService> {
    ServiceBuilder::new()
        .layer(DesTimeoutLayer::new(Duration::from_secs(30), scheduler.clone()))
        .layer(DesCircuitBreakerLayer::new(3, Duration::from_secs(60), scheduler.clone()))
        .layer(exponential_backoff_layer(3, scheduler))
        .layer(DesConcurrencyLimitLayer::new(5))
}

// Apply to multiple services
let api_service = create_resilient_service_stack(scheduler.clone())
    .layer(api_base_service);

let db_service = create_resilient_service_stack(scheduler.clone())
    .layer(db_base_service);
```

## Performance Testing Scenarios

Use middleware to simulate various performance conditions.

### High Load Testing

```rust
// Simulate high load with rate limiting
let high_load_service = ServiceBuilder::new()
    .layer(DesRateLimitLayer::new(1000.0, 2000, scheduler.clone()))  // High rate limit
    .layer(DesConcurrencyLimitLayer::new(2))  // Low concurrency creates bottleneck
    .service(base_service);
```

### Failure Testing

```rust
// Test resilience with aggressive circuit breaker
let failure_test_service = ServiceBuilder::new()
    .layer(DesCircuitBreakerLayer::new(1, Duration::from_millis(100), scheduler))  // Sensitive breaker
    .service(unreliable_service);
```

### Latency Testing

```rust
// Test timeout behavior with slow service
let latency_test_service = ServiceBuilder::new()
    .layer(DesTimeoutLayer::new(Duration::from_millis(50), scheduler))  // Short timeout
    .service(slow_service);  // Service with 200ms response time
```

## Monitoring and Metrics

Middleware integrates with the DESCARTES metrics system:

```rust
// Middleware automatically tracks:
// - Request counts and rates
// - Error rates by type
// - Latency distributions
// - Circuit breaker state changes
// - Rate limiting events

// Access metrics after simulation
let metrics = simulation.metrics();
println!("Total requests: {}", metrics.request_count());
println!("Error rate: {:.2}%", metrics.error_rate() * 100.0);
```

## Next Steps

Continue to [Multi-Tier Architectures](./multi_tier_architectures.md) to learn how to build complex service topologies using these middleware components.