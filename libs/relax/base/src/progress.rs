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

/// `--silent` (the run's command line): nothing of Relax's own on stderr but errors (a program
/// re-running itself, a binary's wrapper script, whose stderr is the program's). The one switch
/// every layer reads; the run sets it.
static SILENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_silent() {
    SILENT.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn silent() -> bool {
    SILENT.load(std::sync::atomic::Ordering::Relaxed)
}

/// The session log's writer for a line the terminal does not show (the run's `tui::log_only`),
/// set by the run when it starts its session log.
static LOG_ONLY: std::sync::OnceLock<fn(&str)> = std::sync::OnceLock::new();

pub fn set_log_only(f: fn(&str)) {
    let _ = LOG_ONLY.set(f);
}

/// A line of Relax's own about the run (what it waits on, what it was given): on stderr, which
/// the session log takes in, unless the run is silent; then in the session log alone. Either way
/// the log has it.
pub fn say(line: &str) {
    if !silent() {
        eprintln!("{line}");
    } else if let Some(log_only) = LOG_ONLY.get() {
        log_only(line);
    }
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
            Some(t) if t > 0 => eprintln!("relax: {label}: {} %", done.min(t) * 100 / t),
            _ => eprintln!("relax: {label}: {} MB", done >> 20),
        }
    }
}

/// Something the person should know about the launch (it runs a stored commit because the
/// remote could not be reached): the launcher's report as `note`, and a stderr line.
pub fn note(text: &str) {
    let text: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    eprintln!("relax: {text}");
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

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&LAST, Mutex::new(None));
}
