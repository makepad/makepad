//! Measured first-frame deadlines for runs whose load screen promises a window.
use crate::inspect::snapshot;
use makepad_remote::hot::{Outcome, Record};
use makepad_strict_json::Value;
use std::io::{Read, Seek, Write};
use std::sync::OnceLock;
use std::sync::atomic::Ordering::Acquire;
use std::time::Duration;

pub(crate) static THREAD: OnceLock<std::thread::Thread> = OnceLock::new();

type Sample = (String, String, u64);

fn samples(text: &str) -> Vec<Sample> {
    text.lines().filter_map(|line| {
        let mut words = line.split_whitespace();
        let (package, state, ms) = (words.next()?, words.next()?, words.next()?);
        if !matches!(state, "warm" | "cold") || words.next().is_some() { return None; }
        Some((package.into(), state.into(), ms.parse().ok()?))
    }).collect()
}

fn history() -> std::path::PathBuf { crate::gitstore::relax_home().join("first-frame.txt") }

fn measured() -> std::io::Result<Vec<Sample>> {
    let mut file = match std::fs::File::open(history()) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    file.lock_shared()?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(samples(&text))
}

fn remember(package: &str, state: &str, ms: u64) -> std::io::Result<()> {
    std::fs::create_dir_all(crate::gitstore::relax_home())?;
    // Lock the file through the rewrite so concurrent app launches keep each other's samples.
    let mut file = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(history())?;
    file.lock()?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let mut rows = samples(&text);
    rows.push((package.into(), state.into(), ms));
    let mut counts = std::collections::HashMap::new();
    let mut keep = Vec::new();
    for row in rows.into_iter().rev() {
        let n = counts.entry((row.0.clone(), row.1.clone())).or_insert(0);
        *n += 1;
        if *n <= 9 { keep.push(row); }
    }
    let mut text = String::new();
    for (package, state, ms) in keep.into_iter().rev() { text.push_str(&format!("{package} {state} {ms}\n")); }
    file.rewind()?;
    file.write_all(text.as_bytes())?;
    file.set_len(text.len() as u64)?;
    file.sync_data()
}

/// The deadline of a start in `state` (warm / cold): 3x the median of its measured first frames,
/// at least the state's floor; and its basis in words.
fn deadline(package: &str, state: &str) -> (u64, String) {
    let floor = if state == "warm" { 30u64 } else { 180 };
    let mut runs: Vec<_> = measured().unwrap_or_else(|e| {
        eprintln!("makepad: cannot read first-frame history {}: {e}", history().display());
        Vec::new()
    }).into_iter().filter(|s| s.0 == package && s.1 == state).map(|s| s.2).collect();
    runs.sort_unstable();
    let median = if runs.is_empty() { None } else {
        let (a, b) = (runs[(runs.len() - 1) / 2], runs[runs.len() / 2]);
        Some(a + (b - a) / 2)
    };
    let secs = median.map_or(floor, |ms| ms.saturating_mul(3).saturating_add(999) / 1000).max(floor);
    let basis = match median {
        Some(ms) => format!("{state}: 3x median {:.1} s of {} runs, floor {floor} s", ms as f64 / 1000.0, runs.len()),
        None => format!("{state}: no measured runs, floor {floor} s"),
    };
    (secs, basis)
}

/// A start counted warm (its cache holds a startup manifest) that compiled most of its code
/// fresh instead of linking it: an edit reached most of the program (a field of a type almost
/// every fn touches), or the entries are gone. It runs as a cold start does and is judged as one.
/// (slots that took cached code: placed by the startup image or linked by a lookup; lookups that
/// missed)
fn links() -> (u64, u64) {
    use crate::cache::warm_count::*;
    (get(IMG_PLACED) + get(HIT), get(NO_ENTRY) + get(INVALIDATED) + get(LINK_FAIL))
}

fn cold_like() -> bool {
    let (linked, miss) = links();
    miss > linked
}

pub(crate) fn arm(package: &str) {
    let _kept = crate::zone_alloc::kept();
    let state = if std::env::var("MAKEPAD_RUN_CACHE_STATE").as_deref() == Ok("warm") { "warm" } else { "cold" };
    let (secs, basis) = deadline(package, state);
    if !crate::run::silent() { eprintln!("makepad: first-frame timeout {secs} s ({basis})"); }
    let package = package.to_string();
    if let Err(e) = std::thread::Builder::new().name("first-frame-watch".into()).spawn(move || {
        let _kept = crate::zone_alloc::kept();
        let _ = THREAD.set(std::thread::current());
        watch(&package, state, secs, &basis);
    }) {
        eprintln!("makepad: cannot arm first-frame timeout: {e}");
        crate::run::failed_run(&format!("cannot arm the first-frame timeout: {e}"));
    }
}

fn watch(package: &str, state: &str, secs: u64, basis: &str) {
    let (mut state, mut secs, mut basis) = (state, secs, basis.to_string());
    let mut deadline_ms = secs as f64 * 1000.0;
    let mut wm = false;
    loop {
        if let Some(ms) = snapshot::first_present_ms().filter(|ms| *ms <= deadline_ms) {
            let shell = snapshot::hal_shell();
            wm |= shell.as_ref().is_some_and(|s| s.get("backend").and_then(Value::as_str) == Some("wm"));
            let shown = shell.as_ref().is_some_and(|s| s.get("app_frame_shown").and_then(Value::as_bool) == Some(true));
            if !wm || shown {
                // a warm start that compiled its code fresh is a cold start's sample
                let state = if state == "warm" && cold_like() { "cold" } else { state };
                if let Err(e) = remember(package, state, ms.round() as u64) {
                    eprintln!("makepad: cannot write first-frame history {}: {e}", history().display());
                }
                return;
            }
        }
        let mut left = deadline_ms - crate::clock::ms_since_process_start();
        if left <= 0.0 && state == "warm" && cold_like() {
            // judged by what the start does: it compiles what a warm one links
            let (linked, miss) = links();
            (state, (secs, basis)) = ("cold", deadline(package, "cold"));
            deadline_ms = secs as f64 * 1000.0;
            left = deadline_ms - crate::clock::ms_since_process_start();
            if !crate::run::silent() {
                eprintln!("makepad: first-frame: the warm start took cached code for {linked} slots and missed {miss} lookups (an edit reached most of the program, or its entries are gone): it starts cold, timeout {secs} s ({basis})");
            }
        }
        if left <= 0.0 { break; }
        if crate::jit::first_present_seen() {
            std::thread::sleep(Duration::from_secs_f64((left / 1000.0).min(0.1)));
        } else {
            std::thread::park_timeout(Duration::from_secs_f64(left / 1000.0));
        }
    }
    if snapshot::hal_shell().as_ref().is_some_and(snapshot::all_hidden) {
        eprintln!("makepad: {package} hid its window before its first frame; no first-frame timeout");
        return;
    }
    let presented = crate::jit::first_present_seen();
    let s = snapshot::capture(&format!("first-frame timeout after {secs} s ({basis})"));
    if presented {
        eprintln!("makepad: first-frame timeout: {package} drew, but no frame of it reached the WM in {secs} s ({basis})");
    } else {
        eprintln!("makepad: first-frame timeout: {package} drew no frame in {secs} s ({basis})");
    }
    eprintln!("makepad: likely cause: {}", s.cause);
    eprintln!("makepad: report: {}", s.path.display());
    let reason = format!("{} [report {}]", s.cause, s.path.display());
    persist("ff", Outcome::FirstFrameTimeout, package, &reason);
    // An app that drew keeps running: the WM link is the suspect and ending the app shows nothing.
    // A live session parked on a compile error or a panic waits for the edit or the decision.
    if presented || crate::layout::LIVE_SESSION.load(Acquire) && snapshot::awaiting_decision() { return; }
    crate::run::failed_run(&format!("first-frame timeout: {}", s.cause));
}

fn persist(prefix: &str, outcome: Outcome, package: &str, reason: &str) {
    let id = format!("{prefix}.{}.{pid}", (crate::clock::process_start_epoch() * 1e9) as u64, pid = std::process::id());
    let record = Record {
        load: id.clone(), id, at_ms: (crate::clock::process_start_epoch() * 1000.0 + crate::clock::ms_since_process_start()) as u64,
        app: package.into(), node: String::new(), pid: std::process::id(), outcome, reason: reason.into(),
        failure: String::new(), units: Vec::new(), logs: Vec::new(),
    };
    if let Err(e) = makepad_remote::hot::persist(&record) { eprintln!("makepad: cannot persist {}: {e}", outcome.name()); }
}

pub(crate) fn no_window(package: &str, reason: &str) {
    let _kept = crate::zone_alloc::kept();
    persist("nw", Outcome::NoWindow, package, reason);
}
