# Server-Side Throttling and Admission Control

Server-side throttling and admission control protect servers from overload by rejecting or delaying requests before they consume server resources. This section demonstrates various server-side strategies including token buckets, Random Early Detection (RED), queue-size based policies, and circuit breakers.

## Overview

Server-side admission control strategies include:

- **Token Bucket Rate Limiting**: Server-side rate limiting with token buckets
- **Queue-Size Based Admission**: Rejecting requests when queues are full
- **Random Early Detection (RED)**: Probabilistic dropping based on queue depth
- **Circuit Breaker Patterns**: Failing fast when server is overloaded
- **Load Shedding**: Dropping lower-priority requests under load

All examples in this section are implemented as runnable tests in `des-components/tests/advanced_examples.rs`.

## Server-Side Token Bucket Rate Limiting

Unlike client-side token buckets, server-side token buckets protect the server by rejecting requests that exceed the configured rate limit, regardless of client behavior.

### Token Bucket Server Implementation

```rust
use des_components::{ServerEvent, Server};
use des_core::{Component, Key, RequestAttempt, Response, Scheduler, SimTime};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct TokenBucketServer {
    pub inner_server: Server,
    pub token_bucket: ServerTokenBucket,
    pub metrics: Arc<Mutex<ServerMetrics>>,
    pub rejected_requests: u64,
}

pub struct ServerTokenBucket {
    capacity: u32,
    tokens: f64,
    refill_rate: f64, // tokens per second
    last_refill: SimTime,
}

impl ServerTokenBucket {
    pub fn new(capacity: u32, refill_rate: f64) -> Self {
        Self {
            capacity,
            tokens: capacity as f64,
            refill_rate,
            last_refill: SimTime::zero(),
        }
    }

    pub fn try_consume(&mut self, current_time: SimTime, tokens: u32) -> bool {
        self.refill(current_time);
        
        if self.tokens >= tokens as f64 {
            self.tokens -= tokens as f64;
            true
        } else {
            false
        }
    }

    fn refill(&mut self, current_time: SimTime) {
        if self.last_refill == SimTime::zero() {
            self.last_refill = current_time;
            return;
        }

        let elapsed = current_time.duration_since(self.last_refill).as_secs_f64();
        let tokens_to_add = elapsed * self.refill_rate;
        self.tokens = (self.tokens + tokens_to_add).min(self.capacity as f64);
        self.last_refill = current_time;
    }

    pub fn available_tokens(&mut self, current_time: SimTime) -> u32 {
        self.refill(current_time);
        self.tokens.floor() as u32
    }
}

impl TokenBucketServer {
    pub fn new(
        name: String,
        capacity: usize,
        service_time: Duration,
        bucket_capacity: u32,
        refill_rate: f64,
        metrics: Arc<Mutex<ServerMetrics>>,
    ) -> Self {
        Self {
            inner_server: Server::with_constant_service_time(name, capacity, service_time),
            token_bucket: ServerTokenBucket::new(bucket_capacity, refill_rate),
            metrics,
            rejected_requests: 0,
        }
    }

    fn should_accept_request(&mut self, scheduler: &mut Scheduler) -> bool {
        self.token_bucket.try_consume(scheduler.time(), 1)
    }

    fn reject_request(&mut self, attempt: &RequestAttempt, client_id: Key<ClientEvent>, scheduler: &mut Scheduler) {
        self.rejected_requests += 1;

        // Record rejection metrics
        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.requests_rejected += 1;
            metrics.rate_limited_requests += 1;
        }

        // Send rejection response
        let response = Response::new(
            attempt.request_id,
            false, // failure
            format!("Rate limited - {} tokens available", 
                   self.token_bucket.available_tokens(scheduler.time())).into_bytes(),
        );

        scheduler.schedule_now(client_id, ClientEvent::ResponseReceived { response });

        println!(
            "[{}] [{}] Rate limited request {} (tokens: {})",
            scheduler.time().as_duration().as_millis(),
            self.inner_server.name,
            attempt.id.0,
            self.token_bucket.available_tokens(scheduler.time())
        );
    }
}

impl Component for TokenBucketServer {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ServerEvent::ProcessRequest { attempt, client_id } => {
                if self.should_accept_request(scheduler) {
                    // Accept request - delegate to inner server
                    self.inner_server.process_event(self_id, event, scheduler);
                    
                    println!(
                        "[{}] [{}] Accepted request {} (tokens: {})",
                        scheduler.time().as_duration().as_millis(),
                        self.inner_server.name,
                        attempt.id.0,
                        self.token_bucket.available_tokens(scheduler.time())
                    );
                } else {
                    // Reject request due to rate limiting
                    self.reject_request(attempt, *client_id, scheduler);
                }
            }
            _ => {
                // Delegate other events to inner server
                self.inner_server.process_event(self_id, event, scheduler);
            }
        }
    }
}
```

## Queue-Size Based Admission Control

Queue-size based admission control rejects requests when the server's queue reaches a certain threshold, preventing unbounded queue growth.

### Queue-Size Admission Server

```rust
pub struct QueueSizeAdmissionServer {
    pub inner_server: Server,
    pub max_queue_size: usize,
    pub admission_threshold: f64, // Fraction of max queue size (0.0-1.0)
    pub metrics: Arc<Mutex<ServerMetrics>>,
    pub rejected_requests: u64,
}

impl QueueSizeAdmissionServer {
    pub fn new(
        name: String,
        capacity: usize,
        service_time: Duration,
        max_queue_size: usize,
        admission_threshold: f64,
        metrics: Arc<Mutex<ServerMetrics>>,
    ) -> Self {
        let server = Server::with_constant_service_time(name, capacity, service_time)
            .with_queue(Box::new(FifoQueue::bounded(max_queue_size)));

        Self {
            inner_server: server,
            max_queue_size,
            admission_threshold,
            metrics,
            rejected_requests: 0,
        }
    }

    fn should_accept_request(&self) -> bool {
        let current_queue_size = self.inner_server.queue_depth();
        let threshold_size = (self.max_queue_size as f64 * self.admission_threshold) as usize;
        
        current_queue_size < threshold_size
    }

    fn reject_request(&mut self, attempt: &RequestAttempt, client_id: Key<ClientEvent>, scheduler: &mut Scheduler) {
        self.rejected_requests += 1;

        // Record rejection metrics
        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.requests_rejected += 1;
            metrics.queue_full_rejections += 1;
        }

        // Send rejection response
        let response = Response::new(
            attempt.request_id,
            false, // failure
            format!("Queue full - current depth: {}/{}", 
                   self.inner_server.queue_depth(), self.max_queue_size).into_bytes(),
        );

        scheduler.schedule_now(client_id, ClientEvent::ResponseReceived { response });

        println!(
            "[{}] [{}] Queue-based rejection for request {} (queue: {}/{})",
            scheduler.time().as_duration().as_millis(),
            self.inner_server.name,
            attempt.id.0,
            self.inner_server.queue_depth(),
            self.max_queue_size
        );
    }
}

impl Component for QueueSizeAdmissionServer {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ServerEvent::ProcessRequest { attempt, client_id } => {
                if self.should_accept_request() {
                    // Accept request - delegate to inner server
                    self.inner_server.process_event(self_id, event, scheduler);
                    
                    println!(
                        "[{}] [{}] Accepted request {} (queue: {}/{})",
                        scheduler.time().as_duration().as_millis(),
                        self.inner_server.name,
                        attempt.id.0,
                        self.inner_server.queue_depth(),
                        self.max_queue_size
                    );
                } else {
                    // Reject request due to queue size
                    self.reject_request(attempt, *client_id, scheduler);
                }
            }
            _ => {
                // Delegate other events to inner server
                self.inner_server.process_event(self_id, event, scheduler);
            }
        }
    }
}
```

## Random Early Detection (RED)

Random Early Detection probabilistically drops requests based on queue depth, providing smoother degradation than hard queue limits.

### RED Admission Server

```rust
use rand::Rng;

pub struct RedAdmissionServer {
    pub inner_server: Server,
    pub min_threshold: usize,    // Queue size where dropping starts
    pub max_threshold: usize,    // Queue size where all requests are dropped
    pub max_drop_probability: f64, // Maximum drop probability (0.0-1.0)
    pub rng: rand::rngs::ThreadRng,
    pub metrics: Arc<Mutex<ServerMetrics>>,
    pub rejected_requests: u64,
}

impl RedAdmissionServer {
    pub fn new(
        name: String,
        capacity: usize,
        service_time: Duration,
        queue_capacity: usize,
        min_threshold: usize,
        max_threshold: usize,
        max_drop_probability: f64,
        metrics: Arc<Mutex<ServerMetrics>>,
    ) -> Self {
        let server = Server::with_constant_service_time(name, capacity, service_time)
            .with_queue(Box::new(FifoQueue::bounded(queue_capacity)));

        Self {
            inner_server: server,
            min_threshold,
            max_threshold,
            max_drop_probability,
            rng: rand::thread_rng(),
            metrics,
            rejected_requests: 0,
        }
    }

    fn calculate_drop_probability(&self) -> f64 {
        let queue_size = self.inner_server.queue_depth();
        
        if queue_size <= self.min_threshold {
            0.0 // No dropping below min threshold
        } else if queue_size >= self.max_threshold {
            1.0 // Drop all requests above max threshold
        } else {
            // Linear interpolation between min and max thresholds
            let range = self.max_threshold - self.min_threshold;
            let position = queue_size - self.min_threshold;
            let ratio = position as f64 / range as f64;
            ratio * self.max_drop_probability
        }
    }

    fn should_accept_request(&mut self) -> bool {
        let drop_probability = self.calculate_drop_probability();
        
        if drop_probability == 0.0 {
            true
        } else if drop_probability >= 1.0 {
            false
        } else {
            let random_value: f64 = self.rng.gen();
            random_value > drop_probability
        }
    }

    fn reject_request(&mut self, attempt: &RequestAttempt, client_id: Key<ClientEvent>, scheduler: &mut Scheduler) {
        self.rejected_requests += 1;
        let drop_prob = self.calculate_drop_probability();

        // Record rejection metrics
        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.requests_rejected += 1;
            metrics.red_drops += 1;
        }

        // Send rejection response
        let response = Response::new(
            attempt.request_id,
            false, // failure
            format!("RED drop - queue: {}, drop prob: {:.2}", 
                   self.inner_server.queue_depth(), drop_prob).into_bytes(),
        );

        scheduler.schedule_now(client_id, ClientEvent::ResponseReceived { response });

        println!(
            "[{}] [{}] RED drop for request {} (queue: {}, prob: {:.2})",
            scheduler.time().as_duration().as_millis(),
            self.inner_server.name,
            attempt.id.0,
            self.inner_server.queue_depth(),
            drop_prob
        );
    }
}

impl Component for RedAdmissionServer {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ServerEvent::ProcessRequest { attempt, client_id } => {
                if self.should_accept_request() {
                    // Accept request - delegate to inner server
                    self.inner_server.process_event(self_id, event, scheduler);
                    
                    let drop_prob = self.calculate_drop_probability();
                    println!(
                        "[{}] [{}] Accepted request {} (queue: {}, drop prob: {:.2})",
                        scheduler.time().as_duration().as_millis(),
                        self.inner_server.name,
                        attempt.id.0,
                        self.inner_server.queue_depth(),
                        drop_prob
                    );
                } else {
                    // Reject request due to RED
                    self.reject_request(attempt, *client_id, scheduler);
                }
            }
            _ => {
                // Delegate other events to inner server
                self.inner_server.process_event(self_id, event, scheduler);
            }
        }
    }
}
```

## Circuit Breaker Pattern

Circuit breakers protect servers by failing fast when error rates exceed thresholds, preventing cascading failures.

### Circuit Breaker Server

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum CircuitState {
    Closed,   // Normal operation
    Open,     // Failing fast
    HalfOpen, // Testing recovery
}

pub struct CircuitBreakerServer {
    pub inner_server: Server,
    pub state: CircuitState,
    pub failure_threshold: f64,    // Failure rate threshold (0.0-1.0)
    pub recovery_timeout: Duration, // Time to wait before trying half-open
    pub test_request_count: usize, // Number of requests to test in half-open
    pub window_size: usize,        // Size of sliding window for failure rate
    pub recent_results: VecDeque<bool>, // Recent request results (true = success)
    pub last_failure_time: Option<SimTime>,
    pub test_requests_sent: usize,
    pub test_requests_successful: usize,
    pub metrics: Arc<Mutex<ServerMetrics>>,
    pub rejected_requests: u64,
}

impl CircuitBreakerServer {
    pub fn new(
        name: String,
        capacity: usize,
        service_time: Duration,
        failure_threshold: f64,
        recovery_timeout: Duration,
        test_request_count: usize,
        window_size: usize,
        metrics: Arc<Mutex<ServerMetrics>>,
    ) -> Self {
        Self {
            inner_server: Server::with_constant_service_time(name, capacity, service_time),
            state: CircuitState::Closed,
            failure_threshold,
            recovery_timeout,
            test_request_count,
            window_size,
            recent_results: VecDeque::new(),
            last_failure_time: None,
            test_requests_sent: 0,
            test_requests_successful: 0,
            metrics,
            rejected_requests: 0,
        }
    }

    fn calculate_failure_rate(&self) -> f64 {
        if self.recent_results.is_empty() {
            0.0
        } else {
            let failures = self.recent_results.iter().filter(|&&success| !success).count();
            failures as f64 / self.recent_results.len() as f64
        }
    }

    fn update_state(&mut self, current_time: SimTime) {
        match self.state {
            CircuitState::Closed => {
                let failure_rate = self.calculate_failure_rate();
                if failure_rate >= self.failure_threshold && self.recent_results.len() >= self.window_size {
                    self.state = CircuitState::Open;
                    self.last_failure_time = Some(current_time);
                    
                    println!(
                        "[{}] [{}] Circuit breaker OPENED (failure rate: {:.2})",
                        current_time.as_duration().as_millis(),
                        self.inner_server.name,
                        failure_rate
                    );
                }
            }
            CircuitState::Open => {
                if let Some(failure_time) = self.last_failure_time {
                    if current_time.duration_since(failure_time) >= self.recovery_timeout {
                        self.state = CircuitState::HalfOpen;
                        self.test_requests_sent = 0;
                        self.test_requests_successful = 0;
                        
                        println!(
                            "[{}] [{}] Circuit breaker HALF-OPEN (testing recovery)",
                            current_time.as_duration().as_millis(),
                            self.inner_server.name
                        );
                    }
                }
            }
            CircuitState::HalfOpen => {
                if self.test_requests_sent >= self.test_request_count {
                    let success_rate = self.test_requests_successful as f64 / self.test_requests_sent as f64;
                    
                    if success_rate >= (1.0 - self.failure_threshold) {
                        self.state = CircuitState::Closed;
                        self.recent_results.clear(); // Reset history
                        
                        println!(
                            "[{}] [{}] Circuit breaker CLOSED (recovery successful: {:.2})",
                            current_time.as_duration().as_millis(),
                            self.inner_server.name,
                            success_rate
                        );
                    } else {
                        self.state = CircuitState::Open;
                        self.last_failure_time = Some(current_time);
                        
                        println!(
                            "[{}] [{}] Circuit breaker back to OPEN (recovery failed: {:.2})",
                            current_time.as_duration().as_millis(),
                            self.inner_server.name,
                            success_rate
                        );
                    }
                }
            }
        }
    }

    fn should_accept_request(&mut self, current_time: SimTime) -> bool {
        self.update_state(current_time);
        
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => false,
            CircuitState::HalfOpen => self.test_requests_sent < self.test_request_count,
        }
    }

    fn record_result(&mut self, success: bool) {
        match self.state {
            CircuitState::Closed => {
                self.recent_results.push_back(success);
                while self.recent_results.len() > self.window_size {
                    self.recent_results.pop_front();
                }
            }
            CircuitState::HalfOpen => {
                if success {
                    self.test_requests_successful += 1;
                }
            }
            CircuitState::Open => {
                // Don't record results when open
            }
        }
    }

    fn reject_request(&mut self, attempt: &RequestAttempt, client_id: Key<ClientEvent>, scheduler: &mut Scheduler) {
        self.rejected_requests += 1;

        // Record rejection metrics
        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.requests_rejected += 1;
            metrics.circuit_breaker_rejections += 1;
        }

        // Send rejection response
        let response = Response::new(
            attempt.request_id,
            false, // failure
            format!("Circuit breaker {} - failing fast", 
                   match self.state {
                       CircuitState::Open => "OPEN",
                       CircuitState::HalfOpen => "HALF-OPEN (testing)",
                       CircuitState::Closed => "CLOSED",
                   }).into_bytes(),
        );

        scheduler.schedule_now(client_id, ClientEvent::ResponseReceived { response });

        println!(
            "[{}] [{}] Circuit breaker rejection for request {} (state: {:?})",
            scheduler.time().as_duration().as_millis(),
            self.inner_server.name,
            attempt.id.0,
            self.state
        );
    }
}

impl Component for CircuitBreakerServer {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ServerEvent::ProcessRequest { attempt, client_id } => {
                if self.should_accept_request(scheduler.time()) {
                    // Accept request - delegate to inner server
                    if self.state == CircuitState::HalfOpen {
                        self.test_requests_sent += 1;
                    }
                    
                    self.inner_server.process_event(self_id, event, scheduler);
                    
                    println!(
                        "[{}] [{}] Accepted request {} (circuit: {:?})",
                        scheduler.time().as_duration().as_millis(),
                        self.inner_server.name,
                        attempt.id.0,
                        self.state
                    );
                } else {
                    // Reject request due to circuit breaker
                    self.reject_request(attempt, *client_id, scheduler);
                }
            }
            ServerEvent::RequestCompleted { response, .. } => {
                // Record the result for circuit breaker logic
                self.record_result(response.is_success());
                
                // Delegate to inner server
                self.inner_server.process_event(self_id, event, scheduler);
            }
            _ => {
                // Delegate other events to inner server
                self.inner_server.process_event(self_id, event, scheduler);
            }
        }
    }
}
```

## Load Shedding with Priority

Load shedding drops lower-priority requests when the server is under high load, preserving capacity for critical requests.

### Priority-Based Load Shedding Server

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequestPriority {
    Low = 1,
    Normal = 2,
    High = 3,
    Critical = 4,
}

pub struct LoadSheddingServer {
    pub inner_server: Server,
    pub load_threshold: f64,      // Utilization threshold for load shedding (0.0-1.0)
    pub priority_thresholds: HashMap<RequestPriority, f64>, // Drop thresholds by priority
    pub metrics: Arc<Mutex<ServerMetrics>>,
    pub rejected_requests: u64,
}

impl LoadSheddingServer {
    pub fn new(
        name: String,
        capacity: usize,
        service_time: Duration,
        load_threshold: f64,
        metrics: Arc<Mutex<ServerMetrics>>,
    ) -> Self {
        let mut priority_thresholds = HashMap::new();
        priority_thresholds.insert(RequestPriority::Low, 0.7);      // Drop low priority at 70% load
        priority_thresholds.insert(RequestPriority::Normal, 0.85);  // Drop normal at 85% load
        priority_thresholds.insert(RequestPriority::High, 0.95);    // Drop high at 95% load
        priority_thresholds.insert(RequestPriority::Critical, 1.0); // Never drop critical

        Self {
            inner_server: Server::with_constant_service_time(name, capacity, service_time),
            load_threshold,
            priority_thresholds,
            metrics,
            rejected_requests: 0,
        }
    }

    fn extract_priority(&self, attempt: &RequestAttempt) -> RequestPriority {
        // In a real implementation, this would extract priority from request headers
        // For simulation, we'll use request ID to determine priority
        match attempt.id.0 % 4 {
            0 => RequestPriority::Critical,
            1 => RequestPriority::High,
            2 => RequestPriority::Normal,
            _ => RequestPriority::Low,
        }
    }

    fn current_load(&self) -> f64 {
        self.inner_server.utilization()
    }

    fn should_accept_request(&self, priority: RequestPriority) -> bool {
        let current_load = self.current_load();
        let threshold = self.priority_thresholds.get(&priority).unwrap_or(&1.0);
        
        current_load < *threshold
    }

    fn reject_request(&mut self, attempt: &RequestAttempt, priority: RequestPriority, client_id: Key<ClientEvent>, scheduler: &mut Scheduler) {
        self.rejected_requests += 1;

        // Record rejection metrics
        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.requests_rejected += 1;
            metrics.load_shedding_drops += 1;
        }

        // Send rejection response
        let response = Response::new(
            attempt.request_id,
            false, // failure
            format!("Load shedding - priority: {:?}, load: {:.2}", 
                   priority, self.current_load()).into_bytes(),
        );

        scheduler.schedule_now(client_id, ClientEvent::ResponseReceived { response });

        println!(
            "[{}] [{}] Load shedding rejection for request {} (priority: {:?}, load: {:.2})",
            scheduler.time().as_duration().as_millis(),
            self.inner_server.name,
            attempt.id.0,
            priority,
            self.current_load()
        );
    }
}

impl Component for LoadSheddingServer {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ServerEvent::ProcessRequest { attempt, client_id } => {
                let priority = self.extract_priority(attempt);
                
                if self.should_accept_request(priority) {
                    // Accept request - delegate to inner server
                    self.inner_server.process_event(self_id, event, scheduler);
                    
                    println!(
                        "[{}] [{}] Accepted request {} (priority: {:?}, load: {:.2})",
                        scheduler.time().as_duration().as_millis(),
                        self.inner_server.name,
                        attempt.id.0,
                        priority,
                        self.current_load()
                    );
                } else {
                    // Reject request due to load shedding
                    self.reject_request(attempt, priority, *client_id, scheduler);
                }
            }
            _ => {
                // Delegate other events to inner server
                self.inner_server.process_event(self_id, event, scheduler);
            }
        }
    }
}
```

## Performance Analysis and Comparison

### Server Metrics Collection

```rust
#[derive(Debug, Default, Clone)]
pub struct ServerMetrics {
    pub requests_processed: u64,
    pub requests_rejected: u64,
    pub rate_limited_requests: u64,
    pub queue_full_rejections: u64,
    pub red_drops: u64,
    pub circuit_breaker_rejections: u64,
    pub load_shedding_drops: u64,
    pub total_processing_time: Duration,
    pub rejection_reasons: HashMap<String, u64>,
}

impl ServerMetrics {
    pub fn rejection_rate(&self) -> f64 {
        let total_requests = self.requests_processed + self.requests_rejected;
        if total_requests > 0 {
            self.requests_rejected as f64 / total_requests as f64
        } else {
            0.0
        }
    }

    pub fn throughput(&self, duration: Duration) -> f64 {
        if duration.as_secs_f64() > 0.0 {
            self.requests_processed as f64 / duration.as_secs_f64()
        } else {
            0.0
        }
    }

    pub fn average_processing_time(&self) -> Duration {
        if self.requests_processed > 0 {
            self.total_processing_time / self.requests_processed as u32
        } else {
            Duration::zero()
        }
    }
}
```

### Comparing Throttling Strategies

```rust
pub fn compare_throttling_strategies() -> Vec<ThrottlingResults> {
    let strategies = vec![
        ("Token Bucket", run_token_bucket_server_test()),
        ("Queue-Size Admission", run_queue_size_test()),
        ("Random Early Detection", run_red_test()),
        ("Circuit Breaker", run_circuit_breaker_test()),
        ("Load Shedding", run_load_shedding_test()),
    ];

    strategies.into_iter().map(|(name, result)| {
        ThrottlingResults {
            strategy: name.to_string(),
            requests_processed: result.requests_processed,
            requests_rejected: result.requests_rejected,
            rejection_rate: result.rejection_rate(),
            throughput: result.throughput,
            avg_latency: result.avg_latency,
            protection_effectiveness: result.protection_effectiveness,
        }
    }).collect()
}

#[derive(Debug, Clone)]
pub struct ThrottlingResults {
    pub strategy: String,
    pub requests_processed: u64,
    pub requests_rejected: u64,
    pub rejection_rate: f64,
    pub throughput: f64,
    pub avg_latency: Duration,
    pub protection_effectiveness: f64, // 0.0-1.0 score
}

pub fn print_throttling_comparison(results: &[ThrottlingResults]) {
    println!("\n=== Server-Side Throttling Strategy Comparison ===");
    println!("{:<25} {:<12} {:<12} {:<15} {:<12} {:<15}", 
             "Strategy", "Processed", "Rejected", "Rejection %", "Throughput", "Avg Latency");
    println!("{}", "-".repeat(100));

    for result in results {
        println!("{:<25} {:<12} {:<12} {:<15.1} {:<12.1} {:<15.0}ms",
                 result.strategy,
                 result.requests_processed,
                 result.requests_rejected,
                 result.rejection_rate * 100.0,
                 result.throughput,
                 result.avg_latency.as_millis());
    }

    // Analysis
    println!("\n=== Strategy Analysis ===");
    
    let best_throughput = results.iter()
        .max_by(|a, b| a.throughput.partial_cmp(&b.throughput).unwrap())
        .unwrap();
    println!("🚀 Best Throughput: {} ({:.1} RPS)", 
             best_throughput.strategy, best_throughput.throughput);

    let best_protection = results.iter()
        .max_by(|a, b| a.protection_effectiveness.partial_cmp(&b.protection_effectiveness).unwrap())
        .unwrap();
    println!("🛡️  Best Protection: {} ({:.1}% effectiveness)", 
             best_protection.strategy, best_protection.protection_effectiveness * 100.0);

    let lowest_latency = results.iter()
        .min_by(|a, b| a.avg_latency.partial_cmp(&b.avg_latency).unwrap())
        .unwrap();
    println!("⚡ Lowest Latency: {} ({}ms)", 
             lowest_latency.strategy, lowest_latency.avg_latency.as_millis());
}
```

## Best Practices

### Strategy Selection Guidelines

1. **Token Bucket Rate Limiting**
   - Use for: Consistent rate limiting with burst capacity
   - Best for: APIs with known rate limits
   - Pros: Predictable, allows bursts, simple to configure
   - Cons: May reject valid requests during bursts

2. **Queue-Size Based Admission**
   - Use for: Preventing memory exhaustion
   - Best for: Systems with limited memory
   - Pros: Prevents unbounded queue growth
   - Cons: Hard cutoff can cause request spikes

3. **Random Early Detection (RED)**
   - Use for: Smooth degradation under load
   - Best for: High-throughput systems
   - Pros: Gradual degradation, prevents queue buildup
   - Cons: More complex to tune, probabilistic behavior

4. **Circuit Breaker**
   - Use for: Preventing cascading failures
   - Best for: Microservices architectures
   - Pros: Fast failure detection, automatic recovery
   - Cons: May reject requests during recovery testing

5. **Load Shedding**
   - Use for: Priority-based request handling
   - Best for: Systems with different request priorities
   - Pros: Preserves capacity for critical requests
   - Cons: Requires priority classification

### Configuration Guidelines

```rust
// Token Bucket Server Configuration
let token_bucket_config = TokenBucketConfig {
    capacity: expected_peak_rps * 2,      // Allow 2x burst
    refill_rate: sustained_rps,           // Sustained rate limit
};

// Queue-Size Admission Configuration
let queue_config = QueueAdmissionConfig {
    max_queue_size: server_capacity * 10, // 10x server capacity
    admission_threshold: 0.8,             // Start rejecting at 80% full
};

// RED Configuration
let red_config = RedConfig {
    min_threshold: queue_capacity * 0.5,  // Start dropping at 50% full
    max_threshold: queue_capacity * 0.8,  // Drop all at 80% full
    max_drop_probability: 0.1,            // Maximum 10% drop rate
};

// Circuit Breaker Configuration
let circuit_config = CircuitBreakerConfig {
    failure_threshold: 0.5,               // Open at 50% failure rate
    recovery_timeout: Duration::from_secs(30), // Wait 30s before testing
    test_request_count: 5,                // Test with 5 requests
    window_size: 20,                      // Track last 20 requests
};
```

### Monitoring and Alerting

```rust
pub struct ThrottlingMonitor {
    pub rejection_rate_alert_threshold: f64,
    pub circuit_breaker_alert_threshold: Duration,
    pub queue_depth_alert_threshold: f64,
}

impl ThrottlingMonitor {
    pub fn check_alerts(&self, metrics: &ServerMetrics) -> Vec<Alert> {
        let mut alerts = Vec::new();

        // High rejection rate alert
        if metrics.rejection_rate() > self.rejection_rate_alert_threshold {
            alerts.push(Alert::HighRejectionRate {
                current_rate: metrics.rejection_rate(),
                threshold: self.rejection_rate_alert_threshold,
            });
        }

        // Circuit breaker open alert
        if metrics.circuit_breaker_rejections > 0 {
            alerts.push(Alert::CircuitBreakerOpen);
        }

        alerts
    }
}

#[derive(Debug)]
pub enum Alert {
    HighRejectionRate { current_rate: f64, threshold: f64 },
    CircuitBreakerOpen,
    QueueDepthHigh { current_depth: usize, threshold: usize },
}
```

## Real-World Applications

### API Gateway Throttling

```rust
// Different throttling strategies for different API tiers
let api_servers = HashMap::from([
    ("free-tier", create_token_bucket_server(100.0)),      // 100 RPS limit
    ("premium-tier", create_queue_size_server(1000)),      // Queue-based for premium
    ("enterprise-tier", create_load_shedding_server()),    // Priority-based for enterprise
]);
```

### Microservices Protection

```rust
// Circuit breakers for external service calls
let external_services = HashMap::from([
    ("user-service", create_circuit_breaker_server(0.3, Duration::from_secs(60))),
    ("payment-service", create_circuit_breaker_server(0.1, Duration::from_secs(30))),
    ("notification-service", create_red_server(50, 80, 0.2)),
]);
```

### Database Connection Pooling

```rust
// Queue-size admission for database connections
let db_server = QueueSizeAdmissionServer::new(
    "database-pool".to_string(),
    10,  // 10 connections
    Duration::from_millis(50), // Average query time
    100, // Queue up to 100 requests
    0.8, // Start rejecting at 80 queued requests
    metrics,
);
```

## Testing Strategies

### Load Testing with Throttling

```rust
#[test]
fn test_throttling_under_load() {
    // Create high-load scenario to test throttling effectiveness
    let load_patterns = vec![
        (Duration::from_secs(10), 50.0),   // Normal load
        (Duration::from_secs(5), 200.0),   // Spike load
        (Duration::from_secs(10), 50.0),   // Recovery
    ];

    for (duration, rps) in load_patterns {
        // Test each throttling strategy under this load
        test_strategy_under_load("token-bucket", duration, rps);
        test_strategy_under_load("queue-size", duration, rps);
        test_strategy_under_load("red", duration, rps);
        test_strategy_under_load("circuit-breaker", duration, rps);
    }
}
```

### Failure Injection Testing

```rust
#[test]
fn test_circuit_breaker_with_failures() {
    // Inject failures to test circuit breaker behavior
    let failure_patterns = vec![
        (10, 0.0),   // 10 successful requests
        (10, 0.8),   // 10 requests with 80% failure rate (should open circuit)
        (5, 0.0),    // 5 requests while circuit is open (should be rejected)
        (10, 0.0),   // 10 successful requests (should close circuit)
    ];

    for (request_count, failure_rate) in failure_patterns {
        test_circuit_breaker_with_failure_rate(request_count, failure_rate);
    }
}
```

## Next Steps

Continue to [Advanced Workload Patterns](./advanced_workload_patterns.md) to learn about implementing complex traffic patterns and stateful client behaviors.