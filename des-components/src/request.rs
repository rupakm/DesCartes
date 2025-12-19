//! Request and response data models for distributed system simulation
//!
//! This module provides types for modeling requests, attempts, and responses in
//! distributed systems. It distinguishes between logical requests (from the client's
//! perspective) and individual request attempts (actual network/server interactions).

use des_core::SimTime;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Unique identifier for logical requests
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(pub u64);

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Request({})", self.0)
    }
}

/// Unique identifier for individual request attempts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestAttemptId(pub u64);

impl std::fmt::Display for RequestAttemptId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Attempt({})", self.0)
    }
}

/// A logical request from a client - may involve multiple attempts
///
/// A Request represents the client's intent to perform an operation. It may
/// result in multiple RequestAttempts if retries are configured.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// Unique identifier for this request
    pub id: RequestId,
    /// Simulation time when the request was created
    pub created_at: SimTime,
    /// Request payload data
    pub payload: Vec<u8>,
    /// List of attempt IDs associated with this request
    pub attempts: Vec<RequestAttemptId>,
    /// Simulation time when the request completed (if completed)
    pub completed_at: Option<SimTime>,
    /// Final status of the request
    pub status: RequestStatus,
}

impl Request {
    /// Create a new request
    pub fn new(id: RequestId, created_at: SimTime, payload: Vec<u8>) -> Self {
        Self {
            id,
            created_at,
            payload,
            attempts: Vec::new(),
            completed_at: None,
            status: RequestStatus::Pending,
        }
    }

    /// Calculate the end-to-end latency of the request
    ///
    /// Returns the duration from request creation to completion, or None if
    /// the request has not yet completed.
    pub fn latency(&self) -> Option<Duration> {
        self.completed_at
            .map(|completed| completed.duration_since(self.created_at))
    }

    /// Get the number of attempts made for this request
    pub fn attempt_count(&self) -> usize {
        self.attempts.len()
    }

    /// Check if the request has completed
    pub fn is_completed(&self) -> bool {
        self.completed_at.is_some()
    }

    /// Check if the request was successful
    pub fn is_successful(&self) -> bool {
        matches!(self.status, RequestStatus::Success)
    }

    /// Add an attempt to this request
    pub fn add_attempt(&mut self, attempt_id: RequestAttemptId) {
        self.attempts.push(attempt_id);
    }

    /// Mark the request as completed with the given status
    pub fn complete(&mut self, completed_at: SimTime, status: RequestStatus) {
        self.completed_at = Some(completed_at);
        self.status = status;
    }
}

/// Status of a logical request
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestStatus {
    /// Request is still pending
    Pending,
    /// Request completed successfully
    Success,
    /// Request failed with a reason
    Failed { reason: String },
    /// All retry attempts exhausted
    Exhausted,
}

/// A single attempt to fulfill a request - may timeout or fail
///
/// A RequestAttempt represents one actual interaction with a server or service.
/// A single Request may have multiple attempts if retries are configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestAttempt {
    /// Unique identifier for this attempt
    pub id: RequestAttemptId,
    /// ID of the parent request
    pub request_id: RequestId,
    /// Attempt number (1-indexed)
    pub attempt_number: usize,
    /// Simulation time when the attempt started
    pub started_at: SimTime,
    /// Simulation time when the attempt completed (if completed)
    pub completed_at: Option<SimTime>,
    /// Status of this attempt
    pub status: AttemptStatus,
    /// Request payload data
    pub payload: Vec<u8>,
}

impl RequestAttempt {
    /// Create a new request attempt
    pub fn new(
        id: RequestAttemptId,
        request_id: RequestId,
        attempt_number: usize,
        started_at: SimTime,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            id,
            request_id,
            attempt_number,
            started_at,
            completed_at: None,
            status: AttemptStatus::Pending,
            payload,
        }
    }

    /// Calculate the duration of this attempt
    ///
    /// Returns the duration from attempt start to completion, or None if
    /// the attempt has not yet completed.
    pub fn duration(&self) -> Option<Duration> {
        self.completed_at
            .map(|completed| completed.duration_since(self.started_at))
    }

    /// Check if the attempt has completed
    pub fn is_completed(&self) -> bool {
        self.completed_at.is_some()
    }

    /// Check if the attempt was successful
    pub fn is_successful(&self) -> bool {
        matches!(self.status, AttemptStatus::Success)
    }

    /// Mark the attempt as completed with the given status
    pub fn complete(&mut self, completed_at: SimTime, status: AttemptStatus) {
        self.completed_at = Some(completed_at);
        self.status = status;
    }
}

/// Status of an individual request attempt
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttemptStatus {
    /// Attempt is still pending
    Pending,
    /// Attempt completed successfully
    Success,
    /// Attempt timed out
    Timeout,
    /// Server returned an error
    ServerError,
    /// Request was rejected (e.g., by throttling or circuit breaker)
    Rejected,
}

/// Response to a request attempt
///
/// A Response is generated when a RequestAttempt completes, either successfully
/// or with an error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// ID of the attempt this response is for
    pub attempt_id: RequestAttemptId,
    /// ID of the original request
    pub request_id: RequestId,
    /// Simulation time when the response was generated
    pub completed_at: SimTime,
    /// Response payload data
    pub payload: Vec<u8>,
    /// Status of the response
    pub status: ResponseStatus,
}

impl Response {
    /// Create a successful response
    pub fn success(
        attempt_id: RequestAttemptId,
        request_id: RequestId,
        completed_at: SimTime,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            attempt_id,
            request_id,
            completed_at,
            payload,
            status: ResponseStatus::Ok,
        }
    }

    /// Create an error response
    pub fn error(
        attempt_id: RequestAttemptId,
        request_id: RequestId,
        completed_at: SimTime,
        code: u32,
        message: String,
    ) -> Self {
        Self {
            attempt_id,
            request_id,
            completed_at,
            payload: Vec::new(),
            status: ResponseStatus::Error { code, message },
        }
    }

    /// Check if the response indicates success
    pub fn is_success(&self) -> bool {
        matches!(self.status, ResponseStatus::Ok)
    }

    /// Check if the response indicates an error
    pub fn is_error(&self) -> bool {
        matches!(self.status, ResponseStatus::Error { .. })
    }
}

/// Status of a response
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponseStatus {
    /// Response indicates success
    Ok,
    /// Response indicates an error
    Error {
        /// Error code
        code: u32,
        /// Error message
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_creation() {
        let id = RequestId(1);
        let created_at = SimTime::from_millis(100);
        let payload = vec![1, 2, 3];

        let request = Request::new(id, created_at, payload.clone());

        assert_eq!(request.id, id);
        assert_eq!(request.created_at, created_at);
        assert_eq!(request.payload, payload);
        assert_eq!(request.attempts.len(), 0);
        assert_eq!(request.completed_at, None);
        assert_eq!(request.status, RequestStatus::Pending);
        assert!(!request.is_completed());
        assert!(!request.is_successful());
    }

    #[test]
    fn test_request_latency() {
        let id = RequestId(1);
        let created_at = SimTime::from_millis(100);
        let completed_at = SimTime::from_millis(250);
        let payload = vec![1, 2, 3];

        let mut request = Request::new(id, created_at, payload);
        assert_eq!(request.latency(), None);

        request.complete(completed_at, RequestStatus::Success);
        assert_eq!(request.latency(), Some(Duration::from_millis(150)));
        assert!(request.is_completed());
        assert!(request.is_successful());
    }

    #[test]
    fn test_request_attempts() {
        let id = RequestId(1);
        let created_at = SimTime::from_millis(100);
        let payload = vec![1, 2, 3];

        let mut request = Request::new(id, created_at, payload);
        assert_eq!(request.attempt_count(), 0);

        request.add_attempt(RequestAttemptId(1));
        assert_eq!(request.attempt_count(), 1);

        request.add_attempt(RequestAttemptId(2));
        assert_eq!(request.attempt_count(), 2);
    }

    #[test]
    fn test_request_attempt_creation() {
        let id = RequestAttemptId(1);
        let request_id = RequestId(1);
        let started_at = SimTime::from_millis(100);
        let payload = vec![1, 2, 3];

        let attempt = RequestAttempt::new(id, request_id, 1, started_at, payload.clone());

        assert_eq!(attempt.id, id);
        assert_eq!(attempt.request_id, request_id);
        assert_eq!(attempt.attempt_number, 1);
        assert_eq!(attempt.started_at, started_at);
        assert_eq!(attempt.completed_at, None);
        assert_eq!(attempt.status, AttemptStatus::Pending);
        assert_eq!(attempt.payload, payload);
        assert!(!attempt.is_completed());
        assert!(!attempt.is_successful());
    }

    #[test]
    fn test_request_attempt_duration() {
        let id = RequestAttemptId(1);
        let request_id = RequestId(1);
        let started_at = SimTime::from_millis(100);
        let completed_at = SimTime::from_millis(150);
        let payload = vec![1, 2, 3];

        let mut attempt = RequestAttempt::new(id, request_id, 1, started_at, payload);
        assert_eq!(attempt.duration(), None);

        attempt.complete(completed_at, AttemptStatus::Success);
        assert_eq!(attempt.duration(), Some(Duration::from_millis(50)));
        assert!(attempt.is_completed());
        assert!(attempt.is_successful());
    }

    #[test]
    fn test_attempt_status_variants() {
        let id = RequestAttemptId(1);
        let request_id = RequestId(1);
        let started_at = SimTime::from_millis(100);
        let completed_at = SimTime::from_millis(150);
        let payload = vec![1, 2, 3];

        let mut attempt = RequestAttempt::new(id, request_id, 1, started_at, payload);

        // Test timeout
        attempt.complete(completed_at, AttemptStatus::Timeout);
        assert_eq!(attempt.status, AttemptStatus::Timeout);
        assert!(!attempt.is_successful());

        // Test server error
        attempt.status = AttemptStatus::ServerError;
        assert_eq!(attempt.status, AttemptStatus::ServerError);
        assert!(!attempt.is_successful());

        // Test rejected
        attempt.status = AttemptStatus::Rejected;
        assert_eq!(attempt.status, AttemptStatus::Rejected);
        assert!(!attempt.is_successful());
    }

    #[test]
    fn test_response_success() {
        let attempt_id = RequestAttemptId(1);
        let request_id = RequestId(1);
        let completed_at = SimTime::from_millis(150);
        let payload = vec![4, 5, 6];

        let response = Response::success(attempt_id, request_id, completed_at, payload.clone());

        assert_eq!(response.attempt_id, attempt_id);
        assert_eq!(response.request_id, request_id);
        assert_eq!(response.completed_at, completed_at);
        assert_eq!(response.payload, payload);
        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.is_success());
        assert!(!response.is_error());
    }

    #[test]
    fn test_response_error() {
        let attempt_id = RequestAttemptId(1);
        let request_id = RequestId(1);
        let completed_at = SimTime::from_millis(150);
        let code = 500;
        let message = "Internal Server Error".to_string();

        let response = Response::error(
            attempt_id,
            request_id,
            completed_at,
            code,
            message.clone(),
        );

        assert_eq!(response.attempt_id, attempt_id);
        assert_eq!(response.request_id, request_id);
        assert_eq!(response.completed_at, completed_at);
        assert!(response.payload.is_empty());
        assert_eq!(
            response.status,
            ResponseStatus::Error {
                code,
                message: message.clone()
            }
        );
        assert!(!response.is_success());
        assert!(response.is_error());
    }

    #[test]
    fn test_request_status_variants() {
        assert_eq!(RequestStatus::Pending, RequestStatus::Pending);
        assert_eq!(RequestStatus::Success, RequestStatus::Success);
        assert_eq!(
            RequestStatus::Failed {
                reason: "timeout".to_string()
            },
            RequestStatus::Failed {
                reason: "timeout".to_string()
            }
        );
        assert_eq!(RequestStatus::Exhausted, RequestStatus::Exhausted);
    }
}
