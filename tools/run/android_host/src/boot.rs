//! Boot the host: compile and run a package through the host-owned HAL.
//! No platform code here, so the Linux probe (examples/probe.rs) runs the same path as the phone.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

/// Where the package's source comes from.
#[derive(Clone)]
pub enum Source {
    /// The current directory is a checkout (the Linux probe).
    Checkout,
    /// A packed source archive (`makepad pack-src`) at a byte range of a file: the APK's asset.
    Archive { file: PathBuf, offset: u64, len: u64 },
}

pub struct Boot {
    pub package: String,
    /// cargo flags after the package (`--no-default-features`, `--features ..`)
    pub cargo_flags: Vec<String>,
    pub source: Source,
    /// Relax's own executable: what build scripts run as
    pub exe: PathBuf,
    /// the app is the window manager: it may start, stop and embed other apps as domains
    pub privileged: bool,
    /// called with the workspace root once the archive is mounted (the embedded devhub node)
    pub on_mounted: Option<Box<dyn FnOnce(&str) + Send>>,
}

/// What the Activity shows while the host boots.
#[derive(Clone, Default)]
pub struct Status {
    /// "starting", "booting", "exited", "failed"
    pub state: &'static str,
    /// the last boot phase Relax finished ("unit graph", "build scripts", ...)
    pub phase: String,
    /// crates loaded of crates in the program
    pub loaded: u32,
    pub total: u32,
    pub error: String,
}

static NOTE: Mutex<String> = Mutex::new(String::new());
pub fn set_note(n: &str) { *NOTE.lock().unwrap() = n.to_string(); }

static STATUS: Mutex<Option<Status>> = Mutex::new(None);

pub fn set_state(state: &'static str, error: String) {
    let mut s = STATUS.lock().unwrap();
    let st = s.get_or_insert_with(Status::default);
    st.state = state;
    st.error = error;
}

/// The current status; while booting, Relax's progress is read at the call.
pub fn status() -> Status {
    let mut st = STATUS.lock().unwrap().clone().unwrap_or(Status { state: "starting", ..Status::default() });
    let failed = relax_compiler::os::proc::EMBEDDED_FAILURE.load(Ordering::Acquire);
    if failed != 0 {
        st.state = "failed";
        st.error = format!("makepad run could not run the app (status {failed}); see the log");
    }
    if st.phase.is_empty() { st.phase = NOTE.lock().unwrap().clone(); }
    if st.state == "booting" && relax_compiler::jit::first_present_seen() {
        st.state = "ready";
    } else if st.state == "booting" {
        let (phase, loaded, total) = relax_compiler::run::progress();
        st.phase = phase;
        st.loaded = loaded;
        st.total = total;
    }
    st
}

/// The compile-memory budget for this device (MAKEPAD_RUN_MEM_BUDGET_MB): the smaller of a quarter of the
/// physical RAM and 60% of what is available, at least 256 MB (/proc/meminfo).
pub fn memory_budget_mb() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb = |key: &str| -> Option<u64> { text.lines().find(|l| l.starts_with(key))?.split_whitespace().nth(1)?.parse().ok() };
    let (total, available) = (kb("MemTotal:")? / 1024, kb("MemAvailable:")? / 1024);
    Some((total / 4).min(available * 6 / 10).max(256))
}

/// The activity stopped (home, recents, the screen off): Android kills a process instead of
/// exiting it, so the loader's quit write runs now, on its own thread -- the startup manifest,
/// this run's code and front images and the collected program reach the cache, and the next
/// launch starts warm. The app keeps running; a later start of the activity resumes it.
fn background_quit_write() {
    crate::logcat::log("background: the compile cache's quit write starts, then the process exits so the exit write runs too (the next launch starts warm)");
    relax_compiler::run::quit_requested();
    // The quit write alone keeps only what the busy compile tiers let it take (a few hundred
    // entries); the exit write quiesces them and writes everything. Android restarts the
    // process on the next launch, from the warm cache.
    let _ = std::thread::Builder::new().name("background-exit".into()).spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(2000));
        std::process::exit(0);
    });
}

/// Compiles and runs the package main on this domain thread.
/// Runs on the boot thread: it holds the compiler's allocation zone while Relax loads the program.
pub fn boot(mut b: Boot) -> Result<(), String> {
    let _zone = relax_compiler::zone_alloc::compiler();
    relax_compiler::clock::mark_process_start();
    relax_runner::init_embedded(&b.exe, &makepad_run::HOST);
    makepad_hal::set_background_hook(background_quit_write);
    if b.privileged {
        relax_runner::set_privileged();
    }
    set_state("booting", String::new());
    let opts = Vec::new();
    let mut rest = vec![b.package.clone()];
    rest.extend(b.cargo_flags.iter().cloned());
    let code = match &b.source.clone() {
        Source::Checkout => relax_runner::run_checkout(opts, rest),
        Source::Archive { file, offset, len } => {
            let workspace = relax_runner::mount_archive(file, *offset, *len, None)?;
            if let Some(f) = b.on_mounted.take() {
                f(&workspace);
            }
            relax_runner::run_mounted(&workspace, opts, rest)
        }
    };
    if code != 0 {
        return Err(format!("makepad run could not boot {} (status {code}); see the log", b.package));
    }
    Ok(())
}
