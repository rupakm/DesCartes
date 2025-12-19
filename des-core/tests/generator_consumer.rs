//! Integration test demonstrating a generator-consumer pattern
//!
//! This test creates:
//! - A generator component that schedules events every 1 second
//! - A consumer component that responds to events and logs "hello" every 0.5 seconds
//! - Runs the simulation for 5 seconds
//!
//! Note: This demonstrates the event-driven nature of the DES framework.
//! Components wake up in response to events and time advancement.

use des_core::{Component, Execute, Executor, Simulation, SimTime};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Shared state for tracking simulation events
#[derive(Debug)]
struct SimulationLog {
    generator_events: Vec<SimTime>,
    consumer_hellos: Vec<SimTime>,
}

impl SimulationLog {
    fn new() -> Self {
        Self {
            generator_events: Vec::new(),
            consumer_hellos: Vec::new(),
        }
    }
}

/// Generator component that produces events every 1 second
struct Generator {
    log: Arc<Mutex<SimulationLog>>,
    events_produced: usize,
    max_events: usize,
}

#[derive(Debug)]
struct GeneratorEvent;

impl Component for Generator {
    type Event = GeneratorEvent;

    fn process_event(
        &mut self,
        self_id: des_core::Key<Self::Event>,
        _event: &Self::Event,
        scheduler: &mut des_core::Scheduler,
    ) {
        let current_time = scheduler.time();
        
        // Log the generator event
        self.log.lock().unwrap().generator_events.push(current_time);
        self.events_produced += 1;
        
        // Schedule next event if we haven't reached the limit
        if self.events_produced < self.max_events {
            scheduler.schedule(
                SimTime::from_duration(Duration::from_secs(1)),
                self_id,
                GeneratorEvent,
            );
        }
    }
}

/// Consumer component that responds with "hello" every 0.5 seconds
struct Consumer {
    log: Arc<Mutex<SimulationLog>>,
    hellos_produced: usize,
    max_hellos: usize,
}

#[derive(Debug)]
struct ConsumerEvent;

impl Component for Consumer {
    type Event = ConsumerEvent;

    fn process_event(
        &mut self,
        self_id: des_core::Key<Self::Event>,
        _event: &Self::Event,
        scheduler: &mut des_core::Scheduler,
    ) {
        let current_time = scheduler.time();
        
        // Log the consumer hello
        self.log.lock().unwrap().consumer_hellos.push(current_time);
        self.hellos_produced += 1;
        
        // Schedule next hello if we haven't reached the limit
        if self.hellos_produced < self.max_hellos {
            scheduler.schedule(
                SimTime::from_duration(Duration::from_millis(500)),
                self_id,
                ConsumerEvent,
            );
        }
    }
}

#[test]
fn test_generator_consumer_pattern() {
    println!("\n=== Generator-Consumer Integration Test ===\n");

    let mut sim = Simulation::default();
    let log = Arc::new(Mutex::new(SimulationLog::new()));
    let end_time = SimTime::from_duration(Duration::from_secs(5));

    println!("Setting up simulation for {} seconds\n", end_time.as_duration().as_secs());

    // Create generator component
    let generator = Generator {
        log: log.clone(),
        events_produced: 0,
        max_events: 5,
    };
    let generator_id = sim.add_component(generator);

    // Create consumer component  
    let consumer = Consumer {
        log: log.clone(),
        hellos_produced: 0,
        max_hellos: 10,
    };
    let consumer_id = sim.add_component(consumer);

    // Schedule initial events
    println!("[Generator] Starting generator (events every 1 second)");
    sim.schedule(SimTime::from_duration(Duration::from_secs(1)), generator_id, GeneratorEvent);
    
    println!("[Consumer] Starting consumer (hellos every 0.5 seconds)");
    sim.schedule(SimTime::from_duration(Duration::from_millis(500)), consumer_id, ConsumerEvent);

    println!("\n[Simulation] Running for {} seconds...\n", end_time.as_duration().as_secs());

    // Run the simulation
    Executor::timed(end_time).execute(&mut sim);

    println!("=== Simulation Complete ===\n");

    // Verify results
    let final_log = log.lock().unwrap();
    
    println!("Final simulation time: {:?}", sim.scheduler.time());
    println!("Generator events: {} (expected 5)", final_log.generator_events.len());
    println!("Consumer hellos: {} (expected 10)", final_log.consumer_hellos.len());

    // Verify generator events
    assert_eq!(
        final_log.generator_events.len(),
        5,
        "Expected 5 generator events"
    );

    // Verify consumer hellos
    assert_eq!(
        final_log.consumer_hellos.len(),
        10,
        "Expected 10 consumer hellos"
    );

    // Verify timing of generator events (should be at 1s intervals)
    println!("\nGenerator event times:");
    for (i, time) in final_log.generator_events.iter().enumerate() {
        println!("  Event {}: {:?}", i + 1, time);
        let expected_time = SimTime::from_duration(Duration::from_secs((i + 1) as u64));
        assert_eq!(
            *time, expected_time,
            "Generator event {} should be at {:?}, got {:?}",
            i + 1, expected_time, time
        );
    }

    // Verify timing of consumer hellos (should be at 0.5s intervals)
    println!("\nConsumer hello times:");
    for (i, time) in final_log.consumer_hellos.iter().enumerate() {
        println!("  Hello {}: {:?}", i + 1, time);
        let expected_time = SimTime::from_duration(Duration::from_millis((i + 1) as u64 * 500));
        assert_eq!(
            *time, expected_time,
            "Consumer hello {} should be at {:?}, got {:?}",
            i + 1, expected_time, time
        );
    }

    println!("\n=== Test Passed ===\n");
}

#[test]
fn test_generator_consumer_interleaved() {
    println!("\n=== Generator-Consumer Interleaved Events Test ===\n");

    let mut sim = Simulation::default();
    let log = Arc::new(Mutex::new(SimulationLog::new()));
    let end_time = SimTime::from_duration(Duration::from_secs(3));

    println!("Simulating interleaved generator-consumer pattern\n");

    // Create components with fewer events for this test
    let generator = Generator {
        log: log.clone(),
        events_produced: 0,
        max_events: 3, // Only 3 events at 1s, 2s, 3s
    };
    let generator_id = sim.add_component(generator);

    let consumer = Consumer {
        log: log.clone(),
        hellos_produced: 0,
        max_hellos: 6, // 6 hellos at 0.5s, 1s, 1.5s, 2s, 2.5s, 3s
    };
    let consumer_id = sim.add_component(consumer);

    // Schedule initial events
    println!("[Generator] Starting at 1s (then every 1s)");
    sim.schedule(SimTime::from_duration(Duration::from_secs(1)), generator_id, GeneratorEvent);
    
    println!("[Consumer] Starting at 0.5s (then every 0.5s)");
    sim.schedule(SimTime::from_duration(Duration::from_millis(500)), consumer_id, ConsumerEvent);

    println!("\n[Simulation] Running for {} seconds...\n", end_time.as_duration().as_secs());

    // Run the simulation
    Executor::timed(end_time).execute(&mut sim);

    println!("=== Simulation Complete ===\n");

    let final_log = log.lock().unwrap();
    
    println!("Final Results:");
    println!("  Simulation time: {:?}", sim.scheduler.time());
    println!("  Generator events: {} (expected 3)", final_log.generator_events.len());
    println!("  Consumer hellos: {} (expected 6)", final_log.consumer_hellos.len());

    // Verify counts
    assert_eq!(final_log.generator_events.len(), 3, "Expected 3 generator events");
    assert_eq!(final_log.consumer_hellos.len(), 6, "Expected 6 consumer hellos");

    // Verify interleaving: events should be ordered by time
    let mut all_events: Vec<(SimTime, &str)> = Vec::new();
    for time in &final_log.generator_events {
        all_events.push((*time, "generator"));
    }
    for time in &final_log.consumer_hellos {
        all_events.push((*time, "consumer"));
    }
    all_events.sort_by_key(|(time, _)| *time);

    println!("\nEvent sequence:");
    for (time, event_type) in &all_events {
        println!("  {time:?}: {event_type}");
    }

    // Verify the expected interleaving pattern
    let expected_pattern = vec![
        (SimTime::from_duration(Duration::from_millis(500)), "consumer"),
        (SimTime::from_duration(Duration::from_secs(1)), "generator"),
        (SimTime::from_duration(Duration::from_secs(1)), "consumer"),
        (SimTime::from_duration(Duration::from_millis(1500)), "consumer"),
        (SimTime::from_duration(Duration::from_secs(2)), "generator"),
        (SimTime::from_duration(Duration::from_secs(2)), "consumer"),
        (SimTime::from_duration(Duration::from_millis(2500)), "consumer"),
        (SimTime::from_duration(Duration::from_secs(3)), "generator"),
        (SimTime::from_duration(Duration::from_secs(3)), "consumer"),
    ];

    assert_eq!(all_events.len(), expected_pattern.len(), "Event count mismatch");
    
    for (i, ((actual_time, actual_type), (expected_time, expected_type))) in 
        all_events.iter().zip(expected_pattern.iter()).enumerate() {
        assert_eq!(
            actual_time, expected_time,
            "Event {i} time mismatch: expected {expected_time:?}, got {actual_time:?}",
        );
        assert_eq!(
            actual_type, expected_type,
            "Event {i} type mismatch: expected {expected_type}, got {actual_type}",
        );
    }

    println!("\n=== Test Passed ===\n");
}
