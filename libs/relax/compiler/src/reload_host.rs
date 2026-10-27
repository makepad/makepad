//! Changes a running process cannot take in place: the provided crates built into makepad, std,
//! makepad run's own sources, the Cargo graph and build scripts (a program edit the patcher
//! cannot absorb restarts in place without a rebuild: reload.rs restart_in_place). The loader is
//! rebuilt with cargo in
//! the background (nice) while the app keeps running the code it has, then the process truly
//! restarts: the rebuilt binary replaces it (exec) with the same command line, and an app that
//! restarts with its state (std::relax::restart_supported) is asked first, at its next safe point
//! (it may postpone for unsaved work), and gets its state back in the new process. A failed
//! rebuild keeps the running app and says why; the next such change builds again.
//!
//! One rebuild at a time; a change during it builds once more after it. The new binary is the
//! one cargo leaves at the loader's own path. Its codegen identity (codegen_id.rs) keys every cache
//! it uses: a rebuild that changed what generates code starts from no stored code; one that changed
//! only the rest of the binary (devhub, platform glue) keeps every stored build.
use relax_session::sync::Mutex;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The cargo package of the running loader and its manifest (tools/run for `makepad`,
/// tools/run/runner for `relax`): set by the runner at startup.
static PACKAGE: Mutex<Option<(&'static str, &'static str)>> = Mutex::new(None);

pub fn set_loader_package(package: &'static str, manifest: &'static str) {
    *PACKAGE.lock().unwrap_or_else(|e| e.into_inner()) = Some((package, manifest));
}

/// The loader binary (MAKEPAD_RUN_LOADER, set by the runner; else this executable), as it was
/// at startup: cargo replaces the file, and Linux's current_exe then names a deleted inode.
pub(crate) fn loader() -> Option<PathBuf> {
    static LOADER: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    LOADER.get_or_init(|| {
        let _k = crate::zone_alloc::kept();
        std::env::var_os("MAKEPAD_RUN_LOADER").map(PathBuf::from).or_else(|| std::env::current_exe().ok())
    }).clone()
}

/// The cargo target directory the loader was built into (cargo keeps `.rustc_info.json` at its
/// root, and `.fingerprint` in each profile directory), and the `--target` triple between it and
/// the profile directory, if any.
fn layout(exe: &Path) -> Option<(PathBuf, String, Option<String>)> {
    let profile_dir = exe.parent()?;
    if !profile_dir.join(".fingerprint").is_dir() { return None; }
    let profile = profile_dir.file_name()?.to_string_lossy().into_owned();
    let root = |d: &Path| d.join(".rustc_info.json").is_file() || d.join("CACHEDIR.TAG").is_file();
    let up = profile_dir.parent()?;
    if root(up) {
        return Some((up.to_path_buf(), profile, None));
    }
    let root_dir = up.parent()?;
    if !root(root_dir) { return None; }
    let root = root_dir;
    Some((root.to_path_buf(), profile, Some(up.file_name()?.to_string_lossy().into_owned())))
}

pub(crate) fn target_dir(exe: &Path) -> Option<PathBuf> {
    layout(exe).map(|l| l.0)
}

/// The cargo command that rebuilds the loader in place, or why there is none.
fn command() -> Result<(std::process::Command, String), String> {
    if !cfg!(unix) {
        return Err("this platform cannot replace a running makepad binary".into());
    }
    let exe = loader().ok_or("the makepad binary's path is unknown")?;
    let (package, manifest) = PACKAGE.lock().unwrap_or_else(|e| e.into_inner()).ok_or("no loader package")?;
    let (target, profile, triple) = layout(&exe).ok_or_else(|| format!("{} is not in a cargo target directory", exe.display()))?;
    if !Path::new(manifest).is_file() {
        return Err(format!("{manifest} is missing"));
    }
    let bin = exe.file_stem().ok_or("no binary name")?.to_string_lossy().into_owned();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut args: Vec<String> = vec!["-n".into(), "10".into(), cargo, "build".into(), "--manifest-path".into(), manifest.into(), "-p".into(), package.into(), "--bin".into(), bin];
    match profile.as_str() {
        "debug" => {}
        "release" => args.push("--release".into()),
        p => { args.push("--profile".into()); args.push(p.into()); }
    }
    args.push("--target-dir".into());
    args.push(target.to_string_lossy().into_owned());
    if let Some(t) = triple { args.push("--target".into()); args.push(t); }
    let shown = args[2..].join(" ");
    let mut cmd = std::process::Command::new("nice");
    cmd.args(&args).env_remove("MAKEPAD_RUN_RESTORE").env_remove("MAKEPAD_RUN_RESTART_T0").stdin(std::process::Stdio::null());
    Ok((cmd, shown))
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Idle,
    /// cargo builds the new binary
    Building,
    /// built; the restart waits for the app's next safe point (or its permission)
    Restarting,
    /// the last rebuild failed: the app runs the previous code
    Failed,
}

struct State {
    phase: Phase,
    since: Option<Instant>,
    /// what changed (the newest request), and every input whose change needs this
    why: String,
    paths: Vec<String>,
    /// changed again while building: build once more
    again: bool,
    /// the failure's message
    failed: String,
}

static STATE: Mutex<State> = Mutex::new(State { phase: Phase::Idle, since: None, why: String::new(), paths: Vec::new(), again: false, failed: String::new() });

/// A built binary waiting for the program's safe point: the stamps it was built against.
struct Handover {
    loader: PathBuf,
    stamps: Vec<super::inputs::Stamp>,
}

static HANDOVER: Mutex<Option<Handover>> = Mutex::new(None);

/// The ring's view: the phase and how long it has lasted.
pub fn status() -> (Phase, std::time::Duration) {
    let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    (st.phase, st.since.map_or(std::time::Duration::ZERO, |t| t.elapsed()))
}

/// The last failure's message (empty without one).
pub fn failure() -> String {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).failed.clone()
}

fn stamps(paths: &[String]) -> Vec<super::inputs::Stamp> {
    paths.iter().map(|p| super::stamp(p)).collect()
}

/// The watcher: `why` changed, which needs a new process. Starts the rebuild (or one more after
/// the running one). Err: this run cannot rebuild itself (the caller says how to restart).
pub(super) fn request(why: String, paths: Vec<String>) -> Result<(), String> {
    let _k = crate::zone_alloc::kept();
    command()?;
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.why = why;
    st.paths = paths;
    match st.phase {
        Phase::Building => {
            st.again = true;
            eprintln!("makepad live: {}; makepad builds again once the running build ends", st.why);
            return Ok(());
        }
        // a newer change than the binary waiting for the app's safe point: build again first
        Phase::Restarting => { HANDOVER.lock().unwrap_or_else(|e| e.into_inner()).take(); }
        Phase::Idle | Phase::Failed => {}
    }
    st.phase = Phase::Building;
    st.since = Some(Instant::now());
    st.again = false;
    st.failed.clear();
    drop(st);
    std::thread::Builder::new().name("run-rebuild".into()).spawn(|| { let _k = crate::zone_alloc::kept(); build_loop() }).map(|_| ()).map_err(|e| e.to_string())
}

fn fail(message: String) {
    eprintln!("makepad live: {message}");
    super::note_restart_pending(&message);
    super::live_log(&format!("{{\"event\":\"host_rebuild\",\"ok\":false,\"message\":{}}}", super::json_str(&message)));
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.phase = Phase::Failed;
    st.since = Some(Instant::now());
    st.failed = message;
}

/// The error lines of cargo's output (else its last lines).
fn errors(log: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    let mut picked: Vec<&str> = lines.iter().copied().filter(|l| l.starts_with("error") || l.trim_start().starts_with("--> ")).take(12).collect();
    if picked.is_empty() { picked = lines[lines.len().saturating_sub(10)..].to_vec(); }
    picked.join("\n  ")
}

fn build_loop() {
    loop {
        let (why, paths) = {
            let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
            st.again = false;
            (st.why.clone(), st.paths.clone())
        };
        let before = stamps(&paths);
        let (mut cmd, shown) = match command() {
            Ok(c) => c,
            Err(e) => return fail(format!("{why}; cannot rebuild makepad: {e}; the app runs the previous code -- restart it by hand")),
        };
        eprintln!("makepad live: {why}; restarting makepad: it builds first in the background (`{shown}`, a no-op when makepad is current); the app keeps running the previous code");
        super::live_log(&format!("{{\"event\":\"host_rebuild\",\"started\":true,\"why\":{}}}", super::json_str(&why)));
        let t0 = Instant::now();
        let out = match cmd.output() {
            Ok(o) => o,
            Err(e) => return fail(format!("rebuilding makepad: cannot start `{shown}`: {e}; the app runs the previous code")),
        };
        if !out.status.success() {
            let log = String::from_utf8_lossy(&out.stderr);
            return fail(format!("rebuilding makepad FAILED ({}); the app runs the previous code; fix and save to build again:\n  {}", out.status, errors(&log)));
        }
        if STATE.lock().unwrap_or_else(|e| e.into_inner()).again || stamps(&paths) != before {
            eprintln!("makepad live: sources changed during the build; building again");
            continue;
        }
        let Some(loader) = loader() else { return fail("the makepad binary's path is unknown".into()) };
        // the new binary starts at all (its own codegen identity and provenance)
        match std::process::Command::new(&loader).arg("--version").stdin(std::process::Stdio::null()).output() {
            Ok(o) if o.status.success() => {}
            Ok(o) => return fail(format!("the rebuilt makepad does not start ({}): {}; the app runs the previous code", o.status, String::from_utf8_lossy(&o.stderr).trim())),
            Err(e) => return fail(format!("the rebuilt makepad does not start: {e}; the app runs the previous code")),
        }
        let secs = t0.elapsed().as_secs_f64();
        super::live_log(&format!("{{\"event\":\"host_rebuild\",\"ok\":true,\"s\":{secs:.1}}}"));
        {
            let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
            if st.again { drop(st); continue; }
            st.phase = Phase::Restarting;
            st.since = Some(Instant::now());
        }
        *HANDOVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Handover { loader, stamps: before });
        let reason = format!("makepad was rebuilt ({why})");
        if super::RESTART_SUPPORTED.load(std::sync::atomic::Ordering::SeqCst) {
            eprintln!("makepad live: makepad rebuilt in {secs:.0} s; restarting into it at the app's next safe point, with its state");
            *super::RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()) = reason;
            super::RESTART_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
            super::wake_program();
        } else {
            eprintln!("makepad live: makepad rebuilt in {secs:.0} s; restarting into it");
            *super::RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()) = reason;
            if let Some(h) = take() { restart(h, None); }
        }
        return;
    }
}

/// The built binary, when one waits for the program's restart.
fn take() -> Option<Handover> {
    HANDOVER.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The program's safe point (std::relax::restart_now): with a rebuilt binary waiting, the process
/// restarts into it with `state`, and does not return. False: no binary waits (the caller
/// restarts as before), or the restart did not happen (said why; the app keeps running).
pub(super) fn restart_now(state: &[u8]) -> bool {
    match take() {
        Some(h) => { restart(h, Some(state)); true }
        None => false,
    }
}

fn restart(h: Handover, state: Option<&[u8]>) {
    let _k = crate::zone_alloc::kept();
    super::RESTART_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
    // changed since the build: that binary is already stale (a provided crate would refuse it)
    let (why, paths) = {
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        (st.why.clone(), st.paths.clone())
    };
    if stamps(&paths) != h.stamps {
        STATE.lock().unwrap_or_else(|e| e.into_inner()).phase = Phase::Idle;
        if let Err(e) = request(format!("{why} (and changed again before the restart)"), paths) { fail(e); }
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        let mut cmd = std::process::Command::new(&h.loader);
        cmd.args(&args);
        match state {
            Some(st) => {
                let dir = std::env::temp_dir().join(format!("relax-restart-{}", std::process::id()));
                let path = dir.join("state");
                if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, st)) {
                    return fail(format!("restart into the rebuilt makepad: cannot write the app's state: {e}; the app runs the previous code"));
                }
                cmd.env("MAKEPAD_RUN_RESTORE", &path);
            }
            None => { cmd.env_remove("MAKEPAD_RUN_RESTORE"); }
        }
        let reason = super::RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()).clone();
        cmd.env("MAKEPAD_RUN_RESTART_T0", super::RESTART_SAVED_MS.load(std::sync::atomic::Ordering::Relaxed).to_string());
        // the run ends as at exit (exec skips the exit handlers); fd 2 goes back to the terminal
        // now (the transcript tee is a thread of this process: the new one starts its own), so
        // the line below and the new process's output are not lost in the old pipe
        crate::run::summary("restart");
        // a restart is a clean end of this process (its transcript is not kept)
        crate::tui::exit_code(0);
        crate::tui::finish(false);
        eprintln!("makepad live: restarting: {reason}");
        let e = cmd.exec();
        fail(format!("restart into the rebuilt makepad failed: {e}; the app runs the previous code"));
    }
    #[cfg(not(unix))]
    {
        let _ = (h, state);
        fail("this platform cannot restart into a rebuilt makepad; restart it by hand".into());
    }
}
