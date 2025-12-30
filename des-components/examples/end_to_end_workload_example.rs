//! End-to-end example with tower service and variable workload client
//!
//! This example demonstrates:
//! 1. A tower service with exponential service time distribution (10 RPS capacity)
//! 2. A client that generates variable workload patterns:
//!    - 6 RPS for T1 seconds
//!    - 20 RPS for T2 seconds  
//!    - 6 RPS for T3 seconds
//! 3. NoRetryPolicy implementation
//! 4. Metrics collection for latency and throughput analysis
//!
//! The goal is to study average latency and throughput under different load conditions
//! to prepare for future enhancements with throttling and admission control policies.
//!
//! Run with: cargo run --package des-components --example end_to_end_workload_example

use des_components::{
    ClientEvent, Server, ServerEvent, FifoQueue,
};
use des_core::{
    Component, Key, Scheduler, SimTime, Simulation, Execute, Executor,
    RequestAttempt, RequestAttemptId, RequestId, Response,
};
use rand_distr::{Exp, Distribution};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Configuration for the workload phases
#[derive(Debug, Clone)]
pub struct WorkloadConfig {
    /// Phase 1: Low load (6 RPS) duration
    pub t1_duration: Duration,
    /// Phase 2: High load (20 RPS) duration  
    pub t2_duration: Duration,
    /// Phase 3: Low load (6 RPS) duration
    pub t3_duration: Duration,
    /// Low load rate (requests per second)
    pub low_rps: f64,
    /// High load rate (requests per second)
    pub high_rps: f64,
}

impl Default for WorkloadConfig {
    fn default() -> Self {
        Self {
            t1_duration: Duration::from_secs(30),  // 30 seconds low load
            t2_duration: Duration::from_secs(20),  // 20 seconds high load
            t3_duration: Duration::from_secs(30),  // 30 seconds low load
            low_rps: 6.0,
            high_rps: 20.0,
        }
    }
}

/// Metrics collector for the simulation
#[derive(Debug, Default, Clone)]
pub struct SimulationMetrics {
    /// Total requests sent by client
    pub requests_sent: u64,
    /// Total responses received by client
    pub responses_received: u64,
    /// Total successful responses
    pub successful_responses: u64,
    /// Total failed responses
    pub failed_responses: u64,
    /// Total request latencies (sum for average calculation)
    pub total_latency_ms: u64,
    /// Minimum observed latency
    pub min_latency_ms: Option<u64>,
    /// Maximum observed latency
    pub max_latency_ms: Option<u64>,
    /// Latency samples for percentile calculation
    pub latency_samples: Vec<u64>,
    /// Requests processed by server
    pub server_requests_processed: u64,
    /// Requests rejected by server
    pub server_requests_rejected: u64,
    /// Phase-specific metrics
    pub phase_metrics: HashMap<String, PhaseMetrics>,
}

#[derive(Debug, Default, Clone)]
pub struct PhaseMetrics {
    pub requests_sent: u64,
    pub responses_received: u64,
    pub successful_responses: u64,
    pub total_latency_ms: u64,
    pub start_time: SimTime,
    pub end_time: SimTime,
}

impl SimulationMetrics {
    pub fn record_request_sent(&mut self, phase: &str) {
        self.requests_sent += 1;
        self.phase_metrics.entry(phase.to_string())
            .or_default()
            .requests_sent += 1;
    }

    pub fn record_response(&mut self, latency_ms: u64, success: bool, phase: &str) {
        self.responses_received += 1;
        self.total_latency_ms += latency_ms;
        self.latency_samples.push(latency_ms);
        
        if success {
            self.successful_responses += 1;
        } else {
            self.failed_responses += 1;
        }

        // Update min/max latency
        match self.min_latency_ms {
            None => self.min_latency_ms = Some(latency_ms),
            Some(min) if latency_ms < min => self.min_latency_ms = Some(latency_ms),
            _ => {}
        }
        
        match self.max_latency_ms {
            None => self.max_latency_ms = Some(latency_ms),
            Some(max) if latency_ms > max => self.max_latency_ms = Some(latency_ms),
            _ => {}
        }

        // Update phase metrics
        let phase_metrics = self.phase_metrics.entry(phase.to_string()).or_default();
        phase_metrics.responses_received += 1;
        phase_metrics.total_latency_ms += latency_ms;
        if success {
            phase_metrics.successful_responses += 1;
        }
    }

    pub fn average_latency_ms(&self) -> f64 {
        if self.responses_received > 0 {
            self.total_latency_ms as f64 / self.responses_received as f64
        } else {
            0.0
        }
    }

    pub fn throughput_rps(&self, total_duration: Duration) -> f64 {
        if total_duration.as_secs_f64() > 0.0 {
            self.successful_responses as f64 / total_duration.as_secs_f64()
        } else {
            0.0
        }
    }

    pub fn success_rate(&self) -> f64 {
        if self.responses_received > 0 {
            self.successful_responses as f64 / self.responses_received as f64
        } else {
            0.0
        }
    }

    pub fn percentile_latency(&self, percentile: f64) -> Option<u64> {
        if self.latency_samples.is_empty() {
            return None;
        }
        
        let mut sorted_samples = self.latency_samples.clone();
        sorted_samples.sort_unstable();
        
        let index = ((percentile / 100.0) * (sorted_samples.len() - 1) as f64).round() as usize;
        sorted_samples.get(index).copied()
    }
}


/// Variable workload client that changes request rate over time
pub struct VariableWorkloadClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub next_request_id: u64,
    pub current_phase: String,
    pub current_rps: f64,
    pub rng: rand::rngs::ThreadRng,
    pub exp_dist: Exp<f64>,
    pub metrics: Arc<Mutex<SimulationMetrics>>,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub workload_config: WorkloadConfig,
    pub simulation_start_time: SimTime,
}

impl VariableWorkloadClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        workload_config: WorkloadConfig,
        metrics: Arc<Mutex<SimulationMetrics>>,
        simulation_start_time: SimTime,
    ) -> Self {
        let initial_rps = workload_config.low_rps;
        let exp_dist = Exp::new(initial_rps).unwrap();
        
        Self {
            name,
            server_key,
            next_request_id: 1,
            current_phase: "phase1".to_string(),
            current_rps: initial_rps,
            rng: rand::thread_rng(),
            exp_dist,
            metrics,
            request_start_times: HashMap::new(),
            workload_config,
            simulation_start_time,
        }
    }

    /// Check current time and update phase if needed
    fn update_phase_if_needed(&mut self, current_time: SimTime) {
        let elapsed = current_time.duration_since(self.simulation_start_time);
        
        let new_phase_info = if elapsed < self.workload_config.t1_duration {
            // Phase 1: Low RPS
            ("phase1", self.workload_config.low_rps)
        } else if elapsed < self.workload_config.t1_duration + self.workload_config.t2_duration {
            // Phase 2: High RPS
            ("phase2", self.workload_config.high_rps)
        } else {
            // Phase 3: Low RPS again
            ("phase3", self.workload_config.low_rps)
        };
        
        let (new_phase, new_rps) = new_phase_info;
        
        // Only update if phase has changed
        if self.current_phase != new_phase {
            println!("=== PHASE TRANSITION: {} -> {} ({} RPS) at {:?} ===", 
                     self.current_phase, new_phase, new_rps, current_time);
            
            self.current_phase = new_phase.to_string();
            self.current_rps = new_rps;
            self.exp_dist = Exp::new(new_rps).unwrap();
        }
    }

    pub fn update_rate(&mut self, new_rps: f64, phase: String, current_time: SimTime) {
        self.current_rps = new_rps;
        self.current_phase = phase.clone();
        self.exp_dist = Exp::new(new_rps).unwrap();
        println!("[{}] Updated to {} RPS ({}) at {:?}", self.name, new_rps, phase, current_time);
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
        self.request_start_times.insert(request_id, scheduler.time());
        
        // Create request attempt
        let attempt = RequestAttempt::new(
            attempt_id,
            request_id,
            1, // First attempt (NoRetryPolicy)
            scheduler.time(),
            format!("Request {} from {} ({})", self.next_request_id, self.name, self.current_phase).into_bytes(),
        );
        
        self.next_request_id += 1;
        
        // Record metrics
        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.record_request_sent(&self.current_phase);
        }
        
        // Send to server
        scheduler.schedule_now(
            self.server_key,
            ServerEvent::ProcessRequest {
                attempt,
                client_id: self_key,
            },
        );
        
        println!("[{}] Sent request {} at {:?} ({})", 
                 self.name, request_id.0, scheduler.time(), self.current_phase);
        
        // Schedule next request
        self.schedule_next_request(scheduler, self_key);
    }

    fn handle_response(&mut self, response: &Response, scheduler: &mut Scheduler) {
        // Calculate latency
        if let Some(start_time) = self.request_start_times.remove(&response.request_id) {
            let latency = scheduler.time().duration_since(start_time);
            let latency_ms = latency.as_millis() as u64;
            
            // Record metrics
            {
                let mut metrics = self.metrics.lock().unwrap();
                metrics.record_response(latency_ms, response.is_success(), &self.current_phase);
            }
            
            println!("[{}] Received response for request {} at {:?} (latency: {}ms, success: {})", 
                     self.name, response.request_id.0, scheduler.time(), latency_ms, response.is_success());
        }
    }
}

impl Component for VariableWorkloadClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        // Always check and update phase based on current time
        self.update_phase_if_needed(scheduler.time());
        
        match event {
            ClientEvent::SendRequest => {
                self.send_request(scheduler, self_id);
            }
            ClientEvent::ResponseReceived { response } => {
                self.handle_response(response, scheduler);
            }
            ClientEvent::RequestTimeout { .. } => {
                // With NoRetryPolicy, timeouts are just recorded as failures
                println!("[{}] Request timeout occurred", self.name);
            }
            ClientEvent::RetryRequest { .. } => {
                // NoRetryPolicy should never trigger retries
                println!("[{}] Warning: Retry requested but NoRetryPolicy is active", self.name);
            }
        }
    }
}

/// Exponential service time server wrapper
pub struct ExponentialServiceServer {
    pub inner_server: Server,
    pub service_time_dist: Exp<f64>,
    pub rng: rand::rngs::ThreadRng,
    pub base_service_time_ms: f64,
}

impl ExponentialServiceServer {
    pub fn new(name: String, capacity: usize, mean_service_time: Duration) -> Self {
        let mean_ms = mean_service_time.as_millis() as f64;
        let rate = 1.0 / mean_ms; // Rate parameter for exponential distribution
        
        Self {
            inner_server: Server::new(name, capacity, mean_service_time),
            service_time_dist: Exp::new(rate).unwrap(),
            rng: rand::thread_rng(),
            base_service_time_ms: mean_ms,
        }
    }

    pub fn with_queue(mut self, queue: Box<dyn des_components::Queue>) -> Self {
        self.inner_server = self.inner_server.with_queue(queue);
        self
    }

    fn sample_service_time(&mut self) -> Duration {
        let service_time_ms = self.service_time_dist.sample(&mut self.rng);
        Duration::from_millis(service_time_ms as u64)
    }
}

impl Component for ExponentialServiceServer {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ServerEvent::ProcessRequest { attempt, client_id: _ } => {
                // Override service time with exponential sample
                let original_service_time = self.inner_server.service_time;
                self.inner_server.service_time = self.sample_service_time();
                
                // Process with the inner server
                self.inner_server.process_event(self_id, event, scheduler);
                
                // Restore original service time for consistency
                self.inner_server.service_time = original_service_time;
                
                println!("[{}] Processing request {} with service time {:?}", 
                         self.inner_server.name, attempt.id.0, self.inner_server.service_time);
            }
            _ => {
                // Delegate other events to inner server
                self.inner_server.process_event(self_id, event, scheduler);
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== End-to-End Workload Example ===\n");
    
    let workload_config = WorkloadConfig::default();
    let total_duration = workload_config.t1_duration + workload_config.t2_duration + workload_config.t3_duration;
    
    println!("Workload Configuration:");
    println!("  Phase 1: {} RPS for {:?}", workload_config.low_rps, workload_config.t1_duration);
    println!("  Phase 2: {} RPS for {:?}", workload_config.high_rps, workload_config.t2_duration);
    println!("  Phase 3: {} RPS for {:?}", workload_config.low_rps, workload_config.t3_duration);
    println!("  Total duration: {total_duration:?}\n");
    
    run_simulation(workload_config)?;
    
    Ok(())
}

fn run_simulation(workload_config: WorkloadConfig) -> Result<(), Box<dyn std::error::Error>> {
    let mut simulation = Simulation::default();
    let metrics = Arc::new(Mutex::new(SimulationMetrics::default()));
    
    // Create exponential service time server (10 RPS capacity = 100ms mean service time)
    let mean_service_time = Duration::from_millis(100); // 10 RPS capacity
    let server = ExponentialServiceServer::new(
        "exponential-server".to_string(),
        1, 
        mean_service_time,
    ).with_queue(Box::new(FifoQueue::bounded(50))); // Queue for overflow
    
    let server_key = simulation.add_component(server);
    
    // Create variable workload client with NoRetryPolicy
    let simulation_start_time = SimTime::from_duration(Duration::from_millis(100));
    let client = VariableWorkloadClient::new(
        "variable-client".to_string(),
        server_key,
        workload_config.clone(),
        metrics.clone(),
        simulation_start_time,
    );
    
    let client_key = simulation.add_component(client);
    
    // Start the client
    simulation.schedule(
        simulation_start_time,
        client_key,
        ClientEvent::SendRequest,
    );
    
    // Run simulation
    let total_duration = workload_config.t1_duration + workload_config.t2_duration + workload_config.t3_duration;
    let executor = Executor::timed(SimTime::from_duration(total_duration + Duration::from_secs(10))); // Extra time for completion
    executor.execute(&mut simulation);
    
    // Get final metrics
    let final_metrics = {
        let metrics_guard = metrics.lock().unwrap();
        metrics_guard.clone()
    };
    
    // Get server metrics
    let final_server = simulation.remove_component::<ServerEvent, ExponentialServiceServer>(server_key).unwrap();
    
    // Print results
    print_results(&final_metrics, &final_server.inner_server, total_duration);
    
    Ok(())
}

fn print_results(
    metrics: &SimulationMetrics,
    server: &Server,
    total_duration: Duration,
) {
    println!("\n=== SIMULATION RESULTS ===");
    
    // Overall metrics
    println!("\n📊 Overall Metrics:");
    println!("  Requests sent: {}", metrics.requests_sent);
    println!("  Responses received: {}", metrics.responses_received);
    println!("  Success rate: {:.2}%", metrics.success_rate() * 100.0);
    println!("  Average latency: {:.2}ms", metrics.average_latency_ms());
    
    if let Some(p50) = metrics.percentile_latency(50.0) {
        println!("  P50 latency: {p50}ms");
    }
    if let Some(p95) = metrics.percentile_latency(95.0) {
        println!("  P95 latency: {p95}ms");
    }
    if let Some(p99) = metrics.percentile_latency(99.0) {
        println!("  P99 latency: {p99}ms");
    }
    
    if let Some(min) = metrics.min_latency_ms {
        println!("  Min latency: {min}ms");
    }
    if let Some(max) = metrics.max_latency_ms {
        println!("  Max latency: {max}ms");
    }
    
    println!("  Throughput: {:.2} RPS", metrics.throughput_rps(total_duration));
    
    // Server metrics
    println!("\n🖥️  Server Metrics:");
    println!("  Requests processed: {}", server.requests_processed);
    println!("  Requests rejected: {}", server.requests_rejected);
    println!("  Final utilization: {:.2}%", server.utilization() * 100.0);
    println!("  Final queue depth: {}", server.queue_depth());
    
    // Phase-specific metrics
    println!("\n📈 Phase-specific Metrics:");
    for (phase, phase_metrics) in &metrics.phase_metrics {
        if phase_metrics.requests_sent > 0 {
            let phase_avg_latency = if phase_metrics.responses_received > 0 {
                phase_metrics.total_latency_ms as f64 / phase_metrics.responses_received as f64
            } else {
                0.0
            };
            
            let phase_success_rate = if phase_metrics.responses_received > 0 {
                phase_metrics.successful_responses as f64 / phase_metrics.responses_received as f64 * 100.0
            } else {
                0.0
            };
            
            println!("  {}: {} sent, {} received, {:.2}ms avg latency, {:.1}% success",
                     phase, phase_metrics.requests_sent, phase_metrics.responses_received,
                     phase_avg_latency, phase_success_rate);
        }
    }
    
    // Analysis
    println!("\n🔍 Analysis:");
    if metrics.successful_responses > 0 {
        let avg_latency = metrics.average_latency_ms();
        if avg_latency > 200.0 {
            println!("  ⚠️  High average latency detected ({}ms) - server may be overloaded", avg_latency as u64);
        } else if avg_latency > 150.0 {
            println!("  ⚡ Moderate latency ({}ms) - system under stress", avg_latency as u64);
        } else {
            println!("  ✅ Good latency performance ({}ms)", avg_latency as u64);
        }
    }
    
    let success_rate = metrics.success_rate();
    if success_rate < 0.95 {
        println!("  ⚠️  Low success rate ({:.1}%) - consider adding throttling or admission control", success_rate * 100.0);
    } else {
        println!("  ✅ Good success rate ({:.1}%)", success_rate * 100.0);
    }
    
    if server.requests_rejected > 0 {
        println!("  📋 {} requests were rejected - queue and capacity limits reached", server.requests_rejected);
    }
    
    println!("\n💡 Next Steps:");
    println!("  - Add throttling middleware to limit request rate");
    println!("  - Implement admission control policies");
    println!("  - Add retry policies for failed requests");
    println!("  - Experiment with different service time distributions");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workload_config_default() {
        let config = WorkloadConfig::default();
        assert_eq!(config.low_rps, 6.0);
        assert_eq!(config.high_rps, 20.0);
        assert_eq!(config.t1_duration, Duration::from_secs(30));
        assert_eq!(config.t2_duration, Duration::from_secs(20));
        assert_eq!(config.t3_duration, Duration::from_secs(30));
    }

    #[test]
    fn test_simulation_metrics() {
        let mut metrics = SimulationMetrics::default();
        
        // Record some requests and responses
        metrics.record_request_sent("phase1");
        metrics.record_request_sent("phase1");
        metrics.record_response(100, true, "phase1");
        metrics.record_response(150, false, "phase1");
        
        assert_eq!(metrics.requests_sent, 2);
        assert_eq!(metrics.responses_received, 2);
        assert_eq!(metrics.successful_responses, 1);
        assert_eq!(metrics.failed_responses, 1);
        assert_eq!(metrics.average_latency_ms(), 125.0);
        assert_eq!(metrics.success_rate(), 0.5);
    }

    #[test]
    fn test_exponential_service_server_creation() {
        let server = ExponentialServiceServer::new(
            "test-server".to_string(),
            5,
            Duration::from_millis(100),
        );
        
        assert_eq!(server.inner_server.name, "test-server");
        assert_eq!(server.inner_server.thread_capacity, 5);
        assert_eq!(server.base_service_time_ms, 100.0);
    }
}