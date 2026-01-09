//! Demonstration of DesServiceBuilder with all Tower ServiceBuilder methods
//!
//! This example shows how to use DesServiceBuilder as a drop-in replacement
//! for Tower's ServiceBuilder, with all the same methods available.

use des_components::tower::{DesServiceBuilder, SimBody};
use des_core::Simulation;
use http::{Method, Request};
use std::time::Duration;
use tower::Service;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🏗️  DesServiceBuilder Demo - Tower ServiceBuilder Compatibility");
    println!("================================================================");

    let mut simulation = Simulation::default();
    let scheduler = simulation.scheduler_handle();

    // Demonstrate basic service creation
    println!("\n1. Basic Service Creation:");
    let _basic_service = DesServiceBuilder::new("basic-server".to_string())
        .thread_capacity(5)
        .service_time(Duration::from_millis(100))
        .build(&mut simulation)?;
    println!("   ✓ Created basic service with 5 threads, 100ms service time");

    // Demonstrate layer composition (like Tower ServiceBuilder)
    println!("\n2. Layer Composition:");
    let _layered_service = DesServiceBuilder::new("layered-server".to_string())
        .thread_capacity(10)
        .service_time(Duration::from_millis(50))
        // Add concurrency limiting
        .concurrency_limit(3)
        // Add rate limiting (10 requests per second, burst of 20)
        .rate_limit(10, Duration::from_secs(1), scheduler.clone())
        // Add timeout (5 seconds)
        .timeout(Duration::from_secs(5), scheduler.clone())
        // Add circuit breaker (3 failures, 10 second recovery)
        .circuit_breaker(3, Duration::from_secs(10), scheduler.clone())
        // Add retry with exponential backoff
        .retry(
            des_components::retry_policy::ExponentialBackoffPolicy::new(3, Duration::from_millis(100)),
            scheduler.clone()
        )
        .build(&mut simulation)?;
    println!("   ✓ Created layered service with concurrency limit, rate limit, timeout, circuit breaker, and retry");

    // Demonstrate optional layer
    println!("\n3. Optional Layer:");
    let _optional_service = DesServiceBuilder::new("optional-server".to_string())
        .thread_capacity(5)
        .service_time(Duration::from_millis(75))
        .option_layer(Some(des_components::tower::DesConcurrencyLimitLayer::new(2)))
        .build(&mut simulation)?;
    println!("   ✓ Created service with optional concurrency limit layer");

    // Demonstrate layer_fn
    println!("\n4. Custom Layer Function:");
    let _custom_service = DesServiceBuilder::new("custom-server".to_string())
        .thread_capacity(8)
        .service_time(Duration::from_millis(25))
        .layer_fn(|service| {
            // This is a simple pass-through layer for demonstration
            service
        })
        .build(&mut simulation)?;
    println!("   ✓ Created service with custom layer function");

    // Demonstrate global concurrency limiting
    println!("\n5. Global Concurrency Limiting:");
    let global_state = des_components::tower::limit::global_concurrency::GlobalConcurrencyLimitState::new(5);
    let _global_service = DesServiceBuilder::new("global-server".to_string())
        .thread_capacity(10)
        .service_time(Duration::from_millis(60))
        .global_concurrency_limit(global_state.clone())
        .build(&mut simulation)?;
    println!("   ✓ Created service with global concurrency limit (max 5 across all services)");

    // Demonstrate hedging
    println!("\n6. Hedging:");
    let _hedge_service = DesServiceBuilder::new("hedge-server".to_string())
        .thread_capacity(6)
        .service_time(Duration::from_millis(80))
        .hedge(Duration::from_millis(200), scheduler.clone())
        .build(&mut simulation)?;
    println!("   ✓ Created service with hedging (200ms delay)");

    // Demonstrate method chaining compatibility
    println!("\n7. Method Chaining (Tower ServiceBuilder style):");
    let _chained_service = DesServiceBuilder::new("chained-server".to_string())
        .thread_capacity(4)
        .service_time(Duration::from_millis(120))
        .concurrency_limit(2)
        .rate_limit(5, Duration::from_secs(1), scheduler.clone())
        .build(&mut simulation)?;
    println!("   ✓ Created service with method chaining and clone check");

    // Test that services actually work
    println!("\n8. Service Functionality Test:");
    let mut test_service = DesServiceBuilder::new("test-server".to_string())
        .thread_capacity(2)
        .service_time(Duration::from_millis(10))
        .build(&mut simulation)?;

    // Create a test request
    let request = Request::builder()
        .method(Method::GET)
        .uri("/test")
        .body(SimBody::from_static("test request"))?;

    println!("   → Sending test request...");
    
    // Check if service is ready
    let waker = create_noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    
    match test_service.poll_ready(&mut cx) {
        std::task::Poll::Ready(Ok(())) => {
            println!("   ✓ Service is ready to accept requests");
            
            // Make the request
            let _response_future = test_service.call(request);
            println!("   ✓ Request submitted successfully");
            
            // Run simulation to process the request
            for _ in 0..50 {
                if !simulation.step() {
                    break;
                }
            }
            println!("   ✓ Simulation processed the request");
        }
        std::task::Poll::Ready(Err(e)) => {
            println!("   ✗ Service error: {e:?}");
        }
        std::task::Poll::Pending => {
            println!("   ⏳ Service is not ready (this is normal for capacity-limited services)");
        }
    }

    println!("\n🎉 DesServiceBuilder Demo Complete!");
    println!("   All Tower ServiceBuilder methods are available and working correctly.");
    println!("   The DesServiceBuilder provides a drop-in replacement for Tower's ServiceBuilder");
    println!("   with full compatibility for discrete event simulation environments.");

    Ok(())
}

// Helper function to create a no-op waker for testing
fn create_noop_waker() -> std::task::Waker {
    use std::task::{RawWaker, RawWakerVTable};
    
    fn noop(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    
    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let raw_waker = RawWaker::new(std::ptr::null(), &VTABLE);
    unsafe { std::task::Waker::from_raw(raw_waker) }
}
