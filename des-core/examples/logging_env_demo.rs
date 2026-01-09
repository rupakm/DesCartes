//! Environment-controlled logging demonstration
//!
//! This example shows how to control logging levels using environment variables.
//!
//! Usage examples:
//! - Default (info level): cargo run --example logging_env_demo
//! - Debug level: RUST_LOG=debug cargo run --example logging_env_demo
//! - Trace level: RUST_LOG=trace cargo run --example logging_env_demo
//! - Module-specific: RUST_LOG=des_core::scheduler=debug cargo run --example logging_env_demo
//! - Multiple modules: RUST_LOG=des_core=debug,logging_env_demo=trace cargo run --example logging_env_demo

use des_core::{init_simulation_logging_with_level, Component, Executor, Key, SimTime, Simulation};
use std::time::Duration;
use tracing::{debug, info};

#[derive(Debug)]
struct SimpleComponent {
    counter: u32,
}

#[derive(Debug)]
enum SimpleEvent {
    Tick,
    Tock,
}

impl Component for SimpleComponent {
    type Event = SimpleEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut des_core::Scheduler,
    ) {
        match event {
            SimpleEvent::Tick => {
                self.counter += 1;

                //trace!(counter = self.counter, "Processing tick event");
                debug!(counter = self.counter, "Tick processed");
                info!(
                    counter = self.counter,
                    "{} Component tick #{}",
                    scheduler.time(),
                    self.counter
                );

                // schedule a tock
                scheduler.schedule(
                    SimTime::from_duration(Duration::from_millis(50)),
                    self_id,
                    SimpleEvent::Tock,
                );
                // Schedule next tick if we haven't reached 3 yet
                if self.counter < 3 {
                    scheduler.schedule(
                        SimTime::from_duration(Duration::from_millis(100)),
                        self_id,
                        SimpleEvent::Tick,
                    );
                    debug!("Scheduled next tick");
                } else {
                    info!("Component finished - no more ticks scheduled");
                }
            }
            SimpleEvent::Tock => {
                // trace!(counter = self.counter, "Processing tock event");
                debug!(counter = self.counter, "Tock processed");
                info!(
                    counter = self.counter,
                    "{} Component tock #{}",
                    scheduler.time(),
                    self.counter
                );
            }
        }
    }
}

fn main() {
    // This will respect the RUST_LOG environment variable
    // If RUST_LOG is not set, it defaults to "info" level
    init_simulation_logging_with_level("info");

    println!("=== Environment-Controlled Logging Demo ===");
    println!(
        "Current RUST_LOG: {:?}",
        std::env::var("RUST_LOG").unwrap_or_else(|_| "not set".to_string())
    );
    println!("Try running with: RUST_LOG=debug cargo run --example logging_env_demo");
    println!("Or: RUST_LOG=trace cargo run --example logging_env_demo");
    println!();

    info!("Starting environment logging demonstration");

    let mut sim = Simulation::default();

    let component = SimpleComponent { counter: 0 };
    let component_key = sim.add_component(component);

    // Start the first tick
    sim.schedule(SimTime::zero(), component_key, SimpleEvent::Tick);

    info!("Running simulation");
    sim.execute(Executor::timed(SimTime::from_secs(1)));

    let final_component = sim
        .remove_component::<SimpleEvent, SimpleComponent>(component_key)
        .unwrap();

    info!(
        final_counter = final_component.counter,
        "Simulation completed"
    );

    println!("\n=== Summary ===");
    println!("Final counter: {}", final_component.counter);
    println!("Try different RUST_LOG values to see different amounts of detail!");
}
