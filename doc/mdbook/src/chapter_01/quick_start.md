# Quick Start Guide

Get up and running with DESCARTES in under 5 minutes! This guide will walk you through installation, setup, and running your first discrete event simulation.

## Installation

### Prerequisites

- **Rust 1.70 or later** - Install from [rustup.rs](https://rustup.rs/)
- **Git** - For cloning the repository

### Platform-Specific Instructions

#### Linux (Ubuntu/Debian)
```bash
# Install Rust if not already installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Install build dependencies
sudo apt update
sudo apt install build-essential pkg-config

# Verify installation
rustc --version
cargo --version
```

#### macOS
```bash
# Install Rust if not already installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Install Xcode command line tools if needed
xcode-select --install

# Verify installation
rustc --version
cargo --version
```

#### Windows
1. Download and install Rust from [rustup.rs](https://rustup.rs/)
2. Install Visual Studio Build Tools or Visual Studio Community
3. Open a new Command Prompt or PowerShell window
4. Verify installation:
```cmd
rustc --version
cargo --version
```

### Create Your First DESCARTES Project

1. **Create a new Rust project:**
```bash
cargo new my-simulation
cd my-simulation
```

2. **Add DESCARTES dependencies to `Cargo.toml`:**
```toml
[package]
name = "my-simulation"
version = "0.1.0"
edition = "2021"

[dependencies]
des-core = "0.1.0"
des-components = "0.1.0"
des-metrics = "0.1.0"
tokio = { version = "1.35", features = ["rt", "macros", "time"] }
tracing = "0.1"
rand = "0.8"
```

3. **Replace `src/main.rs` with this working example:**
```rust
//! Your First DESCARTES Simulation
//!
//! This example demonstrates a simple client-server system where:
//! - A client sends requests every 200ms
//! - A server processes each request in 100ms
//! - The simulation runs for 2 seconds

use des_core::{
    Component, Executor, Key, Simulation, SimTime,
    init_simulation_logging,
};
use std::time::Duration;
use tracing::info;

/// A simple server that processes requests
#[derive(Debug)]
struct Server {
    name: String,
    requests_processed: u32,
    processing_time: Duration,
}

#[derive(Debug)]
enum ServerEvent {
    ProcessRequest { request_id: u32 },
    RequestCompleted { request_id: u32 },
}

impl Server {
    fn new(name: String, processing_time: Duration) -> Self {
        Self {
            name,
            requests_processed: 0,
            processing_time,
        }
    }
}

impl Component for Server {
    type Event = ServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut des_core::Scheduler,
    ) {
        match event {
            ServerEvent::ProcessRequest { request_id } => {
                info!("Server {} starting request {}", self.name, request_id);
                
                // Schedule completion after processing time
                scheduler.schedule(
                    SimTime::from_duration(self.processing_time),
                    self_id,
                    ServerEvent::RequestCompleted { request_id: *request_id },
                );
            }
            ServerEvent::RequestCompleted { request_id } => {
                self.requests_processed += 1;
                info!("Server {} completed request {} (total: {})", 
                      self.name, request_id, self.requests_processed);
            }
        }
    }
}

/// A client that sends requests at regular intervals
#[derive(Debug)]
struct Client {
    name: String,
    server_key: Key<ServerEvent>,
    requests_to_send: u32,
    requests_sent: u32,
    request_interval: Duration,
}

#[derive(Debug)]
enum ClientEvent {
    SendRequest,
}

impl Client {
    fn new(
        name: String,
        server_key: Key<ServerEvent>,
        requests_to_send: u32,
        request_interval: Duration,
    ) -> Self {
        Self {
            name,
            server_key,
            requests_to_send,
            requests_sent: 0,
            request_interval,
        }
    }
}

impl Component for Client {
    type Event = ClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut des_core::Scheduler,
    ) {
        match event {
            ClientEvent::SendRequest => {
                if self.requests_sent < self.requests_to_send {
                    self.requests_sent += 1;
                    
                    info!("Client {} sending request {}", self.name, self.requests_sent);
                    
                    // Send request to server
                    scheduler.schedule_now(
                        self.server_key,
                        ServerEvent::ProcessRequest {
                            request_id: self.requests_sent,
                        },
                    );
                    
                    // Schedule next request if more to send
                    if self.requests_sent < self.requests_to_send {
                        scheduler.schedule(
                            SimTime::from_duration(self.request_interval),
                            self_id,
                            ClientEvent::SendRequest,
                        );
                    }
                }
            }
        }
    }
}

fn main() {
    // Initialize logging to see what's happening
    init_simulation_logging();
    
    println!("🚀 Starting your first DESCARTES simulation!");
    
    // Create simulation
    let mut sim = Simulation::default();
    
    // Create server that takes 100ms to process each request
    let server = Server::new(
        "WebServer".to_string(),
        Duration::from_millis(100),
    );
    let server_key = sim.add_component(server);
    
    // Create client that sends 10 requests every 200ms
    let client = Client::new(
        "WebClient".to_string(),
        server_key,
        10, // Send 10 requests total
        Duration::from_millis(200), // Every 200ms
    );
    let client_key = sim.add_component(client);
    
    // Start the client immediately
    sim.schedule(SimTime::zero(), client_key, ClientEvent::SendRequest);
    
    // Run simulation for 2 seconds
    println!("⏱️  Running simulation for 2 seconds...");
    sim.execute(Executor::timed(SimTime::from_secs(2)));
    
    // Get final results
    let final_server = sim.remove_component::<ServerEvent, Server>(server_key).unwrap();
    let final_client = sim.remove_component::<ClientEvent, Client>(client_key).unwrap();
    
    // Print results
    println!("\n✅ Simulation completed!");
    println!("📊 Results:");
    println!("   • Client '{}' sent {} requests", final_client.name, final_client.requests_sent);
    println!("   • Server '{}' processed {} requests", final_server.name, final_server.requests_processed);
    println!("   • Final simulation time: {:?}", sim.time());
    println!("   • Throughput: {:.1} requests/second", 
             final_server.requests_processed as f64 / 2.0);
}
```

4. **Run your simulation:**
```bash
cargo run
```

### Expected Output

You should see output similar to this:

```
🚀 Starting your first DESCARTES simulation!
⏱️  Running simulation for 2 seconds...
2024-01-09T10:30:00.123456Z  INFO my_simulation: Client WebClient sending request 1
2024-01-09T10:30:00.123456Z  INFO my_simulation: Server WebServer starting request 1
2024-01-09T10:30:00.223456Z  INFO my_simulation: Server WebServer completed request 1 (total: 1)
2024-01-09T10:30:00.323456Z  INFO my_simulation: Client WebClient sending request 2
2024-01-09T10:30:00.323456Z  INFO my_simulation: Server WebServer starting request 2
...

✅ Simulation completed!
📊 Results:
   • Client 'WebClient' sent 10 requests
   • Server 'WebServer' processed 10 requests
   • Final simulation time: 2s
   • Throughput: 5.0 requests/second
```

## Understanding What Happened

This simulation demonstrates key DESCARTES concepts:

1. **Components**: The `Client` and `Server` are simulation components that process events
2. **Events**: `ClientEvent::SendRequest` and `ServerEvent::ProcessRequest` drive the simulation
3. **Scheduling**: Events are scheduled to happen at specific simulation times
4. **Simulation Time**: Time advances based on scheduled events, not wall-clock time
5. **Execution**: The `Executor::timed()` runs the simulation for a specified duration

## Verification Steps

To confirm everything is working correctly:

1. **Check the output**: You should see log messages showing requests being sent and processed
2. **Verify timing**: The client sends requests every 200ms, server processes in 100ms
3. **Count requests**: With a 2-second simulation and 200ms intervals, expect ~10 requests
4. **Check throughput**: Should be around 5 requests/second

## Troubleshooting

### Common Issues

**"cargo: command not found"**
- Rust is not installed or not in PATH
- Solution: Install Rust from [rustup.rs](https://rustup.rs/) and restart your terminal

**"linker 'cc' not found" (Linux)**
- Missing build tools
- Solution: `sudo apt install build-essential`

**"Microsoft C++ Build Tools" error (Windows)**
- Missing Visual Studio Build Tools
- Solution: Install Visual Studio Build Tools or Visual Studio Community

**No log output visible**
- Logging level might be too high
- Solution: Set environment variable: `RUST_LOG=info cargo run`

**Compilation errors about missing dependencies**
- Cargo.toml might have incorrect versions
- Solution: Run `cargo update` to get latest compatible versions

### Getting Help

If you encounter issues:

1. Check that your Rust version is 1.70 or later: `rustc --version`
2. Ensure all dependencies are correctly specified in `Cargo.toml`
3. Try running with verbose output: `cargo run --verbose`
4. Check the [DESCARTES repository](https://github.com/example/des-framework) for known issues

## Next Steps

Now that you have DESCARTES running, you can:

1. **Learn the concepts**: Read [Basics of Discrete Event Simulation](des_basics.md)
2. **Explore components**: Check out [DES-Components documentation](../chapter_03/README.md)
3. **Add metrics**: Learn about [metrics collection](metrics_overview.md)
4. **Try examples**: Run the M/M/k queueing examples in [Chapter 5](../chapter_05/README.md)

Congratulations! You've successfully run your first discrete event simulation with DESCARTES. 🎉