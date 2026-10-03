//! std::time: Instant (CLOCK_UPTIME_RAW on macOS, CLOCK_MONOTONIC on Linux, as real std),
//! SystemTime (CLOCK_REALTIME), plus core's Duration.

use core::error::Error;
use core::fmt;
use core::ops::{Add, AddAssign, Sub, SubAssign};

pub use core::time::{Duration, TryFromFloatSecsError};

use crate::sys;
use crate::sys::time::Timespec;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(Timespec);

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SystemTime(Timespec);

#[derive(Clone, Debug)]
pub struct SystemTimeError(Duration);

pub const UNIX_EPOCH: SystemTime = SystemTime(Timespec::ZERO);

impl Instant {
    pub fn now() -> Instant {
        Instant(sys::time::now(sys::os::CLOCK_INSTANT))
    }
    pub fn duration_since(&self, earlier: Instant) -> Duration {
        self.saturating_duration_since(earlier)
    }
    pub fn checked_duration_since(&self, earlier: Instant) -> Option<Duration> {
        self.0.sub_timespec(&earlier.0).ok()
    }
    pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
        match self.checked_duration_since(earlier) {
            Some(d) => d,
            None => Duration::ZERO,
        }
    }
    pub fn elapsed(&self) -> Duration {
        Instant::now() - *self
    }
    pub fn checked_add(&self, duration: Duration) -> Option<Instant> {
        match self.0.checked_add(duration) {
            Some(t) => Some(Instant(t)),
            None => None,
        }
    }
    pub fn checked_sub(&self, duration: Duration) -> Option<Instant> {
        match self.0.checked_sub(duration) {
            Some(t) => Some(Instant(t)),
            None => None,
        }
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;
    fn add(self, other: Duration) -> Instant {
        match self.checked_add(other) {
            Some(t) => t,
            None => panic!("overflow when adding duration to instant"),
        }
    }
}
impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, other: Duration) {
        *self = *self + other;
    }
}
impl Sub<Duration> for Instant {
    type Output = Instant;
    fn sub(self, other: Duration) -> Instant {
        match self.checked_sub(other) {
            Some(t) => t,
            None => panic!("overflow when subtracting duration from instant"),
        }
    }
}
impl SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, other: Duration) {
        *self = *self - other;
    }
}
impl Sub<Instant> for Instant {
    type Output = Duration;
    fn sub(self, other: Instant) -> Duration {
        self.duration_since(other)
    }
}

impl fmt::Debug for Instant {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Instant").field("tv_sec", &self.0.secs).field("tv_nsec", &self.0.nanos).finish()
    }
}

impl SystemTime {
    pub const UNIX_EPOCH: SystemTime = UNIX_EPOCH;

    pub fn now() -> SystemTime {
        SystemTime(sys::time::now(sys::os::CLOCK_REALTIME))
    }
    pub fn duration_since(&self, earlier: SystemTime) -> Result<Duration, SystemTimeError> {
        match self.0.sub_timespec(&earlier.0) {
            Ok(d) => Ok(d),
            Err(d) => Err(SystemTimeError(d)),
        }
    }
    pub fn elapsed(&self) -> Result<Duration, SystemTimeError> {
        SystemTime::now().duration_since(*self)
    }
    pub fn checked_add(&self, duration: Duration) -> Option<SystemTime> {
        match self.0.checked_add(duration) {
            Some(t) => Some(SystemTime(t)),
            None => None,
        }
    }
    pub fn checked_sub(&self, duration: Duration) -> Option<SystemTime> {
        match self.0.checked_sub(duration) {
            Some(t) => Some(SystemTime(t)),
            None => None,
        }
    }
    /// From a stat-style (seconds, nanoseconds) pair (fs::Metadata times).
    pub(crate) fn from_parts(secs: i64, nanos: i64) -> SystemTime {
        SystemTime(Timespec { secs, nanos: nanos as u32 })
    }
}

impl Add<Duration> for SystemTime {
    type Output = SystemTime;
    fn add(self, dur: Duration) -> SystemTime {
        match self.checked_add(dur) {
            Some(t) => t,
            None => panic!("overflow when adding duration to `SystemTime`"),
        }
    }
}
impl AddAssign<Duration> for SystemTime {
    fn add_assign(&mut self, other: Duration) {
        *self = *self + other;
    }
}
impl Sub<Duration> for SystemTime {
    type Output = SystemTime;
    fn sub(self, dur: Duration) -> SystemTime {
        match self.checked_sub(dur) {
            Some(t) => t,
            None => panic!("overflow when subtracting duration from `SystemTime`"),
        }
    }
}
impl SubAssign<Duration> for SystemTime {
    fn sub_assign(&mut self, other: Duration) {
        *self = *self - other;
    }
}

impl fmt::Debug for SystemTime {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SystemTime").field("tv_sec", &self.0.secs).field("tv_nsec", &self.0.nanos).finish()
    }
}

impl SystemTimeError {
    pub fn duration(&self) -> Duration {
        self.0
    }
}

impl Error for SystemTimeError {}

impl fmt::Display for SystemTimeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("second time provided was later than self")
    }
}
