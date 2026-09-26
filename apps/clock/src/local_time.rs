//! Wall-clock local time, from the shared host clock in
//! `makepad_civil_time` (the OS's own zone rules on Unix and Windows);
//! other targets fall back to UTC and say so.

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
        Self::from_wall(makepad_civil_time::local_now())
    }

    /// UTC breakdown of an epoch second; the portable fallback.
    pub fn utc(secs: i64) -> Self {
        Self::from_wall(makepad_civil_time::utc(secs))
    }

    fn from_wall(t: makepad_civil_time::LocalDateTime) -> Self {
        Self {
            year: t.year,
            month: t.month,
            day: t.day,
            hour: t.hour,
            minute: t.minute,
            second: t.second,
            // The shared weekday counts from Monday; this one from Sunday.
            weekday: (t.weekday() + 1) % 7,
            zoned: t.zoned,
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
