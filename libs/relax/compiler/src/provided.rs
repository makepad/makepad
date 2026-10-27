//! Provided crates (local/plans/loader-core.md section 8): a crate rustc built into the binary
//! that runs Relax (the `makepad` loader builds makepad-script into itself). Relax still loads
//! its source (types, signatures, generic and inline bodies) but binds the fns the crate marks
//! `#[relax_api]` (makepad-relax-api) to the compiled code: each gets a bridge body
//! (lower::bridge_func) with Relax's own ABI that packs its arguments into one uniform C call,
//! `extern "C" fn(args: *const *mut u8, ret: *mut u8) -> u32`, of the wrapper the attribute
//! generated into the crate. Callers, fn pointers and vtables need nothing else: the slot is an
//! ordinary Relax fn.
//!
//! The attribute also marks plain data (`plain`: rustc's size/align/field offsets, checked against
//! Relax's) and opaque handles (`handle`: rustc's size/align/drop and listed trait ops). A marked
//! op must be C-shaped (`unbound_reason`); every unmarked fn of the crate compiles from source when
//! JIT code reaches it.
//!
//! Each op is registered under its key (`core::any::type_name` of the fn item, which Relax
//! computes the same way) with rustc's signature (`type_name` of its fn pointer type) and the
//! layout of each parameter and of the result; a mismatch is a clear "rebuild makepad" error at
//! bind time, never a wrong call.
//!
//! Before binding, every registered crate present in the program must match the loader's
//! library source closure (srchash.rs). A mismatch is always an error naming changed, added
//! and removed inputs and feature/version differences; it never selects a second source copy.

use crate::lang_items::LangItem;
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};
use std::collections::HashMap;
use relax_session::sync::Mutex;

/// A generated shim: arguments moved out of `args[i]`, the result written to `ret`; 0 = returned,
/// 1 = panicked (the message is then read with the crate's LastPanicFn).
pub type ShimFn = unsafe extern "C" fn(*const *mut u8, *mut u8) -> u32;
/// Writes the text of the last panic a shim caught on this thread: (ptr, len), valid until the
/// thread's next shim call.
pub type LastPanicFn = unsafe extern "C" fn(*mut *const u8, *mut usize);

/// A provided type rustc lays out (its fields are the crate's business): size, align, and
/// its `drop_in_place` when it needs drop.
pub type DropFn = unsafe extern "C" fn(*mut u8);

/// One op of a provided crate: rustc's signature (`type_name` of its fn pointer type; empty for a
/// handle's trait ops), per parameter then the result rustc's [size, align, pointee size, pointee
/// align], and the wrapper.
#[derive(Clone, Copy)]
pub struct Shim {
    pub sig: &'static str,
    pub slots: &'static [[u64; 4]],
    pub f: ShimFn,
}

pub struct ProvidedCrate {
    /// the crate's name as Relax loads it (`makepad_script`)
    pub name: String,
    /// key (`type_name` of the fn item; a handle's trait ops `<path as Clone>::clone` ..) -> op
    pub shims: HashMap<String, Shim>,
    /// opaque handles by def path: (size, align, drop, a struct's field offsets in declaration
    /// order: same_layout_handles)
    pub types: HashMap<String, (u64, u64, Option<DropFn>, &'static [u64])>,
    /// plain types by def path: rustc's (size, align, field offsets), checked against Relax's at
    /// unit creation
    pub layouts: HashMap<String, (u64, u64, &'static [u64])>,
    pub last_panic: LastPanicFn,
    /// the source identity rustc built it from (srchash.rs; the loader's build.rs)
    pub source: u64,
    /// Per-file and feature/version identity embedded by the loader build.
    pub manifest: &'static [u8],
}

impl Clone for ProvidedCrate {
    fn clone(&self) -> Self {
        ProvidedCrate { name: self.name.clone(), shims: self.shims.clone(), types: self.types.clone(), layouts: self.layouts.clone(), last_panic: self.last_panic, source: self.source, manifest: self.manifest }
    }
}

static REGISTRY: Mutex<Vec<ProvidedCrate>> = Mutex::new(Vec::new());

pub(crate) fn registered(name: &str) -> bool {
    let name = name.replace('-', "_");
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p.name == name)
}

/// Registers a provided crate (before `cli_main`): its `#[relax_api]` entries as the loader read
/// them from its own binary (makepad_relax_api::entries), and the panic-text accessor.
pub fn register(
    name: &str,
    shims: HashMap<String, Shim>,
    types: HashMap<String, (u64, u64, Option<DropFn>, &'static [u64])>,
    layouts: HashMap<String, (u64, u64, &'static [u64])>,
    last_panic: LastPanicFn,
    source: u64,
    manifest: &'static [u8],
) {
    let name = name.replace('-', "_");
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).push(ProvidedCrate { name, shims, types, layouts, last_panic, source, manifest });
}

/// The registered crates whose source in the program is what the loader was built from
/// (check_sources); only they bind.
static MATCHED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Compares every registered crate present in the program against its embedded library
/// closure. Any mismatch fails with file and feature/version differences; absent crates skip.
const IDENTITY_THREADS: usize = 4;

pub fn check_sources(pkgs: &[crate::srchash::SrcPkg]) -> Result<(), String> {
    let reg: Vec<_> = REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|p| (p.name.clone(), p.source, p.manifest)).collect();
    let mut verified = Vec::new();
    let mut files = std::collections::HashMap::new();
    // the identity files of every package folder the registered crates reach, read and hashed
    // by a few threads (a folder is hashed once whatever number of closures reach it)
    let mut dirs: Vec<String> = Vec::new();
    for (name, _, _) in &reg {
        for (root, _) in pkgs.iter().enumerate().filter(|(_, k)| k.name.replace('-', "_") == *name) {
            for d in crate::srchash::closure_dirs(pkgs, root) { if !dirs.contains(&d) { dirs.push(d); } }
        }
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done: Vec<Mutex<Option<Vec<(String, u64)>>>> = dirs.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..dirs.len().min(IDENTITY_THREADS) {
            scope.spawn(|| {
                let _z = crate::zone_alloc::compiler_thread();
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(dir) = dirs.get(k) else { break };
                    *done[k].lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::srchash::identity_files(std::path::Path::new(dir)));
                }
            });
        }
    });
    for (dir, list) in dirs.iter().zip(done) {
        if let Some(list) = list.into_inner().unwrap_or_else(|e| e.into_inner()) { files.insert(dir.clone(), list); }
    }
    for (name, source, manifest) in &reg {
        for (root, _) in pkgs.iter().enumerate().filter(|(_, k)| k.name.replace('-', "_") == *name) {
            let current = crate::srchash::closure_manifest_with(pkgs, root, &mut files);
            if crate::srchash::manifest_hash(&current) != *source {
                let built = crate::srchash::decode_manifest(manifest)?;
                return Err(crate::srchash::diff_message(name, &built, &current));
            }
            if !verified.contains(name) { verified.push(name.clone()); }
        }
    }
    let mut matched = MATCHED.lock().unwrap_or_else(|e| e.into_inner());
    for name in verified {
        if !matched.contains(&name) {
            crate::zone_alloc::kept_with(|| matched.push(name.clone()));
        }
    }
    Ok(())
}

/// The boot's source check, running beside the load (`check_sources_later`).
static PENDING: Mutex<Option<std::thread::JoinHandle<Result<(), String>>>> = Mutex::new(None);

/// `check_sources`, on a thread of its own. The boot starts it as early as `check_sources` could
/// run and goes on loading; nothing may bind a provided crate before `finish_sources` has
/// returned Ok (`take_for`, `take_for_crates` and the boot's own join before the unit is built
/// call it). The hashes, the manifests and the error are those of `check_sources`.
pub fn check_sources_later(pkgs: std::sync::Arc<Vec<crate::srchash::SrcPkg>>) {
    let _k = crate::zone_alloc::kept();
    let spawned = std::thread::Builder::new().name("run-provided-sources".into()).spawn(move || {
        let _z = crate::zone_alloc::compiler_thread();
        let t = std::time::Instant::now();
        let verdict = check_sources(&pkgs);
        if crate::preload::trace() { eprintln!("boot-trace: provided sources checked in {:.1} ms (beside the load)", t.elapsed().as_secs_f64() * 1e3); }
        verdict
    });
    match spawned {
        Ok(h) => *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(h),
        Err(e) => *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::thread::spawn(move || Err(format!("provided source check: {e}")))),
    }
}

/// The verdict of the check `check_sources_later` started, once it has ended.
static VERDICT: Mutex<Option<Result<(), String>>> = Mutex::new(None);

/// Waits for the check `check_sources_later` started, if one is running: its verdict (every
/// caller gets it, the proc-macro world's unit as well as the program's). Ok when none started.
pub fn finish_sources() -> Result<(), String> {
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = pending.take() {
        let verdict = h.join().unwrap_or_else(|_| Err("provided source check: the thread panicked".into()));
        *VERDICT.lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::zone_alloc::kept_clone(&verdict));
    }
    VERDICT.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or(Ok(()))
}

/// Reads what `check_sources` reads, for the identity of every registered crate in the program,
/// and nothing else. A check for another target binds nothing, but the run on that target
/// (the phone) compares these identities: `pack-src` records the files this reads so its archive
/// holds them.
pub fn read_identity_files(pkgs: &[crate::srchash::SrcPkg]) {
    let reg = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    for p in reg.iter() {
        for (root, _) in pkgs.iter().enumerate().filter(|(_, k)| k.name.replace('-', "_") == p.name) {
            let _ = crate::srchash::closure_hash(pkgs, root);
        }
    }
}

/// The registered crates as (crate index in `prog`, entry) whose source matched (check_sources);
/// crates not in the program are left out. Taken once per unit (Unit::new).
pub fn take_for(prog: &Program) -> Vec<(u32, ProvidedCrate)> {
    // nothing binds before the source check has passed (a failed one binds nothing)
    if finish_sources().is_err() { return Vec::new(); }
    let matched = MATCHED.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for p in REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        if !matched.contains(&p.name) {
            continue;
        }
        if let Some(i) = prog.crates.iter().position(|c| prog.syms.str(c.name) == p.name) {
            out.push((i as u32, p.clone()));
        }
    }
    out
}

/// As [`take_for`], for the crates of `prog` named by `only` (indices a run-time load added) that
/// are not bound yet.
pub fn take_for_crates(prog: &Program, only: &[u32]) -> Vec<(u32, ProvidedCrate)> {
    if finish_sources().is_err() { return Vec::new(); }
    let matched = MATCHED.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for p in REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        if !matched.contains(&p.name) {
            continue;
        }
        for &i in only {
            if prog.syms.str(prog.crates[i as usize].name) == p.name {
                out.push((i, p.clone()));
            }
        }
    }
    out
}

/// Is crate `krate` of `prog` a registered provided crate?
fn is_provided(prog: &Program, krate: u32) -> bool {
    let name = prog.syms.str(prog.crates[krate as usize].name);
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p.name == name)
}

/// `#[repr(transparent)]` on an ADT (Relax's AdtT records repr(C)/int only; transparent lays
/// out as its one non-ZST field, which `trusted` checks).
fn is_transparent(prog: &Program, d: DefId) -> bool {
    let def = prog.def(d);
    let f = &prog.files[def.file as usize];
    f.ast.list(f.ast.item(def.item).attrs).iter().any(|a| {
        a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "repr" && (a.toks.lo + 1..a.toks.hi).any(|i| f.tok_text(i) == "transparent")
    })
}

/// Identity flags follow rustc_middle/src/ty/adt.rs:371; host layout checks remain required.
/// Is `t` the same bytes on both sides of the boundary (rustc-built crate, Relax-built caller)?
/// `krate` is the provided crate: pointers to its own types are opaque handles.
pub fn trusted(prog: &Program, tcx: &Tcx, krate: u32, t: TyId, depth: u32) -> bool {
    trusted_in(prog, tcx, krate, t, depth, false)
}

/// `field`: `t` is a field of a provided type, where a pointer to a trait object may pass
/// through (rustc stores and hands it back, never calls through it: the vtable is Relax's; the
/// VM's `host: &mut dyn ScriptHost`). As a bound fn's own parameter it may not.
fn trusted_in(prog: &Program, tcx: &Tcx, krate: u32, t: TyId, depth: u32, field: bool) -> bool {
    if depth > 8 {
        return false;
    }
    let tr = |t: TyId| trusted_in(prog, tcx, krate, t, depth + 1, false);
    // a pointer to the crate's own values (rustc made them: the pointer is a handle) or to
    // trusted ones
    let own_pub = |x: TyId| matches!(tcx.tys.kind(x), TyKind::Adt(d, a, _) if a.is_empty() && prog.def(*d).krate == krate && prog.def(*d).is_pub);
    let pointee = |x: TyId| match tcx.tys.kind(x) {
        TyKind::Str => true,
        // the crate's own public types as elements: rustc's stride (TYPES / LAYOUTS), read only
        // through the crate's fns
        TyKind::Slice(e) => tr(*e) || own_pub(*e),
        // generic ones too: the pointer is an address, their fields stay unread (lower.rs); so
        // are another provided crate's (rustc-built into the same binary: the census's &mut ScriptVm)
        TyKind::Adt(d, _, _) => prog.def(*d).krate == krate || is_provided(prog, prog.def(*d).krate) || tr(x),
        TyKind::Dyn(..) => field,
        _ => tr(x),
    };
    match tcx.tys.kind(t) {
        TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) | TyKind::Never => true,
        // tuples: rustc's size checked through LAYOUTS (layout_deps)
        TyKind::Tuple(v) => tcx.tys.list(*v).iter().all(|x| tr(*x)),
        TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) => pointee(*x),
        TyKind::FnPtr(_, _, header, _) => header.abi.is_c_like(),
        TyKind::Array(e, _) => tr(*e),
        TyKind::Adt(d, a, _) => {
            // Box<T> (global allocator): both sides allocate with the system malloc, but a box
            // OWNS its value: JIT-compiled code that drops it runs T's drop glue. That glue is
            // rustc's when T is marked `#[relax_api(handle)]` (its entry carries rustc's
            // drop_in_place) or `#[relax_api(plain)]` (layout checked); a private T has no entry, so
            // Relax would lay it out and drop it by its own rules (String/Vec fields: a bogus
            // free). Such a box is no handle: the type holding it stays opaque (TYPES), dropped
            // and cloned by the crate.
            if tcx.adts.get(d).is_some_and(|a| a.is_box()) {
                return tcx.tys.list(*a).len() <= 2
                    && tcx.tys.list(*a).first().is_some_and(|x| match tcx.tys.kind(*x) {
                        TyKind::Str => true,
                        TyKind::Slice(e) => tr(*e) || own_pub(*e),
                        TyKind::Dyn(..) => false,
                        TyKind::Adt(d2, _, _) if prog.def(*d2).krate == krate => own_pub(*x) || tr(*x),
                        _ => pointee(*x),
                    });
            }
            let std_crate = is_std(prog, prog.def(*d).krate);
            // Option<T>: rustc's layout (niche or tag), checked per op slot (the entry's slots)
            // PhantomData<T>: zero-sized, the same on both sides
            if tcx.adts.get(d).is_some_and(|a| a.is_phantom_data()) {
                return true;
            }
            if tcx.lang_items.get(LangItem::Option) == Some(*d) && tcx.tys.list(*a).len() == 1 {
                return tr(tcx.tys.list(*a)[0]);
            }
            if !a.is_empty() {
                return false;
            }
            let adt = match tcx.adts.get(d) {
                Some(x) => x,
                None => return false,
            };
            // std's own types: fieldless enums only (Ordering), checked against LAYOUTS;
            // other crates' plain data (LiveId, the math vectors): as the crate's own
            if std_crate && !(adt.is_enum && adt.variants.iter().all(|v| v.fields.is_empty())) {
                return false;
            }
            if adt.is_union {
                return false;
            }
            let fields_ok = |v: &crate::tcx::VariantT| v.fields.iter().all(|f| trusted_in(prog, tcx, krate, f.ty, depth + 1, true));
            if adt.is_enum {
                if adt.variants.iter().all(|v| v.fields.is_empty()) {
                    return true;
                }
                // repr(C) / repr(int) data enums: laid out by RFC 2195
                if adt.repr_c || adt.repr_int.is_some() {
                    return adt.variants.iter().all(fields_ok);
                }
                // Option-like over a reference: the null niche (rustc's guaranteed layout)
                let opt_like = adt.variants.len() == 2
                    && adt.variants.iter().filter(|v| v.fields.is_empty()).count() == 1
                    && adt.variants.iter().any(|v| v.fields.len() == 1 && matches!(tcx.tys.kind(v.fields[0].ty), TyKind::Ref(..)) && fields_ok(v));
                return opt_like;
            }
            // structs, any repr: Relax lays them out as rustc does (layout.rs); each op's entry
            // carries rustc's size/align per slot and a mismatch refuses the binding
            let _ = is_transparent;
            adt.variants.iter().all(fields_ok)
        }
        _ => false,
    }
}

fn is_std(prog: &Program, krate: u32) -> bool {
    matches!(prog.syms.str(prog.crates[krate as usize].name), "core" | "alloc" | "std")
}

/// Every type in `t` whose layout the bound fn relies on and that is not the crate's own
/// non-generic type (marked plain / handle already): std instances (Option<T>) and other crates'
/// types, whose size/align the op entries' slots record; Relax compares before binding.
pub fn layout_deps(prog: &Program, tcx: &Tcx, krate: u32, t: TyId, out: &mut Vec<TyId>) {
    match tcx.tys.kind(t) {
        TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) => {
            // a pointer to the crate's own values is a handle; to others: their layout matters
            if !matches!(tcx.tys.kind(*x), TyKind::Adt(d, _, _) if prog.def(*d).krate == krate) {
                layout_deps(prog, tcx, krate, *x, out);
            }
        }
        TyKind::Slice(e) | TyKind::Array(e, _) => layout_deps(prog, tcx, krate, *e, out),
        TyKind::Tuple(v) if !v.is_empty() => {
            if !out.contains(&t) {
                out.push(t);
            }
            for x in tcx.tys.list(*v) {
                layout_deps(prog, tcx, krate, *x, out);
            }
        }
        TyKind::Adt(d, a, _) => {
            if tcx.adts.get(d).is_some_and(|a| a.is_box()) {
                return;
            }
            if prog.def(*d).krate != krate || !a.is_empty() {
                if !out.contains(&t) {
                    out.push(t);
                }
            }
            for x in tcx.tys.list(*a) {
                layout_deps(prog, tcx, krate, *x, out);
            }
        }
        _ => {}
    }
}

/// The key an op is registered under, as `#[relax_api]` records it (makepad-relax-api's
/// key_code): a trait impl's method `<T as Trait>::m`, an inherent method `<T>::m` (T as
/// `type_name` prints it), a free fn its path -- [`norm`]ed.
pub fn key(prog: &Program, tcx: &Tcx, d: DefId) -> String {
    let def = prog.def(d);
    let name = prog.syms.str(def.name);
    if def.parent != crate::program::NO_DEF && prog.def(def.parent).kind == DefKind::Impl {
        if let Some(&st) = tcx.impl_self.get(&def.parent) {
            let ty = crate::typeck::type_name(prog, tcx, st);
            return norm(&match tcx.impl_trait.get(&def.parent) {
                Some(&tr) => {
                    let mut t = crate::typeck::item_path(prog, tcx, tr);
                    if let Some(a) = tcx.impl_trait_args.get(&def.parent).filter(|a| !a.is_empty()) {
                        t.push_str(&format!("<{}>", a.iter().map(|x| crate::typeck::type_name(prog, tcx, *x)).collect::<Vec<_>>().join(", ")));
                    }
                    format!("<{} as {}>::{}", ty, t, name)
                }
                None => format!("<{}>::{}", ty, name),
            });
        }
    }
    norm(&crate::typeck::item_path(prog, tcx, d))
}

/// A key or signature as both sides compare it: no lifetimes, no spaces, no empty `<>` (rustc
/// prints `ScriptVm<'_>`, Relax `ScriptVm`), std's items as `core::<name>` (rustc's
/// `alloc::string::String` is Relax's `core::string::String`, `core::ops::deref::Deref` its
/// `core::ops::Deref`; same-named std items stay apart by the slots' layouts).
pub fn norm(s: &str) -> String {
    let s = sig_norm(&s.replace(" as ", " @ "));
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == ':' && out.ends_with(':') {
            for k in ["alloc:", "std:"] {
                let at = out.len() - k.len();
                if out.ends_with(k) && !out[..at].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
                    out.truncate(at);
                    out.push_str("core:");
                    break;
                }
            }
        }
        match c {
            ',' if out.ends_with('<') || out.ends_with(',') => {}
            '>' if out.ends_with(',') => {
                out.pop();
                if out.ends_with('<') {
                    out.pop();
                } else {
                    out.push('>');
                }
            }
            '>' if out.ends_with('<') => {
                out.pop();
            }
            _ => out.push(c),
        }
    }
    // std's items by crate and name only: Relax's std puts them in other modules
    // (`core::ops::Deref`, rustc `core::ops::deref::Deref`)
    let b = out.as_bytes();
    let mut std_out = String::with_capacity(out.len());
    let mut i = 0;
    while i < b.len() {
        let boundary = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b':');
        if boundary && out[i..].starts_with("core::") {
            // the segments: keep the last one
            let mut j = i + 6;
            let mut last = j;
            loop {
                let mut k = j;
                while k < b.len() && (b[k].is_ascii_alphanumeric() || b[k] == b'_') {
                    k += 1;
                }
                if k > j && out[k..].starts_with("::") && k + 2 < b.len() && (b[k + 2].is_ascii_alphabetic() || b[k + 2] == b'_') {
                    j = k + 2;
                    last = j;
                } else {
                    break;
                }
            }
            std_out.push_str("core::");
            i = last;
            continue;
        }
        std_out.push(b[i] as char);
        i += 1;
    }
    std_out.replace('@', " as ")
}

/// The host bridge key for Clone / PartialEq on an opaque type.
pub fn opaque_method_key(prog: &Program, d: DefId, method: &str) -> String {
    match method {
        "eq" => norm(&format!("<{} as PartialEq>::eq", prog.def_path(d))),
        _ => norm(&format!("<{} as Clone>::clone", prog.def_path(d))),
    }
}

/// The signature checked at bind time: `type_name` of the fn's fn pointer type as Relax reads the
/// source, lifetimes and spacing dropped (rustc's side: the same of `#[relax_api]`'s record).
pub fn sig(prog: &Program, tcx: &Tcx, d: DefId) -> String {
    let Some(s) = tcx.fn_sig(prog, d) else { return String::new() };
    let params: Vec<String> = s.params.iter().map(|p| crate::typeck::type_name(prog, tcx, *p)).collect();
    let unit = matches!(tcx.tys.kind(s.ret), TyKind::Tuple(v) if v.is_empty());
    let unsafe_ = if s.header.safety == crate::types::Safety::Unsafe { "unsafe " } else { "" };
    let text = if unit { format!("{unsafe_}fn({})", params.join(", ")) } else { format!("{unsafe_}fn({}) -> {}", params.join(", "), crate::typeck::type_name(prog, tcx, s.ret)) };
    norm(&text)
}

/// A signature without lifetimes or spaces (rustc prints `&'_ T` in fn pointer types).
pub fn sig_norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\'' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            continue;
        }
        if !b[i].is_ascii_whitespace() {
            out.push(b[i] as char);
        }
        i += 1;
    }
    out
}

/// Why `d` (a fn of provided crate `krate`) is compiled from source instead of bound to a shim;
/// None: it is bound.
pub fn unbound_reason(prog: &Program, tcx: &Tcx, krate: u32, d: DefId, opaque: &dyn Fn(DefId) -> bool) -> Option<&'static str> {
    let def = prog.def(d);
    if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn) {
        return Some("not a fn");
    }
    let s = tcx.fn_sig(prog, d)?;
    if s.n_generics > 0 {
        return Some("generic");
    }
    if s.header.abi.is_c_like() || s.header.c_variadic || s.track_caller {
        return Some("abi");
    }
    if crate::jit::inline_attrs(prog, d) & relax_codegen::opt::ATTR_ALWAYS != 0 {
        return Some("#[inline(always)]: compiled on the caller's side");
    }
    let parent = prog.def(def.parent);
    if parent.kind == DefKind::Trait {
        return Some("trait default body");
    }
    // Eq's derived assertion is a rustc-internal type-check helper, not a callable API.
    if parent.kind == DefKind::Impl
        && prog.syms.str(def.name) == "assert_fields_are_eq"
        && tcx.impl_trait.get(&def.parent).is_some_and(|t| prog.def_path(*t) == "core::cmp::Eq")
    {
        return Some("Eq assertion (compiler internal)");
    }
    // `Drop::drop` runs inside the type's drop shim (rustc refuses an explicit call, E0040)
    if parent.kind == DefKind::Impl && tcx.impl_trait.get(&def.parent).is_some_and(|t| tcx.lang_items.get(LangItem::Drop) == Some(*t)) {
        return Some("Drop::drop (in the drop shim)");
    }
    let crosses = |t: TyId| trusted(prog, tcx, krate, t, 0) || matches!(tcx.tys.kind(t), TyKind::Adt(d, a, _) if a.is_empty() && opaque(*d));
    if !s.params.iter().chain(std::iter::once(&s.ret)).all(|t| crosses(*t)) {
        return Some("signature not C-shaped");
    }
    None
}

/// Is ADT `d` of crate `krate` one that may be marked `#[relax_api(handle)]` (rustc's size/align, drop shim):
/// a public non-generic struct or enum that is not `trusted` by value.
pub fn opaque_candidate(prog: &Program, tcx: &Tcx, krate: u32, d: DefId) -> bool {
    let def = prog.def(d);
    if def.krate != krate || !def.is_pub || !matches!(def.kind, DefKind::Struct | DefKind::Enum) {
        return false;
    }
    match tcx.adts.get(&d) {
        Some(a) if a.n_generics == 0 && !a.is_union => {}
        _ => return false,
    }
    let t = tcx.tys.intern(TyKind::Adt(d, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
    !trusted(prog, tcx, krate, t, 0)
}

/// The types whose values cross between the provided crates' rustc code and JIT-compiled code:
/// reachable from the signatures of the bound fns (`bound`) through references, arrays, tuples,
/// type arguments and the fields of provided-crate types (and Option/Box/PhantomData). The walk
/// does not enter other std types: what a rustc-built Vec or String holds is reached only
/// through bound fns, whose signatures are roots themselves. Every other type of a provided
/// crate is made and laid out by JIT-compiled code alone (an inline shell's result, a type only
/// generic or unbound code builds): no opaque layout, no field guard.
pub fn crossing_types(prog: &Program, tcx: &Tcx, krates: &[u32], bound: &dyn Fn(DefId) -> bool) -> std::collections::HashSet<TyId> {
    let mut out = std::collections::HashSet::new();
    let mut stack: Vec<TyId> = Vec::new();
    for (i, def) in prog.defs.iter().enumerate() {
        if !krates.contains(&def.krate) || !matches!(def.kind, DefKind::Fn | DefKind::AssocFn) {
            continue;
        }
        let d = DefId(i as u32);
        if !bound(d) {
            continue;
        }
        if let Some(sg) = tcx.fn_sig(prog, d) {
            stack.extend(sg.params.iter().copied());
            stack.push(sg.ret);
        }
    }
    while let Some(t) = stack.pop() {
        if out.len() > 200_000 || !out.insert(t) {
            continue;
        }
        match tcx.tys.kind(t) {
            TyKind::Adt(d, a, _) => {
                let k = prog.def(*d).krate;
                if krates.contains(&k) {
                    if let Some(adt) = tcx.adts.get(d) {
                        for (vi, v) in adt.variants.iter().enumerate() {
                            for fi in 0..v.fields.len() {
                                stack.push(tcx.field_ty(prog, crate::tcx::TypingEnv::fully_monomorphized(&prog), *d, vi as u32, fi as u32, tcx.tys.list(*a)));
                            }
                        }
                    }
                    stack.extend(tcx.tys.list(*a).iter().copied());
                } else if tcx.lang_items.get(LangItem::Option) == Some(*d) || tcx.adts.get(d).is_some_and(|a| a.is_box() || a.is_phantom_data()) {
                    stack.extend(tcx.tys.list(*a).iter().copied());
                }
            }
            TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) | TyKind::Slice(x) | TyKind::Array(x, _) => stack.push(*x),
            TyKind::Tuple(v) => stack.extend(tcx.tys.list(*v).iter().copied()),
            _ => {}
        }
    }
    out
}



/// Where this binary's std keeps a `String`'s and a `Vec<T>`'s pointer, capacity and length (word
/// indices), and the capacity word of `None` of an `Option<String>` / `Option<Vec<T>>` (the
/// niche), read off live values. relax-compiler is rustc-built into the loader together with the
/// provided crates, so this is the layout of their `String` and `Vec`. None: not three words, or
/// `String` and `Vec<u64>` differ.
fn rustc_vec_words() -> Option<([usize; 3], usize)> {
    fn at(w: [usize; 3], ptr: usize, cap: usize, len: usize) -> Option<[usize; 3]> {
        let f = |x: usize| w.iter().position(|y| *y == x);
        let r = [f(ptr)?, f(cap)?, f(len)?];
        (r[0] != r[1] && r[1] != r[2] && r[0] != r[2]).then_some(r)
    }
    fn words<T>(x: &T) -> [usize; 3] {
        unsafe { *(x as *const T as *const [usize; 3]) }
    }
    let three = 3 * std::mem::size_of::<usize>();
    if std::mem::size_of::<String>() != three || std::mem::size_of::<Vec<u64>>() != three || std::mem::size_of::<Option<String>>() != three || std::mem::size_of::<Option<Vec<u64>>>() != three {
        return None;
    }
    let mut s = String::with_capacity(13);
    s.push_str("ab");
    let a = at(words(&s), s.as_ptr() as usize, s.capacity(), s.len())?;
    let mut v: Vec<u64> = Vec::with_capacity(11);
    v.extend([1, 2, 3]);
    let b = at(words(&v), v.as_ptr() as usize, v.capacity(), v.len())?;
    let (ns, nv): (Option<String>, Option<Vec<u64>>) = (std::hint::black_box(None), std::hint::black_box(None));
    let none = words(&ns)[a[1]];
    (a == b && words(&nv)[a[1]] == none).then_some((a, none))
}

/// What of makepad run's std is laid out as rustc's std lays it out (`rustc_vec_words`): the
/// same bytes on both sides, so a value built by either side's std is read, grown and freed by
/// the other's (both allocate through the loader's allocator; zone_alloc dealloc_layout frees a
/// block of any heap).
#[derive(Clone, Copy)]
struct StdSame {
    /// `Vec` (the ADT of `String`'s one field), with `String`
    vec: DefId,
    /// `Option<String>` / `Option<Vec<T>>`: rustc's niche (None in the capacity word)
    option: bool,
}

fn std_vec_same(prog: &Program, tcx: &Tcx, probe: &crate::layout::Layouts) -> Option<StdSame> {
    let lcx = crate::layout::LayoutCx::new(prog);
    let sd = tcx.lang_items.get(LangItem::String)?;
    let sadt = tcx.adts.get(&sd)?;
    if sadt.variants.len() != 1 || sadt.variants[0].fields.len() != 1 {
        return None;
    }
    let st = tcx.tys.intern(TyKind::Adt(sd, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
    let vt = tcx.field_ty(prog, crate::tcx::TypingEnv::fully_monomorphized(prog), sd, 0, 0, &[]);
    let TyKind::Adt(vd, va, _) = tcx.tys.kind(vt) else { return None };
    let (vd, va) = (*vd, *va);
    let vadt = tcx.adts.get(&vd)?;
    if tcx.tys.list(va).len() != 1 || vadt.variants.len() != 1 {
        return None;
    }
    let pb = std::mem::size_of::<usize>() as u32;
    let (ls, lv) = (probe.of(&lcx, tcx, st), probe.of(&lcx, tcx, vt));
    if ls.size != 3 * pb || ls.fields.as_slice() != [0] || lv.size != 3 * pb || lv.ext.is_some() || ls.ext.is_some() {
        return None;
    }
    let mut ours = [usize::MAX; 3];
    for (i, f) in vadt.variants[0].fields.iter().enumerate() {
        let w = match prog.syms.str(f.name) {
            "ptr" => 0,
            "cap" => 1,
            "len" => 2,
            _ => continue,
        };
        let off = *lv.fields.get(i)?;
        if off % pb != 0 {
            return None;
        }
        ours[w] = (off / pb) as usize;
    }
    let (words, none) = rustc_vec_words()?;
    if ours != words {
        return None;
    }
    // Option<String>: None is rustc's value in the capacity word, Some's payload at 0
    let option = tcx.lang_items.get(LangItem::Option).is_some_and(|od| {
        let ot = tcx.tys.intern(TyKind::Adt(od, tcx.tys.mk_list(&[st]), crate::types::RegionArgsId::EMPTY));
        let lo = probe.of(&lcx, tcx, ot);
        let none_variant = tcx.adts.get(&od).and_then(|a| a.variants.iter().position(|v| v.fields.is_empty()));
        lo.size == 3 * pb
            && lo.tag.is_some_and(|(off, _)| off == words[1] as u32 * pb)
            && matches!(lo.enc, crate::layout::TagEnc::Niche { lo: l, hi: h, start, .. } if Some(l as usize) == none_variant && l == h && start as u64 == none as u64)
            && lo.variant_fields.iter().enumerate().all(|(vi, f)| Some(vi) == none_variant || f.as_slice() == [0])
    });
    Some(StdSame { vec: vd, option })
}

/// The `#[relax_api(handle)]` structs among `cands` (def, rustc's size, align, field offsets)
/// whose layout is the same on both sides: Relax lays the struct out at rustc's size, align and
/// field offsets, and every field is the same bytes (`trusted` data, a `String` / `Vec<T>` of
/// such data when the two stds agree (std_vec_same), or another such handle). JIT-compiled code
/// may build these and read their fields; their drop and the trait ops the attribute lists stay
/// rustc's. `opaque`: the types Relax uses through the crate's fns only (the candidates among
/// them until they pass, a demoted plain type); `known`: handles found the same before (a crate
/// an app domain added after the boot).
pub fn same_layout_handles(prog: &Program, tcx: &Tcx, cands: &[(DefId, u32, u32, &'static [u64])], opaque: &dyn Fn(DefId) -> bool, known: &dyn Fn(DefId) -> bool) -> Vec<DefId> {
    if cands.is_empty() {
        return Vec::new();
    }
    let probe = crate::layout::Layouts::new();
    let lcx = crate::layout::LayoutCx::new(prog);
    let std_same = std_vec_same(prog, tcx, &probe);
    let mut ok: std::collections::HashSet<DefId> = cands
        .iter()
        .filter(|(d, size, align, offs)| {
            let adt = match tcx.adts.get(d) {
                Some(a) if !a.is_enum && !a.is_union && a.variants.len() == 1 => a,
                _ => return false,
            };
            let t = tcx.tys.intern(TyKind::Adt(*d, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
            let l = probe.of(&lcx, tcx, t);
            !offs.is_empty()
                && offs.len() == adt.variants[0].fields.len()
                && l.size == *size
                && l.align == *align
                && l.ext.is_none()
                && offs.len() == l.fields.len()
                && offs.iter().zip(&l.fields).all(|(a, b)| *a == *b as u64)
        })
        .map(|c| c.0)
        .collect();
    // a field of a candidate that turns out not to be the same bytes takes its holders with it
    loop {
        let now = ok.clone();
        let same = |krate: u32, t: TyId| same_bytes(prog, tcx, krate, t, &|d| now.contains(&d) || known(d), std_same, opaque, 0);
        ok.retain(|d| {
            let krate = prog.def(*d).krate;
            let n = tcx.adts.get(d).map_or(0, |a| a.variants[0].fields.len());
            (0..n).all(|fi| same(krate, tcx.field_ty(prog, crate::tcx::TypingEnv::fully_monomorphized(prog), *d, 0, fi as u32, &[])))
        });
        if ok.len() == now.len() {
            break;
        }
    }
    let mut v: Vec<DefId> = ok.into_iter().collect();
    v.sort_by_key(|d| d.0);
    v
}

/// Is a field of type `t` (in a handle of provided crate `krate`) the same bytes on both sides
/// (same_layout_handles)?
fn same_bytes(prog: &Program, tcx: &Tcx, krate: u32, t: TyId, ok: &dyn Fn(DefId) -> bool, std_same: Option<StdSame>, opaque: &dyn Fn(DefId) -> bool, depth: u32) -> bool {
    if depth > 8 {
        return false;
    }
    if let TyKind::Adt(d, a, _) = tcx.tys.kind(t) {
        if ok(*d) {
            return true;
        }
        if opaque(*d) {
            return false;
        }
        if let Some(std) = std_same {
            let args = tcx.tys.list(*a);
            let same = |x: TyId| same_bytes(prog, tcx, krate, x, ok, std_same, opaque, depth + 1);
            // String, Vec<T> of such T
            let vec_like = |x: TyId| match tcx.tys.kind(x) {
                TyKind::Adt(xd, xa, _) => tcx.lang_items.get(LangItem::String) == Some(*xd) || (*xd == std.vec && tcx.tys.list(*xa).first().is_some_and(|e| same(*e))),
                _ => false,
            };
            if vec_like(t) {
                return true;
            }
            if std.option && tcx.lang_items.get(LangItem::Option) == Some(*d) && args.len() == 1 && vec_like(args[0]) {
                return true;
            }
        }
    }
    trusted_in(prog, tcx, krate, t, 0, true)
}
