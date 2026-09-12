//! Disk-based drive history (route.md follow-up; user directive 2026-07-30).
//!
//! Every app session with GPS movement becomes one append-only JSONL drive
//! record under `local/route_history/`: a header line, `trip` events each
//! time the planned trip changes, one line per accepted GPS fix, and an
//! `end` footer. The header reserves `media` (future timelapse video
//! attachments) and `synced` (future server sync) so the format survives
//! those extensions without migration.

use crate::trip::haversine_m;
use makepad_widgets::makepad_micro_serde::*;
use makepad_widgets::{Cx, LocationUpdateEvent};
use std::io::Write;
use std::path::PathBuf;

const HISTORY_DIR: &str = "local/route_history";
/// Ignore jitter below this move distance.
const MIN_MOVE_M: f64 = 3.0;

// The record lines. Each carries its `event` tag first, so a reader can
// dispatch on it before it knows the shape.

#[derive(SerJson)]
struct StartRecord {
    event: String,
    v: u32,
    started_unix: f64,
    /// Future: timelapse video refs.
    media: Vec<String>,
    /// Future: server sync state.
    synced: bool,
}

#[derive(SerJson)]
struct TripRecord {
    event: String,
    t: f64,
    digest: String,
}

#[derive(SerJson)]
struct FixRecord {
    event: String,
    t: f64,
    lon: f64,
    lat: f64,
    acc: f64,
    speed: Option<f64>,
    heading: Option<f64>,
}

#[derive(SerJson)]
struct EndRecord {
    event: String,
    t: f64,
    distance_m: f64,
    samples: usize,
}

#[derive(Default)]
pub struct DriveLog {
    file: Option<std::fs::File>,
    last: Option<(f64, f64)>,
    pub distance_m: f64,
    pub samples: usize,
    closed: bool,
}

fn now_unix() -> f64 {
    Cx::time_now().max(0.0)
}

impl DriveLog {
    fn ensure_file(&mut self) -> Option<&mut std::fs::File> {
        if self.file.is_none() && !self.closed {
            std::fs::create_dir_all(HISTORY_DIR).ok()?;
            let path = PathBuf::from(HISTORY_DIR)
                .join(format!("drive-{}.jsonl", now_unix() as u64));
            let mut file = std::fs::File::create(&path).ok()?;
            let header = StartRecord {
                event: "start".into(),
                v: 1,
                started_unix: now_unix(),
                media: Vec::new(),
                synced: false,
            };
            writeln!(file, "{}", header.serialize_json()).ok()?;
            self.file = Some(file);
        }
        self.file.as_mut()
    }

    /// Record a trip (re)plan so the drive record knows what was navigated.
    pub fn log_trip(&mut self, digest: &str) {
        let line = TripRecord {
            event: "trip".into(),
            t: now_unix(),
            digest: digest.to_string(),
        }
        .serialize_json();
        if let Some(file) = self.ensure_file() {
            let _ = writeln!(file, "{line}");
        }
    }

    /// Record a GPS fix (deduped below MIN_MOVE_M).
    pub fn log_fix(&mut self, fix: &LocationUpdateEvent) {
        if let Some(last) = self.last {
            let moved = haversine_m(last, (fix.lon, fix.lat));
            if moved < MIN_MOVE_M {
                return;
            }
            self.distance_m += moved;
        }
        self.last = Some((fix.lon, fix.lat));
        self.samples += 1;
        let line = FixRecord {
            event: "fix".into(),
            t: fix.time,
            lon: fix.lon,
            lat: fix.lat,
            acc: fix.accuracy_m,
            speed: fix.speed_mps,
            heading: fix.heading_deg,
        }
        .serialize_json();
        if let Some(file) = self.ensure_file() {
            let _ = writeln!(file, "{line}");
        }
    }

    /// Finalize the record (app shutdown).
    pub fn close(&mut self) {
        if let Some(mut file) = self.file.take() {
            let footer = EndRecord {
                event: "end".into(),
                t: now_unix(),
                distance_m: self.distance_m,
                samples: self.samples,
            }
            .serialize_json();
            let _ = writeln!(file, "{footer}");
        }
        self.closed = true;
    }
}

/// Digest of recent drive records for the `trip_history` tool.
pub fn list_drives(limit: usize) -> String {
    let Ok(dir) = std::fs::read_dir(HISTORY_DIR) else {
        return "no drive history yet".into();
    };
    let mut names: Vec<String> = dir
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("drive-") && n.ends_with(".jsonl"))
        .collect();
    if names.is_empty() {
        return "no drive history yet".into();
    }
    names.sort();
    names.reverse();
    names.truncate(limit);
    let mut out = String::new();
    for name in &names {
        let path = PathBuf::from(HISTORY_DIR).join(name);
        let mut date = String::from("?");
        let mut km = String::from("in progress");
        let mut trip = String::new();
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                let Ok(v) = JsonValue::deserialize_json(line) else {
                    continue;
                };
                match v.get("event").and_then(|e| e.as_str()) {
                    Some("start") => {
                        if let Some(t) = v.get("started_unix").and_then(|t| t.as_f64()) {
                            date = format_date(t);
                        }
                    }
                    Some("trip") => {
                        if let Some(d) = v.get("digest").and_then(|d| d.as_str()) {
                            // first + last stop names from the digest
                            trip = d
                                .lines()
                                .filter(|l| l.starts_with("stop_"))
                                .filter_map(|l| l.split(": ").nth(1))
                                .filter_map(|l| l.split(" (").next())
                                .collect::<Vec<_>>()
                                .join(" → ");
                        }
                    }
                    Some("end") => {
                        if let Some(d) = v.get("distance_m").and_then(|d| d.as_f64()) {
                            km = format!("{:.1} km", d / 1000.0);
                        }
                    }
                    _ => {}
                }
            }
        }
        out.push_str(&format!(
            "{name} | {date} | {km}{}\n",
            if trip.is_empty() { String::new() } else { format!(" | {trip}") }
        ));
    }
    out
}

/// Unix seconds → "YYYY-MM-DD HH:MM" (UTC), no chrono dependency.
fn format_date(unix: f64) -> String {
    let secs = unix as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // civil-from-days (Howard Hinnant's algorithm)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60
    )
}
