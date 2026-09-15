//! Wall-clock local time without a timezone database crate. Unix targets
//! (macOS, iOS, Android, Linux) ask libc's `localtime_r`, which applies the
//! platform's own zone rules; other targets fall back to UTC and say so.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1..=12
    pub month: u32,
    /// 1..=31
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// 0 = Sunday .. 6 = Saturday
    pub weekday: u32,
    /// True when the platform applied a real local zone; false means UTC.
    pub zoned: bool,
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

impl LocalTime {
    pub fn now() -> Self {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self::from_epoch(secs)
    }

    pub fn from_epoch(secs: i64) -> Self {
        sys::local(secs).unwrap_or_else(|| Self::utc(secs))
    }

    /// UTC breakdown of an epoch second; the portable fallback.
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
            weekday: ((days + 4).rem_euclid(7)) as u32,
            zoned: false,
        }
    }

    pub fn hms(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }

    pub fn hm(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// "Sat 6 Sep 2026", with a UTC marker when no local zone was available.
    pub fn date_text(&self) -> String {
        let wd = WEEKDAYS[(self.weekday % 7) as usize];
        let mo = MONTHS[((self.month.max(1) - 1) % 12) as usize];
        if self.zoned {
            format!("{} {} {} {}", wd, self.day, mo, self.year)
        } else {
            format!("{} {} {} {} (UTC)", wd, self.day, mo, self.year)
        }
    }

    pub fn weekday_name(&self) -> &'static str {
        WEEKDAYS[(self.weekday % 7) as usize]
    }
}

/// Days since 1970-01-01 to (year, month, day); Howard Hinnant's civil_from_days.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
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
            weekday: tm.tm_wday.clamp(0, 6) as u32,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_breakdown_matches_known_dates() {
        let t = LocalTime::utc(0);
        assert_eq!((t.year, t.month, t.day, t.weekday), (1970, 1, 1, 4));
        // 2026-09-06T12:34:56Z
        let t = LocalTime::utc(1_788_698_096);
        assert_eq!((t.year, t.month, t.day), (2026, 9, 6));
        assert_eq!(t.hms(), "12:34:56");
        assert_eq!(t.weekday_name(), "Sun");
    }

    #[test]
    fn now_is_plausible() {
        let t = LocalTime::now();
        assert!(t.year >= 2024);
        assert!(t.hour < 24 && t.minute < 60 && t.second <= 60);
    }
}
