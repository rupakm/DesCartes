# Payload-Dependent Service Time Distributions

Real-world services often have processing times that depend on request characteristics like payload size, endpoint complexity, or data processing requirements. This section demonstrates how to model these scenarios in DESCARTES.

## Overview

Service time distributions in DESCARTES can be:

- **Request Size-Based**: Processing time scales with payload size
- **Endpoint-Based**: Different endpoints have different processing characteristics  
- **Content-Type Based**: Processing varies by data format (JSON, XML, binary)
- **Computational Complexity**: Time depends on algorithmic complexity

All examples in this section are implemented as runnable tests in `des-components/tests/advanced_examples.rs`.

## Request Size-Based Service Times

Many services have processing times that scale with request payload size. For example:
- File upload services
- Data processing APIs
- Image/video processing services

### Implementation

```rust
use des_core::dists::RequestSizeBasedServiceTime;
use std::time::Duration;

// Create a distribution where:
// - Base time: 10ms (minimum processing time)
// - Additional time: 1ms per 100 bytes of payload
// - Maximum time: 500ms (prevents extremely long processing)
let size_based_dist = RequestSizeBasedServiceTime::new(
    Duration::from_millis(10),    // base_time
    Duration::from_nanos(10_000), // time_per_byte (10μs = 1ms per 100 bytes)
    Duration::from_millis(500),   // max_time
);

let service = DesServiceBuilder::new("file-processor".to_string())
    .thread_capacity(3)
    .service_time_distribution(size_based_dist)
    .build(&mut simulation)?;
```

### Example Scenario: File Upload Service

```rust
// Test different file sizes
let test_cases = vec![
    ("Small file", "Hello World".to_string()),           // ~10ms
    ("Medium file", "X".repeat(1000)),                   // ~20ms  
    ("Large file", "Y".repeat(10000)),                   // ~110ms
    ("Huge file", "Z".repeat(100000)),                   // 500ms (capped)
];

for (description, content) in test_cases {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/upload")
        .body(SimBody::new(content.into_bytes()))?;
    
    let response = service.call(request).await?;
    println!("{}: {} ({}ms)", description, response.status(), 
             measure_processing_time());
}
```

### Performance Characteristics

Request size-based distributions exhibit these patterns:

- **Linear Scaling**: Processing time increases linearly with payload size
- **Base Overhead**: Minimum processing time regardless of size
- **Capacity Limits**: Maximum processing time prevents runaway scenarios
- **Realistic Modeling**: Matches real-world file processing behavior

## Endpoint-Based Service Times

Different API endpoints often have vastly different processing requirements:

- `/health` - Fast health checks (1-5ms)
- `/search` - Database queries (50-200ms)  
- `/analytics` - Complex computations (500-2000ms)
- `/reports` - Heavy data processing (2-10 seconds)

### Implementation

```rust
use des_core::dists::{EndpointBasedServiceTime, ConstantServiceTime, ExponentialDistribution};

let mut endpoint_dist = EndpointBasedServiceTime::new(
    // Default distribution for unknown endpoints
    Box::new(ConstantServiceTime::new(Duration::from_millis(100)))
);

// Fast endpoints
endpoint_dist.add_endpoint(
    "/health".to_string(),
    Box::new(ConstantServiceTime::new(Duration::from_millis(2)))
);

endpoint_dist.add_endpoint(
    "/metrics".to_string(), 
    Box::new(ConstantServiceTime::new(Duration::from_millis(5)))
);

// Database endpoints with some variability
endpoint_dist.add_endpoint(
    "/api/users".to_string(),
    Box::new(ExponentialDistribution::new(10.0)) // ~100ms mean
);

// Heavy computation endpoints
endpoint_dist.add_endpoint(
    "/api/analytics".to_string(),
    Box::new(ExponentialDistribution::new(1.0)) // ~1000ms mean
);

let service = DesServiceBuilder::new("api-server".to_string())
    .thread_capacity(5)
    .service_time_distribution(endpoint_dist)
    .build(&mut simulation)?;
```

### Example Scenario: Multi-Endpoint API

```rust
// Test different endpoints
let endpoints = vec![
    ("/health", "Health check"),
    ("/api/users/123", "User lookup"),
    ("/api/analytics/report", "Analytics report"),
    ("/api/unknown", "Unknown endpoint (uses default)"),
];

for (path, description) in endpoints {
    let request = Request::builder()
        .method(Method::GET)
        .uri(path)
        .body(SimBody::empty())?;
    
    let start = Instant::now();
    let response = service.call(request).await?;
    let duration = start.elapsed();
    
    println!("{}: {}ms ({})", description, duration.as_millis(), 
             response.status());
}
```

### Endpoint Performance Patterns

Different endpoint types show distinct characteristics:

| Endpoint Type | Typical Range | Distribution | Use Case |
|---------------|---------------|--------------|----------|
| Health checks | 1-5ms | Constant | Monitoring, load balancers |
| Simple queries | 10-50ms | Exponential | User lookups, basic CRUD |
| Complex queries | 100-500ms | Gamma/Log-normal | Search, aggregations |
| Heavy processing | 1-10s | Exponential/Custom | Reports, analytics |

## Content-Type Based Processing

Processing time can vary significantly based on data format:

```rust
use des_core::dists::ContentTypeBasedServiceTime;

let mut content_dist = ContentTypeBasedServiceTime::new(
    Box::new(ConstantServiceTime::new(Duration::from_millis(50))) // default
);

// JSON: Fast parsing
content_dist.add_content_type(
    "application/json".to_string(),
    Box::new(ConstantServiceTime::new(Duration::from_millis(20)))
);

// XML: Slower parsing
content_dist.add_content_type(
    "application/xml".to_string(),
    Box::new(ExponentialDistribution::new(5.0)) // ~200ms mean
);

// Binary: Variable processing based on format
content_dist.add_content_type(
    "application/octet-stream".to_string(),
    Box::new(ExponentialDistribution::new(2.0)) // ~500ms mean
);
```

## Computational Complexity-Based Times

For services that perform algorithmic processing, service time can depend on computational complexity:

```rust
use des_core::dists::ComplexityBasedServiceTime;

// O(n) algorithm: time scales linearly with input size
let linear_dist = ComplexityBasedServiceTime::linear(
    Duration::from_millis(1),  // base time
    Duration::from_nanos(100), // time per input element
);

// O(n log n) algorithm: sorting, efficient search
let nlogn_dist = ComplexityBasedServiceTime::nlogn(
    Duration::from_millis(5),  // base time
    Duration::from_nanos(50),  // scaling factor
);

// O(n²) algorithm: nested loops, naive algorithms
let quadratic_dist = ComplexityBasedServiceTime::quadratic(
    Duration::from_millis(2),  // base time
    Duration::from_nanos(10),  // scaling factor
    Duration::from_secs(30),   // max time (prevents runaway)
);
```

### Example: Data Processing Service

```rust
// Simulate different algorithm complexities
let algorithms = vec![
    ("Linear search", linear_dist, 1000),
    ("Merge sort", nlogn_dist, 1000), 
    ("Bubble sort", quadratic_dist, 100), // Smaller input for O(n²)
];

for (name, dist, input_size) in algorithms {
    let service = DesServiceBuilder::new(format!("{}-service", name))
        .service_time_distribution(dist)
        .build(&mut simulation)?;
    
    let request = create_processing_request(input_size);
    let response = service.call(request).await?;
    
    println!("{} (n={}): {}ms", name, input_size, 
             measure_processing_time());
}
```

## Combined Distribution Patterns

Real services often combine multiple factors:

```rust
use des_core::dists::CombinedServiceTime;

// Combine size-based and endpoint-based distributions
let combined_dist = CombinedServiceTime::new()
    .add_factor(Box::new(size_based_dist))      // Base on payload size
    .add_factor(Box::new(endpoint_dist))        // Modify by endpoint
    .add_factor(Box::new(complexity_dist));     // Add computational complexity

let service = DesServiceBuilder::new("complex-service".to_string())
    .service_time_distribution(combined_dist)
    .build(&mut simulation)?;
```

## Performance Analysis

### Measuring Distribution Effectiveness

```rust
// Collect service time samples
let mut samples = Vec::new();

for _ in 0..1000 {
    let request = create_test_request();
    let start = simulation.time();
    let _response = service.call(request).await?;
    let service_time = simulation.time().duration_since(start);
    samples.push(service_time.as_millis() as f64);
}

// Analyze distribution
let mean = samples.iter().sum::<f64>() / samples.len() as f64;
let variance = samples.iter()
    .map(|x| (x - mean).powi(2))
    .sum::<f64>() / samples.len() as f64;

println!("Service Time Analysis:");
println!("  Mean: {:.2}ms", mean);
println!("  Std Dev: {:.2}ms", variance.sqrt());
println!("  Min: {:.2}ms", samples.iter().fold(f64::INFINITY, |a, &b| a.min(b)));
println!("  Max: {:.2}ms", samples.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b)));
```

### Visualization

```rust
// Generate service time histogram
use plotters::prelude::*;

let output_path = "target/service_time_distribution.png";
let root = BitMapBackend::new(output_path, (800, 600)).into_drawing_area();
root.fill(&WHITE)?;

let mut chart = ChartBuilder::on(&root)
    .caption("Service Time Distribution", ("sans-serif", 40))
    .margin(10)
    .x_label_area_size(50)
    .y_label_area_size(60)
    .build_cartesian_2d(0f64..samples.iter().fold(0f64, |a, &b| a.max(b)), 0f64..100f64)?;

// Create histogram
let histogram = samples.iter()
    .fold(std::collections::HashMap::new(), |mut acc, &sample| {
        let bucket = (sample / 10.0).floor() * 10.0; // 10ms buckets
        *acc.entry(bucket as i32).or_insert(0) += 1;
        acc
    });

chart.draw_series(
    histogram.iter().map(|(&bucket, &count)| {
        Rectangle::new([(bucket as f64, 0.0), (bucket as f64 + 10.0, count as f64)], BLUE.filled())
    })
)?;

root.present()?;
println!("Generated histogram: {}", output_path);
```

## Testing Strategies

### Unit Tests for Distributions

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_size_based_distribution() {
        let dist = RequestSizeBasedServiceTime::new(
            Duration::from_millis(10),
            Duration::from_nanos(1000), // 1μs per byte
            Duration::from_millis(100),
        );

        // Small request
        let small_request = create_request_with_size(100); // 100 bytes
        let small_time = dist.sample(&small_request);
        assert!(small_time >= Duration::from_millis(10));
        assert!(small_time <= Duration::from_millis(11)); // 10ms + 100μs

        // Large request (should be capped)
        let large_request = create_request_with_size(1_000_000); // 1MB
        let large_time = dist.sample(&large_request);
        assert_eq!(large_time, Duration::from_millis(100)); // Capped at max
    }

    #[test]
    fn test_endpoint_based_distribution() {
        let mut dist = EndpointBasedServiceTime::new(
            Box::new(ConstantServiceTime::new(Duration::from_millis(50)))
        );
        
        dist.add_endpoint(
            "/fast".to_string(),
            Box::new(ConstantServiceTime::new(Duration::from_millis(5)))
        );

        let fast_request = create_request_for_endpoint("/fast");
        let fast_time = dist.sample(&fast_request);
        assert_eq!(fast_time, Duration::from_millis(5));

        let unknown_request = create_request_for_endpoint("/unknown");
        let unknown_time = dist.sample(&unknown_request);
        assert_eq!(unknown_time, Duration::from_millis(50)); // Default
    }
}
```

### Integration Tests

```rust
#[test]
fn test_payload_dependent_service_integration() {
    let mut simulation = Simulation::default();
    
    // Create service with size-based distribution
    let dist = RequestSizeBasedServiceTime::new(
        Duration::from_millis(10),
        Duration::from_nanos(10_000), // 10μs per byte
        Duration::from_millis(200),
    );
    
    let service = DesServiceBuilder::new("integration-test".to_string())
        .thread_capacity(2)
        .service_time_distribution(dist)
        .build(&mut simulation)?;

    // Test with different payload sizes
    let test_cases = vec![
        (100, Duration::from_millis(11)),   // 10ms + 1ms
        (1000, Duration::from_millis(20)),  // 10ms + 10ms
        (50000, Duration::from_millis(200)), // Capped at 200ms
    ];

    for (size, expected_time) in test_cases {
        let request = create_request_with_size(size);
        let start_time = simulation.time();
        
        // Process request
        let response = service.call(request).await?;
        
        let actual_time = simulation.time().duration_since(start_time);
        assert!(response.status().is_success());
        
        // Allow some tolerance for simulation timing
        let tolerance = Duration::from_millis(5);
        assert!(actual_time >= expected_time - tolerance);
        assert!(actual_time <= expected_time + tolerance);
    }
}
```

## Best Practices

### Distribution Selection

1. **Measure Real Systems**: Base distributions on actual production metrics
2. **Start Simple**: Begin with constant or exponential distributions
3. **Add Complexity Gradually**: Introduce payload dependencies as needed
4. **Validate Behavior**: Test distributions match expected patterns

### Performance Considerations

1. **Sampling Efficiency**: Use efficient sampling algorithms for complex distributions
2. **Memory Usage**: Avoid storing large lookup tables for endpoint mappings
3. **Computation Overhead**: Balance realism with simulation performance
4. **Caching**: Cache distribution parameters for frequently accessed endpoints

### Modeling Guidelines

1. **Realistic Bounds**: Always set maximum processing times to prevent runaway scenarios
2. **Base Times**: Include minimum processing overhead even for empty requests
3. **Variability**: Add appropriate randomness to model real-world variation
4. **Validation**: Compare simulation results with production metrics

## Real-World Applications

### Microservices Architecture

```rust
// Model different microservice characteristics
let user_service = create_service_with_db_distribution("user-service");
let auth_service = create_service_with_cache_distribution("auth-service");  
let analytics_service = create_service_with_compute_distribution("analytics-service");
```

### Content Delivery Networks

```rust
// Model CDN edge server behavior
let cdn_dist = ContentSizeBasedServiceTime::new()
    .with_cache_hit_ratio(0.8)  // 80% cache hits
    .with_cache_time(Duration::from_millis(5))
    .with_origin_fetch_time(Duration::from_millis(200));
```

### Database Services

```rust
// Model database query complexity
let db_dist = QueryComplexityBasedServiceTime::new()
    .add_query_type("SELECT", Duration::from_millis(10))
    .add_query_type("INSERT", Duration::from_millis(20))
    .add_query_type("UPDATE", Duration::from_millis(30))
    .add_query_type("DELETE", Duration::from_millis(25))
    .add_query_type("JOIN", Duration::from_millis(100));
```

## Next Steps

Continue to [Client-Side Admission Control](./client_side_admission_control.md) to learn about implementing token bucket algorithms and adaptive rate limiting strategies.