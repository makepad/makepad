//! Wall-clock labels for the time axis without a timezone crate. Unix
//! targets ask libc's `localtime_r`; anything else prints UTC and says so.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// True when the platform applied a real local zone; false means UTC.
    pub zoned: bool,
}

impl LocalTime {
    pub fn from_epoch_ms(ms: u64) -> Self {
        let secs = (ms / 1000) as i64;
        sys::local(secs).unwrap_or_else(|| Self::utc(secs))
    }

    pub fn utc(secs: i64) -> Self {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400) as u32;
        let (year, month, day) = civil_from_days(days);
        Self {
            year: year as i32,
            month,
            day,
            hour: rem / 3600,
            minute: (rem / 60) % 60,
            second: rem % 60,
            zoned: false,
        }
    }

    pub fn hms(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }

    pub fn hm(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// `2026-09-22 20:29:40`, with the UTC marker last when no zone was available.
    pub fn date_hms(&self) -> String {
        let text = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second);
        if self.zoned { text } else { format!("{text} UTC") }
    }

    /// `2026-09-22 20:29` with a UTC marker when no zone was available.
    pub fn date_hm(&self) -> String {
        let text = format!("{:04}-{:02}-{:02} {:02}:{:02}", self.year, self.month, self.day, self.hour, self.minute);
        if self.zoned { text } else { format!("{text} UTC") }
    }
}

/// Days since 1970-01-01 to (year, month, day); Howard Hinnant's civil_from_days.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(unix)]
mod sys {
    use super::LocalTime;
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

    pub fn local(secs: i64) -> Option<LocalTime> {
        static TZ_INIT: std::sync::Once = std::sync::Once::new();
        // SAFETY: tzset reads the environment once; called before any localtime_r.
        TZ_INIT.call_once(|| unsafe { tzset() });
        let t: c_long = c_long::try_from(secs).ok()?;
        let mut tm = Tm {
            tm_sec: 0, tm_min: 0, tm_hour: 0, tm_mday: 1, tm_mon: 0, tm_year: 70,
            tm_wday: 4, tm_yday: 0, tm_isdst: 0, tm_gmtoff: 0, tm_zone: std::ptr::null(),
        };
        // SAFETY: `localtime_r` writes only into the caller-provided `tm`,
        // whose layout matches the platform prefix declared above.
        let ok = unsafe { !localtime_r(&t, &mut tm).is_null() };
        if !ok {
            return None;
        }
        Some(LocalTime {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1).clamp(1, 12) as u32,
            day: tm.tm_mday.clamp(1, 31) as u32,
            hour: tm.tm_hour.clamp(0, 23) as u32,
            minute: tm.tm_min.clamp(0, 59) as u32,
            second: tm.tm_sec.clamp(0, 60) as u32,
            zoned: true,
        })
    }
}

#[cfg(not(unix))]
mod sys {
    pub fn local(_secs: i64) -> Option<super::LocalTime> {
        None
    }
}
