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
        let p = std::env::var_os("MAKEPAD_RUN_LOADER").map(PathBuf::from).or_else(|| std::env::current_exe().ok());
        // what this process runs: a rebuild that leaves the same file changed nothing it runs
        let _ = LOADED.set(p.as_deref().and_then(identity));
        p
    }).clone()
}

/// The loader file this process started from (`identity`), taken with its path.
static LOADED: std::sync::OnceLock<Option<(u64, u64, std::time::SystemTime)>> = std::sync::OnceLock::new();

/// A file's identity: device, inode, modification time (cargo writes a new binary as a new file).
fn identity(p: &Path) -> Option<(u64, u64, std::time::SystemTime)> {
    let m = std::fs::metadata(p).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((m.dev(), m.ino(), m.modified().ok()?))
    }
    #[cfg(not(unix))]
    Some((0, m.len(), m.modified().ok()?))
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
    /// A source-check mismatch waits for the same rebuild/restart action as a watched edit.
    provided: Option<crate::provided::ProvidedChange>,
    update_message: Option<std::sync::Arc<str>>,
}

static STATE: Mutex<State> = Mutex::new(State { phase: Phase::Idle, since: None, why: String::new(), paths: Vec::new(), again: false, failed: String::new(), provided: None, update_message: None });

/// Best effort, once on the compiler worker when the mismatch is found. Group paths by their
/// repository (a worktree's .git may be a file); removed inputs still name a commit in git log.
fn landed_change(files: &[String]) -> Option<String> {
    let mut repos = std::collections::BTreeMap::<PathBuf, Vec<&Path>>::new();
    for file in files {
        let path = Path::new(file);
        let Some(root) = path.ancestors().skip(1).find(|p| p.join(".git").exists()) else { continue };
        repos.entry(root.to_path_buf()).or_default().push(path.strip_prefix(root).ok()?);
    }
    repos.into_iter().filter_map(|(root, paths)| {
        let out = std::process::Command::new("git").current_dir(root)
            .args(["--literal-pathspecs", "log", "-1", "--format=%ct%x00%h %s", "--"])
            .args(paths).stdin(std::process::Stdio::null()).output().ok()?;
        if !out.status.success() { return None; }
        let text = String::from_utf8(out.stdout).ok()?;
        let (time, subject) = text.trim().split_once('\0')?;
        Some((time.parse::<u64>().ok()?, subject.to_string()))
    }).max_by_key(|(time, _)| *time).map(|(_, subject)| subject)
}

pub(crate) fn offer_provided(change: crate::provided::ProvidedChange, domain: Option<u32>) {
    let _k = crate::zone_alloc::kept();
    let mut message = format!("The JIT itself changed ({}) -- Restart to update", change.crate_name);
    if let Some(landed) = landed_change(&change.files) { message.push_str(&format!(" · {landed}")); }
    let paths = change.files.clone();
    let provided = Some(change.clone());
    let update_message = Some(std::sync::Arc::<str>::from(message.as_str()));
    let why = message.clone();
    {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        st.paths = paths;
        st.why = why;
        st.provided = provided;
        st.update_message = update_message;
        if st.phase == Phase::Building { st.again = true; }
    }
    super::offer_restart(&message);
    crate::run::report("update", &message);
    crate::run_state::emit(crate::run_state::Event::UpdateReady { domain, message });
}

/// The pid of a process that became the rebuilt makepad at boot (`apply_at_boot`): a process
/// started by the program has another pid, and rebuilds for its own boot.
const REBUILT_ENV: &str = "MAKEPAD_RUN_BOOT_REBUILT";

/// A boot whose provided crates changed, with nobody to take the offer: no run window, a hidden
/// one, or a run that is no live session (a command-line program, a test's child). The update is
/// applied before the program runs: makepad builds again in the foreground (cargo's output shows)
/// and the process becomes the rebuilt makepad with the same command line. Returns only when that
/// cannot happen, having said why, with the run's exit code (1: the program did not run), so a
/// stale makepad never ends a run with 0 and nothing done.
pub(crate) fn apply_at_boot(change: &crate::provided::ProvidedChange) -> i32 {
    let _k = crate::zone_alloc::kept();
    let mut what = format!("The JIT itself changed ({}: {}", change.crate_name, change.files.iter().take(3).map(String::as_str).collect::<Vec<_>>().join(", "));
    if change.files.len() > 3 { what.push_str(&format!(" and {} more", change.files.len() - 3)); }
    what.push(')');
    let failed = |e: String| -> i32 {
        let message = format!("{what}: {e}; the program did not run");
        eprintln!("makepad-run: {message}");
        crate::run::report_failed(&message, 1);
        1
    };
    // this process is that rebuild already (the exec below keeps the pid): makepad is built from
    // other sources than the program's copy of the crate, and building again cannot change that
    if std::env::var(REBUILT_ENV).ok().and_then(|v| v.parse::<u32>().ok()) == Some(std::process::id()) {
        return failed("the rebuilt makepad differs from the program's copy too (makepad is built from other sources than the program uses)".into());
    }
    let (mut cmd, shown) = match command() {
        Ok(c) => c,
        Err(e) => return failed(format!("this makepad cannot rebuild itself ({e}): build it again, then run")),
    };
    eprintln!("makepad-run: {what}: makepad builds again first (`{shown}`), then runs this");
    let t0 = Instant::now();
    match cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::inherit()).status() {
        Ok(s) if s.success() => {}
        Ok(s) => return failed(format!("rebuilding makepad FAILED ({s})")),
        Err(e) => return failed(format!("cannot start `{shown}`: {e}")),
    }
    let Some(exe) = loader() else { return failed("the makepad binary's path is unknown".into()) };
    // cargo left the binary this process runs: running it again would find the same change
    if LOADED.get().copied().flatten().is_some_and(|id| identity(&exe) == Some(id)) {
        return failed("rebuilding makepad changed nothing (cargo builds it from other sources than the program uses)".into());
    }
    eprintln!("makepad-run: makepad rebuilt in {:.0} s; the program runs on it", t0.elapsed().as_secs_f64());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(std::env::args_os().skip(1)).env(REBUILT_ENV, std::process::id().to_string());
        // fd 2 back to the terminal before the exec (the transcript tee is a thread of this process)
        crate::tui::exit_code(0);
        crate::tui::finish(false);
        let e = cmd.exec();
        failed(format!("cannot start the rebuilt makepad: {e}"))
    }
    #[cfg(not(unix))]
    failed("this platform cannot start the rebuilt makepad in place".into())
}

/// Cached text only: no manifest scan or git process on the UI thread.
pub fn update_message() -> Option<std::sync::Arc<str>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).update_message.clone()
}

/// The ring button and `session relaunch` both arrive here. A source-check offer starts the
/// existing background build; a build already running finishes before requesting a safe point.
pub(super) fn relaunch() -> Option<Result<&'static str, String>> {
    let (phase, why, paths) = {
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if st.phase == Phase::Idle && st.provided.is_none() { return None; }
        if st.phase == Phase::Building {
            RESTART_AFTER_BUILD.store(true, std::sync::atomic::Ordering::SeqCst);
            return Some(Ok("rebuild makepad, then restart with the program's state"));
        }
        (st.phase, st.why.clone(), st.paths.clone())
    };
    if phase == Phase::Restarting { return Some(apply_ready(why)); }
    RESTART_AFTER_BUILD.store(true, std::sync::atomic::Ordering::SeqCst);
    Some(request(why, paths).map(|()| "rebuild makepad, then restart with the program's state").map_err(|e| {
        RESTART_AFTER_BUILD.store(false, std::sync::atomic::Ordering::SeqCst);
        fail(e.clone());
        e
    }))
}

fn apply_ready(why: String) -> Result<&'static str, String> {
    {
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        // Another landing can supersede the ready binary between the click and this call.
        if st.phase == Phase::Building {
            RESTART_AFTER_BUILD.store(true, std::sync::atomic::Ordering::SeqCst);
            return Ok("rebuild makepad, then restart with the program's state");
        }
    }
    super::RESTART_CONFIRMED.store(true, std::sync::atomic::Ordering::SeqCst);
    *super::RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()) = why.clone();
    if super::pinned() { return Ok("restart waits until unpinned"); }
    if super::RESTART_SUPPORTED.load(std::sync::atomic::Ordering::SeqCst) {
        // This asks AppMain::can_restart and the unsaved registry at the next app event.
        return super::restart_in_place(why);
    }
    let h = take().ok_or("the rebuilt makepad is not ready")?;
    restart(h, None);
    Ok("restart into the rebuilt makepad")
}

/// A built binary waiting for the program's safe point: the stamps it was built against.
struct Handover {
    /// (live-rebuild handover is unix-only: the restart execs this in place)
    #[cfg(unix)]
    loader: PathBuf,
    stamps: Vec<super::inputs::Stamp>,
}

static HANDOVER: Mutex<Option<Handover>> = Mutex::new(None);

/// A live test session's rebuilt makepad (the reason): the watcher restarts the tests into it
/// between runs, as for any edit that needs a restart (`take_harness_restart`).
static HARNESS_RESTART: Mutex<Option<String>> = Mutex::new(None);

pub(super) fn take_harness_restart() -> Option<String> {
    HARNESS_RESTART.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The person asked for the restart, and its binary had to be built again first (sources changed
/// before it): the restart follows the build without asking again.
static RESTART_AFTER_BUILD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    RESTART_AFTER_BUILD.store(false, std::sync::atomic::Ordering::SeqCst);
    eprintln!("makepad live: {message}");
    super::note_restart_pending(&message);
    super::live_log(&format!("{{\"event\":\"host_rebuild\",\"ok\":false,\"message\":{}}}", super::json_str(&message)));
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.phase = Phase::Failed;
    st.since = Some(Instant::now());
    st.failed = message.clone();
    let update = st.provided.is_some();
    let why = st.why.clone();
    drop(st);
    // the edit's last verdict: refused, saying what failed (a build's first error lines)
    let lines: Vec<String> = message.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    super::patch_refused_event(&why, true, &lines);
    // A boot waiting for this update has no running app's ring yet. Show a genuine rebuild
    // failure in its run view too, retaining the update button so the person can retry.
    if update {
        crate::run_state::diagnostic(crate::run::app_domain(), crate::diag::Diagnostic::new("host rebuild", "", 0, 0, message));
    }
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
        // the same binary as this process runs: the change did not reach makepad (a no-op
        // build), so there is nothing to restart into
        if STATE.lock().unwrap_or_else(|e| e.into_inner()).provided.is_none()
            && LOADED.get().copied().flatten().is_some_and(|id| identity(&loader).is_some_and(|now| now == id)) {
            eprintln!("makepad live: {why}: makepad is unchanged (built in {secs:.0} s); no restart");
            let what = "rebuild produced no change: cargo left the makepad this process runs as it was (the edit is not in what makepad is built from, or was undone)".to_string();
            // a live test session's tests already ran on this binary: no run answers it but this
            if crate::harness::live::enabled() { super::patch_refused_event(&why, true, std::slice::from_ref(&what)); }
            else { super::patch_verified_event(&format!("{why}: {what}"), 0, &[]); }
            super::live_log(&format!("{{\"event\":\"host_rebuild\",\"ok\":true,\"s\":{secs:.1},\"unchanged\":true}}"));
            let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
            if st.again { drop(st); continue; }
            st.phase = Phase::Idle;
            st.since = Some(Instant::now());
            RESTART_AFTER_BUILD.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        super::live_log(&format!("{{\"event\":\"host_rebuild\",\"ok\":true,\"s\":{secs:.1}}}"));
        let asked = {
            let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
            if st.again { drop(st); continue; }
            *HANDOVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Handover { #[cfg(unix)] loader, stamps: before });
            st.phase = Phase::Restarting;
            st.since = Some(Instant::now());
            RESTART_AFTER_BUILD.swap(false, std::sync::atomic::Ordering::SeqCst)
        };
        let reason = format!("makepad was rebuilt ({why})");
        // a live test session restarts its tests into it at once (its verdict: the restarted run)
        if crate::harness::live::enabled() {
            eprintln!("makepad live: makepad rebuilt in {secs:.0} s; the tests restart into it");
            *HARNESS_RESTART.lock().unwrap_or_else(|e| e.into_inner()) = Some(reason);
            super::watch_events::kick();
            return;
        }
        if !asked {
            // a running app restarts into it when the person says so (`session relaunch`)
            *super::RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()) = reason.clone();
            super::offer_restart(&reason);
            super::patch_refused_event(&reason, false, std::slice::from_ref(&reason));
            return;
        }
        eprintln!("makepad live: makepad rebuilt in {secs:.0} s; applying the requested restart");
        if let Err(e) = apply_ready(reason) { fail(e); }
        return;
    }
}

/// The built binary, when one waits for the program's restart.
fn take() -> Option<Handover> {
    HANDOVER.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The program's safe point (std::relax::restart_now): with a rebuilt binary waiting, the process
/// restarts into it with `state`, and does not return. False: no binary waits. A pinned or
/// unconfirmed handover consumes the request too: it must never fall through to a domain
/// relaunch against the stale provided libraries.
pub(super) fn restart_now(state: &[u8]) -> bool {
    let handover = {
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let handover = take();
        if handover.is_none() && (st.phase == Phase::Building || st.provided.is_some()) {
            // A newer landing may have started another build after the app was asked. Keep
            // its running domain and ask for a fresh snapshot only when that build finishes.
            if st.phase == Phase::Building && super::RESTART_CONFIRMED.swap(false, std::sync::atomic::Ordering::SeqCst) {
                RESTART_AFTER_BUILD.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            super::RESTART_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
            return true;
        }
        handover
    };
    match handover {
        Some(h) if super::pinned() || !super::RESTART_CONFIRMED.swap(false, std::sync::atomic::Ordering::SeqCst) => {
            // a restart into a rebuilt makepad waits for the person (`session relaunch`)
            *HANDOVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(h);
            super::RESTART_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
            eprintln!("makepad live: the app reached a restart point; the rebuilt makepad waits for `session relaunch`");
            true
        }
        Some(h) => { restart(h, Some(state)); true }
        None => false,
    }
}

/// Unpinned: a rebuilt makepad that waited takes over now.
pub(super) fn unpinned() {
    if !super::RESTART_CONFIRMED.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if HANDOVER.lock().unwrap_or_else(|e| e.into_inner()).is_none() { return; }
    let why = super::RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Err(e) = apply_ready(why) { fail(e); }
}

fn restart(h: Handover, state: Option<&[u8]>) {
    // pinned: the binary waits for the unpin
    if super::pinned() {
        *HANDOVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(h);
        return;
    }
    let _k = crate::zone_alloc::kept();
    super::RESTART_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
    // changed since the build: that binary is already stale (a provided crate would refuse it)
    let (why, paths) = {
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        (st.why.clone(), st.paths.clone())
    };
    if stamps(&paths) != h.stamps {
        STATE.lock().unwrap_or_else(|e| e.into_inner()).phase = Phase::Idle;
        // asked for (a dock app restarts only so): restart once the binary is current
        RESTART_AFTER_BUILD.store(true, std::sync::atomic::Ordering::SeqCst);
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
        super::hand_over(&mut cmd);
        let e = cmd.exec();
        super::hand_back();
        fail(format!("restart into the rebuilt makepad failed: {e}; the app runs the previous code"));
    }
    #[cfg(not(unix))]
    {
        let _ = (h, state);
        fail("this platform cannot restart into a rebuilt makepad; restart it by hand".into());
    }
}
