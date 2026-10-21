
use crate::jit::{self, FnKey, Unit};
use crate::program::{DefId, DefKind};
use crate::types::TyKind;
use std::sync::Arc;

struct Module {
    name: String,
    krate: u32,
    /// the module's root file as it runs (hot reload makes new versions)
    file: u32,
    path: String,
    defs: Vec<DefId>,
    gen: u32,
    alive: bool,
    /// sandboxed: its arena and its host wrapper crate (exports are the wrapper's fns)
    arena: Option<Arc<crate::sandbox::Arena>>,
    wrapper: Option<u32>,
    /// Wrappers generated on demand for exports added by streaming.
    extra_exports: Vec<(String, u32)>,
    /// exports the wrapper could not offer, with why
    skipped: Vec<String>,
}

static MODULES: relax_session::sync::Mutex<Vec<Module>> = relax_session::sync::Mutex::new(Vec::new());

crate::gen_local! {
    static LAST_ERROR: std::cell::RefCell<String>;
}

fn set_error(e: String) {
    LAST_ERROR.with(|l| *l.borrow_mut() = e);
}

fn handle(idx: usize, gen: u32) -> u64 {
    ((gen as u64) << 32) | (idx as u64 + 1)
}

fn unhandle(h: u64) -> Option<(usize, u32)> {
    let idx = (h & 0xffff_ffff) as usize;
    if idx == 0 {
        return None;
    }
    Some((idx - 1, (h >> 32) as u32))
}

unsafe fn bytes<'a>(p: *const u8, n: usize) -> &'a [u8] {
    if p.is_null() || n == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(p, n)
    }
}

/// Std paths a sandboxed module may not reach (hotreload.md §12.2): OS access, threads, raw
/// memory. Everything else of core/alloc/std is allowed; the running program's crates the host
/// names as dependencies are its API.
const DENIED: &[&str] = &[
    "std::fs", "std::net", "std::process", "std::thread", "std::env", "std::os", "std::io", "std::ffi", "std::alloc", "alloc::alloc",
    "core::ptr", "std::ptr", "core::intrinsics", "core::arch", "std::arch", "core::mem::transmute", "std::mem::transmute",
    "core::mem::transmute_copy", "std::panic::set_hook", "std::panic::take_hook", "std::time::Instant", "std::time::SystemTime",
    "std::sync", "core::sync", "alloc::sync", "std::relax", "core::hint::unreachable_unchecked", "core::slice::from_raw_parts",
    "core::str::from_utf8_unchecked", "core::cell::UnsafeCell",
];

/// The sandbox's compile-time refusal (§12.2) over a loaded module: syntax that escapes the
/// language's safety (after macro expansion: the trees are expanded) and std paths outside
/// the allowed surface (resolved, never by spelling). One line per violation.
pub(crate) fn sandbox_check(prog: &crate::program::Program, tcx: &crate::tcx::Tcx, krate: u32, bodies: &[&crate::typeck::Body]) -> Vec<String> {
    use crate::ast::{ExprKind, ItemKind, Ty};
    let mut out = Vec::new();
    let at = |f: u32, pos: u32| -> String {
        let (l, c) = prog.files[f as usize].line_col(pos);
        format!("{}:{}:{}", prog.file_paths[f as usize], l, c)
    };
    // The running versions only. Do not rescan all defs for every retired stream prefix.
    // Retired and refused definitions remain in `defs` for old frames; their source must
    // not poison a later valid edit. Module records track the currently installed files.
    let files: std::collections::BTreeSet<u32> = prog.mods.iter().filter(|m| m.krate == krate).map(|m| m.file).collect();
    for &f in &files {
        let pf = &prog.files[f as usize];
        let ast = &pf.ast;
        let tok = |i: u32| -> &[u8] { pf.toks.get(i as usize).map_or(&[][..], |t| &pf.src[t.lo as usize..t.hi as usize]) };
        for it in &ast.items {
            for a in ast.list(it.attrs) {
                if a.toks.hi > a.toks.lo {
                    let n = tok(a.toks.lo);
                    if matches!(n, b"no_mangle" | b"export_name" | b"link" | b"link_section" | b"used" | b"naked" | b"link_name") {
                        out.push(format!("{}: `#[{}]` is not allowed in a sandboxed module -- remove it", at(f, it.lo), String::from_utf8_lossy(n)));
                    }
                }
            }
            let bad = match &it.kind {
                ItemKind::Fn(sig, _, _) if ast.fn_sig(*sig).unsafe_ => Some("`unsafe fn`"),
                ItemKind::Fn(sig, _, _) if ast.fn_sig(*sig).abi.is_some() => Some("an `extern` fn"),
                ItemKind::Impl(r) if ast.row(*r).unsafe_ => Some("`unsafe impl`"),
                ItemKind::Trait(r) if ast.row(*r).unsafe_ => Some("`unsafe trait`"),
                ItemKind::ForeignMod(..) => Some("an `extern` block"),
                ItemKind::Static(true, _, _) => Some("`static mut`"),
                ItemKind::Union(..) => Some("a `union`"),
                ItemKind::MacroRules(_) => None,
                _ => None,
            };
            if let Some(b) = bad {
                out.push(format!("{}: {} is not allowed in a sandboxed module -- use safe code and the host's API", at(f, it.lo), b));
            }
        }
        for e in &ast.exprs {
            match &e.kind {
                ExprKind::Unsafe(_) => out.push(format!("{}: `unsafe` is not allowed in a sandboxed module -- use safe code and the host's API", at(f, e.lo))),
                ExprKind::Mac(m) if ast.row(*m).args != crate::ast::NO_MAC_ARGS && ast.mac_args.get(ast.row(*m).args as usize).map_or(false, |a| a.asm.is_some()) => {
                    out.push(format!("{}: `asm!` is not allowed in a sandboxed module", at(f, e.lo)))
                }
                _ => {}
            }
        }
        for t in &ast.tys {
            if let Ty::Ptr(..) = t {
                out.push(format!("{}: raw pointers are not allowed in a sandboxed module -- use references", at(f, 0)));
                break;
            }
        }
        // built-in macros that read the host's files or environment (expanded before the tree)
        for (i, t) in pf.toks.iter().enumerate() {
            let w = &pf.src[t.lo as usize..t.hi as usize];
            if matches!(w, b"include_bytes" | b"include_str" | b"include" | b"env" | b"option_env" | b"asm" | b"global_asm") && tok(i as u32 + 1) == b"!" {
                out.push(format!("{}: `{}!` is not allowed in a sandboxed module", at(f, t.lo), String::from_utf8_lossy(w)));
            }
        }
    }
    // resolved uses of std outside the allowed surface
    let std_crates: Vec<u32> = (0..prog.crates.len() as u32).filter(|&c| matches!(prog.syms.str(prog.crates[c as usize].name), "core" | "alloc" | "std" | "proc_macro")).collect();
    for b in bodies {
        let mut uses: Vec<(u32, DefId)> = Vec::new();
        for (k, r) in &b.res {
            if let crate::typeck::Res::Def(d) = r {
                uses.push((*k, *d));
            }
        }
        for (k, m) in &b.methods {
            uses.push((*k, m.def));
        }
        for (k, d) in uses {
            if (d.0 as usize) >= prog.defs.len() || !std_crates.contains(&prog.def(d).krate) {
                continue;
            }
            // an associated item is named through its impl's self type (`std::time::Instant::now`)
            let def = prog.def(d);
            let path = if def.parent.0 != u32::MAX && (def.parent.0 as usize) < prog.defs.len() && prog.def(def.parent).kind == DefKind::Impl {
                match tcx.impl_self.get(&def.parent) {
                    Some(&st) => format!("{}::{}", crate::typeck::type_name(prog, tcx, st), prog.syms.str(def.name)),
                    None => prog.def_path(d),
                }
            } else {
                prog.def_path(d)
            };
            if path == "std::relax::Host" { continue; }
            if let Some(p) = DENIED.iter().find(|p| path == **p || path.starts_with(&format!("{}::", p))) {
                let pos = if (k as usize) < prog.files[b.file as usize].ast.exprs.len() { prog.files[b.file as usize].ast.exprs[k as usize].lo } else { 0 };
                let line = format!("{}: `{}` is not available to sandboxed modules ({}) -- use the host's API", at(b.file, pos), path, p);
                if !out.contains(&line) {
                    out.push(line);
                }
            }
        }
    }
    out
}

/// A crate to add to the running program.
pub(crate) struct NewCrate {
    pub name: String,
    pub path: String,
    pub edition: u16,
    pub cfg: crate::cfg::CfgSet,
    /// (extern name, dependency)
    pub deps: Vec<(String, Dep)>,
    pub proc_macro: bool,
}

/// What a new crate depends on.
#[derive(Clone, Copy)]
pub(crate) enum Dep {
    /// a crate of the program (std crates included)
    Existing(u32),
    /// a crate of the same batch (an earlier one)
    New(usize),
}

/// What `add_crates` put in the program.
pub(crate) struct Added {
    /// crate index of each new crate, in batch order
    pub crates: Vec<u32>,
    pub defs: Vec<DefId>,
    /// the program's counts before the load
    pub d0: usize,
    pub e0: usize,
    pub f0: usize,
    /// diagnostics of the new crates
    pub errs: Vec<String>,
}

/// Loads `new` (in order) into the running program on the owner thread: parse and collect each
/// crate, resolve the imports it added, collect its declarations. Macros expand as for any load
/// (the macro world builds on the first need). The program stays consistent on an error in the
/// load itself (`Err`); diagnostics of the new files come back in `Added::errs` for the caller to
/// refuse the load or not. `tolerant` (an app's crates: a boot goes on after unresolved imports):
/// the declarations are collected whatever the diagnostics.
pub(crate) unsafe fn add_crates(up: *mut Unit, new: &[NewCrate], tolerant: bool, domain: Option<crate::domain::DomainId>) -> Result<Added, Vec<String>> {
    let u = &mut *up;
    let tr = std::env::var_os("MAKEPAD_RUN_MODULE_TRACE").is_some();
    let mut marks: Vec<(&str, std::time::Instant)> = vec![("", std::time::Instant::now())];
    let d0 = u.prog.defs.len();
    let i0 = u.prog.imports.len();
    let e0 = u.prog.errors.len();
    let f0 = u.prog.files.len();
    let mut crates: Vec<u32> = Vec::new();
    {
        let _fw = jit::front_write();
        crate::run::preserve_collected_boot(u);
        u.tcx.collect_epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // parsed on this thread (no parse workers for one small crate)
        crate::preload::SINGLE.with(|s| s.set(true));
        let mut failed = None;
        for (done, nc) in new.iter().enumerate() {
            if domain.is_some() { crate::run_state::progress(domain, crate::run_state::Phase::Parse,
                Some(done as u64), Some(new.len() as u64), &nc.name); }
            let mut dep_list = Vec::new();
            for (n, d) in &nc.deps {
                let k = match d {
                    Dep::Existing(k) => *k,
                    Dep::New(i) => crates[*i],
                };
                dep_list.push((u.prog.syms.intern(n), k));
            }
            let first = u.prog.crates.len();
            let loaded = u.prog.load_crate_deferred(&nc.name, &nc.path, nc.edition, nc.cfg.clone(), dep_list);
            for k in first..u.prog.crates.len() {
                u.prog.crates[k].proc_macro = nc.proc_macro;
                if up == crate::run::running_unit() { jit::zone_swap::remember_runtime(&u.prog, k as u32); }
            }
            match loaded {
                Ok(k) => {
                    crates.push(k);
                }
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        crate::preload::SINGLE.with(|s| s.set(false));
        marks.push(("parse", std::time::Instant::now()));
        if let Some(e) = failed {
            u.prog.reclaim_caches();
            return Err(vec![e]);
        }
        u.prog.resolve_imports_added(i0, d0);
        marks.push(("resolve", std::time::Instant::now()));
        // the caches resolve_imports cleared (front-end write lock on owner thread: no reader)
        u.prog.reclaim_caches();
        marks.push(("reclaim", std::time::Instant::now()));
    }
    let defs: Vec<DefId> = (d0 as u32..u.prog.defs.len() as u32).map(DefId).collect();
    // resolve_imports retries every import still open in the running program and reports the
    // ones that stay open again: the program's own (e.g. items of a derive that failed in the
    // host) are not the module's errors. Only diagnostics in the module's files refuse it.
    let mine: Vec<String> = (f0..u.prog.files.len()).map(|f| u.prog.file_paths[f].clone()).collect();
    let mut errs: Vec<String> = u.prog.errors[e0..].iter().filter(|e| mine.iter().any(|p| e.starts_with(p.as_str()))).cloned().collect();
    u.prog.errors.truncate(e0);
    u.prog.errors.extend(errs.iter().cloned());
    // imports of the module that did not resolve
    for im in u.prog.imports.iter().skip(i0) {
        if !im.done {
            let (l, c) = u.prog.files[im.file as usize].line_col(im.pos);
            errs.push(format!("{}:{}:{}: unresolved import -- name a crate the module depends on", u.prog.file_paths[im.file as usize], l, c));
        }
    }
    if let Some(id) = domain {
        // Observe cancellation between loading and collection, but finish collecting what is
        // already in the shared program. Leaving half-published crates would corrupt later
        // starts and concurrent lazy compilation. The caller records them, then stops.
        let _ = crate::domain::compile_phase(id, "collect");
    }
    if errs.is_empty() || tolerant {
        let _fw = jit::front_write();
        // collect_added also drains diagnostics queued by background checks of the host.
        // As for resolve_imports above, only errors in this module's files refuse its load.
        errs.extend(u.tcx.collect_added(&u.prog, &defs, true).into_iter().filter(|e| mine.iter().any(|p| e.starts_with(p.as_str()))));
    }
    marks.push(("collect", std::time::Instant::now()));
    if tr {
        let mut line = format!("makepad-run module: {} crate(s) front:", new.len());
        for w in marks.windows(2) {
            line.push_str(&format!(" {} {:.2}", w[1].0, (w[1].1 - w[0].1).as_secs_f64() * 1e3));
        }
        eprintln!("{} ms ({} defs)", line, u.prog.defs.len() - d0);
    }
    Ok(Added { crates, defs, d0, e0, f0, errs })
}

/// Parses, resolves, collects and type-checks a crate loaded at run time (on the owner thread);
/// `sandbox`: the compile-time refusal applies. On an error nothing of it stays reachable.
unsafe fn front(up: *mut Unit, name: &str, path: &str, deps: &[String], sandbox: bool) -> Result<(u32, u32, Vec<DefId>), Vec<String>> {
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let sess = &session;
    let u = &mut *up;
    let mut dep_list = Vec::new();
    // std, then the named crates of the running program (the newest crate of each name)
    for c in ["core", "alloc", "std"].iter().map(|s| s.to_string()).chain(deps.iter().cloned()) {
        let found = (0..u.prog.crates.len()).rev().find(|&k| u.prog.syms.str(u.prog.crates[k].name) == c.replace('-', "_"));
        match found {
            Some(k) => dep_list.push((u.prog.syms.str(u.prog.crates[k].name).to_string(), Dep::Existing(k as u32))),
            None if matches!(c.as_str(), "core" | "alloc" | "std") => {}
            None => return Err(vec![format!("{}: dependency `{}` is not a crate of the running program", path, c)]),
        }
    }
    let cfg = {
        let mut c = crate::cfg::CfgSet::host(sess, &[]);
        c.set.push("relax".to_string());
        c
    };
    let tr = std::env::var_os("MAKEPAD_RUN_MODULE_TRACE").is_some();
    let mut marks: Vec<(&str, std::time::Instant)> = vec![("", std::time::Instant::now())];
    let added = add_crates(up, &[NewCrate { name: name.to_string(), path: path.to_string(), edition: 2021, cfg, deps: dep_list, proc_macro: false }], false, None)?;
    let u = &mut *up;
    let Added { crates, defs, d0, e0, f0, mut errs } = added;
    let krate = crates[0];
    if errs.is_empty() {
        errs.extend(check_orphans(u, krate, &defs));
    }
    let root_file = (f0 as u32..u.prog.files.len() as u32).find(|&f| u.prog.file_paths[f as usize] == path).unwrap_or(f0 as u32);
    let mut bodies: Vec<crate::typeck::Body> = Vec::new();
    if errs.is_empty() {
        // every fn of the module, generic bodies included, type-checks
        for &d in &defs {
            if !matches!(u.prog.def(d).kind, DefKind::Fn | DefKind::AssocFn | DefKind::Const | DefKind::Static | DefKind::AssocConst) {
                continue;
            }
            if let Some(a) = crate::borrowck::identity_args(&u.tcx, &u.prog, d) {
                let b = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::typeck::instantiate_body(&crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &crate::tcx::EvalState::new()), &u.prog, &u.tcx, d, &a)));
                match b {
                    Ok(b) => {
                        for e in &b.errors {
                            if !errs.contains(e) {
                                errs.push(e.clone());
                            }
                        }
                        bodies.push(b);
                    }
                    Err(_) => errs.push(format!("internal compiler error type-checking {} -- makepad run bug, report it", u.prog.def_path(d))),
                }
            }
        }
    }
    marks.push(("typeck", std::time::Instant::now()));
    if errs.is_empty() && sandbox {
        errs.extend(sandbox_check(&u.prog, &u.tcx, krate, &bodies.iter().collect::<Vec<_>>()));
    }
    marks.push(("check", std::time::Instant::now()));
    if tr {
        let mut line = format!("makepad-run module: {} front:", name);
        for w in marks.windows(2) {
            line.push_str(&format!(" {} {:.2}", w[1].0, (w[1].1 - w[0].1).as_secs_f64() * 1e3));
        }
        eprintln!("{} ms ({} defs)", line, u.prog.defs.len() - d0);
    }
    if !errs.is_empty() {
        drop_defs(up, &defs);
        let u = &mut *up;
        u.prog.errors.truncate(e0);
        return Err(errs);
    }
    Ok((krate, root_file, defs))
}

/// Takes defs out of the program (a refused load, an unload).
pub(crate) unsafe fn drop_defs(up: *mut Unit, defs: &[DefId]) {
    let u = &mut *up;
    let _fw = jit::front_write();
    let impls: Vec<DefId> = defs.iter().copied().filter(|d| u.prog.def(*d).kind == DefKind::Impl).collect();
    u.prog.unbind_defs(defs);
    u.tcx.uncollect_impls(&impls);
    u.prog.reclaim_caches();
}

/// Compiles `roots` and everything they reach, then installs it (nothing installs on an error
/// in code `mine` claims).
unsafe fn compile_all(up: *mut Unit, roots: Vec<u32>, mine: &dyn Fn(&Unit, u32) -> bool, own: &dyn Fn(&Unit, u32) -> bool) -> Result<(), Vec<String>> {
    let built = crate::reload::compile_reachable(up, roots, mine, own)?;
    for (id, r) in built {
        match r {
            Ok(c) => {
                jit::install_first(up, id, &c.0, c.1, &c.2);
            }
            Err(e) => {
                jit::install_failed(up, id, &e);
            }
        }
    }
    Ok(())
}

fn root_fns(u: &Unit, defs: &[DefId]) -> Vec<DefId> {
    defs.iter().copied().filter(|d| matches!(u.prog.def(*d).kind, DefKind::Fn | DefKind::AssocFn) && u.tcx.fn_sig(&u.prog, *d).map_or(false, |s| s.n_generics == 0)).collect()
}

/// Loads a trusted module (MAKEPAD_RUN_MODULES_TRUSTED=1; on the owner thread): it runs as host code.
unsafe fn load(up: *mut Unit, name: &str, path: &str, deps: &[String], sandbox: bool) -> Result<(u32, u32, Vec<DefId>), Vec<String>> {
    let (krate, root_file, defs) = front(up, name, path, deps, sandbox)?;
    let u = &mut *up;
    let roots: Vec<u32> = root_fns(u, &defs).into_iter().map(|d| u.fn_id(FnKey::Inst(d, Vec::new()))).collect();
    let mine = |u: &Unit, id: u32| match &u.fns[id as usize].key {
        FnKey::Inst(d, _) => u.prog.def(*d).krate == krate,
        FnKey::Closure(file, _, _) => u.prog.file_crate.get(*file as usize) == Some(&krate),
        _ => false,
    };
    if let Err(e) = compile_all(up, roots, &mine, &mine) {
        drop_defs(up, &defs);
        return Err(e);
    }
    Ok((krate, root_file, defs))
}

/// makepad inspect's probes and `eval`: compiles `v` as a new private child module `name` of
/// module `parent` (Program::add_child_module: the parent's private items, imports and `crate::`
/// paths are in scope), type-checks it and compiles its non-generic fns and what they reach
/// (owner thread): (the child module, its definitions). On an error nothing of it stays reachable.
pub(crate) unsafe fn load_in_module(up: *mut Unit, parent: u32, name: &str, v: crate::program::FileVersion) -> Result<(u32, Vec<DefId>), Vec<String>> {
    let u = &mut *up;
    let d0 = u.prog.defs.len();
    let i0 = u.prog.imports.len();
    let e0 = u.prog.errors.len();
    let f0 = u.prog.files.len();
    let path = v.path.clone();
    let child = {
        let _fw = jit::front_write();
        crate::run::preserve_collected_boot(u);
        u.tcx.collect_epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let r = u.prog.add_child_module(parent, name, v);
        if r.is_ok() {
            u.prog.resolve_imports_added(i0, d0);
        }
        u.prog.reclaim_caches();
        r
    };
    let defs: Vec<DefId> = (d0 as u32..u.prog.defs.len() as u32).map(DefId).collect();
    let fail = |up: *mut Unit, errs: Vec<String>| -> Result<(u32, Vec<DefId>), Vec<String>> {
        drop_defs(up, &defs);
        (&mut *up).prog.errors.truncate(e0);
        Err(errs)
    };
    let (child, _file) = match child {
        Ok(c) => c,
        Err(e) => return fail(up, vec![e]),
    };
    let u = &mut *up;
    let mine_file = |p: &str| p == path;
    let mut errs: Vec<String> = u.prog.errors[e0..].iter().filter(|e| e.starts_with(path.as_str())).cloned().collect();
    for im in u.prog.imports.iter().skip(i0) {
        if !im.done {
            let (l, c) = u.prog.files[im.file as usize].line_col(im.pos);
            errs.push(format!("{}:{}:{}: unresolved import", u.prog.file_paths[im.file as usize], l, c));
        }
    }
    if errs.is_empty() {
        let _fw = jit::front_write();
        let visible = u.prog.crates_seeing_files(f0 as u32..u.prog.files.len() as u32);
        errs.extend(u.tcx.collect_added_in(&u.prog, &defs, false, Some(&visible)).into_iter().filter(|e| e.starts_with(path.as_str())));
    }
    if errs.is_empty() {
        for &d in &defs {
            if !matches!(u.prog.def(d).kind, DefKind::Fn | DefKind::AssocFn | DefKind::Const | DefKind::Static | DefKind::AssocConst) {
                continue;
            }
            if let Some(a) = crate::borrowck::identity_args(&u.tcx, &u.prog, d) {
                let b = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::typeck::instantiate_body(&crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &crate::tcx::EvalState::new()), &u.prog, &u.tcx, d, &a)));
                match b {
                    Ok(b) => {
                        for e in &b.errors {
                            if !errs.contains(e) {
                                errs.push(e.clone());
                            }
                        }
                    }
                    Err(_) => errs.push(format!("internal compiler error type-checking {} -- makepad run bug, report it", u.prog.def_path(d))),
                }
            }
        }
    }
    if !errs.is_empty() {
        return fail(up, errs);
    }
    let roots: Vec<u32> = root_fns(u, &defs).into_iter().map(|d| u.fn_id(FnKey::Inst(d, Vec::new()))).collect();
    let mine = |u: &Unit, id: u32| match &u.fns[id as usize].key {
        FnKey::Inst(d, _) => mine_file(&u.prog.file_paths[u.prog.def(*d).file as usize]),
        FnKey::Closure(file, _, _) => u.prog.file_paths.get(*file as usize).is_some_and(|p| mine_file(p)),
        _ => false,
    };
    if let Err(e) = compile_all(up, roots, &mine, &mine) {
        return fail(up, e);
    }
    Ok((child, defs))
}

/// The module a `crate::a::b` / `krate::a::b` path names (a crate's root for `krate`): inspect's
/// `--in` and the scope of a hooked fn.
pub(crate) fn module_named(u: &Unit, path: &str) -> Option<u32> {
    let path = path.trim().trim_start_matches("::");
    let mut segs = path.split("::").filter(|s| !s.is_empty());
    let first = segs.next()?.replace('-', "_");
    let krate = (0..u.prog.crates.len()).rev().find(|&k| u.prog.syms.str(u.prog.crates[k].name) == first)?;
    let mut m = u.prog.crates[krate].root_mod;
    for s in segs {
        let sym = u.prog.syms.get(s)?;
        let d = *u.prog.mods[m as usize].types.get(&sym)?;
        if u.prog.def(d).kind != DefKind::Mod {
            return None;
        }
        m = u.prog.def(d).sub;
    }
    Some(m)
}

/// The module whose scope resolves the names in fn `d`'s body (its block module's named module).
pub(crate) fn scope_of(u: &Unit, d: DefId) -> u32 {
    u.prog.mods[u.prog.def(d).scope as usize].normal
}

/// How an export's parameter or result crosses into the sandbox (wrapper generation).
enum Pass {
    /// a scalar, as is
    Plain(String),
    /// `&T` / `&mut T` of a plain-data type: copied into the arena (and back for &mut)
    Copy(String, bool),
    /// `&[T]` / `&mut [T]` of plain data
    Slice(String, bool),
    Str,
    /// `&M` / `&mut M` of a module type: the host holds it as std::relax::Inst
    Inst(String, bool),
}

fn is_scalar(u: &Unit, t: crate::types::TyId) -> bool {
    matches!(u.tcx.tys.kind(t), TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_))
}

fn ty_src(u: &Unit, t: crate::types::TyId) -> String {
    crate::typeck::type_name(&u.prog, &u.tcx, t)
}

/// Whether type `t` names a type of crate `krate` (an impl's self type: a key no cached answer
/// of the rest of the program can hold).
fn mentions_crate(u: &Unit, t: crate::types::TyId, krate: u32, depth: u32) -> bool {
    if depth > 16 {
        return false;
    }
    match u.tcx.tys.kind(t).clone() {
        TyKind::Adt(d, args, _) => u.prog.def(d).krate == krate || u.tcx.tys.list(args).iter().any(|a| mentions_crate(u, *a, krate, depth + 1)),
        TyKind::Dyn(d, args, _, _, _, _, _) => u.prog.def(d).krate == krate || u.tcx.tys.list(args).iter().any(|a| mentions_crate(u, *a, krate, depth + 1)),
        TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) | TyKind::Slice(x) | TyKind::Array(x, _) => mentions_crate(u, x, krate, depth + 1),
        TyKind::Tuple(v) => u.tcx.tys.list(v).iter().any(|a| mentions_crate(u, *a, krate, depth + 1)),
        _ => false,
    }
}

/// New module impls may not alter trait/method answers for the host's types.
/// Used by cold loads and by reload before publishing newly streamed definitions.
pub(crate) fn check_orphans(u: &Unit, krate: u32, defs: &[DefId]) -> Vec<String> {
    let mut errs = Vec::new();
    for &d in defs {
        if u.prog.def(d).krate != krate || u.prog.def(d).kind != DefKind::Impl { continue; }
        let Some(&st) = u.tcx.impl_self.get(&d) else { continue };
        let trait_local = u.tcx.impl_trait.get(&d).map(|&td| u.prog.def(td).krate == krate);
        if trait_local != Some(true) && !mentions_crate(u, st, krate, 0) {
            let def = u.prog.def(d);
            let pf = &u.prog.files[def.file as usize];
            let (line, col) = pf.line_col(pf.ast.item(def.item).lo);
            errs.push(format!(
                "{}:{}:{}: impl for `{}`: a module may implement only its own traits, or traits for its own types (the orphan rule)",
                u.prog.file_paths[def.file as usize], line, col, crate::typeck::type_name(&u.prog, &u.tcx, st)
            ));
        }
    }
    errs
}

/// Plain data: bytes that mean the same at any address (no pointers, references, boxes or fn
/// pointers anywhere inside): copied bitwise into a module's arena.
fn plain_data(u: &Unit, t: crate::types::TyId) -> bool {
    plain_data_d(u, t, 0)
}

fn plain_data_d(u: &Unit, t: crate::types::TyId, depth: u32) -> bool {
    if depth > 16 {
        return false;
    }
    match u.tcx.tys.kind(t).clone() {
        TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) => true,
        TyKind::Array(e, _) => plain_data_d(u, e, depth + 1),
        TyKind::Tuple(v) => u.tcx.tys.list(v).iter().all(|x| plain_data_d(u, *x, depth + 1)),
        TyKind::Adt(d, args, _) => match u.tcx.adts.get(&d) {
            Some(a) if !a.is_union => a.variants.iter().enumerate().all(|(vi, v)| (0..v.fields.len()).all(|fi| plain_data_d(u, u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, vi as u32, fi as u32, u.tcx.tys.list(args)), depth + 1))),
            _ => false,
        },
        _ => false,
    }
}

/// Results must also be Copy and name only host types, including in generic arguments and
/// nested fields. A dependency can itself be a run-time module: its types cannot escape either.
pub(crate) fn plain_result(u: &Unit, t: crate::types::TyId) -> bool {
    fn host_types(u: &Unit, t: crate::types::TyId, depth: u32) -> bool {
        if depth > 16 { return false; }
        match u.tcx.tys.kind(t).clone() {
            TyKind::Adt(d, args, _) => {
                !crate::sandbox::is_sandboxed_crate_key(&u.prog, &FnKey::Inst(d, Vec::new()))
                    && u.tcx.tys.list(args).iter().all(|a| host_types(u, *a, depth + 1))
                    && u.tcx.adts.get(&d).map_or(false, |a| a.variants.iter().enumerate().all(|(vi, v)| (0..v.fields.len()).all(|fi| host_types(u, u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, vi as u32, fi as u32, u.tcx.tys.list(args)), depth + 1))))
            }
            TyKind::Array(e, _) => host_types(u, e, depth + 1),
            TyKind::Tuple(v) => u.tcx.tys.list(v).iter().all(|x| host_types(u, *x, depth + 1)),
            _ => is_scalar(u, t),
        }
    }
    plain_data(u, t) && u.is_copy(t) && host_types(u, t, 0)
}

fn named_type(u: &Unit, t: crate::types::TyId, path: &str) -> bool {
    matches!(u.tcx.tys.kind(t), TyKind::Adt(d, _, _) if u.prog.def_path(*d) == path)
}

pub(crate) fn boundary_result(u: &Unit, t: crate::types::TyId) -> bool {
    plain_result(u, t) || string_result(u, t) != 0
}

fn string_result(u: &Unit, t: crate::types::TyId) -> u8 {
    if named_type(u, t, "core::string::String") { return 1; }
    if let TyKind::Adt(d, args, _) = u.tcx.tys.kind(t) {
        if u.prog.def_path(*d) == "core::result::Result" && u.tcx.tys.list(*args).len() == 2
            && plain_result(u, u.tcx.tys.list(*args)[0]) && named_type(u, u.tcx.tys.list(*args)[1], "core::string::String") { return 2; }
    }
    0
}

fn classify_param(u: &Unit, krate: u32, t: crate::types::TyId) -> Option<Pass> {
    if is_scalar(u, t) || named_type(u, t, "std::relax::Host") {
        return Some(Pass::Plain(ty_src(u, t)));
    }
    match u.tcx.tys.kind(t).clone() {
        TyKind::Ref(mutbl, inner, _) => match u.tcx.tys.kind(inner).clone() {
            TyKind::Str if !mutbl => Some(Pass::Str),
            TyKind::Slice(e) if plain_data(u, e) => Some(Pass::Slice(ty_src(u, e), mutbl)),
            TyKind::Adt(d, _, _) if u.prog.def(d).krate == krate => Some(Pass::Inst(ty_src(u, inner), mutbl)),
            _ if plain_data(u, inner) => Some(Pass::Copy(ty_src(u, inner), mutbl)),
            _ => None,
        },
        _ => None,
    }
}

/// The host wrapper crate of a sandboxed module: one fn per supported export.
fn wrapper_source(u: &Unit, krate: u32, modname: &str, idx: u32, defs: &[DefId]) -> (String, Vec<String>) {
    let mut src = String::from("#![allow(unused)]\n// generated by Relax: host wrappers of a sandboxed module (modules.rs)\n");
    let mut skipped = Vec::new();
    let root = u.prog.crates[krate as usize].root_mod;
    for d in root_fns(u, defs) {
        let def = u.prog.def(d);
        if def.kind != DefKind::Fn || !def.is_pub || def.scope != root {
            continue;
        }
        let fname = u.prog.syms.str(def.name).to_string();
        let sig = match u.tcx.fn_sig(&u.prog, d) {
            Some(s) => s,
            None => continue,
        };
        let mut params = Vec::new();
        let mut pre = String::new();
        let mut args = Vec::new();
        let mut post = String::new();
        let mut ok = true;
        for (k, &t) in sig.params.iter().enumerate() {
            let a = format!("a{}", k);
            match classify_param(u, krate, t) {
                Some(Pass::Plain(ts)) => {
                    params.push(format!("{}: {}", a, ts));
                    args.push(a);
                }
                Some(Pass::Copy(ts, false)) => {
                    params.push(format!("{}: &{}", a, ts));
                    pre.push_str(&format!("    let {a}s = std::relax::__sb_in({a});\n"));
                    args.push(format!("{}s", a));
                }
                Some(Pass::Copy(ts, true)) => {
                    params.push(format!("{}: &mut {}", a, ts));
                    pre.push_str(&format!("    let {a}s = std::relax::__sb_in_mut(&*{a});\n"));
                    args.push(format!("&mut *{}s", a));
                    post.push_str(&format!("    std::relax::__sb_out({a}, &*{a}s);\n"));
                }
                Some(Pass::Slice(ts, false)) => {
                    params.push(format!("{}: &[{}]", a, ts));
                    pre.push_str(&format!("    let {a}s = std::relax::__sb_in_slice({a});\n"));
                    args.push(format!("{}s", a));
                }
                Some(Pass::Slice(ts, true)) => {
                    params.push(format!("{}: &mut [{}]", a, ts));
                    pre.push_str(&format!("    let {a}s = std::relax::__sb_in_slice_mut(&*{a});\n"));
                    args.push(format!("&mut *{}s", a));
                    post.push_str(&format!("    std::relax::__sb_out_slice({a}, &*{a}s);\n"));
                }
                Some(Pass::Str) => {
                    params.push(format!("{}: &str", a));
                    pre.push_str(&format!("    let {a}s = std::relax::__sb_in_str({a});\n"));
                    args.push(format!("{}s", a));
                }
                Some(Pass::Inst(ts, mutbl)) => {
                    params.push(format!("{}: std::relax::Inst", a));
                    args.push(format!("unsafe {{ {}.__get{}::<{}>({}) }}", a, if mutbl { "_mut" } else { "" }, ts, idx));
                }
                None => {
                    ok = false;
                    skipped.push(format!("`{}`: parameter {} of type `{}` cannot cross into a sandboxed module (use scalars, references to plain data, slices, &str, or the module's own types)", fname, k + 1, ty_src(u, t)));
                }
            }
        }
        let rt = sig.ret;
        let (ret_src, wrap_ret) = if matches!(u.tcx.tys.kind(rt), TyKind::Tuple(v) if v.is_empty()) {
            ("()".to_string(), "__r".to_string())
        } else if string_result(u, rt) != 0 {
            let wrap = if string_result(u, rt) == 1 { "std::relax::__sb_string(__r)" }
                else { "match __r { Ok(v) => Ok(v), Err(s) => Err(std::relax::__sb_string(s)) }" };
            (ty_src(u, rt), wrap.to_string())
        } else if plain_result(u, rt) {
            // lower::emit_call puts aggregate result storage in the arena before calling
            // the twin, then copies it to this host local before __g restores the frame.
            (ty_src(u, rt), "__r".to_string())
        } else {
            let n = ty_src(u, rt);
            let boxed = match u.tcx.tys.kind(rt) {
                TyKind::Adt(d, a, _) if u.tcx.adts.get(d).is_some_and(|adt| adt.is_box()) && !a.is_empty() => match u.tcx.tys.kind(u.tcx.tys.list(*a)[0]) {
                    TyKind::Adt(d, _, _) => u.prog.def(*d).krate == krate,
                    _ => false,
                },
                _ => false,
            };
            if boxed {
                ("std::relax::Inst".to_string(), format!("std::relax::Inst::__from_box({}, __r)", idx))
            } else {
                ok = false;
                skipped.push(format!("`{}`: its result type `{}` cannot leave a sandboxed module (use scalars, (), plain-data arrays/tuples, Copy host structs/enums without pointers/references/Box, or Box of the module's own type)", fname, n));
                (String::new(), String::new())
            }
        };
        if !ok {
            continue;
        }
        src.push_str(&format!(
            "pub fn {f}({ps}) -> {rs} {{\n    let __g = std::relax::__sb_enter({idx});\n{pre}    let __r = {m}::{f}({args});\n{post}    {wr}\n}}\n",
            f = fname,
            ps = params.join(", "),
            rs = ret_src,
            idx = idx,
            pre = pre,
            m = modname,
            args = args.join(", "),
            post = post,
            wr = wrap_ret
        ));
    }
    (src, skipped)
}

/// Loads a sandboxed module (on the owner thread): its code runs only as twins in its own arena;
/// the host calls it through a generated wrapper crate. Ok: (module crate, root file, its
/// defs and the wrapper's, wrapper crate, arena, unsupported exports).
#[allow(clippy::type_complexity)]
unsafe fn load_sandboxed(up: *mut Unit, idx: u32, name: &str, path: &str, deps: &[String]) -> Result<(u32, u32, Vec<DefId>, u32, std::sync::Arc<crate::sandbox::Arena>, Vec<String>), Vec<String>> {
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let sess = &session;
    let runtime = &sess.runtime;
    let options = &sess.options;
    // MAKEPAD_RUN_SANDBOX_NOCHECK=1 (tests of the memory confinement alone): no compile-time refusal
    let check = !options.sandbox_nocheck;
    let t0 = std::time::Instant::now();
    let (krate, root_file, mut defs) = front(up, name, path, deps, check)?;
    let t_front = t0.elapsed();
    let fail = |up: *mut Unit, defs: &[DefId], e: Vec<String>| -> Vec<String> {
        drop_defs(up, defs);
        e
    };
    if let Err(e) = crate::sandbox::add_sandboxed_crate(up, krate) {
        return Err(fail(up, &defs, vec![e]));
    }
    let mem: u64 = runtime.module_mem_mb << 20;
    let fuel: i64 = runtime.module_fuel;
    let arena = match crate::sandbox::new_arena(idx, mem, fuel) {
        Ok(a) => a,
        Err(e) => return Err(fail(up, &defs, vec![e])),
    };
    let (src, skipped) = wrapper_source(&*up, krate, name, idx, &defs);
    let dir = std::env::temp_dir().join(format!("relax-modules-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let wpath = dir.join(format!("__sbw_{}_{}.rs", name, idx));
    if let Err(e) = std::fs::write(&wpath, &src) {
        crate::sandbox::free_arena(&arena);
        return Err(fail(up, &defs, vec![format!("cannot write the module wrapper: {}", e)]));
    }
    let wname = format!("__sbw_{}_{}", name, idx);
    let mut wdeps: Vec<String> = vec![name.to_string()];
    wdeps.extend(deps.iter().cloned());
    let t1 = std::time::Instant::now();
    let (wk, _, wdefs) = match front(up, &wname, &wpath.to_string_lossy(), &wdeps, false) {
        Ok(x) => x,
        Err(e) => {
            crate::sandbox::free_arena(&arena);
            let mut e = e;
            e.insert(0, format!("{}: the generated wrapper did not compile (makepad run bug, report it):", path));
            return Err(fail(up, &defs, e));
        }
    };
    crate::sandbox::add_wrapper_crate(wk);
    let t_wrap = t1.elapsed();
    let t2 = std::time::Instant::now();
    let u = &mut *up;
    let roots: Vec<u32> = root_fns(u, &wdefs).into_iter().map(|d| u.fn_id(FnKey::Inst(d, Vec::new()))).collect();
    let fns0 = u.fns.len();
    if let Err(e) = compile_all(up, roots, &|_, _| true, &|u, id| crate::reload::module_own(u, id)) {
        drop_defs(up, &wdefs);
        crate::sandbox::free_arena(&arena);
        return Err(fail(up, &defs, e));
    }
    // MAKEPAD_RUN_MODULE_TRACE=1: where a load's time goes
    if std::env::var_os("MAKEPAD_RUN_MODULE_TRACE").is_some() {
        eprintln!(
            "makepad-run module: {} loaded in {:.2} ms: front {:.2} (parse, resolve, typeck, check), wrapper {:.2}, compile {:.2} ({} slots)",
            name,
            t0.elapsed().as_secs_f64() * 1e3,
            t_front.as_secs_f64() * 1e3,
            t_wrap.as_secs_f64() * 1e3,
            t2.elapsed().as_secs_f64() * 1e3,
            (*up).fns.len() - fns0
        );
    }
    defs.extend(wdefs);
    Ok((krate, root_file, defs, wk, arena, skipped))
}

/// The export `name` of a module: a non-generic fn at its root, with its fn-pointer type as
/// `core::any::type_name` prints it.
fn export(u: &mut Unit, m: &Module, name: &str) -> Result<(DefId, String), String> {
    if m.wrapper.is_some() {
        if let Some(why) = m.skipped.iter().find(|w| w.starts_with(&format!("`{}`:", name))) {
            return Err(format!("export {} (module `{}`)", why, m.name));
        }
    }
    let wrapper = m.extra_exports.iter().find(|(n, _)| n == name).map(|(_, k)| *k).or(m.wrapper);
    let root = u.prog.crates[wrapper.unwrap_or(m.krate) as usize].root_mod;
    let sym = u.prog.syms.get(name).ok_or_else(|| format!("module `{}` has no export `{}`", m.name, name))?;
    let d = *u.prog.mods[root as usize].values.get(&sym).ok_or_else(|| format!("module `{}` has no export `{}`", m.name, name))?;
    if u.prog.def(d).kind != DefKind::Fn {
        return Err(format!("`{}` of module `{}` is not a fn", name, m.name));
    }
    let sig = u.tcx.fn_sig(&u.prog, d).ok_or_else(|| format!("`{}` of module `{}` has no signature", name, m.name))?;
    if sig.n_generics != 0 {
        return Err(format!("`{}` of module `{}` is generic: exports are concrete fns", name, m.name));
    }
    let t = u.tcx.tys.intern(TyKind::FnPtr(u.tcx.tys.mk_list(&sig.params), sig.ret, sig.header, 0));
    Ok((d, crate::typeck::type_name(&u.prog, &u.tcx, t)))
}

/// A newly streamed export gets the same generated sandbox wrapper as a cold-loaded one.
/// Existing wrappers and their addresses stay intact. Unit and module locks are held.
unsafe fn ensure_export(up: *mut Unit, idx: usize, m: &mut Module, name: &str) -> Result<(), String> {
    let Some(wk) = m.wrapper else { return Ok(()) };
    let u = &*up;
    let Some(sym) = u.prog.syms.get(name) else { return Ok(()) };
    let root = u.prog.crates[wk as usize].root_mod;
    if u.prog.mods[root as usize].values.contains_key(&sym) || m.extra_exports.iter().any(|(n, _)| n == name) {
        return Ok(());
    }
    let root = u.prog.crates[m.krate as usize].root_mod;
    let Some(&d) = u.prog.mods[root as usize].values.get(&sym) else { return Ok(()) };
    let (src, skipped) = wrapper_source(u, m.krate, &m.name, idx as u32, &[d]);
    if !skipped.is_empty() { return Err(skipped.join("\n")); }
    let wname = format!("__sbw_{}_{}_extra{}", m.name, idx, m.extra_exports.len());
    let path = std::env::temp_dir().join(format!("relax-modules-{}", std::process::id())).join(format!("{}.rs", wname));
    std::fs::write(&path, src).map_err(|e| format!("cannot write module wrapper: {}", e))?;
    let mut deps = vec![m.name.clone()];
    deps.extend(u.prog.crates[m.krate as usize].deps.iter().map(|(n, _)| u.prog.syms.str(*n).to_string()));
    let (wk, _, defs) = front(up, &wname, &path.to_string_lossy(), &deps, false).map_err(|e| e.join("\n"))?;
    crate::sandbox::add_wrapper_crate(wk);
    let u = &mut *up;
    let roots = root_fns(u, &defs).into_iter().map(|d| u.fn_id(FnKey::Inst(d, Vec::new()))).collect();
    if let Err(e) = compile_all(up, roots, &|_, _| true, &|u, id| crate::reload::module_own(u, id)) {
        drop_defs(up, &defs);
        return Err(e.join("\n"));
    }
    m.defs.extend(defs);
    m.extra_exports.push((name.to_string(), wk));
    Ok(())
}

/// Streaming resolves the file by its handle, including from the live watcher.
pub(crate) fn stream_target(h: u64) -> Option<(u32, String)> {
    let ms = MODULES.lock().unwrap_or_else(|e| e.into_inner());
    unhandle(h).and_then(|(i, g)| ms.get(i).filter(|m| m.gen == g && m.alive).map(|m| (m.file, m.path.clone())))
}

pub(crate) fn stream_applied(h: u64, file: u32) {
    let mut ms = MODULES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = unhandle(h).and_then(|(i, g)| ms.get_mut(i).filter(|m| m.gen == g && m.alive)) { m.file = file; }
}

pub extern "C" fn __relax_module_load_stream(name: *const u8, nlen: usize, path: *const u8, plen: usize, src: *const u8, slen: usize, deps: *const u8, dlen: usize) -> u64 {
    let up = jit::current_unit();
    if up.is_null() { set_error("no running unit".to_string()); return 0; }
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let sess = &session;
    let path_text = unsafe { String::from_utf8_lossy(bytes(path, plen)) };
    let text = crate::reload::complete_prefix(sess, &path_text, unsafe { bytes(src, slen) }, 2021);
    __relax_module_load_src(name, nlen, path, plen, text.as_ptr(), text.len(), deps, dlen)
}

/// 0 applied, 1 incomplete, 2 needs reload, 3 pending, 4 invalid handle.
pub extern "C" fn __relax_module_stream(h: u64, src: *const u8, slen: usize, kept: *mut usize) -> u32 {
    if stream_target(h).is_none() {
        set_error("the module was unloaded".to_string());
        return 4;
    }
    let up = jit::current_unit();
    let o = unsafe { crate::reload::apply_module_stream(up, h, bytes(src, slen).to_vec()) };
    if !kept.is_null() { unsafe { *kept = o.kept; } }
    if o.pending { return 3; }
    match o.rung {
        crate::reload::Rung::Restart => 2,
        crate::reload::Rung::Refuse => 1,
        crate::reload::Rung::Patch if o.group == 0 => 1,
        crate::reload::Rung::Patch => { crate::sandbox::maybe_recycle(up); 0 }
    }
}

// ------------------------------------------------------------ the program's entries (std::relax)

/// Loads the module at `path` as crate `name`, depending on the running program's crates
/// `deps` (newline-separated names). Returns its handle, or 0 (the errors: __relax_module_error).
pub extern "C" fn __relax_module_load(name: *const u8, nlen: usize, path: *const u8, plen: usize, deps: *const u8, dlen: usize) -> u64 {
    __relax_module_load_src(name, nlen, path, plen, std::ptr::null(), usize::MAX, deps, dlen)
}

/// As __relax_module_load, from source text (`slen` != usize::MAX): `path` then only names the
/// module in errors and positions (embedded presets, user and AI edits held in memory).
pub extern "C" fn __relax_module_load_src(name: *const u8, nlen: usize, path: *const u8, plen: usize, src: *const u8, slen: usize, deps: *const u8, dlen: usize) -> u64 {
    // an internal error of Relax's never unwinds into the program (this is an extern "C" entry)
    match std::panic::catch_unwind(|| load_src_inner(name, nlen, path, plen, src, slen, deps, dlen)) {
        Ok(h) => h,
        Err(p) => {
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".to_string());
            set_error(format!("internal compiler error loading a module: {}{} -- makepad run bug, report it", msg, jit::take_ice_location()));
            0
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn load_src_inner(name: *const u8, nlen: usize, path: *const u8, plen: usize, src: *const u8, slen: usize, deps: *const u8, dlen: usize) -> u64 {
    let (name, mut path, deps) = unsafe {
        (
            String::from_utf8_lossy(bytes(name, nlen)).into_owned(),
            String::from_utf8_lossy(bytes(path, plen)).into_owned(),
            String::from_utf8_lossy(bytes(deps, dlen)).lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>(),
        )
    };
    // text: written to a file of this process's module directory, then named as the caller says
    let shown = path.clone();
    let mut temp: Option<String> = None;
    if slen != usize::MAX {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("relax-modules-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let ext = "rs";
        let f = dir.join(format!("{}-{}.{}", name, N.fetch_add(1, std::sync::atomic::Ordering::Relaxed), ext));
        if let Err(e) = std::fs::write(&f, unsafe { bytes(src, slen) }) {
            set_error(format!("{}: cannot stage the module text: {}", shown, e));
            return 0;
        }
        path = f.to_string_lossy().into_owned();
        temp = Some(path.clone());
    }
    let up = jit::current_unit();
    if up.is_null() {
        set_error("no running unit".to_string());
        return 0;
    }
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    // MAKEPAD_RUN_MODULES_TRUSTED=1: a trusted module (host code, no sandbox; diagnosis only)
    let trusted = options.modules_trusted;
    let fix = |t: String| -> String {
        match &temp {
            Some(tp) => t.replace(tp.as_str(), &shown),
            None => t,
        }
    };
    // the module's index (its handle) before it exists: wrappers name it
    let idx = {
        let mut ms = MODULES.lock().unwrap_or_else(|e| e.into_inner());
        ms.push(Module { name: name.clone(), krate: u32::MAX, file: 0, path: shown.clone(), defs: Vec::new(), gen: 0, alive: false, arena: None, wrapper: None, extra_exports: Vec::new(), skipped: Vec::new() });
        ms.len() - 1
    };
    let r = crate::owner::call(crate::owner::program_class(), || {
        let r = if trusted {
            unsafe { load(up, &name, &path, &deps, false) }.map(|(k, f, d)| (k, f, d, None, None, Vec::new()))
        } else {
            unsafe { load_sandboxed(up, idx as u32, &name, &path, &deps) }.map(|(k, f, d, w, a, sk)| (k, f, d, Some(w), Some(a), sk))
        };
        if r.is_ok() && temp.is_some() {
            let u = unsafe { &mut *up };
            if let Ok((_, file, _, _, _, _)) = &r {
                u.prog.file_paths[*file as usize] = shown.clone();
            }
        }
        r
    });
    match r {
        Ok((krate, file, defs, wrapper, arena, skipped)) => {
            let mut ms = MODULES.lock().unwrap_or_else(|e| e.into_inner());
            ms[idx] = Module { name, krate, file, path: shown, defs, gen: 1, alive: true, arena, wrapper, extra_exports: Vec::new(), skipped };
            handle(idx, 1)
        }
        Err(e) => {
            set_error(fix(e.join("\n")));
            0
        }
    }
}

/// The callable address of export `name` checked against the fn-pointer type `ty`
/// (type_name); 0 with an error otherwise.
pub extern "C" fn __relax_module_get(h: u64, name: *const u8, nlen: usize, ty: *const u8, tlen: usize) -> u64 {
    let (name, ty) = unsafe { (String::from_utf8_lossy(bytes(name, nlen)).into_owned(), String::from_utf8_lossy(bytes(ty, tlen)).into_owned()) };
    let mut ms = MODULES.lock().unwrap_or_else(|e| e.into_inner());
    let (idx, m) = match unhandle(h).and_then(|(i, g)| ms.get_mut(i).filter(|m| m.gen == g && m.alive).map(|m| (i, m))) {
        Some(m) => m,
        None => {
            set_error("the module was unloaded".to_string());
            return 0;
        }
    };
    let up = jit::current_unit();
    let r = crate::owner::call(crate::owner::program_class(), || {
        let ready = unsafe { ensure_export(up, idx, m, &name) };
        let u = unsafe { &mut *up };
        ready.and_then(|_| export(u, m, &name)).and_then(|(d, t)| {
            if t != ty {
                return Err(format!("export `{}` of module `{}` is `{}`, not `{}`", name, m.name, t, ty));
            }
            let id = u.fn_id(FnKey::Inst(d, Vec::new()));
            Ok(u.entry(id))
        })
    });
    match r {
        Ok(a) => a,
        Err(e) => {
            set_error(e);
            0
        }
    }
}

/// Applies a new text of the module (hot reload: one transaction). 0 applied; 1 refused (errors
/// in __relax_module_error); 2 the edit needs a fresh load (layout or item-set change: unload,
/// then load the new text); 3 unloaded handle.
pub extern "C" fn __relax_module_replace(h: u64, src: *const u8, slen: usize) -> u32 {
    let src = unsafe { bytes(src, slen).to_vec() };
    let up = jit::current_unit();
    let Some(o) = (unsafe { crate::reload::finish_module_stream(up, h, src) }) else {
        set_error("the module was unloaded".to_string());
        return 3;
    };
    let error = || o.lines.join("\n");
    match o.rung {
        crate::reload::Rung::Patch => {
            crate::sandbox::maybe_recycle(up);
            0
        }
        crate::reload::Rung::Refuse => {
            set_error(error());
            1
        }
        crate::reload::Rung::Restart => {
            set_error(error());
            2
        }
    }
}

/// Unloads a module: its names leave the program (code stays mapped for frames and stored
/// fn pointers still in it). Later uses of the handle fail.
pub extern "C" fn __relax_module_unload(h: u64) {
    crate::reload::forget_module_stream(jit::current_unit(), h);
    let mut krate: Option<u32> = None;
    let module_crate;
    let mut defs = {
        let mut ms = MODULES.lock().unwrap_or_else(|e| e.into_inner());
        match unhandle(h).and_then(|(i, g)| ms.get_mut(i).filter(|m| m.gen == g && m.alive)) {
            Some(m) => {
                module_crate = m.krate;
                m.alive = false;
                m.gen += 1;
                if let Some(a) = m.arena.take() {
                    crate::sandbox::free_arena(&a);
                    krate = Some(m.krate);
                }
                std::mem::take(&mut m.defs)
            }
            None => return,
        }
    };
    let up = jit::current_unit();
    crate::owner::call(crate::owner::program_class(), || {
        let u = unsafe { &mut *up };
        defs.extend((0..u.prog.defs.len()).filter(|&i| u.prog.defs[i].krate == module_crate).map(|i| DefId(i as u32)));
        defs.sort_unstable_by_key(|d| d.0);
        defs.dedup();
        {
            let _fw = jit::front_write();
            let impls: Vec<DefId> = defs.iter().copied().filter(|d| u.prog.def(*d).kind == DefKind::Impl).collect();
            u.prog.unbind_defs(&defs);
            u.tcx.uncollect_impls(&impls);
            unsafe {
                u.prog.reclaim_caches();
            }
        }
        if let Some(k) = krate {
            unsafe { crate::sandbox::retire_crate(up, k) };
        }
    });
    crate::sandbox::maybe_recycle(up);
}

/// Copies this thread's last module error into `buf` (up to `cap` bytes); returns its length.
pub extern "C" fn __relax_module_error(buf: *mut u8, cap: usize) -> usize {
    LAST_ERROR.with(|l| {
        let l = l.borrow();
        let n = l.len().min(cap);
        if !buf.is_null() && n > 0 {
            unsafe { std::ptr::copy_nonoverlapping(l.as_ptr(), buf, n) };
        }
        l.len()
    })
}

/// A wrapper enters module `idx`'s arena (0; the state before goes to `out`), or finds it
/// unloaded (1).
pub extern "C" fn __relax_sb_enter(idx: u32, out: *mut [i64; 4]) -> u32 {
    match crate::sandbox::enter_module(idx) {
        Some(prev) => {
            unsafe { *out = prev.0 };
            0
        }
        None => 1,
    }
}

/// A wrapper leaves: the state before its entry comes back.
pub extern "C" fn __relax_sb_leave(prev: *const [i64; 4]) {
    crate::sandbox::leave(crate::sandbox::Saved(unsafe { *prev }));
}

pub extern "C" fn __relax_sb_alloca(size: usize, align: usize) -> usize {
    crate::sandbox::alloca(size as u64, align as u64) as usize
}

fn arena_for(h: u64) -> Option<Arc<crate::sandbox::Arena>> {
    let (idx, gen) = unhandle(h)?;
    let modules = MODULES.lock().unwrap_or_else(|e| e.into_inner());
    let m = modules.get(idx)?;
    if !m.alive || m.gen != gen { return None; }
    m.arena.clone()
}

// The account owns its counters independently of JIT stack frames. Recovery may skip
// a caller's frame; active scopes retain a strong reference rather than a stack pointer.
pub extern "C" fn __relax_budget_new(fuel: u64, growth: u64) -> u64 {
    std::rc::Rc::into_raw(std::rc::Rc::new(std::cell::Cell::new([fuel, growth, 0, 0]))) as u64
}
pub extern "C" fn __relax_budget_drop(token: u64) {
    unsafe { drop(std::rc::Rc::from_raw(token as *const std::cell::Cell<[u64; 4]>)); }
}
pub extern "C" fn __relax_budget_read(token: u64, out: *mut [u64; 4]) {
    unsafe { *out = (*(token as *const std::cell::Cell<[u64; 4]>)).get(); }
}
pub extern "C" fn __relax_budget_reset(token: u64, fuel: u64, growth: u64) {
    unsafe { (*(token as *const std::cell::Cell<[u64; 4]>)).set([fuel, growth, 0, 0]); }
}

pub extern "C" fn __relax_budget_enter(h: u64, token: u64) -> u32 {
    let Some(a) = arena_for(h) else { return 1; };
    crate::sandbox::budget_begin(&a, token);
    0
}

pub extern "C" fn __relax_budget_leave() {
    crate::sandbox::budget_end();
}

struct HostEntry {
    id: u64,
    module: u32,
    ptr: u64,
    ty: String,
    busy: bool,
}

crate::gen_local! {
    static HOSTS: std::cell::RefCell<Vec<HostEntry>>;
}
static NEXT_HOST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub extern "C" fn __relax_host_new(h: u64, ptr: u64, ty: *const u8, len: usize) -> u64 {
    let Some(a) = arena_for(h) else { return 0; };
    let id = NEXT_HOST.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if id == 0 { return 0; }
    HOSTS.with(|s| s.borrow_mut().push(HostEntry {
        id, module: a.module, ptr, ty: String::from_utf8_lossy(unsafe { bytes(ty, len) }).into_owned(), busy: false,
    }));
    id
}

pub extern "C" fn __relax_host_drop(id: u64) {
    HOSTS.with(|s| s.borrow_mut().retain(|e| e.id != id));
}

pub extern "C" fn __relax_host_acquire(id: u64, ty: *const u8, len: usize) -> u64 {
    let module = crate::sandbox::current_module();
    HOSTS.with(|s| {
        let mut s = s.borrow_mut();
        let Some(e) = s.iter_mut().find(|e| e.id == id) else { return 0; };
        if Some(e.module) != module || e.busy || e.ty.as_bytes() != unsafe { bytes(ty, len) } { return 0; }
        e.busy = true;
        e.ptr
    })
}

pub extern "C" fn __relax_host_release(id: u64) {
    HOSTS.with(|s| {
        if let Some(e) = s.borrow_mut().iter_mut().find(|e| e.id == id) { e.busy = false; }
    });
}

pub extern "C" fn __relax_sb_string_copy(p: u64, len: u64, out: *mut u8) -> u32 {
    unsafe { crate::sandbox::string_copy(p, len, out) as u32 }
}

/// A guarded call's start: the sandbox state to come back to.
pub struct GuardMark {
    arena: crate::sandbox::Saved,
    budgets: usize,
    entries: u64,
    hosts: Vec<(u64, bool)>,
}

pub fn guard_mark() -> GuardMark {
    GuardMark {
        arena: crate::sandbox::save(),
        budgets: crate::sandbox::budget_depth(),
        entries: crate::sandbox::entry_depth(),
        hosts: HOSTS.with(|s| s.borrow().iter().map(|e| (e.id, e.busy)).collect()),
    }
}

/// A guarded call's end (also after a fault or a deadline, where no wrapper left).
pub fn guard_reset(m: GuardMark) {
    crate::sandbox::budget_recover(m.budgets);
    crate::sandbox::restore(m.arena);
    crate::sandbox::entry_recover(m.entries);
    HOSTS.with(|s| {
        let mut s = s.borrow_mut();
        s.retain(|e| m.hosts.iter().any(|h| h.0 == e.id));
        for e in s.iter_mut() {
            if let Some(h) = m.hosts.iter().find(|h| h.0 == e.id) { e.busy = h.1; }
        }
    });
}

/// Sandbox statistics for tests and memory reports: [recycles, code bytes in the active space,
/// dead code bytes pending, image ro bytes, image rw bytes, arenas live]. `recycle` != 0 runs a
/// recycle now (blocking).
pub extern "C" fn __relax_sb_stats(out: *mut [u64; 6], recycle: u32) -> u32 {
    let mut ok = 1;
    if recycle != 0 {
        let up = jit::current_unit();
        ok = match unsafe { crate::sandbox::recycle(up) } {
            Ok(_) => 1,
            Err(e) => {
                set_error(e);
                0
            }
        };
    }
    let (r, used, dead) = crate::sandbox::code_stats();
    let (ro, rw, n) = crate::sandbox::stats();
    unsafe { *out = [r as u64, used, dead, ro as u64, rw as u64, n as u64] };
    ok
}

/// The `__relax_module_*` entries by name (jit::loader_symbol).
pub fn symbol(name: &[u8]) -> Option<usize> {
    Some(match name {
        b"__relax_budget_new" => __relax_budget_new as *const () as usize,
        b"__relax_budget_drop" => __relax_budget_drop as *const () as usize,
        b"__relax_budget_read" => __relax_budget_read as *const () as usize,
        b"__relax_budget_reset" => __relax_budget_reset as *const () as usize,
        b"__relax_budget_enter" => __relax_budget_enter as *const () as usize,
        b"__relax_budget_leave" => __relax_budget_leave as *const () as usize,
        b"__relax_host_new" => __relax_host_new as *const () as usize,
        b"__relax_host_drop" => __relax_host_drop as *const () as usize,
        b"__relax_host_acquire" => __relax_host_acquire as *const () as usize,
        b"__relax_host_release" => __relax_host_release as *const () as usize,
        b"__relax_sb_string_copy" => __relax_sb_string_copy as *const () as usize,
        b"__relax_module_load" => __relax_module_load as *const () as usize,
        b"__relax_module_load_src" => __relax_module_load_src as *const () as usize,
        b"__relax_module_load_stream" => __relax_module_load_stream as *const () as usize,
        b"__relax_module_stream" => __relax_module_stream as *const () as usize,
        b"__relax_module_get" => __relax_module_get as *const () as usize,
        b"__relax_module_replace" => __relax_module_replace as *const () as usize,
        b"__relax_module_unload" => __relax_module_unload as *const () as usize,
        b"__relax_module_error" => __relax_module_error as *const () as usize,
        b"__relax_sb_enter" => __relax_sb_enter as *const () as usize,
        b"__relax_sb_leave" => __relax_sb_leave as *const () as usize,
        b"__relax_sb_alloca" => __relax_sb_alloca as *const () as usize,
        b"__relax_sb_stats" => __relax_sb_stats as *const () as usize,
        _ => return crate::reload::restart_symbol(name),
    })
}
