//! Example demonstrating SimpleClient with different retry policies
//!
//! This example shows how to use the SimpleClient with various retry policies:
//! - ExponentialBackoffPolicy with jitter
//! - TokenBucketRetryPolicy for rate-limited retries
//! - SuccessBasedRetryPolicy for adaptive retries
//!
//! Run with: cargo run --package des-components --example client_retry_example

use des_components::{
    SimpleClient, RetryPolicy, ExponentialBackoffPolicy, 
    TokenBucketRetryPolicy, SuccessBasedRetryPolicy, ClientEvent
};
use des_core::{Simulation, SimTime, Execute, Executor, task::PeriodicTask};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== SimpleClient Retry Policy Examples ===\n");
    
    // Example 1: Exponential Backoff with Jitter
    exponential_backoff_example();
    
    // Example 2: Token Bucket Rate-Limited Retries
    token_bucket_example();
    
    // Example 3: Success-Based Adaptive Retries
    success_based_example();
    
    // Example 4: Comparison of Different Policies
    policy_comparison_example();
    
    println!("\n=== All Retry Examples Completed ===");
    Ok(())
}

fn exponential_backoff_example() {
    println!("1. Exponential Backoff with Jitter Example");
    println!("   Demonstrating exponential backoff retry policy\n");
    
    let mut sim = Simulation::default();
    
    // Create client with exponential backoff policy
    let retry_policy = ExponentialBackoffPolicy::new(4, Duration::from_millis(100))
        .with_multiplier(2.0)
        .with_max_delay(Duration::from_secs(2))
        .with_jitter(true);
    
    let client = SimpleClient::new(
        "exponential-client".to_string(),
        Duration::from_millis(500), // Send request every 500ms
        retry_policy,
    ).with_timeout(Duration::from_millis(200)); // Short timeout to trigger retries
    
    let client_id = sim.add_component(client);
    
    // Send 3 requests
    let task = PeriodicTask::with_count(
        move |scheduler| {
            scheduler.schedule_now(client_id, ClientEvent::SendRequest);
        },
        SimTime::from_duration(Duration::from_millis(500)),
        3,
    );
    sim.scheduler.schedule_task(SimTime::zero(), task);
    
    // Run simulation for 10 seconds to see retry behavior
    Executor::timed(SimTime::from_duration(Duration::from_secs(10))).execute(&mut sim);
    
    let client = sim.remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(client_id).unwrap();
    let metrics = client.get_metrics();
    
    println!("   📊 Exponential Backoff Results:");
    println!("      - Requests sent: {}", client.requests_sent);
    println!("      - Active requests: {}", client.active_requests.len());
    println!("      - Policy max attempts: {}", client.retry_policy.max_attempts());
    println!();
}

fn token_bucket_example() {
    println!("2. Token Bucket Rate-Limited Retries Example");
    println!("   Demonstrating token bucket retry policy\n");
    
    let mut sim = Simulation::default();
    
    // Create client with token bucket policy
    let retry_policy = TokenBucketRetryPolicy::new(
        5, // max attempts
        3, // max tokens
        1.0, // 1 token per second refill rate
    ).with_base_delay(Duration::from_millis(50));
    
    let client = SimpleClient::new(
        "token-bucket-client".to_string(),
        Duration::from_millis(300),
        retry_policy,
    ).with_timeout(Duration::from_millis(150));
    
    let client_id = sim.add_component(client);
    
    // Send 4 requests rapidly to test token bucket behavior
    let task = PeriodicTask::with_count(
        move |scheduler| {
            scheduler.schedule_now(client_id, ClientEvent::SendRequest);
        },
        SimTime::from_duration(Duration::from_millis(300)),
        4,
    );
    sim.scheduler.schedule_task(SimTime::zero(), task);
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_secs(8))).execute(&mut sim);
    
    let client = sim.remove_component::<ClientEvent, SimpleClient<TokenBucketRetryPolicy>>(client_id).unwrap();
    let metrics = client.get_metrics();
    
    println!("   📊 Token Bucket Results:");
    println!("      - Requests sent: {}", client.requests_sent);
    println!("      - Active requests: {}", client.active_requests.len());
    println!("      - Policy max attempts: {}", client.retry_policy.max_attempts());
    println!();
}

fn success_based_example() {
    println!("3. Success-Based Adaptive Retries Example");
    println!("   Demonstrating success-based adaptive retry policy\n");
    
    let mut sim = Simulation::default();
    
    // Create client with success-based policy
    let retry_policy = SuccessBasedRetryPolicy::new(
        4, // max attempts
        Duration::from_millis(100), // base delay
        10, // window size for success rate calculation
    ).with_min_success_rate(0.6) // 60% success rate threshold
     .with_failure_multiplier(2.5); // 2.5x delay when success rate is low
    
    let client = SimpleClient::new(
        "success-based-client".to_string(),
        Duration::from_millis(400),
        retry_policy,
    ).with_timeout(Duration::from_millis(180));
    
    let client_id = sim.add_component(client);
    
    // Send 5 requests to build up success rate history
    let task = PeriodicTask::with_count(
        move |scheduler| {
            scheduler.schedule_now(client_id, ClientEvent::SendRequest);
        },
        SimTime::from_duration(Duration::from_millis(400)),
        5,
    );
    sim.scheduler.schedule_task(SimTime::zero(), task);
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_secs(12))).execute(&mut sim);
    
    let client = sim.remove_component::<ClientEvent, SimpleClient<SuccessBasedRetryPolicy>>(client_id).unwrap();
    let metrics = client.get_metrics();
    
    println!("   📊 Success-Based Results:");
    println!("      - Requests sent: {}", client.requests_sent);
    println!("      - Active requests: {}", client.active_requests.len());
    println!("      - Policy max attempts: {}", client.retry_policy.max_attempts());
    println!();
}

fn policy_comparison_example() {
    println!("4. Policy Comparison Example");
    println!("   Comparing different retry policies side by side\n");
    
    let mut sim = Simulation::default();
    
    // Create three clients with different policies
    let exp_client = SimpleClient::with_exponential_backoff(
        "exp-compare".to_string(),
        Duration::from_millis(1000), // 1 request per second
        3,
        Duration::from_millis(100),
    ).with_timeout(Duration::from_millis(200));
    
    let token_client = SimpleClient::new(
        "token-compare".to_string(),
        Duration::from_millis(1000),
        TokenBucketRetryPolicy::new(3, 2, 0.5),
    ).with_timeout(Duration::from_millis(200));
    
    let success_client = SimpleClient::new(
        "success-compare".to_string(),
        Duration::from_millis(1000),
        SuccessBasedRetryPolicy::new(3, Duration::from_millis(100), 5),
    ).with_timeout(Duration::from_millis(200));
    
    let exp_id = sim.add_component(exp_client);
    let token_id = sim.add_component(token_client);
    let success_id = sim.add_component(success_client);
    
    // Schedule requests for all clients
    for (client_id, delay_offset) in [(exp_id, 0), (token_id, 100), (success_id, 200)] {
        let task = PeriodicTask::with_count(
            move |scheduler| {
                scheduler.schedule_now(client_id, ClientEvent::SendRequest);
            },
            SimTime::from_duration(Duration::from_millis(1000)),
            3,
        );
        sim.scheduler.schedule_task(SimTime::from_duration(Duration::from_millis(delay_offset)), task);
    }
    
    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_secs(15))).execute(&mut sim);
    
    // Collect results
    let exp_client = sim.remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(exp_id).unwrap();
    let token_client = sim.remove_component::<ClientEvent, SimpleClient<TokenBucketRetryPolicy>>(token_id).unwrap();
    let success_client = sim.remove_component::<ClientEvent, SimpleClient<SuccessBasedRetryPolicy>>(success_id).unwrap();
    
    println!("   📊 Policy Comparison Results:");
    println!("   
   | Policy          | Requests | Active | Max Attempts |
   |-----------------|----------|--------|--------------|");
    
    for (name, client_requests, active_requests, max_attempts) in [
        ("Exponential", exp_client.requests_sent, exp_client.active_requests.len(), exp_client.retry_policy.max_attempts()),
        ("Token Bucket", token_client.requests_sent, token_client.active_requests.len(), token_client.retry_policy.max_attempts()),
        ("Success-Based", success_client.requests_sent, success_client.active_requests.len(), success_client.retry_policy.max_attempts()),
    ] {
        println!(
            "   | {:15} | {:8} | {:6} | {:12} |",
            name,
            client_requests,
            active_requests,
            max_attempts,
        );
    }
    println!();
}

/// Helper function to demonstrate retry policy configuration
#[allow(dead_code)]
fn demonstrate_retry_configurations() {
    println!("Retry Policy Configuration Examples:");
    println!();
    
    println!("1. Exponential Backoff with Custom Settings:");
    println!("   ```rust");
    println!("   let policy = ExponentialBackoffPolicy::new(5, Duration::from_millis(50))");
    println!("       .with_multiplier(1.5)           // Slower growth");
    println!("       .with_max_delay(Duration::from_secs(10))  // Cap at 10s");
    println!("       .with_jitter(true);             // Add randomness");
    println!("   ```");
    println!();
    
    println!("2. Token Bucket for Rate-Limited Retries:");
    println!("   ```rust");
    println!("   let policy = TokenBucketRetryPolicy::new(3, 5, 2.0)");
    println!("       .with_base_delay(Duration::from_millis(100));");
    println!("   // 3 max attempts, 5 tokens max, 2 tokens/sec refill");
    println!("   ```");
    println!();
    
    println!("3. Success-Based Adaptive Retries:");
    println!("   ```rust");
    println!("   let policy = SuccessBasedRetryPolicy::new(4, Duration::from_millis(200), 20)");
    println!("       .with_min_success_rate(0.7)     // 70% success threshold");
    println!("       .with_failure_multiplier(3.0);  // 3x delay when failing");
    println!("   ```");
    println!();
    
    println!("4. Using with SimpleClient:");
    println!("   ```rust");
    println!("   let client = SimpleClient::new(name, interval, policy)");
    println!("       .with_timeout(Duration::from_millis(500))");
    println!("       .with_max_requests(100);");
    println!("   ```");
}