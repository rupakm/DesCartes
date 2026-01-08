//! Distribution traits and implementations for arrival patterns and service times
//!
//! This module provides traits and implementations for various probability distributions
//! used in discrete event simulation, including arrival patterns for clients and
//! service time distributions for servers.

use std::collections::HashMap;
use std::time::Duration;

/// Trait for generating arrival patterns
///
/// This trait abstracts over different arrival patterns for request generation
/// (Poisson, constant, bursty, etc.).
///
/// # Requirements
///
/// - 2.2: Provide Client component that generates requests according to configurable arrival patterns
pub trait ArrivalPattern: Send {
    /// Get the time until the next request arrival
    ///
    /// Returns the duration to wait before generating the next request.
    fn next_arrival_time(&mut self) -> Duration;
}

/// Trait for sampling service times from a distribution
///
/// This trait abstracts over different probability distributions for
/// service time generation (exponential, constant, uniform, etc.).
pub trait ServiceTimeDistribution: Send {
    /// Sample a service time from the distribution
    ///
    /// Returns the duration for processing a single request.
    fn sample(&mut self) -> Duration;
}

// =============================================================================
// Arrival Pattern Implementations
// =============================================================================

/// Simple constant arrival pattern
///
/// Generates requests with a fixed inter-arrival time.
#[derive(Debug, Clone)]
pub struct ConstantArrivalPattern {
    inter_arrival_time: Duration,
}

impl ConstantArrivalPattern {
    /// Create a new constant arrival pattern
    pub fn new(inter_arrival_time: Duration) -> Self {
        Self { inter_arrival_time }
    }
}

impl ArrivalPattern for ConstantArrivalPattern {
    fn next_arrival_time(&mut self) -> Duration {
        self.inter_arrival_time
    }
}

/// Poisson arrival pattern
///
/// Generates requests according to a Poisson process with exponentially
/// distributed inter-arrival times.
///
/// # Requirements
///
/// - 2.2: Provide Client component that generates requests according to configurable arrival patterns
pub struct PoissonArrivals {
    /// Rate parameter (lambda) - average arrivals per unit time
    rate: f64,
    /// Random number generator for sampling
    rng: rand::rngs::StdRng,
    /// Exponential distribution for inter-arrival times
    exp_dist: rand_distr::Exp<f64>,
}

impl PoissonArrivals {
    /// Create a new Poisson arrival pattern
    ///
    /// # Arguments
    ///
    /// * `rate` - Average number of arrivals per second (lambda parameter)
    ///
    /// # Panics
    ///
    /// Panics if rate is not positive.
    pub fn new(rate: f64) -> Self {
        assert!(rate > 0.0, "Rate must be positive");
        
        let exp_dist = rand_distr::Exp::new(rate).expect("Rate must be positive");
        
        Self {
            rate,
            rng: rand::SeedableRng::from_entropy(),
            exp_dist,
        }
    }
    
    /// Get the rate parameter
    pub fn rate(&self) -> f64 {
        self.rate
    }
}

impl ArrivalPattern for PoissonArrivals {
    fn next_arrival_time(&mut self) -> Duration {
        use rand::Rng;
        
        // Sample from exponential distribution to get inter-arrival time in seconds
        let inter_arrival_seconds: f64 = self.rng.sample(self.exp_dist);
        
        // Convert to Duration (nanoseconds)
        Duration::from_secs_f64(inter_arrival_seconds)
    }
}

/// Bursty arrival pattern
///
/// Generates requests in periodic bursts, with high activity during burst periods
/// and low/no activity between bursts.
///
/// # Requirements
///
/// - 2.2: Provide Client component that generates requests according to configurable arrival patterns
pub struct BurstyArrivals {
    /// Duration of each burst period
    burst_duration: Duration,
    /// Duration between bursts (quiet period)
    quiet_duration: Duration,
    /// Current phase of the pattern
    current_phase: BurstPhase,
    /// Time remaining in current phase
    phase_remaining: Duration,
    /// Random number generator
    rng: rand::rngs::StdRng,
    /// Exponential distribution for burst period
    burst_dist: Option<rand_distr::Exp<f64>>,
    /// Exponential distribution for quiet period
    quiet_dist: Option<rand_distr::Exp<f64>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BurstPhase {
    Burst,
    Quiet,
}

impl BurstyArrivals {
    /// Create a new bursty arrival pattern
    ///
    /// # Arguments
    ///
    /// * `burst_duration` - How long each burst lasts
    /// * `quiet_duration` - How long between bursts
    /// * `burst_rate` - Request rate during bursts (requests per second)
    /// * `quiet_rate` - Request rate during quiet periods (requests per second, can be 0)
    ///
    /// # Panics
    ///
    /// Panics if burst_rate is not positive or if quiet_rate is negative.
    pub fn new(
        burst_duration: Duration,
        quiet_duration: Duration,
        burst_rate: f64,
        quiet_rate: f64,
    ) -> Self {
        assert!(burst_rate > 0.0, "Burst rate must be positive");
        assert!(quiet_rate >= 0.0, "Quiet rate must be non-negative");
        
        let burst_dist = Some(rand_distr::Exp::new(burst_rate).expect("Burst rate must be positive"));
        let quiet_dist = if quiet_rate > 0.0 {
            Some(rand_distr::Exp::new(quiet_rate).expect("Quiet rate must be positive"))
        } else {
            None
        };
        
        Self {
            burst_duration,
            quiet_duration,
            current_phase: BurstPhase::Burst,
            phase_remaining: burst_duration,
            rng: rand::SeedableRng::from_entropy(),
            burst_dist,
            quiet_dist,
        }
    }
    
    /// Get the current phase
    pub fn current_phase(&self) -> BurstPhase {
        self.current_phase
    }
    
    /// Get the time remaining in the current phase
    pub fn phase_remaining(&self) -> Duration {
        self.phase_remaining
    }
}

impl ArrivalPattern for BurstyArrivals {
    fn next_arrival_time(&mut self) -> Duration {
        use rand::Rng;
        
        // Determine the inter-arrival time based on current phase
        let inter_arrival = match self.current_phase {
            BurstPhase::Burst => {
                if let Some(ref dist) = self.burst_dist {
                    let seconds: f64 = self.rng.sample(dist);
                    Duration::from_secs_f64(seconds)
                } else {
                    // This shouldn't happen since burst_rate > 0
                    Duration::from_secs(1)
                }
            }
            BurstPhase::Quiet => {
                if let Some(ref dist) = self.quiet_dist {
                    let seconds: f64 = self.rng.sample(dist);
                    Duration::from_secs_f64(seconds)
                } else {
                    // No arrivals during quiet period - return remaining quiet time
                    self.phase_remaining
                }
            }
        };
        
        // Update phase timing
        if inter_arrival >= self.phase_remaining {
            // Phase will end before next arrival
            let remaining_time = inter_arrival - self.phase_remaining;
            
            // Switch phases
            match self.current_phase {
                BurstPhase::Burst => {
                    self.current_phase = BurstPhase::Quiet;
                    self.phase_remaining = self.quiet_duration;
                }
                BurstPhase::Quiet => {
                    self.current_phase = BurstPhase::Burst;
                    self.phase_remaining = self.burst_duration;
                }
            }
            
            // Recursively calculate next arrival in new phase
            if remaining_time > Duration::ZERO {
                self.phase_remaining = self.phase_remaining.saturating_sub(remaining_time);
                inter_arrival
            } else {
                self.next_arrival_time()
            }
        } else {
            // Arrival happens within current phase
            self.phase_remaining = self.phase_remaining.saturating_sub(inter_arrival);
            inter_arrival
        }
    }
}

// =============================================================================
// Service Time Distribution Implementations
// =============================================================================

/// Constant service time distribution
///
/// Always returns the same service time.
#[derive(Debug, Clone)]
pub struct ConstantServiceTime {
    duration: Duration,
}

impl ConstantServiceTime {
    /// Create a new constant service time distribution
    pub fn new(duration: Duration) -> Self {
        Self { duration }
    }
}

impl ServiceTimeDistribution for ConstantServiceTime {
    fn sample(&mut self) -> Duration {
        self.duration
    }
}

/// Exponential service time distribution
///
/// Samples service times from an exponential distribution with a given rate parameter.
/// This is commonly used to model service times in queueing theory.
///
/// # Requirements
///
/// - 2.1: Provide Server component that processes requests with configurable service time distributions
pub struct ExponentialDistribution {
    /// Rate parameter (lambda) - average services per unit time
    rate: f64,
    /// Random number generator for sampling
    rng: rand::rngs::StdRng,
    /// Exponential distribution for service times
    exp_dist: rand_distr::Exp<f64>,
}

impl ExponentialDistribution {
    /// Create a new exponential service time distribution
    ///
    /// # Arguments
    ///
    /// * `rate` - Average number of services per second (lambda parameter)
    ///
    /// # Panics
    ///
    /// Panics if rate is not positive.
    pub fn new(rate: f64) -> Self {
        assert!(rate > 0.0, "Rate must be positive");
        
        let exp_dist = rand_distr::Exp::new(rate).expect("Rate must be positive");
        
        Self {
            rate,
            rng: rand::SeedableRng::from_entropy(),
            exp_dist,
        }
    }
    
    /// Get the rate parameter
    pub fn rate(&self) -> f64 {
        self.rate
    }
    
    /// Get the mean service time (1/rate)
    pub fn mean_service_time(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.rate)
    }
}

impl ServiceTimeDistribution for ExponentialDistribution {
    fn sample(&mut self) -> Duration {
        use rand::Rng;
        
        // Sample from exponential distribution to get service time in seconds
        let service_time_seconds: f64 = self.rng.sample(self.exp_dist);
        
        // Convert to Duration
        Duration::from_secs_f64(service_time_seconds)
    }
}

/// Uniform service time distribution
///
/// Samples service times uniformly from a given range [min, max].
///
/// # Requirements
///
/// - 2.1: Provide Server component that processes requests with configurable service time distributions
pub struct UniformDistribution {
    /// Minimum service time
    min_duration: Duration,
    /// Maximum service time
    max_duration: Duration,
    /// Random number generator for sampling
    rng: rand::rngs::StdRng,
    /// Uniform distribution for service times
    uniform_dist: rand_distr::Uniform<f64>,
}

impl UniformDistribution {
    /// Create a new uniform service time distribution
    ///
    /// # Arguments
    ///
    /// * `min_duration` - Minimum service time
    /// * `max_duration` - Maximum service time
    ///
    /// # Panics
    ///
    /// Panics if min_duration >= max_duration.
    pub fn new(min_duration: Duration, max_duration: Duration) -> Self {
        assert!(
            min_duration < max_duration,
            "Minimum duration must be less than maximum duration"
        );
        
        let min_secs = min_duration.as_secs_f64();
        let max_secs = max_duration.as_secs_f64();
        let uniform_dist = rand_distr::Uniform::new(min_secs, max_secs);
        
        Self {
            min_duration,
            max_duration,
            rng: rand::SeedableRng::from_entropy(),
            uniform_dist,
        }
    }
    
    /// Get the minimum service time
    pub fn min_duration(&self) -> Duration {
        self.min_duration
    }
    
    /// Get the maximum service time
    pub fn max_duration(&self) -> Duration {
        self.max_duration
    }
    
    /// Get the mean service time
    pub fn mean_service_time(&self) -> Duration {
        let mean_secs = (self.min_duration.as_secs_f64() + self.max_duration.as_secs_f64()) / 2.0;
        Duration::from_secs_f64(mean_secs)
    }
}

impl ServiceTimeDistribution for UniformDistribution {
    fn sample(&mut self) -> Duration {
        use rand::Rng;
        
        // Sample from uniform distribution to get service time in seconds
        let service_time_seconds: f64 = self.rng.sample(self.uniform_dist);
        
        // Convert to Duration
        Duration::from_secs_f64(service_time_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =============================================================================
    // Arrival Pattern Tests
    // =============================================================================

    #[test]
    fn test_constant_arrival_pattern() {
        let mut pattern = ConstantArrivalPattern::new(Duration::from_millis(100));
        assert_eq!(pattern.next_arrival_time(), Duration::from_millis(100));
        assert_eq!(pattern.next_arrival_time(), Duration::from_millis(100));
    }

    #[test]
    fn test_poisson_arrivals_creation() {
        let pattern = PoissonArrivals::new(1.0);
        assert_eq!(pattern.rate(), 1.0);
    }

    #[test]
    #[should_panic(expected = "Rate must be positive")]
    fn test_poisson_arrivals_invalid_rate() {
        PoissonArrivals::new(0.0);
    }

    #[test]
    fn test_poisson_arrivals_generates_positive_times() {
        let mut pattern = PoissonArrivals::new(10.0); // 10 arrivals per second
        
        // Generate several inter-arrival times
        for _ in 0..10 {
            let time = pattern.next_arrival_time();
            assert!(time > Duration::ZERO, "Inter-arrival time should be positive");
            // With rate 10, average inter-arrival time should be 0.1 seconds
            // Allow reasonable range for randomness
            assert!(time < Duration::from_secs(1), "Inter-arrival time should be reasonable");
        }
    }

    #[test]
    fn test_bursty_arrivals_creation() {
        let pattern = BurstyArrivals::new(
            Duration::from_secs(1),  // burst duration
            Duration::from_secs(2),  // quiet duration
            10.0,                    // burst rate
            1.0,                     // quiet rate
        );
        
        assert_eq!(pattern.current_phase(), BurstPhase::Burst);
        assert_eq!(pattern.phase_remaining(), Duration::from_secs(1));
    }

    #[test]
    #[should_panic(expected = "Burst rate must be positive")]
    fn test_bursty_arrivals_invalid_burst_rate() {
        BurstyArrivals::new(
            Duration::from_secs(1),
            Duration::from_secs(2),
            0.0,  // invalid
            1.0,
        );
    }

    #[test]
    #[should_panic(expected = "Quiet rate must be non-negative")]
    fn test_bursty_arrivals_invalid_quiet_rate() {
        BurstyArrivals::new(
            Duration::from_secs(1),
            Duration::from_secs(2),
            10.0,
            -1.0,  // invalid
        );
    }

    #[test]
    fn test_bursty_arrivals_generates_positive_times() {
        let mut pattern = BurstyArrivals::new(
            Duration::from_millis(100),  // short burst
            Duration::from_millis(200),  // short quiet
            100.0,                       // high burst rate
            0.0,                         // no quiet arrivals
        );
        
        // Generate several inter-arrival times
        for _ in 0..5 {
            let time = pattern.next_arrival_time();
            assert!(time > Duration::ZERO, "Inter-arrival time should be positive");
        }
    }

    #[test]
    fn test_bursty_arrivals_with_zero_quiet_rate() {
        let mut pattern = BurstyArrivals::new(
            Duration::from_millis(10),   // very short burst
            Duration::from_millis(100),  // longer quiet
            1000.0,                      // very high burst rate
            0.0,                         // no quiet arrivals
        );
        
        // During quiet phase, should return the remaining quiet time
        // This is a bit tricky to test deterministically due to randomness
        let time = pattern.next_arrival_time();
        assert!(time > Duration::ZERO);
    }

    // =============================================================================
    // Service Time Distribution Tests
    // =============================================================================

    #[test]
    fn test_constant_service_time() {
        let mut dist = ConstantServiceTime::new(Duration::from_millis(100));
        assert_eq!(dist.sample(), Duration::from_millis(100));
        assert_eq!(dist.sample(), Duration::from_millis(100));
    }

    #[test]
    fn test_exponential_distribution_creation() {
        let dist = ExponentialDistribution::new(2.0);
        assert_eq!(dist.rate(), 2.0);
        assert_eq!(dist.mean_service_time(), Duration::from_secs_f64(0.5));
    }

    #[test]
    #[should_panic(expected = "Rate must be positive")]
    fn test_exponential_distribution_invalid_rate() {
        ExponentialDistribution::new(0.0);
    }

    #[test]
    fn test_exponential_distribution_sampling() {
        let mut dist = ExponentialDistribution::new(10.0); // 10 services per second
        
        // Generate several service times
        for _ in 0..10 {
            let time = dist.sample();
            assert!(time > Duration::ZERO, "Service time should be positive");
            // With rate 10, average service time should be 0.1 seconds
            // Allow reasonable range for randomness
            assert!(time < Duration::from_secs(1), "Service time should be reasonable");
        }
    }

    #[test]
    fn test_uniform_distribution_creation() {
        let min = Duration::from_millis(50);
        let max = Duration::from_millis(150);
        let dist = UniformDistribution::new(min, max);
        
        assert_eq!(dist.min_duration(), min);
        assert_eq!(dist.max_duration(), max);
        assert_eq!(dist.mean_service_time(), Duration::from_millis(100));
    }

    #[test]
    #[should_panic(expected = "Minimum duration must be less than maximum duration")]
    fn test_uniform_distribution_invalid_range() {
        let min = Duration::from_millis(150);
        let max = Duration::from_millis(50);
        UniformDistribution::new(min, max);
    }

    #[test]
    #[should_panic(expected = "Minimum duration must be less than maximum duration")]
    fn test_uniform_distribution_equal_range() {
        let duration = Duration::from_millis(100);
        UniformDistribution::new(duration, duration);
    }

    #[test]
    fn test_uniform_distribution_sampling() {
        let min = Duration::from_millis(50);
        let max = Duration::from_millis(150);
        let mut dist = UniformDistribution::new(min, max);
        
        // Generate several service times
        for _ in 0..20 {
            let time = dist.sample();
            assert!(time >= min, "Service time should be >= minimum");
            assert!(time <= max, "Service time should be <= maximum");
        }
    }

    #[test]
    fn test_uniform_distribution_range_coverage() {
        let min = Duration::from_millis(100);
        let max = Duration::from_millis(200);
        let mut dist = UniformDistribution::new(min, max);
        
        let mut samples = Vec::new();
        for _ in 0..100 {
            samples.push(dist.sample());
        }
        
        // Check that we get values across the range
        let min_sample = samples.iter().min().unwrap();
        let max_sample = samples.iter().max().unwrap();
        
        // Should be reasonably close to the bounds (within 20% of range)
        let range = max.as_millis() - min.as_millis();
        let tolerance = range / 5; // 20% tolerance
        
        assert!(min_sample.as_millis() <= min.as_millis() + tolerance);
        assert!(max_sample.as_millis() >= max.as_millis() - tolerance);
    }
}