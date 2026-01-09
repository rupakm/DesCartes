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
//! 5. Time-series visualization showing latency, request rate, queue depth, and drop rate
//!
//! The goal is to study average latency and throughput under different load conditions
//! to prepare for future enhancements with throttling and admission control policies.
//!
//! Run with: cargo run --package des-components --example end_to_end_workload_example

use des_components::{ClientEvent, FifoQueue, Server, ServerEvent};
use des_core::{
    Component, Execute, Executor, Key, RequestAttempt, RequestAttemptId, RequestId, Response,
    Scheduler, SimTime, Simulation,
};
use plotters::prelude::*;
use rand_distr::{Distribution, Exp};
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
            t1_duration: Duration::from_secs(300), // 300 seconds low load
            t2_duration: Duration::from_secs(60),  // 20 seconds high load
            t3_duration: Duration::from_secs(300), // 30 seconds low load
            low_rps: 6.0,
            high_rps: 20.0,
        }
    }
}

/// Time-series sample of metrics at a specific point in time
#[derive(Debug, Clone)]
pub struct MetricsSample {
    /// Simulation time when sample was taken
    pub time_secs: f64,
    /// Average latency since last sample (ms)
    pub avg_latency_ms: f64,
    /// Request rate since last sample (requests/sec)
    pub request_rate: f64,
    /// Current queue depth
    pub queue_depth: usize,
    /// Drop rate since last sample (drops/sec)
    pub drop_rate: f64,
}

/// Time-series metrics collector
#[derive(Debug, Default, Clone)]
pub struct TimeSeriesMetrics {
    /// All samples collected over time
    pub samples: Vec<MetricsSample>,
    /// Counters since last sample
    pub requests_sent_since_last: u64,
    pub latency_sum_since_last: u64,
    pub latency_count_since_last: u64,
    pub drops_since_last: u64,
    pub last_sample_time: Option<SimTime>,
    /// Last known queue depth
    pub last_queue_depth: usize,
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
    /// Time-series data for visualization
    pub time_series: TimeSeriesMetrics,
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
        self.time_series.requests_sent_since_last += 1;
        self.phase_metrics
            .entry(phase.to_string())
            .or_default()
            .requests_sent += 1;
    }

    pub fn record_response(&mut self, latency_ms: u64, success: bool, phase: &str) {
        self.responses_received += 1;
        self.total_latency_ms += latency_ms;
        self.latency_samples.push(latency_ms);

        // Update time-series data
        self.time_series.latency_sum_since_last += latency_ms;
        self.time_series.latency_count_since_last += 1;

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

    pub fn record_drop(&mut self) {
        self.server_requests_rejected += 1;
        self.time_series.drops_since_last += 1;
    }

    pub fn update_queue_depth(&mut self, queue_depth: usize) {
        self.time_series.last_queue_depth = queue_depth;
    }

    pub fn sample_now(&mut self, current_time: SimTime) {
        let queue_depth = self.time_series.last_queue_depth;
        let time_secs = current_time.as_duration().as_secs_f64();

        // Calculate time elapsed since last sample
        let elapsed_secs = if let Some(last_time) = self.time_series.last_sample_time {
            current_time.duration_since(last_time).as_secs_f64()
        } else {
            1.0 // First sample, default to 1 second
        };

        // Calculate rates
        let avg_latency_ms = if self.time_series.latency_count_since_last > 0 {
            self.time_series.latency_sum_since_last as f64
                / self.time_series.latency_count_since_last as f64
        } else {
            0.0
        };

        let request_rate = if elapsed_secs > 0.0 {
            self.time_series.requests_sent_since_last as f64 / elapsed_secs
        } else {
            0.0
        };

        let drop_rate = if elapsed_secs > 0.0 {
            self.time_series.drops_since_last as f64 / elapsed_secs
        } else {
            0.0
        };

        // Create sample
        let sample = MetricsSample {
            time_secs,
            avg_latency_ms,
            request_rate,
            queue_depth,
            drop_rate,
        };

        self.time_series.samples.push(sample);

        // Reset counters
        self.time_series.requests_sent_since_last = 0;
        self.time_series.latency_sum_since_last = 0;
        self.time_series.latency_count_since_last = 0;
        self.time_series.drops_since_last = 0;
        self.time_series.last_sample_time = Some(current_time);
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
            println!(
                "=== PHASE TRANSITION: {} -> {} ({} RPS) at {:?} ===",
                self.current_phase, new_phase, new_rps, current_time
            );

            self.current_phase = new_phase.to_string();
            self.current_rps = new_rps;
            self.exp_dist = Exp::new(new_rps).unwrap();
        }
    }

    pub fn update_rate(&mut self, new_rps: f64, phase: String, current_time: SimTime) {
        self.current_rps = new_rps;
        self.current_phase = phase.clone();
        self.exp_dist = Exp::new(new_rps).unwrap();
        println!(
            "[{}] Updated to {} RPS ({}) at {:?}",
            self.name, new_rps, phase, current_time
        );
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
            1, // First attempt (NoRetryPolicy)
            scheduler.time(),
            format!(
                "Request {} from {} ({})",
                self.next_request_id, self.name, self.current_phase
            )
            .into_bytes(),
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

        println!(
            "[{}] Sent request {} at {:?} ({})",
            self.name,
            request_id.0,
            scheduler.time(),
            self.current_phase
        );

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

            println!(
                "[{}] Received response for request {} at {:?} (latency: {}ms, success: {})",
                self.name,
                response.request_id.0,
                scheduler.time(),
                latency_ms,
                response.is_success()
            );
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
                println!(
                    "[{}] Warning: Retry requested but NoRetryPolicy is active",
                    self.name
                );
            }
        }
    }
}

/// Metrics sampler component that periodically samples metrics
pub struct MetricsSampler {
    pub metrics: Arc<Mutex<SimulationMetrics>>,
    pub server_key: Key<ServerEvent>,
    pub sample_interval: Duration,
}

#[derive(Debug, Clone)]
pub enum SamplerEvent {
    Sample,
}

impl MetricsSampler {
    pub fn new(
        metrics: Arc<Mutex<SimulationMetrics>>,
        server_key: Key<ServerEvent>,
        sample_interval: Duration,
    ) -> Self {
        Self {
            metrics,
            server_key,
            sample_interval,
        }
    }

    fn schedule_next_sample(&self, scheduler: &mut Scheduler, self_key: Key<SamplerEvent>) {
        scheduler.schedule(
            SimTime::from_duration(self.sample_interval),
            self_key,
            SamplerEvent::Sample,
        );
    }
}

impl Component for MetricsSampler {
    type Event = SamplerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            SamplerEvent::Sample => {
                // Sample metrics
                {
                    let mut metrics = self.metrics.lock().unwrap();
                    metrics.sample_now(scheduler.time());
                }

                // Schedule next sample
                self.schedule_next_sample(scheduler, self_id);
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
    pub metrics: Arc<Mutex<SimulationMetrics>>,
}

impl ExponentialServiceServer {
    pub fn new(
        name: String,
        capacity: usize,
        mean_service_time: Duration,
        metrics: Arc<Mutex<SimulationMetrics>>,
    ) -> Self {
        let mean_ms = mean_service_time.as_millis() as f64;
        let rate = 1.0 / mean_ms; // Rate parameter for exponential distribution

        Self {
            inner_server: Server::with_exponential_service_time(name, capacity, mean_service_time),
            service_time_dist: Exp::new(rate).unwrap(),
            rng: rand::thread_rng(),
            base_service_time_ms: mean_ms,
            metrics,
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

    pub fn queue_depth(&self) -> usize {
        self.inner_server.queue_depth()
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
            ServerEvent::ProcessRequest {
                attempt,
                client_id: _,
            } => {
                // Check if request will be rejected
                let will_reject = self.inner_server.active_threads
                    >= self.inner_server.thread_capacity
                    && self
                        .inner_server
                        .queue
                        .as_ref()
                        .map_or(false, |q| q.is_full());

                if will_reject {
                    // Record drop in metrics
                    let mut metrics = self.metrics.lock().unwrap();
                    metrics.record_drop();
                }

                // Process with the inner server (now uses built-in exponential distribution)
                self.inner_server.process_event(self_id, event, scheduler);

                // Update queue depth in metrics
                {
                    let mut metrics = self.metrics.lock().unwrap();
                    metrics.update_queue_depth(self.inner_server.queue_depth());
                }

                println!(
                    "[{}] Processing request {} with exponential service time distribution",
                    self.inner_server.name, attempt.id.0
                );
            }
            _ => {
                // Delegate other events to inner server
                self.inner_server.process_event(self_id, event, scheduler);

                // Update queue depth after any event
                {
                    let mut metrics = self.metrics.lock().unwrap();
                    metrics.update_queue_depth(self.inner_server.queue_depth());
                }
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== End-to-End Workload Example ===\n");

    let workload_config = WorkloadConfig::default();
    let total_duration =
        workload_config.t1_duration + workload_config.t2_duration + workload_config.t3_duration;

    println!("Workload Configuration:");
    println!(
        "  Phase 1: {} RPS for {:?}",
        workload_config.low_rps, workload_config.t1_duration
    );
    println!(
        "  Phase 2: {} RPS for {:?}",
        workload_config.high_rps, workload_config.t2_duration
    );
    println!(
        "  Phase 3: {} RPS for {:?}",
        workload_config.low_rps, workload_config.t3_duration
    );
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
        metrics.clone(),
    )
    .with_queue(Box::new(FifoQueue::bounded(50))); // Queue for overflow

    let server_key = simulation.add_component(server);

    // Create metrics sampler (sample every 1 second)
    let sampler = MetricsSampler::new(metrics.clone(), server_key, Duration::from_secs(1));
    let sampler_key = simulation.add_component(sampler);

    // Start sampler
    simulation.schedule(
        SimTime::from_duration(Duration::from_secs(1)),
        sampler_key,
        SamplerEvent::Sample,
    );

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
    simulation.schedule(simulation_start_time, client_key, ClientEvent::SendRequest);

    // Run simulation
    let total_duration =
        workload_config.t1_duration + workload_config.t2_duration + workload_config.t3_duration;
    let executor = Executor::timed(SimTime::from_duration(
        total_duration + Duration::from_secs(10),
    )); // Extra time for completion
    executor.execute(&mut simulation);

    // Get final metrics
    let final_metrics = {
        let metrics_guard = metrics.lock().unwrap();
        metrics_guard.clone()
    };

    // Get server metrics
    let final_server = simulation
        .remove_component::<ServerEvent, ExponentialServiceServer>(server_key)
        .unwrap();

    // Print results
    print_results(&final_metrics, &final_server.inner_server, total_duration);

    // Generate visualizations
    generate_visualizations(&final_metrics, &workload_config)?;

    // Generate HTML report
    println!("\n📄 Generating HTML report...");
    generate_html_report(&final_metrics, &workload_config)?;
    println!("\n✅ Complete! Open target/workload_viz/report.html in your browser");

    Ok(())
}

fn print_results(metrics: &SimulationMetrics, server: &Server, total_duration: Duration) {
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

    println!(
        "  Throughput: {:.2} RPS",
        metrics.throughput_rps(total_duration)
    );

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
                phase_metrics.successful_responses as f64 / phase_metrics.responses_received as f64
                    * 100.0
            } else {
                0.0
            };

            println!(
                "  {}: {} sent, {} received, {:.2}ms avg latency, {:.1}% success",
                phase,
                phase_metrics.requests_sent,
                phase_metrics.responses_received,
                phase_avg_latency,
                phase_success_rate
            );
        }
    }

    // Analysis
    println!("\n🔍 Analysis:");
    if metrics.successful_responses > 0 {
        let avg_latency = metrics.average_latency_ms();
        if avg_latency > 200.0 {
            println!(
                "  ⚠️  High average latency detected ({}ms) - server may be overloaded",
                avg_latency as u64
            );
        } else if avg_latency > 150.0 {
            println!(
                "  ⚡ Moderate latency ({}ms) - system under stress",
                avg_latency as u64
            );
        } else {
            println!("  ✅ Good latency performance ({}ms)", avg_latency as u64);
        }
    }

    let success_rate = metrics.success_rate();
    if success_rate < 0.95 {
        println!(
            "  ⚠️  Low success rate ({:.1}%) - consider adding throttling or admission control",
            success_rate * 100.0
        );
    } else {
        println!("  ✅ Good success rate ({:.1}%)", success_rate * 100.0);
    }

    if server.requests_rejected > 0 {
        println!(
            "  📋 {} requests were rejected - queue and capacity limits reached",
            server.requests_rejected
        );
    }

    println!("\n💡 Next Steps:");
    println!("  - Add throttling middleware to limit request rate");
    println!("  - Implement admission control policies");
    println!("  - Add retry policies for failed requests");
    println!("  - Experiment with different service time distributions");
}

fn generate_visualizations(
    metrics: &SimulationMetrics,
    config: &WorkloadConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n📊 Generating visualizations...");

    std::fs::create_dir_all("target/workload_viz")?;

    // Calculate phase transition times
    let phase1_end = config.t1_duration.as_secs_f64();
    let phase2_end = (config.t1_duration + config.t2_duration).as_secs_f64();

    let samples = &metrics.time_series.samples;
    if samples.is_empty() {
        println!("⚠️  No time-series samples collected - skipping visualization");
        return Ok(());
    }

    // Determine time range
    let max_time = samples.iter().map(|s| s.time_secs).fold(0.0f64, f64::max);

    // Chart 1: Latency over time
    {
        let path = "target/workload_viz/latency_over_time.png";
        let root = BitMapBackend::new(path, (1200, 600)).into_drawing_area();
        root.fill(&WHITE)?;

        let mut chart = ChartBuilder::on(&root)
            .caption("Average Latency Over Time", ("sans-serif", 40))
            .margin(10)
            .x_label_area_size(50)
            .y_label_area_size(60)
            .build_cartesian_2d(0.0..max_time, 0.0..300.0)?;

        chart
            .configure_mesh()
            .x_desc("Time (seconds)")
            .y_desc("Latency (ms)")
            .draw()?;

        // Draw phase transition lines
        chart.draw_series(std::iter::once(PathElement::new(
            vec![(phase1_end, 0.0), (phase1_end, 300.0)],
            RED.stroke_width(2),
        )))?;
        chart.draw_series(std::iter::once(PathElement::new(
            vec![(phase2_end, 0.0), (phase2_end, 300.0)],
            RED.stroke_width(2),
        )))?;

        // Draw latency line
        chart.draw_series(LineSeries::new(
            samples.iter().map(|s| (s.time_secs, s.avg_latency_ms)),
            BLUE.stroke_width(2),
        ))?;

        root.present()?;
        println!("✓ Created: {}", path);
    }

    // Chart 2: Request rate over time
    {
        let path = "target/workload_viz/request_rate_over_time.png";
        let root = BitMapBackend::new(path, (1200, 600)).into_drawing_area();
        root.fill(&WHITE)?;

        let max_rate = samples
            .iter()
            .map(|s| s.request_rate)
            .fold(0.0f64, f64::max)
            * 1.2;

        let mut chart = ChartBuilder::on(&root)
            .caption("Request Rate Over Time", ("sans-serif", 40))
            .margin(10)
            .x_label_area_size(50)
            .y_label_area_size(60)
            .build_cartesian_2d(0.0..max_time, 0.0..max_rate)?;

        chart
            .configure_mesh()
            .x_desc("Time (seconds)")
            .y_desc("Request Rate (requests/sec)")
            .draw()?;

        // Draw phase transition lines
        chart.draw_series(std::iter::once(PathElement::new(
            vec![(phase1_end, 0.0), (phase1_end, max_rate)],
            RED.stroke_width(2),
        )))?;
        chart.draw_series(std::iter::once(PathElement::new(
            vec![(phase2_end, 0.0), (phase2_end, max_rate)],
            RED.stroke_width(2),
        )))?;

        // Draw request rate line
        chart.draw_series(LineSeries::new(
            samples.iter().map(|s| (s.time_secs, s.request_rate)),
            GREEN.stroke_width(2),
        ))?;

        root.present()?;
        println!("✓ Created: {}", path);
    }

    // Chart 3: Queue depth over time
    {
        let path = "target/workload_viz/queue_depth_over_time.png";
        let root = BitMapBackend::new(path, (1200, 600)).into_drawing_area();
        root.fill(&WHITE)?;

        let max_queue = samples.iter().map(|s| s.queue_depth).max().unwrap_or(1) as f64 * 1.2;

        let mut chart = ChartBuilder::on(&root)
            .caption("Queue Depth Over Time", ("sans-serif", 40))
            .margin(10)
            .x_label_area_size(50)
            .y_label_area_size(60)
            .build_cartesian_2d(0.0..max_time, 0.0..max_queue)?;

        chart
            .configure_mesh()
            .x_desc("Time (seconds)")
            .y_desc("Queue Depth")
            .draw()?;

        // Draw phase transition lines
        chart.draw_series(std::iter::once(PathElement::new(
            vec![(phase1_end, 0.0), (phase1_end, max_queue)],
            RED.stroke_width(2),
        )))?;
        chart.draw_series(std::iter::once(PathElement::new(
            vec![(phase2_end, 0.0), (phase2_end, max_queue)],
            RED.stroke_width(2),
        )))?;

        // Draw queue depth line
        chart.draw_series(LineSeries::new(
            samples.iter().map(|s| (s.time_secs, s.queue_depth as f64)),
            MAGENTA.stroke_width(2),
        ))?;

        root.present()?;
        println!("✓ Created: {}", path);
    }

    // Chart 4: Drop rate over time
    {
        let path = "target/workload_viz/drop_rate_over_time.png";
        let root = BitMapBackend::new(path, (1200, 600)).into_drawing_area();
        root.fill(&WHITE)?;

        let max_drop_rate = samples.iter().map(|s| s.drop_rate).fold(0.0f64, f64::max) * 1.2 + 1.0;

        let mut chart = ChartBuilder::on(&root)
            .caption("Request Drop Rate Over Time", ("sans-serif", 40))
            .margin(10)
            .x_label_area_size(50)
            .y_label_area_size(60)
            .build_cartesian_2d(0.0..max_time, 0.0..max_drop_rate)?;

        chart
            .configure_mesh()
            .x_desc("Time (seconds)")
            .y_desc("Drop Rate (drops/sec)")
            .draw()?;

        // Draw phase transition lines
        chart.draw_series(std::iter::once(PathElement::new(
            vec![(phase1_end, 0.0), (phase1_end, max_drop_rate)],
            RED.stroke_width(2),
        )))?;
        chart.draw_series(std::iter::once(PathElement::new(
            vec![(phase2_end, 0.0), (phase2_end, max_drop_rate)],
            RED.stroke_width(2),
        )))?;

        // Draw drop rate line
        chart.draw_series(LineSeries::new(
            samples.iter().map(|s| (s.time_secs, s.drop_rate)),
            RED.stroke_width(2),
        ))?;

        root.present()?;
        println!("✓ Created: {}", path);
    }

    println!("\n📈 Visualization complete!");
    println!("Open the charts in target/workload_viz/");
    println!("  - latency_over_time.png");
    println!("  - request_rate_over_time.png");
    println!("  - queue_depth_over_time.png");
    println!("  - drop_rate_over_time.png");

    Ok(())
}

fn generate_html_report(
    metrics: &SimulationMetrics,
    config: &WorkloadConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::fs;

    let output_path = "target/workload_viz/report.html";

    // Calculate statistics
    let total_sent = metrics.requests_sent;
    let total_received = metrics.responses_received;
    let total_success = metrics.successful_responses;
    let total_errors = metrics.failed_responses;
    let success_rate = if total_received > 0 {
        (total_success as f64 / total_received as f64) * 100.0
    } else {
        0.0
    };

    let avg_latency = if total_received > 0 {
        metrics.total_latency_ms as f64 / total_received as f64
    } else {
        0.0
    };

    // Base64 encode images
    let latency_img = fs::read("target/workload_viz/latency_over_time.png")?;
    let latency_b64 = base64_encode(&latency_img);

    let request_rate_img = fs::read("target/workload_viz/request_rate_over_time.png")?;
    let request_rate_b64 = base64_encode(&request_rate_img);

    let queue_depth_img = fs::read("target/workload_viz/queue_depth_over_time.png")?;
    let queue_depth_b64 = base64_encode(&queue_depth_img);

    let drop_rate_img = fs::read("target/workload_viz/drop_rate_over_time.png")?;
    let drop_rate_b64 = base64_encode(&drop_rate_img);

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>End-to-End Workload Analysis Report</title>
    <style>
        * {{
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }}

        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
            line-height: 1.6;
            color: #333;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            padding: 20px;
        }}

        .container {{
            max-width: 1400px;
            margin: 0 auto;
            background: white;
            border-radius: 12px;
            box-shadow: 0 10px 40px rgba(0,0,0,0.2);
            overflow: hidden;
        }}

        header {{
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
            padding: 40px;
            text-align: center;
        }}

        h1 {{
            font-size: 2.5em;
            margin-bottom: 10px;
            font-weight: 700;
        }}

        .subtitle {{
            font-size: 1.1em;
            opacity: 0.95;
        }}

        .summary {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(250px, 1fr));
            gap: 20px;
            padding: 40px;
            background: #f8f9fa;
        }}

        .metric-card {{
            background: white;
            padding: 25px;
            border-radius: 8px;
            box-shadow: 0 2px 8px rgba(0,0,0,0.1);
            transition: transform 0.2s;
        }}

        .metric-card:hover {{
            transform: translateY(-5px);
            box-shadow: 0 4px 16px rgba(0,0,0,0.15);
        }}

        .metric-label {{
            font-size: 0.9em;
            color: #666;
            text-transform: uppercase;
            letter-spacing: 0.5px;
            margin-bottom: 8px;
        }}

        .metric-value {{
            font-size: 2.5em;
            font-weight: 700;
            color: #667eea;
            line-height: 1;
        }}

        .metric-unit {{
            font-size: 0.5em;
            color: #999;
            margin-left: 5px;
        }}

        .config-section {{
            padding: 40px;
            border-bottom: 1px solid #e0e0e0;
        }}

        .config-section h2 {{
            font-size: 1.8em;
            margin-bottom: 20px;
            color: #333;
        }}

        .config-grid {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 15px;
        }}

        .config-item {{
            padding: 15px;
            background: #f8f9fa;
            border-radius: 6px;
            border-left: 4px solid #667eea;
        }}

        .config-item strong {{
            color: #667eea;
        }}

        .charts-section {{
            padding: 40px;
        }}

        .chart-container {{
            margin-bottom: 50px;
        }}

        .chart-container h3 {{
            font-size: 1.5em;
            margin-bottom: 15px;
            color: #333;
        }}

        .chart-container img {{
            width: 100%;
            height: auto;
            border-radius: 8px;
            box-shadow: 0 4px 12px rgba(0,0,0,0.1);
        }}

        .phase-metrics {{
            padding: 40px;
            background: #f8f9fa;
        }}

        .phase-metrics h2 {{
            font-size: 1.8em;
            margin-bottom: 25px;
            color: #333;
        }}

        table {{
            width: 100%;
            border-collapse: collapse;
            background: white;
            border-radius: 8px;
            overflow: hidden;
            box-shadow: 0 2px 8px rgba(0,0,0,0.1);
        }}

        th {{
            background: #667eea;
            color: white;
            padding: 15px;
            text-align: left;
            font-weight: 600;
        }}

        td {{
            padding: 12px 15px;
            border-bottom: 1px solid #e0e0e0;
        }}

        tr:last-child td {{
            border-bottom: none;
        }}

        tr:hover {{
            background: #f8f9fa;
        }}

        footer {{
            padding: 30px;
            text-align: center;
            color: #666;
            background: #f8f9fa;
            border-top: 1px solid #e0e0e0;
        }}

        .warning {{
            background: #fff3cd;
            border-left: 4px solid #ffc107;
            padding: 15px;
            margin: 20px 0;
            border-radius: 4px;
        }}

        .warning strong {{
            color: #856404;
        }}
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1>📊 End-to-End Workload Analysis</h1>
            <p class="subtitle">Variable Load Performance Report</p>
        </header>

        <div class="summary">
            <div class="metric-card">
                <div class="metric-label">Total Requests</div>
                <div class="metric-value">{total_sent}</div>
            </div>
            <div class="metric-card">
                <div class="metric-label">Completed</div>
                <div class="metric-value">{total_received}</div>
            </div>
            <div class="metric-card">
                <div class="metric-label">Success Rate</div>
                <div class="metric-value">{success_rate:.1}<span class="metric-unit">%</span></div>
            </div>
            <div class="metric-card">
                <div class="metric-label">Avg Latency</div>
                <div class="metric-value">{avg_latency:.1}<span class="metric-unit">ms</span></div>
            </div>
            <div class="metric-card">
                <div class="metric-label">Successful</div>
                <div class="metric-value">{total_success}</div>
            </div>
            <div class="metric-card">
                <div class="metric-label">Errors</div>
                <div class="metric-value">{total_errors}</div>
            </div>
        </div>

        <div class="config-section">
            <h2>🔧 Workload Configuration</h2>
            <div class="config-grid">
                <div class="config-item">
                    <strong>Phase 1 (Low):</strong> {t1_duration:.0}s at {low_rps} RPS
                </div>
                <div class="config-item">
                    <strong>Phase 2 (High):</strong> {t2_duration:.0}s at {high_rps} RPS
                </div>
                <div class="config-item">
                    <strong>Phase 3 (Low):</strong> {t3_duration:.0}s at {low_rps2} RPS
                </div>
                <div class="config-item">
                    <strong>Total Duration:</strong> {total_duration:.0}s
                </div>
            </div>
        </div>

        <div class="charts-section">
            <h2>📈 Time-Series Visualizations</h2>
            <p style="margin-bottom: 30px; color: #666;">Red vertical lines indicate phase transitions</p>

            <div class="chart-container">
                <h3>Average Latency Over Time</h3>
                <img src="data:image/png;base64,{latency_b64}" alt="Latency Over Time">
            </div>

            <div class="chart-container">
                <h3>Request Rate Over Time</h3>
                <img src="data:image/png;base64,{request_rate_b64}" alt="Request Rate Over Time">
            </div>

            <div class="chart-container">
                <h3>Queue Depth Over Time</h3>
                <img src="data:image/png;base64,{queue_depth_b64}" alt="Queue Depth Over Time">
            </div>

            <div class="chart-container">
                <h3>Request Drop Rate Over Time</h3>
                <img src="data:image/png;base64,{drop_rate_b64}" alt="Drop Rate Over Time">
            </div>
        </div>

        <div class="phase-metrics">
            <h2>📋 Phase-Specific Metrics</h2>
            <table>
                <thead>
                    <tr>
                        <th>Phase</th>
                        <th>Sent</th>
                        <th>Received</th>
                        <th>Success</th>
                        <th>Errors</th>
                        <th>Avg Latency (ms)</th>
                        <th>Success Rate</th>
                    </tr>
                </thead>
                <tbody>
                    {phase_rows}
                </tbody>
            </table>
        </div>

        <footer>
            <p>Generated by DesCartes Discrete-Event Simulator</p>
            <p style="font-size: 0.9em; margin-top: 10px;">Report generated at simulation completion</p>
        </footer>
    </div>
</body>
</html>"#,
        total_sent = total_sent,
        total_received = total_received,
        success_rate = success_rate,
        avg_latency = avg_latency,
        total_success = total_success,
        total_errors = total_errors,
        t1_duration = config.t1_duration.as_secs_f64(),
        low_rps = config.low_rps,
        t2_duration = config.t2_duration.as_secs_f64(),
        high_rps = config.high_rps,
        t3_duration = config.t3_duration.as_secs_f64(),
        low_rps2 = config.low_rps,
        total_duration =
            (config.t1_duration + config.t2_duration + config.t3_duration).as_secs_f64(),
        latency_b64 = latency_b64,
        request_rate_b64 = request_rate_b64,
        queue_depth_b64 = queue_depth_b64,
        drop_rate_b64 = drop_rate_b64,
        phase_rows = generate_phase_rows(metrics),
    );

    fs::write(output_path, html)?;
    println!("✓ Created: {}", output_path);

    Ok(())
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();

    for chunk in data.chunks(3) {
        let mut buf = [0u8; 3];
        for (i, &b) in chunk.iter().enumerate() {
            buf[i] = b;
        }

        let b1 = (buf[0] >> 2) as usize;
        let b2 = (((buf[0] & 0x03) << 4) | (buf[1] >> 4)) as usize;
        let b3 = (((buf[1] & 0x0F) << 2) | (buf[2] >> 6)) as usize;
        let b4 = (buf[2] & 0x3F) as usize;

        result.push(CHARS[b1] as char);
        result.push(CHARS[b2] as char);
        result.push(if chunk.len() > 1 {
            CHARS[b3] as char
        } else {
            '='
        });
        result.push(if chunk.len() > 2 {
            CHARS[b4] as char
        } else {
            '='
        });
    }

    result
}

fn generate_phase_rows(metrics: &SimulationMetrics) -> String {
    let mut rows = String::new();

    for phase in ["phase1", "phase2", "phase3"] {
        if let Some(pm) = metrics.phase_metrics.get(phase) {
            let avg_latency = if pm.responses_received > 0 {
                pm.total_latency_ms as f64 / pm.responses_received as f64
            } else {
                0.0
            };
            let success_rate = if pm.responses_received > 0 {
                (pm.successful_responses as f64 / pm.responses_received as f64) * 100.0
            } else {
                0.0
            };
            let errors = pm.responses_received - pm.successful_responses;

            rows.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{:.2}</td><td>{:.1}%</td></tr>\n",
                phase, pm.requests_sent, pm.responses_received, pm.successful_responses, errors, avg_latency, success_rate
            ));
        }
    }

    rows
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
        let metrics = Arc::new(Mutex::new(SimulationMetrics::default()));
        let server = ExponentialServiceServer::new(
            "test-server".to_string(),
            5,
            Duration::from_millis(100),
            metrics,
        );

        assert_eq!(server.inner_server.name, "test-server");
        assert_eq!(server.inner_server.thread_capacity, 5);
        assert_eq!(server.base_service_time_ms, 100.0);
    }
}
