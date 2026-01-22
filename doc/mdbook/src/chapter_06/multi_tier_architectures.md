# Multi-Tier Architectures

This section demonstrates how to build complex, multi-tier service architectures using DESCARTES Tower integration. You'll learn to model realistic distributed systems with load balancers, application servers, and databases.

## Architecture Patterns

### Three-Tier Architecture (LB → App → DB)

The classic three-tier pattern with load balancer, application servers, and database:

```rust
use des_components::tower::{DesServiceBuilder, DesLoadBalancer, DesTimeoutLayer, DesRateLimitLayer};
use tower::ServiceBuilder;
use std::time::Duration;

async fn build_three_tier_architecture() -> Result<(), Box<dyn std::error::Error>> {
    let mut simulation = Simulation::default();
    let scheduler = simulation.scheduler_handle();

    // Tier 3: Database Layer (slow, limited capacity)
    let database = DesServiceBuilder::new("postgres-db".to_string())
        .thread_capacity(10)                    // Limited DB connections
        .service_time(Duration::from_millis(50)) // DB query time
        .build(&mut simulation)?;

    // Add database middleware
    let protected_db = ServiceBuilder::new()
        .layer(DesTimeoutLayer::new(Duration::from_secs(5), scheduler.clone()))
        .layer(DesConcurrencyLimitLayer::new(8))  // Connection pool limit
        .service(database);

    // Tier 2: Application Servers (multiple instances)
    let app_servers = (0..3).map(|i| {
        let app_service = DesServiceBuilder::new(format!("app-server-{}", i))
            .thread_capacity(20)                     // Higher app server capacity
            .service_time(Duration::from_millis(10)) // Fast app processing
            .build(&mut simulation)?;

        // Each app server calls the database
        let app_with_db = AppServerWithDatabase::new(app_service, protected_db.clone());
        
        // Add app server middleware
        ServiceBuilder::new()
            .layer(DesTimeoutLayer::new(Duration::from_secs(10), scheduler.clone()))
            .layer(DesRateLimitLayer::new(50.0, 100, scheduler.clone()))
            .service(app_with_db)
    }).collect::<Result<Vec<_>, _>>()?;

    // Tier 1: Load Balancer
    let load_balancer = DesLoadBalancer::round_robin(app_servers);

    // Add frontend middleware
    let frontend = ServiceBuilder::new()
        .layer(DesTimeoutLayer::new(Duration::from_secs(30), scheduler.clone()))
        .layer(DesRateLimitLayer::new(1000.0, 2000, scheduler))
        .service(load_balancer);

    Ok(())
}
```

### Microservices Architecture

Model a microservices architecture with service-to-service communication:

```rust
async fn build_microservices_architecture() -> Result<(), Box<dyn std::error::Error>> {
    let mut simulation = Simulation::default();
    let scheduler = simulation.scheduler_handle();

    // Shared database
    let database = create_database_service(&mut simulation, scheduler.clone())?;

    // User Service
    let user_service = create_microservice(
        &mut simulation,
        "user-service",
        Duration::from_millis(20),
        Some(database.clone())
    )?;

    // Order Service (calls User Service and Database)
    let order_service = create_microservice_with_dependencies(
        &mut simulation,
        "order-service",
        Duration::from_millis(30),
        vec![user_service.clone(), database.clone()]
    )?;

    // Payment Service
    let payment_service = create_microservice(
        &mut simulation,
        "payment-service",
        Duration::from_millis(100), // Slower external payment processing
        None
    )?;

    // API Gateway (orchestrates microservices)
    let api_gateway = ApiGateway::new(vec![
        ("users".to_string(), user_service),
        ("orders".to_string(), order_service),
        ("payments".to_string(), payment_service),
    ]);

    // Frontend load balancer
    let frontend = ServiceBuilder::new()
        .layer(DesTimeoutLayer::new(Duration::from_secs(30), scheduler.clone()))
        .layer(DesRateLimitLayer::new(500.0, 1000, scheduler))
        .service(api_gateway);

    Ok(())
}

fn create_microservice(
    simulation: &mut Simulation,
    name: &str,
    service_time: Duration,
    database: Option<impl Service<Request<SimBody>> + Clone>
) -> Result<impl Service<Request<SimBody>>, Box<dyn std::error::Error>> {
    let base_service = DesServiceBuilder::new(name.to_string())
        .thread_capacity(15)
        .service_time(service_time)
        .build(simulation)?;

    let scheduler = simulation.scheduler_handle();

    let service_with_deps = if let Some(db) = database {
        MicroserviceWithDatabase::new(base_service, db)
    } else {
        MicroserviceStandalone::new(base_service)
    };

    Ok(ServiceBuilder::new()
        .layer(DesTimeoutLayer::new(Duration::from_secs(5), scheduler.clone()))
        .layer(DesCircuitBreakerLayer::new(3, Duration::from_secs(30), scheduler.clone()))
        .layer(exponential_backoff_layer(3, scheduler))
        .service(service_with_deps))
}
```

## Service Composition Patterns

### Service Chaining

Chain services where each service calls the next:

```rust
use tower::Service;
use std::future::Future;
use std::pin::Pin;

// Service that calls another service
struct ChainedService<S1, S2> {
    first: S1,
    second: S2,
}

impl<S1, S2> ChainedService<S1, S2> {
    fn new(first: S1, second: S2) -> Self {
        Self { first, second }
    }
}

impl<S1, S2> Service<Request<SimBody>> for ChainedService<S1, S2>
where
    S1: Service<Request<SimBody>, Response = Response<SimBody>, Error = ServiceError> + Clone,
    S2: Service<Request<SimBody>, Response = Response<SimBody>, Error = ServiceError> + Clone,
    S1::Future: Unpin,
    S2::Future: Unpin,
{
    type Response = Response<SimBody>;
    type Error = ServiceError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Both services must be ready
        match self.first.poll_ready(cx) {
            Poll::Ready(Ok(())) => self.second.poll_ready(cx),
            other => other,
        }
    }

    fn call(&mut self, req: Request<SimBody>) -> Self::Future {
        let mut first = self.first.clone();
        let mut second = self.second.clone();

        Box::pin(async move {
            // Call first service
            let first_response = first.call(req).await?;
            
            // Use first response to create second request
            let second_request = create_second_request(first_response)?;
            
            // Call second service
            second.call(second_request).await
        })
    }
}

fn create_second_request(first_response: Response<SimBody>) -> Result<Request<SimBody>, ServiceError> {
    // Transform first response into second request
    let body = SimBody::new(format!("Processed: {:?}", first_response.body().data()));
    
    Request::builder()
        .method(Method::POST)
        .uri("/second-service")
        .body(body)
        .map_err(|e| ServiceError::Http(e.to_string()))
}
```

### Fan-Out Pattern

Call multiple services concurrently and aggregate results:

```rust
use futures::future::join_all;

struct FanOutService<S> {
    services: Vec<S>,
}

impl<S> FanOutService<S> {
    fn new(services: Vec<S>) -> Self {
        Self { services }
    }
}

impl<S> Service<Request<SimBody>> for FanOutService<S>
where
    S: Service<Request<SimBody>, Response = Response<SimBody>, Error = ServiceError> + Clone,
    S::Future: Unpin,
{
    type Response = Response<SimBody>;
    type Error = ServiceError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Check if any service is ready
        for service in &mut self.services {
            if let Poll::Ready(Ok(())) = service.poll_ready(cx) {
                return Poll::Ready(Ok(()));
            }
        }
        Poll::Pending
    }

    fn call(&mut self, req: Request<SimBody>) -> Self::Future {
        let services = self.services.clone();

        Box::pin(async move {
            // Create requests for each service
            let futures = services.into_iter().map(|mut service| {
                let req_clone = req.clone();
                async move { service.call(req_clone).await }
            }).collect::<Vec<_>>();

            // Wait for all responses
            let responses = join_all(futures).await;

            // Aggregate successful responses
            let successful_responses: Vec<_> = responses
                .into_iter()
                .filter_map(|r| r.ok())
                .collect();

            if successful_responses.is_empty() {
                return Err(ServiceError::Internal("All services failed".to_string()));
            }

            // Combine responses (simplified)
            let combined_body = successful_responses
                .iter()
                .map(|r| std::str::from_utf8(r.body().data()).unwrap_or(""))
                .collect::<Vec<_>>()
                .join(", ");

            Response::builder()
                .status(200)
                .body(SimBody::new(combined_body))
                .map_err(|e| ServiceError::HttpResponseBuilder { message: e.to_string() })
        })
    }
}
```

## Real-World Architecture Examples

### E-Commerce Platform

```rust
async fn build_ecommerce_platform() -> Result<(), Box<dyn std::error::Error>> {
    let mut simulation = Simulation::default();
    let scheduler = simulation.scheduler_handle();

    // Data Layer
    let user_db = create_database("user-db", 50, &mut simulation)?;
    let product_db = create_database("product-db", 100, &mut simulation)?;
    let order_db = create_database("order-db", 30, &mut simulation)?;

    // Cache Layer
    let redis_cache = create_cache_service("redis", &mut simulation)?;

    // Service Layer
    let user_service = create_service_with_cache_and_db(
        "user-service", user_db, redis_cache.clone(), &mut simulation
    )?;

    let product_service = create_service_with_cache_and_db(
        "product-service", product_db, redis_cache.clone(), &mut simulation
    )?;

    let order_service = create_service_with_dependencies(
        "order-service",
        vec![user_service.clone(), product_db.clone()],
        &mut simulation
    )?;

    let payment_service = create_external_service(
        "payment-gateway", Duration::from_millis(200), &mut simulation
    )?;

    // API Gateway with rate limiting per endpoint
    let api_gateway = create_api_gateway(vec![
        ("/users", user_service, 100.0),      // 100 req/sec
        ("/products", product_service, 500.0), // 500 req/sec  
        ("/orders", order_service, 50.0),      // 50 req/sec
        ("/payments", payment_service, 10.0),  // 10 req/sec
    ], scheduler.clone())?;

    // CDN and Load Balancer
    let cdn = create_cdn_service(&mut simulation)?;
    let load_balancer = create_load_balancer(vec![api_gateway], scheduler)?;

    Ok(())
}
```

### Banking System

```rust
async fn build_banking_system() -> Result<(), Box<dyn std::error::Error>> {
    let mut simulation = Simulation::default();
    let scheduler = simulation.scheduler_handle();

    // Core Banking Database (highly protected)
    let core_banking_db = ServiceBuilder::new()
        .layer(DesTimeoutLayer::new(Duration::from_secs(10), scheduler.clone()))
        .layer(DesConcurrencyLimitLayer::new(5))  // Very limited access
        .service(create_database("core-banking", 200, &mut simulation)?);

    // Account Service (high security, low throughput)
    let account_service = ServiceBuilder::new()
        .layer(DesTimeoutLayer::new(Duration::from_secs(5), scheduler.clone()))
        .layer(DesRateLimitLayer::new(10.0, 20, scheduler.clone()))  // Conservative rate limit
        .layer(DesCircuitBreakerLayer::new(2, Duration::from_secs(60), scheduler.clone()))
        .service(create_service_with_db("account-service", core_banking_db, &mut simulation)?);

    // Transaction Service (needs high reliability)
    let transaction_service = ServiceBuilder::new()
        .layer(DesTimeoutLayer::new(Duration::from_secs(30), scheduler.clone()))
        .layer(exponential_backoff_layer(5, scheduler.clone()))  // Aggressive retry
        .layer(DesCircuitBreakerLayer::new(1, Duration::from_secs(30), scheduler.clone()))
        .service(create_service_with_db("transaction-service", core_banking_db.clone(), &mut simulation)?);

    // Fraud Detection (real-time analysis)
    let fraud_service = create_ml_service("fraud-detection", Duration::from_millis(50), &mut simulation)?;

    // Mobile API (high throughput, lower security requirements)
    let mobile_api = ServiceBuilder::new()
        .layer(DesTimeoutLayer::new(Duration::from_secs(10), scheduler.clone()))
        .layer(DesRateLimitLayer::new(1000.0, 2000, scheduler.clone()))
        .service(create_api_service("mobile-api", &mut simulation)?);

    Ok(())
}
```

## Performance Analysis

### Bottleneck Identification

Use different service capacities to identify bottlenecks:

```rust
// Create services with different capacities
let fast_service = DesServiceBuilder::new("fast-service".to_string())
    .thread_capacity(100)
    .service_time(Duration::from_millis(1))
    .build(&mut simulation)?;

let slow_service = DesServiceBuilder::new("slow-service".to_string())
    .thread_capacity(2)                      // Bottleneck: low capacity
    .service_time(Duration::from_millis(100)) // Bottleneck: slow processing
    .build(&mut simulation)?;

// Chain them to see where queuing occurs
let chained = ChainedService::new(fast_service, slow_service);
```

### Capacity Planning

Model different scaling scenarios:

```rust
// Scenario 1: Scale horizontally (more instances)
let horizontal_scale = DesLoadBalancer::round_robin(
    (0..5).map(|i| create_app_server(i, &mut simulation)).collect()
);

// Scenario 2: Scale vertically (more capacity per instance)
let vertical_scale = DesServiceBuilder::new("big-server".to_string())
    .thread_capacity(100)  // 5x capacity
    .service_time(Duration::from_millis(10))
    .build(&mut simulation)?;

// Compare performance under load
```

### Failure Impact Analysis

Test how failures propagate through the system:

```rust
// Create unreliable service
let unreliable_db = DesServiceBuilder::new("unreliable-db".to_string())
    .thread_capacity(1)
    .service_time(Duration::from_millis(1000))  // Very slow
    .build(&mut simulation)?;

// Protect with circuit breaker
let protected_db = ServiceBuilder::new()
    .layer(DesCircuitBreakerLayer::new(2, Duration::from_secs(5), scheduler))
    .service(unreliable_db);

// Test how circuit breaker affects upstream services
```

## Monitoring Multi-Tier Systems

### Service-Level Metrics

Track metrics for each tier:

```rust
// After running simulation
let metrics = simulation.metrics();

// Analyze per-service metrics
for service_name in ["load-balancer", "app-server-1", "database"] {
    let service_metrics = metrics.service_metrics(service_name);
    println!("{} - Requests: {}, Avg Latency: {:?}", 
        service_name,
        service_metrics.request_count(),
        service_metrics.average_latency()
    );
}
```

### End-to-End Latency

Track request flow through multiple tiers:

```rust
// Trace requests through the system
let trace_id = "req-12345";
let request = Request::builder()
    .method(Method::GET)
    .uri("/api/users/123")
    .header("trace-id", trace_id)
    .body(SimBody::empty())?;

// Request flows: LB → App → DB
// Each service adds timing information
```

### System Health Indicators

Monitor overall system health:

```rust
// Check circuit breaker states
let circuit_breaker_open = metrics.circuit_breakers_open();

// Check rate limiting events
let rate_limited_requests = metrics.rate_limited_count();

// Check timeout rates
let timeout_rate = metrics.timeout_rate();

println!("System Health:");
println!("  Circuit Breakers Open: {}", circuit_breaker_open);
println!("  Rate Limited Requests: {}", rate_limited_requests);
println!("  Timeout Rate: {:.2}%", timeout_rate * 100.0);
```

## Best Practices

### Service Design

1. **Capacity Planning**: Model realistic service capacities based on actual systems
2. **Timeout Configuration**: Set timeouts appropriate for each tier (longer for user-facing, shorter for internal)
3. **Circuit Breaker Tuning**: Use conservative thresholds for critical services
4. **Rate Limiting**: Apply different limits based on service criticality and capacity

### Architecture Patterns

1. **Bulkhead Pattern**: Isolate different workloads with separate service instances
2. **Timeout Propagation**: Ensure timeouts decrease as you go deeper into the stack
3. **Graceful Degradation**: Use circuit breakers to fail fast and preserve system stability
4. **Load Distribution**: Use appropriate load balancing strategies for your use case

### Testing Strategies

1. **Load Testing**: Gradually increase load to find breaking points
2. **Failure Testing**: Inject failures at different tiers to test resilience
3. **Capacity Testing**: Test different scaling scenarios
4. **Latency Testing**: Verify SLA compliance under various conditions

## Next Steps

You now have comprehensive knowledge of Tower integration in DESCARTES. Consider exploring:

- **Chapter 4: Metrics Collection** - Learn how to collect and analyze detailed performance metrics
- **Chapter 7: Advanced Examples** - Explore more complex simulation scenarios
- **API Reference** - Detailed documentation of all Tower middleware options

The Tower integration provides a powerful foundation for modeling realistic distributed systems with deterministic, reproducible behavior.