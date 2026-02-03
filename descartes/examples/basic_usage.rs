//! Basic usage example showing how to use the descartes meta-crate

use des_cartes::prelude::*;

fn main() {
    // Create a simulation with default configuration
    let mut sim = Simulation::default();
    
    println!("Simulation created at time: {:?}", sim.time());
    
    // Run the simulation (will complete immediately as no events are scheduled)
    sim.execute(Executor::unbound());
    
    println!("Simulation completed at time: {:?}", sim.time());
}
