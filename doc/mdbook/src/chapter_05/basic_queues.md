# Basic M/M/1 and M/M/k Queues

This section demonstrates how to implement and analyze classic M/M/k queueing systems using DESCARTES. We'll start with the fundamental M/M/1 queue and extend it to multi-server M/M/k systems.

## M/M/1 Queue Implementation

An M/M/1 queue is the simplest queueing system: single server, Poisson arrivals, exponential service times.

### Complete M/M/1 Example

```rust
//! M/M/1 Queue Example
//!
//! This example demonstrates a classic M/M/1 queueing system with:
//! - Exponential inter-arrival times (Poisson arrivals)
//! - Exponential service times
//! - Single server with FIFO queue
//! - Performance analysis and theoretical validation

use des_components::{ClientEvent, FifoQueue, Server, ServerEvent};
use des_core::{
    Component, Execute, Executor, Key, RequestAttempt, RequestAttemptId, RequestId, Response,
    Scheduler, SimTime, Simulation,
};
use des_metrics::SimulationMetrics;
use rand_distr::{Distribution, Exp};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Configuration for M/M/1 queueing system
#[derive(Debug, Clone)]
pub struct Mm1Config {
    /// Arrival rate λ (requests per second)
    pub lambda: f64,
    /// Service rate μ (requests per second)
    pub mu: f64,
    /// Queue capacity (None = unlimited)
    pub queue_capacity: Option<usize>,
    /// Simulation duration
    pub duration: Duration,
}

impl Mm1Config {
    /// Create a new M/M/1 configuration
    pub fn new(lambda: f64, mu: f64, duration: Duration, queue_capacity: Option<usize>) -> Self {
        Self {
            lambda,
            mu,
            queue_capacity,
            duration,
        }
    }

    /// Calculate theoretical utilization (ρ = λ / μ)
    pub fn utilization(&self) -> f64 {
        self.lambda / self.mu
    }

    /// Check if system is stable (ρ < 1)
    pub fn is_stable(&self) -> bool {
        self.utilization() < 1.0
    }

    /// Calculate theoretical average number in system (L = ρ / (1 - ρ))
    pub fn theoretical_avg_in_system(&self) -> f64 {
        let rho = self.utilization();
        if rho >= 1.0 {
            f64::INFINITY
        } else {
            rho / (1.0 - rho)
        }
    }

    /// Calculate theoretical average response time (W = L / λ)
    pub fn theoretical_avg_response_time(&self) -> Duration {
        let l = self.theoretical_avg_in_system();
        if l.is_infinite() {
            Duration::from_secs(u64::MAX)
        } else {
            Duration::from_secs_f64(l / self.lambda)
        }
    }
}

/// Exponential arrival client for M/M/1 simulation
pub struct ExponentialArrivalClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub lambda: f64,
    pub next_request_id: u64,
    pub rng: rand::rngs::ThreadRng,
    pub exp_dist: Exp<f64>,
    pub metrics: Arc<Mutex<SimulationMetrics>>,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub requests_sent: u64,
}

impl ExponentialArrivalClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        lambda: f64,
        metrics: Arc<Mutex<SimulationMetrics>>,
    ) -> Self {
        let exp_dist = Exp::new(lambda).unwrap();

        Self {
            name,
            server_key,
            lambda,
            next_request_id: 1,
            rng: rand::thread_rng(),
            exp_dist,
            metrics,
            request_start_times: HashMap::new(),
            requests_sent: 0,
        }
    }

    fn schedule_next_request(&mut self, scheduler: &mut Scheduler, self_key: Key<ClientEvent>) {
        // Sample inter-arrival time from exponential distribution
        let inter_arrival = self.exp_dist.sample(&mut self.rng);
        let delay = Duration::from_secs_f64(inter_arrival);

        scheduler.schedule(
            SimTime::from_duration(delay),
            self_key,
            ClientEvent::SendRequest,
        );
    }

    fn send_request(&mut self, scheduler: &mut Scheduler, self_key: Key<ClientEvent>) {
        let request_id = RequestId(self.next_request_id);
        let attempt_id = RequestAttemptId(self.next_request_id);

        // Record request start time for latency calculation
        self.request_start_times
            .insert(request_id, scheduler.time());

        // Create request attempt
        let attempt = RequestAttempt::new(
            attempt_id,
            request_id,
            1, // First attempt
            scheduler.time(),
            format!("Request {} from {}", self.next_request_id, self.name).into_bytes(),
        );

        self.next_request_id += 1;
        self.requests_sent += 1;

        // Record metrics
        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.increment_counter("requests_sent", &[("component", &self.name)]);
        }

        // Send to server
        scheduler.schedule_now(
            self.server_key,
            ServerEvent::ProcessRequest {
                attempt,
                client_id: self_key,
            },
        );

        println!(
            "[{:.3}s] {} sent request {}",
            scheduler.time().as_secs_f64(),
            self.name,
            request_id.0
        );

        // Schedule next request
        self.schedule_next_request(scheduler, self_key);
    }

    fn handle_response(&mut self, response: &Response, scheduler: &mut Scheduler) {
        // Calculate latency
        if let Some(start_time) = self.request_start_times.remove(&response.request_id) {
            let latency = scheduler.time().duration_since(start_time);
            let latency_ms = latency.as_secs_f64() * 1000.0;

            // Record metrics
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
                "[{:.3}s] {} received response for request {} (latency: {:.2}ms, success: {})",
                scheduler.time().as_secs_f64(),
                self.name,
                response.request_id.0,
                latency_ms,
                response.is_success()
            );
        }
    }
}

impl Component for ExponentialArrivalClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                self.send_request(scheduler, self_id);
            }
            ClientEvent::ResponseReceived { response } => {
                self.handle_response(response, scheduler);
            }
            _ => {
                // Handle other events as needed
            }
        }
    }
}

/// Run M/M/1 queue simulation
pub fn run_mm1_simulation(config: Mm1Config) -> Mm1Results {
    println!("\n🔄 Running M/M/1 Queue Simulation");
    println!(
        "λ={:.2} req/s, μ={:.2} req/s, ρ={:.3} ({})",
        config.lambda,
        config.mu,
        config.utilization(),
        if config.is_stable() { "stable" } else { "unstable" }
    );

    let mut simulation = Simulation::default();
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));

    // Create server with exponential service times
    let server = Server::with_exponential_service_time(
        "mm1_server".to_string(),
        1, // Single server
        Duration::from_secs_f64(1.0 / config.mu), // Mean service time
    );

    // Add queue if specified
    let server = if let Some(capacity) = config.queue_capacity {
        server.with_queue(Box::new(FifoQueue::bounded(capacity)))
    } else {
        server.with_queue(Box::new(FifoQueue::unbounded()))
    };

    let server_key = simulation.add_component(server);

    // Create exponential arrival client
    let client = ExponentialArrivalClient::new(
        "mm1_client".to_string(),
        server_key,
        config.lambda,
        metrics.clone(),
    );

    let client_key = simulation.add_component(client);

    // Start the client
    simulation.schedule(
        SimTime::zero(),
        client_key,
        ClientEvent::SendRequest,
    );

    // Run simulation
    let executor = Executor::timed(SimTime::from_duration(config.duration));
    executor.execute(&mut simulation);

    // Collect results
    let final_server = simulation
        .remove_component::<ServerEvent, Server>(server_key)
        .unwrap();
    let final_client = simulation
        .remove_component::<ClientEvent, ExponentialArrivalClient>(client_key)
        .unwrap();

    // Calculate performance metrics
    let requests_sent = final_client.requests_sent;
    let requests_completed = final_server.requests_processed;
    let requests_dropped = final_server.requests_rejected;

    let success_rate = if requests_sent > 0 {
        requests_completed as f64 / requests_sent as f64
    } else {
        0.0
    };

    let throughput_rps = requests_completed as f64 / config.duration.as_secs_f64();

    // Get response time statistics from metrics
    let final_metrics = {
        let metrics_guard = metrics.lock().unwrap();
        metrics_guard.get_metrics_snapshot()
    };

    let response_time_stats = final_metrics
        .histograms
        .iter()
        .find(|(name, labels, _)| {
            name == "response_time_ms" && labels.get("component") == Some(&"mm1_client".to_string())
        })
        .map(|(_, _, stats)| stats);

    let avg_response_time_ms = response_time_stats
        .map(|stats| stats.mean)
        .unwrap_or(0.0);

    Mm1Results {
        config: config.clone(),
        requests_sent,
        requests_completed,
        requests_dropped,
        success_rate,
        throughput_rps,
        avg_response_time_ms,
        server_utilization: final_server.utilization(),
        theoretical_utilization: config.utilization(),
        theoretical_avg_response_time_ms: config.theoretical_avg_response_time().as_secs_f64() * 1000.0,
    }
}

/// Results from M/M/1 simulation
#[derive(Debug, Clone)]
pub struct Mm1Results {
    pub config: Mm1Config,
    pub requests_sent: u64,
    pub requests_completed: u64,
    pub requests_dropped: u64,
    pub success_rate: f64,
    pub throughput_rps: f64,
    pub avg_response_time_ms: f64,
    pub server_utilization: f64,
    pub theoretical_utilization: f64,
    pub theoretical_avg_response_time_ms: f64,
}

impl Mm1Results {
    pub fn print_analysis(&self) {
        println!("\n=== M/M/1 Queue Analysis ===");
        
        // Configuration
        println!("Configuration:");
        println!("  λ (arrival rate): {:.2} req/s", self.config.lambda);
        println!("  μ (service rate): {:.2} req/s", self.config.mu);
        println!("  Queue capacity: {}", 
                 self.config.queue_capacity.map_or("∞".to_string(), |c| c.to_string()));
        
        // Theoretical vs Observed
        println!("\nTheoretical vs Observed:");
        println!("  Theoretical ρ: {:.3}", self.theoretical_utilization);
        println!("  Observed ρ: {:.3}", self.server_utilization);
        println!("  Theoretical avg response time: {:.2} ms", self.theoretical_avg_response_time_ms);
        println!("  Observed avg response time: {:.2} ms", self.avg_response_time_ms);
        
        // Performance Metrics
        println!("\nPerformance Metrics:");
        println!("  Requests sent: {}", self.requests_sent);
        println!("  Requests completed: {}", self.requests_completed);
        println!("  Requests dropped: {}", self.requests_dropped);
        println!("  Success rate: {:.1}%", self.success_rate * 100.0);
        println!("  Throughput: {:.2} req/s", self.throughput_rps);
        
        // Validation
        println!("\nValidation:");
        let utilization_error = (self.server_utilization - self.theoretical_utilization).abs();
        println!("  Utilization error: {:.1}%", utilization_error * 100.0);
        
        if self.config.is_stable() {
            println!("  ✅ System is stable (ρ < 1)");
        } else {
            println!("  ⚠️  System is unstable (ρ ≥ 1)");
        }
        
        if utilization_error < 0.1 {
            println!("  ✅ Utilization matches theory (error < 10%)");
        } else {
            println!("  ⚠️  Utilization deviates from theory (error ≥ 10%)");
        }
    }
}

fn main() {
    // Example 1: Stable M/M/1 queue
    let config1 = Mm1Config::new(
        5.0,                        // λ = 5 req/s
        8.0,                        // μ = 8 req/s  
        Duration::from_secs(10),    // 10 second simulation
        Some(50),                   // Queue capacity of 50
    );
    
    let results1 = run_mm1_simulation(config1);
    results1.print_analysis();
    
    // Example 2: High utilization M/M/1 queue
    let config2 = Mm1Config::new(
        9.5,                        // λ = 9.5 req/s
        10.0,                       // μ = 10 req/s (ρ = 0.95)
        Duration::from_secs(10),    // 10 second simulation
        Some(100),                  // Larger queue for high utilization
    );
    
    let results2 = run_mm1_simulation(config2);
    results2.print_analysis();
}
```

### Key Implementation Details

**Exponential Distributions**: Both inter-arrival times and service times use exponential distributions, which is the defining characteristic of M/M/1 queues.

**Poisson Process**: Exponential inter-arrival times create a Poisson arrival process, meaning arrivals are memoryless and independent.

**FIFO Queue**: First-In-First-Out queue ensures fair processing order.

**Metrics Collection**: Comprehensive metrics track latency, throughput, utilization, and queue behavior.

## M/M/k Queue Extension

Extending to M/M/k (multiple servers) requires minimal changes:

### M/M/k Configuration

```rust
/// Configuration for M/M/k queueing system
#[derive(Debug, Clone)]
pub struct MmkConfig {
    /// Arrival rate λ (requests per second)
    pub lambda: f64,
    /// Service rate μ per server (requests per second)
    pub mu: f64,
    /// Number of servers (k)
    pub k: usize,
    /// Queue capacity (None = unlimited)
    pub queue_capacity: Option<usize>,
    /// Simulation duration
    pub duration: Duration,
}

impl MmkConfig {
    /// Calculate theoretical utilization (ρ = λ / (k × μ))
    pub fn utilization(&self) -> f64 {
        self.lambda / (self.k as f64 * self.mu)
    }
    
    /// Check if system is stable (ρ < 1)
    pub fn is_stable(&self) -> bool {
        self.utilization() < 1.0
    }
    
    /// Calculate total service capacity
    pub fn service_capacity(&self) -> f64 {
        self.k as f64 * self.mu
    }
}
```

### M/M/k Server Setup

```rust
// Create M/M/k server with k parallel servers
let server = Server::with_exponential_service_time(
    format!("mmk_server_k{}", config.k),
    config.k, // Number of parallel servers
    Duration::from_secs_f64(1.0 / config.mu), // Mean service time per server
);

// Add shared queue for all servers
let server = if let Some(capacity) = config.queue_capacity {
    server.with_queue(Box::new(FifoQueue::bounded(capacity)))
} else {
    server.with_queue(Box::new(FifoQueue::unbounded()))
};
```

### Complete M/M/k Example

```rust
/// Run M/M/k queue simulation
pub fn run_mmk_simulation(config: MmkConfig) -> MmkResults {
    println!("\n🔄 Running M/M/{} Queue Simulation", config.k);
    println!(
        "λ={:.2} req/s, μ={:.2} req/s per server, k={}, ρ={:.3} ({})",
        config.lambda,
        config.mu,
        config.k,
        config.utilization(),
        if config.is_stable() { "stable" } else { "unstable" }
    );

    let mut simulation = Simulation::default();
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));

    // Create M/M/k server
    let server = Server::with_exponential_service_time(
        format!("mmk_server_k{}", config.k),
        config.k, // k parallel servers
        Duration::from_secs_f64(1.0 / config.mu),
    );

    let server = if let Some(capacity) = config.queue_capacity {
        server.with_queue(Box::new(FifoQueue::bounded(capacity)))
    } else {
        server.with_queue(Box::new(FifoQueue::unbounded()))
    };

    let server_key = simulation.add_component(server);

    // Create exponential arrival client (same as M/M/1)
    let client = ExponentialArrivalClient::new(
        format!("mmk_client_k{}", config.k),
        server_key,
        config.lambda,
        metrics.clone(),
    );

    let client_key = simulation.add_component(client);

    // Start the simulation
    simulation.schedule(SimTime::zero(), client_key, ClientEvent::SendRequest);

    // Run simulation
    let executor = Executor::timed(SimTime::from_duration(config.duration));
    executor.execute(&mut simulation);

    // Collect and analyze results (similar to M/M/1)
    // ... (result collection code)
}
```

## Performance Analysis and Validation

### Theoretical Validation

For M/M/k queues, we can validate several theoretical properties:

```rust
impl MmkResults {
    pub fn validate_theory(&self) {
        println!("\n=== Theoretical Validation ===");
        
        // 1. Utilization Check
        let theoretical_rho = self.config.utilization();
        let observed_rho = self.server_utilization;
        let rho_error = (observed_rho - theoretical_rho).abs();
        
        println!("Utilization (ρ):");
        println!("  Theoretical: {:.3}", theoretical_rho);
        println!("  Observed: {:.3}", observed_rho);
        println!("  Error: {:.1}%", rho_error * 100.0);
        
        // 2. Stability Check
        if theoretical_rho < 1.0 {
            println!("  ✅ System is theoretically stable");
        } else {
            println!("  ⚠️  System is theoretically unstable");
        }
        
        // 3. Little's Law Validation (L = λW)
        let arrival_rate = self.throughput_rps; // Effective arrival rate
        let avg_time_in_system = self.avg_response_time_ms / 1000.0; // Convert to seconds
        let observed_l = arrival_rate * avg_time_in_system;
        
        println!("\nLittle's Law (L = λW):");
        println!("  λ (throughput): {:.2} req/s", arrival_rate);
        println!("  W (avg time): {:.3} s", avg_time_in_system);
        println!("  L (avg in system): {:.2}", observed_l);
        
        // 4. Service Capacity Check
        let service_capacity = self.config.k as f64 * self.config.mu;
        println!("\nService Capacity:");
        println!("  Total capacity: {:.2} req/s", service_capacity);
        println!("  Throughput: {:.2} req/s", self.throughput_rps);
        println!("  Capacity utilization: {:.1}%", 
                 (self.throughput_rps / service_capacity) * 100.0);
    }
}
```

### Performance Comparison

Compare M/M/1 vs M/M/k performance:

```rust
fn compare_mm1_vs_mmk() {
    let duration = Duration::from_secs(30);
    let lambda = 8.0; // 8 req/s arrival rate
    let mu = 5.0;     // 5 req/s per server
    
    // M/M/1: Single server, ρ = 8/5 = 1.6 (unstable!)
    let mm1_config = Mm1Config::new(lambda, mu, duration, Some(100));
    let mm1_results = run_mm1_simulation(mm1_config);
    
    // M/M/2: Two servers, ρ = 8/(2×5) = 0.8 (stable)
    let mm2_config = MmkConfig::new(lambda, mu, 2, duration, Some(100));
    let mm2_results = run_mmk_simulation(mm2_config);
    
    // M/M/3: Three servers, ρ = 8/(3×5) = 0.53 (very stable)
    let mm3_config = MmkConfig::new(lambda, mu, 3, duration, Some(100));
    let mm3_results = run_mmk_simulation(mm3_config);
    
    println!("\n=== M/M/1 vs M/M/k Comparison ===");
    println!("| System | ρ     | Throughput | Avg Latency | Success Rate |");
    println!("|--------|-------|------------|-------------|--------------|");
    println!("| M/M/1  | {:.2} | {:8.2} | {:9.2} | {:10.1}% |", 
             mm1_results.theoretical_utilization,
             mm1_results.throughput_rps,
             mm1_results.avg_response_time_ms,
             mm1_results.success_rate * 100.0);
    println!("| M/M/2  | {:.2} | {:8.2} | {:9.2} | {:10.1}% |", 
             mm2_results.theoretical_utilization,
             mm2_results.throughput_rps,
             mm2_results.avg_response_time_ms,
             mm2_results.success_rate * 100.0);
    println!("| M/M/3  | {:.2} | {:8.2} | {:9.2} | {:10.1}% |", 
             mm3_results.theoretical_utilization,
             mm3_results.throughput_rps,
             mm3_results.avg_response_time_ms,
             mm3_results.success_rate * 100.0);
}
```

## Queue Capacity Management

### Bounded vs Unbounded Queues

```rust
// Unbounded queue (infinite capacity)
let server = server.with_queue(Box::new(FifoQueue::unbounded()));

// Bounded queue (finite capacity)
let server = server.with_queue(Box::new(FifoQueue::bounded(50)));

// No queue (reject when servers busy)
// Don't add a queue - requests rejected immediately when all servers busy
```

### Overflow Handling

When queues reach capacity, different behaviors are possible:

```rust
// Monitor queue overflow
if server.queue_depth() >= queue_capacity {
    metrics.increment_counter("queue_overflow", &[("component", "server")]);
}

// Track rejection rate
let rejection_rate = requests_dropped as f64 / requests_sent as f64;
println!("Rejection rate: {:.1}%", rejection_rate * 100.0);
```

## Running the Examples

To run these examples:

```bash
# Run the basic M/M/k examples
cargo run --package des-components --example mmk_queueing_example

# Run with specific parameters
RUST_LOG=info cargo run --package des-components --example mmk_queueing_example
```

Expected output shows theoretical vs observed metrics, helping you understand queueing behavior and validate your implementations.

## Next Steps

Now that you understand basic M/M/k queues:
- Add [timeout and retry policies](retry_policies.md) for reliability
- Implement [admission control](admission_control.md) for traffic management
- Explore [advanced patterns](../chapter_07/README.md) for complex scenarios

The foundation you've built here extends to all queueing systems in DESCARTES!