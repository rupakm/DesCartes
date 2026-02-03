//! Request tracking for computing end-to-end metrics
//!
//! This module provides comprehensive tracking of requests and attempts to compute
//! goodput, throughput, retry rates, timeout rates, and latency statistics.

use crate::error::MetricsError;
use descartes_core::{
    AttemptStatus, Request, RequestAttempt, RequestAttemptId, RequestId, RequestStatus, SimTime,
};
use std::collections::HashMap;
use std::time::Duration;

/// Tracks requests and attempts for computing end-to-end metrics
#[derive(Debug)]
pub struct RequestTracker {
    /// Currently active (incomplete) requests
    active_requests: HashMap<RequestId, Request>,
    /// Completed requests for analysis
    completed_requests: Vec<Request>,
    /// Currently active (incomplete) attempts
    active_attempts: HashMap<RequestAttemptId, RequestAttempt>,
    /// Completed attempts for analysis
    completed_attempts: Vec<RequestAttempt>,
    /// Maximum number of completed items to keep (0 = unlimited)
    max_completed: usize,
}

impl Default for RequestTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestTracker {
    /// Create a new request tracker with unlimited storage
    pub fn new() -> Self {
        Self {
            active_requests: HashMap::new(),
            completed_requests: Vec::new(),
            active_attempts: HashMap::new(),
            completed_attempts: Vec::new(),
            max_completed: 0, // 0 = unlimited
        }
    }

    /// Create a new request tracker with limited storage
    pub fn with_max_completed(max_completed: usize) -> Self {
        Self {
            active_requests: HashMap::new(),
            completed_requests: Vec::new(),
            active_attempts: HashMap::new(),
            completed_attempts: Vec::new(),
            max_completed,
        }
    }

    /// Record a new request being created
    pub fn start_request(&mut self, request: Request) -> Result<(), MetricsError> {
        if self.active_requests.contains_key(&request.id) {
            return Err(MetricsError::DuplicateRequest(request.id));
        }
        self.active_requests.insert(request.id, request);
        Ok(())
    }

    /// Record a new attempt for a request
    pub fn start_attempt(&mut self, attempt: RequestAttempt) -> Result<(), MetricsError> {
        if self.active_attempts.contains_key(&attempt.id) {
            return Err(MetricsError::DuplicateAttempt(attempt.id));
        }

        // Add attempt to the parent request if it exists
        if let Some(request) = self.active_requests.get_mut(&attempt.request_id) {
            request.add_attempt(attempt.id);
        }

        self.active_attempts.insert(attempt.id, attempt);
        Ok(())
    }

    /// Record attempt completion
    pub fn complete_attempt(
        &mut self,
        attempt_id: RequestAttemptId,
        status: AttemptStatus,
        completed_at: SimTime,
    ) -> Result<(), MetricsError> {
        let mut attempt = self
            .active_attempts
            .remove(&attempt_id)
            .ok_or(MetricsError::AttemptNotFound(attempt_id))?;

        attempt.complete(completed_at, status);
        self.completed_attempts.push(attempt);
        self.enforce_completed_limit();
        Ok(())
    }

    /// Record request completion (success or failure)
    pub fn complete_request(
        &mut self,
        request_id: RequestId,
        status: RequestStatus,
        completed_at: SimTime,
    ) -> Result<(), MetricsError> {
        let mut request = self
            .active_requests
            .remove(&request_id)
            .ok_or(MetricsError::RequestNotFound(request_id))?;

        request.complete(completed_at, status);
        self.completed_requests.push(request);
        self.enforce_completed_limit();
        Ok(())
    }

    /// Compute end-to-end latency statistics for completed requests
    pub fn request_latency_stats(&self) -> LatencyStats {
        let latencies: Vec<Duration> = self
            .completed_requests
            .iter()
            .filter_map(|r| r.latency())
            .collect();

        LatencyStats::from_durations(&latencies)
    }

    /// Compute per-attempt latency statistics
    pub fn attempt_latency_stats(&self) -> LatencyStats {
        let latencies: Vec<Duration> = self
            .completed_attempts
            .iter()
            .filter_map(|a| a.duration())
            .collect();

        LatencyStats::from_durations(&latencies)
    }

    /// Compute goodput (successful requests per unit time)
    pub fn goodput(&self, time_window: Duration) -> f64 {
        if time_window.is_zero() {
            return 0.0;
        }

        let successful_requests = self
            .completed_requests
            .iter()
            .filter(|r| r.is_successful())
            .count();

        successful_requests as f64 / time_window.as_secs_f64()
    }

    /// Compute throughput (all attempts per unit time)
    pub fn throughput(&self, time_window: Duration) -> f64 {
        if time_window.is_zero() {
            return 0.0;
        }

        let total_attempts = self.completed_attempts.len();
        total_attempts as f64 / time_window.as_secs_f64()
    }

    /// Compute retry rate (attempts per request)
    pub fn retry_rate(&self) -> f64 {
        if self.completed_requests.is_empty() {
            return 0.0;
        }

        let total_attempts: usize = self
            .completed_requests
            .iter()
            .map(|r| r.attempt_count())
            .sum();

        total_attempts as f64 / self.completed_requests.len() as f64
    }

    /// Compute timeout rate (fraction of attempts that timed out)
    pub fn timeout_rate(&self) -> f64 {
        if self.completed_attempts.is_empty() {
            return 0.0;
        }

        let timeout_attempts = self
            .completed_attempts
            .iter()
            .filter(|a| matches!(a.status, AttemptStatus::Timeout))
            .count();

        timeout_attempts as f64 / self.completed_attempts.len() as f64
    }

    /// Compute error rate (fraction of attempts that failed)
    pub fn error_rate(&self) -> f64 {
        if self.completed_attempts.is_empty() {
            return 0.0;
        }

        let error_attempts = self
            .completed_attempts
            .iter()
            .filter(|a| {
                matches!(
                    a.status,
                    AttemptStatus::ServerError | AttemptStatus::Rejected
                )
            })
            .count();

        error_attempts as f64 / self.completed_attempts.len() as f64
    }

    /// Compute success rate (fraction of requests that succeeded)
    pub fn success_rate(&self) -> f64 {
        if self.completed_requests.is_empty() {
            return 0.0;
        }

        let successful_requests = self
            .completed_requests
            .iter()
            .filter(|r| r.is_successful())
            .count();

        successful_requests as f64 / self.completed_requests.len() as f64
    }

    /// Get the number of active requests
    pub fn active_request_count(&self) -> usize {
        self.active_requests.len()
    }

    /// Get the number of active attempts
    pub fn active_attempt_count(&self) -> usize {
        self.active_attempts.len()
    }

    /// Get the number of completed requests
    pub fn completed_request_count(&self) -> usize {
        self.completed_requests.len()
    }

    /// Get the number of completed attempts
    pub fn completed_attempt_count(&self) -> usize {
        self.completed_attempts.len()
    }

    /// Get all completed requests
    pub fn get_completed_requests(&self) -> &[Request] {
        &self.completed_requests
    }

    /// Get all completed attempts
    pub fn get_completed_attempts(&self) -> &[RequestAttempt] {
        &self.completed_attempts
    }

    /// Get all active requests
    pub fn get_active_requests(&self) -> impl Iterator<Item = &Request> {
        self.active_requests.values()
    }

    /// Get all active attempts
    pub fn get_active_attempts(&self) -> impl Iterator<Item = &RequestAttempt> {
        self.active_attempts.values()
    }

    /// Clear all tracking data
    pub fn clear(&mut self) {
        self.active_requests.clear();
        self.completed_requests.clear();
        self.active_attempts.clear();
        self.completed_attempts.clear();
    }

    /// Get comprehensive statistics
    pub fn get_stats(&self, time_window: Duration) -> RequestTrackerStats {
        RequestTrackerStats {
            active_requests: self.active_request_count(),
            active_attempts: self.active_attempt_count(),
            completed_requests: self.completed_request_count(),
            completed_attempts: self.completed_attempt_count(),
            goodput: self.goodput(time_window),
            throughput: self.throughput(time_window),
            retry_rate: self.retry_rate(),
            timeout_rate: self.timeout_rate(),
            error_rate: self.error_rate(),
            success_rate: self.success_rate(),
            request_latency: self.request_latency_stats(),
            attempt_latency: self.attempt_latency_stats(),
        }
    }

    /// Enforce the maximum completed items limit
    fn enforce_completed_limit(&mut self) {
        if self.max_completed == 0 {
            return; // Unlimited
        }

        // Remove oldest completed requests if over limit
        if self.completed_requests.len() > self.max_completed {
            let excess = self.completed_requests.len() - self.max_completed;
            self.completed_requests.drain(0..excess);
        }

        // Remove oldest completed attempts if over limit
        if self.completed_attempts.len() > self.max_completed {
            let excess = self.completed_attempts.len() - self.max_completed;
            self.completed_attempts.drain(0..excess);
        }
    }
}

/// Comprehensive latency statistics
#[derive(Debug, Clone, Default)]
pub struct LatencyStats {
    /// Mean latency
    pub mean: Duration,
    /// Median latency (50th percentile)
    pub median: Duration,
    /// 50th percentile
    pub p50: Duration,
    /// 95th percentile
    pub p95: Duration,
    /// 99th percentile
    pub p99: Duration,
    /// 99.9th percentile
    pub p999: Duration,
    /// Minimum latency
    pub min: Duration,
    /// Maximum latency
    pub max: Duration,
    /// Standard deviation
    pub std_dev: Duration,
    /// Number of samples
    pub count: usize,
}

impl LatencyStats {
    /// Create latency statistics from a collection of durations
    pub fn from_durations(durations: &[Duration]) -> Self {
        if durations.is_empty() {
            return Self::default();
        }

        let mut sorted_durations = durations.to_vec();
        sorted_durations.sort();

        let count = sorted_durations.len();
        let min = sorted_durations[0];
        let max = sorted_durations[count - 1];

        // Calculate mean
        let total_nanos: u64 = sorted_durations.iter().map(|d| d.as_nanos() as u64).sum();
        let mean_nanos = total_nanos / count as u64;
        let mean = Duration::from_nanos(mean_nanos);

        // Calculate percentiles
        let p50 = percentile(&sorted_durations, 50.0);
        let median = p50;
        let p95 = percentile(&sorted_durations, 95.0);
        let p99 = percentile(&sorted_durations, 99.0);
        let p999 = percentile(&sorted_durations, 99.9);

        // Calculate standard deviation
        let variance_nanos: f64 = sorted_durations
            .iter()
            .map(|d| {
                let diff = d.as_nanos() as f64 - mean_nanos as f64;
                diff * diff
            })
            .sum::<f64>()
            / count as f64;
        let std_dev = Duration::from_nanos(variance_nanos.sqrt() as u64);

        Self {
            mean,
            median,
            p50,
            p95,
            p99,
            p999,
            min,
            max,
            std_dev,
            count,
        }
    }
}

/// Calculate a percentile from sorted durations
fn percentile(sorted_durations: &[Duration], p: f64) -> Duration {
    if sorted_durations.is_empty() {
        return Duration::ZERO;
    }

    let index = (p / 100.0 * (sorted_durations.len() - 1) as f64).round() as usize;
    sorted_durations[index.min(sorted_durations.len() - 1)]
}

/// Comprehensive statistics from the request tracker
#[derive(Debug, Clone)]
pub struct RequestTrackerStats {
    /// Number of active requests
    pub active_requests: usize,
    /// Number of active attempts
    pub active_attempts: usize,
    /// Number of completed requests
    pub completed_requests: usize,
    /// Number of completed attempts
    pub completed_attempts: usize,
    /// Goodput (successful requests per second)
    pub goodput: f64,
    /// Throughput (attempts per second)
    pub throughput: f64,
    /// Retry rate (attempts per request)
    pub retry_rate: f64,
    /// Timeout rate (fraction of attempts that timed out)
    pub timeout_rate: f64,
    /// Error rate (fraction of attempts that failed)
    pub error_rate: f64,
    /// Success rate (fraction of requests that succeeded)
    pub success_rate: f64,
    /// Request latency statistics
    pub request_latency: LatencyStats,
    /// Attempt latency statistics
    pub attempt_latency: LatencyStats,
}

impl std::fmt::Display for RequestTrackerStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Request Tracker Statistics:")?;
        writeln!(
            f,
            "  Active: {} requests, {} attempts",
            self.active_requests, self.active_attempts
        )?;
        writeln!(
            f,
            "  Completed: {} requests, {} attempts",
            self.completed_requests, self.completed_attempts
        )?;
        writeln!(f, "  Goodput: {:.2} req/s", self.goodput)?;
        writeln!(f, "  Throughput: {:.2} attempts/s", self.throughput)?;
        writeln!(f, "  Retry rate: {:.2} attempts/req", self.retry_rate)?;
        writeln!(f, "  Timeout rate: {:.1}%", self.timeout_rate * 100.0)?;
        writeln!(f, "  Error rate: {:.1}%", self.error_rate * 100.0)?;
        writeln!(f, "  Success rate: {:.1}%", self.success_rate * 100.0)?;
        writeln!(
            f,
            "  Request latency: mean={:.2}ms, p95={:.2}ms, p99={:.2}ms",
            self.request_latency.mean.as_secs_f64() * 1000.0,
            self.request_latency.p95.as_secs_f64() * 1000.0,
            self.request_latency.p99.as_secs_f64() * 1000.0
        )?;
        write!(
            f,
            "  Attempt latency: mean={:.2}ms, p95={:.2}ms, p99={:.2}ms",
            self.attempt_latency.mean.as_secs_f64() * 1000.0,
            self.attempt_latency.p95.as_secs_f64() * 1000.0,
            self.attempt_latency.p99.as_secs_f64() * 1000.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use descartes_core::{AttemptStatus, RequestStatus};
    use std::time::Duration;

    fn create_test_request(id: u64, created_at: SimTime) -> Request {
        Request::new(RequestId(id), created_at, vec![1, 2, 3])
    }

    fn create_test_attempt(
        id: u64,
        request_id: u64,
        attempt_number: usize,
        started_at: SimTime,
    ) -> RequestAttempt {
        RequestAttempt::new(
            RequestAttemptId(id),
            RequestId(request_id),
            attempt_number,
            started_at,
            vec![1, 2, 3],
        )
    }

    #[test]
    fn test_request_tracking() {
        let mut tracker = RequestTracker::new();
        let start_time = SimTime::from_millis(100);
        let end_time = SimTime::from_millis(200);

        // Start a request
        let request = create_test_request(1, start_time);
        tracker.start_request(request).unwrap();
        assert_eq!(tracker.active_request_count(), 1);

        // Complete the request
        tracker
            .complete_request(RequestId(1), RequestStatus::Success, end_time)
            .unwrap();
        assert_eq!(tracker.active_request_count(), 0);
        assert_eq!(tracker.completed_request_count(), 1);

        // Check latency
        let stats = tracker.request_latency_stats();
        assert_eq!(stats.mean, Duration::from_millis(100));
    }

    #[test]
    fn test_attempt_tracking() {
        let mut tracker = RequestTracker::new();
        let start_time = SimTime::from_millis(100);
        let end_time = SimTime::from_millis(150);

        // Start a request first
        let request = create_test_request(1, start_time);
        tracker.start_request(request).unwrap();

        // Start an attempt
        let attempt = create_test_attempt(1, 1, 1, start_time);
        tracker.start_attempt(attempt).unwrap();
        assert_eq!(tracker.active_attempt_count(), 1);

        // Complete the attempt
        tracker
            .complete_attempt(RequestAttemptId(1), AttemptStatus::Success, end_time)
            .unwrap();
        assert_eq!(tracker.active_attempt_count(), 0);
        assert_eq!(tracker.completed_attempt_count(), 1);

        // Check that attempt was added to request
        let active_requests: Vec<_> = tracker.get_active_requests().collect();
        assert_eq!(active_requests[0].attempt_count(), 1);
    }

    #[test]
    fn test_goodput_calculation() {
        let mut tracker = RequestTracker::new();
        let start_time = SimTime::from_millis(0);
        let end_time = SimTime::from_millis(100);

        // Add successful requests
        for i in 0..5 {
            let request = create_test_request(i, start_time);
            tracker.start_request(request).unwrap();
            tracker
                .complete_request(RequestId(i), RequestStatus::Success, end_time)
                .unwrap();
        }

        // Add failed request
        let request = create_test_request(5, start_time);
        tracker.start_request(request).unwrap();
        tracker
            .complete_request(
                RequestId(5),
                RequestStatus::Failed {
                    reason: "error".to_string(),
                },
                end_time,
            )
            .unwrap();

        let time_window = Duration::from_secs(1);
        let goodput = tracker.goodput(time_window);
        assert_eq!(goodput, 5.0); // 5 successful requests per second
    }

    #[test]
    fn test_retry_rate_calculation() {
        let mut tracker = RequestTracker::new();
        let start_time = SimTime::from_millis(0);

        // Request with 1 attempt
        let request1 = create_test_request(1, start_time);
        tracker.start_request(request1).unwrap();
        let attempt1 = create_test_attempt(1, 1, 1, start_time);
        tracker.start_attempt(attempt1).unwrap();
        tracker
            .complete_request(RequestId(1), RequestStatus::Success, start_time)
            .unwrap();

        // Request with 3 attempts
        let request2 = create_test_request(2, start_time);
        tracker.start_request(request2).unwrap();
        for i in 1..=3 {
            let attempt = create_test_attempt((i + 1) as u64, 2, i, start_time);
            tracker.start_attempt(attempt).unwrap();
        }
        tracker
            .complete_request(RequestId(2), RequestStatus::Success, start_time)
            .unwrap();

        let retry_rate = tracker.retry_rate();
        assert_eq!(retry_rate, 2.0); // (1 + 3) / 2 = 2.0 attempts per request
    }

    #[test]
    fn test_timeout_rate_calculation() {
        let mut tracker = RequestTracker::new();
        let start_time = SimTime::from_millis(0);
        let end_time = SimTime::from_millis(100);

        // Add successful attempt
        let attempt1 = create_test_attempt(1, 1, 1, start_time);
        tracker.start_attempt(attempt1).unwrap();
        tracker
            .complete_attempt(RequestAttemptId(1), AttemptStatus::Success, end_time)
            .unwrap();

        // Add timeout attempt
        let attempt2 = create_test_attempt(2, 2, 1, start_time);
        tracker.start_attempt(attempt2).unwrap();
        tracker
            .complete_attempt(RequestAttemptId(2), AttemptStatus::Timeout, end_time)
            .unwrap();

        let timeout_rate = tracker.timeout_rate();
        assert_eq!(timeout_rate, 0.5); // 1 timeout out of 2 attempts
    }

    #[test]
    fn test_latency_stats() {
        let durations = vec![
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(30),
            Duration::from_millis(40),
            Duration::from_millis(50),
        ];

        let stats = LatencyStats::from_durations(&durations);
        assert_eq!(stats.count, 5);
        assert_eq!(stats.min, Duration::from_millis(10));
        assert_eq!(stats.max, Duration::from_millis(50));
        assert_eq!(stats.mean, Duration::from_millis(30));
        assert_eq!(stats.median, Duration::from_millis(30));
        assert_eq!(stats.p50, Duration::from_millis(30));
    }

    #[test]
    fn test_empty_latency_stats() {
        let stats = LatencyStats::from_durations(&[]);
        assert_eq!(stats.count, 0);
        assert_eq!(stats.mean, Duration::ZERO);
        assert_eq!(stats.min, Duration::ZERO);
        assert_eq!(stats.max, Duration::ZERO);
    }

    #[test]
    fn test_comprehensive_stats() {
        let mut tracker = RequestTracker::new();
        let start_time = SimTime::from_millis(0);
        let end_time = SimTime::from_millis(100);

        // Add some test data
        let request = create_test_request(1, start_time);
        tracker.start_request(request).unwrap();
        let attempt = create_test_attempt(1, 1, 1, start_time);
        tracker.start_attempt(attempt).unwrap();
        tracker
            .complete_attempt(RequestAttemptId(1), AttemptStatus::Success, end_time)
            .unwrap();
        tracker
            .complete_request(RequestId(1), RequestStatus::Success, end_time)
            .unwrap();

        let stats = tracker.get_stats(Duration::from_secs(1));
        assert_eq!(stats.completed_requests, 1);
        assert_eq!(stats.completed_attempts, 1);
        assert_eq!(stats.success_rate, 1.0);
        assert_eq!(stats.retry_rate, 1.0);
    }

    #[test]
    fn test_tracker_with_limit() {
        let mut tracker = RequestTracker::with_max_completed(2);
        let start_time = SimTime::from_millis(0);
        let end_time = SimTime::from_millis(100);

        // Add 3 requests, should only keep the last 2
        for i in 0..3 {
            let request = create_test_request(i, start_time);
            tracker.start_request(request).unwrap();
            tracker
                .complete_request(RequestId(i), RequestStatus::Success, end_time)
                .unwrap();
        }

        assert_eq!(tracker.completed_request_count(), 2);
    }

    #[test]
    fn test_error_handling() {
        let mut tracker = RequestTracker::new();

        // Test duplicate request
        let request1 = create_test_request(1, SimTime::from_millis(0));
        let request2 = create_test_request(1, SimTime::from_millis(0));
        tracker.start_request(request1).unwrap();
        assert!(tracker.start_request(request2).is_err());

        // Test completing non-existent request
        assert!(tracker
            .complete_request(
                RequestId(999),
                RequestStatus::Success,
                SimTime::from_millis(100)
            )
            .is_err());
    }
}
