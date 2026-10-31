//! Trace-driven wasm export (`MAKEPAD_RUN_WASM_EXPORT=<file.wasm>`): every function the run
//! compiles is captured as the RIR its code is made from; at the end of the run the captured
//! set and its static call/data-pointer closure become one module (relax_backend::wasm).
//! Every function ships whole; MAKEPAD_RUN_WASM_EXPORT_BLOCKS=1 opts into the smaller
//! trace-specific profile with untraced block/function stubs. The data the code names (literals, vtables, statics' initial
//! images, writable blocks) is placed in linear memory with pointer words translated: data
//! pointers to their wasm addresses, function pointers to table indices. Runtime entries become
//! imports `r.<name>` in native diagnostic captures (memcpy/memmove/memset/catch_panic
//! are built in), C symbols `c.<name>`, provided-crate entries `p.<key>`. Web exports
//! require module implementations and reject every import except `env.js_*`.
//!
//! First version for size work: run with a fresh cache (no code from disk) and
//! `MAKEPAD_RUN_O1=always MAKEPAD_RUN_O3=0` so every function passes the capture.
//! MAKEPAD_RUN_WASM_EXPORT_STATS=1 prints the size breakdown.

use crate::jit::Unit;
use crate::syms::SymKind;
use relax_backend::rir::{Func, Inst, Sym};
use relax_backend::wasm::{self, Builtin, HostBind, Link, Module};
use relax_session::sync::Mutex;
use std::collections::{BTreeMap, HashMap};

struct Captured {
    entry: u32,
    funcs: BTreeMap<u32, Func>,
}

// Slot and symbol ids belong to a unit. The proc-macro host compiles in the
// same process, with its own numbering; its code must never replace program RIR.
static CAPTURED: Mutex<Option<BTreeMap<usize, Captured>>> = Mutex::new(None);
pub fn new_unit() -> usize {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    if enabled() { NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) } else { 0 }
}
/// the first captured slot: the run's entry (the test fn, main)
static ENTRY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);
static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static CLOSURE_ORIGINS: Mutex<BTreeMap<u32, &'static str>> = Mutex::new(BTreeMap::new());
static CLOSURE_PARENTS: Mutex<BTreeMap<u32, u32>> = Mutex::new(BTreeMap::new());
// Diagnostics for the first retaining data edge and the vtables actually
// reached by the optimized closure. Never seed reachability from this ledger.
static CLOSURE_VIA: Mutex<BTreeMap<u32, u32>> = Mutex::new(BTreeMap::new());
static CLOSURE_VTABLES: Mutex<BTreeMap<u32, (Option<u32>, Vec<u32>, String)>> = Mutex::new(BTreeMap::new());
/// The run's CPU samples per slot (the tier-up sampler's self samples, by weight): which code the
/// program spends its time in.
static SAMPLES: Mutex<Option<HashMap<u32, u64>>> = Mutex::new(None);
pub(crate) fn note_samples(u: &Unit, it: impl Iterator<Item = (u32, u64)>) {
    let mut g = SAMPLES.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    for (mut id, w) in it {
        // a twin's time is its slot's (the export ships the slot)
        while let crate::jit::FnKey::Plain(p) | crate::jit::FnKey::Sandbox(p) = u.fns[id as usize].key {
            if p == id || p as usize >= u.fns.len() { break; }
            id = p;
        }
        *m.entry(id).or_insert(0) += w;
    }
}

/// MAKEPAD_RUN_WASM_EXPORT_HOT=<fraction> (default 0.99): an O3 candidate (o3_policy) ships an O3
/// body only if it is among the slots that took this fraction of the run's CPU samples, hottest
/// first (LLVM's profile summary: hot = the counts that make up 99% of the total). The rest of the
/// shipping code is O1: it runs too rarely for O3's size to pay (`Imdct::new`, run once per
/// decoder: 22 KB unrolled at O3, 1 KB at rustc's opt z). `1`: every candidate (the trace's
/// tier-up alone decides).
fn hot_fraction() -> f64 {
    static F: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var("MAKEPAD_RUN_WASM_EXPORT_HOT").ok().and_then(|v| v.parse().ok()).unwrap_or(0.99))
}

/// Slot `id` is in the sampled hot set (hot_fraction) of the run that is exporting.
pub(crate) fn sampled_hot(id: u32) -> bool {
    static HOT: std::sync::OnceLock<Option<std::collections::HashSet<u32>>> = std::sync::OnceLock::new();
    let hot = HOT.get_or_init(|| {
        let frac = hot_fraction();
        if frac >= 1.0 {
            return None;
        }
        let g = SAMPLES.lock().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<(u64, u32)> = g.as_ref().map(|m| m.iter().map(|(id, w)| (*w, *id)).collect()).unwrap_or_default();
        v.sort_unstable_by(|a, b| b.cmp(a));
        let total: u64 = v.iter().map(|x| x.0).sum();
        let mut set = std::collections::HashSet::new();
        let mut acc = 0u64;
        for (w, id) in v {
            if acc as f64 >= frac * total as f64 {
                break;
            }
            acc += w;
            set.insert(id);
        }
        if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
            eprintln!("wasm export: sampled hot set: {} slots take {:.1}% of {} CPU samples", set.len(), 100.0 * acc as f64 / total.max(1) as f64, total);
        }
        Some(set)
    });
    hot.as_ref().map_or(true, |h| h.contains(&id))
}

/// The shipping bodies the JIT's O3 built (export_rir), for the FNLIST's tier column.
static O3_BODIES: Mutex<Option<std::collections::HashSet<u32>>> = Mutex::new(None);
pub(crate) fn note_o3_body(id: u32) {
    O3_BODIES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(id);
}
static POSTOPT_EFFECTS: [std::sync::atomic::AtomicUsize; 5] = [const { std::sync::atomic::AtomicUsize::new(0) }; 5];

/// Process start (the loader's main): `MAKEPAD_RUN_WASM_EXPORT_BLOCKS=1` turns block coverage on
/// before the program loads (code cache off), so the export drops the blocks the run never entered.
pub fn startup() {
    if enabled() && std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_BLOCKS").is_some_and(|v| v != "0") {
        std::env::set_var("MAKEPAD_RUN_CACHE", "off");
        crate::coverage::enable();
    }
}

/// Block shaking: remove native coverage effects from the captured RIR and
/// optionally trap blocks whose source probes never ran. Required callees and
/// their predecessor paths retain work's keep-reachable behavior.
/// Returns (blocks trapped, coverage effects removed).
fn shake_blocks(f: &mut Func, hits: &HashMap<(u64, u64), bool>, trap: bool, keep: &dyn Fn(u32) -> bool, mut stub: Option<&mut u32>) -> (usize, usize) {
    use relax_backend::rir::Term;
    let cov = |i: &Inst| -> Option<bool> {
        match i {
            Inst::Coverage { word, mask, .. } => hits.get(&(*word, *mask)).copied(),
            _ => None,
        }
    };
    // A block is entered when any of its source probes ran in any lowering.
    let state: Vec<Option<bool>> = f
        .blocks
        .iter()
        .map(|b| {
            let hits: Vec<bool> = b.insts.iter().filter_map(|i| cov(i)).collect();
            (!hits.is_empty()).then(|| hits.iter().any(|h| *h))
        })
        .collect();
    let mut dead: Vec<bool> = state.iter().map(|s| trap && *s == Some(false)).collect();
    // rare-but-required: a never-entered block that calls a keep_reachable function (a GC, a
    // growth path) stays, and so does the never-entered code leading to it
    if dead.iter().any(|d| *d) {
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); f.blocks.len()];
        for (bi, b) in f.blocks.iter().enumerate() {
            for t in succs(&b.term) {
                if (t as usize) < preds.len() {
                    preds[t as usize].push(bi);
                }
            }
        }
        let mut work: Vec<usize> = (0..f.blocks.len()).filter(|&bi| dead[bi] && f.blocks[bi].insts.iter().any(|i| matches!(i, Inst::Call(relax_backend::rir::Callee::Fn(t) | relax_backend::rir::Callee::CFn(t, _), _, _) if keep(*t)))).collect();
        let mut seen = vec![false; f.blocks.len()];
        while let Some(bi) = work.pop() {
            if seen[bi] || state[bi] == Some(true) {
                continue;
            }
            seen[bi] = true;
            dead[bi] = false;
            work.extend(preds[bi].iter().copied());
        }
    }
    let (mut trapped, mut removed) = (0, 0);
    let fname = f.name.clone();
    for (bi, b) in f.blocks.iter_mut().enumerate() {
        if state[bi].is_none() {
            continue;
        }
        let n_cov = b.insts.iter().filter(|i| cov(i).is_some()).count();
        removed += n_cov;
        if dead[bi] {
            b.insts.clear();
            b.pos.clear();
            if let Some(n) = stub.as_deref_mut() {
                // the replay gate's build: the trap names the block (`env.u(id)` of an id past
                // every slot)
                if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_UNTRACED").is_some() {
                    eprintln!("  block stub {} = {} b{}", *n, fname, bi);
                }
                b.insts.push(Inst::Call(relax_backend::rir::Callee::Fn(*n), Vec::new(), Vec::new()));
                b.pos.push(0);
                *n += 1;
            }
            b.term = Term::Unreachable;
            trapped += 1;
            continue;
        }
        let positioned = b.pos.len() == b.insts.len();
        let keep: Vec<bool> = b.insts.iter().map(|i| cov(i).is_none()).collect();
        let mut k = 0;
        b.insts.retain(|_| {
            k += 1;
            keep[k - 1]
        });
        if positioned {
            let mut k = 0;
            b.pos.retain(|_| {
                k += 1;
                keep[k - 1]
            });
        }
    }
    if removed > 0 {
        relax_backend::passes::dce(f);
    }
    (trapped, removed)
}

fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// Blanks the unused top-level statements of the record's blocks in the data images: (blocks in
/// the record, blocks found, bytes of the found blocks, bytes blanked, items dropped). An item
/// with `used` false is dropped; `used` true or null stays.
fn splash_blank(path: &str, images: &mut [(u32, Vec<u8>)]) -> (usize, usize, usize, usize, usize) {
    let src = match std::fs::read(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("wasm export: splash record {}: {}", path, e);
            return (0, 0, 0, 0, 0);
        }
    };
    let rec = match crate::json::parse(&src) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("wasm export: splash record {}: {}", path, e);
            return (0, 0, 0, 0, 0);
        }
    };
    let mut by_hash: HashMap<u64, Vec<(usize, usize)>> = HashMap::new();
    let mut n_blocks = 0;
    let mut dropped = 0;
    for b in rec.get("blocks").map(|b| b.arr()).unwrap_or(&[]) {
        n_blocks += 1;
        let Ok(h) = u64::from_str_radix(b.get("hash").map(|h| h.str()).unwrap_or(""), 16) else { continue };
        let e = by_hash.entry(h).or_default();
        for it in b.get("items").map(|i| i.arr()).unwrap_or(&[]) {
            if matches!(it.get("used"), Some(crate::json::Json::Bool(false))) {
                let (s, t) = (it.get("start").map_or(0.0, |v| v.num()) as usize, it.get("end").map_or(0.0, |v| v.num()) as usize);
                if t > s {
                    e.push((s, t));
                    dropped += 1;
                }
            }
        }
    }
    let (mut found, mut total, mut blanked, mut whole) = (0, 0, 0, 0usize);
    let mut unmatched: Vec<(usize, String)> = Vec::new();
    let splash_files = splash_file_hashes();
    for (_, bytes) in images.iter_mut() {
        if bytes.len() < 16 {
            continue;
        }
        let h = fnv64(bytes);
        let Some(ranges) = by_hash.get(&h) else {
            // a `.splash` file's text (include_str! of a module registry) that no VM evaluated in
            // the run (the record lists every text any VM evaluated): blanked whole
            if splash_files.contains(&h) {
                whole += 1;
                total += bytes.len();
                for c in bytes.iter_mut() {
                    if *c != b'\n' {
                        *c = b' ';
                        blanked += 1;
                    }
                }
            } else if bytes.len() >= 4096 && std::str::from_utf8(bytes).is_ok() {
                unmatched.push((bytes.len(), String::from_utf8_lossy(&bytes[..60]).replace('\n', " ")));
            }
            continue;
        };
        found += 1;
        total += bytes.len();
        for &(s, t) in ranges {
            let n = bytes.len();
            for c in &mut bytes[s.min(n)..t.min(n)] {
                if *c != b'\n' {
                    *c = b' ';
                    blanked += 1;
                }
            }
        }
    }
    unmatched.sort_by(|a, b| b.0.cmp(&a.0));
    if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
        eprintln!("wasm export: splash: {} .splash files in the crates; {} embedded ones no VM evaluated, blanked whole", splash_files.len(), whole);
        eprintln!("wasm export: splash: {} text objects >= 4 KB not in the record, {} bytes", unmatched.len(), unmatched.iter().map(|u| u.0).sum::<usize>());
        for (n, t) in unmatched.iter().take(20) {
            eprintln!("  unrecorded text {:7} {}", n, t);
        }
    }
    (n_blocks, found, total, blanked, dropped)
}

/// FNV-1a 64 of every `.splash` file in the program's crate directories (what include_str! of a
/// `.splash` path embeds verbatim); `target` and hidden directories are skipped.
fn splash_file_hashes() -> std::collections::HashSet<u64> {
    let mut out = std::collections::HashSet::new();
    let mut dirs: Vec<std::path::PathBuf> = crate::run::crate_dirs().into_iter().map(|(_, d)| std::path::PathBuf::from(d)).collect();
    dirs.sort();
    dirs.dedup();
    let mut seen: std::collections::HashSet<std::path::PathBuf> = Default::default();
    while let Some(d) = dirs.pop() {
        if !seen.insert(d.clone()) {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            match e.file_type() {
                Ok(t) if t.is_dir() => {
                    if !name.starts_with('.') && name != "target" {
                        dirs.push(p);
                    }
                }
                Ok(t) if t.is_file() && name.ends_with(".splash") => {
                    if let Ok(b) = std::fs::read(&p) {
                        out.insert(fnv64(&b));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn succs(t: &relax_backend::rir::Term) -> Vec<u32> {
    use relax_backend::rir::Term;
    match t {
        Term::Jump(a) => vec![*a],
        Term::Branch(_, a, b) | Term::Invoke(a, b) => vec![*a, *b],
        Term::Switch(_, tab) => tab.targets.iter().copied().chain(std::iter::once(tab.default)).collect(),
        Term::Ret(_) | Term::Unreachable | Term::Resume => Vec::new(),
    }
}

pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_WASM_EXPORT").is_some())
}

/// (slot, export name: None = a test fn, exported as `t<k>` in registration order)
static ROOTS: Mutex<Vec<(u32, Option<String>)>> = Mutex::new(Vec::new());

/// A test fn the harness calls: a root of the export, exported as `t<k>` (the k-th test).
pub fn root(id: u32, _name: &str) {
    add_root(id, None);
}

/// A `#[no_mangle]` / `#[export_name]` fn the embedder calls (`--exports`: the page's `wasm_*`
/// entries): a root of the export, exported under its symbol name.
pub fn export(id: u32, symbol: &str) {
    add_root(id, Some(symbol.to_string()));
}

fn add_root(id: u32, name: Option<String>) {
    if enabled() {
        let mut r = ROOTS.lock().unwrap_or_else(|e| e.into_inner());
        if !r.iter().any(|x| x.0 == id) {
            r.push((id, name));
        }
    }
}

/// The import module of foreign fn `d` as rustc names it for a wasm target: its extern block's
/// `#[link(wasm_import_module = "..")]`, else "env" (rustc_codegen_llvm: `wasm_import_module`
/// defaults to `env`). None: not a foreign item of an extern block.
fn wasm_import_module(u: &Unit, d: crate::program::DefId) -> Option<String> {
    let def = u.prog.def(d);
    let f = &u.prog.files[def.file as usize];
    let ast = &f.ast;
    for it in &ast.items {
        let crate::ast::ItemKind::ForeignMod(r) = &it.kind else { continue };
        if !ast.list(ast.row(*r).items).contains(&def.item) {
            continue;
        }
        for a in ast.list(it.attrs) {
            let (lo, hi) = (a.toks.lo, a.toks.hi);
            if hi <= lo || f.tok_text(lo) != "link" {
                continue;
            }
            let mut t = lo + 1;
            while t + 2 < hi {
                if f.tok_text(t) == "wasm_import_module" && f.tok_text(t + 1) == "=" {
                    return Some(f.tok_text(t + 2).trim_matches('"').to_string());
                }
                t += 1;
            }
        }
        return Some("env".to_string());
    }
    None
}

/// Records the RIR slot `id`'s code is compiled from (the last compile of a slot wins).
static FINISHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// finish() has begun (the union's compiles still capture)
static FORCING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn capture(unit: usize, id: u32, f: &Func) {
    if !enabled() || FINISHED.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    let mut g = CAPTURED.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        // written when the process ends, however it ends (a test run, the program's exit)
        extern "C" fn at_exit() {
            let u = crate::jit::current_unit();
            if !u.is_null() {
                finish(unsafe { &*u });
                if FAILED.load(std::sync::atomic::Ordering::Acquire) {
                    // The CLI forwards fd 2 from its session log asynchronously.
                    // Drain that log before terminating, so the entry names are
                    // visible to the caller as well as retained in the transcript.
                    crate::tui::exit_code(1);
                    crate::tui::finish(false);
                    // We are already inside exit's callbacks. Calling exit
                    // recursively is undefined; terminate with the failing code.
                    extern "C" { fn _exit(status: i32) -> !; }
                    unsafe { _exit(1) };
                }
            }
        }
        extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        unsafe { atexit(at_exit) };
    }
    // process state: on the kept heap
    crate::zone_alloc::kept_with(|| {
        g.get_or_insert_with(BTreeMap::new).entry(unit)
            .or_insert_with(|| Captured { entry: id, funcs: BTreeMap::new() })
            .funcs.insert(id, f.clone());
    });
}

struct Linked {
    vals: HashMap<u32, u64>,
    host: HashMap<u32, HostBind>,
}

impl Link for Linked {
    fn sym(&self, id: u32) -> u64 {
        self.vals.get(&id).copied().unwrap_or(0)
    }
    fn host(&self, id: u32) -> HostBind {
        self.host.get(&id).cloned().unwrap_or(HostBind::Trap)
    }
}

/// Bytes, size and alignment of the memory a data-like symbol names (None: not data).
fn data_of(u: &Unit, id: u32) -> Option<(Vec<u8>, u64, u64)> {
    let init = u.syms.initial_data.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned();
    let (len, align) = match u.syms.kind(id)? {
        SymKind::Data(i) => {
            let (_, len, align, _) = u.syms.obj_info(i)?;
            (len, align)
        }
        SymKind::Rw(i) => {
            let b = u.syms.rw_blocks.lock().unwrap_or_else(|e| e.into_inner()).get(i as usize).copied()?;
            (b.0.max(1), b.1.max(1))
        }
        SymKind::Static(d) => {
            let start = u.syms.addr(Sym::new(id));
            let (_, &(d2, size)) = u.static_spans.range(..=start).next_back()?;
            if d2 != d {
                return None;
            }
            (size.max(1), 16)
        }
        _ => return None,
    };
    let bytes = match init {
        Some(b) => b,
        None => {
            let a = u.syms.addr(Sym::new(id));
            if a == 0 {
                vec![0; len as usize]
            } else {
                unsafe { std::slice::from_raw_parts(a as *const u8, len as usize).to_vec() }
            }
        }
    };
    Some((bytes, len, align))
}

/// Registered data describes its pointer fields. In particular, byte literals
/// (fonts, embedded files, strings) have none: their integer words can happen to
/// match native addresses and must never be relocated or seed reachability.
/// Storage without a described layout retains the conservative symbol scan.
fn data_refs(u: &Unit, id: u32, bytes: &[u8]) -> Vec<(usize, Sym)> {
    let pw = u.ptr_bytes() as usize;
    let offsets: Vec<usize> = crate::syms::pointer_offsets(u, u.syms.addr(Sym::new(id)), bytes)
        .map(|offsets| offsets.into_iter().map(|o| o as usize).collect())
        .unwrap_or_else(|| (0..bytes.len() / pw).map(|i| i * pw).collect());
    offsets.into_iter().filter_map(|k| {
        if k + pw > bytes.len() { return None; }
        let w = crate::syms::word_in(u, bytes, k);
        if (if pw == 4 { w & 0xffff_ffff } else { w }) <= 0x1_0000 { return None; }
        u.syms.symbol_at(w, u).map(|s| (k, s))
    }).collect()
}

/// Export-time constants include immutable statics' initial images, never the
/// trace host's current storage. Mutable statics, UnsafeCell and unknown layouts
/// are deliberately not constants, even in the single-thread profile.
pub(crate) fn ro_read(ctx: usize, s: Sym, n: u32) -> relax_backend::passes::RoRead {
    use relax_backend::passes::RoRead;
    let u = unsafe { &*(ctx as *const Unit) };
    let Some(SymKind::Static(d)) = u.syms.kind(s.id) else {
        return crate::syms::ro_read(ctx, s, n);
    };
    let def = u.prog.def(d);
    if !matches!(u.prog.files[def.file as usize].ast.item(def.item).kind,
        crate::ast::ItemKind::Static(false, _, e) if e.is_some()) { return RoRead::No; }
    let Some(ty) = u.tcx.const_ty(&u.prog, d) else { return RoRead::No };
    if !u.tcx.is_freeze(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), ty) { return RoRead::No; }
    let image = u.syms.initial_data.lock().unwrap_or_else(|e| e.into_inner()).get(&s.id).cloned();
    let Some(image) = image else { return RoRead::No };
    let Ok(off) = usize::try_from(s.off) else { return RoRead::No };
    if off > image.len() || n as usize > image.len() - off { return RoRead::No; }
    if n == 0 { return RoRead::Range((image.len() - off) as u64); }
    let Some(ptrs) = crate::syms::pointer_offsets(u, u.syms.addr(Sym::new(s.id)), &image) else { return RoRead::No };
    let pw = u.ptr_bytes() as usize;
    for p in ptrs {
        let p = p as usize;
        if p < off + n as usize && off < p + pw {
            return if p == off && n as usize == pw {
                u.syms.symbol_at(crate::syms::word_in(u, &image, p), u).map(|s| {
                    POSTOPT_EFFECTS[3].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    RoRead::Ptr(s)
                }).unwrap_or(RoRead::No)
            } else { RoRead::No };
        }
    }
    if !matches!(n, 1 | 2 | 4 | 8) { return RoRead::No; }
    let mut bytes = [0u8; 8];
    bytes[..n as usize].copy_from_slice(&image[off..off + n as usize]);
    POSTOPT_EFFECTS[3].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    RoRead::Bytes(u64::from_le_bytes(bytes))
}

/// Functions and source blocks executed by the training runs in the same build
/// configuration. A malformed or incomplete union must not produce a module.
fn read_union() -> Result<HashMap<String, std::collections::HashSet<(usize, usize)>>, String> {
    let mut hits: HashMap<String, std::collections::HashSet<(usize, usize)>> = HashMap::new();
    let Ok(file) = std::env::var("MAKEPAD_RUN_WASM_EXPORT_UNION") else { return Ok(hits) };
    for path in file.split(',').filter(|p| !p.is_empty()) {
        let bytes = std::fs::read(path).map_err(|e| format!("union {path}: {e}"))?;
        let j = crate::json::parse(&bytes).map_err(|e| format!("union {path}: {e}"))?;
        if j.get("format").map(|v| v.str()) != Some("makepad-web-trace/1") {
            return Err(format!("union {path}: expected makepad-web-trace/1"));
        }
        let functions = j.get("functions").ok_or_else(|| format!("union {path}: no functions"))?;
        for f in functions.arr() {
            let key = f.get("key").map(|v| v.str()).filter(|s| !s.is_empty()).ok_or_else(|| format!("union {path}: missing function key"))?;
            let entry = hits.entry(key.to_string()).or_default();
            for h in f.get("hit").ok_or_else(|| format!("union {path}: {key}: no block coverage"))?.arr() {
                let pair = h.arr();
                if pair.len() != 2 {
                    return Err(format!("union {path}: {key}: invalid block coordinates"));
                }
                entry.insert((pair[0].num() as usize, pair[1].num() as usize));
            }
        }
    }
    Ok(hits)
}

/// Compile every training function to a fixpoint: lowering one body can register
/// more instances. Slot numbers are local to each process; only keys cross runs.
fn force_union(up: *mut Unit, hits: &HashMap<String, std::collections::HashSet<(usize, usize)>>) -> Result<std::collections::BTreeSet<u32>, String> {
    let mut forced = std::collections::BTreeSet::new();
    if hits.is_empty() { return Ok(forced); }
    let mut tried: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut rounds = 0usize;
    loop {
        rounds += 1;
        let want: Vec<u32> = {
            let u = unsafe { &*up };
            let g = CAPTURED.lock().unwrap_or_else(|e| e.into_inner());
            let captured = g.as_ref().and_then(|g| g.get(&u.wasm_export_id)).ok_or("union: no captured unit")?;
            let funcs = &captured.funcs;
            (0..u.fns.len() as u32).filter(|t| !funcs.contains_key(t) && !tried.contains(t)
                && hits.contains_key(&crate::jit::slot_key(u, *t))).collect()
        };
        if want.is_empty() { break; }
        for t in want {
            tried.insert(t);
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { crate::jit::compile_fn(up, t) })) {
                Ok(Ok(_)) => {
                    forced.insert(t);
                }
                result => return Err(format!("union: failed to compile {}: {}", crate::jit::slot_key(unsafe { &*up }, t),
                    match result { Ok(Err(e)) => e, _ => "compiler panicked".into() })),
            }
        }
    }
    let u = unsafe { &*up };
    let g = CAPTURED.lock().unwrap_or_else(|e| e.into_inner());
    let captured = g.as_ref().and_then(|g| g.get(&u.wasm_export_id)).ok_or("union: no captured unit")?;
    let captured_keys: std::collections::HashSet<String> = captured.funcs.keys().map(|id| crate::jit::slot_key(u, *id)).collect();
    let mut missing: Vec<&str> = hits.keys().filter(|k| !captured_keys.contains(*k)).map(String::as_str).collect();
    missing.sort_unstable();
    if !missing.is_empty() {
        if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
            for key in &missing {
                let slots: Vec<_> = (0..u.fns.len() as u32).filter(|id| crate::jit::slot_key(u, *id) == *key)
                    .map(|id| (id, u.fns[id as usize].compiled, tried.contains(&id))).collect();
                eprintln!("wasm export: missing union key {key}: slots (id, compiled, tried) {slots:?}");
            }
        }
        // MAKEPAD_RUN_WASM_EXPORT_UNION_LENIENT=1: a size A/B that replays an older export's training
        // union with a compiler whose std renamed some functions (local/wasmcg/recap.sh); never a
        // shipping export (the replay gate needs every trained function)
        if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_UNION_LENIENT").is_some_and(|v| v != "0") {
            eprintln!("wasm export: union: {} training keys absent in this build (lenient A/B replay)", missing.len());
        } else {
            return Err(format!("union: {} training functions were not captured:\n{}", missing.len(), missing.join("\n")));
        }
    }
    // Ahead/inlined bodies can already have RIR without installed native code.
    for id in captured.funcs.keys() {
        if hits.contains_key(&crate::jit::slot_key(u, *id)) {
            forced.insert(*id);
        }
    }
    if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
        eprintln!("wasm export: union: {} keys, {} training slots retained, {} functions compiled in ({} rounds, 0 failed)", hits.len(), forced.len(), tried.len(), rounds);
    }
    Ok(forced)
}

/// Writes the module of the captured functions to the export path.
pub fn finish(u: &Unit) {
    let Some(path) = std::env::var("MAKEPAD_RUN_WASM_EXPORT").ok() else { return };
    if FORCING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let (union, forced) = match read_union().and_then(|hits| force_union(u as *const Unit as *mut Unit, &hits).map(|forced| (hits, forced))) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("wasm export: {e}; no module written");
            FAILED.store(true, std::sync::atomic::Ordering::Release);
            let _ = std::fs::remove_file(path.replace("%p", &std::process::id().to_string()));
            return;
        }
    };
    if FINISHED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    // `%p`: the process id (a run's child processes capture their own sets)
    let path = path.replace("%p", &std::process::id().to_string());
    let Some(captured) = CAPTURED.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|g| g.remove(&u.wasm_export_id)) else { return };
    ENTRY.store(captured.entry, std::sync::atomic::Ordering::Relaxed);
    let mut funcs = captured.funcs;
    // the trace: slots whose code was installed (called); a build made ahead of a call that
    // never came is not part of it
    let n_cap = funcs.len();
    let n_initializers = funcs.keys().filter(|id| (**id as usize) < u.fns.len()
        && !forced.contains(*id) && !crate::coverage::runtime_function(u, **id)).count();
    // New slots already have a nonzero lazy-stub address. Only `compiled`
    // distinguishes installed program code from an unused ahead compilation.
    // Const/static initializer slots run only in the compiler. Their declared
    // images are linked as data; their evaluator bodies are not runtime roots.
    // Use the coverage collector's same distinction, keeping every union root.
    funcs.retain(|id, _| (*id as usize) < u.fns.len() && (forced.contains(id)
        || u.fns[*id as usize].compiled && crate::coverage::runtime_function(u, *id)));
    if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
        eprintln!("wasm export: {} slots captured, {} installed runtime/union roots (of {} slots in the unit); {} compiler initializer roots excluded", n_cap, funcs.len(), u.fns.len(), n_initializers);
    }
    if let Ok(pat) = std::env::var("MAKEPAD_RUN_WASM_EXPORT_DUMP") {
        for (id, f) in &funcs {
            if f.name.contains(pat.as_str()) {
                eprintln!("slot {}: {}", id, f.dump());
            }
        }
    }
    // Union bodies are present before discovering retained roots and traversing
    // their calls/vtables. Snapshot coverage only after that closure is lowered.
    let (kept, kept_failed) = match keep_closure(&mut funcs, u, !block_profile()) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("wasm export: {error}; no module written");
            FAILED.store(true, std::sync::atomic::Ordering::Release);
            let _ = std::fs::remove_file(&path);
            return;
        }
    };
    if !block_profile() && module_inline_on() {
        let (sites, bodies) = module_inline(u, &mut funcs, &kept);
        if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
            eprintln!("wasm export: module inline: {sites} call sites inlined into {bodies} bodies");
        }
    }
    let hits = crate::coverage::export_hits(u, &union);
    // MAKEPAD_RUN_WASM_EXPORT_VARIANTS="a=opts;b=opts": one module per variant from this capture
    // (<path>.a.wasm, ...) for size work; else one module with MAKEPAD_RUN_WASM_OPT
    match std::env::var("MAKEPAD_RUN_WASM_EXPORT_VARIANTS") {
        Ok(v) => {
            for var in v.split(';').filter(|v| !v.is_empty()) {
                let (name, o) = var.split_once('=').unwrap_or((var, ""));
                let p = match path.strip_suffix(".wasm") {
                    Some(stem) => format!("{}.{}.wasm", stem, name),
                    None => format!("{}.{}", path, name),
                };
                eprintln!("wasm export variant {} [{}]:", name, o);
                emit(u, &funcs, &kept, kept_failed, &hits, &forced, o, &p);
            }
        }
        Err(_) => emit(u, &funcs, &kept, kept_failed, &hits, &forced, &std::env::var("MAKEPAD_RUN_WASM_OPT").unwrap_or_default(), &path),
    }
}

/// One module of the captured set with options `optv` (`-tee,remat,...`), written to `path`.
fn emit(u: &Unit, funcs: &BTreeMap<u32, Func>, kept: &std::collections::BTreeSet<u32>, kept_failed: usize, hits: &HashMap<(u64, u64), bool>, forced: &std::collections::BTreeSet<u32>, optv: &str, path: &str) {
    let options = u.prog.session.options.clone();
    // the web player's profile: panics abort, one thread (TLS keys and waits built in)
    // untraced calls are a bare trap (option untraced: env.u(id) first, for a message)
    let mut opts = wasm::Options {
        panic_abort: true,
        untraced_import: false,
        memory64: options.check_target.web() == Some(crate::options::PtrWidth::W64),
        heap_pages: 0,
        ..wasm::Options::default()
    };
    let mut single = true;
    // off by default: under brotli the merged module is LARGER (P(DOOM), block-shaken: raw -360 KB,
    // 23,345 -> 18,620 functions, br11 +5.0 KB): the compressor already shares near-identical
    // bodies, the callers' extra constants cost more. Kept as an option for raw size / compile time.
    let mut merge = false;
    let mut splash = true;
    let blocks = block_profile() && crate::coverage::on();
    // -tee,-remat,...: switch size techniques off (or on) to measure them
    {
        for t in optv.split(',').filter(|t| !t.is_empty()) {
            let (on, n) = match t.strip_prefix('-') {
                Some(n) => (false, n),
                None => (true, t),
            };
            match n {
                "tee" => opts.tee = on,
                "remat" => opts.remat = on,
                "sink" => opts.sink = on,
                "declared" => opts.narrow_declared = on,
                "rematslots" => opts.remat_slots = on,
                "datafirst" => opts.data_first = on,
                "sp0" => opts.sp0 = on,
                "abort" => opts.panic_abort = on,
                "byname" => opts.order_by_name = on,
                "bybytes" => opts.order_by_bytes = on,
                "fold" => opts.fold_identical = on,
                "st" => single = on,
                "names" => opts.names = on,
                "untraced" => opts.untraced_import = on,
                "exnref" => opts.exnref = on,
                "merge" => merge = on,
                "splash" => splash = on,
                _ => eprintln!("wasm export: unknown option {}", n),
            }
        }
    }
    let abort = opts.panic_abort;
    // block shaking (coverage on): blocks the run never entered trap; keep_reachable closures stay
    // whole (they are kept because the trace may miss them). The coverage stores go either way.
    let shaken_store: BTreeMap<u32, Func>;
    let funcs: &BTreeMap<u32, Func> = if crate::coverage::on() {
        let (mut nt, mut nr, mut nb) = (0usize, 0usize, 0usize);
        let mut w = funcs.clone();
        let keep = |t: u32| (t as usize) < u.fns.len() && crate::jit::keep_reachable(u, t);
        // block stub ids (the untraced option): past every slot id
        let mut stub_id = 0x4000_0000u32.max(u.fns.len() as u32);
        let untraced = opts.untraced_import;
        for (id, f) in w.iter_mut() {
            nb += f.blocks.len();
            let (t, r) = shake_blocks(f, hits, blocks && !kept.contains(id), &keep, if untraced { Some(&mut stub_id) } else { None });
            nt += t;
            nr += r;
        }
        if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
            eprintln!("wasm export: blocks: {} of {} blocks trapped (never entered), {} coverage stores removed", nt, nb, nr);
        }
        shaken_store = w;
        &shaken_store
    } else {
        funcs
    };
    let mut m = Module::new(opts);
    let mut roots: Vec<(u32, Option<String>)> = ROOTS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if roots.is_empty() {
        roots.push((ENTRY.load(std::sync::atomic::Ordering::Relaxed), None));
    }
    // Captured explicit exports remain callable without an embedder's --exports
    // registration (standalone fixtures and exported cold panic probes).
    if let Ok(exports) = crate::run::exported_fns(&u.prog, &[String::new()]) {
        let names: HashMap<_, _> = exports.into_iter().map(|(name, def)| (def, name)).collect();
        for id in funcs.keys() {
            if roots.iter().any(|r| r.0 == *id) { continue; }
            if let crate::jit::FnKey::Inst(def, args) = &u.fns[*id as usize].key {
                if args.is_empty() {
                    if let Some(name) = names.get(def) { roots.push((*id, Some(name.clone()))); }
                }
            }
        }
    }
    roots.retain(|r| funcs.contains_key(&r.0));
    // roots: the entry and every function whose address any traced code takes (directly or in
    // the data it names): the program enters those through pointers made at run time (callbacks
    // a provided crate holds, vtables), which the static walk from the entry does not see. A slot
    // that ran but is named only by direct calls its callers' later builds inlined is dropped.
    let mut seeds: Vec<u32> = roots.iter().map(|r| r.0).collect();
    if block_profile() || !module_inline_on() {
        seeds.extend(forced.iter().copied());
        if !block_profile() { seeds.extend(funcs.keys().copied()); }
    } else {
        // the module inliner's profile: what the module can still call -- the entry, the exports,
        // every address taken (tables, vtables, callbacks) and the keep_reachable closures; a
        // function every caller inlined is gone (its code ships in them: the manifest names it
        // through their inline instances)
        seeds.extend(funcs.keys().copied().filter(|id| (*id as usize) < u.fns.len() && crate::jit::keep_reachable(u, *id)));
    }
    address_taken(u, funcs.values(), &mut seeds, &mut std::collections::HashSet::new());
    seeds.sort_unstable();
    seeds.dedup();
    let reached = reach(u, funcs, &seeds);
    if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
        eprintln!("wasm export: {} roots (entry + address-taken), {} of {} captured reached", seeds.len(), reached.len(), funcs.len());
        // the untraced callees (slot, name), for the stub ids `env.u(id)` reports
        let mut un: std::collections::BTreeSet<u32> = Default::default();
        for id in &reached {
            for b in &funcs[id].blocks {
                for i in &b.insts {
                    if let Inst::Call(relax_backend::rir::Callee::Fn(t) | relax_backend::rir::Callee::CFn(t, _), _, _) = i {
                        if !funcs.contains_key(t) {
                            un.insert(*t);
                        }
                    }
                }
            }
        }
        if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_UNTRACED").is_some() {
            for t in &un {
                let n = if (*t as usize) < u.fns.len() { u.fns[*t as usize].name.to_string() } else { String::new() };
                eprintln!("  untraced slot {} {}", t, n);
            }
        }
    }
    // functions that differ only in constants become one body (relax_backend::merge)
    let merged_store: BTreeMap<u32, Func>;
    let funcs: &BTreeMap<u32, Func> = if merge {
        let mut w: BTreeMap<u32, Func> = reached.iter().map(|id| (*id, funcs[id].clone())).collect();
        let keep: std::collections::HashSet<u32> = seeds.iter().copied().collect();
        let mut next = (u.fns.len() as u32).max(w.keys().next_back().map_or(0, |k| k + 1));
        let ms = relax_backend::merge::merge_similar(&mut w, &keep, &mut next, 16);
        if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
            eprintln!("wasm export: merge: {} rounds, {} groups, {} functions merged ({} thunks), {} extra params; {} -> {} functions", ms.rounds, ms.groups, ms.merged, ms.thunks, ms.params, reached.len(), w.len());
        }
        merged_store = w;
        &merged_store
    } else {
        funcs
    };
    let reached: Vec<u32> = if merge { funcs.keys().copied().collect() } else { reached };
    for id in &reached {
        m.add_func(*id, &funcs[id]);
    }
    // the embedder's entries under their symbol names (`wasm_create_app`, ...); test fns (and a
    // run's entry) as t0, t1, ... in registration order
    let mut k = 0;
    for (id, name) in &roots {
        match name {
            Some(n) => m.export_func(*id, n),
            None => {
                m.export_func(*id, &format!("t{}", k));
                k += 1;
            }
        }
    }
    // symbols the code names
    let mut addr_syms: Vec<u32> = Vec::new();
    let mut tls_syms: Vec<u32> = Vec::new();
    let mut static_tls_syms = std::collections::BTreeSet::new();
    let mut host_syms: Vec<u32> = Vec::new();
    for f in reached.iter().map(|id| &funcs[id]) {
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Addr(_, s) => addr_syms.push(s.id),
                    Inst::TlsAddr(_, s) => tls_syms.push(s.id),
                    Inst::TlsStatic(_, s) => { static_tls_syms.insert(s.id); }
                    Inst::Call(relax_backend::rir::Callee::Host(s) | relax_backend::rir::Callee::CHost(s, _), _, _) => host_syms.push(s.id),
                    _ => {}
                }
            }
        }
    }
    addr_syms.sort_unstable();
    addr_syms.dedup();
    host_syms.sort_unstable();
    host_syms.dedup();
    let mut vals: HashMap<u32, u64> = HashMap::new();
    let mut host: HashMap<u32, HostBind> = HashMap::new();
    let web = u.web_target;
    for id in host_syms {
        let b = match u.syms.kind(id) {
            Some(SymKind::Rt(n)) => match n {
                "memcpy" => HostBind::Builtin(Builtin::Memcpy),
                "memmove" => HostBind::Builtin(Builtin::Memmove),
                "memset" => HostBind::Builtin(Builtin::Memset),
                "catch_panic" => HostBind::Builtin(Builtin::CatchPanic),
                "panicking" if abort => HostBind::Builtin(Builtin::Nop),
                "rethrow" if abort => HostBind::Trap,
                n => HostBind::Import("r".into(), n.to_string()),
            },
            Some(SymKind::Foreign(n)) => match web.then(|| web_binding(&n)).flatten().or_else(|| single.then(|| single_thread(&n)).flatten()) {
                Some(b) => b,
                // the web target: the module rustc imports it from (the page's `env.js_*`); a
                // native capture's C symbols are `c.<name>`
                None => {
                    let d = u.syms.foreign_defs.lock().unwrap_or_else(|e| e.into_inner()).get(&id).copied();
                    let module = if web { d.and_then(|d| wasm_import_module(u, d)) } else { None };
                    HostBind::Import(module.unwrap_or_else(|| "c".into()), n)
                }
            },
            Some(SymKind::Provided(k)) => HostBind::Import("p".into(), k),
            _ => HostBind::Trap,
        };
        host.insert(id, b);
    }
    // data: placed on first reference, pointer words translated (worklist)
    let tls = m.place(u.tls_next.max(1) as u64, 64);
    let mut placed: HashMap<u32, (u64, u64)> = HashMap::new();
    let mut pending: Vec<u32> = Vec::new();
    let mut images: Vec<(u32, Vec<u8>)> = Vec::new();
    let place = |m: &mut Module, id: u32, placed: &mut HashMap<u32, (u64, u64)>, pending: &mut Vec<u32>| -> Option<u64> {
        if let Some((a, _)) = placed.get(&id) {
            return Some(*a);
        }
        let (_, len, align) = data_of(u, id)?;
        let a = m.place(len, align.min(4096));
        placed.insert(id, (a, len));
        pending.push(id);
        Some(a)
    };
    // a thread-local symbol holds its offset in the thread's block: one block (one thread)
    tls_syms.sort_unstable();
    tls_syms.dedup();
    for &id in &tls_syms {
        let off = u.syms.value(Sym::new(id));
        vals.insert(id, tls + off);
    }
    // Native static TLS offsets are signed offsets from the host's thread pointer,
    // not offsets in the dynamic TLS block. Give each static its own zeroed image
    // for the web profile's one thread; its existing first-use code initializes it.
    for id in static_tls_syms {
        let Some(SymKind::ThreadLocal(d)) = u.syms.kind(id) else {
            eprintln!("wasm export: invalid static TLS symbol {}", id);
            return;
        };
        let Some(ty) = u.tcx.const_ty(&u.prog, d) else {
            eprintln!("wasm export: no layout for static TLS {}", u.prog.def_path(d));
            return;
        };
        let layout = u.layout_of(ty);
        let flag = u.tls_places.get(&d).is_some_and(|p| p.init.is_some());
        let addr = m.place(u64::from(layout.size.max(1)) + u64::from(flag), u64::from(layout.align.max(1)));
        vals.insert(id, addr);
    }
    let mut n_unlinked = 0usize;
    for &id in &addr_syms {
        let v = match u.syms.kind(id) {
            Some(SymKind::Data(_) | SymKind::Rw(_) | SymKind::Static(_)) => place(&mut m, id, &mut placed, &mut pending).unwrap_or(0),
            Some(SymKind::Thunk(slot)) => m.table_index(slot) as u64,
            Some(SymKind::Site(i)) => i as u64,
            Some(SymKind::Rt("tls_base")) => tls,
            Some(SymKind::Rt("web_memory_base")) => 0,
            _ => {
                n_unlinked += 1;
                if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
                    eprintln!("wasm export: unlinked symbol {}: {}", id, crate::syms::describe(u, Sym::new(id)));
                }
                u.syms.value(Sym::new(id))
            }
        };
        vals.insert(id, v);
    }
    let mut n_ptrs = 0usize;
    while let Some(id) = pending.pop() {
        let Some((mut bytes, _, _)) = data_of(u, id) else { continue };
        let pw = u.ptr_bytes() as usize;
        for (k, s) in data_refs(u, id, &bytes) {
            let nv = match u.syms.kind(s.id) {
                Some(SymKind::Data(_) | SymKind::Rw(_) | SymKind::Static(_)) => place(&mut m, s.id, &mut placed, &mut pending).map(|a| a.wrapping_add(s.off as u64)),
                Some(SymKind::Thunk(slot)) => Some(m.table_index(slot) as u64),
                _ => None,
            };
            if let Some(nv) = nv {
                unsafe { crate::syms::write_ptr(u, bytes[k..k + pw].as_mut_ptr() as u64, nv) };
                n_ptrs += 1;
            }
        }
        images.push((id, bytes));
    }
    // Splash shaking: `MAKEPAD_RUN_WASM_EXPORT_SPLASH=<splash use record>` (platform splash_trace.rs,
    // MAKEPAD_SPLASH_TRACE): in each embedded `script_mod!` text (found by its hash), the top-level
    // statements no run used are blanked to spaces (newlines kept): kept statements stay verbatim,
    // length and offsets unchanged, no code patched
    if let (true, Ok(rec)) = (splash, std::env::var("MAKEPAD_RUN_WASM_EXPORT_SPLASH")) {
        let st = splash_blank(&rec, &mut images);
        if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
            eprintln!("wasm export: splash: {} blocks in the record, {} found in the data; {} of {} bytes blanked ({} items dropped)", st.0, st.1, st.3, st.2, st.4);
        }
    }
    let n_data = images.len();
    // MAKEPAD_RUN_WASM_EXPORT_STATS: the data by what it is (literals and vtables, statics, blocks)
    let mut data_kinds: HashMap<String, (usize, usize)> = HashMap::new();
    let mut data_top: Vec<(usize, String)> = Vec::new();
    for (id, bytes) in &images {
        let (k, name) = match u.syms.kind(*id) {
            Some(SymKind::Data(i)) => ("data object".to_string(), u.syms.obj_name(i).unwrap_or_else(|| String::from_utf8_lossy(&bytes[..bytes.len().min(40)]).replace('\n', " "))),
            Some(SymKind::Static(d)) => ("static".to_string(), u.prog.def_path(d)),
            Some(SymKind::Rw(_)) => ("writable block".to_string(), String::new()),
            _ => ("other".to_string(), String::new()),
        };
        let nz = bytes.iter().filter(|b| **b != 0).count();
        let e = data_kinds.entry(k.clone()).or_insert((0, 0));
        e.0 += bytes.len();
        e.1 += nz;
        data_top.push((bytes.len(), format!("{} {}", k, name)));
    }
    data_top.sort_by(|a, b| b.0.cmp(&a.0));
    let image_ids: Vec<_> = images.iter().map(|(id, _)| *id).collect();
    for (id, bytes) in images {
        let a = placed[&id].0;
        m.data_at(a, bytes);
    }
    let link = Linked { vals, host };
    let nf = reached.len();
    let out = m.finish(&options, &link);
    if !block_profile() && (n_unlinked != 0 || !out.stats.failed.is_empty() || !out.stats.untraced_calls.is_empty()) {
        for (id, error) in &out.stats.failed {
            eprintln!("wasm export: shipping function {} failed: {error}", funcs[id].name);
        }
        for (id, target) in &out.stats.untraced_calls {
            eprintln!("wasm export: shipping call {} -> {} is unresolved", funcs[id].name, crate::syms::stable_fn_name(u, *target));
        }
        eprintln!("wasm export: incomplete shipping closure ({n_unlinked} unlinked symbols); no module written");
        FAILED.store(true, std::sync::atomic::Ordering::Release);
        let _ = std::fs::remove_file(path);
        return;
    }
    if web {
        let mut unsupported: Vec<String> = out.imports.iter()
            .filter(|(module, name)| module != "env" || !name.starts_with("js_"))
            .map(|(module, name)| format!("{module}.{name}")).collect();
        unsupported.sort();
        unsupported.dedup();
        if !unsupported.is_empty() {
            eprintln!("wasm export: unsupported web imports: {}; no module written", unsupported.join(", "));
            FAILED.store(true, std::sync::atomic::Ordering::Release);
            // A previous successful export at this path must not masquerade as
            // the result of this failed capture.
            if let Err(e) = std::fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("wasm export: could not remove stale module {path}: {e}");
                }
            }
            return;
        }
    }
    if let Err(e) = std::fs::write(path, &out.bytes) {
        eprintln!("wasm export: {}: {}", std::path::Path::new(&path).display(), e);
        return;
    }
    if let Ok(manifest) = std::env::var("MAKEPAD_RUN_WASM_EXPORT_FUNCTIONS") {
        // A shipping body includes its inline instances even when those instances
        // need no standalone wasm entry. Use provenance from these exact bodies,
        // not historical inline/dependency lists (which also include nounwind
        // queries), registered slots, or observed coverage.
        let mut included: std::collections::BTreeSet<u32> = reached.iter().copied().collect();
        if !block_profile() {
            for id in &reached { included.extend(funcs[id].inl_slots.iter().copied()); }
        }
        let keys = included.iter().map(|id| crate::jit::slot_key(u, *id)).collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>();
        if let Err(error) = std::fs::write(&manifest, format!("makepad-shipped-functions/1\n{}\n", keys.join("\n"))) {
            eprintln!("wasm export: function manifest {manifest}: {error}");
            FAILED.store(true, std::sync::atomic::Ordering::Release);
            let _ = std::fs::remove_file(path);
            return;
        }
    }
    let origins = CLOSURE_ORIGINS.lock().unwrap_or_else(|e| e.into_inner());
    let mut growth: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut closure_rows = String::from("edge\tgeneric\tslot\tbytes\tkey\n");
    for (id, _, bytes) in &out.stats.fn_sizes {
        if let Some(edge) = origins.get(id) {
            let generic = matches!(&u.fns[*id as usize].key, crate::jit::FnKey::Inst(_, args) if !args.is_empty());
            let entry = growth.entry(edge).or_default();
            entry.0 += 1; entry.1 += bytes;
            if generic { let entry = growth.entry("generic (overlaps edges)").or_default(); entry.0 += 1; entry.1 += bytes; }
            closure_rows.push_str(&format!("{edge}\t{generic}\t{id}\t{bytes}\t{}\n", crate::jit::slot_key(u, *id)));
        }
    }
    if let Ok(path) = std::env::var("MAKEPAD_RUN_WASM_EXPORT_FNLIST") {
        let _ = std::fs::write(format!("{}.closure", path.replace("%p", &std::process::id().to_string())), closure_rows);
    }
    for (edge, (count, bytes)) in growth {
        eprintln!("wasm export: closure growth {edge}: {count} functions, {bytes} code bytes");
    }
    drop(origins);
    // MAKEPAD_RUN_WASM_EXPORT_FNLIST=<file>: one line per module function for size analysis:
    // slot, wasm bytes (0: folded into an identical one), RIR insts, blocks, direct call sites to
    // it in the module, address taken, kept (keep_reachable, not traced), name, tier (o3 / o1),
    // the trace's CPU samples, size-first build shipped (1)
    if let Ok(fl) = std::env::var("MAKEPAD_RUN_WASM_EXPORT_FNLIST") {
        use relax_backend::rir::Callee;
        let mut calls_in: HashMap<u32, usize> = HashMap::new();
        for id in &reached {
            for b in &funcs[id].blocks {
                for i in &b.insts {
                    if let Inst::Call(Callee::Fn(t) | Callee::CFn(t, _), _, _) = i {
                        *calls_in.entry(*t).or_insert(0) += 1;
                    }
                }
            }
        }
        let taken: std::collections::HashSet<u32> = seeds.iter().copied().collect();
        let o3 = O3_BODIES.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
        let sf = SIZE_FIRST_SHIPPED.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
        let mut txt = String::new();
        for (id, n, b) in &out.stats.fn_sizes {
            let f = &funcs[id];
            let ni: usize = f.blocks.iter().map(|b| b.insts.len()).sum();
            txt.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", id, b, ni, f.blocks.len(), calls_in.get(id).copied().unwrap_or(0), taken.contains(id) as u8, kept.contains(id) as u8, n, if o3.contains(id) { "o3" } else { "o1" }, samples(*id), sf.contains(id) as u8));
        }
        let fl = fl.replace("%p", &std::process::id().to_string());
        let _ = std::fs::write(&fl, txt);
        let mut aliases = String::from("slot\toffset\tbytes\n");
        for (id, offset, len) in &out.stats.fn_alias_ranges {
            aliases.push_str(&format!("{id}\t{offset}\t{len}\n"));
        }
        let _ = std::fs::write(format!("{}.aliases", fl), aliases);
        // .ctxbytes: each body's code bytes by inline context of the module's final bodies:
        // slot, context, depth, the context's inlined slot, its depth-1 root's inlined slot (the
        // callee whose site brought it in; the body itself for context 0), bytes
        let mut ctxb = String::from("slot\tctx\tdepth\tinlined\troot\tbytes\n");
        for (id, rows) in &out.stats.fn_ctx_bytes {
            let f = &funcs[id];
            for (ctx, bytes) in rows {
                let (mut c, mut depth, mut root) = (*ctx as usize, 0usize, *id);
                let inlined = if c == 0 { *id } else { f.inl_slots.get(c - 1).copied().unwrap_or(u32::MAX) };
                while c != 0 && depth < 256 {
                    let Some(slot) = f.inl_slots.get(c - 1) else { root = u32::MAX; break };
                    root = *slot;
                    c = f.inl.get(c - 1).map_or(0, |(call, _)| relax_backend::rir::pos_ctx(*call) as usize);
                    depth += 1;
                }
                ctxb.push_str(&format!("{id}\t{ctx}\t{depth}\t{inlined}\t{root}\t{bytes}\n"));
            }
        }
        let _ = std::fs::write(format!("{}.ctxbytes", fl), ctxb);
        let parents = CLOSURE_PARENTS.lock().unwrap_or_else(|e| e.into_inner());
        let origins = CLOSURE_ORIGINS.lock().unwrap_or_else(|e| e.into_inner());
        let via = CLOSURE_VIA.lock().unwrap_or_else(|e| e.into_inner());
        let vtables = CLOSURE_VTABLES.lock().unwrap_or_else(|e| e.into_inner());
        let ranges: HashMap<_, _> = out.stats.fn_ranges.iter().map(|(id, offset, len)| (*id, (*offset, *len))).collect();
        let mut provenance = String::from("slot\tbytes\tparent\tcrate\tkey\n");
        let mut reachability = String::from("slot\toffset\tbytes\tedge\tgeneric\tparent\tvia\tcrate\tkey\n");
        for (id, _, bytes) in &out.stats.fn_sizes {
            let parent = parents.get(id).map(u32::to_string).unwrap_or_default();
            let mut owner = *id;
            let krate = loop {
                match &u.fns[owner as usize].key {
                    crate::jit::FnKey::Inst(d, _) => break u.prog.syms.str(u.prog.crates[u.prog.def(*d).krate as usize].name),
                    crate::jit::FnKey::Closure(_, _, p) | crate::jit::FnKey::Plain(p) | crate::jit::FnKey::Sandbox(p)
                        if *p != owner && (*p as usize) < u.fns.len() => owner = *p,
                    _ => break "<generated>",
                }
            };
            provenance.push_str(&format!("{id}\t{bytes}\t{parent}\t{krate}\t{}\n", crate::jit::slot_key(u, *id)));
            let edge = match origins.get(id).copied() {
                None => "captured/required roots",
                Some("direct-call") if parents.get(id).is_some_and(|p| origins.contains_key(p)) => "direct calls from closure functions",
                Some("direct-call") => "direct calls from roots",
                Some(_) => match via.get(id) {
                    None => "function addresses in code",
                    Some(symbol) => match vtables.get(symbol) {
                        Some((Some(_), _, _)) => "vtable methods (code materialized)",
                        Some((None, _, _)) => "vtable methods (data only)",
                        None => "function pointers in data",
                    },
                },
            };
            let generic = matches!(&u.fns[*id as usize].key, crate::jit::FnKey::Inst(_, args) if !args.is_empty());
            let data = via.get(id).map(u32::to_string).unwrap_or_default();
            let (offset, len) = ranges[id];
            reachability.push_str(&format!("{id}\t{offset}\t{len}\t{edge}\t{generic}\t{parent}\t{data}\t{krate}\t{}\n", crate::jit::slot_key(u, *id)));
        }
        let _ = std::fs::write(format!("{}.parents", fl), provenance);
        let _ = std::fs::write(format!("{}.reachability", fl), reachability);
        let mut table_rows = String::from("symbol\tevidence\towner\tmethods\tname\n");
        let mut method_rows = String::from("symbol\tslot\n");
        // Audit the tables actually placed in the module, independently of the
        // closure ledger. An unexpected table stays visible as unproven data.
        for symbol in &image_ids {
            let Some(SymKind::Data(index)) = u.syms.kind(*symbol) else { continue };
            let Some(name) = u.syms.obj_name(index).filter(|n| n.starts_with("vtable:")) else { continue };
            if let Some((owner, methods, _)) = vtables.get(symbol) {
                let evidence = if owner.is_some() { "code-address" } else { "initial-data-pointer" };
                table_rows.push_str(&format!("{symbol}\t{evidence}\t{}\t{}\t{name}\n", owner.map(|id| crate::jit::slot_key(u, id)).unwrap_or_default(), methods.len()));
                for method in methods { method_rows.push_str(&format!("{symbol}\t{method}\n")); }
            } else {
                let methods: Vec<_> = data_of(u, *symbol).map(|(bytes, _, _)| data_refs(u, *symbol, &bytes).iter().filter_map(|(_, s)| match u.syms.kind(s.id) { Some(SymKind::Thunk(slot)) => Some(slot), _ => None }).collect()).unwrap_or_default();
                table_rows.push_str(&format!("{symbol}\tunproven\t\t{}\t{name}\n", methods.len()));
                for method in methods { method_rows.push_str(&format!("{symbol}\t{method}\n")); }
            }
        }
        let _ = std::fs::write(format!("{}.vtables", fl), table_rows);
        let _ = std::fs::write(format!("{}.vtable-methods", fl), method_rows);
        // and the data objects: bytes, what it is, its name or first bytes
        let mut dt = String::new();
        for (b, n) in &data_top {
            dt.push_str(&format!("{}\t{}\n", b, n.chars().take(100).map(|c| if c.is_control() { ' ' } else { c }).collect::<String>()));
        }
        let _ = std::fs::write(format!("{}.data", fl), dt);
    }
    if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
        let s = &out.stats;
        for (id, target) in &s.untraced_calls {
            eprintln!("wasm export: untraced call {} ({}) -> {} ({})", id, funcs[id].name, target, crate::syms::stable_fn_name(u, *target));
        }
        eprintln!(
            "wasm export: {} bytes ({} functions captured, {} in module, {} types, {} imports): code {} data {} types {} imports {} funcs {} elems {} other {}; {} data objects, {} pointer words linked, {} symbols unlinked, {} untraced call sites, {} functions failed, {} folded into identical ones",
            s.total, nf, s.n_funcs, s.n_types, s.n_imports, s.code, s.data, s.types, s.imports, s.funcs, s.elems, s.other, n_data, n_ptrs, n_unlinked, s.untraced_sites, s.failed.len(), s.n_folded
        );
        let top: usize = std::env::var("MAKEPAD_RUN_WASM_EXPORT_TOP").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut sz = s.fn_sizes.clone();
        sz.sort_by(|a, b| b.2.cmp(&a.2));
        let kept_bytes: usize = s.fn_sizes.iter().filter(|x| kept.contains(&x.0)).map(|x| x.2).sum();
        eprintln!("wasm export: whole-function closure: {} retained functions, {} code bytes ({} failed to build)", kept.len(), kept_bytes, kept_failed);
        for (_, n, b) in sz.iter().take(top) {
            eprintln!("  {:7} {}", b, n);
        }
        // bytes per crate (the first path segment of the function's name)
        let mut by_crate: HashMap<String, (usize, usize)> = HashMap::new();
        for (_, n, b) in &s.fn_sizes {
            let k = n.trim_start_matches('<').split("::").next().unwrap_or("").split(' ').next().unwrap_or("").to_string();
            let e = by_crate.entry(k).or_insert((0, 0));
            e.0 += b;
            e.1 += 1;
        }
        let mut by_crate: Vec<_> = by_crate.into_iter().collect();
        by_crate.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
        for (k, (b, n)) in by_crate.iter() {
            eprintln!("  crate {:8} bytes {:5} fns  {}", b, n, k);
        }
        for (k, (b, nz)) in &data_kinds {
            eprintln!("  data {:9} bytes ({} nonzero)  {}", b, nz, k);
        }
        for (b, n) in data_top.iter().take(top) {
            eprintln!("  data {:9} {}", b, n.chars().take(140).collect::<String>());
        }
        let mut why: HashMap<String, usize> = HashMap::new();
        for (id, e) in &s.failed {
            eprintln!("wasm export: failed function {} ({}): {}", id, funcs[id].name, e);
            let k: String = e.chars().take(60).collect();
            *why.entry(k).or_insert(0) += 1;
        }
        let mut why: Vec<_> = why.into_iter().collect();
        why.sort_by(|a, b| b.1.cmp(&a.1));
        for (k, n) in why.iter().take(12) {
            eprintln!("  failed {:5} x {}", n, k);
        }
    }
}

/// Slots whose address the captured code takes: `FnAddr`, thunk addresses, and function pointers
/// in the data those functions name (vtables, statics), followed through data pointers.
fn address_taken<'a>(u: &Unit, funcs: impl IntoIterator<Item = &'a Func>, out: &mut Vec<u32>, data_seen: &mut std::collections::HashSet<u32>) {
    address_edges(u, funcs, &mut |slot, _| out.push(slot), data_seen);
}

/// The immediate data container of a function pointer, or None for an address
/// materialized directly in code. The traversal is the same closure walk used
/// by emission; provenance does not add edges or inspect execution traces.
fn address_edges<'a>(u: &Unit, funcs: impl IntoIterator<Item = &'a Func>, visit: &mut impl FnMut(u32, Option<u32>), data_seen: &mut std::collections::HashSet<u32>) {
    let mut dwork: Vec<u32> = Vec::new();
    for f in funcs {
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::FnAddr(_, t) => visit(*t, None),
                    Inst::Addr(_, s) | Inst::TlsAddr(_, s) => match u.syms.kind(s.id) {
                        Some(SymKind::Thunk(t)) => visit(t, None),
                        Some(SymKind::Data(_) | SymKind::Rw(_) | SymKind::Static(_)) => dwork.push(s.id),
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }
    while let Some(d) = dwork.pop() {
        if !data_seen.insert(d) {
            continue;
        }
        let Some((bytes, _, _)) = data_of(u, d) else { continue };
        for (_, s) in data_refs(u, d, &bytes) {
            match u.syms.kind(s.id) {
                Some(SymKind::Thunk(t)) => visit(t, Some(d)),
                Some(SymKind::Data(_) | SymKind::Rw(_) | SymKind::Static(_)) => dwork.push(s.id),
                _ => {}
            }
        }
    }
}

/// The captured functions the entry reaches: by calls, function addresses, and function pointers
/// stored in the data the reached code names (vtables).
fn reach(u: &Unit, funcs: &BTreeMap<u32, Func>, roots: &[u32]) -> Vec<u32> {
    use relax_backend::rir::Callee;
    let mut seen: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    let mut data_seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut work: Vec<u32> = roots.to_vec();
    let mut dwork: Vec<u32> = Vec::new();
    loop {
        while let Some(id) = work.pop() {
            if !funcs.contains_key(&id) || !seen.insert(id) {
                continue;
            }
            for b in &funcs[&id].blocks {
                for i in &b.insts {
                    match i {
                        Inst::Call(Callee::Fn(t) | Callee::CFn(t, _), _, _) | Inst::FnAddr(_, t) => work.push(*t),
                        Inst::Addr(_, s) | Inst::TlsAddr(_, s) => match u.syms.kind(s.id) {
                            Some(SymKind::Thunk(t)) => work.push(t),
                            Some(SymKind::Data(_) | SymKind::Rw(_) | SymKind::Static(_)) => dwork.push(s.id),
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
        }
        let Some(d) = dwork.pop() else { break };
        if !data_seen.insert(d) {
            continue;
        }
        let Some((bytes, _, _)) = data_of(u, d) else { continue };
        for (_, s) in data_refs(u, d, &bytes) {
            match u.syms.kind(s.id) {
                Some(SymKind::Thunk(t)) => work.push(t),
                Some(SymKind::Data(_) | SymKind::Rw(_) | SymKind::Static(_)) => dwork.push(s.id),
                _ => {}
            }
        }
    }
    seen.into_iter().collect()
}

/// Web host binding table. Only page services remain imports. Arithmetic/string
/// helpers and panics lower to core web code, System to the web dlmalloc port,
/// and SPEC-V dispatch is absent before capture. These four architecture calls
/// become instructions here. Atomic helpers select shared-memory instructions
/// from Options::shared_max_pages; today's page profile is single-threaded,
/// panic=abort (notify=0; wait traps on equality, returns 1 otherwise).
fn web_binding(name: &str) -> Option<HostBind> {
    Some(HostBind::Builtin(match name {
        "relax_web_memory_size" => Builtin::MemorySize,
        "relax_web_memory_grow" => Builtin::MemoryGrow,
        "relax_web_atomic_wait32" => Builtin::AtomicWait32,
        "relax_web_atomic_notify" => Builtin::AtomicNotify,
        _ => return None,
    }))
}

/// A single-threaded program's binding of the threading C symbols: no threads are started,
/// waits and wakes do nothing, thread-local keys are a table in linear memory.
fn single_thread(name: &str) -> Option<HostBind> {
    Some(match name {
        "pthread_create" | "pthread_join" | "pthread_detach" | "pthread_attr_init" | "pthread_attr_setstacksize" | "pthread_attr_destroy" | "pthread_setname_np" => HostBind::Trap,
        n if n.starts_with("__relax_domain_") => HostBind::Trap,
        // Linux std's only syscall is futex wait/wake (std sys/linux.rs): a no-op returning 0, as
        // macOS's __ulock_*
        "syscall" | "__relax_wait_enter" | "__relax_wait_leave" | "__relax_wake" | "__ulock_wake" | "__ulock_wait" | "pthread_threadid_np" | "sched_yield" | "__relax_key_delete" => HostBind::Builtin(Builtin::Nop),
        // the one thread: id 1 (0 is a lock's "no owner": a reentrant lock would think it holds
        // itself), the main thread (gettid == getpid)
        "pthread_self" | "pthread_main_np" | "gettid" | "getpid" => HostBind::Builtin(Builtin::One),
        "__relax_key_create" => HostBind::Builtin(Builtin::KeyCreate),
        "__relax_key_get" => HostBind::Builtin(Builtin::KeyGet),
        "__relax_key_set" => HostBind::Builtin(Builtin::KeySet),
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum O3Policy {
    Off,
    Hot,
    All,
}

/// MAKEPAD_RUN_WASM_EXPORT_FULL (default on): shipping bodies take O1's whole pipeline.
pub(crate) fn full_o1() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_WASM_EXPORT_FULL").map_or(true, |v| v != "0"))
}

/// A cold shipping O1 body's inline limits for `#[inline]` callees, small leaves and
/// closure-chain adapters (RIR instructions), size first as LLVM -Oz: no `#[inline]` hint counts
/// (minsize ignores it; rustc's MIR inliner is off at opt-level z), leaves only as small as their
/// call (8 instructions, 2 blocks), closure-chain adapters as the JIT.
pub(crate) const EXPORT_O1_INLINE: [usize; 4] = [0, 8, 2, 120];

/// `var`=hint,leaf,leaf_blocks,chain (A/B; MAKEPAD_RUN_WASM_EXPORT_O1_INLINE for cold shipping
/// bodies, MAKEPAD_RUN_O1_INLINE for every O1 build) overrides `default` (None: the JIT's O1 limits)
pub(crate) fn o1_inline_limits(lim: relax_codegen::opt::InlineLimits, var: &'static str, default: Option<[usize; 4]>) -> relax_codegen::opt::InlineLimits {
    static L: std::sync::OnceLock<std::sync::Mutex<HashMap<&'static str, Option<[usize; 4]>>>> = std::sync::OnceLock::new();
    let l = (*L.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).entry(var).or_insert_with(|| {
        let v = std::env::var(var).ok()?;
        let n: Vec<usize> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (n.len() == 4).then(|| [n[0], n[1], n[2], n[3]])
    })).or(default);
    match l {
        Some([hint, leaf, leaf_blocks, chain]) => relax_codegen::opt::InlineLimits { hint_insts: hint, leaf_insts: leaf, leaf_blocks, chain_insts: chain, ..lim },
        None => lim,
    }
}

/// MAKEPAD_RUN_WASM_EXPORT_SIZE_FIRST (default on; `0`: every shipping O1 body as the JIT's O1, A/B):
/// a shipping O1 body the trace never sampled ships its size-first build (EXPORT_O1_INLINE), every
/// other body its JIT-limit build. Every export O1 build makes both (jit.rs build_rir_body): the
/// JIT-limit one stays the slot's RIR, so O3 bodies, the inliners' inputs and the module's
/// references are the same as without size first, and the hot path's code with them.
pub(crate) fn size_first_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_WASM_EXPORT_SIZE_FIRST").map_or(true, |v| v != "0"))
}

/// The size-first builds of shipping O1 bodies (jit.rs build_rir_body), until export_rir takes them.
static SIZE_FIRST: Mutex<Option<HashMap<u32, Func>>> = Mutex::new(None);
/// The shipping bodies that took their size-first build, for the FNLIST.
static SIZE_FIRST_SHIPPED: Mutex<Option<std::collections::HashSet<u32>>> = Mutex::new(None);

pub(crate) fn stash_size_first(id: u32, f: Func) {
    SIZE_FIRST.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(id, f);
}

/// The run's CPU samples of slot `id` (note_samples; its twins' count as its own).
fn samples(id: u32) -> u64 {
    SAMPLES.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&id).copied()).unwrap_or(0)
}

/// The size-first build of shipping O1 body `id` when the trace never sampled it (cold: its
/// whole inlined body took no CPU sample of the run) and the slot never ran at O3; None: it
/// ships its JIT-limit RIR. Either way the stashed build is dropped.
pub(crate) fn take_size_first(id: u32, o3: bool) -> Option<Func> {
    let f = SIZE_FIRST.lock().unwrap_or_else(|e| e.into_inner()).as_mut()?.remove(&id)?;
    if o3 || samples(id) > 0 {
        return None;
    }
    SIZE_FIRST_SHIPPED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(id);
    Some(f)
}

/// MAKEPAD_RUN_WASM_EXPORT_O1_FOLDS (default on; `0` off): shipping O1 bodies also take the O3
/// folds that only remove code (value-range compares, small-set compares)
pub(crate) fn o1_folds() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_WASM_EXPORT_O1_FOLDS").map_or(true, |v| v != "0"))
}

/// MAKEPAD_RUN_WASM_EXPORT_O3_LTO (default on; `0` off): shipping O3 bodies inline as a fat-LTO
/// build does (InlineLimits::lto: cross-crate callees as local ones, single callers take their
/// callee's body: the Splash VM's opcode dispatch into run_core, as LLVM has it)
pub(crate) fn o3_lto() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_WASM_EXPORT_O3_LTO").map_or(true, |v| v != "0"))
}

/// MAKEPAD_RUN_WASM_EXPORT_O3_COLD (default `tiny`): a shipping O3 body's sites that are neither
/// hot nor in a loop inline only callees that do not grow it (InlineLimits::cold_tiny); `0`: O3's
/// own reach everywhere (A/B)
pub(crate) fn o3_cold_tiny() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_WASM_EXPORT_O3_COLD").map_or(true, |v| v != "0"))
}

/// MAKEPAD_RUN_WASM_EXPORT_INLINE (default on; `0` off): the shipping module's own inlining
/// (module_inline) and static reachability from the entry, exports and taken addresses. P(DOOM)
/// (WASMCG runs A/B, both gated): 10.29 MB br11 with it, 10.79 without, CPU equal.
fn module_inline_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_WASM_EXPORT_INLINE").map_or(true, |v| v != "0"))
}

/// The shipping module's inlining, as a size-first AOT compiler's: bottom-up over the direct-call
/// graph of the final bodies (a callee is finished before its callers take it), small callees --
/// no bigger than their call, small leaves, accessors and the iterator / Result helpers O1 leaves
/// as calls in cold code and in user crates -- inline at every site; each changed body is
/// simplified again. Returns (call sites inlined, bodies changed).
fn module_inline(u: &Unit, funcs: &mut BTreeMap<u32, Func>, kept: &std::collections::BTreeSet<u32>) -> (usize, usize) {
    use relax_backend::rir::Callee;
    let options = u.prog.session.options.clone();
    let up = crate::jit::current_unit();
    let ro = Some(relax_backend::passes::RoReader::new(ro_read, up as usize));
    // post-order over direct calls (iterative DFS; a back edge of a cycle is not followed)
    let ids: Vec<u32> = funcs.keys().copied().filter(|id| kept.contains(id)).collect();
    let mut state: HashMap<u32, u8> = HashMap::new();
    let mut order: Vec<u32> = Vec::with_capacity(ids.len());
    for &root in &ids {
        if state.contains_key(&root) {
            continue;
        }
        let mut stack: Vec<(u32, Vec<u32>)> = Vec::new();
        let callees = |id: u32| -> Vec<u32> {
            let mut v: Vec<u32> = funcs[&id].blocks.iter().flat_map(|b| &b.insts).filter_map(|i| match i {
                Inst::Call(Callee::Fn(c), _, _) if *c != id && funcs.contains_key(c) => Some(*c),
                _ => None,
            }).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        state.insert(root, 1);
        stack.push((root, callees(root)));
        while let Some((id, todo)) = stack.last_mut() {
            match todo.pop() {
                Some(c) => {
                    if !state.contains_key(&c) {
                        state.insert(c, 1);
                        let cs = callees(c);
                        stack.push((c, cs));
                    }
                }
                None => {
                    let id = *id;
                    state.insert(id, 2);
                    order.push(id);
                    stack.pop();
                }
            }
        }
    }
    // size-first (rustc -Oz's rule): a callee no bigger than its call (RULE_TINY), and a callee
    // with one call site in the module whose address nothing takes (its body moves, the
    // function goes): never a copy that grows the module
    let mut site_count: HashMap<u32, usize> = HashMap::new();
    for id in &ids {
        for i in funcs[id].blocks.iter().flat_map(|b| &b.insts) {
            if let Inst::Call(Callee::Fn(c) | Callee::CFn(c, _), _, _) = i {
                *site_count.entry(*c).or_insert(0) += 1;
            }
        }
    }
    let mut taken: Vec<u32> = Vec::new();
    address_taken(u, funcs.values(), &mut taken, &mut std::collections::HashSet::new());
    let taken: std::collections::HashSet<u32> = taken.into_iter().collect();
    // ... a loop-free one only: a browser engine tiers a function up when it is called again (no
    // on-stack replacement), so a callee's loop moved into a caller that runs once (a decode
    // driver) stays in baseline code (mp3 decode: first run 3.3x slower when merged)
    let loop_free = |f: &Func| relax_backend::regalloc::loop_depths(f, &relax_backend::regalloc::reachable(f)).iter().all(|d| *d == 0);
    let single: std::collections::HashSet<u32> = site_count.iter().filter(|(c, n)| **n == 1 && !taken.contains(c)
        && !((**c as usize) < u.fns.len() && crate::jit::keep_reachable(u, **c))
        && funcs.get(c).is_some_and(|f| loop_free(f) && relax_codegen::opt::inst_count(f) <= 400)).map(|(c, _)| *c).collect();
    let never: std::collections::HashSet<u32> = funcs.keys().copied().filter(|&c| (c as usize) < u.fns.len() && match &u.fns[c as usize].key {
        crate::jit::FnKey::Inst(d, _) => crate::jit::inline_never(&u.prog, *d),
        _ => false,
    }).collect();
    let lim = relax_codegen::opt::InlineLimits { budget: usize::MAX / 4, leaf_insts: 0, leaf_blocks: 0, hint_insts: 0, chain_insts: 0, always_insts: 4000, free_any: false, ordered: false, hot_insts: 0, growth_limit: None, cold_tiny: false, lto: false };
    let (mut sites, mut bodies) = (0usize, 0usize);
    for id in order {
        let Some(mut f) = funcs.remove(&id) else { continue };
        let calls_any = f.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::Call(Callee::Fn(_), _, _)));
        if calls_any {
            // `#[inline(never)]` stays a call, as rustc's and LLVM's inliners keep it
            let get = |c: u32| -> Option<(*const Func, u8)> {
                if never.contains(&c) {
                    return None;
                }
                funcs.get(&c).map(|g| (g as *const Func, if single.contains(&c) { relax_codegen::opt::ATTR_ALWAYS } else { 0u8 }))
            };
            let n = relax_codegen::opt::inline_calls(&options, &mut f, id, &get, &|_| true, &lim).len();
            if n > 0 {
                sites += n;
                bodies += 1;
                relax_backend::passes::simplify(&options, &mut f, ro, true, true);
                relax_backend::passes::live_dce(&mut f);
                relax_backend::passes::dce(&mut f);
                relax_backend::passes::relayout(&mut f);
            }
        }
        funcs.insert(id, f);
    }
    (sites, bodies)
}

/// The crate that defines slot `id` (a closure's or shim's owner's), "" for generated code.
pub(crate) fn slot_crate(u: &Unit, id: u32) -> &str {
    let mut owner = id;
    loop {
        match &u.fns[owner as usize].key {
            crate::jit::FnKey::Inst(d, _) => return u.prog.syms.str(u.prog.crates[u.prog.def(*d).krate as usize].name),
            crate::jit::FnKey::Closure(_, _, p) | crate::jit::FnKey::Plain(p) | crate::jit::FnKey::Sandbox(p)
                if *p != owner && (*p as usize) < u.fns.len() => owner = *p,
            _ => return "",
        }
    }
}

/// MAKEPAD_RUN_WASM_EXPORT_O3_CRATES=a,b: crates whose functions all take the O3 pipeline, as the
/// rustc web build compiles its hot crates at opt-level 3 (default: the Splash VM, makepad_script,
/// the old film exporter's HOT_CRATES).
/// Only the crate's own code, as rustc compiles it: a generic instance (or a closure of one) is
/// codegened in the crate that instantiates it, at that crate's opt-level (opt z for the derive
/// helpers the widget crates instantiate: `script_derive_proto_field::<T>`, 316 instances at O3
/// were 3.8 MB of code against rustc's 35 KB); trace-hot instances still take O3 (o3_policy).
pub(crate) fn o3_crate(u: &Unit, id: u32) -> bool {
    static C: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let crates = C.get_or_init(|| std::env::var("MAKEPAD_RUN_WASM_EXPORT_O3_CRATES").unwrap_or_else(|_| "makepad_script".to_string())
        .split(',').filter(|c| !c.is_empty()).map(|c| c.replace('-', "_")).collect());
    !crates.is_empty() && !generic_instance(u, id) && crates.iter().any(|c| c == slot_crate(u, id))
}

/// Slot `id` is an instance with generic arguments, or a closure / shim / twin of one.
fn generic_instance(u: &Unit, id: u32) -> bool {
    let mut owner = id;
    loop {
        match &u.fns[owner as usize].key {
            crate::jit::FnKey::Inst(_, args) => return !args.is_empty(),
            crate::jit::FnKey::Closure(_, _, p) | crate::jit::FnKey::Plain(p) | crate::jit::FnKey::Sandbox(p)
                if *p != owner && (*p as usize) < u.fns.len() => owner = *p,
            _ => return true,
        }
    }
}

/// MAKEPAD_RUN_WASM_EXPORT_O3=hot|all|0: shipping bodies built by the JIT's O3 pipeline.
pub(crate) fn o3_policy() -> O3Policy {
    static P: std::sync::OnceLock<O3Policy> = std::sync::OnceLock::new();
    *P.get_or_init(|| match std::env::var("MAKEPAD_RUN_WASM_EXPORT_O3").as_deref() {
        Ok("0") | Ok("off") => O3Policy::Off,
        Ok("all") => O3Policy::All,
        _ => O3Policy::Hot,
    })
}

/// One profile switch for capture, closure, emission and gate semantics.
fn block_profile() -> bool {
    std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_BLOCKS").is_some_and(|v| v != "0")
}

/// Apply only semantics shared by every requested emission variant. These are
/// the same abort/thread bindings used by emit(), not observations of a run.
/// In particular, unwind-only cleanup edges cannot execute in panic=abort.
pub(crate) fn prepare_shipping(u: &Unit, f: &mut Func) {
    use relax_backend::rir::{Callee, Term};
    static PROFILE: std::sync::OnceLock<(bool, bool)> = std::sync::OnceLock::new();
    let &(abort, single) = PROFILE.get_or_init(|| {
        let variants = std::env::var("MAKEPAD_RUN_WASM_EXPORT_VARIANTS").ok();
        let opts: Vec<String> = match variants {
            Some(v) => v.split(';').filter(|v| !v.is_empty()).map(|v| v.split_once('=').map_or("", |x| x.1).to_string()).collect(),
            None => vec![std::env::var("MAKEPAD_RUN_WASM_OPT").unwrap_or_default()],
        };
        opts.iter().map(|o| {
            let (mut abort, mut single) = (true, true);
            for t in o.split(',') {
                match t { "abort" => abort = true, "-abort" => abort = false, "st" => single = true, "-st" => single = false, _ => {} }
            }
            (abort, single)
        }).fold((true, true), |a, b| (a.0 && b.0, a.1 && b.1))
    });
    for b in &mut f.blocks {
        if abort {
            b.term = match b.term {
                Term::Invoke(next, _) => {
                    POSTOPT_EFFECTS[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Term::Jump(next)
                }
                Term::Resume => Term::Unreachable,
                ref t => t.clone(),
            };
        }
        let mut insts = Vec::with_capacity(b.insts.len());
        let old_pos = std::mem::take(&mut b.pos);
        for (at, i) in std::mem::take(&mut b.insts).into_iter().enumerate() {
            if matches!(i, Inst::Coverage { .. }) { continue; }
            let pos = old_pos.get(at).copied().unwrap_or(b.term_pos);
            let binding = match &i {
                Inst::Call(Callee::Host(s) | Callee::CHost(s, _), _, _) => match u.syms.kind(s.id) {
                    Some(SymKind::Foreign(n)) if single => single_thread(&n),
                    Some(SymKind::Rt("panicking")) if abort => Some(HostBind::Builtin(Builtin::Nop)),
                    Some(SymKind::Rt("rethrow")) if abort => Some(HostBind::Trap),
                    _ => None,
                },
                _ => None,
            };
            match (&i, binding) {
                (Inst::Call(_, _, rets), Some(HostBind::Builtin(builtin @ (Builtin::Nop | Builtin::One))))
                    if rets.iter().all(|r| f.vregs[r.0 as usize] == relax_backend::rir::Cls::I) => {
                    // The web binding returns a constant integer and has no
                    // effects. Argument computations still execute normally.
                    let one = builtin == Builtin::One;
                    POSTOPT_EFFECTS[1].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    insts.extend(rets.iter().map(|r| Inst::Iconst(*r, i64::from(one))));
                }
                (_, Some(HostBind::Trap)) => {
                    // A specified host trap cannot return or observe its
                    // arguments. This is a semantic trap, never a cold stub.
                    b.term = Term::Unreachable;
                    b.term_pos = pos;
                    POSTOPT_EFFECTS[2].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    break;
                }
                _ => insts.push(i),
            }
            b.pos.resize(insts.len(), pos);
        }
        b.insts = insts;
    }
}

/// A folded vtable load names a thunk symbol; represent its exact target as
/// FnAddr so the normal simplifier can devirtualize local calls. If that was the
/// last use of a vtable, DCE removes the table address and its unrelated methods
/// never enter the closure. Escaping tables keep all of their described edges.
pub(crate) fn shipping_fn_addresses(u: &Unit, f: &mut Func) -> bool {
    let mut changed = false;
    for i in f.blocks.iter_mut().flat_map(|b| &mut b.insts) {
        if let Inst::Addr(d, s) = i {
            if s.off != 0 { continue; }
            if let Some(SymKind::Thunk(slot)) = u.syms.kind(s.id) {
                *i = Inst::FnAddr(*d, slot);
                POSTOPT_EFFECTS[4].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                changed = true;
            }
        }
    }
    changed
}

thread_local! {
    /// keep_closure's first pass: Some(ids) while O3 builds wait (export_rir notes them here)
    static DEFERRED_O3: std::cell::RefCell<Option<Vec<u32>>> = const { std::cell::RefCell::new(None) };
}

/// Whether export_rir's O3 build of `id` waits for the whole closure (noting `id`).
pub(crate) fn defer_o3(id: u32) -> bool {
    DEFERRED_O3.with(|d| match d.borrow_mut().as_mut() {
        Some(v) => {
            v.push(id);
            true
        }
        None => false,
    })
}

/// Shipping closes all captured roots. Size mode closes only annotated roots.
/// Shipping upgrades every body to normal O1, then follows only edges in that
/// exact emitted body. Lowering registers monomorphic calls and vtables on demand;
/// registration alone does not retain them. The worklist reaches a fixpoint over
/// optimized calls and data pointers. A shipping build never
/// substitutes a trap for a body that fails to lower or emit.
fn keep_closure(funcs: &mut BTreeMap<u32, Func>, u: &Unit, whole: bool) -> Result<(std::collections::BTreeSet<u32>, usize), String> {
    use relax_backend::rir::Callee;
    let callees = |f: &Func, out: &mut Vec<u32>| {
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Call(Callee::Fn(t) | Callee::CFn(t, _), _, _) | Inst::FnAddr(_, t) = i {
                    out.push(*t);
                }
            }
        }
    };
    let mut work: Vec<u32> = Vec::new();
    if !whole {
        for f in funcs.values() {
            callees(f, &mut work);
        }
    }
    work.extend(funcs.keys().copied());
    work.retain(|id| (*id as usize) < u.fns.len() && (whole || crate::jit::keep_reachable(u, *id)));
    // The embedder's callback contract is independent of which callbacks the
    // training workload happened to invoke. Shipping compiles their complete
    // closure. Block mode still validates the names, but preserves its traced
    // bodies and annotated keep_reachable closure instead of making every web
    // callback (and effectively the application) a whole-function root.
    let mut required = std::collections::BTreeSet::new();
    if let Ok(names) = std::env::var("MAKEPAD_RUN_WASM_EXPORT_ROOTS") {
        let roots = ROOTS.lock().unwrap_or_else(|e| e.into_inner());
        for name in names.split(',').filter(|s| !s.is_empty()) {
            let id = roots.iter().find(|(_, n)| n.as_deref() == Some(name)).map(|(id, _)| *id)
                .ok_or_else(|| format!("required glue export {name} is not defined"))?;
            required.insert(id);
            if whole {
                work.push(id);
            } else if !funcs.contains_key(&id) {
                // Keep the exported body even when no training input entered it.
                // It goes through normal block shaking, not the whole-body set.
                let up = crate::jit::current_unit();
                let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||
                    crate::owner::call(crate::owner::Class::Program, || crate::zone_alloc::kept_with(|| unsafe { crate::jit::export_rir(up, id, false) }))
                )).unwrap_or_else(|_| Err("compiler panicked".into()))
                    .map_err(|error| format!("required glue export {name} cannot compile: {error}"))?;
                funcs.insert(id, body);
            }
        }
    }
    let mut kept = std::collections::BTreeSet::new();
    let mut visited = std::collections::BTreeSet::new();
    let mut data_seen = std::collections::HashSet::new();
    let mut failed = 0;
    let original: std::collections::BTreeSet<u32> = funcs.keys().copied().chain(required).collect();
    let mut origins = CLOSURE_ORIGINS.lock().unwrap_or_else(|e| e.into_inner());
    origins.clear();
    let mut parents = CLOSURE_PARENTS.lock().unwrap_or_else(|e| e.into_inner());
    parents.clear();
    let mut via = CLOSURE_VIA.lock().unwrap_or_else(|e| e.into_inner());
    via.clear();
    let mut removed_edges = Vec::new();
    let mut expand = |parent: u32, f: &Func, work: &mut Vec<u32>, data_seen: &mut std::collections::HashSet<u32>| {
        let start = work.len();
        for i in f.blocks.iter().flat_map(|b| &b.insts) {
            if let Inst::Call(Callee::Fn(t) | Callee::CFn(t, _), _, _) = i { work.push(*t); }
        }
        for id in &work[start..] { if !original.contains(id) { origins.entry(*id).or_insert("direct-call"); parents.entry(*id).or_insert(parent); } }
        address_edges(u, std::iter::once(f), &mut |id, data| {
            work.push(id);
            if !original.contains(&id) {
                if let std::collections::btree_map::Entry::Vacant(entry) = origins.entry(id) {
                    entry.insert("function-pointer/vtable");
                    parents.insert(id, parent);
                    if let Some(data) = data { via.insert(id, data); }
                }
            }
        }, data_seen);
    };
    let up = crate::jit::current_unit();
    // pass 1: O1 bodies for the whole closure (O3 candidates noted); pass 2: their O3 builds,
    // then whatever new callees those reach
    struct ClearDeferred;
    impl Drop for ClearDeferred {
        fn drop(&mut self) {
            DEFERRED_O3.with(|d| *d.borrow_mut() = None);
        }
    }
    let _clear = ClearDeferred;
    if whole {
        DEFERRED_O3.with(|d| *d.borrow_mut() = Some(Vec::new()));
    }
    let mut deferred_done = !whole;
    loop {
    while let Some(id) = work.pop() {
        if !visited.insert(id) {
            continue;
        }
        if (id as usize) >= u.fns.len() {
            if whole { return Err(format!("shipping closure names unknown function slot {id}")); }
            continue;
        }
        // A traced root still needs its unexecuted grow/error branches and their
        // full call closure. Previously only untraced roots were expanded.
        if !whole {
            if let Some(f) = funcs.get(&id) {
                expand(id, f, &mut work, &mut data_seen);
                kept.insert(id);
                continue;
            }
        }
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||
            crate::owner::call(crate::owner::Class::Program, || crate::zone_alloc::kept_with(|| unsafe { crate::jit::export_rir(up, id, whole) }))
        )).unwrap_or_else(|_| Err("compiler panicked".into()));
        match r {
            Ok(f) => {
                if let Some(before) = funcs.get(&id) {
                    let mut old = Vec::new();
                    let mut new = Vec::new();
                    callees(before, &mut old);
                    callees(&f, &mut new);
                    old.sort_unstable(); old.dedup();
                    let new: std::collections::HashSet<_> = new.into_iter().collect();
                    removed_edges.extend(old.into_iter().filter(|c| !new.contains(c)).map(|c| (id, c)));
                }
                expand(id, &f, &mut work, &mut data_seen);
                funcs.insert(id, f);
                kept.insert(id);
            }
            Err(error) => {
                if whole { return Err(format!("shipping closure cannot compile {} at export tier: {error}", crate::jit::slot_key(u, id))); }
                failed += 1;
            }
        }
    }
    if deferred_done {
        break;
    }
    deferred_done = true;
    let ids = DEFERRED_O3.with(|d| d.borrow_mut().take()).unwrap_or_default();
    for id in ids {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||
            crate::owner::call(crate::owner::Class::Program, || crate::zone_alloc::kept_with(|| unsafe { crate::jit::export_rir(up, id, true) }))
        )).unwrap_or_else(|_| Err("compiler panicked".into()));
        match r {
            Ok(f) => {
                expand(id, &f, &mut work, &mut data_seen);
                funcs.insert(id, f);
            }
            Err(error) => return Err(format!("shipping closure cannot compile {} at export O3: {error}", crate::jit::slot_key(u, id))),
        }
    }
    }
    let mut vtables = CLOSURE_VTABLES.lock().unwrap_or_else(|e| e.into_inner());
    vtables.clear();
    if let Ok(path) = std::env::var("MAKEPAD_RUN_WASM_EXPORT_FNLIST") {
        // Printed closure-type arguments can coincide across owner instances.
        // Join evidence to module ranges by slot, never by the display key.
        let mut inline_rows = String::from("emitted_slot\temitted\tcontext\tparent_context\tinlined_slot\tinlined\n");
        let mut inline_keys = HashMap::new();
        for (id, f) in funcs.iter() {
            let emitted = crate::jit::slot_key(u, *id);
            for (index, (slot, (call, _))) in f.inl_slots.iter().zip(&f.inl).enumerate() {
                let key = inline_keys.entry(*slot).or_insert_with(|| crate::jit::slot_key(u, *slot));
                inline_rows.push_str(&format!("{id}\t{emitted}\t{}\t{}\t{slot}\t{key}\n", index + 1,
                    relax_backend::rir::pos_ctx(*call)));
            }
        }
        let _ = std::fs::write(format!("{}.inlines", path.replace("%p", &std::process::id().to_string())), inline_rows);
        // A later retained constructor can materialize a table first discovered
        // through data. Classify after the entire optimized-body fixpoint.
        let mut code_data = BTreeMap::new();
        for (id, f) in funcs.iter() {
            for i in f.blocks.iter().flat_map(|b| &b.insts) {
                if let Inst::Addr(_, s) | Inst::TlsAddr(_, s) = i { code_data.entry(s.id).or_insert(*id); }
            }
        }
        for symbol in &data_seen {
            let Some(SymKind::Data(index)) = u.syms.kind(*symbol) else { continue };
            let Some(name) = u.syms.obj_name(index).filter(|n| n.starts_with("vtable:")) else { continue };
            let methods = data_of(u, *symbol).map(|(bytes, _, _)| data_refs(u, *symbol, &bytes).iter()
                .filter_map(|(_, s)| match u.syms.kind(s.id) { Some(SymKind::Thunk(slot)) => Some(slot), _ => None }).collect()).unwrap_or_default();
            vtables.insert(*symbol, (code_data.get(symbol).copied(), methods, name));
        }
        let mut rows = String::from("caller\tcallee\n");
        for (caller, callee) in &removed_edges {
            rows.push_str(&format!("{}\t{}\n", crate::jit::slot_key(u, *caller), crate::jit::slot_key(u, *callee)));
        }
        let _ = std::fs::write(format!("{}.removed-edges", path.replace("%p", &std::process::id().to_string())), rows);
    }
    if whole && std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
        eprintln!("wasm export: postopt: {} captured call/address edges eliminated before closure; {} optimized bodies", removed_edges.len(), kept.len());
        let n = POSTOPT_EFFECTS.each_ref().map(|x| x.load(std::sync::atomic::Ordering::Relaxed));
        eprintln!("wasm export: postopt semantics: {} abort unwind edges, {} constant host bindings, {} semantic trap tails, {} immutable static reads resolved", n[0], n[1], n[2], n[3]);
        eprintln!("wasm export: postopt: {} symbolic function addresses exposed to devirtualization", n[4]);
        let shipped = SIZE_FIRST_SHIPPED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, |s| s.len());
        let sampled = SAMPLES.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, |m| m.len());
        eprintln!("wasm export: size first: {} cold O1 bodies shipped their size-first build; {} slots sampled", shipped, sampled);
    }
    Ok((kept, failed))
}
