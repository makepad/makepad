//! One capture for a first-frame timeout and a stalled devhub leg.
use super::{Out, Req};
use makepad_strict_json::Value;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::atomic::Ordering::Acquire;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Snapshot {
    pub cause: String,
    pub path: PathBuf,
}

const COMMANDS: &[&str] = &[
    "why-hung", "session view", "session", "panics", "events", "threads",
    "stacks thread=all depth=48", "waits", "owner", "jit", "jit queue n=30",
    "live", "hal shell", "hal pace", "hal pump", "cache",
];
const WAIT: Duration = Duration::from_secs(2);

fn dispatch(req: &Req, out: &mut Out) {
    if req.cmd == "hal" {
        if !super::host_query("hal", req, out) {
            out.line("(host provider did not answer)");
        }
    } else {
        super::dispatch_command(req, out);
    }
}

fn answer(command: &str) -> String {
    let _kept = crate::zone_alloc::kept();
    let (bytes, _) = makepad_remote::cmd::answer(dispatch, &format!("{command} full=1"));
    if bytes.is_empty() && command.starts_with("hal ") {
        return "(host provider did not answer)\n".into();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

// Owner requests, symbolisation, compile queues and host providers may take locks. Start them
// together: a wedged one cannot delay another section or turn the capture into a serial wait.
fn start(f: impl FnOnce() -> String + Send + 'static) -> (Receiver<String>, Instant) {
    let _kept = crate::zone_alloc::kept();
    let (tx, rx) = mpsc::channel();
    let error = tx.clone();
    let at = Instant::now();
    if let Err(e) = crate::run_threads::spawn("run-capture", move || {
        let _kept = crate::zone_alloc::kept();
        super::server_thread_enter();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
            .unwrap_or_else(|_| "(capture helper panicked)\n".into());
        super::server_thread_exit();
        let _ = tx.send(result);
    }) {
        let _ = error.send(format!("(cannot start capture helper: {e})\n"));
    }
    (rx, at)
}

fn finish((rx, at): (Receiver<String>, Instant)) -> String {
    match rx.recv_timeout(WAIT.saturating_sub(at.elapsed())) {
        Ok(s) => s,
        Err(mpsc::RecvTimeoutError::Timeout) => "(no answer in 2 s)\n".into(),
        Err(mpsc::RecvTimeoutError::Disconnected) => "(capture helper ended without an answer)\n".into(),
    }
}

pub(crate) fn app() -> String {
    std::env::var("MAKEPAD_RUN_PACKAGE").unwrap_or_else(|_| super::APP.lock().unwrap_or_else(|e| e.into_inner()).clone())
}

pub(crate) fn objects(text: &str) -> impl Iterator<Item = Value> + '_ {
    text.lines().filter_map(|s| makepad_strict_json::parse(s.as_bytes()).ok())
}

fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn hal_shell() -> Option<Value> {
    // The HAL publishes a native ledger under a short lock; this never enters the owner or
    // the app. The watchdog can poll it without creating a helper on every 100 ms check.
    objects(&answer("hal shell")).find(|v| v.get("backend").is_some())
}

pub(crate) fn all_hidden(shell: &Value) -> bool {
    shell.get("windows").and_then(Value::as_arr).is_some_and(|windows| {
        !windows.is_empty() && windows.iter().all(|w| w.get("hidden").and_then(Value::as_bool) == Some(true))
    })
}

pub(crate) fn awaiting_decision() -> bool {
    let view = start(|| answer("session view"));
    let panics = start(|| answer("panics"));
    let view = finish(view);
    let panics = finish(panics);
    objects(&view).any(|v| v.get("compile_error_parked").and_then(Value::as_bool) == Some(true))
        || objects(&panics).any(|v| v.get("message").is_some())
}

/// FIRST_PRESENT_NS uses the JIT clock; convert its age to the executable-entry clock.
pub(crate) fn first_present_ms() -> Option<f64> {
    let ns = crate::jit::FIRST_PRESENT_NS.load(Acquire);
    (ns != 0).then(|| (crate::clock::ms_since_process_start()
        - crate::clock::wall_ns().saturating_sub(ns) as f64 / 1e6).max(0.0))
}

fn cause(sections: &[(&str, String)]) -> String {
    // the app's start waits for its boot window, which has not presented (run.rs boot_window_waits);
    // while the load goes on, the cause is the load's
    if let Some((true, note)) = crate::run::boot_window_note() {
        return format!("boot window: {note}");
    }
    let get = |name| sections.iter().find(|s| s.0 == name).map_or("", |s| s.1.as_str());
    for view in objects(get("session view")) {
        if let Some(error) = view.get("compile_errors").and_then(Value::as_arr).and_then(|a| a.first()).and_then(Value::as_str) {
            return format!("compile error: {}", line(error));
        }
    }
    for panic in objects(get("panics")) {
        if let Some(message) = panic.get("message").and_then(Value::as_str) {
            return format!("parked panic: {} at {}", line(message), line(string(&panic, "location")));
        }
    }
    for shell in objects(get("hal shell")) {
        let refused = string(&shell, "refused");
        if string(&shell, "backend") == "incompatible" || !refused.is_empty() {
            return format!("WM cannot host this process: {}", line(if refused.is_empty() { string(&shell, "reason") } else { refused }));
        }
        if string(&shell, "backend") == "wm" && crate::jit::first_present_seen()
            && shell.get("app_frame_shown").and_then(Value::as_bool) == Some(false) {
            let windows = shell.get("windows").and_then(Value::as_arr).unwrap_or(&[]);
            let mut why = Vec::new();
            if string(&shell, "hub") == "broken" { why.push("hub link broken".to_string()); }
            if let Some(ms) = windows.iter().filter_map(|w| w.get("starved_ms").and_then(Value::as_u64)).max() {
                why.push(format!("shared images starved for {ms} ms"));
            }
            if windows.iter().any(|w| w.get("pending_swapchain").and_then(Value::as_bool) == Some(true)) {
                why.push("pending swapchain".into());
            }
            if !string(&shell, "reason").is_empty() { why.push(line(string(&shell, "reason"))); }
            return format!("app drew but no frame reached the WM{}", if why.is_empty() { String::new() } else { format!(": {}", why.join("; ")) });
        }
    }
    for pace in objects(get("hal pace")) {
        if string(&pace, "verdict") == "OWED" { return format!("waiting on the WM: {}", line(string(&pace, "why"))); }
    }
    for hung in objects(get("why-hung")) {
        let verdict = string(&hung, "verdict");
        if !verdict.is_empty() && verdict != "unknown" {
            let finding = hung.get("findings").and_then(Value::as_arr).and_then(|a| a.first());
            return format!("{verdict}: {}", line(finding.map_or_else(|| string(&hung, "why"), |v| string(v, "why"))));
        }
    }
    let main = objects(get("threads")).find(|v| string(v, "role") == "main");
    let main = main.as_ref().map(|v| format!(" ({})", string(v, "id")));
    let mut top = "unavailable".to_string();
    for stack in objects(get("stacks thread=all depth=48")) {
        if !stack.get("threads").and_then(Value::as_arr).is_some_and(|ts| ts.iter().any(|t| {
            t.as_str().is_some_and(|t| main.as_ref().is_some_and(|id| t.ends_with(id)))
        })) { continue; }
        let frames = stack.get("frames").and_then(Value::as_arr).unwrap_or(&[]);
        if let Some(frame) = frames.first() { top = frame_text(frame); }
        for frame in frames {
            let f = super::Frame {
                pc: 0, slot: None, levels: Vec::new(),
                name: string(frame, "fn").into(), at: string(frame, "at").into(),
                module: string(frame, "in").into(), code: String::new(),
            };
            if super::is_program_frame(&f) { return format!("main in {}", frame_text(frame)); }
        }
    }
    format!("no cause found (capture gap): main at {top}")
}

fn frame_text(frame: &Value) -> String {
    let tier = frame.get("code").map(|code| string(code, "tier")).unwrap_or("");
    format!("{}{}{}", string(frame, "fn"),
        if string(frame, "at").is_empty() { String::new() } else { format!(" at {}", string(frame, "at")) },
        if tier.is_empty() { String::new() } else { format!(" ({tier})") })
}

fn line(s: &str) -> String { super::short(&s.lines().next().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" "), 400) }

pub fn capture(why: &str) -> Snapshot {
    let _kept = crate::zone_alloc::kept();
    let app = app();
    let at = stamp(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs());
    let safe_app: String = app.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' }).collect();
    let dir = makepad_remote::hot::home().join("captures");
    let path = dir.join(format!("{at}-{safe_app}-{}.txt", std::process::id()));
    let recording = path.with_extension("mkpf");
    if let Err(e) = std::fs::create_dir_all(&dir) { eprintln!("makepad: cannot create {}: {e}", dir.display()); }
    // Keep every bounded ring, including stall samples. Their naming has a 3 s owner queue
    // wait, so it starts beside the sections and gets a bounded remaining wait after them.
    let rec_path = recording.display().to_string();
    let (rec, _) = start(move || match super::flightrec::save(0, super::flightrec::Why::Asked, Some(&rec_path), true) {
        Ok(_) => String::new(), Err(e) => format!("{e}\n"),
    });
    let pending: Vec<_> = COMMANDS.iter().map(|&cmd| (cmd, start(move || answer(cmd)))).collect();
    let sections: Vec<_> = pending.into_iter().map(|(cmd, pending)| (cmd, finish(pending))).collect();
    let cause = cause(&sections);
    let present = first_present_ms().map_or_else(|| "none".into(), |ms| format!("{ms:.1} ms since process start"));
    let exe = std::env::current_exe().map_or_else(|e| format!("unavailable: {e}"), |p| p.display().to_string());
    let mut report = format!("why: {why}\ncause: {cause}\napp: {app}\npid: {}\nnode: {}\nat: {at}\nuptime_ms: {:.1}\nfirst_present: {}; {present}\ncache: {}\nloader: {}; MAKEPAD_RUN_LOADER={}; provenance={}; codegen_id={:032x}\nflight recording: {}\n",
        std::process::id(), hostname(), crate::clock::ms_since_process_start(), crate::jit::first_present_seen(),
        std::env::var("MAKEPAD_RUN_CACHE_STATE").unwrap_or_default(), exe,
        std::env::var("MAKEPAD_RUN_LOADER").unwrap_or_default(), crate::jit::PROVENANCE, crate::codegen_id::id(), recording.display());
    for (cmd, answer) in sections { report.push_str(&format!("\n== {cmd}\n{answer}")); }
    if let Err(e) = std::fs::write(&path, &report) { eprintln!("makepad: cannot write capture {}: {e}", path.display()); }
    let saved = finish((rec, Instant::now()));
    if !saved.is_empty() {
        use std::io::Write;
        eprintln!("makepad: flight recording {}: {}", recording.display(), saved.trim());
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&path) {
            let _ = writeln!(f, "\n== flight recording\n{saved}");
        }
    }
    prune(&dir);
    Snapshot { cause, path }
}

fn prune(dir: &Path) {
    let Ok(files) = std::fs::read_dir(dir) else { return };
    let mut reports: Vec<_> = files.flatten().map(|f| f.path()).filter(|p| p.extension().is_some_and(|s| s == "txt")).collect();
    reports.sort();
    let old = reports.len().saturating_sub(40);
    for path in reports.into_iter().take(old) {
        let _ = std::fs::remove_file(path.with_extension("mkpf"));
        let _ = std::fs::remove_file(path);
    }
}

/// A copy of `src` (a session's log) in the captures directory, `<stamp>-<app>-<pid>-<what>.log`,
/// where it outlives the run's temporary directory (a devhub lease's, deleted when the lease
/// ends); the newest 20 of each kind are kept. (The terminal session's log: unix only.)
#[cfg(unix)]
pub(crate) fn keep_file(src: &Path, what: &str) -> std::io::Result<PathBuf> {
    let dir = makepad_remote::hot::home().join("captures");
    std::fs::create_dir_all(&dir)?;
    let at = stamp(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs());
    let safe_app: String = app().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' }).collect();
    let suffix = format!("-{what}.log");
    let path = dir.join(format!("{at}-{safe_app}-{}{suffix}", std::process::id()));
    std::fs::copy(src, &path)?;
    if let Ok(files) = std::fs::read_dir(&dir) {
        let mut kept: Vec<_> = files.flatten().map(|f| f.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(&suffix))).collect();
        kept.sort();
        let old = kept.len().saturating_sub(20);
        for p in kept.into_iter().take(old) {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(path)
}

#[cfg(unix)]
fn hostname() -> String {
    extern "C" { fn gethostname(name: *mut u8, len: usize) -> i32; }
    let mut name = [0u8; 256];
    if unsafe { gethostname(name.as_mut_ptr(), name.len()) } != 0 { return "unavailable".into(); }
    String::from_utf8_lossy(&name[..name.iter().position(|b| *b == 0).unwrap_or(name.len())]).into_owned()
}

#[cfg(not(unix))]
fn hostname() -> String { std::env::var("COMPUTERNAME").unwrap_or_default() }

fn stamp(secs: u64) -> String {
    // Civil date from Unix days, as in flightrec's recording names.
    let z = (secs / 86400) as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z", secs % 86400 / 3600, secs % 3600 / 60, secs % 60)
}
