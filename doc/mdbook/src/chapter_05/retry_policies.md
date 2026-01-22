# Clients with Timeouts and Retry Policies

Real-world systems need reliability mechanisms to handle failures, network issues, and temporary overloads. This section shows how to add timeouts and retry policies to your M/M/k queueing systems using DESCARTES.

## Why Timeouts and Retries Matter

In distributed systems, requests can fail for many reasons:
- **Network timeouts**: Packets lost or delayed
- **Server overload**: Temporary capacity exceeded
- **Transient errors**: Brief service unavailability
- **Resource contention**: Temporary resource exhaustion

Timeouts and retries help systems recover gracefully from these issues.

## Client Timeout Configuration

### Basic Timeout Setup

```rust
use des_components::{SimpleClient, FixedRetryPolicy};
use std::time::Duration;

// Create client with timeout
let client = SimpleClient::new(
    "reliable_client".to_string(),
    server_key,
    Duration::from_millis(500), // Send requests every 500ms
    FixedRetryPolicy::new(3, Duration::from_millis(200)), // 3 retries, 200ms delay
)
.with_timeout(Duration::from_millis(1000)); // 1 second timeout
```

### Timeout Behavior

When a timeout occurs:
1. **Request marked as failed**: No response received within timeout period
2. **Retry policy consulted**: Determines if/when to retry
3. **Metrics recorded**: Timeout events tracked for analysis
4. **Client notified**: Timeout event sent to client component

```rust
impl Component for MyClient {
    type Event = ClientEvent;
    
    fn process_event(&mut self, event: &ClientEvent, scheduler: &mut Scheduler) {
        match event {
            ClientEvent::RequestTimeout { request_id, attempt_id } => {
                println!("Request {} timed out after {}ms", 
                         request_id.0, self.timeout.as_millis());
                
                // Record timeout metrics
                self.metrics.increment_counter("timeouts", &[("component", &self.name)]);
            }
            ClientEvent::ResponseReceived { response } => {
                // Handle successful response
                self.handle_response(response);
            }
            _ => {}
        }
    }
}
```

## Retry Policy Types

DESCARTES provides several retry policy implementations:

### 1. Fixed Retry Policy

Simple policy with fixed delay between retries:

```rust
use des_components::FixedRetryPolicy;

// Retry up to 3 times with 500ms delay between attempts
let retry_policy = FixedRetryPolicy::new(3, Duration::from_millis(500));

let client = SimpleClient::new(
    "fixed_retry_client".to_string(),
    server_key,
    Duration::from_secs(1), // Request interval
    retry_policy,
)
.with_timeout(Duration::from_millis(800));
```

**Use cases:**
- Simple, predictable retry behavior
- When you know the optimal retry delay
- Testing and debugging scenarios

### 2. Exponential Backoff Policy

Increases delay exponentially with each retry attempt:

```rust
use des_components::ExponentialBackoffPolicy;

// Start with 100ms, double each time, max 5 attempts
let retry_policy = ExponentialBackoffPolicy::new(5, Duration::from_millis(100))
    .with_multiplier(2.0)                           // Double delay each time
    .with_max_delay(Duration::from_secs(10))        // Cap at 10 seconds
    .with_jitter(true);                             // Add randomness

let client = SimpleClient::new(
    "exponential_client".to_string(),
    server_key,
    Duration::from_secs(1),
    retry_policy,
)
.with_timeout(Duration::from_millis(500));
```

**Retry delays:** 100ms → 200ms → 400ms → 800ms → 1600ms

**Benefits:**
- **Reduces server load**: Longer delays give servers time to recover
- **Prevents thundering herd**: Jitter spreads out retry attempts
- **Adapts to severity**: More retries = longer delays

### 3. Token Bucket Retry Policy

Rate-limits retries using a token bucket algorithm:

```rust
use des_components::TokenBucketRetryPolicy;

// Allow 5 retries initially, refill at 2 retries/second
let retry_policy = TokenBucketRetryPolicy::new(
    3,    // Max attempts per request
    5,    // Token bucket capacity
    2.0,  // Refill rate (tokens per second)
)
.with_base_delay(Duration::from_millis(100));

let client = SimpleClient::new(
    "token_bucket_client".to_string(),
    server_key,
    Duration::from_millis(200),
    retry_policy,
)
.with_timeout(Duration::from_millis(300));
```

**Benefits:**
- **Rate limiting**: Prevents retry storms
- **Burst handling**: Allows bursts up to bucket capacity
- **Steady state**: Maintains sustainable retry rate

### 4. Success-Based Adaptive Policy

Adjusts retry behavior based on recent success rate:

```rust
use des_components::SuccessBasedRetryPolicy;

// Adapt based on last 20 requests, require 70% success rate
let retry_policy = SuccessBasedRetryPolicy::new(
    4,                              // Max attempts
    Duration::from_millis(200),     // Base delay
    20,                             // Window size for success tracking
)
.with_min_success_rate(0.7)         // 70% success rate threshold
.with_failure_multiplier(3.0);      // 3x delay when success rate low

let client = SimpleClient::new(
    "adaptive_client".to_string(),
    server_key,
    Duration::from_millis(300),
    retry_policy,
)
.with_timeout(Duration::from_millis(400));
```

**Adaptive behavior:**
- **High success rate**: Normal retry delays
- **Low success rate**: Increased delays to reduce load
- **Learning**: Continuously adapts to system conditions

## Complete M/M/1 with Retry Example

Here's a complete example showing M/M/1 queue with exponential backoff retries:

```rust
//! M/M/1 Queue with Exponential Backoff Retries
//!
//! This example demonstrates how retry policies affect queueing performance

use des_components::{
    ClientEvent, ExponentialBackoffPolicy, FifoQueue, Server, ServerEvent, SimpleClient,
};
use des_core::{task::PeriodicTask, Execute, Executor, SimTime, Simulation};
use des_metrics::SimulationMetrics;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RetryQueueConfig {
    pub lambda: f64,                    // Arrival rate
    pub mu: f64,                        // Service rate
    pub timeout: Duration,              // Client timeout
    pub max_retries: usize,             // Maximum retry attempts
    pub base_retry_delay: Duration,     // Base retry delay
    pub queue_capacity: Option<usize>,  // Queue capacity
    pub duration: Duration,             // Simulation duration
}

pub fn run_mm1_with_exponential_backoff(config: RetryQueueConfig) -> RetryQueueResults {
    println!("\n🔄 Running M/M/1 with Exponential Backoff");
    println!(
        "λ={:.2}, μ={:.2}, timeout={:?}, max_retries={}, base_delay={:?}",
        config.lambda, config.mu, config.timeout, config.max_retries, config.base_retry_delay
    );

    let mut simulation = Simulation::default();
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));

    // Create server with potential for overload
    let server = Server::with_exponential_service_time(
        "retry_server".to_string(),
        1, // Single server
        Duration::from_secs_f64(1.0 / config.mu),
    );

    let server = if let Some(capacity) = config.queue_capacity {
        server.with_queue(Box::new(FifoQueue::bounded(capacity)))
    } else {
        server.with_queue(Box::new(FifoQueue::unbounded()))
    };

    let server_key = simulation.add_component(server);

    // Create client with exponential backoff retry policy
    let retry_policy = ExponentialBackoffPolicy::new(
        config.max_retries,
        config.base_retry_delay,
    )
    .with_multiplier(2.0)
    .with_max_delay(Duration::from_secs(5))
    .with_jitter(true);

    let client = SimpleClient::new(
        "retry_client".to_string(),
        server_key,
        Duration::from_secs_f64(1.0 / config.lambda), // Inter-request time
        retry_policy,
    )
    .with_timeout(config.timeout);

    let client_key = simulation.add_component(client);

    // Generate requests using periodic task
    let request_task = PeriodicTask::new(
        move |scheduler| {
            scheduler.schedule_now(client_key, ClientEvent::SendRequest);
        },
        SimTime::from_duration(Duration::from_secs_f64(1.0 / config.lambda)),
    );

    simulation.schedule_task(SimTime::from_millis(100), request_task);

    // Run simulation
    let executor = Executor::timed(SimTime::from_duration(config.duration));
    executor.execute(&mut simulation);

    // Collect results
    let final_server = simulation
        .remove_component::<ServerEvent, Server>(server_key)
        .unwrap();
    let final_client = simulation
        .remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(client_key)
        .unwrap();

    let client_metrics = final_client.get_metrics();

    // Calculate metrics
    let requests_sent = final_client.requests_sent;
    let requests_completed = final_server.requests_processed;
    let requests_dropped = final_server.requests_rejected;

    // Get timeout and retry counts from client metrics
    let timeouts = client_metrics
        .get_counter("timeouts", &[("component", "retry_client")])
        .unwrap_or(0);
    let retries = client_metrics
        .get_counter("retries_attempted", &[("component", "retry_client")])
        .unwrap_or(0);

    let success_rate = if requests_sent > 0 {
        requests_completed as f64 / requests_sent as f64
    } else {
        0.0
    };

    RetryQueueResults {
        config,
        requests_sent,
        requests_completed,
        requests_dropped,
        timeouts,
        retries,
        success_rate,
        server_utilization: final_server.utilization(),
    }
}

#[derive(Debug)]
pub struct RetryQueueResults {
    pub config: RetryQueueConfig,
    pub requests_sent: u64,
    pub requests_completed: u64,
    pub requests_dropped: u64,
    pub timeouts: u64,
    pub retries: u64,
    pub success_rate: f64,
    pub server_utilization: f64,
}

impl RetryQueueResults {
    pub fn print_analysis(&self) {
        println!("\n=== M/M/1 with Retry Policy Results ===");
        
        println!("Configuration:");
        println!("  λ (arrival rate): {:.2} req/s", self.config.lambda);
        println!("  μ (service rate): {:.2} req/s", self.config.mu);
        println!("  Timeout: {:?}", self.config.timeout);
        println!("  Max retries: {}", self.config.max_retries);
        println!("  Base retry delay: {:?}", self.config.base_retry_delay);
        
        println!("\nPerformance Metrics:");
        println!("  Requests sent: {}", self.requests_sent);
        println!("  Requests completed: {}", self.requests_completed);
        println!("  Requests dropped: {}", self.requests_dropped);
        println!("  Timeouts: {}", self.timeouts);
        println!("  Retries attempted: {}", self.retries);
        println!("  Success rate: {:.1}%", self.success_rate * 100.0);
        println!("  Server utilization: {:.1}%", self.server_utilization * 100.0);
        
        // Analysis
        println!("\nRetry Analysis:");
        if self.retries > 0 {
            let retry_rate = self.retries as f64 / self.requests_sent as f64;
            println!("  Retry rate: {:.1}%", retry_rate * 100.0);
            
            if retry_rate > 0.5 {
                println!("  ⚠️  High retry rate - consider increasing timeout or server capacity");
            } else if retry_rate > 0.2 {
                println!("  ⚠️  Moderate retry rate - system under stress");
            } else {
                println!("  ✅ Low retry rate - system performing well");
            }
        } else {
            println!("  ✅ No retries needed - excellent performance");
        }
        
        if self.timeouts > 0 {
            let timeout_rate = self.timeouts as f64 / self.requests_sent as f64;
            println!("  Timeout rate: {:.1}%", timeout_rate * 100.0);
            
            if timeout_rate > 0.1 {
                println!("  ⚠️  High timeout rate - consider increasing timeout duration");
            }
        }
    }
}

fn main() {
    // Example 1: Well-behaved system (low load)
    let config1 = RetryQueueConfig {
        lambda: 3.0,                                    // 3 req/s
        mu: 5.0,                                        // 5 req/s capacity
        timeout: Duration::from_millis(500),            // 500ms timeout
        max_retries: 3,                                 // Up to 3 retries
        base_retry_delay: Duration::from_millis(100),   // 100ms base delay
        queue_capacity: Some(20),                       // Small queue
        duration: Duration::from_secs(30),              // 30 second simulation
    };
    
    let results1 = run_mm1_with_exponential_backoff(config1);
    results1.print_analysis();
    
    // Example 2: Overloaded system (high load)
    let config2 = RetryQueueConfig {
        lambda: 8.0,                                    // 8 req/s
        mu: 5.0,                                        // 5 req/s capacity (overloaded!)
        timeout: Duration::from_millis(300),            // Shorter timeout
        max_retries: 4,                                 // More retries
        base_retry_delay: Duration::from_millis(50),    // Shorter base delay
        queue_capacity: Some(10),                       // Very small queue
        duration: Duration::from_secs(30),              // 30 second simulation
    };
    
    let results2 = run_mm1_with_exponential_backoff(config2);
    results2.print_analysis();
    
    // Comparison
    println!("\n=== Retry Policy Impact Comparison ===");
    println!("| Scenario    | Load | Success Rate | Retry Rate | Timeout Rate |");
    println!("|-------------|------|--------------|------------|--------------|");
    println!("| Low Load    | {:.1}% | {:10.1}% | {:8.1}% | {:10.1}% |",
             (results1.config.lambda / results1.config.mu) * 100.0,
             results1.success_rate * 100.0,
             (results1.retries as f64 / results1.requests_sent as f64) * 100.0,
             (results1.timeouts as f64 / results1.requests_sent as f64) * 100.0);
    println!("| High Load   | {:.1}% | {:10.1}% | {:8.1}% | {:10.1}% |",
             (results2.config.lambda / results2.config.mu) * 100.0,
             results2.success_rate * 100.0,
             (results2.retries as f64 / results2.requests_sent as f64) * 100.0,
             (results2.timeouts as f64 / results2.requests_sent as f64) * 100.0);
}
```

## Retry Strategy Comparison

Different retry policies work better in different scenarios:

### Performance Under Load

```rust
fn compare_retry_strategies() {
    let base_config = RetryQueueConfig {
        lambda: 7.0,                                    // High load
        mu: 5.0,                                        // Limited capacity
        timeout: Duration::from_millis(400),
        max_retries: 3,
        base_retry_delay: Duration::from_millis(100),
        queue_capacity: Some(15),
        duration: Duration::from_secs(20),
    };
    
    // Test different retry policies
    println!("\n=== Retry Strategy Comparison ===");
    
    // 1. Fixed retry policy
    let fixed_results = run_with_fixed_retry(base_config.clone());
    
    // 2. Exponential backoff
    let exponential_results = run_with_exponential_backoff(base_config.clone());
    
    // 3. Token bucket
    let token_bucket_results = run_with_token_bucket(base_config.clone());
    
    // 4. Success-based adaptive
    let adaptive_results = run_with_adaptive_retry(base_config);
    
    println!("| Strategy     | Success Rate | Retry Rate | Avg Latency |");
    println!("|--------------|--------------|------------|-------------|");
    println!("| Fixed        | {:10.1}% | {:8.1}% | {:9.1}ms |",
             fixed_results.success_rate * 100.0,
             (fixed_results.retries as f64 / fixed_results.requests_sent as f64) * 100.0,
             fixed_results.avg_latency_ms);
    println!("| Exponential  | {:10.1}% | {:8.1}% | {:9.1}ms |",
             exponential_results.success_rate * 100.0,
             (exponential_results.retries as f64 / exponential_results.requests_sent as f64) * 100.0,
             exponential_results.avg_latency_ms);
    // ... (similar for other strategies)
}
```

### When to Use Each Strategy

**Fixed Retry Policy:**
- ✅ Simple, predictable behavior
- ✅ Good for testing and debugging
- ❌ Can overwhelm servers under load
- ❌ No adaptation to conditions

**Exponential Backoff:**
- ✅ Reduces server load over time
- ✅ Industry standard approach
- ✅ Jitter prevents thundering herd
- ❌ Can be slow to recover

**Token Bucket:**
- ✅ Excellent rate limiting
- ✅ Handles bursts well
- ✅ Protects downstream services
- ❌ Complex to tune parameters

**Success-Based Adaptive:**
- ✅ Automatically adapts to conditions
- ✅ Good for varying load patterns
- ✅ Self-tuning behavior
- ❌ Requires learning period

## Best Practices

### Timeout Configuration

```rust
// Choose timeouts based on expected service times
let p99_service_time = Duration::from_millis(200);
let network_overhead = Duration::from_millis(50);
let safety_margin = Duration::from_millis(100);

let timeout = p99_service_time + network_overhead + safety_margin;
// Result: 350ms timeout
```

### Retry Limits

```rust
// Limit total retry time to prevent excessive delays
let max_total_time = Duration::from_secs(5);
let base_delay = Duration::from_millis(100);

// For exponential backoff: 100ms + 200ms + 400ms + 800ms + 1600ms = 3.1s
let max_retries = 4; // Stays under 5 second limit
```

### Monitoring and Alerting

```rust
// Track key retry metrics
metrics.record_gauge("retry_rate", retry_rate, &[("service", "api")]);
metrics.record_gauge("timeout_rate", timeout_rate, &[("service", "api")]);
metrics.record_histogram("retry_delay_ms", retry_delay_ms, &[("policy", "exponential")]);

// Alert on high retry rates
if retry_rate > 0.2 {
    println!("⚠️  High retry rate detected: {:.1}%", retry_rate * 100.0);
}
```

## Running the Examples

To run the retry policy examples:

```bash
# Run comprehensive retry examples
cargo run --package des-components --example client_retry_example

# Run M/M/k with retry policies
cargo run --package des-components --example mmk_queueing_example

# Enable detailed logging
RUST_LOG=info cargo run --package des-components --example client_retry_example
```

## Next Steps

Now that you understand timeouts and retry policies:
- Learn about [admission control](admission_control.md) for proactive load management
- Explore [advanced patterns](../chapter_07/README.md) for complex scenarios
- Study [Tower integration](../chapter_06/README.md) for middleware patterns

Retry policies are essential for building resilient distributed systems. Choose the right strategy for your use case and monitor their effectiveness in production!