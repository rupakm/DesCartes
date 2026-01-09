//! M/M/k Queueing System Examples
//!
//! This example demonstrates various M/M/k queueing systems with exponential
//! inter-arrival times (Poisson process) and exponential service times.
//!
//! Test scenarios:
//! 1. classic_mm1_queue: M/M/1 queue (k=1), no retries
//! 2. classic_mmk_queue: M/M/k queue (k>1), no retries  
//! 3. mm1_with_retry: M/M/1 with timeout T and K retries (no exponential backoff)
//! 4. mm1_with_exp_backoff: M/M/1 with timeout T, K retries, exponential backoff + jitter
//!
//! Run with: cargo run --package des-components --example mmk_queueing_example

use des_components::{
    SimpleClient, Server, ServerEvent, ClientEvent, FifoQueue,
    ExponentialBackoffPolicy, FixedRetryPolicy,
};
use des_core::{
    Component, Key, Scheduler, SimTime, Simulation, Execute, Executor,
    RequestAttempt, RequestAttemptId, RequestId, Response,
};
use des_metrics::SimulationMetrics;
use des_metrics::MmkTimeSeriesMetrics;
use des_viz::charts::time_series::{TimeSeries, create_mmk_time_series_chart};
use plotters::prelude::{RED, BLUE, GREEN};
use rand_distr::{Exp, Distribution};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Helper function to format simulation time as duration since start
fn format_sim_time(sim_time: SimTime) -> String {
    let duration = sim_time.as_duration();
    let total_ms = duration.as_millis();
    let seconds = total_ms / 1000;
    let ms = total_ms % 1000;
    
    if seconds > 0 {
        format!("{seconds}.{ms:03}s")
    } else {
        format!("{ms}ms")
    }
}

/// Configuration for M/M/k queueing system parameters
#[derive(Debug, Clone)]
pub struct MmkConfig {
    /// Arrival rate λ (requests per second)
    pub lambda: f64,
    /// Service rate μ (requests per second per server)
    pub mu: f64,
    /// Number of servers (k)
    pub k: usize,
    /// Queue capacity (None = unlimited, Some(0) = no queue)
    pub queue_capacity: Option<usize>,
    /// Simulation duration
    pub duration: Duration,
    /// Client timeout (for retry scenarios)
    pub timeout: Option<Duration>,
    /// Maximum retry attempts (for retry scenarios)
    pub max_retries: Option<usize>,
    /// Base retry delay (for exponential backoff scenarios)
    pub base_retry_delay: Option<Duration>,
}

impl MmkConfig {
    /// Create config for classic M/M/1 queue
    pub fn mm1(lambda: f64, mu: f64, duration: Duration, queue_capacity: Option<usize>) -> Self {
        Self {
            lambda,
            mu,
            k: 1,
            queue_capacity,
            duration,
            timeout: None,
            max_retries: None,
            base_retry_delay: None,
        }
    }

    /// Create config for classic M/M/k queue
    pub fn mmk(lambda: f64, mu: f64, k: usize, duration: Duration, queue_capacity: Option<usize>) -> Self {
        Self {
            lambda,
            mu,
            k,
            queue_capacity,
            duration,
            timeout: None,
            max_retries: None,
            base_retry_delay: None,
        }
    }

    /// Create config for M/M/1 with fixed retry policy
    pub fn mm1_with_retry(
        lambda: f64,
        mu: f64,
        duration: Duration,
        timeout: Duration,
        max_retries: usize,
        queue_capacity: Option<usize>,
    ) -> Self {
        Self {
            lambda,
            mu,
            k: 1,
            queue_capacity,
            duration,
            timeout: Some(timeout),
            max_retries: Some(max_retries),
            base_retry_delay: Some(Duration::from_millis(100)),
        }
    }

    /// Create config for M/M/1 with exponential backoff retry policy
    pub fn mm1_with_exp_backoff(
        lambda: f64,
        mu: f64,
        duration: Duration,
        timeout: Duration,
        max_retries: usize,
        base_retry_delay: Duration,
        queue_capacity: Option<usize>,
    ) -> Self {
        Self {
            lambda,
            mu,
            k: 1,
            queue_capacity,
            duration,
            timeout: Some(timeout),
            max_retries: Some(max_retries),
            base_retry_delay: Some(base_retry_delay),
        }
    }

    /// Calculate theoretical utilization (ρ = λ / (k * μ))
    pub fn utilization(&self) -> f64 {
        self.lambda / (self.k as f64 * self.mu)
    }

    /// Check if system is stable (ρ < 1)
    pub fn is_stable(&self) -> bool {
        self.utilization() < 1.0
    }
}

/// Exponential arrival client that generates requests with exponential inter-arrival times
pub struct ExponentialArrivalClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub lambda: f64, // Arrival rate (requests per second)
    pub next_request_id: u64,
    pub rng: rand::rngs::ThreadRng,
    pub exp_dist: Exp<f64>,
    pub metrics: Arc<Mutex<SimulationMetrics>>,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub max_requests: Option<u64>,
    pub requests_sent: u64,
    pub time_series_metrics: Arc<Mutex<MmkTimeSeriesMetrics>>,
}

impl ExponentialArrivalClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        lambda: f64,
        metrics: Arc<Mutex<SimulationMetrics>>,
        time_series_metrics: Arc<Mutex<MmkTimeSeriesMetrics>>,
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
            max_requests: None,
            requests_sent: 0,
            time_series_metrics,
        }
    }

    pub fn with_max_requests(mut self, max_requests: u64) -> Self {
        self.max_requests = Some(max_requests);
        self
    }

    fn schedule_next_request(&mut self, scheduler: &mut Scheduler, self_key: Key<ClientEvent>) {
        // Check if we've reached the maximum number of requests
        if let Some(max) = self.max_requests {
            if self.requests_sent >= max {
                return;
            }
        }

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
        
        println!("[{}] [{}] Sent request {} at {}", 
                 format_sim_time(scheduler.time()), self.name, request_id.0, format_sim_time(scheduler.time()));
        
        // Schedule next request
        self.schedule_next_request(scheduler, self_key);
    }

    fn handle_response(&mut self, response: &Response, scheduler: &mut Scheduler) {
        // Calculate latency
        if let Some(start_time) = self.request_start_times.remove(&response.request_id) {
            let latency = scheduler.time().duration_since(start_time);
            let latency_ms = latency.as_millis() as f64;
            
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
            
            // Record time-series data
            {
                let mut ts_metrics = self.time_series_metrics.lock().unwrap();
                ts_metrics.record_latency(scheduler.time(), latency_ms);
            }
            
            println!("[{}] [{}] Received response for request {} (latency: {:.2}ms, success: {})", 
                     format_sim_time(scheduler.time()), self.name, response.request_id.0, latency_ms, response.is_success());
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
            ClientEvent::RequestTimeout { .. } => {
                println!("[{}] [{}] Request timeout occurred", format_sim_time(scheduler.time()), self.name);
            }
            ClientEvent::RetryRequest { .. } => {
                println!("[{}] [{}] Retry request (should not occur with NoRetryPolicy)", format_sim_time(scheduler.time()), self.name);
            }
        }
    }
}

/// Constant arrival client that generates requests with fixed inter-arrival times
pub struct ConstantArrivalClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub inter_arrival_time: Duration, // Fixed time between requests
    pub next_request_id: u64,
    pub metrics: Arc<Mutex<SimulationMetrics>>,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub max_requests: Option<u64>,
    pub requests_sent: u64,
    pub time_series_metrics: Arc<Mutex<MmkTimeSeriesMetrics>>,
}

impl ConstantArrivalClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        inter_arrival_time: Duration,
        metrics: Arc<Mutex<SimulationMetrics>>,
        time_series_metrics: Arc<Mutex<MmkTimeSeriesMetrics>>,
    ) -> Self {
        Self {
            name,
            server_key,
            inter_arrival_time,
            next_request_id: 1,
            metrics,
            request_start_times: HashMap::new(),
            max_requests: None,
            requests_sent: 0,
            time_series_metrics,
        }
    }

    /// Create from arrival rate (requests per second)
    pub fn from_rate(
        name: String,
        server_key: Key<ServerEvent>,
        rate: f64, // requests per second
        metrics: Arc<Mutex<SimulationMetrics>>,
        time_series_metrics: Arc<Mutex<MmkTimeSeriesMetrics>>,
    ) -> Self {
        let inter_arrival_time = Duration::from_secs_f64(1.0 / rate);
        Self::new(name, server_key, inter_arrival_time, metrics, time_series_metrics)
    }

    pub fn with_max_requests(mut self, max_requests: u64) -> Self {
        self.max_requests = Some(max_requests);
        self
    }

    fn schedule_next_request(&mut self, scheduler: &mut Scheduler, self_key: Key<ClientEvent>) {
        // Check if we've reached the maximum number of requests
        if let Some(max) = self.max_requests {
            if self.requests_sent >= max {
                return;
            }
        }

        // Schedule next request with fixed inter-arrival time
        scheduler.schedule(
            SimTime::from_duration(self.inter_arrival_time),
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
        
        println!("[{}] [{}] Sent request {} at {} (constant interval: {:.0}ms)", 
                 format_sim_time(scheduler.time()), self.name, request_id.0, 
                 format_sim_time(scheduler.time()), self.inter_arrival_time.as_millis());
        
        // Schedule next request
        self.schedule_next_request(scheduler, self_key);
    }

    fn handle_response(&mut self, response: &Response, scheduler: &mut Scheduler) {
        // Calculate latency
        if let Some(start_time) = self.request_start_times.remove(&response.request_id) {
            let latency = scheduler.time().duration_since(start_time);
            let latency_ms = latency.as_millis() as f64;
            
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
            
            // Record time-series data
            {
                let mut ts_metrics = self.time_series_metrics.lock().unwrap();
                ts_metrics.record_latency(scheduler.time(), latency_ms);
            }
            
            println!("[{}] [{}] Received response for request {} (latency: {:.2}ms, success: {})", 
                     format_sim_time(scheduler.time()), self.name, response.request_id.0, latency_ms, response.is_success());
        }
    }
}

impl Component for ConstantArrivalClient {
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
            ClientEvent::RequestTimeout { .. } => {
                println!("[{}] [{}] Request timeout occurred", format_sim_time(scheduler.time()), self.name);
            }
            ClientEvent::RetryRequest { .. } => {
                println!("[{}] [{}] Retry request (should not occur with NoRetryPolicy)", format_sim_time(scheduler.time()), self.name);
            }
        }
    }
}

/// Exponential service server that uses exponential service time distribution
pub struct ExponentialServiceServer {
    pub inner_server: Server,
    pub service_time_dist: Exp<f64>,
    pub rng: rand::rngs::ThreadRng,
    pub mu: f64, // Service rate (requests per second per server)
    pub metrics: Arc<Mutex<SimulationMetrics>>,
    pub time_series_metrics: Arc<Mutex<MmkTimeSeriesMetrics>>,
}

impl ExponentialServiceServer {
    pub fn new(
        name: String,
        capacity: usize,
        mu: f64, // Service rate (requests per second per server)
        metrics: Arc<Mutex<SimulationMetrics>>,
        time_series_metrics: Arc<Mutex<MmkTimeSeriesMetrics>>,
    ) -> Self {
        let mean_service_time = Duration::from_secs_f64(1.0 / mu);
        let rate = mu; // Rate parameter for exponential distribution (requests per second)

        Self {
            inner_server: Server::with_exponential_service_time(name, capacity, mean_service_time),
            service_time_dist: Exp::new(rate).unwrap(),
            rng: rand::thread_rng(),
            mu,
            metrics,
            time_series_metrics,
        }
    }

    pub fn with_queue(mut self, queue: Box<dyn des_components::Queue>) -> Self {
        self.inner_server = self.inner_server.with_queue(queue);
        self
    }

    fn sample_service_time(&mut self) -> Duration {
        // FIXED: For exponential distribution with rate μ, sample directly gives the service time
        // The Exp(μ) distribution already gives us values with mean 1/μ
        let service_time_secs = self.service_time_dist.sample(&mut self.rng);
        Duration::from_secs_f64(service_time_secs)
    }

    pub fn queue_depth(&self) -> usize {
        self.inner_server.queue_depth()
    }

    pub fn utilization(&self) -> f64 {
        self.inner_server.utilization()
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
                // Check if request will be rejected
                let will_reject = self.inner_server.active_threads >= self.inner_server.thread_capacity
                    && self.inner_server.queue.as_ref().is_none_or(|q| q.is_full());

                if will_reject {
                    // Record drop in metrics
                    let mut metrics = self.metrics.lock().unwrap();
                    metrics.increment_counter("requests_dropped", &[("component", &self.inner_server.name)]);
                }

                // Record time-series data for queue size and utilization
                {
                    let mut ts_metrics = self.time_series_metrics.lock().unwrap();
                    ts_metrics.record_queue_size(scheduler.time(), self.queue_depth() as f64);
                    ts_metrics.record_utilization(scheduler.time(), self.utilization());
                }

                // Process with the inner server (now uses built-in exponential distribution)
                self.inner_server.process_event(self_id, event, scheduler);

                println!("[{}] [{}] Processing request {} with exponential service time distribution",
                         format_sim_time(scheduler.time()), self.inner_server.name, attempt.id.0);
            }
            _ => {
                // Delegate other events to inner server
                self.inner_server.process_event(self_id, event, scheduler);
            }
        }
    }
}

/// Results from running an M/M/k simulation
#[derive(Debug, Clone)]
pub struct MmkResults {
    pub config: MmkConfig,
    pub requests_sent: u64,
    pub requests_completed: u64,
    pub requests_dropped: u64,
    pub avg_response_time_ms: f64,
    pub p95_response_time_ms: Option<f64>,
    pub p99_response_time_ms: Option<f64>,
    pub throughput_rps: f64,
    pub server_utilization: f64,
    pub avg_queue_depth: f64,
    pub success_rate: f64,
    pub theoretical_utilization: f64,
}

impl MmkResults {
    pub fn print_summary(&self) {
        println!("\n=== M/M/{} Queueing System Results ===", self.config.k);
        println!("Configuration:");
        println!("  λ (arrival rate): {:.2} req/s", self.config.lambda);
        println!("  μ (service rate): {:.2} req/s per server", self.config.mu);
        println!("  k (servers): {}", self.config.k);
        println!("  Queue capacity: {}", format_queue_capacity(self.config.queue_capacity));
        println!("  Theoretical ρ: {:.3} ({})", 
                 self.theoretical_utilization,
                 if self.config.is_stable() { "stable" } else { "unstable" });
        
        println!("\nPerformance Metrics:");
        println!("  Requests sent: {}", self.requests_sent);
        println!("  Requests completed: {}", self.requests_completed);
        println!("  Requests dropped: {}", self.requests_dropped);
        println!("  Success rate: {:.2}%", self.success_rate * 100.0);
        println!("  Throughput: {:.2} req/s", self.throughput_rps);
        println!("  Server utilization: {:.2}%", self.server_utilization * 100.0);
        
        println!("\nLatency Metrics:");
        println!("  Average response time: {:.2} ms", self.avg_response_time_ms);
        if let Some(p95) = self.p95_response_time_ms {
            println!("  P95 response time: {p95:.2} ms");
        }
        if let Some(p99) = self.p99_response_time_ms {
            println!("  P99 response time: {p99:.2} ms");
        }
        println!("  Average queue depth: {:.2}", self.avg_queue_depth);
        
        // Queue-specific analysis
        match self.config.queue_capacity {
            None => println!("\n🔄 Queue Analysis: Unlimited queue capacity"),
            Some(0) => {
                println!("\n🚫 Queue Analysis: No queue - requests rejected when server busy");
                if self.requests_dropped > 0 {
                    let drop_rate = self.requests_dropped as f64 / self.requests_sent as f64 * 100.0;
                    println!("  Drop rate: {:.1}% ({} requests)", drop_rate, self.requests_dropped);
                }
            },
            Some(capacity) => {
                println!("\n📦 Queue Analysis: Bounded queue (capacity: {capacity})");
                if self.requests_dropped > 0 {
                    let drop_rate = self.requests_dropped as f64 / self.requests_sent as f64 * 100.0;
                    println!("  Drop rate: {:.1}% ({} requests)", drop_rate, self.requests_dropped);
                    println!("  Queue likely reached capacity during high load periods");
                }
                let queue_utilization = self.avg_queue_depth / capacity as f64 * 100.0;
                println!("  Average queue utilization: {queue_utilization:.1}%");
            }
        }
    }
}

/// Helper function to generate time-series charts
fn generate_time_series_charts(
    time_series_metrics: &Arc<Mutex<MmkTimeSeriesMetrics>>,
    scenario_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let ts_metrics = time_series_metrics.lock().unwrap();
    
    // Create output directory
    std::fs::create_dir_all("target/mmk_charts")?;
    
    // Convert time-series data to visualization format
    let latency_data: Vec<des_viz::charts::time_series::TimeSeriesPoint> = ts_metrics.latency.get_aggregated_data()
        .iter()
        .map(|point| des_viz::charts::time_series::TimeSeriesPoint {
            timestamp: point.timestamp,
            value: point.value,
        })
        .collect();
    
    let queue_size_data: Vec<des_viz::charts::time_series::TimeSeriesPoint> = ts_metrics.queue_size.get_aggregated_data()
        .iter()
        .map(|point| des_viz::charts::time_series::TimeSeriesPoint {
            timestamp: point.timestamp,
            value: point.value,
        })
        .collect();
    
    let timeout_rate_data: Vec<des_viz::charts::time_series::TimeSeriesPoint> = ts_metrics.timeout_rate.get_aggregated_data()
        .iter()
        .map(|point| des_viz::charts::time_series::TimeSeriesPoint {
            timestamp: point.timestamp,
            value: point.value,
        })
        .collect();
    
    // Create time series
    let latency_series = TimeSeries::new("Average Latency", latency_data, RED);
    let queue_size_series = TimeSeries::new("Average Queue Size", queue_size_data, BLUE);
    let timeout_rate_series = TimeSeries::new("Timeout Rate", timeout_rate_data, GREEN);
    
    // Generate the chart
    let output_path = format!("target/mmk_charts/{scenario_name}_time_series.png");
    create_mmk_time_series_chart(
        &latency_series,
        &queue_size_series,
        &timeout_rate_series,
        &output_path,
    )?;
    
    println!("📊 Generated time-series chart: {output_path}");
    Ok(())
}

/// Run a classic M/M/1 queue simulation (no retries)
pub fn run_classic_mm1_queue(config: MmkConfig) -> MmkResults {
    println!("\n🔄 Running Classic M/M/1 Queue Simulation");
    println!("λ={:.2}, μ={:.2}, duration={:?}", config.lambda, config.mu, config.duration);
    
    let mut simulation = Simulation::default();
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));
    
    // Create time-series metrics with 100ms aggregation window and 0.1 EMA alpha
    let time_series_metrics = Arc::new(Mutex::new(MmkTimeSeriesMetrics::new(
        Duration::from_millis(100),
        0.1,
    )));

    // Create exponential service server (M/M/1)
    let server = ExponentialServiceServer::new(
        "mm1-server".to_string(),
        1, // k=1 for M/M/1
        config.mu,
        metrics.clone(),
        time_series_metrics.clone(),
    );
    
    let server = if let Some(capacity) = config.queue_capacity {
        if capacity > 0 {
            server.with_queue(Box::new(FifoQueue::bounded(capacity)))
        } else {
            server // No queue
        }
    } else {
        server.with_queue(Box::new(FifoQueue::unbounded())) // Unlimited queue
    };

    let server_key = simulation.add_component(server);

    // Create exponential arrival client
    let client = ExponentialArrivalClient::new(
        "mm1-client".to_string(),
        server_key,
        config.lambda,
        metrics.clone(),
        time_series_metrics.clone(),
    );

    let client_key = simulation.add_component(client);

    // Start the client
    simulation.schedule(
        SimTime::from_duration(Duration::from_millis(0)),
        client_key,
        ClientEvent::SendRequest,
    );

    // Run simulation
    let executor = Executor::timed(SimTime::from_duration(config.duration));
    executor.execute(&mut simulation);

    // Flush time-series data
    {
        let mut ts_metrics = time_series_metrics.lock().unwrap();
        ts_metrics.flush(SimTime::from_duration(config.duration));
    }

    // Generate time-series visualizations
    generate_time_series_charts(&time_series_metrics, "mm1_classic").unwrap_or_else(|e| {
        eprintln!("Warning: Failed to generate time-series charts: {e}");
    });

    // Collect results
    let final_server = simulation.remove_component::<ServerEvent, ExponentialServiceServer>(server_key).unwrap();
    let final_client = simulation.remove_component::<ClientEvent, ExponentialArrivalClient>(client_key).unwrap();
    let final_metrics = {
        let metrics_guard = metrics.lock().unwrap();
        // Get a snapshot instead of cloning the guard
        metrics_guard.get_metrics_snapshot()
    };

    // Calculate results
    let requests_sent = final_client.requests_sent;
    let requests_completed = final_server.inner_server.requests_processed;
    let requests_dropped = final_server.inner_server.requests_rejected;
    
    let success_rate = if requests_sent > 0 {
        requests_completed as f64 / requests_sent as f64
    } else {
        0.0
    };

    let throughput_rps = requests_completed as f64 / config.duration.as_secs_f64();
    
    // Get response time metrics
    let response_times_stats = final_metrics.histograms.iter()
        .find(|(name, labels, _)| name == "response_time_ms" && 
              labels.get("component").is_some_and(|c| c == "mm1-client"))
        .map(|(_, _, stats)| stats);
    
    let avg_response_time_ms = response_times_stats
        .map(|stats| stats.mean)
        .unwrap_or(0.0);

    let p95_response_time_ms = response_times_stats
        .map(|stats| stats.p95);

    let p99_response_time_ms = response_times_stats
        .map(|stats| stats.p99);

    MmkResults {
        config: config.clone(),
        requests_sent,
        requests_completed,
        requests_dropped,
        avg_response_time_ms,
        p95_response_time_ms,
        p99_response_time_ms,
        throughput_rps,
        server_utilization: final_server.utilization(),
        avg_queue_depth: final_server.queue_depth() as f64, // Snapshot at end
        success_rate,
        theoretical_utilization: config.utilization(),
    }
}

/// Run a classic M/M/k queue simulation (no retries)
pub fn run_classic_mmk_queue(config: MmkConfig) -> MmkResults {
    println!("\n🔄 Running Classic M/M/{} Queue Simulation", config.k);
    println!("λ={:.2}, μ={:.2}, k={}, duration={:?}", 
             config.lambda, config.mu, config.k, config.duration);
    
    let mut simulation = Simulation::default();
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));
    
    // Create time-series metrics with 100ms aggregation window and 0.1 EMA alpha
    let time_series_metrics = Arc::new(Mutex::new(MmkTimeSeriesMetrics::new(
        Duration::from_millis(100),
        0.1,
    )));

    // Create exponential service server (M/M/k)
    let server = ExponentialServiceServer::new(
        format!("mm{}-server", config.k),
        config.k, // k servers
        config.mu,
        metrics.clone(),
        time_series_metrics.clone(),
    );
    
    let server = if let Some(capacity) = config.queue_capacity {
        if capacity > 0 {
            server.with_queue(Box::new(FifoQueue::bounded(capacity)))
        } else {
            server // No queue
        }
    } else {
        server.with_queue(Box::new(FifoQueue::unbounded())) // Unlimited queue
    };

    let server_key = simulation.add_component(server);

    // Create exponential arrival client
    let client = ExponentialArrivalClient::new(
        format!("mm{}-client", config.k),
        server_key,
        config.lambda,
        metrics.clone(),
        time_series_metrics.clone(),
    );

    let client_key = simulation.add_component(client);

    // Start the client
    simulation.schedule(
        SimTime::from_duration(Duration::from_millis(10)),
        client_key,
        ClientEvent::SendRequest,
    );

    // Run simulation
    let executor = Executor::timed(SimTime::from_duration(config.duration));
    executor.execute(&mut simulation);

    // Flush time-series data
    {
        let mut ts_metrics = time_series_metrics.lock().unwrap();
        ts_metrics.flush(SimTime::from_duration(config.duration));
    }

    // Generate time-series visualizations
    generate_time_series_charts(&time_series_metrics, &format!("mm{}_classic", config.k)).unwrap_or_else(|e| {
        eprintln!("Warning: Failed to generate time-series charts: {e}");
    });

    // Collect results (same as MM1)
    let final_server = simulation.remove_component::<ServerEvent, ExponentialServiceServer>(server_key).unwrap();
    let final_client = simulation.remove_component::<ClientEvent, ExponentialArrivalClient>(client_key).unwrap();
    let final_metrics = {
        let metrics_guard = metrics.lock().unwrap();
        // Get a snapshot instead of cloning the guard
        metrics_guard.get_metrics_snapshot()
    };

    // Calculate results
    let requests_sent = final_client.requests_sent;
    let requests_completed = final_server.inner_server.requests_processed;
    let requests_dropped = final_server.inner_server.requests_rejected;
    
    let success_rate = if requests_sent > 0 {
        requests_completed as f64 / requests_sent as f64
    } else {
        0.0
    };

    let throughput_rps = requests_completed as f64 / config.duration.as_secs_f64();
    
    // Get response time metrics
    let client_name = format!("mm{}-client", config.k);
    let response_times_stats = final_metrics.histograms.iter()
        .find(|(name, labels, _)| name == "response_time_ms" && 
              (labels.get("component") == Some(&client_name)))
        .map(|(_, _, stats)| stats);
    
    let avg_response_time_ms = response_times_stats
        .map(|stats| stats.mean)
        .unwrap_or(0.0);

    let p95_response_time_ms = response_times_stats
        .map(|stats| stats.p95);

    let p99_response_time_ms = response_times_stats
        .map(|stats| stats.p99);

    MmkResults {
        config: config.clone(),
        requests_sent,
        requests_completed,
        requests_dropped,
        avg_response_time_ms,
        p95_response_time_ms,
        p99_response_time_ms,
        throughput_rps,
        server_utilization: final_server.utilization(),
        avg_queue_depth: final_server.queue_depth() as f64,
        success_rate,
        theoretical_utilization: config.utilization(),
    }
}

/// Run M/M/1 queue with fixed retry policy (no exponential backoff)
pub fn run_mm1_with_retry(config: MmkConfig) -> MmkResults {
    println!("\n🔄 Running M/M/1 with Fixed Retry Policy");
    println!("λ={:.2}, μ={:.2}, timeout={:?}, max_retries={}", 
             config.lambda, config.mu, 
             config.timeout.unwrap(), config.max_retries.unwrap());
    
    let mut simulation = Simulation::default();
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));
    
    // Create time-series metrics with 100ms aggregation window and 0.1 EMA alpha
    let time_series_metrics = Arc::new(Mutex::new(MmkTimeSeriesMetrics::new(
        Duration::from_millis(100),
        0.1,
    )));

    // Create exponential service server with time-series tracking
    let server = ExponentialServiceServer::new(
        "mm1-retry-server".to_string(),
        1, // M/M/1
        config.mu,
        metrics.clone(),
        time_series_metrics.clone(),
    );
    
    let server = if let Some(capacity) = config.queue_capacity {
        if capacity > 0 {
            server.with_queue(Box::new(FifoQueue::bounded(capacity)))
        } else {
            server // No queue
        }
    } else {
        server.with_queue(Box::new(FifoQueue::unbounded())) // Unlimited queue
    };

    let server_key = simulation.add_component(server);

    // Create client with fixed retry policy
    let retry_policy = FixedRetryPolicy::new(
        config.max_retries.unwrap(),
        config.base_retry_delay.unwrap(),
    );
    
    let client = SimpleClient::new(
        "mm1-retry-client".to_string(),
        server_key,
        Duration::from_secs_f64(1.0 / config.lambda), // Mean inter-arrival time
        retry_policy,
    ).with_timeout(config.timeout.unwrap());

    let client_key = simulation.add_component(client);

    // Start periodic request generation
    use des_core::task::PeriodicTask;
    let task = PeriodicTask::new(
        move |scheduler| {
            scheduler.schedule_now(client_key, ClientEvent::SendRequest);
        },
        SimTime::from_duration(Duration::from_secs_f64(1.0 / config.lambda)),
    );
    simulation.scheduler.schedule_task(SimTime::from_duration(Duration::from_millis(10)), task);

    // Run simulation
    let executor = Executor::timed(SimTime::from_duration(config.duration));
    executor.execute(&mut simulation);

    // Flush time-series data
    {
        let mut ts_metrics = time_series_metrics.lock().unwrap();
        ts_metrics.flush(SimTime::from_duration(config.duration));
    }

    // Generate time-series visualizations
    generate_time_series_charts(&time_series_metrics, "mm1_with_retry").unwrap_or_else(|e| {
        eprintln!("Warning: Failed to generate time-series charts: {e}");
    });

    // Collect results
    let final_server = simulation.remove_component::<ServerEvent, ExponentialServiceServer>(server_key).unwrap();
    let final_client = simulation.remove_component::<ClientEvent, SimpleClient<FixedRetryPolicy>>(client_key).unwrap();
    let final_metrics = final_client.get_metrics();

    // Calculate results
    let requests_sent = final_client.requests_sent;
    let requests_completed = final_server.inner_server.requests_processed;
    let requests_dropped = final_server.inner_server.requests_rejected;
    
    let success_rate = if requests_sent > 0 {
        requests_completed as f64 / requests_sent as f64
    } else {
        0.0
    };

    let throughput_rps = requests_completed as f64 / config.duration.as_secs_f64();
    
    // Get response time metrics
    let response_times_stats = final_metrics.get_histogram_stats("response_time_ms", &[("component", "mm1-retry-client")]);
    let avg_response_time_ms = response_times_stats
        .as_ref()
        .map(|stats| stats.mean)
        .unwrap_or(0.0);

    let p95_response_time_ms = response_times_stats
        .as_ref()
        .map(|stats| stats.p95);

    let p99_response_time_ms = response_times_stats
        .as_ref()
        .map(|stats| stats.p99);

    MmkResults {
        config: config.clone(),
        requests_sent,
        requests_completed,
        requests_dropped,
        avg_response_time_ms,
        p95_response_time_ms,
        p99_response_time_ms,
        throughput_rps,
        server_utilization: final_server.utilization(),
        avg_queue_depth: final_server.queue_depth() as f64,
        success_rate,
        theoretical_utilization: config.utilization(),
    }
}

/// Run M/M/1 queue with exponential backoff retry policy
pub fn run_mm1_with_exp_backoff(config: MmkConfig) -> MmkResults {
    println!("\n🔄 Running M/M/1 with Exponential Backoff Retry Policy");
    println!("λ={:.2}, μ={:.2}, timeout={:?}, max_retries={}, base_delay={:?}", 
             config.lambda, config.mu, 
             config.timeout.unwrap(), config.max_retries.unwrap(),
             config.base_retry_delay.unwrap());
    
    let mut simulation = Simulation::default();
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));
    
    // Create time-series metrics with 100ms aggregation window and 0.1 EMA alpha
    let time_series_metrics = Arc::new(Mutex::new(MmkTimeSeriesMetrics::new(
        Duration::from_millis(100),
        0.1,
    )));

    // Create exponential service server with time-series tracking
    let server = ExponentialServiceServer::new(
        "mm1-backoff-server".to_string(),
        1, // M/M/1
        config.mu,
        metrics.clone(),
        time_series_metrics.clone(),
    );
    
    let server = if let Some(capacity) = config.queue_capacity {
        if capacity > 0 {
            server.with_queue(Box::new(FifoQueue::bounded(capacity)))
        } else {
            server // No queue
        }
    } else {
        server.with_queue(Box::new(FifoQueue::unbounded())) // Unlimited queue
    };

    let server_key = simulation.add_component(server);

    // Create client with exponential backoff retry policy
    let retry_policy = ExponentialBackoffPolicy::new(
        config.max_retries.unwrap(),
        config.base_retry_delay.unwrap(),
    ).with_jitter(true) // Add jitter to prevent thundering herd
     .with_multiplier(2.0); // Standard exponential backoff
    
    let client = SimpleClient::new(
        "mm1-backoff-client".to_string(),
        server_key,
        Duration::from_secs_f64(1.0 / config.lambda), // Mean inter-arrival time
        retry_policy,
    ).with_timeout(config.timeout.unwrap());

    let client_key = simulation.add_component(client);

    // Start periodic request generation
    use des_core::task::PeriodicTask;
    let task = PeriodicTask::new(
        move |scheduler| {
            scheduler.schedule_now(client_key, ClientEvent::SendRequest);
        },
        SimTime::from_duration(Duration::from_secs_f64(1.0 / config.lambda)),
    );
    simulation.scheduler.schedule_task(SimTime::from_duration(Duration::from_millis(10)), task);

    // Run simulation
    let executor = Executor::timed(SimTime::from_duration(config.duration));
    executor.execute(&mut simulation);

    // Flush time-series data
    {
        let mut ts_metrics = time_series_metrics.lock().unwrap();
        ts_metrics.flush(SimTime::from_duration(config.duration));
    }

    // Generate time-series visualizations
    generate_time_series_charts(&time_series_metrics, "mm1_with_exp_backoff").unwrap_or_else(|e| {
        eprintln!("Warning: Failed to generate time-series charts: {e}");
    });

    // Collect results
    let final_server = simulation.remove_component::<ServerEvent, ExponentialServiceServer>(server_key).unwrap();
    let final_client = simulation.remove_component::<ClientEvent, SimpleClient<ExponentialBackoffPolicy>>(client_key).unwrap();
    let final_metrics = final_client.get_metrics();

    // Calculate results
    let requests_sent = final_client.requests_sent;
    let requests_completed = final_server.inner_server.requests_processed;
    let requests_dropped = final_server.inner_server.requests_rejected;
    
    let success_rate = if requests_sent > 0 {
        requests_completed as f64 / requests_sent as f64
    } else {
        0.0
    };

    let throughput_rps = requests_completed as f64 / config.duration.as_secs_f64();
    
    // Get response time metrics
    let response_times_stats = final_metrics.get_histogram_stats("response_time_ms", &[("component", "mm1-backoff-client")]);
    let avg_response_time_ms = response_times_stats
        .as_ref()
        .map(|stats| stats.mean)
        .unwrap_or(0.0);

    let p95_response_time_ms = response_times_stats
        .as_ref()
        .map(|stats| stats.p95);

    let p99_response_time_ms = response_times_stats
        .as_ref()
        .map(|stats| stats.p99);

    MmkResults {
        config: config.clone(),
        requests_sent,
        requests_completed,
        requests_dropped,
        avg_response_time_ms,
        p95_response_time_ms,
        p99_response_time_ms,
        throughput_rps,
        server_utilization: final_server.utilization(),
        avg_queue_depth: final_server.queue_depth() as f64,
        success_rate,
        theoretical_utilization: config.utilization(),
    }
}

/// Run a simulation with constant arrival client (for debugging)
pub fn run_constant_arrival_debug(
    inter_arrival_time: Duration,
    server_num_threads: usize,
    mu: f64,
    duration: Duration,
    max_requests: Option<u64>,
    queue_capacity: Option<usize>,
) -> MmkResults {
    println!("\n🔧 Running Constant Arrival Debug Simulation");
    println!("Inter-arrival: {:.0}ms, Service rate: {:.0}rps, Servers: {} threads, {} qsize, Duration: {:?}", 
             inter_arrival_time.as_millis(), mu, server_num_threads, queue_capacity.unwrap_or(0), duration);
    
    let mut simulation = Simulation::default();
    let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));
    
    // Create time-series metrics with 100ms aggregation window and 0.1 EMA alpha
    let time_series_metrics = Arc::new(Mutex::new(MmkTimeSeriesMetrics::new(
        Duration::from_millis(100),
        0.1,
    )));

    let server = ExponentialServiceServer::new(
        format!("mm{server_num_threads}-server"),
        server_num_threads, // k servers
        mu,
        metrics.clone(),
        time_series_metrics.clone(),
    );

    let server = if let Some(capacity) = queue_capacity {
        if capacity > 0 {
            server.with_queue(Box::new(FifoQueue::bounded(capacity)))
        } else {
            server // No queue
        }
    } else {
        server.with_queue(Box::new(FifoQueue::unbounded())) // Unlimited queue
    };

    let server_key = simulation.add_component(server);

    // Create constant arrival client
    let mut client = ConstantArrivalClient::new(
        "debug-client".to_string(),
        server_key,
        inter_arrival_time,
        metrics.clone(),
        time_series_metrics.clone(),
    );
    
    if let Some(max) = max_requests {
        client = client.with_max_requests(max);
    }

    let client_key = simulation.add_component(client);

    // Start the client
    simulation.schedule(
        SimTime::from_duration(Duration::from_millis(0)),
        client_key,
        ClientEvent::SendRequest,
    );

    // Run simulation
    let executor = Executor::timed(SimTime::from_duration(duration));
    executor.execute(&mut simulation);

    // Flush time-series data
    {
        let mut ts_metrics = time_series_metrics.lock().unwrap();
        ts_metrics.flush(SimTime::from_duration(duration));
    }

    // Generate time-series visualizations
    generate_time_series_charts(&time_series_metrics, "constant_arrival_debug").unwrap_or_else(|e| {
        eprintln!("Warning: Failed to generate time-series charts: {e}");
    });

    // Collect results
    let final_server = simulation.remove_component::<ServerEvent, ExponentialServiceServer>(server_key).unwrap();
    let final_client = simulation.remove_component::<ClientEvent, ConstantArrivalClient>(client_key).unwrap();
    let final_metrics = {
        let metrics_guard = metrics.lock().unwrap();
        metrics_guard.get_metrics_snapshot()
    };

    // Calculate results
    let requests_sent = final_client.requests_sent;
    let requests_completed = final_server.inner_server.requests_processed;
    let requests_dropped = final_server.inner_server.requests_rejected;
    
    let success_rate = if requests_sent > 0 {
        requests_completed as f64 / requests_sent as f64
    } else {
        0.0
    };

    let throughput_rps = requests_completed as f64 / duration.as_secs_f64();
    
    // Get response time metrics
    let response_times_stats = final_metrics.histograms.iter()
        .find(|(name, labels, _)| name == "response_time_ms" && 
              labels.get("component").is_some_and(|c| c == "debug-client"))
        .map(|(_, _, stats)| stats);
    
    let avg_response_time_ms = response_times_stats
        .map(|stats| stats.mean)
        .unwrap_or(0.0);

    let p95_response_time_ms = response_times_stats
        .map(|stats| stats.p95);

    let p99_response_time_ms = response_times_stats
        .map(|stats| stats.p99);

    // Calculate theoretical values for comparison
    let arrival_rate = 1.0 / inter_arrival_time.as_secs_f64();
    let service_rate = mu;
    let theoretical_utilization = arrival_rate / (server_num_threads as f64 * service_rate);

    let config = MmkConfig {
        lambda: arrival_rate,
        mu: service_rate,
        k: server_num_threads,
        queue_capacity,
        duration,
        timeout: None,
        max_retries: None,
        base_retry_delay: None,
    };

    MmkResults {
        config,
        requests_sent,
        requests_completed,
        requests_dropped,
        avg_response_time_ms,
        p95_response_time_ms,
        p99_response_time_ms,
        throughput_rps,
        server_utilization: final_server.utilization(),
        avg_queue_depth: final_server.queue_depth() as f64,
        success_rate,
        theoretical_utilization,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== M/M/k Queueing System Examples ===\n");
    
    let duration = Duration::from_secs(2); // 5 seconds for demo
    
    // Test: Constant Arrival Debug - useful for debugging with predictable timing
    println!("\n🔧 === Constant Arrival Debug Test ===");
    let debug_results = run_constant_arrival_debug(
        Duration::from_millis(100), 
        2, 
        2.0, 
        Duration::from_secs(5),
        Some(25), 
        Some(5), 
    );
    debug_results.print_summary();


    // Test 1: Classic M/M/1 queue with bounded queue (capacity 20)
    let mm1_config = MmkConfig::mm1(5.0, 8.0, duration, Some(20)); 
    let mm1a_results = run_classic_mm1_queue(mm1_config);
    
    let mm1_config = MmkConfig::mm1(5.0, 8.0, duration, Some(50)); 
    let mm1b_results = run_classic_mm1_queue(mm1_config);

    let mm1_config = MmkConfig::mmk(16.0, 20.0, 1, duration, Some(50));
    let mm1c_results = run_classic_mm1_queue(mm1_config);

    let mm1_config = MmkConfig::mmk(16.0, 10.0, 2, duration, Some(50));
    let mm1d_results = run_classic_mmk_queue(mm1_config);

    mm1a_results.print_summary();
    mm1b_results.print_summary();
    mm1c_results.print_summary();
    mm1d_results.print_summary();

    

    // Test 2: Classic M/M/k queue (k=3) with larger queue (capacity 100)
    let mmk_config = MmkConfig::mmk(12.0, 5.0, 3, duration, Some(100)); // λ=12, μ=5, k=3, queue=100
    let mmk_results = run_classic_mmk_queue(mmk_config);
    mmk_results.print_summary();
    
    // Test 3: M/M/1 with fixed retry policy and small queue (capacity 10)
    let mm1_retry_config = MmkConfig::mm1_with_retry(
        6.0, 8.0, duration, 
        Duration::from_millis(500), // 500ms timeout
        3, // 3 retries
        Some(10) // Small queue capacity
    );
    let mm1_retry_results = run_mm1_with_retry(mm1_retry_config);
    mm1_retry_results.print_summary();
    
    // Test 4: M/M/1 with exponential backoff and no queue (capacity 0)
    let mm1_backoff_config = MmkConfig::mm1_with_exp_backoff(
        6.0, 8.0, duration,
        Duration::from_millis(400), // 400ms timeout
        4, // 4 retries
        Duration::from_millis(50), // 50ms base delay
        Some(0) // No queue - reject immediately when server busy
    );
    let mm1_backoff_results = run_mm1_with_exp_backoff(mm1_backoff_config);
    mm1_backoff_results.print_summary();
    
    println!("\n=== Summary Comparison ===");
    println!("| Scenario              | Queue | Throughput | Avg Latency | Success Rate | Utilization |");
    println!("|----------------------|-------|------------|-------------|--------------|-------------|");
    println!("| Classic M/M/1        | {:5} | {:8.2} | {:9.2} | {:10.1}% | {:9.1}% |", 
             format_queue_capacity(mm1a_results.config.queue_capacity),
             mm1a_results.throughput_rps, mm1a_results.avg_response_time_ms, 
             mm1a_results.success_rate * 100.0, mm1a_results.server_utilization * 100.0);
    println!("| Classic M/M/1        | {:5} | {:8.2} | {:9.2} | {:10.1}% | {:9.1}% |", 
             format_queue_capacity(mm1b_results.config.queue_capacity),
             mm1b_results.throughput_rps, mm1b_results.avg_response_time_ms, 
             mm1b_results.success_rate * 100.0, mm1b_results.server_utilization * 100.0);
    println!("| Classic M/M/{}        | {:5} | {:8.2} | {:9.2} | {:10.1}% | {:9.1}% |", 
             mmk_results.config.k,
             format_queue_capacity(mmk_results.config.queue_capacity),
             mmk_results.throughput_rps, mmk_results.avg_response_time_ms,
             mmk_results.success_rate * 100.0, mmk_results.server_utilization * 100.0);
    println!("| M/M/1 + Fixed Retry  | {:5} | {:8.2} | {:9.2} | {:10.1}% | {:9.1}% |", 
             format_queue_capacity(mm1_retry_results.config.queue_capacity),
             mm1_retry_results.throughput_rps, mm1_retry_results.avg_response_time_ms,
             mm1_retry_results.success_rate * 100.0, mm1_retry_results.server_utilization * 100.0);
    println!("| M/M/1 + Exp Backoff  | {:5} | {:8.2} | {:9.2} | {:10.1}% | {:9.1}% |", 
             format_queue_capacity(mm1_backoff_results.config.queue_capacity),
             mm1_backoff_results.throughput_rps, mm1_backoff_results.avg_response_time_ms,
             mm1_backoff_results.success_rate * 100.0, mm1_backoff_results.server_utilization * 100.0);
    println!("| Constant Arrival     | {:5} | {:8.2} | {:9.2} | {:10.1}% | {:9.1}% |", 
             format_queue_capacity(debug_results.config.queue_capacity),
             debug_results.throughput_rps, debug_results.avg_response_time_ms,
             debug_results.success_rate * 100.0, debug_results.server_utilization * 100.0);
    
    println!("\n✅ All M/M/k queueing system examples completed!");
    println!("\n📊 Time-series charts generated in target/mmk_charts/:");
    println!("  - mm1_classic_time_series.png");
    println!("  - mm3_classic_time_series.png");
    println!("  - constant_arrival_debug_time_series.png");
    println!("  - mm1_with_retry_time_series.png (if retry examples are updated)");
    println!("  - mm1_with_exp_backoff_time_series.png (if backoff examples are updated)");
    println!("\nCharts show:");
    println!("  • Average latency over time (with exponential moving average)");
    println!("  • Average queue size over time");
    println!("  • Request timeout rate over time");
    println!("  • 100ms aggregation window with smoothing");
    println!("\n🔧 Debug Features:");
    println!("  • ConstantArrivalClient: Sends requests at fixed intervals for predictable debugging");
    println!("  • run_constant_arrival_debug(): Helper function for debugging scenarios");
    
    Ok(())
}

/// Helper function to format queue capacity for display
fn format_queue_capacity(capacity: Option<usize>) -> String {
    match capacity {
        None => "∞".to_string(),
        Some(0) => "0".to_string(),
        Some(n) => n.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mmk_config_creation() {
        let config = MmkConfig::mm1(5.0, 10.0, Duration::from_secs(30), Some(50));
        assert_eq!(config.lambda, 5.0);
        assert_eq!(config.mu, 10.0);
        assert_eq!(config.k, 1);
        assert_eq!(config.queue_capacity, Some(50));
        assert!(config.is_stable());
        assert_eq!(config.utilization(), 0.5);
    }

    #[test]
    fn test_mmk_config_with_custom_queue() {
        let config = MmkConfig::mm1(5.0, 10.0, Duration::from_secs(30), Some(100));
        assert_eq!(config.queue_capacity, Some(100));
        
        let config_no_queue = MmkConfig::mm1(5.0, 10.0, Duration::from_secs(30), Some(0));
        assert_eq!(config_no_queue.queue_capacity, Some(0));
        
        let config_unlimited = MmkConfig::mm1(5.0, 10.0, Duration::from_secs(30), None);
        assert_eq!(config_unlimited.queue_capacity, None);
    }

    #[test]
    fn test_mmk_config_stability() {
        let stable_config = MmkConfig::mmk(8.0, 5.0, 2, Duration::from_secs(10), Some(50)); // ρ = 8/(2*5) = 0.8
        assert!(stable_config.is_stable());
        
        let unstable_config = MmkConfig::mmk(12.0, 5.0, 2, Duration::from_secs(10), Some(50)); // ρ = 12/(2*5) = 1.2
        assert!(!unstable_config.is_stable());
    }

    #[test]
    fn test_format_queue_capacity() {
        assert_eq!(format_queue_capacity(None), "∞");
        assert_eq!(format_queue_capacity(Some(0)), "0");
        assert_eq!(format_queue_capacity(Some(50)), "50");
    }

    #[test]
    fn test_classic_mm1_simulation() {
        let config = MmkConfig::mm1(2.0, 5.0, Duration::from_secs(5), Some(50)); // Short simulation
        let results = run_classic_mm1_queue(config);
        
        assert!(results.requests_sent > 0);
        assert!(results.theoretical_utilization < 1.0); // Should be stable
        assert!(results.success_rate >= 0.0 && results.success_rate <= 1.0);
    }

    #[test]
    fn test_classic_mmk_simulation() {
        let config = MmkConfig::mmk(6.0, 4.0, 2, Duration::from_secs(5), Some(50)); // Short simulation
        let results = run_classic_mmk_queue(config);
        
        assert!(results.requests_sent > 0);
        assert_eq!(results.config.k, 2);
        assert!(results.theoretical_utilization < 1.0); // Should be stable
    }

    #[test]
    fn test_mm1_with_retry_simulation() {
        let config = MmkConfig::mm1_with_retry(
            3.0, 6.0, Duration::from_secs(5),
            Duration::from_millis(200), 2, Some(50)
        );
        let results = run_mm1_with_retry(config);
        
        assert!(results.requests_sent > 0);
        assert!(results.success_rate >= 0.0);
    }

    #[test]
    fn test_mm1_with_exp_backoff_simulation() {
        let config = MmkConfig::mm1_with_exp_backoff(
            5.5, 6.0, Duration::from_secs(5),
            Duration::from_millis(200), 3, Duration::from_millis(0), Some(500)
        );
        let results = run_mm1_with_exp_backoff(config);
        
        assert!(results.requests_sent > 0);
        assert!(results.success_rate >= 0.0);
    }

    #[test]
    fn test_constant_arrival_debug_simulation() {
        let results = run_constant_arrival_debug(
            Duration::from_millis(100), // 100ms intervals (10 req/s)
            1, // Single server
            Duration::from_millis(50), // 50ms service time (20 req/s capacity)
            Duration::from_secs(1), // 1 second simulation
            Some(5), // Send exactly 5 requests
            Some(10), // Small queue
        );
        
        assert!(results.requests_sent > 0);
        assert!(results.success_rate >= 0.0);
        assert!(results.theoretical_utilization < 1.0); // Should be stable (10/20 = 0.5)
    }

    #[test]
    fn test_constant_arrival_client_from_rate() {
        use std::sync::{Arc, Mutex};
        use des_metrics::{SimulationMetrics, MmkTimeSeriesMetrics};
        use des_core::{Simulation, Key};
        use crate::ServerEvent;
        
        let metrics = Arc::new(Mutex::new(SimulationMetrics::new()));
        let time_series_metrics = Arc::new(Mutex::new(MmkTimeSeriesMetrics::new(
            Duration::from_millis(100),
            0.1,
        )));
        
        let simulation = Simulation::default();
        let server_key = Key::<ServerEvent>::new();
        
        // Test creating from rate
        let client = ConstantArrivalClient::from_rate(
            "test-client".to_string(),
            server_key,
            5.0, // 5 requests per second
            metrics,
            time_series_metrics,
        );
        
        // Should have 200ms inter-arrival time (1/5 = 0.2 seconds)
        assert_eq!(client.inter_arrival_time, Duration::from_millis(200));
        assert_eq!(client.name, "test-client");
        assert_eq!(client.requests_sent, 0);
        assert_eq!(client.next_request_id, 1);
    }
}