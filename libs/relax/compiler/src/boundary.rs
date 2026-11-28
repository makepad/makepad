//! Boundary audit: what code compiled by Relax reaches inside a set of crates that would be
//! built by rustc into the embedder ("provided crates": provided.rs).
//!
//! `makepad check --boundary <crate>[,<crate>..]|all <package>` lowers every non-generic fn of the
//! crates that depend on them, then every generic instance those reach (to a fixpoint), and
//! records, from each body Relax would compile (drop glue included: a Drop impl runs from it):
//! - calls to non-generic fns of the provided crates (the C shims), with their signatures;
//! - generic fns of the provided crates instantiated on this side (compiled from source);
//! - field projections into types of the provided crates (layout knowledge needed);
//! - statics of the provided crates read or borrowed.
//! A body is on the provided side when its fn is a non-generic fn of a provided crate (closures
//! count as their owner fn).
//!
//! Every body that does not lower because it reaches into a provided crate the way a run cannot
//! (a static of the crate read from JIT-compiled code, a non-C-shaped field read or value built,
//! an opaque enum matched, rustc's ownership operations: lower.rs provided_err) is a `violation`
//! row; one into an audited crate fails the check (exit 1). It is the error a run reports only
//! when the code that reaches it first runs (an app's deck reached `shell::Shared::global`
//! through the unmarked `makepad_hal::screens`); the gates run the audit over the apps.
//!
//! `--boundary statics` judges every static of every bound provided crate that JIT code can
//! reach, from the crates' source alone (`statics`): the loader's build runs it.
//!
//! An independent declaration scan reports missing or mismatched tagged Option entries as
//! `option-entry` failures, including unreachable items. `--boundary options` runs only
//! this scan for every bound provided crate, before creating a JIT unit. On another check
//! target it scans registered crates from source and checks their typed Option markers.

use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use crate::lang_items::LangItem;
use crate::jit::{FnKey, Unit};
use crate::program::{DefId, DefKind};
use crate::types::{TyId, TyKind};
use std::collections::{BTreeMap, BTreeSet};
use relax_session::sync::Mutex;

#[derive(Default)]
struct Entry {
    sites: u32,
    callers: BTreeSet<String>,
    sample: String,
    /// a static's row: how lowering treats it (`static_class`)
    class: String,
}

#[derive(Default)]
struct Audit {
    crates: Vec<u32>,
    option_entries: Vec<String>,
    /// (kind, item) -> entry; kind: "call", "generic", "field", "static"
    rec: BTreeMap<(&'static str, String), Entry>,
    /// call key -> (param types, return type) as printed
    sigs: BTreeMap<String, (Vec<TyId>, TyId)>,
    bindings: BTreeMap<String, (bool, String)>,
    /// (provided crate reached, body, error): lower.rs provided_err
    violations: BTreeSet<(u32, String, String)>,
}

static AUDIT: Mutex<Option<Audit>> = Mutex::new(None);

/// `--boundary` was given: the provided crates bind to the loader as in a run (run.rs).
pub(crate) static REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn requested() -> bool {
    REQUESTED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Resolve the audited crate set; cross-target options use registry names without binding.
pub(crate) fn crates(prog: &crate::program::Program, provided: &[(u32, crate::provided::ProvidedCrate)], names: &[String], cross_options: bool) -> Result<Vec<u32>, String> {
    let mut crates = Vec::new();
    if names.len() == 1 && matches!(names[0].as_str(), "all" | "options" | "statics") {
        if cross_options {
            crates.extend(crate::provided::registered_in(prog));
        } else {
            crates.extend(provided.iter().map(|(k, _)| *k));
        }
        if crates.is_empty() {
            if cross_options {
                return Err("--boundary options: no registered provided crate in this program".to_string());
            }
            return Err(format!("--boundary {}: no provided crate binds in this program (is the loader built from this source?)", names[0]));
        }
    } else {
        for n in names {
            let n2 = n.replace('-', "_");
            match prog.crates.iter().position(|c| prog.syms.str(c.name) == n2) {
                Some(i) => crates.push(i as u32),
                None => return Err(format!("--boundary: no crate `{}` in the graph", n)),
            }
        }
    }
    crates.sort_unstable();
    crates.dedup();
    Ok(crates)
}

/// Start the static guard before the audit lowers any bodies.
pub fn start(u: &Unit, names: &[String]) -> Result<(), String> {
    let crates = crates(&u.prog, &u.provided, names, false)?;
    let option_entries = u.provided.iter().filter(|(k, _)| crates.contains(k))
        .flat_map(|(k, _)| crate::provided::option_entry_rows(&u.prog, &u.tcx, *k, None)).collect();
    *AUDIT.lock().unwrap() = Some(Audit { crates, option_entries, ..Default::default() });
    Ok(())
}

/// Declaration-only audit, before a program's JIT unit is created.
pub(crate) fn options(prog: &crate::program::Program, tcx: &crate::tcx::Tcx, crates: &[u32], target: Option<&str>) -> (String, usize) {
    // One set for all the crates, as the loader's map is one: an entry any provided crate
    // lists covers the type wherever it crosses
    let covered = target.map(|_| crates.iter().flat_map(|&k| crate::provided::option_listed_types(prog, tcx, k)).collect());
    let mut rows: Vec<_> = crates.iter().flat_map(|&k| crate::provided::option_entry_rows(prog, tcx, k, covered.as_ref())).collect();
    rows.sort();
    let missing = rows.len();
    let target = target.map_or(String::new(), |t| format!(" ({t})"));
    rows.push(format!("relax check --boundary options{target}: {} missing Option entries in {} provided crates", missing, crates.len()));
    (rows.join("\n") + "\n", missing)
}

#[inline]
pub(crate) fn on() -> bool {
    // cheap check first: the lock is taken only while an audit runs
    let on = AUDIT_ON.load(std::sync::atomic::Ordering::Relaxed);
    // Audits count the ordinary build's calls/fields once, never speculative lowering.
    if on { crate::jit::lower_ahead_needs_unit(); }
    on
}
static AUDIT_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn provided(a: &Audit, u: &Unit, d: DefId) -> bool {
    a.crates.contains(&u.prog.def(d).krate)
}

/// Is a body of `def` with `args` compiled by Relax (not on the provided side)?
fn relax_side(a: &Audit, u: &Unit, def: DefId, args: &[TyId]) -> bool {
    !(provided(a, u, def) && binding(u, def, args).0)
}

/// The same signature and table requirements used by Unit's bridge binding.
fn binding(u: &Unit, d: DefId, args: &[TyId]) -> (bool, String) {
    if !args.is_empty() { return (false, "generic".into()); }
    let k = u.prog.def(d).krate;
    let Some((_, pc)) = u.provided.iter().find(|(pk, _)| *pk == k) else {
        return (false, "crate not registered/matched".into());
    };
    if let Some(reason) = crate::provided::unbound_reason(&u.prog, &u.tcx, k, d, &|a| u.provided_types.contains_key(&a)) {
        return (false, reason.into());
    }
    if !pc.shims.contains_key(crate::provided::key(&u.prog, &u.tcx, d).as_str()) {
        return (false, "no generated shim".into());
    }
    (true, String::new())
}

fn caller_name(u: &Unit, def: DefId) -> String {
    u.prog.syms.str(u.prog.crates[u.prog.def(def).krate as usize].name).to_string()
}

/// A call from the body of (`def`, `args`) to slot `callee`.
pub fn note_call(u: &Unit, def: DefId, args: &[TyId], callee: u32) {
    if !on() {
        return;
    }
    // (a lowering ahead's temporary callee identity names its key there: it has no slot yet)
    let key = crate::jit::lower_ahead_fn_key(callee).unwrap_or_else(|| u.fns[callee as usize].key.clone());
    let FnKey::Inst(cd, cargs) = key else { return };
    let mut g = AUDIT.lock().unwrap();
    let Some(a) = g.as_mut() else { return };
    if !provided(a, u, cd) || !relax_side(a, u, def, args) {
        return;
    }
    a.bindings.insert(crate::provided::key(&u.prog, &u.tcx, cd), binding(u, cd, &cargs));
    let generic = !cargs.is_empty();
    let name = u.prog.def_path(cd);
    let kind = if generic { "generic" } else { "call" };
    if !generic && !a.sigs.contains_key(&name) {
        if let Some(s) = u.tcx.fn_sig(&u.prog, cd) {
            a.sigs.insert(name.clone(), (s.params.clone(), s.ret));
        }
    }
    let e = a.rec.entry((kind, name)).or_default();
    e.sites += 1;
    e.callers.insert(caller_name(u, def));
    if e.sample.is_empty() {
        e.sample = u.prog.def_path(def);
    }
}

/// A field projection on a value of type `t` (after auto-deref) in the body of (`def`, `args`).
pub fn note_field(u: &Unit, def: DefId, args: &[TyId], t: TyId, fi: u32) {
    if !on() {
        return;
    }
    let TyKind::Adt(ad, _, _) = u.tcx.tys.kind(t).clone() else { return };
    let mut g = AUDIT.lock().unwrap();
    let Some(a) = g.as_mut() else { return };
    if !provided(a, u, ad) || !relax_side(a, u, def, args) {
        return;
    }
    let fname = match u.tcx.adts.get(&ad) {
        Some(adt) => adt.variants.first().and_then(|v| v.fields.get(fi as usize)).map(|f| u.prog.syms.str(f.name).to_string()).unwrap_or_else(|| fi.to_string()),
        None => fi.to_string(),
    };
    let name = format!("{}.{}", u.prog.def_path(ad), fname);
    let e = a.rec.entry(("field", name)).or_default();
    e.sites += 1;
    e.callers.insert(caller_name(u, def));
    if e.sample.is_empty() {
        e.sample = u.prog.def_path(def);
    }
}

/// JIT-compiled body `body` reaches into provided crate `krate` the way it cannot (`error`).
pub fn note_violation(u: &Unit, krate: u32, body: u32, error: &str) {
    let mut g = AUDIT.lock().unwrap();
    let Some(a) = g.as_mut() else { return };
    a.violations.insert((krate, u.fns[body as usize].name.to_string(), error.lines().next().unwrap_or("").to_string()));
}

/// A static `d` (of type `t`) read or borrowed in the body of (`def`, `args`).
pub fn note_static(u: &Unit, def: DefId, args: &[TyId], d: DefId, t: TyId) {
    if !on() {
        return;
    }
    let mut g = AUDIT.lock().unwrap();
    let Some(a) = g.as_mut() else { return };
    if !provided(a, u, d) || !relax_side(a, u, def, args) {
        return;
    }
    let e = a.rec.entry(("static", u.prog.def_path(d))).or_default();
    e.sites += 1;
    e.callers.insert(caller_name(u, def));
    if e.sample.is_empty() {
        e.sample = u.prog.def_path(def);
    }
    if e.class.is_empty() {
        e.class = static_class(u, d, t);
    }
}

/// The provided-static classifier's answer, alongside `static mut` and `Freeze`.
fn static_class(u: &Unit, d: DefId, t: TyId) -> String {
    let prog = &u.prog;
    let def = prog.def(d);
    let ast = &prog.files[def.file as usize].ast;
    let item = ast.item(def.item);
    let is_mut = matches!(item.kind, crate::ast::ItemKind::Static(true, ..));
    let freeze = u.tcx.is_freeze(prog, crate::tcx::TypingEnv::fully_monomorphized(prog), t);
    let class = match crate::provided::static_use(u, d, t) {
        crate::provided::StaticUse::Shared(_) => "shared",
        crate::provided::StaticUse::OwnInstance | crate::provided::StaticUse::NotProvided => "own",
        crate::provided::StaticUse::Copy => "copy",
        crate::provided::StaticUse::Refused(_) => "refused",
    };
    format!("class={class} mut={} freeze={}", is_mut as u8, freeze as u8)
}

/// The C shape of a type at the boundary.
fn shape(u: &mut Unit, a: &Audit, t: TyId) -> String {
    match u.tcx.tys.kind(t).clone() {
        TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) => "scalar".into(),
        TyKind::Never => "never".into(),
        TyKind::Tuple(v) if v.is_empty() => "unit".into(),
        TyKind::Tuple(_) => "tuple".into(),
        TyKind::Array(..) => "array".into(),
        TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) => match u.tcx.tys.kind(x).clone() {
            TyKind::Str => "&str".into(),
            // a pointer to a scalar (`*const usize`): an address, bound as provided.rs's
            // `pointee` binds it
            TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) => "&scalar".into(),
            TyKind::Slice(el) => format!("&[{}]", shape(u, a, el)),
            TyKind::Dyn(..) => "&dyn".into(),
            TyKind::Adt(d, _, _) if provided(a, u, d) => "&vm".into(),
            TyKind::Adt(d, _, _) => format!("&{}", u.prog.name(d)),
            _ => "&other".into(),
        },
        TyKind::Adt(d, args, _) => {
            let n = u.prog.name(d).to_string();
            if provided(a, u, d) {
                let k = u.prog.def(d).kind;
                let copy = u.is_copy(t);
                format!("vm-{}{}:{}", if k == DefKind::Enum { "enum" } else { "struct" }, if copy { "" } else { "(non-Copy)" }, n)
            } else if u.tcx.lang_items.get(LangItem::Option) == Some(d) || u.tcx.adts.get(&d).is_some_and(|adt| adt.is_box()) || matches!(n.as_str(), "Result" | "Vec" | "Rc" | "Arc") {
                let n = if u.tcx.lang_items.get(LangItem::Option) == Some(d) { "Option" } else if u.tcx.adts.get(&d).is_some_and(|adt| adt.is_box()) { "Box" } else { &n };
                // the element types; a Global allocator says nothing of the shape (another one
                // is shaped as itself: never representable)
                let elems: Vec<TyId> = u.tcx.tys.list(args).iter().copied().filter(|x| !crate::provided::is_global_alloc(&u.tcx, *x)).collect();
                let inner: Vec<String> = elems.into_iter().map(|x| shape(u, a, x)).collect();
                format!("{}<{}>", n, inner.join(","))
            } else {
                n
            }
        }
        TyKind::FnPtr(_, _, c, _) => if c.abi.is_c_like() { "extern-C-fn".into() } else { "rust-fn-ptr".into() },
        TyKind::Dyn(..) => "dyn".into(),
        TyKind::Closure(..) | TyKind::FnDef(..) => "fn-item".into(),
        TyKind::Param(_) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, .. }) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. }) => "generic".into(),
        _ => "other".into(),
    }
}

fn c_representable(s: &str) -> bool {
    matches!(s, "scalar" | "unit" | "never" | "&str" | "&vm" | "extern-C-fn")
        || s.starts_with("&[scalar") || s.starts_with("&[vm-struct:") || s.starts_with("&[tuple")
        || (s.starts_with("vm-") && !s.contains("non-Copy"))
        || s.starts_with("Option<vm-") && !s.contains("non-Copy")
        || s.starts_with('&') && !s.starts_with("&dyn") && !s.starts_with("&other")
}

/// Lowers every JIT-side body a run of package `root` reaches from its `main` (a library: from
/// the non-generic fns of the crates JIT code is made of, the package and its dependencies, not
/// entering a provided crate or std: a crate only a provided crate depends on, wayland-client
/// under the HAL, is rustc's), to a fixpoint;
/// then the report and the number of violations (bodies that reach into a provided crate the
/// way JIT-compiled code cannot).
pub fn run(up: *mut Unit, root: u32) -> (String, usize) {
    AUDIT_ON.store(true, std::sync::atomic::Ordering::Relaxed);
    let u = unsafe { &mut *up };
    let std_crates: Vec<u32> = ["core", "alloc", "std"].iter().filter_map(|n| u.prog.crates.iter().position(|c| u.prog.syms.str(c.name) == *n).map(|i| i as u32)).collect();
    // the audited crate(s) and every crate registered as provided (makepad-script's census)
    let mut provided_crates = AUDIT.lock().unwrap().as_ref().unwrap().crates.clone();
    provided_crates.extend(u.provided.iter().map(|(k, _)| *k).filter(|k| !provided_crates.contains(k)).collect::<Vec<_>>());
    let mut jit_crates = vec![false; u.prog.crates.len()];
    let mut stack = vec![root];
    while let Some(c) = stack.pop() {
        if jit_crates[c as usize] || std_crates.contains(&c) || provided_crates.contains(&c) {
            continue;
        }
        jit_crates[c as usize] = true;
        stack.extend(u.prog.crates[c as usize].deps.iter().map(|(_, d)| *d));
    }
    let mut roots = Vec::new();
    for (di, d) in u.prog.defs.iter().enumerate() {
        if !matches!(d.kind, DefKind::Fn | DefKind::AssocFn) || !jit_crates[d.krate as usize] {
            continue;
        }
        let it = u.prog.files[d.file as usize].ast.item(d.item);
        match &it.kind {
            crate::ast::ItemKind::Fn(_, b, _) if b.is_some() => {}
            crate::ast::ItemKind::Fn(s, _, _) if u.prog.files[d.file as usize].ast.fn_sig(*s).lazy_body.is_some() => {}
            _ => continue,
        }
        let in_trait = u.prog.def(d.parent).kind == DefKind::Trait;
        let generic = u.tcx.fn_sig(&u.prog, DefId(di as u32)).map_or(true, |s| s.n_generics > 0);
        if !generic && !in_trait {
            roots.push(DefId(di as u32));
        }
    }
    // a program: what its run reaches, from its `main` (calls, fn pointers, vtables, closures
    // and drop glue are followed to the fixpoint); a library: every fn of the JIT-side crates
    if let Some(main) = crate::run::find_main(&u.prog, root) {
        roots = vec![main];
    }
    let mut failed = 0usize;
    let mut fails: Vec<String> = Vec::new();
    for d in roots {
        u.fn_id(FnKey::Inst(d, Vec::new()));
    }
    let mut done = 0usize;
    let mut lowered = 0usize;
    // the body whose lowering first made each slot: a violation row names the way JIT code
    // got into the provided crate (the app's fn, then each provided fn compiled from source)
    let mut made_by: Vec<u32> = vec![u32::MAX; u.fns.len()];
    loop {
        let n = u.fns.len();
        if done == n {
            break;
        }
        for id in done..n {
            let key = u.fns[id].key.clone();
            let lower_it = match &key {
                FnKey::Inst(d, args) => {
                    let k = u.prog.def(*d).krate;
                    // with the crate registered as provided (provided.rs): its unbound fns are
                    // JIT-side too
                    // registered: bound when it has a shim (as Unit::new binds); else every fn of
                    // the audited crate is taken as rustc's
                    let bound = args.is_empty()
                        && match u.provided.iter().find(|(pk, _)| *pk == k) {
                            Some((_, pc)) => {
                                crate::provided::unbound_reason(&u.prog, &u.tcx, k, *d, &|a| u.provided_types.contains_key(&a)).is_none()
                                    && pc.shims.contains_key(crate::provided::key(&u.prog, &u.tcx, *d).as_str())
                            }
                            None => true,
                        };
                    !std_crates.contains(&k) && !(provided_crates.contains(&k) && bound)
                }
                // closures are lowered from their parent's body (made while lowering it)
                FnKey::Closure(..) => true,
                // drop glue: a provided type's Drop impl (and what it calls) runs from here
                // when JIT-compiled code drops a value of it (a handle's release, a guard)
                FnKey::Glue(..) => true,
                FnKey::Shim(_) | FnKey::Sandbox(_) | FnKey::Plain(_) | FnKey::Ctfe(_) => false,
            };
            if !lower_it {
                continue;
            }
            lowered += 1;
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { crate::jit::lower_for_audit(up, id as u32) }));
            let now = u.fns.len();
            if made_by.len() < now {
                made_by.resize(now, id as u32);
            }
            if !matches!(r, Ok(Ok(()))) {
                failed += 1;
                let msg = match &r {
                    Ok(Err(e)) => e.clone(),
                    Err(p) => format!(
                        "internal compiler error: {}{}",
                        p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default(),
                        crate::jit::take_ice_location()
                    ),
                    Ok(Ok(())) => String::new(),
                };
                fails.push(format!("fail\t{}\t{}", u.fns[id].name, msg.lines().next().unwrap_or("")));

            }
        }
        done = n;
    }
    AUDIT_ON.store(false, std::sync::atomic::Ordering::Relaxed);
    let g = AUDIT.lock().unwrap();
    let a = g.as_ref().unwrap();
    let mut out = String::new();
    use std::fmt::Write;
    let _ = writeln!(out, "# boundary audit: provided crates {}", a.crates.iter().map(|c| u.prog.syms.str(u.prog.crates[*c as usize].name).to_string()).collect::<Vec<_>>().join(", "));
    let _ = writeln!(out, "# lowered {} JIT-side bodies ({} failed to lower)", lowered, failed);
    fails.sort();
    for f in &fails {
        let _ = writeln!(out, "{}", f);
    }
    for row in &a.option_entries {
        let _ = writeln!(out, "{}", row);
    }
    // the audited crates' violations fail the check; another provided crate's are listed
    let crate_name = |k: u32| u.prog.syms.str(u.prog.crates[k as usize].name).to_string();
    let failing = a.violations.iter().filter(|(k, _, _)| a.crates.contains(k)).count();
    let _ = writeln!(out, "# provided-crate violations (JIT-compiled code reaching into a provided crate the way it cannot): {} in the audited crates, {} in others", failing, a.violations.len() - failing);
    for (k, body, e) in &a.violations {
        let _ = writeln!(out, "violation\t{}\t{}\t{}\t{}", if a.crates.contains(k) { "FAIL" } else { "other" }, crate_name(*k), body, e);
        // the way in: from the app's body to this one, each a body compiled from source
        if let Some(mut id) = (0..u.fns.len()).find(|i| *u.fns[*i].name == **body) {
            let mut path = vec![u.fns[id].name.to_string()];
            while let Some(&p) = made_by.get(id).filter(|p| **p != u32::MAX && path.len() < 24) {
                path.push(u.fns[p as usize].name.to_string());
                id = p as usize;
                if !matches!(&u.fns[id].key, FnKey::Inst(d, _) if u.provided.iter().any(|(pk, _)| *pk == u.prog.def(*d).krate)) {
                    break;
                }
            }
            path.reverse();
            let _ = writeln!(out, "  reached\t{}", path.join(" -> "));
        }
    }
    // every `#[relax_api]` op names a fn of this source with the signature rustc built: an entry
    // whose key no fn has (Relax reads the item's path differently, or the source moved on) or
    // whose signature differs is reported here, before any body reaches it
    let mut entry_bad = Vec::new();
    for (k, pc) in u.provided.iter() {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for (di, d) in u.prog.defs.iter().enumerate() {
            if d.krate != *k || !matches!(d.kind, DefKind::Fn | DefKind::AssocFn) {
                continue;
            }
            let key = crate::provided::key(&u.prog, &u.tcx, DefId(di as u32));
            if let Some((ek, sh)) = pc.shims.get_key_value(key.as_str()) {
                seen.insert(ek);
                if !sh.sig.is_empty() {
                    let ours = crate::provided::sig(&u.prog, &u.tcx, DefId(di as u32));
                    if crate::provided::norm(sh.sig) != ours {
                        entry_bad.push(format!("entry-sig\t{}\trustc `{}`\tRelax `{}`", key, sh.sig, ours));
                    }
                }
            }
        }
        for (di, d) in u.prog.defs.iter().enumerate() {
            if d.krate == *k && matches!(d.kind, DefKind::Struct | DefKind::Enum) {
                let path = crate::provided::norm(&u.prog.def_path(DefId(di as u32)));
                if let Some((ek, _)) = pc.types.get_key_value(path.as_str()).map(|(k, v)| (k, v.0)).or_else(|| pc.layouts.get_key_value(path.as_str()).map(|(k, v)| (k, v.0))) {
                    seen.insert(ek);
                }
            }
        }
        for ek in pc.types.keys().chain(pc.layouts.keys()) {
            if !seen.contains(ek.as_str()) {
                entry_bad.push(format!("entry-notype\t{}\t{}", pc.name, ek));
            }
        }
        for (ek, sh) in pc.shims.iter() {
            // a handle's trait ops are keyed by the type (Relax's opaque bridges)
            if sh.sig.is_empty() || seen.contains(ek.as_str()) {
                continue;
            }
            entry_bad.push(format!("entry-nodef\t{}\t{}", pc.name, ek));
        }
    }
    for (_, pc) in u.provided.iter() {
        let _ = writeln!(out, "# relax_api {}: {} ops, {} handles, {} plain", pc.name, pc.shims.len(), pc.types.len(), pc.layouts.len());
    }
    entry_bad.sort();
    let _ = writeln!(out, "# relax_api entries without a matching fn: {}", entry_bad.len());
    for e in &entry_bad {
        let _ = writeln!(out, "{}", e);
    }
    // ops no body of this program reaches (a mark the app could do without)
    let mut unreached: Vec<String> = Vec::new();
    for (_, pc) in u.provided.iter() {
        for (k, sh) in pc.shims.iter() {
            if !sh.sig.is_empty() && !a.bindings.get(k.as_str()).is_some_and(|(b, _)| *b) {
                unreached.push(format!("entry-unreached\t{}\t{}", pc.name, k));
            }
        }
    }
    unreached.sort();
    let _ = writeln!(out, "# relax_api ops no body reaches: {}", unreached.len());
    for e in &unreached {
        let _ = writeln!(out, "{}", e);
    }
    let bound = a.bindings.values().filter(|(b, _)| *b).count();
    let _ = writeln!(out, "# bindings: {} rustc, {} from source, {} total distinct called functions", bound, a.bindings.len() - bound, a.bindings.len());
    for (key, (bound, reason)) in &a.bindings {
        let _ = writeln!(out, "binding\t{}\t{}\t{}", if *bound { "rustc" } else { "source" }, key, reason);
    }
    let mut shapes: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for ((kind, name), e) in &a.rec {
        let mut sig = String::new();
        let mut bad = Vec::new();
        if *kind == "call" {
            if let Some((ps, r)) = a.sigs.get(name) {
                let mut parts = Vec::new();
                for p in ps {
                    let s = shape(u, a, *p);
                    let x = shapes.entry(s.clone()).or_default();
                    x.0 += 1;
                    x.1 += e.sites;
                    if !c_representable(&s) {
                        bad.push(s.clone());
                    }
                    parts.push(format!("{}={}", crate::typeck::ty_to_string(false, &u.prog, &u.tcx, *p), s));
                }
                let rs = shape(u, a, *r);
                let x = shapes.entry(format!("ret {}", rs)).or_default();
                x.0 += 1;
                x.1 += e.sites;
                if !c_representable(&rs) {
                    bad.push(format!("ret {}", rs));
                }
                sig = format!("({}) -> {}={}", parts.join(", "), crate::typeck::ty_to_string(false, &u.prog, &u.tcx, *r), rs);
            }
        }
        if *kind == "static" {
            sig = e.class.clone();
        }
        let _ = writeln!(out, "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}", kind, if bad.is_empty() { "ok" } else { "CLEANUP" }, e.sites, name, sig, bad.join(" "), e.callers.iter().cloned().collect::<Vec<_>>().join(","), e.sample);
    }
    let _ = writeln!(out, "# shapes (distinct fns, call sites):");
    for (s, (n, sites)) in &shapes {
        let _ = writeln!(out, "#   {:<40} {:>5} {:>6} {}", s, n, sites, if c_representable(s.trim_start_matches("ret ")) { "" } else { "CLEANUP" });
    }
    (out, failing + a.option_entries.len())
}

/// `makepad check --boundary statics [--known-refused <tsv>] <package>`: every static of a provided
/// crate that JIT-compiled code can reach, judged as lowering judges it when that code first runs
/// (provided::static_use, lower.rs check_provided_static). The loader's build runs it
/// (tools/run/fixture/loader_build.sh) over a program holding every crate the loader provides: a
/// static that would be refused at run time fails the build with the message the run would give
/// (an unmarked `OnceLock` in the CPU budget passed cargo check and the crate's own tests, then
/// every app's start refused it).
///
/// What JIT code reaches is taken from the crates' source, not from any one app: the bodies
/// Relax compiles from a provided crate's source are its fns that are not bound ops (generic
/// fns, fns without an op mark or that refuse binding: `binding`). JIT code enters them where
/// rustc's effective visibility lets another crate: the fns it can name (`pub` through every
/// module from the crate's root, or a `pub use`), the pub methods and trait impls of the types it
/// can hold (named, or taken or given by a fn it can call, ops' included, or in a pub field of
/// one), and the impls and default bodies of the traits it can name. A `pub` item of a private
/// module is no way in. Each body is read as typeck left it (closures with it), and every fn,
/// const and static it names is followed: a provided crate's non-op fn or free const is read in
/// turn, a static is judged; the trait impls of the types it uses (a drop, a generic call) and of
/// the traits it calls through are entered from it. A handle's drop is the loader's (rustc's
/// drop fn).
/// A `pub` static JIT code can name is reached as it is. An op's body is rustc's: what it
/// reaches is the loader's.
///
/// `registered`: the crates the loader provides (provided::registered_names), each of which the
/// program must bind. `allowed` (path, why): statics the runtime refuses on purpose (the fixture
/// crate's refusal cases). A listed static that is not a refused one JIT code reaches fails as
/// well, so the list keeps to what is true. Returns the report and the number of failures.
/// (Tested with the opaque-front fixture crate: tier_stuck_tests.rs.)
pub fn statics(u: &Unit, registered: &[String], allowed: &[(String, String)], allow_file: &str) -> (String, usize) {
    use crate::ast::{ItemKind, Vis};
    use std::collections::HashSet;
    let prog = &u.prog;
    let provided: Vec<u32> = u.provided.iter().map(|(k, _)| *k).collect();
    use std::fmt::Write;
    let mut rows = String::new();
    let mut failed = 0usize;
    // every crate the loader provides is in the program and bound: a missing one goes unchecked
    for name in registered {
        if !u.provided.iter().any(|(_, p)| p.name == *name) {
            failed += 1;
            let _ = writeln!(rows, "crate\tFAIL\t{name}\tregistered by the loader but not bound in this program: its statics go unchecked (the program depends on every provided crate)");
        }
    }
    // a fn or static its crate keeps for the loader (`#[relax_api(loader = "..")]`): JIT code's use
    // of one is refused where it is made (lower.rs check_loader_only, provided::static_use), so its
    // body is none JIT code compiles; a use from a body JIT code does compile is a failure
    let loader_text = |d: DefId| -> Option<&'static str> {
        let pc = &u.provided.iter().find(|(k, _)| *k == prog.def(d).krate)?.1;
        let key = if prog.def(d).kind == DefKind::Static { crate::provided::norm(&prog.def_path(d)) } else { crate::provided::key(prog, &u.tcx, d) };
        pc.loader.get(key.as_str()).copied()
    };
    let jit_side = |d: DefId| provided.contains(&prog.def(d).krate) && !binding(u, d, &[]).0 && loader_text(d).is_none();
    // the loader's fns a body JIT code compiles calls or names: (fn, that body)
    let mut loader_used: BTreeMap<String, (DefId, DefId)> = BTreeMap::new();
    let has_body = |d: DefId| {
        let def = prog.def(d);
        let ast = &prog.files[def.file as usize].ast;
        match &ast.item(def.item).kind {
            ItemKind::Fn(s, b, _) => b.is_some() || ast.fn_sig(*s).lazy_body.is_some(),
            _ => false,
        }
    };
    let in_provided = |d: DefId| (d.0 as usize) < prog.defs.len() && provided.contains(&prog.def(d).krate);
    // an item's own `pub` (a module's from its `mod` item; `pub(crate)` and narrower are not
    // another crate's)
    let item_pub = |d: DefId| {
        let def = prog.def(d);
        let it = prog.files[def.file as usize].ast.item(def.item);
        if def.kind == DefKind::Mod && !matches!(it.kind, ItemKind::Mod(_)) {
            return def.is_pub;
        }
        matches!(it.vis, Vis::Pub)
    };
    // What JIT code can name, as rustc's effective visibility has it: from each provided crate's
    // root module through its `pub` items, its `pub use` re-exports and `pub use ..::*` globs. A
    // `pub` item of a private module is not reached that way.
    let pub_imports: HashSet<(u32, u32)> = prog.imports_iter()
        .filter(|im| !im.glob && prog.import_vis(im).is_none())
        .filter_map(|im| Some((im.module, im.bind?)))
        .collect();
    let mut nameable: Vec<DefId> = Vec::new();
    let mut mods_seen: HashSet<u32> = HashSet::new();
    let mut mq: Vec<u32> = provided.iter().map(|&k| prog.crates[k as usize].root_mod).collect();
    while let Some(m) = mq.pop() {
        if !mods_seen.insert(m) {
            continue;
        }
        let md = &prog.mods[m as usize];
        for (ns_type, map) in [(true, &md.types), (false, &md.values)] {
            for (&name, &d) in map {
                let public = if md.import_pub.contains_key(&(name, ns_type)) { pub_imports.contains(&(m, name)) } else { item_pub(d) };
                if !public || !in_provided(d) {
                    continue;
                }
                nameable.push(d);
                if prog.def(d).kind == DefKind::Mod {
                    mq.push(prog.def(d).sub);
                }
            }
        }
        for &(t, vis) in &md.globs {
            if vis.is_none() && in_provided(t) && prog.def(t).kind == DefKind::Mod {
                mq.push(prog.def(t).sub);
            }
        }
    }
    // the provided ADTs (and traits, of a `dyn`) a type holds
    fn adts_in(u: &Unit, t: TyId, out: &mut Vec<DefId>) {
        match u.tcx.tys.kind(t).clone() {
            TyKind::Adt(d, args, _) => {
                out.push(d);
                for &a in u.tcx.tys.list(args).iter() { adts_in(u, a, out); }
            }
            TyKind::Tuple(v) | TyKind::Alias(_, AliasTy { args: v, .. }) => {
                for &a in u.tcx.tys.list(v).iter() { adts_in(u, a, out); }
            }
            TyKind::Array(e, _) | TyKind::Slice(e) | TyKind::Ref(_, e, _) | TyKind::Ptr(_, e) => adts_in(u, e, out),
            TyKind::FnPtr(ps, r, _, _) => {
                for &a in u.tcx.tys.list(ps).iter() { adts_in(u, a, out); }
                adts_in(u, r, out);
            }
            TyKind::Dyn(tr, args, ..) => {
                out.push(tr);
                for &a in u.tcx.tys.list(args).iter() { adts_in(u, a, out); }
            }
            _ => {}
        }
    }
    // the impls of the provided crates, by their self type's ADT (None: it holds none of theirs,
    // `impl Trait for u32`); a handle's Drop is the loader's (its rustc drop fn: provided types)
    let drop_trait = u.tcx.lang_items.get(LangItem::Drop);
    let handle = |d: DefId| u.provided.iter().any(|(k, p)| *k == prog.def(d).krate && p.types.contains_key(crate::provided::norm(&prog.def_path(d)).as_str()));
    let mut impls: Vec<(Option<DefId>, Option<DefId>, &[DefId])> = Vec::new();
    for im in prog.impls.iter() {
        if !in_provided(im.def) {
            continue;
        }
        let Some(&st) = u.tcx.impl_self.get(&im.def) else { continue };
        let mut st = st;
        while let TyKind::Ref(_, x, _) = u.tcx.tys.kind(st).clone() { st = x; }
        let adt = match u.tcx.tys.kind(st).clone() { TyKind::Adt(d, ..) if in_provided(d) => Some(d), _ => None };
        let tr = u.tcx.impl_trait.get(&im.def).copied();
        if tr.is_some() && tr == drop_trait && adt.is_some_and(handle) {
            continue;
        }
        impls.push((adt, tr, &im.items));
    }
    // impls by what enters them: their self type's ADT, their trait; a trait's own fns (the
    // default bodies)
    let mut by_adt: std::collections::HashMap<DefId, Vec<usize>> = std::collections::HashMap::new();
    let mut by_trait: std::collections::HashMap<DefId, Vec<usize>> = std::collections::HashMap::new();
    let mut recheck: Vec<usize> = Vec::new();
    for (i, &(adt, tr, _)) in impls.iter().enumerate() {
        match adt { Some(a) => by_adt.entry(a).or_default().push(i), None => recheck.push(i) }
        if let Some(t) = tr.filter(|&t| in_provided(t)) { by_trait.entry(t).or_default().push(i); }
    }
    let mut trait_fns: std::collections::HashMap<DefId, Vec<DefId>> = std::collections::HashMap::new();
    for (di, def) in prog.defs.iter().enumerate() {
        if def.kind == DefKind::AssocFn && provided.contains(&def.krate) && prog.def(def.parent).kind == DefKind::Trait {
            trait_fns.entry(def.parent).or_default().push(DefId(di as u32));
        }
    }
    let mut taken = vec![false; impls.len()];
    // the body (or the name) each def was first reached from: the way in, for a failure's row
    let mut from: std::collections::HashMap<DefId, Option<DefId>> = std::collections::HashMap::new();
    let mut queue: Vec<DefId> = Vec::new();
    let mut reached_statics: BTreeMap<String, DefId> = BTreeMap::new();
    // Types JIT code can hold (it names them, a fn it can call takes or gives them, or a pub field
    // of one holds them): their pub methods and their trait impls are entered. Types only the
    // JIT-compiled bodies use: their trait impls are (a drop, a generic call), from that body.
    // Traits JIT code can name, hold as `dyn` or the bodies call through: their impls' methods
    // and their default bodies are.
    let mut live: HashSet<DefId> = HashSet::new();
    let mut used: std::collections::HashMap<DefId, Option<DefId>> = std::collections::HashMap::new();
    let mut live_traits: HashSet<DefId> = HashSet::new();
    let mut callable: Vec<(DefId, Option<DefId>)> = Vec::new();
    let mut sigs_read: HashSet<DefId> = HashSet::new();
    let mut new_live: Vec<DefId> = Vec::new();
    let mut new_used: Vec<(DefId, Option<DefId>)> = Vec::new();
    let mut new_traits: Vec<DefId> = Vec::new();
    // why JIT code holds a type or trait (the first way found), and why it can call a method of
    // an impl entered from one: a row whose way in starts at such a method names it
    let mut hold_why: std::collections::HashMap<DefId, String> = std::collections::HashMap::new();
    let mut method_why: std::collections::HashMap<DefId, String> = std::collections::HashMap::new();
    // a trait a JIT-compiled body calls through: that body, the way in to the trait's fns
    let mut trait_from: std::collections::HashMap<DefId, DefId> = std::collections::HashMap::new();
    for &d in &nameable {
        match prog.def(d).kind {
            DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::Trait => {
                hold_why.entry(d).or_insert_with(|| "it is named".to_string());
                if prog.def(d).kind == DefKind::Trait { new_traits.push(d) } else { new_live.push(d) }
            }
            DefKind::Fn => callable.push((d, None)),
            DefKind::Const if !from.contains_key(&d) => {
                from.insert(d, None);
                queue.push(d);
            }
            DefKind::Static => {
                from.insert(d, None);
                reached_statics.insert(prog.def_path(d), d);
            }
            _ => {}
        }
    }
    let mut read = 0usize;
    let mut unread: Vec<String> = Vec::new();
    loop {
        while !(callable.is_empty() && new_live.is_empty() && new_used.is_empty() && new_traits.is_empty() && recheck.is_empty()) {
            // a fn JIT code can call: the types it takes or gives are JIT code's; its body is read.
            // One kept for the loader gives JIT code nothing: its call is refused where it is made.
            while let Some((f, why)) = callable.pop() {
                if sigs_read.insert(f) && loader_text(f).is_none() {
                    if let Some(sig) = u.tcx.fn_sig(prog, f) {
                        let mut out = Vec::new();
                        for &t in sig.params.iter().chain(std::iter::once(&sig.ret)) { adts_in(u, t, &mut out); }
                        for &d in out.iter().filter(|&&d| in_provided(d)) {
                            hold_why.entry(d).or_insert_with(|| format!("the signature of `{}`", prog.def_path(f)));
                        }
                        new_live.extend(out.into_iter().filter(|&d| in_provided(d)));
                    }
                }
                if !from.contains_key(&f) && has_body(f) && jit_side(f) {
                    from.insert(f, why);
                    queue.push(f);
                }
            }
            while let Some(t) = new_live.pop() {
                if prog.def(t).kind == DefKind::Trait {
                    new_traits.push(t);
                } else if live.insert(t) {
                    if let Some(a) = u.tcx.adts.get(&t) {
                        let mut out = Vec::new();
                        for v in &a.variants {
                            for fl in &v.fields {
                                if a.is_enum || fl.vis_mod == u32::MAX { adts_in(u, fl.ty, &mut out); }
                            }
                        }
                        for &d in out.iter().filter(|&&d| in_provided(d)) {
                            hold_why.entry(d).or_insert_with(|| format!("a pub field of `{}`", prog.def_path(t)));
                        }
                        new_live.extend(out.into_iter().filter(|&d| in_provided(d)));
                    }
                    recheck.extend(by_adt.get(&t).into_iter().flatten().copied());
                }
            }
            while let Some((t, why)) = new_used.pop() {
                if prog.def(t).kind == DefKind::Trait {
                    new_traits.push(t);
                } else if !used.contains_key(&t) {
                    used.insert(t, why);
                    recheck.extend(by_adt.get(&t).into_iter().flatten().copied());
                }
            }
            while let Some(t) = new_traits.pop() {
                if live_traits.insert(t) {
                    let via = trait_from.get(&t).copied();
                    callable.extend(trait_fns.get(&t).into_iter().flatten().map(|&f| (f, via)));
                    recheck.extend(by_trait.get(&t).into_iter().flatten().copied());
                }
            }
            // an impl is entered: an inherent one's pub methods for a type JIT code holds; a trait
            // impl's methods for a type it holds or a body uses (or none of the crates'), of a
            // trait not the crates' or one JIT code reaches
            while let Some(i) = recheck.pop() {
                let (adt, tr, items) = impls[i];
                if taken[i] || tr.is_some_and(|t| in_provided(t) && !live_traits.contains(&t)) {
                    continue;
                }
                let why = match (adt, tr) {
                    (Some(a), _) if live.contains(&a) => None,
                    (Some(a), Some(_)) => match used.get(&a) { Some(&w) => w, None => continue },
                    (None, Some(_)) => None,
                    _ => continue,
                };
                taken[i] = true;
                let held = match (adt, tr) {
                    (Some(a), _) if live.contains(&a) => Some(a),
                    (_, Some(t)) if live_traits.contains(&t) => Some(t),
                    _ => None,
                };
                for &f in items {
                    if prog.def(f).kind == DefKind::AssocFn && (tr.is_some() || item_pub(f)) {
                        if let Some(h) = held {
                            method_why.entry(f).or_insert_with(|| format!("JIT code holds `{}`: {}", prog.def_path(h), hold_why.get(&h).map_or("?", |s| s.as_str())));
                        }
                        callable.push((f, why));
                    }
                }
            }
        }
        let Some(d) = queue.pop() else { break };
        read += 1;
        let body = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| u.tcx.typeck(prog, d))) {
            Ok(b) => b,
            Err(p) => {
                unread.push(format!("{}\t{}", prog.def_path(d), p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default().lines().next().unwrap_or("")));
                continue;
            }
        };
        // the types the body uses
        let mut out = Vec::new();
        for &t in body.expr_ty.iter().chain(body.pat_ty.iter()) { adts_in(u, t, &mut out); }
        new_used.extend(out.into_iter().filter(|&t| in_provided(t) && !used.contains_key(&t)).map(|t| (t, Some(d))));
        let named = body.res.values().chain(body.pat_res.values()).chain(body.fmt_named.values())
            .filter_map(|r| if let crate::typeck::Res::Def(x) = r { Some(*x) } else { None })
            .chain(body.methods.values().chain(body.binops.values()).map(|m| m.def));
        for x in named {
            if !in_provided(x) || from.contains_key(&x) {
                continue;
            }
            match prog.def(x).kind {
                DefKind::Static => {
                    from.insert(x, Some(d));
                    reached_statics.insert(prog.def_path(x), x);
                }
                DefKind::Fn | DefKind::AssocFn => {
                    // a call through a trait's fn: the trait's impls are entered
                    let parent = prog.def(x).parent;
                    if prog.def(x).kind == DefKind::AssocFn && prog.def(parent).kind == DefKind::Trait && !live_traits.contains(&parent) {
                        trait_from.entry(parent).or_insert(d);
                        new_traits.push(parent);
                    }
                    if loader_text(x).is_some() {
                        loader_used.entry(prog.def_path(x)).or_insert((x, d));
                    }
                    if has_body(x) && jit_side(x) {
                        from.insert(x, Some(d));
                        queue.push(x);
                    }
                }
                DefKind::Const => {
                    from.insert(x, Some(d));
                    queue.push(x);
                }
                _ => {}
            }
        }
    }
    let way_in = |s: DefId| {
        let mut path = Vec::new();
        let mut root = None;
        let mut at = from.get(&s).copied().flatten();
        while let Some(d) = at.filter(|_| path.len() < 24) {
            path.push(prog.def_path(d));
            root = Some(d);
            at = from.get(&d).copied().flatten();
        }
        path.reverse();
        let why = root.and_then(|r| method_why.get(&r)).map_or(String::new(), |w| format!(" [{w}]"));
        if path.is_empty() { "pub: JIT code names it".to_string() } else { format!("{}{why}", path.join(" -> ")) }
    };
    let mut refused = 0usize;
    let mut kept = 0usize;
    let mut listed_seen: Vec<&str> = Vec::new();
    for (path, &(x, body)) in &loader_used {
        failed += 1;
        let _ = writeln!(rows, "loader\tFAIL\t{path}\tkept for the loader ({}), but a body JIT code compiles uses it: make that body an op or keep it for the loader too", loader_text(x).unwrap_or(""));
        let _ = writeln!(rows, "  reached\t{}", if from.contains_key(&body) { format!("{} -> {}", way_in(body), prog.def_path(body)) } else { prog.def_path(body) });
    }
    for (path, &s) in &reached_statics {
        let Some(t) = u.tcx.const_ty(prog, s) else { continue };
        let crate::provided::StaticUse::Refused(msg) = crate::provided::static_use(u, s, t) else { continue };
        refused += 1;
        // a static kept for the loader that JIT code can name: its use is refused with its text
        if loader_text(s).is_some() && from.get(&s).copied().flatten().is_none() {
            kept += 1;
            let _ = writeln!(rows, "static\tloader\t{path}");
            continue;
        }
        if let Some((p, why)) = allowed.iter().find(|(p, _)| p == path) {
            listed_seen.push(p);
            let _ = writeln!(rows, "static\tallowed\t{path}\t{why}");
            continue;
        }
        failed += 1;
        let _ = writeln!(rows, "static\tFAIL\t{path}\t{msg}");
        let _ = writeln!(rows, "  reached\t{}", way_in(s));
    }
    for (p, _) in allowed {
        if !listed_seen.contains(&p.as_str()) {
            failed += 1;
            let _ = writeln!(rows, "allow\tFAIL\t{p}\tlisted in {allow_file}, but it is no static the runtime refuses that JIT code reaches: drop the row");
        }
    }
    for e in &unread {
        failed += 1;
        let _ = writeln!(rows, "body\tFAIL\t{e}\t(its statics go unchecked)");
    }
    // every loader mark names a fn or static of the crate's source: a stale one keeps nothing back
    for (k, pc) in u.provided.iter() {
        let mut named: HashSet<String> = HashSet::new();
        for (di, d) in prog.defs.iter().enumerate() {
            if d.krate != *k {
                continue;
            }
            match d.kind {
                DefKind::Fn | DefKind::AssocFn => { named.insert(crate::provided::key(prog, &u.tcx, DefId(di as u32))); }
                DefKind::Static => { named.insert(crate::provided::norm(&prog.def_path(DefId(di as u32)))); }
                _ => {}
            }
        }
        let mut stale: Vec<&String> = pc.loader.keys().filter(|key| !named.contains(key.as_str())).collect();
        stale.sort();
        for key in stale {
            failed += 1;
            let _ = writeln!(rows, "loader\tFAIL\t{key}\tmarked #[relax_api(loader)] in {}, but no fn or static of this source has that path: the mark keeps nothing back", pc.name);
        }
    }
    let names: Vec<&str> = u.provided.iter().map(|(_, p)| p.name.as_str()).collect();
    let mut out = String::new();
    let loader_items: usize = u.provided.iter().map(|(_, p)| p.loader.len()).sum();
    let _ = writeln!(out, "# provided statics: {} provided crates ({}), {} bodies JIT code can compile read, {} statics reached, {} refused ({} allowed, {} kept for the loader; {} items kept for the loader): {} failures", names.len(), names.join(", "), read, reached_statics.len(), refused, listed_seen.len(), kept, loader_items, failed);
    out.push_str(&rows);
    (out, failed)
}

/// The `--known-refused` list: `path<TAB>why` rows, `#` comments and blank lines skipped.
pub fn read_allowed(path: &str) -> Result<Vec<(String, String)>, String> {
    // through the vfs, as the program's sources are read (by absolute path, a relative one from the
    // workspace's root): a lane's overlay holds its edit of the list
    let at = std::path::Path::new(&crate::dirs::workspace_dir()).join(path);
    let text = crate::vfs::read_to_string(&at).map_err(|e| format!("--known-refused {}: {e}", at.display()))?;
    let mut rows = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_once('\t') {
            Some((p, why)) if !p.trim().is_empty() && !why.trim().is_empty() => rows.push((p.trim().to_string(), why.trim().to_string())),
            _ => return Err(format!("--known-refused {path}:{}: a row is `path<TAB>why`", i + 1)),
        }
    }
    Ok(rows)
}

