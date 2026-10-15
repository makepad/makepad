//! Gandalf, Relax's lazy borrow checker (scheduling). Named by Kevin Boos.
//!
//! Hot reload: an edit of one or more source files applied as ONE all-or-nothing transaction
//! (local/plans/hotreload.md §9: M1 transaction, M3 classifier).
//!
//! 1. Parse every edited file as a new version (nothing the running program reads changes).
//! 2. Classify each item against the running version: body edits (class B), const value
//!    edits (B, with the readers recompiled) and static initializer edits (the value is kept,
//!    said so) apply; everything else takes the restart rung with a one-line reason, never a
//!    silent stale result (signature, layout, item set, macro, declarative edits until their
//!    mechanisms exist).
//! 3. Freeze: closures of the functions about to be rebuilt that were never compiled are
//!    compiled now, from the version their stored values were made by.
//! 4. Commit the versions (each a new file index: old code, closure and coroutine types and
//!    checked bodies keep meaning the old text), type-check every changed body (never-called
//!    fns and generic bodies included) and every live instance, borrow-check.
//! 5. Compile everything (changed slots, const readers, the callers that inlined them) and
//!    a bounded set of slots created by the edit. Previously cold callees and overflow are
//!    queued after install and can still compile lazily on first call.
//! 6. Only if all of that succeeded: install, one patch group. Otherwise nothing is installed,
//!    the versions are reverted, and the errors are returned.
//! The owner thread holds for the whole transaction (compiling off the unit is the next step).

use crate::lang_items::LangItem;
use crate::jit::{self, FnKey, Unit};
use crate::program::{DefId, DefKind, Program};
use crate::types::TyId;
use std::time::Instant;
use std::sync::atomic::{AtomicBool, Ordering};
#[path = "reload_watch.rs"]
mod watch_events;

// Transaction-local measurements: no clocks, counters or formatting when disabled.
// Stage milliseconds are exclusive wall times; worker CPU is measured by the harness.
#[derive(Default)]
struct ReloadTrace {
    started: Option<Instant>,
    last: Option<Instant>,
    cpu_last: u64,
    cpu: std::collections::BTreeMap<&'static str, f64>,
    stages: std::collections::BTreeMap<&'static str, f64>,
    // sub-stage wall times inside a stage (MAKEPAD_RUN_RELOAD_TRACE: printed as sub.<name>_ms)
    sub_last: Option<Instant>,
    subs: std::collections::BTreeMap<&'static str, f64>,
    counts: std::collections::BTreeMap<&'static str, usize>,
}
crate::gen_local! {
    static TRACE: std::cell::RefCell<ReloadTrace>;
}
thread_local! {
    static TRACE_INPUT: std::cell::Cell<(f64, f64, usize)> = const { std::cell::Cell::new((0.0, 0.0, 0)) };
}
pub(crate) fn trace_input(watch_ms: f64, read_ms: f64, scanned: usize) {
    if std::env::var_os("MAKEPAD_RUN_RELOAD_TRACE").is_some() {
        TRACE_INPUT.with(|s| s.set((watch_ms, read_ms, scanned)));
    }
}
fn trace_stage(name: &'static str) {
    TRACE.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(last) = s.last {
            let now = Instant::now();
            *s.stages.entry(name).or_default() += (now - last).as_secs_f64() * 1000.;
            s.last = Some(now);
            s.sub_last = Some(now);
            let cpu = crate::clock::cpu_ns();
            let elapsed = cpu.saturating_sub(s.cpu_last) as f64 / 1e6;
            *s.cpu.entry(name).or_default() += elapsed;
            s.cpu_last = cpu;
        }
    });
}
/// A finer split of the running stage: wall time since the stage began or the previous sub mark.
fn trace_sub(name: &'static str) {
    TRACE.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(prev) = s.sub_last.or(s.last) {
            let now = Instant::now();
            *s.subs.entry(name).or_default() += (now - prev).as_secs_f64() * 1000.;
            s.sub_last = Some(now);
        }
    });
}
/// A unit call for the reload transaction; MAKEPAD_RUN_RELOAD_TRACE reports a wait over 5 ms with
/// the holder seen when the wait began (whose hold delayed the edit).
fn call_traced<R>(at: &'static str, f: impl FnOnce() -> R) -> R {
    let traced = TRACE.with(|s| s.borrow().started.is_some());
    let holder = if traced { Some(crate::owner::serving_note()) } else { None };
    let t = Instant::now();
    crate::owner::call(crate::owner::Class::App, || {
        let waited = t.elapsed().as_secs_f64() * 1e3;
        if let Some(h) = holder.filter(|_| waited > 5.0) {
            eprintln!("reload-trace: unit call wait {waited:.1} ms before {at}: the owner was serving {h}");
        }
        f()
    })
}
fn trace_count(name: &'static str, n: usize) {
    TRACE.with(|s| {
        let mut s = s.borrow_mut();
        if s.started.is_some() { *s.counts.entry(name).or_default() += n; }
    });
}
struct TraceTransaction;
impl TraceTransaction {
    fn begin(files: usize) -> Self {
        if std::env::var_os("MAKEPAD_RUN_RELOAD_TRACE").is_some() {
            let now = Instant::now();
            let (watch, read, scanned) = TRACE_INPUT.with(|s| s.replace((0.0, 0.0, 0)));
            let mut state = ReloadTrace { started: Some(now), last: Some(now), cpu_last: crate::clock::cpu_ns(), ..Default::default() };
            state.stages.insert("watch", watch);
            state.stages.insert("read", read);
            state.counts.insert("watch_files", scanned);
            state.counts.insert("edit_files", files);
            TRACE.with(|s| *s.borrow_mut() = state);
            crate::parse_work::begin();
        }
        Self
    }
    fn finish(&self, out: &Outcome) {
        trace_stage("other");
        TRACE.with(|s| {
            let s = s.borrow();
            if let Some(start) = s.started {
                let stages = s.stages.iter().map(|(k,v)| format!("{k}_ms={v:.3}")).collect::<Vec<_>>().join(" ");
                let cpu = s.cpu.iter().map(|(k,v)| format!("{k}_cpu_ms={v:.3}")).collect::<Vec<_>>().join(" ");
                let mut counts = s.counts.iter().map(|(k,v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ");
                for (k, v) in &s.subs { counts.push_str(&format!(" sub.{k}_ms={v:.3}")); }
                let parse = crate::parse_work::finish();
                eprintln!("reload-trace: rung={} total_ms={:.3} {} {} {} changed={} installed={} ahead={} parse_detail_ms={:?} style_reapply=unobserved",
                    out.rung.name(), ms(start) + s.stages["watch"] + s.stages["read"], stages, cpu, counts,
                    out.changed, out.names.len(), out.work[1], parse.0);
            }
        });
    }
}
impl Drop for TraceTransaction {
    fn drop(&mut self) { TRACE.with(|s| *s.borrow_mut() = ReloadTrace::default()); }
}


/// The application's crates for Gandalf (borrowck::event's app scope, the one app-crate
/// rule): their root source paths. Target kind (lib/bin) is not ownership, and the loaded
/// Graph's pkg directory no longer distinguishes vendored registry crates from paths.
/// Use the original unit graph's path+file package IDs and source paths instead.
/// The project tree is the launched package's resolver workspace (or its own manifest
/// directory for a standalone package). Path dependencies need not be workspace members.
/// Framework exclusions are rooted at that tree, never a substring of an app's path.
pub(crate) fn gandalf_app_roots(units: &crate::unitgraph::Units, launched_dir: &str) -> Vec<String> {
    let tree = crate::resolve::find_root(launched_dir).unwrap_or_else(|_| launched_dir.to_string());
    let tree = gandalf_path(&tree);
    units.units.iter().filter_map(|unit| {
        if !unit.pkg_id.starts_with("path+file://") { return None; }
        if matches!(unit.name.as_str(), "std" | "core" | "alloc" | "proc_macro") { return None; }
        let source = unit.src_path.as_str();
        let path = gandalf_path(source);
        let rel = std::path::Path::new(&path).strip_prefix(&tree).ok()?;
        // tools/: the framework's runtime crates apps link (relax, ...)
        if ["platform", "draw", "widgets", "libs", "tools", "engine", "relax",
            "apps/commercial/engine"].iter().any(|p| rel.starts_with(p)) {
            return None;
        }
        Some(crate::vfs::normalize_path(source))
    }).collect()
}

fn gandalf_path(path: &str) -> String {
    // Canonical paths prevent a path dependency's symlink from escaping the project.
    // Virtual (git/VFS) workspaces have no filesystem path; lexical normalization works there.
    crate::vfs::canonicalize(path).map(|p| crate::vfs::normalize_path(&p.to_string_lossy()))
        .unwrap_or_else(|_| crate::vfs::normalize_path(path))
}

/// A live edit refused by the gate: one strictly technical console line, the event's
/// diagnostic on the JSON stream, and the live log line. The only console print of a
/// gate event (gandalf_out's Console leaves Origin::Gate to this).
fn report_gandalf_refusal(ev: &crate::borrowck::event::GandalfEvent) {
    let e = &ev.error;
    eprintln!("borrow check failed: {} at {}:{}:{}: {} (live edit refused)", e.fn_path, e.file, e.line, e.col, e.message);
    let mut d = e.to_diagnostic();
    d.notes.push(format!("live edit refused in {}", e.fn_path));
    if crate::run_state::observed() { crate::run_state::diagnostic(crate::run::app_domain(), d.clone()); }
    if crate::diag::enabled() {
        crate::diag::emit(&d);
        // Pipes used by agents/serve must receive each line before the next check.
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
    live_log(&format!("{{\"event\":\"gandalf\",\"origin\":\"LiveEdit\",\"fn_path\":{},\"rendered\":{},\"diagnostic\":{}}}",
        json_str(&e.fn_path), json_str(&e.render()), d.to_json()));
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rung {
    /// errors in the edit: nothing changed
    Refuse,
    /// installed as a patch group
    Patch,
    /// the edit needs a (state-preserving) restart; the running code is unchanged
    Restart,
}

impl Rung {
    pub fn name(self) -> &'static str {
        match self {
            Rung::Refuse => "refuse",
            Rung::Patch => "patch",
            Rung::Restart => "restart",
        }
    }
}

/// One edited file.
pub struct Edit {
    /// the running version's file index
    pub file: u32,
    pub path: String,
    pub src: Vec<u8>,
}

pub struct Outcome {
    pub rung: Rung,
    /// Number of authored items retained by partial composition.
    pub kept: usize,
    pub pending: bool,
    /// per edit: the file index now running for that path (the new version when applied)
    pub files: Vec<u32>,
    /// errors (refuse), reasons (restart), notes: what was kept and what was not (patch)
    pub lines: Vec<String>,
    /// installed functions
    pub names: Vec<String>,
    /// changed items (functions, consts, statics)
    pub changed: usize,
    pub group: u32,
    /// ms: parse + classify, check, compile, install
    pub ms: [f64; 4],
    /// what rollback() needs to undo this patch group
    pub undo: Option<Undo>,
    /// scoreboard: [slots rebuilt (code that existed and was replaced), functions prepared ahead
    /// (compiled synchronously for the edit), of those taken from the disk cache, fns naming
    /// a removed/renamed field, of those the ones that use it]
    pub work: [usize; 5],
}

/// An installed patch group's undo record: the file versions it made current and the const
/// values it replaced.
pub struct Undo {
    pub group: u32,
    /// (running file index before, after) per edited file
    pub versions: Vec<(u32, u32)>,
    pairs: Vec<Vec<(u32, u32)>>,
    added: Vec<DefId>,
    removed: Vec<DefId>,
    consts: Vec<((DefId, Vec<TyId>), u64)>,
    consts_hit: Vec<DefId>,
    adts: Vec<(DefId, crate::tcx::AdtT)>,
    variants: Vec<(DefId, usize)>,
    const_failed: Vec<(DefId, Vec<TyId>)>,
    full_inputs: bool,
    // Lead decision 52 (R1): the crates a declaration publication reached (None: all).
    scope: Option<Vec<bool>>,
}

// Lead decision 51: classify both versions using the definition collector's
// owners (rustc_resolve/src/def_collector.rs:423-448 @8d1a764). Even unchanged inline
// constants in a changed body require full invalidation; do not compare tokens.
fn ctfe_relevant_bodies(prog: &Program, file: u32, changed: &[DefId]) -> bool {
    for &index in prog.defs_in_file(file) {
        let def = DefId(index);
        if !changed_body_owner(prog, def, changed) { continue; }
        match prog.def(def).kind {
            DefKind::Const | DefKind::AssocConst | DefKind::Static => return true,
            DefKind::AnonConst | DefKind::OpaqueTy => {
                if prog.def_id_to_node_id.get(&def).is_some_and(|(owner_file, _)| *owner_file == file) { return true; }
            }
            _ => {},
        }
    }
    for &owner in changed {
        if prog.opaque_types_defined_by.get(&owner).is_some_and(|defs| !defs.is_empty()) { return true; }
    }
    false
}

fn changed_body_owner(prog: &Program, mut def: DefId, changed: &[DefId]) -> bool {
    while def != crate::program::NO_DEF {
        if changed.contains(&def) { return true; }
        def = prog.def(def).parent;
    }
    false
}

/// Whether const `d`'s cached values belong to a publication's `scope` (None: every crate).
fn in_scope(prog: &Program, scope: Option<&[bool]>, d: DefId) -> bool {
    scope.map_or(true, |v| v.get(prog.def(d).krate as usize).copied().unwrap_or(true))
}

/// Lead decision 52 (R2): whether a constant-evaluated context can read one of the changed
/// const or static `items`: a const fn body, a const/associated-const initializer, an anon
/// const (array length, repeat count, const argument) or inline const in a crate that can see
/// them, and for a changed *const* also a static initializer (as the lead specified). A static
/// initializer naming a changed *static* is not such a context: a static's storage is made
/// once (jit::Unit::static_addr) by running its own initializer after installation, and no
/// reload re-evaluates the storage of an existing static -- the full path changes nothing
/// there. Conservative and by name: every identifier equal to an item's name or to a `use`
/// alias renaming one, in the source of a file of those crates or in the text of a macro
/// expansion or materialized generated body (ParsedFile::exp_map, followed to its call site),
/// counts when it lies inside such a context (including the expressions of a materialized
/// generated body). Text no definition holds (a cfg-disabled generated item) is never
/// evaluated, and a generated body never materialized was never evaluated either.
fn const_contexts_mention(prog: &Program, items: &[DefId]) -> bool {
    use crate::def_collector::NodeId;
    let visible = prog.crates_seeing_files(items.iter().map(|d| prog.def(*d).file));
    let alias_closure = |mut names: Vec<Vec<u8>>| -> Vec<Vec<u8>> {
        loop {
            let mut grew = false;
            for im in &prog.imports {
                let (Some(bind), Some(&(_, last))) = (im.bind, im.segs.last()) else { continue };
                if im.glob || bind == last { continue; }
                let (b, l) = (prog.syms.str(bind).as_bytes().to_vec(), prog.syms.str(last).as_bytes().to_vec());
                if names.contains(&l) && !names.contains(&b) { names.push(b); grew = true; }
            }
            if !grew { return names; }
        }
    };
    let all = alias_closure(items.iter().map(|d| def_name(prog, *d).into_bytes()).collect());
    let consts = alias_closure(items.iter().filter(|d| matches!(prog.def(**d).kind, DefKind::Const | DefKind::AssocConst)).map(|d| def_name(prog, *d).into_bytes()).collect());
    // the constant-evaluated contexts, per file: (lo, hi, whether static names count); and the
    // ranges of every other item of those files (text there is not a context's, unless a
    // context range inside it holds it)
    let mut contexts: crate::fxhash::HashMap<u32, Vec<(u32, u32, bool)>> = Default::default();
    let mut others: crate::fxhash::HashMap<u32, Vec<(u32, u32)>> = Default::default();
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if !visible.get(def.krate as usize).copied().unwrap_or(true) { continue; }
        if items.contains(&d) || matches!(def.kind, DefKind::Static | DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::Trait
            | DefKind::Impl | DefKind::TypeAlias | DefKind::Fn | DefKind::AssocFn | DefKind::AssocTy | DefKind::MacroRules) {
            let it = prog.files[def.file as usize].ast.item(def.item);
            others.entry(def.file).or_default().push((it.lo, it.hi));
        }
        if items.contains(&d) { continue; }
        let statics_count = match def.kind {
            DefKind::Const | DefKind::AssocConst | DefKind::AnonConst => true,
            DefKind::Static => false,
            DefKind::Fn | DefKind::AssocFn => match prog.files[def.file as usize].ast.item(def.item).kind {
                crate::ast::ItemKind::Fn(sig, _, _) if prog.files[def.file as usize].ast.fn_sig(sig).konst => true,
                _ => continue,
            },
            _ => continue,
        };
        if def.kind == DefKind::AnonConst {
            match prog.def_id_to_node_id.get(&d) {
                Some(&(file, NodeId::AnonConst(e) | NodeId::InlineConst(e))) => {
                    let x = prog.files[file as usize].ast.expr(e);
                    contexts.entry(file).or_default().push((x.lo, x.hi, true));
                }
                _ => return true,
            }
            continue;
        }
        let pf = &prog.files[def.file as usize];
        let it = pf.ast.item(def.item);
        contexts.entry(def.file).or_default().push((it.lo, it.hi, statics_count));
    }
    let ident = |b: u8| b == b'_' || b.is_ascii_alphanumeric();
    for (&file, ranges) in &contexts {
        let pf = &prog.files[file as usize];
        let other: &[(u32, u32)] = others.get(&file).map_or(&[], |v| v.as_slice());
        let src: &[u8] = &pf.src[..];
        // (source text is UTF-8; text that is not counts as a mention)
        let Ok(text) = std::str::from_utf8(src) else { return true };
        let mut calls: Vec<(u32, u32, u32)> = pf.exp_map.iter().collect();
        calls.sort_unstable();
        for name in &all {
            let Ok(name) = std::str::from_utf8(name) else { return true };
            let is_const = consts.iter().any(|n| n.as_slice() == name.as_bytes());
            for (start, _) in text.match_indices(name) {
                let end = start + name.len();
                if (start > 0 && ident(src[start - 1])) || (end < src.len() && ident(src[end])) { continue; }
                // a context holds it, or another item, or it is expansion text followed back to
                // its call site; text no context, item or call site holds is no definition's
                // (a cfg-disabled generated item, module-level text) and is never evaluated
                let mut at = start as u32;
                let mut steps = 0;
                loop {
                    if ranges.iter().any(|&(lo, hi, statics_count)| at >= lo && at < hi && (statics_count || is_const)) { return true; }
                    if at < pf.src_len || other.iter().any(|&(lo, hi)| at >= lo && at < hi) { break; }
                    let j = calls.partition_point(|c| c.0 <= at);
                    match j.checked_sub(1).map(|j| calls[j]) {
                        Some((x, y, call)) if at >= x && at < y && steps < 64 => { at = call; steps += 1; }
                        _ => break,
                    }
                }
            }
        }
    }
    false
}

/// Lead decision 52 (R2 (a)): an initializer made only of literals -- arrays, repeats with a
/// literal count, tuples, struct literals without a base or omitted fields, negation and
/// shared borrows of such values -- folds without executing code: no call, no path to another
/// value, no block. (fold_without_execution covers scalar results only; generated shader
/// position tables are arrays of struct literals.)
fn literal_initializer(pf: &crate::parser::ParsedFile, e: crate::ast::ExprId, depth: u32) -> bool {
    use crate::ast::{ExprKind, UnOp};
    if depth > 64 { return false; }
    match pf.ast.expr(e).kind {
        ExprKind::Lit(_) => true,
        ExprKind::Paren(x) | ExprKind::Unary(UnOp::Neg, x) | ExprKind::AddrOf(false, false, x) => literal_initializer(pf, x, depth + 1),
        ExprKind::Tuple(xs) | ExprKind::Array(xs) => pf.ast.list(xs).iter().all(|&x| literal_initializer(pf, x, depth + 1)),
        ExprKind::Repeat(x, n) => matches!(pf.ast.expr(n).kind, ExprKind::Lit(_)) && literal_initializer(pf, x, depth + 1),
        ExprKind::Struct(row) => {
            let crate::ast::StructRow { fields, base, rest: defaults, .. } = *pf.ast.row(row);
            base.get().is_none() && !defaults && pf.ast.list(fields).iter().all(|f| literal_initializer(pf, f.expr, depth + 1))
        }
        _ => false,
    }
}

fn publish_reload(u: &mut Unit, versions: &[(u32, u32)], full: bool, scope: Option<&[bool]>) {
    u.tcx.queries.register_files(u.prog.files.len());
    if full {
        u.tcx.publish_declarations(scope);
    } else {
        let mut files = Vec::new();
        for &(old, new) in versions { files.push(old); files.push(new); }
        u.tcx.publish_file_bodies(&files);
    }
}

/// Rolls back an installed patch group (a panic or fault in its code, M7): the slots return
/// to their previous code, the files to their previous versions, the consts to their previous
/// values. Returns the rolled-back functions' names. The caller makes a unit call.
pub unsafe fn rollback(up: *mut Unit, undo: Undo) -> Vec<String> {
    crate::owner::call(crate::owner::Class::App, || {
        let u = &mut *up;
        jit::bump_patch_epoch();
        let ids = u.rollback_gen(undo.group);
        let names = ids.iter().map(|id| u.fns[*id as usize].name.to_string()).collect();
        {
            let fw = jit::front_write();
            u.tcx.collect_epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let impls: Vec<DefId> = undo.added.iter().copied().filter(|d| u.prog.def(*d).kind == DefKind::Impl).collect();
            u.prog.unbind_defs(&undo.added);
            if !impls.is_empty() { u.tcx.uncollect_impls(&impls); }
            u.prog.rebind_defs(&undo.removed);
            for (k, &(o, n)) in undo.versions.iter().enumerate().rev() {
                u.prog.revert_version(o, n, &undo.pairs[k]);
            }
            for (d, n) in &undo.variants { u.prog.truncate_variants(*d, *n); }
            for (d, adt) in &undo.adts { u.tcx.adts.insert(*d, adt.clone()); }
            publish_reload(u, &undo.versions, undo.full_inputs, undo.scope.as_deref());
            u.lay.clear();
            u.prog.reclaim_caches();
            drop(fw);
        }
        if undo.full_inputs {
            let (prog, scope) = (&u.prog, undo.scope.as_deref());
            u.consts.retain(|(d, _), _| !in_scope(prog, scope, *d));
            u.const_failed.retain(|(d, _)| !in_scope(prog, scope, *d));
        }
        for k in &undo.consts_hit {
            let ks: Vec<(DefId, Vec<TyId>)> = u.consts.keys().filter(|(d, _)| d == k).cloned().collect();
            for x in ks {
                u.consts.remove(&x);
            }
        }
        for ((d, args), v) in undo.consts { u.put_const(d, args, v); }
        u.const_failed.retain(|(d, _)| !undo.consts_hit.contains(d));
        u.const_failed.extend(undo.const_failed);
        // RIR built from the rolled-back text (inlining sources) is dropped
        for id in std::mem::take(&mut u.reload_specs) {
            u.fns[id as usize].spec = None;
        }
        for &(_, n) in &undo.versions {
            for f in u.fns.iter_mut() {
                if f.rir.as_ref().map_or(false, |r| r.file == n) {
                    f.rir = None;
                }
            }
        }
        names
    })
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Change {
    Body(DefId),
    Const(DefId),
    StaticInit(DefId),
    /// a named-field struct's field list changed (M10): reconciled with the running layout at
    /// apply time (layout_edit), or the restart rung
    Layout(DefId),
    /// a `macro_rules!` changed: the fns of this file that invoke it are rebuilt from their new
    /// expansions (a macro used from another file takes the restart rung)
    Macro(u32),
}

/// One file's edit as the classifier sees it.
pub(crate) struct Plan {
    /// (old item, new item) of the items both versions have
    pub(crate) pairs: Vec<(u32, u32)>,
    pub(crate) changes: Vec<Change>,
    /// items only the new version has, with where they go: a module, or an existing impl
    pub(crate) added: Vec<(u32, Result<u32, DefId>)>,
    /// defs of items only the old version has
    pub(crate) removed: Vec<DefId>,
    /// `use` items only the old version has (their position in the old file): their imports go
    pub(crate) removed_uses: Vec<u32>,
    pub(crate) attributes_changed: bool,
}

/// Token texts of `pf` in [lo, hi) outside `holes` (comments are not tokens: comment-only
/// edits are no change). Items made by macro expansion lie past the file's own tokens: their
/// bytes are compared instead.
/// `toks` with the position digits of generated `__relax_*` names dropped: a macro's expansion
/// (thread_local!, derives) names its helpers by where they are, so code above that grew or
/// shrank changes them without changing the item.
/// Tokens and expansion text compare alike: whitespace is a separator, kept only between two
/// word characters (one version's macro expansion may be tokenized, the other's still text).
fn toks_norm(pf: &crate::parser::ParsedFile, lo: u32, hi: u32, holes: &[(u32, u32)]) -> Vec<Vec<u8>> {
    let mut joined: Vec<u8> = Vec::with_capacity(hi.saturating_sub(lo) as usize);
    let push = |bytes: &mut dyn Iterator<Item = u8>, joined: &mut Vec<u8>| {
        for c in bytes {
            if c.is_ascii_whitespace() {
                if joined.last() != Some(&0) { joined.push(0); }
            } else {
                joined.push(c);
            }
        }
    };
    for piece in toks(pf, lo, hi, holes) {
        // Almost every token has no generated name. Avoid a temporary allocation per
        // token while retaining exactly the existing normalization for generated names.
        if piece.windows(b"__relax_".len()).any(|w| w == b"__relax_") {
            push(&mut norm_generated(piece).into_iter(), &mut joined);
        } else {
            push(&mut piece.iter().copied(), &mut joined);
        }
        // a separator keeps two identifiers from merging (`pub fn` vs `pubfn`)
        joined.push(0);
    }
    // the separators between punctuation do not carry meaning: drop them next to non-word bytes
    let w = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out: Vec<u8> = Vec::with_capacity(joined.len());
    for (i, &c) in joined.iter().enumerate() {
        if c == 0 {
            let prev = out.last().copied();
            let next = joined[i + 1..].iter().copied().find(|&x| x != 0);
            if !(prev.map_or(false, w) && next.map_or(false, w)) {
                continue;
            }
            out.push(b' ');
            continue;
        }
        out.push(c);
    }
    vec![out]
}

/// toks_norm(a) == toks_norm(b), normalizing only when the raw token texts differ: equal
/// tokens normalize alike, and an edit leaves almost every item's tokens as they were.
#[allow(clippy::too_many_arguments)]
fn same_toks(a: &crate::parser::ParsedFile, alo: u32, ahi: u32, ha: &[(u32, u32)], b: &crate::parser::ParsedFile, blo: u32, bhi: u32, hb: &[(u32, u32)]) -> bool {
    // toks() without its Vec: the token texts in [lo, hi) outside the holes
    fn raw<'a>(pf: &'a crate::parser::ParsedFile, lo: u32, hi: u32, holes: &'a [(u32, u32)]) -> Option<impl Iterator<Item = &'a [u8]>> {
        let i0 = pf.toks.partition_point(|t| t.lo < lo);
        // expansion text (no token inside the range): toks() reads its bytes instead
        if !pf.toks.get(i0).is_some_and(|t| t.hi <= hi) { return None; }
        Some(pf.toks.iter_from(i0).take_while(move |t| t.hi <= hi)
            .filter(move |t| !holes.iter().any(|&(x, y)| t.lo >= x && t.hi <= y))
            .filter_map(move |t| pf.src.get(t.lo as usize..t.hi as usize)))
    }
    let same = match (raw(a, alo, ahi, ha), raw(b, blo, bhi, hb)) {
        (Some(x), Some(y)) => x.eq(y),
        _ => toks(a, alo, ahi, ha) == toks(b, blo, bhi, hb),
    };
    same || toks_norm(a, alo, ahi, ha) == toks_norm(b, blo, bhi, hb)
}

fn norm_generated(b: &[u8]) -> Vec<u8> {
    const P: &[u8] = b"__relax_";
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(P) && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) {
            out.extend_from_slice(P);
            i += P.len();
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                if !b[i].is_ascii_digit() {
                    out.push(b[i]);
                }
                i += 1;
            }
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Index of the first token at or after position `lo`. The token array is in position order
/// except for `T::Interp` tokens, which carry the span of their spliced expression (earlier in
/// the file); a plain binary search that lands on one of them skips the range it looks for
/// (a widget's generated shader items then compared as text against the running version's
/// tokens, and every button.rx body edit took the restart rung). Interp tokens take the
/// position of the next ordinary token, which keeps the search key monotonic.
fn first_tok_at(toks: &(impl crate::parser::lazy_bodies::TokSeq + ?Sized), lo: u32) -> usize {
    let key = |i: usize| -> u32 {
        let mut j = i;
        while j < toks.len() && toks.at(j).kind == crate::lexer::T::Interp { j += 1; }
        if j < toks.len() { toks.at(j).lo } else { u32::MAX }
    };
    let (mut a, mut b) = (0usize, toks.len());
    while a < b {
        let m = a + (b - a) / 2;
        if key(m) < lo { a = m + 1; } else { b = m; }
    }
    while a < toks.len() && toks.at(a).kind == crate::lexer::T::Interp { a += 1; }
    a
}

fn toks<'a>(pf: &'a crate::parser::ParsedFile, lo: u32, hi: u32, holes: &[(u32, u32)]) -> Vec<&'a [u8]> {
    let mut out = Vec::new();
    let i0 = first_tok_at(&pf.toks, lo);
    let mut covered = false;
    let mut prev_hi = lo;
    for t in pf.toks.iter_from(i0) {
        if t.hi > hi {
            break;
        }
        covered = true;
        // a `>>` / `>=` / `>>=` (`<<`, `<=`) the parser split keeps its second part as the
        // token (its first char consumed): read the token with that char, as a version that
        // holds the same text untokenized reads it (an expansion's `Option<X<'a>>` compared
        // as `..'a>` against `..'a>>`, and Scope's next edit of the file took the restart rung)
        let split = t.lo > prev_hi && t.lo > 0 && matches!(pf.src.get(t.lo as usize - 1), Some(b'>' | b'<'))
            && matches!(pf.src.get(t.lo as usize), Some(b'>' | b'=' | b'<'));
        let start = if split { t.lo - 1 } else { t.lo };
        prev_hi = t.hi;
        if holes.iter().any(|&(a, b)| t.lo >= a && t.hi <= b) {
            continue;
        }
        if let Some(b) = pf.src.get(start as usize..t.hi as usize) {
            out.push(b);
        }
    }
    if !covered && hi > lo {
        // expansion text: bytes outside the holes
        let mut at = lo;
        let mut hs: Vec<(u32, u32)> = holes.iter().copied().filter(|&(a, b)| a >= lo && b <= hi).collect();
        hs.sort();
        for (a, b) in hs {
            if a > at {
                out.push(&pf.src[at as usize..a as usize]);
            }
            at = at.max(b);
        }
        if hi > at {
            out.push(&pf.src[at as usize..hi as usize]);
        }
    }
    out
}

fn kind_name(k: &crate::ast::ItemKind) -> &'static str {
    use crate::ast::ItemKind::*;
    match k {
        Use { .. } => "use",
        ExternCrate(_) => "extern crate",
        Fn(..) => "fn",
        Struct(..) => "struct",
        Union(..) => "union",
        Enum(..) => "enum",
        TypeAlias { .. } => "type alias",
        Const(..) => "const",
        Static(..) => "static",
        Trait { .. } => "trait",
        TraitAlias(..) => "trait alias",
        Impl { .. } => "impl",
        Mod(_) => "mod",
        ForeignMod(..) => "extern block",
        MacroRules(_) => "macro_rules!",
        Mac(_) => "item macro",
    }
}

/// What it takes to apply a changed non-code item, as a one-line reason.
fn restart_reason(k: &crate::ast::ItemKind) -> &'static str {
    use crate::ast::ItemKind::*;
    match k {
        Struct(..) | Union(..) | Enum(..) => "its layout changed; live values would be read with the old field offsets (live migration is not built yet)",
        TypeAlias { .. } => "a type changed; every user's types would change",
        Use { .. } | ExternCrate(_) => "imports changed (name resolution is not re-run live yet)",
        Trait { .. } | TraitAlias(..) => "a trait's items or bounds changed (method resolution is not re-run live yet)",
        Impl { .. } => "an impl header or its item list changed (method resolution is not re-run live yet)",
        Mod(_) | ForeignMod(..) => "a module's item list changed",
        MacroRules(_) => "a macro changed; its expansion sites are not re-expanded live yet",
        Mac(_) => "an item macro changed",
        _ => "this kind of edit is not applied live yet",
    }
}

fn child_items(ast: &crate::ast::Ast, k: &crate::ast::ItemKind) -> Vec<crate::ast::ItemId> {
    use crate::ast::ItemKind::*;
    match k {
        Trait(r) => ast.list(ast.row(*r).items).to_vec(),
        Impl(r) => ast.list(ast.row(*r).items).to_vec(),
        Mod(Some(items)) => ast.list(*items).to_vec(),
        ForeignMod(r) => ast.list(ast.row(*r).items).to_vec(),
        _ => Vec::new(),
    }
}

fn spacing_free(text: &str) -> String {
    let word = |c: char| c == '_' || c.is_alphanumeric();
    let mut out = String::with_capacity(text.len());
    let mut gap = false;
    for c in text.chars() {
        if c.is_whitespace() { gap = true; continue; }
        if gap && out.chars().last().is_some_and(word) && word(c) { out.push(' '); }
        gap = false;
        out.push(c);
    }
    out
}

/// An item's identity across versions of a file: its container path, kind and name (impls,
/// imports and item macros by their header tokens; generated `__relax_*` names with their
/// position digits dropped), with an occurrence number for repeats. Items in fn bodies are
/// keyed by their fn.
fn item_keys(pf: &crate::parser::ParsedFile) -> Vec<String> {
    use crate::ast::ItemKind;
    let ast = &pf.ast;
    let mut key: Vec<Option<String>> = vec![None; ast.items.len()];
    fn name_of(pf: &crate::parser::ParsedFile, it: &crate::ast::Item) -> String {
        let n = String::from_utf8_lossy(&pf.src[it.name.lo as usize..it.name.hi as usize]).into_owned();
        if n.starts_with("__relax_") {
            // generated (hidden length consts): their names carry byte positions
            let mut out = String::new();
            let mut digits = false;
            for c in n.chars() {
                if c.is_ascii_digit() {
                    if !digits {
                        out.push('N');
                    }
                    digits = true;
                } else {
                    out.push(c);
                    digits = false;
                }
            }
            out
        } else {
            n
        }
    }
    fn walk(pf: &crate::parser::ParsedFile, items: &[crate::ast::ItemId], prefix: &str, key: &mut Vec<Option<String>>) {
        let ast = &pf.ast;
        for &iid in items {
            let it = ast.item(iid);
            let kids = child_items(ast, &it.kind);
            let holes: Vec<(u32, u32)> = kids.iter().map(|c| (ast.items[c.0 as usize].lo, ast.items[c.0 as usize].hi)).collect();
            let head = match &it.kind {
                ItemKind::Impl { .. } | ItemKind::Use { .. } | ItemKind::Mac(_) | ItemKind::ForeignMod(..) | ItemKind::ExternCrate(_) => {
                    let mut head = String::new();
                    for t in toks(pf, it.lo, it.hi, &holes) {
                        if !head.is_empty() { head.push(' '); }
                        head.push_str(&String::from_utf8_lossy(t));
                    }
                    spacing_free(&head)
                }
                _ => name_of(pf, it),
            };
            let k = format!("{}/{} {}", prefix, kind_name(&it.kind), head);
            key[iid.0 as usize] = Some(k.clone());
            walk(pf, &kids, &k, key);
        }
    }
    walk(pf, &ast.root_items, "", &mut key);
    trace_count("classify_unreached", key.iter().filter(|k| k.is_none()).count());
    let mut order: Vec<usize> = (0..ast.items.len()).collect();
    order.sort_by_key(|&i| (ast.items[i].lo, std::cmp::Reverse(ast.items[i].hi), i));
    let mut holders: Vec<usize> = Vec::new();
    for i in order {
        let it = &ast.items[i];
        while let Some(&top) = holders.last() {
            let o = &ast.items[top];
            if o.lo <= it.lo && it.hi <= o.hi { break; }
            holders.pop();
        }
        if key[i].is_none() {
            let owner = holders.last().and_then(|&top| key[top].clone()).unwrap_or_default();
            key[i] = Some(format!("{}{{}}/{} {}", owner, kind_name(&it.kind), name_of(pf, it)));
        }
        holders.push(i);
    }
    // repeats (cfg'd alternatives, identical imports): numbered in order
    let key: Vec<String> = key.into_iter().map(Option::unwrap).collect();
    let mut seen: crate::fxhash::HashMap<&str, u32> = Default::default();
    let numbers: Vec<u32> = key.iter().map(|k| { let n = seen.entry(k.as_str()).or_insert(0); *n += 1; *n }).collect();
    key.into_iter().zip(numbers).map(|(mut k, n)| { k.push('#'); k.push_str(&n.to_string()); k }).collect()
}

/// Authored top-level boundaries only. Lexing stops at an unfinished literal/comment;
/// no synthetic token or bracket is ever passed to the Rust parser.
fn rust_spans(src: &[u8], edition: u16) -> Vec<(usize, usize)> {
    use crate::lexer::T;
    let mut toks = Vec::new();
    let _ = crate::lexer::lex(src, edition, &mut toks);
    let mut stack = Vec::new();
    let mut spans = Vec::new();
    let mut start = 0;
    let mut semi_item = false;
    for t in toks {
        match t.kind {
            T::KwConst | T::KwStatic | T::KwType | T::KwUse if stack.is_empty() => semi_item = true,
            // `const fn` and `const impl` have brace boundaries.
            T::KwFn | T::KwImpl if stack.is_empty() => semi_item = false,
            T::OpenBrace | T::OpenParen | T::OpenBracket => stack.push(t.kind),
            T::CloseBrace | T::CloseParen | T::CloseBracket => {
                let want = match t.kind {
                    T::CloseBrace => T::OpenBrace,
                    T::CloseParen => T::OpenParen,
                    _ => T::OpenBracket,
                };
                if stack.pop() != Some(want) { break; }
                if stack.is_empty() && t.kind == T::CloseBrace && !semi_item {
                    spans.push((start, t.hi as usize));
                    start = t.hi as usize;
                }
            }
            T::Semi if stack.is_empty() => {
                spans.push((start, t.hi as usize));
                start = t.hi as usize;
                semi_item = false;
            }
            _ => {}
        }
    }
    spans
}

pub(crate) fn complete_prefix(sess: &std::sync::Arc<crate::Session>, _path: &str, src: &[u8], edition: u16) -> Vec<u8> {
    let mut text = Vec::new();
    for (lo, hi) in rust_spans(src, edition) {
        if crate::parser::parse_owned(sess, src[lo..hi].to_vec(), edition).is_ok() {
            text.extend_from_slice(&src[lo..hi]);
            text.push(b'\n');
        }
    }
    text
}

/// Compose complete authored items over the running file. Missing identities are retained:
/// absence in a prefix is not a deletion, including after a reorder or an unfinished header.
fn compose_partial(prog: &Program, old_file: u32, new_src: &[u8]) -> Option<(Vec<u8>, usize)> {
    let sess = &prog.session;
    let started = Instant::now();
    let old = &prog.files[old_file as usize];
    let path = &prog.file_paths[old_file as usize];
    let mut text = complete_prefix(sess, path, new_src, old.edition);
    let parsed = prog.parse_version(old_file, path, text.clone()).ok()?;
    let keys = item_keys(&parsed.pf);
    let keys: std::collections::HashSet<_> = parsed.pf.ast.root_items.iter().map(|i| &keys[i.0 as usize]).collect();
    let old_keys = item_keys(old);
    let mut positions: Vec<_> = old.ast.root_items.iter().map(|i| {
        let mut pos = old.ast.item(*i).lo;
        // Expanded roots belong to their authored macro invocation.
        for _ in 0..32 {
            let Some((_, _, call)) = old.exp_map.iter_rev().find(|&(a, b, _)| a <= pos && pos < b) else { break };
            if call == pos { break; }
            pos = call;
        }
        (pos as usize, i.0 as usize)
    }).collect();
    positions.sort_unstable();
    let mut kept = 0;
    // Spans come from authored text, preserving attributes and macro invocations (AST spans
    // may point into expansion text). Match all root identities produced by that span.
    for (lo, hi) in rust_spans(&old.src[..old.src_len as usize], old.edition) {
        let a = positions.partition_point(|p| p.0 < lo);
        let b = positions.partition_point(|p| p.0 < hi);
        let roots = &positions[a..b];
        if roots.is_empty() || roots.iter().any(|&(_, i)| !keys.contains(&old_keys[i])) {
            text.extend_from_slice(&old.src[lo..hi]);
            text.push(b'\n');
            kept += 1;
        }
    }
    prog.parse_version(old_file, path, text.clone()).ok()?;
    if std::env::var_os("MAKEPAD_RUN_STREAM_TIMING").is_some() {
        eprintln!("makepad-run stream: compose_partial {} bytes {:.3} ms", new_src.len(), ms(started));
    }
    Some((text, kept))
}

/// Each item's container item (impl, trait, inline mod) in the tree walked from the root items;
/// None at the file's root and for items in fn bodies.
fn item_parents(pf: &crate::parser::ParsedFile) -> (Vec<Option<u32>>, Vec<bool>) {
    let ast = &pf.ast;
    let mut parent = vec![None; ast.items.len()];
    let mut reached = vec![false; ast.items.len()];
    fn walk(ast: &crate::ast::Ast, items: &[crate::ast::ItemId], p: Option<u32>, parent: &mut Vec<Option<u32>>, reached: &mut Vec<bool>) {
        for &iid in items {
            parent[iid.0 as usize] = p;
            reached[iid.0 as usize] = true;
            walk(ast, &child_items(ast, &ast.item(iid).kind), Some(iid.0), parent, reached);
        }
    }
    walk(ast, &ast.root_items, None, &mut parent, &mut reached);
    (parent, reached)
}

/// The module whose items are file `file`'s root items.
fn file_root_module(prog: &crate::program::Program, file: u32) -> Option<u32> {
    for (k, c) in prog.crates.iter().enumerate() {
        let _ = k;
        if prog.mods[c.root_mod as usize].file == file {
            return Some(c.root_mod);
        }
    }
    (0..prog.mods.len()).find(|&m| {
        let md = &prog.mods[m];
        md.file == file && md.lexical_parent.is_none() && md.def.0 != u32::MAX && (md.def.0 as usize) < prog.defs.len() && prog.defs[md.def.0 as usize].file != file
    }).map(|m| m as u32)
}

/// The items of two versions of a file paired by identity: (old index, new index), then the
/// items only the old version has (removed) and only the new one has (added).
/// item_keys of a file version, memoized for the next edit: the previous edit's new version is
/// the next edit's old one (a big file's keys cost tens of ms). Keyed by the version's text and
/// item spans; parsing a text is deterministic.
fn item_keys_memo(pf: &crate::parser::ParsedFile) -> std::rc::Rc<Vec<String>> {
    use std::hash::{Hash, Hasher};
    let mut h = crate::fxhash::FxHasher::default();
    pf.src.hash(&mut h);
    pf.ast.items.len().hash(&mut h);
    for it in &pf.ast.items { (it.lo, it.hi).hash(&mut h); }
    let id = h.finish();
    crate::gen_local! {
        static LAST: std::cell::RefCell<Vec<(u64, std::rc::Rc<Vec<String>>)>>;
    }
    if let Some(keys) = LAST.with(|last| last.borrow().iter().find(|(k, _)| *k == id).map(|(_, keys)| keys.clone())) { return keys; }
    let keys = std::rc::Rc::new(item_keys(pf));
    LAST.with(|last| {
        let mut last = last.borrow_mut();
        last.push((id, keys.clone()));
        if last.len() > 2 { last.remove(0); }
    });
    keys
}

pub(crate) fn pair_items(old: &crate::parser::ParsedFile, new: &crate::parser::ParsedFile) -> (Vec<(u32, u32)>, Vec<u32>, Vec<u32>) {
    let ko = item_keys_memo(old);
    let kn = item_keys_memo(new);
    let mut by_key: crate::fxhash::HashMap<&str, u32> = Default::default();
    for (i, k) in kn.iter().enumerate() {
        by_key.insert(k.as_str(), i as u32);
    }
    let mut pairs = Vec::new();
    let mut removed = Vec::new();
    let mut used = vec![false; kn.len()];
    for (i, k) in ko.iter().enumerate() {
        match by_key.get(k.as_str()) {
            Some(&j) => {
                pairs.push((i as u32, j));
                used[j as usize] = true;
            }
            None => removed.push(i as u32),
        }
    }
    let added = (0..kn.len() as u32).filter(|j| !used[*j as usize]).collect();
    (pairs, removed, added)
}

/// Adding an expanded native derive only adds an impl. Removing one or changing marker/glue
/// derives still takes the existing conservative path. Accept only plain derive lists here;
/// cfg_attr and qualified paths need their own semantic attribute comparison.
fn added_expanded_derives(old: &crate::parser::ParsedFile, a: &crate::ast::Item, new: &crate::parser::ParsedFile, b: &crate::ast::Item) -> bool {
    fn attrs(pf: &crate::parser::ParsedFile, it: &crate::ast::Item) -> (Vec<Vec<Vec<u8>>>, Vec<Vec<u8>>) {
        let mut kept = Vec::new();
        let mut expanded = Vec::new();
        for attr in pf.ast.list(it.attrs) {
            let ts = pf.toks.range(attr.toks.lo as usize, attr.toks.hi as usize);
            let text: Vec<Vec<u8>> = ts.iter().map(|t| pf.bytes(t.lo, t.hi).to_vec()).collect();
            if text.first().map(Vec::as_slice) == Some(b"derive") && ts.iter().all(|t| matches!(t.kind, crate::lexer::T::Ident | crate::lexer::T::OpenParen | crate::lexer::T::CloseParen | crate::lexer::T::Comma)) {
                let mut markers = Vec::new();
                for (t, text) in ts.iter().zip(&text).skip(1) {
                    if t.kind != crate::lexer::T::Ident { continue; }
                    if crate::parser::derive::EXPANDED.iter().any(|n| n.as_bytes() == text) {
                        expanded.push(text.clone());
                    } else { markers.push(text.clone()); }
                }
                if !markers.is_empty() { kept.push(markers); }
            } else { kept.push(text); }
        }
        (kept, expanded)
    }
    let (ak, ae) = attrs(old, a);
    let (bk, be) = attrs(new, b);
    ak == bk && ae.iter().all(|n| be.contains(n)) && be.iter().any(|n| !ae.contains(n))
}

/// Classifies the items of one file version against the running one.
/// Ok: the item pairing (old item -> new item) and the changes that apply live.
/// Err: the restart reasons (one line each).
pub(crate) fn classify(prog: &crate::program::Program, old_file: u32, new: &crate::parser::ParsedFile, path: &str) -> Result<Plan, Vec<String>> {
    let _timing = crate::parse_work::time(6);
    use crate::ast::ItemKind;
    let old = &prog.files[old_file as usize];
    let (oa, na) = (&old.ast, &new.ast);
    let pos = |pf: &crate::parser::ParsedFile, at: u32| -> String {
        let (l, c) = pf.line_col(at);
        format!("{}:{}:{}", path, l, c)
    };
    let body_span = |pf: &crate::parser::ParsedFile, kind: &ItemKind| -> Option<(u32, u32)> {
        match kind {
            ItemKind::Fn(_, Some(b), _) => { let b = pf.ast.block(*b); Some((b.lo, b.hi)) }
            ItemKind::Fn(sig, None, _) => pf.ast.fn_sig(*sig).lazy_body.map(|r| (pf.tok(r.lo - 1).lo, pf.tok(r.hi).hi)),
            _ => None,
        }
    };
    let (mut pairs, mut removed, mut added) = pair_items(old, new);
    trace_sub("classify_pair");
    // a fn whose signature changed (M2/M4): the old fn goes and a new one with the new signature
    // comes (its own slots: frames of the old code in flight keep calling the old one), and every
    // fn naming it is type-checked and rebuilt against the new signature (apply: set_names)
    {
        let skip_name = |pf: &crate::parser::ParsedFile, it: &crate::ast::Item| -> bool { pf.src[it.name.lo as usize..it.name.hi as usize].starts_with(b"__relax_") };
        let attr_toks = |pf: &crate::parser::ParsedFile, it: &crate::ast::Item| -> Vec<Vec<u8>> {
            pf.ast.list(it.attrs).iter().flat_map(|t| toks(pf, pf.toks.get(t.toks.lo as usize).map_or(0, |x| x.lo), pf.toks.get(t.toks.hi.saturating_sub(1) as usize).map_or(0, |x| x.hi), &[]).into_iter().map(|x| x.to_vec())).collect()
        };
        let mut k = 0;
        while k < pairs.len() {
            let (i, j) = pairs[k];
            let (a, b) = (&oa.items[i as usize], &na.items[j as usize]);
            let moved = match (&a.kind, &b.kind) {
                // an edited `use`: its old imports go, the new ones resolve (M13)
                (ItemKind::Use { .. }, ItemKind::Use { .. }) => !same_toks(old, a.lo, a.hi, &[], new, b.lo, b.hi, &[]),
                (ItemKind::Fn(sa, _, _), ItemKind::Fn(sb, _, _)) if !skip_name(new, b) && attr_toks(old, a) == attr_toks(new, b) => {
                    match (body_span(old, &a.kind), body_span(new, &b.kind)) {
                        (Some((alo, ahi)), Some((blo, bhi))) => !same_toks(old, a.lo, alo, &[], new, b.lo, blo, &[])
                            || (oa.fn_sig(*sa).asyncness && na.fn_sig(*sb).asyncness
                                && !same_toks(old, alo, ahi, &[], new, blo, bhi, &[])),
                        _ => false,
                    }
                }
                _ => false,
            };
            if moved {
                pairs.remove(k);
                removed.push(i);
                added.push(j);
            } else {
                k += 1;
            }
        }
    }
    let mut out = Vec::new();
    let mut why = Vec::new();
    // items added or removed (M13): free items (fns, consts, statics, types, impls) at module
    // level, and fns/consts of an existing inherent impl. The rest takes the restart rung.
    let (np, nreached) = item_parents(new);
    let (op, oreached) = item_parents(old);
    let fwd: std::collections::HashMap<u32, u32> = pairs.iter().map(|&(a, b)| (b, a)).collect();
    let mut plan_added: Vec<(u32, Result<u32, DefId>)> = Vec::new();
    let mut plan_removed: Vec<DefId> = Vec::new();
    let mut removed_uses: Vec<u32> = Vec::new();
    // (M13: a `use` and an inline `mod` with its items may be added -- their imports resolve
    // incrementally, apply_locked)
    let addable = |pf: &crate::parser::ParsedFile, k: &ItemKind| matches!(k, ItemKind::Fn(sig, _, _) if pf.ast.fn_sig(*sig).lazy_body.is_some()) || matches!(k, ItemKind::Fn(_, Some(_), _) | ItemKind::Const(..) | ItemKind::Static(..) | ItemKind::Struct(..) | ItemKind::Enum(..) | ItemKind::Union(..) | ItemKind::TypeAlias { .. } | ItemKind::Impl { .. } | ItemKind::Use { .. } | ItemKind::Mod(Some(_)));
    let inherent_impl = |pf: &crate::parser::ParsedFile, i: u32| matches!(&pf.ast.items[i as usize].kind, ItemKind::Impl(r) if pf.ast.row(*r).trait_.is_none());
    for &j in &added {
        let it = &na.items[j as usize];
        let name = String::from_utf8_lossy(&new.src[it.name.lo as usize..it.name.hi as usize]).into_owned();
        // inside an added container: collected with it
        if let Some(p) = np[j as usize] {
            if added.contains(&p) {
                continue;
            }
        }
        let at: Option<Result<u32, DefId>> = if !nreached[j as usize] {
            None
        } else {
            match np[j as usize] {
                None if addable(new, &it.kind) => file_root_module(prog, old_file).map(Ok),
                Some(p) => match (&na.items[p as usize].kind, fwd.get(&p)) {
                    (ItemKind::Mod(Some(_)), Some(&op_)) if addable(new, &it.kind) => prog.item_defs.get(&(old_file, op_)).map(|d| Ok(prog.def(*d).sub)),
                    (ItemKind::Impl { .. }, Some(&op_)) if inherent_impl(new, p) && addable(new, &it.kind) && matches!(it.kind, ItemKind::Fn(..) | ItemKind::Const(..)) => prog.item_defs.get(&(old_file, op_)).map(|d| Err(*d)),
                    _ => None,
                },
                _ => None,
            }
        };
        // (a Drop impl changes needs_drop of its type: apply_locked rebuilds what dropped it)
        match at {
            Some(a) => plan_added.push((j, a)),
            None => why.push(format!("{}: {} `{}` added here -- adding this kind of item is not applied live yet: restart the app", pos(new, it.lo), kind_name(&it.kind), name)),
        }
    }
    for &i in &removed {
        let it = &oa.items[i as usize];
        let name = String::from_utf8_lossy(&old.src[it.name.lo as usize..it.name.hi as usize]).into_owned();
        if let Some(p) = op[i as usize] {
            if removed.contains(&p) && !matches!(&oa.items[p as usize].kind, ItemKind::Impl { .. }) {
                continue;
            }
        }
        let ok = oreached[i as usize]
            && match op[i as usize] {
                None => addable(old, &it.kind) && !matches!(it.kind, ItemKind::Impl { .. }),
                Some(p) => match &oa.items[p as usize].kind {
                    ItemKind::Mod(Some(_)) => addable(old, &it.kind) && !matches!(it.kind, ItemKind::Impl { .. }) && !removed.contains(&p),
                    ItemKind::Impl(r) if oa.row(*r).trait_.is_none() => matches!(it.kind, ItemKind::Fn(..) | ItemKind::Const(..)),
                    _ => false,
                },
            };
        match (ok, prog.item_defs.get(&(old_file, i))) {
            (true, Some(&d)) => plan_removed.push(d),
            (true, None) if matches!(it.kind, ItemKind::Use { .. }) => removed_uses.push(it.lo),
            (true, None) => {} // never collected (cfg'd out)
            _ => why.push(format!("{}:1:1: {} `{}` removed -- removing this kind of item is not applied live yet: restart the app", path, kind_name(&it.kind), name)),
        }
    }
    fn attr_text<'a>(pf: &'a crate::parser::ParsedFile, attrs: &[crate::ast::Attr]) -> Vec<&'a [u8]> {
        let mut text = Vec::new();
        for attr in attrs {
            let lo = pf.toks.get(attr.toks.lo as usize).map_or(0, |t| t.lo);
            let hi = pf.toks.get(attr.toks.hi.saturating_sub(1) as usize).map_or(0, |t| t.hi);
            text.extend(toks(pf, lo, hi, &[]));
        }
        text
    }
    let mut attributes_changed = attr_text(old, &oa.root_attrs) != attr_text(new, &na.root_attrs);
    for &(i, j) in &pairs {
        let (a, b) = (&oa.items[i as usize], &na.items[j as usize]);
        let name = String::from_utf8_lossy(&new.src[b.name.lo as usize..b.name.hi as usize]).into_owned();
        let holes = |pf: &crate::parser::ParsedFile, k: &ItemKind| -> Vec<(u32, u32)> { child_items(&pf.ast, k).iter().map(|c| (pf.ast.items[c.0 as usize].lo, pf.ast.items[c.0 as usize].hi)).collect() };
        let ha = holes(old, &a.kind);
        let hb = holes(new, &b.kind);
        // attributes are part of the item (cfg, derive, repr, inline)
        // normalized like signatures: an expansion region held as text in one version and as
        // tokens in the other compares alike (a generated `#[allow(..)]` on Scope's derived
        // shader fns compared unequal, and the next edit of the file took the restart rung)
        fn attr_span(pf: &crate::parser::ParsedFile, t: &crate::ast::Attr) -> (u32, u32) { (pf.toks.get(t.toks.lo as usize).map_or(0, |x| x.lo), pf.toks.get(t.toks.hi.saturating_sub(1) as usize).map_or(0, |x| x.hi)) }
        fn attr_raw<'p>(pf: &'p crate::parser::ParsedFile, it: &crate::ast::Item) -> Vec<&'p [u8]> { pf.ast.list(it.attrs).iter().flat_map(|t| { let (lo, hi) = attr_span(pf, t); toks(pf, lo, hi, &[]) }).collect() }
        fn attr_norm(pf: &crate::parser::ParsedFile, it: &crate::ast::Item) -> Vec<Vec<u8>> { pf.ast.list(it.attrs).iter().flat_map(|t| { let (lo, hi) = attr_span(pf, t); toks_norm(pf, lo, hi, &[]) }).collect() }
        let attrs_same = attr_raw(old, a) == attr_raw(new, b) || attr_norm(old, a) == attr_norm(new, b);
        // a generated item's name differs between versions (it carries positions): compare
        // what follows it
        let skip_name = |pf: &crate::parser::ParsedFile, it: &crate::ast::Item| -> u32 { if pf.src[it.name.lo as usize..it.name.hi as usize].starts_with(b"__relax_") { it.name.hi } else { it.lo } };
        let (la, lb) = (skip_name(old, a), skip_name(new, b));
        // Placeholder IDs describe declarations, not body contents. An edited lazy file
        // produces a new provider; schedule its generated bodies without lowering them here.
        if attrs_same && same_toks(old, la, a.hi, &ha, new, lb, b.hi, &hb) {
            continue;
        }
        let def = prog.item_defs.get(&(old_file, i)).copied();
        attributes_changed |= !attrs_same;
        match (&a.kind, &b.kind) {
            (ItemKind::Fn(..), ItemKind::Fn(..)) if body_span(old, &a.kind).is_some() && body_span(new, &b.kind).is_some() => {
                let (body_a, body_b) = (body_span(old, &a.kind).unwrap().0, body_span(new, &b.kind).unwrap().0);
                // a generated fn (macro expansion text: the two versions may hold it as text or as
                // tokens) compares by its text, whitespace dropped
                let gen = name.starts_with("__relax_");
                let raw = |pf: &crate::parser::ParsedFile, lo: u32, hi: u32| -> Vec<u8> { norm_generated(&pf.src[lo as usize..hi as usize]).into_iter().filter(|c| !c.is_ascii_whitespace()).collect() };
                let sig_same = if gen { raw(old, la, body_a) == raw(new, lb, body_b) } else { same_toks(old, la, body_a, &[], new, lb, body_b, &[]) };
                if !attrs_same || !sig_same {
                    if std::env::var_os("MAKEPAD_RUN_RELOAD_TRACE").is_some() {
                        let sh = |v: Vec<Vec<u8>>| v.iter().map(|x| String::from_utf8_lossy(x).into_owned()).collect::<Vec<_>>().join(" ");
                        eprintln!("reload: signature of `{}` (attrs same {}):\n  old: {}\n  new: {}", name, attrs_same, sh(toks_norm(old, la, body_a, &[])), sh(toks_norm(new, lb, body_b, &[])));
                        if !attrs_same {
                            let at = |v: &[Vec<u8>]| v.iter().map(|x| String::from_utf8_lossy(x).into_owned()).collect::<Vec<_>>().join(" ");
                            eprintln!("  old attrs: {}\n  new attrs: {}", at(&attr_norm(old, a)), at(&attr_norm(new, b)));
                        }
                    }
                    why.push(format!("{}: the signature of `{}` changed -- callers are not re-checked live yet: restart the app", pos(new, b.lo), name));
                    continue;
                }
                match def {
                    Some(d) if matches!(prog.def(d).kind, DefKind::Fn | DefKind::AssocFn) => out.push(Change::Body(d)),
                    _ => why.push(format!("{}: fn `{}` has no definition to patch -- restart the app", pos(new, b.lo), name)),
                }
            }
            (ItemKind::Const(_, ea), ItemKind::Const(_, eb)) if ea.is_some() && eb.is_some() => {
                let (ea, eb) = (oa.expr(ea.get().unwrap()), na.expr(eb.get().unwrap()));
                if !attrs_same || toks_norm(old, la, ea.lo, &[]) != toks_norm(new, lb, eb.lo, &[]) {
                    if std::env::var_os("MAKEPAD_RUN_RELOAD_TRACE").is_some() {
                        let sh = |v: Vec<Vec<u8>>| v.iter().map(|x| String::from_utf8_lossy(x).into_owned()).collect::<Vec<_>>().join(" ");
                        eprintln!("reload: type of const `{}` (attrs same {}; old {}..{} of {}, new {}..{} of {}):\n  old: {}\n  new: {}", name, attrs_same, la, ea.lo, old.src_len, lb, eb.lo, new.src_len,
                            sh(toks_norm(old, la, ea.lo, &[])).chars().take(600).collect::<String>(), sh(toks_norm(new, lb, eb.lo, &[])).chars().take(600).collect::<String>());
                    }
                    why.push(format!("{}: the type of const `{}` changed -- restart the app", pos(new, b.lo), name));
                    continue;
                }
                match def {
                    Some(d) => out.push(Change::Const(d)),
                    None => why.push(format!("{}: const `{}` has no definition to patch -- restart the app", pos(new, b.lo), name)),
                }
            }
            (ItemKind::Static(_, _, ea), ItemKind::Static(_, _, eb)) if ea.is_some() && eb.is_some() => {
                let (ea, eb) = (oa.expr(ea.get().unwrap()), na.expr(eb.get().unwrap()));
                if !attrs_same || toks_norm(old, la, ea.lo, &[]) != toks_norm(new, lb, eb.lo, &[]) {
                    why.push(format!("{}: the type of static `{}` changed; its storage has the old size -- restart the app (the value is not migrated yet)", pos(new, b.lo), name));
                    continue;
                }
                match def {
                    Some(d) => out.push(Change::StaticInit(d)),
                    None => why.push(format!("{}: static `{}` has no definition -- restart the app", pos(new, b.lo), name)),
                }
            }
            // a named-field struct whose fields changed and nothing else (M10, hotreload.md
            // section 13): in place when the running layout can take it (decided with the types)
            (ItemKind::MacroRules(_), ItemKind::MacroRules(_)) if attrs_same => {
                let mut call = new.src[b.name.lo as usize..b.name.hi as usize].to_vec();
                call.push(b'!');
                // Retired versions remain for running frames, but they are not other source
                // files to re-expand. Only files still attached to a module are current.
                let elsewhere = prog.files.iter().enumerate().any(|(f, pf)| f as u32 != old_file && prog.mods.iter().any(|m| m.file == f as u32) && !prog.file_paths[f].contains("/rustlib/") && pf.src[..pf.src_len as usize].windows(call.len()).any(|w| w == &call[..]));
                match prog.syms.get(&name) {
                    Some(sym) if !elsewhere => out.push(Change::Macro(sym)),
                    _ => why.push(format!("{}: macro `{}` changed and is used in other files (their expansions are not redone live yet) -- restart the app", pos(new, b.lo), name)),
                }
            }
            (ItemKind::Struct(..), ItemKind::Struct(..)) | (ItemKind::Enum(..), ItemKind::Enum(..))
                if toks_norm(old, la, a.hi, &ha) == toks_norm(new, lb, b.hi, &hb) && added_expanded_derives(old, a, new, b) => {
                    // Native expanded derives add ordinary impl items, classified above.
                    // The ADT's layout and marker/glue derives did not change.
                }
            (ItemKind::Struct(ra), ItemKind::Struct(rb)) if matches!(oa.row(*ra).data, crate::ast::VariantData::Struct(_)) && matches!(na.row(*rb).data, crate::ast::VariantData::Struct(_)) && attrs_same && struct_head(old, a) == struct_head(new, b) => match def {
                Some(d) => out.push(Change::Layout(d)),
                None => why.push(format!("{}: struct `{}` has no definition -- restart the app", pos(new, b.lo), name)),
            },
            (ItemKind::Enum(..), ItemKind::Enum(..)) if attrs_same && struct_head(old, a) == struct_head(new, b) => match def {
                Some(d) => out.push(Change::Layout(d)),
                None => why.push(format!("{}: struct `{}` has no definition -- restart the app", pos(new, b.lo), name)),
            },
            (_, k) => why.push(format!("{}: {} `{}` changed: {} -- restart the app", pos(new, b.lo), kind_name(k), name, restart_reason(k))),
        }
    }
    if why.is_empty() {
        Ok(Plan { pairs, changes: out, added: plan_added, removed: plan_removed, removed_uses, attributes_changed })
    } else {
        if std::env::var_os("MAKEPAD_RUN_RELOAD_TRACE").is_some() {
            // which items paired and which did not, with their keys (a restart that the
            // source does not explain is a difference between the two parses)
            let (ko, kn) = (item_keys(old), item_keys(new));
            eprintln!("reload-trace: classify {}: old {} items ({} root, {} parts, src {} B), new {} items ({} root, src {} B), {} paired, {} removed, {} added",
                path, oa.items.len(), oa.root_items.len(), old.parts().count(), old.src.len(), na.items.len(), na.root_items.len(), new.src.len(), pairs.len(), removed.len(), added.len());
            for &i in &removed { eprintln!("reload-trace:   removed old#{} reached={} {:?}", i, oreached[i as usize], ko[i as usize]); }
            for &j in &added { eprintln!("reload-trace:   added new#{} reached={} {:?}", j, nreached[j as usize], kn[j as usize]); }
        }
        Err(why)
    }
}

/// The source tokens of a def's item (for the conservative "mentions" test of const users).
/// A struct item's tokens before its field list (visibility, name, generics, where clause).
fn struct_head<'a>(pf: &'a crate::parser::ParsedFile, it: &crate::ast::Item) -> Vec<&'a [u8]> {
    let mut out = Vec::new();
    for t in pf.toks.iter() {
        if t.lo < it.lo || t.hi > it.hi {
            continue;
        }
        let x = &pf.src[t.lo as usize..t.hi as usize];
        if x == b"{" {
            break;
        }
        out.push(x);
    }
    out
}

/// The running struct `d` (its fields in slot order) and its new version (`fresh`, fields in
/// declaration order) reconciled into the struct the running values can be (M10): a field keeps
/// its slot by name and type (a renamed one by type, when exactly one field went and one came
/// with that type), an added one goes to the extension block, a removed one whose type needs no
/// drop stays as a dead slot. Anything else: Err with the reason (restart rung).
fn layout_edit(u: &Unit, d: DefId, fresh: &crate::tcx::AdtT, running_discriminants: &[u128], fresh_discriminants: &[u128]) -> Result<(crate::tcx::AdtT, Vec<String>), String> {
    let old = u.tcx.adts.get(&d).ok_or("no running definition")?;
    let name = u.prog.def_path(d);
    if old.is_enum {
        return enum_edit(u, d, old, fresh, running_discriminants, fresh_discriminants);
    }
    let t = {
        let args: Vec<TyId> = (0..old.n_generics).map(|_| u.tcx.tys.unit).collect();
        u.tcx.tys.intern(crate::types::TyKind::Adt(d, u.tcx.tys.mk_list(&args), crate::types::RegionArgsId::EMPTY))
    };
    if !u.lay.live_editable(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, d, old, t) {
        return Err(format!("struct `{}` keeps its first layout in a live session only when it is in the app scope, has named fields, Rust's repr and no Copy impl -- restart the app", name));
    }
    trace_sub("layout_edit_live_editable");
    if let Some(outer) = outside_value_embedding(u, d) {
        return Err(format!("struct `{}` is embedded by value in `{}` outside the app scope -- restart the app", name, crate::typeck::type_name(&u.prog, &u.tcx, outer)));
    }
    trace_sub("layout_edit_embedding");
    if fresh.n_generics != old.n_generics || fresh.variants.len() != 1 {
        return Err(format!("struct `{}`: its generics changed -- restart the app", name));
    }
    let dead = |s: &str| s.starts_with("__relax_dead_");
    let mut fields = old.variants[0].fields.clone();
    let n_old = fields.len();
    let mut notes = Vec::new();
    let new_fields = &fresh.variants[0].fields;
    let mut taken = vec![false; n_old];
    let mut placed: Vec<Option<usize>> = vec![None; new_fields.len()];
    for (j, nf) in new_fields.iter().enumerate() {
        if let Some(k) = fields.iter().position(|f| f.name == nf.name) {
            if fields[k].ty != nf.ty {
                // retyped: the old slot goes dead (below), the field comes again as an added one
                // with its default in running values
                notes.push(format!("field `{}` of `{}` changed its type: running values get the new type's default", u.prog.syms.str(nf.name), name));
                continue;
            }
            taken[k] = true;
            placed[j] = Some(k);
        }
    }
    // renames: one field went and one came with the same type
    let gone: Vec<usize> = (0..n_old).filter(|&k| !taken[k] && !dead(u.prog.syms.str(fields[k].name))).collect();
    let came: Vec<usize> = (0..new_fields.len()).filter(|&j| placed[j].is_none()).collect();
    for &j in &came {
        let same: Vec<usize> = gone.iter().copied().filter(|&k| !taken[k] && fields[k].ty == new_fields[j].ty).collect();
        let others: Vec<usize> = came.iter().copied().filter(|&x| placed[x].is_none() && new_fields[x].ty == new_fields[j].ty).collect();
        if same.len() == 1 && others.len() == 1 {
            let k = same[0];
            notes.push(format!("field `{}` of `{}` renamed to `{}` (value kept)", u.prog.syms.str(fields[k].name), name, u.prog.syms.str(new_fields[j].name)));
            fields[k].name = new_fields[j].name;
            fields[k].is_pub = new_fields[j].is_pub;
            fields[k].vis_mod = new_fields[j].vis_mod;
            taken[k] = true;
            placed[j] = Some(k);
        }
    }
    // removed: a dead slot, when nothing needs dropping in it
    for k in 0..n_old {
        if taken[k] || dead(u.prog.syms.str(fields[k].name)) {
            continue;
        }
        // a dead slot that owns memory is dropped with the values built before the edit (their ext
        // word's flag bit is clear; lower.rs ext_drop_fn); values built after it leave it unset.
        // In the extension block a running value may set it after the edit (its block grows
        // later): that needs its type's Default (the declared default went with the field)
        if adt_ext_field(old, k) && u.needs_drop_glue(fields[k].ty) && !u.has_default(fields[k].ty) {
            return Err(format!("struct `{}`: field `{}` (added live earlier) removed, and its type owns memory without a Default -- restart the app", name, u.prog.syms.str(fields[k].name)));
        }
        let hidden = format!("__relax_dead_{}_{}", k, u.prog.syms.str(fields[k].name));
        if !notes.iter().any(|n| n.contains(&format!("field `{}` of `{}` changed its type", u.prog.syms.str(fields[k].name), name))) {
            notes.push(format!("field `{}` of `{}` removed (its slot stays unused)", u.prog.syms.str(fields[k].name), name));
        }
        fields[k].name = u.prog.syms.intern_late(&hidden);
    }
    // added: in the extension block, after the ones earlier edits added; a retyped scalar
    // takes its old value converted when the widening is lossless
    let mut adt = old.clone();
    let fsize = |p: crate::program::Prim| -> u32 { if p == crate::program::Prim::F64 { 8 } else { 4 } };
    let widens = |from: TyId, to: TyId| -> bool {
        use crate::types::TyKind::*;
        match (u.tcx.tys.kind(from), u.tcx.tys.kind(to)) {
            (Int(a), Int(b)) => {
                let (sa, ga) = crate::types::Types::int_info(*a);
                let (sb, gb) = crate::types::Types::int_info(*b);
                (ga == gb && sb >= sa) || (!ga && gb && sb > sa)
            }
            (Float(a), Float(b)) => fsize(*a) <= fsize(*b),
            (Int(a), Float(b)) => crate::types::Types::int_info(*a).0 < fsize(*b),
            _ => false,
        }
    };
    for (j, nf) in new_fields.iter().enumerate() {
        if placed[j].is_none() {
            // the retyped field's old slot (by its old name, in the base layout)
            let src = (0..n_old).find(|&k| old.variants[0].fields[k].name == nf.name && (adt.ext_from == u32::MAX || (k as u32) < adt.ext_from));
            if let Some(k) = src {
                if widens(old.variants[0].fields[k].ty, nf.ty) {
                    adt.ext_conv.push((fields.len() as u32, k as u32));
                    notes.retain(|n| !n.starts_with(&format!("field `{}` of `{}` changed its type", u.prog.syms.str(nf.name), name)));
                    notes.push(format!("field `{}` of `{}` widened (values kept)", u.prog.syms.str(nf.name), name));
                    if adt.ext_from == u32::MAX {
                        adt.ext_from = n_old as u32;
                    }
                    fields.push(nf.clone());
                    continue;
                }
            }
            if !u.has_default(nf.ty) && u.field_default_const(d, nf.name).is_none() {
                return Err(format!("struct `{}`: field `{}` added, and its type has no Default for the running values -- restart the app", name, u.prog.syms.str(nf.name)));
            }
            notes.push(format!("field `{}` added to `{}` (running values get its default)", u.prog.syms.str(nf.name), name));
            if adt.ext_from == u32::MAX {
                adt.ext_from = n_old as u32;
            }
            fields.push(nf.clone());
        }
    }
    adt.variants[0].fields = fields;
    Ok((adt, notes))
}

/// The crates of the files this session's edits changed (their code must never come from the
/// disk cache, whose keys predate the edits).
static EDITED_CRATES: relax_session::sync::Mutex<Vec<u32>> = relax_session::sync::Mutex::new(Vec::new());

pub(crate) fn edited_crates() -> Vec<u32> {
    EDITED_CRATES.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn note_edited_crates(u: &Unit, files: &[u32]) {
    let mut g = EDITED_CRATES.lock().unwrap_or_else(|e| e.into_inner());
    for &f in files {
        if let Some(&c) = u.prog.file_crate.get(f as usize) {
            if !g.contains(&c) {
                g.push(c);
            }
        }
    }
}

/// Source bodies can see an edit only in its crate and its transitive dependents.
/// Walk reverse edges once for the whole transaction, including companion files.
pub(crate) fn crates_seeing_edit(prog: &crate::program::Program, files: impl Iterator<Item = u32>) -> Vec<bool> {
    prog.crates_seeing_files(files)
}

/// A scope-preserving edit moved the program from collect epoch `from` to `from + 1`: Gandalf
/// keeps the verdicts (and queued checks) of owners in crates that cannot see the edited files
/// instead of re-checking the whole program (borrowck::lazy::carry_epoch). Front-end write
/// lock held (on the owner thread).
fn carry_gandalf(u: &Unit, from: u64, visible: &[bool], files: Option<&[u32]>) {
    let src = std::sync::Arc::as_ptr(&u.front) as usize;
    let prog = &u.prog;
    crate::borrowck::lazy::carry_epoch(src, from, u.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst), &|d: DefId| {
        if let Some(files) = files { !files.contains(&prog.def(d).file) }
        else { gandalf_keeps(prog, visible, d) }
    });
}

/// Whether an edit keeps what every owner outside the edited crates reads: bodies, const
/// values and static initializers only -- no item or `use` came or went (a changed signature
/// is a removed and an added fn), no field list (layout) and no macro changed. Otherwise the
/// background borrow checker drops all its verdicts, as on any other collect epoch.
pub(crate) fn gandalf_carries<'a>(changes: &[Change], _plans: impl Iterator<Item = &'a Plan>) -> bool {
    // Lead decision 52 (R1): items or `use`s that came or went are seen only by the crates
    // crates_seeing_edit returns, as with a body edit; owners of every other crate keep theirs.
    !changes.iter().any(|c| matches!(c, Change::Layout(_) | Change::Macro(_)))
}

/// An owner keeps its verdict over a carried edit when its crate cannot see the edited files
/// (`visible`: crates_seeing_edit); owners of the edited crates and of every crate depending
/// on them are checked again.
pub(crate) fn gandalf_keeps(prog: &Program, visible: &[bool], d: DefId) -> bool {
    (d.0 as usize) < prog.defs.len() && !visible.get(prog.def(d).krate as usize).copied().unwrap_or(true)
}

/// Whether `d` is still the program's (bound in its module, or in its impl's item list): an
/// edit that replaced or removed a fn unbinds it, while code in flight keeps its slots.
fn def_is_bound(prog: &crate::program::Program, d: DefId) -> bool {
    let def = prog.def(d);
    let parent = def.parent;
    if parent != crate::program::NO_DEF && prog.def(parent).kind == DefKind::Impl {
        return prog.impls.get(prog.def(parent).sub as usize).map_or(false, |i| i.items.contains(&d));
    }
    prog.mods.get(def.scope as usize).map_or(false, |m| m.values.get(&def.name) == Some(&d) || m.types.get(&def.name) == Some(&d))
}

/// Whether type `t` names an ADT of one of `crates`.
fn ty_in_crates(u: &Unit, t: TyId, crates: &[u32], depth: u32) -> bool {
    use crate::types::TyKind::*;
    if depth > 16 {
        return true;
    }
    match u.tcx.tys.kind(t).clone() {
        Adt(d, args, _) => crates.contains(&u.prog.def(d).krate) || u.tcx.tys.list(args).iter().any(|a| ty_in_crates(u, *a, crates, depth + 1)),
        Ref(_, x, _) | Ptr(_, x) | Slice(x) | Array(x, _) => ty_in_crates(u, x, crates, depth + 1),
        Tuple(v) => u.tcx.tys.list(v).iter().any(|a| ty_in_crates(u, *a, crates, depth + 1)),
        Closure(..) | FnDef(..) | Dyn(..) | Alias(..) => true,
        _ => false,
    }
}

/// Whether type `t` names one of the ADTs `ds` (by value, through references, generics, tuples).
fn ty_mentions(u: &Unit, t: TyId, ds: &[DefId], depth: u32) -> bool {
    use crate::types::TyKind::*;
    if depth > 16 {
        return false;
    }
    match u.tcx.tys.kind(t).clone() {
        Adt(d, args, _) => ds.contains(&d) || u.tcx.tys.list(args).iter().any(|a| ty_mentions(u, *a, ds, depth + 1)),
        Ref(_, x, _) | Ptr(_, x) | Slice(x) | Array(x, _) => ty_mentions(u, x, ds, depth + 1),
        Tuple(v) => u.tcx.tys.list(v).iter().any(|a| ty_mentions(u, *a, ds, depth + 1)),
        _ => false,
    }
}

fn adt_ext_field(a: &crate::tcx::AdtT, k: usize) -> bool {
    a.ext_from != u32::MAX && k as u32 >= a.ext_from
}

/// Inspect concrete instances, substituting actual field types rather than treating type
/// arguments as storage (Box<T>, references and PhantomData<T> do not embed T by value).
fn outside_value_embedding(u: &Unit, edited: DefId) -> Option<TyId> {
    use crate::types::TyKind;
    let app: Vec<bool> = (0..u.prog.crates.len()).map(|c| crate::borrowck::event::app_crate(&u.prog, c as u32)).collect();
    fn embeds(u: &Unit, t: TyId, edited: DefId, seen: &mut std::collections::HashSet<TyId>) -> bool {
        if !seen.insert(t) { return false; }
        match u.tcx.tys.kind(t) {
            TyKind::Adt(d, args, _) => *d == edited || u.tcx.adts.get(d).is_some_and(|a|
                a.variants.iter().enumerate().any(|(vi, v)| v.fields.iter().enumerate().any(|(fi, _)| {
                    let field = u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), *d, vi as u32, fi as u32, u.tcx.tys.list(*args));
                    embeds(u, field, edited, seen)
                }))),
            TyKind::Tuple(ts) => u.tcx.tys.list(*ts).iter().any(|t| embeds(u, *t, edited, seen)),
            TyKind::Array(t, n) => n.try_to_target_usize(&u.tcx.tys) != Some(0) && embeds(u, *t, edited, seen),
            _ => false,
        }
    }
    // A type can embed `edited` only if it names `edited` or an ADT whose declared fields could
    // hold it (`holders`), directly or through its type arguments: a declared field type names
    // only items its crate can see -- `edited` only in the crates that can see `edited`'s crate --
    // and anything else reaches the field through a substituted argument; like embeds(), only
    // arrays and tuples carry a value further, and an alias may normalize to a holder. An ADT
    // of another crate whose declared fields
    // project (`T::Out`) may also reach it through an impl for an argument, so an instance of
    // one naming any type of the seeing crates counts too. Skipping every other type leaves the
    // answer unchanged (lead decision 52 R1: after a scoped publication, embeds() normalized
    // ~40k unrelated instances' fields again, ~0.5 s per field-list edit).
    struct Holders { adts: Vec<DefId>, projecting: std::collections::HashSet<DefId>, seeing: Vec<u32> }
    fn may_hold(u: &Unit, t: TyId, h: &Holders, depth: u32) -> bool {
        use crate::types::TyKind::*;
        if depth > 16 { return true; }
        match u.tcx.tys.kind(t) {
            Adt(d, args, _) => h.adts.contains(d) || (h.projecting.contains(d) && ty_in_crates(u, t, &h.seeing, 0))
                || u.tcx.tys.list(*args).iter().any(|a| may_hold(u, *a, h, depth + 1)),
            // (as embeds(): by value through arrays and tuples only)
            Array(x, _) => may_hold(u, *x, h, depth + 1),
            Tuple(v) => u.tcx.tys.list(*v).iter().any(|a| may_hold(u, *a, h, depth + 1)),
            // an alias normalizes through an impl of its trait's crate or of a crate of a type
            // in its arguments (orphan rules), to a type made of those crates' items and parts
            // of its arguments: a holder only if a seeing crate is among them or an argument
            // may hold one
            Alias(_, a) => h.seeing.contains(&u.prog.def(a.kind.def_id()).krate)
                || u.tcx.tys.list(a.args).iter().any(|x| ty_in_crates(u, *x, &h.seeing, 0) || may_hold(u, *x, h, depth + 1)),
            _ => false,
        }
    }
    fn projects(u: &Unit, t: TyId, depth: u32) -> bool {
        use crate::types::TyKind::*;
        if depth > 16 { return true; }
        match u.tcx.tys.kind(t) {
            Alias(..) | Closure(..) | Dyn(..) => true,
            Adt(_, args, _) => u.tcx.tys.list(*args).iter().any(|a| projects(u, *a, depth + 1)),
            Ref(_, x, _) | Ptr(_, x) | Slice(x) | Array(x, _) => projects(u, *x, depth + 1),
            Tuple(v) => u.tcx.tys.list(*v).iter().any(|a| projects(u, *a, depth + 1)),
            _ => false,
        }
    }
    let seeing = crates_seeing_edit(&u.prog, [u.prog.def(edited).file].into_iter());
    let sees = |d: &DefId| seeing.get(u.prog.def(*d).krate as usize).copied().unwrap_or(true);
    let mut h = Holders { adts: vec![edited], projecting: Default::default(), seeing: (0..seeing.len() as u32).filter(|&k| seeing[k as usize]).collect() };
    for (d, a) in u.tcx.adts.iter() {
        if !sees(d) && a.variants.iter().any(|v| v.fields.iter().any(|f| projects(u, f.ty, 0))) { h.projecting.insert(*d); }
    }
    loop {
        let mut grew = false;
        for (d, a) in u.tcx.adts.iter() {
            if h.adts.contains(d) || !sees(d) { continue; }
            if a.variants.iter().any(|v| v.fields.iter().any(|f| may_hold(u, f.ty, &h, 0))) {
                h.adts.push(*d);
                grew = true;
            }
        }
        if !grew { break; }
    }
    let mut seen = std::collections::HashSet::new();
    for i in 0..u.tcx.tys.len() {
        let t = TyId(i as u32);
        if let TyKind::Adt(d, _, _) = u.tcx.tys.kind(t) {
            if !app[u.prog.def(*d).krate as usize] && u.tcx.tys.is_concrete(t)
                && may_hold(u, t, &h, 0)
                // The interner also holds binder payloads, not running instances.
                // Nightly rustc_middle/src/ty/instance.rs:519-524 rejects escaping
                // bound vars; erase_regions.rs:65-72 deliberately preserves them.
                && !u.tcx.tys.has_escaping_bound_vars(t)
                && embeds(u, t, edited, &mut seen) {
                return Some(t);
            }
        }
    }
    None
}

/// Use the same formatter as lower.rs's `type_name` intrinsic. Generic structs publish
/// each concrete instantiation known to the session, matching the widget registry's keys.


/// An enum's edit in a live session (M10): variants added after the running ones (which keep
/// their names, fields and discriminants) apply in place when every value keeps its
/// representation -- live enums are always tagged and give outer types no niche (layout.rs),
/// so the check is the size, alignment and every running variant's field offsets.
fn enum_edit(u: &Unit, d: DefId, old: &crate::tcx::AdtT, fresh: &crate::tcx::AdtT, running_discriminants: &[u128], fresh_discriminants: &[u128]) -> Result<(crate::tcx::AdtT, Vec<String>), String> {
    let name = u.prog.def_path(d);
    if !u.lay.live_editable_enum(&crate::layout::LayoutCx::new(&u.prog), d, old) || !fresh.is_enum || fresh.n_generics != old.n_generics {
        return Err(format!("enum `{}` keeps its first layout in a live session only when it is the program's own with Rust's repr -- restart the app", name));
    }
    let n = old.variants.len();
    if fresh.variants.len() < n {
        return Err(format!("enum `{}`: a variant was removed; a running value may hold it -- restart the app", name));
    }
    for (i, (vo, vn)) in old.variants.iter().zip(fresh.variants.iter()).enumerate() {
        let same_fields = vo.fields.len() == vn.fields.len() && vo.fields.iter().zip(vn.fields.iter()).all(|(a, b)| a.name == b.name && a.ty == b.ty);
        if vo.name != vn.name || vo.shape != vn.shape || running_discriminants[i] != fresh_discriminants[i] || !same_fields {
            return Err(format!("enum `{}`: variant `{}` changed, moved or was removed; running values would be read as another variant -- restart the app", name, u.prog.syms.str(vo.name)));
        }
    }
    if fresh.variants.len() == n {
        return Ok((old.clone(), Vec::new()));
    }
    // (the layout check: apply_locked, once the new definition is in -- enum_fits)
    let notes = fresh.variants[n..].iter().map(|v| format!("variant `{}` added to `{}`", u.prog.syms.str(v.name), name)).collect();
    Ok((fresh.clone(), notes))
}

/// The layout of every compiled instance of enum `d` before (`before`) and after an edit: the
/// running values keep their representation (size, alignment, tag, running variants' fields).
fn enum_fits(before: &[(TyId, crate::layout::Layout)], u: &Unit) -> Result<(), String> {
    for (t, b) in before {
        let a = u.layout_of(*t);
        if a.size != b.size || a.align != b.align || a.tag != b.tag || b.variant_fields.iter().zip(a.variant_fields.iter()).any(|(x, y)| x != y) {
            return Err(format!("enum `{}`: the added variant does not fit the running values' layout ({} -> {} bytes) -- restart the app", crate::typeck::type_name(&u.prog, &u.tcx, *t), b.size, a.size));
        }
    }
    Ok(())
}

fn item_mentions(prog: &crate::program::Program, d: DefId, names: &[&[u8]]) -> bool {
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    let it = pf.ast.item(def.item);
    toks(pf, it.lo, it.hi, &[]).iter().any(|t| names.contains(t))
}

fn def_name(prog: &crate::program::Program, d: DefId) -> String {
    prog.syms.str(prog.def(d).name).to_string()
}

/// RELOAD' (lead design, round 33): whether the running value of executable discriminant
/// `anon` may change with this transaction, decided without running it. Its dependencies are
/// the transitive static callee set of its initializer slot -- each slot's direct calls before
/// inlining and the calls, fn values and thunks of its built RIR -- plus the constants a slot's
/// build read (their initializer slots, walked the same way), the statics it addresses and
/// the type arguments of every instance (a field-list edit of one changes what size_of and
/// field offsets fold to). Ok(true): one of them is in `changed`. Err: the set is not exactly
/// computable (an indirect call, a trait's associated constant, a RIR build failure).
unsafe fn discriminant_deps_changed(up: *mut Unit, anon: DefId, changed: &[DefId], layout_defs: &[DefId]) -> Result<bool, String> {
    use crate::rir::{Callee, Inst};
    let mut todo = vec![(&*up).fn_id(FnKey::Inst(anon, Vec::new()))];
    let mut seen: Vec<u32> = Vec::new();
    while let Some(s) = todo.pop() {
        if seen.contains(&s) { continue; }
        seen.push(s);
        let u = &*up;
        let mut k = s;
        let hit = loop {
            match &u.fns[k as usize].key {
                FnKey::Inst(d, args) => break changed.contains(d) || args.iter().any(|t| ty_mentions(u, *t, layout_defs, 0)),
                FnKey::Glue(_, t) => break ty_mentions(u, *t, layout_defs, 0),
                FnKey::Shim(shim) => break shim.args.iter().any(|t| ty_mentions(u, *t, layout_defs, 0)),
                FnKey::Closure(_, _, parent) | FnKey::Sandbox(parent) => k = *parent,
            }
        };
        if hit { return Ok(true); }
        if u.fns[s as usize].rir.is_none() {
            jit::build_rir(up, s).map_err(|e| format!("`{}`: {}", (&*up).fns[s as usize].name, e))?;
        }
        let u = &*up;
        let Some(rir) = u.fns[s as usize].rir.clone() else { return Err(format!("`{}` has no RIR", u.fns[s as usize].name)) };
        todo.extend_from_slice(&u.fns[s as usize].calls);
        for i in rir.blocks.iter().flat_map(|b| &b.insts) {
            match i {
                Inst::Call(Callee::Fn(c) | Callee::CFn(c, _), _, _) | Inst::FnAddr(_, c) | Inst::SlotAddr(_, c) => todo.push(*c),
                Inst::Call(Callee::Indirect(_) | Callee::CIndirect(..), _, _) => return Err(format!("an indirect call in `{}`", u.fns[s as usize].name)),
                Inst::Addr(_, sym) => match u.syms.kind(sym.id) {
                    Some(crate::syms::SymKind::Thunk(c)) => todo.push(c),
                    Some(crate::syms::SymKind::Static(d)) if changed.contains(&d) => return Ok(true),
                    _ => {}
                },
                _ => {}
            }
        }
        let read: Vec<DefId> = u.const_readers.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|(_, v)| v.contains(&s)).map(|(c, _)| *c).collect();
        for c in read {
            if changed.contains(&c) { return Ok(true); }
            if u.prog.def(c).kind == DefKind::AssocConst && u.prog.def(u.prog.def(c).parent).kind == DefKind::Trait {
                return Err(format!("`{}` reads a trait's associated constant", u.fns[s as usize].name));
            }
            let keys: Vec<Vec<TyId>> = u.consts.keys().filter(|(x, _)| *x == c).map(|(_, a)| a.clone()).collect();
            for args in keys { todo.push(u.fn_id(FnKey::Inst(c, args))); }
        }
    }
    Ok(false)
}

/// Slots referenced by compiled code (calls and fn values: table words and thunks).
fn referenced_slots(relocs: &[crate::rir::RelocAt], out: &mut Vec<u32>) {
    for r in relocs {
        match r.target {
            crate::rir::Reloc::Slot(id) | crate::rir::Reloc::Thunk(id) => out.push(id),
            _ => {}
        }
    }
}

type Code = (Vec<u8>, Vec<(u32, u64)>, Vec<crate::rir::RelocAt>);

/// A closure's identity across versions of its parent: its ordinal among the closure
/// expressions of the parent's item (source order), with its signature, captures and owner
/// types. Coroutines (async blocks and bodies) are not matched: their suspended states keep
/// the old poll code until coroutine versioning exists.
fn closure_ident(u: &Unit, slot: u32) -> Option<(u32, TyId, Vec<TyId>, TyId)> {
    let (file, ce, parent) = match &u.fns[slot as usize].key {
        FnKey::Closure(f, e, p) => (*f, *e, *p),
        _ => return None,
    };
    if ce & crate::tcx::ASYNC_FN_BODY != 0 || file == u32::MAX {
        return None;
    }
    let body = u.body_of(parent, file)?;
    let ast = &u.prog.files[file as usize].ast;
    if !matches!(ast.expr(crate::ast::ExprId(ce)).kind, crate::ast::ExprKind::Closure { .. }) {
        return None;
    }
    if !body.has_expr(crate::ast::ExprId(ce)) {
        return None;
    }
    // the closures before it in the body's expressions (the item's own range, then its lazily
    // parsed body's)
    let mut ord = 0;
    for i in 0..body.expr_ix(crate::ast::ExprId(ce)) {
        let e = body.expr_at(i);
        if ast.has_expr(e) && matches!(ast.expr(e).kind, crate::ast::ExprKind::Closure { .. }) {
            ord += 1;
        }
    }
    let ty = u.tcx.normalize_erasing_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), body.ty(crate::ast::ExprId(ce)));
    match u.tcx.tys.kind(ty).clone() {
        crate::types::TyKind::Closure(_, _, crate::types::ClosureArgs { parent_args: owner, closure_sig_as_fn_ptr_ty: sig, .. }) => {
            let storage = crate::layout::ClosureStorage::for_codegen(&u.prog, &u.tcx, &body, ce);
            Some((ord, sig, storage.fields, owner))
        },
        _ => None,
    }
}

unsafe fn compile_guarded(up: *mut Unit, id: u32) -> Result<Code, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| jit::compile_fn(up, id))) {
        Ok(r) => r,
        Err(p) => {
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".to_string());
            if p.is::<crate::preempt::OwnerBusy>() {
                std::panic::resume_unwind(p);
            }
            let u = &mut *up;
            u.fns[id as usize].building = false;
            Err(format!("internal compiler error in {}: {} -- makepad run bug, report it", u.fns[id as usize].name, msg))
        }
    }
}

/// A run-time module's load: compiles `roots` (its wrappers), the module code they reach and the
/// program functions that code calls directly, nothing installed. The program's own functions
/// beyond those compile as they always do (lazily, or already built): following them, or the
/// slots other threads create meanwhile, made a load during a cold start compile the starting
/// app's whole graph (17,317 slots, 1.6 s seen). Err: errors in functions `mine` claims; errors
/// in others are returned with them as their first call would report them. Called on the owner thread.
/// `own`: the module's functions (its twins and wrappers, or a trusted module's own code), whose
/// callees are followed.
pub(crate) unsafe fn compile_reachable(up: *mut Unit, roots: Vec<u32>, mine: &dyn Fn(&Unit, u32) -> bool, own: &dyn Fn(&Unit, u32) -> bool) -> Result<Vec<(u32, Result<Code, String>)>, Vec<String>> {
    compile_reachable_in(up, roots, Vec::new(), mine, PRECOMPILE_MAX * 4, Some(own), None)
}

pub(crate) unsafe fn compile_reachable_from(up: *mut Unit, roots: Vec<u32>, seeds: Vec<u32>, mine: &dyn Fn(&Unit, u32) -> bool, cap: usize, n0: usize) -> Result<Vec<(u32, Result<Code, String>)>, Vec<String>> {
    compile_reachable_in(up, roots, seeds, mine, cap, None, Some(n0))
}

/// As compile_reachable, also following the relocations of code built already (`seeds`), and
/// compiling at most `cap` functions. Works in waves: each wave's host functions build off the unit
/// for their back ends (jit::compile_batch_off_lock); twins build on it.
/// Reload supplies `n0` to follow only newly created slots; module loads have no such limit.
unsafe fn compile_reachable_in(up: *mut Unit, roots: Vec<u32>, seeds: Vec<u32>, mine: &dyn Fn(&Unit, u32) -> bool, cap: usize, module: Option<&dyn Fn(&Unit, u32) -> bool>, n0: Option<usize>) -> Result<Vec<(u32, Result<Code, String>)>, Vec<String>> {
    let mut out: Vec<(u32, Result<Code, String>)> = Vec::new();
    let mut seen: std::collections::HashSet<u32> = seeds.into_iter().collect();
    let mut frontier: Vec<u32> = roots;
    let mut scanned = (&*up).fns.len();
    let mut errs = Vec::new();
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8);
    loop {
        let mut wave: Vec<u32> = Vec::new();
        for id in frontier.drain(..) {
            if !seen.insert(id) || n0.map_or(false, |n| (id as usize) < n) {
                continue;
            }
            let f = &(&*up).fns[id as usize];
            if f.compiled || f.building || f.inflight || crate::sandbox::is_sandboxed_slot(&*up, id) {
                continue;
            }
            wave.push(id);
        }
        if wave.is_empty() {
            if module.is_some() {
                break;
            }
            // slots created by the builds so far (closures, instances, vtable entries)
            let n = (&*up).fns.len();
            if scanned >= n {
                break;
            }
            frontier.extend((scanned..n).map(|k| k as u32));
            scanned = n;
            continue;
        }
        if out.len() + wave.len() > cap {
            wave.truncate(cap.saturating_sub(out.len()));
            if wave.is_empty() {
                break;
            }
        }
        // twins and module wrappers build under the lock (a twin's transform places sandbox
        // data; a wrapper's calls are redirected to twins), the rest off it
        let (twins, host): (Vec<u32>, Vec<u32>) = wave.into_iter().partition(|id| under_lock(&*up, *id));
        // code of crates no edit touched (std's monomorphisations a new derive reaches): from the
        // disk cache when it has them (its keys hold the crates' fingerprints from the run's
        // start, so an edited crate's code is never taken from it)
        let mut built: Vec<(u32, Result<Code, String>)> = Vec::new();
        let mut to_compile: Vec<u32> = Vec::new();
        for id in host {
            let cacheable = {
                let u = &*up;
                match &u.fns[id as usize].key {
                    FnKey::Inst(d, args) => {
                        let ed = edited_crates();
                        !ed.contains(&u.prog.def(*d).krate) && !args.iter().any(|a| ty_in_crates(u, *a, &ed, 0))
                    }
                    _ => false,
                }
            };
            match if cacheable { jit::cache_lookup(up, id) } else { None } {
                Some(c) => {
                    jit::count_cache_ahead();
                    built.push((id, Ok(c)));
                }
                None => to_compile.push(id),
            }
        }
        built.extend(jit::compile_batch_off_lock(up, &to_compile, threads));
        for id in twins {
            let r = compile_guarded(up, id);
            built.push((id, r));
        }
        for (id, r) in built {
            if matches!((&*up).fns[id as usize].key, FnKey::Sandbox(_)) {
                frontier.extend(crate::sandbox::extra_refs(id));
                frontier.extend(crate::sandbox::extra_refs(u32::MAX));
            }
            // a module's load stops at the program functions its code calls
            let follow = module.map_or(true, |own| own(&*up, id));
            match &r {
                Ok(c) if follow => referenced_slots(&c.2, &mut frontier),
                Ok(_) => {}
                Err(e) if mine(&*up, id) => errs.push(format!("{}: {}", (&*up).fns[id as usize].name, e)),
                Err(_) => {}
            }
            out.push((id, r));
        }
        if !errs.is_empty() || out.len() >= cap {
            break;
        }
    }
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}

/// A sandboxed module's own functions: its twins and wrappers.
pub(crate) fn module_own(u: &Unit, id: u32) -> bool {
    under_lock(u, id)
}

/// Slots whose build must stay on the owner thread: sandbox twins and module wrappers.
fn under_lock(u: &Unit, id: u32) -> bool {
    match &u.fns[id as usize].key {
        FnKey::Sandbox(_) => true,
        FnKey::Inst(d, _) => crate::sandbox::is_wrapper_crate(u.prog.def(*d).krate),
        _ => false,
    }
}

/// Most functions the precompile of one edit may compile (beyond it, they compile lazily)
const PRECOMPILE_MAX: usize = 4000;
/// Of those, compiled before an edit's install; the rest go to the background compilers
const PRECOMPILE_SYNC: usize = 300;

/// Applies `edits` as one transaction. The caller makes a unit call.
pub unsafe fn apply(up: *mut Unit, edits: Vec<Edit>) -> Outcome {
    apply_mode(up, edits, 1, false)
}

/// Final module text must pass in full: no partial parse or type-check fallback.
pub(crate) unsafe fn apply_full(up: *mut Unit, edits: Vec<Edit>) -> Outcome {
    apply_mode(up, edits, 0, true)
}

unsafe fn apply_mode(up: *mut Unit, edits: Vec<Edit>, partial: u8, module: bool) -> Outcome {
    // one transaction at a time (its back ends run off the unit)
    static PATCHING: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());
    let transaction = TraceTransaction::begin(edits.len());
    let _p = PATCHING.lock().unwrap_or_else(|e| e.into_inner());
    let _reloading = jit::Reloading::begin();
    let t0 = Instant::now();
    let changes: Vec<_> = edits.iter().map(|e| (e.path.as_str(), Some(e.src.as_slice()))).collect();
    if let Err(error) = crate::native_macros::reload(&changes, (&(*up).prog.session).runtime.profile) {
        let mut out = idle_outcome(None, &error);
        out.rung = Rung::Refuse;
        return out;
    }
    let edits: Vec<_> = edits.into_iter().filter(|e| !crate::native_macros::owns_path(&e.path)).collect();
    if edits.is_empty() { return idle_outcome(None, "native macro sources match the running compiler"); }
    // an internal compiler panic in the transaction refuses the edit with its message (the app
    // keeps running the code it had; a half-applied state is not installed)
    let files: Vec<u32> = edits.iter().map(|e| e.file).collect();
    let r = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // takes the parse snapshots of the edits (under the unit call)
        let snap = |edits: &[Edit]| {
            let macro_table = (*up).prog.macros.clone();
            let epoch = (*up).tcx.collect_epoch.load(std::sync::atomic::Ordering::Acquire);
            // Whole-file candidates parse off the unit; partial Rust retries stay on it.
            let off_lock = partial != 2;
            let snapshots: Vec<_> = if off_lock {
                let prog = &(*up).prog;
                let mut contexts = std::collections::BTreeMap::new();
                for e in edits {
                    let krate = prog.file_crate[e.file as usize];
                    let context = contexts.entry(krate).or_insert_with(|| prog.crates[krate as usize].context.clone());
                    *context = context.edited(&e.path, e.src.clone().into());
                }
                edits.iter().map(|e| {
                    let mut snapshot = prog.parse_snapshot(e.file, &e.path);
                    snapshot.context = contexts[&prog.file_crate[e.file as usize]].clone();
                    Some(snapshot)
                }).collect()
            } else { Vec::new() };
            (macro_table, epoch, snapshots)
        };
        let (mut edits, mut cur) = call_traced("lock", || {
            trace_stage("lock");
            trace_count("program_defs", (&(*up).prog.defs).len());
            trace_count("program_slots", (*up).fns.len());
            trace_count("program_files", (*up).prog.files.len());
            // A stream and the disk watcher may edit the same file. Resolve stale watcher indices
            // under the lock, before the parse snapshot and the pairing against the running version.
            let edits: Vec<Edit> = {
                let prog = &(*up).prog;
                edits.into_iter().map(|mut e| {
                    if !module {
                        let path = &prog.file_paths[e.file as usize];
                        if let Some(file) = prog.mods.iter().map(|m| m.file).find(|&f| prog.file_paths[f as usize] == *path) { e.file = file; }
                    }
                    e
                }).collect()
            };
            let cur = snap(&edits);
            (edits, cur)
        });
        let mut stale = 0;
        loop {
            let (macro_table, epoch, snapshots) = cur;
            // Only owned snapshots are reachable here; never retain a Unit/Program reference
            // across the unlock. Lazy stubs and background/spec workers can compile meanwhile.
            trace_stage("snapshot");
            let pre: Vec<Option<Result<crate::program::FileVersion, String>>> = edits.iter().zip(snapshots).map(|(e, s)| s.map(|s| s.parse(e.src.clone()))).collect();
            trace_stage("reparse");
            let step = call_traced("relock", || {
                trace_stage("relock");
                // Runtime module loads publish macro tables; rollback also changes the collection
                // epoch. Neither may make this snapshot stale while the parser runs off the unit.
                if std::sync::Arc::ptr_eq(&macro_table, &(*up).prog.macros) && epoch == (*up).tcx.collect_epoch.load(std::sync::atomic::Ordering::Acquire) {
                    drop(macro_table); // allow commit's Arc::make_mut to update an unshared table
                    return Ok(apply_locked(up, edits, pre, t0, partial, 8, module));
                }
                stale += 1;
                if stale == 3 {
                    return Ok(Outcome { kept: 0, pending: false, rung: Rung::Refuse, files: files.clone(), lines: vec!["program context changed while parsing -- retry the edit; running code is unchanged".into()], names: Vec::new(), changed: 0, group: 0, ms: [ms(t0), 0.0, 0.0, 0.0], undo: None, work: [0; 5] });
                }
                let cur = snap(&edits);
                Err((edits, cur))
            });
            match step {
                Ok(outcome) => break outcome,
                Err((e, c)) => { edits = e; cur = c; }
            }
        }
    })) {
        Ok(r) => r,
        Err(p) => {
            let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            Outcome { kept: 0, pending: false, rung: Rung::Restart, files, lines: vec![format!("internal compiler error applying the edit: {} -- makepad run bug, report it; restart the app", msg)], names: Vec::new(), changed: 0, group: 0, ms: [0.0; 4], undo: None, work: [0; 5] }
        }
    };
    crate::owner::call(crate::owner::Class::App, || {
        (*up).reload_rir = None;
        jit::close_patch();
    });
    transaction.finish(&r);
    r
}

/// Live edit transactions so far: a field-view summary context kept by one transaction is
/// used by the next only when no other transaction ran in between.
static TRANSACTIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
crate::gen_local! {
    /// The field-view summaries of the program as the last patched edit left it (the
    /// transaction number it was kept by).
    static FV_SESSION: std::cell::RefCell<Option<(u64, std::rc::Rc<crate::borrowck::fv::SummaryCx>)>>;
}
fn fv_session_take(transaction: u64) -> std::rc::Rc<crate::borrowck::fv::SummaryCx> {
    match FV_SESSION.with(|s| s.borrow_mut().take()) {
        Some((kept, cx)) if kept + 1 == transaction => cx,
        _ => crate::borrowck::fv::SummaryCx::new(),
    }
}
fn fv_session_keep(transaction: u64, cx: std::rc::Rc<crate::borrowck::fv::SummaryCx>) {
    FV_SESSION.with(|s| *s.borrow_mut() = Some((transaction, cx)));
}

unsafe fn apply_locked(up: *mut Unit, edits: Vec<Edit>, mut pre: Vec<Option<Result<crate::program::FileVersion, String>>>, t0: Instant, partial: u8, retries: usize, module: bool) -> Outcome {
    let transaction = TRANSACTIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    let u = &mut *up;
    let olds: Vec<u32> = edits.iter().map(|e| e.file).collect();
    note_edited_crates(u, &olds);
    let cache_ahead0 = jit::CACHE_AHEAD.load(std::sync::atomic::Ordering::Relaxed);
    let mut out = Outcome { kept: 0, pending: false, rung: Rung::Refuse, files: olds.clone(), lines: Vec::new(), names: Vec::new(), changed: 0, group: 0, ms: [0.0; 4], undo: None, work: [0; 5] };
    // 1-2: parse and classify every file; nothing changes yet
    let mut versions = Vec::new();
    let mut changes = Vec::new();
    let mut restart = Vec::new();
    let mut kept = None;
    for (i, e) in edits.into_iter().enumerate() {
        let initial = if partial == 2 {
            Err("incomplete".to_string())
        } else if let Some(p) = pre.get_mut(i).and_then(Option::take) {
            p
        } else { u.prog.parse_version(e.file, &e.path, e.src.clone()) };
        let parsed = initial.or_else(|err| {
            if partial != 0 && e.path.ends_with(".rs") {
                if let Some((src, n)) = compose_partial(&u.prog, e.file, &e.src) {
                    kept = Some(kept.unwrap_or(0) + n);
                    return u.prog.parse_version(e.file, &e.path, src);
                }
            }
            Err(err)
        });
        match parsed {
            Err(m) => out.lines.push(m),
            Ok(v) => {
                trace_count("classify_items", v.pf.ast.items.len() + u.prog.files[e.file as usize].ast.items.len());
                match classify(&u.prog, e.file, &v.pf, &e.path) {
                    Ok(plan) if module && (plan.removed.iter().any(|d| u.prog.defs[d.0 as usize].is_pub) || plan.changes.iter().any(|c| matches!(c, Change::Layout(_)))) => {
                        // the host holds typed pointers to a module's pub items (exports): one
                        // removed or re-signatured (remove + add) needs a fresh module, and its
                        // types have no live layout migration (M10 covers the root crate's
                        // structs). Private items removed unbind as in a program file.
                        restart.push(format!("{}:1:1: a pub item of the module was removed or changed its signature, or a layout changed -- unload and load the new text", e.path));
                    }
                    Ok(mut plan) => {
                        changes.extend(std::mem::take(&mut plan.changes));
                        versions.push((v, plan));
                    }
                    Err(w) => restart.extend(w),
                }

            }
        }
    }
    out.kept = kept.unwrap_or(0);
    trace_stage("classify");
    if !out.lines.is_empty() {
        out.ms[0] = ms(t0);
        return out;
    }
    if kept.is_some() && changes.is_empty() && restart.is_empty()
        && versions.iter().all(|(_, p)| p.added.is_empty() && p.removed.is_empty() && p.removed_uses.is_empty()) {
        out.rung = Rung::Patch;
        out.lines.push(format!("incomplete: {} item(s) kept at their running version", kept.unwrap()));
        out.ms[0] = ms(t0);
        return out;
    }
    // Lead decisions 49-50: checking a changed const fn must not execute
    // candidate code. Nightly uses the candidate MIR (rustc_const_eval/src/
    // const_eval/machine.rs:418-425 @8d1a764); this runtime needs a restart.
    for change in &changes {
        if let Change::Body(def) = *change {
            let d = u.prog.def(def);
            let pf = &u.prog.files[d.file as usize];
            if let crate::ast::ItemKind::Fn(sig, _, _) = pf.ast.item(d.item).kind {
                if pf.ast.fn_sig(sig).konst {
                    restart.push(format!("const fn `{}` changed: checking its users requires the new code -- restart the app", def_name(&u.prog, def)));
                }
            }
        }
    }
    // a const that sized a type (array length, const generic argument) changes types
    for c in &changes {
        if let Change::Const(d) = c {
            if u.tcx.const_was_evaluated(*d) {
                restart.push(format!("const `{}` is used in a type (an array length or const generic argument): its users' types change -- restart the app", def_name(&u.prog, *d)));
            }
        }
    }
    if !restart.is_empty() {
        out.rung = Rung::Restart;
        out.lines = restart;
        out.ms[0] = ms(t0);
        return out;
    }
    let visible_crates = crates_seeing_edit(&u.prog, versions.iter().map(|(v, _)| v.old));
    let mut body_defs: Vec<DefId> = changes.iter().filter_map(|c| if let Change::Body(d) = c { Some(*d) } else { None }).collect();
    // items added or removed change what names resolve to (a new fn shadowing a glob import, a
    // new inherent method shadowing a trait method, a removed fn): every running fn whose text
    // names one of them is type-checked and rebuilt with the edit (conservative, by name)
    let mut set_names: Vec<Vec<u8>> = Vec::new();
    // (scoreboard: fns naming a removed/renamed field, of which this many use it)
    let (mut field_named, mut field_deps) = (0usize, 0usize);
    // a changed macro: the fns invoking it are rebuilt from their new expansions
    for c in &changes {
        if let Change::Macro(n) = c {
            set_names.push(u.prog.syms.str(*n).as_bytes().to_vec());
        }
    }
    // names a removed or edited `use` bound: every fn naming one is type-checked against what
    // the name means now (M13)
    for (v, plan) in &versions {
        for &pos in &plan.removed_uses {
            for im in u.prog.imports.iter().filter(|im| im.file == v.old && im.pos == pos) {
                if let Some(b) = im.bind {
                    set_names.push(u.prog.syms.str(b).as_bytes().to_vec());
                }
            }
        }
    }
    // a struct's field-list edit (M10): every fn naming the struct, or a field that came, went or
    // was renamed, is type-checked and rebuilt with it
    let layout_defs: Vec<DefId> = changes.iter().filter_map(|c| if let Change::Layout(d) = c { Some(*d) } else { None }).collect();
    for &d in &layout_defs {
        // an enum's matches see the new variants; a struct's added fields concern only code
        // that names them (its text changed); removed or renamed ones every fn naming them
        if u.prog.def(d).kind == DefKind::Enum {
            set_names.push(def_name(&u.prog, d).into_bytes());
        }
        let old_names: Vec<Vec<u8>> = u.tcx.adts.get(&d).map(|a| a.variants[0].fields.iter().map(|f| u.prog.syms.str(f.name).as_bytes().to_vec()).collect()).unwrap_or_default();
        let mut new_names: Vec<Vec<u8>> = Vec::new();
        for (v, plan) in &versions {
            for &(i, j) in &plan.pairs {
                if u.prog.item_defs.get(&(v.old, i)) != Some(&d) {
                    continue;
                }
                if let Some(crate::ast::VariantData::Struct(fs)) = match &v.pf.ast.items[j as usize].kind { crate::ast::ItemKind::Struct(r) => Some(v.pf.ast.row(*r).data), _ => None } {
                    for f in v.pf.ast.list(fs) {
                        if let Some(n) = f.name {
                            new_names.push(v.pf.text(n).as_bytes().to_vec());
                        }
                    }
                }
            }
        }
        // a removed or renamed field: the fns that use it (resolved field uses on this struct,
        // found by type-checking the fns that name it), not every fn naming a same-named thing
        let gone: Vec<u32> = old_names.iter().enumerate().filter(|(_, n)| !new_names.contains(n)).map(|(k, _)| k as u32).collect();
        if !gone.is_empty() {
            let names: Vec<&[u8]> = gone.iter().map(|&k| old_names[k as usize].as_slice()).collect();
            let mut seen: Vec<DefId> = Vec::new();
            let mut named = 0usize;
            for f in u.fns.iter() {
            trace_count("slot_visits", 1);
                if let FnKey::Inst(fd, _) = &f.key {
                    if !visible_crates[u.prog.def(*fd).krate as usize] { continue; }
                    // (a fn an earlier edit replaced keeps its slots for code in flight; it is no
                    // longer the program's and is never checked against the new definitions)
                    if !(f.compiled || f.rir.is_some()) || seen.contains(fd) || !matches!(u.prog.def(*fd).kind, DefKind::Fn | DefKind::AssocFn) || body_defs.contains(fd) || !def_is_bound(&u.prog, *fd) {
                        continue;
                    }
                    seen.push(*fd);
                    if !item_mentions(&u.prog, *fd, &names) {
                        continue;
                    }
                    named += 1;
                    let uses = crate::borrowck::identity_args(&u.tcx, &u.prog, *fd).map(|a| crate::typeck::instantiate_body(&crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &crate::tcx::EvalState::new()), &u.prog, &u.tcx, *fd, &a).field_uses).unwrap_or_default();
                    if uses.iter().any(|(ud, k)| *ud == d && gone.contains(k)) {
                        body_defs.push(*fd);
                        field_deps += 1;
                    }
                }
            }
            field_named += named;
        }
    }
    for (v, plan) in &versions {
        let na = &v.pf.ast;
        for &(j, _) in &plan.added {
            let it = &na.items[j as usize];
            set_names.push(v.pf.src[it.name.lo as usize..it.name.hi as usize].to_vec());
            // a trait impl's methods shadow nothing (the trait's own methods resolve as before
            // for every other type); an inherent impl's may hide a trait method of its type
            if matches!(&it.kind, crate::ast::ItemKind::Impl(r) if na.row(*r).trait_.is_some()) {
                continue;
            }
            for c in child_items(na, &it.kind) {
                let ci = na.item(c);
                set_names.push(v.pf.src[ci.name.lo as usize..ci.name.hi as usize].to_vec());
            }
        }
        for &d in &plan.removed {
            set_names.push(def_name(&u.prog, d).into_bytes());
            if u.prog.def(d).kind == DefKind::Impl {
                for &x in &u.prog.impls[u.prog.def(d).sub as usize].items {
                    set_names.push(def_name(&u.prog, x).into_bytes());
                }
            }
        }
        out.changed += plan.added.len() + plan.removed.len();
    }
    set_names.retain(|n| !n.is_empty());
    if !set_names.is_empty() {
        let names: Vec<&[u8]> = set_names.iter().map(|n| n.as_slice()).collect();
        let std_crates: Vec<u32> = (0..u.prog.crates.len() as u32).filter(|&c| matches!(u.prog.syms.str(u.prog.crates[c as usize].name), "core" | "alloc" | "std" | "proc_macro")).collect();
        let mut seen: Vec<DefId> = Vec::new();
        for f in u.fns.iter() {
            trace_count("slot_visits", 1);
            if let FnKey::Inst(d, _) = &f.key {
                if !visible_crates[u.prog.def(*d).krate as usize] { continue; }
                if (f.compiled || f.rir.is_some()) && !seen.contains(d) && !std_crates.contains(&u.prog.def(*d).krate) && matches!(u.prog.def(*d).kind, DefKind::Fn | DefKind::AssocFn) {
                    seen.push(*d);
                    if !body_defs.contains(d) && def_is_bound(&u.prog, *d) && item_mentions(&u.prog, *d, &names) {
                        body_defs.push(*d);
                    }
                }
            }
        }
    }
    // Removed functions keep their old slots for code in flight. In particular, generated
    // child styles name their own entry: the dependency scan above must not schedule that
    // deleted body against the new namespace (or its retired lazy parse) when a child goes.
    body_defs.retain(|d| !versions.iter().any(|(_, plan)| plan.removed.contains(d)));
    let const_defs: Vec<DefId> = changes.iter().filter_map(|c| if let Change::Const(d) = c { Some(*d) } else { None }).collect();
    let static_defs: Vec<DefId> = changes.iter().filter_map(|c| if let Change::StaticInit(d) = c { Some(*d) } else { None }).collect();
    trace_count("changed_consts", const_defs.len());
    trace_count("changed_statics", static_defs.len());
    out.changed += changes.len();
    // consts whose value depends on a changed one: the readers' slots (consts compiled as
    // initializers) and, conservatively, every evaluated const whose initializer names one
    let mut consts_hit: Vec<DefId> = const_defs.clone();
    while !consts_hit.is_empty() {
        let mut grew = false;
        let names: Vec<Vec<u8>> = consts_hit.iter().map(|d| def_name(&u.prog, *d).into_bytes()).collect();
        let names: Vec<&[u8]> = names.iter().map(|n| n.as_slice()).collect();
        let readers = u.const_readers.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut cands: Vec<DefId> = u.consts.keys().map(|(d, _)| *d).collect();
        for d in &consts_hit {
            for &s in readers.get(d).map(|v| v.as_slice()).unwrap_or(&[]) {
                if let FnKey::Inst(c, _) = &u.fns[s as usize].key {
                    if matches!(u.prog.def(*c).kind, DefKind::Const | DefKind::AssocConst | DefKind::Static) {
                        cands.push(*c);
                    }
                }
            }
        }
        cands.sort_by_key(|d| d.0);
        cands.dedup();
        for c in cands {
            if !visible_crates[u.prog.def(c).krate as usize] { continue; }
            if consts_hit.contains(&c) || u.prog.def(c).kind == DefKind::Static {
                continue;
            }
            let via_slot = readers.iter().any(|(d, v)| consts_hit.contains(d) && v.iter().any(|&s| matches!(&u.fns[s as usize].key, FnKey::Inst(x, _) if *x == c)));
            if via_slot || item_mentions(&u.prog, c, &names) {
                if u.tcx.const_was_evaluated(c) {
                    out.rung = Rung::Restart;
                    out.lines.push(format!("const `{}` depends on a changed const and is used in a type -- restart the app", def_name(&u.prog, c)));
                    out.ms[0] = ms(t0);
                    return out;
                }
                consts_hit.push(c);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    // the slots to rebuild: every instance of a changed fn or const initializer, the readers of
    // every affected const, and (transitively) the callers that inlined any of them
    let n0 = u.fns.len();
    let mut slots: Vec<u32> = Vec::new();
    let mut fresh: Vec<u32> = Vec::new(); // slots of changed defs never compiled: compiled now too
    {
        let readers = u.const_readers.lock().unwrap_or_else(|e| e.into_inner());
        for id in u.slots_of_defs(body_defs.iter().chain(&consts_hit).copied()) {
            trace_count("slot_visits", 1);
            if u.fns[id as usize].compiled { slots.push(id); } else { fresh.push(id); }
        }
        // A changed field list invalidates drop glue. Expanded derive bodies
        // are ordinary changed definitions above.
        if !layout_defs.is_empty() {
            let is_it = |t: TyId| matches!(u.tcx.tys.kind(t), crate::types::TyKind::Adt(d, _, _) if layout_defs.contains(d));
            for (i, f) in u.fns.iter().enumerate() {
            trace_count("slot_visits", 1);
                let hit = match &f.key {
                    FnKey::Glue(k, t) => (*k == crate::jit::GLUE_DROP || *k == crate::jit::GLUE_EXT_DROP) && is_it(*t),
                    _ => false,
                };
                if hit && !slots.contains(&(i as u32)) && !fresh.contains(&(i as u32)) {
                    if f.compiled {
                        slots.push(i as u32);
                    } else if f.rir.is_some() {
                        fresh.push(i as u32);
                    }
                }
            }
        }
        for d in &consts_hit {
            for &s in readers.get(d).map(|v| v.as_slice()).unwrap_or(&[]) {
                if slots.contains(&s) || fresh.contains(&s) {
                    continue;
                }
                // a reader only ever inlined (never called) still holds the folded value in its RIR
                if u.fns[s as usize].compiled {
                    slots.push(s);
                } else if u.fns[s as usize].rir.is_some() {
                    fresh.push(s);
                }
            }
        }
    }
    // Callers that inlined a rebuilt slot (also one only ever inlined, never called),
    // and twins built from that slot's RIR. Expand both dependencies here so every
    // affected RIR/speculation is invalidated before any member of the group rebuilds.
    // Compiled callers rebuilt because they inlined changed code: a call of one already
    // running finishes in its old code (hotreload.md 3.1/3.4), as an edited fn's own running
    // frame does; reported below, since a loop that never returns there never picks it up.
    let mut inlining_callers: Vec<u32> = Vec::new();
    let mut work: Vec<u32> = slots.iter().chain(fresh.iter()).copied().collect();
    while let Some(s) = work.pop() {
        let inl = u.fns[s as usize].history().inlined_into.len();
        let mut deps = u.fns[s as usize].history().inlined_into.clone();
        deps.extend(crate::sandbox::twin_of(s));
        for (k, d) in deps.into_iter().enumerate() {
            if slots.contains(&d) || fresh.contains(&d) {
                continue;
            }
            if u.fns[d as usize].compiled {
                if k < inl { inlining_callers.push(d); }
                slots.push(d);
            } else {
                fresh.push(d);
            }
            work.push(d);
        }
    }
    trace_count("selected_slots", slots.len() + fresh.len());
    trace_stage("dependencies");
    // Settle can evict a current closure owner's typeck result. Restore it before its
    // source version is replaced, including owners of already compiled closures: O3
    // and closure matching may need those old bodies long after this edit.
    let old_files: Vec<u32> = versions.iter().map(|(v, _)| v.old).collect();
    if let Err(e) = jit::preserve_closure_bodies(up, &old_files) {
        out.rung = Rung::Refuse;
        out.lines.push(e);
        return out;
    }
    // 3: closures (and coroutine bodies) of the slots about to be rebuilt that were never
    // compiled stay lazy: a stored value of one runs the version that made it, lowered on its
    // first call from that version's checked body, which preserve_closure_bodies (above) has
    // made sure exists and put_body keeps in old_bodies once the new version is checked.
    // (They were compiled here eagerly: ~1.4 s on the first edit of Scope's handle_event.)
    let u = &mut *up;
    trace_stage("freeze");
    // Round-32 lead design: DefIds are retargeted by commit_version. Values
    // queried from an old AdtT after that point already belong to the new text.
    // RELOAD' (round 33): no discriminant is evaluated by running code in a transaction.
    // An edited enum is compared only when const folding gives all its old and new values;
    // an unedited enum computed by code keeps its values when nothing they depend on changes.
    let mut running_discriminants = Vec::new();
    let mut discr_restart: Vec<String> = Vec::new();
    for &d in &layout_defs {
        if let Some(adt) = u.tcx.adts.get(&d) {
            if adt.is_enum {
                match adt.fold_discriminants(&u.tcx, &u.prog) {
                    Some(values) => running_discriminants.push((d, values)),
                    None => discr_restart.push(format!("enum `{}`: its declaration changed and a discriminant is computed by code -- restart the app", def_name(&u.prog, d))),
                }
                for variant in &adt.variants {
                    if let crate::tcx::VariantDiscr::Explicit(def) = variant.discr {
                        if !consts_hit.contains(&def) { consts_hit.push(def); }
                    }
                }
            }
        }
    }
    {
        let mut changed: Vec<DefId> = body_defs.iter().chain(&consts_hit).chain(&static_defs).chain(&layout_defs).copied().collect();
        for (_, plan) in &versions { changed.extend_from_slice(&plan.removed); }
        let names: Vec<&[u8]> = set_names.iter().map(|n| n.as_slice()).collect();
        let mut enums: Vec<DefId> = u.tcx.adts.iter().filter(|(d, a)| a.is_enum && visible_crates[u.prog.def(**d).krate as usize] && !layout_defs.contains(d) && !changed.contains(d)).map(|(d, _)| *d).collect();
        enums.sort_unstable_by_key(|d| d.0);
        for d in enums {
            let u = &*up;
            let adt = &u.tcx.adts[&d];
            let mut code: Vec<DefId> = Vec::new();
            for v in &adt.variants {
                if let crate::tcx::VariantDiscr::Explicit(c) = v.discr {
                    if relax_types::const_eval::fold_without_execution(&u.tcx, &u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), c).is_none() { code.push(c); }
                }
            }
            if code.is_empty() { continue; }
            if item_mentions(&u.prog, d, &names) {
                discr_restart.push(format!("enum `{}`: a discriminant computed by code names an item this edit adds or removes -- restart the app", def_name(&u.prog, d)));
                continue;
            }
            for c in code {
                match discriminant_deps_changed(up, c, &changed, &layout_defs) {
                    Ok(false) => {}
                    Ok(true) => discr_restart.push(format!("enum `{}`: a discriminant is computed by code this edit changes -- restart the app", def_name(&(*up).prog, d))),
                    Err(e) => discr_restart.push(format!("enum `{}`: a discriminant is computed by code whose dependencies are not known exactly ({}) -- restart the app", def_name(&(*up).prog, d), e)),
                }
            }
        }
    }
    trace_sub("gp_discriminants");
    if !discr_restart.is_empty() {
        out.rung = Rung::Restart;
        out.lines = discr_restart;
        out.ms[0] = ms(t0);
        return out;
    }
    let u = &mut *up;
    let enum_before: Vec<(DefId, TyId, crate::layout::Layout)> = {
        let mut v = Vec::new();
        for f in u.fns.iter().take(if layout_defs.is_empty() { 0 } else { u.fns.len() }) {
            trace_count("slot_visits", 1);
            let ts: Vec<TyId> = match &f.key {
                FnKey::Inst(_, a) => a.clone(),
                FnKey::Glue(_, t) => vec![*t],
                _ => Vec::new(),
            };
            for t in ts {
                if let crate::types::TyKind::Adt(ed, _, _) = u.tcx.tys.kind(t).clone() {
                    if layout_defs.contains(&ed) && u.tcx.adts.get(&ed).map_or(false, |a| a.is_enum) && u.tcx.tys.is_concrete(t) && !v.iter().any(|x: &(DefId, TyId, crate::layout::Layout)| x.1 == t) {
                        v.push((ed, t, u.layout_of(t).clone()));
                    }
                }
            }
        }
        for &ed in &layout_defs {
            if let Some(a) = u.tcx.adts.get(&ed) {
                if a.is_enum && a.n_generics == 0 {
                    let t = u.tcx.tys.intern(crate::types::TyKind::Adt(ed, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
                    if !v.iter().any(|x| x.1 == t) {
                        v.push((ed, t, u.layout_of(t).clone()));
                    }
                }
            }
        }
        v
    };
    trace_sub("gp_enum_before");
    let same_scopes = layout_defs.is_empty() && !changes.iter().any(|c| matches!(c, Change::Macro(_)));
    // Round-45 lead design: a change to any resolution input invalidates every
    // legacy owner. Body-only transactions leave signature-owned RPITITs alone.
    let body_only = same_scopes && versions.iter().all(|(_, p)| p.added.is_empty()
        && p.removed.is_empty() && p.removed_uses.is_empty() && !p.attributes_changed);
    // Lead decision 52 (R2): a changed const or static initializer is classified after the
    // commit (const_contexts_mention, then folding the new initializer); a transaction that
    // is not body-only keeps the round-51 rule for them.
    let ctfe_items: Vec<DefId> = const_defs.iter().chain(&static_defs).copied().collect();
    let mut ctfe_relevant = !body_only && !ctfe_items.is_empty();
    for (version, _) in &versions {
        ctfe_relevant |= ctfe_relevant_bodies(&u.prog, version.old, &body_defs);
    }
    let mut full_inputs = !body_only || ctfe_relevant;
    let mut scope: Option<Vec<bool>> = None;
    let saved_consts: Vec<((DefId, Vec<TyId>), u64)>;
    let saved_failed: Vec<(DefId, Vec<TyId>)>;
    // 4: commit the versions; check
    let t1 = Instant::now();
    out.ms[0] = (t1 - t0).as_secs_f64() * 1e3;
    trace_sub("gp_ctfe_scan");
    jit::open_patch();
    // Record only RIR first materialized during this attempt, including concurrent
    // lazy compilation while the transaction releases the owner thread.
    let journal_start = u.fns.len();
    u.reload_rir = Some(Default::default());
    let file_fns: Vec<(DefId, Vec<TyId>)> = if crate::borrowck::enabled(options) {
        body_defs.iter().filter(|d| crate::borrowck::event::admit(&u.prog, u.prog.def(**d).krate)).filter_map(|d| crate::borrowck::identity_args(&u.tcx, &u.prog, *d).map(|a| (*d, a))).collect()
    } else {
        Vec::new()
    };
    trace_sub("gp_file_fns");
    // One summary context for the edit's field-view work (the summaries before it here,
    // the changed bodies' checks and the summaries after it in the gate): callee summaries
    // the edit cannot reach are computed once, not three times. Not across layout or
    // macro edits, which can change any body's places.
    // The context is the session's: the previous patched edit left its after-state summaries
    // in it, which are this edit's before-state (FV_SESSION); a refused or restarted edit, a
    // layout or macro edit, or another transaction in between starts a fresh one.
    let same_bodies_elsewhere = layout_defs.is_empty() && !changes.iter().any(|c| matches!(c, Change::Macro(_)));
    let fv_cx = (crate::borrowck::enabled(options) && same_bodies_elsewhere).then(|| fv_session_take(transaction));
    let pre_summaries = if crate::borrowck::enabled(options) { crate::borrowck::summaries_with(&u.prog, &u.tcx, &file_fns, fv_cx.as_ref()) } else { Vec::new() };
    if let Some(fv_cx) = &fv_cx { fv_cx.invalidate(&body_defs); }
    trace_sub("gp_summaries");
    let mut news: Vec<(u32, u32)> = Vec::new();
    let mut news_pairs: Vec<Vec<(u32, u32)>> = Vec::new();
    let mut added_defs: Vec<DefId> = Vec::new();
    let mut removed_defs: Vec<DefId> = Vec::new();
    let mut add_errs: Vec<String> = Vec::new();
    let mut saved_adts: Vec<(DefId, crate::tcx::AdtT)> = Vec::new();
    let saved_variants: Vec<(DefId, usize)> = layout_defs.iter().filter_map(|&d| {
        let def = u.prog.def(d);
        (def.kind == DefKind::Enum).then(|| (d, u.prog.variants[def.sub as usize].len()))
    }).collect();
    let mut layout_restart: Vec<String> = Vec::new();
    let retry_sources: Vec<_> = if kept.is_some() && retries > 0 {
        versions.iter().map(|(v, _)| (v.old, v.path.clone(), v.pf.src[..v.pf.src_len as usize].to_vec())).collect()
    } else { Vec::new() };
    let imports0 = u.prog.imports.len();
    // bodies, const values and static initializers only: no name binds or unbinds, so the
    // name-resolution memos (keyed by module and name, or by file version) stay valid --
    // clearing them retired program-sized tables and every later lookup filled them again
    let mut legacy_owners = Vec::new();
    if body_only {
        for c in &changes {
            if let Change::Body(d) | Change::Const(d) | Change::StaticInit(d) = *c {
                if !legacy_owners.contains(&d) { legacy_owners.push(d); }
            }
        }
    }
    let carry = gandalf_carries(&changes, versions.iter().map(|(_, p)| p));
    let mut use_undo: Vec<crate::program::UseUndo> = Vec::new();
    let errors0 = u.prog.errors.len();
    trace_stage("gate_prepare");
    {
        let wait_from = jit::wall_ns();
        let fw = jit::front_write();
        if std::env::var_os("MAKEPAD_RUN_FRONT_TRACE").is_some() {
            let now = jit::wall_ns();
            eprintln!("reload-front-wait: {:.1} ms (t={:.0}..{:.0} ms)", (now - wait_from) as f64 / 1e6, wait_from as f64 / 1e6, now as f64 / 1e6);
        }
        trace_stage("front_write_wait");
        let from = u.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst);
        trace_stage("gandalf_carry");
        for (v, plan) in versions {
            let old = v.old;
            // (the caches commit_version clears are freed below, under this write lock)
            let Plan { pairs, added, removed, removed_uses, .. } = plan;
            // removed or edited `use` items: their names go before the new ones bind (M13)
            for pos in removed_uses {
                use_undo.extend(u.prog.unbind_use(old, pos));
            }
            trace_sub("unbind_use");
            // (lead decision 52 R1: the declaration pass of an item edit covers the crates
            // that can see it; DeclarationState saves exactly what that pass replaces)
            let committed = if body_only { u.prog.commit_body_version(v, &pairs) } else { u.prog.commit_version_in(v, &pairs, &visible_crates) };
            match committed {
                Ok(n) => {
                    trace_sub("commit_version");
                    news.push((old, n));
                    news_pairs.push(pairs);
                    let items: Vec<(crate::ast::ItemId, Result<u32, DefId>)> = added.iter().map(|&(j, at)| (crate::ast::ItemId(j), at)).collect();
                    trace_count("added_items", items.len());
                    match u.prog.add_items(n, &items) {
                        Ok(ds) => added_defs.extend(ds),
                        Err(e) => add_errs.push(e),
                    }
                    trace_sub("add_items");
                    u.prog.unbind_defs(&removed);
                    if !removed.is_empty() {
                        // method answers that named a removed associated fn (only crates that
                        // can see the edit name it: lead decision 52 R1)
                        u.tcx.publish_declarations(Some(&visible_crates));
                    }
                    trace_count("removed_defs", removed.len());
                    trace_sub("unbind_defs");
                    removed_defs.extend(removed);
                }
                Err(e) => {
                    for (k, &(o, n)) in news.iter().enumerate().rev() {
                        u.prog.revert_version(o, n, &news_pairs[k]);
                    }
                    u.prog.rebind_use(std::mem::take(&mut use_undo));
                    for &(d, n) in &saved_variants { u.prog.truncate_variants(d, n); }
                    u.tcx.publish_collected();
                    drop(fw);
                    out.rung = Rung::Restart;
                    out.lines.push(e);
                    return out;
                }
            }
        }
        // imports the added items brought (`use`, an inline `mod`'s): resolved now, only theirs
        if u.prog.imports.len() > imports0 {
            u.prog.resolve_imports_from_in(imports0, &visible_crates);
            let new_errs: Vec<String> = u.prog.errors[errors0..].to_vec();
            u.prog.errors.truncate(errors0);
            add_errs.extend(new_errs);
        }
        trace_sub("imports");
        let lowered = if body_only {
            u.prog.lower_body_declarations(&legacy_owners)
        } else {
            crate::def_collector::lower_legacy_const_generics_in(&mut u.prog, Some(&visible_crates))
        };
        add_errs.extend_from_slice(&u.prog.errors[errors0..]);
        u.prog.errors.truncate(errors0);
        trace_count("legacy_owners", lowered);
        trace_count("body_only", usize::from(body_only));
        trace_sub("legacy_const_generics");
        // Collection records new inline/anonymous owners before publication.
        for &(_, new) in &news {
            ctfe_relevant |= ctfe_relevant_bodies(&u.prog, new, &body_defs);
        }
        if !ctfe_relevant && !ctfe_items.is_empty() {
            // R2: (b) no constant-evaluated context can read them; (a) each new initializer
            // folds without executing code (const_eval.rs fold_without_execution), read
            // through the published per-file inputs of the new version.
            ctfe_relevant = const_contexts_mention(&u.prog, &ctfe_items);
            trace_sub("publish_r2_mentions");
            if !ctfe_relevant {
                let files: Vec<u32> = news.iter().flat_map(|&(a, b)| [a, b]).collect();
                u.tcx.queries.register_files(u.prog.files.len());
                u.tcx.publish_file_bodies(&files);
                let env = crate::tcx::TypingEnv::fully_monomorphized(&u.prog);
                // A static whose storage exists is never initialized again (static_addr keeps
                // its address and value), so no step of this transaction runs its new
                // initializer; any other item must fold without execution.
                ctfe_relevant = ctfe_items.iter().any(|&d| {
                    let stored = u.prog.def(d).kind == DefKind::Static && u.statics.get(&d).is_some_and(|&a| a != 0);
                    let literal = relax_types::const_eval::const_body(&u.prog, d).is_some_and(|e| literal_initializer(&u.prog.files[u.prog.def(d).file as usize], e, 0));
                    !stored && !literal && !matches!(relax_types::const_eval::fold_without_execution(&u.tcx, &u.prog, env, d), Some(Ok(_)))
                });
                trace_sub("publish_r2_fold");
            }
            trace_count("r2_ctfe_relevant", usize::from(ctfe_relevant));
        }
        full_inputs |= ctfe_relevant;
        // Lead decision 52 (R1): an item/signature edit that is not CTFE-relevant reaches the
        // crates that can see it (queries, const values, layouts); a CTFE-relevant one keeps
        // the tip's full invalidation (round 51).
        if !ctfe_relevant { scope = Some(visible_crates.clone()); }
        trace_count("full_inputs", usize::from(full_inputs));
        trace_sub("publish_ctfe_scan");
        {
            let (prog, sc) = (&u.prog, scope.as_deref());
            let take = |d: &DefId| (full_inputs && in_scope(prog, sc, *d)) || consts_hit.contains(d);
            saved_consts = u.consts.iter().take(if full_inputs || !consts_hit.is_empty() { u.consts.len() } else { 0 }).filter(|((d, _), _)| take(d)).map(|(k, v)| (k.clone(), *v)).collect();
            saved_failed = u.const_failed.iter().take(if full_inputs || !consts_hit.is_empty() { u.const_failed.len() } else { 0 }).filter(|(d, _)| take(d)).cloned().collect();
        }
        for (key, _) in &saved_consts { u.consts.remove(key); }
        for key in &saved_failed { u.const_failed.remove(key); }
        trace_sub("publish_consts");
        publish_reload(u, &news, full_inputs, scope.as_deref());
        trace_sub("publish_inputs");
        if carry && !ctfe_relevant {
            let files: Vec<u32> = news.iter().flat_map(|&(a, b)| [a, b]).collect();
            carry_gandalf(u, from, &visible_crates, if full_inputs { None } else { Some(&files) });
        }
        trace_sub("publish_collected");
        // struct field-list edits (M10): reconciled with the running layout, or the restart rung

        for &d in &layout_defs {
            let (fresh_adt, perrs) = u.tcx.adt_fresh(&crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &crate::tcx::EvalState::new()), &u.prog, d);
            trace_sub("layout_adt_fresh");
            if !perrs.is_empty() {
                add_errs.extend(perrs);
                continue;
            }
            let Some(fresh_adt) = fresh_adt else { continue };
            let mut after = Vec::new();
            if fresh_adt.is_enum {
                // RELOAD': folded values only (every query below then folds as well)
                let Some(values) = fresh_adt.fold_discriminants(&u.tcx, &u.prog) else {
                    layout_restart.push(format!("enum `{}`: a discriminant is now computed by code -- restart the app", def_name(&u.prog, d)));
                    continue;
                };
                after = values;
                let errors = fresh_adt.check_discriminants(&u.tcx, &u.prog);
                if !errors.is_empty() { add_errs.extend(errors); continue; }
            }
            let before = running_discriminants.iter().find(|(ed, _)| *ed == d).map_or(&[][..], |(_, v)| v.as_slice());
            trace_sub("layout_discr");
            let edited = layout_edit(u, d, &fresh_adt, before, &after);
            trace_sub("layout_edit");
            match edited {
                Ok((adt, notes)) => {
                    if let Some(old) = u.tcx.adts.insert(d, adt) {
                        saved_adts.push((d, old));
                    }
                    out.lines.extend(notes);
                }
                Err(r) => layout_restart.push(r),
            }
        }
        trace_sub("layout_fresh");
        if !saved_adts.is_empty() {
            u.tcx.publish_declarations(scope.as_deref());
            trace_sub("layout_publish");
            match scope.as_deref() {
                // the layouts of types made only of items the edit cannot reach stay
                Some(visible) => {
                    let crates: Vec<u32> = (0..visible.len() as u32).filter(|&k| visible[k as usize]).collect();
                    let uu = &*u;
                    uu.lay.clear_except(&|t| !ty_in_crates(uu, t, &crates, 0));
                }
                None => u.lay.clear(),
            }
            for &ed in &layout_defs {
                let before: Vec<(TyId, crate::layout::Layout)> = enum_before.iter().filter(|x| x.0 == ed).map(|x| (x.1, x.2.clone())).collect();
                if before.is_empty() {
                    continue;
                }
                if let Err(r) = enum_fits(&before, u) {
                    layout_restart.push(r);
                }
            }
        }
        trace_sub("layout");
        // the added items' signatures, types and impl tables
        trace_count("added_defs", added_defs.len());
        if !added_defs.is_empty() {
            let e = u.tcx.collect_added_in(&u.prog, &added_defs, false, scope.as_deref());
            add_errs.extend(e);
        }
        trace_sub("collect_added");
        unsafe {
            u.prog.reclaim_caches();
        }
        drop(fw);
        trace_sub("reclaim");
    }
    let added_impls: Vec<DefId> = added_defs.iter().copied().filter(|d| u.prog.def(*d).kind == DefKind::Impl).collect();
    // an added `impl Drop`: the type now needs dropping where code compiled before left it alone
    // -- the drop glue of every type holding it, the generic instances over it (`Vec<T>`'s
    // drop_in_place) and every fn naming the type are rebuilt; values made before the edit are
    // dropped by the new glue
    {
        let drop_trait = u.tcx.lang_items.get(LangItem::Drop);
        let mut dtypes: Vec<DefId> = Vec::new();
        for &imp in &added_impls {
            if drop_trait.is_some() && u.tcx.impl_trait.get(&imp).copied() == drop_trait {
                if let Some(&st) = u.tcx.impl_self.get(&imp) {
                    if let crate::types::TyKind::Adt(d, _, _) = u.tcx.tys.kind(st) {
                        dtypes.push(*d);
                    }
                }
            }
        }
        if !dtypes.is_empty() {
            u.unwind_drop_cache.clear();
            let mentions = |u: &Unit, t: TyId| -> bool { ty_mentions(u, t, &dtypes, 0) };
            let names: Vec<Vec<u8>> = dtypes.iter().map(|d| def_name(&u.prog, *d).into_bytes()).collect();
            let nref: Vec<&[u8]> = names.iter().map(|n| n.as_slice()).collect();
            for i in 0..u.fns.len() {
                let f = &u.fns[i];
                let hit = match &f.key {
                    FnKey::Glue(_, t) => mentions(u, *t),
                    // Exact type dependencies still rebuild instances such as Vec<Edited>
                    // and drop_in_place<Edited> defined in dependency crates. Only the
                    // conservative source-name scan requires crate visibility.
                    FnKey::Inst(d, args) => args.iter().any(|a| mentions(u, *a)) || (visible_crates[u.prog.def(*d).krate as usize] && matches!(u.prog.def(*d).kind, DefKind::Fn | DefKind::AssocFn) && !u.prog.file_paths[u.prog.def(*d).file as usize].contains("/rustlib/") && def_is_bound(&u.prog, *d) && item_mentions(&u.prog, *d, &nref)),
                    _ => false,
                };
                if hit && !slots.contains(&(i as u32)) && !fresh.contains(&(i as u32)) {
                    if f.compiled {
                        slots.push(i as u32);
                    } else if f.rir.is_some() {
                        fresh.push(i as u32);
                    }
                }
            }
        }
    }
    trace_sub("drop_glue");
    // the added items' diagnostics: only those in the edited files are the edit's (collect_added
    // also flushes what background type checks of other crates queued meanwhile)
    let edited: Vec<String> = olds.iter().map(|&f| u.prog.file_paths[f as usize].clone()).collect();
    let mut errs: Vec<String> = add_errs.into_iter().filter(|e| !(e.contains(".rs:")) || edited.iter().any(|p| e.starts_with(p.as_str()))).collect();
    let as_restart = !layout_restart.is_empty();
    errs.extend(layout_restart);
    trace_sub("consts_errs");
    trace_stage("commit_collect");
    let mut checked_bodies = Vec::new();
    {
        let mut todo: Vec<(DefId, Vec<TyId>)> = Vec::new();
        let added_code: Vec<DefId> = added_defs.iter().copied().filter(|d| matches!(u.prog.def(*d).kind, DefKind::Fn | DefKind::AssocFn | DefKind::Const | DefKind::AssocConst | DefKind::Static)).collect();
        for d in body_defs.iter().chain(const_defs.iter()).chain(added_code.iter()) {
            if let Some(a) = crate::borrowck::identity_args(&u.tcx, &u.prog, *d) {
                todo.push((*d, a));
            }
        }
        for id in u.slots_of_defs(body_defs.iter().chain(&const_defs).copied()) {
            trace_count("slot_visits", 1);
            if let FnKey::Inst(d, a) = &u.fns[id as usize].key {
                if !a.is_empty() { todo.push((*d, a.clone())); }
            }
        }
        let mut uniq: Vec<(DefId, Vec<TyId>)> = Vec::new();
        for t in todo {
            if !uniq.contains(&t) {
                uniq.push(t);
            }
        }
        // (a layout the running values cannot take: the restart's reasons alone)
        if as_restart {
            uniq.clear();
        }
        trace_count("typecheck_items", uniq.len());
        for (d, a) in uniq {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::typeck::instantiate_body(&crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &crate::tcx::EvalState::new()), &u.prog, &u.tcx, d, &a)));
            match r {
                Ok(b) => {
                    for e in &b.errors {
                        if !errs.contains(e) { errs.push(e.clone()); }
                    }
                    // kept for the sandbox check and the borrow gate (which would type-check
                    // the same body again)
                    checked_bodies.push(b);
                }
                Err(_) => errs.push(format!("internal compiler error type-checking {} -- makepad run bug, report it", u.prog.def_path(d))),
            }
        }
    }
    if !options.sandbox_nocheck {
        let mut checked = Vec::new();
        for &file in &olds {
            let krate = u.prog.file_crate[file as usize];
            if !checked.contains(&krate) && crate::sandbox::is_sandboxed_crate_key(&u.prog, &FnKey::Closure(file, 0, 0)) {
                checked.push(krate);
                let bodies: Vec<_> = checked_bodies.iter().filter(|b| u.prog.file_crate[b.file as usize] == krate
                    && crate::sandbox::is_sandboxed_crate_key(&u.prog, &FnKey::Inst(b.def, b.args.to_vec()))).collect();
                errs.extend(crate::modules::sandbox_check(&u.prog, &u.tcx, krate, &bodies));
            }
        }
    }
    if module {
        for &file in &olds {
            errs.extend(crate::modules::check_orphans(u, u.prog.file_crate[file as usize], &added_defs));
        }
    }
    trace_stage("typecheck");
    if errs.is_empty() && crate::borrowck::enabled(options) {
        // Check/report one changed owner at a time: do not hold its diagnostics until
        // another body has finished, compilation has run, or the transaction unwinds.
        let epoch = u.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst);
        use crate::borrowck::{event::{admit, Origin}, lazy::{check_now, CheckRequest, Growth}};
        let gate = |req: CheckRequest<'_>, errs: &mut Vec<String>| {
            trace_count("gate_items", req.changed.len());
            trace_count("gate_instances", req.instances.len());
            let g = check_now(&u.prog, &u.tcx, &req);
            for ev in &g.events {
                let rendered = ev.error.render();
                if !errs.contains(&rendered) {
                    report_gandalf_refusal(ev);
                    errs.push(rendered);
                }
            }
        };
        for &d in body_defs.iter().chain(added_defs.iter()).chain(const_defs.iter()) {
            if !admit(&u.prog, u.prog.def(d).krate) { continue; }
            let insts: Vec<_> = u.slots_of_defs([d]).into_iter().filter_map(|id| match &u.fns[id as usize].key {
                FnKey::Inst(owner, a) if *owner == d => Some((d, a.clone())),
                _ => None,
            }).collect();
            gate(CheckRequest { changed: &[d], instances: &insts, checked: &checked_bodies, growth: None, origin: Origin::Gate, epoch, fv: fv_cx.as_ref() }, &mut errs);
        }
        if errs.is_empty() && !file_fns.is_empty() {
            // A framework-only edit must also bypass field-view growth checks. Restrict
            // both the changed instances (above) and the callers that can refuse it.
            let visible: Vec<_> = visible_crates.iter().enumerate().map(|(k, v)| *v && admit(&u.prog, k as u32)).collect();
            let growth = Growth { changed_insts: &file_fns, before: &pre_summaries, visible_crates: &visible };
            gate(CheckRequest { changed: &[], instances: &[], checked: &[], growth: Some(growth), origin: Origin::Gate, epoch, fv: fv_cx.as_ref() }, &mut errs);
        }
        if !errs.is_empty() {
            out.lines.push(format!("refused by the borrow checker ({} error(s))", errs.len()));
        }
    }
    let t2 = Instant::now();
    out.ms[1] = (t2 - t1).as_secs_f64() * 1e3;
    trace_stage("gandalf");
    // 5: compile everything; nothing is installed until all of it built
    let mut staged: Vec<(u32, Code)> = Vec::new();
    let mut pre: Vec<(u32, Result<Code, String>)> = Vec::new();
    let mut pending_ahead: Vec<u32> = Vec::new();
    let mut pre_n = 0usize;
    if errs.is_empty() {
        for &id in slots.iter().chain(fresh.iter()) {
            let f = &mut u.fns[id as usize];
            f.rir = None;
            if let Some(h) = &mut f.history { h.inlined_into.clear(); }
            f.build_err = None;
            f.spec = None;
            u.reload_errors.remove(&id);
            u.reload_specs.remove(&id);
        }
        // Module base slots only supply RIR. Their twins are already in the dependency
        // closure above; rebuild compiled twins and leave cold twins for first use.
        let targets: Vec<u32> = slots.iter().chain(fresh.iter()).copied().filter(|&id| {
            !crate::sandbox::is_sandboxed_slot(u, id)
                && (!matches!(u.fns[id as usize].key, FnKey::Sandbox(_)) || u.fns[id as usize].compiled)
        }).collect();
        // host slots: front ends under the lock, back ends without it (program threads keep
        // running and compiling); twins under the lock (their transform places sandbox data)
        let (twins, host): (Vec<u32>, Vec<u32>) = targets.into_iter().partition(|id| under_lock(&*up, *id));
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8);
        for (id, r) in jit::compile_batch_off_lock(up, &host, threads) {
            match r {
                Ok(c) => staged.push((id, c)),
                Err(e) => {
                    let u = &*up;
                    errs.push(format!("{}: {}", u.fns[id as usize].name, e));
                }
            }
        }
        for id in twins {
            match compile_guarded(up, id) {
                Ok(c) => staged.push((id, c)),
                Err(e) => {
                    let u = &*up;
                    errs.push(format!("{}: {}", u.fns[id as usize].name, e));
                }
            }
        }
    }
    if errs.is_empty() {
        // Only newly created slots are prepared ahead synchronously. Existing edited slots,
        // inlined callers and invalidated Drop glue are already rebuilt in staged; their
        // installed code is kept until replacement, so none needs an exception here.
        let mut roots: Vec<u32> = Vec::new();
        for (_, c) in &staged {
            referenced_slots(&c.2, &mut roots);
        }
        for &(id, _) in &staged {
            if matches!((&*up).fns[id as usize].key, FnKey::Sandbox(_)) {
                roots.extend(crate::sandbox::extra_refs(id));
            }
        }
        let u = &*up;
        roots.extend((n0..u.fns.len()).map(|k| k as u32));
        let seeds: Vec<u32> = staged.iter().map(|s| s.0).collect();
        // an error in code the edit made (a new closure of a changed fn) refuses the edit; one
        // in code it merely reaches is reported as its first call would
        let mine = |u: &Unit, id: u32| match &u.fns[id as usize].key {
            FnKey::Closure(file, _, _) => news.iter().any(|(_, n)| n == file),
            FnKey::Inst(d, _) => body_defs.contains(d) || consts_hit.contains(d),
            _ => false,
        };
        // a bounded part before the install (a new derive can reach thousands of std fns: a
        // second of compiling would stall the edit); the rest is queued to the background
        // compilers after the install (pending_ahead) and compiles on first call if not yet built
        pending_ahead.extend_from_slice(&roots);
        match compile_reachable_from(up, roots, seeds, &mine, PRECOMPILE_SYNC, n0) {
            Ok(b) => {
                pre_n = b.len();
                // Include every skipped root and callee, not just relocations of the last
                // wave: old cold slots are deferred even when the sync cap was not reached.
                let u = &*up;
                for (id, r) in &b {
                    if let Ok(c) = r {
                        referenced_slots(&c.2, &mut pending_ahead);
                    }
                    if matches!(u.fns[*id as usize].key, FnKey::Sandbox(_)) {
                        pending_ahead.extend(crate::sandbox::extra_refs(*id));
                        pending_ahead.extend(crate::sandbox::extra_refs(u32::MAX));
                    }
                }
                pending_ahead.extend((n0..u.fns.len()).map(|k| k as u32));
                let built: std::collections::HashSet<u32> = b.iter().map(|x| x.0).chain(staged.iter().map(|x| x.0)).collect();
                pending_ahead.retain(|&id| (id as usize) < u.fns.len() && !u.fns[id as usize].compiled && !built.contains(&id));
                pending_ahead.sort_unstable();
                pending_ahead.dedup();
                pre = b;
            }
            Err(e) => errs.extend(e),
        }
    }
    // closures stored by the running version (handlers, callbacks): an old closure whose parent
    // was rebuilt runs the new version of itself when the new parent has a closure at the same
    // place with the same signature and captures (same environment layout); otherwise it keeps
    // its own code (memory-safe; reported)
    let mut rebind: Vec<(u32, u32)> = Vec::new();
    if errs.is_empty() {
        let u = &*up;
        let rebuilt: Vec<u32> = slots.iter().chain(fresh.iter()).copied().collect();
        let new_files: Vec<u32> = news.iter().map(|x| x.1).collect();
        let mut olds_c: Vec<u32> = Vec::new();
        let mut news_c: Vec<u32> = Vec::new();
        for id in u.closures_of_slots(rebuilt.iter().copied()) {
            trace_count("slot_visits", 1);
            let f = &u.fns[id as usize];
            if let FnKey::Closure(file, _, _) = &f.key {
                if new_files.contains(file) { news_c.push(id); }
                else if f.compiled { olds_c.push(id); }
            }
        }
        let new_ids: Vec<(u32, Option<(u32, TyId, Vec<TyId>, TyId)>)> = news_c.iter().map(|&n| (n, closure_ident(u, n))).collect();
        for o in olds_c {
            let oi = closure_ident(u, o);
            let parent = match &u.fns[o as usize].key {
                FnKey::Closure(_, _, p) => *p,
                _ => continue,
            };
            let hit = new_ids.iter().find(|(n, ni)| {
                matches!(&u.fns[*n as usize].key, FnKey::Closure(_, _, p) if *p == parent) && ni.is_some() && *ni == oi
            });
            match hit {
                Some((n, _)) => rebind.push((o, *n)),
                None => {
                    if oi.is_some() && new_ids.iter().any(|(n, ni)| matches!(&u.fns[*n as usize].key, FnKey::Closure(_, _, p) if *p == parent) && ni.as_ref().map(|x| x.0) == oi.as_ref().map(|x| x.0)) {
                        out.lines.push(format!("kept: stored values of closure {} keep its old code (its captures or signature changed)", u.fns[o as usize].name));
                    }
                }
            }
        }
    }
    // suspended async fn bodies (M5): an old poll whose parent was rebuilt resumes in the new
    // code when the new body has the same code up to its last await and the same state shape
    // (what each suspension point saved where); otherwise it finishes on its old code
    if errs.is_empty() {
        let rebuilt: Vec<u32> = slots.iter().chain(fresh.iter()).copied().collect();
        let new_files: Vec<u32> = news.iter().map(|x| x.1).collect();
        let pairs: Vec<(u32, u32)> = {
            let u = &*up;
            let is_body = |k: &FnKey| matches!(k, FnKey::Closure(_, ce, _) if *ce & crate::tcx::ASYNC_FN_BODY != 0);
            let mut v = Vec::new();
            let parents = rebuilt.iter().copied().chain(u.slots_of_defs(removed_defs.iter().copied()));
            let new_closures = u.closures_of_files(&new_files);
            for o in u.closures_of_slots(parents) {
                let fo = &u.fns[o as usize];
                trace_count("slot_visits", 1);
                let (FnKey::Closure(of, oce, op), true) = (&fo.key, is_body(&fo.key) && fo.compiled) else { continue };
                let replaced = matches!(&u.fns[*op as usize].key, FnKey::Inst(d, _) if removed_defs.contains(d));
                if !(rebuilt.contains(op) || replaced) || new_files.contains(of) {
                    continue;
                }
                // the new body: the same parent slot (a body edit), or the async fn that replaced
                // the parent (an async fn's body edit is a new fn: same path)
                let pname = |p: u32| -> Option<String> { match &u.fns[p as usize].key { FnKey::Inst(d, a) => Some(format!("{}{:?}", u.prog.def_path(*d), a)), _ => None } };
                let want = pname(*op);
                if let Some(n) = new_closures.iter().copied().find(|&id| matches!(&u.fns[id as usize].key, FnKey::Closure(nf, nce, np) if new_files.contains(nf) && *nce & crate::tcx::ASYNC_FN_BODY != 0 && (np == op || (want.is_some() && pname(*np) == want)))) {
                    v.push((o as u32, n as u32, *of, *oce));
                }
            }
            v.into_iter().map(|(o, n, _, _)| (o, n)).collect()
        };
        for (o, n) in pairs {
            // the new poll code: built now if the edit's precompile did not reach it
            let built = pre.iter().any(|(id, r)| *id == n && r.is_ok()) || staged.iter().any(|(id, _)| *id == n);
            if !built {
                match compile_guarded(up, n) {
                    Ok(c) => pre.push((n, Ok(c))),
                    Err(_) => continue,
                }
            }
            let u = &*up;
            let shape = |id: u32| u.tcx.coro_slot_ty.get(&id).and_then(|t| u.tcx.coro_shapes.get(&t));
            let prefix = |id: u32| -> Option<Vec<Vec<u8>>> {
                let FnKey::Closure(file, ce, _) = &u.fns[id as usize].key else { return None };
                let pf = &u.prog.files[*file as usize];
                let blk = pf.ast.block(crate::ast::BlockId(*ce & !crate::tcx::ASYNC_FN_BODY));
                let last = pf.toks.iter().filter(|t| t.lo >= blk.lo && t.hi <= blk.hi && &pf.src[t.lo as usize..t.hi as usize] == b"await").map(|t| t.hi).max()?;
                Some(toks_norm(pf, blk.lo, last, &[]))
            };
            match (shape(o), shape(n), prefix(o), prefix(n)) {
                (Some(a), Some(b), Some(pa), Some(pb)) if a == b && pa == pb => {
                    rebind.push((o, n));
                    out.lines.push(format!("suspended `{}` values resume in the new code (same state up to the last await)", u.fns[o as usize].name));
                }
                _ => out.lines.push(format!("kept: suspended `{}` values finish on their old code (its code before an await or its saved state changed)", u.fns[o as usize].name)),
            }
        }
    }
    trace_count("compiled_slots", staged.len() + pre.len());
    trace_stage("compile");
    let t3 = Instant::now();
    out.ms[2] = (t3 - t2).as_secs_f64() * 1e3;
    let u = &mut *up;
    if !errs.is_empty() {
        // nothing installs: the running versions come back, and everything built from the
        // refused text is dropped (built again from the running text when needed)
        let fw = jit::front_write();
        let from = u.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst);
        u.prog.unbind_defs(&added_defs);
        u.prog.unbind_imports_from(imports0);
        u.prog.rebind_use(std::mem::take(&mut use_undo));
        if !added_impls.is_empty() { u.tcx.uncollect_impls(&added_impls); }
        u.prog.rebind_defs(&removed_defs);
        for (k, &(o, n)) in news.iter().enumerate().rev() {
            u.prog.revert_version(o, n, &news_pairs[k]);
        }
        publish_reload(u, &news, full_inputs, scope.as_deref());
        if carry && !ctfe_relevant {
            let files: Vec<u32> = news.iter().flat_map(|&(a, b)| [a, b]).collect();
            carry_gandalf(u, from, &visible_crates, if full_inputs { None } else { Some(&files) });
        }
        unsafe { u.prog.reclaim_caches(); }
        drop(fw);
        jit::bump_patch_epoch();
        let mut invalidated = u.reload_rir.take().unwrap_or_default();
        invalidated.extend(slots.iter().chain(&fresh).copied());
        invalidated.extend((journal_start..u.fns.len()).map(|i| i as u32));
        for id in invalidated {
            let f = &mut u.fns[id as usize];
            f.rir = None;
            f.build_err = None;
            f.spec = None;
            u.reload_errors.remove(&id);
            u.reload_specs.remove(&id);
            u.bodies.remove(&id);
        }
        if full_inputs {
            let (prog, sc) = (&u.prog, scope.as_deref());
            u.consts.retain(|(d, _), _| !in_scope(prog, sc, *d));
            u.const_failed.retain(|(d, _)| !in_scope(prog, sc, *d));
        }
        for k in consts_hit.iter() {
            let ks: Vec<(DefId, Vec<TyId>)> = u.consts.keys().filter(|(d, _)| d == k).cloned().collect();
            for x in ks {
                u.consts.remove(&x);
            }
        }
        for ((d, args), v) in saved_consts {
            u.put_const(d, args, v);
        }
        for k in saved_failed {
            u.const_failed.insert(k);
        }
        for (d, n) in saved_variants {
            u.prog.truncate_variants(d, n);
        }
        if !saved_adts.is_empty() {
            for (d, a) in saved_adts {
                u.tcx.adts.insert(d, a);
            }
            u.tcx.publish_collected();
            u.lay.clear();
            out.lines.retain(|l| !l.starts_with("field `"));
        }
        out.rung = if as_restart { Rung::Restart } else { Rung::Refuse };
        out.lines.extend(errs);
        // A complete item may still fail checking. Remove only items located by diagnostics,
        // restore their running identities, and retry through the same transaction gate.
        // Every retry drops authored items; a fixed bound also caps pathological diagnostics.
        if !as_restart && !retry_sources.is_empty() {
            let mut retry = Vec::new();
            let mut dropped = false;
            let mut n_kept = kept.unwrap_or(0);
            for (file, path, src) in retry_sources {
                let spans = rust_spans(&src, u.prog.files[file as usize].edition);
                let mut bad = Vec::new();
                for error in &out.lines {
                    let Some(rest) = error.strip_prefix(&format!("{}:", path)) else { continue };
                    let mut parts = rest.split(':');
                    let (Some(line), Some(col)) = (parts.next().and_then(|s| s.parse::<usize>().ok()), parts.next().and_then(|s| s.parse::<usize>().ok())) else { continue };
                    let start = if line <= 1 { Some(0) } else { src.iter().enumerate().filter(|(_, b)| **b == b'\n').nth(line - 2).map(|(i, _)| i + 1) };
                    if let Some(start) = start { bad.push(start + col.saturating_sub(1)); }
                }
                let mut usable = Vec::new();
                for (lo, hi) in spans {
                    if bad.iter().any(|&p| lo <= p && p < hi) { continue; }
                    usable.extend_from_slice(&src[lo..hi]);
                    usable.push(b'\n');
                }
                if !bad.is_empty() {
                    if let Some((next, n)) = compose_partial(&u.prog, file, &usable) {
                        // Compare tokens: whitespace inserted by composition is not progress.
                        let a = u.prog.parse_version(file, &path, src.clone()).ok();
                        let b = u.prog.parse_version(file, &path, next.clone()).ok();
                        if a.as_ref().zip(b.as_ref()).is_some_and(|(a, b)| toks_norm(&a.pf, 0, a.pf.src_len, &[]) != toks_norm(&b.pf, 0, b.pf.src_len, &[])) {
                            dropped = true;
                            n_kept += n;
                            retry.push(Edit { file, path, src: next });
                            continue;
                        }
                    }
                }
                retry.push(Edit { file, path, src });
            }
            if dropped {
                let mut r = apply_locked(up, retry, Vec::new(), t0, 2, retries - 1, module);
                if r.rung == Rung::Patch {
                    r.kept = n_kept.max(r.kept);
                    r.lines.retain(|l| !l.starts_with("incomplete:"));
                    r.lines.push(format!("incomplete: {} item(s) kept at their running version", n_kept));
                }
                return r;
            }
        }
        return out;
    }
    // 6: install, one patch group
    let u = &mut *up;
    let gen = u.begin_patch();
    out.group = gen;
    let mut rebind_code: Vec<(u32, Code)> = Vec::new();
    for (o, n) in &rebind {
        let c = pre.iter().find(|(id, _)| id == n).and_then(|(_, r)| r.as_ref().ok()).or_else(|| staged.iter().find(|(id, _)| id == n).map(|(_, c)| c));
        if let Some(c) = c {
            rebind_code.push((*o, c.clone()));
        }
    }
    for (id, r) in pre {
        match r {
            Ok(c) => {
                jit::install_first(up, id, &c.0, c.1, &c.2);
            }
            Err(e) => {
                jit::install_failed(up, id, &e);
            }
        }
    }
    for (id, c) in staged {
        match jit::install_patched(up, id, &c.0, c.1, &c.2, gen) {
            Ok(_) => {
                let u = &*up;
                out.names.push(u.fns[id as usize].name.to_string());
            }
            // linking cannot fail for code that compiled (a full code region is fatal anyway)
            Err(e) => out.lines.push(format!("link error in {}: {}", (&*up).fns[id as usize].name, e)),
        }
    }
    for (id, c) in rebind_code {
        match jit::install_patched(up, id, &c.0, c.1, &c.2, gen) {
            Ok(_) => {
                out.names.push((&*up).fns[id as usize].name.to_string());
            }
            Err(e) => out.lines.push(format!("link error in {}: {}", (&*up).fns[id as usize].name, e)),
        }
    }
    let u = &mut *up;
    u.reload_rir = None;
    // Keep the historical conservative invalidation, but touch only cached failures
    // and speculative results that actually exist, not every slot in the program.
    let mut transient = std::mem::take(&mut u.reload_errors);
    transient.extend(std::mem::take(&mut u.reload_specs));
    trace_count("invalidated_transients", transient.len());
    for id in transient {
        let f = &mut u.fns[id as usize];
        f.build_err = None;
        f.spec = None;
    }
    for d in &static_defs {
        out.lines.push(format!(
            "note: static `{}` keeps its running value; its new initializer applies at the next start",
            def_name(&u.prog, *d)
        ));
    }
    for d in &consts_hit {
        if !const_defs.contains(d) {
            out.lines.push(format!("note: const `{}` is evaluated again (it depends on a changed const)", def_name(&u.prog, *d)));
        }
    }
    out.files = olds.iter().map(|o| news.iter().find(|(x, _)| x == o).map_or(*o, |(_, n)| *n)).collect();
    out.undo = Some(Undo { group: gen, versions: news.clone(), pairs: news_pairs.clone(), added: added_defs.clone(), removed: removed_defs.clone(), consts: saved_consts, consts_hit: consts_hit.clone(), adts: saved_adts, variants: saved_variants, const_failed: saved_failed, full_inputs, scope: scope.clone() });
    out.rung = Rung::Patch;
    if let Some(fv_cx) = fv_cx { fv_session_keep(transaction, fv_cx); }
    trace_count("inlining_callers", inlining_callers.len());
    if !inlining_callers.is_empty() {
        let mut names: Vec<&str> = inlining_callers.iter().take(3).map(|&d| u.fns[d as usize].name.as_ref()).collect();
        if inlining_callers.len() > 3 { names.push("..."); }
        out.lines.push(format!(
            "kept: {} fn(s) that inlined the edited code ({}) are rebuilt for their next call; a call of them already running (a loop that never returns) keeps the old code -- restart the app to apply it there",
            inlining_callers.len(), names.join(", ")
        ));
    }
    if let Some(n) = kept {
        out.lines.push(format!("incomplete: {} item(s) kept at their running version", n));
    }
    out.work = [slots.len() + fresh.len(), pre_n, (jit::CACHE_AHEAD.load(std::sync::atomic::Ordering::Relaxed) - cache_ahead0) as usize, field_named, field_deps];
    trace_stage("install");
    trace_count("background_slots", pending_ahead.len());
    if !pending_ahead.is_empty() {
        jit::spec_seed(up, &pending_ahead);
    }
    out.ms[3] = t3.elapsed().as_secs_f64() * 1e3;
    out
}

fn ms(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64() * 1e3
}

// ------------------------------------------------------------------ state-preserving restart (M9)

/// The program announced it can restart with its state (std::relax::restart_supported).
static RESTART_SUPPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// A restart is due (an edit needs one); the program polls this at its safe point.
static RESTART_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RESTART_REASON: relax_session::sync::Mutex<String> = relax_session::sync::Mutex::new(String::new());
/// When the edit that asked for the restart was saved (unix ms): the restarted run's report
/// measures save -> back on screen from it (MAKEPAD_RUN_RESTART_T0).
static RESTART_SAVED_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DOMAIN_RESTORED: relax_session::sync::Mutex<Option<String>> = relax_session::sync::Mutex::new(None);
static DOMAIN_SNAPSHOT: relax_session::sync::Mutex<Option<String>> = relax_session::sync::Mutex::new(None);
static DOMAIN_LOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static DOMAIN_INPUTS: relax_session::sync::Mutex<Option<Inputs>> = relax_session::sync::Mutex::new(None);
static DOMAIN_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// End native callbacks into the outgoing domain before its static block can be reclaimed.
pub fn domain_stopping(id: u32) {
    if crate::run::app_domain() != Some(id) { return; }
    DOMAIN_LOADING.store(true, std::sync::atomic::Ordering::Release);
    *PATCH_WAKER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    RESTART_SUPPORTED.store(false, std::sync::atomic::Ordering::SeqCst);
    RESTART_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
}

pub(crate) fn domain_loaded(inputs: Option<Inputs>) {
    *DOMAIN_RESTORED.lock().unwrap_or_else(|e| e.into_inner()) = DOMAIN_SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()).take();
    // the swap (zone_swap) may destroy the compiler generation this was built in before the watcher takes it
    *DOMAIN_INPUTS.lock().unwrap_or_else(|e| e.into_inner()) = inputs.as_ref().map(crate::zone_alloc::kept_clone);
    DOMAIN_REVISION.fetch_add(1, std::sync::atomic::Ordering::Release);
    DOMAIN_LOADING.store(false, std::sync::atomic::Ordering::Release);
}

fn restart_domain(reason: String) -> bool {
    if crate::run::app_domain().is_none() { return false; }
    *RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()) = reason;
    if RESTART_SUPPORTED.load(std::sync::atomic::Ordering::SeqCst) {
        RESTART_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
        wake_program();
    } else if let Err(e) = crate::run::relaunch() {
        eprintln!("makepad live: relaunch: {e}");
    }
    true
}

pub extern "C" fn __relax_live_edit_enabled() -> bool {
    crate::layout::LIVE_SESSION.load(std::sync::atomic::Ordering::Relaxed)
}

/// The program's wake for its event loop (std::relax::on_patch): called from the watcher
/// thread after a patch is installed, off the unit, so an idle app polls the new
/// generation at once instead of at its next idle tick.
static PATCH_WAKER: relax_session::sync::Mutex<Option<(crate::domain::Callback, extern "C" fn())>> = relax_session::sync::Mutex::new(None);

pub extern "C" fn __relax_on_patch(f: usize) {
    let wake = if f == 0 { None } else { crate::domain::callback().map(|scope| (scope, unsafe { std::mem::transmute(f) })) };
    *PATCH_WAKER.lock().unwrap_or_else(|e| e.into_inner()) = wake;
}

fn wake_program() {
    let wake = *PATCH_WAKER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((scope, wake)) = wake { let _ = scope.call(|| wake()); }
}

pub extern "C" fn __relax_restart_supported() {
    RESTART_SUPPORTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

pub extern "C" fn __relax_restart_requested() -> u32 {
    RESTART_REQUESTED.load(std::sync::atomic::Ordering::Relaxed) as u32
}

fn copy_out(s: &str, buf: *mut u8, cap: usize) -> usize {
    let n = s.len().min(cap);
    if !buf.is_null() && n > 0 {
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), buf, n) };
    }
    s.len()
}

pub extern "C" fn __relax_restart_reason(buf: *mut u8, cap: usize) -> usize {
    let r = RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()).clone();
    copy_out(&r, buf, cap)
}

/// The state the previous process left for this one (MAKEPAD_RUN_RESTORE), read once.
pub extern "C" fn __relax_restored_state(buf: *mut u8, cap: usize) -> usize {
    if crate::domain::current_app_domain().is_some() && crate::domain::current_app_domain() == crate::run::app_domain() {
        let state = DOMAIN_RESTORED.lock().unwrap_or_else(|e| e.into_inner());
        return state.as_ref().map_or(usize::MAX, |s| copy_out(s, buf, cap));
    }
    let up = jit::current_unit();
    if up.is_null() { return usize::MAX; }
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let runtime = &session.runtime;
    static STATE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let s = STATE.get_or_init(|| {
        let p = runtime.restore.clone()?;
        let t = std::fs::read_to_string(&p).ok();
        let _ = std::fs::remove_file(&p);
        t
    });
    match s {
        Some(t) => copy_out(t, buf, cap),
        None => usize::MAX,
    }
}

/// A hosted app queues a fresh domain with this state and returns for cooperative shutdown.
/// A directly running program writes MAKEPAD_RUN_RESTORE and replaces its process as before.
pub extern "C" fn __relax_restart_now(state: *const u8, len: usize) {
    let st = unsafe { if state.is_null() { &[][..] } else { std::slice::from_raw_parts(state, len) } };
    if crate::domain::current_app_domain().is_some() && crate::domain::current_app_domain() == crate::run::app_domain() {
        *DOMAIN_SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(String::from_utf8_lossy(st).into_owned());
        RESTART_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Err(e) = crate::run::relaunch() { eprintln!("makepad live: relaunch: {e}"); }
        // The queued HAL quit runs the app's normal cleanup after this event returns.
        return;
    }
    let dir = std::env::temp_dir().join(format!("relax-restart-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("state");
    if let Err(e) = std::fs::write(&path, st) {
        eprintln!("makepad live: restart: cannot write the state: {}", e);
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
        let Some(exe) = std::env::current_exe().ok() else { return };
        // the run's end as at exit: compiles stop and this run's code goes into the disk cache,
        // so the restarted run links it instead of compiling it again (exec skips the exit handlers)
        crate::run::summary("restart");
        eprintln!("makepad live: restarting with the program's state ({})", RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()));
        let t0 = RESTART_SAVED_MS.load(std::sync::atomic::Ordering::Relaxed);
        let e = std::process::Command::new(exe).args(&args[1..]).env("MAKEPAD_RUN_RESTORE", &path).env("MAKEPAD_RUN_RESTART_T0", t0.to_string()).exec();
        eprintln!("makepad live: restart failed: {}", e);
    }
}

/// makepad inspect: an edit that did not reach the running program is an agent's event (refused:
/// errors; restart: the change needs one).
fn patch_refused_event(what: &str, refused: bool, lines: &[String]) {
    let reasons: Vec<String> = lines.iter().take(6).map(|l| crate::inspect::json_str(l)).collect();
    crate::inspect::events::emit(
        "patchrefused",
        &format!("{}|{}", what, lines.first().cloned().unwrap_or_default()),
        &format!("\"edit\":{},\"outcome\":\"{}\",\"reasons\":[{}]", crate::inspect::json_str(what), if refused { "refused" } else { "restart" }, reasons.join(",")),
    );
}

// ------------------------------------------------------------ the live session log (scoreboard)

/// What a refused edit reports after its re-check: the first attempt's reasons, then the
/// retry's new ones. The retry's type check finds the first attempt's errors already reported
/// and adds only the summary "type checking failed due to previous errors", which goes when a
/// real error is listed.
fn refusal_lines(first: Vec<String>, retry: Vec<String>) -> Vec<String> {
    const SUMMARY: &str = "type checking failed due to previous errors";
    let mut out = first;
    for l in retry {
        if !out.contains(&l) {
            out.push(l);
        }
    }
    if out.iter().any(|l| !l.ends_with(SUMMARY)) {
        out.retain(|l| !l.ends_with(SUMMARY));
    }
    out
}

/// The session log: one JSON line per edit (rung, reasons, phase latencies) and per restart
/// outcome (the program's kept/lost report, through std::relax::live_log), appended across the
/// restarts of one session (the path travels in MAKEPAD_RUN_LIVE_LOG; default: the temp dir).
fn live_log_path() -> String {
    if let Ok(p) = std::env::var("MAKEPAD_RUN_LIVE_LOG") {
        if !p.is_empty() {
            return p;
        }
    }
    let p = std::env::temp_dir().join(format!("makepad-run-live-{}.jsonl", std::process::id())).to_string_lossy().into_owned();
    // exec'd restarts inherit it: one session, one log
    std::env::set_var("MAKEPAD_RUN_LIVE_LOG", &p);
    p
}

pub(crate) fn live_log(line: &str) {
    use std::io::Write;
    static LOCK: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(live_log_path()) {
        let _ = writeln!(f, "{}", line);
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn unix_ms(t: std::time::SystemTime) -> f64 {
    t.duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1e3)
}

/// This process's edits: (rung name, ms from the save to the rung applied).
static SESSION: relax_session::sync::Mutex<Vec<(&'static str, f64)>> = relax_session::sync::Mutex::new(Vec::new());

/// One edit's record. `saved`: the newest edited file's modification time (the save).
fn log_edit(what: &str, o: &Outcome, saved: Option<std::time::SystemTime>, seen: std::time::Instant) {
    if crate::run_state::observed() {
        let applied = o.rung == Rung::Patch && !o.pending
            && !o.lines.iter().any(|l| l.starts_with("incomplete:"));
        let records = if applied { Vec::new() } else {
            o.lines.iter().map(|line| crate::diag::from_legacy(line, "typeck")).collect()
        };
        crate::run_state::emit(crate::run_state::Event::Patch {
            domain: crate::run::app_domain(), applied, records: records.into(),
        });
    }
    let now = std::time::SystemTime::now();
    let detect = saved.map_or(0.0, |t| unix_ms(now) - unix_ms(t) - seen.elapsed().as_secs_f64() * 1e3).max(0.0);
    let work: f64 = o.ms.iter().sum();
    let total = detect + seen.elapsed().as_secs_f64() * 1e3;
    let rung = match o.rung {
        _ if o.lines.iter().any(|l| l == "incomplete: no item applied") => "incomplete",
        Rung::Patch if o.lines.iter().any(|l| l.starts_with("incomplete:")) => "partial",
        Rung::Patch if o.changed == 0 => "unchanged",
        Rung::Patch => "patch",
        Rung::Restart if crate::run::app_domain().is_some() || RESTART_SUPPORTED.load(std::sync::atomic::Ordering::SeqCst) => "restart",
        Rung::Restart => "restart-unsupported",
        Rung::Refuse => "refuse",
    };
    SESSION.lock().unwrap_or_else(|e| e.into_inner()).push((rung, total));
    let reasons: Vec<String> = o.lines.iter().take(8).map(|l| json_str(l)).collect();
    live_log(&format!(
        "{{\"event\":\"edit\",\"t\":{:.0},\"pid\":{},\"files\":{},\"rung\":\"{}\",\"changed\":{},\"fns\":{},\"rebuilt\":{},\"ahead\":{},\"ahead_cached\":{},\"field_named\":{},\"field_deps\":{},\"ms\":{{\"detect\":{:.1},\"parse\":{:.1},\"check\":{:.1},\"compile\":{:.1},\"install\":{:.1},\"work\":{:.1},\"total\":{:.1}}},\"reasons\":[{}]}}",
        unix_ms(now),
        std::process::id(),
        json_str(what),
        rung,
        o.changed,
        o.names.len(),
        o.work[0],
        o.work[1],
        o.work[2],
        o.work[3],
        o.work[4],
        detect,
        o.ms[0],
        o.ms[1],
        o.ms[2],
        o.ms[3],
        work,
        total,
        reasons.join(",")
    ));
}

/// The session's edits so far in this process, summarised (None without edits): printed at the
/// end of a live run.
pub(crate) fn session_summary() -> Option<String> {
    let v = SESSION.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if v.is_empty() {
        return None;
    }
    let n = v.len();
    let count = |r: &str| v.iter().filter(|x| x.0 == r).count();
    let mut inplace: Vec<f64> = v.iter().filter(|x| x.0 == "patch" || x.0 == "unchanged" || x.0 == "partial").map(|x| x.1).collect();
    inplace.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: f64| -> f64 {
        if inplace.is_empty() {
            return 0.0;
        }
        inplace[((inplace.len() as f64 - 1.0) * p).round() as usize]
    };
    Some(format!(
        "makepad live: session: {} edit(s): {} in place ({:.0}%), {} restart, {} refused; in-place save->applied p50 {:.0} ms, p95 {:.0} ms; log {}",
        n,
        count("patch") + count("unchanged") + count("partial"),
        100.0 * (count("patch") + count("unchanged") + count("partial")) as f64 / n as f64,
        count("restart") + count("restart-unsupported"),
        count("refuse"),
        pct(0.5),
        pct(0.95),
        live_log_path()
    ))
}

/// std::relax::live_log: the program adds a record to the session log (the restart's kept/lost).
pub extern "C" fn __relax_live_log(p: *const u8, n: usize) {
    if p.is_null() {
        return;
    }
    let s = unsafe { std::slice::from_raw_parts(p, n) };
    live_log(&String::from_utf8_lossy(s));
}

/// The restart entries by name (jit::loader_symbol).
pub fn restart_symbol(name: &[u8]) -> Option<usize> {
    Some(match name {
        b"__relax_restart_supported" => __relax_restart_supported as *const () as usize,
        b"__relax_on_patch" => __relax_on_patch as *const () as usize,
        b"__relax_live_edit_enabled" => __relax_live_edit_enabled as *const () as usize,
        b"__relax_restart_requested" => __relax_restart_requested as *const () as usize,
        b"__relax_restart_reason" => __relax_restart_reason as *const () as usize,
        b"__relax_restored_state" => __relax_restored_state as *const () as usize,
        b"__relax_restart_now" => __relax_restart_now as *const () as usize,
        b"__relax_live_stream" => __relax_live_stream as *const () as usize,
        b"__relax_live_log" => __relax_live_log as *const () as usize,
        _ => return None,
    })
}

// Per-file stream state is serialized with feeds, including the watcher draining a pending
// prefix. Only the newest prefix is retained; no queue grows at token-stream frequency.
#[derive(Default)]
struct StreamFile {
    last: Option<Instant>,
    pending: Option<Vec<u8>>,
    text: Vec<u8>,
    changed: Option<Instant>,
    reported: bool,
}
#[derive(Clone, PartialEq, Eq)]
enum StreamTarget {
    Path(String),
    Module(u64),
}
static STREAMS: relax_session::sync::Mutex<Vec<(usize, StreamTarget, StreamFile)>> = relax_session::sync::Mutex::new(Vec::new());

fn idle_outcome(file: Option<u32>, line: &str) -> Outcome {
    Outcome { kept: 0, pending: false, rung: Rung::Patch, files: file.into_iter().collect(), lines: vec![line.to_string()], names: Vec::new(), changed: 0, group: 0, ms: [0.0; 4], undo: None, work: [0; 5] }
}

unsafe fn stream_file(up: *mut Unit, path: &str) -> Option<u32> {
    crate::owner::call(crate::owner::Class::App, || {
        let prog = &(&*up).prog;
        prog.mods.iter().map(|m| m.file).find(|&f| prog.file_paths[f as usize] == path)
    })
}

unsafe fn target_file(up: *mut Unit, target: &StreamTarget) -> Option<(u32, String)> {
    match target {
        StreamTarget::Path(path) => stream_file(up, path).map(|f| (f, path.clone())),
        StreamTarget::Module(h) => crate::modules::stream_target(*h),
    }
}

fn record_stream(target: &StreamTarget, o: &Outcome) {
    if let StreamTarget::Module(h) = target {
        if o.rung == Rung::Patch {
            if let Some(&file) = o.files.first() { crate::modules::stream_applied(*h, file); }
        }
    }
}

/// The shared feed core takes a file index. Runtime modules never resolve a path via mods.
unsafe fn feed_stream(up: *mut Unit, target: &StreamTarget, file: u32, path: &str, state: &mut StreamFile, text: Vec<u8>) -> Outcome {
    let seen = Instant::now();
    let mut o = apply_mode(up, vec![Edit { file, path: path.to_string(), src: text }],
        2, matches!(target, StreamTarget::Module(_)));
    record_stream(target, &o);
    state.last = Some(Instant::now());
    if o.rung == Rung::Refuse {
        o.lines = vec!["incomplete: no item applied".to_string()];
    }
    log_edit(path, &o, None, seen);
    o
}

unsafe fn apply_stream_target(up: *mut Unit, target: StreamTarget, text: Vec<u8>) -> Outcome {
    let mut streams = STREAMS.lock().unwrap_or_else(|e| e.into_inner());
    let Some((file, path)) = target_file(up, &target) else {
        let mut o = idle_outcome(None, "stream target is not loaded");
        o.rung = Rung::Refuse;
        return o;
    };
    let k = match streams.iter().position(|s| s.0 == up as usize && s.1 == target) {
        Some(k) => k,
        None => { streams.push((up as usize, target.clone(), StreamFile::default())); streams.len() - 1 }
    };
    let state = &mut streams[k].2;
    if state.text != text || state.changed.is_none() {
        state.changed = Some(Instant::now());
        state.reported = false;
        state.text = text.clone();
    }
    state.pending = Some(text);
    if state.last.is_some_and(|t| t.elapsed().as_millis() < 16) {
        let mut o = idle_outcome(None, "incomplete: awaiting coalesced prefix");
        o.pending = true;
        return o;
    }
    let text = state.pending.take().unwrap();
    feed_stream(up, &target, file, &path, state, text)
}

/// Feed the entire text so far for one loaded source file. Calls within 16 ms coalesce;
/// the next feed or the live watcher drains the newest pending prefix.
pub unsafe fn apply_stream(up: *mut Unit, path: &str, text: Vec<u8>) -> Outcome {
    let _z = crate::zone_alloc::compiler();
    let path = std::fs::canonicalize(path).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string());
    if let Err(error) = crate::native_macros::reload(&[(&path, Some(&text))], (&(*up).prog.session).runtime.profile) {
        let mut out = idle_outcome(None, &error); out.rung = Rung::Refuse; return out;
    }
    if crate::native_macros::owns_path(&path) { return idle_outcome(None, "native macro sources match the running compiler"); }
    apply_stream_target(up, StreamTarget::Path(path), text)
}

pub(crate) unsafe fn apply_module_stream(up: *mut Unit, h: u64, text: Vec<u8>) -> Outcome {
    apply_stream_target(up, StreamTarget::Module(h), text)
}

/// Serialize final replacement with watcher drains; no queued prefix can overwrite it.
pub(crate) unsafe fn finish_module_stream(up: *mut Unit, h: u64, text: Vec<u8>) -> Option<Outcome> {
    let mut streams = STREAMS.lock().unwrap_or_else(|e| e.into_inner());
    streams.retain(|s| !(s.0 == up as usize && s.1 == StreamTarget::Module(h)));
    let (file, path) = crate::modules::stream_target(h)?;
    let o = apply_full(up, vec![Edit { file, path, src: text }]);
    record_stream(&StreamTarget::Module(h), &o);
    Some(o)
}

pub(crate) fn forget_module_stream(up: *mut Unit, h: u64) {
    STREAMS.lock().unwrap_or_else(|e| e.into_inner()).retain(|s| !(s.0 == up as usize && s.1 == StreamTarget::Module(h)));
}

unsafe fn drain_streams(up: *mut Unit) {
    let mut streams = STREAMS.lock().unwrap_or_else(|e| e.into_inner());
    for (_, target, s) in streams.iter_mut().filter(|s| s.0 == up as usize) {
        let Some((file, path)) = target_file(up, target) else { continue };
        if s.pending.is_some() && s.last.is_none_or(|t| t.elapsed().as_millis() >= 16) {
            let text = s.pending.take().unwrap();
            feed_stream(up, target, file, &path, s, text);
        }
        if !s.reported && s.changed.is_some_and(|t| t.elapsed().as_secs_f64() >= 1.0) {
            s.reported = true;
            if let Some((file, path)) = target_file(up, target) {
                let seen = Instant::now();
                let (parsed, composed) = crate::owner::call(crate::owner::Class::App, || {
                    let parsed = (&*up).prog.parse_version(file, &path, s.text.clone());
                    // A stall is diagnostic, not an instruction to delete the unstreamed tail.
                    let composed = if parsed.is_ok() && (path.ends_with(".rs") || matches!(target, StreamTarget::Module(_))) {
                        compose_partial(&(&*up).prog, file, &s.text).map(|r| r.0)
                    } else { Some(s.text.clone()) };
                    (parsed, composed)
                });
                if let Err(error) = parsed {
                    let mut o = idle_outcome(Some(file), &error);
                    o.rung = Rung::Refuse;
                    log_edit(&path, &o, None, seen);
                    eprintln!("makepad live: {}", error);
                } else if let Some(src) = composed {
                    let o = apply_mode(up, vec![Edit { file, path: path.clone(), src }], 0, matches!(target, StreamTarget::Module(_)));
                    record_stream(target, &o);
                    if o.rung != Rung::Patch {
                        log_edit(&path, &o, None, seen);
                        for line in &o.lines { eprintln!("makepad live: {}: {}", path, line); }
                    }
                }
            }
        }
    }
}

pub extern "C" fn __relax_live_stream(p: *const u8, n: usize, t: *const u8, len: usize) -> u32 {
    if p.is_null() || t.is_null() { return 3; }
    unsafe {
        if jit::UNIT.is_null() { return 3; }
        let Ok(path) = std::str::from_utf8(std::slice::from_raw_parts(p, n)) else { return 3; };
        let o = apply_stream(jit::UNIT, path, std::slice::from_raw_parts(t, len).to_vec());
        match o.rung {
            Rung::Patch if o.changed == 0 && o.lines.iter().any(|l| l.starts_with("incomplete:")) => 1,
            Rung::Patch => 0,
            Rung::Restart => 2,
            Rung::Refuse if o.lines.iter().any(|l| l.starts_with("incomplete:")) => 1,
            Rung::Refuse => 3,
        }
    }
}

// ------------------------------------------------------------------ the live driver in apps

/// `makepad run` of a working tree: a watcher thread polls the program's own source files (not
/// std's, not registry/git dependencies) and applies each settled save as one transaction.
/// Body-class patches are type-safe to install while the program runs (hotreload.md §9.2), so
/// they apply at once; other edits print their rung and reason. Every outcome is one line on
/// stderr ("makepad live: ...").
#[path = "reload_inputs.rs"]
mod inputs;
pub use inputs::Inputs;

/// The inputs the watcher started with (kept: the settle swap restarts the watcher with them),
/// whether the watcher thread runs, and the swap's request that it ends.
static WATCH_INPUTS: relax_session::sync::Mutex<Option<Inputs>> = relax_session::sync::Mutex::new(None);
static WATCH_RUNNING: AtomicBool = AtomicBool::new(false);
static WATCH_STOP: AtomicBool = AtomicBool::new(false);

pub fn start_watcher(up: *mut Unit, inputs: Inputs) {
    *WATCH_INPUTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::zone_alloc::kept_clone(&inputs));
    spawn_watcher(up, inputs, false);
}

fn spawn_watcher(up: *mut Unit, inputs: Inputs, quiet: bool) {
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    struct P(*mut Unit);
    unsafe impl Send for P {}
    let p = P(up);
    WATCH_STOP.store(false, Ordering::SeqCst);
    WATCH_RUNNING.store(true, Ordering::SeqCst);
    let spawned = std::thread::Builder::new()
        .name("run-live".to_string())
        .spawn(move || { let _z = crate::zone_alloc::compiler_thread();
            let runtime = &session.runtime;
            let p = p;
            jit::lower_this_thread_priority(runtime);
            // a panic here (the ICE hook is silent on the compiler's owner thread) would end
            // live editing without a word: say so
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { watch(p.0, inputs, quiet) }));
            if let Err(e) = r {
                let why = e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_default();
                eprintln!("makepad live: the watcher stopped: {why}{}; saves are no longer applied -- restart `makepad run`", jit::take_ice_location());
            }
            WATCH_RUNNING.store(false, Ordering::SeqCst);
        });
    if spawned.is_err() { WATCH_RUNNING.store(false, Ordering::SeqCst); }
}

/// The settle swap (jit zone_swap): ends the watcher thread, which holds compiler-generation
/// state for its whole life, and waits for it (its locals are freed on the way out). Some(true):
/// it ran and has ended, so `swap_restart_watcher` starts it again; Some(false): there was none;
/// None: it did not end within `ms` (it is running again).
pub fn swap_stop_watcher(ms: u64) -> Option<bool> {
    if !WATCH_RUNNING.load(Ordering::SeqCst) { return Some(false); }
    WATCH_STOP.store(true, Ordering::SeqCst);
    let t = Instant::now();
    while WATCH_RUNNING.load(Ordering::SeqCst) {
        if t.elapsed().as_millis() as u64 > ms {
            WATCH_STOP.store(false, Ordering::SeqCst);
            return None;
        }
        std::thread::sleep(std::time::Duration::from_micros(500));
    }
    Some(true)
}

/// Starts the watcher again with the inputs it started with, after `swap_stop_watcher` ended it.
pub fn swap_restart_watcher(up: *mut Unit) {
    let inputs = WATCH_INPUTS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(inputs) = inputs { spawn_watcher(up, inputs, true); }
}

pub(crate) fn stamp(path: &str) -> Option<(std::time::SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

unsafe fn watch(up: *mut Unit, mut inputs: Inputs, quiet: bool) {
    let std_dir = std::env::var("MAKEPAD_RUN_STD").unwrap_or_default();
    // (path, running file index, last stamp)
    // the program's own source files (not std, registry or git dependencies), loaded so far
    let own_files = |u: &Unit| -> Vec<(String, u32)> {
        u.prog.current_source_files().into_iter().filter(|(p, _)| {
            !(p.is_empty() || (!std_dir.is_empty() && crate::vfs::relative_path(p, &std_dir).is_some()) || p.contains("/.cargo/registry/") || p.contains("/.cargo/git/") || p.contains("/rustlib/"))
                && std::path::Path::new(p).is_file()
        }).collect()
    };
    // (the watcher's holds read the program's files only: lock_unit_quick)
    let mut files: Vec<(String, u32, Option<(std::time::SystemTime, u64)>)> = {
        crate::owner::call_quick(crate::owner::Class::App, || {
            let loaded: std::collections::HashMap<_, _> = own_files(&*up).into_iter().collect();
            let mut v: Vec<_> = inputs.paths.iter().map(|(p, (stamp, _))| (p.clone(), loaded.get(p).copied().unwrap_or(u32::MAX), *stamp)).collect();
            for (p, i) in loaded {
                if !inputs.paths.contains_key(&p) { let s = stamp(&p); v.push((p, i, s)); }
            }
            v
        })
    };
    // Proc-macro source crates are stripped from the app graph. Their inputs and
    // directories still participate in the existing watcher's changed-path batch.
    for path in crate::native_macros::paths() {
        if !files.iter().any(|f| f.0 == path) { let s = stamp(&path); files.push((path, u32::MAX, s)); }
    }
    // the post-boot loader adds files after boot; each is watched from its load (an
    // edit made before it is simply what the load read)
    let mut loads_seen = jit::DEFERRED_LOADS.load(std::sync::atomic::Ordering::Relaxed);
    let mut events = watch_events::Watch::new(files.iter().map(|f| f.0.as_str()));
    if !quiet { eprintln!("makepad live: watching {} source files; session log {}", files.len(), live_log_path()); }
    let mut undos: Vec<Undo> = Vec::new();
    // a save is seen within 100 ms (a restart's whole budget is 1 s); one round is a stat per
    // watched file (MAKEPAD_RUN_LIVE_TRACE=1 prints its cost once)
    let mut traced = std::env::var_os("MAKEPAD_RUN_LIVE_TRACE").is_none();
    // the hot set: the program's own package (its root crate's directory) and every file saved
    // this session, polled every 5 ms (a few stats); every file is polled every 100 ms
    let mut hot: Vec<usize> = {
        let dir = crate::owner::call_quick(crate::owner::Class::App, || {
            let u = &*up;
            let root = crate::layout::root_crate() as usize;
            u.prog.crates.get(root).and_then(|c| u.prog.mods.get(c.root_mod as usize)).and_then(|m| u.prog.file_paths.get(m.file as usize)).and_then(|p| std::path::Path::new(p).parent().map(|d| d.to_string_lossy().into_owned()))
        });
        match dir {
            Some(d) => files.iter().enumerate().filter(|(_, f)| crate::vfs::relative_path(&f.0, &d).is_some()).map(|(k, _)| k).take(64).collect(),
            None => Vec::new(),
        }
    };
    // a refused or incomplete edit, re-checked after a second of silence: (when, its text, the
    // first attempt's reasons -- the retry's type check finds the same errors already reported
    // and says only "type checking failed due to previous errors")
    let mut stalled: std::collections::HashMap<usize, (Instant, Vec<u8>, Vec<String>)> = std::collections::HashMap::new();
    let mut tick: u32 = 0;
    let mut startup_scan = true;
    let mut restart_required = false;
    let mut revision = 0;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(5));
        if WATCH_STOP.load(Ordering::SeqCst) { return; }
        if DOMAIN_LOADING.load(std::sync::atomic::Ordering::Acquire) { continue; }
        let latest = DOMAIN_REVISION.load(std::sync::atomic::Ordering::Acquire);
        if latest != revision {
            revision = latest;
            if let Some(next) = DOMAIN_INPUTS.lock().unwrap_or_else(|e| e.into_inner()).take() {
                *WATCH_INPUTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::zone_alloc::kept_clone(&next));
                inputs = next;
            }
            let loaded = crate::owner::call_quick(crate::owner::Class::App, || own_files(&*up));
            files = inputs.paths.iter().map(|(p, (s, _))| (p.clone(), u32::MAX, *s)).collect();
            for (path, file) in loaded {
                if let Some(f) = files.iter_mut().find(|f| f.0 == path) { f.1 = file; }
                else { let s = stamp(&path); files.push((path, file, s)); }
            }
            events = watch_events::Watch::new(files.iter().map(|f| f.0.as_str()));
            hot.clear();
            stalled.clear();
            undos.clear();
            restart_required = false;
            startup_scan = true;
        }
        // Keep the original stamps and queued OS notifications intact until startup
        // finishes. In particular, do not acquire Reloading or drain streamed edits:
        // either can compete with the cold compiler before the first frame.
        if !jit::live_startup_done() { continue; }
        // Once the tree no longer matches this session, later saves must not patch a
        // mixture of revisions into the old process. A fresh run restores live editing.
        if restart_required { continue; }
        let loads = jit::DEFERRED_LOADS.load(std::sync::atomic::Ordering::Relaxed);
        // a new watch set drops the events the old one had not delivered yet (a save made
        // while the post-boot loader loads was never seen): stat every file once instead
        let mut rewatched = std::mem::take(&mut startup_scan);
        if loads != loads_seen || rewatched {
            loads_seen = loads;
            let loaded = crate::owner::call_quick(crate::owner::Class::App, || own_files(&*up));
            let known: std::collections::HashMap<_, _> = files.iter().enumerate().map(|(i, f)| (f.0.clone(), i)).collect();
            let mut added = Vec::new();
            for (path, file) in loaded {
                if let Some(&i) = known.get(&path) { files[i].1 = file; }
                else { added.push((path, file)); }
            }
            if !added.is_empty() {
                if std::env::var_os("MAKEPAD_RUN_DEFERRED_TRACE").is_some() { eprintln!("makepad live: watching {} more file(s) the post-boot loader loaded", added.len()); }
                for (p, i) in added { let st = stamp(&p); files.push((p, i, st)); }
                events = watch_events::Watch::new(files.iter().map(|f| f.0.as_str()));
                rewatched = true;
            }
        }
        drain_streams(up);
        // Re-check an unchanged failing prefix once, after one second of silence.
        let due: Vec<_> = stalled.iter().filter(|(_, (t, _, _))| t.elapsed().as_secs_f64() >= 1.0).map(|(&k, _)| k).collect();
        for k in due {
            let (_, src, first) = stalled.remove(&k).unwrap();
            let f = &mut files[k];
            if stamp(&f.0) != f.2 { continue; }
            let seen = Instant::now();
            let mut o = apply_mode(up, vec![Edit { file: f.1, path: f.0.clone(), src }], 0, false);
            if o.rung != Rung::Patch {
                o.lines = refusal_lines(first, std::mem::take(&mut o.lines));
                log_edit(&f.0, &o, f.2.map(|s| s.0), seen);
                match o.rung {
                    Rung::Refuse => eprintln!("makepad live: {}: NOT applied (errors below); the app runs the previous code", f.0),
                    _ => eprintln!("makepad live: {}: needs a restart (reasons below); the app runs the previous code", f.0),
                }
                for l in &o.lines { eprintln!("makepad live:   {}", l); }
                patch_refused_event(&f.0, o.rung == Rung::Refuse, &o.lines);
            } else {
                // reported as the first attempt's patch would have been (else a save that
                // patched on this retry looks like one that never finished)
                let what = f.0.rsplit('/').next().unwrap_or("").to_string();
                log_edit(&what, &o, f.2.map(|s| s.0), seen);
                eprintln!("makepad live: {}: patched ({} item(s) changed, {} fn(s) installed) in {:.1} ms", what, o.changed, o.names.len(), o.ms.iter().sum::<f64>());
                crate::tui::flush();
                wake_program();
                for l in &o.lines { eprintln!("makepad live:   {}", l); }
                if let Some(&file) = o.files.first() { f.1 = file; }
            }
        }
        tick = tick.wrapping_add(1);
        let notifications = events.as_mut().and_then(|w| w.changed());
        let lost = events.is_some() && notifications.is_none();
        if lost { events = watch_events::Watch::new(files.iter().map(|f| f.0.as_str())); }
        let all = lost || rewatched || (events.is_none() && tick % 20 == 0);
        let t_round = std::time::Instant::now();
        let mut changed: Vec<usize> = Vec::new();
        let mut scanned = 0;
        if all {
            scanned = files.len();
            for (k, f) in files.iter_mut().enumerate() {
                if stamp(&f.0) != f.2 {
                    changed.push(k);
                }
            }
        } else if let Some(ids) = notifications {
            for k in ids {
                scanned += 1;
                if stamp(&files[k].0) != files[k].2 { changed.push(k); }
            }
        } else {
            scanned = hot.len();
            for &k in &hot {
                if stamp(&files[k].0) != files[k].2 {
                    changed.push(k);
                }
            }
        }
        if !traced && all {
            traced = true;
            eprintln!("makepad live: one watch round: {} files in {:.2} ms; hot set {} files", files.len(), t_round.elapsed().as_secs_f64() * 1e3, hot.len());
        }
        if changed.is_empty() {
            continue;
        }
        let seen = std::time::Instant::now();
        // from the save on, the post-boot loader keeps off the unit (the read below
        // compares with the running text on it)
        for &k in &changed {
            if !hot.contains(&k) {
                hot.push(k);
            }
        }
        // let the save settle (editors write in steps; a branch switch touches many files): until
        // the changed files keep their stamps for a full filesystem polling window. A
        // checkout spanning several writes must not be split into individual patches.
        let mut last: Vec<Option<(std::time::SystemTime, u64)>> = changed.iter().map(|&k| stamp(&files[k].0)).collect();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let now: Vec<_> = changed.iter().map(|&k| stamp(&files[k].0)).collect();
            let mut more = false;
            let candidates = match events.as_mut().map(|w| w.changed()) {
                Some(Some(ids)) => ids,
                Some(None) => {
                    events = watch_events::Watch::new(files.iter().map(|f| f.0.as_str()));
                    (0..files.len()).collect()
                }
                None => hot.clone(),
            };
            scanned += candidates.len() + changed.len();
            for k in candidates {
                if stamp(&files[k].0) != files[k].2 && !changed.contains(&k) {
                    changed.push(k);
                    more = true;
                }
            }
            if !more && now == last {
                break;
            }
            last = changed.iter().map(|&k| stamp(&files[k].0)).collect();
            if seen.elapsed().as_millis() > 2000 {
                break;
            }
        }
        let reason = inputs.restart_reason(changed.iter().map(|&k| files[k].0.as_str())).or_else(|| {
            changed.iter().any(|&k| files[k].1 == u32::MAX && !crate::native_macros::owns_path(&files[k].0))
                .then_some("sources outside the loaded program changed")
        });
        if let Some(reason) = reason {
            if !inputs.host_restart_required(changed.iter().map(|&k| files[k].0.as_str())) && restart_domain(reason.to_string()) {
                eprintln!("makepad live: {reason}; relaunching the app domain (window kept)");
                restart_required = true;
                stalled.clear();
                continue;
            }
            let package = std::env::var("MAKEPAD_RUN_PACKAGE").unwrap_or_else(|_| "<pkg>".into());
            let what = if changed.iter().any(|&k| std::path::Path::new(&files[k].0).is_dir()) { "source inputs" } else { "files" };
            // provided crates and std are built into this binary: cargo rebuilds it on the way
            let restart = if inputs.host_restart_required(changed.iter().map(|&k| files[k].0.as_str())) {
                format!("restart it through cargo (which rebuilds makepad): {}", crate::srchash::rebuild_command(&std::env::args().skip(1).collect::<Vec<_>>()))
            } else {
                format!("restart `makepad run {package}`")
            };
            let message = format!("{} {what} changed: {reason}; {restart} to pick them up; keeping the running code", changed.len());
            eprintln!("makepad live: {message}");
            live_log(&format!("{{\"restart_required\":true,\"message\":{}}}", json_str(&message)));
            restart_required = true;
            stalled.clear();
            continue;
        }
        let _reloading = jit::Reloading::begin();
        let macro_changes: Vec<_> = changed.iter().map(|&k| (files[k].0.as_str(), None)).collect();
        if let Err(error) = crate::native_macros::reload(&macro_changes, (&(*up).prog.session).runtime.profile) {
            for &k in &changed { files[k].2 = stamp(&files[k].0); }
            eprintln!("makepad live: {error}");
            live_log(&error);
            continue;
        }
        let saved = changed.iter().filter_map(|&k| stamp(&files[k].0).map(|s| s.0)).max();
        let read_started = Instant::now();
        let watch_ms = saved.and_then(|s| s.elapsed().ok()).map_or_else(|| ms(seen), |d| d.as_secs_f64() * 1000.);
        let mut edits = Vec::new();
        let mut which = Vec::new();
        for &k in &changed {
            stalled.remove(&k);
            let f = &mut files[k];
            f.2 = stamp(&f.0);
            if f.1 == u32::MAX || crate::native_macros::owns_path(&f.0) { continue; }
            let src = match std::fs::read(&f.0) {
                Ok(s) => s,
                Err(_) => continue,
            };
            // unchanged text (touched, or saved back to the running version)
            let same = {
                crate::owner::call(crate::owner::Class::App, || {
                    let u = &*up;
                    let pf = &u.prog.files[f.1 as usize];
                    pf.src[..pf.src_len as usize] == src[..]
                })
            };
            if !same {
                edits.push(Edit { file: f.1, path: f.0.clone(), src });
                which.push(k);
            }
        }
        if edits.is_empty() {
            // saved back to the running text (e.g. the fix after a refused edit): the program
            // already runs it -- an edit applied in place for the scoreboard
            let o = Outcome { kept: 0, pending: false, rung: Rung::Patch, files: Vec::new(), lines: Vec::new(), names: Vec::new(), changed: 0, group: 0, ms: [0.0; 4], undo: None, work: [0; 5] };
            let what: Vec<String> = changed.iter().map(|&k| files[k].0.rsplit('/').next().unwrap_or("").to_string()).collect();
            log_edit(&what.join(", "), &o, saved, seen);
            eprintln!("makepad live: {}: patched (0 item(s) changed, 0 fn(s) installed) in 0.0 ms", what.join(", "));
            continue;
        }
        let names: Vec<String> = edits.iter().map(|e| e.path.rsplit('/').next().unwrap_or("").to_string()).collect();
        let originals: Vec<_> = edits.iter().map(|e| e.src.clone()).collect();
        trace_input(watch_ms, ms(read_started), scanned);
        let o = apply(up, edits);
        let incomplete = o.rung == Rung::Refuse || o.lines.iter().any(|l| l.starts_with("incomplete:"));
        if incomplete {
            for (&k, src) in which.iter().zip(originals) { stalled.insert(k, (Instant::now(), src, if o.rung == Rung::Refuse { o.lines.clone() } else { Vec::new() })); }
        }
        if o.rung == Rung::Refuse {
            // Diagnostics are emitted only by the stalled-file check above.
            continue;
        }
        let what = names.join(", ");
        if o.rung == Rung::Restart {
            RESTART_SAVED_MS.store(saved.map_or(0.0, unix_ms) as u64, std::sync::atomic::Ordering::Relaxed);
        }
        log_edit(&what, &o, saved, seen);
        match o.rung {
            Rung::Patch => eprintln!(
                "makepad live: {}: patched ({} item(s) changed, {} fn(s) installed) in {:.1} ms",
                what,
                o.changed,
                o.names.len(),
                o.ms.iter().sum::<f64>()
            ),
            Rung::Refuse => eprintln!("makepad live: {}: NOT applied (errors below); the app runs the previous code", what),
            Rung::Restart if restart_domain(format!("{}: {}", what, o.lines.first().cloned().unwrap_or_default())) => {
                restart_required = true;
                eprintln!("makepad live: {what}: relaunching the app domain (window kept)");
            }
            Rung::Restart if RESTART_SUPPORTED.load(std::sync::atomic::Ordering::SeqCst) => {
                // the program restarts itself at its next safe point, carrying its state over
                *RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()) = format!("{}: {}", what, o.lines.first().cloned().unwrap_or_default());
                RESTART_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
                eprintln!("makepad live: {}: restarting with state at the app's next safe point (reasons below)", what);
            }
            Rung::Restart => eprintln!("makepad live: {}: needs a restart (reasons below); the app runs the previous code", what),
        }
        for l in &o.lines {
            eprintln!("makepad live:   {}", l);
        }
        if o.rung != Rung::Patch {
            patch_refused_event(&what, o.rung == Rung::Refuse, &o.lines);
        }
        crate::tui::flush();
        if o.rung == Rung::Patch { wake_program(); }
        for (j, &k) in which.iter().enumerate() {
            files[k].1 = o.files[j];
        }
        if let Some(u) = o.undo {
            undos.push(u);
            // rollback by patch group arrives with the runtime panic boundaries (M7)
            if undos.len() > 64 {
                undos.remove(0);
            }
        }
    }
}

#[cfg(test)]
mod round32_tests {
    use super::*;

    #[test]
    fn enum_reload_snapshots() {
        isolated_test!(enum_reload_snapshots);
        const CHILD: &str = "MAKEPAD_RUN_ENUM_RELOAD_CASE";
        let Ok(case) = std::env::var(CHILD) else {
            for case in ["same", "changed", "removed", "reordered", "added", "larger", "named", "executable", "callee", "named_code", "unrelated"] {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "reload::round32_tests::enum_reload_snapshots", "--nocapture", "--test-threads=1"])
                    .env(CHILD, case).env("MAKEPAD_RUN_CTFE", "0").env("MAKEPAD_RUN_CACHE", "off").env("MAKEPAD_RUN_GANDALF", "0")
                    .output().unwrap();
                assert!(output.status.success(), "{case}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            }
            return;
        };
        std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
        crate::borrowck::lazy::disable_gandalf();
        let (old, new, accept) = match case.as_str() {
            "same" => ("enum E { A = 1, B }", "enum E { A = 1 + 0, B }", true),
            "changed" => ("enum E { A = 1, B }", "enum E { A = 2, B }", false),
            "removed" => ("enum E { A = 1, B }", "enum E { A = 1 }", false),
            "reordered" => ("enum E { A = 1, B }", "enum E { B = 1, A }", false),
            "added" => ("enum E { A = 1, B }", "enum E { A = 1, B, C }", true),
            "larger" => ("enum E { A, B }", "enum E { A, B, C([u64; 4]) }", false),
            "named" => ("const N: isize = 1; enum E { A = N, B }", "const N: isize = 2; enum E { A = N, B }", false),
            "executable" => ("const fn d() -> isize { 1 } enum E { A = d(), B }", "const fn d() -> isize { 1 } enum E { A = d() + 1, B }", false),
            "callee" => ("const fn d() -> isize { 1 } enum E { A = d(), B }", "const fn d() -> isize { 2 } enum E { A = d(), B }", false),
            // RELOAD': a named const computed by code is a dependency of the discriminant
            "named_code" => ("const fn d() -> isize { 1 } const N: isize = d(); enum E { A = N, B }", "const fn d() -> isize { 2 } const N: isize = d(); enum E { A = N, B }", false),
            // RELOAD': an edit outside the discriminant's dependencies keeps its value, unevaluated
            "unrelated" => ("const fn d() -> isize { 1 } fn g() -> isize { 5 } enum E { A = d(), B }", "const fn d() -> isize { 1 } fn g() -> isize { 6 } enum E { A = d(), B }", true),
            _ => unreachable!(),
        };
        let path = std::env::temp_dir().join(format!("relax-enum-reload-{}.rs", std::process::id()));
        std::fs::write(&path, old).unwrap();
        let mut session = crate::Session::default();
        session.options.ctfe = crate::options::Ctfe::Off;
        let session = std::sync::Arc::new(session);
        let (prog, core, krate) = crate::load_with_std(&session, path.to_str().unwrap(), "enum_reload", false).unwrap();
        let def = prog.defs.iter().enumerate().find_map(|(i, d)| (d.krate == krate && d.kind == DefKind::Enum).then_some(DefId(i as u32))).unwrap();
        let file = prog.def(def).file;
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        crate::layout::set_root_crate(krate);
        crate::layout::LIVE_SESSION.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut unit = Unit::new(prog, tcx, core);
        let up = &mut *unit as *mut Unit;
        let values = |u: &Unit| u.tcx.adts[&def].discriminants(&u.tcx, &u.prog).into_iter().map(|(_, d)| d.val).collect::<Vec<_>>();
        let before = values(&unit);
        let executable = matches!(case.as_str(), "executable" | "callee" | "named_code" | "unrelated");
        if executable { assert!(unit.stats.ctfe_code > 0, "the initializer must have run through JIT"); }
        let runs = unit.stats.ctfe_code;
        assert_eq!(before, if case == "larger" { vec![0, 1] } else { vec![1, 2] });
        let ty = unit.tcx.tys.intern(crate::types::TyKind::Adt(def, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
        let layout = unit.layout_of(ty).clone();
        let outcome = unsafe { apply(up, vec![Edit { file, path: path.to_string_lossy().into_owned(), src: new.as_bytes().to_vec() }]) };
        assert_eq!(outcome.rung, if accept { Rung::Patch } else { Rung::Restart },
            "{case}: {:?}; queried values after transaction: {:?}", outcome.lines, values(&unit));
        assert_eq!(unit.stats.ctfe_code, runs, "{case}: the transaction must not run an initializer");
        if accept {
            assert_eq!(&values(&unit)[..2], before);
            unsafe { rollback(up, outcome.undo.unwrap()); }
        }
        assert_eq!(values(&unit), before, "running values restored");
        assert_eq!(unit.layout_of(ty).size, layout.size);
        assert_eq!(unit.tcx.adts[&def].variants.len(), 2);
        // A second edit after refusal/rollback must still compare to the original text.
        let outcome = unsafe { apply(up, vec![Edit { file: unit.prog.def(def).file, path: path.to_string_lossy().into_owned(), src: old.as_bytes().to_vec() }]) };
        assert_eq!(outcome.rung, Rung::Patch, "{:?}", outcome.lines);
        std::mem::forget(unit);
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod round43_tests {
    #[test]
    fn generated_impl_identity_ignores_tokenization_whitespace() {
        isolated_test!(generated_impl_identity_ignores_tokenization_whitespace);
        let session = std::sync::Arc::new(crate::Session::default());
        let source = br#"
            struct App;
            trait AppMain { fn restart_state(); fn handle_event(); }
            impl AppMain for App {
                fn restart_state() {
                    trait RestartFieldsFallback { fn restart_fields(); }
                    impl RestartFieldsFallback for App { fn restart_fields() {} }
                }
                fn handle_event() {}
            }
            impl App { fn started() {} }
            impl App { fn stopped() {} }
        "#;
        let old = crate::parser::parse_owned(&session, source.to_vec(), 2021).unwrap();
        let mut new = crate::parser::parse_owned(&session, source.to_vec(), 2021).unwrap();
        // A hosted expansion may still be stored as text, while the running
        // version has tokens after its definition bodies have been collected.
        new.toks = Default::default();
        let (pairs, removed, added) = super::pair_items(&old, &new);
        assert!(removed.is_empty(), "removed: {:?}", removed);
        assert!(added.is_empty(), "added: {:?}", added);
        assert_eq!(pairs.len(), old.ast.items.len());
    }
}

#[cfg(test)]
mod round51_tests {
    use super::*;

    #[test]
    fn ctfe_relevance_uses_both_collected_versions() {
        isolated_test!(ctfe_relevance_uses_both_collected_versions);
        // rustc_resolve/src/def_collector.rs:423-448 @8d1a764: anonymous and
        // inline constants are child owners, including those nested in closures.
        let session = std::sync::Arc::new(crate::Session::default());
        for body in ["let _x = const { 3u32 };", "let _x: [u8; 3];",
            "let _x = [0u8; 3];", "let _x = || { const { 3u32 } };",
            "const N: u32 = 3;", "static N: u32 = 3;"] {
            let source = format!("#![no_std] fn owner() {{ {body} }} fn ordinary() {{ let _x = 1u32; }}");
            let path = std::env::temp_dir().join(format!("relax-r51-classify-{}.rs", std::process::id()));
            std::fs::write(&path, &source).unwrap();
            let mut prog = Program::new(&session);
            prog.load_crate("classify", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
            let owner = prog.defs.iter().enumerate().find_map(|(i, _)| (prog.name(DefId(i as u32)) == "owner").then_some(DefId(i as u32))).unwrap();
            let ordinary = prog.defs.iter().enumerate().find_map(|(i, _)| (prog.name(DefId(i as u32)) == "ordinary").then_some(DefId(i as u32))).unwrap();
            let mut tcx = crate::tcx::Tcx::new(); tcx.collect(&prog);
            let file = prog.def(owner).file;
            assert!(ctfe_relevant_bodies(&prog, file, &[owner]), "before: {body}");
            assert!(!ctfe_relevant_bodies(&prog, file, &[ordinary]), "unrelated owner: {body}");
            // Named items change scopes; the body-only transition below tests
            // inline/anonymous owners, whose presence must be checked on removal.
            if !body.starts_with("const N") && !body.starts_with("static N") {
                let edited = source.replace(body, "let _plain = 2u32;");
                let version = prog.parse_version(file, "classify.rs", edited.into_bytes()).unwrap();
                let pairs = pair_items(&prog.files[file as usize], &version.pf).0;
                let new = prog.commit_body_version(version, &pairs).unwrap();
                prog.lower_body_declarations(&[owner]);
                assert!(!ctfe_relevant_bodies(&prog, new, &[owner]), "removed: {body}");
                prog.revert_version(file, new, &pairs);
                assert!(ctfe_relevant_bodies(&prog, file, &[owner]), "restored: {body}");
            }
            std::fs::remove_file(path).unwrap();
        }
    }
}

#[cfg(test)]
mod round51_token_lookup_tests {
    use crate::lexer::{T, Tok};

    #[test]
    fn first_tok_at_steps_over_interp_tokens() {
        isolated_test!(first_tok_at_steps_over_interp_tokens);
        // ordinary tokens in position order; Interp tokens carry an earlier expression span
        let mut toks = Vec::new();
        for i in 0..64u32 {
            if i % 5 == 3 { toks.push(Tok { kind: T::Interp, lo: 2, hi: 4 }); }
            toks.push(Tok { kind: T::Ident, lo: 100 + i * 10, hi: 105 + i * 10 });
        }
        for i in 0..64u32 {
            let at = super::first_tok_at(&toks, 100 + i * 10);
            assert_eq!({ toks[at].lo }, 100 + i * 10, "token {i}");
            let at = super::first_tok_at(&toks, 101 + i * 10);
            assert!(at == toks.len() || { toks[at].lo } == 110 + i * 10, "after token {i}");
        }
        assert_eq!(super::first_tok_at(&toks, 0), 0);
        assert_eq!(super::first_tok_at(&toks, 10_000), toks.len());
    }
}

#[cfg(test)]
mod round52_scope_tests {
    use super::*;

    fn named(prog: &Program, name: &str) -> DefId {
        (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|d| prog.name(*d) == name).unwrap()
    }

    /// One item edit through the declaration path apply_locked takes for a non-CTFE item
    /// edit (lead decision 52 R1): commit, added items, the crate-scoped legacy pass and
    /// scoped publication. Returns the new version, its pairs and the visible crates.
    fn item_edit(prog: &mut Program, tcx: &mut crate::tcx::Tcx, file: u32, path: &str, src: String) -> (u32, Vec<(u32, u32)>, Vec<bool>, usize) {
        let version = prog.parse_version(file, path, src.into_bytes()).unwrap();
        let plan = classify(prog, file, &version.pf, path).unwrap();
        assert!(!plan.added.is_empty(), "the edit adds an item");
        let visible = crates_seeing_edit(prog, [file].into_iter());
        let new = prog.commit_version_in(version, &plan.pairs, &visible).unwrap();
        let items: Vec<(crate::ast::ItemId, Result<u32, DefId>)> = plan.added.iter().map(|&(j, at)| (crate::ast::ItemId(j), at)).collect();
        let added = prog.add_items(new, &items).unwrap();
        let lowered = crate::def_collector::lower_legacy_const_generics_in(prog, Some(&visible));
        tcx.collect_added_in(prog, &added, false, Some(&visible));
        (new, plan.pairs, visible, lowered)
    }

    #[test]
    fn round52_item_edit_reaches_only_crates_that_see_it() {
        isolated_test!(round52_item_edit_reaches_only_crates_that_see_it);
        let dir = std::env::temp_dir().join(format!("relax-r52-scope-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, src: &str| { let p = dir.join(name); std::fs::write(&p, src).unwrap(); p.to_str().unwrap().to_string() };
        let base_src = "#![no_std]\npub struct B(pub u32);\npub fn base_fn() -> u32 { B(1).0 }\n";
        let lib_src = "#![no_std]\npub fn lib_fn() -> u32 { r52_base::base_fn() + 1 }\n";
        let app_src = "#![no_std]\npub fn app_fn() -> u32 { r52_lib::lib_fn() + 2 }\n";
        let (base_path, lib_path, app_path) = (write("base.rs", base_src), write("lib.rs", lib_src), write("app.rs", app_src));
        let session = std::sync::Arc::new(crate::Session::default());
        let cfg = crate::cfg::CfgSet::host(&session, &[]);
        let mut prog = Program::new(&session);
        let base = prog.load_crate("r52_base", &base_path, 2021, cfg.clone(), vec![]).unwrap();
        let base_sym = prog.syms.intern("r52_base");
        let lib = prog.load_crate("r52_lib", &lib_path, 2021, cfg.clone(), vec![(base_sym, base)]).unwrap();
        let lib_sym = prog.syms.intern("r52_lib");
        let app = prog.load_crate("r52_app", &app_path, 2021, cfg, vec![(lib_sym, lib)]).unwrap();
        prog.resolve_imports();
        assert!(prog.errors.is_empty(), "{:?}", prog.errors);
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        let owners = [named(&prog, "base_fn"), named(&prog, "lib_fn"), named(&prog, "app_fn")];
        let typeck = |prog: &Program, tcx: &crate::tcx::Tcx| owners.map(|d| tcx.typeck(prog, d));
        let legacy = |prog: &Program| owners.map(|d| prog.legacy_const_args[&d].clone());
        let owners_in = |prog: &Program, krate: u32| prog.legacy_const_args.keys().filter(|d| prog.def(**d).krate == krate).count();
        let file_of = |prog: &Program, krate: u32| prog.mods[prog.crates[krate as usize].root_mod as usize].file;

        // An item added in the app crate: only the app crate's owners are lowered again and
        // only its queries recompute; base (std-like) and lib (widgets-like) keep everything.
        let before = typeck(&prog, &tcx);
        let legacy_before = legacy(&prog);
        for b in &before { assert!(b.errors.is_empty(), "{:?}", b.errors); }
        let app_file = file_of(&prog, app);
        let (new, pairs, visible, lowered) = item_edit(&mut prog, &mut tcx, app_file, &app_path, format!("{app_src}pub fn app_added() -> u32 {{ 3 }}\n"));
        assert_eq!(visible, vec![false, false, true]);
        assert_eq!(lowered, owners_in(&prog, app), "the legacy pass covers the app crate's owners only");
        let legacy_after = legacy(&prog);
        assert!(std::sync::Arc::ptr_eq(&legacy_before[0], &legacy_after[0]) && std::sync::Arc::ptr_eq(&legacy_before[1], &legacy_after[1]));
        assert!(!std::sync::Arc::ptr_eq(&legacy_before[2], &legacy_after[2]));
        let after = typeck(&prog, &tcx);
        let recomputed: Vec<bool> = (0..3).map(|i| !std::sync::Arc::ptr_eq(&before[i], &after[i])).collect();
        assert_eq!(recomputed, vec![false, false, true], "only the app owner's typeck recomputes");
        for b in &after { assert!(b.errors.is_empty(), "{:?}", b.errors); }
        // the refused/rolled-back edit: the saved declaration state of the app crate returns
        prog.revert_version(app_file, new, &pairs);
        tcx.publish_declarations(Some(&visible));
        assert!(std::sync::Arc::ptr_eq(&legacy_before[2], &prog.legacy_const_args[&owners[2]]), "revert restores the scoped legacy results");
        assert!(std::sync::Arc::ptr_eq(&legacy_before[0], &prog.legacy_const_args[&owners[0]]));
        let reverted = typeck(&prog, &tcx);
        assert!(std::sync::Arc::ptr_eq(&before[0], &reverted[0]) && std::sync::Arc::ptr_eq(&before[1], &reverted[1]));

        // An item added in the lib (widget) crate reaches the app crate that depends on it.
        let before = reverted;
        let lib_file = file_of(&prog, lib);
        let (_, _, visible, lowered) = item_edit(&mut prog, &mut tcx, lib_file, &lib_path, format!("{lib_src}pub fn lib_added() -> u32 {{ 4 }}\n"));
        assert_eq!(visible, vec![false, true, true]);
        assert_eq!(lowered, owners_in(&prog, lib) + owners_in(&prog, app));
        let after = typeck(&prog, &tcx);
        let recomputed: Vec<bool> = (0..3).map(|i| !std::sync::Arc::ptr_eq(&before[i], &after[i])).collect();
        assert_eq!(recomputed, vec![false, true, true], "lib and its dependent app recompute; base keeps its result");
        for b in &after { assert!(b.errors.is_empty(), "{:?}", b.errors); }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod round52_r2_tests {
    use super::*;

    /// Lead decision 52 (R2): which changed constants and statics a constant-evaluated
    /// context can read (const_contexts_mention), one item at a time.
    #[test]
    fn round52_const_context_mentions() {
        isolated_test!(round52_const_context_mentions);
        let source = r#"#![no_std]
pub const C_FN: u32 = 1;
pub const C_CONST: u32 = 2; const D: u32 = C_CONST + 1;
pub const C_LEN: usize = 3; fn uses_len() { let _a: [u8; C_LEN] = [0; 3]; }
pub const C_REPEAT: usize = 4; fn uses_repeat() { let _a = [0u8; C_REPEAT]; }
pub const C_BODY: u32 = 5; const fn cf() -> u32 { C_BODY }
pub const C_STATIC: u32 = 6; static S_FROM_CONST: u32 = C_STATIC;
pub const C_ALIAS: u32 = 7; mod m { pub use super::C_ALIAS as RENAMED; } const E: u32 = m::RENAMED;
pub const C_INLINE: u32 = 8; fn uses_inline() -> u32 { const { C_INLINE } }
macro_rules! get { () => { C_MACRO } }
pub const C_MACRO: u32 = 9; const F: u32 = get!();
pub static S_FN: [u32; 2] = [1, 2];
pub static S_ADDR: [u32; 2] = [3, 4]; static HOLDER: &[u32; 2] = &S_ADDR;
pub static S_READ: [u32; 2] = [5, 6]; const fn cf2() -> u32 { S_READ[0] }
pub static S_SIZED: usize = 2; fn ss() { let _x: [u8; S_SIZED]; }
fn ordinary() -> u32 { let _a = S_FN[0]; C_FN + S_FN[1] }
"#;
        let path = std::env::temp_dir().join(format!("relax-r52-mentions-{}.rs", std::process::id()));
        std::fs::write(&path, source).unwrap();
        let session = std::sync::Arc::new(crate::Session::default());
        let mut prog = Program::new(&session);
        prog.load_crate("r52_mentions", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
        prog.resolve_imports();
        let named = |name: &str| (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|d| prog.name(*d) == name && matches!(prog.def(*d).kind, DefKind::Const | DefKind::Static)).unwrap();
        for (name, expected) in [("C_FN", false), ("C_CONST", true), ("C_LEN", true), ("C_REPEAT", true), ("C_BODY", true),
            ("C_STATIC", true), ("C_ALIAS", true), ("C_INLINE", true), ("C_MACRO", true),
            ("S_FN", false), ("S_ADDR", false), ("S_READ", true), ("S_SIZED", true)] {
            assert_eq!(const_contexts_mention(&prog, &[named(name)]), expected, "{name}");
        }
        assert!(const_contexts_mention(&prog, &[named("C_FN"), named("C_LEN")]), "any item of the transaction");
        std::fs::remove_file(path).unwrap();
    }

    /// R2 (a): literal initializers fold without executing code; anything else does not count.
    #[test]
    fn round52_literal_initializers() {
        isolated_test!(round52_literal_initializers);
        let source = r#"#![no_std]
pub struct Origin { pub file: &'static str, pub offset: u32, pub line: i32 }
pub struct WithDefault { pub a: u32 }
const fn f() -> u32 { 1 }
const K: u32 = 2;
pub static L_FILES: [(&str, u64); 2] = [("a.rx", 1), ("b.rx", 2)];
pub static L_ORIGINS: [Origin; 1] = [Origin { file: "a.rx", offset: 3, line: -4 }];
pub static L_REPEAT: [u8; 4] = [0; 4];
pub static L_REF: &[u32] = &[1, 2];
pub const L_SCALAR: (u32, bool) = (1, true);
pub static N_CALL: u32 = f();
pub static N_PATH: u32 = K;
pub static N_BLOCK: u32 = { 3 };
pub static N_COUNT: [u8; K as usize] = [0; K as usize];
pub static N_BINARY: u32 = 1 + 2;
"#;
        let path = std::env::temp_dir().join(format!("relax-r52-literal-{}.rs", std::process::id()));
        std::fs::write(&path, source).unwrap();
        let session = std::sync::Arc::new(crate::Session::default());
        let mut prog = Program::new(&session);
        prog.load_crate("r52_literal", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
        for i in 0..prog.defs.len() {
            let d = DefId(i as u32);
            let name = prog.name(d).to_string();
            if !(name.starts_with("L_") || name.starts_with("N_")) || !matches!(prog.def(d).kind, DefKind::Const | DefKind::Static) { continue; }
            let e = relax_types::const_eval::const_body(&prog, d).unwrap();
            assert_eq!(literal_initializer(&prog.files[prog.def(d).file as usize], e, 0), name.starts_with("L_"), "{name}");
        }
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod refusal_tests {
    use super::*;

    /// A refused edit's re-check lists the type error the first attempt found (path:line:col and
    /// message), not only the summary its retry adds.
    #[test]
    fn a_refused_edit_reports_its_type_error_not_only_the_summary() {
        isolated_test!(a_refused_edit_reports_its_type_error_not_only_the_summary);
        let e0308 = "/c/main.rs:51:24: E0308: mismatched types: expected `u32`, found `&str`".to_string();
        let summary = "/c/main.rs:53:13: type checking failed due to previous errors".to_string();
        assert_eq!(refusal_lines(vec![e0308.clone()], vec![summary.clone()]), vec![e0308.clone()]);
        // a retry that finds the error again adds nothing twice
        assert_eq!(refusal_lines(vec![e0308.clone()], vec![e0308.clone(), summary.clone()]), vec![e0308.clone()]);
        // only the summary known: it stays (something must say why)
        assert_eq!(refusal_lines(Vec::new(), vec![summary.clone()]), vec![summary]);
        // a new reason of the retry is kept after the first ones
        let other = "/c/other.rs:3:1: E0425: cannot find value `y`".to_string();
        assert_eq!(refusal_lines(vec![e0308.clone()], vec![other.clone()]), vec![e0308, other]);
    }
}
