# Custom Distribution Patterns

While DESCARTES provides many built-in distributions, real-world systems often require custom distributions that model specific behaviors or match empirical data. This section demonstrates how to implement custom service time distributions, arrival patterns, and statistical models.

## Overview

Custom distribution patterns include:

- **Empirical Distributions**: Based on real-world measurement data
- **Composite Distributions**: Combining multiple distributions
- **Conditional Distributions**: Distributions that change based on system state
- **Time-Varying Distributions**: Distributions that evolve over time
- **Multi-Modal Distributions**: Distributions with multiple peaks
- **Heavy-Tailed Distributions**: Modeling rare but extreme events

All examples in this section are implemented as runnable tests in `des-components/tests/advanced_examples.rs`.

## Empirical Distributions

Empirical distributions are based on actual measurement data from production systems, providing the most realistic modeling.

### Histogram-Based Distribution

```rust
use rand::Rng;
use std::time::Duration;

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
        
        // Find the bucket where cumulative probability >= random_value
        for (duration, cumulative_prob) in &self.buckets {
            if random_value <= *cumulative_prob {
                return *duration;
            }
        }
        
        // Fallback to last bucket
        self.buckets.last().map(|(d, _)| *d).unwrap_or(Duration::from_millis(100))
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
            return Duration::zero();
        }
        
        let total_ms: u64 = self.buckets.iter()
            .map(|(duration, _)| duration.as_millis() as u64)
            .sum();
        
        Duration::from_millis(total_ms / self.buckets.len() as u64)
    }
}
```

## Composite Distributions

Composite distributions combine multiple distributions to model complex behaviors.

### Mixture Distribution

```rust
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

/// Example: Fast/Slow request mixture
pub fn create_fast_slow_mixture() -> MixtureDistribution {
    MixtureDistribution::new("fast-slow-mixture".to_string())
        .add_component(0.8, Duration::from_millis(50))   // 80% fast
        .add_component(0.2, Duration::from_millis(500))  // 20% slow
}
```

## Heavy-Tailed Distributions

Heavy-tailed distributions model systems where rare but extreme events occur more frequently than normal distributions would predict.

### Custom Heavy-Tailed Implementation

```rust
pub struct HeavyTailedDistribution {
    pub name: String,
    pub min_value: Duration,
    pub shape_parameter: f64,
    pub max_value: Option<Duration>,
}

impl HeavyTailedDistribution {
    pub fn new(name: String, min_value: Duration, shape_parameter: f64, max_value: Option<Duration>) -> Self {
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
```

## Performance Analysis Framework

### Distribution Testing and Validation

```rust
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
        let sum_squared_diff: f64 = self.samples.iter()
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

#[derive(Debug, Default)]
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
        println!("\n=== Distribution Analysis: {} ===", self.distribution_name);
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
```

## Best Practices

### Distribution Selection Guidelines

1. **Empirical Distributions**: Use when you have production data
   - Most accurate representation of real behavior
   - Captures all nuances and correlations
   - Requires sufficient sample size

2. **Mixture Distributions**: Use for multi-modal behaviors
   - Fast/slow request patterns
   - Different request types
   - Cache hit/miss scenarios

3. **Heavy-Tailed Distributions**: Use for rare extreme events
   - Network timeouts
   - Database lock contention
   - Garbage collection pauses

### Implementation Guidelines

```rust
// Good: Parameterized distribution with validation
pub fn create_validated_distribution(
    mean_ms: f64,
    cv: f64,
    max_ms: Option<f64>,
) -> Result<MixtureDistribution, String> {
    if mean_ms <= 0.0 {
        return Err("Mean must be positive".to_string());
    }
    
    if cv < 0.0 {
        return Err("Coefficient of variation must be non-negative".to_string());
    }
    
    // Choose distribution based on CV
    if cv < 0.1 {
        // Low variability - use constant
        Ok(MixtureDistribution::new("low-variability".to_string())
            .add_component(1.0, Duration::from_millis(mean_ms as u64)))
    } else {
        // Higher variability - use mixture
        let fast_time = Duration::from_millis((mean_ms * 0.5) as u64);
        let slow_time = Duration::from_millis((mean_ms * 2.0) as u64);
        
        Ok(MixtureDistribution::new("variable".to_string())
            .add_component(0.7, fast_time)
            .add_component(0.3, slow_time))
    }
}
```

## Real-World Applications

### Production System Modeling

```rust
// Model a production web service based on real data
pub fn create_production_web_service() -> EmpiricalDistribution {
    // Based on actual production measurements
    let measurements = vec![
        Duration::from_millis(45),
        Duration::from_millis(52),
        Duration::from_millis(67),
        Duration::from_millis(89),
        Duration::from_millis(123),
        Duration::from_millis(156),
        Duration::from_millis(234),
        Duration::from_millis(445),
    ];
    
    EmpiricalDistribution::from_measurements(
        "production-web-service".to_string(),
        measurements,
    )
}

// Model system with bimodal behavior (cache hit/miss)
pub fn create_cache_service() -> MixtureDistribution {
    MixtureDistribution::new("cache-service".to_string())
        .add_component(0.85, Duration::from_millis(5))   // 85% cache hits
        .add_component(0.15, Duration::from_millis(150)) // 15% cache misses
}
```

### Microservices Architecture

```rust
// Different services with different characteristics
pub fn create_microservices_distributions() -> std::collections::HashMap<String, MixtureDistribution> {
    let mut services = std::collections::HashMap::new();
    
    // User service: bimodal (cache hit/miss)
    services.insert(
        "user-service".to_string(),
        MixtureDistribution::new("user-service".to_string())
            .add_component(0.8, Duration::from_millis(20))  // Fast path
            .add_component(0.2, Duration::from_millis(200)) // Slow path
    );
    
    // Payment service: mostly fast with rare slow operations
    services.insert(
        "payment-service".to_string(),
        MixtureDistribution::new("payment-service".to_string())
            .add_component(0.95, Duration::from_millis(100)) // Normal processing
            .add_component(0.05, Duration::from_millis(2000)) // Fraud checks
    );
    
    services
}
```

## Next Steps

This completes Chapter 7: Advanced Examples. You now have comprehensive knowledge of:

- Payload-dependent service time distributions
- Client-side admission control strategies
- Server-side throttling and admission control
- Advanced workload generation patterns
- Custom distribution implementations

These advanced patterns provide the foundation for modeling complex real-world systems and conducting sophisticated performance analysis with DESCARTES.

For further exploration, consider:

- Implementing custom distributions based on your specific use cases
- Combining multiple patterns for complex system modeling
- Using the testing framework to validate your custom distributions
- Applying these patterns to model your production systems