//! Proc-macro host: source-matched linked libraries run natively; other proc-macro crates
//! are compiled by Relax itself, in process.
//!
//! The loader registers the crate graph (`register`). On the first derive call the host builds
//! a separate "macro world": its own Program (std core/alloc/std, std's `proc_macro`,
//! every proc-macro crate of the graph and their dependencies), Tcx and JIT Unit. A call
//! encodes the input tokens (pm.rs `encode`), calls `proc_macro::__relax::bridge` in the world
//! with the macro fn's address, and decodes the output. Functions compile lazily on first call,
//! like any Relax code; the world lives for the process (later calls reuse it).
//!
//! Trust: the process's policy for third-party compile-time code (sandbox.rs `Policy`, chosen by
//! the entry point):
//! - `Policy::Open` (bare `relax`, the dev/test runner for general crates): every proc-macro
//!   crate runs as cargo would run it, `unsafe` and std capabilities included.
//! - `Policy::Sandboxed` (`makepad run`, and the default): a proc-macro root whose dependency
//!   closure holds a third-party package (a registry or git package by its cargo package id,
//!   wherever its sources are stored) runs confined, root and closure: as sandbox twins
//!   (sandbox.rs, the mechanism run-time modules use) in the world's arena, entered through the
//!   data-only bridge. Its code reaches memory of the arena only, and the host only through the
//!   macro domain's entries (`macro_allow`): tokens in and out, the lexer, the invoking crate's
//!   cargo environment, randomness. A file, socket, process, thread, clock or the process
//!   environment is a C function a twin cannot call: the macro fails with that function named,
//!   at its call site. A root whose closure is path packages only (the workspace's, makepad's)
//!   runs as host code; a crate both kinds of root use has both forms. Where the sandbox has no
//!   arena windows (Windows, so far) a confined root's macros fail, saying so: never run as host.
//!   The twins' code goes to the world's disk cache like its other code (keyed by the macro
//!   domain and their base's key; what it holds of the run's sandbox is linked again): a warm
//!   start compiles none of it.
//! The same switch covers build scripts: under `Policy::Sandboxed` the build script of a package
//! whose units hold a third-party package runs confined (relax-front unitgraph.rs
//! `script_confined`, sandbox/script.rs), and its output lines are held to its folders.
//! Every call runs under the hang watchdog.

use crate::macro_reg::{in_world, set_in_world, trace, Entry, REG, STATS};
use std::cell::Cell;
use crate::pm::{Span, TokenStream};
use crate::program::*;

thread_local! {
    /// the last `relax_pm_parse` result of this thread (the macro copies it right away)
    static PARSE_OUT: std::cell::RefCell<Vec<u8>> = Default::default();
    /// the last `relax_pm_getenv` result of this thread (the macro copies it right away)
    static ENV_OUT: std::cell::RefCell<Vec<u8>> = Default::default();
}

/// `"..".parse::<TokenStream>()` inside JIT-run macro code (Relax's std proc_macro FromStr):
/// Relax's lexer, the stream in the bridge encoding in a buffer of this thread (valid until its
/// next call). 0: lexed; 1: not valid tokens.
pub extern "C" fn relax_pm_parse(src: *const u8, len: usize, out: *mut *const u8, out_len: *mut usize) -> i32 {
    let s = unsafe { std::slice::from_raw_parts(src, len) };
    let Ok(s) = std::str::from_utf8(s) else { return 1 };
    let Some(ts) = crate::builtins::lex_stream(s) else { return 1 };
    PARSE_OUT.with(|b| {
        let mut b = b.borrow_mut();
        *b = crate::pm::encode(Span::DUMMY, &ts);
        unsafe {
            *out = b.as_ptr();
            *out_len = b.len();
        }
    });
    0
}

/// Per-crate compile-time environment variable (`CARGO_MANIFEST_DIR`, `CARGO_PKG_*`, build-script
/// `rustc-env`) for the crate invoking the currently running JIT proc macro.
/// 0: found (`out` and `out_len` set); 1: not set in the invoking crate's environment.
pub extern "C" fn relax_pm_getenv(key: *const u8, len: usize, out: *mut *const u8, out_len: *mut usize) -> i32 {
    let s = unsafe { std::slice::from_raw_parts(key, len) };
    let Ok(s) = std::str::from_utf8(s) else { return 1 };
    let Some(val) = crate::macro_reg::call_krate_env(s) else { return 1 };
    ENV_OUT.with(|b| {
        let mut b = b.borrow_mut();
        b.clear();
        b.extend_from_slice(val.as_bytes());
        unsafe {
            *out = b.as_ptr();
            *out_len = b.len();
        }
    });
    0
}

/// The JIT macro world as the front end's macro runner (macro_reg.rs).
struct JitRunner;

impl crate::macro_reg::MacroRunner for JitRunner {
    fn native(&self, entry: &Entry) -> bool { crate::native_macros::find(entry).is_ok_and(|n| n.is_some()) }
    fn declaration_surface(&self, entry: &Entry) -> bool {
        crate::native_macros::find(entry).is_ok_and(|n| n.is_some_and(|native| native.declaration.is_some()))
    }
    fn declare(&self, sess: &std::sync::Arc<crate::Session>, entry: &Entry,
        attr: Option<&TokenStream>, input: &TokenStream, span: Span) -> Result<TokenStream, String> {
        let native = crate::native_macros::find(entry)?.ok_or("macro declaration source does not match")?;
        let _profile = crate::load_profile::span(sess.runtime.profile, "macro-declaration", &entry.fname);
        native.declarations(attr, input, span)
    }
    fn reusable(&self, entry: &Entry) -> bool { crate::native_macros::find(entry).is_ok_and(|n| n.is_some()) }
    fn registered(&self, _sess: &std::sync::Arc<crate::Session>) -> Result<(), String> {
        crate::native_macros::prepare()
    }
    fn prewarm(&self, sess: &std::sync::Arc<crate::Session>) -> Result<(), String> {
        crate::native_macros::verdict()?;
        prewarm(sess)
    }
    fn call(&self, sess: &std::sync::Arc<crate::Session>, entry: &Entry, input: &TokenStream, call_span: Span) -> Result<TokenStream, String> {
        if let Some(native) = crate::native_macros::find(entry)? {
            let _profile = crate::load_profile::span(sess.runtime.profile, "macro-native", &entry.fname);
            return native.call(None, input, call_span);
        }
        call(sess, entry, None, input, call_span)
    }
    fn call_attr(&self, sess: &std::sync::Arc<crate::Session>, entry: &Entry, attr: &TokenStream, item: &TokenStream, call_span: Span) -> Result<TokenStream, String> {
        if let Some(native) = crate::native_macros::find(entry)? {
            let _profile = crate::load_profile::span(sess.runtime.profile, "macro-native", &entry.fname);
            return native.call(Some(attr), item, call_span);
        }
        call(sess, entry, Some(attr), item, call_span)
    }
}

static JIT_RUNNER: JitRunner = JitRunner;

/// Makes the front end run proc macros in the JIT macro world (every Relax entry point calls
/// it before loading a crate graph).
pub fn install() {
    crate::macro_reg::set_runner(&JIT_RUNNER);
}


/// Built once per process (`world`) and called from any thread at once: parse workers expand
/// their files' macros themselves. Its code compiles on the owner thread like any JIT code; the
/// tables below are behind their own locks.
struct World {
    /// leaked: the world lives for the process (program threads share a unit the same way)
    u: *mut crate::jit::Unit,
    bridge: u64,
    /// unit index -> crate index in the world
    crate_of: Vec<u32>,
    entries: relax_session::sync::Mutex<Vec<((usize, String), u64)>>,
    /// macro fns whose code does not compile: (unit, fn) -> the error (deterministic, not retried)
    broken: relax_session::sync::Mutex<Vec<((usize, String), String)>>,
    /// unit -> its macros run confined (Policy::Sandboxed, a third-party package in its closure)
    confined: Vec<bool>,
    /// the confined macros' arena and the bridge's twin, or why there is none (no arena windows
    /// on this platform): every confined macro reports it
    sandbox: Result<(std::sync::Arc<crate::sandbox::Arena>, u64), String>,
    /// unit -> why its crate is not in the world: it did not load, or a crate it builds on did not
    /// (its macros report that; every other crate's macros work)
    failed: Vec<Option<String>>,
}

impl Drop for World {
    fn drop(&mut self) {
        // the last holder is gone (free_world): park compile jobs off the unit, then drop it
        unsafe {
            if let Ok((a, _)) = &self.sandbox {
                // (no call is inside: the window's pages go, its addresses stay taken)
                crate::sandbox::free_arena(&*self.u, a);
            }
            crate::jit::retire_unit(self.u);
            drop(Box::from_raw(self.u));
        }
    }
}

unsafe impl Send for World {}
unsafe impl Sync for World {}


/// boot trace: [world build start, world ready, first expansion asked, first call returned,
/// waits for the world, their summed wait] (f64 ms bits)
static TL: [std::sync::atomic::AtomicU64; 6] = [const { std::sync::atomic::AtomicU64::new(0) }; 6];

fn tl_set_once(i: usize, v: f64) {
    let _ = TL[i].compare_exchange(0, v.to_bits(), std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed);
}

fn tl_add(i: usize, v: f64) {
    TL[i].update(std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed, |x| (f64::from_bits(x) + v).to_bits());
}

pub fn timeline() -> [f64; 6] {
    let mut o = [0.0; 6];
    for i in 0..6 {
        o[i] = f64::from_bits(TL[i].load(std::sync::atomic::Ordering::Relaxed));
    }
    o
}

pub static PH: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];
/// The macro world while it exists: built on first need, freed once the program's files are
/// loaded (free_world: no expansions until a hot reload or a run-time module loads source), then
/// rebuilt on the next need -- from the disk cache, so a rebuild compiles nothing.
static WORLD: relax_session::sync::Mutex<Option<std::sync::Arc<Result<World, String>>>> = relax_session::sync::Mutex::new(None);
/// the stage worlds, in build order: stage k holds the proc-macro crates whose own crates use only
/// macros of stages < k (zbus_macros -> zvariant uses zvariant_derive -> zvariant_utils uses
/// serde_derive: three stages). rustc builds each proc-macro crate after the ones its dependencies
/// use; the stages are that order, one world per layer.
static BOOT_WORLDS: relax_session::sync::Mutex<Vec<std::sync::Arc<Result<World, String>>>> = relax_session::sync::Mutex::new(Vec::new());
/// The crates whose expansions called into the macro world (by name), in the order of their first
/// call: a live test session whose own crates are among them keeps the world after its load
/// (run.rs), as the reload of an edited file runs their hosted macros again.
static HOSTED_CALLERS: relax_session::sync::Mutex<Vec<String>> = relax_session::sync::Mutex::new(Vec::new());
/// Did a crate that `own` accepts call a hosted macro?
pub fn hosted_caller(own: impl Fn(&str) -> bool) -> bool {
    HOSTED_CALLERS.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|k| own(k))
}
/// held while the world is being built: other threads wait for that build
static BUILDING: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());

fn world_now() -> Option<std::sync::Arc<Result<World, String>>> {
    WORLD.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn world_for(sess: &std::sync::Arc<crate::Session>, unit: usize) -> Option<std::sync::Arc<Result<World, String>>> {
    if in_world() {
        return None;
    }
    if let Some(w) = world_now() {
        return Some(w);
    }
    let stage = BOOT_WORLDS.lock().unwrap_or_else(|e| e.into_inner()).iter()
        .find(|bw| bw.as_ref().as_ref().is_ok_and(|w| w.crate_of.get(unit).copied().unwrap_or(u32::MAX) != u32::MAX || w.failed.get(unit).is_some_and(Option::is_some))).cloned();
    if stage.is_some() {
        return stage;
    }
    world(sess)
}


/// The world, built by the first caller (again after free_world); None on the threads building it.
fn world(sess: &std::sync::Arc<crate::Session>) -> Option<std::sync::Arc<Result<World, String>>> {
    world_inner(sess, false)
}

// macro_reg::world_builder: this thread holds BUILDING (it builds the world) or parses one of the
// world's crates for it (preload jobs carry the loader's state). Its boot-world loads clear
// IN_WORLD to expand macros, so a macro the boot world lacks must not wait for BUILDING here: that
// wait would be on itself, or on a builder that waits for this parse.

fn world_inner(sess: &std::sync::Arc<crate::Session>, prewarm: bool) -> Option<std::sync::Arc<Result<World, String>>> {
    if in_world() || crate::macro_reg::world_builder() {
        return None;
    }
    if let Some(w) = world_now() {
        return Some(w);
    }
    let t = std::time::Instant::now();
    // Any non-prewarm caller is a macro expansion on a demand, parse, or loader thread waiting for
    // the world. Even when it wins BUILDING before prewarm starts, its std files must not wait for
    // the program's std load (which may be blocked behind this caller).
    let _wait = (!prewarm).then(crate::program::WorldWait::begin);
    let _b = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(w) = world_now() {
        tl_add(4, 1.0);
        tl_add(5, t.elapsed().as_secs_f64() * 1e3);
        return Some(w);
    }
    tl_set_once(0, crate::clock::ms_since_start());
    // restored also when build_world panics (the thread may be a pool worker that goes on)
    struct InWorld;
    impl Drop for InWorld {
        fn drop(&mut self) { set_in_world(false); crate::macro_reg::set_world_builder(false); }
    }
    let w = {
        set_in_world(true);
        crate::macro_reg::set_world_builder(true);
        let _in_world = InWorld;
        let _single = SingleLoad::when(single_world_load());
        std::sync::Arc::new(build_world(sess))
    };
    tl_set_once(1, crate::clock::ms_since_start());
    *WORLD.lock().unwrap_or_else(|e| e.into_inner()) = Some(w.clone());
    // BUILDING goes before the owner is asked: the owner may be the thread waiting for it (a
    // module loaded at run time on a program request expands its macros there), and it would
    // wait for this build while the build waits in its queue.
    drop(_b);
    // the runtime's unit is the final world's until the program's runs, set on the owner (no
    // owner-path macro call is in flight there): a stage world's call left its unit behind. A
    // world built while the program runs (an expansion of a lazily parsed body) leaves the
    // program's in place (jit.rs adopt_runtime_unit). A macro call that comes first adopts the
    // world's unit itself (jit.rs call_jit_in), and `w` keeps the world alive until this is done.
    if let Ok(world) = w.as_ref() {
        let wu = world.u as usize;
        crate::owner::call(crate::owner::Class::Program, || unsafe { crate::jit::set_runtime_unit(wu as *mut crate::jit::Unit) });
    }
    tl_add(4, 1.0);
    tl_add(5, t.elapsed().as_secs_f64() * 1e3);
    Some(w)
}

/// Does this world build load its crates on its own thread, with no parse workers? Yes on the
/// owner while the program runs: a worker's expansion with a stage world is then an owner request
/// (`call`), and the owner is the thread that waits in the load for that worker (a live edit's
/// first derive after a warm boot built the world inside the patch's request, and a large app's
/// reload hung with the owner in `Pool::take_ahead`). On the owner a request runs in place, so the
/// loads expand their macros there, as an added crate's do (`modules::add_crates`).
fn single_world_load() -> bool {
    crate::owner::is_owner() && !crate::run::running_unit().is_null()
}

/// Sets the thread's single-thread load (`preload::SINGLE`) while it lives, then restores it.
struct SingleLoad(Option<bool>);
impl SingleLoad {
    fn when(single: bool) -> Self {
        Self(single.then(|| crate::preload::SINGLE.with(|s| s.replace(true))))
    }
}
impl Drop for SingleLoad {
    fn drop(&mut self) {
        if let Some(before) = self.0 {
            crate::preload::SINGLE.with(|s| s.set(before));
        }
    }
}

/// After the program's files are loaded: the macro world goes (its Program, type context and
/// JIT unit: ~120 MB for makepad's macro crates). Its compiled code is in the disk cache, so a
/// later need (hot reload, a run-time module) rebuilds it without compiling. Compile jobs are
/// parked while the unit goes (a speculative worker may still hold the world's unit).
/// free_world drops the macro world when no macro call holds it; true when it went.
/// Is the proc-macro world alive (an auxiliary unit)?
pub fn world_alive() -> bool {
    WORLD.lock().map(|g| g.is_some()).unwrap_or(true) || BOOT_WORLDS.lock().map(|g| !g.is_empty()).unwrap_or(true)
}

pub fn free_world() -> bool {
    // A build in flight keeps the stage worlds until it is done: the final world's crates that use
    // a stage's macros expand them there (world_for), and nothing else holds a stage. The load
    // does not wait for every thread that can start a build (a demand plan's discovery runs a
    // hosted item macro on its own thread): a warm load that parsed one file ended while that
    // build went on, the stages went here, and every derive of the world's own crates failed
    // ("a proc macro used while the macro world itself is being built": 85 errors of a check).
    // The worlds stay for a later free. BUILDING is held only while they are taken: dropping
    // one retires its unit, which parks every compile job, and a thread that waits for BUILDING
    // must not wait behind that.
    let (bw, w) = {
        let _building = match BUILDING.try_lock() {
            Ok(held) => held,
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return false,
        };
        let bw: Vec<_> = {
            let mut g = BOOT_WORLDS.lock().unwrap_or_else(|e| e.into_inner());
            let (free, keep): (Vec<_>, Vec<_>) = g.drain(..).partition(|w| std::sync::Arc::strong_count(w) == 1);
            *g = keep;
            free
        };
        let w = {
            let mut g = WORLD.lock().unwrap_or_else(|e| e.into_inner());
            // Only the caller retires the world's unit (retire_unit parks every compile job): a macro
            // call still in flight keeps it for a later free. Dropped by whichever call finished last,
            // the retire would run on an arbitrary thread -- a parse worker or script thread whose
            // loader holds a compile job and waits for it, so the park would wait forever. Nobody
            // clones it without this lock: a count of 1 here stays 1.
            match g.as_ref() {
                Some(w) if std::sync::Arc::strong_count(w) == 1 => g.take(),
                _ => None,
            }
        };
        (bw, w)
    };
    drop(bw);
    let Some(w) = w else { return false };
    if crate::preload::trace() {
        eprintln!("boot-trace: macro world freed");
    }
    drop(w);
    true
}

/// Starts building the world on its own thread when the graph has proc macros the JIT host
/// runs (none linked for them): it is ready, or nearly, when the first file needs it. Only for
/// a load that parses (run.rs); a warm restore builds it on the first expansion instead.
fn prewarm(sess: &std::sync::Arc<crate::Session>) -> Result<(), String> {
    let entries = REG.with(|r| r.borrow().as_ref().map(|reg|
        reg.derives.iter().chain(&reg.macros).map(|(_, e)| e.clone()).collect::<Vec<_>>()
    ).unwrap_or_default());
    let mut hosted = false;
    for entry in entries { hosted |= crate::native_macros::find(&entry)?.is_none(); }
    if hosted && world_now().is_none() {
        let sess = sess.clone();
        { let _k = crate::zone_alloc::kept();
        let _ = crate::run_threads::spawn_sized("run-pm-world", 64 << 20, move || { let _z = crate::zone_alloc::compiler_thread();
            let _ = world_inner(&sess, true);
        });
        }
    }
    Ok(())
}



/// The confined macros' domain (sandbox.rs `Domain`). No fuel: the world's arena sets no budget,
/// and a macro that never returns is the hang watchdog's (`watch_begin`). The clean-up always:
/// the macro world's code never tiers up, so no O3 pass does it later.
const MACRO_DOMAIN: crate::sandbox::Domain = crate::sandbox::Domain { name: "proc-macro", allow: macro_allow, fuel: false, clean_up: true };

/// The macro domain's own entries (sandbox.rs `Allow`): what a confined macro reaches of the
/// host beyond memory, math and formatting.
fn macro_allow(name: &str) -> Option<Option<(&'static str, u64)>> {
    Some(match name {
        "relax_pm_parse" => Some(("sb_pm_parse", sb_pm_parse as *const () as u64)),
        "relax_pm_getenv" => Some(("sb_pm_getenv", sb_pm_getenv as *const () as u64)),
        // the process environment is not the macro's: `std::env::var` answers from the invoking
        // crate's cargo environment (relax_pm_getenv) or not at all
        "getenv" => Some(("sb_no_getenv", sb_no_getenv as *const () as u64)),
        _ => return None,
    })
}

/// `relax_pm_parse` for a confined macro: the source read from its arena, the encoded stream
/// handed to it there (sandbox::give).
extern "C" fn sb_pm_parse(src: u64, len: u64, out: u64, out_len: u64) -> i32 {
    let src = crate::sandbox::arena_ptr(src, len);
    let (out, out_len) = (crate::sandbox::arena_ptr(out, 8), crate::sandbox::arena_ptr(out_len, 8));
    if src == 0 || out == 0 || out_len == 0 {
        return 1;
    }
    let s = unsafe { std::slice::from_raw_parts(src as *const u8, len as usize) };
    let Ok(s) = std::str::from_utf8(s) else { return 1 };
    let Some(ts) = crate::builtins::lex_stream(s) else { return 1 };
    let b = crate::pm::encode(Span::DUMMY, &ts);
    let p = crate::sandbox::give(&b);
    if p == 0 {
        return 1;
    }
    unsafe {
        *(out as *mut u64) = p;
        *(out_len as *mut u64) = b.len() as u64;
    }
    0
}

/// `relax_pm_getenv` for a confined macro: the invoking crate's cargo environment only.
extern "C" fn sb_pm_getenv(key: u64, len: u64, out: u64, out_len: u64) -> i32 {
    let key = crate::sandbox::arena_ptr(key, len);
    let (out, out_len) = (crate::sandbox::arena_ptr(out, 8), crate::sandbox::arena_ptr(out_len, 8));
    if key == 0 || out == 0 || out_len == 0 {
        return 1;
    }
    let k = unsafe { std::slice::from_raw_parts(key as *const u8, len as usize) };
    let Ok(k) = std::str::from_utf8(k) else { return 1 };
    let Some(val) = crate::macro_reg::call_krate_env(k) else { return 1 };
    let p = crate::sandbox::give(val.as_bytes());
    if p == 0 {
        return 1;
    }
    unsafe {
        *(out as *mut u64) = p;
        *(out_len as *mut u64) = val.len() as u64;
    }
    0
}

extern "C" fn sb_no_getenv(_name: u64) -> u64 {
    0
}

fn build_world(sess: &std::sync::Arc<crate::Session>) -> Result<World, String> {
    let _mem = crate::memstat::tag(crate::memstat::Tag::PmHost);
    let reg = REG.with(|r| r.borrow().as_ref().map(|r| (r.units.clone(), r.std_dir.clone())));
    let (units, std_dir) = reg.ok_or("no crate graph registered")?;
    // Only third-party proc-macro roots and their dependencies need a JIT world.
    let mut need = vec![false; units.len()];
    let mut stack: Vec<usize> = (0..units.len()).filter(|&i| units[i].proc_macro && !crate::native_macros::required(&units[i].name)).collect();
    while let Some(i) = stack.pop() {
        if need[i] {
            continue;
        }
        need[i] = true;
        for (d, _) in &units[i].deps {
            stack.push(*d);
        }
    }
    // the sysroot's proc_macro unit (run.rs with_sysroot_proc_macro, for proc-macro2 and the
    // like) is the world's own proc_macro crate, in every world crate's deps already: a second
    // crate on that file would give its closures two versions of one file
    let pm_root = crate::vfs::normalize_path(&format!("{}/proc_macro/lib.rs", std_dir));
    for i in 0..units.len() {
        if units[i].name == "proc_macro" && crate::vfs::normalize_path(&units[i].src_path) == pm_root {
            need[i] = false;
        }
    }
    // Policy::Sandboxed: a root whose closure holds a third-party package runs confined, root
    // and closure; any other root runs as host code, its closure too (a crate both use has both)
    let mut confined = vec![false; units.len()];
    let mut host = vec![false; units.len()];
    for root in (0..units.len()).filter(|&i| need[i] && units[i].proc_macro) {
        let mut closure = vec![false; units.len()];
        let mut st = vec![root];
        while let Some(i) = st.pop() {
            if closure[i] || !need[i] {
                continue;
            }
            closure[i] = true;
            st.extend(units[i].deps.iter().map(|&(d, _)| d));
        }
        let confine = crate::sandbox::policy() == crate::sandbox::Policy::Sandboxed && (0..units.len()).any(|i| closure[i] && units[i].registry);
        for i in (0..units.len()).filter(|&i| closure[i]) {
            if confine { confined[i] = true } else { host[i] = true }
        }
    }
    let mut boot_root = vec![false; units.len()];
    for i in 0..units.len() {
        if need[i] {
            for &(d, _) in &units[i].deps {
                if units[d].proc_macro && !crate::native_macros::required(&units[d].name) && need[d] {
                    boot_root[d] = true;
                }
            }
        }
    }
    let mut uses_boot = vec![false; units.len()];
    if boot_root.iter().any(|&b| b) {
        let mut progress = true;
        while progress {
            progress = false;
            for i in 0..units.len() {
                if need[i] && !uses_boot[i] && units[i].deps.iter().any(|&(d, _)| boot_root[d] || uses_boot[d]) {
                    uses_boot[i] = true;
                    progress = true;
                }
            }
        }
        // stage worlds: each round takes the proc-macro roots whose dependency closure uses only
        // macros of earlier stages (or none); its crates that use those get them from the stages
        let n = units.len();
        let closure = |p: usize| {
            let mut c = vec![false; n];
            let mut st = vec![p];
            while let Some(i) = st.pop() {
                if c[i] || !need[i] { continue; }
                c[i] = true;
                st.extend(units[i].deps.iter().map(|&(d, _)| d));
            }
            c
        };
        let mut have = vec![false; n];
        loop {
            let mut stage_need = vec![false; n];
            let mut fresh = Vec::new();
            for p in (0..n).filter(|&p| boot_root[p] && !have[p]) {
                let c = closure(p);
                let ready = (0..n).filter(|&x| c[x]).all(|x| units[x].deps.iter().all(|&(d, _)| d == x || !boot_root[d] || have[d] || d == p));
                if ready {
                    for x in (0..n).filter(|&x| c[x]) { stage_need[x] = true; }
                    fresh.push(p);
                }
            }
            if fresh.is_empty() { break; }
            let mut stage_uses = vec![false; n];
            let mut progress = true;
            while progress {
                progress = false;
                for i in 0..n {
                    if stage_need[i] && !stage_uses[i] && units[i].deps.iter().any(|&(d, _)| (boot_root[d] && have[d]) || stage_uses[d]) {
                        stage_uses[i] = true;
                        progress = true;
                    }
                }
            }
            let bw = build_world_inner(sess, &units, &std_dir, &stage_need, &stage_uses, &confined, &host)?;
            BOOT_WORLDS.lock().unwrap_or_else(|e| e.into_inner()).push(std::sync::Arc::new(Ok(bw)));
            for p in fresh { have[p] = true; }
        }
    }
    build_world_inner(sess, &units, &std_dir, &need, &uses_boot, &confined, &host)
}

/// A world of the `need` crates. rustc builds each proc-macro crate on its own, so one that does
/// not load fails only its own macros and those of the crates built on it (relax issue 199): the
/// world is built again without them, and their macros report why.
fn build_world_inner(sess: &std::sync::Arc<crate::Session>, units: &[crate::macro_reg::UnitInfo], std_dir: &str, need: &[bool], uses_boot: &[bool], confined: &[bool], host_run: &[bool]) -> Result<World, String> {
    let mut need = need.to_vec();
    let mut failed: Vec<Option<String>> = vec![None; units.len()];
    loop {
        match build_world_once(sess, units, std_dir, &need, uses_boot, confined, host_run) {
            Ok(mut w) => {
                w.failed = failed;
                return Ok(w);
            }
            Err((Some(i), e)) => {
                need[i] = false;
                failed[i] = Some(e);
                let mut progress = true;
                while progress {
                    progress = false;
                    for j in 0..units.len() {
                        if let Some(&(d, _)) = units[j].deps.iter().find(|&&(d, _)| failed[d].is_some()).filter(|_| need[j]) {
                            need[j] = false;
                            failed[j] = failed[d].clone();
                            progress = true;
                        }
                    }
                }
            }
            Err((None, e)) => return Err(e),
        }
    }
}

/// One build of the world; a macro crate that does not load is named with the error.
fn build_world_once(sess: &std::sync::Arc<crate::Session>, units: &[crate::macro_reg::UnitInfo], std_dir: &str, need: &[bool], uses_boot: &[bool], confined: &[bool], host_run: &[bool]) -> Result<World, (Option<usize>, String)> {
    let t0 = std::time::Instant::now();
    let _mem = crate::memstat::tag(crate::memstat::Tag::PmHost);
    let mut prog = crate::program::Program::new(sess);
    // core/alloc/std: the program's own parses (program::SharedParse), not a second copy
    prog.std_from_shared = true;
    // macros run on this machine: its pointer width, whatever the program's target
    prog.ptr_bytes = 8;
    // macros run here: the machine's cfg, never RELAX_CHECK_TARGET's
    let host = |f: &[String]| crate::cfg::CfgSet::machine(sess, f);
    let core = prog.load_crate_deferred("core", &format!("{}/core/lib.rs", std_dir), 2021, host(&[]), Vec::new()).map_err(|e| (None, e))?;
    let mut std_deps = vec![(prog.syms.intern("core"), core)];
    let alloc_root = format!("{}/alloc/lib.rs", std_dir);
    if crate::vfs::exists(&alloc_root) {
        let a = prog.load_crate_deferred("alloc", &alloc_root, 2021, host(&[]), std_deps.clone()).map_err(|e| (None, e))?;
        std_deps.push((prog.syms.intern("alloc"), a));
    }
    let st = prog.load_crate_deferred("std", &format!("{}/std/lib.rs", std_dir), 2021, host(&[]), std_deps.clone()).map_err(|e| (None, e))?;
    std_deps.push((prog.syms.intern("std"), st));
    let t_std = t0.elapsed().as_secs_f64() * 1e3;
    let m_std = crate::memstat::live_of(crate::memstat::Tag::PmHost) >> 20;
    let pm = prog.load_crate_deferred("proc_macro", &format!("{}/proc_macro/lib.rs", std_dir), 2021, host(&[]), std_deps.clone()).map_err(|e| (None, e))?;
    let mut deps0 = std_deps.clone();
    deps0.push((prog.syms.intern("proc_macro"), pm));
    // dependencies first
    let mut crate_of = vec![u32::MAX; units.len()];
    let mut done = vec![false; units.len()];
    let mut progress = true;
    while progress {
        progress = false;
        for i in 0..units.len() {
            if !need[i] || done[i] || units[i].deps.iter().any(|(d, _)| need[*d] && !done[*d]) {
                continue;
            }
            let u = &units[i];
            let mut deps = deps0.clone();
            for (d, n) in &u.deps {
                if crate_of[*d] != u32::MAX {
                    deps.push((prog.syms.intern(n), crate_of[*d]));
                }
            }
            let mut cs = host(&u.features);
            // Proc macros run in the host Relax session and are never shipped to the web.
            cs.set.push("relax".to_string());
            for c in &u.cfgs {
                cs.set.push(c.replace('"', ""));
            }
            crate::crates::set_crate_env(&u.name, u.env.clone());
            struct AllowBoot(bool);
            impl Drop for AllowBoot {
                fn drop(&mut self) {
                    if self.0 {
                        set_in_world(true);
                    }
                }
            }
            let _allow = if uses_boot[i] {
                set_in_world(false);
                AllowBoot(true)
            } else {
                AllowBoot(false)
            };
            let k = prog.load_crate_deferred(&u.name, &u.src_path, u.edition, cs, deps).map_err(|e| (Some(i), format!("macro crate {}: {}", u.name, e)))?;
            drop(_allow);
            crate_of[i] = k;
            done[i] = true;
            progress = true;
        }
    }
    let t_crates = t0.elapsed().as_secs_f64() * 1e3;
    let m_crates = crate::memstat::live_of(crate::memstat::Tag::PmHost) >> 20;
    prog.prelude_crate = Some(core);
    prog.resolve_imports();
    let t_res = t0.elapsed().as_secs_f64() * 1e3;
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let t_col = t0.elapsed().as_secs_f64() * 1e3;
    let m_col = crate::memstat::live_of(crate::memstat::Tag::PmHost) >> 20;
    // the bridge fn
    let bridge_def = (0..prog.defs.len())
        .map(|i| DefId(i as u32))
        .find(|&d| {
            let def = prog.def(d);
            def.krate == pm && prog.syms.str(def.name) == "bridge" && def.parent != NO_DEF && prog.syms.str(prog.def(def.parent).name) == "__relax"
        })
        .ok_or((None, "std proc_macro has no __relax::bridge".to_string()))?;
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    // an auxiliary unit: never the runtime's in place of the program's (jit.rs adopt_runtime_unit)
    u.aux = true;
    // The whole macro world is freed after load. Its callers still need inline/closure
    // inputs throughout that job; per-install eviction does not reduce settled memory.
    u.keep_compile_state = true;
    // no O3 in the proc-macro world: tier-up state is process-wide (TIER_UNIT, the tier
    // queue, the sampler map slot ids to ONE unit), so this unit's hot slots were queued and
    // built against the program's unit -- other functions' code installed in its slots, or an
    // index past its table ("AppendVec index out of bounds" in O3 builds, an O3
    // thread waiting on the owner: `makepad check` hangs)
    u.tier1 = None;
    // its compiled code in the disk cache like the program's (keyed by the macro crates'
    // fingerprints): a warm start compiles none of it
    if let Some(mode) = crate::cache::mode_from_env(&u.prog.session.options) {
        if let Ok(c) = crate::cache::Cache::open(&u, mode) {
            u.cache = Some(Box::new(crate::cache::CacheCell::new(c)));
        }
    }
    let id = u.fn_id(crate::jit::FnKey::Inst(bridge_def, Vec::new()));
    let bridge = u.entry(id);
    // the confined macros: the unit a confinement domain whose host code is std's and the
    // host-run macros' crates, one arena (no fuel: the hang watchdog is the limit; its window
    // bounds the memory), and the bridge's twin as their entry
    let sandbox = if (0..units.len()).any(|i| need[i] && confined[i]) {
        let up: *mut crate::jit::Unit = &mut *u;
        let mut host_too: Vec<u32> = std_deps.iter().map(|&(_, k)| k).chain([pm]).collect();
        host_too.extend((0..units.len()).filter(|&i| need[i] && host_run[i] && crate_of[i] != u32::MAX).map(|i| crate_of[i]));
        unsafe { crate::sandbox::confine_unit(up, MACRO_DOMAIN, &host_too) }
            .and_then(|()| crate::sandbox::new_arena(up, u32::MAX, "the proc-macro world", u64::MAX, 0))
            .map(|a| {
                let t = unsafe { crate::sandbox::twin(up, id) };
                (a, unsafe { (*up).entry(t) })
            })
            .map_err(|e| format!("third-party proc macros run confined under `{} run`, and here they cannot: {}", crate::tool_name(), e))
    } else {
        Err("no confined proc macro in this world".to_string())
    };
    if crate::preload::trace() {
        eprintln!("boot-trace: macro world: std {:.1} ms, + macro crates {:.1}, + resolve {:.1}, + collect {:.1}, + unit and cache {:.1} ms", t_std, t_crates, t_res, t_col, t0.elapsed().as_secs_f64() * 1e3);
        eprintln!("boot-trace: macro world heap (RELAX_MEM_REPORT): std {} MB, + macro crates {}, + collect {}, + unit {} MB", m_std, m_crates, m_col, crate::memstat::live_of(crate::memstat::Tag::PmHost) >> 20);
    }
    let ns = t0.elapsed().as_nanos() as u64;
    STATS.with(|s| {
        let mut x = s.get();
        x.world_ns += ns;
        s.set(x);
    });
    let confined = (0..units.len()).map(|i| need[i] && confined[i]).collect();
    Ok(World { u: Box::into_raw(u), bridge, crate_of, entries: Default::default(), broken: Default::default(), confined, sandbox, failed: Vec::new() })
}

/// Address of macro fn `fname` at the root of unit `unit`'s crate in the world.
fn entry_of(w: &World, unit: usize, fname: &str) -> Result<u64, String> {
    if let Some((_, a)) = w.entries.lock().unwrap().iter().find(|(k, _)| k.0 == unit && k.1 == fname) {
        return Ok(*a);
    }
    let k = w.crate_of[unit];
    let a = crate::owner::call(crate::owner::Class::Program, || {
        let u = unsafe { &mut *w.u };
        let prog = &u.prog;
        let root = prog.mods[prog.crates[k as usize].root_mod as usize].def;
        let d = (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|&d| {
            let def = prog.def(d);
            def.krate == k && def.parent == root && def.kind == DefKind::Fn && prog.syms.str(def.name) == fname
        });
        d.map(|d| {
            let id = u.fn_id(crate::jit::FnKey::Inst(d, Vec::new()));
            // a confined macro: its twin, as the fn value the bridge's twin calls
            if w.confined[unit] { unsafe { crate::sandbox::fn_value(w.u, id) } } else { Ok(u.entry(id)) }
        })
    });
    let a = a.ok_or_else(|| format!("macro fn `{}` not found", fname))??;
    w.entries.lock().unwrap().push(((unit, fname.to_string()), a));
    Ok(a)
}

#[repr(C)]
struct BridgeIo {
    f: usize,
    kind: u64,
    inp: *const u8,
    inp_len: usize,
    attr: *const u8,
    attr_len: usize,
    out: *mut u8,
    out_len: usize,
}

/// Watchdog for one expansion (ms): an expansion running longer is a hang, reported at the call.
const CALL_LIMIT_MS: u64 = 10_000;

/// A thread's expansion in progress (wall ns since jit::wall_ns's start; 0: none), watched by
/// one monitor thread for all expanding threads (a watchdog thread per call cost ~100 us, the
/// whole of a small expansion, and its process-wide generation let concurrent calls cancel
/// each other's).
struct WatchSlot {
    tid: u64,
    start: std::sync::atomic::AtomicU64,
    /// its thread has ended: a run's end frees it (`reset_statics`)
    ended: std::sync::atomic::AtomicBool,
}

struct SlotEnds(&'static WatchSlot);

impl Drop for SlotEnds {
    fn drop(&mut self) {
        self.0.start.store(0, std::sync::atomic::Ordering::Release);
        self.0.ended.store(true, std::sync::atomic::Ordering::Release);
    }
}

static WATCH: relax_session::sync::Mutex<Vec<&'static WatchSlot>> = relax_session::sync::Mutex::new(Vec::new());

thread_local! {
    static MY_SLOT: Cell<Option<&'static WatchSlot>> = const { Cell::new(None) };
    static SLOT_ENDS: Cell<Option<SlotEnds>> = const { Cell::new(None) };
}

/// Hosted macro calls running now, on any thread (the settle rebuild's replace checks there are none).
pub fn calls_in_flight() -> usize {
    WATCH.lock().map_or(0, |v| v.iter().filter(|s| s.start.load(std::sync::atomic::Ordering::Acquire) != 0).count())
}

fn watch_begin() -> &'static WatchSlot {
    let slot = MY_SLOT.with(|m| match m.get() {
        Some(s) => s,
        None => {
            let s: &'static WatchSlot = Box::leak(Box::new(WatchSlot { tid: crate::os::thread::current(), start: std::sync::atomic::AtomicU64::new(0), ended: std::sync::atomic::AtomicBool::new(false) }));
            SLOT_ENDS.with(|e| e.set(Some(SlotEnds(s))));
            m.set(Some(s));
            let mut v = WATCH.lock().unwrap();
            if v.is_empty() {
                { let _k = crate::zone_alloc::kept();
                let _ = crate::run_threads::spawn("run-pm-watch", watch_loop);
                }
            }
            v.push(s);
            s
        }
    });
    slot.start.store(crate::jit::wall_ns().max(1), std::sync::atomic::Ordering::Release);
    slot
}

fn watch_loop() {
    let limit = CALL_LIMIT_MS * 1_000_000;
    while crate::run_threads::sleep(std::time::Duration::from_millis(20)) {
        let now = crate::jit::wall_ns();
        for s in WATCH.lock().unwrap().iter() {
            let t = s.start.load(std::sync::atomic::Ordering::Acquire);
            // kicked every 20 ms until it returns: the fault handler ignores a kick that lands
            // outside JIT code (in the compiler) and acts on the next
            if t != 0 && now.saturating_sub(t) > limit {
                crate::os::thread::kick(s.tid);
            }
        }
    }
}

/// One macro call: a derive / function-like macro (`attr` None) or an attribute macro
/// (`attr`: the attribute's arguments; `input`: the item it is on, without it).
fn call(sess: &std::sync::Arc<crate::Session>, entry: &Entry, attr: Option<&TokenStream>, input: &TokenStream, call_span: Span) -> Result<TokenStream, String> {
    let _profile = crate::load_profile::span(sess.runtime.profile, "macro-jit", &entry.fname);
    let t0 = std::time::Instant::now();
    let _mem = crate::memstat::tag(crate::memstat::Tag::PmHost);
    tl_set_once(2, crate::clock::ms_since_start());
    let wait = crate::load_profile::span(sess.runtime.profile, "wait-macro-world", &entry.fname);
    let wa = world_for(sess, entry.unit);
    drop(wait);
    let w = match wa.as_deref() {
        Some(Ok(w)) => w,
        Some(Err(e)) => return Err(format!("proc-macro host: {}", e)),
        None => return Err("a proc macro used while the macro world itself is being built".to_string()),
    };
    if let Some(Some(e)) = w.failed.get(entry.unit) {
        return Err(format!("proc-macro host: {}", e));
    }
    // Reload's lexer/parser now run off the unit, but the JIT macro world still uses
    // the runtime's process-wide UNIT and TLS machinery. Preserve the original reload
    // serialization for this step. Cached script expansions never enter the host.
    let call_krate = crate::macro_reg::current_call_krate();
    {
        let mut callers = HOSTED_CALLERS.lock().unwrap_or_else(|e| e.into_inner());
        if !callers.contains(&call_krate) {
            let _k = crate::zone_alloc::kept();
            callers.push(call_krate.clone());
        }
    }
    // While the program's crates load (no program unit yet: no program code runs) and the call is
    // into the final world, the expansion runs on the asking thread: the runtime's unit is that
    // world's for every such call at once, and its lazy compiles are owner requests as any thread's.
    // Expansions of a parse workers' pool then run side by side instead of one at a time on the
    // owner. Later ones (hot reload, a lazy body parsed while the program runs, a stage world's
    // macro while the world is built) keep the owner's serialization.
    let off_owner = crate::run::running_unit().is_null() && wa.as_ref().zip(world_now()).is_some_and(|(a, b)| std::sync::Arc::ptr_eq(a, &b));
    let wait = crate::load_profile::span(sess.runtime.profile, "wait-macro-unit", &entry.fname);
    let body = || -> Result<TokenStream, String> {
    drop(wait);
    let _krate = crate::macro_reg::with_call_krate(&call_krate);
    let _held = crate::load_profile::span(sess.runtime.profile, "macro-unit-held", &entry.fname);
    let confined = match w.confined.get(entry.unit) {
        Some(true) => Some(w.sandbox.as_ref().map_err(|e| e.clone())?),
        _ => None,
    };
    if let Some((_, e)) = w.broken.lock().unwrap().iter().find(|(k, _)| k.0 == entry.unit && k.1 == entry.fname) {
        STATS.with(|s| {
            let mut x = s.get();
            x.failed += 1;
            s.set(x);
        });
        return Err(e.clone());
    }
    let f = entry_of(w, entry.unit, &entry.fname)?;
    PH[0].fetch_add(t0.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    let te = std::time::Instant::now();
    let bytes = crate::pm::encode(call_span, input);
    let abytes = attr.map(|a| crate::pm::encode(call_span, a));
    let enc_ns = te.elapsed().as_nanos() as u64;
    let (kind, ap, al) = match &abytes {
        Some(b) => (1u64, b.as_ptr(), b.len()),
        None => (0u64, std::ptr::null(), 0),
    };
    let mut io = BridgeIo { f: f as usize, kind, inp: bytes.as_ptr(), inp_len: bytes.len(), attr: ap, attr_len: al, out: std::ptr::null_mut(), out_len: 0 };
    let tj = std::time::Instant::now();
    if trace() {
        eprintln!("pmhost: enter {:?} unit={} fn={} crate={} input={}{}", std::thread::current().id(), entry.unit, entry.fname, call_krate, bytes.len(), if confined.is_some() { " confined" } else { "" });
    }
    let slot = watch_begin();
    // a confined macro: the bridge's twin in the world's arena, with copies of the input there
    // (the window is all its code reaches); its output range is checked and copied out
    let mut confined_out: Option<Vec<u8>> = None;
    let r = match confined {
        None => unsafe { crate::jit::call_jit(w.u, w.bridge, &mut io as *mut BridgeIo as u64) },
        Some((arena, bridge_twin)) => match call_confined(w, arena, *bridge_twin, &io) {
            Ok(Ok(out)) => {
                confined_out = Some(out);
                Ok(0)
            }
            Ok(Err(p)) => Err(p),
            Err(e) => {
                slot.start.store(0, std::sync::atomic::Ordering::Release);
                return Err(e);
            }
        },
    };
    slot.start.store(0, std::sync::atomic::Ordering::Release);
    if trace() {
        eprintln!("pmhost: leave {:?} unit={} fn={} elapsed={:.1}ms", std::thread::current().id(), entry.unit, entry.fname, tj.elapsed().as_secs_f64() * 1e3);
    }
    PH[1].fetch_add(tj.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    match r {
        Ok(_) => {}
        Err(p) => {
            STATS.with(|s| {
                let mut x = s.get();
                x.failed += 1;
                x.call_ns += t0.elapsed().as_nanos() as u64;
                s.set(x);
            });
            let errs = crate::owner::call(crate::owner::Class::Program, || {
                let u = unsafe { &mut *w.u };
                let errs = std::mem::take(&mut u.errors);
                if trace() {
                    eprintln!("pmhost: report {}", u.report(&p));
                    for e in &errs {
                        eprintln!("pmhost:   {}", e);
                    }
                }
                errs
            });
            let first = errs.first().cloned().unwrap_or_default();
            let msg = format!("`{}` {}: {}", entry.fname, p.kind, if first.is_empty() { p.message.clone() } else { first });
            if p.message.starts_with("compile error in") {
                w.broken.lock().unwrap().push(((entry.unit, entry.fname.clone()), msg.clone()));
            }
            return Err(msg);
        }
    }
    let out: &[u8] = match &confined_out {
        Some(o) => o,
        None if io.out.is_null() => return Err("proc macro returned no output".to_string()),
        None => unsafe { std::slice::from_raw_parts(io.out, io.out_len) },
    };
    tl_set_once(3, crate::clock::ms_since_start());
    let td = std::time::Instant::now();
    let ts = crate::pm::decode_stream(out).ok_or("proc macro output is malformed")?;
    let dec_ns = td.elapsed().as_nanos() as u64;
    let tz = std::time::Instant::now();
    // Macro calls can run while lazy/spec compilers update this unit's counters.
    let (cns, cnt) = crate::owner::call(crate::owner::Class::Program, || {
        let st = unsafe { &(*w.u).stats };
        (st.typeck_ns + st.lower_ns + st.opt_ns + st.codegen_ns, st.compiled as u64)
    });
    STATS.with(|s| {
        let mut x = s.get();
        x.calls += 1;
        x.call_ns += t0.elapsed().as_nanos() as u64;
        x.encode_ns += enc_ns;
        x.out_bytes += out.len() as u64;
        x.decode_ns += dec_ns;
        x.compile_ns = cns;
        x.compiled = cnt;
        s.set(x);
    });
    PH[2].fetch_add(tz.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    Ok(ts)
    };
    if off_owner { body() } else { crate::owner::call(crate::owner::Class::Program, body) }
}


/// A confined macro's call: the bridge's twin runs in the world's arena on a copy of `io` and its
/// input there. Ok(Ok(the output bytes)), Ok(Err(the panic or fault that ended it: the arena
/// state before it is back)), or Err (the call could not start).
fn call_confined(w: &World, arena: &crate::sandbox::Arena, bridge_twin: u64, io: &BridgeIo) -> Result<Result<Vec<u8>, crate::jit::PanicInfo>, String> {
    use crate::sandbox as sb;
    let depth = sb::entry_depth();
    let saved = sb::save();
    sb::enter_arena(arena).map_err(|_| "the proc-macro world's arena cannot be entered on this thread".to_string())?;
    let put = |p: *const u8, len: usize| -> u64 {
        let a = sb::arena_alloc(len.max(1) as u64, 16);
        if a != 0 && len != 0 {
            unsafe { std::ptr::copy_nonoverlapping(p, a as *mut u8, len) };
        }
        a
    };
    let inp = put(io.inp, io.inp_len);
    let attr = if io.attr.is_null() { 0 } else { put(io.attr, io.attr_len) };
    let at = sb::arena_alloc(std::mem::size_of::<BridgeIo>() as u64, 8);
    if inp == 0 || at == 0 || (!io.attr.is_null() && attr == 0) {
        for p in [inp, attr, at] {
            sb::arena_free(p);
        }
        sb::leave();
        return Err("the macro's input does not fit the proc-macro world's arena".to_string());
    }
    unsafe {
        (at as *mut BridgeIo).write(BridgeIo { f: io.f, kind: io.kind, inp: inp as *const u8, inp_len: io.inp_len, attr: attr as *const u8, attr_len: io.attr_len, out: std::ptr::null_mut(), out_len: 0 })
    };
    let r = unsafe { crate::jit::call_jit(w.u, bridge_twin, at) };
    if let Err(p) = r {
        // (the arena state of the call is gone: its input copies stay in the heap)
        sb::entry_recover(depth);
        sb::restore(saved);
        return Ok(Err(p));
    }
    let (out, out_len) = unsafe { ((*(at as *const BridgeIo)).out as u64, (*(at as *const BridgeIo)).out_len as u64) };
    let bytes = unsafe { sb::arena_bytes(out, out_len) }.map(|b| b.to_vec());
    for p in [out, inp, attr, at] {
        sb::arena_free(p);
    }
    sb::leave();
    bytes.map(Ok).ok_or_else(|| "the proc macro's output lies outside its arena".to_string())
}

/// After the program's files are loaded (no more expansions): the world's newly compiled code
/// goes to the disk cache.
pub fn flush_cache() {
    let stages = BOOT_WORLDS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for w in stages.into_iter().map(Some).chain([world_now()]) {
        if let Some(Ok(w)) = w.as_deref() {
            crate::owner::call(crate::owner::Class::Program, || {
                let u = unsafe { &mut *w.u };
                u.flush_cache_records();
                if let Some(c) = u.cache.as_mut() {
                    let _ = c.flush();
                }
            });
        }
    }
}

/// The world's disk-cache counters (boot trace).
pub fn cache_line() -> String {
    match world_now().as_deref() {
        Some(Ok(w)) => {
            let u = unsafe { &*w.u };
            match u.cache.as_ref() {
                Some(c) => format!("world cache: {} hits, {} misses, {} recorded", c.stats.hits, c.stats.misses, c.stats.recorded),
                None => "world cache: off".to_string(),
            }
        }
        _ => "no world".to_string(),
    }
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&TL, [const { std::sync::atomic::AtomicU64::new(0) }; 6]);
    reset(&PH, [const { std::sync::atomic::AtomicU64::new(0) }; 3]);
    reset(&WORLD, relax_session::sync::Mutex::new(None));
    reset(&BOOT_WORLDS, relax_session::sync::Mutex::new(Vec::new()));
    WATCH.lock().unwrap_or_else(|e| e.into_inner()).retain(|s| {
        let ended = s.ended.load(std::sync::atomic::Ordering::Acquire);
        if ended {
            drop(Box::from_raw(*s as *const WatchSlot as *mut WatchSlot));
        }
        !ended
    });
}

/// Tests: a world is alive (a stand-in that is no world: a call through it fails).
#[cfg(test)]
pub(crate) fn stand_in_world() {
    *WORLD.lock().unwrap() = Some(std::sync::Arc::new(Err("stand-in".into())));
}

#[cfg(test)]
mod tests {
    /// Off the owner (no running program) a world build keeps its parse workers; the single-thread
    /// load it sets on the owner is the thread's own and goes back to what it was, an outer one
    /// included.
    #[test]
    fn a_single_world_load_is_the_owners_and_restores_the_thread() {
        assert!(!super::single_world_load(), "a test thread is not the owner");
        let single = || crate::preload::SINGLE.with(|s| s.get());
        {
            let _off = super::SingleLoad::when(false);
            assert!(!single(), "off: untouched");
            let _on = super::SingleLoad::when(true);
            assert!(single(), "on");
        }
        assert!(!single(), "restored");
        crate::preload::SINGLE.with(|s| s.set(true));
        drop(super::SingleLoad::when(true));
        assert!(single(), "an outer single load stays");
        crate::preload::SINGLE.with(|s| s.set(false));
    }

    /// The worlds stay while a world is being built (BUILDING held by its builder): the build's
    /// crates expand a stage's macros there. A free after the build takes them.
    #[test]
    fn a_world_build_in_flight_keeps_the_worlds() {
        isolated_test!(a_world_build_in_flight_keeps_the_worlds);
        let stand_in = || std::sync::Arc::new(Err::<super::World, String>("stand-in".into()));
        super::BOOT_WORLDS.lock().unwrap().push(stand_in());
        *super::WORLD.lock().unwrap() = Some(stand_in());
        {
            let _building = super::BUILDING.lock().unwrap();
            assert!(!std::thread::spawn(super::free_world).join().unwrap(), "nothing goes while a build is in flight");
            assert!(super::world_alive(), "the world and the stage world stay");
        }
        assert!(super::free_world(), "the world goes after the build");
        assert!(!super::world_alive(), "and the stage world with it");
    }
}
