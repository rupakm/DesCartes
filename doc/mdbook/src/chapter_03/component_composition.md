# Component Composition

Component composition allows you to build complex distributed systems by combining servers, clients, queues, and retry policies. This section covers patterns for creating realistic system architectures using DES-Components.

## Basic Composition Patterns

### Single Client-Server

The simplest composition connects one client to one server:

```rust
use des_components::{Server, SimpleClient, ExponentialBackoffPolicy, FifoQueue};
use des_core::{Simulation, SimTime, task::PeriodicTask};
use std::time::Duration;

fn create_simple_system() -> Simulation {
    let mut sim = Simulation::default();
    
    // Create server
    let server = Server::with_exponential_service_time(
        "api-server".to_string(),
        4,
        Duration::from_millis(100),
    ).with_queue(Box::new(FifoQueue::bounded(20)));
    
    let server_id = sim.add_component(server);
    
    // Create client
    let client = SimpleClient::with_exponential_backoff(
        "web-client".to_string(),
        server_id,
        Duration::from_millis(200),
        3,
        Duration::from_millis(100),
    );
    
    let client_id = sim.add_component(client);
    
    // Start periodic requests
    let task = PeriodicTask::new(
        move |scheduler| {
            scheduler.schedule_now(client_id, des_components::ClientEvent::SendRequest);
        },
        SimTime::from_duration(Duration::from_millis(200)),
    );
    sim.schedule_task(SimTime::zero(), task);
    
    sim
}
```

### Multiple Clients, Single Server

Model multiple clients accessing a shared server:

```rust
use des_components::{Server, SimpleClient, ClientEvent, ExponentialBackoffPolicy, FifoQueue};
use des_core::{Simulation, SimTime, task::PeriodicTask};
use std::time::Duration;

fn create_multi_client_system() -> Simulation {
    let mut sim = Simulation::default();
    
    // Shared server
    let server = Server::with_constant_service_time(
        "shared-server".to_string(),
        8, // Higher capacity for multiple clients
        Duration::from_millis(80),
    ).with_queue(Box::new(FifoQueue::bounded(50)));
    
    let server_id = sim.add_component(server);
    
    // Create multiple clients with different characteristics
    let client_configs = [
        ("heavy-client", Duration::from_millis(50), 5),   // High rate, many retries
        ("normal-client", Duration::from_millis(200), 3), // Normal rate
        ("light-client", Duration::from_millis(500), 2),  // Low rate, few retries
    ];
    
    for (name, interval, max_retries) in client_configs {
        let client = SimpleClient::with_exponential_backoff(
            name.to_string(),
            server_id,
            interval,
            max_retries,
            Duration::from_millis(100),
        );
        
        let client_id = sim.add_component(client);
        
        // Start each client with slight offset
        let task = PeriodicTask::new(
            move |scheduler| {
                scheduler.schedule_now(client_id, ClientEvent::SendRequest);
            },
            SimTime::from_duration(interval),
        );
        
        let offset = match name {
            "heavy-client" => 0,
            "normal-client" => 50,
            "light-client" => 100,
            _ => 0,
        };
        
        sim.schedule_task(SimTime::from_duration(Duration::from_millis(offset)), task);
    }
    
    sim
}
```

## Load Balancing Patterns

### Round-Robin Load Balancing

Distribute requests across multiple servers:

```rust
use des_components::{Server, ServerEvent, FifoQueue};
use des_core::{Simulation, Component, Key, Scheduler, SimTime};
use std::time::Duration;

pub struct LoadBalancer {
    pub name: String,
    pub servers: Vec<Key<ServerEvent>>,
    pub current_server: usize,
    pub requests_distributed: u64,
}

impl LoadBalancer {
    pub fn new(name: String, servers: Vec<Key<ServerEvent>>) -> Self {
        Self {
            name,
            servers,
            current_server: 0,
            requests_distributed: 0,
        }
    }
    
    fn next_server(&mut self) -> Key<ServerEvent> {
        let server = self.servers[self.current_server];
        self.current_server = (self.current_server + 1) % self.servers.len();
        self.requests_distributed += 1;
        server
    }
}

#[derive(Debug, Clone)]
pub enum LoadBalancerEvent {
    RouteRequest {
        attempt: des_core::RequestAttempt,
        client_id: Key<des_components::ClientEvent>,
    },
}

impl Component for LoadBalancer {
    type Event = LoadBalancerEvent;
    
    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            LoadBalancerEvent::RouteRequest { attempt, client_id } => {
                let target_server = self.next_server();
                
                println!("[{}] Load balancer routing request {} to server {}",
                    des_components::server::format_sim_time(scheduler.time()),
                    attempt.id.0,
                    target_server.id());
                
                scheduler.schedule_now(
                    target_server,
                    ServerEvent::ProcessRequest {
                        attempt: attempt.clone(),
                        client_id: *client_id,
                    },
                );
            }
        }
    }
}

fn create_load_balanced_system() -> Simulation {
    let mut sim = Simulation::default();
    
    // Create multiple servers
    let mut server_keys = Vec::new();
    for i in 0..3 {
        let server = Server::with_exponential_service_time(
            format!("server-{}", i),
            4,
            Duration::from_millis(100),
        ).with_queue(Box::new(FifoQueue::bounded(15)));
        
        server_keys.push(sim.add_component(server));
    }
    
    // Create load balancer
    let load_balancer = LoadBalancer::new("lb".to_string(), server_keys);
    let lb_id = sim.add_component(load_balancer);
    
    // Create client that sends to load balancer
    // Note: This would require modifying SimpleClient to support custom targets
    // For now, this demonstrates the pattern
    
    sim
}
```

### Weighted Load Balancing

Distribute load based on server capacity:

```rust
pub struct WeightedLoadBalancer {
    pub name: String,
    pub servers: Vec<(Key<ServerEvent>, u32)>, // (server, weight)
    pub current_weights: Vec<u32>,
    pub requests_distributed: u64,
}

impl WeightedLoadBalancer {
    pub fn new(name: String, servers: Vec<(Key<ServerEvent>, u32)>) -> Self {
        let current_weights = servers.iter().map(|(_, weight)| *weight).collect();
        
        Self {
            name,
            servers,
            current_weights,
            requests_distributed: 0,
        }
    }
    
    fn next_server(&mut self) -> Key<ServerEvent> {
        // Find server with highest current weight
        let max_weight_idx = self.current_weights
            .iter()
            .enumerate()
            .max_by_key(|(_, &weight)| weight)
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        
        let server = self.servers[max_weight_idx].0;
        
        // Decrease current weight and reset if needed
        self.current_weights[max_weight_idx] -= 1;
        if self.current_weights[max_weight_idx] == 0 {
            self.current_weights[max_weight_idx] = self.servers[max_weight_idx].1;
        }
        
        self.requests_distributed += 1;
        server
    }
}
```

## Multi-Tier Architectures

### Three-Tier Architecture

Model web server → application server → database:

```rust
use des_components::{Server, SimpleClient, ClientEvent, ServerEvent, FifoQueue};
use des_core::{Simulation, SimTime, task::PeriodicTask};
use std::time::Duration;

fn create_three_tier_system() -> Simulation {
    let mut sim = Simulation::default();
    
    // Tier 3: Database server (slowest, highest capacity)
    let database = Server::with_constant_service_time(
        "database".to_string(),
        2, // Limited DB connections
        Duration::from_millis(500), // Slow DB queries
    ).with_queue(Box::new(FifoQueue::bounded(100))); // Large queue for DB
    
    let db_id = sim.add_component(database);
    
    // Tier 2: Application servers (medium speed, medium capacity)
    let mut app_server_ids = Vec::new();
    for i in 0..2 {
        let app_server = Server::with_constant_service_time(
            format!("app-server-{}", i),
            4,
            Duration::from_millis(200),
        ).with_queue(Box::new(FifoQueue::bounded(30)));
        
        app_server_ids.push(sim.add_component(app_server));
        
        // Each app server has a client that talks to the database
        let db_client = SimpleClient::with_exponential_backoff(
            format!("app-to-db-{}", i),
            db_id,
            Duration::from_millis(100), // App servers query DB frequently
            3,
            Duration::from_millis(50),
        );
        
        let db_client_id = sim.add_component(db_client);
        
        // App servers make DB requests when processing
        let task = PeriodicTask::new(
            move |scheduler| {
                scheduler.schedule_now(db_client_id, ClientEvent::SendRequest);
            },
            SimTime::from_duration(Duration::from_millis(300)),
        );
        sim.schedule_task(SimTime::from_duration(Duration::from_millis(i * 50)), task);
    }
    
    // Tier 1: Web servers (fastest, highest capacity)
    let mut web_server_ids = Vec::new();
    for i in 0..3 {
        let web_server = Server::with_constant_service_time(
            format!("web-server-{}", i),
            8,
            Duration::from_millis(50),
        ).with_queue(Box::new(FifoQueue::bounded(20)));
        
        web_server_ids.push(sim.add_component(web_server));
        
        // Each web server talks to app servers
        for (j, &app_server_id) in app_server_ids.iter().enumerate() {
            let app_client = SimpleClient::with_exponential_backoff(
                format!("web-to-app-{}-{}", i, j),
                app_server_id,
                Duration::from_millis(150),
                2,
                Duration::from_millis(25),
            );
            
            let app_client_id = sim.add_component(app_client);
            
            let task = PeriodicTask::new(
                move |scheduler| {
                    scheduler.schedule_now(app_client_id, ClientEvent::SendRequest);
                },
                SimTime::from_duration(Duration::from_millis(400)),
            );
            sim.schedule_task(SimTime::from_duration(Duration::from_millis(i * 30 + j * 10)), task);
        }
    }
    
    // External clients hitting web servers
    for (i, &web_server_id) in web_server_ids.iter().enumerate() {
        let external_client = SimpleClient::with_exponential_backoff(
            format!("external-client-{}", i),
            web_server_id,
            Duration::from_millis(100),
            3,
            Duration::from_millis(100),
        );
        
        let client_id = sim.add_component(external_client);
        
        let task = PeriodicTask::new(
            move |scheduler| {
                scheduler.schedule_now(client_id, ClientEvent::SendRequest);
            },
            SimTime::from_duration(Duration::from_millis(100)),
        );
        sim.schedule_task(SimTime::from_duration(Duration::from_millis(i * 20)), task);
    }
    
    sim
}
```

## Microservices Architecture

### Service Mesh Pattern

Model a microservices architecture with service-to-service communication:

```rust
use des_components::{Server, SimpleClient, FifoQueue};
use des_core::{Simulation, SimTime};
use std::time::Duration;
use std::collections::HashMap;

struct MicroserviceSystem {
    services: HashMap<String, des_core::Key<ServerEvent>>,
    clients: HashMap<String, des_core::Key<des_components::ClientEvent>>,
}

impl MicroserviceSystem {
    fn new() -> Self {
        Self {
            services: HashMap::new(),
            clients: HashMap::new(),
        }
    }
    
    fn add_service(
        &mut self,
        sim: &mut Simulation,
        name: &str,
        capacity: usize,
        service_time: Duration,
        queue_size: usize,
    ) {
        let server = Server::with_exponential_service_time(
            name.to_string(),
            capacity,
            service_time,
        ).with_queue(Box::new(FifoQueue::bounded(queue_size)));
        
        let server_id = sim.add_component(server);
        self.services.insert(name.to_string(), server_id);
    }
    
    fn add_service_client(
        &mut self,
        sim: &mut Simulation,
        client_name: &str,
        target_service: &str,
        request_rate: Duration,
    ) -> Result<(), String> {
        let target_id = self.services.get(target_service)
            .ok_or_else(|| format!("Service {} not found", target_service))?;
        
        let client = SimpleClient::with_exponential_backoff(
            client_name.to_string(),
            *target_id,
            request_rate,
            3,
            Duration::from_millis(100),
        );
        
        let client_id = sim.add_component(client);
        self.clients.insert(client_name.to_string(), client_id);
        
        Ok(())
    }
}

fn create_microservices_system() -> Result<Simulation, String> {
    let mut sim = Simulation::default();
    let mut system = MicroserviceSystem::new();
    
    // Add services
    system.add_service(&mut sim, "user-service", 4, Duration::from_millis(50), 20);
    system.add_service(&mut sim, "order-service", 6, Duration::from_millis(100), 30);
    system.add_service(&mut sim, "payment-service", 2, Duration::from_millis(200), 15);
    system.add_service(&mut sim, "inventory-service", 3, Duration::from_millis(80), 25);
    system.add_service(&mut sim, "notification-service", 8, Duration::from_millis(30), 40);
    
    // Add service-to-service communication
    // Order service calls user, payment, and inventory services
    system.add_service_client(&mut sim, "order-to-user", "user-service", Duration::from_millis(200))?;
    system.add_service_client(&mut sim, "order-to-payment", "payment-service", Duration::from_millis(300))?;
    system.add_service_client(&mut sim, "order-to-inventory", "inventory-service", Duration::from_millis(250))?;
    
    // Payment service calls notification service
    system.add_service_client(&mut sim, "payment-to-notification", "notification-service", Duration::from_millis(400))?;
    
    // Order service calls notification service
    system.add_service_client(&mut sim, "order-to-notification", "notification-service", Duration::from_millis(500))?;
    
    // Start service clients
    for (client_name, &client_id) in &system.clients {
        let task = des_core::task::PeriodicTask::new(
            move |scheduler| {
                scheduler.schedule_now(client_id, ClientEvent::SendRequest);
            },
            SimTime::from_duration(Duration::from_millis(300)),
        );
        
        // Stagger start times
        let offset = client_name.len() as u64 * 10;
        sim.schedule_task(SimTime::from_duration(Duration::from_millis(offset)), task);
    }
    
    Ok(sim)
}
```

## Circuit Breaker Pattern

Implement circuit breaker for fault tolerance:

```rust
use des_components::{Server, SimpleClient, RetryPolicy};
use des_core::{Simulation, Component, Key, Scheduler, SimTime};
use std::time::Duration;

#[derive(Clone)]
pub enum CircuitState {
    Closed,   // Normal operation
    Open,     // Failing, reject requests
    HalfOpen, // Testing if service recovered
}

#[derive(Clone)]
pub struct CircuitBreakerClient<P: RetryPolicy> {
    inner_client: SimpleClient<P>,
    circuit_state: CircuitState,
    failure_count: u32,
    failure_threshold: u32,
    recovery_timeout: Duration,
    last_failure_time: Option<SimTime>,
    success_count_in_half_open: u32,
    required_successes: u32,
}

impl<P: RetryPolicy> CircuitBreakerClient<P> {
    pub fn new(
        inner_client: SimpleClient<P>,
        failure_threshold: u32,
        recovery_timeout: Duration,
        required_successes: u32,
    ) -> Self {
        Self {
            inner_client,
            circuit_state: CircuitState::Closed,
            failure_count: 0,
            failure_threshold,
            recovery_timeout,
            last_failure_time: None,
            success_count_in_half_open: 0,
            required_successes,
        }
    }
    
    fn should_allow_request(&mut self, current_time: SimTime) -> bool {
        match self.circuit_state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if recovery timeout has passed
                if let Some(last_failure) = self.last_failure_time {
                    if current_time.duration_since(last_failure) >= self.recovery_timeout {
                        self.circuit_state = CircuitState::HalfOpen;
                        self.success_count_in_half_open = 0;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }
    
    fn record_success(&mut self) {
        match self.circuit_state {
            CircuitState::Closed => {
                self.failure_count = 0;
            }
            CircuitState::HalfOpen => {
                self.success_count_in_half_open += 1;
                if self.success_count_in_half_open >= self.required_successes {
                    self.circuit_state = CircuitState::Closed;
                    self.failure_count = 0;
                }
            }
            CircuitState::Open => {
                // Shouldn't happen, but reset if it does
                self.circuit_state = CircuitState::Closed;
                self.failure_count = 0;
            }
        }
    }
    
    fn record_failure(&mut self, current_time: SimTime) {
        self.failure_count += 1;
        self.last_failure_time = Some(current_time);
        
        match self.circuit_state {
            CircuitState::Closed => {
                if self.failure_count >= self.failure_threshold {
                    self.circuit_state = CircuitState::Open;
                }
            }
            CircuitState::HalfOpen => {
                self.circuit_state = CircuitState::Open;
            }
            CircuitState::Open => {
                // Already open, just update timestamp
            }
        }
    }
}

// Implementation would require custom event handling to integrate circuit breaker logic
```

## Best Practices for Component Composition

### Design Principles

1. **Separation of Concerns**: Each component should have a single responsibility
2. **Loose Coupling**: Components should interact through well-defined interfaces
3. **Scalability**: Design for horizontal scaling by adding more instances
4. **Fault Tolerance**: Include retry policies and circuit breakers
5. **Observability**: Ensure all components emit metrics

### Performance Considerations

```rust
// Good: Balanced system design
let web_tier_capacity = 8;
let app_tier_capacity = 4;
let db_tier_capacity = 2;

// Rule of thumb: Each tier should have 2-4x capacity of the next tier
assert!(web_tier_capacity >= app_tier_capacity * 2);
assert!(app_tier_capacity >= db_tier_capacity * 2);
```

### Resource Management

```rust
// Configure queue sizes based on expected load
let queue_size = server_capacity * expected_load_multiplier;

// Example: Server with capacity 4, expecting 3x load spikes
let server_queue = FifoQueue::bounded(4 * 3); // 12 request buffer
```

### Monitoring Composition

```rust
fn analyze_system_health(sim: &mut Simulation) {
    // Collect metrics from all components
    let mut total_requests = 0;
    let mut total_successes = 0;
    let mut total_failures = 0;
    
    // Iterate through components and aggregate metrics
    // This would require access to component metrics
    
    let success_rate = total_successes as f64 / total_requests as f64;
    let failure_rate = total_failures as f64 / total_requests as f64;
    
    println!("System Health:");
    println!("  Success Rate: {:.2}%", success_rate * 100.0);
    println!("  Failure Rate: {:.2}%", failure_rate * 100.0);
    
    if success_rate < 0.95 {
        println!("  ⚠️  Low success rate - check server capacity");
    }
    
    if failure_rate > 0.05 {
        println!("  ⚠️  High failure rate - check retry policies");
    }
}
```

Component composition enables building realistic distributed system simulations that capture the complexity and interactions of real-world architectures. Start with simple patterns and gradually add complexity as needed for your simulation goals.