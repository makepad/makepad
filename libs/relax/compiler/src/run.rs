//! `makepad run <package> [cargo flags] [-- app args]`: boots a whole cargo package from source
//! inside Relax and runs its `fn main`.
//!
//! Boot: cargo's unit graph for the package (host target, release, features as given), crate
//! substitution (std/substitutes.txt), build scripts (cfgs, link libraries, OUT_DIR,
//! rustc-env), then core/alloc/std (std unless MAKEPAD_RUN_STD says otherwise) and every crate in
//! dependency order, import resolution, signatures/layouts, the libraries the build scripts
//! link, and finally `main` through its lazy stub: every function compiles on its first call.
//! `--ahead=<level>` (MAKEPAD_RUN_AHEAD; jit::AheadPolicy) sets how much is compiled ahead of the
//! program's calls: `minimal`, `balanced` (default) or `aot`, which also compiles the functions the
//! previous run compiled (recorded per package in <target>/makepad-run/startup/<package>.startup,
//! dirs.rs) on a worker while the program runs.
//!
//! Options (before the package): `--units <json>` uses a saved unit graph instead of cargo;
//! `--ahead=<level>`; `--quiet` (no compile-error stream); `--verbose` (phase timings and info).
//! Env: MAKEPAD_RUN_VERBOSE=1, MAKEPAD_RUN_STD, MAKEPAD_RUN_ERRORS=N (resolve /
//! signature errors printed, default 20), MAKEPAD_RUN_REPORT=<path> (JSON boot report).
//! `--report-to <path>`: progress for a launcher (the runner app), see `report` below.
//! Errors stay on stderr; verbose adds phases and the exit summary. The program's first-present
//! hook prints its startup line from the MAKEPAD_RUN_* metadata published before main.

macro_rules! eprintln {
    ($($arg:tt)*) => { if !crate::diag::enabled() { stderr_line!($($arg)*); } };
}

// Callbacks without a session use the verbosity frozen by cmd_run_inner, never the environment.
macro_rules! verbose {
    ($($arg:tt)*) => { if VERBOSE.load(Ordering::Relaxed) { eprintln!($($arg)*); } };
}

use crate::jit::{self, FnKey, Unit};
use crate::program::{DefId, DefKind};
use crate::unitgraph;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

static mut RUN_UNIT: *mut Unit = std::ptr::null_mut();

/// The unit of the program `makepad run` booted (null before it did).
pub(crate) fn running_unit() -> *mut Unit {
    unsafe { RUN_UNIT }
}
static PHASES: relax_session::sync::Mutex<Vec<(String, f64)>> = relax_session::sync::Mutex::new(Vec::new());
static MAIN_START_MS: AtomicU64 = AtomicU64::new(0);
static ERRORS_SEEN: AtomicU64 = AtomicU64::new(0);
static QUIET: AtomicBool = AtomicBool::new(false);
static SILENT: AtomicBool = AtomicBool::new(false);
pub(crate) static VERBOSE: AtomicBool = AtomicBool::new(false);
static STARTUP_PATH: relax_session::sync::Mutex<String> = relax_session::sync::Mutex::new(String::new());
static REPORT_PATH: relax_session::sync::Mutex<String> = relax_session::sync::Mutex::new(String::new());
/// The package's root crate and manifest dir (MAKEPAD_RUN_MAIN_COMPILE_LOG's header).
static ROOT_DIR: relax_session::sync::Mutex<String> = relax_session::sync::Mutex::new(String::new());

/// The running program's root crate name and package dir (makepad inspect `asm`'s native build).
pub(crate) fn root_package() -> Option<(String, String)> {
    let g = ROOT_DIR.lock().unwrap();
    let (n, d) = g.split_once(' ')?;
    Some((n.to_string(), d.to_string()))
}

fn phase(name: &str, t: Instant) {
    phase_ms(name, t.elapsed().as_secs_f64() * 1e3);
}

fn phase_ms(name: &str, ms: f64) {
    if !SILENT.load(Ordering::Relaxed) {
        verbose!("makepad-run: {:<28} {:>9.1} ms   (t={:.0} ms)", name, ms, crate::clock::ms_since_start());
    }
    // the phase's name without its counts: "build scripts (12)" -> "build scripts"
    report("phase", name.split(" (").next().unwrap_or(name));
    { let _k = crate::zone_alloc::kept(); PHASES.lock().unwrap().push((name.to_string(), ms)); }
    crate::tui::phase_done(name);
    use crate::run_state::{self, Phase};
    let next = match name.split(" (").next().unwrap_or(name) {
        "build scripts" => Some(Phase::Parse),
        "load+parse" => Some(Phase::Resolve),
        "resolve" => Some(Phase::Typecheck),
        "signatures" => Some(Phase::Link),
        "image" => Some(Phase::Codegen),
        _ => None,
    };
    if let Some(next) = next { run_state::progress(None, next, None, None, ""); }
    // makepad inspect's memory timeline: the boot phase that just ended
    crate::inspect::mem::mark(&format!("{} done", name.split(" (").next().unwrap_or(name)));
}

// ---- `--report-to <path>`: progress lines for a launcher (the runner app)
//
// Appended to <path> (a file or a FIFO), one event per line, tab-separated, each with the
// milliseconds since Relax started:
//   phase <name> <ms>      a boot phase finished (unit graph, build scripts, load+parse,
//                          resolve, signatures, link libraries, image)
//   main <ms>              boot done, `main` called
//   fns <n> <ms>           functions compiled so far (every 100 ms while it changes)
//   error <text> <ms>      a compile error (newlines and tabs as spaces)
//   panic <text> <ms>      main panicked: kind, message and Relax's crash report
//   exit <code> <ms>       main returned
//   progress <done> <total|-> <label> <ms>   a git store fetch of the launch (progress.rs),
//                          throttled to whole percents
// The program gets MAKEPAD_RUN_REPORT_TO=<path>: the makepad platform appends `present` (no time:
// the launcher times every line on receipt) at its first frame on glass. Writes are whole lines with O_APPEND, so the
// writers never interleave within a line.
static REPORT_FILE: relax_session::sync::Mutex<Option<std::fs::File>> = relax_session::sync::Mutex::new(None);

pub fn report_open(path: &str) -> Result<(), String> {
    let f = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| format!("--report-to {}: {}", path, e))?;
    *REPORT_FILE.lock().unwrap() = Some(f);
    crate::progress::set_report(report);
    unsafe { std::env::set_var("MAKEPAD_RUN_REPORT_TO", path) };
    Ok(())
}

pub(crate) fn report(kind: &str, text: &str) {
    if kind == "error" && crate::run_state::observed() {
        crate::run_state::diagnostic(app_domain(), crate::diag::from_legacy(text, "codegen"));
    }
    use std::io::Write;
    let mut g = REPORT_FILE.lock().unwrap();
    if let Some(f) = g.as_mut() {
        let text: String = text.chars().map(|c| if c == '\n' || c == '\t' || c == '\r' { ' ' } else { c }).collect();
        let _ = f.write_all(format!("{}\t{}\t{:.0}\n", kind, text, crate::clock::ms_since_start()).as_bytes());
    }
}

/// A start that ends before `main` (an unreachable store, a resolve error): its reason and exit
/// code for the launcher following it.
pub fn report_failed(error: &str, code: i32) {
    report("error", error);
    report("exit", &code.to_string());
}

pub(crate) fn reporting() -> bool {
    REPORT_FILE.lock().unwrap().is_some()
}

extern "C" {
    fn atexit(f: extern "C" fn()) -> i32;
}

fn usage() -> i32 {
    eprintln!("usage: makepad run [--units <unit-graph.json>] [--ahead=minimal|balanced|aot-max|aot-min] [--quiet] [--verbose] [--no-asm] [--allow-c] <package> [cargo flags] [-- program args]");
    2
}

static GANDALF_CONSOLE: relax_session::sync::Mutex<Option<crate::gandalf_out::Console>> = relax_session::sync::Mutex::new(None);

pub fn cmd_run(sess: &std::sync::Arc<crate::Session>, args: &[String]) -> i32 {
    let json = args.iter().any(|a| a == "--json") && args.iter().any(|a| a == "--check");
    let start = std::time::Instant::now();
    if json { crate::diag::enable(); }
    if args.iter().any(|a| a == "--watch") {
        session_cache_off();
    }
    if !args.iter().any(|a| a == "--check") {
        let _k = crate::zone_alloc::kept();
        *GANDALF_CONSOLE.lock().unwrap() = Some(crate::gandalf_out::Console::start(crate::diag::enabled(), args.iter().any(|a| a == "--quiet" || a == "--silent")));
    }
    let host = MAIN_HOST.get().filter(|_| {
        !sess.runtime.boot_only && std::env::var_os("MAKEPAD_RUN_SCRIPT_DEPTH").is_none()
            && !args.iter().take_while(|a| a.as_str() != "--").any(|a| matches!(a.as_str(), "--check" | "--watch" | "--entry" | "--exports" | "--harness"))
    }).filter(|_| hosted_program(sess, args));
    let code = if let Some(host) = host {
        HOSTED_RUN.store(true, Ordering::Release);
        let sess = sess.clone();
        let args = args.to_vec();
        (host.run)(Box::new(move || cmd_run_inner(&sess, &args, None)))
    } else {
        cmd_run_inner(sess, args, None)
    };
    crate::tui::exit_code(code);
    if let Some(exit) = EXIT_SUMMARY.get() {
        // atexit runs after application TLS destructors. Do not quiesce earlier:
        // those destructors may still call JIT code on the returning main thread.
        let _ = exit.code.set(code);
    } else {
        if let Some(console) = GANDALF_CONSOLE.lock().unwrap().take() { console.detach(); }
        crate::why::event("boundary", "run-exit", 0, "", Some(crate::why::Cause::new("", 0, "exit", &code.to_string())));
    }
    if json && (code != 0 || !args.iter().any(|a| a == "--watch")) {
        if code != 0 && crate::diag::emitted_errors() == 0 {
            crate::diag::emit(&crate::diag::Diagnostic::new("run", "", 0, 0, "check could not load or initialize the package"));
        }
        println!("{{\"type\":\"summary\",\"errors\":{},\"warnings\":0,\"ms\":{}}}", crate::diag::emitted_errors(), start.elapsed().as_millis());
    }
    code
}

/// Whether the program is a HAL program: its crate graph links makepad-hal (or the run starts
/// `--domain` packages into it). Only such a program gets the main-thread host (the HAL's main
/// loop, the window, the boot screen); any other program -- a terminal tool, a pure-Rust crate --
/// runs on the process's main thread with its output on the terminal, as its rustc build does.
/// The graph resolved here is the boot's (BOOT_UNITS): the resolve does not run twice.
fn hosted_program(sess: &std::sync::Arc<crate::Session>, args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() && args[i].starts_with("--") {
        match args[i].as_str() {
            // a build script's child (a saved graph) or a re-run: not a host decision made here
            "--units" => return true,
            "--report-to" | "--run-dir" | "--workspace-dir" | "--cfg" | "--boundary" => i += 2,
            _ => i += 1,
        }
    }
    let Some(package) = args.get(i).cloned() else { return false };
    let mut flags = Vec::new();
    let mut words = args[i + 1..].iter().take_while(|a| a.as_str() != "--");
    while let Some(w) = words.next() {
        match w.as_str() {
            "--domain" => return true,
            "--cfg" => { words.next(); }
            _ if w.starts_with("--cfg=") => {}
            _ => flags.push(w.clone()),
        }
    }
    let Ok(units) = unitgraph::unit_graph(sess, &workspace_dir(), &package, &flags) else { return false };
    let hal = units.units.iter().any(|u| u.name.replace('-', "_") == "makepad_hal");
    *BOOT_UNITS.lock().unwrap() = Some((package, flags, units));
    hal
}

/// The unit graph `hosted_program` resolved for the boot, taken by the boot's resolve.
static BOOT_UNITS: relax_session::sync::Mutex<Option<(String, Vec<String>, unitgraph::Units)>> = relax_session::sync::Mutex::new(None);

/// A warm check session runs with the disk cache off (unless MAKEPAD_RUN_CACHE says otherwise): with
/// cached code installed, constant evaluation in a session crashed (makepad-platform's
/// font_policy consts, 2026-10-07) -- to be fixed, then the cache can serve the cold start.
fn session_cache_off() {
    if std::env::var_os("MAKEPAD_RUN_CACHE").is_none() {
        unsafe { std::env::set_var("MAKEPAD_RUN_CACHE", "off") };
    }
}

/// Load the same front end as `makepad check`, transferring ownership to a warm caller.
pub fn load_check_session(sess: &std::sync::Arc<crate::Session>, args: &[String]) -> Result<crate::serve::Session, String> {
    session_cache_off();
    let mut a = vec!["--check".to_string()]; a.extend_from_slice(args);
    let mut session = None;
    let code = cmd_run_inner(sess, &a, Some(&mut session));
    session.ok_or_else(|| format!("check could not load or initialize the package (status {})", code))
}

/// Who asks for a package's crate list: the boot of a run, or a domain started into a running
/// unit (domain.rs `start`). The domain's graph is registered with the macro host additively and
/// binds no provided crate (the unit's are bound).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolveFor {
    Boot,
    Domain(crate::domain::DomainId),
}

/// A package resolved to the crates that make it up: cargo's unit graph (or a saved one), the
/// substitutions of provided crates, the build scripts run (their cfgs, env and link libraries),
/// and each crate's cfg set. `makepad run` and `domain::start` build exactly this, then load.
/// The host window's opening (boot of a hosted run), started by `resolve_package` and joined by
/// `finish_window`.
static PENDING_WINDOW: relax_session::sync::Mutex<Option<std::io::Result<std::thread::JoinHandle<Result<(), String>>>>> = relax_session::sync::Mutex::new(None);

/// Waits for the host window the resolve started to open (it opens beside the load, which does not
/// need it), and says whether the run is hosted (HAL_RUN).
fn finish_window() -> Result<(), String> {
    let Some(window) = PENDING_WINDOW.lock().unwrap().take() else { return Ok(()) };
    let t_window = Instant::now();
    let opened = window.map_err(|e| format!("opening the host window: {e}"))?.join().map_err(|_| "opening the host window: the thread panicked".to_string())?;
    opened?;
    if crate::preload::trace() { eprintln!("boot-trace: host window open, waited {:.1} ms for it after the load", t_window.elapsed().as_secs_f64() * 1e3); }
    HAL_RUN.store(!MAIN_HOST.get().expect("run host").domain_zero, Ordering::Release);
    Ok(())
}

pub(crate) struct Resolved {
    pub units: unitgraph::Units,
    pub g: unitgraph::Graph,
    /// crate indices of `g`, every crate after its dependencies
    pub order: Vec<usize>,
    /// per crate: its cfg set (features, relax, the build script's cfgs)
    pub cfgs: Vec<crate::cfg::CfgSet>,
    /// the files a live session watches (boot only)
    pub live_inputs: Option<crate::reload::Inputs>,
}

/// The boot's resolved package, handed to the app domain it starts (once: a relaunch resolves
/// again, so an edited source or manifest is read).
static HANDOFF: relax_session::sync::Mutex<Option<Resolved>> = relax_session::sync::Mutex::new(None);

pub(crate) fn take_boot_resolved() -> Option<Resolved> {
    HANDOFF.lock().unwrap_or_else(|e| e.into_inner()).take()
}


/// rustc's crate locator finds `extern crate proc_macro;` in the sysroot for any crate (a
/// `#![crate_type = "proc-macro"]` file checked on its own, a test's helper). Relax's sysroot is
/// its std dir, whose `proc_macro` the app graph otherwise never loads (proc-macro crates run in the
/// macro host): a check whose root unit declares the crate gets it as a dependency unit.
fn with_sysroot_proc_macro(mut units: unitgraph::Units, std_dir: &str) -> unitgraph::Units {
    let pm_root = format!("{}/proc_macro/lib.rs", std_dir);
    if units.units.iter().any(|u| u.name == "proc_macro") || !crate::vfs::exists(&pm_root) { return units; }
    let wants = |u: &unitgraph::GraphUnit| crate::vfs::read_shared(&u.src_path).is_ok_and(|src| {
        const NEEDLE: &[u8] = b"extern crate proc_macro";
        src.windows(NEEDLE.len()).any(|w| w == NEEDLE)
    });
    let roots: Vec<usize> = units.roots.iter().copied().filter(|&r| r < units.units.len() && !units.units[r].kind.iter().any(|k| k == "proc-macro") && wants(&units.units[r])).collect();
    if roots.is_empty() { return units; }
    let pm = units.units.len();
    units.units.push(unitgraph::GraphUnit {
        pkg_id: format!("path+file://{}/proc_macro#proc_macro@0.0.0", std_dir),
        kind: vec!["lib".to_string()], name: "proc_macro".to_string(), src_path: pm_root, edition: "2021".to_string(),
        host: false, mode: "build".to_string(), features: Vec::new(), deps: Vec::new(),
    });
    for r in roots { units.units[r].deps.push((pm, "proc_macro".to_string())); }
    units
}

pub(crate) fn resolve_package(
    sess: &std::sync::Arc<crate::Session>,
    workspace: &str,
    package: &str,
    cargo_flags: &[String],
    units_json: &str,
    target_dir: &str,
    std_dir: &str,
    who: ResolveFor,
    check_only: bool,
) -> Result<Resolved, String> {
    let boot = who == ResolveFor::Boot;
    let checkpoint = |name: &str| match who {
        ResolveFor::Domain(id) => crate::domain::compile_phase(id, name),
        ResolveFor::Boot => Ok(()),
    };
    checkpoint("resolve")?;
    let options = &sess.options;
    let _mem = crate::memstat::tag(crate::memstat::Tag::Boot);
    // ---- crate graph
    let t = Instant::now();
    // the resolver's graph in memory; a build script's child reads its units from JSON
    let pre = if boot {
        BOOT_UNITS.lock().unwrap().take()
    } else { None };
    let units = if let Some((_, _, units)) = pre.filter(|(p, f, _)| units_json.is_empty() && p == package && f == cargo_flags) {
        Ok(units)
    } else if units_json.is_empty() {
        unitgraph::unit_graph(sess, workspace, package, cargo_flags)
    } else {
        std::fs::read(units_json).map_err(|e| format!("{}: {}", units_json, e)).and_then(|d| unitgraph::Units::from_json(&d))
    }?;
    let units = if check_only { with_sysroot_proc_macro(units, std_dir) } else { units };
    let t_load = Instant::now();
    let mut g = unitgraph::load(&units, check_only)?;
    let t_loaded = Instant::now();
    // The graph is enough to choose the host. Its window opens now, beside the rest of the
    // resolve (the watch inputs, the substitutions, the hash and the build scripts); the process
    // main thread is already serving the HAL. The resolve ends when the window is open.
    let window = (boot && HOSTED_RUN.load(Ordering::Acquire) && g.crates.iter().any(|c| c.name == "makepad_hal")).then(|| {
        let (package, dir) = (package.to_string(), g.pkg[g.root].0.clone());
        let _k = crate::zone_alloc::kept();
        std::thread::Builder::new().name("run-open-window".into()).spawn(move || {
            let _z = crate::zone_alloc::compiler_thread();
            (MAIN_HOST.get().expect("run host").open)(&package, &dir)
        })
    });
    // provided crates bind only to their own source (before substitution rewrites deps)
    let pkgs: Vec<crate::srchash::SrcPkg> = g
        .crates
        .iter()
        .zip(g.pkg.iter())
        .map(|(c, (dir, name, ver))| crate::srchash::SrcPkg { name: name.clone(), version: ver.clone(), dir: dir.clone(), features: c.features.clone(), deps: c.deps.iter().map(|d| d.0).collect(), proc_macro: c.proc_macro })
        .collect();
    let watches = boot || matches!(who, ResolveFor::Domain(id) if app_domain() == Some(id));
    let t_inputs = Instant::now();
    let live_inputs = (watches && options.live).then(|| crate::reload::Inputs::collect(&g, std_dir, workspace));
    let t_subst = Instant::now();
    // Validate native macros before any overlapping provided-crate identity error.
    crate::crates::apply_substitutes(sess, &mut g.crates, std_dir, !boot).map_err(|e| format!("crate substitution: {e}"))?;
    let t_subst_done = Instant::now();
    {
        // The loader's provided crates are host code built for the machine: their shims bind
        // only where the program is read for the machine too. A program read for another target
        // (MAKEPAD_RUN_CHECK_TARGET) has a different crate graph (makepad-script-std reaches
        // makepad-windows-sys on Windows) and only type-checks: it never runs the crates' code,
        // so nothing binds and every provided crate is read from source.
        // `makepad check` runs none of it either: every provided crate is read from source,
        // whatever source the loader was built from (an identity mismatch refused to check).
        // The boundary audit (`check --boundary`) binds as a run does: what it reports is what
        // JIT-compiled code reaches past the bound ops.
        let machine = crate::cfg::CfgSet::machine(sess, &[]).triple();
        // A test harness (`makepad test`) binds none either: its crates are the tests' subject, built
        // from their source as `cargo test` builds them, internals and all.
        let bind = crate::cfg::CfgSet::host(sess, &[]).triple() == machine && (!check_only || crate::boundary::requested()) && !HARNESS_RUN.load(Ordering::Acquire);
        // The sources are hashed while the build scripts run (and, for a run's boot, while the
        // program loads): they share nothing (the hash reads the provided packages, the scripts
        // write the graph's cfgs and env).
        if boot { phase("unit graph", t); }
        checkpoint("build scripts")?;
        let t_scripts = Instant::now();
        // The boot of a run does not wait for the hash at all: the load goes on and the unit is not
        // built before the hash has passed (`finish_sources` below). A check, a domain's own
        // resolve and a failing hash all end here.
        let later = bind && boot && !check_only;
        let pkgs = std::sync::Arc::new(pkgs);
        if !bind { crate::provided::read_identity_files(&pkgs); }
        if later { crate::provided::check_sources_later(pkgs.clone()); }
        let (sources, scripts, scripts_ms, wait_ms) = std::thread::scope(|scope| {
            let sources = (bind && !later).then(|| scope.spawn(|| {
                let _z = crate::zone_alloc::compiler_thread();
                let t = Instant::now();
                (crate::provided::check_sources(&pkgs), t.elapsed())
            }));
            // `--allow-c`: cargo builds the native dependencies (front native.rs); a build
            // script's own child run never builds them again
            let triple = crate::cfg::CfgSet::host(sess, &[]).triple();
            let native_request = (options.allow_c && std::env::var_os("MAKEPAD_RUN_SCRIPT_DEPTH").is_none()).then(|| crate::native::Request { workspace, package, cargo_flags, relax_dir: target_dir, triple });
            let scripts = unitgraph::run_build_scripts(sess, &mut g, target_dir, native_request.as_ref());
            let scripts_ms = t_scripts.elapsed().as_secs_f64() * 1e3;
            let t_wait = Instant::now();
            (sources.map(|h| h.join().expect("provided source check")), scripts, scripts_ms, t_wait.elapsed().as_secs_f64() * 1e3)
        });
        if boot {
            phase_ms(&format!("build scripts ({})", g.build_scripts.len()), scripts_ms);
            if !later { phase_ms("provided sources", wait_ms); }
        }
        if crate::preload::trace() {
            if let Some((_, took)) = &sources { eprintln!("boot-trace: provided sources hashed in {:.1} ms (beside the build scripts)", took.as_secs_f64() * 1e3); }
        }
        if let Some((checked, _)) = sources { checked?; }
        scripts?;
        crate::macro_reg::set_build_outputs(sess, &g.crates, &g.cfgs, &g.env);
    }
    if crate::preload::trace() {
        eprintln!("boot-trace: unit graph (resolve) {:.1} ms, graph {:.1} ms (load {:.1}, watch inputs {:.1}, substitutes {:.1})", (t_load - t).as_secs_f64() * 1e3, t_load.elapsed().as_secs_f64() * 1e3,
            (t_loaded - t_load).as_secs_f64() * 1e3, (t_subst - t_inputs).as_secs_f64() * 1e3, t_subst_done.duration_since(t_subst).as_secs_f64() * 1e3);
    }
    checkpoint("loading crates")?;
    if boot {
        // the scripts that still run (as Relax programs) that the source is to lose (unitgraph::script_role)
        let mut l = unitgraph::TO_REMOVE.lock().unwrap().clone();
        if !l.is_empty() && !SILENT.load(Ordering::Relaxed) {
            l.sort();
            let names: Vec<String> = l.iter().map(|(p, src)| format!("{} ({})", p, src)).collect();
            verbose!("makepad-run: build scripts to remove ({}; only makepad-platform keeps one): {}", l.len(), names.join(", "));
        }
    }
    if boot {
        let _k = crate::zone_alloc::kept();
        *CRATE_DIRS.lock().unwrap() = g.crates.iter().zip(g.pkg.iter()).map(|(c, (dir, _, _))| (c.name.clone(), dir.clone())).collect();
    }
    // every crate's cfg set (its env!() environment is set when it is loaded: `set_crate_env`)
    // the root package's crates, and whatever depends on them, load last: the crates an edit
    // of the app reaches are a suffix from the first start on (FrontImages::load_order keeps
    // that for the crates edited later)
    let order = {
        let topo = crate::crates::topo_order(&g.crates);
        let root_dir = &g.pkg[g.root].0;
        let mut late: Vec<bool> = g.pkg.iter().map(|(dir, _, _)| dir == root_dir).collect();
        for &ci in &topo { if !late[ci] && g.crates[ci].deps.iter().any(|(d, _)| late.get(*d).copied().unwrap_or(false)) { late[ci] = true; } }
        let (mut first, last): (Vec<usize>, Vec<usize>) = topo.into_iter().partition(|&ci| !late[ci]);
        first.extend(last);
        first
    };
    let mut cfgs: Vec<crate::cfg::CfgSet> = Vec::with_capacity(g.crates.len());
    for (ci, c) in g.crates.iter().enumerate() {
        let mut cs = crate::program_cfg(sess, &c.features);
        for x in &g.cfgs[ci] {
            cs.set.push(x.replace('"', ""));
        }
        cfgs.push(cs);
    }
    // The window is still opening: the load goes on beside it and `finish_window` joins it where
    // the run needs the host (before the program starts).
    if let Some(window) = window { *PENDING_WINDOW.lock().unwrap() = Some(window); }
    Ok(Resolved { units, g, order, cfgs, live_inputs })
}

/// The warm front end: the program and type context restored from this launch's collected
/// image, and the proof the cache opens with. None: no usable image (the caller loads the sources).
fn warm_front(images: &crate::cache::FrontImages, sess: &std::sync::Arc<crate::Session>, pre: &[crate::preload::PreCrate])
    -> Option<(crate::program::Program, crate::tcx::Tcx, Option<crate::cache::CollectedCache>)> {
    images.restore_collected(sess, pre)
}


fn cmd_run_inner(sess: &std::sync::Arc<crate::Session>, args: &[String], session: Option<&mut Option<crate::serve::Session>>) -> i32 {
    PHASES.lock().unwrap().clear();
    ERRORS_SEEN.store(0, Ordering::Relaxed);
    crate::clock::mark_process_start();
    jit::begin_startup_phase();
    let mut driver_options = sess.options.clone();
    crate::registry_net::install();
    crate::pmhost::install();
    if let Err(e) = sess.runtime.validate_native_macros() {
        eprintln!("makepad-run: {e}");
        return 1;
    }
    jit::COMPILE_ERRORS_FATAL.store(true, Ordering::Relaxed);
    jit::FAULTS_FATAL.store(true, Ordering::Relaxed);
    crate::clock::mark_start();
    let _ = jit::wall_ns();
    let mut units_json = String::new();
    if let Some(v) = std::env::var_os("MAKEPAD_RUN_AHEAD") {
        if v.to_str().and_then(crate::options::AheadLevel::parse).is_none() {
            eprintln!("makepad-run: MAKEPAD_RUN_AHEAD={} is not a level: minimal, balanced, aot-max or aot-min", v.to_string_lossy());
            return 2;
        }
    }
    jit::set_ahead_level(sess.runtime.ahead);
    let mut entries: Vec<String> = Vec::new();
    // `--exports a,b`: compile (and publish the addresses of) the `#[no_mangle]` / `#[export_name]`
    // fns whose names start with one of these, then return to the caller instead of running `main`
    // (the Android host: the Activity's JNI natives are these fns, `exports()`)
    let mut export_names: Vec<String> = Vec::new();
    // `--no-live`: a working tree's run that is no live session (a trace host: what it runs must
    // be the sources it started with, never a hot reload of an edit made meanwhile)
    let mut no_live = false;
    let mut check = false;
    let mut watch = session.is_some();
    let mut deep = false;
    let mut boundary: Option<Vec<String>> = None;
    let mut root_only = false;
    let mut selftest = false;
    // `makepad check --workspace`: every workspace crate the package reaches, not only its app crates
    let mut workspace_check = false;
    // `makepad test <package>` (lib.rs): the program is the package's test harness, not its main
    let mut harness = false;
    let mut no_fail_fast = false;
    let mut i = 0;
    while i < args.len() && args[i].starts_with("--") {
        match args[i].as_str() {
            "--units" if i + 1 < args.len() => {
                units_json = args[i + 1].clone();
                i += 1;
            }
            // how much is compiled ahead of the program's calls (jit::AheadPolicy); the
            // environment carries it to processes this one starts
            a if a.starts_with("--ahead=") => match crate::options::AheadLevel::parse(&a[8..]) {
                Some(l) => {
                    jit::set_ahead_level(l);
                    std::env::set_var("MAKEPAD_RUN_AHEAD", l.name());
                }
                None => {
                    eprintln!("makepad-run: --ahead=<level> wants minimal, balanced, aot-max or aot-min");
                    return 2;
                }
            },
            "--harness" => {
                harness = true;
                HARNESS_RUN.store(true, Ordering::Release);
                // a test whose code Relax cannot compile fails as that test; the others still run
                jit::COMPILE_ERRORS_FATAL.store(false, Ordering::Relaxed);
            }
            "--no-fail-fast" => no_fail_fast = true,
            // `makepad selftest --root-only`: the root crate's fns and what they reach
            "--root-only" => root_only = true,
            // `makepad selftest` (lib.rs): the check, then every fn compiled (check.rs)
            "--selftest" => selftest = true,
            "--workspace" => workspace_check = true,
            "--verbose" => driver_options.verbose = true,
            // read into the session (session_from_env)
            "--no-asm" | "--allow-c" => {}
            "--report-to" if i + 1 < args.len() => {
                if let Err(e) = report_open(&args[i + 1]) {
                    eprintln!("makepad-run: {}", e);
                    return 2;
                }
                i += 1;
            }
            // run these fns (no-argument, by def path) one after another instead of `main`
            "--entry" if i + 1 < args.len() => {
                entries.push(args[i + 1].clone());
                i += 1;
            }
            "--cfg" if i + 1 < args.len() => {
                driver_options.cfgs.push(crate::options::Options::cfg_entry(&args[i + 1]));
                i += 1;
            }
            "--exports" if i + 1 < args.len() => {
                export_names.extend(args[i + 1].split(',').filter(|n| !n.is_empty()).map(str::to_string));
                i += 1;
            }
            "--quiet" => QUIET.store(true, Ordering::Relaxed),
            "--no-live" => no_live = true,
            // non-lazy Gandalf: no body runs before the borrow checker accepted it (jit::CHECK_FIRST)
            "--check-first" => jit::CHECK_FIRST.store(true, Ordering::Relaxed),
            // the parent run's relax dir (dirs.rs): a build script it runs shares it
            "--run-dir" if i + 1 < args.len() => {
                crate::dirs::set_relax_dir(std::path::PathBuf::from(&args[i + 1]));
                i += 1;
            }
            "--workspace-dir" if i + 1 < args.len() => {
                crate::dirs::set_workspace_dir(args[i + 1].clone());
                i += 1;
            }
            // nothing of Relax's own on stderr but errors (a program re-running itself)
            "--silent" => {
                QUIET.store(true, Ordering::Relaxed);
                SILENT.store(true, Ordering::Relaxed);
            }
            // type/borrow check (`--deep` also compiles), run nothing
            "--check" => check = true,
            "--watch" => watch = true,
            // makepad inspect: panics park for an agent and events are printed on stderr
            // `--agent` also tracks the heap from the start (sampled, the default rate) unless
            // `--heap=<rate>` set it
            "--agent" => {
                crate::inspect::AGENT.store(true, Ordering::Relaxed);
                let _ = crate::inspect::heap::AT_LOAD.compare_exchange(0, 64 << 10, Ordering::Relaxed, Ordering::Relaxed);
            }
            // makepad inspect: sampled heap tracking (program + Relax's own) from the first
            // allocation, one sample per <rate> bytes (default 64 KB)
            "--heap" => crate::inspect::heap::AT_LOAD.store(64 << 10, Ordering::Relaxed),
            a if a.starts_with("--heap=") => match a[7..].parse::<u64>() {
                Ok(r) if r > 0 => crate::inspect::heap::AT_LOAD.store(r, Ordering::Relaxed),
                _ => {
                    eprintln!("makepad-run: --heap=<bytes per sample> wants a positive number");
                    return 2;
                }
            },
            "--deep" => deep = true,
            "--json" if crate::diag::enabled() => {
                QUIET.store(true, Ordering::Relaxed);
                SILENT.store(true, Ordering::Relaxed);
            },
            // relaxed Rust for every crate of this session (relaxed.rs): all features, or a list
            "--relaxed" => driver_options.relaxed = Some("all".to_string()),
            a if a.starts_with("--relaxed=") => driver_options.relaxed = Some(a[10..].to_string()),
            // with --check: list every site only relaxed mode accepts (what rustc would reject)
            "--strict-report" => crate::relaxed::STRICT_REPORT.store(true, Ordering::Relaxed),
            // `--boundary a,b`: with --check, audit what JIT-compiled code reaches inside
            // crates a, b (boundary.rs)
            "--boundary" => {
                crate::boundary::REQUESTED.store(true, Ordering::Relaxed);
                i += 1;
                boundary = args.get(i).map(|s| s.split(',').map(|x| x.to_string()).collect());
            }
            _ => return usage(),
        }
        i += 1;
    }
    // the level in effect, for the program's first-frame line and the processes it starts
    std::env::set_var("MAKEPAD_RUN_AHEAD", jit::ahead_level().name());
    if i >= args.len() {
        return usage();
    }
    driver_options.verbose &= !SILENT.load(Ordering::Relaxed);
    VERBOSE.store(driver_options.verbose, Ordering::Relaxed);
    if driver_options.verbose { crate::tui::track_for_verbose(); }
    // the boot progress line (tui.rs): a terminal run of a program, with nothing asking for plain output
    unsafe { std::env::remove_var("MAKEPAD_RUN_PROGRESS") };
    // a test harness is a terminal program: no progress line, no inspect socket, no live session
    let hosted = !entries.is_empty() || !export_names.is_empty() || harness;
    // the `unused_macros` lints need the macro uses of the whole load
    if check { relax_types::macros::uses::record(true); relax_front::program::set_record_import_uses(true); }
    // Android: stderr is the embedder's pipe into logcat, line by line; the session-log tee in
    // front of it held lines back for up to a minute and lost a crashing run's last lines
    if !check && !watch && session.is_none() && !hosted && !cfg!(target_os = "android") {
        crate::tui::start(!driver_options.verbose && !QUIET.load(Ordering::Relaxed) && !crate::diag::enabled());
    }
    crate::memstat::start_sampler();
    // `makepad run` of a working tree is a live session (watcher + hot reload); sources served from
    // a store (`makepad run <git url>`, mounted bundles) are not, and neither are checks and the
    // build scripts / self re-runs a program starts.
    // A store run whose embedder writes pushed sources into its edits folder (the Android host's
    // devhub store applier) asks for a live session with MAKEPAD_RUN_STORE_LIVE=1.
    let store_live = std::env::var_os("MAKEPAD_RUN_STORE_LIVE").is_some_and(|v| v == "1") && crate::vfs::edits_enabled();
    if !check && !harness && !no_live && std::env::var_os("MAKEPAD_RUN_SCRIPT_DEPTH").is_none() && !SILENT.load(Ordering::Relaxed)
        && (store_live || !crate::vfs::is_virtual(&workspace_dir())) {
        driver_options.live = true;
    }
    // `--cfg <name>` / `--cfg=<name>` among the cargo flags (`makepad run <pkg> --cfg custom`) or
    // before the package: extra cfgs for the program's crates, not cargo flags
    let mut cargo_flags_in: Vec<String> = Vec::new();
    // `--domain <package>` (a test vehicle, any number): once the program has presented its first
    // frame, start each of these packages as a domain of this process (domain.rs)
    let mut domain_packages: Vec<String> = Vec::new();
    {
        let mut words = args[i..].iter().cloned();
        let mut in_prog = false;
        while let Some(w) = words.next() {
            if in_prog || w == "--" {
                in_prog = true;
                cargo_flags_in.push(w);
            } else if w == "--domain" {
                match words.next() {
                    Some(v) => domain_packages.push(v),
                    None => {
                        eprintln!("makepad-run: --domain wants a package");
                        return 2;
                    }
                }
            } else if w == "--cfg" {
                match words.next() {
                    Some(v) => driver_options.cfgs.push(crate::options::Options::cfg_entry(&v)),
                    None => {
                        eprintln!("makepad-run: --cfg wants a value");
                        return 2;
                    }
                }
            } else if let Some(v) = w.strip_prefix("--cfg=") {
                driver_options.cfgs.push(crate::options::Options::cfg_entry(v));
            } else {
                cargo_flags_in.push(w);
            }
        }
    }
    let args: &[String] = &{
        let mut a = args[..i].to_vec();
        a.extend(cargo_flags_in);
        a
    };
    // a check runs code only to evaluate constants: their loops count iterations (long_running_const_eval)
    driver_options.const_fuel = check;
    // MAKEPAD_RUN_CHECK_EAGER=1 (check only): load every file of the provided crates instead of the demand
    // plan's selection, so their front images do not depend on the checked crate (a sweep shares one
    // read-only image cache)
    driver_options.source_eager = check && std::env::var("MAKEPAD_RUN_CHECK_EAGER").is_ok_and(|v| v == "1");
    // kept: the session is process state (the program's threads hold it)
    let frozen = crate::zone_alloc::kept_with(|| std::sync::Arc::new(crate::Session { options: driver_options.clone(), runtime: sess.runtime.clone() }));
    let sess = &frozen;
    let options = &sess.options;
    let runtime = &sess.runtime;
    // edits made in the virtual folders (MAKEPAD_RUN_EDITS) against the release this run is: carried
    // over, or kept aside and reported (never dropped); not again inside a build script
    if std::env::var_os("MAKEPAD_RUN_SCRIPT_DEPTH").is_none() {
        for c in crate::vfs::reconcile_edits() {
            eprintln!("makepad: edit conflict: {c}");
            report("note", &c);
        }
        // the app's own folders written out for tools outside the run (vfs.rs SEED)
        if let Some(seed) = std::env::var("MAKEPAD_RUN_EDITS_SEED").ok().filter(|s| !s.is_empty()) {
            let folders: Vec<String> = seed.split(',').map(str::to_string).collect();
            if let Err(e) = crate::vfs::seed_edits(&folders) {
                eprintln!("makepad: writing the app's source into its edits folder: {e}");
            }
        }
    }
    let package = args[i].clone();
    // makepad inspect: the program answers questions from now on (compiles included)
    if !check && session.is_none() && !hosted && std::env::var_os("MAKEPAD_RUN_SCRIPT_DEPTH").is_none() {
        crate::inspect::start(&package);
    }
    let mut cargo_flags = Vec::new();
    let mut prog_args: Vec<Vec<u8>> = vec![package.clone().into_bytes()];
    let mut k = i + 1;
    while k < args.len() && args[k] != "--" {
        cargo_flags.push(args[k].clone());
        k += 1;
    }
    if harness && cargo_flags.iter().any(|f| f == "--bin" || f.starts_with("--bin=")) && !cargo_flags.iter().any(|f| f == "--bins") {
        cargo_flags.push("--bins".to_string());
    }
    if k < args.len() {
        // the program's arguments as given (bytes; `args` is Relax's lossy view)
        let raw: Vec<Vec<u8>> = crate::os::proc::args_bytes();
        if let Some(d) = raw.iter().position(|a| a == b"--") {
            for a in &raw[d + 1..] {
                prog_args.push(a.clone());
            }
        }
    }
    // the program's argv[0] / current_exe() is the Relax binary; spawning it again
    // (`Command::new(env::current_exe()?)`, `Command::new(&args[0])`) runs this program again
    // with the new arguments (std process.rs: MAKEPAD_RUN_SELF_EXEC holds this command line up to `--`);
    // a spawn of it with `run` as the first argument launches another program instead
    if let Ok(exe) = std::env::current_exe().and_then(|e| e.canonicalize()) {
        let mut pre: Vec<String> = crate::os::proc::args_bytes().iter().map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        if let Some(d) = pre.iter().position(|a| a == "--") {
            pre.truncate(d);
        }
        pre[0] = exe.display().to_string();
        // the re-run is silent: its stderr is the program's (rustc's binary prints nothing else)
        if pre.len() > 1 && !pre.iter().any(|a| a == "--silent") {
            pre.insert(2, "--silent".to_string());
            // an exec replacing this process (the program exec'ing its own executable) is this
            // session going on, not a re-run: it drops the added flag and stays live
            unsafe { std::env::set_var("MAKEPAD_RUN_SELF_EXEC_SILENT_ADDED", "1") };
        }
        for j in 1..pre.len() {
            if pre[j - 1] == "--units" && !std::path::Path::new(&pre[j]).is_absolute() {
                pre[j] = format!("{}/{}", workspace_dir(), pre[j]);
            }
        }
        pre.push("--".to_string());
        unsafe { std::env::set_var("MAKEPAD_RUN_SELF_EXEC", pre.join("\u{1f}")) };
        prog_args[0] = exe.display().to_string().into_bytes();
    }
    let workspace = workspace_dir();
    // everything this run writes: the workspace's <target>/relax (dirs.rs)
    let target_dir = crate::dirs::relax_dir().display().to_string();
    // the relax dir is made private before anything writes into it: the build scripts' out dirs
    // (and the host's window state) created it with create_dir_all, so on Windows it inherited the
    // checkout's ACL and the cache's own check then turned the cache off ("writable by
    // NT SERVICE\MakepadTunnel" on a tunnel box). The verdict stays the cache's (cache.rs).
    let _ = crate::pack::ensure_private_dir(std::path::Path::new(&target_dir));
    crate::source_context::set_output_dir(&target_dir);
    // std is the std the program runs on, unless MAKEPAD_RUN_STD says otherwise
    if crate::vfs::is_virtual(&workspace) {
        crate::std_source::select(&workspace, std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/std")));
    }
    if std::env::var_os("MAKEPAD_RUN_STD").is_none() {
        let mut d = std::env::current_exe().unwrap_or_default();
        for _ in 0..3 {
            d.pop();
        }
        let mut candidates = vec![format!("{}/libs/relax/compiler/std", workspace), format!("{}/src/libs/relax/compiler/std", d.display())];
        candidates.retain(|c| std::path::Path::new(c).join("std/lib.rs").exists());
        if let Some(c) = candidates.first() {
            std::env::set_var("MAKEPAD_RUN_STD", c);
        }
    }
    let std_dir = crate::std_dir();
    if !SILENT.load(Ordering::Relaxed) {
        verbose!("makepad-run: package {} ({}), std {}", package, if cargo_flags.is_empty() { "default features".to_string() } else { cargo_flags.join(" ") }, std_dir);
    }

    let Resolved { units, g, order, cfgs, live_inputs } = match resolve_package(sess, &workspace, &package, &cargo_flags, &units_json, &target_dir, &std_dir, ResolveFor::Boot, check) {
        Ok(r) => r,
        Err(e) => {
            if crate::run_state::observed() { crate::run_state::diagnostic(None, crate::diag::from_legacy(&e, "resolve")); }
            eprintln!("makepad-run: {e}");
            // a launcher following the start (--report-to) learns why it ended
            report_failed(&e, 1);
            return 1;
        }
    };
    let mem_g = crate::memstat::tag(crate::memstat::Tag::Load);
    // fn bodies a later parse can do alone are parsed on their first need (a warm run needs
    // few: their code comes from the disk cache); --check reads every body. MAKEPAD_RUN_LAZY_BODIES=0: off
    if !check && options.lazy_bodies {
        crate::parser::LAZY_AUTO.store(true, Ordering::Relaxed);
    }
    // ---- load + parse + collect items, crate by crate in dependency order
    let t = Instant::now();
    let load_profile = crate::load_profile::span(sess.runtime.profile, "load-parse", "main");
    let mut prog = crate::program::Program::new(sess);
    if sess.runtime.parse_verify { let _ = crate::preload::VERIFY.set(crate::front::verify_file); }
    // every crate's cfg set and env!() environment, then all of them scanned and parsed ahead
    // (preload.rs) while the loop below collects them in order
    for (ci, c) in g.crates.iter().enumerate() {
        crate::crates::set_crate_env(&c.name, g.env[ci].clone());
    }
    prog.std_simd = crate::std_simd_wanted(g.crates.iter().map(|c| c.src_path.as_str()));
    let mut pre = crate::std_pre_crates(sess, prog.std_simd);
    let std_count = pre.len();
    let std_names: Vec<_> = pre.iter().map(|c| (c.name.clone(), c.name.clone())).collect();
    let mut pre_index = vec![usize::MAX; g.crates.len()];
    for &ci in &order {
        let c = &g.crates[ci];
        let deps: Vec<_> = c.deps.iter().filter_map(|(d, e)| g.crates.get(*d).map(|x| (e.clone(), x.name.clone()))).collect();
        let mut image_dep_names = std_names.clone();
        image_dep_names.extend(deps.iter().cloned());
        let mut image_deps: Vec<_> = (0..std_count).collect();
        image_deps.extend(c.deps.iter().map(|(d, _)| pre_index[*d]));
        pre_index[ci] = pre.len();
        pre.push(crate::preload::PreCrate { name: c.name.clone(), root: c.src_path.clone(), edition: c.edition, cfg: cfgs[ci].clone(), deps,  proc_macro: c.proc_macro, image_deps, image_dep_names });
    }
    let front_images = crate::cache::FrontImages::open(options);
    // the load order the stored image was recorded in, with the changed crates and their
    // dependents last (FrontImages::load_order): the restore keeps every crate before them
    let order = match front_images.as_ref().filter(|_| !check).and_then(|images| images.load_order(&pre, std_count)) {
        Some(perm) => match crate::preload::reorder(&pre, &perm) {
            Some(reordered) => {
                let order: Vec<usize> = perm[std_count..].iter().map(|&k| order[k - std_count]).collect();
                for (k, &ci) in order.iter().enumerate() { pre_index[ci] = std_count + k; }
                pre = reordered;
                order
            }
            None => order,
        },
        None => order,
    };
    let tp = Instant::now();
    let root_dir = crate::unitgraph::manifest_dir_of(&g.crates[g.root].src_path);
    // the one app-crate rule (borrowck::event): check, serve and run share it
    let mut app_roots = crate::reload::gandalf_app_roots(&units, &root_dir);
    // a check of a package with several bins checks each of them
    app_roots.extend(g.bins.iter().map(|&b| crate::vfs::normalize_path(&g.crates[b].src_path)));
    if crate::borrowck::lazy::gandalf_enabled(options) {
        verbose!("makepad-run: gandalf app scope ({} crates): {}", app_roots.len(), app_roots.join(" "));
    }
    crate::borrowck::event::set_app_scope(app_roots);
    // `makepad check` borrow-checks every crate it loads: all keep their user-written types
    if check { crate::borrowck::event::set_check_scope(); }
    jit::settle_rebuild::remember_boot(&pre);
    prog.record_front = front_images.is_some();
    prog.front_sink = front_images.as_ref().and_then(|images| images.sink());
    if front_images.is_none() && (crate::preload::trace() || crate::memstat::enabled()) {
        verbose!("makepad-run front: 0 crates from crate images (0 files, no parse), {} crates parsed", pre.len());
    }
    let verify_collected = std::env::var("MAKEPAD_RUN_COLLECTED_VERIFY").is_ok_and(|v| v == "1");
    let mut verification_graph = None;
    let mut collected_tcx = None;
    let mut collected_definitions = None;
    let restored = if check { None } else {
        front_images.as_ref().and_then(|images| warm_front(images, sess, &pre))
    };
    let restored = if verify_collected {
        verification_graph = restored.filter(|(p, _, _)| p.crates.len() == pre.len());
        if let Some((saved, _, _)) = &verification_graph { prog.seed_collected_symbols(saved); }
        else {
            eprintln!("makepad-run collected: verification requires a current complete image; run once without MAKEPAD_RUN_COLLECTED_VERIFY first");
            return 1;
        }
        None
    } else { restored };
    let restored_crates = restored.as_ref().map_or(0, |(p, _, _)| p.crates.len());
    // std came from the collected image, so this load makes no std parses for the proc-macro
    // world: it parses its own instead of waiting for them (share_std_begin at the graph's
    // registration; load_std_crates_with ends the sharing only when it runs). An app-edit
    // relaunch built the world for the suffix's derives and waited 10 s for every std file.
    if restored_crates != 0 { crate::program::share_std_end(); }
    let collected_restored = restored_crates == pre.len();
    // a load that parses crates expands their macros: the proc-macro world starts building on
    // its own thread now. A warm restore of the whole program expands nothing: no world (the
    // first live edit that expands builds it).
    if !collected_restored {
        if let Err(e) = crate::macro_reg::prewarm(sess) {
            if crate::run_state::observed() { crate::run_state::diagnostic(None, crate::diag::from_legacy(&e, "macro")); }
            eprintln!("makepad-run: {e}");
            return 1;
        }
    }
    // the whole program came from the collected store: it holds this program already
    COLLECTED_CURRENT.store(collected_restored, Ordering::Relaxed);
    COLLECTED_RESTORED.store(collected_restored, Ordering::Relaxed);
    let collected_suffix = restored.as_ref().filter(|_| !collected_restored)
        .map(|(p, _, _)| (p.defs.len(), p.imports.len()));
    let mut cold_front = crate::cache::ColdFront::default();
    let mut crate_of = vec![u32::MAX; g.crates.len()];
    let mut load_errors = 0;
    let mut watch_load_errors = Vec::new();
    let core;
    if let Some((restored_prog, restored_tcx, definitions)) = restored {
        prog = restored_prog;
        for &ci in &order { if pre_index[ci] < restored_crates { crate_of[ci] = pre_index[ci] as u32; } }
        collected_tcx = Some(restored_tcx);
        collected_definitions = definitions;
    }
    if collected_restored {
        core = prog.prelude_crate.expect("collected graph prelude");
    } else {
        // (a reordered load's base graph: the restore's)
        let recipe = crate::crate_image::GraphRecipe { base: prog.front_graph.as_ref().and_then(|r| r.base), ..crate::crate_image::GraphRecipe::new(&pre) };
        let replay = (restored_crates != 0).then(|| pre.clone());
        let mut remaining: Vec<_> = pre.into_iter().skip(restored_crates).collect();
        for c in &mut remaining {
            c.image_deps = c.image_deps.iter().filter_map(|d| d.checked_sub(restored_crates)).collect();
        }
        // A partial restore has already checked selection against the complete
        // graph. Continue that plan and its contexts, not a new suffix closure.
        let plan = prog.demand_plan.take();
        prog.preload_images_bounded(remaining, front_images.as_ref().map(|x| x as &dyn crate::crate_image::CrateImages), crate::cache::ColdFront::enabled(options) && mem_budget_mode(), plan);
        if let Some(replay) = replay {
            prog.front_graph = Some(recipe);
            // Replay uses the full graph, not the suffix's rebased image indices.
            prog.load_recipe = replay;
        }
        let (loaded_core, std_deps) = if restored_crates == 0 {
            match crate::load_std_crates_with(sess, &mut prog, |p, k| cold_front.loaded(p, k)) {
                Ok(x) => x,
                Err(e) => {
                    if crate::run_state::observed() { crate::run_state::diagnostic(None, crate::diag::from_legacy(&e, "parse")); }
                    eprintln!("makepad-run: std: {}", e); return 1;
                }
            }
        } else {
            (prog.prelude_crate.expect("collected graph prelude"),
                (0..std_count).map(|k| (prog.crates[k].name, k as u32)).collect())
        };
        core = loaded_core;
        crate::tui::LOAD_TOTAL.store(order.len() as u32, Ordering::Relaxed);
        for (done, &ci) in order.iter().enumerate() {
            crate::run_state::progress(None, crate::run_state::Phase::Parse, Some(done as u64), Some(order.len() as u64), &g.crates[ci].name);
            crate::tui::LOAD_DONE.fetch_add(1, Ordering::Relaxed);
            if pre_index[ci] < restored_crates { continue; }
            let c = &g.crates[ci];
            crate::tui::crate_begin(&c.name);
            let mut deps = std_deps.clone();
            for (d, n) in &c.deps {
                if crate_of[*d] != u32::MAX {
                    deps.push((prog.syms.intern(n), crate_of[*d]));
                }
            }
            match prog.load_crate_deferred(&c.name, &c.src_path, c.edition, cfgs[ci].clone(), deps) {
                Ok(kk) => {
                    prog.crates[kk as usize].proc_macro = c.proc_macro;
                    prog.crates[kk as usize].is_bin = c.bin;
                    crate_of[ci] = kk;
                    cold_front.loaded(&mut prog, kk);
                    if crate::preload::trace() {
                        // one line per crate loaded: what its files hold, and the process after it
                        let (mut nf, mut src, mut ext, mut toks) = (0usize, 0usize, 0usize, 0usize);
                        for (fi, f) in prog.files.iter().enumerate() {
                            if prog.file_crate.get(fi).copied() == Some(kk) { nf += 1; src += f.src_len as usize; ext += f.src.len() - f.src_len as usize; toks += f.toks.len(); }
                        }
                        eprintln!("boot-trace-loaded: {} files={} src={}KB expansion={}KB toks={}k rss={}MB live={}MB t={:.0}ms", c.name, nf, src >> 10, ext >> 10, toks >> 10,
                            crate::memstat::resident() >> 20, crate::memstat::live_total() >> 20, crate::clock::ms_since_start());
                    }
                    crate::boot_stats::FILES_PARSED.store(prog.files.len() as u32, Ordering::Relaxed);
                    crate::boot_stats::LINES_PARSED.store(prog.lines_loaded as u64, Ordering::Relaxed);

                }
                Err(e) => {
                    if crate::run_state::observed() { crate::run_state::diagnostic(None, crate::diag::from_legacy(&e, "parse")); }
                    if watch {
                        // A syntax error at startup must still leave an editable session. The
                        // loader has already allocated this crate's root module. Give an
                        // unparsed root an empty AST until its first valid editor version.
                        let krate = prog.crates.len() as u32 - 1;
                        crate_of[ci] = krate;
                        let module = prog.crates[krate as usize].root_mod;
                        if !prog.file_paths.iter().any(|p| crate::vfs::normalize_path(p) == crate::vfs::normalize_path(&c.src_path)) {
                            if let Ok(pf) = crate::parser::parse_owned(sess, Vec::new(), c.edition) {
                                if let Ok(file) = prog.add_generated_file(&c.src_path, pf, krate, module) {
                                    prog.set_mod_file(module, file);
                                    let d = prog.mods[module as usize].def;
                                    prog.set_def_file(d, file);
                                }
                            }
                        }
                        let d = crate::diag::from_legacy(&e, "parse");
                        watch_load_errors.push(crate::diag::record(e.clone(), d));
                    } else if crate::diag::enabled() { crate::diag::emit(&crate::diag::from_legacy(&e, "parse")); }
                    eprintln!("makepad-run: load error {}: {}", c.name, e);
                    load_errors += 1;
                }
            }
        }
        if load_errors == 0 { cold_front.finish(&mut prog); }
        cold_front.report(&prog);
        if load_errors == 0 {
            // All preload workers have joined. Each crate now owns its lowering
            // context exclusively; retain its source identities and validation ledger,
            // not the compiler's already-consumed lowering intermediates.
            // A context still shared (another owner kept a clone) keeps its state.
            let _release = crate::load_profile::span(sess.runtime.profile, "front-context-release", "main");
            for c in &mut prog.crates {
                if let Some(cx) = std::sync::Arc::get_mut(&mut c.context) { cx.finish_load(); }
            }
        }
    }
    let preload_ms = tp.elapsed().as_secs_f64() * 1e3;
    prog.prelude_crate = Some(core);
    let lines = prog.lines_loaded;
    let again = crate::program::PARSE_AGAIN.load(std::sync::atomic::Ordering::Relaxed);
    let again = if again > 0 { format!(", {} parsed again", again) } else { String::new() };
    crate::tui::crate_end();
    phase(&format!("load+parse ({} crates, {} files, {}k lines{})", g.crates.len(), prog.files.len(), lines / 1000, again), t);
    drop(load_profile);
    if sess.runtime.profile { crate::load_profile::flush(); }
    if crate::preload::trace() {
        let ext: usize = prog.files.iter().map(|f| f.src.len() - f.src_len as usize).sum();
        let toks: usize = prog.files.iter().map(|f| f.toks.len()).sum();
        eprintln!("boot-trace: expansion text {} KB, tokens {}k", ext >> 10, toks >> 10);
    }
    let tf = Instant::now();
    crate::pmhost::flush_cache();
    if crate::preload::trace() {
        eprintln!("boot-trace: proc-macro world cache flush {:.1} ms; {}", tf.elapsed().as_secs_f64() * 1e3, crate::pmhost::cache_line());
    }
    // the program's files are loaded: the macro world goes (hot reload or a run-time module
    // builds it again from its disk cache)
    let pm0 = crate::memstat::live_of(crate::memstat::Tag::PmHost);
    let freed = crate::pmhost::free_world();
    crate::native_macros::report(sess.runtime.profile);
    if crate::preload::trace() && crate::memstat::enabled() {
        eprintln!("boot-trace: pmhost heap {} KB before the free, {} KB after", pm0 >> 10, crate::memstat::live_of(crate::memstat::Tag::PmHost) >> 10);
    }
    // (freed or not: the loader's worker threads have finished, and glibc keeps what they freed in
    // their own arenas, where the compile threads cannot reuse it)
    let _ = freed;
    {
        // its heap goes back to the system (freed chunks otherwise stay resident, scattered
        // between the program's allocations: ~70 MB on counter), off the boot path
        let trace = crate::preload::trace();
        { let _k = crate::zone_alloc::kept();
        std::thread::spawn(move || {
            let (r0, t) = (if trace { crate::memstat::resident() } else { 0 }, Instant::now());
            crate::memstat::trim();
            if trace {
                eprintln!("boot-trace: macro world heap trimmed: rss {} -> {} MB in {:.1} ms", r0 >> 20, crate::memstat::resident() >> 20, t.elapsed().as_secs_f64() * 1e3);
            }
        });
        }
    }
    if crate::preload::trace() {
        eprintln!("boot-trace: preload (scan) {:.1} ms; loader waited {:.1} ms for parses; parse cpu {:.1} ms; publish (under the pool lock) {:.1} ms", preload_ms, crate::preload::WAIT_NS.load(Ordering::Relaxed) as f64 / 1e6, crate::preload::PARSE_NS.load(Ordering::Relaxed) as f64 / 1e6, crate::preload::PUBLISH_NS.load(Ordering::Relaxed) as f64 / 1e6);
        eprintln!("boot-trace: macro scopes {:.1} ms; relaxed settings {:.1} ms", crate::macros::SCOPE_NS.load(Ordering::Relaxed) as f64 / 1e6, crate::relaxed::FEATURES_NS.load(Ordering::Relaxed) as f64 / 1e6);
        let ph: Vec<f64> = crate::preload::PHASE_NS.iter().map(|x| x.load(Ordering::Relaxed) as f64 / 1e6).collect();
        eprintln!("boot-trace: parse cpu split: fn bodies {:.0} ms, derives {:.0} ms, macro_rules transcription {:.0} ms, format family {:.0} ms, other built-in / hosted fn-like macros {:.0} ms", ph[0], ph[1], ph[2], ph[3], ph[4]);
        let hs = crate::macro_reg::stats();
        if hs.calls + hs.failed > 0 {
            eprintln!("boot-trace: proc-macro host: world {:.1} ms (built on its own thread), {} expansions {:.1} ms wall summed (JIT compile {} fns {:.1} ms CPU, encode {:.1} ms, decode {:.1} ms), {} failed", hs.world_ns as f64 / 1e6, hs.calls, hs.call_ns as f64 / 1e6, hs.compiled, hs.compile_ns as f64 / 1e6, hs.encode_ns as f64 / 1e6, hs.decode_ns as f64 / 1e6, hs.failed);
            let ph: Vec<f64> = crate::pmhost::PH.iter().map(|x| x.load(Ordering::Relaxed) as f64 / 1e6).collect();
            eprintln!("boot-trace: proc-macro host phases (summed): lookup {:.1} ms, call {:.1} ms, stats {:.1} ms", ph[0], ph[1], ph[2]);
            let tl = crate::pmhost::timeline();
            eprintln!("boot-trace: proc-macro host timeline: world build started t={:.0} ms, ready t={:.0} ms; first expansion asked t={:.0} ms; waits for the world {} ({:.0} ms summed); first call returned t={:.0} ms", tl[0], tl[1], tl[2], tl[4] as u64, tl[5], tl[3]);
        }
        let mut v = crate::preload::BY_NAME.lock().unwrap().clone();
        v.sort_by_key(|e| std::cmp::Reverse(e.2));
        for e in v.iter().take(12) {
            eprintln!("boot-trace:   {}! {} calls {:.1} ms", e.0, e.1, e.2 as f64 / 1e6);
        }
    }
    // a failed derive leaves its generated items out of the program (rustc would reject it):
    // never silent; --check counts them as errors
    let derive_failures = crate::macro_reg::derive_failures();
    if !derive_failures.is_empty() {
        if crate::run_state::observed() {
            for m in &derive_failures { crate::run_state::diagnostic(None, crate::diag::from_legacy(m, "macro")); }
        }
        eprintln!("makepad-run: derive failures: {} (their generated items are missing)", derive_failures.len());
        for m in derive_failures.iter().take(std::env::var("MAKEPAD_RUN_ERRORS").ok().and_then(|v| v.parse().ok()).unwrap_or(20)) {
            eprintln!("  {}", m);
        }
    }
    if load_errors > 0 && !watch {
        return 1;
    }
    {
        // what the unit holds, by identity: a domain started later shares these crates (domain.rs)
        let idents = crate::domain::identities(&g, &cfgs, &order);
        for &ci in &order {
            if crate_of[ci] != u32::MAX {
                crate::domain::record_loaded(idents[ci], crate_of[ci], &g.crates[ci], &cfgs[ci]);
            }
        }
    }
    let root = crate_of[g.root];
    if root == u32::MAX {
        eprintln!("makepad-run: the package root did not load");
        return 1;
    }

    drop(mem_g);
    let mem_g = crate::memstat::tag(crate::memstat::Tag::Resolve);
    // ---- resolve
    let t = Instant::now();
    if let Some((first, imports)) = collected_suffix { prog.resolve_imports_added(imports, first); }
    else if !collected_restored { prog.resolve_imports(); }
    if runtime.lazy_verify {
        if let Err(error) = prog.verify_lazy(crate::front::verify_file) {
            eprintln!("{error}");
            return 1;
        }
    }
    phase("resolve", t);
    // C0 (relax-cache.md): crate fingerprints + codegen identity, computed and reported only
    if std::env::var_os("MAKEPAD_RUN_CACHE_FP").is_some() {
        crate::stable::report_fingerprints(&prog, crate::codegen_id::id());
        if std::env::var("MAKEPAD_RUN_CACHE_FP").map_or(false, |v| v == "features") {
            return 0;
        }
    }
    let max_err: usize = std::env::var("MAKEPAD_RUN_ERRORS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    if !prog.errors.is_empty() {
        for e in &prog.errors {
            if crate::run_state::observed() { crate::run_state::diagnostic(None, crate::diag::from_legacy(e, "resolve")); }
        }
        eprintln!("makepad-run: {} unresolved imports (first {}):", prog.errors.len(), max_err.min(prog.errors.len()));
        for e in prog.errors.iter().take(max_err) {
            eprintln!("  {}", e);
        }
    }

    drop(mem_g);
    let mem_g = crate::memstat::tag(crate::memstat::Tag::Sigs);
    // ---- signatures, layouts, impl tables
    let t = Instant::now();
    // A check reports validation errors of its crates' impl headers and field types, which
    // declaration collection lowers: the crates are marked before it runs (a run marks none).
    if check {
        let checked = if watch || root_only || session.is_some() { vec![root] } else if selftest || workspace_check { crate::check::workspace_crates(&prog, root) } else { crate::check::app_crates(&prog, root) };
        let mut own = vec![false; prog.crates.len()];
        for k in checked { prog.crates[k as usize].validate = true; own[k as usize] = true; }
        // rustc's import resolution replayed over the checked crates: the glob ambiguities it finds
        // answer the name lookups of the declarations collected next
        prog.own_crates = own.clone();
        relax_types::lint::enable();
        prog.run_import_sim(&own);
    }
    let mut tcx = collected_tcx.take().unwrap_or_else(crate::tcx::Tcx::new);
    // MAKEPAD_RUN_PERTURB: type ids shift (code must not depend on them; syms.rs reloc_sig)
    for i in 0..crate::syms::perturb(options) {
        let u8_ = tcx.tys.prim(crate::program::Prim::U8);
        let _ = tcx.tys.intern(crate::types::TyKind::Array(u8_, tcx.tys.const_usize(1000 + i)));
    }
    *ROOT_DIR.lock().unwrap() = crate::zone_alloc::kept_with(|| format!("{} {}", g.crates[g.root].name, root_dir));
    crate::program::collect_misses_begin();
    if let Some((first, _)) = collected_suffix { tcx.collect_collected_suffix(&prog, first); }
    else if !collected_restored { tcx.collect_declarations(&prog); }
    // A signature, field or impl header that names a type in a file the lazy selection skipped
    // (a closure miss): load those files and collect again, until nothing is missing.
    let mut deferred_errors: Vec<String> = Vec::new();
    loop {
        let mods = prog.misses_to_load(&crate::program::collect_misses_take());
        if mods.is_empty() { break; }
        if std::env::var_os("MAKEPAD_RUN_DEFERRED_TRACE").is_some() {
            eprintln!("makepad-run deferred: signatures name types in {} skipped file(s): loaded before main", mods.len());
        }
        let (f0, d0) = (prog.files.len() as u32, prog.defs.len());
        for m in mods {
            if let Err(e) = prog.load_deferred_module(m) {
                eprintln!("makepad: loading a deferred module failed: {}", e);
                deferred_errors.push(format!("loading a deferred module failed: {}", e));
            }
        }
        prog.link_deferred_modules(f0, d0);
        tcx = crate::tcx::Tcx::new();
        crate::program::collect_misses_begin();
        tcx.collect_declarations(&prog);
    }
    phase("signatures", t);
    if !tcx.errors.is_empty() {
        for e in &tcx.errors {
            if crate::run_state::observed() { crate::run_state::diagnostic(None, crate::diag::from_legacy(e, "typeck")); }
        }
        eprintln!("makepad-run: {} signature errors (first {}):", tcx.errors.len(), max_err.min(tcx.errors.len()));
        for e in tcx.errors.iter().take(max_err) {
            eprintln!("  {}", e);
        }
    }

    if check {
        // a check runs no program: a constant that cannot be evaluated is a reported error
        // (E0080), as for `makepad test` / `live`, not the end of the process
        jit::COMPILE_ERRORS_FATAL.store(false, Ordering::Relaxed);
        crate::jit::compile_watch(Some(("check: own errors", 0)));
        let mut pre = crate::macro_reg::derive_failures();
        pre.extend(watch_load_errors);
        // the watch session reports its root crate's; a one-shot check every workspace crate's
        let checked = if watch || root_only || session.is_some() { vec![root] } else if selftest || workspace_check { crate::check::workspace_crates(&prog, root) } else { crate::check::app_crates(&prog, root) };
        crate::jit::compile_watch(Some(("check: unit", 0)));
        let t = Instant::now();
        let mut u = Unit::new(prog, tcx, core);
        // the program's unit names itself for the diagnostics that have no unit at hand (an
        // auxiliary unit -- the proc-macro world -- never takes it over)
        crate::syms::set_unit(&mut *u as *mut Unit);
        phase("unit", t);
        // Coherence evaluates the constants of impl headers, which needs the unit's evaluator (the
        // provider Unit::new registers). A run checks coherence in the background; a check needs its errors now: the app
        // check its own crates' impls (cargo check), every other check all of them.
        let t = Instant::now();
        if selftest || watch || session.is_some() { u.tcx.check_coherence(&u.prog); }
        else { u.tcx.check_coherence_of(&u.prog, Some(&checked)); }
        phase("coherence", t);
        for k in checked {
            pre.extend(crate::check::own_errors(&u.prog, k, &u.prog.errors, "resolve"));
            pre.extend(crate::check::own_errors(&u.prog, k, &u.tcx.errors, "typeck"));
        }
        // every body is checked, so the files the selection skipped load first: all of them for
        // selftest or a watch session; the checked crates' for --root-only or the app check
        let t = Instant::now();
        if selftest || watch || session.is_some() {
            unsafe { jit::load_all_skipped(&mut *u) };
        } else {
            let crates = if root_only { vec![root] } else if workspace_check { crate::check::workspace_crates(&u.prog, root) } else { crate::check::app_crates(&u.prog, root) };
            unsafe { jit::load_skipped_of(&mut *u, &crates) };
        }
        phase("load checked files", t);
        crate::jit::compile_watch(Some(("check: crate", 0)));
        if let Some(b) = &boundary {
            if let Err(e) = crate::boundary::start(&u, b) {
                eprintln!("makepad-run: {}", e);
                return 2;
            }
            // the audit lowers every fn of every crate: a constant that cannot be evaluated is one
            // failed row, not the end of the run
            jit::COMPILE_ERRORS_FATAL.store(false, Ordering::Relaxed);
            // on the owner thread, as every build of a run (check.rs compile_all): the unit is
            // the owner's, and the type checks ahead ask it for constants (const_addr_shared) while
            // the audit lowers; lowering on this thread raced them on the unit's tables (a
            // SIGSEGV rehashing `consts` under the gate, 2 of ~40 runs)
            let up: *mut Unit = &mut *u;
            let (rep, violations) = {
                let _job = jit::compile_job();
                crate::owner::call(crate::owner::Class::Program, || crate::boundary::run(up))
            };
            print!("{}", rep);
            std::mem::forget(u);
            if violations > 0 {
                eprintln!("makepad check --boundary: {} provided-crate violation(s) (`violation` rows)", violations);
                return 1;
            }
            return 0;
        }
        if let Some(out) = session {
            *out = Some(crate::serve::Session::new(u, root, pre));
            return 0;
        }
        // the warm session (type + borrow check; --deep also compiles); a one-shot `relax
        // check` keeps compiling everything, as the gates that use it expect
        if watch {
            let mut session = crate::serve::Session::new(u, root, pre);
            return session.watch(deep);
        }
        let code = crate::check::check_crate(&mut u, root, pre, root_only, selftest, workspace_check);
        // the parsed files go to the cache's front images as a run's do (one store shared with
        // `makepad run`): the next check (or run) of the package maps them instead of parsing
        // every file again (Stage: 6 s of a warm check was parsing)
        if !selftest { record_check_front(&mut u, options); }
        if crate::memstat::enabled() {
            crate::memstat::print("check complete", &crate::memreport::unit_sizes(&u));
        }
        std::mem::forget(u);
        return code;
    }

    // ---- main
    let exports_mode = !export_names.is_empty();
    // the exported fns a host asked for (`--exports`): no `main` at all (an Android app has none)
    let mut export_defs: Vec<(String, DefId)> = Vec::new();
    if exports_mode {
        match exported_fns(&prog, &export_names) {
            Ok(v) => export_defs = v,
            Err(e) => {
                eprintln!("makepad-run: --exports: {e}");
                return 1;
            }
        }
    }
    let main_def = if exports_mode || harness {
        None
    } else {
        match find_main(&prog, root) {
            Some(d) => Some(d),
            None => {
                if crate::run_state::observed() {
                    crate::run_state::diagnostic(None, crate::diag::Diagnostic::new("resolve", "", 0, 0,
                        format!("no `fn main` at the root of {}", g.crates[g.root].name)));
                }
                eprintln!("makepad-run: no `fn main` at the root of {}", g.crates[g.root].name);
                return 1;
            }
        }
    };
    let mut entry_defs = Vec::new();
    for e in &entries {
        let mut found = None;
        for (di, d) in prog.defs.iter().enumerate() {
            if matches!(d.kind, DefKind::Fn | DefKind::AssocFn) && prog.def_path(DefId(di as u32)) == *e {
                found = Some(DefId(di as u32));
            }
        }
        match found {
            Some(d) => entry_defs.push(d),
            None => {
                eprintln!("makepad-run: --entry `{}`: no such fn", e);
                return 1;
            }
        }
    }

    // ---- the libraries the build scripts link (#[link] blocks load on the first miss)
    let t = Instant::now();
    let (n_link, missing) = link_libraries(&prog, &g.link_libs, g.native.as_ref(), 0);
    phase(&format!("link libraries ({})", n_link), t);
    // a library that is not there only matters when one of its symbols is used: that miss is
    // reported where it happens (the #[link] block's lazy load); here only with --verbose
    if !missing.is_empty() {
        verbose!("makepad-run: libraries not found: {:?}", missing);
    }

    let t = Instant::now();
    let ahead = jit::ahead_policy().last_run;
    let fn_defs = if ahead { startup_defs(&prog, &format!("{}/startup/{}.startup", target_dir, package)) } else { Vec::new() };
    // `main` lives at the root of the root crate (find_main)
    let root_crate = root;
    crate::layout::set_root_crate(root_crate);
    // A live session (hot reload of the program's own sources, reload.rs); set
    // before anything is laid out (app structs take the live layout, layout.rs live_editable)
    if options.live {
        crate::layout::LIVE_SESSION.store(true, Ordering::Relaxed);
    }
    // The provided crates bind in Unit::new: their source check has passed before it.
    {
        let t = Instant::now();
        if let Err(e) = crate::provided::finish_sources() {
            if crate::run_state::observed() { crate::run_state::diagnostic(None, crate::diag::from_legacy(&e, "resolve")); }
            eprintln!("makepad-run: {e}");
            return 1;
        }
        phase("provided sources", t);
    }
    let mut u = Unit::new(prog, tcx, core);
    // the program's unit names itself for the diagnostics that have no unit at hand (an
    // auxiliary unit -- the proc-macro world -- never takes it over)
    crate::syms::set_unit(&mut *u as *mut Unit);
    if let Some((saved_prog, saved_tcx, _)) = verification_graph {
        saved_prog.verify_collected(&u.prog);
        saved_tcx.verify_collected(&saved_prog, &u.prog, &u.tcx);
        eprintln!("makepad-run collected: verified restored graph against rebuilt graph");
    }
    let enum_errors = u.tcx.check_enum_discriminants(&u.prog, root_crate);
    if !enum_errors.is_empty() {
        for error in enum_errors {
            if crate::run_state::observed() { crate::run_state::diagnostic(None, crate::diag::from_legacy(&error, "typeck")); }
            eprintln!("error: {}", error);
        }
        return 1;
    }
    jit::set_program_args(prog_args.clone());
    drop(mem_g);
    let mem_g = crate::memstat::tag(crate::memstat::Tag::CacheLoad);
    // Classify the disk cache before workers add newly compiled entries.
    // a warm start: every crate's pack accepted, every front image restored, and the startup
    // slot manifest an earlier run recorded still names entries of this cache
    let mut warm_start = None;
    unsafe { std::env::set_var("MAKEPAD_RUN_CACHE_STATE", "off") };
    // a fully restored collected program keeps the boot file count it was recorded with
    if !collected_restored { u.prog.boot_files = u.prog.files.len(); }
    if let Some(mode) = crate::cache::mode_from_env(options) {
        match crate::cache::Cache::open_collected(&u, mode, collected_definitions.as_ref()) {
            Ok(mut c) => {
                c.load_manifest(root_crate);
                let warm = c.startup_manifest_valid() && c.stats.packs_rejected == 0 && c.stats.packs as usize == u.prog.crates.len() && (!u.prog.record_front || u.prog.front_restored.iter().all(|r| *r));
                relax_codegen::warm::classify(warm);
                crate::run_state::emit(crate::run_state::Event::Warm { domain: None, warm });
                // the startup links cached code (warm, or an edited start of a program that ran
                // before: its manifest's entries are all in the cache): no speculative body checks
                // ahead of the lazy compiles -- an edited start's slots link stored code but for the
                // few the edit reaches, and a build checks its own body (an edited Sandbox start
                // checked 80k bodies ahead, 13 s cpu, for 31 its 8 builds took)
                u.front.linked_start.store(c.startup_manifest_valid() && c.stats.packs_rejected == 0, Ordering::Relaxed);
                jit::CODE_CACHE_EMPTY.store(c.stats.code_stores == 0, Ordering::Relaxed);
                if warm {
                    warm_start = Some((c.manifest.len(), c.manifest_present, c.manifest_uncached));
                } else if c.has_startup_manifest() && c.manifest_missing > 0 {
                    verbose!("makepad-run cache: startup manifest stale: {} of its {} slots' entries are gone", c.manifest_missing, c.manifest.len());
                }
                verbose!("makepad-run cache: {:?}, {} packs ({} rejected), {} code stores, {} entries, opened in {:.1} ms (interfaces {:.1} ms)", mode, c.stats.packs, c.stats.packs_rejected, c.stats.code_stores, c.stats.index_entries, c.stats.open_ns as f64 / 1e6, c.stats.iface_ns as f64 / 1e6);
                // warm: this program ran with this cache before (its startup manifest, recorded
                // after an earlier run reached main); entries other processes of this launch wrote
                // (build scripts, the proc-macro host) do not make a first run warm
                unsafe { std::env::set_var("MAKEPAD_RUN_CACHE_STATE", if c.has_startup_manifest() { "warm" } else { "cold" }) };
                u.cache = Some(Box::new(crate::cache::CacheCell::new(c)));
            }
            Err(e) => eprintln!("makepad-run cache: off ({})", e),
        }
    }
    phase("image", t);
    drop(mem_g);
    // after the cache is open: a validated warm start with the program's clean verdict stored skips the check
    jit::start_background_coherence(&u);
    *STARTUP_PATH.lock().unwrap() = crate::zone_alloc::kept_with(|| format!("{}/startup/{}.startup", target_dir, package));
    if let Ok(p) = crate::zone_alloc::kept_with(|| std::env::var("MAKEPAD_RUN_REPORT")) {
        *REPORT_PATH.lock().unwrap() = p;
    }
    if crate::why::enabled() {
        for (i, d) in u.prog.defs.iter().enumerate() {
            if u.prog.files.get(d.file as usize).and_then(|f| f.ast.items.get(d.item.0 as usize)).is_some() {
                let c = u.prog.why_def(crate::program::DefId(i as u32), "declaration");
                crate::why::event("item-decl", &c.file, c.line, &c.symbol, None);
            }
            if matches!(d.kind, crate::program::DefKind::Fn | crate::program::DefKind::AssocFn) {
                let c = u.prog.why_def(crate::program::DefId(i as u32), "declaration");
                crate::why::event("fn-decl", &c.file, c.line, &c.symbol, None);
            }
        }
    }
    let entry = match main_def {
        Some(main_def) => {
            let id = {
                let _why = crate::why::enabled().then(|| {
                    let mut c = u.prog.why_def(main_def, "entry-point");
                    c.symbol = "run main stub before execution".into();
                    crate::why::enter(c)
                });
                u.fn_id(FnKey::Inst(main_def, Vec::new()))
            };
            u.entry(id)
        }
        None => 0,
    };
    let up: *mut Unit = &mut *u;
    unsafe { RUN_UNIT = up };
    if let Err(e) = finish_window() {
        eprintln!("makepad-run: {e}");
        return 1;
    }
    if !HAL_RUN.load(Ordering::Acquire) { crate::domain::register_boot(&package, u.syms.dom0()); }
    start_exit_summary();
    *RUN_DIAGNOSTICS.lock().unwrap() = Some(u.tcx.dcx.clone());
    unsafe { atexit(at_exit) };
    crate::jit::set_exit_hook(at_exit);
    start_monitor(runtime, options);
    // MAKEPAD_RUN_MEM_AT_MS=n[,m..]: the memory report (MAKEPAD_RUN_MEM_REPORT=1) n ms after main starts,
    // without ending the run's summary (programs that never reach their exit summary: Stage)
    if let Ok(v) = std::env::var("MAKEPAD_RUN_MEM_AT_MS") {
        let mem_at_trim = runtime.mem_at_trim;
        let at: Vec<u64> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        { let _k = crate::zone_alloc::kept();
        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            for ms in at {
                let d = std::time::Duration::from_millis(ms).saturating_sub(t0.elapsed());
                std::thread::sleep(d);
                let up = unsafe { RUN_UNIT };
                if up.is_null() || !crate::memstat::enabled() {
                    continue;
                }
                crate::owner::call(crate::owner::Class::Program, || {
                    crate::memstat::print(&format!("at {} ms", ms), &crate::memreport::unit_sizes(unsafe { &*up }));
                });
                // MAKEPAD_RUN_MEM_AT_TRIM=1: the allocator's free pages back to the system, then the report
                // again (how much of the footprint is freed-but-held memory a trim can return)
                if mem_at_trim {
                    let t = std::time::Instant::now();
                    crate::memstat::trim();
                    let ns = t.elapsed().as_secs_f64() * 1e3;
                    crate::owner::call(crate::owner::Class::Program, || {
                        crate::memstat::print(&format!("at {} ms after a {:.1} ms trim", ms, ns), &crate::memreport::unit_sizes(unsafe { &*up }));
                    });
                }
            }
        });
        }
    }
    // MAKEPAD_RUN_STATS_AFTER_MS=n: print the compile totals and timeline n ms after boot (programs
    // that never exit: the counter's event loop)
    if let Some(ms) = std::env::var("MAKEPAD_RUN_STATS_AFTER_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
        { let _k = crate::zone_alloc::kept();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            crate::owner::call(crate::owner::Class::Program, || summary("stats"));
        });
        }
    }
    jit::enable_pipelining(options);
    jit::MAIN_TID.store(jit::current_thread_id(), Ordering::Relaxed);
    // the startup manifest: the slots installed before the first present and in the first 5 s
    // (the next run's workers link the later ones ahead)
    jit::ORDER_UNTIL_NS.store(jit::wall_ns() + 5_000_000_000, Ordering::Relaxed);
    // the manifest's slots before the first present: installed now, from its prelinked image
    let t_pre = Instant::now();
    let prelinked = unsafe { jit::prelink_startup(up) };
    if prelinked == 0 && warm_start.is_some() {
        if let Some(c) = unsafe { (*up).cache.as_ref() } {
            verbose!("makepad-run cache: warm start without a prelinked startup image (image {}, {} stale, {} unresolved, {} had code)",
                if c.has_startup_image() { "present" } else { "missing" }, c.stats.prelink_changed, c.stats.prelink_failed, c.stats.prelink_had_code);
        }
    }
    if prelinked > 0 {
        if let Some(c) = unsafe { (*up).cache.as_ref() } {
            let s = &c.stats;
            let ms = |ns: u64| ns as f64 / 1e6;
            verbose!("makepad-run cache: prelinked {} startup slots and {} data objects in {:.1} ms (image read {:.1}, keys {:.1}, symbols {:.1}, placed+linked+installed {:.1}; {} naming a changed crate kept, checked in {:.1}; {} left lazy: {} stale, {} unresolved, {} had code)", s.prelinked, s.prelink_data, t_pre.elapsed().as_secs_f64() * 1e3,
                ms(s.prelink_parse_ns), ms(s.prelink_keys_ns), ms(s.prelink_syms_ns), ms(s.prelink_ns.saturating_sub(s.prelink_parse_ns + s.prelink_keys_ns + s.prelink_syms_ns + s.prelink_check_ns)),
                s.prelink_rekeyed, ms(s.prelink_check_ns), s.prelink_changed + s.prelink_failed + s.prelink_had_code, s.prelink_changed, s.prelink_failed, s.prelink_had_code);
        }
    }
    unsafe { jit::spec_seed_manifest(up) };
    if let Some((n, present, uncached)) = warm_start {
        verbose!("makepad-run: warm start, startup manifest of {} slots ({} before the first present, {} without cached code)", n, present, uncached);
    }
    crate::mainprof::start();
    MAIN_START_WALL.store(jit::wall_ns(), Ordering::Relaxed);
    if ahead {
        start_ahead(up, fn_defs, runtime);
    }
    let mut domain_inputs = None;
    // a live session: hot reload of the program's own sources (reload.rs); none for store sources
    if crate::layout::LIVE_SESSION.load(Ordering::Relaxed) {
        // the app domain this boot starts takes the same inputs (handoff below)
        domain_inputs = if HAL_RUN.load(Ordering::Acquire) { live_inputs.as_ref().map(crate::zone_alloc::kept_clone) } else { None };
        crate::reload::start_watcher(up, live_inputs.unwrap_or_default());
    }
    jit::start_completion(up, runtime);
    jit::live_needs_present(u.prog.crates.iter().any(|c| u.prog.syms.str(c.name) == "makepad_platform"));
    jit::startup_main(runtime);
    relax_codegen::fncost::mark("boot done, calling main");
    let main_ms = crate::clock::ms_since_start();
    MAIN_START_MS.store(main_ms as u64, Ordering::Relaxed);
    crate::boot_stats::main_called();
    crate::tui::compiling();
    unsafe {
        std::env::set_var("MAKEPAD_RUN_STARTUP_T0", crate::clock::process_start_epoch().to_string());
        std::env::set_var("MAKEPAD_RUN_MAIN_MS", crate::clock::ms_since_process_start().to_string());
        if SILENT.load(Ordering::Relaxed) {
            std::env::remove_var("MAKEPAD_RUN_PACKAGE");
        } else {
            std::env::set_var("MAKEPAD_RUN_PACKAGE", &package);
        }
    }
    if !SILENT.load(Ordering::Relaxed) {
        verbose!("makepad-run: boot done, calling main (t={:.0} ms)", main_ms);
    }
    report("main", "");
    if crate::memstat::enabled() {
        crate::memstat::print("boot", &crate::memreport::unit_sizes(&u));
        let l: Vec<u64> = crate::parser::LAZY_STATS.iter().map(|x| x.load(Ordering::Relaxed)).collect();
        for (k, why) in crate::parser::LAZY_WHY.iter().enumerate() {
            if l[k * 2] > 0 {
                eprintln!("makepad-run mem: fn bodies parsed at load, {}: {} bodies, {} MB parse working set", why, l[k * 2], l[k * 2 + 1] >> 20);
            }
        }
        let mut ms: Vec<(String, u64)> = crate::parser::LAZY_MACROS.lock().unwrap().iter().map(|(k, v)| (k.clone(), *v)).collect();
        ms.sort_by_key(|x| std::cmp::Reverse(x.1));
        if !ms.is_empty() {
            eprintln!("makepad-run mem: macros keeping bodies eager: {}", ms.iter().take(15).map(|(k, v)| format!("{} {}", k, v)).collect::<Vec<_>>().join(", "));
        }
    }
    if runtime.profile {
        for extension in ["rs"] {
            let suffix = format!(".{extension}");
            let mut eager = 0usize;
            let mut deferred = 0usize;
            let mut parsed = 0usize;
            let mut body_files = 0usize;
            let mut files = std::collections::BTreeSet::new();
            for (path, file) in u.prog.file_paths.iter().zip(&u.prog.files) {
                if !path.ends_with(&suffix) { continue; }
                files.insert(path);
                for item in &file.ast.items {
                    if let crate::ast::ItemKind::Fn(sig, body, _) = item.kind {
                        eager += usize::from(body.is_some());
                        deferred += usize::from(file.ast.fn_sig(sig).lazy_body.is_some());
                    }
                }
                let bodies: usize = file.parts().map(|part| part.bodies.len()).sum();
                parsed += bodies;
                body_files += usize::from(bodies != 0);
            }
            eprintln!("source-file-demand: language={extension} unique_parsed={}", files.len());
            eprintln!("source-body-demand: language={extension} eager={eager} deferred={deferred} parsed_deferred={parsed} deferred_body_files={body_files}");
        }
    }
    // MAKEPAD_RUN_BOOT_ONLY=1: stop here (boot timing / profiling)
    if runtime.boot_only && std::env::var_os("MAKEPAD_RUN_BUILD_SCRIPT").is_none() {
        crate::why::event("boundary", "boot-done-before-main", 0, "", Some(crate::why::Cause::new("", 0, "boot-only", "application main has not run")));
        std::process::exit(0);
    }
    for (k, d) in entry_defs.iter().enumerate() {
        // speculative workers may be compiling (they add slots): the owner thread around fn_id
        let en = crate::owner::call(crate::owner::Class::Program, || {
            let id = u.fn_id(FnKey::Inst(*d, Vec::new()));
            u.entry(id)
        });
        let r = unsafe { jit::call_jit(up, en, 0) };
        flush_errors(true);
        match r {
            Ok(v) => verbose!("makepad-run: entry {} returned {:#x} (t={:.0} ms)", entries[k], v, crate::clock::ms_since_start()),
            Err(p) => {
                let rep = crate::owner::call(crate::owner::Class::Program, || u.report(&p));
                eprintln!("makepad-run: entry {} {}: {}\n  report: {}", entries[k], p.kind, p.message, rep);
            }
        }
    }
    if !entry_defs.is_empty() {
        std::mem::forget(u);
        return 0;
    }
    if exports_mode {
        // the exported fns' permanent addresses (lazy stubs: each compiles on its first call, on
        // whatever thread calls it); the unit outlives this call as it does for `main`
        let mut addrs = crate::zone_alloc::kept_with(|| Vec::with_capacity(export_defs.len()));
        for (name, d) in &export_defs {
            let en = crate::owner::call(crate::owner::Class::Program, || {
                let id = u.fn_id(FnKey::Inst(*d, Vec::new()));
                crate::wasm_export::export(id, name);
                u.entry_direct(id)
            });
            crate::zone_alloc::kept_with(|| addrs.push((name.clone(), en)));
        }
        *EXPORTS.lock().unwrap() = addrs;
        // the host calls these from its own threads, not through call_jit
        unsafe { jit::set_runtime_unit(up) };
        std::mem::forget(u);
        return 0;
    }
    if harness {
        // the root crate and, in an integration test graph, every test target
        let pkg_dir = &g.pkg[g.root].0;
        let label = |ci: usize| {
            let p = &g.crates[ci].src_path;
            let rel = p.strip_prefix(pkg_dir.as_str()).map_or(p.as_str(), |r| r.trim_start_matches(['/', '\\']));
            let is_test_target = units.units.iter().any(|u| &u.src_path == p && u.kind.iter().any(|k| k == "test"));
            if is_test_target { rel.to_string() } else { format!("unittests {rel}") }
        };
        let mut groups = vec![(label(g.root), root)];
        for &b in &g.bins {
            if b != g.root && crate_of[b] != u32::MAX {
                groups.push((label(b), crate_of[b]));
            }
        }
        groups.sort();
        let code = crate::harness::run(&mut u, up, &groups, no_fail_fast);
        flush_errors(true);
        std::mem::forget(u);
        return code;
    }
    let main_def = main_def.expect("a run without --exports has main");
    if HAL_RUN.load(Ordering::Acquire) {
        unsafe { jit::set_runtime_unit(up) };
        // The app domain is this very graph, resolved, verified and built a moment ago in this
        // process: it takes it instead of resolving the package again (domain.rs start_prepared).
        *HANDOFF.lock().unwrap() = Some(Resolved { units, g, order, cfgs, live_inputs: domain_inputs });
        let spec = crate::domain::AppSpec { package, cargo_flags, units_json, argv: Some(prog_args), ..Default::default() };
        let result = (MAIN_HOST.get().expect("run host").start)(spec);
        if result.is_ok() && !domain_packages.is_empty() { crate::domain::run_vehicle(up, domain_packages); }
        std::mem::forget(u);
        return match result {
            Ok(()) => 0,
            Err(e) => { eprintln!("makepad-run: {e}"); 1 }
        };
    }
    if !domain_packages.is_empty() { crate::domain::run_vehicle(up, domain_packages); }
    let r = match main_call(&mut u, main_def, entry) {
        Ok((target, arg)) => unsafe { jit::call_jit(up, target, arg) },
        Err(e) => {
            eprintln!("makepad-run: {e}");
            // Workers already hold this unit's pointer. Its Drop waits
            // for the front end and O3; process shutdown owns it now.
            std::mem::forget(u);
            return 1;
        }
    };
    let code = match r {
        Ok(_) => 0,
        Err(p) => {
            // The message now, on this thread (it needs no compiler state): a panic is
            // never silent, whatever the exit's cleanup gets to. Its report (backtrace
            // on the owner thread) is the cleanup worker's, after the compilers stopped.
            eprintln!("makepad-run: main {}: {}", p.kind, p.message);
            let _ = RUN_PANIC.set(p);
            101
        }
    };
    // threads the program spawned may still run (and compile) until the process exits, as
    // under rustc where returning from main exits with detached threads still running: the
    // unit must outlive cmd_run (dropping it here freed it under them)
    std::mem::forget(u);
    code
}

/// The booted program's crates: (crate name, package dir), in graph order.
static CRATE_DIRS: relax_session::sync::Mutex<Vec<(String, String)>> = relax_session::sync::Mutex::new(Vec::new());

/// The crates of the program `cmd_run` booted, by name, with their package directories (a host
/// serving the program's resources: the web trace host maps `/<crate>/<path>` to them).
pub fn crate_dirs() -> Vec<(String, String)> {
    CRATE_DIRS.lock().unwrap().clone()
}

/// What `--exports` published: (symbol, address) of each exported fn, in program order.
static EXPORTS: relax_session::sync::Mutex<Vec<(String, u64)>> = relax_session::sync::Mutex::new(Vec::new());

/// The symbols (and their callable addresses) a `cmd_run` with `--exports` published.
pub fn exports() -> Vec<(String, u64)> {
    EXPORTS.lock().unwrap().clone()
}

/// The last boot phase that finished ("unit graph", "build scripts", ...), for a host's progress
/// display, and how many crates the front end has loaded of how many (tui.rs counters).
pub fn progress() -> (String, u32, u32) {
    let phase = PHASES.lock().unwrap().last().map(|p| p.0.split(" (").next().unwrap_or(&p.0).to_string()).unwrap_or_default();
    (phase, crate::tui::LOAD_DONE.load(Ordering::Relaxed), crate::tui::LOAD_TOTAL.load(Ordering::Relaxed))
}

/// The C-ABI fns carrying `#[no_mangle]` (symbol = the fn's name) or `#[export_name = ".."]` whose
/// symbol starts with one of `prefixes`. A symbol defined twice is an error: the host would bind
/// one of the two silently.
pub(crate) fn exported_fns(prog: &crate::program::Program, prefixes: &[String]) -> Result<Vec<(String, DefId)>, String> {
    let mut out: Vec<(String, DefId)> = Vec::new();
    for (di, d) in prog.defs.iter().enumerate() {
        if d.kind != DefKind::Fn {
            continue;
        }
        let f = &prog.files[d.file as usize];
        let mut symbol = None;
        for a in f.ast.list(f.ast.item(d.item).attrs) {
            let toks: Vec<&str> = (a.toks.lo..a.toks.hi).map(|t| f.tok_text(t)).collect();
            // `#[no_mangle]`, `#[unsafe(no_mangle)]`, `#[export_name = ".."]`, `#[unsafe(export_name = "..")]`
            if toks.contains(&"no_mangle") {
                symbol = Some(prog.name(DefId(di as u32)).to_string());
            } else if let Some(k) = toks.iter().position(|t| *t == "export_name") {
                if let (Some(&"="), Some(lit)) = (toks.get(k + 1), toks.get(k + 2)) {
                    symbol = Some(lit.trim_matches('"').to_string());
                }
            }
        }
        let Some(symbol) = symbol else { continue };
        if !prefixes.iter().any(|p| symbol.starts_with(p.as_str())) {
            continue;
        }
        if let Some((_, other)) = out.iter().find(|(s, _)| *s == symbol) {
            return Err(format!("`{symbol}` is defined twice ({} and {})", prog.def_path(*other), prog.def_path(DefId(di as u32))));
        }
        out.push((symbol, DefId(di as u32)));
    }
    if out.is_empty() {
        return Err(format!("no exported fn starts with {}", prefixes.join(" or ")));
    }
    Ok(out)
}

/// Loads the libraries the build scripts link and the ones `#[link]` blocks of files from
/// `first_file` on name: (how many, the ones not found).
pub(crate) fn link_libraries(prog: &crate::program::Program, link_libs: &[(String, bool)], native: Option<&crate::native::NativeLib>, first_file: usize) -> (usize, Vec<String>) {
    let mut missing = Vec::new();
    // no-codegen mode: nothing is linked at run time -- native code is the host binary's, and
    // the program reaches only the symbols the host exports (jit.rs foreign_addr)
    if prog.session.options.no_codegen {
        return (0, missing);
    }
    // `--allow-c`: the native dependencies' library (front native.rs), its symbols global
    if let Some(n) = native {
        if !crate::os::proc::load_path(&n.path) {
            missing.push(format!("{} (--allow-c native library of {})", n.path, n.crates.join(", ")));
        }
    }
    let provided_static = |name: &str| native.is_some_and(|n| n.statics.iter().any(|s| s == name));
    for (name, fw) in link_libs {
        if !crate::os::proc::load_library(name, *fw) && (*fw || !crate::os::proc::load_library_versioned(name)) {
            missing.push(name.clone());
        }
    }
    // what the system linker would have linked: every library an active extern block names
    // with #[link] (frameworks register their Objective-C classes and protocols on load, and
    // the program looks those up by name at run time)
    let mut n_link = link_libs.len();
    for (name, fw) in link_attr_libs(prog, first_file) {
        if link_libs.iter().any(|l| l.0 == name && l.1 == fw) || provided_static(&name) {
            continue;
        }
        n_link += 1;
        if !crate::os::proc::load_library(&name, fw) && (fw || !crate::os::proc::load_library_versioned(&name)) {
            missing.push(name);
        }
    }
    (n_link, missing)
}

/// The native run host: the OS loop starts before compilation, the graph opens a host window,
/// and the compiled program starts as an ordinary domain. No HAL dependency in the compiler.
pub struct MainHost {
    /// Android lends the Activity directly to the process app until the WM hosts child domains.
    pub domain_zero: bool,
    pub run: fn(Box<dyn FnMut() -> i32 + Send>) -> i32,
    pub open: fn(&str, &str) -> Result<(), String>,
    pub start: fn(crate::domain::AppSpec) -> Result<(), String>,
    pub relaunch: fn() -> Result<(), String>,
}

static MAIN_HOST: std::sync::OnceLock<MainHost> = std::sync::OnceLock::new();
static HOSTED_RUN: AtomicBool = AtomicBool::new(false);
/// `makepad run --harness` (`makepad test`): the program is a test harness.
static HARNESS_RUN: AtomicBool = AtomicBool::new(false);
static HAL_RUN: AtomicBool = AtomicBool::new(false);
static APP_DOMAIN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn set_app_domain(id: u32) { APP_DOMAIN.store(id, Ordering::Release); }
pub fn app_domain() -> Option<u32> {
    let id = APP_DOMAIN.load(Ordering::Acquire);
    (id != 0).then_some(id)
}

/// All whole-app restart requests use the native host's existing HAL domain relaunch path.
pub fn relaunch() -> Result<(), String> {
    if app_domain().is_none() { return Err("no hosted app domain is running".into()); }
    (MAIN_HOST.get().ok_or("no run host")?.relaunch)()
}

/// Registers the host for programs that link the HAL (once, before `cmd_run`).
pub fn set_main_host(host: MainHost) {
    let _ = MAIN_HOST.set(host);
}

/// (name, framework) of every `#[link(name = .., kind = ..)]` on an extern block whose items
/// are active (got definitions), or an empty link-only block without a cfg; files from
/// `first_file` on.
pub(crate) fn link_attr_libs(prog: &crate::program::Program, first_file: usize) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for (fi, f) in prog.files.iter().enumerate().skip(first_file) {
        let Some(cfg) = prog.file_crate.get(fi).and_then(|&k| prog.crates.get(k as usize)).map(|c| &c.cfg) else { continue };
        let src = f.src_view();
        // Every item of the file's arena, as before: it also holds the items of macro expansions
        // the collector dropped (libc's cfg_if! branches for other targets carry no #[cfg] of
        // their own), so an empty `#[link] extern "C" {}` of another target's branch still
        // reaches here (root, network, bsd, rt); the collector would have to record the extern
        // blocks it keeps to tell them apart. The `cfg(..)` of `#[link(.., cfg(..))]` is honoured.
        for it in &f.ast.items {
            let active = match &it.kind {
                // an extern block without items: its own #[cfg]s decide, as for any item
                crate::ast::ItemKind::ForeignMod(r) if f.ast.row(*r).items.is_empty() => crate::cfg::active(src, f.ast.list(it.attrs), cfg),
                crate::ast::ItemKind::ForeignMod(r) => f.ast.list(f.ast.row(*r).items).iter().any(|si| prog.item_defs.contains_key(&(fi as u32, si.0))),
                _ => false,
            };
            if !active {
                continue;
            }
            for a in crate::cfg::expand_cfg_attr_list(src, f.ast.list(it.attrs), cfg) {
                if a.toks.hi <= a.toks.lo || f.tok_text(a.toks.lo) != "link" {
                    continue;
                }
                let mut name = String::new();
                let mut fw = false;
                let mut holds = true;
                let mut k = a.toks.lo;
                while k + 2 < a.toks.hi {
                    if f.tok_text(k + 1) == "=" {
                        let v = f.tok_text(k + 2).trim_matches('"').to_string();
                        match f.tok_text(k) {
                            "name" => name = v,
                            "kind" => fw = v == "framework",
                            _ => {}
                        }
                    } else if f.tok_text(k) == "cfg" && f.tok_text(k + 1) == "(" {
                        // `#[link(name = "x", cfg(pred))]` (link_cfg): the library only when
                        // the predicate holds (libc's crt-static variants)
                        let mut depth = 0;
                        let mut end = k + 1;
                        while end < a.toks.hi {
                            match f.tok_text(end) { "(" => depth += 1, ")" => { depth -= 1; if depth == 0 { break; } } _ => {} }
                            end += 1;
                        }
                        holds &= crate::cfg::eval_tokens(src, k as usize + 2, end as usize, cfg);
                        k = end;
                        continue;
                    }
                    k += 1;
                }
                if std::env::var_os("MAKEPAD_RUN_LINK_TRACE").is_some() {
                    let item_attrs: Vec<String> = f.ast.list(it.attrs).iter().map(|x| (x.toks.lo..x.toks.hi).map(|t| f.tok_text(t)).collect::<Vec<_>>().join(" ")).collect();
                    eprintln!("makepad-run link: {} holds={} in {} (item attrs {:?})", name, holds, prog.file_paths[fi], item_attrs);
                }
                if holds && !name.is_empty() && !out.iter().any(|l| l.0 == name && l.1 == fw) {
                    out.push((name, fw));
                }
            }
        }
    }
    out
}

/// The shortest `lib<name>.so.<N>` in the usual library dirs.
/// `fn main` at the root of crate `root`.
/// `main` as rustc's resolver finds it (rustc_resolve lib.rs: `main` resolved in the crate root's
/// value namespace, imports and globs included -- `pub use app::*;` brings it in), then entry.rs:
/// it must be a fn.
/// What to call to run `main` (whose lazy stub is `entry`): (target, argument) for `call_jit`.
/// `fn main() -> T` (T != ()): rustc's lang_start runs `main().report()` (Termination) and exits
/// with its code; std::process::__relax_lang_start::<T>(main) does the same.
/// (a `#[no_main]` crate's `extern "C" fn main(argc, argv) -> c_int` is called as before)
pub(crate) fn main_call(u: &mut Unit, main_def: DefId, entry: u64) -> Result<(u64, u64), String> {
    let main_ret = u.tcx.fn_sig(&u.prog, main_def).filter(|s| !s.header.abi.is_c_like()).map(|s| s.ret);
    match main_ret {
        Some(ret) if ret != u.tcx.tys.unit => {
            let start = (0..u.prog.defs.len() as u32)
                .map(DefId)
                // std::process re-exports its OS half (`pub use imp::*`: process_unix.rs /
                // process_windows.rs), so the fn's own path is std::process::imp::...
                .find(|d| {
                    u.prog.def(*d).kind == DefKind::Fn
                        && matches!(u.prog.def_path(*d).as_str(), "std::process::__relax_lang_start" | "std::process::imp::__relax_lang_start")
                });
            match start {
                Some(sd) => {
                    let sid = u.fn_id(FnKey::Inst(sd, vec![ret]));
                    Ok((u.entry(sid), entry))
                }
                None => Err("`fn main` returns a value but std has no process::__relax_lang_start".to_string()),
            }
        }
        _ => Ok((entry, 0)),
    }
}

pub(crate) fn find_main(prog: &crate::program::Program, root: u32) -> Option<DefId> {
    let sym = prog.syms.get("main")?;
    let m = prog.crates[root as usize].root_mod;
    let d = prog.lookup_in_module(m, sym, false, &mut Vec::new())?;
    if prog.def(d).kind == DefKind::Fn {
        Some(d)
    } else {
        None
    }
}

/// Non-generic fns listed (by def path) in the previous run's startup file, in order.
fn startup_defs(prog: &crate::program::Program, path: &str) -> Vec<DefId> {
    let list = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    // A path names one fn only when no other fn has it: the methods of different impls share
    // theirs (`core::sync::atomic::::store` is every atomic type's), and a generic impl's
    // method compiled with no type arguments does not normalize (aot warm runs: 63 internal
    // compiler errors in the ahead worker). Ambiguous paths are left to the program's calls.
    let mut by_path: std::collections::HashMap<String, Option<DefId>> = std::collections::HashMap::new();
    for (i, d) in prog.defs.iter().enumerate() {
        if matches!(d.kind, DefKind::Fn | DefKind::AssocFn) {
            by_path.entry(prog.def_path(DefId(i as u32))).and_modify(|e| *e = None).or_insert(Some(DefId(i as u32)));
        }
    }
    let mut out = Vec::new();
    for l in list.lines() {
        if let Some(Some(d)) = by_path.get(l.trim()) {
            out.push(*d);
        }
    }
    out
}

/// Ahead-of-call compilation on a worker (`--ahead=aot-max|aot-min`): the previous run's startup functions, in
/// order, at worker priority. Compiles serialize with lazy compiles on the owner thread, so the
/// worker overlaps with the program's own execution, not with its compiles.
fn start_ahead(up: *mut Unit, defs: Vec<DefId>, runtime: &crate::options::RuntimeOptions) {
    let upn = up as usize;
    let runtime = crate::zone_alloc::kept_clone(runtime);
    { let _k = crate::zone_alloc::kept();
    std::thread::spawn(move || { let _z = crate::zone_alloc::compiler_thread();
        jit::lower_this_thread_priority(&runtime);
        let up = upn as *mut Unit;
        let t = Instant::now();
        let mut n = 0;
        for d in defs {
            let _job = jit::compile_job();
            let (id, done) = crate::owner::call(crate::owner::Class::Program, || {
                let id = unsafe { (&mut *up).fn_id(FnKey::Inst(d, Vec::new())) };
                let done = unsafe { (&*up).fns[id as usize].compiled };
                (id, done)
            });
            if !done {
                let _origin = crate::clock::origin("fncost-ahead");
                unsafe { jit::compile_slot(up, id) };
                n += 1;
            }
        }
        verbose!("makepad-run: ahead worker compiled {} fns in {:.0} ms (t={:.0} ms)", n, t.elapsed().as_secs_f64() * 1e3, crate::clock::ms_since_start());
    });
    }
}

/// Prints new compile errors as they happen (each failed fn panics only if it is called).
fn flush_errors(block: bool) {
    let up = unsafe { RUN_UNIT };
    if up.is_null() {
        return;
    }
    // errors come from unit work, which the owner serves: no request served since the last
    // flush (its own), nothing new to print -- no owner request from an idle run's monitor
    static FLUSHED: AtomicU64 = AtomicU64::new(u64::MAX);
    if !block && FLUSHED.load(Ordering::Relaxed) == crate::owner::SERVED.load(Ordering::Relaxed) {
        return;
    }
    let body = || {
        // (on the owner, this request already counted: later ones make the next flush look)
        FLUSHED.store(crate::owner::SERVED.load(Ordering::Relaxed), Ordering::Relaxed);
        let u = unsafe { &mut *up };
        u.flush_query_errors(false);
        let seen = ERRORS_SEEN.load(Ordering::Relaxed) as usize;
        if !QUIET.load(Ordering::Relaxed) {
            for e in u.errors.iter().skip(seen) {
                eprintln!("makepad-run: [t={:.0} ms] {}", crate::clock::ms_since_start(), e);
            }
        }
        // the report file may be a FIFO a slow reader drains: written after the owner thread releases it
        let to_report: Vec<String> = if u.errors.len() > seen && (reporting() || crate::run_state::observed()) { u.errors.iter().skip(seen).map(|e| e.to_string()).collect() } else { Vec::new() };
        ERRORS_SEEN.store(u.errors.len() as u64, Ordering::Relaxed);
        to_report
    };
    let to_report = if block {
        crate::owner::call(crate::owner::Class::Program, body)
    } else {
        crate::owner::publish(body)
    };
    for e in to_report {
        report("error", &e);
    }

}

/// Writes the disk cache's recorded entries once MAKEPAD_RUN_CACHE_SPILL_MB (default 16; 0 = only at
/// exit) of them are pending: taken on the owner thread (only if free now), written
/// off the unit. A cold Stage run held ~200 MB of recorded code until the exit flush.
fn spill_cache() {
    static LIM: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let lim = *LIM.get_or_init(|| std::env::var("MAKEPAD_RUN_CACHE_SPILL_MB").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(16) << 20);
    let up = unsafe { RUN_UNIT };
    if lim == 0 || up.is_null() {
        return;
    }
    // nothing recorded since the last look: no owner request (an idle run's monitor asked the
    // owner ten times a second)
    static SEEN: AtomicU64 = AtomicU64::new(u64::MAX);
    let records = crate::cache::RECORDS.load(Ordering::Relaxed);
    if SEEN.swap(records, Ordering::Relaxed) == records {
        return;
    }
    let Some(spill) = Some(crate::owner::publish(|| {
        let u = unsafe { &mut *up };
        match u.cache.as_mut() {
            Some(c) if c.pending_bytes() >= lim => c.spill_take(),
            _ => None,
        }
    })) else { return };
    if let Some(s) = spill {
        // its own thread: the monitor loop's other duties (errors, trims) do not wait for the disk
        { let _k = crate::zone_alloc::kept();
        std::thread::spawn(move || { let _z = crate::zone_alloc::compiler_thread();
            let t = Instant::now();
            let r = s.write();
            if !VERBOSE.load(Ordering::Relaxed) {
                if let Err(e) = &r { eprintln!("makepad-run cache: spill: {}", e); }
            }
            if crate::memstat::enabled() || crate::preload::trace() {
                verbose!("makepad-run cache: spilled {:?} entries in {:.1} ms at t={:.0} ms", r, t.elapsed().as_secs_f64() * 1e3, crate::clock::ms_since_start());
            }
        });
        }
    }
}

static SETTLE_REQUESTED: AtomicBool = AtomicBool::new(false);
static FIRST_PRESENT_WALL: AtomicU64 = AtomicU64::new(0);
/// A domain's first frame re-arms settlement even when its code was already installed.
static DOMAIN_PRESENT_WALL: AtomicU64 = AtomicU64::new(0);

pub(crate) fn domain_present() {
    DOMAIN_PRESENT_WALL.fetch_max(jit::wall_ns().max(1), Ordering::Release);
}

static PRESENT_PATH: relax_session::sync::Mutex<Option<std::path::PathBuf>> = relax_session::sync::Mutex::new(None);

/// Observe the platform's existing `present\n` report, independently of whether a launcher
/// requested progress. Only this small report goes through the private rendezvous; progress
/// still goes directly to the launcher (which may be a FIFO).
fn present_report() -> std::io::Result<std::fs::File> {
    let path = std::env::temp_dir().join(format!("makepad-run-present-{}-{}", std::process::id(), jit::wall_ns()));
    let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;
    *PRESENT_PATH.lock().unwrap() = Some(crate::zone_alloc::kept_with(|| path.clone()));
    unsafe { std::env::set_var("MAKEPAD_RUN_REPORT_TO", path); }
    Ok(file)
}

fn remove_present_report() {
    if let Some(path) = PRESENT_PATH.lock().unwrap().take() {
        let _ = std::fs::remove_file(path);
    }
}

/// Embedders may signal that boot has settled before the automatic quiet detector fires.
/// Nonblocking and safe from a JIT callback / CTFE: the monitor performs the drops outside
/// compilation, retrying until the unit and front locks are available. MAKEPAD_RUN_SETTLE_DROP=0
/// disables explicit requests too. This applies to the unit launched by run::cmd_run.
pub fn settle() {
    SETTLE_REQUESTED.store(true, Ordering::Release);
    monitor_wake();
}

/// The run monitor's thread, for `monitor_wake`.
static MONITOR: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();

/// The run monitor looks now (a settle request; it may be in its idle period).
fn monitor_wake() {
    if let Some(t) = MONITOR.get() { t.unpark(); }
}

/// The run monitor's period: 100 ms while anything moves (compiles, installs, cache records,
/// verdicts, a present), MONITOR_IDLE once nothing has moved for MONITOR_QUIET ticks after the
/// first present. Every duty of the loop is either driven by those counters or time-based at
/// a second or more, so an idle app's monitor wakes once a second, not ten times.
const MONITOR_TICK: std::time::Duration = std::time::Duration::from_millis(100);
const MONITOR_IDLE: std::time::Duration = std::time::Duration::from_secs(1);
const MONITOR_QUIET: u32 = 50;

/// What the monitor's duties follow: when none of it changes, there is nothing to do soon.
fn monitor_activity() -> [u64; 7] {
    [
        jit::COMPILED_FNS.load(Ordering::Relaxed) + jit::TIER1_INSTALLS.load(Ordering::Relaxed),
        jit::reclaim_generation(),
        jit::demand_compiled(),
        FIRST_PRESENT_WALL.load(Ordering::Relaxed),
        DOMAIN_PRESENT_WALL.load(Ordering::Relaxed),
        crate::cache::RECORDS.load(Ordering::Relaxed),
        crate::borrowck::lazy::checked(),
    ]
}

/// A run that goes on reaching new code a few fns at a time (a game played: the Sandbox race
/// compiled 1-2 fns a second for minutes) is settled too: at most TRICKLE_MAX first compiles in
/// the last TRICKLE_NS. With "no compile for 1 s" alone such a run never settled, so its
/// compiler generation was never swapped (race, 13 min: RSS 9.5 GB vs 4 GB once swapped).
const TRICKLE_NS: u64 = 5_000_000_000;
const TRICKLE_MAX: u64 = 10;

#[derive(Default)]
struct SettleWatch {
    seen: Option<u64>,
    quiet_since: u64,
    settled: Option<u64>,
    /// (wall ns, compile count) once per TRICKLE_NS / 8, the last 9: the trickle test's window
    marks: [(u64, u64); 9],
    marks_n: usize,
}

impl SettleWatch {
    /// Records the count; whether at most TRICKLE_MAX compiles happened in the last TRICKLE_NS.
    fn trickle(&mut self, now: u64, compiled: u64) -> bool {
        let step = TRICKLE_NS / 8;
        if self.marks_n == 0 || now.saturating_sub(self.marks[(self.marks_n - 1) % 9].0) >= step {
            self.marks[self.marks_n % 9] = (now, compiled);
            self.marks_n += 1;
        }
        if self.marks_n < 9 {
            return false;
        }
        // the oldest mark is 8 steps back (at least TRICKLE_NS ago)
        self.slow(compiled)
    }

    fn ready(&mut self, now: u64, first_present: u64, compiled: u64) -> bool {
        if self.seen != Some(compiled) {
            self.seen = Some(compiled);
            self.quiet_since = now;
        }
        let trickle = self.trickle(now, compiled);
        if first_present == 0 {
            return false;
        }
        let quiet = now.saturating_sub(first_present.max(self.quiet_since)) >= 1_000_000_000;
        let slow = trickle && now.saturating_sub(first_present) >= TRICKLE_NS;
        // a trickle settles again only once it compiled more than TRICKLE_MAX since the last
        // settle (each settle retires the type side: recomputing it after every fn costs CPU)
        (quiet && self.settled != Some(compiled)) || (slow && self.settled.map_or(true, |n| compiled > n + TRICKLE_MAX))
    }

    /// Compiles since the oldest mark of the window (None before the window is covered).
    fn window_delta(&self, compiled: u64) -> Option<u64> {
        (self.marks_n >= 9).then(|| compiled.saturating_sub(self.marks[self.marks_n % 9].1))
    }

    /// At most a trickle of compiles in the last window (as of the last `trickle` mark).
    fn slow(&self, compiled: u64) -> bool {
        self.window_delta(compiled).is_some_and(|d| d <= TRICKLE_MAX)
    }

    /// Settled, and still at most a trickle of compiles since: the swap may go.
    fn settled_now(&self, compiled: u64) -> Option<u64> {
        let n = self.settled?;
        (n == compiled || self.slow(compiled)).then_some(n)
    }
}

/// A successful settle takes pending cache entries even below the normal spill threshold.
/// Disk IO and allocator relief run off the monitor and never hold the unit/front locks.
/// False: a checker held FRONT_RW (the type side was not retired).
fn try_settle(drops: &mut jit::SettleDrops, trim: bool) -> bool {
    let up = unsafe { RUN_UNIT };
    if up.is_null() { return false; }
    // the deferred cache records first, in bounded publishes (the type side they are keyed from
    // is retired below: what installs add meanwhile is flushed in the request)
    jit::drain_records_bounded();
    // a front write: structural work, a program-class request (rule I-PUB: no publish)
    let Some(Some((drops, pending, spill, at, landmark, retired))) = Some(crate::owner::call(crate::owner::Class::Program, || {
        // Immediate releases must be reported even when an ahead checker kept FRONT_RW busy.
        let completed = jit::take_completed_drops();
        drops.fns += completed.fns;
        drops.rir += completed.rir;
        drops.typeck += completed.typeck;
        let drops = std::mem::take(drops);
        unsafe { (&mut *up).flush_cache_records() };
        let retired = unsafe { jit::retire_type_side(up) };
        let u = unsafe { &mut *up };
        let pending = u.cache.as_ref().map_or(0, |c| c.pending_bytes());
        let spill = u.cache.as_mut().and_then(|c| c.spill_take());
        let at = crate::clock::ms_since_start();
        let present = FIRST_PRESENT_WALL.load(Ordering::Acquire).max(DOMAIN_PRESENT_WALL.load(Ordering::Acquire));
        let landmark = if present == 0 { "before first present".to_owned() } else {
            format!("latest first present +{:.0} ms", jit::wall_ns().saturating_sub(present) as f64 / 1e6)
        };
        Some((drops, pending, spill, at, landmark, retired))
    })) else { return false };
    { let _k = crate::zone_alloc::kept();
    std::thread::spawn(move || { let _z = crate::zone_alloc::compiler_thread();
        let result = spill.map(|s| s.write());
        // Borrowck already releases its body-owned queries at each completed check.
        let before = crate::memstat::process_rss();
        let trim_start = Instant::now();
        if trim { crate::memstat::trim(); crate::pack::release_resident(); }
        let trim_ms = trim_start.elapsed().as_secs_f64() * 1e3;
        let after = crate::memstat::process_rss();
        crate::boot_stats::settle_done(before.0, after.0);
        if crate::memstat::enabled() || crate::preload::trace() || VERBOSE.load(Ordering::Relaxed) {
            let mb = |n: u64| n as f64 / 1048576.0;
            eprintln!("makepad-run mem: settle at t={:.0} ms ({}): completed-body drops RIR of {} fns ({:.1} MB), typeck {:.1} MB, lower {:.1} MB, spec {:.1} MB (max sweep {:.1} ms), cache spill {:.1} MB ({:?}); retired {} query values + {} solver entries (recomputed since: {} values); trim{} rss {:.1} -> {:.1} MB in {:.1} ms; before [{}], after [{}]{}",
                at, landmark, drops.fns, mb(drops.rir), mb(drops.typeck), mb(drops.lower), mb(drops.spec),
                drops.max_sweep_ns as f64 / 1e6, mb(pending as u64), result, retired.map_or(0, |r| r.0), retired.map_or(0, |r| r.1), crate::query::RECOMPUTED_VALUES.load(Ordering::Relaxed), if trim { "" } else { " disabled" }, mb(before.0), mb(after.0), trim_ms, before.2, after.2,
                if crate::memstat::enabled() { "" } else { " (RIR estimated; other heap bytes require MAKEPAD_RUN_MEM_REPORT)" });
        }
    });
    }
    // a checker kept FRONT_RW busy: the type side was not retired, so this compile count is not
    // settled yet (the next quiet period tries again)
    retired.is_some()
}

/// The background loop's reclamation pass (jit::settle_drop) runs at most this often after the
/// first present.
const SETTLE_DROP_EVERY_NS: u64 = 2_000_000_000;

fn start_monitor(runtime: &crate::options::RuntimeOptions, options: &crate::options::Options) {
    let present_forward = std::env::var_os("MAKEPAD_RUN_REPORT_TO");
    let mut present = match present_report() {
        Ok(file) => Some(file),
        Err(e) => { eprintln!("makepad-run: first-present report: {}", e); None }
    };
    let (runtime, options, present_forward) = crate::zone_alloc::kept_with(|| (runtime.clone(), options.clone(), present_forward.clone()));
    let _k = crate::zone_alloc::kept();
    { let _k = crate::zone_alloc::kept();
    // named: inspect lists it as makepad run's, not the program's
    // a panic here ended every later settle, swap and trim of the run without a word: the loop
    // runs under the lifeline (reported with its position, restarted with fresh round state)
    crate::lifeline::spawn("run-monitor".into(), crate::lifeline::OnPanic::Restart, None, move || {
        let runtime = &runtime;
        let options = &options;
        // A measurement harness requests an allocator snapshot at its own landmark
        // (for example first present + 20 s), before it reads smaps and stops the app.
        let mut mem_snapshot = std::env::var_os("MAKEPAD_RUN_MEM_SNAPSHOT").map(std::path::PathBuf::from);
        let mut last = u64::MAX;
        let settle_on = runtime.settle_drop;
        // MAKEPAD_RUN_ADOPT=0: the heap tables stay (the cold run's footprint, for comparison)
        let adopt_on = std::env::var("MAKEPAD_RUN_ADOPT").map_or(true, |v| v != "0");
        let mut memos_pending = true;
        let mut settle_watch = SettleWatch::default();
        let mut settle_present = 0;
        let mut drops = jit::SettleDrops::default();
        let mut reclaimed = None;
        let mut last_drop_ns = 0u64;
        let mut reclaim_requested = false;
        // MAKEPAD_RUN_SETTLE_STRESS=<ms>: a settle (drops, retire, trim) is requested that often,
        // whatever compiles run meanwhile (tools/run/fixture/settle_stress.sh)
        let settle_stress = std::env::var("MAKEPAD_RUN_SETTLE_STRESS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|&ms| ms > 0);
        let mut last_stress = Instant::now();
        // the settle rebuild (jit settle_rebuild.rs): once per first present, retried while its
        // preconditions fail
        let (mut rebuild_over, mut rebuild_round, mut rebuild_done) = (!jit::settle_rebuild::enabled(), None, false);
        let (mut rebuild_next, mut rebuild_tries, mut rebuild_said) = (Instant::now(), 0u32, String::new());
        let mut present_bytes = Vec::new();
        let (mut quiet, mut seen, mut trimmed) = (0u32, 0u64, 0u64);
        // growth relief (below): the resident size after the last trim and when it ran
        // burst relief (below): the compile count at the last pass, and when it ran
        let (mut burst_seen, mut burst_at) = (0u64, Instant::now());
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let mut densify_at = Instant::now();
        let (mut grown_from, mut grown_at) = (0u64, std::time::Instant::now().checked_sub(std::time::Duration::from_millis(500)).unwrap_or_else(std::time::Instant::now));
        // mid-run cache writes once main ran 3 s and something new was compiled: after 2 s without
        // compiles (5 s after the last write), or every 10 s while compiles go on
        // (MAKEPAD_RUN_CACHE_MID=0: off)
        let mid_on = std::env::var("MAKEPAD_RUN_CACHE_MID").map_or(true, |v| v != "0");
        let (mut mid_quiet, mut mid_seen, mut mid_flushed, mut mid_last) = (0u32, u64::MAX, 0u64, 0u64);
        let mid_busy = std::sync::Arc::new(AtomicBool::new(false));
        let mut teardown_started = false;
        let _ = MONITOR.set(std::thread::current());
        let (mut activity, mut still) = (monitor_activity(), 0u32);
        loop {
            // idle: one look a second (a settle request wakes it at once)
            let idle = still >= MONITOR_QUIET && present.is_none() && mem_snapshot.is_none() && settle_stress.is_none()
                && !mid_busy.load(Ordering::Relaxed) && FIRST_PRESENT_WALL.load(Ordering::Relaxed) != 0;
            if idle { std::thread::park_timeout(MONITOR_IDLE) } else { std::thread::sleep(MONITOR_TICK) }
            let now = monitor_activity();
            if now == activity { still = still.saturating_add(1) } else { (activity, still) = (now, 0) }
            crate::lifeline::beat();
            if let Some(file) = present.as_mut() {
                use std::io::{Read, Write};
                let _ = file.read_to_end(&mut present_bytes);
                if present_bytes.windows(8).any(|b| b == b"present\n") {
                    FIRST_PRESENT_WALL.store(jit::wall_ns(), Ordering::Release);
                    present = None;
                    remove_present_report();
                    // Forward the unchanged protocol line, off the rendering thread.
                    if let Some(file) = REPORT_FILE.lock().unwrap().as_mut() {
                        let _ = file.write_all(b"present\n");
                    } else if let Some(path) = &present_forward {
                        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(path) {
                            let _ = file.write_all(b"present\n");
                        }
                    }
                }
            }
            if let Some(path) = &mem_snapshot {
                if path.exists() {
                    let mut report = crate::memstat::report("harness snapshot");
                    // structure sizes too (who holds the tags' bytes), when accounting is on
                    let up = unsafe { RUN_UNIT };
                    if crate::memstat::enabled() && !up.is_null() {
                        crate::owner::call(crate::owner::Class::Program, || {
                            for (k, v) in crate::memreport::unit_sizes(unsafe { &*up }) {
                                report.push_str(&format!("makepad-run mem: harness snapshot:   size {:<40} {:>9.1} MB\n", k, v as f64 / 1048576.0));
                            }
                        });
                    }
                    let _ = std::fs::write(path.with_extension("report"), report);
                    mem_snapshot = None;
                }
            }
            if mid_on {
                let n = jit::COMPILED_FNS.load(Ordering::Relaxed) + jit::TIER1_INSTALLS.load(Ordering::Relaxed);
                if n != mid_seen {
                    mid_seen = n;
                    mid_quiet = 0;
                } else {
                    mid_quiet += 1;
                }
                let main_ms = (jit::wall_ns().saturating_sub(MAIN_START_WALL.load(Ordering::Relaxed))) / 1_000_000;
                let start = MAIN_START_WALL.load(Ordering::Relaxed);
                let since = main_ms.saturating_sub(mid_last);
                // also with no new code: the manifest whose window closed, the collected program
                let left = !MID_MANIFEST_DONE.load(Ordering::Relaxed) && jit::FIRST_PRESENT.load(Ordering::Acquire) && jit::wall_ns() >= jit::ORDER_UNTIL_NS.load(Ordering::Relaxed)
                    || !COLLECTED_CURRENT.load(Ordering::Relaxed) && !MID_COLLECTED_TRIED.load(Ordering::Relaxed)
                    || jit::FIRST_PRESENT.load(Ordering::Acquire) && crate::cache::front_graph_pending();
                // the warm record (manifest, front graph, collected program) is written as soon
                // as its pieces are ready after the first present, not after seconds of quiet
                // (a cold run compiles O2 code for long after it): users close apps quickly, and
                // a killed run that wrote nothing left every relaunch unrestored. At most once a
                // second; the collected program's encode makes a unit call only when free.
                let record = left && jit::FIRST_PRESENT.load(Ordering::Acquire) && since >= 1_000;
                // a trickle of first compiles counts as quiet (SettleWatch: a game keeps reaching code)
                let mid_still = mid_quiet >= 20 || settle_watch.slow(jit::demand_compiled());
                let due = n != mid_flushed && ((mid_still && since >= 5_000) || since >= 10_000) || left && mid_still && since >= 5_000;
                // not while the UI thread is in a burst of lazy compiles after the present: the
                // write's unit calls cannot be interrupted once served (one held the owner 620 ms
                // in front of a tab's first visit); it goes in the next pause
                if start != 0 && (record || main_ms >= 3000 && due) && !jit::ui_burst() && !mid_busy.swap(true, Ordering::SeqCst) {
                    mid_flushed = n;
                    mid_last = main_ms;
                    let busy = mid_busy.clone();
                    let quiet = record || mid_still;
                    let _ = std::thread::Builder::new().name("run-cache-write".to_string()).spawn(move || { let _z = crate::zone_alloc::compiler_thread();
                        let _mem = crate::memstat::tag(crate::memstat::Tag::CacheStore);
                        let t = Instant::now();
                        let r = mid_flush(quiet, record);
                        if crate::memstat::enabled() || std::env::var_os("MAKEPAD_RUN_CACHE_TRACE").is_some() {
                            verbose!("makepad-run cache: mid-run write at t={:.0} ms: {:?} (entries, front images) in {:.1} ms", crate::clock::ms_since_start(), r, t.elapsed().as_secs_f64() * 1e3);
                        }
                        busy.store(false, Ordering::SeqCst);
                    });
                }
            }
            // the program's teardown is built ahead once the first frame has been up for 1 s
            // (jit::prepare_teardown): a quit then runs it instead of compiling it
            let presented = jit::FIRST_PRESENT_NS.load(Ordering::Acquire);
            if !teardown_started && presented != 0 && jit::wall_ns().saturating_sub(presented) >= 1_000_000_000 {
                teardown_started = true;
                let up = unsafe { RUN_UNIT };
                if !up.is_null() {
                    unsafe { jit::prepare_teardown(up) };
                }
            }
            let _job = jit::compile_job();
            jit::reassert_signals();
            flush_errors(false);
            // the deferred cache records of this tick's installs (rule I-PUB: bounded publishes)
            if jit::records_due() { jit::drain_records_bounded(); }
            if settle_on {
                let present = FIRST_PRESENT_WALL.load(Ordering::Acquire).max(DOMAIN_PRESENT_WALL.load(Ordering::Acquire));
                if present != settle_present {
                    settle_present = present;
                    settle_watch.settled = None;
                }
                if let Some(ms) = settle_stress {
                    if last_stress.elapsed().as_millis() as u64 >= ms {
                        last_stress = Instant::now();
                        SETTLE_REQUESTED.store(true, Ordering::Release);
                        // and a rebuild after it
                        rebuild_done = false;
                    }
                }
                let requested = SETTLE_REQUESTED.swap(false, Ordering::AcqRel);
                reclaim_requested |= requested;
                // the program's own first compiles: the ahead workers' speculation goes on for
                // minutes after the present (Scope: never 10 in 5 s) and is no reason to wait
                let compiled = jit::demand_compiled();
                let n = jit::reclaim_generation();
                // Installations release their own finished bodies. Shared inline/template
                // inputs stay through the startup frame: evicting them earlier just makes
                // subsequent startup callers rebuild them. After present, retry shared
                // reclamation as compilation progresses, without a global quiet gate.
                // at most every SETTLE_DROP_EVERY_NS (a request goes at once): each pass scans every
                // slot on the owner, and an O3 tail bumped the generation with every install
                let now_ns = jit::wall_ns();
                let settled_d = if (FIRST_PRESENT_WALL.load(Ordering::Acquire) != 0 || reclaim_requested)
                    && (reclaimed != Some(n) && now_ns.saturating_sub(last_drop_ns) >= SETTLE_DROP_EVERY_NS || reclaim_requested) {
                    last_drop_ns = now_ns;
                    Some(crate::owner::publish(|| unsafe { jit::settle_drop(RUN_UNIT) }))
                } else { None };
                if let Some(mut d) = settled_d {
                    d.release_rir();
                    drops.fns += d.fns;
                    drops.rir += d.rir;
                    drops.typeck += d.typeck;
                    drops.lower += d.lower;
                    drops.spec += d.spec;
                    drops.max_sweep_ns = drops.max_sweep_ns.max(d.max_sweep_ns);
                    if d.fns != 0 || !d.front_pending || d.typeck != 0 || d.lower != 0 || d.spec != 0 {
                        // A reader may finish after the last installation was trimmed.
                        // Re-arm allocator relief for pages released at this boundary.
                        quiet = 0;
                        trimmed = u64::MAX;
                    }
                    if !d.front_pending {
                        reclaimed = Some(n);
                        reclaim_requested = false;
                    }
                }
                let quiet = settle_watch.ready(jit::wall_ns(), present, compiled);
                if quiet || requested {
                    if try_settle(&mut drops, runtime.mem_trim) {
                        settle_watch.settled = Some(compiled);
                    } else if requested {
                        SETTLE_REQUESTED.store(true, Ordering::Release);
                    }
                }
                // once the settle ran and this run's collected image is written: the heap tables
                // it was encoded from become views of it (what a warm run maps), then the freed
                // pages go back
                let mut converged = false;
                if settle_watch.settled.is_some() && runtime.mem_trim && adopt_on {
                    if memos_pending {
                        if COLLECTED_RESTORED.load(Ordering::Relaxed) { memos_pending = false; }
                        else if drop_program_memos() { memos_pending = false; converged = true; }
                    }
                    if COLLECTED_WRITTEN.lock().unwrap_or_else(|e| e.into_inner()).is_some() && adopt_collected_tables() { converged = true; }
                }
                if converged {
                    crate::memstat::trim();
                    // MAKEPAD_RUN_MEM_AT_TRIM=1 (with MAKEPAD_RUN_MEM_REPORT): the report after the adoption's trim
                    let up = unsafe { RUN_UNIT };
                    if runtime.mem_at_trim && crate::memstat::enabled() && !up.is_null() {
                        crate::owner::call(crate::owner::Class::Program, || {
                            crate::memstat::print("after the settle and the adoption", &crate::memreport::unit_sizes(unsafe { &*up }));
                        });
                    }
                }
            }
            // one rebuild per first present (the run's, then each domain's) once its compiles have
            // settled: a later burst (tier-up) does not rebuild again (MAKEPAD_RUN_SETTLE_STRESS does)
            let settled = settle_watch.settled_now(jit::demand_compiled());
            if settled.is_some() && rebuild_round != Some(settle_present) {
                rebuild_round = Some(settle_present);
                rebuild_done = false;
                rebuild_tries = 0;
                rebuild_next = Instant::now();
            }
            let rebuild_block: Option<&'static str> = if rebuild_over || rebuild_done || Instant::now() < rebuild_next { Some("") }
                else if settled.is_none() { Some("not settled") }
                else if !(COLLECTED_CURRENT.load(Ordering::Relaxed) || COLLECTED_RESTORED.load(Ordering::Relaxed)) { Some("the collected image is not current") }
                else if COLLECTED_WRITTEN.lock().unwrap_or_else(|e| e.into_inner()).is_some() { Some("the collected image's tables are not adopted yet") }
                else if !MID_MANIFEST_DONE.load(Ordering::Relaxed) { Some("the mid-run manifest is not written") }
                else if crate::cache::front_graph_pending() { Some("front graphs are pending") }
                else if mid_busy.load(Ordering::SeqCst) { Some("a mid-run cache write is running") }
                else if !teardown_started { Some("the teardown is not scheduled") }
                else if jit::QUIT_SEEN.load(Ordering::Relaxed) { Some("the run is quitting") }
                else { None };
            if let Some(why) = rebuild_block.filter(|w| !w.is_empty()) {
                if rebuild_said != why { verbose!("makepad-run settle rebuild waits: {why}"); rebuild_said = why.to_string(); }
            }
            if rebuild_block.is_none() {
                let up = unsafe { RUN_UNIT };
                if !up.is_null() {
                    rebuild_tries += 1;
                    match jit::settle_rebuild::run(up, settle_stress.is_none() && rebuild_tries <= jit::settle_rebuild::TAIL_WAIT) {
                        jit::settle_rebuild::Outcome::Done => rebuild_done = true,
                        jit::settle_rebuild::Outcome::Refused(why) => { verbose!("makepad-run settle rebuild refused: {why}"); rebuild_over = true; }
                        jit::settle_rebuild::Outcome::Retry(why) => {
                            if why != rebuild_said { verbose!("makepad-run settle rebuild deferred (try {rebuild_tries}): {why}"); rebuild_said = why; }
                            rebuild_next = Instant::now() + std::time::Duration::from_secs(1);
                            if rebuild_tries >= jit::settle_rebuild::TAIL_WAIT + 40 { rebuild_done = true; }
                        }
                    }
                }
            }
            // speculative results nobody called within the TTL go (memory: jit::expire_spec)
            jit::expire_spec(options);
            jit::aot_drain();
            jit::ahead_complete_tick();
            // recorded disk-cache entries are written out as they pile up, not held to the exit
            spill_cache();
            // Independent allocator relief for later O3 bursts and non-GUI programs.
            // After 1 s without new compiles (O1 or O3), the malloc allocator's free
            // pages go back to the system, once per compile burst: compile working sets (O3
            // register allocation of a big fn holds 100s of MB for a moment) otherwise stay
            // resident as free-but-held heap (counter warm, x64: 10 s RSS 1,050 -> 892 MB;
            // 2-5 ms on this thread). MAKEPAD_RUN_MEM_TRIM=0: off.
            // Growth relief while compiles go on (cold main -> first present never goes quiet for
            // the trim below): the loader's freed parse memory and earlier compile working sets
            // stay in the malloc arenas of the threads that freed them, where other threads'
            // allocations cannot reuse them, so a cold run's footprint adds them up. Whenever the
            // resident size grew MAKEPAD_RUN_MEM_TRIM_GROWTH_MB (256) since the last trim -- the first
            // check is at main start, right after the load -- and at most every 500 ms, the free
            // pages go back to the system (Calculator cold, 165: 350 MB at main start in 23 ms).
            // Burst relief: the scratch arena's chunks hold the few blocks that outlived their
            // compile (thread-kept buffers, results moved out), and a detached chunk's free slots
            // are never handed out again; every 5 s in which something was compiled, the free
            // granules around them go back (a walk of the chunk headers under each chunk's lock,
            // a few ms; Scope cold: 450 MB that waited for a quiet trim the O3 tail never gave)
            // the program heaps' dense chunks get huge pages, sparse ones lose them (arena.rs
            // `densify`): once a second, at most 64 collapses per heap
            #[cfg(any(target_os = "linux", target_os = "android"))]
            if densify_at.elapsed() >= std::time::Duration::from_secs(1) {
                densify_at = Instant::now();
                crate::zone_alloc::domain::densify_all(64);
            }
            if runtime.mem_trim && burst_at.elapsed() >= std::time::Duration::from_secs(5) {
                let n = jit::COMPILED_FNS.load(Ordering::Relaxed) + jit::TIER1_INSTALLS.load(Ordering::Relaxed);
                if n != burst_seen {
                    burst_seen = n;
                    crate::zone_alloc::burst_trim();
                }
                burst_at = Instant::now();
            }
            if runtime.mem_trim && grown_at.elapsed() >= std::time::Duration::from_millis(500) {
                static STEP: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
                let step = *STEP.get_or_init(|| std::env::var("MAKEPAD_RUN_MEM_TRIM_GROWTH_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(256u64)) << 20;
                let rss = crate::memstat::resident();
                if step > 0 && rss >= grown_from + step {
                    let t = Instant::now();
                    crate::memstat::trim();
                    let after = crate::memstat::resident();
                    if crate::memstat::enabled() || crate::preload::trace() {
                        eprintln!("makepad-run mem: growth trim at t={:.0} ms: rss {} -> {} MB in {:.1} ms", crate::clock::ms_since_start(), rss >> 20, after >> 20, t.elapsed().as_secs_f64() * 1e3);
                    }
                    grown_from = after;
                    grown_at = std::time::Instant::now();
                } else if rss < grown_from {
                    grown_from = rss;
                }
            }
            if runtime.mem_trim {
                let n = if FIRST_PRESENT_WALL.load(Ordering::Acquire) != 0 {
                    jit::reclaim_generation()
                } else {
                    jit::COMPILED_FNS.load(Ordering::Relaxed) + jit::TIER1_INSTALLS.load(Ordering::Relaxed)
                };
                if n != seen {
                    seen = n;
                    quiet = 0;
                } else {
                    quiet += 1;
                    if quiet == 10 && n != trimmed {
                        trimmed = n;
                        let show = crate::memstat::enabled() || crate::preload::trace();
                        let r0 = if show { crate::memstat::resident() } else { 0 };
                        let t = Instant::now();
                        crate::memstat::trim();
                        if show {
                            let r1 = crate::memstat::resident();
                            eprintln!("makepad-run mem: trim at t={:.0} ms: rss {} -> {} MB in {:.1} ms", crate::clock::ms_since_start(), r0 >> 20, r1 >> 20, t.elapsed().as_secs_f64() * 1e3);
                        }
                    }
                }
            }
            // compile progress, read off the unit (the owner thread may be busy)
            let n = jit::COMPILED_FNS.load(Ordering::Relaxed);
            if n != last && reporting() {
                last = n;
                report("fns", &n.to_string());
            }
        }
    });
    }
}

static RUN_DIAGNOSTICS: relax_session::sync::Mutex<Option<relax_types::infer::diagnostics::DiagCtxt>> = relax_session::sync::Mutex::new(None);

/// The diagnostics context the run reports through (a settle rebuild installs its new one).
pub(crate) fn set_diagnostics(dcx: relax_types::infer::diagnostics::DiagCtxt) {
    *RUN_DIAGNOSTICS.lock().unwrap_or_else(|e| e.into_inner()) = Some(dcx);
}


extern "C" fn at_exit() {
    if jit::QUIT_SEEN.load(Ordering::Relaxed) {
        verbose!("makepad-run quit: {} lazy compiles after the quit request, {:.0} ms",
            jit::QUIT_COMPILES[0].load(Ordering::Relaxed), jit::QUIT_COMPILES[1].load(Ordering::Relaxed) as f64 / 1e6);
    }
    // (here, not in the run summary: every exit passes this point)
    verbose!("makepad-run: {}", jit::o2_drop_census());
    finish_run("exit", EXIT_SUMMARY.get().and_then(|exit| exit.code.get().copied()));
}

struct ExitSummary {
    request: std::sync::mpsc::SyncSender<(&'static str, Option<i32>)>,
    worker: std::thread::JoinHandle<()>,
    code: std::sync::OnceLock<i32>,
}
static EXIT_SUMMARY: std::sync::OnceLock<ExitSummary> = std::sync::OnceLock::new();
static RUN_PANIC: std::sync::OnceLock<jit::PanicInfo> = std::sync::OnceLock::new();
/// The exit worker's cache write: not begun (0), under way (1), done (2). finish_run waits for
/// it to begin and to end: a worker not yet scheduled when a fixed wait ran out lost the whole
/// write (the quit's last code: Sandbox's ~4,000 teardown fns, built again by every session).
static EXIT_STAGE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// How long the exit waits for background compiles to finish (they are abandoned
/// at their next safe point). Not getting it means no cache write at all.
const EXIT_QUIESCE_MS: u64 = 200;
/// The thread ending the process runs a unit call itself (a program error in a build or
/// a link called exit): its compile is half done, the exit worker can never intervene, and
/// waiting for it would only delay the exit.
static EXIT_CALLER_HOLDS: AtomicBool = AtomicBool::new(false);
fn exit_quiesce_ms() -> u64 {
    if EXIT_CALLER_HOLDS.load(Ordering::SeqCst) { 0 } else { EXIT_QUIESCE_MS }
}

fn start_exit_summary() {
    let _k = crate::zone_alloc::kept();
    let (request, receive) = std::sync::mpsc::sync_channel::<(&'static str, Option<i32>)>(1);
    let worker = std::thread::Builder::new().name("run-exit".into()).spawn(move || {
        let Ok((why, code)) = receive.recv() else { return };
        EXIT_STAGE.store(1, Ordering::SeqCst);
        exit_mark("start");
        remove_present_report();
        if let Some(code) = code { report("exit", &code.to_string()); }
        // this thread works for the exiting one: its unit calls are served (the owner serves only
        // the exit's from now on); the errors, the panic's report and the summary below are unit
        // calls, made only if the owner answers in time
        crate::owner::exit_helper();
        let up = unsafe { RUN_UNIT };
        let got = !up.is_null() && crate::owner::call_within(crate::owner::Class::Program, exit_quiesce_ms(), || ()).is_some();
        flush_errors(false);
        if let Some(p) = RUN_PANIC.get() {
            // the message itself was printed when main returned
            let rep = if got { crate::owner::call(crate::owner::Class::Program, || unsafe { (&*up).report(p) }) } else { "compiler busy; backtrace unavailable at exit".to_string() };
            eprintln!("makepad-run: main {} report: {}", p.kind, rep);
            report("panic", &format!("main {}: {} -- report: {}", p.kind, p.message, rep));
        }
        if let Some(code) = code {
            crate::why::event("boundary", "run-exit", 0, "", Some(crate::why::Cause::new("", 0, "exit", &code.to_string())));
        }
        exit_mark("errors");
        summary_inner(why);
        EXIT_STAGE.store(2, Ordering::SeqCst);
        if let Some(dcx) = RUN_DIAGNOSTICS.lock().unwrap().as_ref() { dcx.flush_delayed(); }
        exit_mark("summary tail");
        exit_report();
        crate::tui::finish(false);
    });
    // Failure to start optional cleanup must not turn a successful run into a failure.
    if let Ok(worker) = worker { let _ = EXIT_SUMMARY.set(ExitSummary { request, worker, code: std::sync::OnceLock::new() }); }
}

fn finish_run(why: &'static str, code: Option<i32>) {
    jit::exec_check();
    crate::borrowck::lazy::stop();
    let Some(exit) = EXIT_SUMMARY.get() else { return };
    // Only the first caller requests cleanup. In particular the later atexit hook
    // does not spend a second budget after a JIT exit hook ran.
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) { return; }
    // ending on the owner thread itself (an initializer's process::exit): it cannot serve the
    // exit worker while it waits for it
    EXIT_CALLER_HOLDS.store(crate::owner::is_owner(), Ordering::SeqCst);
    jit::begin_exit();
    // a JIT thread that panicked during the run is said once more at its end (inspect threads has the rows)
    if let Some(l) = crate::lifeline::summary() {
        eprintln!("makepad-run: {}", l);
    }
    if exit.request.try_send((why, code)).is_err() { return; }
    // 100 ms for the cleanup, counted from the end of the cache write; the write (and the
    // worker's start before it) only has a hang guard (a store on a stalled disk must not keep
    // the process forever)
    let start = Instant::now();
    let mut deadline = start + std::time::Duration::from_millis(100);
    while !exit.worker.is_finished() {
        let now = Instant::now();
        if EXIT_STAGE.load(Ordering::SeqCst) < 2 && now.duration_since(start).as_secs() < 30 {
            deadline = now + std::time::Duration::from_millis(100);
        }
        if now >= deadline { break; }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// MAKEPAD_RUN_EXIT_TRACE=1: wall ms of each part of the exit work (at_exit), one line at the end.
fn exit_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_EXIT_TRACE").is_some())
}
static EXIT_MARKS: relax_session::sync::Mutex<Vec<(&'static str, u64)>> = relax_session::sync::Mutex::new(Vec::new());
/// Ends the exit-work part `name` (the time since the previous mark).
pub(crate) fn exit_mark(name: &'static str) {
    if exit_trace() {
        let mut marks = EXIT_MARKS.lock().unwrap_or_else(|e| e.into_inner());
        // a cache flush during the run also marks: the exit's parts start at its own start
        if name == "start" { marks.clear(); }
        crate::zone_alloc::kept_with(|| marks.push((name, jit::wall_ns())));
    }
}
fn exit_report() {
    if !exit_trace() { return; }
    let marks = std::mem::take(&mut *EXIT_MARKS.lock().unwrap_or_else(|e| e.into_inner()));
    let Some(&(_, t0)) = marks.first() else { return };
    let mut prev = t0;
    let parts: Vec<String> = marks.iter().skip(1).map(|&(n, t)| { let d = t.saturating_sub(prev); prev = t; format!("{}={:.1}", n.replace(' ', "_"), d as f64 / 1e6) }).collect();
    eprintln!("makepad-run exit: total_ms={:.1} {} (thread {:?}, t={:.0} ms)", prev.saturating_sub(t0) as f64 / 1e6, parts.join(" "), std::thread::current().name().unwrap_or("?"), crate::clock::ms_since_start());
}

/// The boot's collected program is restored or recorded: neither the mid-run writes nor the
/// exit encode it again. Encoding it took
/// 0.3-0.6 s of every exit (Calculator, Files) and rewrote the store whenever the run's type
/// interner had grown.
static COLLECTED_CURRENT: AtomicBool = AtomicBool::new(false);
/// This run's boot came from a collected image (a warm start): nothing converges.
static COLLECTED_RESTORED: AtomicBool = AtomicBool::new(false);

/// Run-time crates must not become part of the next boot's collected image. The caller
/// holds the front-end write lock on the owner thread, before appending the first such crate. Publication
/// still belongs to the post-present writer; an already-restored/written boot needs no copy.
pub(crate) fn preserve_collected_boot(u: &mut jit::Unit) {
    if !COLLECTED_CURRENT.load(Ordering::Relaxed) && front_recordable() {
        if let Some(c) = u.cache.as_mut() { c.preserve_collected_boot(&u.prog, &u.tcx); }
    }
}
/// The boot collected entry this run wrote mid-run (not one a warm restore mapped): the
/// settle makes the program's tables views of it (adopt_collected_tables).
static COLLECTED_WRITTEN: relax_session::sync::Mutex<Option<Box<crate::cache::CollectedWritten>>> = relax_session::sync::Mutex::new(None);

/// A cold run holds what a warm run maps: once the collected image is written and the program
/// is quiet, the heap tables it was encoded from become views of it, decoded entry by entry on
/// use as after a restore (Program::adopt_collected, Tcx::adopt_collected). Tables changed since
/// the encode stay. Retried while the locks are busy; true: done (adopted or nothing to adopt).
fn adopt_collected_tables() -> bool {
    let up = unsafe { RUN_UNIT };
    if up.is_null() { return true; }
    let Some(written) = COLLECTED_WRITTEN.lock().unwrap_or_else(|e| e.into_inner()).as_deref().cloned() else { return true };
    let mut retired: Vec<Box<dyn Send>> = Vec::new();
    let mut notes: Vec<(&'static str, crate::collected::Adopt)> = Vec::new();
    let before = crate::memstat::live_total();
    let t = Instant::now();
    // a front write (try only): structural, a program-class request
    let Some(done) = crate::owner::call(crate::owner::Class::Program, || {
        let Some(_front) = jit::front_try_write() else { return None };
        let u = unsafe { &mut *up };
        if !front_recordable() { return Some(None); }
        let cache = u.cache.as_ref()?;
        Some(cache.adopt_collected(&mut u.prog, &mut u.tcx, &written, &mut retired, |name, how| notes.push((name, how))))
    }) else { return false };
    let held = t.elapsed().as_secs_f64() * 1e3;
    drop(retired);
    let after = crate::memstat::live_total();
    *COLLECTED_WRITTEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
    {
        use crate::collected::Adopt;
        let count = |f: fn(&Adopt) -> bool| notes.iter().filter(|(_, a)| f(a)).count();
        let entries: usize = notes.iter().map(|(_, a)| if let Adopt::Adopted(n) = a { *n } else { 0 }).sum();
        verbose!("makepad-run mem: adopt collected tables at t={:.0} ms: {} adopted ({} entries), {} dirty, {} mismatch, {} already mapped, {} empty; counted heap {:.1} -> {:.1} MB; locks held {:.1} ms{}",
            crate::clock::ms_since_start(), count(|a| matches!(a, Adopt::Adopted(_))), entries, count(|a| matches!(a, Adopt::Dirty)),
            count(|a| matches!(a, Adopt::Mismatch)), count(|a| matches!(a, Adopt::Mapped)), count(|a| matches!(a, Adopt::Empty)),
            before as f64 / 1048576.0, after as f64 / 1048576.0, held,
            if done.is_none() { " (image not the one written: unchanged)" } else { "" });
        if crate::memstat::enabled() { for (name, how) in &notes { verbose!("makepad-run mem:   adopt {} {:?}", name, how); } }
    }
    true
}

/// The settle drops the program's name-resolution memo tables (Program::detach_caches) and the
/// trait solver's caches (as a declaration edit does): what they held refills on use, as in a
/// warm run (which starts with none).
/// Retried while the locks are busy; true: done.
fn drop_program_memos() -> bool {
    let up = unsafe { RUN_UNIT };
    if up.is_null() { return true; }
    let mut retired: Vec<Box<dyn Send>> = Vec::new();
    let before = crate::memstat::live_total();
    let t = Instant::now();
    // a front write (try only): structural, a program-class request
    let Some(()) = crate::owner::call(crate::owner::Class::Program, || {
        let Some(_front) = jit::front_try_write() else { return None };
        let u = unsafe { &mut *up };
        unsafe { u.prog.detach_caches(&mut retired) };
        retired.push(u.tcx.retire_solver_cache());
        Some(())
    }) else { return false };
    let held = t.elapsed().as_secs_f64() * 1e3;
    drop(retired);
    verbose!("makepad-run mem: dropped the name-resolution memos and solver caches at t={:.0} ms: counted heap {:.1} -> {:.1} MB; locks held {:.1} ms",
        crate::clock::ms_since_start(), before as f64 / 1048576.0, crate::memstat::live_total() as f64 / 1048576.0, held);
    true
}

/// A mid-run write tried to record the collected program (once per run; the exit tries again
/// only if it is still not current).
static MID_COLLECTED_TRIED: AtomicBool = AtomicBool::new(false);
/// A mid-run write recorded the startup manifest (its window had closed).
static MID_MANIFEST_DONE: AtomicBool = AtomicBool::new(false);
/// The manifest's slot count when a mid-run write last recorded it: the quit write records it
/// again with the session's later installs
static MID_MANIFEST_LEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Compile totals since boot, the startup list for the aot levels, and the JSON report.
/// jit::wall_ns() when main was called (MAKEPAD_RUN_MAIN_COMPILE_LOG's time base)
static MAIN_START_WALL: AtomicU64 = AtomicU64::new(0);

/// Front images, crate records and the collected graph describe the sources the run booted
/// from. A live session records them too, until its first edit: from then on the Program holds
/// reloaded file versions that are not the crate versions its stores are named after.
pub(crate) fn front_recordable() -> bool {
    crate::front::enabled() && crate::reload::edited_crates().is_empty()
}

/// The app was asked to quit (SIGTERM, SIGINT, SIGHUP: platform termination_signal.rs): the
/// cache is written now, on its own thread, while the app shuts down -- the startup manifest
/// (its window closes here), this run's code and front images, and the collected program.
/// A cold app's shutdown compiles for seconds before the exit write; a harness that kills it
/// 5 s after SIGTERM left no manifest and no collected program, so every relaunch was cold.
/// The exit write still runs and adds what came after.
/// The host's quit request (a signal to the `makepad run` host process, whose app domain has no
/// termination handler of its own): the same quit write the platform's hook starts.
pub fn quit_requested() {
    relax_quit_requested();
}

/// The run ends (its window closed, the app quit, a signal).
pub fn quitting() {
    jit::QUIT_SEEN.store(true, Ordering::Relaxed);
}

/// std::relax::main_hosted: the runner's host has the process's main thread (the HAL's main loop
/// and window) and the program's `main` runs as a guest on a thread of its own. False under the
/// bare runner (`relax`: nothing hosts, the program's own HAL runs its main loop) and for a
/// program without the HAL.
pub(crate) extern "C" fn relax_main_hosted() -> bool {
    HOSTED_RUN.load(Ordering::Acquire)
}

pub(crate) extern "C" fn relax_quit_requested() {
    crate::tui::abnormal();
    quit_mark();
}

/// The app's own shutdown starts (it quit, its last window closed, a remote quit: platform
/// cx.rs): the quit is marked from here, and the quit write starts, as for a signal.
pub(crate) extern "C" fn relax_quit_started() {
    // a child app of a window manager (a domain with a parent) quitting is not the run's end
    if crate::domain::current_app_domain().and_then(crate::domain::info).is_some_and(|i| i.parent.is_some()) {
        return;
    }
    quit_mark();
}

/// The quit write finished (its thread wrote the entries, front images and the collected program):
/// a host that exits the process after it (Android, backgrounded) waits for this.
static QUIT_WRITE_DONE: AtomicBool = AtomicBool::new(false);
pub fn quit_write_done() -> bool {
    QUIT_WRITE_DONE.load(Ordering::SeqCst)
}

fn quit_mark() {
    static ONCE: AtomicBool = AtomicBool::new(false);
    jit::QUIT_SEEN.store(true, Ordering::Relaxed);
    if ONCE.swap(true, Ordering::SeqCst) || unsafe { RUN_UNIT }.is_null() { return; }
    if jit::FIRST_PRESENT.load(Ordering::Acquire) {
        jit::ORDER_UNTIL_NS.fetch_min(jit::wall_ns(), Ordering::Relaxed);
    }
    { let _k = crate::zone_alloc::kept();
    let _ = std::thread::Builder::new().name("run-quit-write".to_string()).spawn(|| { let _z = crate::zone_alloc::compiler_thread();
        let _mem = crate::memstat::tag(crate::memstat::Tag::CacheStore);
        let t = Instant::now();
        // the owner thread is busy while the app's shutdown compiles: retried until it is written
        // (no time cap: the session's final code must reach the cache; the exit ends this thread)
        let mut r = None;
        loop {
            r = mid_flush(true, true).or(r);
            if r.is_some() && (COLLECTED_CURRENT.load(Ordering::Relaxed) || MID_COLLECTED_TRIED.load(Ordering::Relaxed)) { break; }
            // written, the collected program still waiting for the front-end lock: retried less often
            std::thread::sleep(std::time::Duration::from_millis(if r.is_some() { 100 } else { 10 }));
        }
        // the generations the write replaced go now: the exit summary may never run (a harness
        // kills the app, a framework ends the process)
        if r.is_some() { crate::cache::retire_replaced_in_use(); }
        verbose!("makepad-run cache: quit write at t={:.0} ms: {:?} (entries, front images), collected {} in {:.1} ms",
            crate::clock::ms_since_start(), r, COLLECTED_CURRENT.load(Ordering::Relaxed), t.elapsed().as_secs_f64() * 1e3);
        // MAKEPAD_RUN_CACHE_TRACE: written here too (the exit's write may never run)
        crate::cache::write_trace();
        QUIT_WRITE_DONE.store(true, Ordering::SeqCst);
    });
    }
}

/// A write of the disk cache while the program runs (front images, code, borrowck verdicts):
/// programs that never reach Relax's exit summary (killed, or a framework ends the process
/// with _exit) keep what they compiled. The owner thread is called only to take things out of the
/// cache; images are encoded and stores written off the unit. Ok(None): the owner thread was busy.
/// The front images of the program's files the store does not have yet (front.rs), queued
/// for the next spill; the count.
/// `makepad check`: this load's files written as front images and its crate records, as a run's
/// boot does (`push_front_images` + `record_front_crates` + the front graph), so the next check
/// maps them. Only when the cache is on and some file was parsed (nothing new otherwise).
fn record_check_front(u: &mut Unit, options: &crate::options::Options) {
    if !front_recordable() { return; }
    // the loader's front graph write runs on its own thread from load finish: a check ends
    // sooner than a run's first present, so it waits for that write here (else the process
    // exited under it, no graph certificate was ever recorded, and every check validated each
    // crate image again -- or re-parsed, when a selection could not be proven without it)
    if u.prog.front_restored.iter().all(|r| *r) {
        let r = crate::cache::Cache::flush_front_graphs();
        verbose!("makepad check: front graph {:?}", r.err());
        return;
    }
    let Some(mode) = crate::cache::mode_from_env(options) else { return };
    let t = Instant::now();
    let mut c = match crate::cache::Cache::open_collected(u, mode, None) {
        Ok(c) => c,
        Err(e) => { verbose!("makepad check: front images not written: {}", e); return; }
    };
    let n = push_front_images(&u.prog, &mut c);
    c.record_front_crates(&u.prog);
    let written = match c.spill_take() {
        Some(s) => s.write().map(|_| ()),
        None => Ok(()),
    };
    let graph = crate::cache::Cache::flush_front_graphs();
    verbose!("makepad check: {} front images written ({:?}, graph {:?}) in {:.0} ms", n, written.err(), graph.err(), t.elapsed().as_secs_f64() * 1e3);
}

/// References to the images of the program's files (and their parts) that their crate stores
/// do not have yet (front.rs: each image was written when its file or part was published);
/// the count queued.
pub(crate) fn push_front_images(prog: &crate::program::Program, c: &mut crate::cache::Cache) -> usize {
    let mut n = 0;
    for i in 0..prog.files.len() {
        let f = &prog.files[i];
        let (Some(path), Some(&krate)) = (prog.file_paths.get(i), prog.file_crate.get(i)) else { continue };
        let part = f.part.get().and_then(|p| p.image.as_ref());
        if f.image.is_none() && part.is_none() { continue; }
        n += c.push_front_refs(krate, path, f.source_hash(path), f.image.as_ref(), part);
    }
    n
}

/// (verbose) a background owner request's longest stretch between two of its yield checks (the
/// longest the main thread could wait behind it), and the check that ended it
struct YieldProbe(std::sync::Mutex<(Option<Instant>, f64, &'static str, u32, &'static str)>);
/// (YieldProbe) the section of a background owner request running now: a gap between two
/// checks is reported by the section it began in
pub(crate) static YIELD_SECTION: relax_session::sync::Mutex<&'static str> = relax_session::sync::Mutex::new("");
pub(crate) fn yield_section(s: &'static str) { *YIELD_SECTION.lock().unwrap_or_else(|e| e.into_inner()) = s; }
impl YieldProbe {
    fn new() -> Self { Self(std::sync::Mutex::new((None, 0.0, "", 0, ""))) }
    fn start(&self) { self.0.lock().unwrap_or_else(|e| e.into_inner()).0 = Some(Instant::now()); }
    fn tick(&self, at: &'static str) {
        let now = Instant::now();
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let section = *YIELD_SECTION.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = g.0 {
            let ms = (now - l).as_secs_f64() * 1e3;
            if ms > g.1 { g.1 = ms; g.2 = if g.4.is_empty() { at } else { g.4 }; }
        }
        g.4 = section;
        g.0 = Some(now);
        g.3 += 1;
    }
    fn report(&self, what: &str) {
        let g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if g.1 >= 5.0 { verbose!("makepad-run cache: {} held the owner up to {:.1} ms without a yield check (ending at {}; {} checks)", what, g.1, g.2, g.3); }
    }
}

/// Why a mid-run pass did not record the collected program, said once per distinct reason (verbose).
fn collected_skip(why: &'static str) {
    static LAST: relax_session::sync::Mutex<&'static str> = relax_session::sync::Mutex::new("");
    let mut l = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if *l != why { *l = why; verbose!("makepad-run collected: not recorded: {why}"); }
}

/// `wait`: the warm record's writes (after first present, at a quit) wait for the owner thread to
/// complete its work (which a cold run's compiles fill nearly all the time); other writes skip a busy unit.
fn mid_flush(quiet: bool, wait: bool) -> Option<(usize, usize)> {
    // the compiled kernels offered so far
    crate::kernel_store::flush();
    // the first write comes after the start: the mapped front images are checksummed now
    crate::cache::verify_front_images();
    let up = unsafe { RUN_UNIT };
    // one unit call: waits for the unit when `wait`, else only if it is free now
    fn unit_call<R>(wait: bool, f: impl FnOnce() -> R) -> Option<R> {
        // (front writes and whole-cache encodes: structural, a program-class request -- the main
        // thread's requests go first; bounded record publishes are step 4 of OWNERARCH.DESIGN.md)
        let _ = wait;
        Some(crate::owner::call(crate::owner::Class::Program, f))
    }
    if up.is_null() {
        return None;
    }
    // the deferred cache records in bounded publishes (a program thread waits for one batch);
    // the request below holds only what installs add meanwhile
    jit::drain_records_bounded();
    let mut parts = [0f64; 4];
    let mut n_imgs = 0;
    // the files whose front images this write holds: the collected program below is recorded
    // only if no file was loaded since (its restore maps every file from these images)
    let mut imaged_files = usize::MAX;
    let probe = YieldProbe::new();
    let Some(Some((held, spill))) = unit_call(wait, || {
    let held = Instant::now();
    probe.start();
    yield_section("mid write");
    let spill = {
        let u = unsafe { &mut *up };
        u.flush_cache_records();
        parts[3] = held.elapsed().as_secs_f64() * 1e3;
        probe.tick("records");
        yield_section("manifest");
        // the startup manifest, once its window has closed (first present + 5 s after main):
        // what the exit recorded before, so a killed or force-quit app starts warm too; the exit
        // writes it again only if O2 code replaced startup code since (record_manifest)
        // (the quit write records it again when the session installed more since: the whole
        // session's order, linked ahead by the next launch)
        let again = jit::QUIT_SEEN.load(Ordering::Relaxed) && MID_MANIFEST_DONE.load(Ordering::Relaxed) && u.start_order.len() > MID_MANIFEST_LEN.load(Ordering::Relaxed);
        if u.cache.is_some() && jit::FIRST_PRESENT.load(Ordering::Acquire) && jit::wall_ns() >= jit::ORDER_UNTIL_NS.load(Ordering::Relaxed) && (!MID_MANIFEST_DONE.swap(true, Ordering::Relaxed) || again) {
            MID_MANIFEST_LEN.store(u.start_order.len(), Ordering::Relaxed);
            let (order, present) = jit::manifest_order(u.start_order.clone(), jit::ORDER_PRESENT_LEN.load(Ordering::Relaxed));
            let mut cache = u.cache.take();
            if let Some(c) = cache.as_mut() {
                // a frame's lazy compile goes first: the manifest is made again by a later write
                let stop = || { probe.tick("manifest"); crate::owner::main_waiting() && !jit::QUIT_SEEN.load(Ordering::Relaxed) };
                if !c.record_manifest(u, crate::layout::root_crate(), &order, present, &stop) {
                    MID_MANIFEST_DONE.store(false, Ordering::Relaxed);
                    verbose!("makepad-run cache: startup manifest yielded to the main thread; retried later");
                }
            }
            u.cache = cache;
        }
        parts[0] = held.elapsed().as_secs_f64() * 1e3;
        let Some(c) = u.cache.as_mut() else {
            return None;
        };
        yield_section("borrowck keys");
        probe.tick("manifest end");
        for (k, v) in crate::borrowck::lazy::take_verdict_keys() {
            c.record_borrowck(k, v);
        }
        probe.tick("borrowck keys");
        if front_recordable() {
            yield_section("front images");
            n_imgs += push_front_images(&u.prog, c);
            parts[1] = held.elapsed().as_secs_f64() * 1e3;
            yield_section("crate records");
            probe.tick("front images");
            c.record_front_crates(&u.prog);
            parts[2] = held.elapsed().as_secs_f64() * 1e3;
            yield_section("spill take");
            probe.tick("crate records");
            imaged_files = u.prog.files.len();
        }
        let s = c.spill_take();
        probe.tick("spill take");
        s
    };
    Some((held, spill))
    }) else { return None };
    probe.report("mid write");
    let held1 = held.elapsed().as_secs_f64() * 1e3;
    let n = match spill {
        Some(s) => s.write().ok()?,
        None => 0,
    };
    // the front images' line once the program presented (a process ended by its framework
    // never prints the exit summary): files mapped, images not matching, arrays copied back
    static FRONT_REPORTED: AtomicBool = AtomicBool::new(false);
    if jit::FIRST_PRESENT.load(Ordering::Acquire) && !FRONT_REPORTED.swap(true, Ordering::Relaxed) {
        if let Some(s) = crate::front::summary() { verbose!("{} (after the first present)", s); }
    }
    let graph_ready = jit::FIRST_PRESENT.load(Ordering::Acquire)
        && crate::cache::Cache::flush_front_graphs().is_ok();
    // the collected program, once per run and only when the program is quiet (it is encoded
    // under the unit and front-end locks): not at the exit
    if graph_ready && quiet && !COLLECTED_CURRENT.load(Ordering::Relaxed) && !MID_COLLECTED_TRIED.load(Ordering::Relaxed) {
        // not when a file was loaded since this write took its front images (its restore maps
        // every file from them); a later quiet flush or the exit tries again. Files the lazy
        // selection still skips travel in the image (skipped_files, deferred imports).
        // Encoded on the owner thread, but never in the main thread's way: the encode stops when
        // the main thread waits for the owner thread (a frame's lazy compile) and a later pass
        // tries again; the store write runs after the owner thread releases it.
        let cprobe = YieldProbe::new();
        if let Some((ready, retired_boot, t)) = unit_call(wait, || {
            cprobe.start();
            yield_section("collected");
            let mut ready = None;
            let mut retired_boot = None;
            let t = Instant::now();
            let front = jit::front_write_within(if jit::QUIT_SEEN.load(Ordering::Relaxed) { 1000 } else { 50 });
            if front.is_none() { collected_skip("the front-end lock is busy"); }
            if let Some(_front) = front {
                let u = unsafe { &mut *up };
                // files the post-boot loader loaded since this write took its front images (a
                // run that did not restore loads for seconds after present): their images are
                // written now, under the locks, so the record can be made in this pass
                if wait && u.prog.files.len() != imaged_files && front_recordable() {
                    if let Some(c) = u.cache.as_mut() {
                        push_front_images(&u.prog, c);
                        c.record_front_crates(&u.prog);
                        let written = match c.spill_take() {
                            Some(s) => s.write().is_ok(),
                            None => true,
                        };
                        if written { imaged_files = u.prog.files.len(); }
                    }
                }
                if u.prog.files.len() != imaged_files { collected_skip("files were loaded since the front images were taken"); }
                if let Some(c) = u.cache.as_mut().filter(|_| u.prog.files.len() == imaged_files) {
                    let stop = || { cprobe.tick("collected"); crate::owner::main_waiting() && !jit::QUIT_SEEN.load(Ordering::Relaxed) };
                    match c.encode_collected(&u.prog, &u.tcx, &stop) {
                        crate::cache::CollectedRecord::Ready(dest, payload) => { MID_COLLECTED_TRIED.store(true, Ordering::Relaxed); ready = Some((u.prog.session.options.verbose.then_some(c.collected_crates(&u.prog)), dest, payload)); }
                        crate::cache::CollectedRecord::Current => { MID_COLLECTED_TRIED.store(true, Ordering::Relaxed); COLLECTED_CURRENT.store(true, Ordering::Relaxed); retired_boot = Some(c.finish_collected()); }
                        crate::cache::CollectedRecord::Yielded => verbose!("makepad-run collected: record yielded to the main thread after {:.1} ms; retried later", t.elapsed().as_secs_f64() * 1e3),
                        crate::cache::CollectedRecord::Failed => MID_COLLECTED_TRIED.store(true, Ordering::Relaxed),
                    }
                }
            }
            cprobe.tick("collected end");
            (ready, retired_boot, t)
        }) {
            cprobe.report("collected record");
            verbose!("makepad-run cache: collected record held the owner {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
            drop(retired_boot);
            if let Some((report, dest, payload)) = ready {
                if let Some(written) = crate::cache::Cache::commit_collected(report, dest, payload) {
                    COLLECTED_CURRENT.store(true, Ordering::Relaxed);
                    crate::zone_alloc::kept_with(|| *COLLECTED_WRITTEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(written.clone())));
                    let retired_boot = crate::owner::call(crate::owner::Class::Program, || unsafe { &mut *up }.cache.as_mut().map(|c| c.finish_collected()));
                    drop(retired_boot);
                }
            }
        }
    }
    verbose!("makepad-run cache: mid write held the owner {:.1} ms (records to {:.1}, manifest to {:.1}, front images to {:.1}, crate records to {:.1}, then spill)", held1, parts[3], parts[0], parts[1], parts[2]);
    crate::inspect::flightrec::event("cache.write", &format!("mid write: {} records, {} images; held the owner {:.1} ms (manifest {:.1}, front images {:.1}, crate records {:.1})", n, n_imgs, held1, parts[0], parts[1], parts[2]), (held1 * 1e6) as u64, 0);
    // All temporary encoders and retired boot arrays are now outside compiler locks.
    crate::memstat::trim();
    Some((n, n_imgs))
}

pub(crate) fn summary(why: &'static str) {
    if why == "stats" { summary_inner(why); }
    else { finish_run(why, None); }
}

fn summary_inner(why: &str) {
    relax_codegen::fncost::mark("summary");
    // Independent of owner thread state: still emit when exit quiescence times out.
    if why != "stats" { crate::exec_profile::report(false); }
    if why != "stats" {
        // the run is ending: background borrow checks stop at their next preemption point
        // (nothing waits for them; finished verdicts are stored below)
        crate::borrowck::lazy::stop();
        relax_codegen::warm::finish();
        if let Some(console) = GANDALF_CONSOLE.lock().unwrap_or_else(|e| e.into_inner()).take() { console.detach(); }
        exit_mark("warm+console");
    }
    if why != "stats" && std::env::var_os("MAKEPAD_RUN_CACHE_FP").is_some() {
        let late = crate::sip::late_inputs();
        for (c, k, n, h) in &late {
            verbose!("makepad-run cache: late {} {} {} {:032x}", c, if *k == crate::sip::LATE_FILE { "file" } else { "env" }, n, h);
        }
        verbose!("makepad-run cache: {} late inputs", late.len());
    }
    let up = unsafe { RUN_UNIT };
    if up.is_null() {
        return;
    }
    let session = unsafe { crate::jit::unit_session(up) };
    let options = &session.options;
    // at the end of the run the background compilers stop (a mid-run
    // "stats" summary keeps them going); the summary reads the owner thread
    let holder = crate::owner::serving_note();
    let t_wait = std::time::Instant::now();
    // the end of the run never waits on compiles: other threads' builds are abandoned at their next
    // safe point (jit::quiesce); a holder still busy after EXIT_QUIESCE_MS costs this run's
    // summary + cache pack
    let preamble = |got: bool| -> bool {
        if got && holder != "idle" && t_wait.elapsed().as_micros() > 100 && !SILENT.load(Ordering::Relaxed) {
            verbose!("makepad-run: {}: waited {:.1} ms for the owner thread (serving {})", why, t_wait.elapsed().as_secs_f64() * 1e3, holder);
        }
        if why != "stats" { exit_mark("quiesce"); jit::tier1_stats_report(); }
        if !got {
            verbose!("makepad-run: {}: no summary -- the owner thread is still serving {}", why, crate::owner::serving_note());
            return false;
        }
        true
    };
    let rest = || {
        if why != "stats" {
            let (ms, site) = jit::main_post_present_wait();
            verbose!("makepad-run: main thread's longest unit-call wait after first present: {:.1} ms ({})", ms, site);
            verbose!("{}", crate::postframe::summary());
        }
        verbose!("makepad-run: startup seeds skipped: {} initializers (demand evaluation required), {} recursive builds", jit::SEED_INITIALIZERS_SKIPPED.load(Ordering::Relaxed), jit::SEED_RECURSIVE_SKIPPED.load(Ordering::Relaxed));
        // borrowck lane: the background checker's summary, with or without the disk cache; checker
        // panics are counted and named, never silent
        if why != "stats" {
            crate::borrowck::lazy::print_summary(options);
        }
        if let Some(line) = crate::reload::session_summary() {
            verbose!("{}", line);
        }
        if why != "stats" { exit_mark("reports"); }
        // the disk cache: this run's new entries become a pack (on the owner thread: no compiles)
        if why != "stats" {
            let u = unsafe { &mut *up };
            u.flush_cache_records();
            let (order, present) = jit::manifest_order(std::mem::take(&mut u.start_order), jit::ORDER_PRESENT_LEN.load(Ordering::Relaxed));
            let mut cache = u.cache.take();
            if let Some(c) = cache.as_mut() {
                c.record_manifest(u, crate::layout::root_crate(), &order, present, &|| false);
            }
            u.cache = cache;
            exit_mark("manifest");
            if let Some(c) = u.cache.as_mut() {
                // borrowck lane: clean verdicts found in the background are remembered
                for (k, v) in crate::borrowck::lazy::take_verdict_keys() {
                    c.record_borrowck(k, v);
                }

                // front images of the files the store does not have yet (front.rs) and the crate
                // records: a run quit before its first mid-run write has all of them left
                if front_recordable() {
                    push_front_images(&u.prog, c);
                    c.record_front_crates(&u.prog);
                }
                exit_mark("front images");
                let t = std::time::Instant::now();
                // The graph writer opens the crate records just queued above.
                // Flush them first, as the mid-run writer does, then publish the
                // demand parts/graph and finally its collected image. Trying the
                // graph first can fail on a missing crate store and skip the very
                // flush that would make it recordable (also on boot-only exits).
                let r = c.flush().and_then(|written| {
                    crate::cache::Cache::flush_front_graphs()?;
                    Ok(written)
                });
                exit_mark("flush");
                // the generations the writes replaced go now: the rest of the exit can be cut short
                if r.is_ok() { crate::cache::retire_replaced_in_use(); }
                // the collected program, unless restored whole or written mid-run; never waits long
                // for a reader: an app thread parked for good (EXITING) may hold one
                if r.is_ok() && front_recordable() && !COLLECTED_CURRENT.load(Ordering::Relaxed) {
                    if let Some(_front) = jit::front_write_within(2000) {
                        exit_mark("front lock");
                        if c.record_collected(&u.prog, &u.tcx).is_some() { COLLECTED_CURRENT.store(true, Ordering::Relaxed); }
                        exit_mark("collected");
                    } else {
                        verbose!("makepad-run collected: not recorded at exit: the front end is still read by another thread");
                    }
                }
                if !options.verbose {
                    if let Err(e) = &r { eprintln!("makepad-run cache: write: {}", e); }
                }
                let st = &c.stats;
                verbose!(
                    "makepad-run cache: {} hits, {} misses, {} link failures, {} refused by their dependency certificate (per-item cpu: body digests {:.1} ms, certificates written {:.1} ms, checked {:.1} ms) (link {:.1} ms wall with the lock; by speculative workers {} hits {:.1} ms), {} recorded, {} not storable {:?}; written {:?} in {:.1} ms",
                    st.hits, st.misses, st.link_fail, st.cert_rejects, st.body_ns as f64 / 1e6, st.cert_write_ns as f64 / 1e6, st.cert_check_ns as f64 / 1e6, st.link_ns as f64 / 1e6, st.spec_hits, st.spec_link_ns as f64 / 1e6, st.recorded, st.unencodable, c.why_unencodable, r, t.elapsed().as_secs_f64() * 1e3
                );
                if let Some(s) = crate::front::summary() {
                    verbose!("{}", s);
                }
                if c.mode == crate::cache::Mode::Verify {
                    let st = &c.stats;
                    verbose!("makepad-run cache: verify: {} same, {} DIFFERENT", st.verify_same, st.verify_diff);
                }
            }
        }
        if why != "stats" {
            crate::mainprof::write(unsafe { &*up });
            crate::cache::write_trace();
            exit_mark("profiles");
        }
        let u = unsafe { &*up };
        if crate::memstat::enabled() {
            crate::memstat::print(why, &crate::memreport::unit_sizes(u));
            let lp = |i: usize| crate::parser::LAZY_PARSED[i].load(Ordering::Relaxed);
            eprintln!("makepad-run mem: lazy fn bodies: {} files parsed their skipped bodies ({} bodies)", lp(0), lp(1));
            let w = crate::parser::LAZY_WHY_PARSED.lock().unwrap();
            if !w.is_empty() {
                eprintln!("makepad-run mem: lazy parts parsed by: {}", w.iter().map(|(k, v)| format!("{} {}", k, v)).collect::<Vec<_>>().join(", "));
            }
            if let Some(set) = crate::typeck::CHECKED_DEFS.lock().unwrap().as_ref() {
                // a lazy parse by item parses the checked defs' bodies; by file, every body of the files they are in
                let mut files = std::collections::HashSet::new();
                let mut item_exprs = 0usize;
                for &d in set {
                    let df = u.prog.def(crate::program::DefId(d));
                    files.insert(df.file);
                    let it = u.prog.files[df.file as usize].ast.item(df.item);
                    item_exprs += (it.ranges[1] - it.ranges[0]) as usize;
                }
                let all: usize = u.prog.files.iter().map(|f| f.ast.exprs.len()).sum();
                let touched: usize = files.iter().map(|f| u.prog.files[*f as usize].ast.exprs.len()).sum();
                eprintln!("makepad-run mem: lazy-body need: {} defs checked ({} exprs of {} = {:.0}%); {} of {} files touched ({:.0}% of exprs)", set.len(), item_exprs, all, 100.0 * item_exprs as f64 / all as f64, files.len(), u.prog.files.len(), 100.0 * touched as f64 / all as f64);
            }
        }
        let s = &u.stats;
        let now = crate::clock::ms_since_start();
        if !SILENT.load(Ordering::Relaxed) {
        verbose!(
            "makepad-run: {} at t={:.0} ms (main from t={} ms): compiled {} fns, {} KB code, {} compile errors; cpu typeck {:.0} ms, lower {:.0} ms, opt {:.0} ms, codegen {:.0} ms",
            why,
            now,
            MAIN_START_MS.load(Ordering::Relaxed),
            s.compiled,
            s.code_bytes / 1024,
            u.errors.len(),
            s.typeck_ns as f64 / 1e6,
            s.lower_ns as f64 / 1e6,
            s.opt_ns as f64 / 1e6,
            s.codegen_ns as f64 / 1e6
        );
        }
        crate::tui::print_slowest();
        // wall clocks are jit::wall_ns() (started at the first call of it, at boot)
        verbose!(
            "makepad-run: compile wall {:.0} ms (lock held); last compile ended at {:.0} ms after the clock start",
            s.compile_wall_ns as f64 / 1e6,
            s.last_compile_end_ns as f64 / 1e6
        );
        let rt = jit::RT_LAZY.load(Ordering::Relaxed);
        if rt > 0 {
            verbose!("makepad-run: real-time threads compiled {} fns lazily, stalled {:.1} ms in all", rt, jit::RT_LAZY_NS.load(Ordering::Relaxed) as f64 / 1e6);
        }
        {
            let n: Vec<f64> = jit::SPLIT_NS.iter().map(|x| x.load(Ordering::Relaxed) as f64 / 1e6).collect();
            let (mut built, mut used) = (0, 0);
            for f in u.fns.iter() {
                if f.spec_state > 0 {
                    built += 1;
                }
                if f.spec_state == 2 {
                    used += 1;
                }
            }
            if built > 0 || n[0] > 0.0 {
                verbose!(
                    "makepad-run: pipelined compile: callers front {:.0} ms locked, back {:.0} ms unlocked, relock wait {:.0} ms, in-flight waits {:.0} ms; speculative {} built ({} called, {} unused; {} expired after MAKEPAD_RUN_SPEC_TTL_MS, {:.0} MB freed), workers front {:.0} ms back {:.0} ms, {} cold calls skipped{}",
                    n[0], n[1], n[2], n[3], built, used, built - used, jit::SPEC_EXPIRED[0].load(Ordering::Relaxed), jit::SPEC_EXPIRED[1].load(Ordering::Relaxed) as f64 / 1048576.0, n[4], n[5], jit::SPEC_COLD_SKIPPED.load(Ordering::Relaxed), if jit::spec_hot_only(options) { " (fall-through only)" } else { "" }
                );
            }
        }
        if !SILENT.load(Ordering::Relaxed) {
            let h = |i: usize| crate::owner::BUSY_NS[i].load(Ordering::Relaxed) as f64 / 1e6;
            verbose!("makepad-run: owner thread busy: job {:.0} ms, publish {:.0} ms, program {:.0} ms (t={:.0} ms)", h(0), h(1), h(2), crate::clock::ms_since_start());
            let b = |i: usize| crate::owner::BAILS[i].load(Ordering::Relaxed);
            let pf = |i: usize| jit::PREFRONTS[i].load(Ordering::Relaxed);
            let fail: Vec<String> = (1..jit::PREFRONT_FAIL.len()).map(|i| format!("{} {}", jit::PREFRONT_FAIL_NAMES[i], jit::PREFRONT_FAIL[i].load(Ordering::Relaxed))).collect();
            verbose!("makepad-run: background takes (rule I-PURE): {} fronts run off the owner ({:.0} ms cpu, type checks {:.0} ms; {} distinct instance slots, {} found the body checked; made nothing: {}); bails: front {}, initializer {}, jit {}, wait {}; builds dropped {}",
                pf(0), pf(1) as f64 / 1e6, pf(2) as f64 / 1e6, pf(3), pf(4), fail.join(", "), b(0), b(1), b(2), b(3), b(crate::owner::BAIL_DROPPED));
            let m = |i: usize| crate::owner::MAIN_ACC[i].load(Ordering::Relaxed);
            verbose!("makepad-run: main thread's {} owner requests: waited {:.0} ms (queue delay before service {:.0} ms, wake delay after completion {:.0} ms); by wait length <20us {} ({:.0} ms), <200us {} ({:.0} ms), <2ms {} ({:.0} ms), longer {} ({:.0} ms)",
                m(0), m(1) as f64 / 1e6, m(2) as f64 / 1e6, m(3) as f64 / 1e6, m(4), m(5) as f64 / 1e6, m(6), m(7) as f64 / 1e6, m(8), m(9) as f64 / 1e6, m(10), m(11) as f64 / 1e6);
            verbose!("makepad-run: owner by site: {}", crate::owner::sites_summary(14));
            verbose!("makepad-run: publish O1 on the owner by kind: {}", jit::publish_opt_summary());
            verbose!("makepad-run: O3 candidate takes: {}", jit::cand_skip_summary());
            verbose!("makepad-run: O3 install records on the owner: {}x {:.0} ms", jit::O3_INSTALL_REC[0].load(std::sync::atomic::Ordering::Relaxed), jit::O3_INSTALL_REC[1].load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6);
            verbose!("makepad-run: O3 owner cpu by part: {}", jit::o3_owner_summary());
        }
        if !SILENT.load(Ordering::Relaxed) {
            let b = |i: usize| crate::owner::PURE_BAILS[i].load(Ordering::Relaxed);
            let p = |i: usize| crate::owner::PURE_AT_PRESENT[i].load(Ordering::Relaxed);
            use crate::owner::{PURE_AHEAD_BAIL, PURE_BG_EVAL, PURE_BG_FRONT, PURE_BG_JIT, PURE_BG_JOB};
            verbose!("makepad-run: pure-or-bail census (total / before the present): background fronts on the owner {} / {}, background initializer runs {} / {}, background JIT calls {} / {}, const requests of Relax threads {} / {}, lowering-ahead NeedsUnit bails {} / {}",
                b(PURE_BG_FRONT), p(PURE_BG_FRONT), b(PURE_BG_EVAL), p(PURE_BG_EVAL), b(PURE_BG_JIT), p(PURE_BG_JIT), b(PURE_BG_JOB), p(PURE_BG_JOB), b(PURE_AHEAD_BAIL), p(PURE_AHEAD_BAIL));
        }
        let ah = |i: usize| jit::AHEAD_STATS[i].load(Ordering::Relaxed);
        if ah(0) + ah(2) > 0 && !SILENT.load(Ordering::Relaxed) {
            verbose!("makepad-run: type checks ahead (off the lock): {} bodies ({} taken by builds, {} with errors), cpu callers {:.0} ms, workers {:.0} ms", ah(0), ah(1), ah(2), ah(3) as f64 / 1e6, ah(4) as f64 / 1e6);
        }
        if ah(0) > 0 && !SILENT.load(Ordering::Relaxed) {
            let p = |i: usize| jit::AHEAD_AT_PRESENT[i].load(Ordering::Relaxed);
            verbose!("makepad-run: ahead until the first present: {} bodies checked, workers cpu {:.0} ms; {} lowered ({} taken by builds), cpu {:.0} ms", p(0), p(4) as f64 / 1e6, p(5), p(6), p(7) as f64 / 1e6);
        }
        if s.ctfe_data + s.ctfe_code > 0 && !SILENT.load(Ordering::Relaxed) {
            verbose!("makepad-run: constant initializers: {} evaluated to data, {} compiled and run; evaluation cpu {:.1} ms", s.ctfe_data, s.ctfe_code, s.ctfe_ns as f64 / 1e6);
        }
        // MAKEPAD_RUN_MAIN_COMPILE_LOG=<path>: the main thread's compiles, ms since main started: app
        // execution vs compile on the critical path. Per line: start end slot how typeck_ahead
        // lock_wait front back relock inflight_wait (ms) slot-key name (spaces as `~`), then `cpu` front opt codegen (thread cpu ms) `insts` entering opt / codegen; `slot` indexes MAKEPAD_RUN_COMPILE_REPORT's rows
        if let Ok(path) = std::env::var("MAKEPAD_RUN_MAIN_COMPILE_LOG") {
            let base = MAIN_START_WALL.load(Ordering::Relaxed);
            let mut out = format!("# package {}\n", ROOT_DIR.lock().unwrap());
            let ms = |ns: u64| ns as f64 / 1e6;
            for r in jit::MAIN_COMPILES.lock().unwrap().iter() {
                let how = jit::MAIN_HOW_NAMES.get(r.how as usize).copied().unwrap_or("?");
                let (name, hint) = if (r.slot as usize) < u.fns.len() {
                    let f = &u.fns[r.slot as usize];
                    let hint = jit::slot_key(u, r.slot);
                    (f.name.as_ref(), hint)
                } else {
                    ("-", "-".to_string())
                };
                out.push_str(&format!(
                    "{:.3} {:.3} {} {} {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} {} {}  cpu {:.3} {:.3} {:.3} insts {} {}\n",
                    ms(r.start.saturating_sub(base)), ms(r.end.saturating_sub(base)), r.slot, how, ms(r.ns[0]), ms(r.ns[1]), ms(r.ns[2]), ms(r.ns[3]), ms(r.ns[4]), ms(r.ns[5]), hint.replace(' ', "~"), name.replace(' ', "~"), ms(r.cpu[0]), ms(r.cpu[1]), ms(r.cpu[2]), r.insts[0], r.insts[1]
                ));
            }
            let _ = std::fs::write(path, out);
        }
        let q = jit::TIER_QUEUED.load(Ordering::Relaxed);
        if q > 0 {
            verbose!(
                "makepad-run: O3 (ahead {}): {} hot, {} counted again before their build (gone cold), {} installed ({} rebuild requests), {} dropped {:?}; {} RIRs rebuilt ({:.0} ms); snapshots {:.0} ms cpu on the owner (root {:.0}, candidates {:.0}, take tails {:.0}, singles + counts {:.0} ms; callee scans {:.0}, assembly {:.0} ms off it; {} inline candidates built there, {:.0} ms; {} fronts off it, {:.0} ms cpu; {} optimised off it, {:.0} ms cpu, {} panics, {} bails), back ends {:.0} ms cpu off it; bottom-up preparation {:.0} ms cpu ({} bodies simplified, {} kept, {} shared, {} of the simplified had disagreeing shared ones: body {} attrs {} presence {} preparation {} single {}); {} code bytes",
                jit::ahead_level().name(),
                q,
                jit::TIER_REVALIDATED.load(Ordering::Relaxed),
                s.tier1_installed,
                s.tier1_retier,
                s.tier1_dropped,
                s.tier1_drop_why,
                jit::TIER1_RIR_REBUILT[0].load(Ordering::Relaxed),
                jit::TIER1_RIR_REBUILT[1].load(Ordering::Relaxed) as f64 / 1e6,
                s.tier1_locked_ns as f64 / 1e6,
                jit::TIER1_TAKE_PARTS[0].load(Ordering::Relaxed) as f64 / 1e6,
                jit::TIER1_TAKE_PARTS[1].load(Ordering::Relaxed) as f64 / 1e6,
                jit::TIER1_TAKE_PARTS[3].load(Ordering::Relaxed) as f64 / 1e6,
                jit::TIER1_TAKE_PARTS[4].load(Ordering::Relaxed) as f64 / 1e6,
                jit::TIER1_TAKE_PARTS[2].load(Ordering::Relaxed) as f64 / 1e6,
                jit::TIER1_TAKE_PARTS[5].load(Ordering::Relaxed) as f64 / 1e6,
                jit::TIER1_SNAP_BUILT[0].load(Ordering::Relaxed),
                jit::TIER1_SNAP_BUILT[1].load(Ordering::Relaxed) as f64 / 1e6,
                jit::TIER1_SNAP_FRONTS[0].load(Ordering::Relaxed),
                jit::TIER1_SNAP_FRONTS[1].load(Ordering::Relaxed) as f64 / 1e6,
                jit::TIER1_SNAP_OFF[0].load(Ordering::Relaxed),
                jit::TIER1_SNAP_OFF[1].load(Ordering::Relaxed) as f64 / 1e6,
                jit::O3_OPT_PANICS[0].load(Ordering::Relaxed),
                jit::O3_OPT_PANICS[1].load(Ordering::Relaxed),
                s.tier1_free_ns as f64 / 1e6,
                jit::TIER1_PREP[0].load(Ordering::Relaxed) as f64 / 1e6,
                jit::TIER1_PREP[1].load(Ordering::Relaxed),
                jit::TIER1_PREP[2].load(Ordering::Relaxed),
                jit::TIER1_PREP[3].load(Ordering::Relaxed),
                jit::TIER1_PREP[4].load(Ordering::Relaxed),
                jit::TIER1_PREP[5].load(Ordering::Relaxed),
                jit::TIER1_PREP[6].load(Ordering::Relaxed),
                jit::TIER1_PREP[7].load(Ordering::Relaxed),
                jit::TIER1_PREP[8].load(Ordering::Relaxed),
                jit::TIER1_PREP[9].load(Ordering::Relaxed),
                s.tier1_code_bytes,
            );
        }
        let mut tl = String::new();
        for (i, (n, w)) in s.timeline.iter().enumerate() {
            if *n > 0 {
                tl.push_str(&format!(" {}s:{}fns/{}ms", i as f64 / 10.0, n, w / 1_000_000));
            }
        }
    if !SILENT.load(Ordering::Relaxed) {     verbose!("makepad-run: compile timeline (100 ms buckets):{}", tl); }
        if std::env::var("MAKEPAD_RUN_PASS_TIMES").is_ok() {
            let mut passes = String::new();
            for i in 0..relax_backend::stats::PASS_NAMES.len() {
                passes.push_str(&format!("{} {:.0}  ", relax_backend::stats::PASS_NAMES[i], relax_backend::stats::PASS_NS[i].load(Ordering::Relaxed) as f64 / 1e6));
            }
    if !SILENT.load(Ordering::Relaxed) {         verbose!("makepad-run: opt passes (cpu ms): {}", passes); }
            verbose!("makepad-run: invokes {} seen, {} made plain calls", relax_codegen::opt::INVOKES_SEEN.load(Ordering::Relaxed), relax_codegen::opt::INVOKES_DEMOTED.load(Ordering::Relaxed));
        }
        if relax_backend::dbgloc::stats_on() {
            let st = &relax_backend::dbgloc::STATS;
            let g = |k: usize| st[k].load(Ordering::Relaxed);
            eprintln!(
                "makepad-run: variable tables: {} of {} bodies, {} KB of tables, {} KB in pc maps (+{:.1}% on the code entries' {} KB), {:.0} B per body with a table",
                g(0), g(1), g(2) / 1024, g(3) / 1024, 100.0 * g(3) as f64 / g(4).max(1) as f64, g(4) / 1024, g(3) as f64 / g(0).max(1) as f64
            );
        }
        if relax_backend::rir::unwind_stats_on() {
            let st = &relax_backend::rir::UNWIND_STATS;
            verbose!("makepad-run: unwind bytes: pads {} KB, landing stubs {} KB, trailers {} KB; invokes emitted {}", st[0].load(Ordering::Relaxed) / 1024, st[1].load(Ordering::Relaxed) / 1024, st[2].load(Ordering::Relaxed) / 1024, st[3].load(Ordering::Relaxed));
        }
        // startup list: non-generic fns compiled during this run, in slot order (first-call order)
        let mut list = String::new();
        for f in u.fns.iter() {
            if let FnKey::Inst(d, args) = &f.key {
                if f.compiled && args.is_empty() {
                    list.push_str(&u.prog.def_path(*d));
                    list.push('\n');
                }
            }
        }
        let path = STARTUP_PATH.lock().unwrap().clone();
        if !path.is_empty() {
            if let Some(dir) = std::path::Path::new(&path).parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&path, list);
        }
        // per-slot compile report: kind, crate, generic, compiled (= entered: compiles happen on
        // the first call through the lazy stub), RIR built, times inlined, cpu ns; last column
        // `spec`: 0 not built ahead, 1 built ahead and never called (wasted), 2 built ahead and
        // installed by its first call; `spec_depth`: speculation depth it was queued at (-1 none)
        if let Ok(p) = std::env::var("MAKEPAD_RUN_COMPILE_REPORT") {
            let mut out = format!("# build: {}\n", crate::jit::PROVENANCE);
            out.push_str("kind\tcrate\tgeneric\tcompiled\trir\tinlined_into\tbuild_ns\tcg_ns\tsrc\tname\tspec\tspec_depth\torigin\ttrait\tself\thint\tcode_bytes\tO3\tframe0\tframe1\tslots0\tslots1\n");
            let frames = jit::FRAMES.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let fslots = jit::FRAME_SLOTS.lock().unwrap_or_else(|e| e.into_inner()).clone();
            for (idx, f) in u.fns.iter().enumerate() {
                let (kind, krate, generic) = match &f.key {
                    FnKey::Inst(d, args) => ("inst", u.prog.syms.str(u.prog.crates[u.prog.def(*d).krate as usize].name).to_string(), !args.is_empty()),
                    FnKey::Closure(..) => ("closure", "-".to_string(), false),
                    FnKey::Shim(_) => ("shim", "-".to_string(), true),
                    FnKey::Glue(g, _) => (if *g == jit::GLUE_DROP { "drop" } else { "glue" }, "-".to_string(), true),
                    FnKey::Sandbox(_) => ("sandbox", "-".to_string(), false),
                    FnKey::Plain(_) => ("plain", "-".to_string(), false),
                };
                let src = jit::src_size(u, idx as u32);
                let (origin, tr, selfty) = report_origin(u, &f.key);
                // def path of a non-generic fn instance (jit::slot_key has every slot's full key)
                let hint = match &f.key {
                    FnKey::Inst(d, args) if args.is_empty() => crate::program::hint_path(&u.prog.defs, &u.prog.syms, &u.prog.files, *d),
                    _ => "-".to_string(),
                };
                out.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", kind, krate, generic as u8, f.compiled as u8, (f.rir.is_some() || f.build_ns > 0) as u8, f.history().inlined_into.len(), f.build_ns, f.cg_ns, src as i64, f.name, f.spec_state, if f.spec_depth == u8::MAX { -1 } else { f.spec_depth as i32 }, origin, tr, selfty, hint, if f.compiled { f.code_len } else { 0 }, !f.history().retired.is_empty() as u8, frames.get(idx).map_or(0, |x| x.0), frames.get(idx).map_or(0, |x| x.1), fslots.get(idx).map_or(0, |x| x.0), fslots.get(idx).map_or(0, |x| x.1)));
            }
            let _ = std::fs::write(&p, out);
            // code-size sanity: the largest O3 functions (an inliner gone wild shows here first;
            // R13's parser function was 1.9 MB)
            let mut t1: Vec<(u32, &str)> = u.fns.iter().filter(|f| f.compiled && !f.history().retired.is_empty()).map(|f| (f.code_len, f.name.as_ref())).collect();
            t1.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
            let top: Vec<String> = t1.iter().take(5).map(|(n, name)| format!("{} {} KB", name, n / 1024)).collect();
            verbose!("makepad-run: largest O3 functions: {}", if top.is_empty() { "none".to_string() } else { top.join(", ") });
            // stack sanity: the largest frames (O1 and O3)
            let mut fr: Vec<(u32, &str, &str)> = Vec::new();
            for (idx, f) in u.fns.iter().enumerate() {
                if let Some(&(a, b)) = frames.get(idx) {
                    if a > 0 {
                        fr.push((a, "t0", f.name.as_ref()));
                    }
                    if b > 0 {
                        fr.push((b, "t1", f.name.as_ref()));
                    }
                }
            }
            fr.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(b.2)));
            let top: Vec<String> = fr.iter().take(8).map(|(n, t, name)| format!("{} ({}) {} KB", name, t, n / 1024)).collect();
            verbose!("makepad-run: largest frames: {}", top.join(", "));
        }
        let rp = REPORT_PATH.lock().unwrap().clone();
        if !rp.is_empty() {
            let mut j = String::from("{\"phases\":{");
            for (i, (n, ms)) in PHASES.lock().unwrap().iter().enumerate() {
                if i > 0 {
                    j.push(',');
                }
                j.push_str(&format!("\"{}\":{:.1}", n.replace('"', ""), ms));
            }
            j.push_str(&format!(
                "}},\"main_ms\":{},\"end_ms\":{:.0},\"end\":\"{}\",\"compiled\":{},\"compile_errors\":{},\"typeck_ms\":{:.1},\"lower_ms\":{:.1},\"opt_ms\":{:.1},\"codegen_ms\":{:.1}}}\n",
                MAIN_START_MS.load(Ordering::Relaxed),
                now,
                why,
                s.compiled,
                u.errors.len(),
                s.typeck_ns as f64 / 1e6,
                s.lower_ns as f64 / 1e6,
                s.opt_ns as f64 / 1e6,
                s.codegen_ns as f64 / 1e6
            ));
            let _ = std::fs::write(&rp, j);
        }
    };
    if why == "stats" {
        if crate::owner::call_within(crate::owner::Class::Program, 0, || { if preamble(true) { rest(); } }).is_none() {
            preamble(false);
        }
    } else if crate::owner::call_within(crate::owner::Class::Program, exit_quiesce_ms(), || { if preamble(true) { rest(); } }).is_none() {
        preamble(false);
    }
}

/// Compile report columns `origin`, `trait`, `self` of a slot: where its body's source is
/// (`s` the file itself, `r` a macro_rules expansion, `x` a proc-macro / built-in expansion such
/// as a derive), the trait of its impl (or the trait whose default method it instantiates) and
/// its first type argument (the Self of a default method instance).
fn report_origin(u: &jit::Unit, key: &FnKey) -> (&'static str, String, String) {
    let pos_origin = |file: u32, lo: u32| -> &'static str {
        let pf = &u.prog.files[file as usize];
        if lo < pf.src_len {
            "s"
        } else if pf.rules_exp.iter().any(|r| lo >= r.0 && lo <= r.1) {
            "r"
        } else {
            "x"
        }
    };
    match key {
        FnKey::Inst(d, args) => {
            let def = u.prog.def(*d);
            if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn) {
                return ("-", String::new(), String::new());
            }
            let it = u.prog.files[def.file as usize].ast.item(def.item);
            let origin = pos_origin(def.file, it.lo);
            let mut tr = String::new();
            if def.parent != crate::program::NO_DEF {
                let p = u.prog.def(def.parent);
                match p.kind {
                    DefKind::Trait => tr = format!("T:{}", u.prog.syms.str(p.name)),
                    DefKind::Impl => {
                        tr = "I:".to_string();
                        let pf = &u.prog.files[p.file as usize];
                        if let Some(crate::ast::ImplItem { trait_: Some(path), .. }) = pf.ast.as_impl(&pf.ast.item(p.item).kind) {
                            if let Some(seg) = pf.ast.segs(path.segs).last() {
                                tr.push_str(pf.text(seg.name));
                            }
                        }
                    }
                    _ => {}
                }
            }
            let selfty = args.first().map(|t| crate::typeck::type_name(&u.prog, &u.tcx, *t)).unwrap_or_default();
            (origin, tr, selfty)
        }
        FnKey::Closure(file, ce, parent) => {
            let e = u.prog.files[*file as usize].ast.expr(crate::ast::ExprId(*ce));
            let origin = pos_origin(*file, e.lo);
            let (_, tr, selfty) = if (*parent as usize) < u.fns.len() && !matches!(u.fns[*parent as usize].key, FnKey::Closure(..)) {
                report_origin(u, &u.fns[*parent as usize].key)
            } else {
                ("-", String::new(), String::new())
            };
            (origin, tr, selfty)
        }
        FnKey::Shim(_) | FnKey::Glue(..) | FnKey::Sandbox(_) | FnKey::Plain(_) => ("-", String::new(), String::new()),
    }
}

/// The workspace (dirs.rs): MAKEPAD_RUN_WORKSPACE, else the current folder.
pub fn workspace_dir() -> String {
    crate::dirs::workspace_dir()
}

#[cfg(test)]
mod settle_tests {
    use super::SettleWatch;

    #[test]
    fn settle_waits_for_present_and_quiet_then_rearms_after_a_burst() {
        let mut w = SettleWatch::default();
        let sec = 1_000_000_000;
        assert!(!w.ready(sec, 0, 0));
        assert!(!w.ready(2 * sec, 0, 10));
        assert!(!w.ready(3 * sec, 3 * sec, 10));
        assert!(w.ready(4 * sec, 3 * sec, 10));
        w.settled = Some(10);
        assert!(!w.ready(8 * sec, sec, 10));
        assert!(!w.ready(9 * sec, sec, 11));
        assert!(w.ready(10 * sec, sec, 11));
    }

    #[test]
    fn a_trickle_of_compiles_settles_once_then_again_after_more_than_the_trickle() {
        let mut w = SettleWatch::default();
        let tick = 100_000_000;
        let present = tick;
        // one compile every 700 ms: never 1 s quiet
        let mut settled_at = None;
        for t in 1..200u64 {
            let compiled = t / 7;
            if w.ready(t * tick, present, compiled) {
                settled_at = Some(t);
                w.settled = Some(compiled);
                break;
            }
        }
        let t0 = settled_at.expect("a trickle settles");
        assert!(t0 * tick >= present + super::TRICKLE_NS);
        assert!(w.settled_now(t0 / 7 + 1).is_some());
        // the next settle needs more than TRICKLE_MAX new compiles
        let n0 = w.settled.unwrap();
        for t in t0 + 1..t0 + 40 {
            assert!(!w.ready(t * tick, present, t / 7) || t / 7 > n0 + super::TRICKLE_MAX);
        }
        // a burst: not slow, the swap waits
        assert!(w.settled_now(n0 + 500).is_none());
    }
}

/// The memory-throttled compile mode (mobile; local/plans/memory-use.md): MAKEPAD_RUN_MEM_BUDGET_MB set.
/// Desktop default: unset, full parallelism -- the cold preload then runs ahead over all crates
/// and write-then-map only frees each crate's front end after its load.
fn mem_budget_mode() -> bool {
    std::env::var("MAKEPAD_RUN_MEM_BUDGET_MB").map_or(false, |v| v.parse::<u64>().map_or(false, |n| n > 0))
}
