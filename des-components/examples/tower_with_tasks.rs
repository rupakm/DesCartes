//! Example demonstrating Tower middleware with Task-based implementations
//!
//! This example shows how the Task interface is used internally by Tower layers
//! for timeout handling, circuit breaker recovery, and other operations.
//!
//! Note: This example requires tokio runtime features.
//! For a simpler example without async, see task_usage.rs
//!
//! Run with: cargo run --package des-components --example tower_with_tasks

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Tower Integration with Task Interface ===\n");

    // Since we don't have tokio runtime features enabled,
    // we'll demonstrate the concepts without async execution
    demonstrate_task_patterns();

    println!("\n=== Tower Task Integration Concepts Demonstrated ===");
    Ok(())
}

// Helper function to demonstrate Task usage patterns
fn demonstrate_task_patterns() {
    println!("Task Patterns Used in Tower Layers:");
    println!();

    println!("1. TimeoutTask in DesTimeoutLayer:");
    println!("   The timeout layer creates a TimeoutTask when a request starts:");
    println!("   ```rust");
    println!("   let timeout_task = TimeoutTask::new(move |_scheduler| {{");
    println!("       // Wake the future when timeout expires");
    println!("       timeout_state.expire();");
    println!("       waker.wake();");
    println!("   }});");
    println!("   scheduler.schedule_task(timeout_deadline, timeout_task);");
    println!("   ```");
    println!();

    println!("2. TimeoutTask in Circuit Breaker Recovery:");
    println!("   When the circuit breaker opens, it schedules recovery:");
    println!("   ```rust");
    println!("   let recovery_task = TimeoutTask::new(move |_scheduler| {{");
    println!("       let mut state = state.lock().unwrap();");
    println!("       if matches!(*state, CircuitBreakerState::Open) {{");
    println!("           *state = CircuitBreakerState::HalfOpen;");
    println!("       }}");
    println!("   }});");
    println!("   scheduler.schedule_task(recovery_time, recovery_task);");
    println!("   ```");
    println!();

    println!("3. PeriodicTask for Rate Limiting:");
    println!("   ```rust");
    println!("   let refill_task = PeriodicTask::new(move |_scheduler| {{");
    println!("       let current = tokens.load(Ordering::Relaxed);");
    println!("       let new_tokens = (current + refill_rate).min(max_tokens);");
    println!("       tokens.store(new_tokens, Ordering::Relaxed);");
    println!("   }}, refill_interval);");
    println!("   ```");
    println!();

    println!("Key Task implementations in des-components:");
    println!("• TowerResponseTask - handles Tower service responses");
    println!("• TimeoutTask - used in timeout layer and circuit breaker");
    println!("• PeriodicTask - used in SimpleClient for request generation");
    println!("• ClosureTask - used for simple deferred operations");
}
