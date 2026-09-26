//! The host's wall clock: an epoch second broken down in the local zone,
//! by the operating system's own zone rules, without a timezone database.
//! Unix targets (macOS, iOS, Android, Linux) ask libc's `localtime_r`;
//! Windows asks `SystemTimeToTzSpecificLocalTime`. Elsewhere there is no
//! local zone and [`local`] returns `None`.

use crate::{from_ymd, to_ymd, weekday, Day};

/// An instant as a wall clock shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalDateTime {
    pub year: i32,
    /// 1–12.
    pub month: u32,
    /// 1–31.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// Seconds east of UTC at this instant, daylight saving applied.
    pub offset: i32,
    /// True when the host's local zone was applied; false means UTC.
    pub zoned: bool,
}

impl LocalDateTime {
    /// The civil day number.
    pub fn day_number(&self) -> Day {
        from_ymd(self.year, self.month, self.day)
    }

    /// 0 = Monday, matching [`weekday`].
    pub fn weekday(&self) -> u32 {
        weekday(self.day_number())
    }
}

/// Seconds since the Unix epoch, now.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `secs` in the host's local zone, or `None` where the platform has none.
pub fn local(secs: i64) -> Option<LocalDateTime> {
    let (year, month, day, hour, minute, second) = sys::local(secs)?;
    let wall = from_ymd(year, month, day) as i64 * 86_400 + (hour * 3600 + minute * 60 + second) as i64;
    Some(LocalDateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        offset: (wall - secs) as i32,
        zoned: true,
    })
}

/// `secs` in UTC.
pub fn utc(secs: i64) -> LocalDateTime {
    let rem = secs.rem_euclid(86_400) as u32;
    let (year, month, day) = to_ymd(secs.div_euclid(86_400) as Day);
    LocalDateTime {
        year,
        month,
        day,
        hour: rem / 3600,
        minute: (rem / 60) % 60,
        second: rem % 60,
        offset: 0,
        zoned: false,
    }
}

/// `secs` in the local zone, falling back to UTC (`zoned == false`).
pub fn local_or_utc(secs: i64) -> LocalDateTime {
    local(secs).unwrap_or_else(|| utc(secs))
}

/// Now in the local zone, falling back to UTC (`zoned == false`).
pub fn local_now() -> LocalDateTime {
    local_or_utc(now_secs())
}

/// (year, month, day, hour, minute, second) on the local wall clock.
type Wall = (i32, u32, u32, u32, u32, u32);

#[cfg(unix)]
mod sys {
    use super::Wall;
    use std::os::raw::{c_char, c_int, c_long};

    /// The common prefix of `struct tm` on Darwin, glibc, musl and bionic.
    #[repr(C)]
    struct Tm {
        tm_sec: c_int,
        tm_min: c_int,
        tm_hour: c_int,
        tm_mday: c_int,
        tm_mon: c_int,
        tm_year: c_int,
        tm_wday: c_int,
        tm_yday: c_int,
        tm_isdst: c_int,
        tm_gmtoff: c_long,
        tm_zone: *const c_char,
    }

    extern "C" {
        fn tzset();
        fn localtime_r(time: *const c_long, out: *mut Tm) -> *mut Tm;
    }

    pub fn local(secs: i64) -> Option<Wall> {
        static TZ_INIT: std::sync::Once = std::sync::Once::new();
        // SAFETY: tzset reads the zone once, before any localtime_r.
        TZ_INIT.call_once(|| unsafe { tzset() });
        let t: c_long = c_long::try_from(secs).ok()?;
        let mut tm = Tm {
            tm_sec: 0, tm_min: 0, tm_hour: 0, tm_mday: 1, tm_mon: 0, tm_year: 70,
            tm_wday: 4, tm_yday: 0, tm_isdst: 0, tm_gmtoff: 0, tm_zone: std::ptr::null(),
        };
        // SAFETY: `localtime_r` writes only into the caller-provided `tm`,
        // whose layout matches the platform prefix declared above.
        if unsafe { localtime_r(&t, &mut tm) }.is_null() {
            return None;
        }
        Some((
            tm.tm_year + 1900,
            (tm.tm_mon + 1).clamp(1, 12) as u32,
            tm.tm_mday.clamp(1, 31) as u32,
            tm.tm_hour.clamp(0, 23) as u32,
            tm.tm_min.clamp(0, 59) as u32,
            tm.tm_sec.clamp(0, 59) as u32,
        ))
    }
}

#[cfg(windows)]
mod sys {
    use super::Wall;

    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FileTimeToSystemTime(file_time: *const u64, out: *mut SystemTime) -> i32;
        fn SystemTimeToTzSpecificLocalTime(zone: *const u8, utc: *const SystemTime, out: *mut SystemTime) -> i32;
    }

    /// Seconds from 1601-01-01 (the FILETIME epoch) to 1970-01-01.
    const FILETIME_UNIX_OFFSET: i64 = 11_644_473_600;

    pub fn local(secs: i64) -> Option<Wall> {
        let ticks = u64::try_from(secs.checked_add(FILETIME_UNIX_OFFSET)?.checked_mul(10_000_000)?).ok()?;
        let mut utc = SystemTime::default();
        let mut wall = SystemTime::default();
        // SAFETY: both calls read the value given and write only the
        // SYSTEMTIME passed; a FILETIME is one little-endian u64. A null
        // zone means the zone the system is set to, with its DST rules.
        let ok = unsafe {
            FileTimeToSystemTime(&ticks, &mut utc) != 0
                && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut wall) != 0
        };
        if !ok {
            return None;
        }
        Some((
            wall.year as i32,
            (wall.month as u32).clamp(1, 12),
            (wall.day as u32).clamp(1, 31),
            (wall.hour as u32).min(23),
            (wall.minute as u32).min(59),
            (wall.second as u32).min(59),
        ))
    }
}

#[cfg(not(any(unix, windows)))]
mod sys {
    pub fn local(_secs: i64) -> Option<super::Wall> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_breakdown() {
        let t = utc(1_788_698_096); // 2026-09-06T12:34:56Z, a Sunday
        assert_eq!((t.year, t.month, t.day, t.hour, t.minute, t.second), (2026, 9, 6, 12, 34, 56));
        assert_eq!(t.weekday(), 6);
        assert!(!t.zoned);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn local_is_zoned_and_offset_round_trips() {
        let secs = 1_788_698_096;
        let t = local(secs).expect("host has a local zone");
        assert!(t.zoned);
        let wall = t.day_number() as i64 * 86_400 + (t.hour * 3600 + t.minute * 60 + t.second) as i64;
        assert_eq!(wall - t.offset as i64, secs);
        assert!(t.offset.abs() <= 14 * 3600);
    }
}
