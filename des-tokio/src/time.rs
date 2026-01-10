use std::future::Future;
use std::ops::{Add, Sub};
use std::pin::Pin;
use std::task::{Context, Poll};
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

    /// Returns the amount of time elapsed since `earlier`.
    ///
    /// # Panics
    ///
    /// Panics if `earlier` is later than `self`.
    pub fn duration_since(&self, earlier: Instant) -> Duration {
        self.checked_duration_since(earlier)
            .expect("des_tokio::time::Instant::duration_since: earlier is later than self")
    }

    /// Returns the amount of time elapsed since `earlier`, or `None` if `earlier > self`.
    pub fn checked_duration_since(&self, earlier: Instant) -> Option<Duration> {
        self.0.checked_duration_since(earlier.0)
    }

    /// Returns the amount of time elapsed since `earlier`, saturating at zero.
    pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
        self.checked_duration_since(earlier)
            .unwrap_or(Duration::ZERO)
    }

    pub fn elapsed(&self) -> Duration {
        Instant::now().saturating_duration_since(*self)
    }

    pub fn checked_add(&self, duration: Duration) -> Option<Instant> {
        self.0.checked_add_duration(duration).map(Instant)
    }

    pub fn checked_sub(&self, duration: Duration) -> Option<Instant> {
        self.0.checked_sub_duration(duration).map(Instant)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed;

pub struct Timeout<F> {
    future: Pin<Box<F>>,
    sleep: Pin<Box<Sleep>>,
}

impl<F: Future> Future for Timeout<F> {
    type Output = Result<F::Output, Elapsed>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Poll::Ready(v) = self.future.as_mut().poll(cx) {
            return Poll::Ready(Ok(v));
        }

        if let Poll::Ready(()) = self.sleep.as_mut().poll(cx) {
            return Poll::Ready(Err(Elapsed));
        }

        Poll::Pending
    }
}

pub fn timeout<F>(duration: Duration, future: F) -> Timeout<F>
where
    F: Future,
{
    Timeout {
        future: Box::pin(future),
        sleep: Box::pin(sleep(duration)),
    }
}
