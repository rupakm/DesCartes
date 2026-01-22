# Retry Policies

Retry policies determine when and how clients should retry failed requests. DES-Components provides several built-in retry policies with different strategies for handling failures, timeouts, and network conditions.

## Retry Policy Trait

All retry policies implement the `RetryPolicy` trait:

```rust
use des_core::{RequestAttempt, Response};
use std::time::Duration;

pub trait RetryPolicy: Clone + Send + Sync + 'static {
    /// Determine if a request should be retried
    fn should_retry(
        &mut self,
        attempt: &RequestAttempt,
        response: Option<&Response>,
    ) -> Option<Duration>;
    
    /// Reset the policy state for a new request
    fn reset(&mut self);
    
    /// Get the maximum number of attempts this policy allows
    fn max_attempts(&self) -> usize;
}
```

## Exponential Backoff Policy

The most common retry policy, using exponential backoff with optional jitter:

### Basic Exponential Backoff

```rust
use des_components::ExponentialBackoffPolicy;
use std::time::Duration;

let policy = ExponentialBackoffPolicy::new(
    3, // max retry attempts
    Duration::from_millis(100), // base delay
);

// Default behavior: 100ms, 200ms, 400ms delays
```

### Advanced Configuration

```rust
let policy = ExponentialBackoffPolicy::new(
    5, // max attempts
    Duration::from_millis(50), // base delay
)
.with_multiplier(1.5) // Slower growth: 50ms, 75ms, 112ms, 168ms, 252ms
.with_max_delay(Duration::from_secs(2)) // Cap delays at 2 seconds
.with_jitter(true); // Add randomness to prevent thundering herd
```

### Deterministic Behavior

For reproducible simulations, use seeded random number generation:

```rust
use des_core::SimulationConfig;

let config = SimulationConfig::default();
let policy = ExponentialBackoffPolicy::from_config(
    &config,
    4, // max attempts
    Duration::from_millis(100),
);
// Uses simulation seed for deterministic jitter
```

## Token Bucket Retry Policy

Rate-limited retries using a token bucket algorithm:

### Basic Token Bucket

```rust
use des_components::TokenBucketRetryPolicy;
use std::time::Duration;

let policy = TokenBucketRetryPolicy::new(
    3, // max retry attempts
    5, // max tokens in bucket
    2.0, // refill rate (2 tokens per second)
);
```

### With Base Delay

```rust
let policy = TokenBucketRetryPolicy::new(3, 5, 2.0)
    .with_base_delay(Duration::from_millis(100)); // Delay when no tokens available
```

### Token Bucket Behavior

```rust
// Token bucket retry behavior:
// - Consumes 1 token per retry attempt
// - If tokens available: retry immediately
// - If no tokens: wait for base_delay
// - Tokens refill at specified rate over time
```

## Success-Based Adaptive Policy

Adaptive retry policy that adjusts behavior based on recent success rates:

### Basic Success-Based Policy

```rust
use des_components::SuccessBasedRetryPolicy;
use std::time::Duration;

let policy = SuccessBasedRetryPolicy::new(
    4, // max retry attempts
    Duration::from_millis(100), // base delay
    10, // window size for success rate tracking
);
```

### Advanced Configuration

```rust
let policy = SuccessBasedRetryPolicy::new(4, Duration::from_millis(100), 20)
    .with_min_success_rate(0.7) // 70% success rate threshold
    .with_failure_multiplier(3.0); // 3x delay when success rate is low

// Behavior:
// - Tracks success rate over last 20 requests
// - If success rate >= 70%: use base delay (100ms)
// - If success rate < 70%: use 3x delay (300ms)
```

### Recording Outcomes

```rust
// Success-based policy needs outcome tracking
let mut policy = SuccessBasedRetryPolicy::new(3, Duration::from_millis(100), 10);

// Record successful request
policy.record_outcome(true);

// Record failed request  
policy.record_outcome(false);

// Policy adjusts delays based on recorded outcomes
```

## Fixed Retry Policy

Simple policy with fixed delays and attempt limits:

### Basic Fixed Policy

```rust
use des_components::FixedRetryPolicy;
use std::time::Duration;

let policy = FixedRetryPolicy::new(
    3, // max retry attempts
    Duration::from_millis(500), // fixed delay between retries
);

// Retry pattern: 500ms, 500ms, 500ms
```

## No Retry Policy

For scenarios where retries are not desired:

```rust
use des_components::NoRetryPolicy;

let policy = NoRetryPolicy::new();
// Never retries, max_attempts() returns 1
```

## Policy Comparison Example

Here's a comprehensive example comparing different retry policies:

```rust
use des_components::{
    SimpleClient, Server, ClientEvent, ServerEvent,
    ExponentialBackoffPolicy, TokenBucketRetryPolicy, 
    SuccessBasedRetryPolicy, FixedRetryPolicy, NoRetryPolicy
};
use des_core::{Simulation, Execute, Executor, SimTime, task::PeriodicTask};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Retry Policy Comparison ===\n");
    
    let mut sim = Simulation::default();
    
    // Create server with limited capacity to trigger retries
    let server = Server::with_constant_service_time(
        "overloaded-server".to_string(),
        1, // Only 1 thread
        Duration::from_millis(200), // Slow processing
    );
    let server_id = sim.add_component(server);
    
    // Create clients with different retry policies
    let policies = vec![
        ("Exponential", create_exponential_client(server_id)),
        ("Token Bucket", create_token_bucket_client(server_id)),
        ("Success-Based", create_success_based_client(server_id)),
        ("Fixed", create_fixed_client(server_id)),
        ("No Retry", create_no_retry_client(server_id)),
    ];
    
    let mut client_ids = Vec::new();
    
    for (name, client) in policies {
        let client_id = sim.add_component(client);
        client_ids.push((name, client_id));
        
        // Each client sends 5 requests
        let task = PeriodicTask::with_count(
            move |scheduler| {
                scheduler.schedule_now(client_id, ClientEvent::SendRequest);
            },
            SimTime::from_duration(Duration::from_millis(100)),
            5,
        );
        sim.schedule_task(SimTime::zero(), task);
    }
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_secs(20)))
        .execute(&mut sim);
    
    // Analyze results
    println!("Results Summary:");
    println!("{:<15} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8}", 
        "Policy", "Requests", "Attempts", "Success", "Failures", "Timeouts");
    println!("{:-<15}-+-{:-<8}-+-{:-<8}-+-{:-<8}-+-{:-<8}-+-{:-<8}", 
        "", "", "", "", "", "");
    
    for (name, client_id) in client_ids {
        match name {
            "Exponential" => {
                let client = sim.remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(client_id)?;
                print_client_stats(name, &client, client.get_metrics());
            }
            "Token Bucket" => {
                let client = sim.remove_component::<ClientEvent, SimpleClient<TokenBucketRetryPolicy>>(client_id)?;
                print_client_stats(name, &client, client.get_metrics());
            }
            "Success-Based" => {
                let client = sim.remove_component::<ClientEvent, SimpleClient<SuccessBasedRetryPolicy>>(client_id)?;
                print_client_stats(name, &client, client.get_metrics());
            }
            "Fixed" => {
                let client = sim.remove_component::<ClientEvent, SimpleClient<FixedRetryPolicy>>(client_id)?;
                print_client_stats(name, &client, client.get_metrics());
            }
            "No Retry" => {
                let client = sim.remove_component::<ClientEvent, SimpleClient<NoRetryPolicy>>(client_id)?;
                print_client_stats(name, &client, client.get_metrics());
            }
            _ => {}
        }
    }
    
    Ok(())
}

fn create_exponential_client(server_id: des_core::Key<ServerEvent>) -> SimpleClient<ExponentialBackoffPolicy> {
    SimpleClient::with_exponential_backoff(
        "exp-client".to_string(),
        server_id,
        Duration::from_millis(100),
        3,
        Duration::from_millis(100),
    ).with_timeout(Duration::from_millis(150))
}

fn create_token_bucket_client(server_id: des_core::Key<ServerEvent>) -> SimpleClient<TokenBucketRetryPolicy> {
    let policy = TokenBucketRetryPolicy::new(3, 2, 1.0);
    SimpleClient::new(
        "token-client".to_string(),
        server_id,
        Duration::from_millis(100),
        policy,
    ).with_timeout(Duration::from_millis(150))
}

fn create_success_based_client(server_id: des_core::Key<ServerEvent>) -> SimpleClient<SuccessBasedRetryPolicy> {
    let policy = SuccessBasedRetryPolicy::new(3, Duration::from_millis(100), 5);
    SimpleClient::new(
        "success-client".to_string(),
        server_id,
        Duration::from_millis(100),
        policy,
    ).with_timeout(Duration::from_millis(150))
}

fn create_fixed_client(server_id: des_core::Key<ServerEvent>) -> SimpleClient<FixedRetryPolicy> {
    let policy = FixedRetryPolicy::new(3, Duration::from_millis(200));
    SimpleClient::new(
        "fixed-client".to_string(),
        server_id,
        Duration::from_millis(100),
        policy,
    ).with_timeout(Duration::from_millis(150))
}

fn create_no_retry_client(server_id: des_core::Key<ServerEvent>) -> SimpleClient<NoRetryPolicy> {
    SimpleClient::new(
        "no-retry-client".to_string(),
        server_id,
        Duration::from_millis(100),
        NoRetryPolicy::new(),
    ).with_timeout(Duration::from_millis(150))
}

fn print_client_stats<P: des_components::RetryPolicy>(
    name: &str, 
    client: &SimpleClient<P>, 
    metrics: &des_metrics::SimulationMetrics
) {
    let component_label = [("component", client.name.as_str())];
    
    let requests = client.requests_sent;
    let attempts = metrics.get_counter("attempts_sent", &component_label).unwrap_or(0);
    let successes = metrics.get_counter("responses_success", &component_label).unwrap_or(0);
    let failures = metrics.get_counter("responses_failure", &component_label).unwrap_or(0);
    let timeouts = metrics.get_counter("requests_timeout", &component_label).unwrap_or(0);
    
    println!("{:<15} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8}", 
        name, requests, attempts, successes, failures, timeouts);
}
```

## Policy Selection Guidelines

### When to Use Each Policy

**Exponential Backoff**
- **Best for**: General-purpose retries, API calls, web services
- **Pros**: Reduces server load, handles temporary failures well
- **Cons**: Can lead to long delays for persistent failures

**Token Bucket**
- **Best for**: Rate-limited APIs, preventing retry storms
- **Pros**: Controls retry rate, prevents overwhelming servers
- **Cons**: May delay retries even when server is available

**Success-Based Adaptive**
- **Best for**: Variable network conditions, adaptive systems
- **Pros**: Adjusts to current conditions, learns from experience
- **Cons**: More complex, requires outcome tracking

**Fixed Retry**
- **Best for**: Predictable timing requirements, simple scenarios
- **Pros**: Predictable behavior, easy to understand
- **Cons**: Doesn't adapt to conditions

**No Retry**
- **Best for**: Batch processing, when failures should be handled externally
- **Pros**: Simple, fast failure detection
- **Cons**: No resilience to temporary failures

### Configuration Best Practices

#### Exponential Backoff Configuration

```rust
// Conservative (for production-like scenarios)
let conservative = ExponentialBackoffPolicy::new(5, Duration::from_millis(200))
    .with_multiplier(2.0)
    .with_max_delay(Duration::from_secs(30))
    .with_jitter(true);

// Aggressive (for testing/development)
let aggressive = ExponentialBackoffPolicy::new(3, Duration::from_millis(50))
    .with_multiplier(1.5)
    .with_max_delay(Duration::from_secs(2))
    .with_jitter(false);
```

#### Token Bucket Configuration

```rust
// High-rate API (allows burst retries)
let high_rate = TokenBucketRetryPolicy::new(5, 10, 5.0);

// Rate-limited API (conservative retries)
let rate_limited = TokenBucketRetryPolicy::new(3, 2, 0.5);
```

#### Success-Based Configuration

```rust
// Responsive to changes
let responsive = SuccessBasedRetryPolicy::new(4, Duration::from_millis(100), 5)
    .with_min_success_rate(0.8)
    .with_failure_multiplier(2.0);

// Stable in noisy environments
let stable = SuccessBasedRetryPolicy::new(6, Duration::from_millis(200), 20)
    .with_min_success_rate(0.6)
    .with_failure_multiplier(3.0);
```

## Custom Retry Policies

You can implement custom retry policies for specific requirements:

```rust
use des_components::RetryPolicy;
use des_core::{RequestAttempt, Response};
use std::time::Duration;

#[derive(Clone)]
pub struct CustomRetryPolicy {
    max_attempts: usize,
    current_attempt: usize,
    delays: Vec<Duration>,
}

impl CustomRetryPolicy {
    pub fn new(delays: Vec<Duration>) -> Self {
        Self {
            max_attempts: delays.len(),
            current_attempt: 0,
            delays,
        }
    }
}

impl RetryPolicy for CustomRetryPolicy {
    fn should_retry(
        &mut self,
        _attempt: &RequestAttempt,
        response: Option<&Response>,
    ) -> Option<Duration> {
        self.current_attempt += 1;
        
        // Only retry on failures or timeouts
        if let Some(resp) = response {
            if resp.is_success() {
                return None;
            }
        }
        
        // Check if we have more attempts
        if self.current_attempt > self.max_attempts {
            return None;
        }
        
        // Return the configured delay for this attempt
        self.delays.get(self.current_attempt - 1).copied()
    }
    
    fn reset(&mut self) {
        self.current_attempt = 0;
    }
    
    fn max_attempts(&self) -> usize {
        self.max_attempts
    }
}

// Usage
let custom_policy = CustomRetryPolicy::new(vec![
    Duration::from_millis(100),  // First retry after 100ms
    Duration::from_millis(500),  // Second retry after 500ms
    Duration::from_secs(2),      // Third retry after 2s
]);
```

## Testing Retry Policies

Test retry policies in isolation:

```rust
use des_components::{ExponentialBackoffPolicy, RetryPolicy};
use des_core::{RequestAttempt, RequestAttemptId, RequestId, Response, SimTime};

fn test_retry_policy() {
    let mut policy = ExponentialBackoffPolicy::new(3, Duration::from_millis(100));
    
    let attempt = RequestAttempt::new(
        RequestAttemptId(1),
        RequestId(1),
        1,
        SimTime::zero(),
        vec![],
    );
    
    // Simulate failure response
    let error_response = Response::error(
        RequestAttemptId(1),
        RequestId(1),
        SimTime::from_millis(100),
        500,
        "Internal Server Error".to_string(),
    );
    
    // Test retry decisions
    assert_eq!(policy.should_retry(&attempt, Some(&error_response)), 
               Some(Duration::from_millis(100)));
    assert_eq!(policy.should_retry(&attempt, Some(&error_response)), 
               Some(Duration::from_millis(200)));
    assert_eq!(policy.should_retry(&attempt, Some(&error_response)), 
               Some(Duration::from_millis(400)));
    assert_eq!(policy.should_retry(&attempt, Some(&error_response)), None);
    
    // Test reset
    policy.reset();
    assert_eq!(policy.should_retry(&attempt, Some(&error_response)), 
               Some(Duration::from_millis(100)));
}
```

## Monitoring Retry Behavior

Monitor retry metrics to optimize policy configuration:

```rust
// Key metrics to monitor:
let attempts_per_request = total_attempts as f64 / total_requests as f64;
let retry_success_rate = successful_retries as f64 / total_retries as f64;
let permanent_failure_rate = permanent_failures as f64 / total_requests as f64;

// Optimization guidelines:
if attempts_per_request > 3.0 {
    // Consider more aggressive backoff or shorter timeouts
}

if retry_success_rate < 0.3 {
    // Retries aren't helping - check server capacity or timeout settings
}

if permanent_failure_rate > 0.1 {
    // Too many requests failing permanently - increase max attempts or adjust policy
}
```

Retry policies are essential for building resilient distributed systems. Choose and configure policies based on your specific requirements for latency, throughput, and fault tolerance.