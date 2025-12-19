//! Formal reasoning capabilities for simulation verification
//!
//! This module provides tools for formal verification of simulation properties,
//! including Lyapunov function evaluation and certificate verification.

use thiserror::Error;

/// Errors related to Lyapunov function evaluation
#[derive(Debug, Error)]
pub enum LyapunovError {
    #[error("Lyapunov drift condition violated: drift={drift}, bound={bound}")]
    DriftViolation { drift: f64, bound: f64 },

    #[error("Invalid Lyapunov function: {0}")]
    InvalidFunction(String),

    #[error("Evaluation failed: {0}")]
    EvaluationFailed(String),
}

/// Errors related to certificate verification
#[derive(Debug, Error)]
pub enum CertificateError {
    #[error("Certificate violated: {name} - {reason}")]
    Violated { name: String, reason: String },

    #[error("Invalid certificate: {0}")]
    InvalidCertificate(String),

    #[error("Verification failed: {0}")]
    VerificationFailed(String),
}

/// Top-level verification error
#[derive(Debug, Error)]
pub enum VerificationError {
    #[error("Lyapunov verification failed: {0}")]
    Lyapunov(#[from] LyapunovError),

    #[error("Certificate verification failed: {0}")]
    Certificate(#[from] CertificateError),

    #[error("Verification error: {0}")]
    General(String),
}
