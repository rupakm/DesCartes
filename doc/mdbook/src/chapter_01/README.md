# Chapter 1: Basics of DES Simulation

Welcome to DESCARTES! This chapter provides everything you need to get started with discrete event simulation using the DESCARTES framework.

## What You'll Learn

By the end of this chapter, you'll understand:

- How to install and set up DESCARTES
- The fundamental concepts of discrete event simulation
- How DESCARTES collects and analyzes simulation metrics
- How to run your first simulation and interpret the results

## Chapter Contents

### [Quick Start Guide](quick_start.md)
Get DESCARTES running in under 5 minutes with a complete working example. This section includes:
- Platform-specific installation instructions
- A simple client-server simulation
- Verification steps to ensure everything works
- Troubleshooting common issues

### [Basics of Discrete Event Simulation](des_basics.md)
Learn the theoretical foundation behind discrete event simulation:
- What DES is and when to use it
- Core concepts: events, components, simulation time
- How DES differs from other simulation approaches
- Common patterns and best practices

### [The Metrics Collection System](metrics_overview.md)
Understand how DESCARTES helps you analyze simulation results:
- Architecture of the metrics system
- Types of metrics: counters, gauges, histograms
- Export formats: JSON, CSV, Prometheus
- Visualization capabilities and integration

## Prerequisites

- **Rust 1.70 or later** - The only hard requirement
- **Basic programming knowledge** - Familiarity with any programming language
- **No simulation experience required** - We'll teach you everything you need

## Learning Path

If you're new to simulation:
1. Start with the [Quick Start Guide](quick_start.md) to get hands-on experience
2. Read [DES Basics](des_basics.md) to understand the theory
3. Explore [Metrics Overview](metrics_overview.md) to learn about analysis

If you're experienced with simulation but new to DESCARTES:
1. Skim the [Quick Start Guide](quick_start.md) for installation
2. Focus on [Metrics Overview](metrics_overview.md) for DESCARTES-specific features
3. Jump to [Chapter 2](../chapter_02/README.md) for core abstractions

## What Makes DESCARTES Different

DESCARTES is designed with several key principles:

- **Rust Performance**: Native performance with memory safety
- **Type Safety**: Catch errors at compile time, not runtime
- **Composability**: Build complex systems from simple components
- **Observability**: Built-in metrics and visualization
- **Ecosystem Integration**: Works with standard Rust tools and libraries

## Example: Your First Simulation

Here's a taste of what DESCARTES code looks like:

```rust
use des_core::{Component, Simulation, SimTime, Executor};
use std::time::Duration;

// Define a simple server component
struct Server {
    requests_processed: u32,
}

impl Component for Server {
    type Event = ServerEvent;
    
    fn process_event(&mut self, event: &ServerEvent, scheduler: &mut Scheduler) {
        match event {
            ServerEvent::ProcessRequest => {
                self.requests_processed += 1;
                println!("Processed request #{}", self.requests_processed);
            }
        }
    }
}

fn main() {
    let mut sim = Simulation::default();
    let server = Server { requests_processed: 0 };
    let server_key = sim.add_component(server);
    
    // Schedule some requests
    sim.schedule(SimTime::zero(), server_key, ServerEvent::ProcessRequest);
    sim.schedule(SimTime::from_secs(1), server_key, ServerEvent::ProcessRequest);
    
    // Run for 2 seconds
    sim.execute(Executor::timed(SimTime::from_secs(2)));
}
```

This simple example demonstrates:
- **Components**: The `Server` struct that maintains state
- **Events**: `ServerEvent::ProcessRequest` that drives behavior
- **Scheduling**: Events scheduled at specific simulation times
- **Execution**: Running the simulation for a specified duration

## Next Steps

Ready to dive in? Start with the [Quick Start Guide](quick_start.md) to get your development environment set up and run your first simulation.

After completing this chapter, you'll be ready to explore:
- [Chapter 2: DES-Core](../chapter_02/README.md) - Deep dive into core abstractions
- [Chapter 3: DES-Components](../chapter_03/README.md) - Pre-built simulation components
- [Chapter 5: M/M/k Queue Examples](../chapter_05/README.md) - Practical queueing system examples

Let's get started! 🚀