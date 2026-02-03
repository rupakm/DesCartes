use std::future::Future;
use std::ops::{Add, Sub};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use descartes_core::async_runtime::{self, SimSleep};
use descartes_core::SimTime;

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
            "descartes_tokio::time::Instant::now called outside scheduler context. \
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
            .expect("descartes_tokio::time::Instant::duration_since: earlier is later than self")
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

/// Tokio-like sleep future (backed by `descartes_core::async_runtime::SimSleep`).
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

/// Defines the behavior of an [`Interval`] when it misses a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissedTickBehavior {
    /// Ticks as fast as possible until caught up.
    Burst,
    /// Schedule the next tick relative to the current time.
    Delay,
    /// Skip missed ticks and tick on the next multiple of `period` from the original schedule.
    Skip,
}

impl Default for MissedTickBehavior {
    fn default() -> Self {
        Self::Burst
    }
}

impl MissedTickBehavior {
    fn next_timeout(&self, timeout: Instant, now: Instant, period: Duration) -> Instant {
        match self {
            Self::Burst => timeout + period,
            Self::Delay => now + period,
            Self::Skip => {
                let period_nanos = u128::from(u64::try_from(period.as_nanos()).expect(
                    "interval period too large for descartes_tokio simulated time (must fit u64 nanos)",
                ));

                let delta_nanos = (now - timeout).as_nanos();
                let rem = delta_nanos % period_nanos;
                let rem_u64 = u64::try_from(rem)
                    .expect("too much time has elapsed since the interval was supposed to tick");

                now + period - Duration::from_nanos(rem_u64)
            }
        }
    }
}

/// Creates a new [`Interval`] that yields with interval of `period`.
///
/// The first tick completes immediately.
///
/// This is equivalent to `interval_at(Instant::now(), period)`.
///
/// # Panics
///
/// Panics if `period` is zero.
pub fn interval(period: Duration) -> Interval {
    interval_at(Instant::now(), period)
}

/// Creates a new [`Interval`] that yields with interval of `period` with the first
/// tick completing at `start`.
///
/// # Panics
///
/// Panics if `period` is zero.
pub fn interval_at(start: Instant, period: Duration) -> Interval {
    assert!(period > Duration::ZERO, "interval period must be non-zero");
    assert!(
        u64::try_from(period.as_nanos()).is_ok(),
        "interval period too large for descartes_tokio simulated time (must fit u64 nanos)"
    );

    Interval {
        next: start,
        period,
        missed_tick_behavior: MissedTickBehavior::default(),
    }
}

/// Interval returned by [`interval`] and [`interval_at`].
#[derive(Debug)]
pub struct Interval {
    next: Instant,
    period: Duration,
    missed_tick_behavior: MissedTickBehavior,
}

impl Interval {
    /// Completes when the next instant in the interval has been reached.
    pub async fn tick(&mut self) -> Instant {
        let timeout = self.next;
        let now = Instant::now();

        if now < timeout {
            sleep_until(timeout).await;
        }

        let now = Instant::now();
        let next = if now > timeout {
            self.missed_tick_behavior
                .next_timeout(timeout, now, self.period)
        } else {
            timeout + self.period
        };

        self.next = next;
        timeout
    }

    pub fn missed_tick_behavior(&self) -> MissedTickBehavior {
        self.missed_tick_behavior
    }

    pub fn set_missed_tick_behavior(&mut self, behavior: MissedTickBehavior) {
        self.missed_tick_behavior = behavior;
    }

    pub fn period(&self) -> Duration {
        self.period
    }

    /// Resets the interval to complete one period after the current time.
    ///
    /// This method ignores [`MissedTickBehavior`].
    pub fn reset(&mut self) {
        self.next = Instant::now() + self.period;
    }

    /// Resets the interval immediately.
    ///
    /// This method ignores [`MissedTickBehavior`].
    pub fn reset_immediately(&mut self) {
        self.next = Instant::now();
    }

    /// Resets the interval after `after`.
    ///
    /// This method ignores [`MissedTickBehavior`].
    pub fn reset_after(&mut self, after: Duration) {
        self.next = Instant::now() + after;
    }

    /// Resets the interval to the specified deadline.
    pub fn reset_at(&mut self, deadline: Instant) {
        self.next = deadline;
    }
}
