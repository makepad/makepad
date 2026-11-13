//! Changes a running process cannot take in place: the provided crates built into makepad, std,
//! makepad run's own sources, the Cargo graph and build scripts (a program edit the patcher
//! cannot absorb restarts in place without a rebuild: reload.rs restart_in_place). A running app
//! offers the update ([`offer`]): each landing built makepad in every app on the person's machine,
//! whether or not they meant to restart (96 min of builds in 13 h, 11-04). The person's restart,
//! or the next `cargo run`, builds it; a dock app's sync is offered too, devhub's deploy having built
//! it already (sync/build.rs prebuild), so that cargo finds makepad current. A live test session
//! builds at once.
//! The loader is rebuilt with cargo while the app keeps running the code it has (at low priority
//! when nobody waits for it, at the person's when they asked for the restart: [`Priority`], and
//! their build goes before the low ones: [`Gate`]), then the process truly restarts: the rebuilt binary replaces it (exec) with the same command line, and an app that
//! restarts with its state (std::relax::restart_supported) is asked first, at its next safe point
//! (it may postpone for unsaved work), and gets its state back in the new process. A failed
//! rebuild keeps the running app and says why; the next such change builds again. From the click
//! until the restart the ring says what happens ([`restart_asked`], [`build_progress`]) and what a
//! click would do before it ([`route`]).
//!
//! One rebuild at a time; a change during it builds once more after it. The new binary is the
//! one cargo leaves at the loader's own path, which a JIT devhub picked by its inputs' content has
//! not ([`picked`]): its run picks again. Its codegen identity (codegen_id.rs) keys every cache
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

/// The loader binary (RELAX_LOADER, set by the runner; else this executable), as it was
/// at startup: cargo replaces the file, and Linux's current_exe then names a deleted inode.
pub(crate) fn loader() -> Option<PathBuf> {
    static LOADER: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    LOADER.get_or_init(|| {
        let _k = crate::zone_alloc::kept();
        // (never a run's own executable: dirs::loader_exe)
        let p = crate::dirs::loader_exe().ok();
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

/// A JIT devhub picked by the content of its inputs for the tree it runs (`makepad.jit-<key>`:
/// lease/loaders.rs, lease/jitstore.rs). Cargo never writes that file, and the checkout its
/// target builds is not that tree: a lane's session on the box's binary rebuilt the box's
/// checkout for 79 s and ran the same binary again, its lane's own JIT sources never built
/// (W3752, 11-06). It cannot rebuild itself; the run that picked it picks again for the tree.
pub(crate) fn picked(exe: &Path) -> bool {
    exe.file_name().is_some_and(|n| n.to_string_lossy().starts_with("makepad.jit-"))
}

/// How a build of the loader runs.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Priority {
    /// Nobody waits for it: the app keeps running while it builds (a landing that changed makepad,
    /// a live test session). On macOS at background QoS (efficiency cores, throttled I/O; cargo's
    /// rustc inherit it, and nothing can lift that clamp from a running process), never at full
    /// width beside the app the person works in: three running apps' rebuilds after a landing took
    /// rustc to ~1080% of a core on the person's Mac while they tweaked an app (11-03). Elsewhere,
    /// nice 10.
    Background,
    /// A boot waits for it: nice 10.
    Boot,
    /// The person asked for the restart it leads to and looks at the ring until it comes: their
    /// own priority, on all cores (the clamp held rustc at ~230% for over six minutes; 63 s at ~840%).
    Asked,
}

/// The cargo command that rebuilds the loader in place, or why there is none. Apps that start
/// their rebuilds at once build
/// once: cargo's lock on the profile's build directory keeps the others waiting for that build,
/// and their cargo then finds makepad current.
fn command(priority: Priority) -> Result<(std::process::Command, String), String> {
    if !cfg!(unix) {
        return Err(format!("this platform cannot replace a running {tool} binary", tool = crate::tool_name()));
    }
    let exe = loader().ok_or_else(|| format!("the {tool} binary's path is unknown", tool = crate::tool_name()))?;
    if picked(&exe) {
        return Err(format!("{} was picked for this tree by its inputs' content: cargo never writes it, and its run picks the JIT again", exe.display()));
    }
    let (package, manifest) = PACKAGE.lock().unwrap_or_else(|e| e.into_inner()).ok_or("no loader package")?;
    let (target, profile, triple) = layout(&exe).ok_or_else(|| format!("{} is not in a cargo target directory", exe.display()))?;
    if !Path::new(manifest).is_file() {
        return Err(format!("{manifest} is missing"));
    }
    let bin = exe.file_stem().ok_or("no binary name")?.to_string_lossy().into_owned();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut args: Vec<String> = vec![cargo, "build".into(), "--manifest-path".into(), manifest.into(), "-p".into(), package.into(), "--bin".into(), bin];
    match profile.as_str() {
        "debug" => {}
        "release" => args.push("--release".into()),
        p => { args.push("--profile".into()); args.push(p.into()); }
    }
    args.push("--target-dir".into());
    args.push(target.to_string_lossy().into_owned());
    if let Some(t) = triple { args.push("--target".into()); args.push(t); }
    let shown = args.join(" ");
    let low: &[&str] = match priority {
        Priority::Asked => &[],
        Priority::Background if cfg!(target_os = "macos") => &["/usr/sbin/taskpolicy", "-c", "background"],
        Priority::Background | Priority::Boot => &["nice", "-n", "10"],
    };
    let line: Vec<&str> = low.iter().copied().chain(args.iter().map(String::as_str)).collect();
    let mut cmd = std::process::Command::new(line[0]);
    cmd.args(&line[1..]).env_remove("RELAX_RESTORE").env_remove("RELAX_RESTART_T0").stdin(std::process::Stdio::null());
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

/// What cargo said last of the running build: the ring's progress and tooltip.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct BuildProgress {
    /// units finished, of how many (0 of 0: cargo has not said yet)
    pub done: u32,
    pub total: u32,
    /// the first of what it builds now, in cargo's words
    pub building: String,
    /// what another cargo holds that this one waits for (empty: it does not wait)
    pub waiting: String,
}

/// What a click on the ring's dot (`session relaunch`) does now, so the dot can say it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Route {
    /// The program's run starts again on this JIT.
    Plain,
    /// The JIT's own inputs changed: it builds first, then the restart.
    BuildFirst,
    /// The new JIT builds already: the restart follows it.
    Building,
    /// The new JIT is built: the restart goes into it.
    Built,
    /// The JIT's own inputs changed, and this process cannot build it (Windows cannot replace a
    /// running binary): the restart stays on this JIT.
    CannotBuild,
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
    /// A change of what makepad is built from waits for the person's restart ([`offer`]).
    offered: bool,
    /// A change of the JIT's own inputs came and this process cannot build it.
    cannot_build: bool,
    /// What makepad is built from changed since the binary this process runs (a change the
    /// watcher saw, the boot's source check), and no rebuild has shown makepad unchanged by it:
    /// set whether or not this process can build it, kept through the build and its failure
    /// ([`inputs_changed`]).
    stale: bool,
    progress: BuildProgress,
    /// the low-priority cargo running now: its process and where it is marked ([`Gate`])
    low_cargo: Option<(u32, PathBuf)>,
    update_message: Option<std::sync::Arc<str>>,
}

static STATE: Mutex<State> = Mutex::new(State { phase: Phase::Idle, since: None, why: String::new(), paths: Vec::new(), again: false, failed: String::new(), provided: None, offered: false, cannot_build: false, stale: false, low_cargo: None, progress: BuildProgress { done: 0, total: 0, building: String::new(), waiting: String::new() }, update_message: None });

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
    let cannot_build = command(Priority::Background).is_err();
    {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        st.paths = paths;
        st.why = why;
        st.provided = provided;
        st.cannot_build = cannot_build;
        st.stale = true;
        st.update_message = update_message;
        if st.phase == Phase::Building { st.again = true; }
    }
    super::offer_restart(&message);
    crate::run::report("update", &message);
    crate::run_state::emit(crate::run_state::Event::UpdateReady { domain, message });
}

/// The pid of a process that became the rebuilt makepad at boot (`apply_at_boot`): a process
/// started by the program has another pid, and rebuilds for its own boot.
const REBUILT_ENV: &str = "RELAX_BOOT_REBUILT";

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
        eprintln!("relax: {message}");
        crate::run::report_failed(&message, 1);
        1
    };
    // this process is that rebuild already (the exec below keeps the pid): makepad is built from
    // other sources than the program's copy of the crate, and building again cannot change that
    if std::env::var(REBUILT_ENV).ok().and_then(|v| v.parse::<u32>().ok()) == Some(std::process::id()) {
        return failed(format!("the rebuilt {tool} differs from the program's copy too ({tool} is built from other sources than the program uses)", tool = crate::tool_name()));
    }
    let (mut cmd, shown) = match command(Priority::Boot) {
        Ok(c) => c,
        Err(e) => return failed(format!("this {tool} cannot rebuild itself ({e}): build it again, then run", tool = crate::tool_name())),
    };
    eprintln!("relax: {what}: {tool} builds again first (`{shown}`), then runs this", tool = crate::tool_name());
    let t0 = Instant::now();
    match cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::inherit()).status() {
        Ok(s) if s.success() => {}
        Ok(s) => return failed(format!("rebuilding {tool} FAILED ({s})", tool = crate::tool_name())),
        Err(e) => return failed(format!("cannot start `{shown}`: {e}")),
    }
    let Some(exe) = loader() else { return failed(format!("the {tool} binary's path is unknown", tool = crate::tool_name())) };
    // cargo left the binary this process runs: running it again would find the same change
    if LOADED.get().copied().flatten().is_some_and(|id| identity(&exe) == Some(id)) {
        return failed(format!("rebuilding {tool} changed nothing (cargo builds it from other sources than the program uses)", tool = crate::tool_name()));
    }
    eprintln!("relax: {tool} rebuilt in {:.0} s; the program runs on it", t0.elapsed().as_secs_f64(), tool = crate::tool_name());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(std::env::args_os().skip(1)).env(REBUILT_ENV, std::process::id().to_string());
        // the rebuilt loader builds the program's native packages as a restart does (NATIVE_RECEIPTS)
        for name in super::NATIVE_RECEIPTS { cmd.env_remove(name); }
        // fd 2 back to the terminal before the exec (the transcript tee is a thread of this process)
        crate::tui::exit_code(0);
        crate::tui::finish(false);
        let e = cmd.exec();
        failed(format!("cannot start the rebuilt {tool}: {e}", tool = crate::tool_name()))
    }
    #[cfg(not(unix))]
    failed(format!("this platform cannot start the rebuilt {tool} in place", tool = crate::tool_name()))
}

/// Cached text only: no manifest scan or git process on the UI thread.
pub fn update_message() -> Option<std::sync::Arc<str>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).update_message.clone()
}

/// The ring button and `session relaunch` both arrive here. A source-check offer starts the
/// existing background build; a build already running finishes before requesting a safe point.
pub(super) fn relaunch() -> Option<Result<String, String>> {
    let (phase, why, paths) = {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if st.phase == Phase::Idle && st.provided.is_none() && !st.offered { return None; }
        if st.phase == Phase::Building {
            RESTART_AFTER_BUILD.store(true, std::sync::atomic::Ordering::SeqCst);
            // a build nobody waited for: it goes on at the person's priority now
            end_low_cargo(&mut st);
            return Some(Ok(format!("rebuild {tool}, then restart with the program's state", tool = crate::tool_name())));
        }
        (st.phase, st.why.clone(), st.paths.clone())
    };
    if phase == Phase::Restarting { return Some(apply_ready(why)); }
    RESTART_AFTER_BUILD.store(true, std::sync::atomic::Ordering::SeqCst);
    Some(request(why, paths).map(|()| format!("rebuild {tool}, then restart with the program's state", tool = crate::tool_name())).map_err(|e| {
        RESTART_AFTER_BUILD.store(false, std::sync::atomic::Ordering::SeqCst);
        fail(e.clone());
        e
    }))
}

fn apply_ready(why: String) -> Result<String, String> {
    {
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        // Another landing can supersede the ready binary between the click and this call.
        if st.phase == Phase::Building {
            RESTART_AFTER_BUILD.store(true, std::sync::atomic::Ordering::SeqCst);
            return Ok(format!("rebuild {tool}, then restart with the program's state", tool = crate::tool_name()));
        }
    }
    super::RESTART_CONFIRMED.store(true, std::sync::atomic::Ordering::SeqCst);
    *super::RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()) = why.clone();
    if super::pinned() { return Ok("restart waits until unpinned".into()); }
    if super::RESTART_SUPPORTED.load(std::sync::atomic::Ordering::SeqCst) {
        match super::handover_blocked() {
            // This asks AppMain::can_restart and the unsaved registry at the next app event.
            None => return super::restart_in_place(why).map(str::to_string),
            Some(blocked) => eprintln!("relax live: the restart into the rebuilt {tool} does not wait for the app's state: {blocked}", tool = crate::tool_name()),
        }
    }
    let h = take().ok_or_else(|| format!("the rebuilt {tool} is not ready", tool = crate::tool_name()))?;
    // (an app before its first event: the state its run was started with)
    restart(h, super::start_state().as_deref().map(str::as_bytes));
    Ok(format!("restart into the rebuilt {tool}", tool = crate::tool_name()))
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

/// What a click on the dot would do now (cached state: this is for the UI thread).
pub fn route() -> Route {
    let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    match st.phase {
        Phase::Building => Route::Building,
        Phase::Restarting => Route::Built,
        Phase::Idle | Phase::Failed if st.cannot_build => Route::CannotBuild,
        Phase::Idle | Phase::Failed if st.offered || st.provided.is_some() => Route::BuildFirst,
        Phase::Idle | Phase::Failed => Route::Plain,
    }
}

/// The restart was asked for and is on its way (the new JIT builds first, or the program's next
/// safe point takes it): nothing is left to click, and a repeat of the request changes nothing.
pub fn restart_asked() -> bool {
    let phase = STATE.lock().unwrap_or_else(|e| e.into_inner()).phase;
    use std::sync::atomic::Ordering::SeqCst;
    match phase {
        Phase::Building => RESTART_AFTER_BUILD.load(SeqCst),
        Phase::Restarting => super::RESTART_CONFIRMED.load(SeqCst),
        Phase::Idle => super::RESTART_CONFIRMED.load(SeqCst) && super::RESTART_REQUESTED.load(SeqCst),
        Phase::Failed => false,
    }
}

/// A restart that waits for the app to be unpinned.
pub fn restart_pinned() -> bool {
    super::pinned()
}

/// Where the ring's dot is in the window (points), while it shows: `session pending` says, so an
/// agent clicks where the person does.
static DOT: Mutex<Option<[f32; 4]>> = Mutex::new(None);

pub fn note_dot(place: Option<[f32; 4]>) {
    *DOT.lock().unwrap_or_else(|e| e.into_inner()) = place;
}

pub fn dot() -> Option<[f32; 4]> {
    *DOT.lock().unwrap_or_else(|e| e.into_inner())
}

/// What cargo said last of the running build.
pub fn build_progress() -> BuildProgress {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).progress.clone()
}

/// A change of the JIT's own inputs that this process cannot build (the watcher: [`offer`] said so).
pub(super) fn note_cannot_build() {
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.cannot_build = true;
    st.stale = true;
}

/// What makepad is built from changed since the binary this process runs: a provided crate's
/// item an edit names can be in its sources and not in this makepad (reload.rs provided_lacks).
/// Unlike [`route`], it stays while no build runs, the rebuild failed, or this process cannot
/// build makepad (a dock app's devhub sync, 11-10: its E0425 on a new `makepad_hal` op stood as
/// the edit's own error).
pub fn inputs_changed() -> bool {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).stale
}

/// Notes a change of what makepad is built from, before whatever answers it can fail.
fn note_stale() {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).stale = true;
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
/// A change of what makepad is built from, in a running app: offered on the ring, and built when
/// the person restarts ([`relaunch`]). Err: this makepad cannot rebuild itself.
pub(super) fn offer(why: String, paths: Vec<String>) -> Result<(), String> {
    let _k = crate::zone_alloc::kept();
    note_stale();
    command(Priority::Background)?;
    {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        st.why = why.clone();
        st.paths = paths;
        st.offered = true;
        match st.phase {
            // the build the person asked for takes it too
            Phase::Building => st.again = true,
            // older than this change: the click builds first
            Phase::Restarting => {
                HANDOVER.lock().unwrap_or_else(|e| e.into_inner()).take();
                st.phase = Phase::Idle;
            }
            // the failed build is answered by this change: the dot offers it again
            Phase::Failed => {
                st.phase = Phase::Idle;
                st.failed.clear();
            }
            Phase::Idle => {}
        }
    }
    super::offer_restart(&why);
    Ok(())
}

pub(crate) fn request(why: String, paths: Vec<String>) -> Result<(), String> {
    let _k = crate::zone_alloc::kept();
    note_stale();
    command(Priority::Background)?;
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.why = why;
    st.paths = paths;
    st.offered = false;
    match st.phase {
        Phase::Building => {
            st.again = true;
            eprintln!("relax live: {}; {tool} builds again once the running build ends", st.why, tool = crate::tool_name());
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
    let _at_the_run = crate::run_threads::process_work();
    let asked = RESTART_AFTER_BUILD.swap(false, std::sync::atomic::Ordering::SeqCst);
    eprintln!("relax live: {message}");
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
    // failure in its run view too, retaining the update button so the person can retry. A build
    // the person asked for: the view the failed dot opens has cargo's errors.
    if update || asked {
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

/// A line of cargo's stderr.
#[derive(PartialEq, Debug)]
enum CargoLine<'a> {
    /// `    Building [===>    ] 12/345: a, b(build), c`
    Progress { done: u32, total: u32, building: &'a str },
    /// `    Blocking waiting for file lock on build directory`
    Waiting(&'a str),
    Other,
}

fn cargo_line(line: &str) -> CargoLine<'_> {
    let line = line.trim();
    if let Some(what) = line.strip_prefix("Blocking waiting for file lock on ") {
        return CargoLine::Waiting(what);
    }
    let parsed = (|| {
        let (_, rest) = line.strip_prefix("Building [")?.split_once("] ")?;
        let (count, names) = rest.split_once(": ").unwrap_or((rest, ""));
        let (done, total) = count.split_once('/')?;
        let building = names.split(", ").next().unwrap_or("").trim();
        Some(CargoLine::Progress { done: done.parse().ok()?, total: total.parse().ok()?, building })
    })();
    parsed.unwrap_or(CargoLine::Other)
}

/// The builds of one profile directory, across the makepad processes that run from it: cargo's
/// lock lets one build at a time, and a low-priority one can hold it for minutes. So the person's
/// build goes first: it ends the low-priority ones, which build again after it (their cargo then
/// finds makepad current). In `<profile dir>/.makepad-rebuilds`: `turn`, locked by the person's
/// build while it runs and by a low-priority build until its cargo has started and is marked;
/// `low.<pid>` per low-priority cargo, locked while the makepad that started it lives (unlocked
/// is left behind by a crash, its pid no longer ours to signal), removed by whoever ends it.
struct Gate {
    dir: PathBuf,
    turn: Option<std::fs::File>,
    low: Option<(PathBuf, std::fs::File)>,
}

impl Gate {
    fn enter(dir: &Path, asked: bool, waits: impl FnOnce()) -> std::io::Result<Gate> {
        std::fs::create_dir_all(dir)?;
        let turn = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("turn"))?;
        if asked {
            if turn.try_lock().is_err() {
                waits();
                turn.lock()?;
            }
            for entry in std::fs::read_dir(dir)?.flatten() {
                let name = entry.file_name();
                let Some(pid) = name.to_str().and_then(|n| n.strip_prefix("low.")).and_then(|p| p.parse::<u32>().ok()) else { continue };
                let held = std::fs::File::open(entry.path()).is_ok_and(|f| matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
                let _ = std::fs::remove_file(entry.path());
                if held { end_group(pid); }
            }
        } else if turn.try_lock_shared().is_err() {
            waits();
            turn.lock_shared()?;
        }
        Ok(Gate { dir: dir.to_path_buf(), turn: Some(turn), low: None })
    }

    /// A low-priority cargo started: marked, where the person's build finds it.
    fn mark_low(&mut self, pid: u32) -> Option<PathBuf> {
        let path = self.dir.join(format!("low.{pid}"));
        let marker = std::fs::File::create(&path).ok()?;
        marker.lock().ok()?;
        self.low = Some((path.clone(), marker));
        self.turn = None;
        Some(path)
    }

    /// The build is over: whether it was ended for a build the person waits for.
    fn ended_for_asked(self) -> bool {
        let Some((path, _marker)) = self.low else { return false };
        if !path.exists() { return true; }
        let _ = std::fs::remove_file(&path);
        false
    }
}

/// `pid` leads a process group (`run_cargo`): the group ends.
pub(crate) fn end_group(pid: u32) {
    #[cfg(unix)]
    {
        extern "C" { fn kill(pid: i32, sig: i32) -> i32; }
        const SIGTERM: i32 = 15;
        // (never -1, which is every process)
        if pid > 1 && pid <= i32::MAX as u32 {
            // SAFETY: a plain syscall on the group this process started.
            unsafe { kill(-(pid as i32), SIGTERM) };
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// The person asked while their own process's low-priority build runs: it ends, and `build_loop`
/// starts it again at their priority.
fn end_low_cargo(st: &mut State) {
    if let Some((pid, path)) = st.low_cargo.take() {
        let _ = std::fs::remove_file(&path);
        end_group(pid);
        st.progress = BuildProgress::default();
    }
}

/// Runs the build, the state following what cargo says. Its status, and its output without the
/// progress lines (a pipe is no terminal: cargo shows progress only when told, at a given width).
fn run_cargo(cmd: &mut std::process::Command, started: impl FnOnce(u32)) -> std::io::Result<(std::process::ExitStatus, String)> {
    use std::io::Read;
    cmd.env("CARGO_TERM_PROGRESS_WHEN", "always").env("CARGO_TERM_PROGRESS_WIDTH", "200").env("CARGO_TERM_COLOR", "never");
    // (a process group of its own: ending this build ends its rustc too)
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(cmd, 0);
    let mut child = cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).spawn()?;
    started(child.id());
    let mut err = child.stderr.take().ok_or_else(|| std::io::Error::other("cargo has no stderr pipe"))?;
    let (mut log, mut line, mut buf) = (String::new(), Vec::new(), [0u8; 4096]);
    let mut take = |line: &mut Vec<u8>| {
        let text = String::from_utf8_lossy(line).into_owned();
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        match cargo_line(&text) {
            CargoLine::Progress { done, total, building } => {
                st.progress = BuildProgress { done, total, building: building.to_string(), waiting: String::new() };
            }
            CargoLine::Waiting(what) => {
                st.progress.waiting = what.to_string();
                drop(st);
                eprintln!("relax live: its cargo waits for another build (file lock on {what})");
            }
            CargoLine::Other if !text.trim().is_empty() => {
                st.progress.waiting.clear();
                log.push_str(&text);
                log.push('\n');
            }
            CargoLine::Other => {}
        }
        line.clear();
    };
    loop {
        let n = match err.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' { take(&mut line); } else { line.push(b); }
        }
    }
    take(&mut line);
    Ok((child.wait()?, log))
}

fn build_loop() {
    loop {
        let (why, paths) = {
            let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
            st.again = false;
            st.progress = BuildProgress::default();
            (st.why.clone(), st.paths.clone())
        };
        let before = stamps(&paths);
        let asked = RESTART_AFTER_BUILD.load(std::sync::atomic::Ordering::SeqCst);
        let (mut cmd, shown) = match command(if asked { Priority::Asked } else { Priority::Background }) {
            Ok(c) => c,
            Err(e) => return fail(format!("{why}; cannot rebuild {tool}: {e}; the app runs the previous code -- restart it by hand", tool = crate::tool_name())),
        };
        let Some(dir) = loader().as_deref().and_then(Path::parent).map(|d| d.join(".makepad-rebuilds")) else { return fail(format!("the {tool} binary's path is unknown", tool = crate::tool_name())) };
        let waits = || STATE.lock().unwrap_or_else(|e| e.into_inner()).progress.waiting = "another restart's build".into();
        let mut gate = match Gate::enter(&dir, asked, waits) {
            Ok(g) => g,
            Err(e) => return fail(format!("{why}; cannot rebuild {tool}: {}: {e}; the app runs the previous code", dir.display(), tool = crate::tool_name())),
        };
        STATE.lock().unwrap_or_else(|e| e.into_inner()).progress.waiting.clear();
        eprintln!("relax live: {why}; restarting {tool}: it builds first in the background (`{shown}`, a no-op when {tool} is current); the app keeps running the previous code", tool = crate::tool_name());
        super::live_log(&format!("{{\"event\":\"host_rebuild\",\"started\":true,\"why\":{}}}", super::json_str(&why)));
        let t0 = Instant::now();
        let started = |pid: u32| {
            if asked { return; }
            let Some(path) = gate.mark_low(pid) else { return };
            let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
            st.low_cargo = Some((pid, path));
            // (the click came while this cargo started)
            if RESTART_AFTER_BUILD.load(std::sync::atomic::Ordering::SeqCst) { end_low_cargo(&mut st); }
        };
        let (status, log) = match run_cargo(&mut cmd, started) {
            Ok(done) => done,
            Err(e) => return fail(format!("rebuilding {tool}: cannot start `{shown}`: {e}; the app runs the previous code", tool = crate::tool_name())),
        };
        STATE.lock().unwrap_or_else(|e| e.into_inner()).low_cargo = None;
        if gate.ended_for_asked() {
            eprintln!("relax live: a restart was asked for: its build goes first, at the person's priority");
            continue;
        }
        if !status.success() {
            return fail(format!("rebuilding {tool} FAILED ({status}); the app runs the previous code; fix and save to build again:\n  {}", errors(&log), tool = crate::tool_name()));
        }
        if STATE.lock().unwrap_or_else(|e| e.into_inner()).again || stamps(&paths) != before {
            eprintln!("relax live: sources changed during the build; building again");
            continue;
        }
        let Some(loader) = loader() else { return fail(format!("the {tool} binary's path is unknown", tool = crate::tool_name())) };
        // the new binary starts at all (its own codegen identity and provenance)
        match std::process::Command::new(&loader).arg("--version").stdin(std::process::Stdio::null()).output() {
            Ok(o) if o.status.success() => {}
            Ok(o) => return fail(format!("the rebuilt {tool} does not start ({}): {}; the app runs the previous code", o.status, String::from_utf8_lossy(&o.stderr).trim(), tool = crate::tool_name())),
            Err(e) => return fail(format!("the rebuilt {tool} does not start: {e}; the app runs the previous code", tool = crate::tool_name())),
        }
        let secs = t0.elapsed().as_secs_f64();
        let at_the_run = crate::run_threads::process_work();
        // the same binary as this process runs: the change did not reach makepad (a no-op
        // build), so there is nothing to restart into
        if STATE.lock().unwrap_or_else(|e| e.into_inner()).provided.is_none()
            && LOADED.get().copied().flatten().is_some_and(|id| identity(&loader).is_some_and(|now| now == id)) {
            let what = format!("rebuild produced no change: cargo left the {tool} this process runs as it was (the edit is not in what {tool} is built from, or was undone)", tool = crate::tool_name());
            // a live test session's tests already ran on this binary: no run answers it but this
            if crate::harness::live::enabled() { super::patch_refused_event(&why, true, std::slice::from_ref(&what)); }
            else { super::patch_verified_event(&format!("{why}: {what}"), 0, &[]); }
            super::live_log(&format!("{{\"event\":\"host_rebuild\",\"ok\":true,\"s\":{secs:.1},\"unchanged\":true}}"));
            let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
            if st.again { drop(st); continue; }
            st.phase = Phase::Idle;
            st.since = Some(Instant::now());
            // this makepad is what the sources build
            st.stale = false;
            let asked = RESTART_AFTER_BUILD.swap(false, std::sync::atomic::Ordering::SeqCst);
            drop(st);
            // makepad's own change: nothing is left to update to. The edits that wait for the
            // app's restart still wait (a struct's layout, a const in a type).
            super::drop_restart_pending(&why);
            if !asked {
                eprintln!("relax live: {why}: {tool} is unchanged (built in {secs:.0} s); no restart", tool = crate::tool_name());
                return;
            }
            // the relaunch the person asked for restarts the app, as one with no makepad change
            let rest = super::restart_pending().join("; ");
            let rest = if rest.is_empty() { "session relaunch".to_string() } else { rest };
            eprintln!("relax live: {why}: {tool} is unchanged (built in {secs:.0} s); restarting the app as asked ({rest})", tool = crate::tool_name());
            drop(at_the_run);
            if let Err(e) = super::restart_in_place(rest) { fail(format!("the relaunch cannot restart the app: {e}")); }
            return;
        }
        super::live_log(&format!("{{\"event\":\"host_rebuild\",\"ok\":true,\"s\":{secs:.1}}}"));
        let asked = {
            let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
            if st.again { drop(st); continue; }
            *HANDOVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Handover { #[cfg(unix)] loader, stamps: before });
            let asked = RESTART_AFTER_BUILD.swap(false, std::sync::atomic::Ordering::SeqCst);
            // (under the lock: the ring never sees the asked restart as one still to ask for)
            if asked { super::RESTART_CONFIRMED.store(true, std::sync::atomic::Ordering::SeqCst); }
            st.phase = Phase::Restarting;
            st.since = Some(Instant::now());
            asked
        };
        let reason = format!("{tool} was rebuilt ({why})", tool = crate::tool_name());
        // a live test session restarts its tests into it at once (its verdict: the restarted run)
        if crate::harness::live::enabled() {
            eprintln!("relax live: {tool} rebuilt in {secs:.0} s; the tests restart into it", tool = crate::tool_name());
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
        eprintln!("relax live: {tool} rebuilt in {secs:.0} s; applying the requested restart", tool = crate::tool_name());
        drop(at_the_run);
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
            eprintln!("relax live: the app reached a restart point; the rebuilt {tool} waits for `session relaunch`", tool = crate::tool_name());
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
            Some(st) => match super::restart_state_file(st) {
                Ok(path) => { cmd.env("RELAX_RESTORE", &path); }
                Err(e) => return fail(format!("restart into the rebuilt {tool}: {e}; the app runs the previous code", tool = crate::tool_name())),
            },
            None => { cmd.env_remove("RELAX_RESTORE"); }
        }
        let reason = super::RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()).clone();
        cmd.env("RELAX_RESTART_T0", super::RESTART_SAVED_MS.load(std::sync::atomic::Ordering::Relaxed).to_string());
        // the restart compiles the edited inputs: no native build this run took or made stands
        // for the next one (NATIVE_RECEIPTS)
        for name in super::NATIVE_RECEIPTS { cmd.env_remove(name); }
        super::restart_begins();
        // the run ends as at exit (exec skips the exit handlers); fd 2 goes back to the terminal
        // now (the transcript tee is a thread of this process: the new one starts its own), so
        // the line below and the new process's output are not lost in the old pipe
        crate::run::summary("restart");
        // a restart is a clean end of this process (its transcript is not kept)
        crate::tui::exit_code(0);
        crate::tui::finish(false);
        eprintln!("relax live: restarting: {reason}");
        super::hand_over(&mut cmd);
        let e = cmd.exec();
        super::hand_back();
        fail(format!("restart into the rebuilt {tool} failed: {e}; the app runs the previous code", tool = crate::tool_name()));
    }
    #[cfg(not(unix))]
    {
        let _ = (h, state);
        fail(format!("this platform cannot restart into a rebuilt {tool}; restart it by hand", tool = crate::tool_name()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The next run takes none of this run's native builds: a run's end in place forgets the
    /// native libraries it noted (they stay loaded; run.rs reset_statics), and a restart into a
    /// rebuilt loader hands it neither receipt (reload.rs NATIVE_RECEIPTS) and the rest of the
    /// environment as it was. This test runs again as the child that does both, and restarts into
    /// a loader that prints what it was given.
    #[cfg(unix)]
    #[test]
    fn the_next_run_takes_none_of_the_run_s_native_builds() {
        use std::os::unix::fs::PermissionsExt;
        const CHILD: &str = "RELAX_TEST_RESTART_LOADER";
        if let Some(dir) = std::env::var_os(CHILD) {
            // a handle no library has stands for one: noting it loads nothing
            crate::run::note_native_bundle(usize::MAX, "/relax-test/libnative.so");
            let noted = crate::run::native_bundle_count();
            unsafe { crate::run::reset_statics() };
            println!("native libraries noted {noted}, after the run's end {}", crate::run::native_bundle_count());
            std::env::set_var("RELAX_RUN_NATIVE", "run-native");
            std::env::set_var(crate::unitgraph::BIN_NATIVE_RECEIPT, "bin-native");
            std::env::set_var("RELAX_TEST_RESTART_KEPT", "kept");
            restart(Handover { loader: PathBuf::from(dir).join("loader"), stamps: Vec::new() }, None);
            panic!("the restart did not exec the rebuilt loader");
        }
        let dir = std::env::temp_dir().join(format!("relax-restart-loader-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let loader = dir.join("loader");
        std::fs::write(&loader, "#!/bin/sh\necho \"loader got [${RELAX_RUN_NATIVE-}] [${RELAX_BIN_NATIVE_RECEIPT-}] [${RELAX_TEST_RESTART_KEPT-}]\"\n").unwrap();
        std::fs::set_permissions(&loader, std::fs::Permissions::from_mode(0o700)).unwrap();
        let out = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "reload::host::tests::the_next_run_takes_none_of_the_run_s_native_builds", "--nocapture"])
            .env(CHILD, &dir));
        let _ = std::fs::remove_dir_all(&dir);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "the restarting child: {stdout}\n{}", String::from_utf8_lossy(&out.stderr));
        assert!(stdout.contains("native libraries noted 1, after the run's end 0"), "a run's end in place kept the native libraries it noted: {stdout}");
        assert!(stdout.contains("loader got [] [] [kept]"), "the rebuilt loader got a native build of the run, or lost the rest of its environment: {stdout}");
    }

    /// The gate's locks are flock locks, which a forked process holds with its parent until its
    /// exec or exit. Among the suite's tests, another test's child forked while the low build held
    /// the turn kept it held past mark_low, and the person's build found it taken ("the low build
    /// gave the turn up", once under the full suite's load). So the gate runs in a child of this
    /// test binary that runs this test alone: its only forks are its own children, whose spawn
    /// returns after their exec closed the gate's files (opened close-on-exec).
    #[cfg(unix)]
    #[test]
    fn the_person_s_build_ends_the_low_ones_and_goes_first() {
        use std::os::unix::process::{CommandExt, ExitStatusExt};
        const CHILD: &str = "RELAX_TEST_GATE_ALONE";
        if std::env::var_os(CHILD).is_none() {
            let out = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "reload::host::tests::the_person_s_build_ends_the_low_ones_and_goes_first", "--nocapture", "--test-threads=1"])
                .env(CHILD, "1"));
            let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert!(out.status.success() && stdout.contains(" 1 passed;"), "the gate's run alone: {stdout}\n{stderr}");
            return;
        }
        let dir = std::env::temp_dir().join(format!("relax-gate-test-{}", std::process::id()));
        // a process that lives until it is ended (`cat` on a pipe this test keeps open), not for a
        // time: a `sleep 30` could end on its own under load before the checks below
        let sleep = || std::process::Command::new("cat").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).process_group(0).spawn().unwrap();
        // a low-priority build, marked, and a marker a crashed makepad left behind
        let mut low = Gate::enter(&dir, false, || panic!("nothing holds the turn")).unwrap();
        let mut running = sleep();
        // (held here: `wait` would close it, and the build must end by the person's signal alone)
        let _running_input = running.stdin.take();
        low.mark_low(running.id()).unwrap();
        let mut unrelated = sleep();
        std::fs::write(dir.join(format!("low.{}", unrelated.id())), "").unwrap();
        let asked = Gate::enter(&dir, true, || panic!("the low build gave the turn up")).unwrap();
        assert_eq!(running.wait().unwrap().signal(), Some(15));
        assert!(low.ended_for_asked(), "the low build sees why it ended");
        assert_eq!(unrelated.try_wait().unwrap(), None, "a stale marker's pid is not ours to signal");
        assert!(!dir.join(format!("low.{}", unrelated.id())).exists());
        // a low build waits for the person's before it starts, and its cargo then finds it current
        let (done, entered) = std::sync::mpsc::channel();
        let waiter = {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let gate = Gate::enter(&dir, false, || {}).unwrap();
                done.send(()).unwrap();
                gate.ended_for_asked()
            })
        };
        assert!(entered.recv_timeout(std::time::Duration::from_millis(30)).is_err());
        drop(asked);
        entered.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(!waiter.join().unwrap(), "unmarked: nothing ended it");
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_cargo_says_is_read_from_its_own_lines() {
        assert_eq!(
            cargo_line("    Building [========>                    ] 1/3: relax-session(build), relax-base                    "),
            CargoLine::Progress { done: 1, total: 3, building: "relax-session(build)" },
        );
        assert_eq!(cargo_line("    Building [==========================>] 790/790"), CargoLine::Progress { done: 790, total: 790, building: "" });
        assert_eq!(cargo_line("    Blocking waiting for file lock on build directory"), CargoLine::Waiting("build directory"));
        for other in ["   Compiling relax-session v0.1.0 (/x)", "    Finished `release` profile [optimized] target(s) in 0.87s", "error[E0308]: mismatched types", "    Building something else", ""] {
            assert_eq!(cargo_line(other), CargoLine::Other, "{other}");
        }
    }
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&DOT, Mutex::new(None));
}
