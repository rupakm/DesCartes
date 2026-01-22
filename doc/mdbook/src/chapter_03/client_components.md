# Client Components

The `SimpleClient` component generates requests with configurable retry policies, timeouts, and request patterns. It provides realistic modeling of client behavior including failure handling and adaptive retry strategies.

## Basic Client Creation

### Simple Client with Retry Policy

Create a client with exponential backoff retry policy:

```rust
use des_components::{SimpleClient, ExponentialBackoffPolicy, ServerEvent};
use des_core::{Simulation, Key};
use std::time::Duration;

let mut sim = Simulation::default();
let server_key: Key<ServerEvent> = /* server component key */;

let retry_policy = ExponentialBackoffPolicy::new(
    3, // max retry attempts
    Duration::from_millis(100), // base delay
);

let client = SimpleClient::new(
    "api-client".to_string(),
    server_key,
    Duration::from_millis(200), // request interval
    retry_policy,
);
```

### Convenience Constructor

Use the convenience constructor for common exponential backoff scenarios:

```rust
let client = SimpleClient::with_exponential_backoff(
    "web-client".to_string(),
    server_key,
    Duration::from_millis(500), // request every 500ms
    5, // max 5 retries
    Duration::from_millis(200), // base retry delay
);
```

## Client Configuration

### Request Limits

Limit the total number of requests:

```rust
let client = SimpleClient::with_exponential_backoff(
    "batch-client".to_string(),
    server_key,
    Duration::from_millis(100),
    3,
    Duration::from_millis(150),
).with_max_requests(1000); // Stop after 1000 requests
```

### Timeout Configuration

Set request timeout duration:

```rust
let client = SimpleClient::new(
    "timeout-client".to_string(),
    server_key,
    Duration::from_millis(300),
    retry_policy,
).with_timeout(Duration::from_secs(2)); // 2 second timeout
```

## Retry Policies

### Exponential Backoff

Exponential backoff with jitter prevents thundering herd problems:

```rust
use des_components::ExponentialBackoffPolicy;

let policy = ExponentialBackoffPolicy::new(
    4, // max attempts
    Duration::from_millis(100), // base delay
)
.with_multiplier(2.0) // 2x growth (100ms, 200ms, 400ms, 800ms)
.with_max_delay(Duration::from_secs(5)) // cap at 5 seconds
.with_jitter(true); // add randomness to prevent thundering herd
```

### Token Bucket Retry

Rate-limited retries using token bucket algorithm:

```rust
use des_components::TokenBucketRetryPolicy;

let policy = TokenBucketRetryPolicy::new(
    3, // max attempts
    5, // max tokens in bucket
    2.0, // refill rate (2 tokens per second)
).with_base_delay(Duration::from_millis(50)); // delay when no tokens
```

### Success-Based Adaptive Retry

Adaptive retry policy that adjusts based on recent success rate:

```rust
use des_components::SuccessBasedRetryPolicy;

let policy = SuccessBasedRetryPolicy::new(
    4, // max attempts
    Duration::from_millis(100), // base delay
    10, // window size for success rate calculation
)
.with_min_success_rate(0.7) // 70% success rate threshold
.with_failure_multiplier(3.0); // 3x delay when success rate is low
```

### No Retry Policy

For scenarios where retries are not desired:

```rust
use des_components::NoRetryPolicy;

let client = SimpleClient::new(
    "no-retry-client".to_string(),
    server_key,
    Duration::from_millis(100),
    NoRetryPolicy::new(),
);
```

### Fixed Retry Policy

Fixed number of retries with constant delay:

```rust
use des_components::FixedRetryPolicy;

let policy = FixedRetryPolicy::new(
    3, // max retry attempts
    Duration::from_millis(500), // fixed 500ms delay between retries
);
```

## Complete Client Example

Here's a comprehensive example showing client usage with different retry policies:

```rust
use des_components::{
    SimpleClient, Server, ClientEvent, ServerEvent,
    ExponentialBackoffPolicy, TokenBucketRetryPolicy, SuccessBasedRetryPolicy
};
use des_core::{Simulation, Execute, Executor, SimTime, task::PeriodicTask};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sim = Simulation::default();
    
    // Create server
    let server = Server::with_constant_service_time(
        "test-server".to_string(),
        2, // Limited capacity to trigger retries
        Duration::from_millis(100),
    );
    let server_id = sim.add_component(server);
    
    // Client 1: Exponential backoff
    let exp_client = SimpleClient::with_exponential_backoff(
        "exp-client".to_string(),
        server_id,
        Duration::from_millis(50), // Aggressive request rate
        3,
        Duration::from_millis(100),
    ).with_timeout(Duration::from_millis(200));
    
    let exp_client_id = sim.add_component(exp_client);
    
    // Client 2: Token bucket retry
    let token_policy = TokenBucketRetryPolicy::new(3, 2, 1.0);
    let token_client = SimpleClient::new(
        "token-client".to_string(),
        server_id,
        Duration::from_millis(75),
        token_policy,
    ).with_timeout(Duration::from_millis(200));
    
    let token_client_id = sim.add_component(token_client);
    
    // Client 3: Success-based adaptive
    let success_policy = SuccessBasedRetryPolicy::new(
        3,
        Duration::from_millis(100),
        5,
    );
    let success_client = SimpleClient::new(
        "success-client".to_string(),
        server_id,
        Duration::from_millis(100),
        success_policy,
    ).with_timeout(Duration::from_millis(200));
    
    let success_client_id = sim.add_component(success_client);
    
    // Start all clients with periodic requests
    for (client_id, name) in [
        (exp_client_id, "exp-client"),
        (token_client_id, "token-client"), 
        (success_client_id, "success-client")
    ] {
        let task = PeriodicTask::with_count(
            move |scheduler| {
                scheduler.schedule_now(client_id, ClientEvent::SendRequest);
            },
            SimTime::from_duration(Duration::from_millis(200)),
            10, // 10 requests per client
        );
        sim.schedule_task(SimTime::zero(), task);
    }
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_secs(15)))
        .execute(&mut sim);
    
    // Analyze results
    let exp_client = sim.remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(exp_client_id)?;
    let token_client = sim.remove_component::<ClientEvent, SimpleClient<TokenBucketRetryPolicy>>(token_client_id)?;
    let success_client = sim.remove_component::<ClientEvent, SimpleClient<SuccessBasedRetryPolicy>>(success_client_id)?;
    
    println!("=== Client Performance Comparison ===");
    
    for (name, requests_sent, active_requests, metrics) in [
        ("Exponential Backoff", exp_client.requests_sent, exp_client.active_requests.len(), exp_client.get_metrics()),
        ("Token Bucket", token_client.requests_sent, token_client.active_requests.len(), token_client.get_metrics()),
        ("Success-Based", success_client.requests_sent, success_client.active_requests.len(), success_client.get_metrics()),
    ] {
        println!("\n{} Client:", name);
        println!("  Requests sent: {}", requests_sent);
        println!("  Active requests: {}", active_requests);
        
        let component_label = [("component", name.to_lowercase().replace(" ", "-").as_str())];
        let successes = metrics.get_counter("responses_success", &component_label).unwrap_or(0);
        let failures = metrics.get_counter("responses_failure", &component_label).unwrap_or(0);
        let timeouts = metrics.get_counter("requests_timeout", &component_label).unwrap_or(0);
        let retries = metrics.get_counter("retries_scheduled", &component_label).unwrap_or(0);
        
        println!("  Successful responses: {}", successes);
        println!("  Failed responses: {}", failures);
        println!("  Timeouts: {}", timeouts);
        println!("  Retries scheduled: {}", retries);
        
        if successes + failures > 0 {
            let success_rate = successes as f64 / (successes + failures) as f64 * 100.0;
            println!("  Success rate: {:.1}%", success_rate);
        }
    }
    
    Ok(())
}
```

## Client Metrics

Clients automatically collect detailed metrics about request patterns and retry behavior:

```rust
let client_metrics = client.get_metrics();
let component_label = [("component", "my-client")];

// Request metrics
let requests_sent = client_metrics.get_counter("requests_sent", &component_label);
let attempts_sent = client_metrics.get_counter("attempts_sent", &component_label);

// Response metrics  
let successes = client_metrics.get_counter("responses_success", &component_label);
let failures = client_metrics.get_counter("responses_failure", &component_label);
let timeouts = client_metrics.get_counter("requests_timeout", &component_label);

// Retry metrics
let retries_scheduled = client_metrics.get_counter("retries_scheduled", &component_label);
let permanent_failures = client_metrics.get_counter("requests_failed_permanently", &component_label);

// Latency metrics (histogram)
let response_times = client_metrics.get_histogram("response_time_ms", &component_label);
```

### Available Metrics

- **Counters**: `requests_sent`, `attempts_sent`, `responses_success`, `responses_failure`, `requests_timeout`, `retries_scheduled`, `requests_failed_permanently`
- **Gauges**: `total_requests`, `active_requests`
- **Histograms**: `response_time_ms` (end-to-end latency including retries)

## Client Patterns

### Load Testing Client

Generate sustained load for performance testing:

```rust
let load_client = SimpleClient::with_exponential_backoff(
    "load-tester".to_string(),
    server_key,
    Duration::from_millis(10), // High request rate
    2, // Limited retries for load testing
    Duration::from_millis(50),
).with_timeout(Duration::from_millis(500))
 .with_max_requests(10000); // Large number of requests
```

### Resilient Client

Client designed for unreliable networks:

```rust
let resilient_policy = ExponentialBackoffPolicy::new(
    8, // Many retry attempts
    Duration::from_millis(200),
).with_multiplier(1.5) // Slower growth
 .with_max_delay(Duration::from_secs(30)) // High max delay
 .with_jitter(true);

let resilient_client = SimpleClient::new(
    "resilient-client".to_string(),
    server_key,
    Duration::from_secs(1),
    resilient_policy,
).with_timeout(Duration::from_secs(10)); // Long timeout
```

### Batch Client

Client for batch processing scenarios:

```rust
let batch_client = SimpleClient::new(
    "batch-processor".to_string(),
    server_key,
    Duration::from_secs(5), // Infrequent requests
    NoRetryPolicy::new(), // No retries for batch jobs
).with_max_requests(100) // Fixed batch size
 .with_timeout(Duration::from_secs(60)); // Long timeout for batch processing
```

## Best Practices

### Retry Policy Selection

Choose retry policies based on your use case:
- **Exponential Backoff**: General purpose, good for most scenarios
- **Token Bucket**: When you need to limit retry rate across clients
- **Success-Based**: For adaptive behavior in varying network conditions
- **Fixed**: When you need predictable retry timing
- **No Retry**: For batch jobs or when failures should be handled externally

### Timeout Configuration

Set timeouts appropriately:
- **Short timeouts** (100-500ms): Interactive applications, fast failure detection
- **Medium timeouts** (1-5s): API calls, web services
- **Long timeouts** (10-60s): Batch processing, file uploads

### Request Rate Management

Balance request rate with server capacity:
- Start with conservative rates and increase based on metrics
- Monitor server rejection rates and client timeout rates
- Use multiple clients to simulate realistic load distribution

### Monitoring and Alerting

Set up monitoring for key client metrics:
- Success rate < 95% may indicate server issues
- High retry rates may indicate capacity problems
- Increasing response times may indicate performance degradation

## Error Handling

Clients handle various error conditions:

```rust
// Clients automatically handle:
// - Server rejections (503 Service Unavailable)
// - Request timeouts
// - Retry policy limits exceeded
// - Network simulation errors

// Check final client state
if client.active_requests.len() > 0 {
    println!("Warning: {} requests still active at simulation end", 
        client.active_requests.len());
}

let total_attempts = client_metrics.get_counter("attempts_sent", &component_label).unwrap_or(0);
let total_requests = client.requests_sent;
let avg_attempts_per_request = total_attempts as f64 / total_requests as f64;

if avg_attempts_per_request > 2.0 {
    println!("High retry rate: {:.1} attempts per request", avg_attempts_per_request);
}
```

The SimpleClient component provides flexible, realistic client behavior modeling with comprehensive retry policies and detailed metrics for analyzing distributed system performance under various failure conditions.