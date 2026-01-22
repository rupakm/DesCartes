# Tower Service Abstraction

This section explains how Tower services integrate with DESCARTES discrete event simulations, providing deterministic behavior while maintaining full compatibility with the Tower ecosystem.

## Tower Service Trait Integration

DESCARTES implements the standard Tower `Service` trait but adapts all timing operations to use simulation time instead of wall-clock time. This ensures reproducible, deterministic behavior in your simulations.

### Basic Service Creation

The `DesServiceBuilder` provides a Tower-compatible way to create services:

```rust
use des_components::tower::DesServiceBuilder;
use des_core::Simulation;
use std::time::Duration;

let mut simulation = Simulation::default();

let service = DesServiceBuilder::new("web-server".to_string())
    .thread_capacity(10)           // Maximum concurrent requests
    .service_time(Duration::from_millis(50))  // Processing time per request
    .build(&mut simulation)?;
```

### Service Configuration Options

The `DesServiceBuilder` supports various configuration options:

```rust
let service = DesServiceBuilder::new("api-server".to_string())
    .thread_capacity(5)            // Concurrency limit
    .service_time(Duration::from_millis(100))  // Base service time
    .build(&mut simulation)?;
```

**Configuration Parameters:**

- **`thread_capacity`**: Maximum number of concurrent requests the service can handle
- **`service_time`**: Base processing time for each request
- **Service name**: Used for metrics and debugging

## Simulation Time Integration

All Tower middleware in DESCARTES uses simulation time (`SimTime`) instead of wall-clock time:

### Time-Based Operations

```rust
use des_components::tower::{DesTimeoutLayer, DesRateLimitLayer};
use tower::ServiceBuilder;

let scheduler = simulation.scheduler_handle();

let service = ServiceBuilder::new()
    // Timeout after 5 simulation seconds
    .layer(DesTimeoutLayer::new(Duration::from_secs(5), scheduler.clone()))
    // Rate limit: 10 requests per simulation second
    .layer(DesRateLimitLayer::new(10.0, 20, scheduler))
    .service(base_service);
```

### Event Scheduling

Tower middleware schedules events in the simulation:

- **Timeouts**: Scheduled as `TimeoutTask` events
- **Rate limiting**: Token refill based on simulation time progression
- **Circuit breaker recovery**: Recovery timeouts use simulation events
- **Retry delays**: Backoff delays use simulation time

## HTTP Request/Response Model

DESCARTES uses a simplified HTTP model optimized for simulation:

### Request Structure

```rust
use http::{Request, Method};
use des_components::tower::SimBody;

let request = Request::builder()
    .method(Method::GET)
    .uri("/api/users")
    .body(SimBody::from_static("request payload"))?;
```

### Response Handling

```rust
// Services return standard HTTP responses
let response: http::Response<SimBody> = service.call(request).await?;

println!("Status: {}", response.status());
println!("Body: {:?}", response.body().data());
```

### SimBody Type

The `SimBody` type provides a lightweight HTTP body implementation:

```rust
use des_components::tower::SimBody;

// Create from static string
let body = SimBody::from_static("Hello, world!");

// Create from bytes
let body = SimBody::new(b"binary data".to_vec());

// Empty body
let body = SimBody::empty();
```

## Service Lifecycle

### Service Readiness

Tower services implement readiness checking:

```rust
use tower::Service;
use std::task::{Context, Poll};

// Check if service is ready to accept requests
match service.poll_ready(&mut cx) {
    Poll::Ready(Ok(())) => {
        // Service is ready
        let response_future = service.call(request);
    }
    Poll::Ready(Err(e)) => {
        // Service error
        eprintln!("Service error: {}", e);
    }
    Poll::Pending => {
        // Service is not ready (e.g., at capacity)
    }
}
```

### Request Processing

Services process requests asynchronously:

```rust
use std::pin::Pin;

let mut response_future = service.call(request);

// Run simulation to process the request
for _ in 0..100 {
    if !simulation.step() {
        break;
    }
}

// Check if response is ready
match Pin::new(&mut response_future).poll(&mut cx) {
    Poll::Ready(Ok(response)) => {
        println!("Request completed: {}", response.status());
    }
    Poll::Ready(Err(e)) => {
        println!("Request failed: {}", e);
    }
    Poll::Pending => {
        println!("Request still processing");
    }
}
```

## Error Handling

DESCARTES defines simulation-specific error types:

```rust
use des_components::tower::ServiceError;

match service_result {
    Err(ServiceError::Timeout { duration }) => {
        println!("Request timed out after {:?}", duration);
    }
    Err(ServiceError::Overloaded) => {
        println!("Service is overloaded");
    }
    Err(ServiceError::NotReady) => {
        println!("Service is not ready");
    }
    Err(ServiceError::Internal(msg)) => {
        println!("Internal error: {}", msg);
    }
    Ok(response) => {
        println!("Success: {}", response.status());
    }
}
```

## Performance Characteristics

### Service Time Modeling

Services model realistic processing times:

```rust
// Fast API service
let api_service = DesServiceBuilder::new("api".to_string())
    .service_time(Duration::from_millis(10))
    .build(&mut simulation)?;

// Slow database service
let db_service = DesServiceBuilder::new("database".to_string())
    .service_time(Duration::from_millis(200))
    .build(&mut simulation)?;
```

### Capacity Management

Thread capacity controls concurrency:

```rust
// High-capacity service (can handle many concurrent requests)
let high_capacity = DesServiceBuilder::new("cache".to_string())
    .thread_capacity(100)
    .service_time(Duration::from_millis(1))
    .build(&mut simulation)?;

// Limited capacity service (creates queueing)
let limited_capacity = DesServiceBuilder::new("legacy-db".to_string())
    .thread_capacity(2)
    .service_time(Duration::from_millis(500))
    .build(&mut simulation)?;
```

### Queue Behavior

When services reach capacity, requests queue automatically:

- Requests wait in FIFO order
- Queue delays are included in total response time
- Backpressure propagates to calling services

## Integration with Tower Ecosystem

DESCARTES services work with standard Tower utilities:

### ServiceBuilder Pattern

```rust
use tower::ServiceBuilder;

let service = ServiceBuilder::new()
    .concurrency_limit(10)         // Standard Tower layer
    .layer(DesTimeoutLayer::new(Duration::from_secs(30), scheduler))
    .service(base_service);
```

### Layer Composition

```rust
use tower::Layer;

// Create reusable layer stack
let middleware_stack = ServiceBuilder::new()
    .layer(DesRateLimitLayer::new(100.0, 200, scheduler.clone()))
    .layer(DesCircuitBreakerLayer::new(5, Duration::from_secs(30), scheduler));

// Apply to multiple services
let service1 = middleware_stack.service(base_service1);
let service2 = middleware_stack.service(base_service2);
```

## Best Practices

### Service Naming

Use descriptive names for metrics and debugging:

```rust
let services = vec![
    DesServiceBuilder::new("user-service".to_string()),
    DesServiceBuilder::new("auth-service".to_string()),
    DesServiceBuilder::new("payment-service".to_string()),
];
```

### Capacity Planning

Model realistic service capacities:

```rust
// Web server: high concurrency, low latency
let web_server = DesServiceBuilder::new("web-server".to_string())
    .thread_capacity(50)
    .service_time(Duration::from_millis(5));

// Database: limited concurrency, higher latency
let database = DesServiceBuilder::new("database".to_string())
    .thread_capacity(10)
    .service_time(Duration::from_millis(100));
```

### Error Simulation

Test error conditions by configuring services appropriately:

```rust
// Unreliable service for testing resilience
let unreliable_service = DesServiceBuilder::new("unreliable-api".to_string())
    .thread_capacity(1)           // Low capacity causes overload
    .service_time(Duration::from_millis(1000))  // Slow responses cause timeouts
    .build(&mut simulation)?;
```

## Next Steps

Now that you understand Tower service integration, continue to [Middleware Examples](./middleware_examples.md) to learn about specific middleware implementations like timeouts, retries, and circuit breakers.