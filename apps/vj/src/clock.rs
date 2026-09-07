use makepad_widgets::Cx;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::ops::{Add, AddAssign, Sub};
use std::time::Duration;

/// Monotonic platform-clock seconds with the arithmetic used by the VJ's
/// media, beat, fade, and profiling paths.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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

impl PartialOrd for Instant {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

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

// ---------------------------------------------------------------------------
// how a time is written down
// ---------------------------------------------------------------------------
//
// Three, and deliberately not one. A playhead, a countdown and a length are
// different questions and want different answers: a tenth matters when you
// are reading where the record is and would only flicker on a countdown,
// and an hours field is noise on anything shorter than an hour but the only
// honest answer above one. What they must NOT be is four private copies of
// the same arithmetic, drifting apart, which is what they were.

/// Where a record is, to a tenth: `1:23.4`.
///
/// The tenth is the point: at a glance it is the digit that says whether the
/// deck is moving.
pub fn playhead(secs: f64) -> String {
    let secs = secs.max(0.0);
    let minutes = (secs / 60.0).floor() as u64;
    format!("{minutes}:{:04.1}", secs - minutes as f64 * 60.0)
}

/// How long until something happens, to the second: `1:23`.
///
/// No tenth: a countdown ticking a tenth at a time is a thing that moves in
/// the corner of the eye and says nothing more for it.
pub fn countdown(secs: f64) -> String {
    let total = secs.max(0.0).round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// How long a thing is, with an hours field only when there is one:
/// `2:03` or `1:02:03`.
pub fn length(secs: f64) -> String {
    let total = secs.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A distance in time, with the sign said out loud: `+1:23` or `-0:04`.
///
/// For a move that has not happened yet, where which WAY is the whole
/// question and a bare number would leave it to be guessed.
pub fn offset(secs: f64) -> String {
    let sign = if secs < 0.0 { '-' } else { '+' };
    let total = secs.abs();
    let minutes = (total / 60.0).floor() as u64;
    format!("{sign}{minutes}:{:04.1}", total - minutes as f64 * 60.0)
}

#[cfg(test)]
mod time_tests {
    use super::*;

    #[test]
    fn each_way_of_writing_a_time_answers_its_own_question() {
        // A playhead keeps its tenth, and pads so the digits do not jump
        // sideways as the seconds roll over.
        assert_eq!(playhead(0.0), "0:00.0");
        assert_eq!(playhead(83.44), "1:23.4");
        assert_eq!(playhead(9.0), "0:09.0");
        assert_eq!(playhead(-5.0), "0:00.0", "before the start is the start");
        // A countdown rounds to the second rather than truncating: a wait
        // of 0.9 seconds is about to happen, not zero.
        assert_eq!(countdown(0.9), "0:01");
        assert_eq!(countdown(83.4), "1:23");
        assert_eq!(countdown(3599.6), "60:00");
        // A length grows an hours field only when it has one.
        assert_eq!(length(596.5), "9:57");
        assert_eq!(length(3601.0), "1:00:01");
        assert_eq!(length(0.0), "0:00");
        // An offset always says which way, including at zero, where the
        // question "which way" still has an answer the operator can see.
        assert_eq!(offset(0.0), "+0:00.0");
        assert_eq!(offset(83.44), "+1:23.4");
        assert_eq!(offset(-4.25), "-0:04.2");
    }
}
