//! Boundary audit: what code compiled by Relax reaches inside a set of crates that would be
//! built by rustc into the embedder ("provided crates", local/plans/loader-core.md section 8).
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
//! when the code that reaches it first runs (Stage's deck reached `shell::Shared::global`
//! through the unmarked `makepad_hal::screens`); the gates run the audit over the apps.

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
}

#[derive(Default)]
struct Audit {
    crates: Vec<u32>,
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

/// Starts an audit of the named crates (before lowering anything); `all`: every provided crate
/// the loader binds in this program.
pub fn start(u: &Unit, names: &[String]) -> Result<(), String> {
    let mut crates = Vec::new();
    if names.len() == 1 && names[0] == "all" {
        crates.extend(u.provided.iter().map(|(k, _)| *k));
        if crates.is_empty() {
            return Err("--boundary all: no provided crate binds in this program (is the loader built from this source?)".into());
        }
    }
    for n in names.iter().filter(|n| n.as_str() != "all") {
        let n2 = n.replace('-', "_");
        match u.prog.crates.iter().position(|c| u.prog.syms.str(c.name) == n2) {
            Some(i) => crates.push(i as u32),
            None => return Err(format!("--boundary: no crate `{}` in the graph", n)),
        }
    }
    *AUDIT.lock().unwrap() = Some(Audit { crates, ..Default::default() });
    Ok(())
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
    let key = u.fns[callee as usize].key.clone();
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

/// A static `d` read or borrowed in the body of (`def`, `args`).
pub fn note_static(u: &Unit, def: DefId, args: &[TyId], d: DefId) {
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
                let inner: Vec<String> = u.tcx.tys.list(args).to_vec().into_iter().map(|x| shape(u, a, x)).collect();
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

/// Lowers every JIT-side body reachable from the non-generic fns of crates other than the
/// provided ones and std, to a fixpoint; then the report and the number of violations (bodies
/// that reach into a provided crate the way JIT-compiled code cannot).
pub fn run(up: *mut Unit) -> (String, usize) {
    AUDIT_ON.store(true, std::sync::atomic::Ordering::Relaxed);
    let u = unsafe { &mut *up };
    let std_crates: Vec<u32> = ["core", "alloc", "std"].iter().filter_map(|n| u.prog.crates.iter().position(|c| u.prog.syms.str(c.name) == *n).map(|i| i as u32)).collect();
    // the audited crate(s) and every crate registered as provided (makepad-script's census)
    let mut provided_crates = AUDIT.lock().unwrap().as_ref().unwrap().crates.clone();
    provided_crates.extend(u.provided.iter().map(|(k, _)| *k).filter(|k| !provided_crates.contains(k)).collect::<Vec<_>>());
    let mut roots = Vec::new();
    for (di, d) in u.prog.defs.iter().enumerate() {
        if !matches!(d.kind, DefKind::Fn | DefKind::AssocFn) || std_crates.contains(&d.krate) || provided_crates.contains(&d.krate) {
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
    let mut failed = 0usize;
    let mut fails: Vec<String> = Vec::new();
    for d in roots {
        u.fn_id(FnKey::Inst(d, Vec::new()));
    }
    let mut done = 0usize;
    let mut lowered = 0usize;
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
                FnKey::Shim(_) | FnKey::Sandbox(_) | FnKey::Plain(_) => false,
            };
            if !lower_it {
                continue;
            }
            lowered += 1;
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { crate::jit::lower_for_audit(up, id as u32) }));
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
    // the audited crates' violations fail the check; another provided crate's are listed
    let crate_name = |k: u32| u.prog.syms.str(u.prog.crates[k as usize].name).to_string();
    let failing = a.violations.iter().filter(|(k, _, _)| a.crates.contains(k)).count();
    let _ = writeln!(out, "# provided-crate violations (JIT-compiled code reaching into a provided crate the way it cannot): {} in the audited crates, {} in others", failing, a.violations.len() - failing);
    for (k, body, e) in &a.violations {
        let _ = writeln!(out, "violation\t{}\t{}\t{}\t{}", if a.crates.contains(k) { "FAIL" } else { "other" }, crate_name(*k), body, e);
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
        let _ = writeln!(out, "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}", kind, if bad.is_empty() { "ok" } else { "CLEANUP" }, e.sites, name, sig, bad.join(" "), e.callers.iter().cloned().collect::<Vec<_>>().join(","), e.sample);
    }
    let _ = writeln!(out, "# shapes (distinct fns, call sites):");
    for (s, (n, sites)) in &shapes {
        let _ = writeln!(out, "#   {:<40} {:>5} {:>6} {}", s, n, sites, if c_representable(s.trim_start_matches("ret ")) { "" } else { "CLEANUP" });
    }
    (out, failing)
}
