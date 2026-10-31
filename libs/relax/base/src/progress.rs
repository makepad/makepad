//! Progress of what a Relax launch waits on: the loader's git store fetches. With a launcher's
//! report (`--report-to`, the runner) it goes there as `progress <done> <total|-> <label>`;
//! in a terminal it is a short stderr line. Throttled here at the source: a label reports when
//! its whole percent changes (or, without a total, every 4 MB), at most every 250 ms, and always
//! at the end.
use std::collections::HashMap;
use relax_session::sync::Mutex;
use std::time::Instant;

struct Last {
    step: u64,
    at: Instant,
}

static LAST: Mutex<Option<HashMap<String, Last>>> = Mutex::new(None);

/// The launcher's report (`--report-to`, the run's `report(kind, text)`), set by the run when it
/// has one.
static REPORT: std::sync::OnceLock<fn(&str, &str)> = std::sync::OnceLock::new();

pub fn set_report(f: fn(&str, &str)) {
    let _ = REPORT.set(f);
}

fn step_of(done: u64, total: Option<u64>) -> u64 {
    match total {
        Some(t) if t > 0 => done.min(t) * 100 / t,
        _ => done >> 22,
    }
}

/// Whether this report should go out (the throttle).
fn due(label: &str, done: u64, total: Option<u64>, min_ms: u128) -> bool {
    let finished = total.is_some_and(|t| done >= t);
    let step = step_of(done, total);
    let mut g = LAST.lock().unwrap();
    let map = g.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    match map.get_mut(label) {
        Some(l) => {
            if !finished && (l.step == step || now.duration_since(l.at).as_millis() < min_ms) {
                return false;
            }
            l.step = step;
            l.at = now;
        }
        None => {
            map.insert(label.to_string(), Last { step, at: now });
        }
    }
    if finished {
        map.remove(label);
    }
    true
}

fn total_text(total: Option<u64>) -> String {
    total.map(|t| t.to_string()).unwrap_or_else(|| "-".to_string())
}

/// Reports `done` of `total` bytes (or units) for `label` ("makepad from host").
pub fn progress(label: &str, done: u64, total: Option<u64>) {
    let label: String = label.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let report = REPORT.get().copied();
    if !due(&label, done, total, if report.is_some() { 250 } else { 500 }) {
        return;
    }
    if let Some(report) = report {
        report("progress", &format!("{done} {} {label}", total_text(total)));
    } else {
        match total {
            Some(t) if t > 0 => eprintln!("makepad: {label}: {} %", done.min(t) * 100 / t),
            _ => eprintln!("makepad: {label}: {} MB", done >> 20),
        }
    }
}

/// Something the person should know about the launch (it runs a stored commit because the
/// remote could not be reached): the launcher's report as `note`, and a stderr line.
pub fn note(text: &str) {
    let text: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    eprintln!("makepad: {text}");
    if let Some(report) = REPORT.get().copied() {
        report("note", &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_throttle_passes_percent_steps_and_the_end() {
        assert!(due("t1", 0, Some(1000), 0));
        assert!(!due("t1", 5, Some(1000), 0)); // same percent
        assert!(due("t1", 20, Some(1000), 0));
        assert!(due("t1", 1000, Some(1000), 100_000)); // the end always
    }
}
