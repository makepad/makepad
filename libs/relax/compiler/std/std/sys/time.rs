//! A clock reading (seconds + nanoseconds, signed seconds) shared by Instant and SystemTime.

use core::time::Duration;

/// A normalized fractional second, matching the niche used by Duration.
#[cfg(not(target_family = "wasm"))]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
#[cfg_attr(not(relax_check), rustc_layout_scalar_valid_range_end(999_999_999))]
struct Nanoseconds(u32);

#[cfg(not(target_family = "wasm"))]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timespec {
    pub secs: i64,
    nanos: Nanoseconds,
}

#[cfg(not(target_family = "wasm"))]
const NSEC_PER_SEC: u32 = 1_000_000_000;

/// The web targets: rustc's sys/time/unsupported.rs `Instant` / `SystemTime`, a Duration since
/// the epoch (nothing can read a clock, so a value comes from `UNIX_EPOCH` and arithmetic; a
/// time before the epoch does not exist).
#[cfg(target_family = "wasm")]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timespec(Duration);

#[cfg(target_family = "wasm")]
impl Timespec {
    pub const ZERO: Timespec = Timespec(Duration::from_secs(0));

    pub fn duration(&self) -> Duration {
        self.0
    }

    pub fn checked_add(&self, d: Duration) -> Option<Timespec> {
        Some(Timespec(self.0.checked_add(d)?))
    }

    pub fn checked_sub(&self, d: Duration) -> Option<Timespec> {
        Some(Timespec(self.0.checked_sub(d)?))
    }

    /// `self - other` as a Duration: Ok if self >= other, else Err(other - self).
    pub fn sub_timespec(&self, other: &Timespec) -> Result<Duration, Duration> {
        self.0.checked_sub(other.0).ok_or_else(|| other.0 - self.0)
    }
}

#[cfg(any(windows, target_family = "wasm"))]
pub use super::os::{now, sleep};

#[cfg(unix)]
pub fn now(clock: super::os::clockid_t) -> Timespec {
    let mut t = super::timespec { tv_sec: 0, tv_nsec: 0 };
    let r = unsafe { super::clock_gettime(clock, &mut t as *mut super::timespec) };
    if r != 0 {
        panic!("clock_gettime failed: {}", super::error_string(super::errno()));
    }
    Timespec::new(t.tv_sec, t.tv_nsec as i64)
}

#[cfg(not(target_family = "wasm"))]
impl Timespec {
    pub const ZERO: Timespec = Timespec::new(0, 0);

    /// Checks OS nanoseconds before narrowing or storing the restricted scalar.
    pub const fn new(secs: i64, nanos: i64) -> Timespec {
        assert!(nanos >= 0 && nanos < NSEC_PER_SEC as i64, "invalid timespec nanoseconds");
        Timespec { secs, nanos: unsafe { Nanoseconds(nanos as u32) } }
    }

    pub const fn nanos(&self) -> u32 {
        self.nanos.0
    }

    pub fn checked_add(&self, d: Duration) -> Option<Timespec> {
        if d.as_secs() > i64::MAX as u64 {
            return None;
        }
        let mut secs = self.secs.checked_add(d.as_secs() as i64)?;
        let mut nanos = self.nanos() + d.subsec_nanos();
        if nanos >= NSEC_PER_SEC {
            nanos -= NSEC_PER_SEC;
            secs = secs.checked_add(1)?;
        }
        Some(Timespec::new(secs, nanos as i64))
    }

    pub fn checked_sub(&self, d: Duration) -> Option<Timespec> {
        if d.as_secs() > i64::MAX as u64 {
            return None;
        }
        let mut secs = self.secs.checked_sub(d.as_secs() as i64)?;
        let nanos = if self.nanos() >= d.subsec_nanos() {
            self.nanos() - d.subsec_nanos()
        } else {
            secs = secs.checked_sub(1)?;
            self.nanos() + NSEC_PER_SEC - d.subsec_nanos()
        };
        Some(Timespec::new(secs, nanos as i64))
    }

    /// `self - other` as a Duration: Ok if self >= other, else Err(other - self).
    pub fn sub_timespec(&self, other: &Timespec) -> Result<Duration, Duration> {
        if self >= other {
            let (secs, nanos) = if self.nanos() >= other.nanos() {
                ((self.secs as i128 - other.secs as i128) as u64, self.nanos() - other.nanos())
            } else {
                ((self.secs as i128 - other.secs as i128 - 1) as u64, self.nanos() + NSEC_PER_SEC - other.nanos())
            };
            Ok(Duration::new(secs, nanos))
        } else {
            match other.sub_timespec(self) {
                Ok(d) => Err(d),
                Err(d) => Ok(d),
            }
        }
    }
}

/// Sleeps for `d`, resuming after signals (nanosleep with the remainder).
#[cfg(unix)]
pub fn sleep(d: Duration) {
    let mut secs = d.as_secs();
    let mut nsecs = d.subsec_nanos() as i64;
    while secs > 0 || nsecs > 0 {
        let chunk = if secs > i64::MAX as u64 { i64::MAX } else { secs as i64 };
        let req = super::timespec { tv_sec: chunk, tv_nsec: nsecs as core::ffi::c_long };
        let mut rem = super::timespec { tv_sec: 0, tv_nsec: 0 };
        secs -= chunk as u64;
        let r = unsafe { super::nanosleep(&req as *const super::timespec, &mut rem as *mut super::timespec) };
        if r == -1 {
            if super::errno() != super::os::EINTR {
                panic!("nanosleep failed");
            }
            secs += rem.tv_sec as u64;
            nsecs = rem.tv_nsec as i64;
        } else {
            nsecs = 0;
        }
    }
}
