use std::ops::{Add, Sub};
use std::time::Duration;

use des_core::async_runtime::{self, SimSleep};
use des_core::SimTime;

/// Tokio-like `Instant` backed by DES simulation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instant(SimTime);

impl Instant {
    /// Returns the current simulated time.
    ///
    /// # Panics
    ///
    /// Panics if called outside scheduler context (i.e., when the DES runtime is
    /// not installed or not currently polling within the simulation).
    pub fn now() -> Self {
        let t = async_runtime::current_sim_time().expect(
            "des_tokio::time::Instant::now called outside scheduler context. \
             Install the runtime and call from within simulation execution",
        );
        Self(t)
    }

    pub fn duration_since(&self, earlier: Instant) -> Duration {
        self.0.duration_since(earlier.0)
    }

    pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
        self.0.duration_since(earlier.0)
    }

    pub fn elapsed(&self) -> Duration {
        Instant::now().duration_since(*self)
    }

    pub fn checked_add(&self, duration: Duration) -> Option<Instant> {
        Some(Instant(self.0.add_duration(duration)))
    }

    pub fn checked_sub(&self, duration: Duration) -> Option<Instant> {
        Some(Instant(self.0.sub_duration(duration)))
    }

    pub(crate) fn as_sim_time(&self) -> SimTime {
        self.0
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;

    fn add(self, rhs: Duration) -> Self::Output {
        Instant(self.0 + rhs)
    }
}

impl Sub<Duration> for Instant {
    type Output = Instant;

    fn sub(self, rhs: Duration) -> Self::Output {
        Instant(self.0 - rhs)
    }
}

impl Sub<Instant> for Instant {
    type Output = Duration;

    fn sub(self, rhs: Instant) -> Self::Output {
        self.0 - rhs.0
    }
}

/// Tokio-like sleep future (backed by `des_core::async_runtime::SimSleep`).
pub type Sleep = SimSleep;

pub fn sleep(duration: Duration) -> Sleep {
    async_runtime::sim_sleep(duration)
}

pub fn sleep_until(deadline: Instant) -> Sleep {
    async_runtime::sim_sleep_until(deadline.as_sim_time())
}
