# Advanced Workload Generation Patterns

Real-world systems experience complex traffic patterns that go beyond simple constant or exponential arrival rates. This section demonstrates advanced workload generation patterns including seasonal traffic, bursty workloads, multi-tenant scenarios, and stateful client behaviors.

## Overview

Advanced workload patterns include:

- **Seasonal Traffic Patterns**: Daily, weekly, and monthly traffic cycles
- **Bursty Workloads**: Traffic spikes and flash crowds
- **Multi-Tenant Workloads**: Different tenants with varying characteristics
- **Stateful Client Behaviors**: Clients that maintain session state
- **Correlated Request Patterns**: Requests that depend on previous responses
- **Geographically Distributed Traffic**: Clients from different time zones

All examples in this section are implemented as runnable tests in `des-components/tests/advanced_examples.rs`.

## Seasonal Traffic Patterns

Seasonal patterns model predictable traffic variations based on time of day, day of week, or other cyclical factors.

### Sinusoidal Traffic Pattern

```rust
use std::f64::consts::PI;

pub struct SeasonalTrafficClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub base_rate: f64,           // Base requests per second
    pub amplitude: f64,           // Peak variation from base
    pub period: Duration,         // Length of one complete cycle
    pub phase_offset: f64,        // Phase shift (0.0 to 2π)
    pub simulation_start: SimTime,
    pub next_request_id: u64,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub metrics: Arc<Mutex<ClientMetrics>>,
    pub last_request_time: Option<SimTime>,
}

impl SeasonalTrafficClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        base_rate: f64,
        amplitude: f64,
        period: Duration,
        phase_offset: f64,
        simulation_start: SimTime,
        metrics: Arc<Mutex<ClientMetrics>>,
    ) -> Self {
        Self {
            name,
            server_key,
            base_rate,
            amplitude,
            period,
            phase_offset,
            simulation_start,
            next_request_id: 1,
            request_start_times: HashMap::new(),
            metrics,
            last_request_time: None,
        }
    }

    /// Calculate current request rate based on seasonal pattern
    fn current_rate(&self, current_time: SimTime) -> f64 {
        let elapsed = current_time.duration_since(self.simulation_start);
        let elapsed_secs = elapsed.as_secs_f64();
        let period_secs = self.period.as_secs_f64();
        
        // Sinusoidal pattern: rate = base + amplitude * sin(2π * t / period + phase)
        let angle = 2.0 * PI * elapsed_secs / period_secs + self.phase_offset;
        let seasonal_factor = angle.sin();
        
        (self.base_rate + self.amplitude * seasonal_factor).max(0.1) // Minimum 0.1 RPS
    }

    fn should_send_request(&self, current_time: SimTime) -> bool {
        if let Some(last_time) = self.last_request_time {
            let elapsed = current_time.duration_since(last_time);
            let current_rate = self.current_rate(current_time);
            let required_interval = Duration::from_secs_f64(1.0 / current_rate);
            elapsed >= required_interval
        } else {
            true // First request
        }
    }
}

impl Component for SeasonalTrafficClient {
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
                    let current_rate = self.current_rate(scheduler.time());
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
                        format!("Seasonal request {} at {:.2} RPS", request_id.0, current_rate).into_bytes(),
                    );

                    // Record metrics
                    {
                        let mut metrics = self.metrics.lock().unwrap();
                        metrics.requests_sent += 1;
                        metrics.current_rate = current_rate;
                    }

                    scheduler.schedule_now(
                        self.server_key,
                        ServerEvent::ProcessRequest {
                            attempt,
                            client_id: self_id,
                        },
                    );

                    println!(
                        "[{}] [{}] Sent seasonal request {} (rate: {:.2} RPS)",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        request_id.0,
                        current_rate
                    );
                }

                // Schedule next attempt based on current rate
                let current_rate = self.current_rate(scheduler.time());
                let next_interval = Duration::from_secs_f64(1.0 / current_rate);
                scheduler.schedule(
                    SimTime::from_duration(next_interval),
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
                        "[{}] [{}] Response for seasonal request {} ({}ms)",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        response.request_id.0,
                        latency.as_millis()
                    );
                }
            }
            _ => {}
        }
    }
}
```

### Daily Traffic Cycle

```rust
pub struct DailyTrafficClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub peak_hours: Vec<(u32, f64)>, // (hour, rate_multiplier) pairs
    pub base_rate: f64,
    pub simulation_start: SimTime,
    pub next_request_id: u64,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub metrics: Arc<Mutex<ClientMetrics>>,
    pub last_request_time: Option<SimTime>,
}

impl DailyTrafficClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        base_rate: f64,
        peak_hours: Vec<(u32, f64)>,
        simulation_start: SimTime,
        metrics: Arc<Mutex<ClientMetrics>>,
    ) -> Self {
        Self {
            name,
            server_key,
            peak_hours,
            base_rate,
            simulation_start,
            next_request_id: 1,
            request_start_times: HashMap::new(),
            metrics,
            last_request_time: None,
        }
    }

    /// Calculate current rate based on simulated hour of day
    fn current_rate(&self, current_time: SimTime) -> f64 {
        let elapsed = current_time.duration_since(self.simulation_start);
        let elapsed_hours = elapsed.as_secs_f64() / 3600.0;
        let hour_of_day = (elapsed_hours % 24.0) as u32;

        // Find the rate multiplier for current hour
        let multiplier = self.peak_hours.iter()
            .find(|(hour, _)| *hour == hour_of_day)
            .map(|(_, multiplier)| *multiplier)
            .unwrap_or(1.0); // Default multiplier

        self.base_rate * multiplier
    }
}

impl Component for DailyTrafficClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                let current_rate = self.current_rate(scheduler.time());
                
                if let Some(last_time) = self.last_request_time {
                    let elapsed = scheduler.time().duration_since(last_time);
                    let required_interval = Duration::from_secs_f64(1.0 / current_rate);
                    
                    if elapsed >= required_interval {
                        // Send request logic similar to SeasonalTrafficClient
                        let request_id = RequestId(self.next_request_id);
                        self.next_request_id += 1;
                        
                        // ... (request sending logic)
                        
                        self.last_request_time = Some(scheduler.time());
                    }
                } else {
                    // First request
                    self.last_request_time = Some(scheduler.time());
                }

                // Schedule next attempt
                let next_interval = Duration::from_secs_f64(1.0 / current_rate);
                scheduler.schedule(
                    SimTime::from_duration(next_interval),
                    self_id,
                    ClientEvent::SendRequest,
                );
            }
            _ => {
                // Handle other events similar to SeasonalTrafficClient
            }
        }
    }
}
```

## Bursty Workload Patterns

Bursty workloads simulate sudden traffic spikes, flash crowds, and other irregular traffic patterns.

### Flash Crowd Client

```rust
pub struct FlashCrowdClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub normal_rate: f64,
    pub burst_rate: f64,
    pub burst_duration: Duration,
    pub burst_intervals: Vec<(SimTime, SimTime)>, // (start, end) times for bursts
    pub current_burst_index: usize,
    pub simulation_start: SimTime,
    pub next_request_id: u64,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub metrics: Arc<Mutex<ClientMetrics>>,
    pub last_request_time: Option<SimTime>,
}

impl FlashCrowdClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        normal_rate: f64,
        burst_rate: f64,
        burst_duration: Duration,
        burst_start_times: Vec<SimTime>,
        simulation_start: SimTime,
        metrics: Arc<Mutex<ClientMetrics>>,
    ) -> Self {
        // Convert burst start times to intervals
        let burst_intervals = burst_start_times.into_iter()
            .map(|start| (start, SimTime::from_duration(start.as_duration() + burst_duration)))
            .collect();

        Self {
            name,
            server_key,
            normal_rate,
            burst_rate,
            burst_duration,
            burst_intervals,
            current_burst_index: 0,
            simulation_start,
            next_request_id: 1,
            request_start_times: HashMap::new(),
            metrics,
            last_request_time: None,
        }
    }

    fn is_in_burst(&self, current_time: SimTime) -> bool {
        self.burst_intervals.iter().any(|(start, end)| {
            current_time >= *start && current_time <= *end
        })
    }

    fn current_rate(&self, current_time: SimTime) -> f64 {
        if self.is_in_burst(current_time) {
            self.burst_rate
        } else {
            self.normal_rate
        }
    }
}

impl Component for FlashCrowdClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                let current_rate = self.current_rate(scheduler.time());
                let in_burst = self.is_in_burst(scheduler.time());
                
                if let Some(last_time) = self.last_request_time {
                    let elapsed = scheduler.time().duration_since(last_time);
                    let required_interval = Duration::from_secs_f64(1.0 / current_rate);
                    
                    if elapsed >= required_interval {
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
                            format!("Flash crowd request {} ({})", 
                                   request_id.0, 
                                   if in_burst { "BURST" } else { "normal" }).into_bytes(),
                        );

                        // Record metrics
                        {
                            let mut metrics = self.metrics.lock().unwrap();
                            metrics.requests_sent += 1;
                            if in_burst {
                                metrics.burst_requests += 1;
                            }
                        }

                        scheduler.schedule_now(
                            self.server_key,
                            ServerEvent::ProcessRequest {
                                attempt,
                                client_id: self_id,
                            },
                        );

                        println!(
                            "[{}] [{}] Sent flash crowd request {} ({}, rate: {:.2} RPS)",
                            scheduler.time().as_duration().as_millis(),
                            self.name,
                            request_id.0,
                            if in_burst { "BURST" } else { "normal" },
                            current_rate
                        );
                    }
                } else {
                    // First request
                    self.last_request_time = Some(scheduler.time());
                }

                // Schedule next attempt
                let next_interval = Duration::from_secs_f64(1.0 / current_rate);
                scheduler.schedule(
                    SimTime::from_duration(next_interval),
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
                        "[{}] [{}] Response for flash crowd request {} ({}ms)",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        response.request_id.0,
                        latency.as_millis()
                    );
                }
            }
            _ => {}
        }
    }
}
```

## Multi-Tenant Workload Patterns

Multi-tenant systems serve different types of clients with varying characteristics and requirements.

### Multi-Tenant Client Manager

```rust
#[derive(Debug, Clone)]
pub struct TenantProfile {
    pub name: String,
    pub base_rate: f64,
    pub burst_probability: f64,
    pub burst_multiplier: f64,
    pub priority: RequestPriority,
    pub retry_policy: RetryBehavior,
}

#[derive(Debug, Clone)]
pub enum RetryBehavior {
    NoRetry,
    FixedRetry { attempts: u32, delay: Duration },
    ExponentialBackoff { base_delay: Duration, max_delay: Duration },
}

pub struct MultiTenantClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub tenants: Vec<TenantProfile>,
    pub tenant_states: HashMap<String, TenantState>,
    pub next_request_id: u64,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub metrics: Arc<Mutex<MultiTenantMetrics>>,
    pub rng: rand::rngs::ThreadRng,
}

#[derive(Debug)]
struct TenantState {
    last_request_time: Option<SimTime>,
    current_rate: f64,
    in_burst: bool,
    burst_end_time: Option<SimTime>,
}

impl MultiTenantClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        tenants: Vec<TenantProfile>,
        metrics: Arc<Mutex<MultiTenantMetrics>>,
    ) -> Self {
        let mut tenant_states = HashMap::new();
        for tenant in &tenants {
            tenant_states.insert(tenant.name.clone(), TenantState {
                last_request_time: None,
                current_rate: tenant.base_rate,
                in_burst: false,
                burst_end_time: None,
            });
        }

        Self {
            name,
            server_key,
            tenants,
            tenant_states,
            next_request_id: 1,
            request_start_times: HashMap::new(),
            metrics,
            rng: rand::thread_rng(),
        }
    }

    fn update_tenant_state(&mut self, tenant: &TenantProfile, current_time: SimTime) {
        let state = self.tenant_states.get_mut(&tenant.name).unwrap();

        // Check if burst should end
        if let Some(burst_end) = state.burst_end_time {
            if current_time >= burst_end {
                state.in_burst = false;
                state.burst_end_time = None;
                state.current_rate = tenant.base_rate;
                
                println!(
                    "[{}] Tenant {} burst ended, returning to {:.2} RPS",
                    current_time.as_duration().as_millis(),
                    tenant.name,
                    tenant.base_rate
                );
            }
        }

        // Check if burst should start
        if !state.in_burst && self.rng.gen::<f64>() < tenant.burst_probability {
            state.in_burst = true;
            state.current_rate = tenant.base_rate * tenant.burst_multiplier;
            state.burst_end_time = Some(SimTime::from_duration(
                current_time.as_duration() + Duration::from_secs(30) // 30 second bursts
            ));
            
            println!(
                "[{}] Tenant {} burst started, rate: {:.2} RPS",
                current_time.as_duration().as_millis(),
                tenant.name,
                state.current_rate
            );
        }
    }

    fn should_send_request(&self, tenant: &TenantProfile, current_time: SimTime) -> bool {
        let state = self.tenant_states.get(&tenant.name).unwrap();
        
        if let Some(last_time) = state.last_request_time {
            let elapsed = current_time.duration_since(last_time);
            let required_interval = Duration::from_secs_f64(1.0 / state.current_rate);
            elapsed >= required_interval
        } else {
            true // First request for this tenant
        }
    }
}

impl Component for MultiTenantClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                // Process each tenant
                for tenant in self.tenants.clone() {
                    self.update_tenant_state(&tenant, scheduler.time());
                    
                    if self.should_send_request(&tenant, scheduler.time()) {
                        let request_id = RequestId(self.next_request_id);
                        let attempt_id = RequestAttemptId(self.next_request_id);
                        self.next_request_id += 1;

                        // Update tenant state
                        let state = self.tenant_states.get_mut(&tenant.name).unwrap();
                        state.last_request_time = Some(scheduler.time());

                        // Record start time
                        self.request_start_times.insert(request_id, scheduler.time());

                        let attempt = RequestAttempt::new(
                            attempt_id,
                            request_id,
                            1,
                            scheduler.time(),
                            format!("Tenant {} request {} (priority: {:?})", 
                                   tenant.name, request_id.0, tenant.priority).into_bytes(),
                        );

                        // Record metrics
                        {
                            let mut metrics = self.metrics.lock().unwrap();
                            metrics.record_request(&tenant.name, state.current_rate, state.in_burst);
                        }

                        scheduler.schedule_now(
                            self.server_key,
                            ServerEvent::ProcessRequest {
                                attempt,
                                client_id: self_id,
                            },
                        );

                        println!(
                            "[{}] [{}] Sent tenant {} request {} (rate: {:.2} RPS, {})",
                            scheduler.time().as_duration().as_millis(),
                            self.name,
                            tenant.name,
                            request_id.0,
                            state.current_rate,
                            if state.in_burst { "BURST" } else { "normal" }
                        );
                    }
                }

                // Schedule next processing cycle
                scheduler.schedule(
                    SimTime::from_duration(Duration::from_millis(100)), // Check every 100ms
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
                        metrics.record_response(latency, response.is_success());
                    }

                    println!(
                        "[{}] [{}] Multi-tenant response for request {} ({}ms, success: {})",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        response.request_id.0,
                        latency.as_millis(),
                        response.is_success()
                    );
                }
            }
            _ => {}
        }
    }
}
```

## Stateful Client Behaviors

Stateful clients maintain session state and exhibit behaviors that depend on previous interactions.

### Session-Based Client

```rust
#[derive(Debug, Clone)]
pub enum SessionState {
    Idle,
    Active { session_id: String, requests_sent: u32 },
    Cooldown { end_time: SimTime },
}

pub struct StatefulSessionClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub session_state: SessionState,
    pub session_duration: Duration,
    pub requests_per_session: u32,
    pub inter_request_delay: Duration,
    pub cooldown_duration: Duration,
    pub next_request_id: u64,
    pub next_session_id: u64,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub metrics: Arc<Mutex<ClientMetrics>>,
}

impl StatefulSessionClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        session_duration: Duration,
        requests_per_session: u32,
        inter_request_delay: Duration,
        cooldown_duration: Duration,
        metrics: Arc<Mutex<ClientMetrics>>,
    ) -> Self {
        Self {
            name,
            server_key,
            session_state: SessionState::Idle,
            session_duration,
            requests_per_session,
            inter_request_delay,
            cooldown_duration,
            next_request_id: 1,
            next_session_id: 1,
            request_start_times: HashMap::new(),
            metrics,
        }
    }

    fn start_new_session(&mut self, current_time: SimTime) {
        let session_id = format!("session-{}", self.next_session_id);
        self.next_session_id += 1;
        
        self.session_state = SessionState::Active {
            session_id: session_id.clone(),
            requests_sent: 0,
        };

        println!(
            "[{}] [{}] Started new session: {}",
            current_time.as_duration().as_millis(),
            self.name,
            session_id
        );
    }

    fn end_session(&mut self, current_time: SimTime) {
        if let SessionState::Active { session_id, requests_sent } = &self.session_state {
            println!(
                "[{}] [{}] Ended session: {} ({} requests sent)",
                current_time.as_duration().as_millis(),
                self.name,
                session_id,
                requests_sent
            );
        }

        self.session_state = SessionState::Cooldown {
            end_time: SimTime::from_duration(current_time.as_duration() + self.cooldown_duration),
        };
    }

    fn update_state(&mut self, current_time: SimTime) {
        match &self.session_state {
            SessionState::Cooldown { end_time } => {
                if current_time >= *end_time {
                    self.session_state = SessionState::Idle;
                    println!(
                        "[{}] [{}] Cooldown ended, ready for new session",
                        current_time.as_duration().as_millis(),
                        self.name
                    );
                }
            }
            SessionState::Active { requests_sent, .. } => {
                if *requests_sent >= self.requests_per_session {
                    self.end_session(current_time);
                }
            }
            SessionState::Idle => {
                // Ready to start new session
            }
        }
    }
}

impl Component for StatefulSessionClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                self.update_state(scheduler.time());

                match &mut self.session_state {
                    SessionState::Idle => {
                        self.start_new_session(scheduler.time());
                        // Schedule first request of session
                        scheduler.schedule(
                            SimTime::from_duration(self.inter_request_delay),
                            self_id,
                            ClientEvent::SendRequest,
                        );
                    }
                    SessionState::Active { session_id, requests_sent } => {
                        let request_id = RequestId(self.next_request_id);
                        let attempt_id = RequestAttemptId(self.next_request_id);
                        self.next_request_id += 1;
                        *requests_sent += 1;

                        // Record start time
                        self.request_start_times.insert(request_id, scheduler.time());

                        let attempt = RequestAttempt::new(
                            attempt_id,
                            request_id,
                            1,
                            scheduler.time(),
                            format!("Session {} request {} ({})", 
                                   session_id, request_id.0, *requests_sent).into_bytes(),
                        );

                        // Record metrics
                        {
                            let mut metrics = self.metrics.lock().unwrap();
                            metrics.requests_sent += 1;
                            metrics.session_requests += 1;
                        }

                        scheduler.schedule_now(
                            self.server_key,
                            ServerEvent::ProcessRequest {
                                attempt,
                                client_id: self_id,
                            },
                        );

                        println!(
                            "[{}] [{}] Sent session request {} (session: {}, #{}/{})",
                            scheduler.time().as_duration().as_millis(),
                            self.name,
                            request_id.0,
                            session_id,
                            *requests_sent,
                            self.requests_per_session
                        );

                        // Schedule next request if session continues
                        if *requests_sent < self.requests_per_session {
                            scheduler.schedule(
                                SimTime::from_duration(self.inter_request_delay),
                                self_id,
                                ClientEvent::SendRequest,
                            );
                        }
                    }
                    SessionState::Cooldown { .. } => {
                        // Schedule check for end of cooldown
                        scheduler.schedule(
                            SimTime::from_duration(Duration::from_millis(100)),
                            self_id,
                            ClientEvent::SendRequest,
                        );
                    }
                }
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
                        "[{}] [{}] Session response for request {} ({}ms, success: {})",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        response.request_id.0,
                        latency.as_millis(),
                        response.is_success()
                    );
                }
            }
            _ => {}
        }
    }
}
```

## Correlated Request Patterns

Some clients generate requests that depend on responses from previous requests, creating correlated traffic patterns.

### Dependent Request Client

```rust
#[derive(Debug, Clone)]
pub struct RequestDependency {
    pub trigger_response_pattern: String, // Pattern to match in response
    pub dependent_requests: Vec<String>,  // Requests to send when pattern matches
    pub delay: Duration,                  // Delay before sending dependent requests
}

pub struct CorrelatedRequestClient {
    pub name: String,
    pub server_key: Key<ServerEvent>,
    pub dependencies: Vec<RequestDependency>,
    pub pending_dependencies: VecDeque<(SimTime, Vec<String>)>, // (send_time, requests)
    pub next_request_id: u64,
    pub request_start_times: HashMap<RequestId, SimTime>,
    pub metrics: Arc<Mutex<ClientMetrics>>,
}

impl CorrelatedRequestClient {
    pub fn new(
        name: String,
        server_key: Key<ServerEvent>,
        dependencies: Vec<RequestDependency>,
        metrics: Arc<Mutex<ClientMetrics>>,
    ) -> Self {
        Self {
            name,
            server_key,
            dependencies,
            pending_dependencies: VecDeque::new(),
            next_request_id: 1,
            request_start_times: HashMap::new(),
            metrics,
        }
    }

    fn check_response_for_dependencies(&mut self, response: &Response, current_time: SimTime) {
        let response_body = String::from_utf8_lossy(&response.body);
        
        for dependency in &self.dependencies {
            if response_body.contains(&dependency.trigger_response_pattern) {
                let send_time = SimTime::from_duration(current_time.as_duration() + dependency.delay);
                self.pending_dependencies.push_back((send_time, dependency.dependent_requests.clone()));
                
                println!(
                    "[{}] [{}] Triggered dependency: {} dependent requests in {}ms",
                    current_time.as_duration().as_millis(),
                    self.name,
                    dependency.dependent_requests.len(),
                    dependency.delay.as_millis()
                );
            }
        }
    }

    fn process_pending_dependencies(&mut self, scheduler: &mut Scheduler, self_key: Key<ClientEvent>) {
        let current_time = scheduler.time();
        
        while let Some((send_time, _)) = self.pending_dependencies.front() {
            if current_time >= *send_time {
                let (_, requests) = self.pending_dependencies.pop_front().unwrap();
                
                for request_type in requests {
                    self.send_dependent_request(request_type, scheduler, self_key);
                }
            } else {
                break; // No more ready dependencies
            }
        }
    }

    fn send_dependent_request(&mut self, request_type: String, scheduler: &mut Scheduler, self_key: Key<ClientEvent>) {
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
            format!("Dependent request {} (type: {})", request_id.0, request_type).into_bytes(),
        );

        // Record metrics
        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.requests_sent += 1;
            metrics.dependent_requests += 1;
        }

        scheduler.schedule_now(
            self.server_key,
            ServerEvent::ProcessRequest {
                attempt,
                client_id: self_key,
            },
        );

        println!(
            "[{}] [{}] Sent dependent request {} (type: {})",
            scheduler.time().as_duration().as_millis(),
            self.name,
            request_id.0,
            request_type
        );
    }
}

impl Component for CorrelatedRequestClient {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                // Process any pending dependencies
                self.process_pending_dependencies(scheduler, self_id);
                
                // Schedule next dependency check
                scheduler.schedule(
                    SimTime::from_duration(Duration::from_millis(100)),
                    self_id,
                    ClientEvent::SendRequest,
                );
            }
            ClientEvent::ResponseReceived { response } => {
                if let Some(start_time) = self.request_start_times.remove(&response.request_id) {
                    let latency = scheduler.time().duration_since(start_time);

                    // Check for dependencies triggered by this response
                    self.check_response_for_dependencies(response, scheduler.time());

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
                        "[{}] [{}] Correlated response for request {} ({}ms, success: {})",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        response.request_id.0,
                        latency.as_millis(),
                        response.is_success()
                    );
                }
            }
            _ => {}
        }
    }
}
```

## Performance Analysis

### Workload Pattern Metrics

```rust
#[derive(Debug, Default, Clone)]
pub struct WorkloadPatternMetrics {
    pub requests_sent: u64,
    pub responses_received: u64,
    pub successful_responses: u64,
    pub total_latency: Duration,
    pub burst_requests: u64,
    pub session_requests: u64,
    pub dependent_requests: u64,
    pub current_rate: f64,
    pub peak_rate: f64,
    pub pattern_type: String,
}

impl WorkloadPatternMetrics {
    pub fn record_request(&mut self, rate: f64, is_burst: bool) {
        self.requests_sent += 1;
        self.current_rate = rate;
        if rate > self.peak_rate {
            self.peak_rate = rate;
        }
        if is_burst {
            self.burst_requests += 1;
        }
    }

    pub fn average_latency(&self) -> Duration {
        if self.responses_received > 0 {
            self.total_latency / self.responses_received as u32
        } else {
            Duration::zero()
        }
    }

    pub fn success_rate(&self) -> f64 {
        if self.responses_received > 0 {
            self.successful_responses as f64 / self.responses_received as f64
        } else {
            0.0
        }
    }

    pub fn burst_ratio(&self) -> f64 {
        if self.requests_sent > 0 {
            self.burst_requests as f64 / self.requests_sent as f64
        } else {
            0.0
        }
    }
}
```

### Comparing Workload Patterns

```rust
pub fn compare_workload_patterns() -> Vec<WorkloadPatternResults> {
    let patterns = vec![
        ("Seasonal Traffic", run_seasonal_pattern_test()),
        ("Flash Crowd", run_flash_crowd_test()),
        ("Multi-Tenant", run_multi_tenant_test()),
        ("Session-Based", run_session_based_test()),
        ("Correlated Requests", run_correlated_requests_test()),
    ];

    patterns.into_iter().map(|(name, result)| {
        WorkloadPatternResults {
            pattern: name.to_string(),
            requests_sent: result.requests_sent,
            peak_rate: result.peak_rate,
            burst_ratio: result.burst_ratio(),
            avg_latency: result.average_latency(),
            success_rate: result.success_rate(),
            complexity_score: calculate_complexity_score(&result),
        }
    }).collect()
}

fn calculate_complexity_score(metrics: &WorkloadPatternMetrics) -> f64 {
    // Score based on rate variation, burst frequency, and dependencies
    let rate_variation = metrics.peak_rate / metrics.current_rate.max(1.0);
    let burst_factor = metrics.burst_ratio() * 2.0;
    let dependency_factor = if metrics.dependent_requests > 0 { 1.5 } else { 1.0 };
    
    (rate_variation + burst_factor) * dependency_factor
}
```

## Best Practices

### Pattern Selection Guidelines

1. **Seasonal Traffic**: Use for predictable cyclical patterns
   - Daily business hours
   - Weekly patterns (weekday vs weekend)
   - Monthly billing cycles

2. **Flash Crowds**: Use for sudden traffic spikes
   - Product launches
   - Breaking news events
   - Social media viral content

3. **Multi-Tenant**: Use for SaaS applications
   - Different customer tiers
   - Varying usage patterns per tenant
   - Priority-based resource allocation

4. **Session-Based**: Use for user interaction patterns
   - Web application sessions
   - Gaming sessions
   - Batch processing jobs

5. **Correlated Requests**: Use for dependent operations
   - Microservices call chains
   - Database transaction patterns
   - Workflow orchestration

### Configuration Guidelines

```rust
// Seasonal Pattern Configuration
let seasonal_config = SeasonalConfig {
    base_rate: 10.0,           // 10 RPS baseline
    amplitude: 8.0,            // ±8 RPS variation
    period: Duration::from_hours(24), // Daily cycle
    phase_offset: 0.0,         // Peak at noon
};

// Flash Crowd Configuration
let flash_crowd_config = FlashCrowdConfig {
    normal_rate: 5.0,          // 5 RPS normal
    burst_rate: 50.0,          // 50 RPS during burst
    burst_duration: Duration::from_minutes(5), // 5-minute bursts
    burst_frequency: Duration::from_hours(2),  // Every 2 hours
};

// Multi-Tenant Configuration
let tenant_configs = vec![
    TenantProfile {
        name: "free-tier".to_string(),
        base_rate: 1.0,        // 1 RPS
        burst_probability: 0.1, // 10% chance of burst
        burst_multiplier: 2.0,  // 2x rate during burst
        priority: RequestPriority::Low,
    },
    TenantProfile {
        name: "premium-tier".to_string(),
        base_rate: 10.0,       // 10 RPS
        burst_probability: 0.3, // 30% chance of burst
        burst_multiplier: 5.0,  // 5x rate during burst
        priority: RequestPriority::High,
    },
];
```

## Real-World Applications

### E-commerce Platform

```rust
// Model different traffic patterns for e-commerce
let ecommerce_patterns = vec![
    // Regular browsing traffic with daily cycles
    create_seasonal_client("browsing", 50.0, 30.0, Duration::from_hours(24)),
    
    // Flash sales creating sudden spikes
    create_flash_crowd_client("flash-sale", 10.0, 200.0, Duration::from_minutes(15)),
    
    // Different customer tiers
    create_multi_tenant_client("customers", customer_tiers),
    
    // Shopping cart sessions
    create_session_client("shopping-cart", Duration::from_minutes(30), 15),
];
```

### Social Media Platform

```rust
// Model social media traffic patterns
let social_patterns = vec![
    // Posting activity with timezone variations
    create_seasonal_client("posts", 100.0, 80.0, Duration::from_hours(24)),
    
    // Viral content causing traffic spikes
    create_flash_crowd_client("viral-content", 50.0, 1000.0, Duration::from_hours(2)),
    
    // User sessions with correlated actions
    create_correlated_client("user-actions", social_dependencies),
];
```

### IoT Platform

```rust
// Model IoT device reporting patterns
let iot_patterns = vec![
    // Regular sensor data with seasonal variations
    create_seasonal_client("sensors", 1000.0, 200.0, Duration::from_hours(24)),
    
    // Alert bursts during incidents
    create_flash_crowd_client("alerts", 10.0, 500.0, Duration::from_minutes(5)),
    
    // Different device types with varying patterns
    create_multi_tenant_client("devices", device_profiles),
];
```

## Testing Strategies

### Pattern Validation

```rust
#[test]
fn test_seasonal_pattern_accuracy() {
    // Verify that seasonal patterns follow expected mathematical curves
    let client = create_seasonal_client(10.0, 5.0, Duration::from_hours(24));
    
    // Sample rates at different times
    let samples = vec![
        (Duration::from_hours(0), 10.0),   // Baseline at start
        (Duration::from_hours(6), 15.0),   // Peak at 6 hours
        (Duration::from_hours(12), 10.0),  // Back to baseline
        (Duration::from_hours(18), 5.0),   // Minimum at 18 hours
    ];
    
    for (time, expected_rate) in samples {
        let actual_rate = client.current_rate(SimTime::from_duration(time));
        assert!((actual_rate - expected_rate).abs() < 0.5, 
               "Rate mismatch at {:?}: expected {}, got {}", time, expected_rate, actual_rate);
    }
}
```

### Load Testing with Patterns

```rust
#[test]
fn test_server_under_pattern_load() {
    // Test server behavior under different workload patterns
    let patterns = vec![
        create_seasonal_client("seasonal", 20.0, 10.0, Duration::from_minutes(10)),
        create_flash_crowd_client("burst", 5.0, 100.0, Duration::from_seconds(30)),
        create_session_client("session", Duration::from_minutes(2), 10),
    ];
    
    for pattern in patterns {
        let server_metrics = run_pattern_load_test(pattern);
        
        // Verify server handles the pattern gracefully
        assert!(server_metrics.success_rate() > 0.8, "Server should maintain >80% success rate");
        assert!(server_metrics.average_latency() < Duration::from_millis(500), 
               "Average latency should be <500ms");
    }
}
```

## Next Steps

Continue to [Custom Distributions](./custom_distributions.md) to learn about implementing custom service time distributions and advanced statistical modeling techniques.