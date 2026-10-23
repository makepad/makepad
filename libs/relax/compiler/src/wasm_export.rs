//! Trace-driven wasm export (`MAKEPAD_RUN_WASM_EXPORT=<file.wasm>`): every function the run
//! compiles is captured as the RIR its code is made from; at the end of the run the captured
//! set is one module (relax_backend::wasm): calls to functions the run never compiled go to the
//! shared untraced stub. The data the captured code names (literals, vtables, statics' initial
//! images, writable blocks) is placed in linear memory with pointer words translated: data
//! pointers to their wasm addresses, function pointers to table indices. Runtime entries become
//! imports `r.<name>` (memcpy/memmove/memset/catch_panic are built in), C symbols `c.<name>`,
//! provided-crate entries `p.<key>`.
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
fn shake_blocks(f: &mut Func, trap: bool, keep: &dyn Fn(u32) -> bool, mut stub: Option<&mut u32>) -> (usize, usize) {
    use relax_backend::rir::Term;
    let cov = |i: &Inst| -> Option<bool> {
        match i {
            Inst::Coverage { word, mask } => crate::coverage::hit(*word, *mask),
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
            }
        }
        extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        unsafe { atexit(at_exit) };
    }
    // on the kept heap: a zone swap frees the compile arenas while the run goes on
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

/// The union of several runs (`MAKEPAD_RUN_WASM_EXPORT_UNION=<coverage.json>[,...]`: the
/// makepad-web-trace coverage of the drivers' runs, functions by `jit::slot_key`): every function the
/// captured code can reach (calls, function addresses, pointers in its data) whose key another run
/// executed is compiled now, so it is captured too; to a fixpoint (a forced function's own callees).
/// Returns the slots forced (kept in the module although this run never installed them).
fn force_union(up: *mut Unit) -> std::collections::BTreeSet<u32> {
    let mut forced = std::collections::BTreeSet::new();
    let Ok(file) = std::env::var("MAKEPAD_RUN_WASM_EXPORT_UNION") else { return forced };
    let mut keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    for path in file.split(',').filter(|p| !p.is_empty()) {
        let j = std::fs::read(path).map_err(|e| e.to_string()).and_then(|d| crate::json::parse(&d).map_err(|e| e.to_string()));
        match j {
            Ok(j) => keys.extend(j.get("functions").map_or(&[][..], |f| f.arr()).iter().filter_map(|f| f.get("key")).map(|k| k.str().to_string())),
            Err(e) => eprintln!("wasm export: union {}: {}", path, e),
        }
    }
    let u = unsafe { &*up };
    let mut tried: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let (mut failed, mut rounds) = (0usize, 0usize);
    loop {
        rounds += 1;
        // the slots the captured set names that it lacks
        let want: Vec<u32> = {
            let g = CAPTURED.lock().unwrap_or_else(|e| e.into_inner());
            let Some(captured) = g.as_ref().and_then(|g| g.get(&u.wasm_export_id)) else { return forced };
            let funcs = &captured.funcs;
            let mut named: Vec<u32> = Vec::new();
            address_taken(u, funcs, &mut named);
            for f in funcs.values() {
                for b in &f.blocks {
                    for i in &b.insts {
                        if let Inst::Call(relax_backend::rir::Callee::Fn(t) | relax_backend::rir::Callee::CFn(t, _), _, _) = i {
                            named.push(*t);
                        }
                    }
                }
            }
            named.sort_unstable();
            named.dedup();
            named.into_iter().filter(|t| (*t as usize) < u.fns.len() && !funcs.contains_key(t) && !tried.contains(t)).collect()
        };
        let mut any = false;
        for t in want {
            tried.insert(t);
            if !keys.contains(crate::jit::slot_key(u, t).as_str()) {
                continue;
            }
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { crate::jit::compile_fn(up, t) })) {
                Ok(Ok(_)) => {
                    forced.insert(t);
                    any = true;
                }
                _ => failed += 1,
            }
        }
        if !any {
            break;
        }
    }
    if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
        eprintln!("wasm export: union: {} keys, {} functions other runs executed compiled in ({} rounds, {} failed)", keys.len(), forced.len(), rounds, failed);
    }
    forced
}

/// Writes the module of the captured functions to the export path.
pub fn finish(u: &Unit) {
    let Some(path) = std::env::var("MAKEPAD_RUN_WASM_EXPORT").ok() else { return };
    if FORCING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let forced = force_union(u as *const Unit as *mut Unit);
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
    funcs.retain(|id, _| (*id as usize) < u.fns.len() && (u.fns[*id as usize].code != 0 || forced.contains(id)));
    if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
        eprintln!("wasm export: {} slots captured, {} installed (of {} slots in the unit)", n_cap, funcs.len(), u.fns.len());
    }
    if let Ok(pat) = std::env::var("MAKEPAD_RUN_WASM_EXPORT_DUMP") {
        for (id, f) in &funcs {
            if f.name.contains(pat.as_str()) {
                eprintln!("slot {}: {}", id, f.dump());
            }
        }
    }
    let (kept, kept_failed) = keep_closure(&mut funcs, u);
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
                emit(u, &funcs, &kept, kept_failed, &forced, o, &p);
            }
        }
        Err(_) => emit(u, &funcs, &kept, kept_failed, &forced, &std::env::var("MAKEPAD_RUN_WASM_OPT").unwrap_or_default(), &path),
    }
}

/// One module of the captured set with options `optv` (`-tee,remat,...`), written to `path`.
fn emit(u: &Unit, funcs: &BTreeMap<u32, Func>, kept: &std::collections::BTreeSet<u32>, kept_failed: usize, forced: &std::collections::BTreeSet<u32>, optv: &str, path: &str) {
    let options = u.prog.session.options.clone();
    // the web player's profile: panics abort, one thread (TLS keys and waits built in)
    // untraced calls are a bare trap (option untraced: env.u(id) first, for a message)
    let mut opts = wasm::Options {
        panic_abort: true,
        untraced_import: false,
        memory64: options.check_target.web() == Some(crate::options::PtrWidth::W64),
        ..wasm::Options::default()
    };
    let mut single = true;
    // off by default: under brotli the merged module is LARGER (P(DOOM), block-shaken: raw -360 KB,
    // 23,345 -> 18,620 functions, br11 +5.0 KB): the compressor already shares near-identical
    // bodies, the callers' extra constants cost more. Kept as an option for raw size / compile time.
    let mut merge = false;
    let mut splash = true;
    let mut blocks = crate::coverage::on();
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
                "blocks" => blocks = on && crate::coverage::on(),
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
            // a function another run executed (the union) has no block coverage here: whole
            let (t, r) = shake_blocks(f, blocks && !kept.contains(id) && !forced.contains(id), &keep, if untraced { Some(&mut stub_id) } else { None });
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
    roots.retain(|r| funcs.contains_key(&r.0));
    // roots: the entry and every function whose address any traced code takes (directly or in
    // the data it names): the program enters those through pointers made at run time (callbacks
    // a provided crate holds, vtables), which the static walk from the entry does not see. A slot
    // that ran but is named only by direct calls its callers' later builds inlined is dropped.
    let mut seeds: Vec<u32> = roots.iter().map(|r| r.0).collect();
    address_taken(u, funcs, &mut seeds);
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
    let web = options.check_target.web().is_some();
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
            Some(SymKind::Foreign(n)) => match single.then(|| single_thread(&n)).flatten() {
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
        // pointer-sized words (4 bytes on wasm32, holding window offsets: crate::syms::word_in)
        let pw = u.ptr_bytes() as usize;
        let mut k = 0;
        while k + pw <= bytes.len() {
            let w = crate::syms::word_in(u, &bytes, k);
            if (if pw == 4 { w & 0xffff_ffff } else { w }) > 0x1_0000 {
                if let Some(s) = u.syms.symbol_at(w, u) {
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
            }
            k += pw;
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
    for (id, bytes) in images {
        let a = placed[&id].0;
        m.data_at(a, bytes);
    }
    let link = Linked { vals, host };
    let nf = reached.len();
    let out = m.finish(&options, &link);
    if let Err(e) = std::fs::write(path, &out.bytes) {
        eprintln!("wasm export: {}: {}", std::path::Path::new(&path).display(), e);
        return;
    }
    // MAKEPAD_RUN_WASM_EXPORT_FNLIST=<file>: one line per module function for size analysis:
    // slot, wasm bytes (0: folded into an identical one), RIR insts, blocks, direct call sites to
    // it in the module, address taken, kept (keep_reachable, not traced), name
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
        let mut txt = String::new();
        for (id, n, b) in &out.stats.fn_sizes {
            let f = &funcs[id];
            let ni: usize = f.blocks.iter().map(|b| b.insts.len()).sum();
            txt.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", id, b, ni, f.blocks.len(), calls_in.get(id).copied().unwrap_or(0), taken.contains(id) as u8, kept.contains(id) as u8, n));
        }
        let fl = fl.replace("%p", &std::process::id().to_string());
        let _ = std::fs::write(&fl, txt);
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
        eprintln!("wasm export: keep_reachable closure: {} functions not traced, {} code bytes ({} failed to build)", kept.len(), kept_bytes, kept_failed);
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
fn address_taken(u: &Unit, funcs: &BTreeMap<u32, Func>, out: &mut Vec<u32>) {
    let mut data_seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut dwork: Vec<u32> = Vec::new();
    for f in funcs.values() {
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::FnAddr(_, t) => out.push(*t),
                    Inst::Addr(_, s) | Inst::TlsAddr(_, s) => match u.syms.kind(s.id) {
                        Some(SymKind::Thunk(t)) => out.push(t),
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
        let pw = u.ptr_bytes() as usize;
        for k in (0..bytes.len() / pw).map(|i| i * pw) {
            let w = crate::syms::word_in(u, &bytes, k);
            if (if pw == 4 { w & 0xffff_ffff } else { w }) > 0x1_0000 {
                if let Some(s) = u.syms.symbol_at(w, u) {
                    match u.syms.kind(s.id) {
                        Some(SymKind::Thunk(t)) => out.push(t),
                        Some(SymKind::Data(_) | SymKind::Rw(_) | SymKind::Static(_)) => dwork.push(s.id),
                        _ => {}
                    }
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
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
        let pw = u.ptr_bytes() as usize;
        for k in (0..bytes.len() / pw).map(|i| i * pw) {
            let w = crate::syms::word_in(u, &bytes, k);
            if (if pw == 4 { w & 0xffff_ffff } else { w }) > 0x1_0000 {
                if let Some(s) = u.syms.symbol_at(w, u) {
                    match u.syms.kind(s.id) {
                        Some(SymKind::Thunk(t)) => work.push(t),
                        Some(SymKind::Data(_) | SymKind::Rw(_) | SymKind::Static(_)) => dwork.push(s.id),
                        _ => {}
                    }
                }
            }
        }
    }
    seen.into_iter().collect()
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

/// Callees the traced code names but never ran that carry `#[relax(keep_reachable)]` (a GC, a
/// growth path), built now with their whole static call closure: (the slots added, how many
/// could not be built).
fn keep_closure(funcs: &mut BTreeMap<u32, Func>, u: &Unit) -> (std::collections::BTreeSet<u32>, usize) {
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
    for f in funcs.values() {
        callees(f, &mut work);
    }
    work.retain(|id| !funcs.contains_key(id) && (*id as usize) < u.fns.len() && crate::jit::keep_reachable(u, *id));
    let mut kept = std::collections::BTreeSet::new();
    let mut failed = 0;
    let up = crate::jit::current_unit();
    while let Some(id) = work.pop() {
        if funcs.contains_key(&id) || (id as usize) >= u.fns.len() {
            continue;
        }
        let r = crate::owner::call(crate::owner::Class::Program, || crate::zone_alloc::kept_with(|| unsafe { crate::jit::export_rir(up, id) }));
        match r {
            Ok(f) => {
                callees(&f, &mut work);
                funcs.insert(id, f);
                kept.insert(id);
            }
            Err(_) => failed += 1,
        }
    }
    (kept, failed)
}
