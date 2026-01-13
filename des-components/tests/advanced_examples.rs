//! Advanced Examples Test Suite
//!
//! This file contains runnable tests for all examples in Chapter 7: Advanced Examples.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;
use std::time::Duration;

// Custom distribution types for testing
#[derive(Debug, Clone)]
pub struct EmpiricalDistribution {
    pub name: String,
    pub buckets: Vec<(Duration, f64)>, // (upper_bound, cumulative_probability)
    pub total_samples: usize,
}

impl EmpiricalDistribution {
    pub fn from_measurements(name: String, measurements: Vec<Duration>) -> Self {
        let mut sorted_measurements = measurements.clone();
        sorted_measurements.sort();

        // Create histogram buckets
        let mut buckets = Vec::new();
        let total_samples = measurements.len();

        for (i, &duration) in sorted_measurements.iter().enumerate() {
            let cumulative_prob = (i + 1) as f64 / total_samples as f64;
            buckets.push((duration, cumulative_prob));
        }

        Self {
            name,
            buckets,
            total_samples,
        }
    }

    pub fn sample(&self, rng: &mut impl Rng) -> Duration {
        let random_value: f64 = rng.gen();

        // Find the first bucket where cumulative_prob >= random_value
        for (duration, cumulative_prob) in &self.buckets {
            if random_value <= *cumulative_prob {
                return *duration;
            }
        }

        // Fallback to last bucket
        self.buckets
            .last()
            .map(|(d, _)| *d)
            .unwrap_or(Duration::from_millis(100))
    }

    pub fn percentile(&self, p: f64) -> Option<Duration> {
        let target_prob = p / 100.0;

        for (duration, cumulative_prob) in &self.buckets {
            if *cumulative_prob >= target_prob {
                return Some(*duration);
            }
        }

        None
    }

    pub fn mean(&self) -> Duration {
        if self.buckets.is_empty() {
            return Duration::from_millis(0);
        }

        let total_ms: u64 = self
            .buckets
            .iter()
            .map(|(duration, _)| duration.as_millis() as u64)
            .sum();

        Duration::from_millis(total_ms / self.buckets.len() as u64)
    }
}

#[derive(Debug, Clone)]
pub struct MixtureDistribution {
    pub name: String,
    pub components: Vec<(f64, Duration)>, // (weight, fixed_duration) for simplicity
}

impl MixtureDistribution {
    pub fn new(name: String) -> Self {
        Self {
            name,
            components: Vec::new(),
        }
    }

    pub fn add_component(mut self, weight: f64, duration: Duration) -> Self {
        self.components.push((weight, duration));
        self
    }

    pub fn sample(&self, rng: &mut impl Rng) -> Duration {
        if self.components.is_empty() {
            return Duration::from_millis(100); // Default
        }

        // Normalize weights
        let total_weight: f64 = self.components.iter().map(|(w, _)| w).sum();
        let random_value: f64 = rng.gen::<f64>() * total_weight;

        let mut cumulative_weight = 0.0;
        for (weight, duration) in &self.components {
            cumulative_weight += weight;
            if random_value <= cumulative_weight {
                return *duration;
            }
        }

        // Fallback to last component
        self.components.last().unwrap().1
    }
}

#[test]
fn test_payload_dependent_service_times() {
    println!("🚀 Testing Payload-Dependent Service Times");
    println!("==========================================");

    // Test different payload sizes and their impact on service times
    let test_cases = vec![
        ("Small payload", 100, 10),   // 100 bytes -> ~10ms base time
        ("Medium payload", 1000, 20), // 1000 bytes -> ~20ms
        ("Large payload", 5000, 60),  // 5000 bytes -> ~60ms
        ("Huge payload", 50000, 200), // 50000 bytes -> capped at 200ms
    ];

    println!("📋 Test Configuration:");
    println!("   - Base service time: 10ms");
    println!("   - Additional time: 10μs per byte");
    println!("   - Maximum service time: 200ms");
    println!();

    for (description, payload_size, expected_time_ms) in test_cases {
        // Simulate payload-dependent service time calculation
        let base_time_ms = 10;
        let time_per_byte_us = 10; // 10 microseconds per byte
        let max_time_ms = 200;

        let additional_time_ms = (payload_size * time_per_byte_us) / 1000; // Convert μs to ms
        let calculated_time_ms = (base_time_ms + additional_time_ms).min(max_time_ms);

        println!(
            "📨 {}: {} bytes -> {}ms (expected ~{}ms)",
            description, payload_size, calculated_time_ms, expected_time_ms
        );

        // Verify the calculation is reasonable
        assert!(
            calculated_time_ms >= base_time_ms,
            "Should be at least base time"
        );
        assert!(
            calculated_time_ms <= max_time_ms,
            "Should not exceed maximum"
        );

        // Allow some tolerance in the expected time
        let tolerance = 10; // 10ms tolerance
        assert!(
            (calculated_time_ms as i32 - expected_time_ms as i32).abs() <= tolerance,
            "Calculated time should be close to expected"
        );
    }

    println!();
    println!("✅ Payload-dependent service time calculations verified!");
    println!("✅ All test cases passed with expected service time patterns!");
}

#[test]
fn test_empirical_distribution_from_data() {
    println!("🚀 Testing Empirical Distribution from Data");
    println!("===========================================");

    // Create empirical distribution from sample data
    let sample_measurements = vec![
        Duration::from_millis(45),
        Duration::from_millis(52),
        Duration::from_millis(67),
        Duration::from_millis(89),
        Duration::from_millis(123),
        Duration::from_millis(156),
        Duration::from_millis(234),
        Duration::from_millis(445),
    ];

    println!("📋 Test Configuration:");
    println!(
        "   - Sample measurements: {} data points",
        sample_measurements.len()
    );
    println!(
        "   - Range: {}ms - {}ms",
        sample_measurements.iter().min().unwrap().as_millis(),
        sample_measurements.iter().max().unwrap().as_millis()
    );
    println!();

    // Create empirical distribution
    let empirical_dist = EmpiricalDistribution::from_measurements(
        "production-data".to_string(),
        sample_measurements.clone(),
    );

    // Test distribution properties
    let mean = empirical_dist.mean();
    let p50 = empirical_dist.percentile(50.0);
    let p95 = empirical_dist.percentile(95.0);

    println!("📊 Empirical Distribution Analysis:");
    println!("   Mean: {}ms", mean.as_millis());
    if let Some(p50_val) = p50 {
        println!("   P50: {}ms", p50_val.as_millis());
    }
    if let Some(p95_val) = p95 {
        println!("   P95: {}ms", p95_val.as_millis());
    }

    // Sample from the distribution.
    // Use a fixed seed to keep this test deterministic and non-flaky.
    let mut rng = StdRng::seed_from_u64(1);
    let sample_size = 1000;

    let measurement_min = *sample_measurements.iter().min().unwrap();
    let measurement_max = *sample_measurements.iter().max().unwrap();

    println!(
        "🎲 Sampling {} times from empirical distribution:",
        sample_size
    );

    let mut observed_min = Duration::MAX;
    let mut observed_max = Duration::ZERO;

    for _ in 0..sample_size {
        let sample = empirical_dist.sample(&mut rng);
        assert!(
            sample >= measurement_min && sample <= measurement_max,
            "empirical sample {sample:?} outside observed data range"
        );

        observed_min = observed_min.min(sample);
        observed_max = observed_max.max(sample);
    }

    println!(
        "   Observed sample range: {}ms - {}ms",
        observed_min.as_millis(),
        observed_max.as_millis()
    );

    println!("✅ Empirical distribution test completed successfully!");
}

#[test]
fn test_mixture_distribution_patterns() {
    println!("🚀 Testing Mixture Distribution Patterns");
    println!("======================================");

    let mixture_dist = MixtureDistribution::new("bimodal-test".to_string())
        .add_component(0.8, Duration::from_millis(50))
        .add_component(0.2, Duration::from_millis(500));

    // Use a fixed seed so the ratio assertions are stable.
    let mut rng = StdRng::seed_from_u64(2);

    let mut fast_count = 0;
    let mut slow_count = 0;
    let sample_size = 1000;

    println!(
        "🎲 Sampling {} times from mixture distribution:",
        sample_size
    );

    for _ in 0..sample_size {
        let sample = mixture_dist.sample(&mut rng);
        if sample.as_millis() < 100 {
            fast_count += 1;
        } else {
            slow_count += 1;
        }
    }

    let fast_ratio = fast_count as f64 / sample_size as f64;
    let slow_ratio = slow_count as f64 / sample_size as f64;

    println!(
        "   Fast samples (< 100ms): {} ({:.1}%)",
        fast_count,
        fast_ratio * 100.0
    );
    println!(
        "   Slow samples (≥ 100ms): {} ({:.1}%)",
        slow_count,
        slow_ratio * 100.0
    );

    // Verify the distribution matches expected ratios (within tolerance)
    let tolerance = 0.05; // 5% tolerance
    assert!(
        (fast_ratio - 0.8).abs() < tolerance,
        "Fast ratio should be ~80%, got {:.1}%",
        fast_ratio * 100.0
    );
    assert!(
        (slow_ratio - 0.2).abs() < tolerance,
        "Slow ratio should be ~20%, got {:.1}%",
        slow_ratio * 100.0
    );

    println!("✅ Mixture distribution patterns test completed successfully!");
}

#[derive(Debug, Clone)]
pub struct HeavyTailedDistribution {
    pub name: String,
    pub min_value: Duration,
    pub shape_parameter: f64,
    pub max_value: Option<Duration>,
}

impl HeavyTailedDistribution {
    pub fn new(
        name: String,
        min_value: Duration,
        shape_parameter: f64,
        max_value: Option<Duration>,
    ) -> Self {
        Self {
            name,
            min_value,
            shape_parameter,
            max_value,
        }
    }

    pub fn sample(&self, rng: &mut impl Rng) -> Duration {
        // Simple power-law distribution: P(X > x) = (x_min/x)^α
        let u: f64 = rng.gen();
        let min_ms = self.min_value.as_millis() as f64;

        // Inverse transform sampling for Pareto distribution
        let value_ms = min_ms * (1.0 - u).powf(-1.0 / self.shape_parameter);

        let mut result = Duration::from_millis(value_ms as u64);

        // Apply maximum if specified
        if let Some(max_val) = self.max_value {
            if result > max_val {
                result = max_val;
            }
        }

        result
    }
}

#[derive(Debug, Clone)]
pub struct DistributionTester {
    pub samples: Vec<Duration>,
    pub distribution_name: String,
}

impl DistributionTester {
    pub fn new(distribution_name: String) -> Self {
        Self {
            samples: Vec::new(),
            distribution_name,
        }
    }

    pub fn add_sample(&mut self, sample: Duration) {
        self.samples.push(sample);
    }

    pub fn analyze(&self) -> DistributionAnalysis {
        if self.samples.is_empty() {
            return DistributionAnalysis::default();
        }

        let mut sorted_samples = self.samples.clone();
        sorted_samples.sort();

        let mean = self.calculate_mean();
        let variance = self.calculate_variance(mean);
        let std_dev = variance.sqrt();

        DistributionAnalysis {
            distribution_name: self.distribution_name.clone(),
            sample_count: self.samples.len(),
            mean_ms: mean,
            std_dev_ms: std_dev,
            min_ms: sorted_samples.first().unwrap().as_millis() as f64,
            max_ms: sorted_samples.last().unwrap().as_millis() as f64,
            p50_ms: self.percentile(&sorted_samples, 50.0),
            p95_ms: self.percentile(&sorted_samples, 95.0),
            p99_ms: self.percentile(&sorted_samples, 99.0),
            coefficient_of_variation: std_dev / mean,
        }
    }

    fn calculate_mean(&self) -> f64 {
        let sum: u128 = self.samples.iter().map(|d| d.as_millis()).sum();
        sum as f64 / self.samples.len() as f64
    }

    fn calculate_variance(&self, mean: f64) -> f64 {
        let sum_squared_diff: f64 = self
            .samples
            .iter()
            .map(|d| {
                let diff = d.as_millis() as f64 - mean;
                diff * diff
            })
            .sum();

        sum_squared_diff / self.samples.len() as f64
    }

    fn percentile(&self, sorted_samples: &[Duration], p: f64) -> f64 {
        let index = ((p / 100.0) * (sorted_samples.len() - 1) as f64).round() as usize;
        sorted_samples[index].as_millis() as f64
    }
}

#[derive(Debug, Default, Clone)]
pub struct DistributionAnalysis {
    pub distribution_name: String,
    pub sample_count: usize,
    pub mean_ms: f64,
    pub std_dev_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub coefficient_of_variation: f64,
}

impl DistributionAnalysis {
    pub fn print_summary(&self) {
        println!(
            "\n=== Distribution Analysis: {} ===",
            self.distribution_name
        );
        println!("Sample count: {}", self.sample_count);
        println!("Mean: {:.2}ms", self.mean_ms);
        println!("Std Dev: {:.2}ms", self.std_dev_ms);
        println!("CV: {:.3}", self.coefficient_of_variation);
        println!("Range: {:.2}ms - {:.2}ms", self.min_ms, self.max_ms);
        println!("Percentiles:");
        println!("  P50: {:.2}ms", self.p50_ms);
        println!("  P95: {:.2}ms", self.p95_ms);
        println!("  P99: {:.2}ms", self.p99_ms);

        // Interpretation
        if self.coefficient_of_variation > 1.0 {
            println!("📊 High variability distribution (CV > 1.0)");
        } else if self.coefficient_of_variation > 0.5 {
            println!("📊 Moderate variability distribution");
        } else {
            println!("📊 Low variability distribution");
        }

        if self.p99_ms > self.mean_ms * 10.0 {
            println!("⚠️  Heavy-tailed behavior detected (P99 >> mean)");
        }
    }
}

#[test]
fn test_heavy_tailed_distribution() {
    println!("🚀 Testing Heavy-Tailed Distribution");
    println!("====================================");

    // Create a heavy-tailed distribution
    let heavy_tailed_dist = HeavyTailedDistribution::new(
        "heavy-tailed-service".to_string(),
        Duration::from_millis(50),         // Minimum 50ms
        1.5,                               // Shape parameter (α = 1.5)
        Some(Duration::from_millis(5000)), // Max 5 seconds
    );

    println!("📋 Test Configuration:");
    println!("   - Minimum value: 50ms");
    println!("   - Shape parameter: 1.5 (heavy-tailed)");
    println!("   - Maximum value: 5000ms");
    println!();

    // Sample from the distribution
    let mut rng = rand::thread_rng();
    let mut samples = Vec::new();
    let sample_size = 1000;

    println!(
        "🎲 Sampling {} times from heavy-tailed distribution:",
        sample_size
    );

    for _ in 0..sample_size {
        let sample = heavy_tailed_dist.sample(&mut rng);
        samples.push(sample);
    }

    // Analyze the samples
    let mut tester = DistributionTester::new("heavy-tailed-test".to_string());
    for sample in &samples {
        tester.add_sample(*sample);
    }

    let analysis = tester.analyze();
    analysis.print_summary();

    // Count extreme events (> 1 second)
    let extreme_threshold = Duration::from_millis(1000);
    let extreme_count = samples.iter().filter(|&&s| s > extreme_threshold).count();
    let extreme_ratio = extreme_count as f64 / sample_size as f64;

    println!();
    println!("📊 Heavy-Tail Analysis:");
    println!(
        "   Extreme events (> 1s): {} ({:.1}%)",
        extreme_count,
        extreme_ratio * 100.0
    );
    println!(
        "   P99/Mean ratio: {:.2}",
        analysis.p99_ms / analysis.mean_ms
    );

    // Verify heavy-tailed characteristics
    assert!(
        analysis.coefficient_of_variation > 1.0,
        "CV should be > 1.0 for heavy-tailed"
    );
    assert!(
        analysis.p99_ms > analysis.mean_ms * 5.0,
        "P99 should be >> mean for heavy-tailed"
    );
    assert!(
        extreme_ratio >= 0.005,
        "Should have some extreme events (>= 0.5%)"
    );

    println!("   ✅ Heavy-tailed characteristics confirmed");
    println!(
        "   ✅ High coefficient of variation: {:.2}",
        analysis.coefficient_of_variation
    );
    println!(
        "   ✅ P99/Mean ratio: {:.2}",
        analysis.p99_ms / analysis.mean_ms
    );

    println!("✅ Heavy-tailed distribution test completed successfully!");
}
#[test]
fn test_advanced_patterns_integration() {
    println!("🚀 Testing Advanced Patterns Integration");
    println!("=======================================");

    println!("📋 Testing integration of multiple advanced patterns:");
    println!("   - Payload-dependent service times");
    println!("   - Client-side admission control (token bucket simulation)");
    println!("   - Server-side throttling (rate limiting simulation)");
    println!("   - Workload patterns (seasonal, bursty)");
    println!("   - Custom distributions (empirical, mixture, heavy-tailed)");
    println!();

    // Simulate a complete system with multiple patterns
    let mut system_metrics = HashMap::new();

    // 1. Payload-dependent processing
    println!("🔄 Simulating payload-dependent processing:");
    let payload_sizes = vec![100, 1000, 5000, 10000];
    for size in payload_sizes {
        let base_time = 10;
        let processing_time = base_time + (size / 100); // 1ms per 100 bytes
        system_metrics.insert(format!("payload_{}", size), processing_time);
        println!("   Payload {}B -> {}ms", size, processing_time);
    }

    // 2. Token bucket admission control
    println!();
    println!("🪣 Simulating token bucket admission control:");
    let mut tokens = 10.0; // Start with 10 tokens
    let _refill_rate = 5.0; // 5 tokens per second
    let requests = vec![1, 1, 1, 1, 1, 1, 1, 1]; // 8 requests

    let mut accepted = 0;
    let mut rejected = 0;

    for (i, _request) in requests.iter().enumerate() {
        if tokens >= 1.0 {
            tokens -= 1.0;
            accepted += 1;
            println!("   Request {} accepted (tokens: {:.1})", i + 1, tokens);
        } else {
            rejected += 1;
            println!("   Request {} rejected (tokens: {:.1})", i + 1, tokens);
        }

        // Simulate time passing and token refill
        if i == 4 {
            tokens = f64::min(tokens + 2.0, 10.0); // Refill some tokens
            println!("   Token refill: {:.1} tokens", tokens);
        }
    }

    system_metrics.insert("requests_accepted".to_string(), accepted);
    system_metrics.insert("requests_rejected".to_string(), rejected);

    // 3. Seasonal workload simulation
    println!();
    println!("🌊 Simulating seasonal workload patterns:");
    let hours = vec![0, 6, 12, 18]; // Different hours of day
    let base_rate = 10.0; // 10 RPS base

    for hour in hours {
        // Simple sinusoidal pattern
        let seasonal_factor = 1.0 + 0.5 * ((hour as f64 * std::f64::consts::PI / 12.0).sin());
        let current_rate = base_rate * seasonal_factor;
        system_metrics.insert(format!("rate_hour_{}", hour), current_rate as i32);
        println!("   Hour {}: {:.1} RPS", hour, current_rate);
    }

    // 4. Custom distribution sampling
    println!();
    println!("🎲 Sampling from custom distributions:");

    // Mixture distribution
    let mixture = MixtureDistribution::new("test-mixture".to_string())
        .add_component(0.7, Duration::from_millis(50))
        .add_component(0.3, Duration::from_millis(200));

    let mut rng = rand::thread_rng();
    let mut fast_samples = 0;
    let mut slow_samples = 0;

    for i in 0..20 {
        let sample = mixture.sample(&mut rng);
        if sample.as_millis() < 100 {
            fast_samples += 1;
        } else {
            slow_samples += 1;
        }

        if i < 5 {
            println!("   Sample {}: {}ms", i + 1, sample.as_millis());
        }
    }

    system_metrics.insert("fast_samples".to_string(), fast_samples);
    system_metrics.insert("slow_samples".to_string(), slow_samples);

    // 5. System analysis
    println!();
    println!("📊 Integrated System Analysis:");
    println!("   Payload processing: Variable (10-110ms based on size)");
    println!(
        "   Admission control: {}/{} requests accepted",
        system_metrics.get("requests_accepted").unwrap_or(&0),
        system_metrics.get("requests_accepted").unwrap_or(&0)
            + system_metrics.get("requests_rejected").unwrap_or(&0)
    );
    println!("   Seasonal variation: 5.0-15.0 RPS range");
    println!(
        "   Distribution mix: {}/{} fast/slow samples",
        fast_samples, slow_samples
    );

    // Verify integration
    assert!(
        system_metrics.len() > 10,
        "Should have collected multiple metrics"
    );
    assert!(
        *system_metrics.get("requests_accepted").unwrap_or(&0) > 0,
        "Should accept some requests"
    );
    assert!(
        fast_samples + slow_samples == 20,
        "Should have all distribution samples"
    );

    println!();
    println!("✅ Integration test completed successfully!");
    println!("✅ All patterns working together as expected!");
}
// ============================================================================
// CLIENT-SIDE ADMISSION CONTROL STRATEGIES
// ============================================================================

#[derive(Debug, Clone)]
pub struct TokenBucket {
    pub name: String,
    pub capacity: f64,
    pub current_tokens: f64,
    pub refill_rate: f64,      // tokens per second
    pub last_refill_time: f64, // simulation time in seconds
}

impl TokenBucket {
    pub fn new(name: String, capacity: f64, refill_rate: f64) -> Self {
        Self {
            name,
            capacity,
            current_tokens: capacity, // Start full
            refill_rate,
            last_refill_time: 0.0,
        }
    }

    pub fn try_consume(&mut self, tokens: f64, current_time: f64) -> bool {
        self.refill(current_time);

        if self.current_tokens >= tokens {
            self.current_tokens -= tokens;
            true
        } else {
            false
        }
    }

    pub fn refill(&mut self, current_time: f64) {
        let time_elapsed = current_time - self.last_refill_time;
        let tokens_to_add = time_elapsed * self.refill_rate;
        self.current_tokens = f64::min(self.capacity, self.current_tokens + tokens_to_add);
        self.last_refill_time = current_time;
    }

    pub fn get_tokens(&self) -> f64 {
        self.current_tokens
    }
}

#[derive(Debug, Clone)]
pub struct SuccessBasedTokenBucket {
    pub name: String,
    pub capacity: f64,
    pub current_tokens: f64,
    pub base_refill_rate: f64,
    pub success_multiplier: f64,
    pub failure_penalty: f64,
    pub recent_success_rate: f64,
    pub success_window: Vec<bool>, // Rolling window of recent results
    pub window_size: usize,
}

impl SuccessBasedTokenBucket {
    pub fn new(name: String, capacity: f64, base_refill_rate: f64, window_size: usize) -> Self {
        Self {
            name,
            capacity,
            current_tokens: capacity,
            base_refill_rate,
            success_multiplier: 2.0, // Double refill rate on success
            failure_penalty: 0.5,    // Half refill rate on failure
            recent_success_rate: 1.0,
            success_window: Vec::new(),
            window_size,
        }
    }

    pub fn try_consume(&mut self, tokens: f64) -> bool {
        if self.current_tokens >= tokens {
            self.current_tokens -= tokens;
            true
        } else {
            false
        }
    }

    pub fn record_result(&mut self, success: bool, time_elapsed: f64) {
        // Update success window
        self.success_window.push(success);
        if self.success_window.len() > self.window_size {
            self.success_window.remove(0);
        }

        // Calculate success rate
        if !self.success_window.is_empty() {
            let successes = self.success_window.iter().filter(|&&s| s).count();
            self.recent_success_rate = successes as f64 / self.success_window.len() as f64;
        }

        // Adaptive refill based on success rate
        let adaptive_rate = if success {
            self.base_refill_rate * self.success_multiplier * self.recent_success_rate
        } else {
            self.base_refill_rate * self.failure_penalty
        };

        let tokens_to_add = time_elapsed * adaptive_rate;
        self.current_tokens = f64::min(self.capacity, self.current_tokens + tokens_to_add);
    }

    pub fn get_success_rate(&self) -> f64 {
        self.recent_success_rate
    }
}

#[derive(Debug, Clone)]
pub struct TimeBasedTokenBucket {
    pub name: String,
    pub capacity: f64,
    pub current_tokens: f64,
    pub base_refill_rate: f64,
    pub time_multipliers: Vec<(f64, f64)>, // (hour, multiplier) pairs
    pub last_refill_time: f64,
}

impl TimeBasedTokenBucket {
    pub fn new(name: String, capacity: f64, base_refill_rate: f64) -> Self {
        // Default time-based multipliers (simulating daily patterns)
        let time_multipliers = vec![
            (0.0, 0.5),  // Midnight - low traffic
            (6.0, 1.0),  // Morning - normal traffic
            (12.0, 1.5), // Noon - high traffic
            (18.0, 1.2), // Evening - moderate traffic
            (22.0, 0.8), // Night - reduced traffic
        ];

        Self {
            name,
            capacity,
            current_tokens: capacity,
            base_refill_rate,
            time_multipliers,
            last_refill_time: 0.0,
        }
    }

    pub fn try_consume(&mut self, tokens: f64, current_time: f64) -> bool {
        self.refill(current_time);

        if self.current_tokens >= tokens {
            self.current_tokens -= tokens;
            true
        } else {
            false
        }
    }

    pub fn refill(&mut self, current_time: f64) {
        let time_elapsed = current_time - self.last_refill_time;
        let hour_of_day = (current_time / 3600.0) % 24.0; // Convert to hour of day

        // Find appropriate time multiplier
        let mut multiplier = 1.0;
        for (hour, mult) in &self.time_multipliers {
            if hour_of_day >= *hour {
                multiplier = *mult;
            }
        }

        let adaptive_rate = self.base_refill_rate * multiplier;
        let tokens_to_add = time_elapsed * adaptive_rate;
        self.current_tokens = f64::min(self.capacity, self.current_tokens + tokens_to_add);
        self.last_refill_time = current_time;
    }

    pub fn get_current_multiplier(&self, current_time: f64) -> f64 {
        let hour_of_day = (current_time / 3600.0) % 24.0;
        let mut multiplier = 1.0;
        for (hour, mult) in &self.time_multipliers {
            if hour_of_day >= *hour {
                multiplier = *mult;
            }
        }
        multiplier
    }

    pub fn get_tokens(&self) -> f64 {
        self.current_tokens
    }
}

#[test]
fn test_client_token_bucket_admission_control() {
    println!("🚀 Testing Client-Side Token Bucket Admission Control");
    println!("=====================================================");

    let mut token_bucket = TokenBucket::new(
        "client-rate-limiter".to_string(),
        10.0, // 10 token capacity
        2.0,  // 2 tokens per second refill rate
    );

    println!("📋 Test Configuration:");
    println!("   - Bucket capacity: 10 tokens");
    println!("   - Refill rate: 2 tokens/second");
    println!("   - Request cost: 1 token each");
    println!();

    let mut accepted = 0;
    let mut rejected = 0;
    let mut current_time = 0.0;

    println!("🎯 Simulating burst of 15 requests:");

    // Simulate burst of requests
    for i in 1..=15 {
        if token_bucket.try_consume(1.0, current_time) {
            accepted += 1;
            println!(
                "   Request {}: ACCEPTED (tokens: {:.1})",
                i,
                token_bucket.get_tokens()
            );
        } else {
            rejected += 1;
            println!(
                "   Request {}: REJECTED (tokens: {:.1})",
                i,
                token_bucket.get_tokens()
            );
        }
        current_time += 0.1; // 100ms between requests
    }

    println!();
    println!("⏰ Waiting 3 seconds for token refill...");
    current_time += 3.0;
    token_bucket.refill(current_time);
    println!("   Tokens after refill: {:.1}", token_bucket.get_tokens());

    println!();
    println!("🎯 Simulating 5 more requests after refill:");
    for i in 16..=20 {
        if token_bucket.try_consume(1.0, current_time) {
            accepted += 1;
            println!(
                "   Request {}: ACCEPTED (tokens: {:.1})",
                i,
                token_bucket.get_tokens()
            );
        } else {
            rejected += 1;
            println!(
                "   Request {}: REJECTED (tokens: {:.1})",
                i,
                token_bucket.get_tokens()
            );
        }
        current_time += 0.1;
    }

    println!();
    println!("📊 Final Results:");
    println!("   Total requests: 20");
    println!("   Accepted: {}", accepted);
    println!("   Rejected: {}", rejected);
    println!(
        "   Acceptance rate: {:.1}%",
        (accepted as f64 / 20.0) * 100.0
    );

    // Verify token bucket behavior
    assert!(
        accepted >= 10,
        "Should accept at least initial bucket capacity"
    );
    assert!(rejected > 0, "Should reject some requests during burst");
    assert!(accepted > 10, "Should accept more requests after refill");

    println!("✅ Token bucket admission control test completed successfully!");
}

#[test]
fn test_client_success_based_admission_control() {
    println!("🚀 Testing Client-Side Success-Based Admission Control");
    println!("======================================================");

    let mut success_bucket = SuccessBasedTokenBucket::new(
        "adaptive-client".to_string(),
        10.0, // 10 token capacity
        1.0,  // 1 token per second base rate
        5,    // 5-request success window
    );

    println!("📋 Test Configuration:");
    println!("   - Bucket capacity: 10 tokens");
    println!("   - Base refill rate: 1 token/second");
    println!("   - Success window: 5 requests");
    println!("   - Success multiplier: 2.0x");
    println!("   - Failure penalty: 0.5x");
    println!();

    let mut scenario_results = Vec::new();

    // Scenario 1: High success rate
    println!("🎯 Scenario 1: High success rate (80% success)");
    for i in 1..=10 {
        if success_bucket.try_consume(1.0) {
            let success = i % 5 != 0; // 80% success rate
            success_bucket.record_result(success, 1.0); // 1 second elapsed
            scenario_results.push((i, success, success_bucket.get_success_rate()));
            println!(
                "   Request {}: {} (success rate: {:.1}%, tokens: {:.1})",
                i,
                if success { "SUCCESS" } else { "FAILURE" },
                success_bucket.get_success_rate() * 100.0,
                success_bucket.current_tokens
            );
        }
    }

    println!();
    println!("🎯 Scenario 2: Low success rate (20% success)");
    for i in 11..=20 {
        if success_bucket.try_consume(1.0) {
            let success = i % 5 == 0; // 20% success rate
            success_bucket.record_result(success, 1.0);
            scenario_results.push((i, success, success_bucket.get_success_rate()));
            println!(
                "   Request {}: {} (success rate: {:.1}%, tokens: {:.1})",
                i,
                if success { "SUCCESS" } else { "FAILURE" },
                success_bucket.get_success_rate() * 100.0,
                success_bucket.current_tokens
            );
        }
    }

    println!();
    println!("📊 Adaptive Behavior Analysis:");
    let high_success_rate = scenario_results
        .iter()
        .take(10)
        .map(|(_, _, rate)| rate)
        .last()
        .unwrap_or(&0.0);
    let low_success_rate = scenario_results
        .iter()
        .skip(10)
        .map(|(_, _, rate)| rate)
        .last()
        .unwrap_or(&0.0);

    println!(
        "   High success scenario final rate: {:.1}%",
        high_success_rate * 100.0
    );
    println!(
        "   Low success scenario final rate: {:.1}%",
        low_success_rate * 100.0
    );
    println!("   ✅ Bucket adapts refill rate based on success patterns");

    assert!(
        *high_success_rate > 0.6,
        "High success scenario should maintain good rate"
    );
    assert!(
        *low_success_rate < 0.4,
        "Low success scenario should show degraded rate"
    );

    println!("✅ Success-based admission control test completed successfully!");
}

#[test]
fn test_client_time_based_admission_control() {
    println!("🚀 Testing Client-Side Time-Based Admission Control");
    println!("===================================================");

    let mut time_bucket = TimeBasedTokenBucket::new(
        "time-aware-client".to_string(),
        20.0, // 20 token capacity
        2.0,  // 2 tokens per second base rate
    );

    println!("📋 Test Configuration:");
    println!("   - Bucket capacity: 20 tokens");
    println!("   - Base refill rate: 2 tokens/second");
    println!("   - Time multipliers:");
    println!("     * Midnight (0h): 0.5x (low traffic)");
    println!("     * Morning (6h): 1.0x (normal traffic)");
    println!("     * Noon (12h): 1.5x (high traffic)");
    println!("     * Evening (18h): 1.2x (moderate traffic)");
    println!("     * Night (22h): 0.8x (reduced traffic)");
    println!();

    let test_times = vec![
        (0.0 * 3600.0, "Midnight"), // 0:00
        (6.0 * 3600.0, "Morning"),  // 6:00
        (12.0 * 3600.0, "Noon"),    // 12:00
        (18.0 * 3600.0, "Evening"), // 18:00
        (22.0 * 3600.0, "Night"),   // 22:00
    ];

    for (time, period) in test_times {
        println!("🕐 Testing {} period ({}h):", period, time / 3600.0);

        // Reset bucket for each test
        time_bucket.current_tokens = 5.0; // Start with some tokens
        time_bucket.last_refill_time = time;

        let multiplier = time_bucket.get_current_multiplier(time);
        println!("   Time multiplier: {:.1}x", multiplier);

        // Simulate 5 seconds of refill
        let refill_time = time + 5.0;
        time_bucket.refill(refill_time);

        println!("   Tokens after 5s refill: {:.1}", time_bucket.get_tokens());

        // Test request acceptance
        let mut accepted = 0;
        for i in 1..=10 {
            if time_bucket.try_consume(1.0, refill_time + i as f64 * 0.1) {
                accepted += 1;
            }
        }

        println!("   Requests accepted (out of 10): {}", accepted);
        println!();
    }

    println!("📊 Time-Based Behavior Verification:");

    // Test that noon has higher capacity than midnight
    let mut midnight_bucket = time_bucket.clone();
    midnight_bucket.current_tokens = 0.0;
    midnight_bucket.last_refill_time = 0.0;
    midnight_bucket.refill(10.0); // 10 seconds of refill

    let mut noon_bucket = time_bucket.clone();
    noon_bucket.current_tokens = 0.0;
    noon_bucket.last_refill_time = 12.0 * 3600.0;
    noon_bucket.refill(12.0 * 3600.0 + 10.0); // 10 seconds of refill

    println!(
        "   Midnight refill (10s): {:.1} tokens",
        midnight_bucket.get_tokens()
    );
    println!(
        "   Noon refill (10s): {:.1} tokens",
        noon_bucket.get_tokens()
    );

    assert!(
        noon_bucket.get_tokens() > midnight_bucket.get_tokens(),
        "Noon should have higher refill rate than midnight"
    );

    println!("   ✅ Time-based multipliers working correctly");
    println!("✅ Time-based admission control test completed successfully!");
}
// ============================================================================
// SERVER-SIDE THROTTLING AND ADMISSION CONTROL
// ============================================================================

#[derive(Debug, Clone)]
pub struct ServerTokenBucket {
    pub name: String,
    pub capacity: f64,
    pub current_tokens: f64,
    pub refill_rate: f64,
    pub last_refill_time: f64,
    pub requests_processed: u64,
    pub requests_rejected: u64,
}

impl ServerTokenBucket {
    pub fn new(name: String, capacity: f64, refill_rate: f64) -> Self {
        Self {
            name,
            capacity,
            current_tokens: capacity,
            refill_rate,
            last_refill_time: 0.0,
            requests_processed: 0,
            requests_rejected: 0,
        }
    }

    pub fn try_admit_request(&mut self, current_time: f64) -> bool {
        self.refill(current_time);

        if self.current_tokens >= 1.0 {
            self.current_tokens -= 1.0;
            self.requests_processed += 1;
            true
        } else {
            self.requests_rejected += 1;
            false
        }
    }

    pub fn refill(&mut self, current_time: f64) {
        let time_elapsed = current_time - self.last_refill_time;
        let tokens_to_add = time_elapsed * self.refill_rate;
        self.current_tokens = f64::min(self.capacity, self.current_tokens + tokens_to_add);
        self.last_refill_time = current_time;
    }

    pub fn get_stats(&self) -> (u64, u64, f64) {
        let total = self.requests_processed + self.requests_rejected;
        let acceptance_rate = if total > 0 {
            self.requests_processed as f64 / total as f64
        } else {
            0.0
        };
        (
            self.requests_processed,
            self.requests_rejected,
            acceptance_rate,
        )
    }
}

#[derive(Debug, Clone)]
pub struct RedAdmissionControl {
    pub name: String,
    pub min_threshold: f64,
    pub max_threshold: f64,
    pub current_queue_size: f64,
    pub max_drop_probability: f64,
    pub requests_processed: u64,
    pub requests_rejected: u64,
}

impl RedAdmissionControl {
    pub fn new(
        name: String,
        min_threshold: f64,
        max_threshold: f64,
        max_drop_probability: f64,
    ) -> Self {
        Self {
            name,
            min_threshold,
            max_threshold,
            current_queue_size: 0.0,
            max_drop_probability,
            requests_processed: 0,
            requests_rejected: 0,
        }
    }

    pub fn try_admit_request(&mut self, rng: &mut impl Rng) -> bool {
        let drop_probability = self.calculate_drop_probability();
        let random_value: f64 = rng.gen();

        if random_value < drop_probability {
            self.requests_rejected += 1;
            false
        } else {
            self.requests_processed += 1;
            self.current_queue_size += 1.0; // Simulate adding to queue
            true
        }
    }

    pub fn complete_request(&mut self) {
        if self.current_queue_size > 0.0 {
            self.current_queue_size -= 1.0;
        }
    }

    fn calculate_drop_probability(&self) -> f64 {
        if self.current_queue_size <= self.min_threshold {
            0.0 // No dropping below min threshold
        } else if self.current_queue_size >= self.max_threshold {
            self.max_drop_probability // Maximum dropping above max threshold
        } else {
            // Linear interpolation between min and max thresholds
            let range = self.max_threshold - self.min_threshold;
            let position = self.current_queue_size - self.min_threshold;
            (position / range) * self.max_drop_probability
        }
    }

    pub fn get_stats(&self) -> (u64, u64, f64, f64) {
        let total = self.requests_processed + self.requests_rejected;
        let acceptance_rate = if total > 0 {
            self.requests_processed as f64 / total as f64
        } else {
            0.0
        };
        (
            self.requests_processed,
            self.requests_rejected,
            acceptance_rate,
            self.current_queue_size,
        )
    }

    pub fn get_current_drop_probability(&self) -> f64 {
        self.calculate_drop_probability()
    }
}

#[derive(Debug, Clone)]
pub struct QueueSizeAdmissionControl {
    pub name: String,
    pub max_queue_size: usize,
    pub current_queue_size: usize,
    pub soft_limit: usize,
    pub priority_levels: Vec<f64>, // Drop probabilities for different priority levels
    pub requests_processed: u64,
    pub requests_rejected: u64,
}

impl QueueSizeAdmissionControl {
    pub fn new(name: String, max_queue_size: usize, soft_limit: usize) -> Self {
        Self {
            name,
            max_queue_size,
            current_queue_size: 0,
            soft_limit,
            priority_levels: vec![0.1, 0.3, 0.6, 0.9], // Drop probabilities for priorities 0-3
            requests_processed: 0,
            requests_rejected: 0,
        }
    }

    pub fn try_admit_request(&mut self, priority: usize, rng: &mut impl Rng) -> bool {
        // Hard limit - reject all requests
        if self.current_queue_size >= self.max_queue_size {
            self.requests_rejected += 1;
            return false;
        }

        // Below soft limit - accept all requests
        if self.current_queue_size < self.soft_limit {
            self.current_queue_size += 1;
            self.requests_processed += 1;
            return true;
        }

        // Between soft and hard limit - probabilistic dropping based on priority
        let drop_probability = if priority < self.priority_levels.len() {
            self.priority_levels[priority]
        } else {
            0.9 // High drop probability for unknown priorities
        };

        // Scale drop probability based on how close we are to hard limit
        let queue_pressure = (self.current_queue_size - self.soft_limit) as f64
            / (self.max_queue_size - self.soft_limit) as f64;
        let adjusted_drop_probability = drop_probability * queue_pressure;

        let random_value: f64 = rng.gen();
        if random_value < adjusted_drop_probability {
            self.requests_rejected += 1;
            false
        } else {
            self.current_queue_size += 1;
            self.requests_processed += 1;
            true
        }
    }

    pub fn complete_request(&mut self) {
        if self.current_queue_size > 0 {
            self.current_queue_size -= 1;
        }
    }

    pub fn get_stats(&self) -> (u64, u64, f64, usize) {
        let total = self.requests_processed + self.requests_rejected;
        let acceptance_rate = if total > 0 {
            self.requests_processed as f64 / total as f64
        } else {
            0.0
        };
        (
            self.requests_processed,
            self.requests_rejected,
            acceptance_rate,
            self.current_queue_size,
        )
    }
}

#[test]
fn test_server_token_bucket_throttling() {
    println!("🚀 Testing Server-Side Token Bucket Throttling");
    println!("===============================================");

    let mut server_bucket = ServerTokenBucket::new(
        "api-server".to_string(),
        50.0, // 50 request capacity
        10.0, // 10 requests per second
    );

    println!("📋 Test Configuration:");
    println!("   - Server capacity: 50 requests");
    println!("   - Refill rate: 10 requests/second");
    println!("   - Test scenario: Burst followed by sustained load");
    println!();

    let mut current_time = 0.0;

    // Phase 1: Initial burst (should consume most tokens)
    println!("🚀 Phase 1: Initial burst (100 requests in 1 second)");
    for i in 1..=100 {
        let admitted = server_bucket.try_admit_request(current_time);
        if i <= 10 || i % 10 == 0 {
            println!(
                "   Request {}: {} (tokens: {:.1})",
                i,
                if admitted { "ADMITTED" } else { "REJECTED" },
                server_bucket.current_tokens
            );
        }
        current_time += 0.01; // 10ms between requests
    }

    let (processed1, rejected1, rate1) = server_bucket.get_stats();
    println!(
        "   Phase 1 results: {}/{} admitted ({:.1}%)",
        processed1,
        processed1 + rejected1,
        rate1 * 100.0
    );

    // Phase 2: Wait for refill
    println!();
    println!("⏰ Phase 2: Waiting 3 seconds for token refill...");
    current_time += 3.0;
    server_bucket.refill(current_time);
    println!(
        "   Tokens after refill: {:.1}",
        server_bucket.current_tokens
    );

    // Phase 3: Sustained load at exactly the refill rate
    println!();
    println!("🔄 Phase 3: Sustained load (10 requests/second for 5 seconds)");
    for i in 1..=50 {
        let admitted = server_bucket.try_admit_request(current_time);
        if i <= 5 || i % 10 == 0 {
            println!(
                "   Request {}: {} (tokens: {:.1})",
                i,
                if admitted { "ADMITTED" } else { "REJECTED" },
                server_bucket.current_tokens
            );
        }
        current_time += 0.1; // 100ms between requests (10 RPS)
    }

    let (processed_total, rejected_total, rate_total) = server_bucket.get_stats();
    println!();
    println!("📊 Final Server Statistics:");
    println!("   Total requests: {}", processed_total + rejected_total);
    println!("   Admitted: {}", processed_total);
    println!("   Rejected: {}", rejected_total);
    println!("   Overall acceptance rate: {:.1}%", rate_total * 100.0);

    // Verify throttling behavior
    assert!(
        rejected_total > 0,
        "Server should reject some requests during burst"
    );
    assert!(
        rate_total > 0.5,
        "Server should maintain reasonable acceptance rate"
    );
    assert!(
        processed_total >= 50,
        "Server should process at least bucket capacity"
    );

    println!("✅ Server token bucket throttling test completed successfully!");
}

#[test]
fn test_server_red_admission_control() {
    println!("🚀 Testing Server-Side RED Admission Control");
    println!("============================================");

    let mut red_controller = RedAdmissionControl::new(
        "load-balancer".to_string(),
        10.0, // Min threshold (no dropping below this)
        30.0, // Max threshold (maximum dropping above this)
        0.8,  // 80% max drop probability
    );

    println!("📋 Test Configuration:");
    println!("   - Min threshold: 10 (no dropping)");
    println!("   - Max threshold: 30 (max dropping)");
    println!("   - Max drop probability: 80%");
    println!();

    let mut rng = rand::thread_rng();
    let mut phase_results = Vec::new();

    // Test different queue load levels
    let test_phases = vec![
        (5.0, "Low load (below min threshold)"),
        (20.0, "Medium load (between thresholds)"),
        (35.0, "High load (above max threshold)"),
        (15.0, "Recovery (back to medium)"),
    ];

    for (target_queue_size, phase_name) in test_phases {
        println!("📊 Testing {}", phase_name);

        // Reset and adjust queue size to target
        red_controller.current_queue_size = target_queue_size;
        red_controller.requests_processed = 0;
        red_controller.requests_rejected = 0;

        let mut phase_admitted = 0;
        let mut phase_rejected = 0;

        // Send 100 requests at this load level
        for i in 1..=100 {
            let admitted = red_controller.try_admit_request(&mut rng);
            if admitted {
                phase_admitted += 1;
                // Simulate request completion to maintain target queue size
                if red_controller.current_queue_size > target_queue_size {
                    red_controller.complete_request();
                }
            } else {
                phase_rejected += 1;
            }

            // Periodically complete some requests to maintain queue size around target
            if i % 5 == 0 && red_controller.current_queue_size > target_queue_size {
                red_controller.complete_request();
            }
        }

        let drop_prob = red_controller.get_current_drop_probability();
        let phase_rate = phase_admitted as f64 / (phase_admitted + phase_rejected) as f64;

        println!("   Queue size: {:.1}", red_controller.current_queue_size);
        println!("   Drop probability: {:.1}%", drop_prob * 100.0);
        println!(
            "   Admission rate: {:.1}% ({}/{})",
            phase_rate * 100.0,
            phase_admitted,
            phase_admitted + phase_rejected
        );

        phase_results.push((target_queue_size, drop_prob, phase_rate));
        println!();
    }

    println!("📈 RED Behavior Analysis:");
    for (i, (queue_size, drop_prob, admission_rate)) in phase_results.iter().enumerate() {
        println!(
            "   Phase {}: Queue={:.1}, Drop={:.1}%, Admit={:.1}%",
            i + 1,
            queue_size,
            drop_prob * 100.0,
            admission_rate * 100.0
        );
    }

    // Verify RED behavior
    assert!(
        phase_results[0].1 < 0.1,
        "Low load should have minimal dropping"
    );
    assert!(
        phase_results[1].1 > phase_results[0].1,
        "Medium load should drop more than low load"
    );
    assert!(
        phase_results[2].1 > phase_results[1].1,
        "High load should drop more than medium load"
    );
    assert!(
        phase_results[2].2 < phase_results[0].2,
        "High load should admit fewer requests"
    );

    let (processed, rejected, final_rate, queue_size) = red_controller.get_stats();
    println!();
    println!("📊 Final RED Statistics:");
    println!("   Total processed: {}", processed);
    println!("   Total rejected: {}", rejected);
    println!("   Final admission rate: {:.1}%", final_rate * 100.0);
    println!("   Final queue size: {:.1}", queue_size);

    println!("✅ RED admission control test completed successfully!");
}

#[test]
fn test_server_queue_size_admission_control() {
    println!("🚀 Testing Server-Side Queue Size Admission Control");
    println!("===================================================");

    let mut queue_controller = QueueSizeAdmissionControl::new(
        "priority-server".to_string(),
        100, // Max queue size
        60,  // Soft limit
    );

    println!("📋 Test Configuration:");
    println!("   - Max queue size: 100");
    println!("   - Soft limit: 60");
    println!("   - Priority levels: 0 (high) to 3 (low)");
    println!("   - Drop probabilities: [10%, 30%, 60%, 90%]");
    println!();

    let mut rng = rand::thread_rng();

    // Test different scenarios
    let scenarios = vec![
        (40, "Below soft limit"),
        (80, "Above soft limit"),
        (95, "Near hard limit"),
    ];

    for (target_queue, scenario_name) in scenarios {
        println!(
            "🎯 Scenario: {} (target queue: {})",
            scenario_name, target_queue
        );

        // Adjust queue to target size
        queue_controller.current_queue_size = target_queue;

        let mut priority_stats = vec![(0, 0); 4]; // (admitted, total) for each priority

        // Test each priority level
        for priority in 0..4 {
            let mut admitted = 0;
            let total_requests = 25;

            for _ in 0..total_requests {
                if queue_controller.try_admit_request(priority, &mut rng) {
                    admitted += 1;
                    // Simulate request completion to maintain queue size
                    if rng.gen::<f64>() < 0.3 {
                        queue_controller.complete_request();
                    }
                }
            }

            priority_stats[priority] = (admitted, total_requests);
            let rate = admitted as f64 / total_requests as f64;
            println!(
                "   Priority {}: {}/{} admitted ({:.1}%)",
                priority,
                admitted,
                total_requests,
                rate * 100.0
            );
        }

        // Verify priority-based admission
        let rates: Vec<f64> = priority_stats
            .iter()
            .map(|(admitted, total)| *admitted as f64 / *total as f64)
            .collect();

        if target_queue > 60 {
            // Above soft limit - higher priorities should have better rates
            for i in 1..rates.len() {
                assert!(
                    rates[i - 1] >= rates[i],
                    "Priority {} should have better or equal rate than priority {}",
                    i - 1,
                    i
                );
            }
        }

        println!(
            "   Current queue size: {}",
            queue_controller.current_queue_size
        );
        println!();
    }

    // Test hard limit enforcement
    println!("🚫 Testing hard limit enforcement:");
    queue_controller.current_queue_size = 100; // At max capacity

    let mut rejected_at_limit = 0;
    for _ in 0..20 {
        if !queue_controller.try_admit_request(0, &mut rng) {
            // Even highest priority
            rejected_at_limit += 1;
        }
    }

    println!(
        "   Requests rejected at hard limit: {}/20",
        rejected_at_limit
    );
    assert!(
        rejected_at_limit == 20,
        "All requests should be rejected at hard limit"
    );

    let (processed, rejected, final_rate, final_queue) = queue_controller.get_stats();
    println!();
    println!("📊 Final Queue Controller Statistics:");
    println!("   Total processed: {}", processed);
    println!("   Total rejected: {}", rejected);
    println!("   Final admission rate: {:.1}%", final_rate * 100.0);
    println!("   Final queue size: {}", final_queue);

    println!("✅ Queue size admission control test completed successfully!");
}
