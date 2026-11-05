//! Short words for times and sizes, as the panel prints them.

/// `42 s`, `3m05s`, `1h12m`.
pub fn dur(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s / 60) % 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{s} s")
    }
}

/// `42 s`, `3 min`, `1h12m`: a coarse duration for dense rows.
pub fn dur_short(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s / 60) % 60)
    } else if s >= 60 {
        format!("{} min", s / 60)
    } else {
        format!("{s} s")
    }
}

/// The local zone's offset in seconds (UTC when the platform has none).
pub fn local_offset_s(now_ms: u64) -> i64 {
    makepad_civil_time::local_or_utc((now_ms / 1000) as i64).offset as i64
}

/// `14:05` (`with_s`: `14:05:09`) in the zone `offset_s` from UTC.
pub fn clock(ms: u64, offset_s: i64, with_s: bool) -> String {
    let s = ((ms / 1000) as i64 + offset_s).rem_euclid(86_400);
    if with_s {
        format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 3600, (s / 60) % 60)
    }
}

/// A commit's first `n` characters.
pub fn short(sha: &str, n: usize) -> &str {
    &sha[..sha.len().min(n)]
}
