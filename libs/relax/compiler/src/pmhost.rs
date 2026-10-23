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
//! Sandbox: compile-time code gets no capabilities. A macro crate (and its dependencies) that
//! is not trusted may not use `unsafe`, `extern` blocks, `asm!`, `static mut` or `transmute`,
//! nor name std's fs/net/process/env/thread/time modules (refused before anything compiles);
//! every call runs under the hang watchdog. Trusted: local path crates (the workspace's own,
//! makepad's) and std; registry/git crates are not. Read-only paths a manifest declares:
//! not needed by any macro yet.

use crate::macro_reg::{in_world, set_in_world, trace, Entry, REG, STATS};
use std::cell::Cell;
use crate::pm::{Span, TokenStream};
use crate::program::*;

crate::gen_local! {
    /// the last `relax_pm_parse` result of this thread (the macro copies it right away)
    static PARSE_OUT: std::cell::RefCell<Vec<u8>>;
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
    fn registered(&self, sess: &std::sync::Arc<crate::Session>) -> Result<(), String> {
        sess.runtime.validate_native_macros()?;
        crate::native_macros::prepare()
    }
    fn prewarm(&self, sess: &std::sync::Arc<crate::Session>) -> Result<(), String> {
        sess.runtime.validate_native_macros()?;
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
    /// unit -> why the sandbox refused it
    refused: Vec<Option<String>>,
}

impl Drop for World {
    fn drop(&mut self) {
        // the last holder is gone (free_world): park compile jobs off the unit, then drop it
        unsafe {
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
    let _ = TL[i].fetch_update(std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed, |x| Some((f64::from_bits(x) + v).to_bits()));
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
/// held while the world is being built: other threads wait for that build
static BUILDING: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());

fn world_now() -> Option<std::sync::Arc<Result<World, String>>> {
    WORLD.lock().unwrap_or_else(|e| e.into_inner()).clone()
}


/// The world, built by the first caller (again after free_world); None on the threads building it.
fn world(sess: &std::sync::Arc<crate::Session>) -> Option<std::sync::Arc<Result<World, String>>> {
    if in_world() {
        return None;
    }
    if let Some(w) = world_now() {
        return Some(w);
    }
    let t = std::time::Instant::now();
    let _b = match BUILDING.try_lock() {
        Ok(b) => b,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            // another thread builds it: from now on its std files must not wait for this one
            let _wait = crate::program::WorldWait::begin();
            BUILDING.lock().unwrap_or_else(|e| e.into_inner())
        }
    };
    if let Some(w) = world_now() {
        tl_add(4, 1.0);
        tl_add(5, t.elapsed().as_secs_f64() * 1e3);
        return Some(w);
    }
    tl_set_once(0, crate::clock::ms_since_start());
    // restored also when build_world panics (the thread may be a pool worker that goes on)
    struct InWorld;
    impl Drop for InWorld {
        fn drop(&mut self) { set_in_world(false); }
    }
    let w = {
        set_in_world(true);
        let _in_world = InWorld;
        std::sync::Arc::new(build_world(sess))
    };
    tl_set_once(1, crate::clock::ms_since_start());
    *WORLD.lock().unwrap_or_else(|e| e.into_inner()) = Some(w.clone());
    tl_add(4, 1.0);
    tl_add(5, t.elapsed().as_secs_f64() * 1e3);
    Some(w)
}

/// After the program's files are loaded: the macro world goes (its Program, type context and
/// JIT unit: ~120 MB for makepad's macro crates). Its compiled code is in the disk cache, so a
/// later need (hot reload, a run-time module) rebuilds it without compiling. Compile jobs are
/// parked while the unit goes (a speculative worker may still hold the world's unit).
/// free_world drops the macro world when no macro call holds it; true when it went.
/// Is the proc-macro world alive (an auxiliary unit: the settle swap is refused meanwhile)?
pub fn world_alive() -> bool {
    WORLD.lock().map(|g| g.is_some()).unwrap_or(true)
}

pub fn free_world() -> bool {
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
        let _ = std::thread::Builder::new().name("run-pm-world".to_string()).stack_size(64 << 20).spawn(move || { let _z = crate::zone_alloc::compiler_thread();
            let _ = world(&sess);
        });
        }
    }
    Ok(())
}



/// Trusted crates may use unsafe/FFI: std and local path crates (the workspace's own,
/// makepad's); crates from a registry or git checkout (third-party) are sandboxed.
fn trusted(path: &str, std_dir: &str) -> bool {
    if !std_dir.is_empty() && crate::vfs::relative_path(path, std_dir).is_some() {
        return true;
    }
    let path = crate::vfs::normalize_path(path);
    !(path.contains("/.cargo/registry/") || path.contains("/.cargo/git/"))
}

/// The sandbox's dialect-level refusal for an untrusted macro crate's files.
fn sandbox_check(root: &str, edition: u16) -> Result<(), String> {
    use crate::lexer::T;
    let mut stack = vec![root.to_string()];
    let mut seen = Vec::new();
    while let Some(path) = stack.pop() {
        if seen.contains(&path) {
            continue;
        }
        seen.push(path.clone());
        let src = crate::vfs::read(&path).map_err(|e| format!("{}: {}", path, e))?;
        let mut toks = Vec::new();
        crate::lexer::lex(&src, edition, &mut toks).map_err(|e| format!("{}: {}", path, e.msg))?;
        let text = |i: usize| -> &[u8] { &src[toks[i].lo as usize..toks[i].hi as usize] };
        let dir = crate::program::dir_of(&path).to_string();
        for i in 0..toks.len() {
            let bad = match toks[i].kind {
                T::KwUnsafe => Some("`unsafe`"),
                T::KwExtern if i + 1 < toks.len() && matches!(toks[i + 1].kind, T::Str | T::OpenBrace) => Some("`extern` block"),
                T::KwStatic if i + 1 < toks.len() && toks[i + 1].kind == T::KwMut => Some("`static mut`"),
                T::Ident if text(i) == b"asm" || text(i) == b"global_asm" => Some("`asm!`"),
                T::Ident if text(i) == b"transmute" => Some("`transmute`"),
                T::Ident if i >= 2 && toks[i - 1].kind == T::PathSep && text(i - 2) == b"std" && matches!(text(i), b"fs" | b"net" | b"process" | b"env" | b"thread" | b"time") => {
                    Some("a std capability (fs/net/process/env/thread/time)")
                }
                T::KwMod if i + 2 < toks.len() && toks[i + 2].kind == T::Semi => {
                    let name = String::from_utf8_lossy(text(i + 1)).into_owned();
                    for c in [format!("{}/{}.rs", dir, name), format!("{}/{}/mod.rs", dir, name)] {
                        if crate::vfs::exists(&c) {
                            stack.push(c);
                            break;
                        }
                    }
                    None
                }
                _ => None,
            };
            if let Some(what) = bad {
                let (l, c) = crate::lexer::line_col(&src, toks[i].lo);
                return Err(format!("{}:{}:{}: proc macro uses {} -- not allowed in compile-time code of an untrusted crate", path, l, c, what));
            }
        }
    }
    Ok(())
}

fn build_world(sess: &std::sync::Arc<crate::Session>) -> Result<World, String> {
    let t0 = std::time::Instant::now();
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
    // the sandbox's refusal, per crate: a refused crate (and crates using it) stays out of the
    // world; its macros report the reason at their call sites
    let mut refused: Vec<Option<String>> = vec![None; units.len()];
    for i in 0..units.len() {
        if need[i] && !trusted(&units[i].src_path, &std_dir) {
            if let Err(e) = sandbox_check(&units[i].src_path, units[i].edition) {
                refused[i] = Some(e);
            }
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..units.len() {
            if need[i] && refused[i].is_none() {
                if let Some(d) = units[i].deps.iter().find(|(d, _)| refused[*d].is_some()) {
                    refused[i] = Some(format!("depends on refused crate {}: {}", units[d.0].name, refused[d.0].clone().unwrap_or_default()));
                    changed = true;
                }
            }
        }
    }
    for i in 0..units.len() {
        if refused[i].is_some() {
            need[i] = false;
        }
    }
    let mut prog = crate::program::Program::new(sess);
    // core/alloc/std: the program's own parses (program::SharedParse), not a second copy
    prog.std_from_shared = true;
    // macros run on this machine: its pointer width, whatever the program's target
    prog.ptr_bytes = 8;
    // macros run here: the machine's cfg, never MAKEPAD_RUN_CHECK_TARGET's
    let host = |f: &[String]| crate::cfg::CfgSet::machine(sess, f);
    let core = prog.load_crate_deferred("core", &format!("{}/core/lib.rs", std_dir), 2021, host(&[]), Vec::new())?;
    let mut std_deps = vec![(prog.syms.intern("core"), core)];
    let alloc_root = format!("{}/alloc/lib.rs", std_dir);
    if crate::vfs::exists(&alloc_root) {
        let a = prog.load_crate_deferred("alloc", &alloc_root, 2021, host(&[]), std_deps.clone())?;
        std_deps.push((prog.syms.intern("alloc"), a));
    }
    let st = prog.load_crate_deferred("std", &format!("{}/std/lib.rs", std_dir), 2021, host(&[]), std_deps.clone())?;
    std_deps.push((prog.syms.intern("std"), st));
    let t_std = t0.elapsed().as_secs_f64() * 1e3;
    let m_std = crate::memstat::live_of(crate::memstat::Tag::PmHost) >> 20;
    let pm = prog.load_crate_deferred("proc_macro", &format!("{}/proc_macro/lib.rs", std_dir), 2021, host(&[]), std_deps.clone())?;
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
            cs.set.push("relax".to_string());
            let k = prog.load_crate_deferred(&u.name, &u.src_path, u.edition, cs, deps).map_err(|e| format!("macro crate {}: {}", u.name, e))?;
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
        .ok_or("std proc_macro has no __relax::bridge")?;
    let mut u = crate::jit::Unit::new(prog, tcx, core);
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
    if crate::preload::trace() {
        eprintln!("boot-trace: macro world: std {:.1} ms, + macro crates {:.1}, + resolve {:.1}, + collect {:.1}, + unit and cache {:.1} ms", t_std, t_crates, t_res, t_col, t0.elapsed().as_secs_f64() * 1e3);
        eprintln!("boot-trace: macro world heap (MAKEPAD_RUN_MEM_REPORT): std {} MB, + macro crates {}, + collect {}, + unit {} MB", m_std, m_crates, m_col, crate::memstat::live_of(crate::memstat::Tag::PmHost) >> 20);
    }
    let ns = t0.elapsed().as_nanos() as u64;
    STATS.with(|s| {
        let mut x = s.get();
        x.world_ns += ns;
        s.set(x);
    });
    Ok(World { u: Box::into_raw(u), bridge, crate_of, entries: Default::default(), broken: Default::default(), refused })
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
            u.entry(id)
        })
    });
    let a = a.ok_or_else(|| format!("macro fn `{}` not found", fname))?;
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
}

static WATCH: relax_session::sync::Mutex<Vec<&'static WatchSlot>> = relax_session::sync::Mutex::new(Vec::new());

thread_local! {
    static MY_SLOT: Cell<Option<&'static WatchSlot>> = const { Cell::new(None) };
}

fn watch_begin() -> &'static WatchSlot {
    let slot = MY_SLOT.with(|m| match m.get() {
        Some(s) => s,
        None => {
            let s: &'static WatchSlot = Box::leak(Box::new(WatchSlot { tid: crate::os::thread::current(), start: std::sync::atomic::AtomicU64::new(0) }));
            m.set(Some(s));
            let mut v = WATCH.lock().unwrap();
            if v.is_empty() {
                { let _k = crate::zone_alloc::kept();
                let _ = std::thread::Builder::new().name("run-pm-watch".to_string()).spawn(watch_loop);
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
    loop {
        std::thread::sleep(std::time::Duration::from_millis(20));
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
    let wa = world(sess);
    drop(wait);
    let w = match wa.as_deref() {
        Some(Ok(w)) => w,
        Some(Err(e)) => return Err(format!("proc-macro host: {}", e)),
        None => return Err("a proc macro used while the macro world itself is being built".to_string()),
    };
    // Reload's lexer/parser now run off the unit, but the JIT macro world still uses
    // the runtime's process-wide UNIT and TLS machinery. Preserve the original reload
    // serialization for this step. Cached script expansions never enter the host.
    let wait = crate::load_profile::span(sess.runtime.profile, "wait-macro-unit", &entry.fname);
    crate::owner::call(crate::owner::Class::Program, || -> Result<TokenStream, String> {
    drop(wait);
    let _held = crate::load_profile::span(sess.runtime.profile, "macro-unit-held", &entry.fname);
    if let Some(Some(e)) = w.refused.get(entry.unit) {
        return Err(format!("sandbox: {}", e));
    }
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
    let slot = watch_begin();
    let r = unsafe { crate::jit::call_jit(w.u, w.bridge, &mut io as *mut BridgeIo as u64) };
    slot.start.store(0, std::sync::atomic::Ordering::Release);
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
    if io.out.is_null() {
        return Err("proc macro returned no output".to_string());
    }
    tl_set_once(3, crate::clock::ms_since_start());
    let out = unsafe { std::slice::from_raw_parts(io.out, io.out_len) };
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
        x.out_bytes += io.out_len as u64;
        x.decode_ns += dec_ns;
        x.compile_ns = cns;
        x.compiled = cnt;
        s.set(x);
    });
    PH[2].fetch_add(tz.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    Ok(ts)
    })
}


/// After the program's files are loaded (no more expansions): the world's newly compiled code
/// goes to the disk cache.
pub fn flush_cache() {
    if let Some(Ok(w)) = world_now().as_deref() {
        crate::owner::call(crate::owner::Class::Program, || {
            let u = unsafe { &mut *w.u };
            u.flush_cache_records();
            if let Some(c) = u.cache.as_mut() {
                let _ = c.flush();
            }
        });
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
