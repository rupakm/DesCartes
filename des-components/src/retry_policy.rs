//! Retry policy implementations for SimpleClient
//!
//! This module provides retry policies that can be used with SimpleClient to handle
//! request failures with various backoff strategies including exponential backoff,
//! token bucket-based retries, and success-based adaptive retries.

use des_core::{RequestAttempt, Response, SimTime};
use rand::Rng;
use std::time::Duration;

/// Trait for retry policies that determine when and how to retry failed requests
pub trait RetryPolicy: Clone + Send + Sync + 'static {
    /// Determine if a request should be retried based on the response or timeout
    ///
    /// Returns Some(delay) if the request should be retried after the specified delay,
    /// or None if no retry should be attempted.
    fn should_retry(
        &mut self,
        attempt: &RequestAttempt,
        response: Option<&Response>,
    ) -> Option<Duration>;

    /// Reset the policy state for a new request
    fn reset(&mut self);

    /// Get the maximum number of attempts this policy allows
    fn max_attempts(&self) -> usize;
}

/// Exponential backoff retry policy with optional jitter
#[derive(Clone)]
pub struct ExponentialBackoffPolicy {
    /// Maximum number of retry attempts
    max_attempts: usize,
    /// Current attempt number (1-indexed)
    current_attempt: usize,
    /// Base delay for the first retry
    base_delay: Duration,
    /// Multiplier for each subsequent retry
    multiplier: f64,
    /// Maximum delay cap
    max_delay: Duration,
    /// Whether to add jitter to prevent thundering herd
    jitter: bool,
}

impl ExponentialBackoffPolicy {
    /// Create a new exponential backoff policy
    pub fn new(max_attempts: usize, base_delay: Duration) -> Self {
        Self {
            max_attempts,
            current_attempt: 0,
            base_delay,
            multiplier: 2.0,
            max_delay: Duration::from_secs(60),
            jitter: false,
        }
    }

    /// Set the backoff multiplier (default: 2.0)
    pub fn with_multiplier(mut self, multiplier: f64) -> Self {
        self.multiplier = multiplier;
        self
    }

    /// Set the maximum delay cap (default: 60 seconds)
    pub fn with_max_delay(mut self, max_delay: Duration) -> Self {
        self.max_delay = max_delay;
        self
    }

    /// Enable jitter to add randomness to delays (default: false)
    pub fn with_jitter(mut self, jitter: bool) -> Self {
        self.jitter = jitter;
        self
    }

    /// Calculate the delay for the current attempt
    fn calculate_delay(&self) -> Duration {
        let delay_ms = self.base_delay.as_millis() as f64
            * self.multiplier.powi((self.current_attempt - 1) as i32);

        let delay = Duration::from_millis(delay_ms as u64).min(self.max_delay);

        if self.jitter {
            // Add ±25% jitter
            let mut rng = rand::thread_rng();
            let jitter_factor = rng.gen_range(0.75..=1.25);
            Duration::from_millis((delay.as_millis() as f64 * jitter_factor) as u64)
        } else {
            delay
        }
    }

    /// Check if the error/response is retryable
    fn is_retryable(&self, response: Option<&Response>) -> bool {
        match response {
            Some(resp) => !resp.is_success(), // Retry on any error response
            None => true,                     // Retry on timeout (no response)
        }
    }
}

impl RetryPolicy for ExponentialBackoffPolicy {
    fn should_retry(
        &mut self,
        _attempt: &RequestAttempt,
        response: Option<&Response>,
    ) -> Option<Duration> {
        self.current_attempt += 1;

        if self.current_attempt > self.max_attempts || !self.is_retryable(response) {
            return None;
        }

        Some(self.calculate_delay())
    }

    fn reset(&mut self) {
        self.current_attempt = 0;
    }

    fn max_attempts(&self) -> usize {
        self.max_attempts
    }
}

/// Token bucket-based retry policy that limits retry rate
#[derive(Clone)]
pub struct TokenBucketRetryPolicy {
    /// Maximum number of retry attempts
    max_attempts: usize,
    /// Current attempt number
    current_attempt: usize,
    /// Maximum number of tokens in the bucket
    max_tokens: u32,
    /// Current number of tokens
    current_tokens: u32,
    /// Rate at which tokens are replenished (tokens per second)
    refill_rate: f64,
    /// Last time tokens were refilled
    last_refill: Option<SimTime>,
    /// Base delay when no tokens are available
    base_delay: Duration,
}

impl TokenBucketRetryPolicy {
    /// Create a new token bucket retry policy
    pub fn new(max_attempts: usize, max_tokens: u32, refill_rate: f64) -> Self {
        Self {
            max_attempts,
            current_attempt: 0,
            max_tokens,
            current_tokens: max_tokens, // Start with full bucket
            refill_rate,
            last_refill: None,
            base_delay: Duration::from_millis(100),
        }
    }

    /// Set the base delay when no tokens are available
    pub fn with_base_delay(mut self, base_delay: Duration) -> Self {
        self.base_delay = base_delay;
        self
    }

    /// Refill tokens based on elapsed time
    fn refill_tokens(&mut self, current_time: SimTime) {
        if let Some(last_refill) = self.last_refill {
            let elapsed = current_time.duration_since(last_refill);
            let tokens_to_add = (elapsed.as_secs_f64() * self.refill_rate) as u32;

            if tokens_to_add > 0 {
                self.current_tokens = (self.current_tokens + tokens_to_add).min(self.max_tokens);
                self.last_refill = Some(current_time);
            }
        } else {
            self.last_refill = Some(current_time);
        }
    }

    /// Try to consume a token for retry
    fn try_consume_token(&mut self, current_time: SimTime) -> bool {
        self.refill_tokens(current_time);

        if self.current_tokens > 0 {
            self.current_tokens -= 1;
            true
        } else {
            false
        }
    }
}

impl RetryPolicy for TokenBucketRetryPolicy {
    fn should_retry(
        &mut self,
        attempt: &RequestAttempt,
        response: Option<&Response>,
    ) -> Option<Duration> {
        self.current_attempt += 1;

        if self.current_attempt > self.max_attempts {
            return None;
        }

        // Only retry on failures
        if let Some(resp) = response {
            if resp.is_success() {
                return None;
            }
        }

        // Try to consume a token
        if self.try_consume_token(attempt.started_at) {
            // Token available, retry immediately
            Some(Duration::ZERO)
        } else {
            // No tokens available, wait for base delay
            Some(self.base_delay)
        }
    }

    fn reset(&mut self) {
        self.current_attempt = 0;
        // Don't reset tokens - they persist across requests
    }

    fn max_attempts(&self) -> usize {
        self.max_attempts
    }
}

/// Success-based adaptive retry policy that adjusts retry behavior based on recent success rate
#[derive(Clone)]
pub struct SuccessBasedRetryPolicy {
    /// Maximum number of retry attempts
    max_attempts: usize,
    /// Current attempt number
    current_attempt: usize,
    /// Base delay for retries
    base_delay: Duration,
    /// Window size for tracking success rate
    window_size: usize,
    /// Recent request outcomes (true = success, false = failure)
    recent_outcomes: Vec<bool>,
    /// Minimum success rate to allow retries (0.0 to 1.0)
    min_success_rate: f64,
    /// Delay multiplier when success rate is low
    failure_multiplier: f64,
}

impl SuccessBasedRetryPolicy {
    /// Create a new success-based retry policy
    pub fn new(max_attempts: usize, base_delay: Duration, window_size: usize) -> Self {
        Self {
            max_attempts,
            current_attempt: 0,
            base_delay,
            window_size,
            recent_outcomes: Vec::new(),
            min_success_rate: 0.5,   // 50% success rate threshold
            failure_multiplier: 3.0, // 3x delay when success rate is low
        }
    }

    /// Set the minimum success rate threshold (default: 0.5)
    pub fn with_min_success_rate(mut self, min_success_rate: f64) -> Self {
        self.min_success_rate = min_success_rate.clamp(0.0, 1.0);
        self
    }

    /// Set the delay multiplier when success rate is low (default: 3.0)
    pub fn with_failure_multiplier(mut self, failure_multiplier: f64) -> Self {
        self.failure_multiplier = failure_multiplier;
        self
    }

    /// Record the outcome of a request
    pub fn record_outcome(&mut self, success: bool) {
        self.recent_outcomes.push(success);

        // Keep only the most recent outcomes within the window
        if self.recent_outcomes.len() > self.window_size {
            self.recent_outcomes.remove(0);
        }
    }

    /// Calculate the current success rate
    fn success_rate(&self) -> f64 {
        if self.recent_outcomes.is_empty() {
            1.0 // Assume success if no data
        } else {
            let successes = self.recent_outcomes.iter().filter(|&&x| x).count();
            successes as f64 / self.recent_outcomes.len() as f64
        }
    }

    /// Calculate delay based on success rate
    fn calculate_delay(&self) -> Duration {
        let success_rate = self.success_rate();

        if success_rate < self.min_success_rate {
            // Low success rate, increase delay
            Duration::from_millis(
                (self.base_delay.as_millis() as f64 * self.failure_multiplier) as u64,
            )
        } else {
            self.base_delay
        }
    }
}

impl RetryPolicy for SuccessBasedRetryPolicy {
    fn should_retry(
        &mut self,
        _attempt: &RequestAttempt,
        response: Option<&Response>,
    ) -> Option<Duration> {
        self.current_attempt += 1;

        if self.current_attempt > self.max_attempts {
            return None;
        }

        // Only retry on failures
        let is_success = response.is_some_and(|r| r.is_success());
        if is_success {
            return None;
        }

        Some(self.calculate_delay())
    }

    fn reset(&mut self) {
        self.current_attempt = 0;
    }

    fn max_attempts(&self) -> usize {
        self.max_attempts
    }
}

/// No retry policy - never retries failed requests
#[derive(Clone)]
pub struct NoRetryPolicy;

impl NoRetryPolicy {
    /// Create a new no-retry policy
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoRetryPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl RetryPolicy for NoRetryPolicy {
    fn should_retry(
        &mut self,
        _attempt: &RequestAttempt,
        _response: Option<&Response>,
    ) -> Option<Duration> {
        // Never retry
        None
    }

    fn reset(&mut self) {
        // Nothing to reset
    }

    fn max_attempts(&self) -> usize {
        1 // Only the initial attempt
    }
}

/// Fixed retry policy that performs a fixed number of retries with a fixed delay
#[derive(Clone)]
pub struct FixedRetryPolicy {
    /// Maximum number of retry attempts
    max_attempts: usize,
    /// Current attempt number (1-indexed)
    current_attempt: usize,
    /// Fixed delay between retry attempts
    retry_delay: Duration,
}

impl FixedRetryPolicy {
    /// Create a new fixed retry policy
    ///
    /// # Arguments
    /// * `max_attempts` - Maximum number of retry attempts (not including the initial attempt)
    /// * `retry_delay` - Fixed delay between retry attempts
    pub fn new(max_attempts: usize, retry_delay: Duration) -> Self {
        Self {
            max_attempts,
            current_attempt: 0,
            retry_delay,
        }
    }

    /// Check if the error/response is retryable
    fn is_retryable(&self, response: Option<&Response>) -> bool {
        match response {
            Some(resp) => !resp.is_success(), // Retry on any error response
            None => true,                     // Retry on timeout (no response)
        }
    }
}

impl RetryPolicy for FixedRetryPolicy {
    fn should_retry(
        &mut self,
        _attempt: &RequestAttempt,
        response: Option<&Response>,
    ) -> Option<Duration> {
        self.current_attempt += 1;

        if self.current_attempt > self.max_attempts || !self.is_retryable(response) {
            return None;
        }

        Some(self.retry_delay)
    }

    fn reset(&mut self) {
        self.current_attempt = 0;
    }

    fn max_attempts(&self) -> usize {
        self.max_attempts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use des_core::{RequestAttemptId, RequestId};

    fn create_test_attempt() -> RequestAttempt {
        RequestAttempt::new(
            RequestAttemptId(1),
            RequestId(1),
            1,
            SimTime::zero(),
            vec![],
        )
    }

    fn create_success_response() -> Response {
        Response::success(
            RequestAttemptId(1),
            RequestId(1),
            SimTime::from_millis(100),
            vec![],
        )
    }

    fn create_error_response() -> Response {
        Response::error(
            RequestAttemptId(1),
            RequestId(1),
            SimTime::from_millis(100),
            500,
            "Internal Server Error".to_string(),
        )
    }

    #[test]
    fn test_exponential_backoff_policy() {
        let mut policy = ExponentialBackoffPolicy::new(3, Duration::from_millis(100));
        let attempt = create_test_attempt();
        let error_response = create_error_response();

        // First retry
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_some());
        assert_eq!(delay.unwrap(), Duration::from_millis(100));

        // Second retry
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_some());
        assert_eq!(delay.unwrap(), Duration::from_millis(200));

        // Third retry
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_some());
        assert_eq!(delay.unwrap(), Duration::from_millis(400));

        // Fourth attempt should be rejected
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_none());
    }

    #[test]
    fn test_exponential_backoff_no_retry_on_success() {
        let mut policy = ExponentialBackoffPolicy::new(3, Duration::from_millis(100));
        let attempt = create_test_attempt();
        let success_response = create_success_response();

        let delay = policy.should_retry(&attempt, Some(&success_response));
        assert!(delay.is_none());
    }

    #[test]
    fn test_token_bucket_policy() {
        let mut policy = TokenBucketRetryPolicy::new(3, 2, 1.0); // 2 tokens, 1 token/sec
        let attempt = create_test_attempt();
        let error_response = create_error_response();

        // First retry - should consume token and retry immediately
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_some());
        assert_eq!(delay.unwrap(), Duration::ZERO);

        // Second retry - should consume last token
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_some());
        assert_eq!(delay.unwrap(), Duration::ZERO);

        // Third retry - no tokens left, should wait
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_some());
        assert_eq!(delay.unwrap(), Duration::from_millis(100));
    }

    #[test]
    fn test_success_based_policy() {
        let mut policy = SuccessBasedRetryPolicy::new(3, Duration::from_millis(100), 5);
        let attempt = create_test_attempt();
        let error_response = create_error_response();

        // Record some failures to lower success rate
        policy.record_outcome(false);
        policy.record_outcome(false);
        policy.record_outcome(false);

        // Should retry with increased delay due to low success rate
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_some());
        assert_eq!(delay.unwrap(), Duration::from_millis(300)); // 3x multiplier
    }

    #[test]
    fn test_policy_reset() {
        let mut policy = ExponentialBackoffPolicy::new(3, Duration::from_millis(100));
        let attempt = create_test_attempt();
        let error_response = create_error_response();

        // Make some attempts
        policy.should_retry(&attempt, Some(&error_response));
        policy.should_retry(&attempt, Some(&error_response));

        // Reset and try again
        policy.reset();
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_some());
        assert_eq!(delay.unwrap(), Duration::from_millis(100)); // Back to base delay
    }

    #[test]
    fn test_no_retry_policy() {
        let mut policy = NoRetryPolicy::new();
        let attempt = create_test_attempt();
        let error_response = create_error_response();

        // Should never retry, regardless of response
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_none());

        // Should not retry even on timeout (no response)
        let delay = policy.should_retry(&attempt, None);
        assert!(delay.is_none());

        // Max attempts should be 1 (only initial attempt)
        assert_eq!(policy.max_attempts(), 1);
    }

    #[test]
    fn test_no_retry_policy_default() {
        let mut policy = NoRetryPolicy::default();
        let attempt = create_test_attempt();
        let error_response = create_error_response();

        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_none());
    }

    #[test]
    fn test_fixed_retry_policy() {
        let mut policy = FixedRetryPolicy::new(3, Duration::from_millis(500));
        let attempt = create_test_attempt();
        let error_response = create_error_response();

        // First retry - should use fixed delay
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_some());
        assert_eq!(delay.unwrap(), Duration::from_millis(500));

        // Second retry - should use same fixed delay
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_some());
        assert_eq!(delay.unwrap(), Duration::from_millis(500));

        // Third retry - should use same fixed delay
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_some());
        assert_eq!(delay.unwrap(), Duration::from_millis(500));

        // Fourth attempt should be rejected (exceeded max_attempts)
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_none());
    }

    #[test]
    fn test_fixed_retry_policy_no_retry_on_success() {
        let mut policy = FixedRetryPolicy::new(3, Duration::from_millis(500));
        let attempt = create_test_attempt();
        let success_response = create_success_response();

        // Should not retry on success
        let delay = policy.should_retry(&attempt, Some(&success_response));
        assert!(delay.is_none());
    }

    #[test]
    fn test_fixed_retry_policy_reset() {
        let mut policy = FixedRetryPolicy::new(2, Duration::from_millis(200));
        let attempt = create_test_attempt();
        let error_response = create_error_response();

        // Make some attempts
        policy.should_retry(&attempt, Some(&error_response));
        policy.should_retry(&attempt, Some(&error_response));

        // Should be at max attempts now
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_none());

        // Reset and try again
        policy.reset();
        let delay = policy.should_retry(&attempt, Some(&error_response));
        assert!(delay.is_some());
        assert_eq!(delay.unwrap(), Duration::from_millis(200));
    }
}
