//! Distribution traits and implementations for arrival patterns and service times
//!
//! This module provides traits and implementations for various probability distributions
//! used in discrete event simulation, including arrival patterns for clients and
//! service time distributions for servers.

use std::collections::HashMap;
use std::time::Duration;

use crate::{DrawSite, RandomProvider, SimulationConfig};
use rand::SeedableRng;

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

/// Request context containing all information needed for service time calculation
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    pub method: String,
    pub uri: String,
    pub headers: HashMap<String, String>,
    pub body_size: usize,
    pub payload: Vec<u8>,
    pub client_info: Option<ClientInfo>,
}

#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub client_id: String,
    pub priority: u8,
    pub region: String,
}

/// Trait for sampling service times from a distribution
///
/// This trait abstracts over different probability distributions for
/// service time generation (exponential, constant, uniform, etc.).
/// It supports both simple distributions and request-dependent distributions.
pub trait ServiceTimeDistribution: Send {
    /// Sample a service time from the distribution
    ///
    /// Returns the duration for processing a single request.
    /// This is the legacy method for backward compatibility.
    fn sample(&mut self) -> Duration {
        self.sample_service_time(&RequestContext::default())
    }

    /// Sample service time based on request context
    ///
    /// This method allows distributions to vary service time based on
    /// request characteristics like method, URI, body size, etc.
    fn sample_service_time(&mut self, request: &RequestContext) -> Duration;

    /// Get expected service time for capacity planning
    ///
    /// Implementations should provide this for performance analysis.
    /// This method is required and must be implemented by each distribution.
    fn expected_service_time(&self, _request: &RequestContext) -> Duration {
        unimplemented!("expected_service_time must be implemented by each distribution")
    }
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

    /// Optional provider used to instrument / bias sampling.
    provider: Option<Box<dyn RandomProvider>>,

    /// Sampling site label used when delegating to `provider`.
    site: DrawSite,
}

impl PoissonArrivals {
    /// Create a new Poisson arrival pattern
    ///
    /// Uses an internal RNG seeded from entropy. For
    /// deterministic behavior tied to a specific simulation,
    /// prefer [`PoissonArrivals::from_config`].
    pub fn new(rate: f64) -> Self {
        assert!(rate > 0.0, "Rate must be positive");

        let exp_dist = rand_distr::Exp::new(rate).expect("Rate must be positive");

        Self {
            rate,
            rng: rand::SeedableRng::from_entropy(),
            exp_dist,
            provider: None,
            site: DrawSite::new("arrival", 0),
        }
    }

    /// Create a new Poisson arrival pattern using a
    /// `SimulationConfig`-derived RNG seed for deterministic
    /// behavior across runs.
    pub fn from_config(config: &SimulationConfig, rate: f64) -> Self {
        assert!(rate > 0.0, "Rate must be positive");

        let exp_dist = rand_distr::Exp::new(rate).expect("Rate must be positive");

        let mut seed = config.seed ^ 0xA5A5_5A5A_0101_0203u64;
        seed ^= rate.to_bits();

        Self {
            rate,
            rng: rand::rngs::StdRng::seed_from_u64(seed),
            exp_dist,
            provider: None,
            site: DrawSite::new("arrival", 0),
        }
    }
    ///
    /// Attach a provider used for sampling instrumentation.
    #[must_use]
    pub fn with_provider(mut self, provider: Box<dyn RandomProvider>, site: DrawSite) -> Self {
        self.provider = Some(provider);
        self.site = site;
        self
    }

    /// Get the rate parameter
    pub fn rate(&self) -> f64 {
        self.rate
    }
}

impl ArrivalPattern for PoissonArrivals {
    fn next_arrival_time(&mut self) -> Duration {
        use rand::Rng;

        // Sample from exponential distribution to get inter-arrival time in seconds.
        // When a provider is configured, delegate to it (for tracing / biasing).
        let inter_arrival_seconds: f64 = if let Some(provider) = &mut self.provider {
            provider.sample_exp_seconds(self.site, self.rate)
        } else {
            self.rng.sample(self.exp_dist)
        };

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
    /// Rate parameter for burst period
    burst_rate: f64,
    /// Rate parameter for quiet period
    quiet_rate: f64,

    /// Optional provider used to instrument / bias sampling.
    provider: Option<Box<dyn RandomProvider>>,

    /// Sampling site label used when delegating to `provider`.
    site: DrawSite,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BurstPhase {
    Burst,
    Quiet,
}

impl BurstyArrivals {
    /// Create a new bursty arrival pattern
    ///
    /// Uses an internal RNG seeded from entropy. For
    /// deterministic behavior tied to a specific simulation,
    /// prefer [`BurstyArrivals::from_config`].
    pub fn new(
        burst_duration: Duration,
        quiet_duration: Duration,
        burst_rate: f64,
        quiet_rate: f64,
    ) -> Self {
        assert!(burst_rate > 0.0, "Burst rate must be positive");
        assert!(quiet_rate >= 0.0, "Quiet rate must be non-negative");

        let burst_dist =
            Some(rand_distr::Exp::new(burst_rate).expect("Burst rate must be positive"));
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
            burst_rate,
            quiet_rate,
            provider: None,
            site: DrawSite::new("bursty_arrival", 0),
        }
    }

    /// Create a new bursty arrival pattern using a
    /// `SimulationConfig`-derived RNG seed for deterministic
    /// behavior across runs.
    pub fn from_config(
        config: &SimulationConfig,
        burst_duration: Duration,
        quiet_duration: Duration,
        burst_rate: f64,
        quiet_rate: f64,
    ) -> Self {
        assert!(burst_rate > 0.0, "Burst rate must be positive");
        assert!(quiet_rate >= 0.0, "Quiet rate must be non-negative");

        let burst_dist =
            Some(rand_distr::Exp::new(burst_rate).expect("Burst rate must be positive"));
        let quiet_dist = if quiet_rate > 0.0 {
            Some(rand_distr::Exp::new(quiet_rate).expect("Quiet rate must be positive"))
        } else {
            None
        };

        let mut seed = config.seed ^ 0xB4B4_4B4B_0202_0305u64;
        seed ^= burst_duration.as_nanos() as u64;
        seed ^= (quiet_duration.as_nanos() as u64).rotate_left(11);
        seed ^= burst_rate.to_bits();
        seed ^= quiet_rate.to_bits().rotate_left(7);

        Self {
            burst_duration,
            quiet_duration,
            current_phase: BurstPhase::Burst,
            phase_remaining: burst_duration,
            rng: rand::rngs::StdRng::seed_from_u64(seed),
            burst_dist,
            quiet_dist,
            burst_rate,
            quiet_rate,
            provider: None,
            site: DrawSite::new("bursty_arrival", 0),
        }
    }
    ///
    /// Get the current phase
    pub fn current_phase(&self) -> BurstPhase {
        self.current_phase
    }

    /// Get the time remaining in the current phase
    pub fn phase_remaining(&self) -> Duration {
        self.phase_remaining
    }

    ///
    /// Attach a provider used for sampling instrumentation.
    #[must_use]
    pub fn with_provider(mut self, provider: Box<dyn RandomProvider>, site: DrawSite) -> Self {
        self.provider = Some(provider);
        self.site = site;
        self
    }
}

impl ArrivalPattern for BurstyArrivals {
    fn next_arrival_time(&mut self) -> Duration {
        use rand::Rng;

        // Determine the inter-arrival time based on current phase
        let inter_arrival = match self.current_phase {
            BurstPhase::Burst => {
                if let Some(ref dist) = self.burst_dist {
                    // Sample from exponential distribution to get inter-arrival time in seconds.
                    // When a provider is configured, delegate to it (for tracing / biasing).
                    let seconds: f64 = if let Some(provider) = &mut self.provider {
                        provider.sample_exp_seconds(self.site, self.burst_rate)
                    } else {
                        self.rng.sample(dist)
                    };
                    Duration::from_secs_f64(seconds)
                } else {
                    // This shouldn't happen since burst_rate > 0
                    Duration::from_secs(1)
                }
            }
            BurstPhase::Quiet => {
                if let Some(ref dist) = self.quiet_dist {
                    // Sample from exponential distribution to get inter-arrival time in seconds.
                    // When a provider is configured, delegate to it (for tracing / biasing).
                    let seconds: f64 = if let Some(provider) = &mut self.provider {
                        provider.sample_exp_seconds(self.site, self.quiet_rate)
                    } else {
                        self.rng.sample(dist)
                    };
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

    fn sample_service_time(&mut self, _request: &RequestContext) -> Duration {
        self.duration
    }

    fn expected_service_time(&self, _request: &RequestContext) -> Duration {
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

    /// Optional provider used to instrument / bias sampling.
    provider: Option<Box<dyn RandomProvider>>,

    /// Sampling site label used when delegating to `provider`.
    site: DrawSite,
}

impl ExponentialDistribution {
    /// Create a new exponential service time distribution
    ///
    /// Uses an internal RNG seeded from entropy. For
    /// deterministic behavior tied to a specific simulation,
    /// prefer [`ExponentialDistribution::from_config`].
    pub fn new(rate: f64) -> Self {
        assert!(rate > 0.0, "Rate must be positive");

        let exp_dist = rand_distr::Exp::new(rate).expect("Rate must be positive");

        Self {
            rate,
            rng: rand::SeedableRng::from_entropy(),
            exp_dist,
            provider: None,
            site: DrawSite::new("service", 0),
        }
    }

    /// Create a new exponential service time distribution using a
    /// `SimulationConfig`-derived RNG seed for deterministic
    /// behavior across runs.
    pub fn from_config(config: &SimulationConfig, rate: f64) -> Self {
        assert!(rate > 0.0, "Rate must be positive");

        let exp_dist = rand_distr::Exp::new(rate).expect("Rate must be positive");

        let mut seed = config.seed ^ 0xC3C3_3C3C_0303_0407u64;
        seed ^= rate.to_bits();

        Self {
            rate,
            rng: rand::rngs::StdRng::seed_from_u64(seed),
            exp_dist,
            provider: None,
            site: DrawSite::new("service", 0),
        }
    }
    ///
    /// Attach a provider used for sampling instrumentation.
    #[must_use]
    pub fn with_provider(mut self, provider: Box<dyn RandomProvider>, site: DrawSite) -> Self {
        self.provider = Some(provider);
        self.site = site;
        self
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

        // Sample from exponential distribution to get service time in seconds.
        // When a provider is configured, delegate to it (for tracing / biasing).
        let service_time_seconds: f64 = if let Some(provider) = &mut self.provider {
            provider.sample_exp_seconds(self.site, self.rate)
        } else {
            self.rng.sample(self.exp_dist)
        };

        // Convert to Duration
        Duration::from_secs_f64(service_time_seconds)
    }

    fn sample_service_time(&mut self, _request: &RequestContext) -> Duration {
        self.sample()
    }

    fn expected_service_time(&self, _request: &RequestContext) -> Duration {
        self.mean_service_time()
    }
}

/// Uniform service time distribution
///
/// Samples service times uniformly from a given range [min, max].
///
/// Note: This distribution does not currently support RandomProvider injection
/// because the RandomProvider trait only provides sample_exp_seconds(). Adding
/// uniform sampling would require extending the trait interface, which is
/// out of scope for the current provider injection effort.
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
    /// Uses an internal RNG seeded from entropy. For
    /// deterministic behavior tied to a specific simulation,
    /// prefer [`UniformDistribution::from_config`].
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

    /// Create a new uniform service time distribution using a
    /// `SimulationConfig`-derived RNG seed for deterministic
    /// behavior across runs.
    pub fn from_config(
        config: &SimulationConfig,
        min_duration: Duration,
        max_duration: Duration,
    ) -> Self {
        assert!(
            min_duration < max_duration,
            "Minimum duration must be less than maximum duration"
        );

        let min_secs = min_duration.as_secs_f64();
        let max_secs = max_duration.as_secs_f64();
        let uniform_dist = rand_distr::Uniform::new(min_secs, max_secs);

        let mut seed = config.seed ^ 0xD2D2_2D2D_0404_0509u64;
        seed ^= min_duration.as_nanos() as u64;
        seed ^= (max_duration.as_nanos() as u64).rotate_left(13);

        Self {
            min_duration,
            max_duration,
            rng: rand::rngs::StdRng::seed_from_u64(seed),
            uniform_dist,
        }
    }
    ///
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

    fn sample_service_time(&mut self, _request: &RequestContext) -> Duration {
        self.sample()
    }

    fn expected_service_time(&self, _request: &RequestContext) -> Duration {
        self.mean_service_time()
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
            assert!(
                time > Duration::ZERO,
                "Inter-arrival time should be positive"
            );
            // With rate 10, average inter-arrival time should be 0.1 seconds
            // Allow reasonable range for randomness
            assert!(
                time < Duration::from_secs(1),
                "Inter-arrival time should be reasonable"
            );
        }
    }

    #[test]
    fn test_poisson_arrivals_provider_injection() {
        // Mock provider that returns deterministic values
        struct MockProvider {
            call_count: std::cell::RefCell<usize>,
            fixed_value: f64,
        }

        impl MockProvider {
            fn new(fixed_value: f64) -> Self {
                Self {
                    call_count: std::cell::RefCell::new(0),
                    fixed_value,
                }
            }

            fn get_call_count(&self) -> usize {
                *self.call_count.borrow()
            }
        }

        impl RandomProvider for MockProvider {
            fn sample_exp_seconds(&mut self, _site: DrawSite, _rate: f64) -> f64 {
                *self.call_count.borrow_mut() += 1;
                self.fixed_value
            }
        }

        let provider = Box::new(MockProvider::new(0.123)); // Fixed inter-arrival time
        let provider_ptr = provider.as_ref() as *const MockProvider;

        let mut pattern =
            PoissonArrivals::new(2.0).with_provider(provider, DrawSite::new("test", 1));

        // Sample a few times to ensure provider is used
        for _ in 0..3 {
            let time = pattern.next_arrival_time();
            assert_eq!(time, Duration::from_secs_f64(0.123));
        }

        // Verify provider was called
        unsafe {
            assert_eq!((*provider_ptr).get_call_count(), 3);
        }
    }

    #[test]
    fn test_bursty_arrivals_creation() {
        let pattern = BurstyArrivals::new(
            Duration::from_secs(1), // burst duration
            Duration::from_secs(2), // quiet duration
            10.0,                   // burst rate
            1.0,                    // quiet rate
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
            0.0, // invalid
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
            -1.0, // invalid
        );
    }

    #[test]
    fn test_bursty_arrivals_generates_positive_times() {
        let mut pattern = BurstyArrivals::new(
            Duration::from_millis(100), // short burst
            Duration::from_millis(200), // short quiet
            100.0,                      // high burst rate
            0.0,                        // no quiet arrivals
        );

        // Generate several inter-arrival times
        for _ in 0..5 {
            let time = pattern.next_arrival_time();
            assert!(
                time > Duration::ZERO,
                "Inter-arrival time should be positive"
            );
        }
    }

    #[test]
    fn test_bursty_arrivals_with_zero_quiet_rate() {
        let mut pattern = BurstyArrivals::new(
            Duration::from_millis(10),  // very short burst
            Duration::from_millis(100), // longer quiet
            1000.0,                     // very high burst rate
            0.0,                        // no quiet arrivals
        );

        // During quiet phase, should return the remaining quiet time
        // This is a bit tricky to test deterministically due to randomness
        let time = pattern.next_arrival_time();
        assert!(time > Duration::ZERO);
    }

    #[test]
    fn test_bursty_arrivals_provider_injection() {
        // Mock provider that returns deterministic values
        struct MockProvider {
            call_count: std::cell::RefCell<usize>,
            fixed_value: f64,
        }

        impl MockProvider {
            fn new(fixed_value: f64) -> Self {
                Self {
                    call_count: std::cell::RefCell::new(0),
                    fixed_value,
                }
            }

            fn get_call_count(&self) -> usize {
                *self.call_count.borrow()
            }
        }

        impl RandomProvider for MockProvider {
            fn sample_exp_seconds(&mut self, _site: DrawSite, _rate: f64) -> f64 {
                *self.call_count.borrow_mut() += 1;
                self.fixed_value
            }
        }

        let provider = Box::new(MockProvider::new(0.5)); // Fixed 0.5 second inter-arrival
        let provider_ptr = provider.as_ref() as *const MockProvider;

        let mut pattern = BurstyArrivals::new(
            Duration::from_secs(1), // burst duration
            Duration::from_secs(1), // quiet duration
            2.0,                    // burst rate
            1.0,                    // quiet rate
        )
        .with_provider(provider, DrawSite::new("test", 1));

        // Sample a few times to ensure provider is used
        for _ in 0..5 {
            let time = pattern.next_arrival_time();
            assert_eq!(time, Duration::from_secs_f64(0.5));
        }

        // Verify provider was called (should be called during burst phase)
        unsafe {
            assert!((*provider_ptr).get_call_count() > 0);
        }
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
            assert!(
                time < Duration::from_secs(1),
                "Service time should be reasonable"
            );
        }
    }

    #[test]
    fn test_exponential_distribution_provider_injection() {
        // Mock provider that returns deterministic values
        struct MockProvider {
            call_count: std::cell::RefCell<usize>,
            fixed_value: f64,
        }

        impl MockProvider {
            fn new(fixed_value: f64) -> Self {
                Self {
                    call_count: std::cell::RefCell::new(0),
                    fixed_value,
                }
            }

            fn get_call_count(&self) -> usize {
                *self.call_count.borrow()
            }
        }

        impl RandomProvider for MockProvider {
            fn sample_exp_seconds(&mut self, _site: DrawSite, _rate: f64) -> f64 {
                *self.call_count.borrow_mut() += 1;
                self.fixed_value
            }
        }

        let provider = Box::new(MockProvider::new(0.456)); // Fixed service time
        let provider_ptr = provider.as_ref() as *const MockProvider;

        let mut dist =
            ExponentialDistribution::new(5.0).with_provider(provider, DrawSite::new("test", 1));

        // Sample a few times to ensure provider is used
        for _ in 0..3 {
            let time = dist.sample();
            assert_eq!(time, Duration::from_secs_f64(0.456));
        }

        // Verify provider was called
        unsafe {
            assert_eq!((*provider_ptr).get_call_count(), 3);
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

// =============================================================================
// Request-Dependent Service Time Distribution Implementations
// =============================================================================

/// Request size-based service time distribution
///
/// Service time is calculated as base_time + (body_size * time_per_byte),
/// capped at max_time to prevent unrealistic service times.
pub struct RequestSizeBasedServiceTime {
    base_time: Duration,
    time_per_byte: Duration,
    max_time: Duration,
}

impl RequestSizeBasedServiceTime {
    /// Create a new request size-based service time distribution
    ///
    /// # Arguments
    ///
    /// * `base_time` - Minimum service time regardless of request size
    /// * `time_per_byte` - Additional time per byte of request body
    /// * `max_time` - Maximum service time cap
    pub fn new(base_time: Duration, time_per_byte: Duration, max_time: Duration) -> Self {
        Self {
            base_time,
            time_per_byte,
            max_time,
        }
    }

    /// Get the base service time
    pub fn base_time(&self) -> Duration {
        self.base_time
    }

    /// Get the time per byte
    pub fn time_per_byte(&self) -> Duration {
        self.time_per_byte
    }

    /// Get the maximum service time
    pub fn max_time(&self) -> Duration {
        self.max_time
    }
}

impl ServiceTimeDistribution for RequestSizeBasedServiceTime {
    fn sample_service_time(&mut self, request: &RequestContext) -> Duration {
        let size_time = self.time_per_byte * request.body_size as u32;
        let total_time = self.base_time + size_time;
        std::cmp::min(total_time, self.max_time)
    }

    fn expected_service_time(&self, request: &RequestContext) -> Duration {
        let size_time = self.time_per_byte * request.body_size as u32;
        let total_time = self.base_time + size_time;
        std::cmp::min(total_time, self.max_time)
    }
}

/// Endpoint-based service time distribution
///
/// Different endpoints can have different service time distributions.
/// Falls back to a default distribution for unmatched endpoints.
pub struct EndpointBasedServiceTime {
    endpoint_distributions: HashMap<String, Box<dyn ServiceTimeDistribution>>,
    default_distribution: Box<dyn ServiceTimeDistribution>,
}

impl EndpointBasedServiceTime {
    /// Create a new endpoint-based service time distribution
    ///
    /// # Arguments
    ///
    /// * `default_distribution` - Distribution to use for unmatched endpoints
    pub fn new(default_distribution: Box<dyn ServiceTimeDistribution>) -> Self {
        Self {
            endpoint_distributions: HashMap::new(),
            default_distribution,
        }
    }

    /// Add a distribution for a specific endpoint
    ///
    /// # Arguments
    ///
    /// * `endpoint` - URI path to match (e.g., "/api/users")
    /// * `distribution` - Service time distribution for this endpoint
    pub fn add_endpoint(
        &mut self,
        endpoint: String,
        distribution: Box<dyn ServiceTimeDistribution>,
    ) {
        self.endpoint_distributions.insert(endpoint, distribution);
    }
}

impl ServiceTimeDistribution for EndpointBasedServiceTime {
    fn sample_service_time(&mut self, request: &RequestContext) -> Duration {
        if let Some(dist) = self.endpoint_distributions.get_mut(&request.uri) {
            dist.sample_service_time(request)
        } else {
            self.default_distribution.sample_service_time(request)
        }
    }

    fn expected_service_time(&self, request: &RequestContext) -> Duration {
        if let Some(dist) = self.endpoint_distributions.get(&request.uri) {
            dist.expected_service_time(request)
        } else {
            self.default_distribution.expected_service_time(request)
        }
    }
}

/// Trait for request predicates used in composite distributions
pub trait RequestPredicate: Send {
    /// Check if the predicate matches the given request
    fn matches(&self, request: &RequestContext) -> bool;
}

/// Service time rule for composite distributions
pub struct ServiceTimeRule {
    pub condition: Box<dyn RequestPredicate>,
    pub distribution: Box<dyn ServiceTimeDistribution>,
}

/// Composite service time distribution with conditional rules
///
/// Evaluates rules in order and uses the first matching distribution.
/// Falls back to default distribution if no rules match.
pub struct CompositeServiceTime {
    rules: Vec<ServiceTimeRule>,
    default_distribution: Box<dyn ServiceTimeDistribution>,
}

impl CompositeServiceTime {
    /// Create a new composite service time distribution
    ///
    /// # Arguments
    ///
    /// * `default_distribution` - Distribution to use when no rules match
    pub fn new(default_distribution: Box<dyn ServiceTimeDistribution>) -> Self {
        Self {
            rules: Vec::new(),
            default_distribution,
        }
    }

    /// Add a rule to the composite distribution
    ///
    /// Rules are evaluated in the order they are added.
    ///
    /// # Arguments
    ///
    /// * `condition` - Predicate to match requests
    /// * `distribution` - Distribution to use for matching requests
    pub fn add_rule(
        &mut self,
        condition: Box<dyn RequestPredicate>,
        distribution: Box<dyn ServiceTimeDistribution>,
    ) {
        self.rules.push(ServiceTimeRule {
            condition,
            distribution,
        });
    }
}

impl ServiceTimeDistribution for CompositeServiceTime {
    fn sample_service_time(&mut self, request: &RequestContext) -> Duration {
        for rule in &mut self.rules {
            if rule.condition.matches(request) {
                return rule.distribution.sample_service_time(request);
            }
        }
        self.default_distribution.sample_service_time(request)
    }

    fn expected_service_time(&self, request: &RequestContext) -> Duration {
        for rule in &self.rules {
            if rule.condition.matches(request) {
                return rule.distribution.expected_service_time(request);
            }
        }
        self.default_distribution.expected_service_time(request)
    }
}

// =============================================================================
// Request Predicate Implementations
// =============================================================================

/// Predicate that matches requests by HTTP method
pub struct MethodPredicate(pub String);

impl RequestPredicate for MethodPredicate {
    fn matches(&self, request: &RequestContext) -> bool {
        request.method == self.0
    }
}

/// Predicate that matches requests by URI prefix
pub struct UriPrefixPredicate(pub String);

impl RequestPredicate for UriPrefixPredicate {
    fn matches(&self, request: &RequestContext) -> bool {
        request.uri.starts_with(&self.0)
    }
}

/// Predicate that matches requests by exact URI
pub struct UriExactPredicate(pub String);

impl RequestPredicate for UriExactPredicate {
    fn matches(&self, request: &RequestContext) -> bool {
        request.uri == self.0
    }
}

/// Predicate that matches requests by body size range
pub struct BodySizePredicate(pub std::ops::Range<usize>);

impl RequestPredicate for BodySizePredicate {
    fn matches(&self, request: &RequestContext) -> bool {
        self.0.contains(&request.body_size)
    }
}

/// Predicate that matches requests by header presence and value
pub struct HeaderPredicate {
    pub header_name: String,
    pub header_value: String,
}

impl RequestPredicate for HeaderPredicate {
    fn matches(&self, request: &RequestContext) -> bool {
        request
            .headers
            .get(&self.header_name)
            .map(|value| value == &self.header_value)
            .unwrap_or(false)
    }
}

/// Predicate that combines multiple predicates with AND logic
pub struct AndPredicate(pub Vec<Box<dyn RequestPredicate>>);

impl RequestPredicate for AndPredicate {
    fn matches(&self, request: &RequestContext) -> bool {
        self.0.iter().all(|predicate| predicate.matches(request))
    }
}

/// Predicate that combines multiple predicates with OR logic
pub struct OrPredicate(pub Vec<Box<dyn RequestPredicate>>);

impl RequestPredicate for OrPredicate {
    fn matches(&self, request: &RequestContext) -> bool {
        self.0.iter().any(|predicate| predicate.matches(request))
    }
}
