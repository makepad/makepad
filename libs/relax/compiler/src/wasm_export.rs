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

static CAPTURED: Mutex<Option<BTreeMap<u32, Func>>> = Mutex::new(None);
/// the first captured slot: the run's entry (the test fn, main)
static ENTRY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);

pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_WASM_EXPORT").is_some())
}

static ROOTS: Mutex<Vec<(u32, String)>> = Mutex::new(Vec::new());

/// A function the host calls (a test fn): a root of the export, exported under its name.
pub fn root(id: u32, name: &str) {
    if enabled() {
        let mut r = ROOTS.lock().unwrap_or_else(|e| e.into_inner());
        if !r.iter().any(|x| x.0 == id) {
            r.push((id, name.to_string()));
        }
    }
}

/// Records the RIR slot `id`'s code is compiled from (the last compile of a slot wins).
static FINISHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn capture(id: u32, f: &Func) {
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
        ENTRY.store(id, std::sync::atomic::Ordering::Relaxed);
    }
    // on the kept heap: a zone swap frees the compile arenas while the run goes on
    crate::zone_alloc::kept_with(|| {
        g.get_or_insert_with(BTreeMap::new).insert(id, f.clone());
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

/// Writes the module of the captured functions to the export path.
pub fn finish(u: &Unit) {
    let Some(path) = std::env::var("MAKEPAD_RUN_WASM_EXPORT").ok() else { return };
    if FINISHED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    // `%p`: the process id (a run's child processes capture their own sets)
    let path = path.replace("%p", &std::process::id().to_string());
    let Some(mut funcs) = CAPTURED.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    // the trace: slots whose code was installed (called); a build made ahead of a call that
    // never came is not part of it
    funcs.retain(|id, _| (*id as usize) < u.fns.len() && u.fns[*id as usize].code != 0);
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
                emit(u, &funcs, &kept, kept_failed, o, &p);
            }
        }
        Err(_) => emit(u, &funcs, &kept, kept_failed, &std::env::var("MAKEPAD_RUN_WASM_OPT").unwrap_or_default(), &path),
    }
}

/// One module of the captured set with options `optv` (`-tee,remat,...`), written to `path`.
fn emit(u: &Unit, funcs: &BTreeMap<u32, Func>, kept: &std::collections::BTreeSet<u32>, kept_failed: usize, optv: &str, path: &str) {
    let options = u.prog.session.options.clone();
    // the web player's profile: panics abort, one thread (TLS keys and waits built in)
    // untraced calls are a bare trap (option untraced: env.u(id) first, for a message)
    let mut opts = wasm::Options { panic_abort: true, untraced_import: false, ..wasm::Options::default() };
    let mut single = true;
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
                "fold" => opts.fold_identical = on,
                "st" => single = on,
                "names" => opts.names = on,
                "untraced" => opts.untraced_import = on,
                "exnref" => opts.exnref = on,
                _ => eprintln!("wasm export: unknown option {}", n),
            }
        }
    }
    let abort = opts.panic_abort;
    let mut m = Module::new(opts);
    let mut roots: Vec<(u32, String)> = ROOTS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if roots.is_empty() {
        roots.push((ENTRY.load(std::sync::atomic::Ordering::Relaxed), "main".to_string()));
    }
    roots.retain(|r| funcs.contains_key(&r.0));
    let reached = reach(u, funcs, &roots.iter().map(|r| r.0).collect::<Vec<_>>());
    for id in &reached {
        m.add_func(*id, &funcs[id]);
    }
    for (k, (id, name)) in roots.iter().enumerate() {
        // short export names; MAKEPAD_RUN_WASM_EXPORT_STATS lists them
        let _ = name;
        m.export_func(*id, &format!("t{}", k));
    }
    // symbols the code names
    let mut addr_syms: Vec<u32> = Vec::new();
    let mut tls_syms: Vec<u32> = Vec::new();
    let mut host_syms: Vec<u32> = Vec::new();
    for f in reached.iter().map(|id| &funcs[id]) {
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Addr(_, s) => addr_syms.push(s.id),
                    Inst::TlsAddr(_, s) => tls_syms.push(s.id),
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
            Some(SymKind::Foreign(n)) if single => single_thread(&n).unwrap_or(HostBind::Import("c".into(), n)),
            Some(SymKind::Foreign(n)) => HostBind::Import("c".into(), n),
            Some(SymKind::Provided(k)) => HostBind::Import("p".into(), k),
            _ => HostBind::Trap,
        };
        host.insert(id, b);
    }
    // data: placed on first reference, pointer words translated (worklist)
    let tls = m.place(64 * 1024, 64);
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
    let tls_size = 64 * 1024u64;
    for &id in &tls_syms {
        let off = u.syms.value(Sym::new(id));
        if off >= tls_size {
            eprintln!("wasm export: thread-local offset {} past the {} byte block", off, tls_size);
        }
        vals.insert(id, tls + off);
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
                u.syms.value(Sym::new(id))
            }
        };
        vals.insert(id, v);
    }
    let mut n_ptrs = 0usize;
    while let Some(id) = pending.pop() {
        let Some((mut bytes, _, _)) = data_of(u, id) else { continue };
        let mut k = 0;
        while k + 8 <= bytes.len() {
            let w = u64::from_le_bytes(bytes[k..k + 8].try_into().unwrap());
            if w > 0x1_0000 {
                if let Some(s) = u.syms.symbol_at(w, u) {
                    let nv = match u.syms.kind(s.id) {
                        Some(SymKind::Data(_) | SymKind::Rw(_) | SymKind::Static(_)) => place(&mut m, s.id, &mut placed, &mut pending).map(|a| a.wrapping_add(s.off as u64)),
                        Some(SymKind::Thunk(slot)) => Some(m.table_index(slot) as u64),
                        _ => None,
                    };
                    if let Some(nv) = nv {
                        bytes[k..k + 8].copy_from_slice(&nv.to_le_bytes());
                        n_ptrs += 1;
                    }
                }
            }
            k += 8;
        }
        images.push((id, bytes));
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
    if std::env::var_os("MAKEPAD_RUN_WASM_EXPORT_STATS").is_some() {
        let s = &out.stats;
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
        for (_, e) in &s.failed {
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
        for w in bytes.chunks_exact(8) {
            let w = u64::from_le_bytes(w.try_into().unwrap());
            if w > 0x1_0000 {
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
        "__relax_wait_enter" | "__relax_wait_leave" | "__relax_wake" | "__ulock_wake" | "__ulock_wait" | "pthread_self" | "pthread_threadid_np" | "pthread_main_np" | "sched_yield" | "__relax_key_delete" => HostBind::Builtin(Builtin::Nop),
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
        let r = crate::owner::call(crate::owner::Class::App, || crate::zone_alloc::kept_with(|| unsafe { crate::jit::export_rir(up, id) }));
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
