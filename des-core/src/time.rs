//! Simulation time management

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, Sub, Mul};
use std::time::Duration;

/// Simulation time with nanosecond precision
///
/// SimTime represents a point in simulation time, stored as nanoseconds since
/// the simulation start. It supports arithmetic operations and conversions
/// to/from standard Duration types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SimTime(u64);

impl SimTime {
    /// Create a new SimTime at the simulation start (time zero)
    pub const fn zero() -> Self {
        SimTime(0)
    }

    /// Create a SimTime from nanoseconds
    pub const fn from_nanos(nanos: u64) -> Self {
        SimTime(nanos)
    }

    /// Create a SimTime from microseconds
    pub const fn from_micros(micros: u64) -> Self {
        SimTime(micros * 1_000)
    }

    /// Create a SimTime from milliseconds
    pub const fn from_millis(millis: u64) -> Self {
        SimTime(millis * 1_000_000)
    }

    /// Create a SimTime from seconds
    pub const fn from_secs(secs: u64) -> Self {
        SimTime(secs * 1_000_000_000)
    }

    /// Create a SimTime from a Duration
    pub fn from_duration(duration: Duration) -> Self {
        SimTime(duration.as_nanos() as u64)
    }

    /// Convert SimTime to a Duration
    pub fn as_duration(&self) -> Duration {
        Duration::from_nanos(self.0)
    }

    /// Get the raw nanosecond value
    pub const fn as_nanos(&self) -> u64 {
        self.0
    }

    /// Calculate the duration since another SimTime
    pub fn duration_since(&self, earlier: SimTime) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }

    /// Add a duration to this SimTime
    pub fn add_duration(&self, duration: Duration) -> Self {
        SimTime(self.0.saturating_add(duration.as_nanos() as u64))
    }

    /// Subtract a duration from this SimTime
    pub fn sub_duration(&self, duration: Duration) -> Self {
        SimTime(self.0.saturating_sub(duration.as_nanos() as u64))
    }
}

impl Add<SimTime> for SimTime {
    type Output = SimTime;

    fn add(self, rhs: SimTime) -> Self::Output {
        SimTime(self.0.saturating_add(rhs.0))
    }
}
impl Add<Duration> for SimTime {
    type Output = SimTime;

    fn add(self, rhs: Duration) -> Self::Output {
        self.add_duration(rhs)
    }
}

impl Sub<Duration> for SimTime {
    type Output = SimTime;

    fn sub(self, rhs: Duration) -> Self::Output {
        self.sub_duration(rhs)
    }
}

impl Sub<SimTime> for SimTime {
    type Output = Duration;

    fn sub(self, rhs: SimTime) -> Self::Output {
        self.duration_since(rhs)
    }
}

impl Mul<u64> for SimTime {
    type Output = SimTime;

    fn mul(self, rhs: u64) -> Self::Output {
        SimTime(self.0.saturating_mul(rhs))
    }
}

impl Default for SimTime {
    fn default() -> Self {
        SimTime::zero()
    }
}

impl From<f64> for SimTime {
    /// Convert from seconds (as f64) to SimTime
    /// 
    /// # Examples
    /// ```
    /// # use des_core::SimTime;
    /// let time = SimTime::from(1.5); // 1.5 seconds
    /// assert_eq!(time.as_nanos(), 1_500_000_000);
    /// ```
    /// 
    /// # Panics
    /// 
    /// Panics if the input is negative, infinite, or NaN.
    fn from(secs: f64) -> Self {
        if !secs.is_finite() {
            panic!("SimTime cannot be created from non-finite value: {secs}");
        }
        if secs < 0.0 {
            panic!("SimTime cannot be negative: {secs}");
        }
        
        // Check for potential overflow when converting to nanoseconds
        const MAX_SECS: f64 = (u64::MAX as f64) / 1_000_000_000.0;
        if secs > MAX_SECS {
            panic!("SimTime value too large: {secs} seconds (max: {MAX_SECS} seconds)");
        }
        
        SimTime::from_nanos((secs * 1_000_000_000.0) as u64)
    }
}

impl fmt::Display for SimTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let duration = self.as_duration();
        let secs = duration.as_secs();
        let millis = duration.subsec_millis();
        let micros = duration.subsec_micros() % 1000;
        let nanos = duration.subsec_nanos() % 1000;

        if secs > 0 {
            write!(f, "{secs}.{millis:03}s")
        } else if millis > 0 {
            write!(f, "{millis}.{micros:03}ms")
        } else if micros > 0 {
            write!(f, "{micros}.{nanos:03}µs")
        } else {
            write!(f, "{nanos}ns")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simtime_creation() {
        assert_eq!(SimTime::zero().as_nanos(), 0);
        assert_eq!(SimTime::from_nanos(1000).as_nanos(), 1000);
        assert_eq!(SimTime::from_micros(1).as_nanos(), 1_000);
        assert_eq!(SimTime::from_millis(1).as_nanos(), 1_000_000);
        assert_eq!(SimTime::from_secs(1).as_nanos(), 1_000_000_000);
    }

    #[test]
    fn test_simtime_arithmetic() {
        let t1 = SimTime::from_millis(100);
        let t2 = SimTime::from_millis(50);
        let duration = Duration::from_millis(25);

        assert_eq!(t1 + duration, SimTime::from_millis(125));
        assert_eq!(t1 - duration, SimTime::from_millis(75));
        assert_eq!(t1 - t2, Duration::from_millis(50));
    }

    #[test]
    fn test_simtime_ordering() {
        let t1 = SimTime::from_millis(100);
        let t2 = SimTime::from_millis(200);

        assert!(t1 < t2);
        assert!(t2 > t1);
        assert_eq!(t1, t1);
    }

    #[test]
    fn test_simtime_from_f64() {
        let t1 = SimTime::from(1.0); // 1 second
        assert_eq!(t1.as_nanos(), 1_000_000_000);

        let t2 = SimTime::from(0.5); // 0.5 seconds
        assert_eq!(t2.as_nanos(), 500_000_000);

        let t3 = SimTime::from(1.5); // 1.5 seconds
        assert_eq!(t3.as_nanos(), 1_500_000_000);

        let t4 = SimTime::from(0.000001); // 1 microsecond
        assert_eq!(t4.as_nanos(), 1_000);
    }

    #[test]
    #[should_panic(expected = "SimTime cannot be negative")]
    fn test_simtime_from_negative_f64() {
        let _ = SimTime::from(-1.0);
    }

    #[test]
    #[should_panic(expected = "SimTime cannot be created from non-finite value")]
    fn test_simtime_from_infinite_f64() {
        let _ = SimTime::from(f64::INFINITY);
    }

    #[test]
    #[should_panic(expected = "SimTime cannot be created from non-finite value")]
    fn test_simtime_from_nan_f64() {
        let _ = SimTime::from(f64::NAN);
    }

    #[test]
    #[should_panic(expected = "SimTime value too large")]
    fn test_simtime_from_too_large_f64() {
        // This should be larger than MAX_SECS
        let max_secs = (u64::MAX as f64) / 1_000_000_000.0;
        let _ = SimTime::from(max_secs + 1.0);
    }
}
