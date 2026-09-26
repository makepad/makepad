//! Wall-clock labels for the time axis, from the shared host clock in
//! `makepad_civil_time`; where there is no local zone it prints UTC and says so.

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
        let t = makepad_civil_time::local_or_utc((ms / 1000) as i64);
        Self { year: t.year, month: t.month, day: t.day, hour: t.hour, minute: t.minute, second: t.second, zoned: t.zoned }
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
