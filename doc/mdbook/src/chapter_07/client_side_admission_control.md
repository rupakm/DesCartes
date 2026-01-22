# Client-Side Admission Control Strategies

Client-side admission control prevents clients from overwhelming servers by limiting the rate at which requests are sent. This section demonstrates various client-side strategies including token bucket algorithms, success-based rate limiting, and adaptive backpressure mechanisms.

## Overview

Client-side admission control strategies include:

- **Token Bucket Algorithms**: Rate limiting based on token availability
- **Success-Based Rate Limiting**: Adjusting rate based on response success rates
- **Time-Based Rate Limiting**: Fixed or exponential backoff strategies
- **Adaptive Backpressure**: Dynamic rate adjustment based on server feedback

All examples in this section are implemented as runnable tests in `des-components/tests/advanced_examples.rs`.

## Token Bucket Algorithms

Token bucket algorithms are the most common approach to client-side rate limiting. Tokens are added to a bucket at a fixed rate, and each request consumes one token. When the bucket is empty, requests are either queued or rejected.

### Basic Token Bucket Implementation

```rust
use std::time::{Duration, Instant};

pub struct TokenBucket {
    capacity: u32,
    tokens: f64,
    refill_rate: f64, // tokens per second
    last_refill: Instant,
}

impl TokenBucket {
    pub fn new(capacity: u32, refill_rate: f64) -> Self {
        Self {
            capacity,
            tokens: capacity as f64,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    pub fn try_consume(&mut self, tokens: u32) -> bool {
        self.refill();
        
        if self.tokens >= tokens as f64 {
            self.tokens -= tokens as f64;
            true
        } else {
            false
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        
        let tokens_to_add = elapsed * self.refill_rate;
        self.tokens = (self.tokens + tokens_to_add).min(self.capacity as f64);
        self.last_refill = now;
    }

    pub fn available_tokens(&mut self) -> u32 {
        self.refill();
        self.tokens.floor() as u32
    }
}
```

### Token Bucket Client Implementation

```rust
use des_components::{ClientEvent, ServerEvent};
use des_core::{Component, Key, RequestAttempt, RequestAttemptId, RequestId, Scheduler, SimTime};

pub struct TokenBucketClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub token_bucket: TokenBucket,
    pub next_request_id: u64,
    pub pending_requests: VecDeque<PendingRequest>,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub metrics: Arc<Mutex<ClientMetrics>>,
}

#[derive(Debug)]
struct PendingRequest {
    id: RequestId,
    payload: Vec<u8>,
    created_at: SimTime,
}

impl TokenBucketClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        bucket_capacity: u32,
        refill_rate: f64,
        metrics: Arc<Mutex<ClientMetrics>>,
    ) -> Self {
        Self {
            name,
            server_key,
            token_bucket: TokenBucket::new(bucket_capacity, refill_rate),
            next_request_id: 1,
            pending_requests: VecDeque::new(),
            request_start_times: HashMap::new(),
            metrics,
        }
    }

    fn try_send_request(&mut self, scheduler: &mut Scheduler, self_key: Key<ClientEvent>) {
        // Try to send pending requests first
        while let Some(pending) = self.pending_requests.front() {
            if self.token_bucket.try_consume(1) {
                let pending = self.pending_requests.pop_front().unwrap();
                self.send_request_now(pending, scheduler, self_key);
            } else {
                break; // No tokens available
            }
        }
    }

    fn send_request_now(
        &mut self,
        pending: PendingRequest,
        scheduler: &mut Scheduler,
        self_key: Key<ClientEvent>,
    ) {
        let attempt_id = RequestAttemptId(pending.id.0);
        
        // Record start time
        self.request_start_times.insert(pending.id, scheduler.time());

        let attempt = RequestAttempt::new(
            attempt_id,
            pending.id,
            1,
            scheduler.time(),
            pending.payload,
        );

        // Record metrics
        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.requests_sent += 1;
            metrics.tokens_consumed += 1;
        }

        scheduler.schedule_now(
            self.server_key,
            ServerEvent::ProcessRequest {
                attempt,
                client_id: self_key,
            },
        );

        println!(
            "[{}] [{}] Sent request {} (tokens available: {})",
            scheduler.time().as_duration().as_millis(),
            self.name,
            pending.id.0,
            self.token_bucket.available_tokens()
        );
    }

    fn queue_request(&mut self, payload: Vec<u8>, scheduler: &mut Scheduler) {
        let request_id = RequestId(self.next_request_id);
        self.next_request_id += 1;

        let pending = PendingRequest {
            id: request_id,
            payload,
            created_at: scheduler.time(),
        };

        self.pending_requests.push_back(pending);

        // Record metrics
        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.requests_queued += 1;
        }

        println!(
            "[{}] [{}] Queued request {} (no tokens available)",
            scheduler.time().as_duration().as_millis(),
            self.name,
            request_id.0
        );
    }
}

impl Component for TokenBucketClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                // Try to send any pending requests first
                self.try_send_request(scheduler, self_id);
                
                // Create new request
                let payload = format!("Request {}", self.next_request_id).into_bytes();
                
                if self.token_bucket.try_consume(1) {
                    // Send immediately
                    let request_id = RequestId(self.next_request_id);
                    self.next_request_id += 1;
                    
                    let pending = PendingRequest {
                        id: request_id,
                        payload,
                        created_at: scheduler.time(),
                    };
                    
                    self.send_request_now(pending, scheduler, self_id);
                } else {
                    // Queue for later
                    self.queue_request(payload, scheduler);
                }
                
                // Schedule token refill check
                scheduler.schedule(
                    SimTime::from_duration(Duration::from_millis(100)),
                    self_id,
                    ClientEvent::SendRequest,
                );
            }
            ClientEvent::ResponseReceived { response } => {
                if let Some(start_time) = self.request_start_times.remove(&response.request_id) {
                    let latency = scheduler.time().duration_since(start_time);
                    
                    // Record metrics
                    {
                        let mut metrics = self.metrics.lock().unwrap();
                        metrics.responses_received += 1;
                        if response.is_success() {
                            metrics.successful_responses += 1;
                        }
                        metrics.total_latency += latency;
                    }
                    
                    println!(
                        "[{}] [{}] Response for request {} ({}ms, success: {})",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        response.request_id.0,
                        latency.as_millis(),
                        response.is_success()
                    );
                }
                
                // Try to send pending requests when we get a response
                self.try_send_request(scheduler, self_id);
            }
            _ => {}
        }
    }
}
```

## Success-Based Rate Limiting

Success-based rate limiting adjusts the request rate based on the success rate of recent responses. When success rates drop, the client reduces its request rate to give the server time to recover.

### Success-Based Client Implementation

```rust
pub struct SuccessBasedClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub current_rate: f64, // requests per second
    pub min_rate: f64,
    pub max_rate: f64,
    pub success_window: VecDeque<bool>, // Recent success/failure history
    pub window_size: usize,
    pub adjustment_factor: f64,
    pub next_request_id: u64,
    pub last_request_time: Option<SimTime>,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub metrics: Arc<Mutex<ClientMetrics>>,
}

impl SuccessBasedClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        initial_rate: f64,
        min_rate: f64,
        max_rate: f64,
        window_size: usize,
        adjustment_factor: f64,
        metrics: Arc<Mutex<ClientMetrics>>,
    ) -> Self {
        Self {
            name,
            server_key,
            current_rate: initial_rate,
            min_rate,
            max_rate,
            success_window: VecDeque::new(),
            window_size,
            adjustment_factor,
            next_request_id: 1,
            last_request_time: None,
            request_start_times: HashMap::new(),
            metrics,
        }
    }

    fn update_rate_based_on_success(&mut self) {
        if self.success_window.len() < self.window_size {
            return; // Not enough data yet
        }

        let success_count = self.success_window.iter().filter(|&&success| success).count();
        let success_rate = success_count as f64 / self.success_window.len() as f64;

        let old_rate = self.current_rate;

        if success_rate >= 0.95 {
            // High success rate - increase rate
            self.current_rate = (self.current_rate * (1.0 + self.adjustment_factor))
                .min(self.max_rate);
        } else if success_rate <= 0.8 {
            // Low success rate - decrease rate aggressively
            self.current_rate = (self.current_rate * (1.0 - self.adjustment_factor * 2.0))
                .max(self.min_rate);
        } else if success_rate <= 0.9 {
            // Moderate success rate - decrease rate slightly
            self.current_rate = (self.current_rate * (1.0 - self.adjustment_factor))
                .max(self.min_rate);
        }

        if (self.current_rate - old_rate).abs() > 0.01 {
            println!(
                "[{}] Rate adjustment: {:.2} -> {:.2} RPS (success rate: {:.1}%)",
                self.name,
                old_rate,
                self.current_rate,
                success_rate * 100.0
            );
        }
    }

    fn should_send_request(&self, current_time: SimTime) -> bool {
        if let Some(last_time) = self.last_request_time {
            let elapsed = current_time.duration_since(last_time);
            let required_interval = Duration::from_secs_f64(1.0 / self.current_rate);
            elapsed >= required_interval
        } else {
            true // First request
        }
    }

    fn record_response(&mut self, success: bool) {
        self.success_window.push_back(success);
        
        // Keep window at fixed size
        while self.success_window.len() > self.window_size {
            self.success_window.pop_front();
        }
        
        self.update_rate_based_on_success();
    }
}

impl Component for SuccessBasedClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                if self.should_send_request(scheduler.time()) {
                    let request_id = RequestId(self.next_request_id);
                    let attempt_id = RequestAttemptId(self.next_request_id);
                    self.next_request_id += 1;

                    // Record start time
                    self.request_start_times.insert(request_id, scheduler.time());
                    self.last_request_time = Some(scheduler.time());

                    let attempt = RequestAttempt::new(
                        attempt_id,
                        request_id,
                        1,
                        scheduler.time(),
                        format!("Success-based request {}", request_id.0).into_bytes(),
                    );

                    // Record metrics
                    {
                        let mut metrics = self.metrics.lock().unwrap();
                        metrics.requests_sent += 1;
                    }

                    scheduler.schedule_now(
                        self.server_key,
                        ServerEvent::ProcessRequest {
                            attempt,
                            client_id: self_id,
                        },
                    );

                    println!(
                        "[{}] [{}] Sent request {} at {:.2} RPS",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        request_id.0,
                        self.current_rate
                    );
                }

                // Schedule next attempt
                let next_interval = Duration::from_secs_f64(1.0 / self.current_rate);
                scheduler.schedule(
                    SimTime::from_duration(next_interval),
                    self_id,
                    ClientEvent::SendRequest,
                );
            }
            ClientEvent::ResponseReceived { response } => {
                if let Some(start_time) = self.request_start_times.remove(&response.request_id) {
                    let latency = scheduler.time().duration_since(start_time);
                    let success = response.is_success();

                    // Record response for rate adjustment
                    self.record_response(success);

                    // Record metrics
                    {
                        let mut metrics = self.metrics.lock().unwrap();
                        metrics.responses_received += 1;
                        if success {
                            metrics.successful_responses += 1;
                        }
                        metrics.total_latency += latency;
                    }

                    println!(
                        "[{}] [{}] Response for request {} ({}ms, success: {})",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        response.request_id.0,
                        latency.as_millis(),
                        success
                    );
                }
            }
            _ => {}
        }
    }
}
```

## Time-Based Rate Limiting

Time-based rate limiting uses fixed intervals or exponential backoff to control request timing, often in response to failures or timeouts.

### Exponential Backoff Client

```rust
pub struct ExponentialBackoffClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub base_interval: Duration,
    pub max_interval: Duration,
    pub current_interval: Duration,
    pub backoff_multiplier: f64,
    pub consecutive_failures: u32,
    pub max_failures_before_backoff: u32,
    pub next_request_id: u64,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub metrics: Arc<Mutex<ClientMetrics>>,
}

impl ExponentialBackoffClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        base_interval: Duration,
        max_interval: Duration,
        backoff_multiplier: f64,
        max_failures_before_backoff: u32,
        metrics: Arc<Mutex<ClientMetrics>>,
    ) -> Self {
        Self {
            name,
            server_key,
            base_interval,
            max_interval,
            current_interval: base_interval,
            backoff_multiplier,
            consecutive_failures: 0,
            max_failures_before_backoff,
            next_request_id: 1,
            request_start_times: HashMap::new(),
            metrics,
        }
    }

    fn handle_success(&mut self) {
        self.consecutive_failures = 0;
        self.current_interval = self.base_interval;
        
        println!(
            "[{}] Success - reset interval to {:?}",
            self.name,
            self.current_interval
        );
    }

    fn handle_failure(&mut self) {
        self.consecutive_failures += 1;
        
        if self.consecutive_failures >= self.max_failures_before_backoff {
            let old_interval = self.current_interval;
            self.current_interval = Duration::from_secs_f64(
                (self.current_interval.as_secs_f64() * self.backoff_multiplier)
                    .min(self.max_interval.as_secs_f64())
            );
            
            println!(
                "[{}] Failure #{} - backing off: {:?} -> {:?}",
                self.name,
                self.consecutive_failures,
                old_interval,
                self.current_interval
            );
        }
    }
}

impl Component for ExponentialBackoffClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                let request_id = RequestId(self.next_request_id);
                let attempt_id = RequestAttemptId(self.next_request_id);
                self.next_request_id += 1;

                // Record start time
                self.request_start_times.insert(request_id, scheduler.time());

                let attempt = RequestAttempt::new(
                    attempt_id,
                    request_id,
                    1,
                    scheduler.time(),
                    format!("Backoff request {}", request_id.0).into_bytes(),
                );

                // Record metrics
                {
                    let mut metrics = self.metrics.lock().unwrap();
                    metrics.requests_sent += 1;
                }

                scheduler.schedule_now(
                    self.server_key,
                    ServerEvent::ProcessRequest {
                        attempt,
                        client_id: self_id,
                    },
                );

                println!(
                    "[{}] [{}] Sent request {} (interval: {:?})",
                    scheduler.time().as_duration().as_millis(),
                    self.name,
                    request_id.0,
                    self.current_interval
                );

                // Schedule next request
                scheduler.schedule(
                    SimTime::from_duration(self.current_interval),
                    self_id,
                    ClientEvent::SendRequest,
                );
            }
            ClientEvent::ResponseReceived { response } => {
                if let Some(start_time) = self.request_start_times.remove(&response.request_id) {
                    let latency = scheduler.time().duration_since(start_time);
                    let success = response.is_success();

                    // Update backoff based on response
                    if success {
                        self.handle_success();
                    } else {
                        self.handle_failure();
                    }

                    // Record metrics
                    {
                        let mut metrics = self.metrics.lock().unwrap();
                        metrics.responses_received += 1;
                        if success {
                            metrics.successful_responses += 1;
                        }
                        metrics.total_latency += latency;
                    }

                    println!(
                        "[{}] [{}] Response for request {} ({}ms, success: {})",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        response.request_id.0,
                        latency.as_millis(),
                        success
                    );
                }
            }
            _ => {}
        }
    }
}
```

## Adaptive Backpressure

Adaptive backpressure adjusts client behavior based on server-side signals like queue depth, response times, or explicit backpressure headers.

### Latency-Based Adaptive Client

```rust
pub struct LatencyBasedAdaptiveClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub target_latency: Duration,
    pub current_rate: f64,
    pub min_rate: f64,
    pub max_rate: f64,
    pub latency_window: VecDeque<Duration>,
    pub window_size: usize,
    pub adjustment_factor: f64,
    pub next_request_id: u64,
    pub last_request_time: Option<SimTime>,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub metrics: Arc<Mutex<ClientMetrics>>,
}

impl LatencyBasedAdaptiveClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        target_latency: Duration,
        initial_rate: f64,
        min_rate: f64,
        max_rate: f64,
        window_size: usize,
        adjustment_factor: f64,
        metrics: Arc<Mutex<ClientMetrics>>,
    ) -> Self {
        Self {
            name,
            server_key,
            target_latency,
            current_rate: initial_rate,
            min_rate,
            max_rate,
            latency_window: VecDeque::new(),
            window_size,
            adjustment_factor,
            next_request_id: 1,
            last_request_time: None,
            request_start_times: HashMap::new(),
            metrics,
        }
    }

    fn update_rate_based_on_latency(&mut self) {
        if self.latency_window.len() < self.window_size {
            return; // Not enough data
        }

        let avg_latency = self.latency_window.iter().sum::<Duration>() / self.latency_window.len() as u32;
        let old_rate = self.current_rate;

        if avg_latency < self.target_latency {
            // Latency is good - can increase rate
            let headroom = self.target_latency.saturating_sub(avg_latency);
            let headroom_ratio = headroom.as_secs_f64() / self.target_latency.as_secs_f64();
            
            if headroom_ratio > 0.2 {
                // Significant headroom - increase rate
                self.current_rate = (self.current_rate * (1.0 + self.adjustment_factor))
                    .min(self.max_rate);
            }
        } else {
            // Latency is too high - decrease rate
            let overage = avg_latency.saturating_sub(self.target_latency);
            let overage_ratio = overage.as_secs_f64() / self.target_latency.as_secs_f64();
            
            let reduction_factor = if overage_ratio > 1.0 {
                // Very high latency - aggressive reduction
                self.adjustment_factor * 2.0
            } else {
                // Moderate latency - normal reduction
                self.adjustment_factor
            };
            
            self.current_rate = (self.current_rate * (1.0 - reduction_factor))
                .max(self.min_rate);
        }

        if (self.current_rate - old_rate).abs() > 0.01 {
            println!(
                "[{}] Latency-based adjustment: {:.2} -> {:.2} RPS (avg latency: {}ms, target: {}ms)",
                self.name,
                old_rate,
                self.current_rate,
                avg_latency.as_millis(),
                self.target_latency.as_millis()
            );
        }
    }

    fn record_latency(&mut self, latency: Duration) {
        self.latency_window.push_back(latency);
        
        // Keep window at fixed size
        while self.latency_window.len() > self.window_size {
            self.latency_window.pop_front();
        }
        
        self.update_rate_based_on_latency();
    }
}

impl Component for LatencyBasedAdaptiveClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                let should_send = if let Some(last_time) = self.last_request_time {
                    let elapsed = scheduler.time().duration_since(last_time);
                    let required_interval = Duration::from_secs_f64(1.0 / self.current_rate);
                    elapsed >= required_interval
                } else {
                    true // First request
                };

                if should_send {
                    let request_id = RequestId(self.next_request_id);
                    let attempt_id = RequestAttemptId(self.next_request_id);
                    self.next_request_id += 1;

                    // Record start time
                    self.request_start_times.insert(request_id, scheduler.time());
                    self.last_request_time = Some(scheduler.time());

                    let attempt = RequestAttempt::new(
                        attempt_id,
                        request_id,
                        1,
                        scheduler.time(),
                        format!("Adaptive request {}", request_id.0).into_bytes(),
                    );

                    // Record metrics
                    {
                        let mut metrics = self.metrics.lock().unwrap();
                        metrics.requests_sent += 1;
                    }

                    scheduler.schedule_now(
                        self.server_key,
                        ServerEvent::ProcessRequest {
                            attempt,
                            client_id: self_id,
                        },
                    );

                    println!(
                        "[{}] [{}] Sent request {} at {:.2} RPS",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        request_id.0,
                        self.current_rate
                    );
                }

                // Schedule next attempt
                let next_interval = Duration::from_secs_f64(1.0 / self.current_rate);
                scheduler.schedule(
                    SimTime::from_duration(next_interval),
                    self_id,
                    ClientEvent::SendRequest,
                );
            }
            ClientEvent::ResponseReceived { response } => {
                if let Some(start_time) = self.request_start_times.remove(&response.request_id) {
                    let latency = scheduler.time().duration_since(start_time);
                    let success = response.is_success();

                    // Record latency for rate adjustment
                    if success {
                        self.record_latency(latency);
                    }

                    // Record metrics
                    {
                        let mut metrics = self.metrics.lock().unwrap();
                        metrics.responses_received += 1;
                        if success {
                            metrics.successful_responses += 1;
                        }
                        metrics.total_latency += latency;
                    }

                    println!(
                        "[{}] [{}] Response for request {} ({}ms, success: {})",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        response.request_id.0,
                        latency.as_millis(),
                        success
                    );
                }
            }
            _ => {}
        }
    }
}
```

## Performance Analysis

### Comparing Admission Control Strategies

```rust
#[derive(Debug, Clone)]
pub struct AdmissionControlResults {
    pub strategy: String,
    pub requests_sent: u64,
    pub requests_completed: u64,
    pub success_rate: f64,
    pub average_latency_ms: f64,
    pub throughput_rps: f64,
    pub tokens_consumed: Option<u64>,
    pub rate_adjustments: Option<u32>,
}

pub fn compare_admission_control_strategies() -> Vec<AdmissionControlResults> {
    let strategies = vec![
        ("Token Bucket", run_token_bucket_simulation()),
        ("Success-Based", run_success_based_simulation()),
        ("Exponential Backoff", run_backoff_simulation()),
        ("Latency-Adaptive", run_adaptive_simulation()),
    ];

    strategies.into_iter().map(|(name, result)| {
        AdmissionControlResults {
            strategy: name.to_string(),
            requests_sent: result.requests_sent,
            requests_completed: result.requests_completed,
            success_rate: result.success_rate,
            average_latency_ms: result.average_latency_ms,
            throughput_rps: result.throughput_rps,
            tokens_consumed: result.tokens_consumed,
            rate_adjustments: result.rate_adjustments,
        }
    }).collect()
}

pub fn print_comparison_results(results: &[AdmissionControlResults]) {
    println!("\n=== Admission Control Strategy Comparison ===");
    println!("{:<20} {:<10} {:<10} {:<12} {:<12} {:<10}", 
             "Strategy", "Sent", "Completed", "Success %", "Avg Lat (ms)", "RPS");
    println!("{}", "-".repeat(80));

    for result in results {
        println!("{:<20} {:<10} {:<10} {:<12.1} {:<12.1} {:<10.1}",
                 result.strategy,
                 result.requests_sent,
                 result.requests_completed,
                 result.success_rate * 100.0,
                 result.average_latency_ms,
                 result.throughput_rps);
    }

    // Analysis
    println!("\n=== Strategy Analysis ===");
    
    let best_throughput = results.iter()
        .max_by(|a, b| a.throughput_rps.partial_cmp(&b.throughput_rps).unwrap())
        .unwrap();
    println!("🚀 Best Throughput: {} ({:.1} RPS)", 
             best_throughput.strategy, best_throughput.throughput_rps);

    let best_latency = results.iter()
        .min_by(|a, b| a.average_latency_ms.partial_cmp(&b.average_latency_ms).unwrap())
        .unwrap();
    println!("⚡ Best Latency: {} ({:.1}ms)", 
             best_latency.strategy, best_latency.average_latency_ms);

    let best_success = results.iter()
        .max_by(|a, b| a.success_rate.partial_cmp(&b.success_rate).unwrap())
        .unwrap();
    println!("✅ Best Success Rate: {} ({:.1}%)", 
             best_success.strategy, best_success.success_rate * 100.0);
}
```

## Testing Strategies

### Unit Tests for Admission Control

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_bucket_basic_functionality() {
        let mut bucket = TokenBucket::new(10, 5.0); // 10 tokens, 5 tokens/sec

        // Should have full capacity initially
        assert_eq!(bucket.available_tokens(), 10);

        // Consume some tokens
        assert!(bucket.try_consume(5));
        assert_eq!(bucket.available_tokens(), 5);

        // Try to consume more than available
        assert!(!bucket.try_consume(10));
        assert_eq!(bucket.available_tokens(), 5);

        // Consume remaining tokens
        assert!(bucket.try_consume(5));
        assert_eq!(bucket.available_tokens(), 0);
    }

    #[test]
    fn test_token_bucket_refill() {
        let mut bucket = TokenBucket::new(10, 10.0); // 10 tokens/sec

        // Consume all tokens
        assert!(bucket.try_consume(10));
        assert_eq!(bucket.available_tokens(), 0);

        // Wait and check refill (simulated)
        std::thread::sleep(Duration::from_millis(500)); // 0.5 seconds
        assert!(bucket.available_tokens() >= 4); // Should have ~5 tokens
    }

    #[test]
    fn test_success_based_rate_adjustment() {
        let metrics = Arc::new(Mutex::new(ClientMetrics::default()));
        let mut client = SuccessBasedClient::new(
            "test".to_string(),
            Key::new(0), // dummy key
            10.0, // initial rate
            1.0,  // min rate
            20.0, // max rate
            5,    // window size
            0.1,  // adjustment factor
            metrics,
        );

        // Record high success rate
        for _ in 0..5 {
            client.record_response(true);
        }
        assert!(client.current_rate > 10.0); // Should increase

        // Record low success rate
        for _ in 0..5 {
            client.record_response(false);
        }
        assert!(client.current_rate < 10.0); // Should decrease
    }
}
```

### Integration Tests

```rust
#[test]
fn test_token_bucket_client_integration() {
    let mut simulation = Simulation::default();
    let metrics = Arc::new(Mutex::new(ClientMetrics::default()));

    // Create server
    let server = Server::with_constant_service_time(
        "test-server".to_string(),
        2,
        Duration::from_millis(50),
    );
    let server_key = simulation.add_component(server);

    // Create token bucket client (5 tokens, 2 tokens/sec)
    let client = TokenBucketClient::new(
        "token-client".to_string(),
        server_key,
        5,   // capacity
        2.0, // refill rate
        metrics.clone(),
    );
    let client_key = simulation.add_component(client);

    // Start client
    simulation.schedule(
        SimTime::zero(),
        client_key,
        ClientEvent::SendRequest,
    );

    // Run simulation
    Executor::timed(SimTime::from_duration(Duration::from_secs(10)))
        .execute(&mut simulation);

    // Verify rate limiting worked
    let final_metrics = metrics.lock().unwrap();
    let expected_max_requests = 10 * 2 + 5; // 10 seconds * 2 RPS + initial tokens
    assert!(final_metrics.requests_sent <= expected_max_requests);
}
```

## Best Practices

### Strategy Selection Guidelines

1. **Token Bucket**: Best for consistent rate limiting with burst capacity
   - Use when you need predictable rate limits
   - Good for protecting against traffic spikes
   - Simple to implement and understand

2. **Success-Based**: Best for adaptive systems with variable server capacity
   - Use when server capacity varies over time
   - Good for handling degraded server performance
   - Requires tuning of success rate thresholds

3. **Exponential Backoff**: Best for handling temporary failures
   - Use when failures are typically transient
   - Good for retry scenarios
   - Simple but can be slow to recover

4. **Latency-Adaptive**: Best for latency-sensitive applications
   - Use when latency is more important than throughput
   - Good for real-time systems
   - Requires careful tuning of target latency

### Configuration Guidelines

```rust
// Token Bucket Configuration
let token_bucket_config = TokenBucketConfig {
    capacity: server_capacity * 2,        // Allow some burst
    refill_rate: server_capacity * 0.8,   // Stay below server capacity
};

// Success-Based Configuration
let success_based_config = SuccessBasedConfig {
    window_size: 20,                      // Enough samples for stability
    adjustment_factor: 0.1,               // Conservative adjustments
    min_rate: server_capacity * 0.1,      // Always maintain some traffic
    max_rate: server_capacity * 1.5,      // Don't exceed server capacity too much
};

// Latency-Adaptive Configuration
let adaptive_config = AdaptiveConfig {
    target_latency: Duration::from_millis(100), // Application requirement
    window_size: 10,                            // Recent samples only
    adjustment_factor: 0.05,                    // Very conservative
};
```

### Monitoring and Observability

```rust
pub struct AdmissionControlMetrics {
    pub requests_sent: Counter,
    pub requests_queued: Counter,
    pub tokens_consumed: Counter,
    pub rate_adjustments: Counter,
    pub current_rate: Gauge,
    pub queue_depth: Gauge,
    pub success_rate: Histogram,
    pub latency_distribution: Histogram,
}

impl AdmissionControlMetrics {
    pub fn record_rate_adjustment(&mut self, old_rate: f64, new_rate: f64, reason: &str) {
        self.rate_adjustments.increment();
        self.current_rate.set(new_rate);
        
        println!("Rate adjustment: {:.2} -> {:.2} RPS (reason: {})", 
                 old_rate, new_rate, reason);
    }

    pub fn record_request_queued(&mut self, queue_depth: usize) {
        self.requests_queued.increment();
        self.queue_depth.set(queue_depth as f64);
    }
}
```

## Real-World Applications

### Microservices Communication

```rust
// Configure different strategies for different service types
let user_service_client = TokenBucketClient::new(
    "user-service-client".to_string(),
    user_service_key,
    100,  // High capacity for user operations
    50.0, // 50 RPS sustained rate
    metrics.clone(),
);

let analytics_service_client = SuccessBasedClient::new(
    "analytics-service-client".to_string(),
    analytics_service_key,
    5.0,  // Start conservative
    0.5,  // Very low minimum
    20.0, // Reasonable maximum
    10,   // Larger window for stability
    0.2,  // More aggressive adjustments
    metrics.clone(),
);
```

### API Gateway Integration

```rust
// Different strategies per API endpoint
let api_clients = HashMap::from([
    ("/api/critical", create_token_bucket_client(100.0)),
    ("/api/batch", create_success_based_client(10.0)),
    ("/api/analytics", create_adaptive_client(Duration::from_millis(500))),
]);
```

### Load Testing Scenarios

```rust
// Gradually increase load to test admission control
let load_test_client = VariableRateClient::new(
    "load-test".to_string(),
    server_key,
    vec![
        (Duration::from_secs(60), 10.0),   // Warm up
        (Duration::from_secs(120), 50.0),  // Normal load
        (Duration::from_secs(60), 100.0),  // High load
        (Duration::from_secs(60), 200.0),  // Overload
        (Duration::from_secs(120), 50.0),  // Recovery
    ],
    metrics,
);
```

## Next Steps

Continue to [Server-Side Throttling](./server_side_throttling.md) to learn about implementing server-side admission control and load shedding strategies.