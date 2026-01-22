# Clients with Admission Control

Admission control is a proactive approach to managing system load by limiting the rate at which requests enter the system. Unlike reactive approaches (like retries), admission control prevents overload before it occurs. This section shows how to implement various admission control strategies in DESCARTES.

## Why Admission Control Matters

Without admission control, systems can experience:
- **Queue buildup**: Requests accumulate faster than they can be processed
- **Cascading failures**: Overloaded services affect upstream components
- **Resource exhaustion**: Memory, CPU, or network bandwidth depletion
- **Latency spikes**: Response times increase dramatically under load

Admission control provides:
- **Predictable performance**: Maintains consistent response times
- **System stability**: Prevents overload conditions
- **Graceful degradation**: Controlled behavior under high load
- **Resource protection**: Prevents resource exhaustion

## Token Bucket Algorithm

The token bucket is the most common admission control mechanism. It works like this:

1. **Bucket**: Holds a limited number of tokens
2. **Refill**: Tokens are added at a steady rate
3. **Consumption**: Each request consumes one token
4. **Admission**: Requests are admitted only if tokens are available

### Basic Token Bucket Implementation

```rust
use std::time::Duration;
use des_core::{SimTime, SchedulerHandle};

/// Token bucket for admission control
#[derive(Debug)]
pub struct TokenBucket {
    /// Maximum number of tokens
    capacity: usize,
    /// Current number of tokens
    tokens: f64,
    /// Rate at which tokens are refilled (tokens per second)
    refill_rate: f64,
    /// Last time tokens were refilled
    last_refill: SimTime,
}

impl TokenBucket {
    /// Create a new token bucket
    pub fn new(capacity: usize, refill_rate: f64) -> Self {
        Self {
            capacity,
            tokens: capacity as f64, // Start full
            refill_rate,
            last_refill: SimTime::zero(),
        }
    }

    /// Try to consume a token
    pub fn try_consume(&mut self, current_time: SimTime) -> bool {
        self.refill(current_time);
        
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Refill tokens based on elapsed time
    fn refill(&mut self, current_time: SimTime) {
        let elapsed = current_time.duration_since(self.last_refill);
        let tokens_to_add = elapsed.as_secs_f64() * self.refill_rate;
        
        self.tokens = (self.tokens + tokens_to_add).min(self.capacity as f64);
        self.last_refill = current_time;
    }

    /// Get current token count
    pub fn available_tokens(&self) -> f64 {
        self.tokens
    }

    /// Get bucket utilization (0.0 to 1.0)
    pub fn utilization(&self) -> f64 {
        1.0 - (self.tokens / self.capacity as f64)
    }
}
```

## Client-Side Admission Control

### Rate-Limited Client

Here's a client that uses token bucket admission control:

```rust
use des_components::{ClientEvent, Server, ServerEvent};
use des_core::{
    Component, Execute, Executor, Key, RequestAttempt, RequestAttemptId, RequestId, Response,
    Scheduler, SimTime, Simulation,
};
use des_metrics::SimulationMetrics;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Client with token bucket admission control
pub struct AdmissionControlClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub token_bucket: TokenBucket,
    pub request_interval: Duration,
    pub next_request_id: u64,
    pub metrics: Arc<Mutex<SimulationMetrics>>,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub requests_sent: u64,
    pub requests_admitted: u64,
    pub requests_rejected: u64,
}

impl AdmissionControlClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        request_interval: Duration,
        bucket_capacity: usize,
        refill_rate: f64,
        metrics: Arc<Mutex<SimulationMetrics>>,
    ) -> Self {
        Self {
            name,
            server_key,
            token_bucket: TokenBucket::new(bucket_capacity, refill_rate),
            request_interval,
            next_request_id: 1,
            metrics,
            request_start_times: HashMap::new(),
            requests_sent: 0,
            requests_admitted: 0,
            requests_rejected: 0,
        }
    }

    fn schedule_next_request(&mut self, scheduler: &mut Scheduler, self_key: Key<ClientEvent>) {
        scheduler.schedule(
            SimTime::from_duration(self.request_interval),
            self_key,
            ClientEvent::SendRequest,
        );
    }

    fn try_send_request(&mut self, scheduler: &mut Scheduler, self_key: Key<ClientEvent>) {
        self.requests_sent += 1;

        // Try to get admission through token bucket
        if self.token_bucket.try_consume(scheduler.time()) {
            // Admitted - send the request
            self.requests_admitted += 1;
            self.send_request(scheduler);
            
            // Record admission metrics
            {
                let mut metrics = self.metrics.lock().unwrap();
                metrics.increment_counter("requests_admitted", &[("component", &self.name)]);
                metrics.record_gauge(
                    "token_bucket_utilization",
                    self.token_bucket.utilization(),
                    &[("component", &self.name)],
                );
            }
        } else {
            // Rejected - no tokens available
            self.requests_rejected += 1;
            
            println!(
                "[{:.3}s] {} rejected request {} (no tokens available)",
                scheduler.time().as_secs_f64(),
                self.name,
                self.next_request_id
            );
            
            // Record rejection metrics
            {
                let mut metrics = self.metrics.lock().unwrap();
                metrics.increment_counter("requests_rejected", &[("component", &self.name)]);
            }
        }

        // Schedule next request regardless of admission decision
        self.schedule_next_request(scheduler, self_key);
    }

    fn send_request(&mut self, scheduler: &mut Scheduler) {
        let request_id = RequestId(self.next_request_id);
        let attempt_id = RequestAttemptId(self.next_request_id);

        // Record request start time
        self.request_start_times
            .insert(request_id, scheduler.time());

        let attempt = RequestAttempt::new(
            attempt_id,
            request_id,
            1,
            scheduler.time(),
            format!("Request {} from {}", self.next_request_id, self.name).into_bytes(),
        );

        self.next_request_id += 1;

        // Send to server
        scheduler.schedule_now(
            self.server_key,
            ServerEvent::ProcessRequest {
                attempt,
                client_id: Key::new(), // Placeholder
            },
        );

        println!(
            "[{:.3}s] {} sent request {} (tokens: {:.1})",
            scheduler.time().as_secs_f64(),
            self.name,
            request_id.0,
            self.token_bucket.available_tokens()
        );
    }

    fn handle_response(&mut self, response: &Response, scheduler: &mut Scheduler) {
        if let Some(start_time) = self.request_start_times.remove(&response.request_id) {
            let latency = scheduler.time().duration_since(start_time);
            let latency_ms = latency.as_secs_f64() * 1000.0;

            // Record response metrics
            {
                let mut metrics = self.metrics.lock().unwrap();
                if response.is_success() {
                    metrics.increment_counter("responses_success", &[("component", &self.name)]);
                } else {
                    metrics.increment_counter("responses_failure", &[("component", &self.name)]);
                }
                metrics.record_histogram("response_time_ms", latency_ms, &[("component", &self.name)]);
            }

            println!(
                "[{:.3}s] {} received response for request {} (latency: {:.2}ms)",
                scheduler.time().as_secs_f64(),
                self.name,
                response.request_id.0,
                latency_ms
            );
        }
    }
}

impl Component for AdmissionControlClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                self.try_send_request(scheduler, self_id);
            }
            ClientEvent::ResponseReceived { response } => {
                self.handle_response(response, scheduler);
            }
            _ => {}
        }
    }
}
```

## Admission Control Strategies

### 1. Fixed Rate Limiting

Simple rate limiting with a fixed token refill rate:

```rust
/// Fixed rate admission control configuration
#[derive(Debug, Clone)]
pub struct FixedRateConfig {
    pub capacity: usize,     // Bucket capacity (burst size)
    pub rate: f64,           // Tokens per second
    pub request_rate: f64,   // Client request generation rate
}

pub fn run_fixed_rate_admission_control(config: FixedRateConfig) -> AdmissionResults {
    println!("\n🔄 Running Fixed Rate Admission Control");
    println!(
        "Bucket: {} tokens, Rate: {:.1} tokens/s, Request rate: {:.1} req/s",
        config.capacity, config.rate, config.request_rate
    );

    let mut simulation = Simulation::default();
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));

    // Create server
    let server = Server::with_exponential_service_time(
        "admission_server".to_string(),
        1,
        Duration::from_millis(100), // 100ms average service time
    );
    let server_key = simulation.add_component(server);

    // Create client with admission control
    let client = AdmissionControlClient::new(
        "rate_limited_client".to_string(),
        server_key,
        Duration::from_secs_f64(1.0 / config.request_rate),
        config.capacity,
        config.rate,
        metrics.clone(),
    );
    let client_key = simulation.add_component(client);

    // Start sending requests
    simulation.schedule(SimTime::zero(), client_key, ClientEvent::SendRequest);

    // Run simulation
    let executor = Executor::timed(SimTime::from_duration(Duration::from_secs(30)));
    executor.execute(&mut simulation);

    // Collect results
    let final_client = simulation
        .remove_component::<ClientEvent, AdmissionControlClient>(client_key)
        .unwrap();

    AdmissionResults {
        config,
        requests_sent: final_client.requests_sent,
        requests_admitted: final_client.requests_admitted,
        requests_rejected: final_client.requests_rejected,
        admission_rate: final_client.requests_admitted as f64 / final_client.requests_sent as f64,
        rejection_rate: final_client.requests_rejected as f64 / final_client.requests_sent as f64,
    }
}

#[derive(Debug)]
pub struct AdmissionResults {
    pub config: FixedRateConfig,
    pub requests_sent: u64,
    pub requests_admitted: u64,
    pub requests_rejected: u64,
    pub admission_rate: f64,
    pub rejection_rate: f64,
}

impl AdmissionResults {
    pub fn print_analysis(&self) {
        println!("\n=== Admission Control Results ===");
        
        println!("Configuration:");
        println!("  Bucket capacity: {} tokens", self.config.capacity);
        println!("  Refill rate: {:.1} tokens/s", self.config.rate);
        println!("  Request rate: {:.1} req/s", self.config.request_rate);
        
        println!("\nPerformance:");
        println!("  Requests sent: {}", self.requests_sent);
        println!("  Requests admitted: {}", self.requests_admitted);
        println!("  Requests rejected: {}", self.requests_rejected);
        println!("  Admission rate: {:.1}%", self.admission_rate * 100.0);
        println!("  Rejection rate: {:.1}%", self.rejection_rate * 100.0);
        
        // Analysis
        println!("\nAnalysis:");
        let theoretical_max_rate = self.config.rate;
        let effective_rate = self.requests_admitted as f64 / 30.0; // 30 second simulation
        
        println!("  Theoretical max rate: {:.1} req/s", theoretical_max_rate);
        println!("  Effective admission rate: {:.1} req/s", effective_rate);
        
        if self.config.request_rate <= self.config.rate {
            println!("  ✅ Request rate within capacity - low rejection expected");
        } else {
            println!("  ⚠️  Request rate exceeds capacity - high rejection expected");
        }
        
        if self.rejection_rate > 0.1 {
            println!("  ⚠️  High rejection rate - consider increasing capacity or rate");
        } else {
            println!("  ✅ Low rejection rate - system performing well");
        }
    }
}
```

### 2. Success-Based Token Filling

Adjust token refill rate based on success rate:

```rust
/// Success-based admission control that adjusts token refill based on server performance
pub struct SuccessBasedAdmissionClient {
    pub base_client: AdmissionControlClient,
    pub success_window: Vec<bool>,
    pub window_size: usize,
    pub base_refill_rate: f64,
    pub min_refill_rate: f64,
    pub max_refill_rate: f64,
    pub target_success_rate: f64,
}

impl SuccessBasedAdmissionClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        request_interval: Duration,
        bucket_capacity: usize,
        base_refill_rate: f64,
        metrics: Arc<Mutex<SimulationMetrics>>,
    ) -> Self {
        Self {
            base_client: AdmissionControlClient::new(
                name,
                server_key,
                request_interval,
                bucket_capacity,
                base_refill_rate,
                metrics,
            ),
            success_window: Vec::new(),
            window_size: 20,
            base_refill_rate,
            min_refill_rate: base_refill_rate * 0.1,
            max_refill_rate: base_refill_rate * 2.0,
            target_success_rate: 0.95,
        }
    }

    fn record_response(&mut self, success: bool) {
        self.success_window.push(success);
        if self.success_window.len() > self.window_size {
            self.success_window.remove(0);
        }
        
        // Adjust refill rate based on success rate
        self.adjust_refill_rate();
    }

    fn adjust_refill_rate(&mut self) {
        if self.success_window.len() < 5 {
            return; // Need minimum data
        }

        let success_rate = self.success_window.iter().filter(|&&x| x).count() as f64
            / self.success_window.len() as f64;

        let new_rate = if success_rate < self.target_success_rate {
            // Low success rate - reduce token refill to decrease load
            let reduction_factor = success_rate / self.target_success_rate;
            (self.base_refill_rate * reduction_factor).max(self.min_refill_rate)
        } else {
            // High success rate - can increase token refill
            let increase_factor = 1.0 + (success_rate - self.target_success_rate);
            (self.base_refill_rate * increase_factor).min(self.max_refill_rate)
        };

        // Update token bucket refill rate
        self.base_client.token_bucket.refill_rate = new_rate;

        println!(
            "Adjusted refill rate: {:.2} -> {:.2} (success rate: {:.1}%)",
            self.base_refill_rate,
            new_rate,
            success_rate * 100.0
        );
    }
}

impl Component for SuccessBasedAdmissionClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::ResponseReceived { response } => {
                // Record response for adaptation
                self.record_response(response.is_success());
                
                // Delegate to base client
                self.base_client.handle_response(response, scheduler);
            }
            _ => {
                // Delegate other events to base client
                self.base_client.process_event(self_id, event, scheduler);
            }
        }
    }
}
```

### 3. Time-Based Token Filling

Adjust token refill based on time of day or load patterns:

```rust
/// Time-based admission control that varies token refill rate over time
pub struct TimeBasedAdmissionClient {
    pub base_client: AdmissionControlClient,
    pub base_refill_rate: f64,
    pub peak_hours: Vec<(f64, f64)>, // (start_hour, end_hour) pairs
    pub peak_multiplier: f64,
    pub off_peak_multiplier: f64,
}

impl TimeBasedAdmissionClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        request_interval: Duration,
        bucket_capacity: usize,
        base_refill_rate: f64,
        metrics: Arc<Mutex<SimulationMetrics>>,
    ) -> Self {
        Self {
            base_client: AdmissionControlClient::new(
                name,
                server_key,
                request_interval,
                bucket_capacity,
                base_refill_rate,
                metrics,
            ),
            base_refill_rate,
            peak_hours: vec![(9.0, 17.0)], // 9 AM to 5 PM
            peak_multiplier: 0.5,          // Reduce capacity during peak
            off_peak_multiplier: 1.5,      // Increase capacity off-peak
        }
    }

    fn update_refill_rate(&mut self, current_time: SimTime) {
        let hour_of_day = (current_time.as_secs_f64() / 3600.0) % 24.0;
        
        let is_peak = self.peak_hours.iter().any(|(start, end)| {
            hour_of_day >= *start && hour_of_day < *end
        });

        let multiplier = if is_peak {
            self.peak_multiplier
        } else {
            self.off_peak_multiplier
        };

        let new_rate = self.base_refill_rate * multiplier;
        self.base_client.token_bucket.refill_rate = new_rate;

        println!(
            "Time-based rate adjustment: {:.1}h -> {:.2} tokens/s ({})",
            hour_of_day,
            new_rate,
            if is_peak { "peak" } else { "off-peak" }
        );
    }
}

impl Component for TimeBasedAdmissionClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        // Update refill rate based on current time
        self.update_refill_rate(scheduler.time());
        
        // Delegate to base client
        self.base_client.process_event(self_id, event, scheduler);
    }
}
```

## Server-Side Admission Control

### Rate-Limited Server

Using DESCARTES Tower integration for server-side rate limiting:

```rust
use des_components::tower::{DesRateLimit, DesRateLimitLayer, DesServiceBuilder};

/// Create a rate-limited server using Tower middleware
pub fn create_rate_limited_server(
    simulation: &mut Simulation,
    name: String,
    capacity: usize,
    rate_limit: f64,
    burst_capacity: usize,
) -> Key<ServerEvent> {
    let scheduler = simulation.scheduler_handle();
    
    // Create base service
    let base_service = DesServiceBuilder::new(name.clone())
        .with_capacity(capacity)
        .with_service_time_distribution(|_| Duration::from_millis(100))
        .build(simulation)
        .unwrap();
    
    // Add rate limiting layer
    let rate_limited_service = DesRateLimit::new(
        base_service,
        rate_limit,      // requests per second
        burst_capacity,  // burst capacity
        scheduler,
    );
    
    // Convert to server component
    let server = Server::from_tower_service(name, rate_limited_service);
    simulation.add_component(server)
}

/// Example using rate-limited server
pub fn run_server_side_admission_control() {
    let mut simulation = Simulation::default();
    
    // Create rate-limited server: 5 req/s with burst of 10
    let server_key = create_rate_limited_server(
        &mut simulation,
        "rate_limited_server".to_string(),
        2,    // 2 worker threads
        5.0,  // 5 requests per second
        10,   // burst capacity of 10
    );
    
    // Create high-rate client to test rate limiting
    let client = AdmissionControlClient::new(
        "high_rate_client".to_string(),
        server_key,
        Duration::from_millis(50), // 20 req/s (exceeds server limit)
        100,                       // Large client-side bucket
        20.0,                      // High client-side rate
        Arc::new(Mutex::new(SimulationMetrics::new())),
    );
    
    let client_key = simulation.add_component(client);
    
    // Run simulation
    simulation.schedule(SimTime::zero(), client_key, ClientEvent::SendRequest);
    
    let executor = Executor::timed(SimTime::from_duration(Duration::from_secs(20)));
    executor.execute(&mut simulation);
    
    println!("Server-side rate limiting demonstration completed");
}
```

## Admission Control Comparison

### Comparing Different Strategies

```rust
fn compare_admission_strategies() {
    println!("\n=== Admission Control Strategy Comparison ===");
    
    // Test parameters
    let high_request_rate = 15.0; // 15 req/s
    let server_capacity = 8.0;    // 8 req/s
    
    // 1. No admission control (baseline)
    let no_control_results = run_no_admission_control(high_request_rate, server_capacity);
    
    // 2. Fixed rate limiting
    let fixed_rate_results = run_fixed_rate_admission_control(FixedRateConfig {
        capacity: 10,
        rate: 8.0,
        request_rate: high_request_rate,
    });
    
    // 3. Success-based adaptive
    let adaptive_results = run_success_based_admission_control(high_request_rate, server_capacity);
    
    // 4. Time-based
    let time_based_results = run_time_based_admission_control(high_request_rate, server_capacity);
    
    println!("| Strategy      | Admission Rate | Rejection Rate | Avg Latency | Server Load |");
    println!("|---------------|----------------|----------------|-------------|-------------|");
    println!("| No Control    | {:12.1}% | {:12.1}% | {:9.1}ms | {:9.1}% |",
             100.0, 0.0, no_control_results.avg_latency_ms, no_control_results.server_utilization * 100.0);
    println!("| Fixed Rate    | {:12.1}% | {:12.1}% | {:9.1}ms | {:9.1}% |",
             fixed_rate_results.admission_rate * 100.0,
             fixed_rate_results.rejection_rate * 100.0,
             fixed_rate_results.avg_latency_ms,
             fixed_rate_results.server_utilization * 100.0);
    // ... (similar for other strategies)
}
```

## Best Practices

### Choosing Bucket Parameters

```rust
/// Calculate optimal token bucket parameters
pub fn calculate_bucket_parameters(
    target_rate: f64,           // Desired sustained rate (req/s)
    burst_duration: Duration,   // How long to allow bursts
    burst_multiplier: f64,      // Burst rate as multiple of sustained rate
) -> (usize, f64) {
    let refill_rate = target_rate;
    let burst_capacity = (target_rate * burst_duration.as_secs_f64() * burst_multiplier) as usize;
    
    (burst_capacity, refill_rate)
}

// Example: Allow 10 req/s sustained, 5-second bursts at 3x rate
let (capacity, rate) = calculate_bucket_parameters(
    10.0,                           // 10 req/s sustained
    Duration::from_secs(5),         // 5 second bursts
    3.0,                            // 3x burst rate (30 req/s)
);
// Result: capacity = 150 tokens, rate = 10.0 tokens/s
```

### Monitoring Admission Control

```rust
/// Track admission control metrics
pub fn track_admission_metrics(
    metrics: &mut SimulationMetrics,
    component: &str,
    admitted: bool,
    bucket_utilization: f64,
    current_rate: f64,
) {
    if admitted {
        metrics.increment_counter("requests_admitted", &[("component", component)]);
    } else {
        metrics.increment_counter("requests_rejected", &[("component", component)]);
    }
    
    metrics.record_gauge("bucket_utilization", bucket_utilization, &[("component", component)]);
    metrics.record_gauge("current_admission_rate", current_rate, &[("component", component)]);
    
    // Alert on high rejection rates
    let total_requests = metrics.get_counter("requests_admitted", &[("component", component)]).unwrap_or(0)
                       + metrics.get_counter("requests_rejected", &[("component", component)]).unwrap_or(0);
    
    if total_requests > 100 {
        let rejected = metrics.get_counter("requests_rejected", &[("component", component)]).unwrap_or(0);
        let rejection_rate = rejected as f64 / total_requests as f64;
        
        if rejection_rate > 0.2 {
            println!("⚠️  High rejection rate for {}: {:.1}%", component, rejection_rate * 100.0);
        }
    }
}
```

## Running the Examples

To run admission control examples:

```bash
# Run comprehensive admission control examples
cargo run --package des-components --example admission_control_example

# Run Tower rate limiting examples
cargo run --package des-components --example tower_rate_limit_example

# Enable detailed logging
RUST_LOG=info cargo run --package des-components --example admission_control_example
```

## Key Takeaways

**Admission Control Benefits:**
- **Prevents overload**: Stops problems before they start
- **Predictable performance**: Maintains consistent response times
- **Resource protection**: Prevents resource exhaustion
- **Graceful degradation**: Controlled behavior under high load

**Strategy Selection:**
- **Fixed rate**: Simple, predictable, good for known workloads
- **Success-based**: Adaptive, good for varying server performance
- **Time-based**: Good for predictable load patterns
- **Server-side**: Protects server resources, enforces global limits

**Implementation Tips:**
- Start with fixed rate limiting for simplicity
- Monitor rejection rates and adjust parameters
- Use burst capacity for handling traffic spikes
- Combine client-side and server-side controls for defense in depth

## Next Steps

Now that you understand admission control:
- Explore [Tower integration](../chapter_06/README.md) for middleware patterns
- Learn [advanced examples](../chapter_07/README.md) for complex scenarios
- Study [performance analysis](../chapter_04/README.md) for optimization

Admission control is a powerful tool for building resilient systems that maintain performance under load!