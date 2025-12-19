//! Distribution patterns for simulation components
//!
//! This module provides traits and implementations for various distribution patterns
//! used in discrete event simulation, including arrival patterns and service time distributions.

use std::time::Duration;

/// Trait for generating arrival patterns
///
/// Arrival patterns determine the timing between events in a simulation.
pub trait ArrivalPattern: Send {
    /// Get the time until the next arrival
    fn next_arrival_time(&mut self) -> Duration;
}

/// Trait for service time distributions
///
/// Service time distributions determine how long it takes to process a request.
pub trait ServiceTimeDistribution: Send {
    /// Get the service time for processing a request
    fn next_service_time(&mut self) -> Duration;
}

/// Constant arrival pattern - arrivals occur at fixed intervals
#[derive(Debug, Clone)]
pub struct ConstantArrivalPattern {
    interval: Duration,
}

impl ConstantArrivalPattern {
    /// Create a new constant arrival pattern
    pub fn new(interval: Duration) -> Self {
        Self { interval }
    }
}

impl ArrivalPattern for ConstantArrivalPattern {
    fn next_arrival_time(&mut self) -> Duration {
        self.interval
    }
}

/// Constant service time distribution - all requests take the same time to process
#[derive(Debug, Clone)]
pub struct ConstantServiceTime {
    service_time: Duration,
}

impl ConstantServiceTime {
    /// Create a new constant service time distribution
    pub fn new(service_time: Duration) -> Self {
        Self { service_time }
    }
}

impl ServiceTimeDistribution for ConstantServiceTime {
    fn next_service_time(&mut self) -> Duration {
        self.service_time
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_arrival_pattern() {
        let mut pattern = ConstantArrivalPattern::new(Duration::from_millis(100));
        
        assert_eq!(pattern.next_arrival_time(), Duration::from_millis(100));
        assert_eq!(pattern.next_arrival_time(), Duration::from_millis(100));
    }

    #[test]
    fn test_constant_service_time() {
        let mut service = ConstantServiceTime::new(Duration::from_millis(50));
        
        assert_eq!(service.next_service_time(), Duration::from_millis(50));
        assert_eq!(service.next_service_time(), Duration::from_millis(50));
    }
}