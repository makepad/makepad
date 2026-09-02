use makepad_widgets::Cx;
use std::ops::{Add, Sub};
use std::time::Duration;

/// Instant-shaped adapter over Makepad's monotonic platform clock.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub(crate) struct Instant(f64);

impl Instant {
    pub(crate) fn now() -> Self {
        Self(Cx::monotonic_now())
    }

    pub(crate) fn elapsed(self) -> Duration {
        duration(Cx::monotonic_now() - self.0)
    }

    pub(crate) fn duration_since(self, earlier: Self) -> Duration {
        duration(self.0 - earlier.0)
    }

    pub(crate) fn saturating_duration_since(self, earlier: Self) -> Duration {
        self.duration_since(earlier)
    }

    pub(crate) fn checked_sub(self, value: Duration) -> Option<Self> {
        Some(Self(self.0 - value.as_secs_f64()))
    }
}

impl Add<Duration> for Instant {
    type Output = Self;

    fn add(self, value: Duration) -> Self {
        Self(self.0 + value.as_secs_f64())
    }
}

impl Sub<Duration> for Instant {
    type Output = Self;

    fn sub(self, value: Duration) -> Self {
        Self(self.0 - value.as_secs_f64())
    }
}

impl Sub<Instant> for Instant {
    type Output = Duration;

    fn sub(self, earlier: Instant) -> Duration {
        self.duration_since(earlier)
    }
}

fn duration(seconds: f64) -> Duration {
    Duration::from_secs_f64(seconds.max(0.0))
}
