use makepad_widgets::Cx;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::ops::{Add, AddAssign, Sub};
use std::time::Duration;

/// Monotonic platform-clock seconds with the arithmetic used by the VJ's
/// media, beat, fade, and profiling paths.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Instant(f64);

impl Instant {
    pub fn now() -> Self {
        Self(Cx::monotonic_now())
    }

    pub fn elapsed(self) -> Duration {
        Self::now().duration_since(self)
    }

    pub fn duration_since(self, earlier: Self) -> Duration {
        Duration::from_secs_f64((self.0 - earlier.0).max(0.0))
    }

    pub fn saturating_duration_since(self, earlier: Self) -> Duration {
        self.duration_since(earlier)
    }

    pub fn checked_sub(self, duration: Duration) -> Option<Self> {
        let seconds = duration.as_secs_f64();
        (seconds <= self.0).then(|| Self(self.0 - seconds))
    }
}

impl Eq for Instant {}

impl Ord for Instant {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Hash for Instant {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl Add<Duration> for Instant {
    type Output = Self;

    fn add(self, duration: Duration) -> Self {
        Self(self.0 + duration.as_secs_f64())
    }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, duration: Duration) {
        self.0 += duration.as_secs_f64();
    }
}

impl Sub<Duration> for Instant {
    type Output = Self;

    fn sub(self, duration: Duration) -> Self {
        Self(self.0 - duration.as_secs_f64())
    }
}

impl Sub for Instant {
    type Output = Duration;

    fn sub(self, earlier: Self) -> Duration {
        self.duration_since(earlier)
    }
}
