//! Gandalf, Relax's lazy borrow checker (scheduling). Named by Kevin Boos.
//!
//! Hot reload: an edit of one or more source files applied as ONE all-or-nothing transaction
//! (local/plans/hotreload.md §9: M1 transaction, M3 classifier).
//!
//! 1. Parse every edited file as a new version (nothing the running program reads changes).
//! 2. Classify each item against the running version: body edits (class B), const value
//!    edits (B, with the readers recompiled) and initializer edits of statics not placed yet
//!    apply, as do supported layout, macro and visibility edits. Eligible named-field structs
//!    keep existing field slots and add defaulted fields in extension blocks. Unsupported edits
//!    offer a restart with a reason; the app keeps running until `session relaunch` is requested.
//!    The landing check also offers a restart when old code remains on a thread's stack.
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
use relax_types::mir::transform::const_check::is_live_default;
use std::time::Instant;
use std::sync::atomic::Ordering;
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
    // the names of the bodies the patch replaced (printed on the patch line)
    bodies: Vec<String>,
}
// The transaction runs on the watcher thread and on the owner thread (unit calls): one process-wide trace.
static TRACE_CELL: relax_session::sync::Mutex<ReloadTrace> = relax_session::sync::Mutex::new(ReloadTrace::new());
struct TraceCell;
impl TraceCell {
    fn with<R>(&self, f: impl FnOnce(&mut ReloadTrace) -> R) -> R {
        f(&mut TRACE_CELL.lock().unwrap_or_else(|e| e.into_inner()))
    }
}
static TRACE: TraceCell = TraceCell;
thread_local! {
    static TRACE_INPUT: std::cell::Cell<(f64, f64, f64, usize)> = const { std::cell::Cell::new((0.0, 0.0, 0.0, 0)) };
}

pub(crate) fn reload_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_RELOAD_TRACE").is_some())
}

fn stream_timing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_STREAM_TIMING").is_some())
}

fn live_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_LIVE_TRACE").is_some())
}

/// The watcher's part of an edit: `watch_ms` from the save to the read, `look_ms` of it from the
/// change seen to the read (the settle looks), `read_ms` reading the files.
pub(crate) fn trace_input(watch_ms: f64, look_ms: f64, read_ms: f64, scanned: usize) {
    if reload_trace() {
        TRACE_INPUT.with(|s| s.set((watch_ms, look_ms, read_ms, scanned)));
    }
}
fn trace_stage(name: &'static str) {
    TRACE.with(|s| {
        if let Some(last) = s.last {
            let now = Instant::now();
            *s.stages.entry(name).or_default() += (now - last).as_secs_f64() * 1000.;
            s.last = Some(now);
            s.sub_last = Some(now);
            let cpu = relax_session::clock::cpu_ns();
            let elapsed = cpu.saturating_sub(s.cpu_last) as f64 / 1e6;
            *s.cpu.entry(name).or_default() += elapsed;
            s.cpu_last = cpu;
        }
    });
}
/// A finer split of the running stage: wall time since the stage began or the previous sub mark.
pub(crate) fn trace_sub(name: &'static str) {
    TRACE.with(|s| {
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
    let traced = TRACE.with(|s| s.started.is_some());
    let holder = if traced { Some(crate::owner::serving_note()) } else { None };
    let t = Instant::now();
    crate::owner::call(crate::owner::Class::Program, || {
        let waited = t.elapsed().as_secs_f64() * 1e3;
        if let Some(h) = holder.filter(|_| waited > 5.0) {
            eprintln!("reload-trace: unit call wait {waited:.1} ms before {at}: the owner was serving {h}");
        }
        f()
    })
}
fn trace_bodies(prog: &Program, bodies: &[DefId]) {
    TRACE.with(|s| {
        if s.started.is_some() { s.bodies = bodies.iter().map(|&d| def_name(prog, d)).collect(); }
    });
}
pub(crate) fn trace_count(name: &'static str, n: usize) {
    TRACE.with(|s| {
        if s.started.is_some() { *s.counts.entry(name).or_default() += n; }
    });
}
struct TraceTransaction;
impl ReloadTrace {
    const fn new() -> Self {
        Self { started: None, last: None, cpu_last: 0, cpu: std::collections::BTreeMap::new(), stages: std::collections::BTreeMap::new(),
            sub_last: None, subs: std::collections::BTreeMap::new(), counts: std::collections::BTreeMap::new(), bodies: Vec::new() }
    }
}
impl TraceTransaction {
    fn begin(files: usize) -> Self {
        if reload_trace() {
            let now = Instant::now();
            let (watch, look, read, scanned) = TRACE_INPUT.with(|s| s.replace((0.0, 0.0, 0.0, 0)));
            let mut state = ReloadTrace { started: Some(now), last: Some(now), cpu_last: relax_session::clock::cpu_ns(), ..Default::default() };
            state.stages.insert("watch", watch);
            state.subs.insert("look", look);
            state.stages.insert("read", read);
            state.counts.insert("watch_files", scanned);
            state.counts.insert("edit_files", files);
            TRACE.with(|s| *s = state);
            crate::parse_work::begin();
        }
        Self
    }
    fn finish(&self, out: &Outcome) {
        trace_stage("other");
        TRACE.with(|s| {
            if let Some(start) = s.started {
                let stages = s.stages.iter().map(|(k,v)| format!("{k}_ms={v:.3}")).collect::<Vec<_>>().join(" ");
                let cpu = s.cpu.iter().map(|(k,v)| format!("{k}_cpu_ms={v:.3}")).collect::<Vec<_>>().join(" ");
                let mut counts = s.counts.iter().map(|(k,v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ");
                for (k, v) in &s.subs { counts.push_str(&format!(" sub.{k}_ms={v:.3}")); }
                let parse = crate::parse_work::finish();
                let bodies = if out.rung == Rung::Patch { format!(" (bodies {:?})", s.bodies) } else { String::new() };
                eprintln!("reload-trace: rung={}{} total_ms={:.3} {} {} {} changed={} installed={} ahead={} parse_detail_ms={:?} style_reapply=unobserved",
                    out.rung.name(), bodies, ms(start) + s.stages["watch"] + s.stages["read"], stages, cpu, counts,
                    out.changed, out.names.len(), out.work[1], parse.0);
            }
        });
    }
}
impl Drop for TraceTransaction {
    fn drop(&mut self) { TRACE.with(|s| *s = ReloadTrace::default()); }
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
    // Canonical paths prevent a path dependency's symlink from escaping the project. Virtual
    // (git/VFS) workspaces have no filesystem path; lexical normalization works there.
    let tree = crate::vfs::canonical(&tree);
    units.units.iter().filter_map(|unit| {
        if !unit.pkg_id.starts_with("path+file://") { return None; }
        if matches!(unit.name.as_str(), "std" | "core" | "alloc" | "proc_macro") { return None; }
        let source = unit.src_path.as_str();
        let path = crate::vfs::canonical(source);
        let rel = std::path::Path::new(&path).strip_prefix(&tree).ok()?;
        // tools/: the framework's runtime crates apps link (relax, ...)
        if ["platform", "draw", "widgets", "libs", "tools", "engine", "relax",
            "apps/commercial/engine"].iter().any(|p| rel.starts_with(p)) {
            return None;
        }
        Some(crate::vfs::normalize_path(source))
    }).collect()
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
    /// The running version's file index. `apply_update` also accepts u32::MAX
    /// for an unloaded source snapshot accompanying its edited module parent.
    pub file: u32,
    pub path: String,
    pub src: Vec<u8>,
}

pub struct Outcome {
    pub rung: Rung,
    /// A compiler panic is a failed hot load even though a fresh load can take it.
    pub failure: Option<makepad_remote::hot::Outcome>,
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
    /// Body definitions, including uninstantiated fns; None requires a full test run.
    pub test_changes: Option<Vec<DefId>>,
    /// the body owners whose bodies the edit changed (a test session lints them: test_edit)
    pub bodies: Vec<DefId>,
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
    variants: Vec<(DefId, Vec<DefId>)>,
    const_failed: Vec<(DefId, Vec<TyId>)>,
    full_inputs: bool,
    /// a const edit moved the code keys: they follow the rollback's text
    rekeyed: bool,
    // Lead decision 52 (R1): the crates a declaration publication reached (None: all).
    scope: Option<Vec<bool>>,
    /// the imports the edit unbound (its removed `use` items) or resolved again (theirs had gone)
    uses: Vec<crate::program::UseUndo>,
    /// the items of the traits the edit changed, in their order before it
    traits: Vec<(usize, Vec<DefId>)>,
    /// the fns the edit declared again (a type their signature names moved)
    redeclared: Vec<DefId>,
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

/// The running storage an edited const or static initializer made, once placed: `d` itself (a
/// static) or a static defined inside its initializer (a `thread_local!` key's).
fn placed_storage(u: &Unit, d: DefId) -> Option<DefId> {
    u.statics.keys().chain(u.tls_places.keys()).copied().find(|&s| changed_body_owner(&u.prog, s, &[d]))
}

/// Every constant-evaluated context that reads one of `hit` by name, directly or through the consts
/// and const fns it names (`const_contexts_reading`, so aliases and macro text count). None: it
/// cannot say. Same-named items count: a false restart, never stale code.
fn consts_read_through(prog: &Program, hit: &[DefId]) -> Option<crate::fxhash::HashSet<DefId>> {
    let mut found = crate::fxhash::HashSet::default();
    let mut next = hit.to_vec();
    while !next.is_empty() {
        let reads = const_contexts_reading(prog, &next)?;
        next = reads.into_iter().filter(|d| found.insert(*d) && matches!(prog.def(*d).kind, DefKind::Const | DefKind::AssocConst | DefKind::Fn | DefKind::AssocFn)).collect();
    }
    Some(found)
}

/// Whether a type reads `hit`: an array length, repeat count, const argument or discriminant, or a
/// type or impl header naming it as a generic argument. A body naming it lands: its keys show it.
fn read_in_a_type(prog: &Program, hit: &[DefId]) -> bool {
    consts_read_through(prog, hit).is_none_or(|found| found.iter().any(|c| matches!(prog.def(*c).kind,
        DefKind::AnonConst | DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TypeAlias | DefKind::AssocTy | DefKind::Impl)))
}

/// One of `hit` a type reads: it was evaluated, or `read_in_a_type`. Types have no key of their own,
/// and a warm run evaluated only what it checked.
fn const_read_by_a_type(u: &Unit, hit: &[DefId]) -> Option<DefId> {
    let prog = &u.prog;
    if let Some(&d) = hit.iter().find(|d| u.tcx.const_was_evaluated(**d)) { return Some(d); }
    if !read_in_a_type(prog, hit) { return None; }
    hit.iter().copied().find(|d| read_in_a_type(prog, &[*d])).or(hit.first().copied())
}

/// A static with storage whose initializer reads one of `hit`: its running value is the old text's.
fn static_of_changed_consts(u: &Unit, hit: &[DefId]) -> Option<DefId> {
    let mut live = u.statics.iter().filter(|(_, &a)| a != 0).map(|(d, _)| *d).peekable();
    live.peek()?;
    let found = consts_read_through(&u.prog, hit)?;
    live.find(|d| found.contains(d))
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
    const_contexts_reading(prog, items).is_none_or(|found| !found.is_empty())
}

/// `names` and what else can call those items: a `use` renaming one, a type alias whose text names one.
fn renames_of(prog: &Program, mut names: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let aliases: Vec<DefId> = (0..prog.defs.len()).map(|i| DefId(i as u32)).filter(|d| prog.def(*d).kind == DefKind::TypeAlias).collect();
    let mut idents: Vec<Vec<u8>> = names.iter().filter(|n| n.first().is_some_and(|b| *b == b'_' || b.is_ascii_alphabetic())).cloned().collect();
    loop {
        let refs: Vec<&[u8]> = idents.iter().map(|n| n.as_slice()).collect();
        let mut more: Vec<Vec<u8>> = prog.imports.iter().filter(|im| !im.glob).filter_map(|im| {
            let (bind, last) = (im.bind?, im.segs.last()?.1);
            (bind != last && refs.contains(&prog.syms.str(last).as_bytes())).then(|| prog.syms.str(bind).as_bytes().to_vec())
        }).collect();
        more.extend(aliases.iter().filter(|a| item_mentions(prog, **a, &refs)).map(|a| def_name(prog, *a).into_bytes()));
        more.retain(|n| !names.contains(n));
        more.sort_unstable();
        more.dedup();
        if more.is_empty() {
            return names;
        }
        names.extend(more.iter().cloned());
        idents.extend(more);
    }
}

/// The definitions of the contexts `const_contexts_mention` counts; None: it cannot say.
fn const_contexts_reading(prog: &Program, items: &[DefId]) -> Option<crate::fxhash::HashSet<DefId>> {
    use crate::def_collector::NodeId;
    let mut found = crate::fxhash::HashSet::default();
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
    let consts = alias_closure(items.iter().filter(|d| matches!(prog.def(**d).kind, DefKind::Const | DefKind::AssocConst | DefKind::Fn | DefKind::AssocFn)).map(|d| def_name(prog, *d).into_bytes()).collect());
    // the constant-evaluated contexts, per file: (lo, hi, whether static names count, whose); and the
    // ranges of every other item of those files (text there is not a context's, unless a
    // context range inside it holds it)
    let mut contexts: crate::fxhash::HashMap<u32, Vec<(u32, u32, bool, DefId)>> = Default::default();
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
        // a type's own text names a const only as a generic argument, an array length or a
        // discriminant; an impl's only in its header
        if matches!(def.kind, DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TypeAlias | DefKind::AssocTy | DefKind::Impl) {
            let pf = &prog.files[def.file as usize];
            let it = pf.ast.item(def.item);
            let hi = if def.kind != DefKind::Impl { it.hi } else {
                pf.src.get(it.lo as usize..it.hi as usize).and_then(|t| t.iter().position(|b| *b == b'{')).map_or(it.hi, |at| it.lo + at as u32)
            };
            contexts.entry(def.file).or_default().push((it.lo, hi, true, d));
            continue;
        }
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
                    contexts.entry(file).or_default().push((x.lo, x.hi, true, d));
                }
                _ => return None,
            }
            continue;
        }
        let pf = &prog.files[def.file as usize];
        let it = pf.ast.item(def.item);
        contexts.entry(def.file).or_default().push((it.lo, it.hi, statics_count, d));
    }
    let ident = |b: u8| b == b'_' || b.is_ascii_alphanumeric();
    // A const fn of an inherent impl goes by its type: `OnceLock::new` is not this `new`. What may
    // name its type (its impl's header, `Self`, a renaming of those) counts; None: any mention counts.
    // (No path reaches a const fn through a generic parameter: const trait calls are unstable.)
    let owners = |name: &str| -> Option<Vec<Vec<u8>>> {
        let mut named = items.iter().filter(|d| prog.syms.str(prog.def(**d).name) == name).peekable();
        named.peek()?;
        let mut owners = vec![b"Self".to_vec()];
        for d in named {
            let parent = prog.def(*d).parent;
            if prog.def(*d).kind != DefKind::AssocFn || parent == crate::program::NO_DEF || prog.def(parent).kind != DefKind::Impl { return None; }
            let pf = &prog.files[prog.def(parent).file as usize];
            let it = pf.ast.item(prog.def(parent).item);
            let hi = pf.src.get(it.lo as usize..it.hi as usize).and_then(|t| t.iter().position(|b| *b == b'{')).map_or(it.hi, |at| it.lo + at as u32);
            owners.extend(toks(pf, it.lo, hi, &[]).into_iter().map(|t| t.to_vec()));
        }
        Some(renames_of(prog, owners))
    };
    for (&file, ranges) in &contexts {
        let pf = &prog.files[file as usize];
        let other: &[(u32, u32)] = others.get(&file).map_or(&[], |v| v.as_slice());
        let src: &[u8] = &pf.src[..];
        // (source text is UTF-8; text that is not counts as a mention)
        let Ok(text) = std::str::from_utf8(src) else { return None };
        let mut calls: Vec<(u32, u32, u32)> = pf.exp_map.iter().collect();
        calls.sort_unstable();
        for name in &all {
            let Ok(name) = std::str::from_utf8(name) else { return None };
            let is_const = consts.iter().any(|n| n.as_slice() == name.as_bytes());
            let owners = owners(name);
            for (start, _) in text.match_indices(name) {
                let end = start + name.len();
                if (start > 0 && ident(src[start - 1])) || (end < src.len() && ident(src[end])) { continue; }
                // another item's definition under this name reads nothing
                let before = src[..start].trim_ascii_end();
                if [&b"fn"[..], b"const", b"static"].iter().any(|k| before.ends_with(k) && before.len() < start && !before[..before.len() - k.len()].last().is_some_and(|b| ident(*b))) { continue; }
                if let Some(owners) = owners.as_ref().filter(|_| start >= 2 && &src[start - 2..start] == b"::") {
                    let path = src[..start - 2].iter().rposition(|b| !ident(*b)).map_or(0, |at| at + 1);
                    let q = &src[path..start - 2];
                    if !q.is_empty() && !owners.iter().any(|o| o.as_slice() == q) { continue; }
                }
                // a context holds it, or another item, or it is expansion text followed back to
                // its call site; text no context, item or call site holds is no definition's
                // (a cfg-disabled generated item, module-level text) and is never evaluated
                let mut at = start as u32;
                let mut steps = 0;
                loop {
                    let mut held = false;
                    for &(lo, hi, statics_count, d) in ranges {
                        if at >= lo && at < hi && (statics_count || is_const) { held = true; found.insert(d); }
                    }
                    if held { break; }
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
    Some(found)
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

/// A refused or rolled-back edit's traits get their items back in their order (the vtables the
/// running code makes are laid out from it), and the impls are linked to them again.
fn restore_traits(u: &mut Unit, saved: &[(usize, Vec<DefId>)], scope: Option<&[bool]>) {
    if saved.is_empty() {
        return;
    }
    for (ti, items) in saved {
        u.prog.traits[*ti] = items.clone();
    }
    relax_front::assoc::collect_in(&mut u.prog, scope);
}

/// A refused or rolled-back edit's removed impls go back into the impl tables (the edit took
/// them out: apply_locked's commit).
/// Fns declared again against the program as it is now: a type their unchanged signature names
/// moved, so the name means another type (decls_naming_removed); and back when the edit is
/// refused or rolled back. The collection's errors (none for the running text).
fn redeclare(u: &mut Unit, fns: &[DefId], scope: Option<&[bool]>) -> Vec<String> {
    if fns.is_empty() {
        return Vec::new();
    }
    u.tcx.invalidate_check(fns);
    u.tcx.collect_added_in(&u.prog, fns, false, scope)
}

fn recollect_impls(u: &mut Unit, removed: &[DefId], scope: Option<&[bool]>) {
    let impls: Vec<DefId> = removed.iter().copied().filter(|d| u.prog.def(*d).kind == DefKind::Impl).collect();
    if !impls.is_empty() {
        // (they were collected without errors before the edit)
        let _ = u.tcx.collect_added_in(&u.prog, &impls, false, scope);
    }
    // a trait impl's fn back in its impl is linked to the trait's item again (the edit linked the
    // impl's items without it: a fn replaced for a new signature left before its new one came)
    if removed.iter().any(|&d| u.prog.def(d).kind == DefKind::AssocFn && u.tcx.impl_trait.contains_key(&u.prog.def(d).parent) && !u.prog.trait_item_def_id.contains_key(&d)) {
        relax_front::assoc::collect_in(&mut u.prog, scope);
    }
}

/// Rolls back an installed patch group (a panic or fault in its code, M7): the slots return
/// to their previous code, the files to their previous versions, the consts to their previous
/// values. Returns the rolled-back functions' names. The caller makes a unit call.
/// The group's code is invalidated (`why`: what rejected it): it never runs again (invalid.rs), its
/// O3 code and unverified-mode twins included; a thread still in it parks at its next instruction.
pub unsafe fn rollback(up: *mut Unit, undo: Undo, why: &str) -> Vec<String> {
    crate::owner::call(crate::owner::Class::Program, || {
        let u = &mut *up;
        u.flush_cache_records();
        jit::bump_patch_epoch();
        let ids = u.rollback_gen(undo.group);
        let group = undo.group;
        // code built after the patch that inlined its bodies is in its group (invalid::group_of);
        // RIR built meanwhile that took them in and has no code yet is dropped (built anew from
        // the restored source)
        for &id in &ids {
            for d in u.fns[id as usize].history().inlined_into.clone() {
                if !u.fns[d as usize].compiled {
                    u.fns[d as usize].rir = None;
                }
            }
        }
        crate::invalid::invalidate(u, |g| g.gen() == group, group, "invalidated code", None, &format!("patch gen {} rolled back: {}", group, why));
        let names = ids.iter().map(|id| u.fns[*id as usize].name.to_string()).collect();
        {
            let fw = jit::front_write();
            u.tcx.collect_epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let impls: Vec<DefId> = undo.added.iter().copied().filter(|d| u.prog.def(*d).kind == DefKind::Impl).collect();
            u.prog.unbind_defs(&undo.added);
            if !impls.is_empty() { u.tcx.uncollect_impls(&impls); }
            u.prog.rebind_use(undo.uses);
            u.prog.rebind_defs(&undo.removed);
            for (k, &(o, n)) in undo.versions.iter().enumerate().rev() {
                u.prog.revert_version(o, n, &undo.pairs[k]);
            }
            recollect_impls(u, &undo.removed, undo.scope.as_deref());
            restore_traits(u, &undo.traits, undo.scope.as_deref());
            let _ = redeclare(u, &undo.redeclared, undo.scope.as_deref());
            for (d, had) in &undo.variants { u.prog.restore_variants(*d, had); }
            for (d, adt) in &undo.adts {
                if adt.is_enum { u.tcx.insert_ctor_sigs(*d, adt); }
                u.tcx.adts.insert(*d, adt.clone());
            }
            publish_reload(u, &undo.versions, undo.full_inputs, undo.scope.as_deref());
            u.lay.clear();
            u.prog.reclaim_caches();
            drop(fw);
        }
        if undo.rekeyed { crate::cache::rekey_after_revert(up); }
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
        // (the versions before the patch's: the boot text again, for a first edit)
        settle_edited_crates(up);
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
    /// a module is more visible than before: it takes its new visibility with the commit, and
    /// the fns naming it are checked again
    Vis(DefId),
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
    // past the file's text: the expansions of its lazily parsed bodies (a part's tokens and text,
    // ParsedFile::part). The file's own text does not reach them: a range read from it there
    // panicked the live watcher and an edit's apply ("range start index 403878 out of range for
    // slice of length 403483": an app's include lookup after a landing looked into a parsed
    // body's macro expansions).
    if lo as usize >= pf.src.len() {
        let Some(p) = pf.part_at(lo) else { return Vec::new() };
        let i0 = first_tok_at(&p.toks, lo);
        let out: Vec<&[u8]> = p.toks.iter_from(i0).take_while(|t| t.hi <= hi)
            .filter(|t| !holes.iter().any(|&(a, b)| t.lo >= a && t.hi <= b))
            .map(|t| pf.bytes(t.lo, t.hi)).collect();
        if out.is_empty() && hi > lo && holes.is_empty() {
            return vec![pf.bytes(lo, hi)];
        }
        return out;
    }
    let mut out = Vec::new();
    let i0 = first_tok_at(&pf.toks, lo);
    let mut covered = false;
    let mut prev_hi = lo;
    for t in pf.toks.iter_from(i0) {
        if t.hi > hi {
            break;
        }
        covered = true;
        // every split of break_and_eat keeps only the remaining token: read its consumed
        // char too, so tokens compare like text (fab-model's `&&r.file` lost its first `&`).
        let split = t.lo > prev_hi && t.lo > 0 && matches!((pf.src.get(t.lo as usize - 1), pf.src.get(t.lo as usize)),
            (Some(b'<'), Some(b'<' | b'=' | b'-')) | (Some(b'>'), Some(b'>' | b'='))
            | (Some(b'&'), Some(b'&' | b'=')) | (Some(b'|'), Some(b'|' | b'=')) | (Some(b'+'), Some(b'=')));
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
                out.push(pf.bytes(at, a));
            }
            at = at.max(b);
        }
        if hi > at {
            out.push(pf.bytes(at, hi));
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
        ForeignMod(..) => "an extern block's item list changed",
        MacroRules(_) => "a macro changed; its expansion sites are not re-expanded live yet",
        Mac(_) => "an item macro changed",
        _ => "this kind of edit is not applied live yet",
    }
}

/// Whether item `j` of `pf` is a type alias written in a body (a fn's, a const's or a
/// static's) that no other item of that body names: it is part of the body, which is all that
/// names it. An item beside it naming it (a nested fn's signature) would keep its old type.
fn body_local_alias(pf: &crate::parser::ParsedFile, reached: &[bool], j: u32) -> bool {
    use crate::ast::ItemKind;
    let items = &pf.ast.items;
    let it = &items[j as usize];
    if !matches!(it.kind, ItemKind::TypeAlias(_)) || reached[j as usize] || it.lo >= pf.src_len {
        return false;
    }
    // the item whose body holds it: the narrowest one around it in the module tree
    let around = |k: usize, at: u32| items[k].lo < at && at < items[k].hi;
    let Some(h) = (0..items.len()).filter(|&k| reached[k] && matches!(items[k].kind, ItemKind::Fn(..) | ItemKind::Const(..) | ItemKind::Static(..)) && around(k, it.lo))
        .min_by_key(|&k| items[k].hi - items[k].lo) else { return false };
    let name = &pf.src[it.name.lo as usize..it.name.hi as usize];
    // (an item a macro call in the body made is at the call)
    !(0..items.len()).any(|k| k != j as usize && !reached[k] && around(h, pf.call_site(items[k].lo))
        && toks(pf, items[k].lo, items[k].hi, &[]).iter().any(|t| *t == name))
}

/// A module item's head as written: its visibility's tokens (none: private) and the rest
/// (`mod name`, and the `;` of a module in a file of its own).
fn mod_head<'a>(pf: &'a crate::parser::ParsedFile, it: &crate::ast::Item) -> (Vec<&'a [u8]>, Vec<&'a [u8]>) {
    let end = if matches!(it.kind, crate::ast::ItemKind::Mod(Some(_))) { it.name.hi } else { it.hi };
    let head = toks(pf, it.lo, end, &[]);
    let k = head.iter().position(|t| *t == b"mod").unwrap_or(0);
    (head[..k].to_vec(), head[k..].to_vec())
}

/// Whether visibility `new` reaches everywhere `old` does: from private (`pub(self)`) to any,
/// from any to `pub`, from `pub(super)` or `pub(in ..)` to `pub(crate)`.
fn vis_widens(old: &crate::ast::Vis, new: &crate::ast::Vis) -> bool {
    use crate::ast::Vis;
    let rank = |v: &Vis| match v { Vis::Private | Vis::SelfMod => 0, Vis::Super | Vis::In(_) => 1, Vis::Crate => 2, Vis::Pub => 3 };
    rank(old) < rank(new)
}

/// A fn item's body, as a byte range of its file (a lazily parsed one's too).
fn fn_body_span(pf: &crate::parser::ParsedFile, kind: &crate::ast::ItemKind) -> Option<(u32, u32)> {
    use crate::ast::ItemKind;
    match kind {
        ItemKind::Fn(_, Some(b), _) => { let b = pf.ast.block(*b); Some((b.lo, b.hi)) }
        ItemKind::Fn(sig, None, _) => pf.ast.fn_sig(*sig).lazy_body.map(|r| (pf.tok(r.lo - 1).lo, pf.tok(r.hi).hi)),
        _ => None,
    }
}

/// Whether type `t` names one of `ds`: an ADT, a foreign type, a trait object's trait, a
/// type alias, or the trait of a projection.
fn ty_names_def(u: &Unit, t: TyId, ds: &[DefId], depth: u32) -> bool {
    use crate::types::TyKind::*;
    if depth > 24 {
        return false;
    }
    let any = |l: &[TyId]| l.iter().any(|a| ty_names_def(u, *a, ds, depth + 1));
    match u.tcx.tys.kind(t).clone() {
        Adt(d, args, _) => ds.contains(&d) || any(u.tcx.tys.list(args)),
        Foreign(d) => ds.contains(&d),
        Ref(_, x, _) | Ptr(_, x) | Slice(x) | Array(x, _) => ty_names_def(u, x, ds, depth + 1),
        Tuple(v) => any(u.tcx.tys.list(v)),
        FnPtr(ps, r, ..) => any(u.tcx.tys.list(ps)) || ty_names_def(u, r, ds, depth + 1),
        Dyn(tr, args, binds, _, _, _, autos) => ds.contains(&tr) || u.tcx.tys.list(autos).iter().any(|a| ds.contains(a)) || any(u.tcx.tys.list(args))
            || u.tcx.tys.list(binds).iter().any(|&(_, x, _)| ty_names_def(u, x, ds, depth + 1)),
        Alias(_, a) => {
            let d = a.kind.def_id();
            ds.contains(&d) || ds.contains(&u.prog.def(d).parent) || any(u.tcx.tys.list(a.args))
        }
        _ => false,
    }
}

/// The declarations of the current files the edit reaches (`visible` crates; `news`: the edit's
/// new versions) that still name a type or trait the edit removed: a fn's signature or bounds,
/// an impl's header or bounds, a field's, const's or static's type. They kept the removed def.
/// Where a fresh build of the new text does not resolve the name: E0412 / E0405, one error each,
/// at the name. Where every use of the name resolves now (the type moved: `use decode::Pic`), the
/// declaration means the new type, which its collection from before the edit does not know: the
/// second list, `(declaration, removed def)`.
fn decls_naming_removed(u: &Unit, removed: &[DefId], news: &[u32], visible: &[bool]) -> (Vec<String>, Vec<(DefId, DefId)>) {
    let prog = &u.prog;
    let gone: Vec<DefId> = removed.iter().copied().filter(|d| matches!(prog.def(*d).kind, DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TypeAlias | DefKind::Trait)).collect();
    if gone.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let names: Vec<Vec<u8>> = gone.iter().map(|d| def_name(prog, *d).into_bytes()).filter(|n| !n.is_empty()).collect();
    let syms: Vec<(DefId, crate::program::Sym)> = gone.iter().map(|&d| (d, prog.def(d).name)).collect();
    // the live modules' files (Program::current_source_file_sets) and the edit's new versions
    let mut live = Vec::with_capacity(prog.mods.len());
    let mut files: Vec<u32> = news.to_vec();
    for m in prog.mods.iter() {
        let ok = m.parent_mod.is_none_or(|p| live[p as usize] && (m.lexical_parent.is_some() || prog.mods[p as usize].types.get(&prog.def(m.def).name) == Some(&m.def)));
        live.push(ok);
        if ok { files.push(m.file); }
    }
    files.sort_unstable();
    files.dedup();
    files.retain(|&f| prog.is_current_version(f) && prog.file_crate.get(f as usize).is_some_and(|&k| visible.get(k as usize).copied().unwrap_or(false)));
    let named_by = |d: DefId| -> Option<DefId> {
        let of = |t: TyId| gone.iter().copied().find(|g| ty_names_def(u, t, &[*g], 0));
        let tr = |t: DefId| gone.contains(&t).then_some(t);
        match prog.def(d).kind {
            DefKind::Fn | DefKind::AssocFn => {
                let sig = u.tcx.fn_sig(prog, d)?;
                sig.params.iter().chain([&sig.ret]).find_map(|&t| of(t))
                    .or_else(|| u.tcx.fn_trait_bounds(prog, d).and_then(|b| b.iter().find_map(|(_, t, a)| tr(*t).or_else(|| a.iter().find_map(|&x| of(x))))))
                    .or_else(|| u.tcx.fn_ty_bounds(prog, d).and_then(|b| b.iter().find_map(|(s, t, a)| tr(*t).or_else(|| of(*s)).or_else(|| a.iter().find_map(|&x| of(x))))))
            }
            DefKind::Impl => u.tcx.impl_trait.get(&d).and_then(|t| tr(*t))
                .or_else(|| u.tcx.impl_self.get(&d).and_then(|t| of(*t)))
                .or_else(|| u.tcx.impl_trait_bounds.get(&d).and_then(|b| b.iter().find_map(|(_, t, a)| tr(*t).or_else(|| a.iter().find_map(|&x| of(x)))))),
            DefKind::Struct | DefKind::Enum | DefKind::Union => u.tcx.adts.get(&d).and_then(|a| a.variants.iter().flat_map(|v| v.fields.iter()).find_map(|f| of(f.ty))),
            DefKind::Const | DefKind::Static | DefKind::AssocConst => u.tcx.const_ty(prog, d).and_then(of),
            // its supertraits (transitively: `trait Loud: Words`)
            DefKind::Trait => u.tcx.trait_closure.get(&d).and_then(|v| v.iter().find_map(|t| tr(*t))),
            _ => None,
        }
    };
    // the module a path's segments before the last name (`crate::a::`, `self::`, `a::b::`)
    let module_of_path = |scope: u32, krate: u32, segs: &[&[u8]]| -> Option<u32> {
        let normal = prog.mods[scope as usize].normal;
        let mut m = None;
        for (k, seg) in segs.iter().enumerate() {
            m = Some(match (*seg, m) {
                (b"crate", None) => prog.crates[krate as usize].root_mod,
                (b"self", None) => normal,
                (b"super", None) => prog.mods[normal as usize].parent_mod?,
                (b"super", Some(at)) => prog.mods[at as usize].parent_mod?,
                (name, at) => {
                    let sym = prog.syms.get(std::str::from_utf8(name).ok()?)?;
                    let d = match at {
                        None if k == 0 => prog.lookup_in_scope(scope, sym, true)?,
                        None => return None,
                        Some(at) => prog.lookup_in_module(at, sym, true, &mut Vec::new())?,
                    };
                    if prog.def(d).kind != DefKind::Mod {
                        return None;
                    }
                    prog.def(d).sub
                }
            });
        }
        m
    };
    let mut out = Vec::new();
    let mut moved = Vec::new();
    for f in files {
        let pf = &prog.files[f as usize];
        let src = &pf.src[..pf.src_len as usize];
        if !names.iter().any(|n| src.windows(n.len()).any(|w| w == n.as_slice())) {
            continue;
        }
        // an item a macro made (a derive's impl) only when the file's own text names it nowhere
        // else: the derive's struct field is the use site rustc reports
        let reported = out.len();
        let n = pf.ast.items.len();
        let parent: std::collections::HashMap<usize, usize> = (0..n).flat_map(|p| child_items(&pf.ast, &pf.ast.items[p].kind).into_iter().map(move |c| (c.0 as usize, p))).collect();
        let own = |i: &usize| pf.ast.items[*i].lo < pf.src_len;
        for i in (0..n).filter(own).chain((0..n).filter(|i| !own(i))) {
            let it = &pf.ast.items[i];
            if !own(&i) && out.len() > reported {
                break;
            }
            let Some(&d) = prog.item_defs.get(&(f, i as u32)) else { continue };
            if removed.contains(&d) {
                continue;
            }
            // the declaration's own text: not a fn's body, nor the items it holds
            let mut holes: Vec<(u32, u32)> = fn_body_span(pf, &it.kind).into_iter().collect();
            holes.extend(child_items(&pf.ast, &it.kind).into_iter().map(|c| { let c = pf.ast.item(c); (c.lo, c.hi) }));
            let in_hole = |at: u32| holes.iter().any(|&(a, b)| a <= at && at < b);
            // the declaration's tokens: (position, text, whether a `::` precedes it)
            let mut decl: Vec<(u32, &[u8], bool)> = Vec::new();
            let mut prev: &[u8] = b"";
            for t in pf.toks.iter_from(first_tok_at(&pf.toks, it.lo)).take_while(|t| t.hi <= it.hi) {
                let text = pf.src.get(t.lo as usize..t.hi as usize).unwrap_or(b"");
                if !in_hole(t.lo) {
                    decl.push((t.lo, text, prev == b"::"));
                }
                prev = text;
            }
            let at_name = |name: &[u8]| decl.iter().find(|(_, x, _)| *x == name).map(|(at, _, _)| *at);
            if !names.iter().any(|n| at_name(n).is_some()) {
                continue;
            }
            let report = |g: DefId, at: u32| {
                let name = def_name(prog, g);
                let (code, what) = if prog.def(g).kind == DefKind::Trait { ("E0405", "trait") } else { ("E0412", "type") };
                format!("{}: {}: cannot find {} `{}` in this scope (this edit removes it)", u.pos_str(f, pf.call_site(at)), code, what, name)
            };
            let mut params: Vec<&[u8]> = Vec::new();
            let mut k = Some(i);
            while let Some(x) = k {
                if let Some(g) = pf.ast.kind_generics(&pf.ast.items[x].kind) {
                    params.extend(pf.ast.list(g.params).iter().map(|p| pf.src.get(p.name.lo as usize..p.name.hi as usize).unwrap_or(b"")));
                }
                k = parent.get(&x).copied();
            }
            let (scope, krate) = (prog.def(d).scope, prog.def(d).krate);
            // a use of it: the name alone, resolving to nothing in the item's scope now, or the
            // last name of a module path whose module no longer has it (`a::Id`)
            let gone_use = |k: usize, sym: crate::program::Sym| -> bool {
                if !decl[k].2 {
                    return !params.contains(&decl[k].1) && prog.lookup_in_scope(scope, sym, true).is_none();
                }
                let mut segs: Vec<&[u8]> = Vec::new();
                let mut j = k;
                while j >= 2 && decl[j].2 && decl[j - 1].1 == b"::" {
                    segs.push(decl[j - 2].1);
                    j -= 2;
                }
                segs.reverse();
                let global = segs.first().is_some_and(|s| !s.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_'));
                !global && module_of_path(scope, krate, &segs).is_some_and(|m| prog.lookup_in_module(m, sym, true, &mut Vec::new()).is_none())
            };
            if let Some(g) = named_by(d) {
                // where a use of the name resolves to something now (the type moved), the
                // declaration is not an error but means that
                let name = def_name(prog, g);
                let uses: Vec<usize> = (0..decl.len()).filter(|&k| decl[k].1 == name.as_bytes()).collect();
                match uses.iter().find(|&&k| gone_use(k, prog.def(g).name)) {
                    Some(&k) => out.push(report(g, decl[k].0)),
                    None if !uses.is_empty() => moved.push((d, g)),
                    None => out.push(report(g, it.lo)),
                }
                continue;
            }
            // a type alias leaves no trace in the types it stood for: a name of a removed one
            // that is not a generic parameter here and no longer resolves in the item's scope.
            // So does an alias's own type (its type is lowered at its uses): every removed name
            // in it is checked so (its names are type paths)
            if !own(&i) {
                continue;
            }
            let alias_item = prog.def(d).kind == DefKind::TypeAlias;
            if let Some((g, at)) = syms.iter().filter(|(g, _)| alias_item || prog.def(*g).kind == DefKind::TypeAlias).find_map(|&(g, sym)| {
                let name = prog.syms.str(sym).as_bytes();
                (0..decl.len()).find(|&k| decl[k].1 == name && gone_use(k, sym)).map(|k| (g, decl[k].0))
            }) {
                out.push(report(g, at));
            }
        }
    }
    (out, moved)
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

/// An expanded trait impl belongs to the ADT that requested it. Follow expansion edges
/// only as far as that item: following all the way to authored text loses a deriving ADT
/// that was itself made by a macro. Neither the expansion's offsets nor its header's
/// tokenization are part of its identity.
fn derive_owners(pf: &crate::parser::ParsedFile) -> Vec<Option<(usize, String)>> {
    use crate::ast::{ItemKind, Ty};
    let ast = &pf.ast;
    let mut owners = Vec::new();
    for (i, it) in ast.items.iter().enumerate() {
        if !matches!(it.kind, ItemKind::Struct(..) | ItemKind::Enum(..) | ItemKind::Union(..)) { continue; }
        owners.push((it.lo, it.hi, i));
    }
    owners.sort_unstable();
    let mut regions: Vec<_> = pf.exp_map.iter().collect();
    for part in pf.parts() { regions.extend(part.exp_map.iter()); }
    regions.sort_unstable();
    ast.items.iter().map(|it| {
        let ItemKind::Impl(row) = it.kind else { return None };
        let imp = ast.row(row);
        let trait_ = imp.trait_?;
        let Ty::Path(self_ty) = ast.ty(imp.self_ty) else { return None };
        let name = pf.text(ast.segs(self_ty.segs).last()?.name);
        let mut pos = it.lo;
        loop {
            let r = regions.partition_point(|r| r.0 <= pos).checked_sub(1)?;
            let (lo, hi, call) = regions[r];
            if pos < lo || pos >= hi || call >= pos { return None; }
            pos = call;
            let next = owners.partition_point(|o| o.0 <= pos);
            let owner = next.checked_sub(1).and_then(|k| {
                let (_, end, owner) = owners[k];
                (pos < end && pf.text(ast.items[owner].name) == name).then_some(owner)
            }).or_else(|| {
                // Hosted derives point at the first `#`, before Item::lo. Read that
                // attribute prefix from bytes: Attr's token indices may have no stored
                // tokens in this representation. Only whole outer attributes may intervene.
                let &(start, _, owner) = owners.get(next)?;
                if pf.text(ast.items[owner].name) != name { return None; }
                let mut tokens = Vec::new();
                crate::lexer::lex(pf.bytes(pos, start), pf.edition, &mut tokens).ok()?;
                use crate::lexer::T;
                let mut i = 0;
                while tokens.get(i).is_some_and(|t| matches!(t.kind, T::Pound | T::DocComment)) {
                    if tokens[i].kind == T::DocComment { i += 1; continue; }
                    if tokens.get(i + 1)?.kind != T::OpenBracket { return None; }
                    i += 2;
                    let mut depth = 1;
                    while depth != 0 {
                        match tokens.get(i)?.kind {
                            T::OpenBracket => depth += 1,
                            T::CloseBracket => depth -= 1,
                            T::Eof => return None,
                            _ => {}
                        }
                        i += 1;
                    }
                }
                (i != 0 && tokens.get(i)?.kind == T::Eof).then_some(owner)
            });
            if let Some(owner) = owner {
                let path = ast.segs(trait_.segs).iter().map(|s| pf.text(s.name)).collect::<Vec<_>>().join("::");
                return Some((owner, path));
            }
        }
    }).collect()
}

/// An item's identity across versions of a file: its container path, kind and name (impls,
/// imports and item macros by their header tokens; generated `__relax_*` names with their
/// position digits dropped), with an occurrence number for repeats. Items in fn bodies are
/// keyed by their fn. Derive impls are keyed by their deriving item and trait instead of
/// their rendered header; their methods inherit that identity.
fn item_keys(pf: &crate::parser::ParsedFile) -> Vec<String> {
    use crate::ast::ItemKind;
    let ast = &pf.ast;
    let mut key: Vec<Option<String>> = vec![None; ast.items.len()];
    let derives = derive_owners(pf);
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
    fn walk(pf: &crate::parser::ParsedFile, items: &[crate::ast::ItemId], prefix: &str, derives: &[Option<(usize, String)>], key: &mut Vec<Option<String>>) {
        let ast = &pf.ast;
        for &iid in items {
            // The position-ordered pass below keys the deriving item before its output,
            // including block-local ADTs that the root item walk does not visit.
            if derives[iid.0 as usize].is_some() { continue; }
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
            walk(pf, &kids, &k, derives, key);
        }
    }
    walk(pf, &ast.root_items, "", &derives, &mut key);
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
            if let Some((owner, trait_)) = &derives[i] {
                let k = format!("{}/derive impl {}", key[*owner].as_ref().expect("deriving item precedes its expansion"), trait_);
                key[i] = Some(k.clone());
                walk(pf, &child_items(ast, &it.kind), &k, &derives, &mut key);
            } else {
                let owner = holders.last().and_then(|&top| key[top].clone()).unwrap_or_default();
                key[i] = Some(format!("{}{{}}/{} {}", owner, kind_name(&it.kind), name_of(pf, it)));
            }
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
    let positions = root_positions(old);
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
    if stream_timing() {
        eprintln!("makepad-run stream: compose_partial {} bytes {:.3} ms", new_src.len(), ms(started));
    }
    Some((text, kept))
}

/// A whole save's transaction applied again with items held back (hold_back): restart lines and
/// refusals go back to the hold-back that asked, which may hold more.
const HOLD_ROUND: u8 = 3;
/// How many times a save is applied again with more items held back before it restarts whole.
const HOLD_ROUNDS: usize = 4;
/// An outcome line of an item held back: the restart it waits for gives it its new text.
pub(crate) const HELD: &str = "held for a restart: ";
/// An outcome line of an item held back because the rest of the edit reaches a held one: its
/// error against the held item's running form.
const HELD_REACHES: &str = "held, as it needs a held item: ";

/// The byte offset in `src` (file `path`'s text) of a diagnostic line's `path:line:col:` position.
fn line_at(line: &str, path: &str, src: &[u8]) -> Option<usize> {
    let rest = line.strip_prefix(path)?.strip_prefix(':')?;
    let mut parts = rest.split(':');
    let l: usize = parts.next()?.parse().ok()?;
    let c: usize = parts.next()?.parse().ok()?;
    let start = if l <= 1 { 0 } else { src.iter().enumerate().filter(|(_, b)| **b == b'\n').nth(l - 2)?.0 + 1 };
    Some(start + c.saturating_sub(1))
}

/// "path:line:col: " of def `d`'s item in the edit's new texts (`versions`), "" if they lack it: a
/// restart line that names where its item is lets hold_back keep that item alone.
fn new_pos(u: &Unit, versions: &[(crate::program::FileVersion, Plan)], d: DefId) -> String {
    let file = u.prog.def(d).file;
    for (v, plan) in versions.iter().filter(|(v, _)| v.old == file) {
        if let Some(&(_, j)) = plan.pairs.iter().find(|&&(i, _)| u.prog.item_defs.get(&(v.old, i)) == Some(&d)) {
            let (l, c) = v.pf.line_col(v.pf.call_site(v.pf.ast.items[j as usize].lo));
            return format!("{}:{}:{}: ", v.path, l, c);
        }
    }
    String::new()
}

/// File `file`'s new text `src` (`path`) with the authored items at the positions `at` kept as
/// they run: each top-level span holding one is blanked (its newlines kept, so every other
/// position of the text stays where the file has it: diagnostics and line tables name the file
/// as saved), and the running version's spans of the same items (by identity: item_keys) go at
/// the end of the text. An item the running version lacks goes. With the number of spans kept;
/// None if the text made does not parse.
fn keep_running(prog: &Program, file: u32, path: &str, src: &[u8], at: &[usize]) -> Option<(Vec<u8>, usize)> {
    let old = &prog.files[file as usize];
    let new = prog.parse_version(file, path, src.to_vec()).ok()?;
    let (new_keys, old_keys) = (item_keys(&new.pf), item_keys(old));
    let (new_roots, old_roots) = (root_positions(&new.pf), root_positions(old));
    let old_spans = rust_spans(&old.src[..old.src_len as usize], old.edition);
    let mut used = vec![false; old_spans.len()];
    let mut out = src.to_vec();
    let mut tail = Vec::new();
    let mut kept = 0;
    for (lo, hi) in rust_spans(src, old.edition) {
        if !at.iter().any(|&p| lo <= p && p < hi) { continue; }
        kept += 1;
        let keys: std::collections::HashSet<&str> = new_roots.iter().filter(|r| lo <= r.0 && r.0 < hi).map(|r| new_keys[r.1].as_str()).collect();
        for b in &mut out[lo..hi] {
            if *b != b'\n' && *b != b'\r' { *b = b' '; }
        }
        for (k, &(olo, ohi)) in old_spans.iter().enumerate() {
            if !used[k] && old_roots.iter().any(|r| olo <= r.0 && r.0 < ohi && keys.contains(old_keys[r.1].as_str())) {
                used[k] = true;
                tail.push(b'\n');
                tail.extend_from_slice(&old.src[olo..ohi]);
            }
        }
    }
    if kept == 0 { return None; }
    out.push(b'\n');
    out.extend_from_slice(&tail);
    prog.parse_version(file, path, out.clone()).ok()?;
    Some((out, kept))
}

/// The root items of `pf` by the authored position of each (an expanded root at its macro call),
/// sorted: (position, item index).
fn root_positions(pf: &crate::parser::ParsedFile) -> Vec<(usize, usize)> {
    let mut positions: Vec<_> = pf.ast.root_items.iter().map(|i| {
        let mut pos = pf.ast.item(*i).lo;
        // Expanded roots belong to their authored macro invocation.
        for _ in 0..32 {
            let Some((_, _, call)) = pf.exp_map.iter_rev().find(|&(a, b, _)| a <= pos && pos < b) else { break };
            if call == pos { break; }
            pos = call;
        }
        (pos as usize, i.0 as usize)
    }).collect();
    positions.sort_unstable();
    positions
}

/// Restart-only items hold back only themselves and what needs them. An edit with one took the
/// restart rung whole, and so did every later one while it waited (an app, 11-07: a type alias, a
/// layout a static holds, two consts in one landing, and nine hours of landings and saves after
/// it waited with them). The items the restart lines name (`lines`: each at `path:line:col` of the
/// edit's text) keep their running text, and the edit applies again without them; an item the
/// rest of it cannot type-check without a held one's new form is held too (its error names it),
/// and so are the restart-only items a round finds, round by round. The held items' reasons wait
/// for the restart as before ([`HELD`] lines: the watcher offers it). None: a line names no item
/// of the edit's text (its reason is the whole program's), or the rounds ran out: the edit
/// restarts whole.
unsafe fn hold_back(up: *mut Unit, edits: &[Edit], lines: &[String], t0: Instant, retries: usize, base: &IncludeBase) -> Option<Outcome> {
    const SUMMARY: &str = "type checking failed due to previous errors";
    let mut srcs: Vec<Edit> = edits.iter().map(|e| Edit { file: e.file, path: e.path.clone(), src: e.src.clone() }).collect();
    // the held items' running texts go past each file's own end
    let ends: Vec<usize> = srcs.iter().map(|e| e.src.len()).collect();
    let (mut reasons, mut reached) = (lines.to_vec(), Vec::new());
    let mut named: Vec<String> = lines.to_vec();
    let mut held = 0;
    for round in 0..HOLD_ROUNDS.min(retries) {
        let mut at: Vec<Vec<usize>> = vec![Vec::new(); srcs.len()];
        for l in named.iter().filter(|l| !l.ends_with(SUMMARY)) {
            let (k, p) = srcs.iter().enumerate().find_map(|(k, e)| line_at(l, &e.path, &e.src).map(|p| (k, p)))?;
            if p >= ends[k] { return None; }
            at[k].push(p);
        }
        for (k, e) in srcs.iter_mut().enumerate() {
            if at[k].is_empty() { continue; }
            let (next, n) = keep_running(&(*up).prog, e.file, &e.path, &e.src, &at[k])?;
            e.src = next;
            held += n;
        }
        let again: Vec<Edit> = srcs.iter().map(|e| Edit { file: e.file, path: e.path.clone(), src: e.src.clone() }).collect();
        let mut r = apply_locked(up, again, Vec::new(), t0, HOLD_ROUND, retries - 1 - round, false, base);
        match r.rung {
            Rung::Patch => {
                r.kept += held;
                r.lines.retain(|l| !l.starts_with("incomplete:"));
                r.lines.push(format!("incomplete: {held} item(s) kept at their running version"));
                r.lines.extend(reasons.iter().map(|l| format!("{HELD}{l}")));
                r.lines.extend(reached.iter().map(|l| format!("{HELD_REACHES}{l}")));
                return Some(r);
            }
            Rung::Restart => reasons.extend(r.lines.iter().cloned()),
            Rung::Refuse if internal_failure(&r.lines) => return None,
            Rung::Refuse => reached.extend(r.lines.iter().filter(|l| !l.ends_with(SUMMARY)).cloned()),
        }
        named = r.lines;
    }
    None
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
/// item spans, tokens and expansion origins. The same text can also be held as an image or
/// expansion text, so a representation change must not reuse keys from a different view.
fn item_keys_memo(pf: &crate::parser::ParsedFile) -> std::rc::Rc<Vec<String>> {
    use std::hash::{Hash, Hasher};
    let mut h = crate::fxhash::FxHasher::default();
    pf.src.hash(&mut h);
    pf.toks.words().hash(&mut h);
    for region in pf.exp_map.iter() { region.hash(&mut h); }
    for part in pf.parts() { for region in part.exp_map.iter() { region.hash(&mut h); } }
    pf.ast.items.len().hash(&mut h);
    for it in &pf.ast.items { (it.lo, it.hi).hash(&mut h); }
    let id = h.finish();
    thread_local! {
        static LAST: std::cell::RefCell<Vec<(u64, std::rc::Rc<Vec<String>>)>> = Default::default();
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

/// Adding or removing an expanded native derive only adds or removes an impl (a removed impl's
/// callers are checked with the edit: apply_locked). Changing marker/glue derives still takes
/// the existing conservative path. Accept only plain derive lists here; cfg_attr and qualified
/// paths need their own semantic attribute comparison.
fn changed_expanded_derives(old: &crate::parser::ParsedFile, a: &crate::ast::Item, new: &crate::parser::ParsedFile, b: &crate::ast::Item) -> bool {
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
    ak == bk && (be.iter().any(|n| !ae.contains(n)) || ae.iter().any(|n| !be.contains(n)))
}

/// The files the program's own code included (include!/include_str!/include_bytes!) so far. A
/// Rust file include! reads is no source of the program's own: its text is the includer's
/// expansion (an app's main file includes the files of its parts), and a save of it is an edit
/// of the includer. They were left out here, so a save of one was "not one of its sources".
fn included_files(std_dir: &str) -> Vec<String> {
    crate::sip::late_files().into_iter().filter(|p| own_source(p, std_dir)).collect()
}

/// The files file `pf` (file `path`) names in an include!/include_str!/include_bytes! call
/// anywhere in its text and its expansions: an item-level `include!("x.rs");` is in no item
/// (`item_includes` looks at fns, consts and statics), the items it expands to are the file's.
fn file_includes(pf: &crate::parser::ParsedFile, path: &str) -> Vec<String> {
    if !pf.src.windows(8).any(|w| w == b"include!" || w == b"include_") { return Vec::new(); }
    crate::builtins::include_literals(&toks(pf, 0, pf.src.len() as u32, &[])).iter().map(|rel| crate::builtins::include_path(Some(path), rel)).collect()
}

/// The files fn, const or static item `it` includes with include!/include_str!/include_bytes! (`pf` is
/// file `path`'s text). Its whole text: what `classify` compares as the item, and the expansions
/// of the macro calls in it (a macro's `include_str!(concat!($key, ".txt"))`: Fab's demo plates;
/// a save of such a file said "patched" and the program kept the old text).
/// `expanded`: `expansions_include(pf)`.
fn item_includes(pf: &crate::parser::ParsedFile, path: &str, it: &crate::ast::Item, expanded: bool) -> Vec<String> {
    use crate::ast::ItemKind;
    if !matches!(it.kind, ItemKind::Fn(..) | ItemKind::Const(..) | ItemKind::Static(..)) { return Vec::new(); }
    // (most code includes nothing: one look for the macro name first)
    if !expanded && !pf.bytes(it.lo, it.hi).windows(8).any(|w| w == b"include_" || w == b"include!") { return Vec::new(); }
    let mut texts = toks(pf, it.lo, it.hi, &[]);
    if expanded {
        // the expansions whose call is in the item, or in one of these
        let mut regions: Vec<(u32, u32, u32)> = pf.exp_map.iter().collect();
        for part in pf.parts() { regions.extend(part.exp_map.iter()); }
        let mut inside: Vec<(u32, u32)> = vec![(it.lo, it.hi)];
        let mut grew = true;
        while grew {
            grew = false;
            for &(a, b, call) in &regions {
                if a < b && !inside.contains(&(a, b)) && inside.iter().any(|&(lo, hi)| lo <= call && call < hi) {
                    inside.push((a, b));
                    grew = true;
                }
            }
        }
        for &(a, b) in &inside[1..] {
            texts.extend(toks(pf, a, b, &[]));
        }
    }
    crate::builtins::include_literals(&texts).iter().map(|rel| crate::builtins::include_path(Some(path), rel)).collect()
}

/// The content hashes each included file was read with by the running code (one per crate that
/// read it), as a live transaction found them before its parse read the files again.
pub(crate) type IncludeBase = std::collections::HashMap<String, Vec<u128>>;

fn include_base(records: &[(String, String, u128)]) -> IncludeBase {
    let mut base = IncludeBase::new();
    for (_, path, hash) in records { base.entry(path.clone()).or_default().push(*hash); }
    base
}

/// Whether the expansion text of file `pf` names include!/include_str!/include_bytes! (`item_includes`
/// then looks into the expansions of each item's macro calls).
fn expansions_include(pf: &crate::parser::ParsedFile) -> bool {
    let src: &[u8] = &pf.src[..];
    !pf.exp_map.is_empty() && src.get(pf.src_len as usize..).is_some_and(|t| t.windows(8).any(|w| w == b"include_" || w == b"include!"))
}

/// Whether item `it`'s code includes a file whose content is not what the running code read
/// (`base`; a file it never read is no part of running code).
fn includes_moved(pf: &crate::parser::ParsedFile, path: &str, it: &crate::ast::Item, base: &IncludeBase, expanded: bool) -> bool {
    if base.is_empty() { return false; }
    item_includes(pf, path, it, expanded).iter().any(|p| base.get(p).is_some_and(|seen| {
        let now = crate::sip::late_hash(crate::vfs::read(p).ok().as_deref());
        seen.iter().any(|h| *h != now)
    }))
}

/// A live session's edits for files the program's code includes (include!/include_str!/
/// include_bytes!) that were saved (`changed`): each own source file whose code includes one, with
/// its running text (`classify` finds the items whose included content moved, and those an
/// include! expands to read the file anew). `skip`: the source files the batch edits already.
pub(crate) fn include_edits(prog: &crate::program::Program, changed: &[String], skip: &[String]) -> Vec<Edit> {
    let mut out = Vec::new();
    for (path, file) in prog.current_source_files(|p| !skip.iter().any(|s| s == p)) {
        if !own_source_file(prog, file) { continue; }
        let pf = &prog.files[file as usize];
        let src = &pf.src[..pf.src_len as usize];
        let expanded = expansions_include(pf);
        if file_includes(pf, &path).iter().any(|p| changed.contains(p))
            || (expanded || src.windows(8).any(|w| w == b"include_" || w == b"include!")) && pf.ast.items.iter().any(|it| item_includes(pf, &path, it, expanded).iter().any(|p| changed.contains(p))) {
            out.push(Edit { file, path, src: src.to_vec() });
        }
    }
    out
}

/// The saved included files (`changed`) no own source file's code names (`item_includes`): a path
/// made by other tokens (`env!`, a macro of another file). Which code read them is not known.
pub(crate) fn include_unmatched(prog: &crate::program::Program, changed: &[String]) -> Vec<String> {
    let mut left: Vec<String> = changed.to_vec();
    for (path, file) in prog.current_source_files(|_| true) {
        if left.is_empty() { break; }
        if !own_source_file(prog, file) { continue; }
        let pf = &prog.files[file as usize];
        let named = file_includes(pf, &path);
        left.retain(|p| !named.contains(p));
        let expanded = expansions_include(pf);
        if !expanded && !pf.src[..pf.src_len as usize].windows(8).any(|w| w == b"include_" || w == b"include!") { continue; }
        for it in pf.ast.items.iter() {
            let named = item_includes(pf, &path, it, expanded);
            left.retain(|p| !named.contains(p));
        }
    }
    left
}

/// Where item `it`'s text has its expression, which starts at `e`: an expression a macro made
/// (`const T: &str = include_str!("t.txt");`) lies in the expansion text past the file's, and its
/// call is where the item's text has it. The item's head (its type) is the text up to there: up to
/// the expansion it took in the file's text after the item and every expansion before this one,
/// a body's `format!` above it among them, so an edit of the included file alone said "the type
/// of const `T` changed" (an app's two `include_str!` consts, 11-07).
fn expr_at(pf: &crate::parser::ParsedFile, it: &crate::ast::Item, e: u32) -> u32 {
    if (it.lo..=it.hi).contains(&e) { return e; }
    let call = pf.call_site(e);
    if (it.lo..=it.hi).contains(&call) { call } else { e }
}

/// Classifies the items of one file version against the running one.
/// Ok: the item pairing (old item -> new item) and the changes that apply live.
/// Err: the restart reasons (one line each).
pub(crate) fn classify(prog: &crate::program::Program, old_file: u32, new: &crate::parser::ParsedFile, path: &str, base: &IncludeBase) -> Result<Plan, Vec<String>> {
    classify_moving(prog, old_file, new, path, base, &[])
}

/// Whether `d` is a fn whose signature returns an opaque type (an async fn, or `-> impl Trait`):
/// the type its callers' code is built for (the coroutine or closure of its body, the
/// iterator its body builds) is made by its body.
fn defines_opaque(prog: &crate::program::Program, d: DefId) -> bool {
    prog.opaque_types_defined_by.get(&d).is_some_and(|defs| !defs.is_empty())
}

/// `classify`, with the fns `moving` (of the running version) handled as a signature change: the
/// old fn goes, a new one comes from the new text, and its callers are rebuilt against it.
fn classify_moving(prog: &crate::program::Program, old_file: u32, new: &crate::parser::ParsedFile, path: &str, base: &IncludeBase, moving: &[DefId]) -> Result<Plan, Vec<String>> {
    let _timing = crate::parse_work::time(6);
    use crate::ast::ItemKind;
    let old = &prog.files[old_file as usize];
    let (oa, na) = (&old.ast, &new.ast);
    let pos = |pf: &crate::parser::ParsedFile, at: u32| -> String {
        let (l, c) = pf.line_col(at);
        format!("{}:{}:{}", path, l, c)
    };
    let body_span = fn_body_span;
    let (mut pairs, mut removed, mut added) = pair_items(old, new);
    let old_expanded = !base.is_empty() && expansions_include(old);
    trace_sub("classify_pair");
    // a fn whose signature changed (M2/M4): the old fn goes and a new one with the new signature
    // comes (its own slots: frames of the old code in flight keep calling the old one), and every
    // fn naming it is type-checked and rebuilt against the new signature (apply: set_names).
    // A fn returning an opaque type whose body changed is one too: its callers' code is built
    // for the type its body makes (an async fn's coroutine: its poll code and state layout), and
    // the new body makes another one. So is a fn of `moving` (apply_locked: a rebuilt fn of that
    // kind whose own text is unchanged).
    {
        let skip_name = |pf: &crate::parser::ParsedFile, it: &crate::ast::Item| -> bool { pf.src[it.name.lo as usize..it.name.hi as usize].starts_with(b"__relax_") };
        let attr_toks = |pf: &crate::parser::ParsedFile, it: &crate::ast::Item| -> Vec<Vec<u8>> {
            pf.ast.list(it.attrs).iter().flat_map(|t| toks(pf, pf.toks.get(t.toks.lo as usize).map_or(0, |x| x.lo), pf.toks.get(t.toks.hi.saturating_sub(1) as usize).map_or(0, |x| x.hi), &[]).into_iter().map(|x| x.to_vec())).collect()
        };
        // a trait's fn: its trait item (the vtable's entry, what impls implement) is replaced when
        // its signature changes or it gains or loses its default body
        let (old_parent, old_reached) = item_parents(old);
        let new_reached = item_parents(new).1;
        let in_trait = |i: u32| old_parent[i as usize].is_some_and(|p| matches!(oa.items[p as usize].kind, ItemKind::Trait(..)));
        let mut k = 0;
        while k < pairs.len() {
            let (i, j) = pairs[k];
            let (a, b) = (&oa.items[i as usize], &na.items[j as usize]);
            let moved = match (&a.kind, &b.kind) {
                // an edited `use`: its old imports go, the new ones resolve (M13)
                (ItemKind::Use { .. }, ItemKind::Use { .. }) => !same_toks(old, a.lo, a.hi, &[], new, b.lo, b.hi, &[]),
                // an edited type alias in a body (`type Found = ..;` in a fn): part of the edit of
                // the body holding it. It goes and a new one comes with that body
                // (Program::collect_new_body_blocks), and the body is checked against its type.
                (ItemKind::TypeAlias(_), ItemKind::TypeAlias(_)) => body_local_alias(old, &old_reached, i) && body_local_alias(new, &new_reached, j)
                    && !(attr_toks(old, a) == attr_toks(new, b) && same_toks(old, a.lo, a.hi, &[], new, b.lo, b.hi, &[])),
                (ItemKind::Fn(sa, _, _), ItemKind::Fn(sb, _, _)) if !skip_name(new, b) && attr_toks(old, a) == attr_toks(new, b) => {
                    let def = prog.item_defs.get(&(old_file, i)).copied();
                    let opaque = (oa.fn_sig(*sa).asyncness && na.fn_sig(*sb).asyncness) || def.is_some_and(|d| defines_opaque(prog, d));
                    match (body_span(old, &a.kind), body_span(new, &b.kind)) {
                        (Some((alo, ahi)), Some((blo, bhi))) => !same_toks(old, a.lo, alo, &[], new, b.lo, blo, &[])
                            || (opaque && !same_toks(old, alo, ahi, &[], new, blo, bhi, &[]))
                            || def.is_some_and(|d| moving.contains(&d)),
                        (None, None) if in_trait(i) => !same_toks(old, a.lo, a.hi, &[], new, b.lo, b.hi, &[]),
                        (Some(_), None) | (None, Some(_)) if in_trait(i) => true,
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
    // A new module (inline or in a file) is collected and checked with every
    // other edit in the transaction, including callers saved before it existed.
    let addable = |pf: &crate::parser::ParsedFile, k: &ItemKind| matches!(k, ItemKind::Fn(sig, _, _) if pf.ast.fn_sig(*sig).lazy_body.is_some()) || matches!(k, ItemKind::Fn(_, Some(_), _) | ItemKind::Const(..) | ItemKind::Static(..) | ItemKind::Struct(..) | ItemKind::Enum(..) | ItemKind::Union(..) | ItemKind::TypeAlias { .. } | ItemKind::Impl { .. } | ItemKind::Trait(..) | ItemKind::Use { .. } | ItemKind::Mod(_));
    let inherent_impl = |pf: &crate::parser::ParsedFile, i: u32| matches!(&pf.ast.items[i as usize].kind, ItemKind::Impl(r) if pf.ast.row(*r).trait_.is_none());
    // an item in a body (`use` in a fn or closure body, a nested fn, a const): part of the edit
    // of the item whose body holds it. In a paired item's body: its body is checked again, and
    // its blocks' items get their scopes when the version is committed
    // (Program::collect_new_body_blocks). In an added one's (also a fn replaced for a new
    // signature): collected with it (Program::add_items); in a removed one's: gone with it.
    // An item a macro call in the body made is at the call (its text is in the expansion).
    let holds = |pf: &crate::parser::ParsedFile, k: u32, at: u32| -> bool {
        let it = &pf.ast.items[k as usize];
        if !matches!(it.kind, ItemKind::Fn(..) | ItemKind::Const(..) | ItemKind::Static(..)) {
            return false;
        }
        let mut pos = at;
        for _ in 0..32 {
            if it.lo < pos && pos < it.hi {
                return true;
            }
            match pf.exp_map.iter_rev().find(|&(a, b, _)| a <= pos && pos < b) {
                Some((_, _, call)) if call != pos => pos = call,
                _ => break,
            }
        }
        false
    };
    let in_paired_body = |pf: &crate::parser::ParsedFile, side_new: bool, at: u32| -> bool {
        pairs.iter().any(|&(i, j)| holds(pf, if side_new { j } else { i }, at))
    };
    for &j in &added {
        let it = &na.items[j as usize];
        let name = String::from_utf8_lossy(&new.src[it.name.lo as usize..it.name.hi as usize]).into_owned();
        if !nreached[j as usize] && (in_paired_body(new, true, it.lo) || added.iter().any(|&k| k != j && holds(new, k, it.lo))) {
            continue;
        }
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
                    // a fn of a trait impl (an override of a provided method, or one replaced for
                    // a new signature): apply_locked resolves the instances the trait reaches anew
                    (ItemKind::Impl { .. }, Some(&op_)) if !inherent_impl(new, p) && addable(new, &it.kind) && matches!(it.kind, ItemKind::Fn(..)) => prog.item_defs.get(&(old_file, op_)).map(|d| Err(*d)),
                    // a trait's fn (required or provided, or one replaced for a new signature):
                    // after its other items; apply_locked checks the trait's impls and vtables
                    (ItemKind::Trait(..), Some(&op_)) if matches!(it.kind, ItemKind::Fn(..)) => prog.item_defs.get(&(old_file, op_)).map(|d| Err(*d)),
                    _ => None,
                },
                _ => None,
            }
        };
        // (a Drop impl changes needs_drop of its type: apply_locked rebuilds what dropped it)
        match at {
            Some(a) => plan_added.push((j, a)),
            None => {
                let shown = if name.is_empty() { String::from_utf8_lossy(&new.src[it.lo as usize..(it.hi as usize).min(it.lo as usize + 80)]).split_whitespace().collect::<Vec<_>>().join(" ") } else { format!("`{}`", name) };
                let place = if nreached[j as usize] { "" } else { " (inside a fn body)" };
                why.push(format!("{}: {} {} added here{} -- adding this kind of item is not applied live yet: restart the app", pos(new, it.lo), kind_name(&it.kind), shown, place))
            }
        }
    }
    // where a removed item was, in the new text: where the paired item after it in its container
    // starts, else at the end of the container (an impl's `}`), else where the item before it
    // ends; positions in an expansion are its call's
    let gone_at = |i: u32| -> String {
        let span = |pf: &crate::parser::ParsedFile, it: &crate::ast::Item| (pf.call_site(it.lo), pf.call_site(it.hi.saturating_sub(1)));
        let (lo, hi) = span(old, &oa.items[i as usize]);
        let olds: Vec<((u32, u32), u32)> = pairs.iter().map(|&(a, b)| (span(old, &oa.items[a as usize]), b)).collect();
        let inside = olds.iter().filter(|((a, z), _)| *a <= lo && hi <= *z && (*a, *z) != (lo, hi)).max_by_key(|((a, _), _)| *a).copied();
        let end = inside.map_or(u32::MAX, |((_, z), _)| z);
        let after = olds.iter().filter(|((a, _), _)| *a > hi && *a < end).min_by_key(|((a, _), _)| *a);
        let before = olds.iter().filter(|((_, z), _)| *z < lo).max_by_key(|((_, z), _)| *z);
        match (after, inside, before) {
            (Some((_, b)), _, _) => pos(new, span(new, &na.items[*b as usize]).0),
            (None, Some((_, b)), _) => pos(new, span(new, &na.items[b as usize]).1),
            (None, None, Some((_, b))) => pos(new, new.call_site(na.items[*b as usize].hi)),
            (None, None, None) => format!("{}:1:1", path),
        }
    };
    for &i in &removed {
        let it = &oa.items[i as usize];
        let name = String::from_utf8_lossy(&old.src[it.name.lo as usize..it.name.hi as usize]).into_owned();
        if !oreached[i as usize] && (in_paired_body(old, false, it.lo) || removed.iter().any(|&k| k != i && holds(old, k, it.lo))) {
            continue;
        }
        // inside a removed module: unbound with it (its impls leave the impl tables themselves)
        if let Some(p) = op[i as usize] {
            if removed.contains(&p) && !matches!(&oa.items[p as usize].kind, ItemKind::Impl { .. }) && !matches!(it.kind, ItemKind::Impl { .. }) {
                continue;
            }
        }
        // A removed impl (inherent or trait, a derive's too) goes with its items: apply_locked
        // takes it out of the impl tables and checks and rebuilds the code that called them. A
        // removed trait goes with its items too: unbound, it is in no scope, and the code naming
        // it or its items is checked and rebuilt (its impls must go with it, or they name it)
        let removable = |k: &ItemKind| addable(old, k) || matches!(k, ItemKind::Trait(..));
        let ok = oreached[i as usize]
            && match op[i as usize] {
                None => removable(&it.kind),
                Some(p) => match &oa.items[p as usize].kind {
                    ItemKind::Mod(Some(_)) => removable(&it.kind),
                    ItemKind::Impl(_) | ItemKind::Trait(..) if removed.contains(&p) => true,
                    ItemKind::Impl(r) if oa.row(*r).trait_.is_none() => matches!(it.kind, ItemKind::Fn(..) | ItemKind::Const(..)),
                    // a trait impl's fn (not one whose callers hold the future or `impl Trait`
                    // its body makes): the trait's provided one, if any, takes its place
                    ItemKind::Impl(_) => matches!(it.kind, ItemKind::Fn(..)) && !prog.item_defs.get(&(old_file, i)).is_some_and(|d| moving.contains(d)),
                    // a trait's fn: the code naming it is checked, and the trait's impls
                    ItemKind::Trait(..) => matches!(it.kind, ItemKind::Fn(..)),
                    _ => false,
                },
            };
        match (ok, prog.item_defs.get(&(old_file, i))) {
            (true, Some(&d)) => plan_removed.push(d),
            (true, None) if matches!(it.kind, ItemKind::Use { .. }) => removed_uses.push(it.lo),
            (true, None) => {} // never collected (cfg'd out)
            (false, Some(d)) if moving.contains(d) => why.push(format!("{}: fn `{}` is rebuilt with this edit and returns a future or `impl Trait` its body makes (its callers' code holds it), so it is replaced like a changed signature -- replacing a fn here (a trait impl's) is not applied live yet: restart the app", pos(old, it.lo), name)),
            _ => why.push(format!("{}: {} `{}` removed here -- removing this kind of item is not applied live yet: restart the app", gone_at(i), kind_name(&it.kind), name)),
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
    // A derive's output is a function of its deriving item: while that item compares equal, its
    // impls and their items are unchanged, whatever form each version holds the expansion in.
    // Each item of a derived impl (the impl too) names its deriving item; only read on a mismatch.
    fn derived_from(pf: &crate::parser::ParsedFile) -> Vec<Option<usize>> {
        let owners = derive_owners(pf);
        let mut of: Vec<Option<usize>> = owners.iter().map(|o| o.as_ref().map(|o| o.0)).collect();
        for (x, o) in owners.iter().enumerate() {
            let Some((owner, _)) = o else { continue };
            for c in child_items(&pf.ast, &pf.ast.items[x].kind) { of[c.0 as usize] = Some(*owner); }
        }
        of
    }
    let derived_cell = std::cell::OnceCell::new();
    let derived = || derived_cell.get_or_init(|| (derived_from(old), derived_from(new), pairs.iter().copied().collect::<std::collections::HashMap<u32, u32>>()));
    for &(i, j) in &pairs {
        let (a, b) = (&oa.items[i as usize], &na.items[j as usize]);
        let name = String::from_utf8_lossy(&new.src[b.name.lo as usize..b.name.hi as usize]).into_owned();
        // (a module's items with their attributes: an item's attributes are classified with it)
        let holes = |pf: &crate::parser::ParsedFile, k: &ItemKind| -> Vec<(u32, u32)> {
            let module = matches!(k, ItemKind::Mod(_));
            child_items(&pf.ast, k).iter().map(|c| {
                let it = &pf.ast.items[c.0 as usize];
                let lo = if module { pf.ast.list(it.attrs).iter().filter_map(|t| pf.toks.get(t.toks.lo as usize).map(|x| x.lo)).fold(it.lo, u32::min) } else { it.lo };
                (lo, it.hi)
            }).collect()
        };
        // A module's body is its items, each paired, added or removed on its own as a file's
        // are. The text between them is not the module's: an item its cfg leaves out, an item
        // macro's call (its expansion is items), the `#[test]` a test build takes off a fn.
        // Compared as text, an edit of any of them restarted the app; the module's own text is
        // its head (its inner attributes are among its attributes).
        let body = |it: &crate::ast::Item| matches!(it.kind, ItemKind::Mod(Some(_))).then(|| vec![(it.name.hi, it.hi)]);
        let ha = body(a).unwrap_or_else(|| holes(old, &a.kind));
        let hb = body(b).unwrap_or_else(|| holes(new, &b.kind));
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
        // The same text whose code includes a file that changed since the running code read it
        // changed too (include_str!: its body or initializer is classified as edited below).
        if attrs_same && same_toks(old, la, a.hi, &ha, new, lb, b.hi, &hb) && !includes_moved(old, &prog.file_paths[old_file as usize], a, base, old_expanded) {
            continue;
        }
        // derive output is expansion text: an authored item is never one
        if a.lo >= old.src_len && b.lo >= new.src_len {
            let (od, nd, to_new) = derived();
            if let (Some(oo), Some(no)) = (od[i as usize], nd[j as usize]) {
                let (ao, bo) = (&oa.items[oo], &na.items[no]);
                let (holes_o, holes_n) = (holes(old, &ao.kind), holes(new, &bo.kind));
                if to_new.get(&(oo as u32)) == Some(&(no as u32)) && (attr_raw(old, ao) == attr_raw(new, bo) || attr_norm(old, ao) == attr_norm(new, bo))
                    && same_toks(old, ao.lo, ao.hi, &holes_o, new, bo.lo, bo.hi, &holes_n) {
                    continue;
                }
            }
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
                    if reload_trace() {
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
                    Some(d) if matches!(prog.def(d).kind, DefKind::Fn | DefKind::AssocFn) => {
                        if reload_trace() {
                            let (a, b) = (toks_norm(old, la, a.hi, &ha).concat(), toks_norm(new, lb, b.hi, &hb).concat());
                            let k = a.iter().zip(&b).position(|(a, b)| a != b).unwrap_or(a.len().min(b.len()));
                            let snippet = |bytes: &[u8]| {
                                let text = String::from_utf8_lossy(bytes);
                                let mut start = k.saturating_sub(15).min(text.len());
                                while !text.is_char_boundary(start) { start -= 1; }
                                text[start..].chars().take(60).collect::<String>()
                            };
                            eprintln!("reload: body of `{}` changed at byte {} of its text: old `{}` new `{}`", name, k, snippet(&a), snippet(&b));
                        }
                        out.push(Change::Body(d));
                    }
                    _ => why.push(format!("{}: fn `{}` has no definition to patch -- restart the app", pos(new, b.lo), name)),
                }
            }
            (ItemKind::Const(_, ea), ItemKind::Const(_, eb)) if ea.is_some() && eb.is_some() => {
                let (ea, eb) = (expr_at(old, a, oa.expr(ea.get().unwrap()).lo), expr_at(new, b, na.expr(eb.get().unwrap()).lo));
                if !attrs_same || toks_norm(old, la, ea, &[]) != toks_norm(new, lb, eb, &[]) {
                    if reload_trace() {
                        let sh = |v: Vec<Vec<u8>>| v.iter().map(|x| String::from_utf8_lossy(x).into_owned()).collect::<Vec<_>>().join(" ");
                        eprintln!("reload: type of const `{}` (attrs same {}; old {}..{} of {}, new {}..{} of {}):\n  old: {}\n  new: {}", name, attrs_same, la, ea, old.src_len, lb, eb, new.src_len,
                            sh(toks_norm(old, la, ea, &[])).chars().take(600).collect::<String>(), sh(toks_norm(new, lb, eb, &[])).chars().take(600).collect::<String>());
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
                let (ea, eb) = (expr_at(old, a, oa.expr(ea.get().unwrap()).lo), expr_at(new, b, na.expr(eb.get().unwrap()).lo));
                if !attrs_same || toks_norm(old, la, ea, &[]) != toks_norm(new, lb, eb, &[]) {
                    // its declared type as each version spells it (between the name and the `=`)
                    let ty = |pf: &crate::parser::ParsedFile, it: &crate::ast::Item, e: u32| -> String {
                        let t = pf.src.get(it.name.hi as usize..e as usize).map(String::from_utf8_lossy).unwrap_or_default();
                        let t = t.trim().trim_start_matches(':').trim_end().trim_end_matches('=');
                        t.split_whitespace().collect::<Vec<_>>().join(" ")
                    };
                    let (ta, tb) = (ty(old, a, ea), ty(new, b, eb));
                    if ta != tb {
                        why.push(format!("{}: static `{}` changed its type from `{}` to `{}`: its storage holds a value of the old type -- restart the app", pos(new, b.lo), name, ta, tb));
                    } else {
                        why.push(format!("{}: static `{}` changed its declaration (attributes or mutability): its storage was made for the old one -- restart the app", pos(new, b.lo), name));
                    }
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
                if toks_norm(old, la, a.hi, &ha) == toks_norm(new, lb, b.hi, &hb) && changed_expanded_derives(old, a, new, b) => {
                    // Native expanded derives add or remove ordinary impl items, classified above.
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
            // A module whose visibility is all that changed (`mod m;` to `pub mod m;`). A wider
            // one breaks no code that compiled: it takes the new text's (Program::retarget_file),
            // and the code naming it is checked again. A narrower one would need every path
            // through it checked again, `use` items included.
            (ItemKind::Mod(x), ItemKind::Mod(y)) if attrs_same && x.is_some() == y.is_some() && mod_head(old, a).1 == mod_head(new, b).1 => {
                let (va, vb) = (mod_head(old, a).0.join(&b' '), mod_head(new, b).0.join(&b' '));
                let shown = |v: &[u8]| if v.is_empty() { "private".to_string() } else { format!("`{}`", spacing_free(&String::from_utf8_lossy(v))) };
                match def {
                    Some(d) if vis_widens(&a.vis, &b.vis) => out.push(Change::Vis(d)),
                    Some(_) => why.push(format!("{}: mod `{}` is less visible than before ({} to {}): the paths through it are not checked again live yet -- restart the app", pos(new, b.lo), name, shown(&va), shown(&vb))),
                    None => why.push(format!("{}: mod `{}` has no definition -- restart the app", pos(new, b.lo), name)),
                }
            }
            (ItemKind::Mod(x), ItemKind::Mod(y)) => {
                let what = if !attrs_same { "its attributes changed" } else if x.is_some() != y.is_some() { "it moved between a file of its own and its parent's text" } else { "its head changed" };
                why.push(format!("{}: mod `{}` changed: {} -- restart the app", pos(new, b.lo), name, what));
            }
            (_, k) => {
                if reload_trace() {
                    let sh = |v: Vec<Vec<u8>>| v.iter().map(|x| String::from_utf8_lossy(x).into_owned()).collect::<Vec<_>>().join(" ");
                    eprintln!("reload: {} `{}` (attrs same {}; old {}..{} of {}, new {}..{} of {}):\n  old: {}\n  new: {}", kind_name(k), name, attrs_same, la, a.hi, old.src_len, lb, b.hi, new.src_len,
                        sh(toks_norm(old, la, a.hi, &ha)).chars().take(600).collect::<String>(), sh(toks_norm(new, lb, b.hi, &hb)).chars().take(600).collect::<String>());
                }
                why.push(format!("{}: {} `{}` changed: {} -- restart the app", pos(new, b.lo), kind_name(k), name, restart_reason(k)));
            }
        }
    }
    if why.is_empty() {
        Ok(Plan { pairs, changes: out, added: plan_added, removed: plan_removed, removed_uses, attributes_changed })
    } else {
        if reload_trace() {
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

/// A struct item's named fields: each name and the tokens of its type.
fn struct_fields<'a>(pf: &'a crate::parser::ParsedFile, it: &crate::ast::Item) -> Vec<(&'a [u8], Vec<&'a [u8]>)> {
    let crate::ast::ItemKind::Struct(r) = &it.kind else { return Vec::new() };
    let crate::ast::VariantData::Struct(fs) = pf.ast.row(*r).data else { return Vec::new() };
    pf.ast.list(fs).iter().filter_map(|f| f.name.map(|n| (pf.text(n).as_bytes(), toks(pf, f.ty_span.0, f.ty_span.1, &[])))).collect()
}

/// `names` and the macros whose definitions name one of them, or such a macro: what a fn's text
/// reaches them through. A body not yet expanded (its code came from the disk cache) has no
/// expansion text to look at.
fn through_macros(prog: &Program, visible: &[bool], names: &[&[u8]]) -> Vec<Vec<u8>> {
    let mut all: Vec<Vec<u8>> = names.iter().map(|n| n.to_vec()).collect();
    let macros: Vec<DefId> = (0..prog.defs.len()).map(|i| DefId(i as u32)).filter(|d| prog.def(*d).kind == DefKind::MacroRules && visible.get(prog.def(*d).krate as usize).copied().unwrap_or(true)).collect();
    loop {
        let refs: Vec<&[u8]> = all.iter().map(|n| n.as_slice()).collect();
        let more: Vec<Vec<u8>> = macros.iter().filter(|m| item_mentions(prog, **m, &refs)).map(|m| def_name(prog, *m).into_bytes()).filter(|n| !all.contains(n)).collect();
        if more.is_empty() {
            return all;
        }
        all.extend(more);
    }
}

/// Whether `d`'s text can hold a `T { a, ..base }` of the struct `types` names, which takes the
/// other fields without naming them: `..` and a name of it, or `Self` in an impl naming it, or a
/// macro naming it.
fn may_update_struct(prog: &Program, d: DefId, types: &[&[u8]], macros: &[&[u8]]) -> bool {
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    let it = pf.ast.item(def.item);
    let text = toks(pf, it.lo, it.hi, &[]);
    if text.iter().any(|t| macros.contains(t) && !types.contains(t)) {
        return true;
    }
    if !text.iter().any(|t| *t == b"..") {
        return false;
    }
    if text.iter().any(|t| types.contains(t)) {
        return true;
    }
    def.parent != crate::program::NO_DEF && prog.def(def.parent).kind == DefKind::Impl && text.iter().any(|t| *t == b"Self") && {
        let (ipf, imp) = (&prog.files[prog.def(def.parent).file as usize], &prog.files[prog.def(def.parent).file as usize].ast.item(prog.def(def.parent).item));
        let hi = ipf.src.get(imp.lo as usize..imp.hi as usize).and_then(|t| t.iter().position(|b| *b == b'{')).map_or(imp.hi, |at| imp.lo + at as u32);
        toks(ipf, imp.lo, hi, &[]).iter().any(|t| types.contains(t))
    }
}

/// Constant-evaluated code using a field of `d` that leaves its slot: its value holds the old
/// layout or what the old type gave it, and it is not evaluated again. An initializer whose own
/// text changed is another edit's answer; a const fn counts when a constant-evaluated context names
/// it (at run time it is rebuilt like any fn). The restart line.
fn const_code_using(u: &Unit, d: DefId, gone: &[u32], names: &[&[u8]], visible: &[bool], changes: &[Change]) -> Option<String> {
    use crate::def_collector::NodeId;
    let prog = &u.prog;
    for i in 0..prog.defs.len() {
        let c = DefId(i as u32);
        let def = prog.def(c);
        if !visible.get(def.krate as usize).copied().unwrap_or(true) {
            continue;
        }
        let konst_fn = matches!(def.kind, DefKind::Fn | DefKind::AssocFn) && matches!(prog.files[def.file as usize].ast.item(def.item).kind, crate::ast::ItemKind::Fn(sig, _, _) if prog.files[def.file as usize].ast.fn_sig(sig).konst);
        let text_names = match def.kind {
            DefKind::AnonConst => match prog.def_id_to_node_id.get(&c) {
                Some(&(file, NodeId::AnonConst(e) | NodeId::InlineConst(e))) => {
                    let pf = &prog.files[file as usize];
                    let x = pf.ast.expr(e);
                    toks(pf, x.lo, x.hi, &[]).iter().any(|t| names.contains(t))
                }
                _ => true,
            },
            DefKind::Const | DefKind::AssocConst | DefKind::Static => item_mentions(prog, c, names),
            _ => konst_fn && item_mentions(prog, c, names),
        };
        if !text_names || changes.iter().any(|x| matches!(x, Change::Const(y) | Change::StaticInit(y) if *y == c)) {
            continue;
        }
        // an inline const of a fn this edit rebuilds (an `offset_of!` in it) is evaluated again
        // with the fn's new text
        if def.kind == DefKind::AnonConst {
            let mut owner = def.parent;
            while owner != crate::program::NO_DEF && matches!(prog.def(owner).kind, DefKind::AnonConst | DefKind::Closure) {
                owner = prog.def(owner).parent;
            }
            if changes.iter().any(|x| matches!(x, Change::Body(y) if *y == owner)) {
                continue;
            }
        }
        let body = u.tcx.typeck(prog, c);
        let Some((_, k)) = body.field_uses.iter().find(|(ud, k)| *ud == d && gone.contains(k)) else { continue };
        if konst_fn && !const_contexts_mention(prog, &[c]) {
            continue;
        }
        let field = u.tcx.adts.get(&d).and_then(|a| a.variants[0].fields.get(*k as usize)).map_or("?", |f| prog.syms.str(f.name));
        return Some(format!("`{}` is evaluated at compile time and uses field `{}` of `{}`, which leaves its slot in this edit: its value is not made again -- restart the app", prog.def_path(c), field, def_name(prog, d)));
    }
    None
}

/// The running struct `d` (its fields in slot order) and its new version (`fresh`, fields in
/// declaration order) reconciled into the struct the running values can be (M10): a field keeps
/// its slot by name and type (a renamed one by type, when exactly one field went and one came
/// with that type), an added one goes to the extension block, a removed one whose type needs no
/// drop stays as a dead slot. Anything else: Err with the reason (restart rung).
///
/// `new_layout`: a struct no running value of is left (fresh_layout_struct, no_live_values),
/// which takes its new definition as it is.
fn layout_edit(u: &Unit, d: DefId, fresh: &crate::tcx::AdtT, running_discriminants: &[u128], fresh_discriminants: &[u128], new_layout: bool) -> Result<(crate::tcx::AdtT, Vec<String>), String> {
    let old = u.tcx.adts.get(&d).ok_or("no running definition")?;
    let name = u.prog.def_path(d);
    if old.is_enum {
        return enum_edit(u, d, old, fresh, running_discriminants, fresh_discriminants);
    }
    if new_layout {
        if fresh.is_enum || fresh.n_generics != 0 || fresh.variants.len() != 1 {
            return Err(format!("struct `{}`: its generics changed -- restart the app", name));
        }
        return Ok((fresh.clone(), vec![format!("struct `{}` takes its new layout: no running value of it is left, and the code using it is rebuilt", name)]));
    }
    let t = {
        let args: Vec<TyId> = (0..old.n_generics).map(|_| u.tcx.tys.unit).collect();
        u.tcx.tys.intern(crate::types::TyKind::Adt(d, u.tcx.tys.mk_list(&args), crate::types::RegionArgsId::EMPTY))
    };
    if !u.lay.live_editable(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, d, old, t) {
        return Err(struct_layout_refusal(u, d, old, &name));
    }
    trace_sub("layout_edit_live_editable");
    // An outer type that holds the struct by value (`Option<App>` of the platform, anywhere) keeps
    // working: the struct keeps its first layout (size, alignment, the running fields' offsets and
    // its niche; added fields go to the extension block), the outer types' drop glue reaches the
    // rebuilt glue through its slot or the inlined-into closure, and every crate that sees the
    // edit is checked for users of a removed field. What it cannot take is a niche that goes dead:
    // removed and retyped fields leave their slot unwritten in values built after the edit, so the
    // one holding the niche an outer enum reads its tag from must stay (below).
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
    // removed: a dead slot, when nothing needs dropping in it and it holds no niche
    let niche = niche_fields(u, d);
    for k in 0..n_old {
        if taken[k] || dead(u.prog.syms.str(fields[k].name)) {
            continue;
        }
        if niche.contains(&k) {
            return Err(format!("struct `{}`: field `{}` was removed or retyped, and it holds the niche an outer type (e.g. `Option<{}>`) may keep its tag in; values built after the edit leave it unset -- restart the app", name, u.prog.syms.str(fields[k].name), name));
        }
        // a dead slot that owns memory is dropped with the values built before the edit (their ext
        // word says they set it; lower.rs ext_drop_fn); values built after it leave it unset.
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
                let (sa, ga) = u.tcx.tys.int_info(*a);
                let (sb, gb) = u.tcx.tys.int_info(*b);
                (ga == gb && sb >= sa) || (!ga && gb && sb > sa)
            }
            (Float(a), Float(b)) => fsize(*a) <= fsize(*b),
            (Int(a), Float(b)) => u.tcx.tys.int_info(*a).0 < fsize(*b),
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

/// The crates whose text differs from the text the run booted from: the files of a transaction
/// in flight, and a kept edit's (a current version of other text, an included file read anew).
/// Their code must never come from the disk cache, whose keys predate the edits, and the run's
/// front records wait for them (run::front_recordable). A refused edit leaves no crate edited,
/// nor does one that puts the boot text back (settle_edited_crates).
static EDITED_CRATES: relax_session::sync::Mutex<Vec<u32>> = relax_session::sync::Mutex::new(Vec::new());
/// The crates a kept edit read an included file of anew: a version's text does not show what
/// its includes held, so they stay edited.
static INCLUDES_EDITED: relax_session::sync::Mutex<Vec<u32>> = relax_session::sync::Mutex::new(Vec::new());
/// Live edit transactions that changed the program's tables (committed a file version, kept or
/// not). From the first one on the tables are not the boot's, with every edit refused or undone
/// too (the versions and definitions stay loaded): what a check began before one, or found
/// after one, does not describe the boot.
static PROGRAM_EDITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn edited_crates() -> Vec<u32> {
    EDITED_CRATES.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// PROGRAM_EDITS.
pub(crate) fn program_edits() -> u64 {
    PROGRAM_EDITS.load(std::sync::atomic::Ordering::SeqCst)
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

/// A transaction ended, or a patch was rolled back: a crate stays edited while a file of it has
/// a current version whose text is not its boot version's, or a kept edit read its includes
/// anew. Before a crate stops being edited the keys follow the text back if a const edit moved
/// them (its walk covers the crate while it still is edited): an interface its item edit had
/// would stay in them. On the owner thread.
unsafe fn settle_edited_crates(up: *mut Unit) {
    let prog = &(*up).prog;
    let mut differ = INCLUDES_EDITED.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for (&boot, &now) in prog.current_versions() {
        let path = &prog.file_paths[boot as usize];
        if prog.files[now as usize].source_hash(path) != prog.files[boot as usize].source_hash(path) {
            differ.push(prog.file_crate[boot as usize]);
        }
    }
    let clean: Vec<u32> = edited_crates().into_iter().filter(|c| !differ.contains(c)).collect();
    if clean.is_empty() { return; }
    crate::cache::rekey_if_moved(up);
    EDITED_CRATES.lock().unwrap_or_else(|e| e.into_inner()).retain(|c| !clean.contains(c));
    if live_trace() {
        let prog = &(*up).prog;
        let names = |cs: &[u32]| cs.iter().map(|&c| prog.syms.str(prog.crates[c as usize].name).to_string()).collect::<Vec<String>>().join(", ");
        eprintln!("makepad live: not edited (the text the run booted from): {}; edited: [{}]", names(&clean), names(&edited_crates()));
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
    let src = u.front.checker_id;
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

/// Whether `d` is still the program's (bound in its module, or in its impl's or bound trait's
/// item list): an edit that replaced or removed a fn unbinds it, while code in flight keeps its
/// slots.
fn def_is_bound(prog: &crate::program::Program, d: DefId) -> bool {
    let def = prog.def(d);
    let parent = def.parent;
    if parent != crate::program::NO_DEF && prog.def(parent).kind == DefKind::Impl {
        return prog.impls.get(prog.def(parent).sub as usize).map_or(false, |i| i.items.contains(&d));
    }
    if parent != crate::program::NO_DEF && prog.def(parent).kind == DefKind::Trait {
        return prog.traits.get(prog.def(parent).sub as usize).is_some_and(|v| v.contains(&d)) && def_is_bound(prog, parent);
    }
    prog.mods.get(def.scope as usize).map_or(false, |m| m.values.get(&def.name) == Some(&d) || m.types.get(&def.name) == Some(&d))
}

/// The two types a vtable's data name gives (syms::VTABLE_NAME: `vtable:<type>#<hash>:<dyn
/// type>#<hash>`, jit.rs Unit::vtable), as printed, without their hashes.
fn vtable_name_types(name: &str) -> Option<(String, String)> {
    let (t, dy) = crate::syms::vtable_name_parts(name)?;
    let bare = |t: &str| t.rsplit_once('#').map_or(t, |x| x.0).to_string();
    Some((bare(t), bare(dy)))
}

/// "T` as `dyn Tr" from a vtable's data name, for a message's backquotes.
fn vtable_types(name: &str) -> String {
    match vtable_name_types(name) {
        Some((t, d)) => format!("{t}` as `{d}"),
        None => name.to_string(),
    }
}

/// A fn of a trait impl that stays (not in `removed`): its impl and the trait's item it implements.
fn override_of(prog: &crate::program::Program, tcx: &crate::tcx::Tcx, d: DefId, removed: &[DefId]) -> Option<(DefId, DefId)> {
    let def = prog.def(d);
    if def.kind != DefKind::AssocFn || def.parent == crate::program::NO_DEF || prog.def(def.parent).kind != DefKind::Impl || !tcx.impl_trait.contains_key(&def.parent) || removed.contains(&def.parent) {
        return None;
    }
    Some((def.parent, *prog.trait_item_def_id.get(&d)?))
}

/// The instance slot `s` is code of (a closure's: its fn's).
fn slot_instance(u: &Unit, s: u32) -> Option<(DefId, &[TyId])> {
    let mut k = s;
    for _ in 0..64 {
        match &u.fns[k as usize].key {
            FnKey::Closure(_, _, p) => k = *p,
            FnKey::Inst(d, args) => return Some((*d, args)),
            _ => return None,
        }
    }
    None
}

/// Whether slot `s` is code of a fn that is no longer the program's (`removed` by this edit, or
/// replaced by an earlier one): code in flight, which keeps its slots and is never rebuilt against
/// the new definitions (its old text need not check against them).
fn in_flight_only(u: &Unit, s: u32, removed: &[DefId]) -> bool {
    let Some((d, _)) = slot_instance(u, s) else { return false };
    let def = u.prog.def(d);
    if removed.contains(&d) {
        return true;
    }
    // a trait's provided method is its trait's
    if def.parent != crate::program::NO_DEF && u.prog.def(def.parent).kind == DefKind::Trait {
        return removed.contains(&def.parent) || !def_is_bound(&u.prog, d);
    }
    matches!(def.kind, DefKind::Fn | DefKind::AssocFn) && !def_is_bound(&u.prog, d)
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

/// The ADTs type `t` names (as ty_mentions walks it).
fn ty_adts(u: &Unit, t: TyId, out: &mut Vec<DefId>, depth: u32) {
    use crate::types::TyKind::*;
    if depth > 16 {
        return;
    }
    match u.tcx.tys.kind(t).clone() {
        Adt(d, args, _) => {
            if !out.contains(&d) {
                out.push(d);
            }
            for a in u.tcx.tys.list(args).iter() {
                ty_adts(u, *a, out, depth + 1);
            }
        }
        Ref(_, x, _) | Ptr(_, x) | Slice(x) | Array(x, _) => ty_adts(u, x, out, depth + 1),
        Tuple(v) => for a in u.tcx.tys.list(v).iter() { ty_adts(u, *a, out, depth + 1) },
        _ => {}
    }
}

/// Live sessions (M10): the structs the code of `d` writes in place through a raw pointer -- a
/// raw or `&mut` borrow of a place reached through the deref of a raw pointer, an assignment to
/// one, a `&mut self` method called on one: `addr_of_mut!((*p).f)`, `&raw mut (*p).f`,
/// `&mut *p`, `(*u.as_mut_ptr()).f = x`, `(*p).f.write(x)`, and the `&raw const (*base).f` of
/// `offset_of!`. Such code makes a value without writing its ext word: the memory keeps what it
/// held there, a moved-out value's word maybe. The places' types are typeck's. Into `conv`: the
/// structs whose bytes it reaches as other data, at offsets nothing checks, and those a pointer
/// handed to inline assembly points at (`converted`).
fn raw_written(u: &Unit, d: DefId, out: &mut Vec<DefId>, conv: &mut Vec<DefId>) {
    use crate::ast::{ExprKind, UnOp};
    use crate::types::TyKind;
    let body = u.tcx.typeck(&u.prog, d);
    let pf = &u.prog.files[body.file as usize];
    let ast = &pf.ast;
    for i in 0..body.expr_ty.len() {
        let e = body.expr_at(i);
        converted(u, &body, pf, e, conv);
        let place = match ast.expr(e).kind {
            ExprKind::AddrOf(raw, mutbl, p) if raw || mutbl => p,
            ExprKind::Assign(l, _) | ExprKind::AssignOp(_, l, _) => l,
            ExprKind::MethodCall(recv, _) if body.methods.get(&e.0).is_some_and(|m| m.autoref == 2 && m.derefs == 0) => recv,
            _ => continue,
        };
        let mut adts: Vec<DefId> = Vec::new();
        let mut p = place;
        loop {
            match ast.expr(p).kind {
                ExprKind::Paren(x) => p = x,
                ExprKind::Index(x, _) if body.ov_index.get(&p.0).is_none() => p = x,
                ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => {
                    // a field reached through a reference (autoderef) is written where that points
                    if body.field_idx.get(&p.0).is_some_and(|f| f.0 != 0) || !body.has_expr(x) {
                        break;
                    }
                    if let TyKind::Adt(a, _, _) = u.tcx.tys.kind(body.ty(x)) {
                        adts.push(*a);
                    }
                    p = x;
                }
                ExprKind::Unary(UnOp::Deref, y) => {
                    if body.has_expr(y) && matches!(u.tcx.tys.kind(body.ty(y)), TyKind::Ptr(..)) {
                        if let TyKind::Adt(a, _, _) = u.tcx.tys.kind(body.ty(p)) {
                            adts.push(*a);
                        }
                        for a in adts {
                            if !out.contains(&a) {
                                out.push(a);
                            }
                        }
                    }
                    break;
                }
                _ => break,
            }
        }
    }
}

/// The struct pointer type `t` points at (a raw pointer, a reference, NonNull; through arrays and
/// slices of it).
fn pointee_adt(u: &Unit, t: TyId) -> Option<DefId> {
    use crate::types::TyKind;
    let mut x = match u.tcx.tys.kind(t) {
        TyKind::Ptr(_, x) | TyKind::Ref(_, x, _) => *x,
        TyKind::Adt(nn, args, _) if u.prog.name(*nn) == "NonNull" => *u.tcx.tys.list(*args).first()?,
        _ => return None,
    };
    loop {
        match u.tcx.tys.kind(x) {
            TyKind::Array(y, _) | TyKind::Slice(y) => x = *y,
            TyKind::Adt(a, _, _) => return Some(*a),
            _ => return None,
        }
    }
}

/// Whether `t` is a raw pointer (or NonNull), a mutable view of bytes (`&mut [u8]`, `&mut [u32; 4]`),
/// or (`ints`) an address.
fn pointer_like(u: &Unit, t: TyId, ints: bool) -> bool {
    use crate::types::TyKind;
    let int = |x: TyId| matches!(u.tcx.tys.kind(x), TyKind::Int(_));
    match u.tcx.tys.kind(t) {
        TyKind::Ptr(..) => true,
        TyKind::Adt(nn, ..) => u.prog.name(*nn) == "NonNull",
        TyKind::Ref(true, x, _) => matches!(u.tcx.tys.kind(*x), TyKind::Slice(y) | TyKind::Array(y, _) if int(*y)),
        TyKind::Int(_) => ints,
        _ => false,
    }
}

/// Live sessions (M10), for raw_written: the structs expression `e` of `body` reaches as other
/// bytes -- a pointer to one made a pointer of another type, an address or a byte view, or one
/// made from those: `as *mut u8`, `bytes.as_ptr() as *const T`, `.cast::<U>()`, `.byte_add(n)`,
/// `.addr()`, `transmute`, a method of one returning a pointer to bytes, a call taking a pointer
/// of one type and returning one of another (two references to structs are a field accessor's,
/// not this) -- and the structs an `asm!` operand points at. Code writing there makes a value
/// whose ext word nothing wrote; which code writes is not followed (a restart is cheap).
fn converted(u: &Unit, body: &relax_types::typeck::Body, pf: &crate::parser::ParsedFile, e: crate::ast::ExprId, out: &mut Vec<DefId>) {
    use crate::ast::ExprKind;
    let ast = &pf.ast;
    let mut mark = |from: Option<DefId>, to: Option<DefId>| if from != to {
        for a in [from, to].into_iter().flatten() {
            if !out.contains(&a) { out.push(a); }
        }
    };
    let adt = |t: TyId| match u.tcx.tys.kind(t) { crate::types::TyKind::Adt(a, _, _) => Some(*a), _ => None };
    let pointerish = |t: TyId| pointer_like(u, t, false) || pointee_adt(u, t).is_some();
    match ast.expr(e).kind {
        ExprKind::Cast(x, _) if body.has_expr(x) => {
            let (from, to) = (body.ty(x), body.ty(e));
            if pointer_like(u, from, true) && pointer_like(u, to, true) {
                mark(pointee_adt(u, from), pointee_adt(u, to));
            }
        }
        ExprKind::MethodCall(recv, row) if body.has_expr(recv) => {
            let (from, to) = (body.ty(recv), body.ty(e));
            if pointer_like(u, from, false) {
                if pointer_like(u, to, true) {
                    // (a byte offset keeps the type and moves off its fields)
                    let bytewise = matches!(pf.text(ast.row(row).name), "byte_add" | "byte_sub" | "byte_offset" | "wrapping_byte_add" | "wrapping_byte_sub" | "wrapping_byte_offset");
                    mark(pointee_adt(u, from), if bytewise { None } else { pointee_adt(u, to) });
                }
            } else if pointer_like(u, to, false) && pointee_adt(u, to).is_none() {
                // a value's or a reference's own bytes (a container's element pointer is no conversion)
                mark(pointee_adt(u, from).or_else(|| adt(from)), None);
            }
        }
        ExprKind::Call(f, args) => {
            let transmute = matches!(ast.expr(f).kind, ExprKind::Path(p) if ast.segs(p.segs).last().is_some_and(|s| matches!(pf.text(s.name), "transmute" | "transmute_copy")));
            let to = body.ty(e);
            for &x in ast.list(args) {
                if !body.has_expr(x) {
                    continue;
                }
                let from = body.ty(x);
                if transmute {
                    mark(pointee_adt(u, from).or_else(|| adt(from)), pointee_adt(u, to).or_else(|| adt(to)));
                } else if (pointer_like(u, from, false) && pointerish(to)) || (pointerish(from) && pointer_like(u, to, false)) {
                    mark(pointee_adt(u, from), pointee_adt(u, to));
                }
            }
        }
        ExprKind::Mac(m) => {
            let m = *ast.row(m);
            if m.args == crate::ast::NO_MAC_ARGS {
                return;
            }
            let Some(asm) = ast.mac_arg(m.args).asm else { return };
            for op in ast.list(asm.ops) {
                for x in [op.input, op.output].into_iter().flatten() {
                    if body.has_expr(x) {
                        mark(pointee_adt(u, body.ty(x)), None);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Whether `text` converts a pointer (`as *T`, `as usize`, `.cast()`, `.byte_add()`, `.addr()`,
/// `transmute`) or holds inline assembly: what `converted` reads, by its tokens.
fn text_converts(text: &[&[u8]]) -> bool {
    text.windows(2).any(|w| w[0] == b"as" && matches!(w[1], b"*" | b"usize" | b"isize" | b"u64" | b"i64" | b"u128"))
        || text.iter().any(|t| matches!(*t, b"cast" | b"cast_mut" | b"cast_const" | b"byte_add" | b"byte_sub" | b"byte_offset" | b"wrapping_byte_add"
            | b"wrapping_byte_sub" | b"wrapping_byte_offset" | b"addr" | b"expose_provenance" | b"transmute" | b"transmute_copy" | b"asm"))
}

/// Live sessions (M10): the structs whose running layout needs their values' ext word -- fields
/// in the extension block (a block a stale word would share), or a dead slot to drop (a stale
/// EXT_BUILT word would drop what the value never set).
fn structs_needing_ext_word(u: &Unit) -> Vec<DefId> {
    let mut out: Vec<DefId> = u.tcx.adts.iter().filter(|(_, a)| !a.is_enum && !a.is_union && a.variants.len() == 1 && {
        let fields = &a.variants[0].fields;
        (a.ext_from as usize) < fields.len()
            || fields.iter().enumerate().any(|(k, f)| !adt_ext_field(a, k) && crate::lower::is_dead_field(u, f.name) && u.needs_drop_glue(f.ty))
    }).map(|(d, _)| *d).collect();
    out.sort();
    out
}

/// Live sessions (M10): whether code can write through a raw pointer, which takes an unsafe
/// context, or convert one (text_converts). Code is read by its text where the text is its code:
/// it says `unsafe` (or converts), or it invokes a macro_rules whose text does (by name, through
/// the macros that one invokes). Code a macro made (a derive's impl), and code invoking a
/// function-like proc macro, itself or through the macro_rules it invokes (an expansion that is
/// not text to read), is read parsed, with every expansion in it (parsed_raw_write).
struct RawGate {
    procs: Vec<Vec<u8>>,
    /// the macro_rules by name, made at the first lookup
    macros: std::cell::OnceCell<std::collections::HashMap<Vec<u8>, Vec<DefId>>>,
    /// by macro name, each Ask's answer once known
    memo: std::cell::RefCell<std::collections::HashMap<Vec<u8>, [Option<bool>; 3]>>,
}

/// What RawGate asks of a text: does it say `unsafe`, convert a pointer, invoke a proc macro.
#[derive(Clone, Copy)]
enum Ask {
    Unsafe,
    Converts,
    Proc,
}

impl RawGate {
    fn new() -> RawGate {
        let procs = crate::macro_reg::REG.with(|r| r.borrow().as_ref().map(|g| g.macros.iter().filter(|(_, e)| e.kind == crate::macro_reg::Kind::Bang).map(|(n, _)| n.as_bytes().to_vec()).collect()).unwrap_or_default());
        RawGate { procs, macros: Default::default(), memo: Default::default() }
    }

    /// The names `text` invokes as macros (`name!`).
    fn invoked<'a, 't: 'a>(text: &'a [&'t [u8]]) -> impl Iterator<Item = &'t [u8]> + 'a {
        text.windows(2).filter(|w| w[1] == b"!" && w[0].first().is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')).map(|w| w[0])
    }

    /// Whether code `d`, whose text is `text`, is read parsed: a macro expansion made it, or it
    /// invokes a proc macro, itself or through a macro_rules.
    fn read_parsed(&self, prog: &Program, d: DefId, text: &[&[u8]]) -> bool {
        let def = prog.def(d);
        let pf = &prog.files[def.file as usize];
        pf.ast.item(def.item).lo >= pf.src_len || self.text_has(prog, text, Ask::Proc)
    }

    /// Whether code `d`, whose text is `text`, can write through a raw pointer.
    fn can_write_raw(&self, prog: &Program, d: DefId, text: &[&[u8]]) -> bool {
        if self.read_parsed(prog, d, text) { parsed_raw_write(prog, d) } else { self.text_has(prog, text, Ask::Unsafe) }
    }

    /// Whether `text` (code read as text, read_parsed) asks `ask`: by its own tokens or the
    /// macro_rules it invokes (a proc macro it invokes can do anything).
    fn text_has(&self, prog: &Program, text: &[&[u8]], ask: Ask) -> bool {
        let own = match ask {
            Ask::Unsafe => text.iter().any(|t| *t == b"unsafe"),
            Ask::Converts => text_converts(text),
            Ask::Proc => false,
        };
        own || Self::invoked(text).any(|n| self.procs.iter().any(|p| p.as_slice() == n) || self.macro_has(prog, n, ask))
    }

    fn macro_has(&self, prog: &Program, name: &[u8], ask: Ask) -> bool {
        if let Some(known) = self.memo.borrow().get(name).and_then(|m| m[ask as usize]) {
            return known;
        }
        // (a macro invoking itself: no until shown otherwise)
        self.memo.borrow_mut().entry(name.to_vec()).or_default()[ask as usize] = Some(false);
        let macros = self.macros.get_or_init(|| {
            let mut by_name: std::collections::HashMap<Vec<u8>, Vec<DefId>> = Default::default();
            for i in 0..prog.defs.len() {
                let d = DefId(i as u32);
                if prog.def(d).kind == DefKind::MacroRules {
                    by_name.entry(def_name(prog, d).into_bytes()).or_default().push(d);
                }
            }
            by_name
        });
        let found = macros.get(name).is_some_and(|defs| defs.iter().any(|&m| {
            let pf = &prog.files[prog.def(m).file as usize];
            let it = pf.ast.item(prog.def(m).item);
            self.text_has(prog, &toks(pf, it.lo, it.hi, &[]), ask)
        }));
        self.memo.borrow_mut().entry(name.to_vec()).or_default()[ask as usize] = Some(found);
        found
    }
}

/// Live sessions (M10): whether the parsed code of `d`, every macro expansion in it included, can
/// write in place through a raw pointer, as raw_written reads it but by shape alone: in an unsafe
/// context (an `unsafe` block, an `unsafe fn`), a raw or `&mut` borrow of, an assignment to, or a
/// method call on a place reached through a deref; inline assembly; or, with an unsafe context
/// anywhere in it, a pointer conversion (a cast to a pointer or an address, a method or call
/// text_converts names). Code that does not parse counts as one that can.
fn parsed_raw_write(prog: &Program, d: DefId) -> bool {
    use crate::ast::{Ast, ExprId, ExprKind, ItemKind, Ty, UnOp};
    use relax_types::ast_visit::{walk_expr, Visitor};
    struct Find<'a> {
        pf: &'a crate::parser::ParsedFile,
        ast: &'a Ast,
        unsafe_depth: u32,
        saw_unsafe: bool,
        converts: bool,
        found: bool,
    }
    impl Find<'_> {
        fn converting(&self, kind: ExprKind) -> bool {
            let named = |n: &str| text_converts(&[n.as_bytes()]);
            match kind {
                ExprKind::Cast(_, t) => match *self.ast.ty(t) {
                    Ty::Ptr(..) => true,
                    Ty::Path(p) => self.ast.segs(p.segs).last().is_some_and(|s| matches!(self.pf.text(s.name), "usize" | "isize" | "u64" | "i64" | "u128")),
                    _ => false,
                },
                ExprKind::MethodCall(_, row) => named(self.pf.text(self.ast.row(row).name)),
                ExprKind::Call(f, _) => matches!(self.ast.expr(f).kind, ExprKind::Path(p) if self.ast.segs(p.segs).last().is_some_and(|s| named(self.pf.text(s.name)))),
                _ => false,
            }
        }

        fn through_deref(&self, mut p: ExprId) -> bool {
            loop {
                match self.ast.expr(p).kind {
                    ExprKind::Paren(x) | ExprKind::Index(x, _) | ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => p = x,
                    ExprKind::Unary(UnOp::Deref, _) => return true,
                    _ => return false,
                }
            }
        }
    }
    impl<'a> Visitor<'a> for Find<'a> {
        fn ast(&self) -> &'a Ast {
            self.ast
        }
        fn expr(&mut self, e: ExprId) {
            if self.found {
                return;
            }
            let kind = self.ast.expr(e).kind;
            if let ExprKind::Mac(m) = kind {
                let m = *self.ast.row(m);
                if m.args != crate::ast::NO_MAC_ARGS && self.ast.mac_arg(m.args).asm.is_some() {
                    self.found = true;
                    return;
                }
            }
            self.converts = self.converts || self.converting(kind);
            if self.unsafe_depth > 0 {
                self.saw_unsafe = true;
                let place = match kind {
                    ExprKind::AddrOf(raw, mutbl, p) if raw || mutbl => Some(p),
                    ExprKind::Assign(l, _) | ExprKind::AssignOp(_, l, _) => Some(l),
                    ExprKind::MethodCall(recv, _) => Some(recv),
                    _ => None,
                };
                if place.is_some_and(|p| self.through_deref(p)) {
                    self.found = true;
                    return;
                }
            }
            let inner = matches!(kind, ExprKind::Unsafe(_)) as u32;
            self.unsafe_depth += inner;
            walk_expr(self, e);
            self.unsafe_depth -= inner;
        }
    }
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    let ast = &pf.ast;
    let mut f = Find { pf, ast, unsafe_depth: 0, saw_unsafe: false, converts: false, found: false };
    match ast.item(def.item).kind {
        ItemKind::Fn(sig, ..) => {
            f.unsafe_depth = ast.fn_sig(sig).unsafe_ as u32;
            match prog.fn_body(def.file, def.item) {
                Ok(Some(b)) => f.block(b),
                Ok(None) => {}
                Err(_) => return true,
            }
        }
        ItemKind::Const(..) | ItemKind::Static(..) => match prog.const_body(def.file, def.item) {
            Ok(Some(e)) => f.expr(e),
            Ok(None) => {}
            Err(_) => return true,
        },
        _ => return true,
    }
    f.found || (f.saw_unsafe && f.converts)
}

/// Live sessions (M10): of the structs `need` (structs_needing_ext_word), one whose fields code
/// writes in place through a raw pointer (raw_written), with that code and whether it reaches
/// the struct's bytes as other data (its own, or a holder's: Reach). The program is read, not its
/// compiled code: a builder compiled only after the edit counts. So does `replaced` (the earlier
/// versions of what the edit replaces: code that may still be running). Code that can do it is
/// type-checked: its text names one of the struct's fields (or a macro_rules that can) and can
/// write through a raw pointer, or it converts a pointer (text_converts) in an unsafe context and
/// names a way to one of the structs (Reach), or (code a macro made, code invoking a proc macro)
/// its parsed code has the shape of such a write or conversion (RawGate).
fn raw_builder(u: &Unit, need: &[DefId], replaced: &[(DefId, DefId, bool)]) -> Option<(DefId, DefId, bool)> {
    if let Some(&found) = replaced.iter().find(|(s, _, _)| need.contains(s)) {
        return Some(found);
    }
    let prog = &u.prog;
    let gate = RawGate::new();
    // one pass for all of them: the crates seeing any, the names of all their fields
    let visible = prog.crates_seeing_files(need.iter().map(|&s| prog.def(s).file));
    let mut fields: Vec<&[u8]> = Vec::new();
    for &s in need {
        let Some(adt) = u.tcx.adts.get(&s) else { continue };
        // a dead slot's field went by its own name (`__relax_dead_<k>_<name>`)
        fields.extend(adt.variants[0].fields.iter().map(|f| {
            let n = prog.syms.str(f.name).as_bytes();
            match n.strip_prefix(b"__relax_dead_") {
                Some(rest) => rest.iter().position(|c| *c == b'_').map_or(n, |at| &rest[at + 1..]),
                None => n,
            }
        }));
    }
    let names = through_macros(prog, &visible, &fields);
    let names: Vec<&[u8]> = names.iter().map(|n| n.as_slice()).collect();
    // made once code converting a pointer needs it
    let reach_once = std::cell::OnceCell::new();
    let reach = || reach_once.get_or_init(|| Reach::new(prog, &visible, need));
    // the struct of `need` whose bytes code reaching `conv` as other data reaches
    let reached = |conv: &[DefId]| conv.iter().find_map(|&c| if need.contains(&c) { Some(c) } else { reach().holds(c) });
    for &(s, by, conv) in replaced {
        if let Some(n) = conv.then(|| reached(&[s])).flatten() {
            return Some((n, by, true));
        }
    }
    let mut converting: Vec<(DefId, Vec<&[u8]>)> = Vec::new();
    let check = |d: DefId| -> Option<(DefId, DefId, bool)> {
        let (mut written, mut conv) = (Vec::new(), Vec::new());
        raw_written(u, d, &mut written, &mut conv);
        if let Some(&s) = written.iter().find(|w| need.contains(w)) {
            return Some((s, d, false));
        }
        reached(&conv).map(|s| (s, d, true))
    };
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if !visible.get(def.krate as usize).copied().unwrap_or(false) || !matches!(def.kind, DefKind::Fn | DefKind::AssocFn | DefKind::Const | DefKind::AssocConst | DefKind::Static) {
            continue;
        }
        let pf = &prog.files[def.file as usize];
        let it = pf.ast.item(def.item);
        let text = toks(pf, it.lo, it.hi, &[]);
        // code a macro made, or code invoking a proc macro, may name the fields where its text does
        // not
        let can = if gate.read_parsed(prog, d, &text) {
            parsed_raw_write(prog, d)
        } else if text.iter().any(|t| names.contains(t)) {
            gate.text_has(prog, &text, Ask::Unsafe)
        } else {
            // code converting a pointer, naming none of the fields: checked after the rest, when it
            // names a way to one of the structs (`unsafe` asked first: the rarer one)
            if gate.text_has(prog, &text, Ask::Unsafe) && gate.text_has(prog, &text, Ask::Converts) {
                converting.push((d, text));
            }
            continue;
        };
        if !can {
            continue;
        }
        if let Some(found) = check(d) {
            return Some(found);
        }
    }
    for (d, text) in converting {
        if reach().named_by(d, &text) {
            if let Some(found) = check(d) {
                return Some(found);
            }
        }
    }
    None
}

/// Live sessions (M10), for raw_builder: how code reaches the structs `need`, read as text (by
/// name, as far as code asks). A type holds one when its text names one or a type holding one
/// (a field's type, through aliases): code viewing a holder's bytes views that one's. Code gets a
/// value of one by a name of such a type, of a field whose type names one, of a fn, static or
/// const whose signature (or type) names one, or of a macro_rules whose text names any of these,
/// or by `self` in an impl of one. Code naming none of them gets such a value only through
/// generic code, whose pointers raw_written does not read anyway.
struct Reach<'p> {
    prog: &'p Program,
    need: &'p [DefId],
    /// the defs of the crates seeing the structs, by name
    by_name: std::collections::HashMap<&'p [u8], Vec<DefId>>,
    /// by name: the struct of `need` a type of that name holds, a name gets a value of
    types: std::cell::RefCell<std::collections::HashMap<&'p [u8], Option<DefId>>>,
    names: std::cell::RefCell<std::collections::HashMap<&'p [u8], Option<DefId>>>,
}

impl<'p> Reach<'p> {
    fn new(prog: &'p Program, visible: &[bool], need: &'p [DefId]) -> Reach<'p> {
        let mut by_name: std::collections::HashMap<&'p [u8], Vec<DefId>> = Default::default();
        for i in 0..prog.defs.len() {
            let d = DefId(i as u32);
            let def = prog.def(d);
            if visible.get(def.krate as usize).copied().unwrap_or(false) {
                by_name.entry(prog.syms.str(def.name).as_bytes()).or_default().push(d);
            }
        }
        Reach { prog, need, by_name, types: Default::default(), names: Default::default() }
    }

    fn text(&self, d: DefId) -> (&'p crate::parser::ParsedFile, u32, u32) {
        let def = self.prog.def(d);
        let pf = &self.prog.files[def.file as usize];
        let it = pf.ast.item(def.item);
        (pf, it.lo, it.hi)
    }

    /// The struct of `need` a type named `name` holds (or is).
    fn ty(&self, name: &'p [u8]) -> Option<DefId> {
        if let Some(&known) = self.types.borrow().get(name) {
            return known;
        }
        // (a type holding itself: no until shown otherwise)
        self.types.borrow_mut().insert(name, None);
        let found = self.need.iter().copied().find(|&s| self.prog.syms.str(self.prog.def(s).name).as_bytes() == name).or_else(|| {
            self.by_name.get(name)?.iter().filter(|&&d| matches!(self.prog.def(d).kind, DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TypeAlias)).find_map(|&d| {
                let (pf, lo, hi) = self.text(d);
                toks(pf, lo, hi, &[]).into_iter().find_map(|t| self.ty(t))
            })
        });
        self.types.borrow_mut().insert(name, found);
        found
    }

    /// The struct of `need` code naming `name` can get a value of.
    fn name(&self, name: &'p [u8]) -> Option<DefId> {
        if let Some(&known) = self.names.borrow().get(name) {
            return known;
        }
        self.names.borrow_mut().insert(name, None);
        let found = self.ty(name).or_else(|| self.by_name.get(name)?.iter().find_map(|&d| {
            let sig = match self.prog.def(d).kind {
                DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => true,
                DefKind::Field | DefKind::Static | DefKind::Const | DefKind::AssocConst | DefKind::ForeignStatic | DefKind::MacroRules => false,
                _ => return None,
            };
            let (pf, lo, hi) = self.text(d);
            match self.prog.def(d).kind {
                DefKind::Field => match struct_fields(pf, pf.ast.item(self.prog.def(d).item)).into_iter().find(|(f, _)| *f == name) {
                    Some((_, ty)) => ty.into_iter().find_map(|t| self.ty(t)),
                    // (a variant's or a union's: its item's text)
                    None => toks(pf, lo, hi, &[]).into_iter().find_map(|t| self.ty(t)),
                },
                DefKind::MacroRules => toks(pf, lo, hi, &[]).into_iter().find_map(|t| self.name(t)),
                _ => head_toks(pf, lo, hi, sig).into_iter().find_map(|t| self.ty(t)),
            }
        }));
        self.names.borrow_mut().insert(name, found);
        found
    }

    /// The struct of `need` type `d` holds (or is).
    fn holds(&self, d: DefId) -> Option<DefId> {
        if self.need.contains(&d) {
            return Some(d);
        }
        let name = self.prog.syms.str(self.prog.def(d).name).as_bytes();
        self.by_name.get(name).is_some_and(|ds| ds.contains(&d)).then(|| self.ty(name)).flatten()
    }

    /// Whether code `d`, whose text is `text`, names a way to one of the structs.
    fn named_by(&self, d: DefId, text: &[&'p [u8]]) -> bool {
        if text.iter().any(|t| self.name(*t).is_some()) {
            return true;
        }
        // `self` in an impl of one
        let def = self.prog.def(d);
        def.parent != crate::program::NO_DEF && self.prog.def(def.parent).kind == DefKind::Impl && text.iter().any(|t| *t == b"self" || *t == b"Self") && {
            let (pf, lo, hi) = self.text(def.parent);
            head_toks(pf, lo, hi, true).into_iter().any(|t| self.ty(t).is_some())
        }
    }
}

/// The tokens of the text `lo..hi` of an item: (`sig`) its signature, those before its first `{`
/// or `;`, or all.
fn head_toks(pf: &crate::parser::ParsedFile, lo: u32, hi: u32, sig: bool) -> Vec<&[u8]> {
    if !sig {
        return toks(pf, lo, hi, &[]);
    }
    let mut out = Vec::new();
    for t in pf.toks.iter_from(first_tok_at(&pf.toks, lo)) {
        match pf.src.get(t.lo as usize..t.hi as usize) {
            Some(x) if t.hi <= hi && x != b"{" && x != b";" => out.push(x),
            _ => break,
        }
    }
    out
}

fn adt_ext_field(a: &crate::tcx::AdtT, k: usize) -> bool {
    a.ext_from != u32::MAX && k as u32 >= a.ext_from
}

/// The fields of struct `d` (running slot order) that hold the niche of one of its compiled
/// instances: an outer enum (`Option<T>`) may keep its tag there.
fn niche_fields(u: &Unit, d: DefId) -> Vec<usize> {
    use crate::types::TyKind;
    let mut out = Vec::new();
    for i in 0..u.tcx.tys.len() {
        let t = TyId(i as u32);
        let TyKind::Adt(dd, args, _) = u.tcx.tys.kind(t) else { continue };
        if *dd != d || !u.tcx.tys.is_concrete(t) || u.tcx.tys.has_escaping_bound_vars(t) {
            continue;
        }
        let args = *args;
        let (fields, niche) = {
            let l = u.layout_of(t);
            (l.fields.clone(), l.niche.as_ref().map(|n| (n.off, n.bytes)))
        };
        let Some((n_off, n_bytes)) = niche else { continue };
        for (k, off) in fields.into_iter().enumerate() {
            if off == u32::MAX || out.contains(&k) {
                continue;
            }
            let ft = u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, 0, k as u32, u.tcx.tys.list(args));
            let size = u.layout_of(ft).size;
            if off < n_off + n_bytes && n_off < off + size {
                out.push(k);
            }
        }
    }
    out
}

/// Use the same formatter as lower.rs's `type_name` intrinsic. Generic structs publish
/// each concrete instantiation known to the session, matching the widget registry's keys.


/// Crates a layout edit of this run was refused in only because they are outside the app scope
/// (layout_scope_refusal): the session's next run lays them out for live edits, with this run's
/// (live_crates_next, MAKEPAD_RUN_LIVE_CRATES).
static LIVE_CRATES_NEXT: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The crates outside the app scope the session's next run lays out for live edits: this run's
/// (layout::live_crates) and those a refusal of this run added.
pub(crate) fn live_crates_next() -> Vec<String> {
    let mut all = crate::layout::live_crates();
    for c in LIVE_CRATES_NEXT.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        if !all.contains(c) {
            all.push(c.clone());
        }
    }
    all
}

/// The restart reason of a layout edit of `what` `name`, an ADT of crate `krate` outside the app
/// scope: the crate goes to the next run (live_crates_next), whose edits of it apply live.
fn layout_scope_refusal(u: &Unit, krate: u32, what: &str, name: &str) -> String {
    let root = crate::vfs::normalize_path(u.prog.crates[krate as usize].root_path());
    {
        let _k = crate::zone_alloc::kept();
        let mut next = LIVE_CRATES_NEXT.lock().unwrap_or_else(|e| e.into_inner());
        if !next.contains(&root) {
            next.push(root);
        }
    }
    format!("{what} `{name}` kept its first layout: its crate `{}` is outside the app scope; the restart lays the crate out for live edits, and its next edits apply live -- restart the app",
        u.prog.syms.str(u.prog.crates[krate as usize].name))
}

/// Why struct `d` (its running definition `adt`) cannot take a layout edit in place, naming the
/// condition it fails (Layouts::live_editable): a provided crate's, its form, its crate's scope,
/// its Copy impl.
fn struct_layout_refusal(u: &Unit, d: DefId, adt: &crate::tcx::AdtT, name: &str) -> String {
    let krate = u.prog.def(d).krate;
    if u.prog.crates[krate as usize].rustc_laid {
        return format!("struct `{name}` is a provided crate's, laid out as rustc lays it (its values are rustc's bytes): it keeps its first layout -- restart the app");
    }
    if !crate::layout::Layouts::struct_form(adt) {
        return format!("struct `{name}` keeps its first layout in a live session only with named fields and Rust's repr -- restart the app");
    }
    if !crate::layout::live_laid(&u.prog, krate) {
        return layout_scope_refusal(u, krate, "struct", name);
    }
    format!("struct `{name}` is Copy, and a bitwise copy would share the extension block of added fields: it keeps its first layout -- restart the app")
}

/// An enum's edit in a live session (M10): the running variants keep their names, fields and
/// order among themselves, and a variant may be added anywhere (or a discriminant change). Every
/// value keeps its representation -- live enums are always tagged and give outer types no niche
/// (layout.rs) -- so a running variant keeps its tag by name and an added one takes a tag no
/// running value holds (AdtT::live_tags; the discriminants are the new text's, decoded from the
/// tags). The layout check is the size, alignment, tag and every running variant's field offsets
/// (enum_fits, once the new definition is in); what running code made of discriminants that move
/// is asked before the commit (enum_numbers_read).
fn enum_edit(u: &Unit, d: DefId, old: &crate::tcx::AdtT, fresh: &crate::tcx::AdtT, running_discriminants: &[u128], fresh_discriminants: &[u128]) -> Result<(crate::tcx::AdtT, Vec<String>), String> {
    let name = u.prog.def_path(d);
    if !fresh.is_enum || fresh.n_generics != old.n_generics {
        return Err(format!("enum `{}`: its kind or its generics changed -- restart the app", name));
    }
    if !u.lay.live_editable_enum(&crate::layout::LayoutCx::new(&u.prog), d, old) {
        let krate = u.prog.def(d).krate;
        return Err(if u.prog.crates[krate as usize].rustc_laid {
            format!("enum `{}` is a provided crate's, laid out as rustc lays it (its values are rustc's bytes): it keeps its first layout -- restart the app", name)
        } else if !crate::layout::Layouts::enum_form(old) {
            format!("enum `{}` keeps its first layout in a live session only with Rust's repr -- restart the app", name)
        } else {
            layout_scope_refusal(u, krate, "enum", &name)
        });
    }
    let at = running_variants_at(old, fresh).map_err(|v| match v {
        Some(v) => format!("enum `{}`: variant `{}` was removed; a running value may hold it{} -- restart the app", name, u.prog.syms.str(v), holders_note(u, d)),
        None => format!("enum `{}`: its running variants changed their order; values kept in that order (a sorted list, a `BTreeMap`) would be out of it -- restart the app", name),
    })?;
    for (vo, &j) in old.variants.iter().zip(&at) {
        let vn = &fresh.variants[j];
        let same_fields = vo.fields.len() == vn.fields.len() && vo.fields.iter().zip(vn.fields.iter()).all(|(a, b)| a.name == b.name && a.ty == b.ty);
        if vo.shape != vn.shape || !same_fields {
            return Err(format!("enum `{}`: variant `{}` changed its fields; running values would be read as the old ones -- restart the app", name, u.prog.syms.str(vo.name)));
        }
    }
    let n = old.variants.len();
    let signed = |v: u128| crate::tcx::Discr { val: v, ty: crate::program::Prim::Isize, bytes: u.tcx.tys.ptr_bytes }.signed_value();
    // a derived `PartialOrd`/`Ord` orders variants by their discriminants: the running ones keep
    // theirs in the same order (a written-out discriminant can change it in place)
    let mut by_old: Vec<usize> = (0..n).collect();
    by_old.sort_by_key(|&i| signed(running_discriminants[i]));
    if let Some(w) = by_old.windows(2).find(|w| signed(fresh_discriminants[at[w[0]]]) >= signed(fresh_discriminants[at[w[1]]])) {
        return Err(format!("enum `{}`: the running variants' discriminants change their order (`{}` is no longer below `{}`); values kept in that order (a sorted list, a `BTreeMap`) would be out of it -- restart the app", name, u.prog.syms.str(old.variants[w[0]].name), u.prog.syms.str(old.variants[w[1]].name)));
    }
    let unmoved = at.iter().enumerate().all(|(i, &j)| i == j) && running_discriminants[..n] == fresh_discriminants[..n];
    if fresh.variants.len() == n && unmoved && old.live_tags.is_empty() {
        return Ok((old.clone(), Vec::new()));
    }
    let old_tags: Vec<u128> = if old.live_tags.is_empty() { running_discriminants.to_vec() } else { old.live_tags.clone() };
    let wide = |v: u128| v as i64 as i128 != signed(v);
    let tags = match kept_tags(&old_tags, &at, fresh.variants.len()) {
        Some(tags) if !tags.iter().chain(fresh_discriminants).any(|&v| wide(v)) => tags,
        _ => return Err(format!("enum `{}`: a discriminant does not fit 64 bits -- restart the app", name)),
    };
    let mut adt = fresh.clone();
    adt.live_tags = if tags.as_slice() == fresh_discriminants { Vec::new() } else { tags };
    let notes = fresh.variants.iter().enumerate().filter(|(j, _)| !at.contains(j)).map(|(_, v)| format!("variant `{}` added to `{}`", u.prog.syms.str(v.name), name)).collect();
    Ok((adt, notes))
}

/// Live sessions (M10): whether a value of type `t` can hold a value of one of the ADTs `s`, by
/// value or behind a pointer, as far as its type shows: through ADT arguments, references,
/// pointers, arrays, slices, tuples, fn pointers and fn items (their signatures), closures (their
/// captures and signature), a trait object's arguments, an alias's arguments, and an opaque
/// type's hidden type (as its defining fn's check found it; one not found counts as holding one).
/// Trait objects made from one are asked apart (no_live_values: the vtables).
fn ty_holds(u: &Unit, t: TyId, s: &[DefId], depth: u32) -> bool {
    use crate::types::TyKind::*;
    if depth > 24 {
        return true;
    }
    let tys = &u.tcx.tys;
    let any = |l: crate::types::TyList| tys.list(l).iter().any(|a| ty_holds(u, *a, s, depth + 1));
    match tys.kind(t).clone() {
        Adt(d, args, _) => s.contains(&d) || any(args),
        Ref(_, x, _) | Ptr(_, x) | Slice(x) | Array(x, _) => ty_holds(u, x, s, depth + 1),
        Tuple(v) => any(v),
        FnPtr(ps, r, _, _) => any(ps) || ty_holds(u, r, s, depth + 1),
        FnDef(d, args, _) => any(args) || u.tcx.fn_item_sig(&u.prog, d).is_some_and(|g| g.params.iter().chain([&g.ret]).any(|p| ty_holds(u, *p, s, depth + 1))),
        Closure(_, _, c) => [c.parent_args, c.closure_sig_as_fn_ptr_ty, c.tupled_upvars_ty].into_iter().any(|x| ty_holds(u, x, s, depth + 1)),
        Dyn(_, args, bounds, ..) => any(args) || tys.list(bounds).iter().any(|b| ty_holds(u, b.1, s, depth + 1)),
        Alias(_, a) => any(a.args) || match a.kind {
            crate::types::AliasTyKind::Opaque { def_id } => opaque_hidden(u, def_id).map_or(true, |h| ty_holds(u, h, s, depth + 1)),
            _ => false,
        },
        _ => false,
    }
}

/// The hidden type of opaque type `o`, as the check of the fn defining it found it.
fn opaque_hidden(u: &Unit, o: DefId) -> Option<TyId> {
    let mut f = u.prog.def(o).parent;
    for _ in 0..8 {
        if f == crate::program::NO_DEF {
            return None;
        }
        if matches!(u.prog.def(f).kind, DefKind::Fn | DefKind::AssocFn) {
            break;
        }
        f = u.prog.def(f).parent;
    }
    if !matches!(u.prog.def(f).kind, DefKind::Fn | DefKind::AssocFn) || !def_is_bound(&u.prog, f) {
        return None;
    }
    u.tcx.typeck(&u.prog, f).hidden_types.iter().find(|(k, _)| **k == o).map(|(_, t)| *t)
}

/// Live sessions (M10): struct `d` with the ADTs whose fields hold it (ty_holds), and theirs: the
/// types whose layout follows `d`'s, and every type a value of `d` can be held in.
fn holding_adts(u: &Unit, d: DefId) -> Vec<DefId> {
    let mut s = vec![d];
    loop {
        let n = s.len();
        for (a, adt) in u.tcx.adts.iter() {
            if !s.contains(a) && adt.variants.iter().any(|v| v.fields.iter().any(|f| ty_holds(u, f.ty, &s, 0))) {
                s.push(*a);
            }
        }
        if s.len() == n {
            return s;
        }
    }
}

/// Live sessions (M10): a struct whose layout can change in place, because no running value of it
/// is left (no_live_values) and every user of its layout is rebuilt with the edit (fresh_users):
/// one with the form of a live-editable struct that is Copy (a bitwise copy would share the
/// extension block, so it cannot keep its first layout), and no generics.
fn fresh_layout_struct(u: &Unit, d: DefId) -> bool {
    let Some(adt) = u.tcx.adts.get(&d) else { return false };
    let lcx = crate::layout::LayoutCx::new(&u.prog);
    if adt.n_generics != 0 || !u.lay.live_editable_form(&lcx, d, adt) {
        return false;
    }
    let t = u.tcx.tys.intern(crate::types::TyKind::Adt(d, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
    crate::layout::Layouts::is_copy(&lcx, &u.tcx, t) == Some(true)
}

/// Live sessions (M10), for a struct that takes its new layout (fresh_layout_struct; `s`: it and
/// the ADTs holding it, holding_adts): where a running value of it may be, as the restart's reason.
/// A static or thread-local the run knows (placed, or with its place reserved) whose type can hold
/// one; a trait object made from a type that can (its vtable, made this run: the object may be
/// alive).
/// Values behind pointers converted to other types, and frames, are asked apart (raw_builder; the
/// frames at the install). None: no running value of it is left.
fn no_live_values(u: &Unit, d: DefId, s: &[DefId]) -> Option<String> {
    let name = def_name(&u.prog, d);
    let held = |t: TyId| ty_holds(u, t, s, 0);
    let mut at: Vec<DefId> = u.statics.keys().chain(u.tls_places.keys()).copied().collect();
    at.sort_unstable_by_key(|s| s.0);
    for st in at {
        if u.tcx.const_ty(&u.prog, st).map_or(true, held) {
            let mut n = st;
            while u.prog.syms.str(u.prog.def(n).name).starts_with("__") && u.prog.def(n).parent != crate::program::NO_DEF {
                n = u.prog.def(n).parent;
            }
            return Some(format!("struct `{}` changed its layout, and `{}` may hold values of it in the old one -- restart the app", name, u.prog.def_path(n)));
        }
    }
    if let Some((src, _)) = u.vtables.snapshot().into_iter().map(|(k, _)| k).find(|(src, _)| held(*src)) {
        return Some(format!("struct `{}` changed its layout, and the running code made trait objects of `{}`, which may hold values of it in the old one -- restart the app", name, crate::typeck::type_name(&u.prog, &u.tcx, src)));
    }
    None
}

/// Live sessions (M10): whether the code of `f` (its signature and its checked body: the types
/// of its expressions and patterns, and the generic arguments of its paths and method calls) can
/// handle a value of one of `s` (ty_holds): it is built for their layouts. Some: the first such
/// type.
fn body_holds(u: &Unit, f: DefId, s: &[DefId]) -> Option<TyId> {
    let held = |t: &TyId| ty_holds(u, *t, s, 0);
    if let Some(t) = u.tcx.fn_sig(&u.prog, f).and_then(|g| g.params.iter().chain([&g.ret]).copied().find(held)) {
        return Some(t);
    }
    let body = u.tcx.typeck(&u.prog, f);
    let mut seen: crate::fxhash::HashSet<TyId> = Default::default();
    let mut ask = |t: &TyId| seen.insert(*t) && held(t);
    body.expr_ty.iter().chain(body.pat_ty.iter()).copied().find(&mut ask)
        .or_else(|| body.res_args.values().chain(body.methods.values().map(|m| &m.args)).find_map(|l| u.tcx.tys.list(*l).iter().copied().find(&mut ask)))
}

/// The statics and thread-locals of the running program whose type holds `d` (where a value of
/// it may live), for a note: ` (`A`, `B` hold values of it)`, or empty.
fn holders_note(u: &Unit, d: DefId) -> String {
    // (a `thread_local!`'s storage is a static of its expansion: named by the item it declares)
    let named = |mut s: DefId| {
        while u.prog.syms.str(u.prog.def(s).name).starts_with("__") && u.prog.def(s).parent != crate::program::NO_DEF {
            s = u.prog.def(s).parent;
        }
        def_name(&u.prog, s)
    };
    let mut names: Vec<String> = u.statics.keys().chain(u.tls_places.keys()).filter(|&&s| u.tcx.const_ty(&u.prog, s).is_some_and(|t| ty_mentions(u, t, &[d], 0))).map(|&s| named(s)).collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        return String::new();
    }
    let more = names.len() > 4;
    names.truncate(4);
    format!(" (`{}`{} hold{} values of it)", names.join("`, `"), if more { " and others" } else { "" }, if names.len() == 1 && !more { "s" } else { "" })
}

/// The tags of an enum's `n` variants after a live edit (M10): running variant `i`, now variant
/// `at[i]`, keeps its tag `old_tags[i]` (the value its running values hold); an added one takes the
/// next tag none of them holds, from 0 up. None: no tag is left.
fn kept_tags(old_tags: &[u128], at: &[usize], n: usize) -> Option<Vec<u128>> {
    let mut next = old_tags.iter().map(|&t| t as i64).max().map_or(0, |m| m.max(-1)).checked_add(1)?;
    let mut tags: Vec<Option<u128>> = vec![None; n];
    for (i, &j) in at.iter().enumerate() {
        *tags.get_mut(j)? = Some(*old_tags.get(i)?);
    }
    tags.into_iter().map(|t| match t {
        Some(t) => Some(t),
        None => {
            let t = next as u128;
            next = next.checked_add(1)?;
            Some(t)
        }
    }).collect()
}

#[cfg(test)]
mod kept_tags_tests {
    use super::kept_tags;

    #[test]
    fn a_running_variant_keeps_its_tag_and_an_inserted_one_takes_a_free_one() {
        // A B C, then X inserted before B: A 0, X 3, B 1, C 2
        assert_eq!(kept_tags(&[0, 1, 2], &[0, 2, 3], 4), Some(vec![0, 3, 1, 2]));
        // then Z inserted first: the running tags stay, Z takes 4
        assert_eq!(kept_tags(&[0, 3, 1, 2], &[1, 2, 3, 4], 5), Some(vec![4, 0, 3, 1, 2]));
        // appended: the tags are the discriminants
        assert_eq!(kept_tags(&[0, 1], &[0, 1], 3), Some(vec![0, 1, 2]));
        // negative discriminants (-2, -1 as the tag's bits): an added variant's tag is no running one's
        let m = |v: i64| v as u64 as u128;
        assert_eq!(kept_tags(&[m(-2), m(-1)], &[1, 2], 3), Some(vec![0, m(-2), m(-1)]));
        // a running variant placed where the new list has none
        assert_eq!(kept_tags(&[0, 1], &[0, 5], 3), None);
    }
}

/// Where each running variant of `old` is in `fresh`, by name; Err(Some(name)): one is gone,
/// Err(None): they changed their order among themselves.
fn running_variants_at(old: &crate::tcx::AdtT, fresh: &crate::tcx::AdtT) -> Result<Vec<usize>, Option<crate::program::Sym>> {
    let mut at = Vec::with_capacity(old.variants.len());
    for vo in &old.variants {
        match fresh.variants.iter().position(|vn| vn.name == vo.name) {
            Some(j) => at.push(j),
            None => return Err(Some(vo.name)),
        }
    }
    if at.windows(2).any(|w| w[0] >= w[1]) {
        return Err(None);
    }
    Ok(at)
}

/// The layout of every compiled instance of enum `d` before (`before`) and after an edit: the
/// running values keep their representation (size, alignment, tag, running variants' fields;
/// `at`: where each running variant is now, running_variants_at).
fn enum_fits(before: &[(TyId, crate::layout::Layout)], u: &Unit, at: &[usize]) -> Result<(), String> {
    for (t, b) in before {
        let a = u.layout_of(*t);
        let moved = b.variant_fields.iter().enumerate().any(|(i, x)| at.get(i).and_then(|&j| a.variant_fields.get(j)) != Some(x));
        if a.size != b.size || a.align != b.align || a.tag != b.tag || a.enc != b.enc || moved {
            return Err(format!("enum `{}`: the added variant does not fit the running values' layout ({} -> {} bytes) -- restart the app", crate::typeck::type_name(&u.prog, &u.tcx, *t), b.size, a.size));
        }
    }
    Ok(())
}

/// The variant names of enum item `it` (with whether one has an explicit discriminant), or None.
fn enum_variant_names<'a>(pf: &'a crate::parser::ParsedFile, it: &crate::ast::Item) -> Option<Vec<(&'a [u8], bool)>> {
    let crate::ast::ItemKind::Enum(r) = &it.kind else { return None };
    Some(pf.ast.list(pf.ast.row(*r).variants).iter().map(|v| (pf.text(v.name).as_bytes(), v.disc.is_some())).collect())
}

/// Live sessions (M10), before the commit of an edit of enum `d` that moves where its running
/// variants are or a discriminant (a variant added before running ones; a discriminant written
/// out): code that ran and read them as numbers -- an `as` cast, `mem::discriminant`,
/// `discriminant_value`, a derived `Hash` (it hashes the discriminant) -- may have kept what
/// it made (a hash table's keys, an index), which the new numbers read differently: the restart
/// rung, naming it. A derived `PartialEq`, `PartialOrd` or `Ord` of the enum compares them only:
/// equal stays equal and the running variants keep their order, by position and by discriminant
/// (enum_edit), and its new text is rebuilt. None: nothing that ran reads them, or the edit keeps them.
fn enum_numbers_read(u: &Unit, d: DefId, versions: &[(crate::program::FileVersion, Plan)], visible: &[bool]) -> Option<String> {
    let old = u.tcx.adts.get(&d)?;
    let mut new: Option<Vec<(&[u8], bool)>> = None;
    for (v, plan) in versions {
        for &(i, j) in &plan.pairs {
            if u.prog.item_defs.get(&(v.old, i)) == Some(&d) {
                new = enum_variant_names(&v.pf, &v.pf.ast.items[j as usize]);
            }
        }
    }
    let new = new?;
    // (a running variant gone or out of order: enum_edit's restart at the commit)
    let at: Vec<usize> = old.variants.iter().map(|vo| new.iter().position(|(m, _)| *m == u.prog.syms.str(vo.name).as_bytes())).collect::<Option<_>>()?;
    let positions_move = at.iter().enumerate().any(|(i, &j)| i != j);
    let written = new.iter().any(|v| v.1) || old.variants.iter().any(|v| matches!(v.discr, crate::tcx::VariantDiscr::Explicit(_)));
    if !positions_move && !written {
        return None;
    }
    let name = def_name(&u.prog, d);
    let fieldless = old.variants.iter().all(|v| v.fields.is_empty());
    let prog = &u.prog;
    let is_it = |t: TyId| matches!(u.tcx.tys.kind(t), crate::types::TyKind::Adt(a, _, _) if *a == d);
    // the impl `f` is a method of, when it is a derived one of this enum: its trait's name
    let derived_of_it = |f: DefId| -> Option<&'static str> {
        let imp = prog.def(f).parent;
        if imp == crate::program::NO_DEF || prog.def(imp).kind != DefKind::Impl || !relax_types::lint::is_automatically_derived(prog, imp) {
            return None;
        }
        if !u.tcx.impl_self.get(&imp).is_some_and(|&t| is_it(t)) {
            return None;
        }
        let tr = prog.syms.str(prog.def(*u.tcx.impl_trait.get(&imp)?).name);
        Some(["PartialEq", "PartialOrd", "Ord", "Hash"].into_iter().find(|k| *k == tr).unwrap_or("other"))
    };
    // `at_it`: the instance's type arguments name the enum
    let reads = |f: DefId, at_it: bool| -> bool {
        let pf = &prog.files[prog.def(f).file as usize];
        let it = pf.ast.item(prog.def(f).item);
        let text = toks(pf, it.lo, it.hi, &[]);
        let cast = fieldless && text.iter().any(|t| *t == b"as");
        let call = text.iter().any(|t| *t == b"discriminant_value" || *t == b"discriminant");
        if !cast && !call {
            return false;
        }
        if !def_is_bound(prog, f) {
            // code an earlier edit replaced: its text is all there is to go by
            return call || text.iter().any(|t| *t == name.as_bytes());
        }
        let body = u.tcx.typeck(prog, f);
        let bpf = &prog.files[body.file as usize];
        (0..body.expr_ty.len()).any(|i| {
            let e = body.expr_at(i);
            match bpf.ast.expr(e).kind {
                crate::ast::ExprKind::Cast(x, _) => cast && body.has_expr(x) && is_it(body.ty(x)),
                crate::ast::ExprKind::Call(callee, xs) if call => {
                    let named = matches!(body.res.get(&callee.0), Some(relax_types::typeck::Res::Def(c)) if matches!(prog.syms.str(prog.def(*c).name), "discriminant_value" | "discriminant"));
                    named && (at_it || bpf.ast.list(xs).first().is_some_and(|&x| body.has_expr(x) && ty_mentions(u, body.ty(x), &[d], 0)))
                }
                _ => false,
            }
        })
    };
    let moves = if positions_move { "a variant added before running ones moves their positions and discriminants" } else { "a discriminant written out may change" };
    let mut seen: crate::fxhash::HashSet<(DefId, bool)> = Default::default();
    for f in u.fns.iter() {
        let FnKey::Inst(fd, args) = &f.key else { continue };
        if !(f.compiled || f.rir.is_some() || !f.history().inlined_into.is_empty()) {
            continue;
        }
        let fd = *fd;
        if !visible.get(prog.def(fd).krate as usize).copied().unwrap_or(true) {
            // the library's `mem::discriminant` at it
            if matches!(prog.syms.str(prog.def(fd).name), "discriminant" | "discriminant_value") && args.iter().any(|t| ty_mentions(u, *t, &[d], 0)) {
                return Some(format!("enum `{}`: {}, and `{}` read them as numbers in the running code: what it made of them may be kept -- restart the app", name, moves, u.prog.def_path(fd)));
            }
            continue;
        }
        let at_it = args.iter().any(|t| ty_mentions(u, *t, &[d], 0));
        if !matches!(prog.def(fd).kind, DefKind::Fn | DefKind::AssocFn) || !seen.insert((fd, at_it)) {
            continue;
        }
        let derived = derived_of_it(fd);
        if derived == Some("Hash") {
            return Some(format!("enum `{}`: {}, and its derived `Hash` (which hashes the discriminant) ran: a hash table may hold the hashes it made -- restart the app", name, moves));
        }
        if matches!(derived, Some("PartialEq" | "PartialOrd" | "Ord")) || !reads(fd, at_it) {
            continue;
        }
        return Some(format!("enum `{}`: {}, and `{}` read them as numbers in the running code: what it made of them may be kept (a hash, an index) -- restart the app", name, moves, u.prog.def_path(fd)));
    }
    // a constant evaluated from them holds the old number
    for i in 0..prog.defs.len() {
        let c = DefId(i as u32);
        let def = prog.def(c);
        if !matches!(def.kind, DefKind::Const | DefKind::AssocConst | DefKind::AnonConst | DefKind::Static) || !visible.get(def.krate as usize).copied().unwrap_or(false) {
            continue;
        }
        let ran = if def.kind == DefKind::Static { u.statics.get(&c).is_some_and(|&a| a != 0) } else { u.tcx.const_was_evaluated(c) };
        if ran && reads(c, false) {
            return Some(format!("enum `{}`: {}, and `{}` was evaluated from them: its value holds the old number -- restart the app", name, moves, u.prog.def_path(c)));
        }
    }
    None
}

fn item_mentions(prog: &crate::program::Program, d: DefId, names: &[&[u8]]) -> bool {
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    let it = pf.ast.item(def.item);
    toks(pf, it.lo, it.hi, &[]).iter().any(|t| names.contains(t))
}

/// item_mentions, where an impl's item also names what its impl's header names: its `Self`
/// (a method's match on `Self` sees its enum's new variants).
fn item_or_impl_mentions(prog: &crate::program::Program, d: DefId, names: &[&[u8]]) -> bool {
    if item_mentions(prog, d, names) { return true; }
    let p = prog.def(d).parent;
    if p == crate::program::NO_DEF || prog.def(p).kind != DefKind::Impl { return false; }
    let pf = &prog.files[prog.def(p).file as usize];
    let it = pf.ast.item(prog.def(p).item);
    let hi = pf.src.get(it.lo as usize..it.hi as usize).and_then(|t| t.iter().position(|b| *b == b'{')).map_or(it.hi, |at| it.lo + at as u32);
    toks(pf, it.lo, hi, &[]).iter().any(|t| names.contains(t))
}

fn def_name(prog: &crate::program::Program, d: DefId) -> String {
    prog.syms.str(prog.def(d).name).to_string()
}

/// The checks `makepad check` and the app check at boot run on a type-checked body after its
/// types (check.rs body_errors, in its order): type privacy, then E0004 / E0005 and the binding
/// conflicts of `x @ pat`. For the crates whose errors refuse live edits (borrowck admit); run
/// mode's unsafety errors are typeck's own (type_queries check_body). An internal failure of the
/// pattern analysis reports nothing, as in a check.
fn mir_phase_errors(prog: &Program, tcx: &crate::tcx::Tcx, d: DefId) -> Vec<String> {
    if !crate::borrowck::event::admit(prog, prog.def(d).krate) { return Vec::new(); }
    let _quiet = crate::lint::Suppress::new();
    let body = tcx.typeck(prog, d);
    if !body.errors.is_empty() { return body.errors.to_vec(); }
    let privacy = relax_types::privacy_types::check_body(prog, tcx, &body);
    if !privacy.is_empty() { return privacy; }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| relax_types::pattern_analysis::check_body(prog, tcx, &body))) {
        Ok(r) if !r.errors.is_empty() => r.errors.into_iter().chain(r.at_patterns).collect(),
        Ok(r) => r.at_patterns,
        Err(_) => Vec::new(),
    }
}

/// RELOAD' (lead design, round 33): whether the running value an initializer slot made (an
/// executable discriminant, a constant whose value a type uses) may change with this
/// transaction, decided without running it. Its dependencies are
/// the transitive static callee set of its initializer slot -- each slot's direct calls before
/// inlining and the calls, fn values and thunks of its built RIR -- plus the constants a slot's
/// build read (their initializer slots, walked the same way), the statics it addresses and
/// the type arguments of every instance (a field-list edit of one changes what size_of and
/// field offsets fold to). Ok(true): one of them is in `changed`. Err: the set is not exactly
/// computable (an indirect call, a trait's associated constant, a RIR build failure).
unsafe fn initializer_deps_changed(up: *mut Unit, root: u32, changed: &[DefId], layout_defs: &[DefId]) -> Result<bool, String> {
    use relax_backend::rir::{Callee, Inst};
    let mut todo = vec![root];
    let mut seen: Vec<u32> = Vec::new();
    // the slot each one was reached from: an unknown dependency names the way it was reached
    let mut from: std::collections::HashMap<u32, u32> = Default::default();
    let path = |from: &std::collections::HashMap<u32, u32>, mut s: u32| -> String {
        let u = &*up;
        let mut names = vec![u.fns[s as usize].name.to_string()];
        while let Some(&p) = from.get(&s) {
            if names.len() > 12 { names.push("..".into()); break; }
            names.push(u.fns[p as usize].name.to_string());
            s = p;
        }
        names.reverse();
        names.join(" -> ")
    };
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
                FnKey::Closure(_, _, parent) | FnKey::Sandbox(parent) | FnKey::Plain(parent) => k = *parent,
            }
        };
        if hit { return Ok(true); }
        if u.fns[s as usize].rir.is_none() {
            jit::build_rir(up, s).map_err(|e| format!("`{}`: {}", (&*up).fns[s as usize].name, e))?;
        }
        let u = &*up;
        let Some(rir) = u.fns[s as usize].rir.clone() else { return Err(format!("`{}` has no RIR", u.fns[s as usize].name)) };
        fn reach(from: &mut std::collections::HashMap<u32, u32>, todo: &mut Vec<u32>, by: u32, c: u32) {
            from.entry(c).or_insert(by);
            todo.push(c);
        }
        for &c in &u.fns[s as usize].calls { reach(&mut from, &mut todo, s, c); }
        for i in rir.blocks.iter().flat_map(|b| &b.insts) {
            match i {
                // (SlotAddr is a frame slot of this code, not a fn slot)
                Inst::Call(Callee::Fn(c) | Callee::CFn(c, _), _, _) | Inst::FnAddr(_, c) => reach(&mut from, &mut todo, s, *c),
                Inst::Call(Callee::Indirect(_) | Callee::CIndirect(..), _, _) => return Err(format!("an indirect call in `{}`, reached as {}", u.fns[s as usize].name, path(&from, s))),
                Inst::Addr(_, sym) => match u.syms.kind(sym.id) {
                    Some(crate::syms::SymKind::Thunk(c)) => reach(&mut from, &mut todo, s, c),
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
            for args in keys { reach(&mut from, &mut todo, s, u.fn_id(FnKey::Inst(c, args))); }
        }
    }
    Ok(false)
}

/// Slots referenced by compiled code (calls and fn values: table words and thunks).
fn referenced_slots(relocs: &[relax_backend::rir::RelocAt], out: &mut Vec<u32>) {
    for r in relocs {
        match r.target {
            relax_backend::rir::Reloc::Slot(id) | relax_backend::rir::Reloc::Thunk(id) | relax_backend::rir::Reloc::Call(id) => out.push(id),
            _ => {}
        }
    }
}

type Code = (Vec<u8>, Vec<(u32, u64)>, Vec<relax_backend::rir::RelocAt>);

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

/// The line of an old closure that keeps its code (`why` its new version does not take over).
/// When a trait object was made of a value holding it (a boxed handler or callback, a spawned
/// thread's entry), values of it may be stored and run the old code: the edit waits for a restart
/// for it ([`HELD`]: the restart is offered and `session view` lists it). A closure only called
/// in its parent's frame is not run again once its parent's new code runs: that line only says so.
fn kept_closure_line(u: &Unit, slot: u32, why: &str) -> String {
    let FnKey::Closure(file, ce, parent) = u.fns[slot as usize].key else {
        return format!("kept: closure {} keeps its old code ({why})", u.fns[slot as usize].name);
    };
    let stored = u.vtables.snapshot().into_iter().any(|((src, _), _)| holds_closure(&u.tcx.tys, src, (file, ce)));
    if stored {
        format!("{HELD}stored values of a closure of `{}` ({}) keep its old code ({why})", u.fns[parent as usize].name, u.fns[slot as usize].name)
    } else {
        format!("kept: closure {} keeps its old code where it runs on ({why})", u.fns[slot as usize].name)
    }
}

/// Whether a value of type `t` holds a value of closure `at` (its file and expression): the
/// closure itself, or a field, element or capture of `t` that holds it.
fn holds_closure(tys: &crate::types::Types, t: TyId, at: (u32, u32)) -> bool {
    use crate::types::TyKind;
    match *tys.kind(t) {
        TyKind::Closure(file, ce, args) => (file, ce) == at || holds_closure(tys, args.tupled_upvars_ty, at),
        TyKind::Tuple(v) | TyKind::Adt(_, v, _) => tys.list(v).iter().any(|&x| holds_closure(tys, x, at)),
        TyKind::Array(e, _) | TyKind::Slice(e) => holds_closure(tys, e, at),
        _ => false,
    }
}

unsafe fn compile_guarded(up: *mut Unit, id: u32) -> Result<Code, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| jit::compile_fn(up, id))) {
        Ok(r) => r,
        Err(p) => {
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".to_string());
            if p.is::<crate::preempt::OwnerBusy>() || p.is::<crate::preempt::PublishBail>() {
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
    apply_mode(up, edits, &[], 1, false)
}

/// `apply`, with the texts of `sources` (files no running version has: a Rust file an include!
/// read) as the transaction's snapshots of them.
pub unsafe fn apply_with_sources(up: *mut Unit, edits: Vec<Edit>, sources: &[Edit]) -> Outcome {
    apply_mode(up, edits, sources, 1, false)
}

/// A live update of a running program: keep its changed units for the JIT
/// channel before applying. Offline classifier tests use `apply` directly.
pub unsafe fn apply_update(up: *mut Unit, edits: Vec<Edit>) -> Outcome {
    crate::hot_failure::begin(up, &edits);
    // Unloaded source files have no running version to pair. Their snapshots
    // still belong to this transaction: a new `mod` reads them while collecting
    // the edited parent, rather than a stale source-registry read from a refusal.
    let (sources, edits): (Vec<_>, Vec<_>) = edits.into_iter().partition(|e| e.file == u32::MAX);
    // A disk save/checkout is one transaction, including on retry. Never install
    // a parseable subset of a tree that is still being written.
    let out = apply_mode(up, edits, &sources, 0, false);
    crate::hot_failure::classified(&out.lines);
    if out.rung == Rung::Patch { crate::hot_failure::applied(out.group); }
    out
}

/// Final module text must pass in full: no partial parse or type-check fallback.
pub(crate) unsafe fn apply_full(up: *mut Unit, edits: Vec<Edit>) -> Outcome {
    apply_mode(up, edits, &[], 0, true)
}

unsafe fn apply_mode(up: *mut Unit, edits: Vec<Edit>, sources: &[Edit], partial: u8, module: bool) -> Outcome {
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
    if edits.is_empty() { return idle_outcome(None, "no loaded source items changed"); }
    let sources: Vec<_> = sources.iter().map(|source| (source.path.as_str(), std::sync::Arc::<[u8]>::from(source.src.as_slice()))).collect();
    let files: Vec<u32> = edits.iter().map(|e| e.file).collect();
    // the included files' contents the running code read, before this transaction's parse reads
    // them again (`classify`: an item whose include moved changed); restored unless it applies
    let late = crate::sip::late_file_records();
    let base = include_base(&late);
    // an earlier transaction panicked: the program's tables may be half-way through it, and no
    // edit applies to them again -- the restart that recovery offers applies this one
    if let Some(why) = unit_suspect() {
        return Outcome { failure: Some(makepad_remote::hot::Outcome::Panic), kept: 0, pending: false, rung: Rung::Restart, files, lines: vec![format!("not applied to a program an earlier edit left half-made ({why}): a process restart applies it")], names: Vec::new(), changed: 0, test_changes: None, bodies: Vec::new(), group: 0, ms: [0.0; 4], undo: None, work: [0; 5] };
    }
    // an internal compiler panic in the transaction refuses the edit with its message. Nothing
    // undoes what the transaction had changed of the program by then (its file versions, defs,
    // query inputs), so the unit is marked suspect: no edit applies to it again, and the
    // recovery is the next run, once the person takes the restart it offers (mark_unit_suspect).
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
                for context in contexts.values_mut() {
                    for (path, src) in &sources { *context = context.edited(path, src.clone()); }
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
                let current = (!module).then(|| prog.current_source_file_sets(|p| edits.iter().any(|e| prog.file_paths[e.file as usize] == p)));
                // every crate's copy of the file takes the edit: a --lib test root's harness crate
                // and the plain library its dev-dependencies link (M50). The other copies follow
                // the edits as given, whose outcome files stay at their positions.
                let mut twins = Vec::new();
                let mut out: Vec<Edit> = edits.into_iter().map(|mut e| {
                    if let Some((&last, others)) = current.as_ref().and_then(|c| c.get(&prog.file_paths[e.file as usize])).and_then(|f| f.split_last()) {
                        e.file = last;
                        twins.extend(others.iter().map(|&file| Edit { file, path: e.path.clone(), src: e.src.clone() }));
                    }
                    e
                }).collect();
                out.append(&mut twins);
                out
            };
            let cur = snap(&edits);
            (edits, cur)
        });
        loop {
            let (macro_table, epoch, mut snapshots) = cur;
            // Only owned snapshots are reachable here; never retain a Unit/Program reference
            // across the unlock. Lazy stubs and background/spec workers can compile meanwhile.
            trace_stage("snapshot");
            crate::program::share_import_edits(&mut snapshots, edits.iter().map(|e| e.src.as_slice()));
            let pre: Vec<Option<Result<crate::program::FileVersion, String>>> = edits.iter().zip(snapshots).map(|(e, s)| s.map(|s| s.parse(e.src.clone()))).collect();
            trace_stage("reparse");
            let step = call_traced("relock", || {
                trace_stage("relock");
                // Runtime module loads publish macro tables; rollback also changes the collection
                // epoch. Neither may make this snapshot stale while the parser runs off the unit.
                if std::sync::Arc::ptr_eq(&macro_table, &(*up).prog.macros) && epoch == (*up).tcx.collect_epoch.load(std::sync::atomic::Ordering::Acquire) {
                    drop(macro_table); // allow commit's Arc::make_mut to update an unshared table
                    return Ok(apply_locked(up, edits, pre, t0, partial, 8, module, &base));
                }
                // a module load or a rollback moved the program while this parsed: parse again
                // against it. Each pass follows such a move, and loads end (the parse itself
                // moves nothing), so the edit applies once they do instead of being refused.
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
            // the message and where the compiler panicked: the report names the spot
            let msg = crate::lifeline::panic_line(&*p);
            mark_unit_suspect(&format!("internal compiler error applying an edit: {msg}"));
            Outcome { failure: Some(makepad_remote::hot::Outcome::Panic), kept: 0, pending: false, rung: Rung::Restart, files, lines: vec![format!("internal compiler error applying the edit: {} -- makepad run bug, report it; the app runs on with its code, and a restart applies the edit", msg)], names: Vec::new(), changed: 0, test_changes: None, bodies: Vec::new(), group: 0, ms: [0.0; 4], undo: None, work: [0; 5] }
        }
    };
    if r.rung != Rung::Patch { crate::sip::restore_late_files(&late); }
    // a kept edit that read an included file anew (its versions' text does not show it)
    let read_anew = r.rung == Rung::Patch && (!sources.is_empty() || crate::sip::late_file_records().iter().any(|(c, n, h)| late.iter().any(|(c0, n0, h0)| c0 == c && n0 == n && h0 != h)));
    if read_anew {
        let mut g = INCLUDES_EDITED.lock().unwrap_or_else(|e| e.into_inner());
        for c in edited_crates() { if !g.contains(&c) { g.push(c); } }
    }
    crate::owner::call(crate::owner::Class::Program, || {
        (*up).reload_rir = None;
        jit::close_patch();
        // (a panic's transaction left the tables half-made: its crates stay edited)
        if !matches!(r.failure, Some(makepad_remote::hot::Outcome::Panic)) { settle_edited_crates(up); }
    });
    transaction.finish(&r);
    r
}

/// Live edit transactions so far: a field-view summary context kept by one transaction is
/// used by the next only when no other transaction ran in between.
static TRANSACTIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
thread_local! {
    /// The field-view summaries of the program as the last patched edit left it (the
    /// transaction number it was kept by).
    static FV_SESSION: std::cell::RefCell<Option<(u64, std::rc::Rc<crate::borrowck::field_views::SummaryCx>)>> = Default::default();
}
fn fv_session_take(transaction: u64) -> std::rc::Rc<crate::borrowck::field_views::SummaryCx> {
    match FV_SESSION.with(|s| s.borrow_mut().take()) {
        Some((kept, cx)) if kept + 1 == transaction => cx,
        _ => crate::borrowck::field_views::SummaryCx::new(),
    }
}
fn fv_session_keep(transaction: u64, cx: std::rc::Rc<crate::borrowck::field_views::SummaryCx>) {
    FV_SESSION.with(|s| *s.borrow_mut() = Some((transaction, cx)));
}

unsafe fn apply_locked(up: *mut Unit, edits: Vec<Edit>, mut pre: Vec<Option<Result<crate::program::FileVersion, String>>>, t0: Instant, partial: u8, retries: usize, module: bool, base: &IncludeBase) -> Outcome {
    let transaction = TRANSACTIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let session = unsafe { crate::jit::unit_session(up) };
    let options = &session.options;
    let u = &mut *up;
    let olds: Vec<u32> = edits.iter().map(|e| e.file).collect();
    // the paths the new texts are parsed under: their diagnostics name them (`makepad live`'s
    // steps are other files than the ones they replace)
    let edit_paths: Vec<String> = edits.iter().map(|e| e.path.clone()).collect();
    u.flush_cache_records();
    note_edited_crates(u, &olds);
    let cache_ahead0 = jit::CACHE_AHEAD.load(std::sync::atomic::Ordering::Relaxed);
    let mut out = Outcome { failure: None, kept: 0, pending: false, rung: Rung::Refuse, files: olds.clone(), lines: Vec::new(), names: Vec::new(), changed: 0, test_changes: None, bodies: Vec::new(), group: 0, ms: [0.0; 4], undo: None, work: [0; 5] };
    // 1-2: parse and classify every file; nothing changes yet
    let mut versions = Vec::new();
    let mut changes = Vec::new();
    let mut restart = Vec::new();
    let mut kept = None;
    // a whole save holds back its restart-only items alone (hold_back): its texts stay at hand. A
    // live test session takes its restart at once, the whole edit with it: nothing waits there
    let hold = partial == 0 && !module && retries > 0 && !crate::harness::live::enabled();
    for (i, e) in edits.iter().enumerate() {
        let initial = if partial == 2 {
            Err("incomplete".to_string())
        } else if let Some(p) = pre.get_mut(i).and_then(Option::take) {
            p
        } else { u.prog.parse_version(e.file, &e.path, e.src.clone()) };
        let parsed = initial.or_else(|err| {
            if partial != 0 && partial != HOLD_ROUND && (e.path.ends_with(".rs") || module) {
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
                match classify(&u.prog, e.file, &v.pf, &e.path, base) {
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
                    restart.push(format!("{}const fn `{}` changed: checking its users requires the new code -- restart the app", new_pos(u, &versions, def), def_name(&u.prog, def)));
                }
            }
        }
    }
    // a const that sized a type (array length, const generic argument) changes types
    for c in &changes {
        if let Change::Const(d) = c {
            if u.tcx.const_was_evaluated(*d) {
                restart.push(format!("{}const `{}` is used in a type (an array length or const generic argument): its users' types change -- restart the app", new_pos(u, &versions, *d), def_name(&u.prog, *d)));
            }
        }
    }
    // Code linked from the disk cache was lowered by an earlier run, and what it read (const_readers,
    // const_was_evaluated) is known only for code this run lowered. The keys show its readers: each
    // changed const's closure before the edit, to compare after it (a const with no node has no key).
    let linked_process = crate::cache::linked_code(up);
    let const_before = {
        let consts: Vec<DefId> = changes.iter().filter_map(|c| if let Change::Const(d) = c { Some(*d) } else { None }).collect();
        if consts.is_empty() { Vec::new() } else { crate::cache::const_closures(up, &consts) }
    };
    if linked_process {
        for (d, closure) in &const_before {
            if closure.is_none() {
                restart.push(if crate::cache::in_skipped_file(&u.prog, *d) {
                    format!("{}const `{}` changed in a module the boot skipped (loaded after it): this run's code from the disk cache is keyed by that module's text whole, not by its readers -- restart the app", new_pos(u, &versions, *d), def_name(&u.prog, *d))
                } else {
                    format!("{}const `{}` changed, and this run's code comes from the disk cache: no key shows its readers there -- restart the app", new_pos(u, &versions, *d), def_name(&u.prog, *d))
                });
            }
        }
    }
    // an initializer runs once: a static (or the storage a `thread_local!` key keeps) that holds
    // its running value never sees the new initializer, so the edit lands by a restart
    for c in &changes {
        if let Change::Const(d) | Change::StaticInit(d) = *c {
            if let Some(s) = placed_storage(u, d) {
                restart.push(format!("{}the initializer of `{}` changed, and `{}` already holds the value the old one made (an initializer runs once) -- restart the app", new_pos(u, &versions, d), def_name(&u.prog, d), def_name(&u.prog, s)));
            }
        }
    }
    // a laid-out link section is one block of the statics it had (sections.rs)
    for (v, _) in &versions {
        if let Some(why) = crate::sections::edit_moves_a_section(u, v.old, &v.pf) {
            restart.push(why);
        }
    }
    if !restart.is_empty() {
        if hold {
            if let Some(r) = hold_back(up, &edits, &restart, t0, retries, base) { return r; }
        }
        out.rung = Rung::Restart;
        out.lines = restart;
        out.ms[0] = ms(t0);
        return out;
    }
    out.bodies = changes.iter().filter_map(|c| match c { Change::Body(d) => Some(*d), _ => None }).collect();
    if changes.iter().all(|c| matches!(c, Change::Body(_))) && versions.iter().all(|(_, p)|
        p.added.is_empty() && p.removed.is_empty() && p.removed_uses.is_empty() && !p.attributes_changed) {
        out.test_changes = Some(out.bodies.clone());
    }
    let visible_crates = crates_seeing_edit(&u.prog, versions.iter().map(|(v, _)| v.old));
    // the structs the code this edit replaces writes in place through raw pointers (M10): that
    // code may still be running after the commit (raw_builder)
    let mut replaced_raw: Vec<(DefId, DefId, bool)> = Vec::new();
    let mut raw_gate: Option<RawGate> = None;
    for d in changes.iter().filter_map(|c| match *c { Change::Body(d) | Change::Const(d) | Change::StaticInit(d) => Some(d), _ => None })
        .chain(versions.iter().flat_map(|(_, p)| p.removed.iter().copied()))
    {
        let def = u.prog.def(d);
        if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn | DefKind::Const | DefKind::AssocConst | DefKind::Static) {
            continue;
        }
        let pf = &u.prog.files[def.file as usize];
        let it = pf.ast.item(def.item);
        let text = toks(pf, it.lo, it.hi, &[]);
        if raw_gate.get_or_insert_with(RawGate::new).can_write_raw(&u.prog, d, &text) {
            let (mut written, mut conv) = (Vec::new(), Vec::new());
            raw_written(u, d, &mut written, &mut conv);
            replaced_raw.extend(written.into_iter().map(|s| (s, d, false)).chain(conv.into_iter().map(|s| (s, d, true))));
        }
    }
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
        // a module more visible than before: what its name resolves to can change (a glob
        // import that now reaches it)
        if let Change::Vis(d) = c {
            set_names.push(def_name(&u.prog, *d).into_bytes());
        }
    }
    // names a removed or edited `use` bound: every fn naming one is type-checked against what
    // the name means now (M13)
    let mut use_names: Vec<Vec<u8>> = Vec::new();
    for (v, plan) in &versions {
        for &pos in &plan.removed_uses {
            for im in u.prog.imports.iter().filter(|im| im.file == v.old && im.pos == pos) {
                if let Some(b) = im.bind {
                    set_names.push(u.prog.syms.str(b).as_bytes().to_vec());
                    use_names.push(u.prog.syms.str(b).as_bytes().to_vec());
                }
            }
        }
    }
    // a struct's field-list edit (M10): a field that went, was renamed or changed its type leaves
    // its slot (layout_edit), and every fn using it is type-checked and rebuilt with the edit. An
    // added one, or one that only moved in the declaration, concerns only code that names it
    // (its text changed)
    let layout_defs: Vec<DefId> = changes.iter().filter_map(|c| if let Change::Layout(d) = c { Some(*d) } else { None }).collect();
    // where each is in the edit's text: a restart its running frames ask for names it, so that
    // hold_back keeps it alone at its running text
    let layout_pos: Vec<(DefId, String)> = layout_defs.iter().map(|&d| (d, new_pos(u, &versions, d))).collect();
    let mut moved_fields: Vec<(DefId, Vec<u32>)> = Vec::new();
    // the structs that take their new layout (fresh_layout_struct), each with the ADTs holding it
    let mut fresh_structs: Vec<(DefId, Vec<DefId>)> = Vec::new();
    // what the running values (or the numbers and constants code read) cannot take: a restart
    // decided after the commit, where the edit's text is type-checked -- its errors refuse it
    let mut late_restart: Vec<String> = Vec::new();
    for &d in &layout_defs {
        // an enum's matches see the new variants
        if u.prog.def(d).kind == DefKind::Enum {
            set_names.push(def_name(&u.prog, d).into_bytes());
            if let Some(line) = enum_numbers_read(u, d, &versions, &visible_crates) {
                late_restart.push(line);
            }
            continue;
        }
        if fresh_layout_struct(u, d) {
            let s = holding_adts(u, d);
            if let Some(line) = no_live_values(u, d, &s) {
                late_restart.push(format!("{}{line}", new_pos(u, &versions, d)));
            }
            fresh_structs.push((d, s));
        }
        let old_names: Vec<Vec<u8>> = u.tcx.adts.get(&d).map(|a| a.variants[0].fields.iter().map(|f| u.prog.syms.str(f.name).as_bytes().to_vec()).collect()).unwrap_or_default();
        // what the text shows; the commit compares the resolved types (layout_edit), and a field
        // that left its slot with no change here restarts there
        let (mut old_fields, mut new_fields) = (Vec::new(), Vec::new());
        for (v, plan) in &versions {
            for &(i, j) in &plan.pairs {
                if u.prog.item_defs.get(&(v.old, i)) != Some(&d) {
                    continue;
                }
                let old_pf = &u.prog.files[v.old as usize];
                old_fields.extend(struct_fields(old_pf, &old_pf.ast.items[i as usize]));
                new_fields.extend(struct_fields(&v.pf, &v.pf.ast.items[j as usize]));
            }
        }
        let stays = |n: &[u8]| n.starts_with(b"__relax_dead_") || new_fields.iter().any(|(m, t)| *m == n && old_fields.iter().any(|(o, ot)| *o == n && ot == t));
        // a field that left its slot: the fns that use it (resolved field uses on this struct,
        // found by type-checking the fns whose text can reach it), not every fn naming a
        // same-named thing
        let gone: Vec<u32> = old_names.iter().enumerate().filter(|(_, n)| !stays(n)).map(|(k, _)| k as u32).collect();
        moved_fields.push((d, gone.clone()));
        if !gone.is_empty() {
            let fields: Vec<&[u8]> = gone.iter().map(|&k| old_names[k as usize].as_slice()).collect();
            let names = through_macros(&u.prog, &visible_crates, &fields);
            let names: Vec<&[u8]> = names.iter().map(|n| n.as_slice()).collect();
            let types = renames_of(&u.prog, vec![def_name(&u.prog, d).into_bytes()]);
            let types: Vec<&[u8]> = types.iter().map(|n| n.as_slice()).collect();
            let type_macros = through_macros(&u.prog, &visible_crates, &types);
            let type_macros: Vec<&[u8]> = type_macros.iter().map(|n| n.as_slice()).collect();
            if let Some(line) = const_code_using(u, d, &gone, &names, &visible_crates, &changes) {
                late_restart.push(line);
            }
            let mut seen: crate::fxhash::HashSet<DefId> = Default::default();
            let mut named = 0usize;
            for f in u.fns.iter() {
            trace_count("slot_visits", 1);
                if let FnKey::Inst(fd, _) = &f.key {
                    if !visible_crates[u.prog.def(*fd).krate as usize] { continue; }
                    // (a fn an earlier edit replaced keeps its slots for code in flight; it is no
                    // longer the program's and is never checked against the new definitions)
                    // (code of a reader only ever inlined, as stored code holds it, has no build of its own)
                    if !(f.compiled || f.rir.is_some() || !f.history().inlined_into.is_empty()) || !seen.insert(*fd) || !matches!(u.prog.def(*fd).kind, DefKind::Fn | DefKind::AssocFn) || body_defs.contains(fd) || !def_is_bound(&u.prog, *fd) {
                        continue;
                    }
                    if !item_mentions(&u.prog, *fd, &names) && !may_update_struct(&u.prog, *fd, &types, &type_macros) {
                        continue;
                    }
                    named += 1;
                    // typeck resolves a field use (rustc: no field is reached through a generic);
                    // its body as typeck left it, never instantiated: a generic fn at its own
                    // parameters has no codegen answer (`<impl IntoIterator>::IntoIter`: NoSolution)
                    let body = u.tcx.typeck(&u.prog, *fd);
                    if body.field_uses.iter().any(|(ud, k)| *ud == d && gone.contains(k)) || body.struct_updates.contains(&d) {
                        body_defs.push(*fd);
                        field_deps += 1;
                    }
                }
            }
            field_named += named;
        }
    }
    // A struct taking its new layout: every fn whose code can handle its values (body_holds) is
    // type-checked and rebuilt with the edit (its text may not change: a fn building an outer
    // struct that holds it), and every constant that can hold them is evaluated again. The code
    // of other crates and generic instances over it are rebuilt after the commit (fresh_users).
    let fresh_held: Vec<DefId> = fresh_structs.iter().flat_map(|(_, s)| s.iter().copied()).collect();
    let mut fresh_consts: Vec<DefId> = Vec::new();
    if !fresh_structs.is_empty() {
        let mut seen: crate::fxhash::HashSet<DefId> = Default::default();
        for f in u.fns.iter() {
            let FnKey::Inst(fd, _) = &f.key else { continue };
            let fd = *fd;
            if !(f.compiled || f.rir.is_some() || !f.history().inlined_into.is_empty()) || !visible_crates[u.prog.def(fd).krate as usize] || !def_is_bound(&u.prog, fd) || !seen.insert(fd) {
                continue;
            }
            match u.prog.def(fd).kind {
                DefKind::Fn | DefKind::AssocFn if !body_defs.contains(&fd) => {
                    if let Some(t) = body_holds(u, fd, &fresh_held) {
                        if reload_trace() {
                            eprintln!("reload: `{}` handles `{}`, which holds a struct taking its new layout: rebuilt", u.prog.def_path(fd), crate::typeck::type_name(&u.prog, &u.tcx, t));
                        }
                        body_defs.push(fd);
                    }
                }
                DefKind::Const | DefKind::AssocConst => fresh_consts.push(fd),
                _ => {}
            }
        }
        let held = |c: DefId| u.tcx.const_ty(&u.prog, c).map_or(true, |t| ty_holds(u, t, &fresh_held, 0));
        fresh_consts.retain(|&c| held(c));
        for (c, _) in u.consts.keys() {
            if !fresh_consts.contains(c) && visible_crates[u.prog.def(*c).krate as usize] && held(*c) {
                fresh_consts.push(*c);
            }
        }
    }
    for (v, plan) in &versions {
        let na = &v.pf.ast;
        for &(j, at) in &plan.added {
            // a fn added to a trait impl names nothing new: the code that reaches the trait's
            // method calls it through the instances that resolve to it (after the commit)
            if matches!(at, Err(imp) if u.tcx.impl_trait.contains_key(&imp)) {
                continue;
            }
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
            // (a fn removed from a trait impl that stays: the same)
            if override_of(&u.prog, &u.tcx, d, &plan.removed).is_some() {
                continue;
            }
            set_names.push(def_name(&u.prog, d).into_bytes());
            let items: &[DefId] = match u.prog.def(d).kind {
                DefKind::Impl => &u.prog.impls[u.prog.def(d).sub as usize].items,
                // a method call names the method, not its trait
                DefKind::Trait => u.prog.traits.get(u.prog.def(d).sub as usize).map_or(&[], |v| v.as_slice()),
                _ => &[],
            };
            for &x in items {
                set_names.push(def_name(&u.prog, x).into_bytes());
            }
        }
        out.changed += plan.added.len() + plan.removed.len();
    }
    // A trait whose methods change (one added, removed, or replaced for a new signature or a
    // default body gained or lost): its vtable is its methods in their order
    // (own_existential_vtable_entries). A trait object the running code made holds the old one,
    // which the new code would index by the new order: that edit restarts. Otherwise every
    // vtable of it is made anew, and the code calling through one is rebuilt with the new order:
    // every fn naming a method whose entry can move -- all of its trait's when one goes (those
    // after it move up), and those of the traits it is a supertrait of (their entries follow its
    // own). An added method goes after the others, which keep their entries.
    let edited_traits: Vec<DefId> = {
        let mut t: Vec<DefId> = Vec::new();
        for (_, plan) in &versions {
            t.extend(plan.added.iter().filter_map(|&(_, at)| at.err()).filter(|&td| u.prog.def(td).kind == DefKind::Trait));
            t.extend(plan.removed.iter().map(|&d| u.prog.def(d).parent).filter(|&td| td != crate::program::NO_DEF && u.prog.def(td).kind == DefKind::Trait && !plan.removed.contains(&td)));
        }
        t.sort_unstable();
        t.dedup();
        t
    };
    for &td in &edited_traits {
        let gone: Vec<DefId> = versions.iter().flat_map(|(_, p)| p.removed.iter().copied().filter(|&d| u.prog.def(d).parent == td)).collect();
        let methods_go = !gone.is_empty();
        if let Some(&d) = gone.iter().find(|&&d| u.prog.associated_types_for_impl_traits.get(&d).is_some_and(|v| !v.is_empty())) {
            restart.push(format!("`{}` returns `impl Trait` or a future: removing or replacing it in a trait is not applied live yet -- restart the app", u.prog.def_path(d)));
        }
        let subs: Vec<DefId> = u.tcx.trait_closure.iter().filter(|(k, v)| **k != td && v.contains(&td)).map(|(k, _)| *k).collect();
        let lowered = u.vtables.snapshot();
        let mut held = false;
        for (&(src, dy), _) in &lowered {
            if let crate::types::TyKind::Dyn(p, ..) = u.tcx.tys.kind(dy) {
                if *p == td || u.tcx.trait_closure.get(p).is_some_and(|v| v.contains(&td)) {
                    restart.push(format!("trait `{}` changes its methods, and trait objects of it (`{}` as `{}`) the running code made hold its old method table -- restart the app", u.prog.def_path(td), crate::syms::stable_ty(u, src), crate::syms::stable_ty(u, dy)));
                    held = true;
                    break;
                }
            }
        }
        // the vtables of code linked from the disk cache or placed from the startup image are
        // not in Unit::vtables: their names say whose they are (a warm run patched the edit, and
        // a reader built after it called the new method table's entries on the old one)
        if !held {
            let lowered: std::collections::HashSet<u64> = lowered.values().copied().collect();
            let heads: Vec<String> = std::iter::once(td).chain(subs.iter().copied()).map(|t| format!("dyn {}", u.prog.def_path(t))).collect();
            for (a, n) in u.syms.vtable_objs() {
                if lowered.contains(&a) { continue; }
                let Some((_, dy)) = vtable_name_types(&n) else { continue };
                if heads.iter().any(|h| dy.strip_prefix(h.as_str()).is_some_and(|r| r.is_empty() || r.starts_with(['<', ' ']))) {
                    restart.push(format!("trait `{}` changes its methods, and trait objects of it (`{}`) the running code made hold its old method table -- restart the app", u.prog.def_path(td), vtable_types(&n)));
                    break;
                }
            }
        }
        let mut moved: Vec<DefId> = subs.clone();
        if methods_go {
            moved.push(td);
        }
        for t in moved {
            for &m in u.prog.traits.get(u.prog.def(t).sub as usize).map_or(&[][..], |v| v.as_slice()) {
                if u.prog.def(m).kind == DefKind::AssocFn {
                    set_names.push(def_name(&u.prog, m).into_bytes());
                }
            }
        }
    }
    set_names.retain(|n| !n.is_empty());
    // The fns the edit's names reach are type-checked and rebuilt. What a rebuilt fn's body makes
    // must then be new too (rustc builds it from the new types; here the running values of the
    // old ones live on):
    // - its closures and coroutines (async blocks): their identity is their file version, and
    //   their code is where the names are used. A rebuilt fn of a file the edit does not touch
    //   gets a new version of that file (the same text), so its closures and async blocks are new
    //   types with their own slots, layouts and vtables, built from the new body; values the old
    //   code made keep their own code (closures of the same shape take the new one, below).
    // - the type it returns, when that is opaque (an async fn's coroutine: its state holds the
    //   futures it awaits): the fn is replaced like one whose signature changed, and its callers
    //   are rebuilt against the new one, to a fixpoint.
    // (Previously the closures of such a fn kept their compiled code: an async block that awaited
    // an edited async fn went on calling the replaced fn, and the patch reported success.)
    let std_crates: Vec<u32> = (0..u.prog.crates.len() as u32).filter(|&c| matches!(u.prog.syms.str(u.prog.crates[c as usize].name), "core" | "alloc" | "std" | "proc_macro")).collect();
    // A removed impl: its fns' slots stay for the code in flight, and the code that calls them
    // is checked and rebuilt with the edit. That is the fns naming its items (by name, above)
    // and every caller the running code has: operators, `{:?}`, other types' derives and generic
    // code reach a trait impl without naming it (rustc: the obligations a body proved). The
    // callers are the slots whose code calls one of its fns, takes its address or inlined it.
    // One that is generic code at the impl's type (`<Vec<T> as Clone>::clone` at `T = X`,
    // `fmt::Argument::new_debug::<X>`) is followed to its own callers, up to the code that chose
    // the type: that code is checked (it errs if it still needs the impl; otherwise its new code
    // does not reach it), and the generic code is rebuilt only when another impl takes the
    // removed one's place (at the commit). A trait object holding one of its fns was made by
    // code no call shows: that edit restarts, unless the impl's type goes too (then every value
    // of it is made by code naming it).
    let removed_all: Vec<DefId> = versions.iter().flat_map(|(_, p)| p.removed.iter().copied()).collect();
    let gone_impls: Vec<DefId> = removed_all.iter().copied().filter(|d| u.prog.def(*d).kind == DefKind::Impl).collect();
    // the removed impls with no items (the commit takes a removed impl's items out of its list)
    let empty_gone: Vec<DefId> = gone_impls.iter().copied().filter(|&imp| u.prog.impls[u.prog.def(imp).sub as usize].items.is_empty()).collect();
    let mut impl_over: Vec<u32> = Vec::new();
    if !gone_impls.is_empty() {
        let slots_of_impls = |u: &Unit, impls: &[DefId]| -> std::collections::HashSet<u32> {
            let fns = impls.iter().flat_map(|&imp| u.prog.impls[u.prog.def(imp).sub as usize].items.iter().copied()).filter(|d| u.prog.def(*d).kind == DefKind::AssocFn);
            let mut set: std::collections::HashSet<u32> = u.slots_of_defs(fns).into_iter().collect();
            let mut parents: Vec<u32> = set.iter().copied().collect();
            while !parents.is_empty() {
                parents = u.closures_of_slots(parents).into_iter().filter(|c| set.insert(*c)).collect();
            }
            set
        };
        let self_adts = |u: &Unit, imp: DefId| -> Vec<DefId> {
            let mut a = Vec::new();
            if let Some(&t) = u.tcx.impl_self.get(&imp) { ty_adts(u, t, &mut a, 0); }
            a
        };
        let mut adts: Vec<DefId> = Vec::new();
        let mut blanket = false;
        for &imp in &gone_impls {
            let a = self_adts(u, imp);
            blanket |= a.is_empty();
            adts.extend(a);
        }
        let kept: Vec<DefId> = gone_impls.iter().copied().filter(|&imp| { let a = self_adts(u, imp); a.is_empty() || !a.iter().all(|d| removed_all.contains(d)) }).collect();
        // the trait objects the running code made (every vtable: lowered, linked from the disk
        // cache, placed from the startup image) and the methods each one calls (its drop glue is
        // no method: a coercion to `dyn` needs no `Drop` impl)
        let vts: Vec<(u64, String, Vec<u32>)> = if kept.is_empty() { Vec::new() } else {
            u.syms.vtable_objs().into_iter().filter_map(|(a, n)| {
                let mut t = Vec::new();
                crate::syms::data_thunks(u, a, &mut t);
                t.retain(|&s| !matches!(u.fns[s as usize].key, FnKey::Glue(..)));
                (!t.is_empty()).then_some((a, n, t))
            }).collect()
        };
        let mut vts_seen = vec![false; vts.len()];
        let gone = slots_of_impls(u, &gone_impls);
        let mut seen = gone.clone();
        let mut frontier = gone;
        while !frontier.is_empty() && restart.is_empty() {
            let mut users: Vec<u32> = frontier.iter().flat_map(|&s| u.fns[s as usize].history().inlined_into.iter().copied()).collect();
            // A trait object holding the frontier's code was made by code no call shows (a
            // coercion to `dyn`, at the type): that code is checked as a caller is
            // (Unit::vtable_makers), and with it the code making the trait objects whose
            // supertrait vtable it is. The objects themselves keep their vtable and its code. One
            // whose maker this run does not know (code placed from the startup image, a constant's
            // value) asks for a restart.
            let hit: Vec<usize> = (0..vts.len()).filter(|&i| !vts_seen[i] && vts[i].2.iter().any(|s| frontier.contains(s))).collect();
            if !hit.is_empty() {
                let makers = u.vtable_makers.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let mut covered: std::collections::HashSet<u64> = std::collections::HashSet::new();
                for &i in &hit {
                    vts_seen[i] = true;
                    if let Some(m) = makers.get(&vts[i].0) {
                        users.extend(m.iter().copied());
                        let mut todo = vec![vts[i].0];
                        while let Some(a) = todo.pop() {
                            for c in crate::syms::data_children(u, a) {
                                if covered.insert(c) {
                                    todo.push(c);
                                }
                            }
                        }
                    }
                }
                if let Some(&i) = hit.iter().find(|&&i| !makers.contains_key(&vts[i].0) && !covered.contains(&vts[i].0)) {
                    let s = vts[i].2.iter().copied().find(|s| frontier.contains(s)).unwrap_or(vts[i].2[0]);
                    restart.push(format!("`{}` of a removed impl is in trait objects (`{}`) the running code made, and which code makes them is not known -- removing it is applied by a restart", u.fns[s as usize].name, vtable_types(&vts[i].1)));
                    break;
                }
            }
            for (i, f) in u.fns.iter().enumerate() {
                trace_count("slot_visits", 1);
                if f.rir_refs().iter().chain(f.calls.iter()).any(|c| frontier.contains(c)) {
                    users.push(i as u32);
                }
            }
            let mut next = std::collections::HashSet::new();
            for c in users {
                if !seen.insert(c) {
                    continue;
                }
                // the instance this code belongs to (a closure's: its fn's)
                match slot_instance(u, c) {
                    // a removed fn's or an earlier version's: code in flight
                    Some((d, _)) if removed_all.contains(&d) || !def_is_bound(&u.prog, d) => {}
                    Some((d, args)) if !std_crates.contains(&u.prog.def(d).krate) && !(blanket && !args.is_empty()) && !args.iter().any(|a| ty_mentions(u, *a, &adts, 0)) => {
                        if !matches!(u.prog.def(d).kind, DefKind::Fn | DefKind::AssocFn) {
                            restart.push(format!("`{}` was evaluated with a removed impl's `{}` -- restart the app", u.prog.def_path(d), u.fns[c as usize].name));
                        } else if !body_defs.contains(&d) {
                            body_defs.push(d);
                        }
                    }
                    _ => {
                        impl_over.push(c);
                        next.insert(c);
                    }
                }
            }
            frontier = next;
        }
        trace_count("impl_callers", impl_over.len());
    }
    let mut scanned = 0usize;
    let mut moving: Vec<DefId> = Vec::new();
    loop {
        if set_names.len() > scanned {
            let names: Vec<&[u8]> = set_names[scanned..].iter().map(|n| n.as_slice()).collect();
            scanned = set_names.len();
            let mut seen: std::collections::HashSet<DefId> = std::collections::HashSet::new();
            for f in u.fns.iter() {
                trace_count("slot_visits", 1);
                if let FnKey::Inst(d, _) = &f.key {
                    if !visible_crates[u.prog.def(*d).krate as usize] { continue; }
                    // (a fn returning an opaque type with a slot: compiled code built for the type
                    // it returns calls it, or will)
                    if (f.compiled || f.rir.is_some() || defines_opaque(&u.prog, *d)) && !std_crates.contains(&u.prog.def(*d).krate) && matches!(u.prog.def(*d).kind, DefKind::Fn | DefKind::AssocFn) && seen.insert(*d) {
                        if !body_defs.contains(d) && def_is_bound(&u.prog, *d) && item_or_impl_mentions(&u.prog, *d, &names) {
                            body_defs.push(*d);
                        }
                    }
                }
            }
        }
        // Removed functions keep their old slots for code in flight. In particular, generated
        // child styles name their own entry: the dependency scan above must not schedule that
        // deleted body against the new namespace (or its retired lazy parse) when a child goes.
        // (a removed trait's provided methods go with it)
        body_defs.retain(|d| !versions.iter().any(|(_, plan)| plan.removed.contains(d) || plan.removed.contains(&u.prog.def(*d).parent)));
        let versioned: Vec<u32> = versions.iter().map(|(v, _)| v.old).collect();
        let mut touch: Vec<u32> = Vec::new();
        let mut moves: Vec<DefId> = Vec::new();
        for &d in &body_defs {
            let file = u.prog.def(d).file;
            if defines_opaque(&u.prog, d) {
                if moving.contains(&d) { continue; }
                moves.push(d);
            } else if versioned.contains(&file) || u.closures_of_slots(u.slots_of_defs([d])).is_empty() {
                continue;
            }
            if !versioned.contains(&file) && !touch.contains(&file) {
                if !own_source_file(&u.prog, file) {
                    restart.push(format!("`{}` is rebuilt with this edit, and its closures or async blocks (or the future it returns) need new code, but its file `{}` cannot take a new version live -- restart the app", u.prog.def_path(d), u.prog.file_paths[file as usize]));
                    continue;
                }
                touch.push(file);
            }
        }
        if !restart.is_empty() || (touch.is_empty() && moves.is_empty()) {
            break;
        }
        moving.extend_from_slice(&moves);
        trace_count("reversioned_files", touch.len());
        trace_count("moved_dependents", moves.len());
        for &file in &touch {
            let path = u.prog.file_paths[file as usize].clone();
            let src = { let pf = &u.prog.files[file as usize]; pf.src[..pf.src_len as usize].to_vec() };
            match u.prog.parse_version(file, &path, src) {
                Ok(v) => match classify_moving(&u.prog, file, &v.pf, &path, base, &moving) {
                    // (its text is the running one's: no change of its own but an include that moved)
                    Ok(mut plan) => {
                        changes.extend(std::mem::take(&mut plan.changes));
                        for &d in &plan.removed { set_names.push(def_name(&u.prog, d).into_bytes()); }
                        out.changed += plan.removed.len();
                        versions.push((v, plan));
                    }
                    Err(w) => restart.extend(w),
                },
                Err(e) => restart.push(format!("{}: a new version of this file (its closures and async blocks are rebuilt with the edit) did not parse: {} -- restart the app", path, e)),
            }
        }
        // the versions that already were in the transaction, with fns now moving
        for (v, plan) in versions.iter_mut() {
            if touch.contains(&v.old) || !moves.iter().any(|&d| u.prog.def(d).file == v.old) { continue; }
            match classify_moving(&u.prog, v.old, &v.pf, &v.path, base, &moving) {
                Ok(mut p) => {
                    // (its changes were taken already; a moving fn's text is unchanged: it had none)
                    p.changes.clear();
                    for &d in p.removed.iter().filter(|d| !plan.removed.contains(d)) {
                        set_names.push(def_name(&u.prog, d).into_bytes());
                        out.changed += 1;
                    }
                    *plan = p;
                }
                Err(w) => restart.extend(w),
            }
        }
        if !restart.is_empty() {
            break;
        }
    }
    if !restart.is_empty() {
        out.rung = Rung::Restart;
        out.lines = late_restart;
        out.lines.extend(restart);
        out.ms[0] = ms(t0);
        return out;
    }
    if !moving.is_empty() {
        out.test_changes = None;
    }
    // (the code of the files given new versions is rebuilt against the edit: never from the disk cache)
    note_edited_crates(u, &versions.iter().map(|(v, _)| v.old).filter(|f| !olds.contains(f)).collect::<Vec<u32>>());
    let const_defs: Vec<DefId> = changes.iter().filter_map(|c| if let Change::Const(d) = c { Some(*d) } else { None }).collect();
    let static_defs: Vec<DefId> = changes.iter().filter_map(|c| if let Change::StaticInit(d) = c { Some(*d) } else { None }).collect();
    trace_count("changed_consts", const_defs.len());
    trace_count("changed_statics", static_defs.len());
    out.changed += changes.len();
    // consts whose value depends on a changed one: the readers' slots (consts compiled as
    // initializers) and, conservatively, every evaluated const whose initializer names one
    let mut consts_hit: Vec<DefId> = const_defs.clone();
    for c in fresh_consts {
        if !consts_hit.contains(&c) {
            consts_hit.push(c);
        }
    }
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
                    out.lines.extend(late_restart.iter().cloned());
                    let named = if u.prog.def(c).kind == DefKind::AnonConst { consts_hit[0] } else { c };
                    out.lines.push(format!("const `{}` depends on a changed const and is used in a type -- restart the app", def_name(&u.prog, named)));
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
    if let Some(d) = const_read_by_a_type(u, &consts_hit) {
        out.rung = Rung::Restart;
        out.lines.extend(late_restart.iter().cloned());
        out.lines.push(format!("const `{}` is used in a type (an array length or const generic argument): its users' types change -- restart the app", def_name(&u.prog, d)));
        out.ms[0] = ms(t0);
        return out;
    }
    if let Some(st) = static_of_changed_consts(u, &consts_hit) {
        out.rung = Rung::Restart;
        out.lines.extend(late_restart.iter().cloned());
        out.lines.push(format!("the initializer of `{}` reads a changed const, and `{}` already holds the value the old one made (an initializer runs once) -- restart the app", def_name(&u.prog, st), def_name(&u.prog, st)));
        out.ms[0] = ms(t0);
        return out;
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
    // The slots whose own body changes (these and their twins) get their patch flags set at the
    // install: a frame running a copy inlined before calls the new body at the copy's next run
    // (jit::set_patched); the callers that only inlined them are rebuilt for their next call.
    let mut flagged: Vec<u32> = slots.iter().chain(fresh.iter()).copied().collect();
    let mut inlining_callers: Vec<u32> = Vec::new();
    let mut work: Vec<u32> = flagged.clone();
    while let Some(s) = work.pop() {
        let inl = u.fns[s as usize].history().inlined_into.len();
        let mut deps = u.fns[s as usize].history().inlined_into.clone();
        deps.extend(crate::sandbox::twin_of(s));
        for (k, d) in deps.into_iter().enumerate() {
            if k >= inl && flagged.contains(&s) && !flagged.contains(&d) {
                flagged.push(d);
            }
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
    // an armed real-time root's code is not replaced under it: the edit waits for the disarm
    // (realtime_disarmed retries it), nothing of it is installed now
    if let Some(line) = jit::realtime_frozen(u, &slots.iter().chain(fresh.iter()).copied().collect::<Vec<u32>>()) {
        out.rung = Rung::Refuse;
        out.lines.push(line);
        return out;
    }
    // Settle can evict a current closure owner's typeck result. Restore it before its
    // source version is replaced, including owners of already compiled closures: O3
    // and closure matching may need those old bodies long after this edit.
    let old_files: Vec<u32> = versions.iter().map(|(v, _)| v.old).collect();
    trace_sub("realtime_frozen");
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
                match initializer_deps_changed(up, (&*up).fn_id(FnKey::Inst(c, Vec::new())), &changed, &layout_defs) {
                    Ok(false) => {}
                    Ok(true) => discr_restart.push(format!("enum `{}`: a discriminant is computed by code this edit changes -- restart the app", def_name(&(*up).prog, d))),
                    Err(e) => discr_restart.push(format!("enum `{}`: a discriminant is computed by code whose dependencies are not known exactly ({}) -- restart the app", def_name(&(*up).prog, d), e)),
                }
            }
        }
    }
    trace_sub("gp_discriminants");
    // RELOAD' for constants: a constant a type uses whose value its initializer's code made
    // (not folded) is never evaluated again in a transaction; when what that code depends on
    // changes, the edit lands by a restart (the types sized by it hold the old value).
    {
        let mut changed: Vec<DefId> = body_defs.iter().chain(&consts_hit).chain(&static_defs).chain(&layout_defs).copied().collect();
        for (_, plan) in &versions { changed.extend_from_slice(&plan.removed); }
        let u = &*up;
        let mut ran: Vec<(DefId, u32)> = Vec::new();
        for (d, args) in u.consts.keys() {
            if !matches!(u.prog.def(*d).kind, DefKind::Const | DefKind::AssocConst) || !visible_crates[u.prog.def(*d).krate as usize] || changed.contains(d) { continue; }
            for s in u.slots_of_defs([*d]) {
                if matches!(&u.fns[s as usize].key, FnKey::Inst(x, a) if x == d && a == args) && u.fns[s as usize].compiled { ran.push((*d, s)); }
            }
        }
        ran.sort_unstable_by_key(|r| r.1);
        ran.dedup();
        for (c, s) in ran {
            if !(&*up).tcx.const_was_evaluated(c) { continue; }
            match initializer_deps_changed(up, s, &changed, &layout_defs) {
                Ok(false) => {}
                Ok(true) => discr_restart.push(format!("const `{}` is used in a type and its value is computed by code this edit changes -- restart the app", def_name(&(*up).prog, c))),
                Err(e) => discr_restart.push(format!("const `{}` is used in a type and its value is computed by code whose dependencies are not known exactly ({}) -- restart the app", def_name(&(*up).prog, c), e)),
            }
        }
    }
    trace_sub("gp_const_code");
    if !discr_restart.is_empty() {
        out.rung = Rung::Restart;
        out.lines = late_restart;
        out.lines.extend(discr_restart);
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
    let same_scopes = layout_defs.is_empty() && !changes.iter().any(|c| matches!(c, Change::Macro(_) | Change::Vis(_)));
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
    // the fns removed from trait impls that stay: (fn, impl, the trait's item), read before the
    // commit unlinks them
    // (one whose trait item goes too implements nothing that stays: its callers name the item)
    let gone_overrides: Vec<(DefId, DefId, DefId)> = versions.iter().flat_map(|(_, p)| p.removed.iter().filter_map(|&d| override_of(&u.prog, &u.tcx, d, &p.removed).map(|(imp, ti)| (d, imp, ti))).filter(|x| !p.removed.contains(&x.2))).collect();
    // the edited traits' items in their order, for a refused or rolled-back edit (the vtables
    // the running code makes are laid out from it)
    let saved_traits: Vec<(usize, Vec<DefId>)> = edited_traits.iter().map(|&td| { let ti = u.prog.def(td).sub as usize; (ti, u.prog.traits[ti].clone()) }).collect();
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
    let pre_summaries = if crate::borrowck::enabled(options) && crate::jit::CHECK_FIRST.load(std::sync::atomic::Ordering::Relaxed) { crate::borrowck::summaries_with(&u.prog, &u.tcx, &file_fns, fv_cx.as_ref()) } else { Vec::new() };
    if let Some(fv_cx) = &fv_cx { fv_cx.invalidate(&body_defs); }
    trace_sub("gp_summaries");
    let mut news: Vec<(u32, u32)> = Vec::new();
    let mut news_pairs: Vec<Vec<(u32, u32)>> = Vec::new();
    let mut added_defs: Vec<DefId> = Vec::new();
    let mut removed_defs: Vec<DefId> = Vec::new();
    let mut add_errs: Vec<String> = Vec::new();
    let mut saved_adts: Vec<(DefId, crate::tcx::AdtT)> = Vec::new();
    let saved_variants: Vec<(DefId, Vec<DefId>)> = layout_defs.iter().filter_map(|&d| {
        let def = u.prog.def(d);
        (def.kind == DefKind::Enum).then(|| (d, u.prog.variants[def.sub as usize].clone()))
    }).collect();
    let mut layout_restart: Vec<String> = late_restart;
    // a type edit the commit left at its running definition, or a trait's opaque types it does
    // not apply live (its restart's reason says why): bodies checked against them would report
    // errors the new text does not have
    let mut types_unapplied = false;
    let mut added_defaults: Vec<(DefId, u32, DefId)> = Vec::new();
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
    let mut removed_errs: Vec<String> = Vec::new();
    // fns declared again: a type their unchanged signature names moved (decls_naming_removed)
    let mut redeclared: Vec<DefId> = Vec::new();
    let errors0 = u.prog.errors.len();
    trace_stage("gate_prepare");
    {
        // parsing may have served requests: more startup installs may have been made since the
        // first drain; their entries are encoded against the sources they were built from
        u.flush_cache_records();
        let wait_from = jit::wall_ns();
        let fw = jit::front_write();
        if jit::front_trace() {
            let now = jit::wall_ns();
            eprintln!("reload-front-wait: {:.1} ms (t={:.0}..{:.0} ms)", (now - wait_from) as f64 / 1e6, wait_from as f64 / 1e6, now as f64 / 1e6);
        }
        trace_stage("front_write_wait");
        // (the tables are not the boot's from here on, kept or refused: PROGRAM_EDITS)
        if !versions.is_empty() { PROGRAM_EDITS.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
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
                    // items added inside a paired body (a `use` in a closure) got their defs at the
                    // commit: the type context collects them with the other added items
                    added_defs.extend(u.prog.take_body_added());
                    // a fn replaced in a trait impl (a new signature) leaves it before the new one
                    // is linked to the trait's item (with both there it is E0201)
                    // (so does a trait's fn: the impls link to the new one)
                    let trait_fn = |u: &Unit, d: DefId| { let p = u.prog.def(d).parent; p != crate::program::NO_DEF && u.prog.def(p).kind == DefKind::Trait && !removed.contains(&p) };
                    let replaced: Vec<DefId> = removed.iter().copied().filter(|&d| override_of(&u.prog, &u.tcx, d, &removed).is_some() || trait_fn(u, d)).collect();
                    u.prog.unbind_defs(&replaced);
                    let first_added = u.prog.defs.len();
                    match u.prog.add_items(n, &items) {
                        Ok(ds) => added_defs.extend(ds),
                        Err(e) => {
                            // A missing/invalid child module can fail collection
                            // after binding its parent. Roll those defs back too.
                            added_defs.extend((first_added as u32..u.prog.defs.len() as u32).map(DefId));
                            add_errs.push(e);
                        }
                    }
                    trace_sub("add_items");
                    u.prog.unbind_defs(&removed);
                    // a trait's fn that went: the impls are linked to its items again (an impl
                    // still defining it is E0407)
                    if !replaced.is_empty() && items.is_empty() {
                        relax_front::assoc::collect_in(&mut u.prog, Some(&visible_crates));
                    }
                    // removed impls leave the impl tables: trait selection and method lookup no
                    // longer find them (their defs stay for the code in flight)
                    let gone: Vec<DefId> = removed.iter().copied().filter(|d| u.prog.def(*d).kind == DefKind::Impl).collect();
                    if !gone.is_empty() {
                        u.tcx.uncollect_impls(&gone);
                    }
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
                    let _ = u.prog.take_body_added();
                    for (k, &(o, n)) in news.iter().enumerate().rev() {
                        u.prog.revert_version(o, n, &news_pairs[k]);
                    }
                    u.prog.rebind_use(std::mem::take(&mut use_undo));
                    for (d, had) in &saved_variants { u.prog.restore_variants(*d, had); }
                    restore_traits(u, &saved_traits, Some(&visible_crates));
                    u.tcx.publish_collected();
                    drop(fw);
                    out.rung = Rung::Restart;
                    out.lines.push(e);
                    return out;
                }
            }
        }
        // the defs the commits made (closures and anonymous constants of the edited bodies)
        u.tcx.grow_to(&u.prog);
        // imports the added items brought (`use`, an inline `mod`'s): resolved now, only theirs
        if u.prog.imports.len() > imports0 {
            u.prog.resolve_imports_from_in(imports0, &visible_crates);
            let new_errs: Vec<String> = u.prog.errors[errors0..].to_vec();
            u.prog.errors.truncate(errors0);
            add_errs.extend(new_errs);
        }
        // the imports, in any file, that bound a removed def resolve again, as a fresh run
        // resolves them: `use w::f` of a fn with a new signature binds the new fn, and one of a
        // fn that went binds nothing -- E0432, as rustc refuses the program (it went on calling
        // the removed fn)
        if !removed_defs.is_empty() {
            let (undo, unresolved) = u.prog.reresolve_imports_of(&removed_defs);
            use_undo.extend(undo);
            for (file, pos, path) in unresolved {
                removed_errs.push(format!("{}: E0432: unresolved import `{}`: what it named is removed by this edit", u.pos_str(file, pos), path));
            }
            // and the declarations whose unchanged text names a removed type or trait
            let new_files: Vec<u32> = news.iter().map(|&(_, n)| n).collect();
            let (errs, moved) = decls_naming_removed(u, &removed_defs, &new_files, &visible_crates);
            removed_errs.extend(errs);
            // a declaration naming a type the edit moves (its text unchanged, the name resolving
            // to the moved type now): a fn is declared again, as a fresh build declares it, so
            // the edit's code checks against what its text means. Code built for it before the
            // edit, or any other declaration (read as it was collected), is not rebuilt live
            for (d, g) in moved {
                let is_fn = matches!(u.prog.def(d).kind, DefKind::Fn | DefKind::AssocFn);
                if is_fn {
                    redeclared.push(d);
                }
                if !is_fn || !u.slots_of_defs([d]).is_empty() {
                    let now = u.prog.lookup_in_scope(u.prog.def(d).scope, u.prog.def(g).name, true).map_or_else(|| def_name(&u.prog, g), |n| u.prog.def_path(n));
                    layout_restart.push(format!("`{}` names `{}`, which this edit moves (the name means `{}` now), and what was built from its declaration before the edit is not rebuilt live -- restart the app", u.prog.def_path(d), def_name(&u.prog, g), now));
                }
            }
            let errs = redeclare(u, &redeclared, Some(&visible_crates));
            removed_errs.extend(errs);
        }
        trace_sub("imports");
        let lowered = if body_only {
            u.prog.lower_body_declarations(&legacy_owners, &news.iter().map(|&(_, n)| n).collect::<Vec<_>>())
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
        // the added items' signatures, types and impl tables: before the layout edits, whose
        // checks of an added field's type (its Default, its drop) ask about the items the edit
        // adds (a field of a struct a new module declares: its ADT was not collected yet, an ICE)
        trace_count("added_defs", added_defs.len());
        if !added_defs.is_empty() {
            let e = u.tcx.collect_added_in(&u.prog, &added_defs, false, scope.as_deref());
            add_errs.extend(e);
        }
        trace_sub("collect_added");
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
                    types_unapplied = true;
                    continue;
                };
                after = values;
                let errors = fresh_adt.check_discriminants(&u.tcx, &u.prog);
                if !errors.is_empty() { add_errs.extend(errors); continue; }
            }
            let before = running_discriminants.iter().find(|(ed, _)| *ed == d).map_or(&[][..], |(_, v)| v.as_slice());
            trace_sub("layout_discr");
            let new_layout = fresh_structs.iter().any(|x| x.0 == d);
            let edited = layout_edit(u, d, &fresh_adt, before, &after, new_layout);
            trace_sub("layout_edit");
            match edited {
                Ok((adt, notes)) => {
                    if let (false, Some(old), Some((_, moved))) = (new_layout, u.tcx.adts.get(&d), moved_fields.iter().find(|m| m.0 == d)) {
                        for (k, (o, n)) in old.variants[0].fields.iter().zip(&adt.variants[0].fields).enumerate() {
                            if o.name != n.name && !u.prog.syms.str(o.name).starts_with("__relax_dead_") && !moved.contains(&(k as u32)) {
                                layout_restart.push(format!("struct `{}`: field `{}` changed its type though its text did not (what the type names changed), and its readers are not known -- restart the app", def_name(&u.prog, d), u.prog.syms.str(o.name)));
                            }
                        }
                    }
                    // (a tuple variant the edit adds: its constructor's signature)
                    if adt.is_enum {
                        u.tcx.insert_ctor_sigs(d, &adt);
                    }
                    if let Some(old) = u.tcx.adts.insert(d, adt) {
                        if !old.is_enum && !new_layout {
                            for f in &u.tcx.adts[&d].variants[0].fields {
                                if old.variants[0].fields.iter().any(|o| o.name == f.name) { continue; }
                                if let Some(c) = u.field_default_const(d, f.name).filter(|&c| is_live_default(&u.prog, c)) {
                                    added_defaults.push((d, f.name, c));
                                }
                            }
                        }
                        saved_adts.push((d, old));
                    }
                    out.lines.extend(notes);
                }
                Err(r) => {
                    layout_restart.push(r);
                    types_unapplied = true;
                }
            }
        }
        trace_sub("layout_fresh");
        // an edited `use`: a type declared through the name it bound can mean another type now
        let use_refs: Vec<&[u8]> = use_names.iter().map(|n| n.as_slice()).collect();
        for i in 0..u.prog.defs.len() {
            let d = DefId(i as u32);
            if use_refs.is_empty() || layout_defs.contains(&d) || !matches!(u.prog.def(d).kind, DefKind::Struct | DefKind::Enum | DefKind::Union) || !visible_crates[u.prog.def(d).krate as usize] || !item_mentions(&u.prog, d, &use_refs) {
                continue;
            }
            let (fresh_adt, _) = u.tcx.adt_fresh(&crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &crate::tcx::EvalState::new()), &u.prog, d);
            let (Some(fresh_adt), Some(old)) = (fresh_adt, u.tcx.adts.get(&d)) else { continue };
            let retyped = old.variants.iter().zip(&fresh_adt.variants).flat_map(|(o, n)| o.fields.iter().zip(&n.fields)).find(|(o, n)| o.ty != n.ty);
            if let Some((o, _)) = retyped {
                layout_restart.push(format!("struct `{}`: field `{}` changed its type though its text did not (what the type names changed), and its readers are not known -- restart the app", def_name(&u.prog, d), u.prog.syms.str(o.name)));
                types_unapplied = true;
            }
        }
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
                let at = match (saved_adts.iter().find(|(x, _)| *x == ed), u.tcx.adts.get(&ed)) {
                    (Some((_, old)), Some(now)) => running_variants_at(old, now).unwrap_or_default(),
                    _ => Vec::new(),
                };
                if let Err(r) = enum_fits(&before, u, &at) {
                    layout_restart.push(r);
                }
            }
        }
        trace_sub("layout");
        // a value code builds by raw writes keeps the ext word its memory held (M10): a struct
        // whose running layout needs that word is built so by no code, running or to be compiled
        let need = structs_needing_ext_word(u);
        if !need.is_empty() {
            if let Some((s, by, conv)) = raw_builder(u, &need, &replaced_raw) {
                let how = if conv { "reaches its bytes as other data (a pointer cast to another type, an address, inline assembly)" } else { "writes its fields in place through a raw pointer" };
                layout_restart.push(format!("struct `{}`: `{}` {}, which leaves the hidden word of its live layout as the memory had it, and after this edit its values need that word (fields added live, or a removed field to drop) -- restart the app", u.prog.def_path(s), u.prog.def_path(by), how));
            }
        }
        // a struct taking its new layout: a value of it behind a pointer converted to another type
        // (an address kept as a number, bytes) is not seen by its holders' types
        let fresh_defs: Vec<DefId> = fresh_structs.iter().map(|x| x.0).collect();
        if !fresh_defs.is_empty() && layout_restart.is_empty() {
            if let Some((s, by, conv)) = raw_builder(u, &fresh_defs, &replaced_raw) {
                let how = if conv { "reaches its bytes as other data (a pointer cast to another type, an address, inline assembly)" } else { "writes its fields in place through a raw pointer" };
                layout_restart.push(format!("struct `{}` changed its layout, and `{}` {}: a value of it may be kept where its type does not show -- restart the app", u.prog.def_path(s), u.prog.def_path(by), how));
            }
        }
        trace_sub("raw_builders");
        unsafe {
            u.prog.reclaim_caches();
        }
        drop(fw);
        trace_sub("reclaim");
        if !saved_adts.is_empty() {
            let files: Vec<u32> = saved_adts.iter().map(|(d, _)| u.prog.def(*d).file).collect();
            crate::cache::layout_edit_committed(up, files.into_iter());
        }
    }
    // A struct that took its new layout (fresh_layout_struct): the code of every instance over the
    // types holding it (`Vec<P>::push`, other crates' generic code), their glue and shims, and what
    // inlined any of them, is rebuilt; the fns that can handle its values were type-checked with
    // the edit (fresh_held, above) and are rebuilt with their closures.
    if !fresh_held.is_empty() {
        let holds = |u: &Unit, t: TyId| ty_holds(u, t, &fresh_held, 0);
        let mut users: Vec<u32> = Vec::new();
        for (i, f) in u.fns.iter().enumerate() {
            let hit = match &f.key {
                FnKey::Inst(_, args) => args.iter().any(|a| holds(u, *a)),
                FnKey::Glue(_, t) => holds(u, *t),
                FnKey::Shim(sh) => sh.args.iter().any(|a| holds(u, *a)),
                _ => false,
            };
            if hit && !in_flight_only(u, i as u32, &removed_all) {
                users.push(i as u32);
            }
        }
        users.extend(u.slots_of_defs(body_defs.iter().copied()));
        users.extend(u.closures_of_slots(users.clone()));
        let mut seen: crate::fxhash::HashSet<u32> = Default::default();
        let mut work = users;
        while let Some(c) = work.pop() {
            if !seen.insert(c) {
                continue;
            }
            let f = &u.fns[c as usize];
            if !slots.contains(&c) && !fresh.contains(&c) {
                if f.compiled {
                    slots.push(c);
                } else if f.rir.is_some() {
                    fresh.push(c);
                }
            }
            for &x in f.history().inlined_into.iter() {
                if !in_flight_only(u, x, &removed_all) {
                    if reload_trace() && !seen.contains(&x) {
                        eprintln!("reload: `{}` inlined `{}`, rebuilt for a struct taking its new layout", u.fns[x as usize].name, f.name);
                    }
                    work.push(x);
                }
            }
        }
    }
    let added_impls: Vec<DefId> = added_defs.iter().copied().filter(|d| u.prog.def(*d).kind == DefKind::Impl).collect();
    // an added `impl Drop`: the type now needs dropping where code compiled before left it alone
    // -- the drop glue of every type holding it, the generic instances over it (`Vec<T>`'s
    // drop_in_place) and every fn naming the type are rebuilt; values made before the edit are
    // dropped by the new glue. A removed one: the same code, which no longer drops it (also
    // where a caller inlined the glue). A marker impl (a trait impl with no items, `Copy`): no
    // code calls it, but it picks which specializing impl an instance takes (std's `to_vec` for
    // `T: Copy`), so the same code is rebuilt.
    {
        let drop_trait = u.tcx.lang_items.get(LangItem::Drop);
        let mut dtypes: Vec<DefId> = Vec::new();
        for &imp in added_impls.iter().chain(&gone_impls) {
            let trait_of = u.tcx.impl_trait.get(&imp).copied();
            let empty = if gone_impls.contains(&imp) { empty_gone.contains(&imp) } else { u.prog.impls[u.prog.def(imp).sub as usize].items.is_empty() };
            let marker = trait_of.is_some() && empty;
            if marker || (drop_trait.is_some() && trait_of == drop_trait) {
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
            // (not the code of a fn the edit replaced or removed: it keeps its slots, and its old
            // text need not check against the new impls -- a removed derive's marker impl)
            for i in 0..u.fns.len() {
                let f = &u.fns[i];
                if in_flight_only(u, i as u32, &removed_all) {
                    continue;
                }
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
            let mut work: Vec<u32> = slots.iter().chain(fresh.iter()).copied().filter(|&s| matches!(u.fns[s as usize].key, FnKey::Glue(..))).collect();
            while let Some(s) = work.pop() {
                for c in u.fns[s as usize].history().inlined_into.clone() {
                    if slots.contains(&c) || fresh.contains(&c) || in_flight_only(u, c, &removed_all) {
                        continue;
                    }
                    if u.fns[c as usize].compiled {
                        slots.push(c);
                    } else if u.fns[c as usize].rir.is_some() {
                        fresh.push(c);
                    }
                    work.push(c);
                }
            }
        }
    }
    // A removed impl another impl replaces (the same trait for the same type: its header was
    // edited): the generic code at the type that called the removed one is rebuilt, and calls
    // the new one. (Without a replacement it is dead code: the code that chose the type needs
    // the impl, and was checked with the edit.)
    if !impl_over.is_empty() {
        let head = |u: &Unit, imp: &DefId| -> Option<(DefId, Vec<DefId>)> {
            let td = *u.tcx.impl_trait.get(imp)?;
            let mut a = Vec::new();
            ty_adts(u, *u.tcx.impl_self.get(imp)?, &mut a, 0);
            Some((td, a))
        };
        let replaced = gone_impls.iter().filter_map(|imp| head(u, imp)).any(|h| added_impls.iter().any(|a| head(u, a).as_ref() == Some(&h)));
        if replaced {
            for &c in &impl_over {
                if slots.contains(&c) || fresh.contains(&c) {
                    continue;
                }
                if u.fns[c as usize].compiled {
                    slots.push(c);
                } else if u.fns[c as usize].rir.is_some() {
                    fresh.push(c);
                }
            }
        }
    }
    trace_sub("drop_glue");
    // the added items' diagnostics: only those in the edited files are the edit's (collect_added
    // also flushes what background type checks of other crates queued meanwhile)
    let edited: std::collections::HashSet<String> = olds.iter().copied()
        .chain(added_defs.iter().map(|&d| u.prog.def(d).file))
        .map(|f| u.prog.file_paths[f as usize].clone()).chain(edit_paths).collect();
    let mut errs: Vec<String> = add_errs.into_iter().filter(|e| !(e.contains(".rs:")) || edited.iter().any(|p| e.starts_with(p.as_str()))).collect();
    errs.extend(removed_errs);
    // The edit's own errors, as a fresh build of its text reports them: they refuse it even
    // where a restart is also needed (a restart would only stop at them)
    let mut real: Vec<String> = errs.clone();
    let mut as_restart = !layout_restart.is_empty();
    errs.extend(layout_restart);
    // `Copy` removed and `Drop` added for one type: code compiled for the copies it made copies
    // its values bit for bit, and the new glue would drop every copy
    if let (Some(copy), Some(drop_trait)) = (u.tcx.lang_items.get(LangItem::Copy), u.tcx.lang_items.get(LangItem::Drop)) {
        let of = |u: &Unit, imp: &DefId, t: DefId| -> Option<DefId> {
            if u.tcx.impl_trait.get(imp) != Some(&t) { return None; }
            match u.tcx.tys.kind(*u.tcx.impl_self.get(imp)?) { crate::types::TyKind::Adt(d, _, _) => Some(*d), _ => None }
        };
        for imp in &gone_impls {
            if let Some(d) = of(u, imp, copy) {
                if added_impls.iter().any(|a| of(u, a, drop_trait) == Some(d)) {
                    errs.push(format!("`{}` stops being `Copy` and gets a `Drop` impl: code compiled before copies its values -- restart the app", u.prog.def_path(d)));
                    as_restart = true;
                }
            }
        }
    }
    // An impl added (or its header edited) for a type whose trait objects of that trait still
    // hold a removed impl's methods -- the impl was removed while such objects lived, in this edit
    // or an earlier one: a new coercion of the type takes the vtable this run built for it
    // (Unit::vtables) and would run the removed code. Applied by a restart.
    'readd: for &imp in &added_impls {
        let Some(&td) = u.tcx.impl_trait.get(&imp) else { continue };
        let mut adts = Vec::new();
        if let Some(&t) = u.tcx.impl_self.get(&imp) { ty_adts(u, t, &mut adts, 0); }
        for ((src, dy), addr) in u.vtables.snapshot() {
            let crate::types::TyKind::Dyn(p, ..) = u.tcx.tys.kind(dy) else { continue };
            if *p != td && !u.tcx.trait_closure.get(p).is_some_and(|v| v.contains(&td)) { continue; }
            let mut a = Vec::new();
            ty_adts(u, src, &mut a, 0);
            if !adts.is_empty() && !a.iter().any(|d| adts.contains(d)) { continue; }
            let mut thunks = Vec::new();
            crate::syms::data_thunks(u, addr, &mut thunks);
            if let Some(&s) = thunks.iter().find(|&&s| slot_instance(u, s).is_some_and(|(d, _)| removed_all.contains(&d) || !def_is_bound(&u.prog, d))) {
                let ty = |t| crate::typeck::ty_to_string(false, &u.prog, &u.tcx, t);
                errs.push(format!("`{}` gets an impl of `{}` again, and its trait objects (`{}` as `{}`) the running code made hold the removed impl's `{}` -- restart the app", ty(src), u.prog.def_path(td), ty(src), ty(dy), u.fns[s as usize].name));
                as_restart = true;
                break 'readd;
            }
        }
    }
    // A fn added to or removed from a trait impl that stays (an override of a provided method,
    // or one replaced for a new signature): the instances of the trait's method resolve anew, as
    // a fresh run of this text resolves them. A slot whose instance now resolves to another fn
    // runs that fn's code from the install on, through a forwarder (jit::install_forward): the
    // code that reaches it without naming it -- a trait object's vtable, a fn pointer, generic
    // and std code -- runs the new one. The callers that inlined it are rebuilt, and a frame in a
    // copy inlined before calls the forwarder at the copy's next run (its patch flag). A slot an
    // earlier edit forwarded whose instance is its own again (the override went) gets its own
    // code back.
    let mut retarget: Vec<(u32, u32)> = Vec::new();
    let mut homes: Vec<u32> = Vec::new();
    {
        let added_overrides: Vec<(DefId, DefId, DefId)> = added_defs.iter().filter_map(|&d| override_of(&u.prog, &u.tcx, d, &added_impls).map(|(imp, ti)| (d, imp, ti))).collect();
        let mut items: Vec<DefId> = added_overrides.iter().chain(&gone_overrides).map(|x| x.2).collect();
        items.sort_unstable();
        items.dedup();
        let mut impls: Vec<DefId> = added_overrides.iter().chain(&gone_overrides).map(|x| x.1).collect();
        // (and every impl of a trait whose methods changed: E0046 for a required one added)
        for &td in &edited_traits {
            impls.extend(u.tcx.trait_impls_of.get(&td).map_or(&[][..], |v| v.as_slice()).iter().copied().filter(|imp| !removed_all.contains(imp)));
        }
        impls.sort_unstable();
        impls.dedup();
        // a trait's fn added returning `impl Trait` or a future: its impls' are types of their own
        for &d in &added_defs {
            let p = u.prog.def(d).parent;
            if p != crate::program::NO_DEF && edited_traits.contains(&p) && u.prog.associated_types_for_impl_traits.get(&d).is_some_and(|v| !v.is_empty()) {
                errs.push(format!("`{}` returns `impl Trait` or a future: adding it to a trait is not applied live yet -- restart the app", u.prog.def_path(d)));
                as_restart = true;
                types_unapplied = true;
            }
        }
        // the impls' items against their traits, as a fresh build checks them (E0046: a required
        // fn removed; E0053: an override's signature)
        for &imp in &impls {
            let e = u.tcx.check_impl_items_against_trait(&u.prog, imp);
            real.extend(e.iter().cloned());
            errs.extend(e);
        }
        for &ti in &items {
            if u.prog.associated_types_for_impl_traits.get(&ti).is_some_and(|v| !v.is_empty()) {
                errs.push(format!("`{}` returns `impl Trait` or a future: an override's is a type of its own, and the code compiled before holds the other one -- restart the app", u.prog.def_path(ti)));
                as_restart = true;
                types_unapplied = true;
            }
        }
        let env = crate::tcx::TypingEnv::fully_monomorphized(&u.prog);
        for s in 0..u.fns.len() as u32 {
            let (ti, targs): (DefId, Vec<TyId>) = match &u.fns[s as usize].key {
                FnKey::Inst(d, args) if items.contains(d) => (*d, args.clone()),
                FnKey::Inst(d, args) => {
                    let Some(&(_, imp, ti)) = gone_overrides.iter().find(|x| x.0 == *d) else { continue };
                    // the impl's args, then the method's: the trait's args, then the method's
                    let ind = u.tcx.generic_indices_of(&u.prog, imp);
                    let k = ind.types_consts.len();
                    if args.len() < k {
                        continue;
                    }
                    let regions = u.tcx.tys.intern_region_args(&vec![crate::types::RegionId::ERASED; ind.lifetimes.len()]);
                    let full = u.tcx.complete_args(&u.prog, imp, &args[..k], regions);
                    let tr = u.tcx.impl_trait_ref(&u.prog, imp, &full);
                    use relax_types::consts::GenericArg;
                    let mut t: Vec<TyId> = tr.args.iter().filter_map(|a| match *a {
                        GenericArg::Type(t) => Some(t),
                        GenericArg::Const(c) => Some(u.tcx.tys.intern(crate::types::TyKind::Const(c))),
                        GenericArg::Lifetime(_) => None,
                    }).collect();
                    t.extend_from_slice(&args[k..]);
                    (ti, t)
                }
                _ => continue,
            };
            let Ok(inst) = u.tcx.resolve_instance_raw(&u.prog, env, ti, &targs) else { continue };
            let crate::instance::InstanceKind::Item(nd) = inst.def else { continue };
            let key = FnKey::Inst(nd, inst.compact_args(&u.tcx));
            if key == u.fns[s as usize].key {
                if u.forward_of(s).is_some() {
                    homes.push(s);
                }
                continue;
            }
            let t = u.fn_id(key);
            if u.forward_of(s).map(|x| x.0) != Some(t) {
                retarget.push((s, t));
            }
        }
        for h in homes.clone() {
            if !slots.contains(&h) && !fresh.contains(&h) {
                slots.push(h);
            }
            // (its unverified-mode twin is built again with it: its own code too)
            if let Some(tw) = u.plain_twin_of(h).filter(|&tw| u.forward_of(tw).is_some()) {
                homes.push(tw);
            }
        }
        let mut twins: Vec<(u32, u32)> = Vec::new();
        for &(s, t) in &retarget {
            if crate::sandbox::twin_of(s).is_some() {
                errs.push(format!("`{}` has sandboxed code, which does not take a forwarder -- restart the app", u.fns[s as usize].name));
                as_restart = true;
            }
            // an unverified-mode twin runs the target's twin
            if let Some(tw) = u.plain_twin_of(s) {
                twins.push((tw, u.plain_twin(t)));
            }
        }
        // the callers that inlined a forwarded slot, and theirs
        let mut work: Vec<u32> = retarget.iter().map(|x| x.0).collect();
        let mut seen: std::collections::HashSet<u32> = work.iter().copied().collect();
        while let Some(s) = work.pop() {
            for c in u.fns[s as usize].history().inlined_into.clone() {
                if !seen.insert(c) || in_flight_only(u, c, &removed_all) {
                    continue;
                }
                if !slots.contains(&c) && !fresh.contains(&c) {
                    if u.fns[c as usize].compiled {
                        slots.push(c);
                    } else if u.fns[c as usize].rir.is_some() {
                        fresh.push(c);
                    }
                }
                work.push(c);
            }
        }
        for &(s, _) in &retarget {
            if !flagged.contains(&s) {
                flagged.push(s);
            }
        }
        retarget.extend(twins);
        trace_count("retargeted", retarget.len());
    }
    // The keys follow the text. What a new run of this text would not link is rebuilt with the
    // edit's slots, by the same walk as const_readers' readers before the commit.
    let rekeyed = !const_before.is_empty();
    if rekeyed {
        let found = crate::cache::rekey_after_const_edit(up, &const_before);
        trace_count("rekey_readers", found.readers.len());
        if linked_process {
            for d in &found.blind {
                errs.push(format!("const `{}` changed outside the text the keys are made from (an included file): no key shows its readers in this run's code from the disk cache -- restart the app", def_name(&u.prog, *d)));
                as_restart = true;
            }
        }
        let mut seen: std::collections::HashSet<u32> = slots.iter().chain(fresh.iter()).copied().collect();
        let mut marked: std::collections::HashSet<u32> = flagged.iter().copied().collect();
        let mut work: Vec<u32> = Vec::new();
        for s in found.readers {
            // an initializer that ran does not run again. Whether it read the edit was asked of its
            // text before the commit (static_of_changed_consts): a key also moves with its fn's names
            if matches!(&u.fns[s as usize].key, FnKey::Inst(d, _) if u.prog.def(*d).kind == DefKind::Static && u.statics.get(d).is_some_and(|&a| a != 0)) { continue; }
            if marked.insert(s) { flagged.push(s); }
            if seen.insert(s) {
                if u.fns[s as usize].compiled { slots.push(s); } else { fresh.push(s); }
                work.push(s);
            }
        }
        while let Some(s) = work.pop() {
            let inl = u.fns[s as usize].history().inlined_into.len();
            let mut deps = u.fns[s as usize].history().inlined_into.clone();
            deps.extend(crate::sandbox::twin_of(s));
            for (k, d) in deps.into_iter().enumerate() {
                if k >= inl && marked.contains(&s) && marked.insert(d) { flagged.push(d); }
                if !seen.insert(d) { continue; }
                if u.fns[d as usize].compiled {
                    if k < inl { inlining_callers.push(d); }
                    slots.push(d);
                } else {
                    fresh.push(d);
                }
                work.push(d);
            }
        }
        // an armed real-time root's code is not replaced under it (as before the commit)
        if let Some(line) = jit::realtime_frozen(u, &slots.iter().chain(fresh.iter()).copied().collect::<Vec<u32>>()) {
            errs.push(line);
        }
        trace_count("selected_slots_rekey", slots.len() + fresh.len());
    }
    trace_sub("consts_errs");
    trace_stage("commit_collect");
    trace_bodies(&u.prog, &body_defs);
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
        // (checked when a restart is also needed: their errors refuse the edit, and the restart
        // would stop at them -- unless a type of the edit kept its running definition)
        if as_restart && types_unapplied {
            uniq.clear();
        }
        trace_count("typecheck_items", uniq.len());
        let mut mir_checked: Vec<DefId> = Vec::new();
        for (d, a) in uniq {
            // Derive code, not the program's items; whether running values can take a default
            // is decided by evaluating it below.
            if is_live_default(&u.prog, d) { continue; }
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::typeck::checked_body(&crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &crate::tcx::EvalState::new()), &u.prog, &u.tcx, d, &a)));
            match r {
                Ok(b) => {
                    let mut found = b.errors.to_vec();
                    // the MIR-phase checks of `makepad check` (and the app check at boot) on the
                    // body's own text, once per item: those of its instances are the same
                    if found.is_empty() && !mir_checked.contains(&d) {
                        mir_checked.push(d);
                        found = mir_phase_errors(&u.prog, &u.tcx, d);
                    }
                    for e in found {
                        if !real.contains(&e) { real.push(e.clone()); }
                        if !errs.contains(&e) { errs.push(e); }
                    }
                    // kept for the sandbox check and the borrow gate (which would type-check
                    // the same body again)
                    checked_bodies.push(b);
                }
                // (not the text's error: where a restart is needed, the restart builds it anew)
                Err(p) => errs.push(format!("internal compiler error type-checking {} ({}) -- makepad run bug, report it", u.prog.def_path(d), crate::lifeline::panic_line(&*p))),
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
                let e = crate::modules::sandbox_check(&u.prog, &u.tcx, krate, &bodies);
                real.extend(e.iter().cloned());
                errs.extend(e);
            }
        }
    }
    if module {
        for &file in &olds {
            let e = crate::modules::check_orphans(u, u.prog.file_crate[file as usize], &added_defs);
            real.extend(e.iter().cloned());
            errs.extend(e);
        }
    }
    // What a crate the host provides lacks: its items are this makepad's own, and its changed
    // sources wait for the rebuilt makepad, so no patch adds them -- the restart into that
    // makepad applies the edit (the held items wait for it, the rest patches now). Only while
    // makepad's own inputs have changed (host::inputs_changed: seen, whether or not this process
    // can build makepad, until a rebuild shows them unchanged): with none, no makepad has them and
    // the errors are the edit's own. (A new op runs in the host, beside its state: no JIT copy of
    // it can stand in.)
    if !real.is_empty() && host::inputs_changed() {
        if let Some(lines) = provided_lacks(&real) {
            errs.retain(|e| !real.contains(e));
            errs.extend(lines);
            real.clear();
            as_restart = true;
        }
    }
    // a restart that the edit's errors would stop: the errors alone refuse it
    if as_restart && !real.is_empty() {
        errs = real.clone();
        as_restart = false;
    }
    // The running values cannot take such an initial value: a restart, unless the edit also
    // has real errors (those alone refuse it).
    if errs.is_empty() {
        let mut default_restart = Vec::new();
        let mut default_errs = Vec::new();
        for (d, name, c) in added_defaults {
            // The derive runs the initial value where its `vm` is bound; the const the front makes
            // of it binds nothing. One that names `vm` is never evaluated here (its build fails), and
            // any other error in it is the edit's own.
            let checked = {
                let u = &*up;
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::typeck::checked_body(&crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &crate::tcx::EvalState::new()), &u.prog, &u.tcx, c, &[])))
            };
            let names_vm = match &checked {
                Ok(b) if !b.errors.is_empty() && b.errors.iter().all(|e| e.contains("E0425: cannot find value `vm` in this scope")) => true,
                Ok(b) if !b.errors.is_empty() => {
                    default_errs.extend(b.errors.iter().cloned());
                    continue;
                }
                Ok(_) => false,
                Err(p) => {
                    default_errs.push(format!("internal compiler error type-checking {} ({}) -- makepad run bug, report it", (&*up).prog.def_path(c), crate::lifeline::panic_line(&*p)));
                    continue;
                }
            };
            if names_vm || (&*up).const_addr_shared(c, Vec::new()).is_none() {
                let u = &*up;
                default_restart.push(format!("struct `{}`: field `{}` added; its `#[rust(..)]` initial value is its derive's code ({}), running values cannot take it -- restart the app", u.prog.def_path(d), u.prog.syms.str(name), if names_vm { "it names the derive's `vm`" } else { "not a constant" }));
            }
        }
        if !default_errs.is_empty() {
            errs = default_errs;
        } else if !default_restart.is_empty() {
            errs = default_restart;
            as_restart = true;
        }
    }
    let u = &mut *up;
    trace_stage("typecheck");
    // D3: the gate is synchronous only under `makepad run --check-first`; otherwise the patched
    // instances are queued to Gandalf when their new code is installed (jit.rs install, RANK_EDIT).
    if errs.is_empty() && crate::borrowck::enabled(options) && crate::jit::CHECK_FIRST.load(std::sync::atomic::Ordering::Relaxed) {
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
            // SPEC-V: the edited bodies have no verdict for their new text yet: flagged before any
            // caller of this batch looks at them (unverified code is not inlined)
            let unverified = !u.verdict_clean(id);
            if unverified { crate::jit::note_unverified(u, id); }
            let f = &mut u.fns[id as usize];
            f.unverified.store(unverified, Ordering::Relaxed);
            f.rir = None;
            if let Some(h) = &mut f.history { h.inlined_into.clear(); }
            f.build_err = None;
            f.spec = None;
            u.reload_errors.remove(&id);
            u.reload_specs.remove(&id);
        }
        // SPEC-V: the unverified-mode twins of the edited bodies are the old text: rebuilt if they had
        // code, else only forgotten (they compile from the new text on their first call)
        let plain: Vec<u32> = slots.iter().chain(fresh.iter()).filter_map(|&id| u.plain_twin_of(id)).collect();
        for &id in &plain {
            let f = &mut u.fns[id as usize];
            f.rir = None;
            f.build_err = None;
            f.spec = None;
            u.reload_errors.remove(&id);
            u.reload_specs.remove(&id);
        }
        // Module base slots only supply RIR. Their twins are already in the dependency
        // closure above; rebuild compiled twins and leave cold twins for first use.
        let targets: Vec<u32> = slots.iter().chain(fresh.iter()).copied().chain(plain.iter().copied().filter(|&id| u.fns[id as usize].compiled)).filter(|&id| {
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
    // Linked code's type-level uses show here, as its readers lower: an evaluation of the new text
    // (publish_reload dropped the evaluations of the old; a const no constant context names is not
    // read by one). The types of the code around them changed: none of it installs.
    if linked_process && ctfe_relevant && real.is_empty() {
        let u = &*up;
        if let Some(d) = const_read_by_a_type(u, &consts_hit) {
            errs = vec![format!("const `{}` is used in a type (an array length or const generic argument): its users' types change -- restart the app", def_name(&u.prog, d))];
            as_restart = true;
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
    // its own code (memory-safe; reported, and waiting for a restart when values of it may be
    // stored: kept_closure_line). The new code is built now if the edit's precompile
    // did not reach it. An older version that forwards to its lineage's cell (install_forward)
    // follows the cell and is not looked at again: an edit's work stays the same however many
    // edits came before.
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
                else if f.compiled && !matches!(u.forward_of(id), Some((_, false))) { olds_c.push(id); }
            }
        }
        // a new closure's identity is read from its parent's checked body, which the parent's
        // build may have dropped already (the closure was lowered with it)
        if !olds_c.is_empty() {
            for &n in &news_c {
                if let FnKey::Closure(file, _, parent) = (&*up).fns[n as usize].key {
                    let _ = jit::ensure_parent_body(up, parent, file, 0);
                }
            }
        }
        let u = &*up;
        let new_ids: Vec<(u32, Option<(u32, TyId, Vec<TyId>, TyId)>)> = news_c.iter().map(|&n| (n, closure_ident(u, n))).collect();
        let mut pairs = Vec::new();
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
                Some((n, _)) => pairs.push((o, *n)),
                None => {
                    if oi.is_some() && new_ids.iter().any(|(n, ni)| matches!(&u.fns[*n as usize].key, FnKey::Closure(_, _, p) if *p == parent) && ni.as_ref().map(|x| x.0) == oi.as_ref().map(|x| x.0)) {
                        out.lines.push(kept_closure_line(u, o, "its captures or signature changed"));
                    }
                }
            }
        }
        for (o, n) in pairs {
            let built = pre.iter().any(|(id, r)| *id == n && r.is_ok()) || staged.iter().any(|(id, _)| *id == n);
            if !built {
                match compile_guarded(up, n) {
                    Ok(c) => pre.push((n, Ok(c))),
                    Err(_) => {
                        out.lines.push(kept_closure_line(&*up, o, "its new code did not build"));
                        continue;
                    }
                }
            }
            rebind.push((o, n));
        }
    }
    // suspended async fn bodies (M5): an old poll whose parent was rebuilt resumes in the new
    // code when the new body has the same code up to its last await and the same state shape
    // (what each suspension point saved where); otherwise it finishes on its old code
    if errs.is_empty() {
        let rebuilt: Vec<u32> = slots.iter().chain(fresh.iter()).copied().collect();
        let new_files: Vec<u32> = news.iter().map(|x| x.1).collect();
        // A stored fn item can keep calling the removed constructor. Rebuilding
        // its caller does not necessarily instantiate the replacement async fn:
        // the caller's old future may still be running. Prepare matching
        // constructors so their poll bodies participate in compatibility checks.
        let replacements: Vec<(DefId, Vec<TyId>)> = {
            let u = &*up;
            let signature = |d: DefId| -> Option<Vec<Vec<u8>>> {
                let d = u.prog.def(d);
                let pf = &u.prog.files[d.file as usize];
                let item = pf.ast.item(d.item);
                let crate::ast::ItemKind::Fn(sig, body, _) = &item.kind else { return None };
                let sig = pf.ast.fn_sig(*sig);
                if !sig.asyncness { return None; }
                let end = body.map(|b| pf.ast.block(b).lo)
                    .or_else(|| sig.lazy_body.map(|r| pf.tok(r.lo - 1).lo))?;
                Some(toks_norm(pf, item.lo, end, &[]))
            };
            u.slots_of_defs(removed_defs.iter().copied()).into_iter().filter_map(|id| {
                let FnKey::Inst(old, args) = &u.fns[id as usize].key else { return None };
                if !u.closures_of_slots([id]).iter().any(|&c| u.fns[c as usize].compiled
                    && matches!(u.fns[c as usize].key, FnKey::Closure(_, ce, _) if ce & crate::tcx::ASYNC_FN_BODY != 0)) {
                    return None;
                }
                let sig = signature(*old)?;
                added_defs.iter().copied().find(|&d| u.prog.def_path(d) == u.prog.def_path(*old)
                    && signature(d).as_ref() == Some(&sig)).map(|d| (d, args.clone()))
            }).collect()
        };
        for (d, args) in replacements {
            let id = (&*up).fn_id(FnKey::Inst(d, args));
            if !pre.iter().any(|(s, r)| *s == id && r.is_ok()) && !staged.iter().any(|(s, _)| *s == id) {
                if let Ok(code) = compile_guarded(up, id) { pre.push((id, Ok(code))); }
            }
        }
        let pairs: Vec<(u32, u32)> = {
            let u = &*up;
            let is_body = |k: &FnKey| matches!(k, FnKey::Closure(_, ce, _) if *ce & crate::tcx::ASYNC_FN_BODY != 0);
            let mut v = Vec::new();
            let parents = rebuilt.iter().copied().chain(u.slots_of_defs(removed_defs.iter().copied()));
            let new_closures = u.closures_of_files(&new_files);
            // Values can retain any earlier constructor, including one rebound
            // by a previous edit. Compare those original states to this version
            // too, rather than leaving them on the first replacement's code.
            let old_files: Vec<u32> = u.prog.file_paths.iter().enumerate().filter_map(|(i, path)| {
                (!new_files.contains(&(i as u32)) && news.iter().any(|(old, _)| &u.prog.file_paths[*old as usize] == path)).then_some(i as u32)
            }).collect();
            let replaced_paths: Vec<String> = removed_defs.iter().map(|&d| u.prog.def_path(d)).collect();
            let mut old_closures = u.closures_of_slots(parents);
            old_closures.extend(u.closures_of_files(&old_files));
            old_closures.sort_unstable();
            old_closures.dedup();
            for o in old_closures {
                let fo = &u.fns[o as usize];
                trace_count("slot_visits", 1);
                let (FnKey::Closure(of, oce, op), true) = (&fo.key, is_body(&fo.key) && fo.compiled && !matches!(u.forward_of(o), Some((_, false)))) else { continue };
                let replaced = matches!(&u.fns[*op as usize].key, FnKey::Inst(d, _) if removed_defs.contains(d) || replaced_paths.contains(&u.prog.def_path(*d)));
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
            // Equal frame sizes do not make outputs or owned captures interchangeable:
            // an old caller still uses its original output ABI and drop glue.
            let interface = |id: u32| -> Option<(TyId, Vec<TyId>)> {
                let ty = u.tcx.coro_slot_ty.get(&id)?;
                let crate::types::TyKind::Closure(_, _, args) = u.tcx.tys.kind(ty) else { return None };
                Some((args.closure_sig_as_fn_ptr_ty, u.closure_storage(ty).fields))
            };
            let prefix = |id: u32| -> Option<Vec<Vec<u8>>> {
                let FnKey::Closure(file, ce, _) = &u.fns[id as usize].key else { return None };
                let pf = &u.prog.files[*file as usize];
                let blk = pf.ast.block(crate::ast::BlockId(*ce & !crate::tcx::ASYNC_FN_BODY));
                // A future with no await has no suspended body state. Its captured
                // arguments and output still have to match, as checked above.
                let last = pf.toks.iter().filter(|t| t.lo >= blk.lo && t.hi <= blk.hi && &pf.src[t.lo as usize..t.hi as usize] == b"await").map(|t| t.hi).max().unwrap_or(blk.lo);
                Some(toks_norm(pf, blk.lo, last, &[]))
            };
            match (shape(o), shape(n), prefix(o), prefix(n)) {
                (Some(a), Some(b), Some(pa), Some(pb)) if a == b && pa == pb && interface(o).is_some() && interface(o) == interface(n) => {
                    rebind.push((o, n));
                    out.lines.push(format!("suspended `{}` values resume in the new code (same state up to the last await)", u.fns[o as usize].name));
                }
                _ => out.lines.push(format!("kept: suspended `{}` values finish on their old code (its code before an await, saved state, captures or output changed)", u.fns[o as usize].name)),
            }
        }
    }
    // a rebound poll fn's slot runs the new body too
    flagged.extend(rebind.iter().map(|&(o, _)| o));
    trace_count("compiled_slots", staged.len() + pre.len());
    trace_stage("compile");
    let t3 = Instant::now();
    out.ms[2] = (t3 - t2).as_secs_f64() * 1e3;
    let u = &mut *up;
    // A layout that changes under the running code: an enum's variants (code built before can
    // meet a variant it does not know, a match falls through, and the discriminants it reads may
    // have moved) or a struct taking its new layout. A struct edit (M10) otherwise keeps the
    // running layout -- its first fields stay where they are and added fields live in the
    // extension block -- so code built before it stays valid for every value and its frames run
    // on. No program thread may be in code the patch rebuilds (built for the old layout) when it
    // installs. The threads stand still from here to the install (pause_for_patch), so none enters
    // that code before the patch is in; one in it, or a stack not read to its end, makes the edit a
    // restart: nothing installs, the running code goes on as it was, and the restart is offered.
    // (An enum patch installed first and held such a thread for good: its report paused the whole
    // app with nobody told, and a quit waited for the paused main forever, 11-08.)
    let breaking: Vec<DefId> = saved_adts.iter().filter(|(d, old)| old.is_enum || fresh_structs.iter().any(|x| x.0 == *d)).map(|(d, _)| *d).collect();
    let mut pre_paused = false;
    if !breaking.is_empty() && errs.is_empty() {
        pre_paused = crate::inspect::instrument::pause_for_patch();
        let rebuilt: crate::fxhash::HashSet<u32> = slots.iter().copied().collect();
        let pos = layout_pos.iter().find(|x| x.0 == breaking[0]).map_or("", |x| x.1.as_str());
        let enums = saved_adts.iter().any(|(d, old)| old.is_enum && breaking.contains(d));
        let (what, holds) = if enums { ("enum", "was built for its old variants") } else { ("struct", "may hold values of it in the old layout") };
        let names: Vec<String> = breaking.iter().map(|&d| def_name(&u.prog, d)).collect();
        let why = match crate::invalid::frame_in(u, &rebuilt) {
            Ok(None) => None,
            Ok(Some(running)) => Some(format!("{pos}{what} `{}` changed its layout, and {running}: its frame {holds} -- restart the app", names.join("`, `"))),
            Err(e) => Some(format!("{pos}{what} `{}` changed its layout, and the program's stacks could not be read to see that no frame runs code built for the old one ({e}) -- restart the app", names.join("`, `"))),
        };
        if let Some(why) = why {
            errs.push(why);
            as_restart = true;
        }
    }
    if !errs.is_empty() {
        if pre_paused {
            crate::inspect::instrument::resume_program();
        }
        // nothing installs: the running versions come back, and everything built from the
        // refused text is dropped (built again from the running text when needed)
        u.flush_cache_records();
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
        recollect_impls(u, &removed_defs, scope.as_deref());
        restore_traits(u, &saved_traits, scope.as_deref());
        let _ = redeclare(u, &redeclared, scope.as_deref());
        publish_reload(u, &news, full_inputs, scope.as_deref());
        if carry && !ctfe_relevant {
            let files: Vec<u32> = news.iter().flat_map(|&(a, b)| [a, b]).collect();
            carry_gandalf(u, from, &visible_crates, if full_inputs { None } else { Some(&files) });
        }
        unsafe { u.prog.reclaim_caches(); }
        // the running enum variants and struct layouts come back with the versions, under the
        // same write: a background type check reads them (and the declarations they publish)
        for (d, had) in saved_variants {
            u.prog.restore_variants(d, &had);
        }
        if !saved_adts.is_empty() {
            for (d, a) in saved_adts {
                if a.is_enum { u.tcx.insert_ctor_sigs(d, &a); }
                u.tcx.adts.insert(d, a);
            }
            u.tcx.publish_collected();
            u.lay.clear();
            out.lines.retain(|l| !l.starts_with("field `"));
        }
        drop(fw);
        if rekeyed { crate::cache::rekey_after_revert(up); }
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
            // (a body of the refused text; one of an older version stays: that version's closures
            // are lowered from it, and only it can give them their bodies)
            if u.bodies.get(&id).is_some_and(|b| news.iter().any(|&(_, n)| n == b.file)) {
                u.bodies.remove(&id);
            }
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
        out.rung = if as_restart { Rung::Restart } else { Rung::Refuse };
        out.lines.extend(errs);
        if as_restart && hold {
            if let Some(r) = hold_back(up, &edits, &out.lines, t0, retries, base) { return r; }
        }
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
                let mut r = apply_locked(up, retry, Vec::new(), t0, 2, retries - 1, module, base);
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
    crate::zone_alloc::kept_with(|| for &(_, n) in &news { u.file_gen.insert(n, gen); });
    // the rebound older versions run the new ones through forwarders (jit::install_forward): per
    // new slot, its lineage's cell jumps to it, and every other older version paired with it jumps
    // to the cell (and is never looked at again). A lineage without a cell yet makes its oldest
    // version the cell. Cells go first: a version that becomes a forwarder may be what its cell
    // jumps to until then (no jump cycle at any moment).
    let forwards: Vec<(u32, u32, bool)> = {
        let u = &*up;
        let mut targets: Vec<u32> = rebind.iter().map(|x| x.1).collect();
        targets.sort_unstable();
        targets.dedup();
        let (mut cells, mut rest) = (Vec::new(), Vec::new());
        for n in targets {
            let mut olds: Vec<u32> = rebind.iter().filter(|x| x.1 == n).map(|x| x.0).collect();
            olds.sort_unstable();
            let cell = olds.iter().copied().find(|&o| u.forward_of(o).is_some()).unwrap_or(olds[0]);
            for o in olds {
                if o == cell || u.forward_of(o).is_some() { cells.push((o, n, true)); } else { rest.push((o, cell, false)); }
            }
        }
        cells.extend(rest);
        cells
    };
    trace_count("forwarders", forwards.len());
    // (a layout patch installs under the pause its frames were asked in: `breaking` above)
    let paused = pre_paused;
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
        let installed = if homes.contains(&id) { jit::install_unforwarded(up, id, &c.0, c.1, &c.2, gen) } else { jit::install_patched(up, id, &c.0, c.1, &c.2, gen) };
        match installed {
            Ok(_) => {
                let u = &*up;
                out.names.push(u.fns[id as usize].name.to_string());
            }
            // linking cannot fail for code that compiled (a full code region is fatal anyway)
            Err(e) => out.lines.push(format!("link error in {}: {}", (&*up).fns[id as usize].name, e)),
        }
    }
    for (id, target, cell) in forwards {
        if let Err(e) = jit::install_forward(up, id, target, cell, gen) {
            out.lines.push(format!("link error in {}: {}", (&*up).fns[id as usize].name, e));
        }
    }
    // (the instances that resolve to another fn now: no lineage, each jumps to its fn)
    for &(id, target) in &retarget {
        let u = &*up;
        if !matches!(u.fns[id as usize].key, FnKey::Plain(_)) {
            out.lines.push(format!("forwarded: `{}` runs `{}`", u.fns[id as usize].name, u.fns[target as usize].name));
        }
        if let Err(e) = jit::install_forward(up, id, target, false, gen) {
            out.lines.push(format!("link error in {}: {}", (&*up).fns[id as usize].name, e));
        }
    }
    // the new bodies are installed: the guards of their copies inlined before call them now
    for &id in &flagged {
        jit::set_patched(&*up, id);
        // (which running code that reaches: the fns whose code was built with a copy of it)
        let u = &*up;
        let hosts = &u.fns[id as usize].history().copy_hosts;
        if !hosts.is_empty() {
            let mut names: Vec<&str> = hosts.iter().take(4).map(|&h| &u.fns[h as usize].name[..]).collect();
            if hosts.len() > 4 { names.push(".."); }
            out.lines.push(format!("guarded copies: `{}` was inlined into {} fn(s), whose copies call it now: {}", u.fns[id as usize].name, hosts.len(), names.join(", ")));
        }
    }
    let u = &mut *up;
    // the patch event (flight recorder `reload.patch`; inspect `stale` names it)
    if !saved_adts.is_empty() {
        LAYOUT_GROUP.store(gen, std::sync::atomic::Ordering::Relaxed);
    }
    let layout_names: Vec<String> = saved_adts.iter().map(|(d, _)| def_name(&u.prog, *d)).collect();
    {
        let mut shown: Vec<&str> = out.names.iter().take(4).map(String::as_str).collect();
        if out.names.len() > 4 { shown.push(".."); }
        crate::invalid::patch_event(gen, format!("gen {}: {} fn(s) [{}]{}", gen, out.names.len(), shown.join(", "), if layout_names.is_empty() { String::new() } else { format!("; layout of {}", layout_names.join(", ")) }));
    }
    // a layout patch: the earlier patch versions of the slots it rebuilt were built for the old
    // layout and never run again (invalid.rs; a slot's first code shares pages with other code and
    // is not invalidated: only frames live at the patch can still reach it -- none was, asked under
    // this pause, but a thread the pause did not stop may have entered it since: held for good)
    if !breaking.is_empty() {
        let breaking_names: Vec<String> = breaking.iter().map(|d| def_name(&u.prog, *d)).collect();
        let rebuilt: std::collections::HashSet<u32> = u.patch_log.iter().filter(|(_, g)| *g == gen).map(|(id, _)| *id).collect();
        let why = format!("layout-changing patch gen {} (fields or variants of {}) replaced it: code built for the old layout never runs again", gen, breaking_names.join(", "));
        crate::invalid::invalidate(u, |g| matches!(g, crate::invalid::Group::Patch { slot, gen: g0 } if g0 < gen && rebuilt.contains(&slot)), gen, "invalidated code", None, &why);
        let held = crate::invalid::hold_old_frames(u, gen, &breaking_names.join(", "), paused);
        if held > 0 {
            out.lines.push(format!("held: {} thread(s) with a frame in the first-version code of a rebuilt fn: the app runs on without them, the restart is offered (makepad inspect {} stale)", held, std::process::id()));
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
    for d in &consts_hit {
        if !const_defs.contains(d) {
            out.lines.push(format!("note: const `{}` is evaluated again (it depends on a changed const)", def_name(&u.prog, *d)));
        }
    }
    out.files = olds.iter().map(|o| news.iter().find(|(x, _)| x == o).map_or(*o, |(_, n)| *n)).collect();
    out.undo = Some(Undo { rekeyed, group: gen, versions: news.clone(), pairs: news_pairs.clone(), added: added_defs.clone(), removed: removed_defs.clone(), consts: saved_consts, consts_hit: consts_hit.clone(), adts: saved_adts, variants: saved_variants, const_failed: saved_failed, full_inputs, scope: scope.clone(), uses: std::mem::take(&mut use_undo), traits: saved_traits, redeclared });
    out.rung = Rung::Patch;
    if let Some(fv_cx) = fv_cx { fv_session_keep(transaction, fv_cx); }
    trace_count("inlining_callers", inlining_callers.len());
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
/// Why this process's unit cannot be trusted (None: it can): a compiler panic inside an edit's
/// transaction, or an internal compiler error at a call after edits. No edit applies to it again:
/// the recovery is a next run (run_end::restart: the same sources, the warm cache, the app's state
/// when it left one).
static UNIT_SUSPECT: relax_session::sync::Mutex<Option<String>> = relax_session::sync::Mutex::new(None);

pub(crate) fn mark_unit_suspect(why: &str) {
    let _k = crate::zone_alloc::kept();
    let mut s = UNIT_SUSPECT.lock().unwrap_or_else(|e| e.into_inner());
    if s.is_none() { *s = Some(why.lines().next().unwrap_or("").to_string()); }
}

pub(crate) fn unit_suspect() -> Option<String> {
    UNIT_SUSPECT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
/// A restart is due (an edit needs one); the program polls this at its safe point.
static RESTART_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The app is pinned: the live watch stands still and a rebuilt makepad waits.
static PINNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Unpinned: the watch's next round looks at every file.
static CATCH_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The last armed real-time root was disarmed: the watch retries the edits it held now.
static RETRY_NOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// std::relax::Realtime::arm's last guard went: edits refused as frozen apply now.
pub(crate) fn realtime_disarmed() {
    RETRY_NOW.store(true, std::sync::atomic::Ordering::Release);
    watch_events::kick();
}

/// `inspect pin on|off`: while on, no edit or landing reaches the running app and it does not
/// restart; off, it catches up (patches, or the rebuilt makepad that waited). Returns the state.
pub(crate) fn set_pinned(on: bool) -> bool {
    let was = PINNED.swap(on, std::sync::atomic::Ordering::AcqRel);
    if was && !on {
        CATCH_UP.store(true, std::sync::atomic::Ordering::Release);
        host::unpinned();
        watch_events::kick();
    }
    on
}

pub(crate) fn pinned() -> bool {
    PINNED.load(std::sync::atomic::Ordering::Acquire)
}

/// A landing the train holds back (`inspect hold`: devhub's show of the app, until the landed tip
/// is proven for it): each of its files (absolute) -> the git blob id the landing wrote there (None:
/// it removed the file), and whether the watch said it waits. A change that leaves a file saying
/// just that waits; any other content of it, and every other file, is the person's own save and
/// applies at once (the file is the person's from then on). Meanwhile a read of a held file (a
/// module loaded late, an include a save reads again) gives its text from before the landing
/// (`vfs::hold_read`): a held app never runs the landed code. `hold take` moves what a held file
/// reads to a proven tip's text, which the next round applies (the third field, until then).
/// `hold off` lets the landing in (one whole scan). A run's end in place drops it: the restarted
/// run reads every file as it is.
static LANDING_HOLD: relax_session::sync::Mutex<std::collections::BTreeMap<String, (Option<[u8; 20]>, bool, bool)>> = relax_session::sync::Mutex::new(std::collections::BTreeMap::new());

/// Files a take or a release of a held landing names: the watch's next round looks at each, whatever
/// its stamp says (a taken file's text applied under the stamp the landing left on disk).
static LANDING_FORCED: relax_session::sync::Mutex<Vec<String>> = relax_session::sync::Mutex::new(Vec::new());

/// `inspect hold files=...`: holds back these files of a landing (workspace-relative, each with
/// the blob id it lands at, None when it removes the file), with those it holds already. It comes
/// before the landing moves the checkout (devhub's fast-forward waits for it), so a file held now
/// says on disk what the app was built from: its reads give that until the release. A file held
/// already keeps the text it held (a later landing of it changes only what it lands at). Returns
/// how many it holds.
pub(crate) fn hold_landing(files: &[(String, Option<[u8; 20]>)]) -> Result<usize, String> {
    let workspace = WATCH_INPUTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|v| v.inputs.workspace())
        .ok_or("this run watches no sources (not a live session, or its watcher ended)")?;
    let mut held = LANDING_HOLD.lock().unwrap_or_else(|e| e.into_inner());
    for (rel, oid) in files {
        let path = crate::vfs::normalize_path(&format!("{workspace}/{rel}"));
        if !held.contains_key(&path) {
            let before = match crate::vfs::read_unheld(&path) {
                Ok(text) => Some(std::sync::Arc::<[u8]>::from(text)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(format!("{rel}: {e}")),
            };
            crate::vfs::hold_read(&path, before);
        }
        let take = held.get(&path).is_some_and(|h| h.2);
        held.insert(path, (*oid, false, take));
    }
    eprintln!("makepad live: a landing is held back: {} file(s) wait while they say what it wrote; other saves apply", held.len());
    Ok(held.len())
}

/// `inspect hold take dir=... files=...`: the held landing moves to a proven tip P (devhub's show,
/// while newer landings wait): each file (workspace-relative) with its blob id at P (None: P has
/// no such file), P's text at `<dir>/<file>`, read here. A file the landing writes as P has it is
/// held no more (its disk text applies); any other reads P's text (or is gone), which the next
/// round applies, and stays held at what it lands at: `hold off` brings the rest in. A file not
/// held is the person's and skipped; a file not listed keeps its text. Returns (held, taken).
pub(crate) fn take_landing(dir: &str, files: &[(String, Option<[u8; 20]>)]) -> Result<(usize, usize), String> {
    let workspace = WATCH_INPUTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|v| v.inputs.workspace())
        .ok_or("this run watches no sources (not a live session, or its watcher ended)")?;
    // every text read and checked first: an error takes nothing
    let mut texts = Vec::new();
    for (rel, oid) in files {
        let text = match oid {
            None => None,
            Some(id) => {
                let data = std::fs::read(format!("{dir}/{rel}")).map_err(|e| format!("{rel}: {e}"))?;
                if crate::vfs::blob_oid(&data) != *id { return Err(format!("{rel}: its text in {dir} is not the blob id given")); }
                Some(std::sync::Arc::<[u8]>::from(data))
            }
        };
        texts.push((crate::vfs::normalize_path(&format!("{workspace}/{rel}")), *oid, text));
    }
    let mut held = LANDING_HOLD.lock().unwrap_or_else(|e| e.into_inner());
    let mut taken = Vec::new();
    for (path, oid, text) in texts {
        let Some(h) = held.get_mut(&path) else { continue };
        if h.0 == oid {
            held.remove(&path);
            crate::vfs::release_read(Some(&path));
        } else {
            crate::vfs::hold_read(&path, text);
            h.2 = true;
        }
        taken.push(path);
    }
    let n = held.len();
    drop(held);
    eprintln!("makepad live: the held landing takes {} file(s) at a proven tip; {n} still held", taken.len());
    let t = taken.len();
    if t > 0 {
        LANDING_FORCED.lock().unwrap_or_else(|e| e.into_inner()).extend(taken);
        watch_events::kick();
    }
    Ok((n, t))
}

/// `inspect hold off`: the held landing goes in (one whole scan). Returns how many files it held.
pub(crate) fn release_landing() -> usize {
    let held = std::mem::take(&mut *LANDING_HOLD.lock().unwrap_or_else(|e| e.into_inner()));
    crate::vfs::release_read(None);
    let n = held.len();
    if n > 0 {
        eprintln!("makepad live: the held landing goes in ({n} file(s))");
        LANDING_FORCED.lock().unwrap_or_else(|e| e.into_inner()).extend(held.into_keys());
        CATCH_UP.store(true, std::sync::atomic::Ordering::Release);
        watch_events::kick();
    }
    n
}

/// A taken file's text goes into this round (`hold take`): whether `path` had one waiting.
fn untake(path: &str) -> bool {
    LANDING_HOLD.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&crate::vfs::normalize_path(path)).is_some_and(|h| std::mem::take(&mut h.2))
}

pub(crate) fn landing_held() -> usize {
    LANDING_HOLD.lock().unwrap_or_else(|e| e.into_inner()).len()
}

/// Whether the change of `path` waits: it says what a held landing wrote there. Its stamp stays,
/// so the scan after `hold off` sees it changed. Once `settled` (the save's quiet window is over)
/// on other text, the file is the person's: it is held no more.
fn held_back(path: &str, settled: bool) -> bool {
    let key = crate::vfs::normalize_path(path);
    let Some((landed, _, take)) = LANDING_HOLD.lock().unwrap_or_else(|e| e.into_inner()).get(&key).copied() else { return false };
    if crate::vfs::read_unheld(path).ok().map(|data| crate::vfs::blob_oid(&data)) != landed {
        if settled {
            LANDING_HOLD.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
            crate::vfs::release_read(Some(&key));
            eprintln!("makepad live: {path}: the person's save of a held landing's file applies; it is held no more");
        }
        return false;
    }
    // its reads moved to a proven tip's text (`hold take`): that applies, the landing still waits
    if take { return false; }
    let mut held = LANDING_HOLD.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, said, _)) = held.get_mut(&key).filter(|h| !h.1) {
        *said = true;
        eprintln!("makepad live: {path} waits: it says what the held landing wrote (`inspect hold off` lets it in)");
    }
    true
}
static RESTART_REASON: relax_session::sync::Mutex<String> = relax_session::sync::Mutex::new(String::new());
/// When the edit that asked for the restart was saved (unix ms): the restarted run's report
/// measures save -> back on screen from it (MAKEPAD_RUN_RESTART_T0).
static RESTART_SAVED_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DOMAIN_RESTORED: relax_session::sync::Mutex<Option<String>> = relax_session::sync::Mutex::new(None);
static DOMAIN_LOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static DOMAIN_INPUTS: relax_session::sync::Mutex<Option<Inputs>> = relax_session::sync::Mutex::new(None);
/// What the watcher watches, also readable while it waits for a live test verdict: its reload
/// inputs and the program's own source files loaded so far (`input_paths`, inspect `inputs`).
/// Read without the owner: asked for the loaded files, an `inputs` waited behind the program's
/// requests (a cold Fab's first edit: 0.5 s of its ship, at the first frames, 11-04).
struct WatchView {
    inputs: std::sync::Arc<Inputs>,
    loaded: std::sync::Arc<Vec<String>>,
}
static WATCH_INPUTS: relax_session::sync::Mutex<Option<WatchView>> = relax_session::sync::Mutex::new(None);
/// Told when the watcher's inputs are set, or when it ends without them ([`WATCH_GONE`]).
static WATCH_READY: relax_session::sync::Condvar = relax_session::sync::Condvar::new();
/// The live session's watcher could not start, or ended: nothing waits for its inputs.
static WATCH_GONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

type WatchFile = (String, u32, Option<(std::time::SystemTime, u64)>);

fn set_watch_inputs(inputs: &Inputs, files: &[WatchFile]) {
    let inputs = crate::zone_alloc::kept_with(|| std::sync::Arc::new(inputs.clone()));
    *WATCH_INPUTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(WatchView { inputs, loaded: loaded_of(files) });
    WATCH_READY.notify_all();
}

/// The watcher's files changed which of them the program loaded (a rewatch, a patch).
fn set_watch_loaded(files: &[WatchFile]) {
    let loaded = loaded_of(files);
    if let Some(v) = WATCH_INPUTS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() { v.loaded = loaded; }
}

fn loaded_of(files: &[WatchFile]) -> std::sync::Arc<Vec<String>> {
    crate::zone_alloc::kept_with(|| std::sync::Arc::new(files.iter().filter(|f| f.1 != u32::MAX).map(|f| f.0.clone()).collect()))
}

fn watch_gone() {
    let _g = WATCH_INPUTS.lock().unwrap_or_else(|e| e.into_inner());
    WATCH_GONE.store(true, std::sync::atomic::Ordering::Release);
    WATCH_READY.notify_all();
}
static DOMAIN_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// End native callbacks into the outgoing domain before its static block can be reclaimed.
pub fn domain_stopping(id: u32) {
    if crate::run::app_domain() != Some(id) { return; }
    DOMAIN_LOADING.store(true, std::sync::atomic::Ordering::Release);
    *PATCH_WAKER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    RESTART_SUPPORTED.store(false, std::sync::atomic::Ordering::SeqCst);
    RESTART_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
}

// the first domain of a restarted run gets the state the previous run left
// (MAKEPAD_RUN_RESTORE, read once: run_end::run_input)
static FIRST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub(crate) fn domain_loaded(inputs: Option<Inputs>) {
    // this run loads the sources as they are: no edit waits for a restart any more
    SOURCES_WAIT.store(false, Ordering::Release);
    clear_restart_pending();
    let mut restored = None;
    if FIRST.swap(false, std::sync::atomic::Ordering::AcqRel) {
        if let Some(path) = crate::run_end::run_input("MAKEPAD_RUN_RESTORE") {
            let _k = crate::zone_alloc::kept();
            restored = std::fs::read_to_string(&path).ok();
            let _ = std::fs::remove_file(&path);
        }
    }
    *DOMAIN_RESTORED.lock().unwrap_or_else(|e| e.into_inner()) = restored;
    *DOMAIN_INPUTS.lock().unwrap_or_else(|e| e.into_inner()) = inputs;
    {
        let mut p = PATCHED.lock().unwrap_or_else(|e| e.into_inner());
        p.files.clear();
        p.items.clear();
    }
    DOMAIN_REVISION.fetch_add(1, std::sync::atomic::Ordering::Release);
    DOMAIN_LOADING.store(false, std::sync::atomic::Ordering::Release);
    watch_events::kick();
}

/// The live watcher looks now (it sleeps on file events while nothing else is in flight).
pub(crate) fn watch_kick() {
    watch_events::kick();
}

/// An embedder wrote sources the watcher cannot hear (files of a mounted store's edits folder: a
/// JIT host's devhub push): the live watcher looks now.
pub fn sources_changed() {
    watch_events::kick();
}


pub extern "C" fn __relax_live_edit_enabled() -> bool {
    crate::jit::host_boundary("__relax_live_edit_enabled", move || {
        crate::layout::LIVE_SESSION.load(std::sync::atomic::Ordering::Relaxed)
    })
}

/// A WM dock app's watcher gate. A dock app runs from a machine's synced checkout of landed work
/// (`MAKEPAD_RUN_DOCK_REVISION` names its `FLEET-REVISION.txt`, which only devhub's sync writes,
/// after it moved the files): a change of its sources is applied (patched, or the app restarts
/// with its state) only in the watch round that follows a new revision. Anything else that
/// writes into that tree -- a lane's build or checkout there, a hand edit -- never reaches the
/// running app: it keeps the synced code and the change waits for the next sync.
pub(crate) struct DockGate {
    revision: std::path::PathBuf,
    applied: String,
    warned: bool,
}

impl DockGate {
    pub(crate) fn from_env() -> Option<DockGate> {
        Some(DockGate::new(std::path::PathBuf::from(std::env::var_os("MAKEPAD_RUN_DOCK_REVISION").filter(|p| !p.is_empty())?)))
    }

    /// The gate of a checkout whose revision file is `revision`, open for what it says now.
    pub(crate) fn new(revision: std::path::PathBuf) -> DockGate {
        let applied = std::fs::read_to_string(&revision).unwrap_or_default();
        DockGate { revision, applied, warned: false }
    }

    /// The checkout's revision moved since the last applied one (taken now): this round applies.
    pub(crate) fn synced(&mut self) -> bool {
        let now = std::fs::read_to_string(&self.revision).unwrap_or_default();
        if now.is_empty() || now == self.applied {
            return false;
        }
        self.applied = now;
        self.warned = false;
        true
    }

    /// Files changed outside a sync: reported once per revision, not applied. A sync writes
    /// the files first and its revision last, so files seen before their revision are given
    /// that long to be a sync's: true when the revision moved (the next watch round, due at once,
    /// applies them).
    pub(crate) fn held<'a>(&mut self, paths: impl Iterator<Item = &'a str>) -> bool {
        if self.warned {
            return false;
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < until {
            let now = std::fs::read_to_string(&self.revision).unwrap_or_default();
            if !now.is_empty() && now != self.applied {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        self.warned = true;
        let paths: Vec<&str> = paths.take(3).collect();
        let message = format!("dock app: {} changed outside a devhub sync of landed work; not applied: the app keeps running the synced code", paths.join(", "));
        eprintln!("makepad live: {message}");
        live_log(&format!("{{\"event\":\"dock_held\",\"message\":{}}}", json_str(&message)));
        false
    }
}

/// The program's wake for its event loop (std::relax::on_patch): called from the watcher
/// thread after a patch is installed, off the unit, so an idle app polls the new
/// generation at once instead of at its next idle tick.
static PATCH_WAKER: relax_session::sync::Mutex<Option<(crate::domain::Callback, extern "C" fn())>> = relax_session::sync::Mutex::new(None);

pub extern "C" fn __relax_on_patch(f: usize) {
    crate::jit::host_boundary("__relax_on_patch", move || {
        let wake = if f == 0 { None } else { crate::domain::callback().map(|scope| (scope, unsafe { std::mem::transmute(f) })) };
        *PATCH_WAKER.lock().unwrap_or_else(|e| e.into_inner()) = wake;
    })
}

fn wake_program() {
    let wake = *PATCH_WAKER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((scope, wake)) = wake { let _ = scope.call(|| wake()); }
}

/// What installed patches changed, for the program (std::relax::patched): what it built from code
/// at run time (makepad's `script_mod!` designs, evaluated from their fns once) it builds again
/// when that code changed. A loaded domain starts with none.
struct Patched {
    /// patches installed
    count: u64,
    /// each source file a patch changed, with the count when one last did
    files: Vec<(u64, String)>,
    /// each item whose body a patch changed (its name), with the count when one last did
    items: Vec<(u64, String)>,
    /// the count of the last patch that changed more than item bodies (an item added or removed,
    /// a const, a layout): to a program that has not seen it every item changed
    whole: u64,
}

impl Patched {
    const fn new() -> Self {
        Self { count: 0, files: Vec::new(), items: Vec::new(), whole: 0 }
    }

    /// A patch of `paths` that changed the bodies of `items` (None: more than bodies).
    fn land(&mut self, paths: impl Iterator<Item = String>, items: Option<Vec<String>>) {
        self.count += 1;
        let n = self.count;
        for path in paths {
            self.files.retain(|(_, q)| *q != path);
            self.files.push((n, path));
        }
        match items {
            Some(items) => for item in items {
                self.items.retain(|(_, q)| *q != item);
                self.items.push((n, item));
            },
            None => self.whole = n,
        }
    }

    /// What changed after count `since`: "<count> <1 when every item>\n", a path per line, then
    /// a tab and an item name per line.
    fn since(&self, since: u64) -> String {
        let mut s = format!("{} {}\n", self.count, u8::from(self.whole > since));
        for (n, path) in &self.files {
            if *n > since { s.push_str(path); s.push('\n'); }
        }
        for (n, item) in &self.items {
            if *n > since { s.push('\t'); s.push_str(item); s.push('\n'); }
        }
        s
    }
}

static PATCHED: relax_session::sync::Mutex<Patched> = relax_session::sync::Mutex::new(Patched::new());

/// SPEC-V: the slots patch `group` installed without a clean verdict (owner thread); their checks
/// were queued at the install, first in the borrow checker's queue (`RANK_EDIT`).
unsafe fn unverified_patch(up: *mut Unit, group: u32) -> Vec<u32> {
    let u = &*up;
    (0..u.fns.len() as u32).filter(|&id| u.fns[id as usize].patch_gen == group && u.fns[id as usize].unverified.load(Ordering::Relaxed)).collect()
}

/// The names of the items whose bodies an installed patch changed; None when it changed more (an
/// item added or removed, a const, a layout).
unsafe fn changed_items(up: *mut Unit, o: &Outcome) -> Option<Vec<String>> {
    o.test_changes.as_ref().map(|defs| defs.iter().map(|&d| def_name(&(*up).prog, d)).collect())
}

/// A patch of `paths` is installed: noted for the program, which is woken.
fn patch_landed(paths: impl Iterator<Item = String>, items: Option<Vec<String>>) {
    PATCHED.lock().unwrap_or_else(|e| e.into_inner()).land(paths, items);
    wake_program();
}

/// How many patches are installed (std::relax::patched's count): a grab's `patch` that is lower
/// shows a frame drawn before the newest one.
pub fn patches_installed() -> u64 {
    PATCHED.lock().unwrap_or_else(|e| e.into_inner()).count
}

/// What patches changed after count `since` ([`Patched::since`]).
pub extern "C" fn __relax_patched_files(since: u64, buf: *mut u8, cap: usize) -> usize {
    crate::jit::host_boundary("__relax_patched_files", move || {
        copy_out(&PATCHED.lock().unwrap_or_else(|e| e.into_inner()).since(since), buf, cap)
    })
}

/// Whether the program restarts with its state when asked (std::relax::restart_supported).
pub fn restart_keeps_state() -> bool {
    RESTART_SUPPORTED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Asks the program to restart with its state at its safe point (the paused view's "Restart app
/// (keep state)" for a panic parked on another thread than the UI's). Err when it cannot.
pub fn restart_with_state(why: String) -> Result<(), String> {
    if !restart_keeps_state() {
        return Err("the app does not restart with its state (std::relax::restart_supported)".into());
    }
    if let Some(blocked) = handover_blocked() {
        return Err(format!("the app cannot hand its state over: {blocked}"));
    }
    restart_in_place(why).map(|_| ())
}

pub extern "C" fn __relax_restart_supported() {
    crate::jit::host_boundary("__relax_restart_supported", move || {
        RESTART_SUPPORTED.store(true, std::sync::atomic::Ordering::SeqCst);
    })
}

pub extern "C" fn __relax_restart_requested() -> u32 {
    crate::jit::host_boundary("__relax_restart_requested", move || {
        RESTART_REQUESTED.load(std::sync::atomic::Ordering::Relaxed) as u32
    })
}

fn copy_out(s: &str, buf: *mut u8, cap: usize) -> usize {
    let n = s.len().min(cap);
    if !buf.is_null() && n > 0 {
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), buf, n) };
    }
    s.len()
}

pub extern "C" fn __relax_restart_reason(buf: *mut u8, cap: usize) -> usize {
    crate::jit::host_boundary("__relax_restart_reason", move || {
        let r = RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()).clone();
        copy_out(&r, buf, cap)
    })
}

/// The state the previous run left for this one (MAKEPAD_RUN_RESTORE), read once.
pub extern "C" fn __relax_restored_state(buf: *mut u8, cap: usize) -> usize {
    crate::jit::host_boundary("__relax_restored_state", move || {
        if crate::domain::current_app_domain().is_some() && crate::domain::current_app_domain() == crate::run::app_domain() {
            let state = DOMAIN_RESTORED.lock().unwrap_or_else(|e| e.into_inner());
            return state.as_ref().map_or(usize::MAX, |s| copy_out(s, buf, cap));
        }
        let up = jit::current_unit();
        if up.is_null() { return usize::MAX; }
        let session = unsafe { crate::jit::unit_session(up) };
        let runtime = &session.runtime;
        let s = RESTORED_STATE.get_or_init(|| {
            let p = runtime.restore.clone()?;
            let t = std::fs::read_to_string(&p).ok();
            let _ = std::fs::remove_file(&p);
            t
        });
        match s {
            Some(t) => copy_out(t, buf, cap),
            None => usize::MAX,
        }
    })
}

/// When the edit whose restart started this run was saved (unix ms; 0 for a run no restart started):
/// MAKEPAD_RUN_RESTART_T0 as this run has it (run_end::run_input), for save -> back on screen.
pub extern "C" fn __relax_restart_saved_ms() -> f64 {
    crate::jit::host_boundary("__relax_restart_saved_ms", || {
        crate::run_end::run_input("MAKEPAD_RUN_RESTART_T0").and_then(|v| v.into_string().ok()).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0)
    })
}

/// The program found that part of a live edit did not reach it (std::relax::hot_load_failed: a
/// value it built from the old code cannot take the new one). A failed hot load like a stale
/// patch: recorded, and the restart with the app's state applies it.
pub extern "C" fn __relax_hot_load_failed(why: *const u8, len: usize) {
    crate::jit::host_boundary("__relax_hot_load_failed", move || {
        let why = unsafe { if why.is_null() { String::new() } else { String::from_utf8_lossy(std::slice::from_raw_parts(why, len)).into_owned() } };
        eprintln!("makepad live:   {why}");
        recover_hot_failure(makepad_remote::hot::Outcome::StaleFrame, &[why], None);
    })
}

/// The app's restart with its state: the next run starts with it (MAKEPAD_RUN_RESTORE). Returns only
/// when the restart could not start.
pub extern "C" fn __relax_restart_now(state: *const u8, len: usize) {
    crate::jit::host_boundary("__relax_restart_now", move || {
        let st = unsafe { if state.is_null() { &[][..] } else { std::slice::from_raw_parts(state, len) } };
        // a rebuilt makepad waits: the process restarts into it (or this returns, said why)
        if host::restart_now(st) { return; }
        RESTART_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
        let why = RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match next_run(&why, Some(st)) {
            Ok(()) => crate::run_end::leave(),
            Err(e) => {
                crate::hot_failure::restart_failed(&e);
                eprintln!("makepad live: {e}");
            }
        }
    })
}

/// makepad inspect: an edit that did not reach the running program is an agent's event (refused:
/// errors; restart: the change needs one).
/// SPEC-V: patch groups that are installed and run while their borrow-check verdict is still out.
/// Observations made meanwhile (inspect answers, events) say `verified:false`.
pub(crate) static UNVERIFIED_PATCHES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Whether the code the program runs now all has its clean verdict (no patch group awaits one).
pub fn observations_verified() -> bool {
    UNVERIFIED_PATCHES.load(Ordering::Acquire) == 0
}

/// A patch group whose verdict has not arrived: the slots it installed, and where the event stream
/// stood when it was installed (the events after it were made with its code).
pub(crate) struct PendingPatch {
    pub(crate) group: u32,
    pub(crate) what: String,
    pub(crate) ids: Vec<u32>,
    pub(crate) event_seq: u64,
}

/// MAKEPAD_RUN_SPECV_TRACE: the slots a patch group's verdict waits on, printed when they change
/// (the queue position aside: it moves on every poll).
fn trace_patch_wait(group: u32, waits: &[String]) {
    static LAST: relax_session::sync::Mutex<Vec<(u32, String)>> = relax_session::sync::Mutex::new(Vec::new());
    let stable: String = waits.iter().map(|w| w.split(" queued Some((").next().unwrap_or(w)).collect::<Vec<_>>().join("; ");
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.iter().any(|(g, s)| *g == group && *s == stable) { return; }
    last.retain(|(g, _)| *g != group);
    last.push((group, stable));
    eprintln!("specv @{:.3} patch group {group} waits on {}", crate::jit::wall_ns() as f64 / 1e9, waits.join("; "));
}

/// SPEC-V: settles the patch groups awaiting verdicts. A group whose every slot is clean is verified
/// (an inspect event says which observations that makes verified). A group with a failed verdict is
/// rolled back -- newer groups first, they were made on top of it -- the slots return to their
/// previous code, and the error is reported with the observations it invalidates. Returns the
/// (running file before, after) pairs of the rolled-back groups.
pub(crate) fn settle_verdicts(up: *mut Unit, pending: &mut Vec<PendingPatch>, undos: &mut Vec<Undo>) -> Vec<(u32, u32)> {
    use crate::jit::Verdict;
    let mut back = Vec::new();
    let mut i = 0;
    while i < pending.len() {
        let p = &pending[i];
        // Some(None): verified, Some(Some(events)): rejected, None: not yet
        let mut unchecked: Vec<String> = Vec::new();
        let state: Option<Option<Vec<crate::borrowck::event::GandalfEvent>>> = unsafe {
            crate::owner::call(crate::owner::Class::Program, || {
                let u = &*up;
                let mut failed = Vec::new();
                let mut waiting = false;
                let mut waits: Vec<String> = Vec::new();
                for &id in &p.ids {
                    // an unverified-mode twin is its base's body, never promoted: the base's verdict
                    // is the one to wait for and report (it said "did not check" of every twin)
                    let of = match u.fns[id as usize].key { FnKey::Plain(b) => b, _ => id };
                    match u.verdict_state(of) {
                        Verdict::Clean => {}
                        Verdict::Failed(events) => failed.extend(events),
                        // never verified, never reported: the slots stay unverified (baseline)
                        Verdict::Unverifiable(why) => unchecked.push(format!("{}: {}", u.fns[id as usize].name, why)),
                        Verdict::Pending => {
                            waiting = true;
                            if crate::jit::specv_trace() { waits.push(format!("#{id} {}", u.pending_why(id))); }
                        }
                    }
                }
                if !waits.is_empty() { trace_patch_wait(p.group, &waits); }
                if !failed.is_empty() { Some(Some(failed)) }
                else if waiting { None }
                else {
                    for &id in &p.ids { u.refresh_verified(id); }
                    Some(None)
                }
            })
        };
        match state {
            None => { i += 1; }
            Some(None) => {
                let p = pending.remove(i);
                for why in &unchecked {
                    eprintln!("makepad live: {}: the borrow checker did not check {} (it stays unverified baseline code)", p.what, why);
                }
                UNVERIFIED_PATCHES.fetch_sub(1, Ordering::AcqRel);
                let seqs = crate::inspect::events::unverified_seqs_since(p.event_seq);
                let seqs: Vec<String> = seqs.iter().map(|s| s.to_string()).collect();
                patch_verified_event(&p.what, p.group, &seqs);
            }
            Some(Some(events)) => {
                let group = pending[i].group;
                // newest first, down to the failed group
                let mut reverted = Vec::new();
                let mut names: Vec<String> = Vec::new();
                let mut ok = false;
                while let Some(undo) = undos.pop() {
                    let g = undo.group;
                    let versions = undo.versions.clone();
                    let rolled = unsafe { rollback(up, undo, "the borrow checker rejected it after it ran (SPEC-V)") };
                    names.extend(rolled);
                    back.extend(versions);
                    reverted.push(g);
                    if g == group { ok = true; break; }
                }
                let mut invalidated: Vec<String> = Vec::new();
                let mut first_seq = u64::MAX;
                let what = pending[i].what.clone();
                pending.retain(|q| {
                    if reverted.contains(&q.group) || (!ok && q.group == group) {
                        first_seq = first_seq.min(q.event_seq);
                        UNVERIFIED_PATCHES.fetch_sub(1, Ordering::AcqRel);
                        false
                    } else { true }
                });
                if first_seq != u64::MAX {
                    invalidated = crate::inspect::events::unverified_seqs_since(first_seq).iter().map(|s| s.to_string()).collect();
                }
                let mut rendered: Vec<String> = Vec::new();
                for ev in &events {
                    let r = ev.error.render();
                    if !rendered.contains(&r) { rendered.push(r); }
                }
                let e = &events[0].error;
                eprintln!(
                    "makepad live: {}: REVERTED -- the borrow checker rejected it after it ran: {} at {}:{}:{}: {}; {} fn(s) run their previous code again{}",
                    what, e.fn_path, e.file, e.line, e.col, e.message, names.len(),
                    if ok { "" } else { " (its undo record was already dropped: not reverted)" }
                );
                for r in &rendered { eprintln!("{}", r); }
                let d = e.to_diagnostic();
                live_log(&format!("{{\"event\":\"patch_reverted\",\"edit\":{},\"groups\":{:?},\"fns\":[{}],\"invalidated_events\":[{}],\"diagnostic\":{}}}",
                    json_str(&what), reverted, names.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(","), invalidated.join(","), d.to_json()));
                crate::inspect::events::emit(
                    "patchreverted",
                    &format!("{}|{}", what, group),
                    &format!(
                        "\"edit\":{},\"reverted\":{},\"groups\":{:?},\"fns\":[{}],\"error\":{},\"invalidates\":{{\"events\":[{}]}}",
                        json_str(&what), ok, reverted, names.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(","),
                        json_str(&e.one_line()), invalidated.join(",")
                    ),
                );
                if crate::run::reporting() { crate::run::report("borrowck", &d.to_json()); }
                // the edit's own borrow error (rustc rejects it too): the previous code runs again
                // and the error shows; no restart could run the edit
                i = 0;
            }
        }
    }
    back
}

/// An edit the patcher cannot absorb waits for the person: a running app never restarts by itself.
fn restart_for_edit(what: &str, lines: &[String]) {
    SOURCES_WAIT.store(true, Ordering::Release);
    offer_restart(&format!("{}: {}", what, lines.first().cloned().unwrap_or_default()));
}

/// Source edits wait for a restart: the running program has not taken them (a mod added, a type
/// changed). A later edit is checked against the running program, so what it uses of them is not
/// there: its errors are no errors of the sources (rustc accepts them), and a restart applies it
/// with them. Cleared when a run loads the sources again (`domain_loaded`, a relaunch).
static SOURCES_WAIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The newest patch group that changed a struct's layout (0: none yet).
static LAYOUT_GROUP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// How long a patch's old code may stay on a program thread's stack before the edit counts as
/// not landed there (a call made before the patch that does not return: a loop).
const LANDING: std::time::Duration = std::time::Duration::from_secs(1);

/// The landing check of patch group `gen`: the fn with a frame on a program thread in code made
/// before the patch, if any. A call that started before the patch finishes in its old code; one
/// still there a while later never picks the edit up (a loop in it, or in a caller that inlined
/// the edited code).
///
/// Code from before a struct's layout changed reads the dead slots, so after such an edit a walk
/// that ended short (a thread blocked in a libc call whose caller its CFI does not give) is not
/// enough: the rest of that thread's stack is read for return addresses into the old code. A
/// stale word there costs a restart; a frame missed would draw wrong data with no word said.
fn running_old_code(u: &Unit, gen: u32) -> Option<String> {
    let slots: std::collections::HashSet<u32> = u.patch_log.iter().filter(|(_, g)| *g == gen).map(|(id, _)| *id).collect();
    if slots.is_empty() {
        return None;
    }
    let layout = gen <= LAYOUT_GROUP.load(std::sync::atomic::Ordering::Relaxed);
    let caps = match crate::inspect::capture_all(&crate::inspect::instrument::program_thread) {
        Ok(caps) => caps,
        Err(e) => {
            if layout {
                eprintln!("makepad live: the landing check after a field's change could not read the stacks ({e}): a frame still running code from before the edit reads the old layout -- restart the app if it misbehaves");
            }
            return None;
        }
    };
    let code = u.code.read().unwrap_or_else(|e| e.into_inner());
    // the fn whose code from before the patch holds `a`
    let old = |a: u64| -> Option<String> {
        let (start, _, slot) = code.entry_at(a)?;
        if !slots.contains(&slot) || crate::stale::lookup(a).is_some() {
            return None;
        }
        let f = &u.fns[slot as usize];
        // the patch group that made this code (a replaced version keeps it in the history)
        let made = if start == f.code { f.patch_gen } else { f.history().prev.iter().find(|p| p.0 == start).map_or(gen, |p| p.3) };
        (made < gen).then(|| f.name.to_string())
    };
    for c in caps {
        for (k, &a) in c.frames.iter().enumerate() {
            // pc, then return addresses (the call is the byte before)
            if let Some(name) = old(if k == 0 { a } else { a.wrapping_sub(1) }) {
                return Some(name);
            }
        }
        if layout && c.stop.is_some() && c.sp != 0 {
            if let Some(name) = return_address_above(c.sp, &old) {
                return Some(name);
            }
        }
    }
    None
}

/// What `old` says of the first word of the stack above `sp` it knows as a return address. A
/// stack ends where its mapping does (or at a size no thread here has).
fn return_address_above(sp: u64, old: &dyn Fn(u64) -> Option<String>) -> Option<String> {
    const PAGE: u64 = 4096;
    const MOST: u64 = 64 << 20;
    let mut at = sp & !7;
    while at - sp < MOST {
        let len = PAGE - at % PAGE;
        for word in crate::inspect::threads::read_mem(at, len as usize)?.chunks_exact(8) {
            if let Some(name) = old(u64::from_le_bytes(word.try_into().unwrap()).wrapping_sub(1)) {
                return Some(name);
            }
        }
        at += len;
    }
    None
}

/// `__relax_restored_state`: the state the restart left, read once.
static RESTORED_STATE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Restarts the program to apply `why`, in this process and its window: a program that restarts
/// with its state (std::relax::restart_supported) is asked to at its next safe point and its next
/// run starts with the state; any other program's run ends and the next starts right away, as does
/// one that cannot reach that point ([`handover_blocked`]). The program's sources are compiled by
/// the new run; the loader itself is current (a change of its own inputs goes through
/// host::request), so nothing is rebuilt. Returns how, or why it cannot.
fn restart_in_place(why: String) -> Result<&'static str, String> {
    if crate::harness::live::enabled() {
        return restart_run(&why).map(|_| "test restart");
    }
    *RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()) = why.clone();
    RESTART_CONFIRMED.store(true, std::sync::atomic::Ordering::SeqCst);
    if RESTART_SUPPORTED.load(std::sync::atomic::Ordering::SeqCst) {
        let Some(blocked) = handover_blocked() else {
            RESTART_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
            wake_program();
            return Ok("restart with the program's state");
        };
        eprintln!("makepad live: the restart does not wait for the app's state: {blocked}; the next run starts without it");
        return next_run(&why, None).map(|_| "restart without the program's state");
    }
    match start_state() {
        Some(state) => next_run(&why, Some(state.as_bytes())).map(|_| "restart with the state the run was started with"),
        None => next_run(&why, None).map(|_| "restart"),
    }
}

/// The state this run was started with, while its program has not said that it restarts with its
/// own: an app says so at its first event (std::relax::restart_supported), and until then that
/// state is all of the app's state. A relaunch between a restart's boot and the app's first event
/// took the app for a program with no state and dropped it (restart_in_place.py, 11-08). None:
/// the run was started with none, or the program said (it hands its own).
fn start_state() -> Option<String> {
    if RESTART_SUPPORTED.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    DOMAIN_RESTORED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Why the program cannot hand its state over to a restart now (None: it can): a thread of it is
/// parked at a stale access (stale.rs) and the program is paused for that report, so it never
/// reaches the safe point that would. Its restart is the next run without the state, as `resume
/// P<n> restart` is. (A thread held for good at a layout patch does not stop the rest: the app
/// hands its state over, and the run's end lets that thread go.)
fn handover_blocked() -> Option<String> {
    let stuck = crate::stale::stuck_threads();
    (stuck > 0).then(|| format!("{stuck} thread(s) of it are parked at a stale access and the app is paused for the report (`makepad inspect {} stale`)", std::process::id()))
}

/// The next run starts now, without the app's state (the app is not asked: a test run of `makepad
/// test --live`, an app stopped at a parked panic or at an internal compiler error in a call).
pub fn restart_run(why: &str) -> Result<(), String> {
    *RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()) = why.to_string();
    next_run(why, None)
}

/// The next run starts, with `state` as the app left it. An app's run ends in this process, which
/// keeps its window (`run_end`); this returns and the run's end comes to the thread. A run with no
/// app domain, whose memory is the process's (a terminal program, a test harness), is its process:
/// that starts again, and this returns only when it could not.
fn next_run(why: &str, state: Option<&[u8]>) -> Result<(), String> {
    if crate::run::hosted_run() {
        return crate::run_end::restart(why.to_string(), state);
    }
    exec_restart(state)
}

/// A live edit failed (an internal compiler error applying it, the watcher's own panic, a value
/// built by the old code that the edit cannot reach): recorded, and the restart that applies it is
/// offered -- the app runs on with the code it has, and the watcher stands still, until the
/// person takes the offer (the ring's Update ready, the WM's toast, `session relaunch`); its next
/// run completes the recovery. It never restarts the app on its own: a landing reaches a running
/// app this way, and the app may hold unsaved work, music, a print. A live test session restarts
/// its tests at once (their verdict is the restarted run's).
pub fn recover_hot_failure(kind: makepad_remote::hot::Outcome, lines: &[String], group: Option<u32>) {
    if !crate::hot_failure::failed(kind, lines, group) { return; }
    let why = lines.join("; ");
    if !crate::harness::live::enabled() {
        offer_restart(&why);
        return;
    }
    note_restart_pending(&why);
    if let Err(e) = restart_in_place(why) {
        crate::hot_failure::restart_failed(&e);
        crate::hot_failure::pump();
        if crate::harness::live::enabled() && crate::hot_failure::has_error() {
            std::process::exit(crate::hot_failure::verdict(101).code());
        }
    }
}

/// An internal compiler error at a program thread's call after live edits, in an app the person
/// runs (no live test session): the live update failed in that call. A failed hot load like any
/// other -- recorded, the unit suspect (no edit applies to it again), the restart offered -- and
/// the call parks for the person (jit.rs ice_at_call): the app is never restarted by itself.
/// False: a live test session (recover_compile_ice), or no live edit ran in this process.
pub(crate) fn update_failed_at_call(message: &str) -> bool {
    if crate::harness::live::enabled() || !crate::hot_failure::edit_ran() && unit_suspect().is_none() { return false; }
    crate::hot_failure::failed(makepad_remote::hot::Outcome::Panic, &[message.to_string()], None);
    mark_unit_suspect(message);
    offer_restart(message);
    true
}

/// An internal compiler error at a program thread's call in a live test session (jit.rs
/// compile_ice): after live edits it is a failed hot load like any other -- recorded, the unit
/// suspect, the tests' next run asked for at once (`on_main`: not asked; that thread is in the
/// call, the caller restarts the run itself). False: no recovery started (no edit ran in this
/// process, or a recovery runs already) or asking for the restart failed.
pub(crate) fn recover_compile_ice(message: &str, on_main: bool) -> bool {
    if !crate::hot_failure::edit_ran() || !crate::hot_failure::failed(makepad_remote::hot::Outcome::Panic, &[message.to_string()], None) { return false; }
    mark_unit_suspect(message);
    note_restart_pending(message);
    if on_main { return true; }
    match restart_in_place(message.to_string()) {
        Ok(_) => true,
        Err(e) => {
            crate::hot_failure::restart_failed(&e);
            crate::hot_failure::pump();
            false
        }
    }
}

/// What the host keeps across an exec of this process (the HAL's WM connection: the new image
/// resumes on it, so the WM keeps the app's window): `prepare`, right before exec, readies what
/// it hands over (no CLOEXEC) and returns the environment that names it to the new image;
/// `undo`, when the exec failed, takes it back so this process goes on with it.
type Handover = (fn() -> Vec<(String, String)>, fn());
static BEFORE_EXEC: std::sync::Mutex<Option<Handover>> = std::sync::Mutex::new(None);

/// What the host does when the restart itself begins, on the thread that restarts: the window goes
/// from the app to the start screen (hal_run), before the run's end takes its time.
static RESTART_BEGINS: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// Set once by the runner.
pub fn set_restart_begins(begins: fn()) {
    let _ = RESTART_BEGINS.set(begins);
}

pub(crate) fn restart_begins() {
    if let Some(begins) = RESTART_BEGINS.get() {
        begins();
    }
}

/// The host's handover at an exec ([`BEFORE_EXEC`]); set once by the runner.
pub fn set_before_exec(prepare: fn() -> Vec<(String, String)>, undo: fn()) {
    *BEFORE_EXEC.lock().unwrap_or_else(|e| e.into_inner()) = Some((prepare, undo));
}

/// Readies the host's handover for `cmd`, which execs next (its environment is added).
#[cfg(unix)]
pub(crate) fn hand_over(cmd: &mut std::process::Command) {
    let f = *BEFORE_EXEC.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((prepare, _)) = f {
        for (k, v) in prepare() {
            cmd.env(k, v);
        }
    }
}

/// The exec after [`hand_over`] failed: the host takes its handover back.
#[cfg(unix)]
pub(crate) fn hand_back() {
    let f = *BEFORE_EXEC.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, undo)) = f {
        undo();
    }
}

/// What the next run reads (run_end::run_input; a value, or none): the recovery on its way, `state`
/// as the app left it, when the edit behind this was saved. A process started for the next run
/// finds it in its environment, a run started in place in the restart's hand-over.
pub(crate) fn next_run_env(state: Option<&[u8]>) -> Result<Vec<(&'static str, Option<std::ffi::OsString>)>, String> {
    let recovery = crate::hot_failure::hand_over()?.map(|path| path.into_os_string());
    let restore = match state {
        Some(st) => Some(restart_state_file(st)?.into_os_string()),
        None => None,
    };
    let saved = RESTART_SAVED_MS.load(std::sync::atomic::Ordering::Relaxed).to_string();
    // the crates the next run lays out for live edits beyond the app scope (live_crates_next), one
    // root path a line
    let live = live_crates_next();
    let live = (!live.is_empty()).then(|| std::ffi::OsString::from(live.join("\n")));
    Ok(vec![("MAKEPAD_JIT_RECOVERY", recovery), ("MAKEPAD_RUN_RESTORE", restore), ("MAKEPAD_RUN_RESTART_T0", Some(saved.into())), ("MAKEPAD_RUN_LIVE_CRATES", live)])
}

/// A restart that waited out the run's boot (run_end::restart) and hands no state of its own: the
/// next run gets the state this run was started with, which its program barely began with (a
/// relaunch 3 s after a restart with the app's state dropped it). Read already by the domain's
/// load ([`domain_loaded`]), it goes in a file of its own again; else its file is still there.
pub(crate) fn carry_restore(next: &mut [(&'static str, Option<std::ffi::OsString>)]) {
    let Some(slot) = next.iter_mut().find(|(k, v)| *k == "MAKEPAD_RUN_RESTORE" && v.is_none()) else { return };
    let loaded = DOMAIN_RESTORED.lock().unwrap_or_else(|e| e.into_inner()).clone();
    slot.1 = match loaded {
        Some(text) => restart_state_file(text.as_bytes()).map_err(|e| eprintln!("makepad live: the state this run was started with is lost: {e}")).ok().map(|p| p.into_os_string()),
        None => crate::run_end::run_input("MAKEPAD_RUN_RESTORE").filter(|p| std::path::Path::new(p).exists()),
    };
    if slot.1.is_some() {
        eprintln!("makepad live: the state this run was started with goes to the next run");
    }
}

/// The app's state for the next run (MAKEPAD_RUN_RESTORE), in a file of its own, which that run
/// removes when it reads it: made new in this user's own directory (makepad_remote's socket dir),
/// so no other user can read it, put it there first or point it elsewhere.
pub(crate) fn restart_state_file(st: &[u8]) -> Result<std::path::PathBuf, String> {
    #[cfg(unix)]
    let dir = makepad_remote::carrier::unix::own_socket_dir().map_err(|e| format!("cannot write the state: {e}"))?;
    #[cfg(not(unix))]
    let dir = makepad_remote::carrier::unix::socket_dir();
    let path = dir.join(format!("relax-restart-state-{}-{}", std::process::id(), crate::clock::wall_ns()));
    write_new_private(&path, st).map_err(|e| format!("cannot write the state {}: {e}", path.display()))?;
    Ok(path)
}

/// `bytes` in a file made at `path` (0600 on Unix), never one that is there already, nor a link's
/// target: another file or link at `path` is an error.
fn write_new_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(path)?.write_all(bytes)
}

#[cfg(all(test, unix))]
#[test]
fn a_restarts_state_file_is_made_new_never_through_a_link() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("relax-state-file-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("target");
    std::fs::write(&target, b"keep").unwrap();
    let link = dir.join("state-link");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(write_new_private(&link, b"state").is_err(), "a link at the path is refused");
    assert_eq!(std::fs::read(&target).unwrap(), b"keep", "the link's target is untouched");
    assert!(write_new_private(&target, b"state").is_err(), "a file at the path is refused");
    let fresh = dir.join("state-fresh");
    let _ = std::fs::remove_file(&fresh);
    write_new_private(&fresh, b"state").unwrap();
    assert_eq!(std::fs::read(&fresh).unwrap(), b"state");
    assert_eq!(std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777, 0o600);
    for f in [&fresh, &link, &target] {
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_dir(&dir);
    // the restart's own: in this user's 0700 directory, 0600
    let state = restart_state_file(b"state").unwrap();
    let own = makepad_remote::carrier::unix::socket_dir();
    assert_eq!(state.parent(), Some(own.as_path()));
    assert_eq!(std::fs::metadata(&own).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(&state).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::read(&state).unwrap(), b"state");
    std::fs::remove_file(&state).unwrap();
}

/// A restart that waited out the run's boot and has no state of its own hands on the state the
/// run was started with (a relaunch 3 s after a restart with the app's state dropped it): read
/// by the domain's load already, it goes in a file of its own again. A restart's own state stays.
#[cfg(all(test, unix))]
#[test]
fn a_run_ended_in_its_boot_hands_on_the_state_it_was_started_with() {
    isolated_test!(a_run_ended_in_its_boot_hands_on_the_state_it_was_started_with);
    *DOMAIN_RESTORED.lock().unwrap_or_else(|e| e.into_inner()) = Some("tick 41".into());
    let mut next = vec![("MAKEPAD_JIT_RECOVERY", None), ("MAKEPAD_RUN_RESTORE", None)];
    carry_restore(&mut next);
    let carried = next[1].1.clone().expect("the state the run was started with goes on");
    assert_eq!(std::fs::read_to_string(&carried).unwrap(), "tick 41");
    assert_eq!(next[0].1, None, "only the state is handed on");
    std::fs::remove_file(&carried).unwrap();
    let own = std::ffi::OsString::from("its own state");
    let mut next = vec![("MAKEPAD_RUN_RESTORE", Some(own.clone()))];
    carry_restore(&mut next);
    assert_eq!(next[0].1, Some(own), "a restart with a state of its own hands that on");
}

/// A run with no app domain starts again as a process: the loader binary with the same command
/// line. An exec in place; Windows has none, there its parent process starts the next run
/// (restart_parent.rs). Returns only on failure.
fn exec_restart(state: Option<&[u8]>) -> Result<(), String> {
    #[cfg(any(unix, windows))]
    {
        let exe = host::loader().ok_or("the makepad binary's path is unknown")?;
        let mut cmd = std::process::Command::new(exe);
        cmd.args(crate::harness::live::restart_args(std::env::args_os().skip(1).collect()));
        for (name, value) in next_run_env(state)? {
            match value {
                Some(value) => { cmd.env(name, value); }
                None => { cmd.env_remove(name); }
            }
        }
        // the run's end as at exit: compiles stop and this run's code goes into the disk cache,
        // so the restarted run links it instead of compiling it again (exec skips the exit handlers)
        crate::run::summary("restart");
        // a restart is a clean end of this process (its transcript is not kept)
        crate::tui::exit_code(0);
        crate::tui::finish(false);
        eprintln!("makepad live: restarting ({})", RESTART_REASON.lock().unwrap_or_else(|e| e.into_inner()));
        #[cfg(unix)]
        let e = std::os::unix::process::CommandExt::exec(&mut cmd);
        #[cfg(windows)]
        let e = crate::restart_parent::request(&cmd);
        Err(format!("restart failed: {e}"))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = state;
        Err("this platform cannot start the process again in place".into())
    }
}

/// A restart is ready: noted for `session relaunch` (the JIT ring's Update ready notice, a click
/// on it asks for it), and in the live log for the WM's toast (devhub follows it).
pub(crate) fn offer_restart(why: &str) {
    note_restart_pending(why);
    eprintln!("makepad live: {why}: a restart applies it; offered (Update ready on the JIT ring, the WM's toast, or `makepad inspect <app> session relaunch`); the app keeps running meanwhile");
    live_log(&format!("{{\"event\":\"restart_offered\",\"pid\":{},\"why\":{}}}", std::process::id(), json_str(why)));
}

/// What a change of the sources after the app crashed waits for (offer_restart_after_crash).
pub const CRASHED_SOURCES_CHANGED: &str = "the sources changed since the app crashed";

/// The sources changed after the app crashed (the run window shows its failure): the next run,
/// which runs them, is offered as a failed edit's restart is -- the window's Restart, the WM's
/// toast, `session relaunch` -- and never started on its own: a landing changes the sources as
/// the person's save does. Offered once until a run loads them (every save says it again).
pub fn offer_restart_after_crash() {
    let why = CRASHED_SOURCES_CHANGED;
    if RESTART_PENDING.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p.0 == why) { return; }
    note_restart_pending(why);
    eprintln!("makepad live: {why}: Restart runs them; offered (the run window's Restart, the WM's toast, or `makepad inspect <app> session relaunch`)");
    live_log(&format!("{{\"event\":\"restart_offered\",\"pid\":{},\"why\":{}}}", std::process::id(), json_str(why)));
}

/// A restart the person asked for (`session relaunch`, a restart toast's click): a dock app's
/// waiting rebuilt makepad may take it (`host::restart_now`).
pub(crate) static RESTART_CONFIRMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Edits waiting for a relaunch (`session` in inspect lists them), each with when it began to
/// wait (unix ms).
static RESTART_PENDING: std::sync::Mutex<Vec<(String, u64)>> = std::sync::Mutex::new(Vec::new());

fn note_restart_pending(why: &str) {
    let mut p = RESTART_PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if !p.iter().any(|x| x.0 == why) {
        p.push((why.to_string(), unix_ms(std::time::SystemTime::now()) as u64));
    }
}

/// `session relaunch`: applies the edits waiting for a restart (restart_in_place). Returns how.
/// `who` asked, said in the log with every request: a click in the run window, or the inspect
/// client's process ([`inspect_asker`]). A restart nobody could name read as the person's own
/// click (Scope, Fab and Story restarted for a landing, 11-06).
pub fn relaunch_pending(who: &str) -> Result<&'static str, String> {
    RELAUNCH_ASKED.store(true, Ordering::SeqCst);
    let why = restart_pending().join("; ");
    eprintln!("{}", relaunch_line(who, &why, host::restart_asked()));
    if let Some(result) = host::relaunch() { return result; }
    restart_in_place(if why.is_empty() { "session relaunch".into() } else { why })
}

/// The log's line of a `session relaunch` that `who` asked for, `why` the edits it applies (a
/// repeat while the restart is on its way changes nothing, and says so).
fn relaunch_line(who: &str, why: &str, again: bool) -> String {
    let what = if why.is_empty() { "nothing waits" } else { why };
    if again {
        format!("makepad live: session relaunch, asked by {who} again: the restart is on its way ({what})")
    } else {
        format!("makepad live: session relaunch, asked by {who}: {what}")
    }
}

/// Who sent the `session relaunch` an inspect command answers: the agent its client says it is,
/// and its process and parents from its socket's credentials ([`makepad_remote::cmd::asked_by`]).
pub fn inspect_asker() -> String {
    match makepad_remote::cmd::asked_by() {
        Some(who) => format!("an inspect client, {who}"),
        None => "an inspect client whose process its connection does not name".into(),
    }
}

/// `session relaunch` (a click on the ring's dot) asked for this restart: the window shows it
/// happening. An edit's restart keeps the app's last frame instead.
static RELAUNCH_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn relaunch_asked() -> bool {
    RELAUNCH_ASKED.load(Ordering::SeqCst)
}

/// The edits that wait for a relaunch, oldest first (cleared once a run loads the sources).
pub fn restart_pending() -> Vec<String> {
    RESTART_PENDING.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|p| p.0.clone()).collect()
}

/// The oldest edit that waits for a relaunch: since when (unix ms), and why.
pub fn restart_pending_since() -> Option<(u64, String)> {
    RESTART_PENDING.lock().unwrap_or_else(|e| e.into_inner()).first().map(|p| (p.1, p.0.clone()))
}

/// One reason no longer waits for a restart (a rebuild of makepad that changed nothing).
fn drop_restart_pending(why: &str) {
    RESTART_PENDING.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| p.0 != why);
}

/// The checkouts' commits the run's sources were read at (its start or its last restart): each
/// repository's worktree with its HEAD commit, the workspace's own first, and when (unix ms).
/// `inspect live` says them (loaded_sha), beside the edits that wait for a restart: an app's show
/// listed a checkout's tip while the app ran code nine hours older (11-07).
static LOADED_HEADS: std::sync::Mutex<Option<(Vec<(String, String)>, u64)>> = std::sync::Mutex::new(None);

pub fn loaded_heads() -> Option<(Vec<(String, String)>, u64)> {
    LOADED_HEADS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A run loaded the sources as they are (`domain_loaded`): no edit waits for a restart any more.
fn clear_restart_pending() {
    let old = std::mem::take(&mut *RESTART_PENDING.lock().unwrap_or_else(|e| e.into_inner()));
    drop(old);
}

/// A key of its own per verdict: events with one key within 30 s are one event, and every edit has
/// its verdict (a save that gets the same answer again too).
fn verdict_key(what: &str) -> String {
    static VERDICT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!("{}.{}", what, VERDICT.fetch_add(1, Ordering::Relaxed))
}

/// An applied edit's one verdict: every patch reports it, also one with nothing left to check.
pub(crate) fn patch_verified_event(what: &str, group: u32, seqs: &[String]) {
    crate::hot_failure::verified(group);
    crate::inspect::events::emit(
        "patchverified",
        &verdict_key(&group.to_string()),
        &format!("\"edit\":{},\"group\":{},\"verified_events\":[{}]", json_str(what), group, seqs.join(",")),
    );
}

pub(crate) fn patch_refused_event(what: &str, refused: bool, lines: &[String]) {
    let reasons: Vec<String> = lines.iter().map(|l| crate::inspect::json_str(l)).collect();
    crate::inspect::events::emit(
        "patchrefused",
        &verdict_key(&format!("{}|{}", what, lines.first().cloned().unwrap_or_default())),
        &format!("\"edit\":{},\"outcome\":\"{}\",\"reasons\":[{}]", crate::inspect::json_str(what), if refused { "refused" } else { "restart" }, reasons.join(",")),
    );
    // Reporting does not restart. The watcher retries the whole settled batch
    // before deciding whether a refusal is a failure of makepad itself.
}

/// The path an unresolved name's error names: a `use` (E0432), a value (E0425) or a type (E0412),
/// as the resolver and the type checker print them.
fn unresolved_path(e: &str) -> Option<&str> {
    let at = ["E0432: unresolved import `", "E0425: cannot find value `", "E0412: cannot find type `"].iter().find_map(|h| e.find(h).map(|i| i + h.len()))?;
    Some(&e[at..at + e[at..].find('`')?])
}

/// `errs`, said as the restart rung's lines, when every one is a path into a provided crate that
/// does not resolve or a name such a path would have bound (a `use` of its re-export, a call
/// through that): None when any is another error.
fn provided_lacks(errs: &[String]) -> Option<Vec<String>> {
    let last = |p: &str| p.rsplit("::").next().unwrap_or(p).to_string();
    let lacked: Vec<(String, String)> = errs.iter().filter_map(|e| unresolved_path(e)).filter_map(|p| {
        let (krate, rest) = p.trim_start_matches("::").split_once("::")?;
        (!rest.is_empty() && crate::provided::registered(krate)).then(|| (last(p), krate.to_string()))
    }).collect();
    errs.iter().map(|e| {
        let name = last(unresolved_path(e)?);
        let (_, krate) = lacked.iter().find(|(n, _)| *n == name)?;
        Some(format!("{e} -- `{krate}` is built into makepad, and this one lacks it: needs the rebuilt makepad: restart to apply"))
    }).collect()
}

/// Whether a refusal's lines hold a failure of makepad itself, not of the edit.
fn internal_failure(lines: &[String]) -> bool {
    lines.iter().any(|l| l.contains("internal compiler error") || l.lines().any(|l| l.trim_start().starts_with("internal:")))
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
        // an edit waiting for a restart is no failure: the app runs on, the restart is offered
        let applied = o.failure.is_none() && (o.rung == Rung::Restart || o.rung == Rung::Patch && !o.pending
            && !o.lines.iter().any(|l| l.starts_with("incomplete:")));
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
        _ if o.failure.is_some() => "refuse",
        _ if o.lines.iter().any(|l| l == "incomplete: no item applied") => "incomplete",
        Rung::Patch if o.lines.iter().any(|l| l.starts_with("incomplete:")) => "partial",
        Rung::Patch if o.changed == 0 => "unchanged",
        Rung::Patch => "patch",
        Rung::Restart => "restart",
        Rung::Refuse => "refuse",
    };
    SESSION.lock().unwrap_or_else(|e| e.into_inner()).push((rung, total));
    // engine requests: a program waiting on an engine change hears that code changed under it
    if matches!(rung, "patch" | "partial" | "restart") {
        crate::inspect::engine_req::code_update(if rung == "restart" { "restart" } else { "patched" }, what, o.changed as u64);
    }
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
        count("restart"),
        count("refuse"),
        pct(0.5),
        pct(0.95),
        live_log_path()
    ))
}

/// std::relax::live_log: the program adds a record to the session log (the restart's kept/lost).
pub extern "C" fn __relax_live_log(p: *const u8, n: usize) {
    crate::jit::host_boundary("__relax_live_log", move || {
        if p.is_null() {
            return;
        }
        let s = unsafe { std::slice::from_raw_parts(p, n) };
        live_log(&String::from_utf8_lossy(s));
    })
}

/// The restart entries by name (jit::loader_symbol).
pub fn restart_symbol(name: &[u8]) -> Option<usize> {
    Some(match name {
        b"__relax_restart_supported" => __relax_restart_supported as *const () as usize,
        b"__relax_on_patch" => __relax_on_patch as *const () as usize,
        b"__relax_patched_files" => __relax_patched_files as *const () as usize,
        b"__relax_live_edit_enabled" => __relax_live_edit_enabled as *const () as usize,
        b"__relax_restart_requested" => __relax_restart_requested as *const () as usize,
        b"__relax_restart_reason" => __relax_restart_reason as *const () as usize,
        b"__relax_restored_state" => __relax_restored_state as *const () as usize,
        b"__relax_restart_saved_ms" => __relax_restart_saved_ms as *const () as usize,
        b"__relax_restart_now" => __relax_restart_now as *const () as usize,
        b"__relax_hot_load_failed" => __relax_hot_load_failed as *const () as usize,
        b"__relax_live_stream" => __relax_live_stream as *const () as usize,
        b"__relax_live_log" => __relax_live_log as *const () as usize,
        _ => return crate::inspect::engine_req::symbol(name),
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
    Outcome { failure: None, kept: 0, pending: false, rung: Rung::Patch, files: file.into_iter().collect(), lines: vec![line.to_string()], names: Vec::new(), changed: 0, test_changes: None, bodies: Vec::new(), group: 0, ms: [0.0; 4], undo: None, work: [0; 5] }
}

unsafe fn stream_file(up: *mut Unit, path: &str) -> Option<u32> {
    crate::owner::call(crate::owner::Class::Program, || {
        let prog = &(&*up).prog;
        prog.current_source_files(|p| p == path).get(path).copied()
    })
}

unsafe fn target_file(up: *mut Unit, target: &StreamTarget) -> Option<(u32, String)> {
    match target {
        StreamTarget::Path(path) => stream_file(up, path).map(|f| (f, path.clone())),
        StreamTarget::Module(h) => crate::modules::stream_target(*h),
    }
}

unsafe fn record_stream(up: *mut Unit, target: &StreamTarget, o: &Outcome) {
    if o.rung != Rung::Patch { return; }
    match target {
        StreamTarget::Module(h) => if let Some(&file) = o.files.first() { crate::modules::stream_applied(*h, file); },
        // (a streamed edit of a program file: the program hears of it as of a save)
        StreamTarget::Path(path) => if o.changed > 0 { patch_landed(std::iter::once(path.clone()), changed_items(up, o)); },
    }
}

/// The shared feed core takes a file index. Runtime modules never resolve a path via mods.
unsafe fn feed_stream(up: *mut Unit, target: &StreamTarget, file: u32, path: &str, state: &mut StreamFile, text: Vec<u8>) -> Outcome {
    let seen = Instant::now();
    let mut o = apply_mode(up, vec![Edit { file, path: path.to_string(), src: text }], &[],
        2, matches!(target, StreamTarget::Module(_)));
    record_stream(up, target, &o);
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
    // the watcher drains a coalesced prefix and reports a stalled stream: it polls while one is open
    watch_events::kick();
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
    record_stream(up, &StreamTarget::Module(h), &o);
    Some(o)
}

pub(crate) fn forget_module_stream(up: *mut Unit, h: u64) {
    STREAMS.lock().unwrap_or_else(|e| e.into_inner()).retain(|s| !(s.0 == up as usize && s.1 == StreamTarget::Module(h)));
}

/// A stream of `up` has a coalesced prefix to feed or a stall still to report (drain_streams
/// has work in the next rounds).
fn streams_busy(up: *mut Unit) -> bool {
    STREAMS.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|s| s.0 == up as usize && (s.2.pending.is_some() || !s.2.reported && s.2.changed.is_some()))
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
                let (parsed, composed) = crate::owner::call(crate::owner::Class::Program, || {
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
                    let o = apply_mode(up, vec![Edit { file, path: path.clone(), src }], &[], 0, matches!(target, StreamTarget::Module(_)));
                    record_stream(up, target, &o);
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
    crate::jit::host_boundary("__relax_live_stream", move || {
        if p.is_null() || t.is_null() { return 3; }
        unsafe {
            if jit::cur_unit().is_null() { return 3; }
            let Ok(path) = std::str::from_utf8(std::slice::from_raw_parts(p, n)) else { return 3; };
            let o = apply_stream(jit::cur_unit(), path, std::slice::from_raw_parts(t, len).to_vec());
            match o.rung {
                Rung::Patch if o.changed == 0 && o.lines.iter().any(|l| l.starts_with("incomplete:")) => 1,
                Rung::Patch => 0,
                Rung::Restart => 2,
                Rung::Refuse if o.lines.iter().any(|l| l.starts_with("incomplete:")) => 1,
                Rung::Refuse => 3,
            }
        }
    })
}

// ------------------------------------------------------------------ the live driver in apps

/// `makepad run` of a working tree: a watcher thread polls the program's own source files (not
/// std's, not registry/git dependencies) and applies each settled save as one transaction.
/// Body-class patches are type-safe to install while the program runs (hotreload.md §9.2), so
/// they apply at once; other edits print their rung and reason. Every outcome is one line on
/// stderr ("makepad live: ...").
#[path = "reload_inputs.rs"]
mod inputs;
#[path = "reload_git.rs"]
mod git;
pub use inputs::Inputs;
#[path = "reload_host.rs"]
pub mod host;
pub use host::set_loader_package;

pub fn start_watcher(up: *mut Unit, inputs: Inputs) {
    crate::hot_failure::enable();
    crate::hot_failure::restore();
    struct P(*mut Unit);
    unsafe impl Send for P {}
    let p = P(up);
    let spawned = crate::run_threads::spawn("run-live", move || { let _z = crate::zone_alloc::compiler_thread();
            let p = p;
            makepad_cpu_budget::set_thread_class(makepad_cpu_budget::BACKGROUND);
            // a panic here (the ICE hook is silent on the compiler's owner thread) would end
            // live editing without a word: say so
            loop {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { watch(p.0, inputs.clone()) }));
                if let Err(e) = r {
                    let why = e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_default();
                    let why = format!("the live watcher panicked: {why}{}", jit::take_ice_location());
                    eprintln!("makepad live: {why}");
                    recover_hot_failure(makepad_remote::hot::Outcome::Panic, &[why], None);
                    if !await_recovery() || crate::hot_failure::has_error() { break; }
                } else {
                    break;
                }
            }
            watch_gone();
        });
    if spawned.is_err() {
        watch_gone();
    }
}

/// After a watcher panic: the recovery it asked for is reported from here while the restart it
/// offered waits for the person (the same worker owns the reporting). False: the run ends
/// meanwhile -- the restart in place -- and the thread leaves with it; the next run's watcher
/// reports the recovery. (It waited
/// on, out of the run's reach: the run's end left it behind ("threads that did not end may read
/// it: [\"run-live\"]"), and it watched the ended run's unit again once the next run's restore
/// completed the recovery: a fault outside JIT code, not contained.)
fn await_recovery() -> bool {
    while crate::hot_failure::recovering() {
        crate::hot_failure::pump();
        if !crate::run_threads::sleep(std::time::Duration::from_millis(100)) {
            return false;
        }
    }
    !crate::run_threads::ending()
}

/// The files of `removed` (gone from disk) that are running modules whose declaring file is not
/// among `edited`: the program still declares them, each one's refusal line. A file the program
/// does not run (its module unbound by an earlier edit) or a crate root is no such module.
fn undeclared_removals(prog: &crate::program::Program, removed: &[&str], edited: &std::collections::HashSet<&str>) -> Vec<String> {
    let loaded = prog.current_source_files(|p| removed.contains(&p));
    removed.iter().filter_map(|&path| {
        let &file = loaded.get(path)?;
        let module = prog.mods.iter().find(|m| m.file == file)?;
        let declares = prog.file_paths.get(prog.mods[module.parent_mod? as usize].file as usize)?;
        (!edited.contains(declares.as_str())).then(|| removed_module_line(path, declares))
    }).collect()
}

/// The refusal of a module whose file `path` was removed while `declares` still declares it:
/// rustc's E0583 for the module's name (its folder's for a `mod.rs`).
fn removed_module_line(path: &str, declares: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let name = match file {
        "mod.rs" => path.rsplit('/').nth(1).unwrap_or(file),
        _ => file.strip_suffix(".rs").unwrap_or(file),
    };
    format!("error[E0583]: file not found for module `{name}`: {path} was removed and {declares} still declares it")
}

/// Inspect takes the loaded paths on the owner: tests can load more files while the watcher
/// waits for their verdict. No inputs lock survives that request or the path formatting.
/// The run's inputs `keep` takes (an absolute path; `inspect inputs` answers relative ones).
pub(crate) fn input_paths(keep: &dyn Fn(&str) -> bool) -> Result<Vec<String>, String> {
    // a live session's request that comes before its watcher started (a patch sent right after
    // the launch) waits for it: it was refused "not started" (11-03). The run's own flag: this
    // answers from before the program is laid out (the layout's flag), and a save that came then
    // read "watches no sources" and ended the session (12 min of native build lost, 11-04)
    let mut g = WATCH_INPUTS.lock().unwrap_or_else(|e| e.into_inner());
    while g.is_none() && crate::run::live_session() && !WATCH_GONE.load(std::sync::atomic::Ordering::Acquire) {
        g = WATCH_READY.wait(g).unwrap_or_else(|e| e.into_inner());
    }
    let (inputs, loaded) = g.as_ref().map(|v| (v.inputs.clone(), v.loaded.clone())).ok_or("this run watches no sources (not a live session, or its watcher ended)")?;
    drop(g);
    // (and what its code included: the watcher patches those files' includers, so a save of one
    // is an edit the process reads)
    let std_dir = std::env::var("MAKEPAD_RUN_STD").unwrap_or_default();
    let included = included_files(&std_dir).into_iter().filter(|p| keep(p));
    Ok(inputs.source_paths(loaded.iter().filter(|p| keep(p)).cloned().chain(included), keep))
}

/// Of `asked` (workspace-relative paths), those in no package of the app's graph.
pub(crate) fn outside_packages<'a>(asked: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let g = WATCH_INPUTS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(v) = g.as_ref() else { return Vec::new() };
    let std_dir = std::env::var("MAKEPAD_RUN_STD").unwrap_or_default();
    asked.filter(|p| v.inputs.outside_packages(p, &std_dir)).collect()
}

static STD: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// A source file a live session watches and patches (`own_source`).
pub(crate) fn own_source_file(prog: &crate::program::Program, file: u32) -> bool {
    let std_dir = STD.get_or_init(|| std::env::var("MAKEPAD_RUN_STD").unwrap_or_default());
    prog.file_paths.get(file as usize).is_some_and(|p| own_source(p, std_dir))
}

fn own_source(path: &str, std_dir: &str) -> bool {
    !(path.is_empty() || (!std_dir.is_empty() && crate::vfs::relative_path(path, std_dir).is_some()) || path.contains("/.cargo/registry/") || path.contains("/.cargo/git/") || path.contains("/rustlib/"))
}

pub(crate) fn stamp(path: &str) -> Option<(std::time::SystemTime, u64)> {
    // a file of a mounted store changes only by an edit (its file in the edits folder: a device's
    // devhub push); the store under a run never changes, and is not read to stat it
    if crate::vfs::is_virtual(path) {
        return Some(match crate::vfs::edited_disk_path(path).and_then(|p| std::fs::metadata(p).ok()) {
            Some(m) => (m.modified().ok()?, m.len()),
            None => (std::time::UNIX_EPOCH, 0),
        });
    }
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// The watcher's rounds while live tracing: its wakeups, the rounds that looked at files and why,
/// printed once a minute with more than one wakeup (at rest a native watch wakes for nothing).
struct WatchCost { since: Instant, wakes: u32, rounds: u32, stats: usize, why: [u32; 5] }

impl WatchCost {
    const WHY: [&'static str; 5] = ["poll (no native watch)", "events lost", "retry", "rescan", "events"];

    fn new() -> WatchCost {
        WatchCost { since: Instant::now(), wakes: 0, rounds: 0, stats: 0, why: [0; 5] }
    }

    /// A round that stats `n` files; `why`: its first reason of WHY.
    fn round(&mut self, n: usize, why: usize) {
        self.wakes += 1;
        if n > 0 {
            self.rounds += 1;
            self.stats += n;
            self.why[why] += 1;
        }
        if self.since.elapsed() >= std::time::Duration::from_secs(60) {
            if self.wakes > 1 {
                let why: Vec<String> = self.why.iter().zip(Self::WHY).filter(|(n, _)| **n > 0).map(|(n, w)| format!("{w} {n}")).collect();
                eprintln!("makepad live: watch cost: {} wakeup(s), {} round(s) looked at {} file(s) in {:.0} s ({})", self.wakes, self.rounds, self.stats, self.since.elapsed().as_secs_f64(), why.join(", "));
            }
            *self = WatchCost::new();
        }
    }
}

/// Which watched files are a mounted store's, and where an edit of each would sit in the edits
/// folder: asked once per watch set (each answer normalizes the path and walks the mounts).
#[derive(Default)]
struct Virtual {
    is: Vec<bool>,
    rels: std::collections::HashMap<String, usize>,
}

impl Virtual {
    fn of(files: &[(String, u32, Option<(std::time::SystemTime, u64)>)]) -> Virtual {
        let is: Vec<bool> = files.iter().map(|f| crate::vfs::is_virtual(&f.0)).collect();
        let rels = files.iter().enumerate().filter(|&(k, _)| is[k]).filter_map(|(k, f)| Some((crate::vfs::edit_rel(&f.0)?, k))).collect();
        Virtual { is, rels }
    }
}

/// One round's look at the watched files, as `stamp` gives them: a disk file's from its stat; the
/// virtual files' from one walk of the edits folder at the round's first virtual file, matched to
/// them by their place there. A round over thousands of virtual files costs the few edits, not a
/// path lookup and a failing stat each (two looks on every edit took 11-29 ms on Fab's 5,500).
struct Look(std::cell::OnceCell<std::collections::HashMap<usize, (std::time::SystemTime, u64)>>);

impl Look {
    fn new() -> Look {
        Look(std::cell::OnceCell::new())
    }

    /// Watched file `k` (`path`); one added since `virt` was taken is stat'ed on its own.
    fn stamp(&self, virt: &Virtual, k: usize, path: &str) -> Option<(std::time::SystemTime, u64)> {
        if !virt.is.get(k).copied().unwrap_or(false) {
            return stamp(path);
        }
        let edits = self.0.get_or_init(|| {
            crate::vfs::EditsLook::now().map(|l| l.entries().filter_map(|(rel, s)| Some((*virt.rels.get(rel)?, s))).collect()).unwrap_or_default()
        });
        Some(edits.get(&k).copied().unwrap_or((std::time::UNIX_EPOCH, 0)))
    }
}

/// Whether every changed path is an edit of a mounted store that the edits ledger committed:
/// devhub's ship-done (`relaxgit::finish`) renames each file into the edits folder and the ledger
/// last, so an edit whose file was in place when the ledger was renamed is whole, and a save after
/// it is not settled by it (its file came later). Told by the files' and the ledger's change times,
/// set by this machine's clock at each rename (a modified time is the saver's, set to its save).
fn committed_by_ledger<'a>(mut paths: impl Iterator<Item = &'a str>) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let changed_at = |m: &std::fs::Metadata| (m.ctime(), m.ctime_nsec());
        let Some(ledger) = crate::vfs::edits_record().and_then(|p| std::fs::metadata(p).ok()) else { return false };
        let mut any = false;
        let whole = paths.all(|p| {
            any = true;
            // an edit taken back has no file: the ledger's rewrite removed it
            crate::vfs::is_virtual(p) && crate::vfs::edited_disk_path(p).is_none_or(|d| std::fs::metadata(d).is_ok_and(|m| changed_at(&m) <= changed_at(&ledger)))
        });
        any && whole
    }
    #[cfg(not(unix))]
    {
        let _ = paths.next();
        false
    }
}

fn gate_note(g: u8) {
    static LAST: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(255);
    if LAST.swap(g, Ordering::Relaxed) != g && live_trace() {
        eprintln!("makepad live: watcher gate {} at t={:.0} ms", ["open", "domain loading", "no first present", "restart required"][g as usize], crate::clock::ms_since_start());
    }
}

/// The native file watch of the live set. A virtual file (a store run with an edits folder) is
/// watched where its edit lives, when that folder exists; the edits record, which every new edit
/// replaces, stands for the rest: its event is a whole scan and a new watch (new folders).
/// Without this a store run fell back to polling every file (5% of a core idle on Fab).
struct LiveWatch {
    watch: watch_events::Watch,
    /// watch index -> watched file index (usize::MAX: the edits record)
    files: Vec<usize>,
}

impl LiveWatch {
    fn new<'a>(paths: impl Iterator<Item = &'a str>) -> Option<Self> {
        let mut watched: Vec<String> = Vec::new();
        let mut files = Vec::new();
        let mut any_virtual = false;
        for (i, p) in paths.enumerate() {
            if crate::vfs::is_virtual(p) {
                any_virtual = true;
                let Some(at) = crate::vfs::edit_location(p) else { continue };
                if !at.parent().is_some_and(|d| d.is_dir()) { continue; }
                watched.push(at.to_string_lossy().into_owned());
            } else {
                watched.push(p.to_string());
            }
            files.push(i);
        }
        if any_virtual {
            watched.push(crate::vfs::edits_record()?.to_string_lossy().into_owned());
            files.push(usize::MAX);
        }
        Some(Self { watch: watch_events::Watch::new(watched.iter().map(String::as_str))?, files })
    }
    fn wait(&mut self, timeout: Option<std::time::Duration>) { self.watch.wait(timeout) }
    /// None: events may have been lost, or a new edit was recorded: stat every file, watch again.
    fn changed(&mut self) -> Option<Vec<usize>> {
        let ids = self.watch.changed()?;
        let files: Vec<usize> = ids.into_iter().map(|k| self.files[k]).collect();
        if files.contains(&usize::MAX) { return None; }
        Some(files)
    }
}

/// The edits ledger (`.makepad-run-edits`) as the watcher last looked at the virtual files. A
/// mounted store's file changes only through the edits folder, and every edit rewrites the ledger
/// (a devhub ship writes it last, after its files): while it stands still the virtual files are
/// as they were, and their stamps (an edits-folder lookup and a failing stat each; 2,700 files
/// every 100 ms took a third of a core on a phone) are not read. A round that sees it move looks
/// at every virtual file, whatever woke it, and only that round takes the new stamp. (A backstop
/// round that took it without looking lost overlay edits for good, 11-03: the stamp moved between
/// a ship's ledger write and its file event, whose round then found the ledger quiet.)
struct Ledger(Option<(std::time::SystemTime, u64)>);

impl Ledger {
    fn now() -> Ledger {
        Ledger(crate::vfs::edits_ledger_stamp())
    }

    /// The watched files a round stats, given the ledger's stamp now: every disk file (`full`), or
    /// those the native watch named (none: the polled hot set's disk files); and every virtual
    /// file when the ledger moved.
    fn round(&mut self, now: Option<(std::time::SystemTime, u64)>, full: bool, named: Option<Vec<usize>>, hot: &[usize], virt: &[bool]) -> Vec<usize> {
        let disk = |k: &usize| !virt[*k];
        let mut ids: Vec<usize> = if full { (0..virt.len()).filter(disk).collect() } else { named.unwrap_or_else(|| hot.iter().copied().filter(disk).collect()) };
        if now != self.0 {
            self.0 = now;
            ids.extend((0..virt.len()).filter(|&k| virt[k]));
            ids.sort_unstable();
            ids.dedup();
        }
        ids
    }
}

/// Stillness, not an overall deadline: a busy checkout can take as long as it needs. Also the
/// stat poll of a watch without native file events.
const LIVE_QUIET: std::time::Duration = std::time::Duration::from_millis(100);
/// How often a watcher whose startup scan waits for the first frames to pass looks again.
const STARTUP_SCAN_LOOK: std::time::Duration = std::time::Duration::from_millis(50);

/// The harness is parked: verify this group, run its tests, then undo faulting code before another edit.
unsafe fn test_edit(up: *mut Unit, paths: Vec<String>, o: &mut Outcome, undos: &mut Vec<Undo>) -> Vec<(u32, u32)> {
    use crate::harness::live;
    let what = paths.join(", ");
    let mut edit = live::Edit { paths, group: o.group, compile_ms: Some(o.ms[2]), restarted: false, changed: o.test_changes.clone(), patch: Some((o.changed, o.names.len())) };
    for line in &o.lines { eprintln!("makepad live:   {line}"); }
    if o.rung != Rung::Patch {
        patch_refused_event(&what, o.rung == Rung::Refuse, &o.lines);
        if o.rung == Rung::Restart {
            if edit.group == 0 { edit.group = crate::owner::call(crate::owner::Class::Program, || (*up).begin_patch()); }
            live::restart(edit, &what);
        }
        return Vec::new();
    }
    eprintln!("makepad live: {what}: patched ({} item(s) changed, {} fn(s) installed) in {:.1} ms", o.changed, o.names.len(), o.ms.iter().sum::<f64>());
    // the warnings of the bodies it changed or added, as the test check reports them (those that
    // rest on every body -- dead code, unused imports -- wait for the next test check)
    let mut bodies = o.bodies.clone();
    if let Some(u) = &o.undo { bodies.extend(u.added.iter().copied()); }
    for l in crate::owner::call(crate::owner::Class::Program, || crate::check::live_lints(&(*up).prog, &(*up).tcx, &bodies)) {
        eprintln!("makepad live:   {l}");
    }
    patch_landed(edit.paths.iter().cloned(), changed_items(up, o));
    let mut pending = Vec::new();
    if let Some(undo) = o.undo.take() {
        let ids = crate::owner::call(crate::owner::Class::Program, || unverified_patch(up, o.group));
        if !ids.is_empty() {
            UNVERIFIED_PATCHES.fetch_add(1, Ordering::AcqRel);
            pending.push(PendingPatch { group: o.group, what: what.clone(), ids, event_seq: crate::inspect::events::last_seq() });
        }
        undos.push(undo);
    }
    if pending.is_empty() { patch_verified_event(&what, o.group, &[]); }
    while !pending.is_empty() {
        let back = settle_verdicts(up, &mut pending, undos);
        if !back.is_empty() { return back; }
        if !pending.is_empty() { std::thread::sleep(std::time::Duration::from_millis(5)); }
    }
    let mut back = Vec::new();
    loop {
        let faults = live::rerun(&edit);
        // A later group may depend on the faulting one: unwind newest first, as SPEC-V does.
        let Some(first) = undos.iter().position(|u| faults.contains(&u.group)) else { break };
        let mut groups = Vec::new();
        let mut names = Vec::new();
        for undo in undos.split_off(first).into_iter().rev() {
            groups.push(undo.group);
            back.extend_from_slice(&undo.versions);
            names.extend(rollback(up, undo, "a fault in live test code"));
        }
        crate::inspect::events::emit("patchreverted", &verdict_key(&what), &format!(
            "\"edit\":[{}],\"reverted\":true,\"groups\":{:?},\"fns\":[{}],\"error\":\"a fault in live test code\"",
            edit.paths.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","), groups, names.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(",")));
        eprintln!("makepad live: {what}: reverted patch groups {groups:?}; rerunning tests");
        edit.compile_ms = None;
        edit.changed = None;
    }
    back
}

unsafe fn watch(up: *mut Unit, mut inputs: Inputs) {
    // a body that includes a file for the first time: the watcher watches it from now on
    crate::sip::on_late_new(watch_events::kick);
    inputs.add_runner();
    *LOADED_HEADS.lock().unwrap_or_else(|e| e.into_inner()) = inputs.heads.clone();
    let std_dir = std::env::var("MAKEPAD_RUN_STD").unwrap_or_default();
    // (path, running file index, last stamp)
    // the program's own source files (not std, registry or git dependencies), loaded so far
    let own_files = |u: &Unit| -> Vec<(String, u32)> {
        u.prog.current_source_files(|p| own_source(p, &std_dir)).into_iter().filter(|(p, _)| crate::vfs::is_file(p)).collect()
    };
    // (the watcher's holds read the program's files only: lock_unit_quick)
    let mut files: Vec<WatchFile> = {
        crate::owner::call(crate::owner::Class::Program, || {
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
    set_watch_inputs(&inputs, &files);
    // the files the program's own code includes (include_str!/include_bytes!): a save of one is
    // an edit of the files that include it (`include_edits`). Bodies read more as they compile:
    // each is watched from its read (late_count moves).
    let add_included = |files: &mut Vec<WatchFile>| -> bool {
        let mut added = false;
        for path in included_files(&std_dir) {
            if !files.iter().any(|f| f.0 == path) { let s = stamp(&path); files.push((path, u32::MAX, s)); added = true; }
        }
        added
    };
    let mut late_seen = crate::sip::late_count();
    add_included(&mut files);
    // the post-boot loader adds files after boot; each is watched from its load (an
    // edit made before it is simply what the load read)
    let mut loads_seen = jit::DEFERRED_LOADS.load(std::sync::atomic::Ordering::Relaxed);
    let mut events = LiveWatch::new(files.iter().map(|f| f.0.as_str()));
    let mut repositories = git::Repositories::default();
    repositories.include(files.iter().map(|f| f.0.as_str()));
    if crate::run::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) { eprintln!("makepad live: watching {} source files; session log {}", files.len(), live_log_path()); }
    // the readiness signal for tools and tests, whatever the log verbosity: MAKEPAD_RUN_LIVE_READY=<file>
    // gets the watched file count once the watcher runs (edits saved from now on are seen)
    if let Some(ready) = std::env::var_os("MAKEPAD_RUN_LIVE_READY") { let _ = std::fs::write(ready, format!("{}\n", files.len())); }
    let mut undos: Vec<Undo> = Vec::new();
    // SPEC-V: patches that ran before their verdict
    let mut pending: Vec<PendingPatch> = Vec::new();
    // with native file events a round runs only on an event or a kick: a watcher at rest makes no
    // system call. Without them a save is seen within 100 ms; one round is a stat per watched file
    // (MAKEPAD_RUN_LIVE_TRACE=1 prints its cost once)
    let mut traced = !live_trace();
    // the hot set (no native events): the program's own package (its root crate's directory) and
    // every file saved this session, polled every 5 ms (a few stats); every file every 100 ms
    let mut hot: Vec<usize> = {
        let dir = crate::owner::call(crate::owner::Class::Program, || {
            let u = &*up;
            let root = crate::layout::root_crate() as usize;
            u.prog.crates.get(root).and_then(|c| u.prog.mods.get(c.root_mod as usize)).and_then(|m| u.prog.file_paths.get(m.file as usize)).and_then(|p| std::path::Path::new(p).parent().map(|d| d.to_string_lossy().into_owned()))
        });
        match dir {
            Some(d) => files.iter().enumerate().filter(|(_, f)| crate::vfs::relative_path(&f.0, &d).is_some()).map(|(k, _)| k).take(64).collect(),
            None => Vec::new(),
        }
    };
    // Refused texts stay pending until a batch containing them applies, or they
    // return to their running text. Every tree change retries the entire set, and
    // nothing else does: a batch's inputs are its files, so an unchanged tree
    // compiles the same verdict (no timer; at rest the watch makes no wakeup).
    let mut stalled: std::collections::HashMap<usize, (Vec<u8>, Vec<String>)> = std::collections::HashMap::new();
    // one more round at once, whatever the events say: a write resumed while the batch was read,
    // or files moved outside a dock app's sync
    let mut again = false;
    // installed patches whose landing is checked once LANDING has passed: (when, patch group, the edit)
    let mut landing: Vec<(Instant, u32, String)> = Vec::new();
    let mut last_scan = Instant::now();
    // a WM dock app: only a devhub sync of landed work may change its code (`DockGate`)
    let mut dock = DockGate::from_env();
    // the edits ledger as the last round that looked at the virtual files saw it (`Ledger`), and
    // which files are a mounted store's (asked once per watch set: the lookup normalizes the path
    // and walks the mounts)
    let mut ledger = Ledger::now();
    let mut cost = WatchCost::new();
    let mut virt = Virtual::default();
    let mut startup_scan = true;
    let mut restart_required = false;
    let mut restart_message = String::new();
    // the files of the edit a rebuild of makepad is for (a live test session's restart names them)
    let mut host_edit: Vec<String> = Vec::new();
    let mut revision = 0;
    // the last two path parts say which crate and file
    let short = |p: &str| -> String { let v: Vec<&str> = p.rsplit('/').take(2).collect(); v.into_iter().rev().collect::<Vec<_>>().join("/") };
    crate::harness::live::wait_initial();
    crate::run_threads::on_end(watch_events::kick);
    // (from a run's first step of its end no edit goes into it: what the edit queues would keep its
    // end waiting, and the next run reads the sources anyway)
    while !crate::run_threads::ending() && !crate::run_threads::run_ending() {
        crate::hot_failure::pump();
        if crate::hot_failure::recovering() {
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }
        // pinned (`inspect pin on`, an operation runs on this app): no edit or landing reaches it
        // and it does not restart; on unpin one whole scan catches up
        if PINNED.load(std::sync::atomic::Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }
        // native file events: sleep on them while nothing else is in flight (startup, a domain
        // loading, verdicts pending, an open stream); else the 5 ms poll. Every other input wakes
        // it (watch_events::kick: a streamed edit, a domain's inputs, the post-boot loader's files,
        // a late include, the startup's end, an unpin, a recovery, a disarmed real-time root); a
        // timeout only for what is due (a landing check, the startup scan). At rest it makes no
        // system call.
        let quiet = (crate::harness::live::enabled() || jit::live_startup_done()) && !DOMAIN_LOADING.load(std::sync::atomic::Ordering::Acquire)
            && pending.is_empty() && !streams_busy(up);
        let t_wait = Instant::now();
        match events.as_mut() {
            Some(w) if quiet => {
                let next = again.then_some(std::time::Duration::ZERO)
                    .into_iter().chain(landing.iter().map(|l| l.0.saturating_duration_since(Instant::now())))
                    .chain(startup_scan.then_some(STARTUP_SCAN_LOOK)).min();
                w.wait(next.map(|t| t.max(std::time::Duration::from_millis(5))));
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
        let woke = Instant::now();
        if DOMAIN_LOADING.load(std::sync::atomic::Ordering::Acquire) { gate_note(1); continue; }
        let latest = DOMAIN_REVISION.load(std::sync::atomic::Ordering::Acquire);
        if latest != revision {
            revision = latest;
            if let Some(next) = DOMAIN_INPUTS.lock().unwrap_or_else(|e| e.into_inner()).take() {
                inputs = next;
                inputs.add_runner();
                *LOADED_HEADS.lock().unwrap_or_else(|e| e.into_inner()) = inputs.heads.clone();
            }
            let loaded = crate::owner::call(crate::owner::Class::Program, || own_files(&*up));
            files = inputs.paths.iter().map(|(p, (s, _))| (p.clone(), u32::MAX, *s)).collect();
            for (path, file) in loaded {
                if let Some(f) = files.iter_mut().find(|f| f.0 == path) { f.1 = file; }
                else { let s = stamp(&path); files.push((path, file, s)); }
            }
            set_watch_inputs(&inputs, &files);
            late_seen = crate::sip::late_count();
            add_included(&mut files);
            events = LiveWatch::new(files.iter().map(|f| f.0.as_str()));
            repositories = git::Repositories::default();
            repositories.include(files.iter().map(|f| f.0.as_str()));
            hot.clear();
            stalled.clear();
            again = false;
            undos.clear();
            restart_required = false;
            restart_message.clear();
            landing.clear();
            startup_scan = true;
        }
        // Keep the original stamps and queued OS notifications intact until startup
        // finishes. In particular, do not acquire Reloading or drain streamed edits:
        // either can compete with the cold compiler before the first frame.
        if !crate::harness::live::enabled() && !jit::live_startup_done() { gate_note(2); continue; }
        // a live test session's rebuilt makepad: the tests restart into it now, between runs
        if let Some(why) = host::take_harness_restart() {
            let group = crate::owner::call(crate::owner::Class::Program, || (*up).begin_patch());
            crate::harness::live::restart(crate::harness::live::Edit {
                paths: std::mem::take(&mut host_edit), group, compile_ms: None, restarted: true, changed: None, patch: None,
            }, &why);
        }
        // A patch's verdict settles ahead of the gates below: they hold the watch's own work (the
        // startup scan for the UI thread's first frames, a pending restart), never the verdict
        // the person waits for. A patch sets the startup scan again when the tree holds files
        // the program has not loaded (Fab: always), so the first edit after a cold start had its
        // verdict held until the first-frames window closed, 10 s after the present (11-04).
        if !pending.is_empty() {
            for (o, n) in settle_verdicts(up, &mut pending, &mut undos) {
                for f in files.iter_mut() {
                    if f.1 == n { f.1 = o; }
                }
            }
        }
        // Once the tree no longer matches this session, later saves must not patch a
        // mixture of revisions into the old process. A fresh run restores live editing.
        if restart_required {
            gate_note(3);
            let notifications = events.as_mut().and_then(|w| w.changed());
            let lost = events.is_some() && notifications.is_none();
            if lost { events = LiveWatch::new(files.iter().map(|f| f.0.as_str())); }
            let full = lost || (events.is_none() && last_scan.elapsed() >= LIVE_QUIET);
            if full { last_scan = Instant::now(); }
            if virt.is.len() != files.len() { virt = Virtual::of(&files); }
            let ids = ledger.round(crate::vfs::edits_ledger_stamp(), full, notifications, &hot, &virt.is);
            let look = Look::new();
            let changed: Vec<_> = ids.into_iter().filter_map(|k| {
                let f = &files[k];
                let current = look.stamp(&virt, k, &f.0);
                (current != f.2).then_some((k, current))
            }).collect();
            if !changed.is_empty() {
                let names: Vec<String> = changed.iter().take(3).map(|&(k, _)| short(&files[k].0)).collect();
                let message = format!("{}{}: restart pending ({restart_message}); this edit applies with it", names.join(", "), if changed.len() > 3 { ", ..." } else { "" });
                // (a live test session's restart would not apply it either: refused there)
                patch_refused_event(&message, crate::harness::live::enabled(), std::slice::from_ref(&message));
                for (k, current) in changed { files[k].2 = current; }
            }
            continue;
        }
        // the startup's file scan (own_files: an owner request of 10-15 ms on Sandbox) waits until
        // the UI thread's first frames after the present are done: run right at the present, the
        // UI thread's first lazy links queued behind it (36 ms of a first post-present episode).
        // A save ends the wait: the person waits for that edit, not for the first frames (Fab's
        // first edit right after its window showed waited 7.6 s here, 11-04). The scan's round
        // looks at every file, those the taken notifications named among them.
        if startup_scan && crate::postframe::in_first_frames() {
            let named = events.as_mut().map(|w| w.changed());
            // (events lost: the watch starts over)
            if matches!(named, Some(None)) { events = LiveWatch::new(files.iter().map(|f| f.0.as_str())); }
            if !named.is_some_and(|n| n.is_none_or(|c| !c.is_empty())) && crate::vfs::edits_ledger_stamp() == ledger.0 { continue; }
        }
        gate_note(0);
        let loads = jit::DEFERRED_LOADS.load(std::sync::atomic::Ordering::Relaxed);
        // a new watch set drops the events the old one had not delivered yet (a save made
        // while the post-boot loader loads was never seen): stat every file once instead
        let mut rewatched = std::mem::take(&mut startup_scan);
        let late = crate::sip::late_count();
        if loads != loads_seen || rewatched || late != late_seen {
            loads_seen = loads;
            late_seen = late;
            let t_own = Instant::now();
            let loaded = crate::owner::call(crate::owner::Class::Program, || own_files(&*up));
            if live_trace() { eprintln!("makepad live: rewatch (loads {loads}, startup scan {rewatched}): own files in {:.1} ms at t={:.0} ms", t_own.elapsed().as_secs_f64() * 1e3, crate::clock::ms_since_start()); }
            let known: std::collections::HashMap<_, _> = files.iter().enumerate().map(|(i, f)| (f.0.clone(), i)).collect();
            let mut added = Vec::new();
            for (path, file) in loaded {
                if let Some(&i) = known.get(&path) { files[i].1 = file; }
                else { added.push((path, file)); }
            }
            let loaded_more = !added.is_empty();
            if loaded_more {
                if jit::deferred_trace() { eprintln!("makepad live: watching {} more file(s) the post-boot loader loaded", added.len()); }
                for (p, i) in added { let st = stamp(&p); files.push((p, i, st)); }
            }
            set_watch_loaded(&files);
            if add_included(&mut files) || loaded_more {
                repositories.include(files.iter().map(|f| f.0.as_str()));
                events = LiveWatch::new(files.iter().map(|f| f.0.as_str()));
                rewatched = true;
            }
        }
        let t_dr = Instant::now();
        drain_streams(up);
        if live_trace() && t_dr.elapsed().as_millis() > 20 { eprintln!("makepad live: drain_streams took {:.1} ms at t={:.0} ms", t_dr.elapsed().as_secs_f64() * 1e3, crate::clock::ms_since_start()); }
        if crate::hot_failure::recovering() { continue; }
        // The landing check: a patch whose old code is still running a while after the install
        // has not reached that call (a loop that never returns): the edit restarts the app.
        let now = Instant::now();
        if landing.iter().any(|l| l.0 <= now) {
            let due: Vec<(u32, String)> = landing.iter().filter(|l| l.0 <= now).map(|l| (l.1, l.2.clone())).collect();
            landing.retain(|l| l.0 > now);
            for (gen, what) in due {
                let t_land = Instant::now();
                let w0 = crate::jit::wall_ns();
                let old = crate::owner::call(crate::owner::Class::Program, || { let held = t_land.elapsed(); (running_old_code(&*up, gen), held) });
                if live_trace() {
                    // a long wait names what the owner served meanwhile
                    let behind = if old.1.as_millis() >= 20 { format!("; behind {}", crate::owner::served_during(w0, w0 + old.1.as_nanos() as u64)) } else { String::new() };
                    eprintln!("makepad live: landing check of patch group {gen} at t={:.0} ms: {:.1} ms ({:.1} ms waiting for the owner{behind})", crate::clock::ms_since_start(), t_land.elapsed().as_secs_f64() * 1e3, old.1.as_secs_f64() * 1e3);
                }
                if let Some(name) = old.0 {
                    let lines = vec![format!("`{name}` still runs the code from before the edit: a call made before the patch has not returned (a loop) -- restart the app")];
                    log_edit(&what, &Outcome { failure: None, kept: 0, pending: false, rung: Rung::Restart, files: Vec::new(), lines: lines.clone(), names: Vec::new(), changed: 0, test_changes: None, bodies: Vec::new(), group: gen, ms: [0.0; 4], undo: None, work: [0; 5] }, None, Instant::now());
                    restart_for_edit(&what, &lines);
                    for l in &lines { eprintln!("makepad live:   {}", l); }
                    patch_refused_event(&what, false, &lines);
                    landing.clear();
                    break;
                }
            }
        }
        let t_ev = Instant::now();
        let notifications = events.as_mut().and_then(|w| w.changed());
        if t_ev.elapsed().as_millis() > 20 && live_trace() { eprintln!("makepad live: events drain took {:.1} ms", t_ev.elapsed().as_secs_f64() * 1e3); }
        let lost = events.is_some() && notifications.is_none();
        let t_rewatch = Instant::now();
        if lost { events = LiveWatch::new(files.iter().map(|f| f.0.as_str())); }
        let rewatch_ms = t_rewatch.elapsed().as_secs_f64() * 1e3;
        // a sync moved the dock app's checkout: every file is compared with what runs
        let synced = dock.as_mut().is_some_and(DockGate::synced);
        let caught_up = CATCH_UP.swap(false, std::sync::atomic::Ordering::AcqRel);
        let retry_due = std::mem::take(&mut again) | RETRY_NOW.swap(false, std::sync::atomic::Ordering::AcqRel);
        let all = lost || rewatched || synced || caught_up || retry_due
            || (events.is_none() && last_scan.elapsed() >= LIVE_QUIET);
        if rewatched || synced || virt.is.len() != files.len() { virt = Virtual::of(&files); }
        let t_round = std::time::Instant::now();
        if all { last_scan = Instant::now(); }
        // a sync moved the checkout: every file, the virtual ones too
        let named = notifications.as_ref().map(Vec::len);
        let ledger_was = ledger.0;
        let ids = if synced { (0..files.len()).collect() } else { ledger.round(crate::vfs::edits_ledger_stamp(), all, notifications, &hot, &virt.is) };
        let mut scanned = ids.len();
        if live_trace() {
            let why = [events.is_none(), lost, retry_due, rewatched || synced || caught_up].iter().position(|&w| w).unwrap_or(4);
            cost.round(scanned, why);
        }
        let look = Look::new();
        let mut changed: Vec<usize> = ids.into_iter().filter(|&k| look.stamp(&virt, k, &files[k].0) != files[k].2).collect();
        // what a held landing's take or release names, whatever its stamp says
        let forced = std::mem::take(&mut *LANDING_FORCED.lock().unwrap_or_else(|e| e.into_inner()));
        if !forced.is_empty() {
            let forced: std::collections::HashSet<String> = forced.into_iter().collect();
            changed.extend((0..files.len()).filter(|&k| forced.contains(&crate::vfs::normalize_path(&files[k].0))));
            changed.sort_unstable();
            changed.dedup();
        }
        // a round that looked at something: when it woke, what woke it and what it found (the
        // 5 ms polls while a verdict is pending look at nothing and print nothing)
        if live_trace() && scanned > 0 {
            eprintln!("makepad live: watch round at t={:.0} ms: woke after {:.1} ms, round {:.1} ms after the wake; {}named {:?}, ledger {}, looked {scanned}, changed {}",
                crate::clock::ms_since_start(), woke.duration_since(t_wait).as_secs_f64() * 1e3, woke.elapsed().as_secs_f64() * 1e3,
                if lost { format!("events lost (rewatched in {rewatch_ms:.1} ms), ") } else { String::new() }, named, if ledger.0 != ledger_was { "moved" } else { "still" }, changed.len());
        }
        if !traced && all {
            traced = true;
            eprintln!("makepad live: one watch round: {} files in {:.2} ms; hot set {} files", files.len(), t_round.elapsed().as_secs_f64() * 1e3, hot.len());
        }
        // a held landing's files wait (`hold_landing`); the person's saves go on
        changed.retain(|&k| !held_back(&files[k].0, false));
        let mut quiet_retry = changed.is_empty() && retry_due;
        if changed.is_empty() && !quiet_retry {
            continue;
        }
        changed.extend(stalled.keys().copied());
        changed.sort_unstable();
        changed.dedup();
        if let Some(gate) = dock.as_mut().filter(|_| !synced) {
            // changed by something other than the sync (a lane's build or checkout in this tree,
            // a hand edit): held, the app keeps running the synced code; the next sync applies
            // whatever the files say then
            // (its revision moved meanwhile: the next round is the sync's, at once)
            if gate.held(changed.iter().map(|&k| files[k].0.as_str())) { again = true; }
            continue;
        }
        let seen = std::time::Instant::now();
        if live_trace() { eprintln!("makepad live: a change seen at t={:.0} ms ({} file(s))", crate::clock::ms_since_start(), changed.len()); }
        // from the save on, the post-boot loader keeps off the unit (the read below
        // compares with the running text on it)
        for &k in &changed {
            if !hot.contains(&k) {
                hot.push(k);
            }
        }
        // Let the save settle across the whole watch set. A native notification
        // is a wakeup, not proof that a checkout has finished: index.lock must
        // be gone and HEAD, its ref and every changed file must stay still for
        // a full quiet window. Never give up waiting on a slow writer.
        let look = Look::new();
        let mut last: Vec<_> = files.iter().enumerate().map(|(k, f)| look.stamp(&virt, k, &f.0)).collect();
        let mut git_state = repositories.snapshot();
        let mut still_since = Instant::now();
        // an overlay edit its ledger committed is whole: one look, no quiet window (100 ms of
        // every overlay edit's save -> frame)
        let mut committed = committed_by_ledger(changed.iter().map(|&k| files[k].0.as_str()));
        let mut waited = false;
        loop {
            if !committed {
                // the rest of the quiet window, or for an overlay edit until the watch hears a file
                // or the ledger: one seen between its file's rename and ship-done's last write, the
                // ledger's, goes on at that rename (a third of overlay edits waited out the whole
                // window, 11-03). Every file is looked at below, so an event is only a wakeup; a
                // disk save waits out the window without a look per event (a checkout's files).
                let left = LIVE_QUIET.saturating_sub(still_since.elapsed());
                let overlay = cfg!(unix) && changed.iter().all(|&k| crate::vfs::is_virtual(&files[k].0));
                let rewatch = match events.as_mut().filter(|_| overlay) {
                    Some(w) => {
                        waited = true;
                        // whole milliseconds, rounded up: a wait that ends short of the window
                        // would look again at once
                        w.wait(Some(std::time::Duration::from_millis(left.as_micros().div_ceil(1000) as u64)));
                        w.changed().is_none()
                    }
                    None => {
                        std::thread::sleep(left);
                        false
                    }
                };
                // events lost, or a new edit recorded (it may have made a folder): watched anew
                if rewatch { events = LiveWatch::new(files.iter().map(|f| f.0.as_str())); }
                committed = committed_by_ledger(changed.iter().map(|&k| files[k].0.as_str()));
            }
            let additions = inputs.discover_sources(files.iter().map(|f| f.0.as_str()));
            if !additions.is_empty() {
                let known: std::collections::HashSet<_> = files.iter().map(|f| f.0.clone()).collect();
                for path in additions {
                    if !known.contains(&path) { files.push((path, u32::MAX, None)); }
                }
                repositories.include(files.iter().map(|f| f.0.as_str()));
                events = LiveWatch::new(files.iter().map(|f| f.0.as_str()));
            }
            let look = Look::new();
            let now: Vec<_> = files.iter().enumerate().map(|(k, f)| look.stamp(&virt, k, &f.0)).collect();
            scanned += files.len();
            for (k, f) in files.iter().enumerate() {
                if now[k] != f.2 { changed.push(k); }
            }
            changed.sort_unstable();
            changed.dedup();
            let next_git = repositories.snapshot();
            let moving = now != last || next_git != git_state || next_git.busy;
            last = now;
            git_state = next_git;
            if moving {
                quiet_retry = false;
                still_since = Instant::now();
            } else if committed || still_since.elapsed() >= LIVE_QUIET {
                break;
            }
            committed = false;
        }
        // a wait above may have taken a wakeup meant for the loop's own (a streamed edit, a
        // retry, a domain's inputs): one more round sees what it was for
        if waited { watch_events::kick(); }
        let settled_stamps = last;
        // (what settled may be a held landing's: it waits, the rest applies)
        changed.retain(|&k| !held_back(&files[k].0, true));
        if changed.is_empty() && !quiet_retry {
            continue;
        }
        // A moved lockfile or manifest (any landing moves the checkout's) is this app's only when
        // its own resolved graph moved; else there is nothing to rebuild or restart.
        let manifests: Vec<usize> = changed.iter().copied().filter(|&k| inputs.is_manifest(&files[k].0)).collect();
        if !manifests.is_empty() && !inputs.graph_moved(&(*up).prog.session) {
            eprintln!("makepad live: {} manifest(s) changed, this app's Cargo graph did not", manifests.len());
            for &k in &manifests { files[k].2 = settled_stamps[k]; }
            changed.retain(|k| !manifests.contains(k));
            if changed.is_empty() {
                // nothing to do is the edit's verdict: the app already runs what it means
                if crate::harness::live::enabled() {
                    let mut o = idle_outcome(None, "Cargo graph unchanged");
                    let back = test_edit(up, manifests.iter().map(|&k| files[k].0.clone()).collect(), &mut o, &mut undos);
                    for (old, new) in back { for f in &mut files { if f.1 == new { f.1 = old; } } }
                } else { patch_verified_event(&format!("{} manifest(s)", manifests.len()), 0, &[]); }
                continue;
            }
        }
        // included files are no sources of their own: their includers take the edit (below)
        let included = included_files(&std_dir);
        if synced {
            // A devhub sync of landed work: one whole tree, so no part of it is a half-done
            // checkout. What makepad itself is built from (provided crates, std, the Cargo graph,
            // build scripts) goes to a rebuild of makepad, which restarts only if the binary
            // changed; sources of crates the program has not loaded are read when it loads them;
            // the program's own sources patch as any save does.
            let (host_side, rest): (Vec<usize>, Vec<usize>) = changed.iter().partition(|&&k| inputs.process_input(&files[k].0));
            if !host_side.is_empty() {
                let short = |p: &str| -> String { let v: Vec<&str> = p.rsplit('/').take(2).collect(); v.into_iter().rev().collect::<Vec<_>>().join("/") };
                let names: Vec<String> = host_side.iter().take(3).map(|&k| short(&files[k].0)).collect();
                let why = format!("{} makepad input(s) changed ({}{}): {}", host_side.len(), names.join(", "), if host_side.len() > 3 { ", ..." } else { "" },
                    inputs.restart_reason(host_side.iter().map(|&k| files[k].0.as_str())).unwrap_or("its sources changed"));
                if let Err(e) = host::request(why.clone(), inputs.process_paths(&|_| true)) {
                    let message = format!("{why}; cannot rebuild makepad here ({e}); the app goes on with the makepad it runs");
                    eprintln!("makepad live: {message}");
                    live_log(&format!("{{\"event\":\"host_rebuild\",\"ok\":false,\"message\":{}}}", json_str(&message)));
                }
                for &k in &host_side { files[k].2 = settled_stamps[k]; }
            }
            let (unloaded, rest): (Vec<usize>, Vec<usize>) = rest.into_iter().partition(|&k| files[k].1 == u32::MAX && !files[k].0.ends_with(".rs") && !crate::native_macros::owns_path(&files[k].0) && !included.contains(&files[k].0));
            for &k in &unloaded { files[k].2 = settled_stamps[k]; }
            changed = rest;
            if changed.is_empty() {
                continue;
            }
        }
        // Directory identities only trigger collection. Unloaded Rust sources
        // accompany the batch so new module declarations read its exact texts.
        if !synced {
            let (unloaded, rest): (Vec<usize>, Vec<usize>) = changed.iter().partition(|&&k| files[k].1 == u32::MAX && !files[k].0.ends_with(".rs") && !inputs.process_input(&files[k].0) && !inputs.is_manifest(&files[k].0) && !crate::native_macros::owns_path(&files[k].0) && !included.contains(&files[k].0));
            for &k in &unloaded { files[k].2 = settled_stamps[k]; }
            changed = rest;
            if changed.is_empty() {
                let unloaded: Vec<_> = unloaded.into_iter().filter(|&k| !std::path::Path::new(&files[k].0).is_dir()).collect();
                if !unloaded.is_empty() {
                    let names: Vec<String> = unloaded.iter().take(3).map(|&k| short(&files[k].0)).collect();
                    let what = format!("{}{}: not loaded by the app (nothing to patch)", names.join(", "), if unloaded.len() > 3 { ", ..." } else { "" });
                    // a live test session's verdict is its tests' run: they run again as they are
                    if crate::harness::live::enabled() {
                        let mut o = idle_outcome(None, &what);
                        let back = test_edit(up, unloaded.iter().map(|&k| files[k].0.clone()).collect(), &mut o, &mut undos);
                        for (old, new) in back { for f in &mut files { if f.1 == new { f.1 = old; } } }
                    } else { patch_verified_event(&what, 0, &[]); }
                }
                continue;
            }
        }
        let reason = if synced { None } else { inputs.restart_reason(changed.iter().map(|&k| files[k].0.as_str())) };
        if let Some(reason) = reason {
            // nothing of makepad's own moved (a bulk change of the program's sources): no rebuild
            if !changed.iter().any(|&k| inputs.process_input(&files[k].0)) {
                let why = format!("{} files changed: {reason}", changed.len());
                patch_refused_event(&why, false, std::slice::from_ref(&why));
                if crate::harness::live::enabled() {
                    let group = crate::owner::call(crate::owner::Class::Program, || (*up).begin_patch());
                    crate::harness::live::restart(crate::harness::live::Edit {
                        paths: changed.iter().map(|&k| files[k].0.clone()).collect(), group, compile_ms: None, restarted: true, changed: None, patch: None,
                    }, &why);
                } else { offer_restart(&why); }
                for &k in &changed { files[k].2 = settled_stamps[k]; stalled.remove(&k); }
                continue;
            }
            // a change the running process cannot take: offered, and the person's restart builds
            // makepad in the background and restarts into it; the app keeps running (and
            // patching) meanwhile. A live test session builds now: its verdict is the restarted run,
            // which takes the whole batch. Offered, only makepad's inputs wait for the restart: the
            // program's own sources of the batch patch now, as a dock app's synced batch's do. (They
            // went with the offer, marked seen and never patched: a landing moves makepad's inputs
            // and an app's sources at once, so a later edit of their users read them half-new, and
            // an app refused E0425 on a const a swallowed file added, 11-07.)
            let harness = crate::harness::live::enabled();
            let (host_side, rest): (Vec<usize>, Vec<usize>) = if harness { (changed.clone(), Vec::new()) } else {
                changed.iter().partition(|&&k| inputs.process_input(&files[k].0))
            };
            let what = if host_side.iter().any(|&k| std::path::Path::new(&files[k].0).is_dir()) { "source inputs" } else { "files" };
            let names: Vec<String> = host_side.iter().take(3).map(|&k| short(&files[k].0)).collect();
            let why = format!("{} {what} changed ({}{}): {reason}", host_side.len(), names.join(", "), if host_side.len() > 3 { ", ..." } else { "" });
            let paths = inputs.process_paths(&|_| true);
            let offered = match if harness { host::request(why.clone(), paths) } else { host::offer(why.clone(), paths) } {
                Ok(()) => {
                    // answered at once (a restart follows the build); the build's end answers
                    // again: refused with its errors, no change, or the restart into it
                    patch_refused_event(&why, false, std::slice::from_ref(&why));
                    host_edit = host_side.iter().map(|&k| files[k].0.clone()).collect();
                    for &k in &host_side { files[k].2 = settled_stamps[k]; }
                    true
                }
                Err(e) => { eprintln!("makepad live: cannot rebuild and restart makepad here: {e}"); false }
            };
            if offered {
                if rest.is_empty() { continue; }
                changed = rest;
            } else {
                let package = std::env::var("MAKEPAD_RUN_PACKAGE").unwrap_or_else(|_| "<pkg>".into());
                let what = if changed.iter().any(|&k| std::path::Path::new(&files[k].0).is_dir()) { "source inputs" } else { "files" };
                // provided crates and std are built into this binary: cargo rebuilds it on the way
                let restart = if inputs.host_restart_required(changed.iter().map(|&k| files[k].0.as_str())) {
                    host::note_cannot_build();
                    format!("needs a JIT rebuild: restart it through cargo (which rebuilds makepad): {}", crate::srchash::rebuild_command(&std::env::args().skip(1).collect::<Vec<_>>()))
                } else {
                    format!("restart `makepad run {package}`")
                };
                let message = format!("{} {what} changed: {reason}; {restart} (or `makepad inspect <app> session relaunch`) to pick them up; keeping the running code", changed.len());
                note_restart_pending(&message);
                eprintln!("makepad live: {message}");
                live_log(&format!("{{\"restart_required\":true,\"message\":{}}}", json_str(&message)));
                // The edit's one verdict, at once. A live test session cannot restart into it (the
                // restarted tests would run this binary again): there it is refused.
                patch_refused_event(&message, crate::harness::live::enabled(), std::slice::from_ref(&message));
                for &k in &changed { files[k].2 = settled_stamps[k]; }
                restart_message = message;
                restart_required = true;
                continue;
            }
        }
        let _reloading = jit::Reloading::begin();
        let macro_changes: Vec<_> = changed.iter().map(|&k| (files[k].0.as_str(), None)).collect();
        if let Err(error) = crate::native_macros::reload(&macro_changes, (&(*up).prog.session).runtime.profile) {
            eprintln!("makepad live: {error}");
            live_log(&error);
            if !synced && !crate::harness::live::enabled() {
                patch_refused_event(&error, true, std::slice::from_ref(&error));
                for &k in &changed { files[k].2 = settled_stamps[k]; }
            }
            continue;
        }
        // (a store's file whose edit was taken back stamps UNIX_EPOCH: it has no save time, and
        // watch_ms printed the epoch's age)
        let saved = changed.iter().filter_map(|&k| stamp(&files[k].0).map(|s| s.0)).filter(|t| *t != std::time::UNIX_EPOCH).max();
        let read_started = Instant::now();
        let watch_ms = saved.and_then(|s| s.elapsed().ok()).map_or_else(|| ms(seen), |d| d.as_secs_f64() * 1000.);
        let mut edits = Vec::new();
        let mut which = Vec::new();
        let mut read_failed = false;
        // the files gone from disk (a module's file removed, or a store's edit taken back)
        let mut removed = Vec::new();
        // the included files saved
        let mut moved = Vec::new();
        for &k in &changed {
            let f = &mut files[k];
            let current = settled_stamps[k];
            // a taken file reads its proven tip's text now, never a stalled read of before
            if untake(&f.0) { stalled.remove(&k); }
            if crate::native_macros::owns_path(&f.0) { f.2 = current; continue; }
            if f.1 == u32::MAX && included.contains(&f.0) {
                f.2 = current;
                stalled.remove(&k);
                moved.push(k);
                // a Rust file include! read goes with the batch too, as a source of its own
                // (apply_update): the includer's parse registers its text, and the load's snapshot
                // of it is the old one ("source snapshot changed during load", a restart)
                if !f.0.ends_with(".rs") { continue; }
            }
            let src = match stalled.get(&k).filter(|_| current == f.2).map(|(src, _)| Ok(src.clone())).unwrap_or_else(|| crate::vfs::read(&f.0)) {
                Ok(s) => s,
                Err(e) => {
                    // A removed module can be unbound by its parent's edit: a file gone from disk,
                    // or a mounted store's file whose edit was taken back and that the store
                    // lacks (its stamp is the store's, its read finds nothing). A file that still
                    // exists but cannot be read must never turn a multi-file batch into a partial
                    // transaction: the batch waits for that file's next change (its stamp kept).
                    if current.is_none() || e.kind() == std::io::ErrorKind::NotFound {
                        f.2 = current;
                        removed.push(k);
                    } else {
                        read_failed = true;
                    }
                    continue;
                }
            };
            f.2 = current;
            // unchanged text (touched, or saved back to the running version)
            let same = f.1 != u32::MAX && {
                crate::owner::call(crate::owner::Class::Program, || {
                    let u = &*up;
                    let pf = &u.prog.files[f.1 as usize];
                    pf.src[..pf.src_len as usize] == src[..]
                })
            };
            if !same {
                let first = stalled.get(&k).filter(|(before, _)| *before == src).map(|(_, lines)| lines.clone()).unwrap_or_default();
                stalled.insert(k, (src.clone(), first));
                edits.push(Edit { file: f.1, path: f.0.clone(), src });
                which.push(k);
            } else {
                stalled.remove(&k);
            }
        }
        if read_failed {
            continue;
        }
        // A running module's file removed while the file that declares it is not in this batch:
        // the program still declares the module and has no file for it (rustc E0583). Nothing of
        // the batch is installed and the app keeps the module's previous code; the batch's edits
        // wait for the next change (the file's return, or the declaring file's edit).
        let undeclared = if removed.is_empty() { Vec::new() } else {
            let edited: std::collections::HashSet<&str> = edits.iter().map(|e| e.path.as_str()).chain(moved.iter().map(|&k| files[k].0.as_str())).collect();
            let gone: Vec<&str> = removed.iter().map(|&k| files[k].0.as_str()).collect();
            crate::owner::call(crate::owner::Class::Program, || undeclared_removals(&(*up).prog, &gone, &edited))
        };
        if !undeclared.is_empty() {
            for (&k, e) in which.iter().zip(&edits) {
                stalled.insert(k, (e.src.clone(), undeclared.clone()));
            }
            for &k in &moved { stalled.insert(k, (Vec::new(), undeclared.clone())); }
            let what: Vec<String> = removed.iter().chain(&which).map(|&k| files[k].0.rsplit('/').next().unwrap_or("").to_string()).collect();
            let what = what.join(", ");
            let o = Outcome { failure: None, kept: 0, pending: false, rung: Rung::Refuse, files: Vec::new(), lines: undeclared.clone(), names: Vec::new(), changed: 0, test_changes: None, bodies: Vec::new(), group: 0, ms: [0.0; 4], undo: None, work: [0; 5] };
            log_edit(&what, &o, saved, seen);
            eprintln!("makepad live: {}: NOT applied (errors below); the app runs the previous code", what);
            for l in &undeclared { eprintln!("makepad live:   {}", l); }
            patch_refused_event(&what, true, &undeclared);
            continue;
        }
        if !moved.is_empty() {
            // each own file whose code includes a saved file: its running text again, whose items
            // that include it `classify` finds changed
            let paths: Vec<String> = moved.iter().map(|&k| files[k].0.clone()).collect();
            let skip: Vec<String> = edits.iter().map(|e| e.path.clone()).collect();
            for e in crate::owner::call(crate::owner::Class::Program, || include_edits(&(*up).prog, &paths, &skip)) {
                if let Some(k) = files.iter().position(|f| f.0 == e.path) {
                    edits.push(e);
                    which.push(k);
                }
            }
            // one no code's text names: never "patched" while the program keeps what it read
            let unmatched = crate::owner::call(crate::owner::Class::Program, || include_unmatched(&(*up).prog, &paths));
            if !unmatched.is_empty() {
                let names: Vec<&str> = unmatched.iter().map(|p| p.rsplit('/').next().unwrap_or("")).collect();
                offer_restart(&format!("{}: changed, and the code that includes it makes its path from other tokens (no item's text or expansion names it): which code read it is not known", names.join(", ")));
                if edits.is_empty() {
                    continue;
                }
            }
        }
        if edits.is_empty() {
            // saved back to the running text (e.g. the fix after a refused edit): the program
            // already runs it -- an edit applied in place for the scoreboard
            let mut o = Outcome { failure: None, kept: 0, pending: false, rung: Rung::Patch, files: Vec::new(), lines: Vec::new(), names: Vec::new(), changed: 0, test_changes: None, bodies: Vec::new(), group: 0, ms: [0.0; 4], undo: None, work: [0; 5] };
            let what: Vec<String> = changed.iter().map(|&k| files[k].0.rsplit('/').next().unwrap_or("").to_string()).collect();
            log_edit(&what.join(", "), &o, saved, seen);
            if crate::harness::live::enabled() {
                drop(_reloading);
                let back = test_edit(up, changed.iter().map(|&k| files[k].0.clone()).collect(), &mut o, &mut undos);
                for (old, new) in back { for f in &mut files { if f.1 == new { f.1 = old; } } }
                continue;
            }
            eprintln!("makepad live: {}: patched (0 item(s) changed, 0 fn(s) installed) in 0.0 ms", what.join(", "));
            patch_verified_event(&what.join(", "), 0, &[]);
            continue;
        }
        let names: Vec<String> = edits.iter().map(|e| e.path.rsplit('/').next().unwrap_or("").to_string()).collect();
        let originals: Vec<_> = edits.iter().map(|e| e.src.clone()).collect();
        // A writer may have resumed while we read. Keep these texts pending and
        // collect its next batch before compiling or requesting a recovery.
        let look = Look::new();
        if files.iter().enumerate().map(|(k, f)| look.stamp(&virt, k, &f.0)).ne(settled_stamps.iter().copied()) || repositories.snapshot() != git_state {
            again = true;
            continue;
        }
        trace_input(watch_ms, (read_started - seen).as_secs_f64() * 1000., ms(read_started), scanned);
        let mut o = apply_update(up, edits);
        let only_previous_errors = !o.lines.is_empty() && o.lines.iter().all(|l| l.ends_with("type checking failed due to previous errors"));
        let mut failure = o.failure.or_else(|| (o.rung == Rung::Refuse && internal_failure(&o.lines)).then_some(makepad_remote::hot::Outcome::Refused));
        let what = names.join(", ");
        if o.rung == Rung::Refuse || o.failure.is_some() {
            // Only an unchanged-tree retry can reuse earlier diagnostics. A
            // new batch must not inherit an ICE from an older tree and mistake
            // its own source error for a reason to restart.
            if quiet_retry {
                for (&k, src) in which.iter().zip(&originals) {
                    if let Some((_, first)) = stalled.get(&k).filter(|(before, _)| before == src) {
                        o.lines = refusal_lines(first.clone(), std::mem::take(&mut o.lines));
                    }
                }
            }
            for (&k, src) in which.iter().zip(originals) {
                stalled.insert(k, (src, o.lines.clone()));
            }
            for &k in &moved { stalled.insert(k, (Vec::new(), o.lines.clone())); }
            if quiet_retry && only_previous_errors && internal_failure(&o.lines) {
                failure = Some(makepad_remote::hot::Outcome::Refused);
            }
            log_edit(&what, &o, saved, seen);
            eprintln!("makepad live: {}: NOT applied (errors below); the app runs the previous code", what);
            for l in &o.lines { eprintln!("makepad live:   {}", l); }
            patch_refused_event(&what, true, &o.lines);
            // An unchanged tree compiles the same verdict again: a failure of makepad itself
            // recovers now, the edit's own errors wait for the tree's next change. A tree that
            // moved while this compiled has that move's events queued: their round retries it.
            let look = Look::new();
            let unchanged = files.iter().enumerate().map(|(k, f)| look.stamp(&virt, k, &f.0)).eq(settled_stamps.iter().copied()) && repositories.snapshot() == git_state;
            if unchanged {
                if let Some(failure) = failure {
                    drop(_reloading);
                    if crate::harness::live::enabled() {
                        if crate::hot_failure::failed(failure, &o.lines, None) {
                            let group = crate::owner::call(crate::owner::Class::Program, || (*up).begin_patch());
                            crate::harness::live::restart(crate::harness::live::Edit {
                                paths: which.iter().map(|&k| files[k].0.clone()).collect(), group,
                                compile_ms: Some(o.ms[2]), restarted: true, changed: None, patch: None,
                            }, &o.lines.join("; "));
                        }
                    } else { recover_hot_failure(failure, &o.lines, None); }
                }
            }
            continue;
        }
        if o.rung == Rung::Patch {
            let loaded = crate::owner::call(crate::owner::Class::Program, || (*up).prog.current_source_files(|p| own_source(p, &std_dir)));
            for &k in &which {
                if loaded.contains_key(&files[k].0) {
                    stalled.remove(&k);
                }
            }
            // every path: a patch also gives files it did not edit new versions (a rebuilt fn's
            // closures and async blocks are new: apply_locked)
            for f in files.iter_mut() {
                if let Some(&file) = loaded.get(&f.0) {
                    f.1 = file;
                }
            }
            set_watch_loaded(&files);
            // Module additions can load files without a post-boot loader turn: a loaded file
            // the watcher does not know yet is taken in by a rescan before the next save reaches
            // it (the known ones took their indices above). Asked when any watched file was not
            // loaded, it rescanned after every Fab patch (an owner request) and held the watch
            // round for the first frames again.
            let known: std::collections::HashSet<&str> = files.iter().map(|f| f.0.as_str()).collect();
            if loaded.keys().any(|p| !known.contains(p.as_str()) && crate::vfs::is_file(p)) {
                startup_scan = true;
            }
        }
        if o.rung == Rung::Restart {
            RESTART_SAVED_MS.store(saved.map_or(0.0, unix_ms) as u64, std::sync::atomic::Ordering::Relaxed);
        }
        log_edit(&what, &o, saved, seen);
        if crate::harness::live::enabled() {
            drop(_reloading);
            let back = test_edit(up, which.iter().map(|&k| files[k].0.clone()).collect(), &mut o, &mut undos);
            for (old, new) in back { for f in &mut files { if f.1 == new { f.1 = old; } } }
            continue;
        }
        match o.rung {
            Rung::Patch => eprintln!(
                "makepad live: {}: patched ({} item(s) changed, {} fn(s) installed) in {:.1} ms",
                what,
                o.changed,
                o.names.len(),
                o.ms.iter().sum::<f64>()
            ),
            Rung::Refuse => eprintln!("makepad live: {}: NOT applied (errors below); the app runs the previous code", what),
            // an edit the patcher cannot absorb: makepad restarts for real (the app is asked at its
            // next safe point and keeps its state); it runs the previous code until then
            Rung::Restart => restart_for_edit(&what, &o.lines),
        }
        // what of it the patch held back waits for the restart (hold_back)
        let held: Vec<String> = o.lines.iter().filter_map(|l| l.strip_prefix(HELD)).map(str::to_string).collect();
        if o.rung == Rung::Patch && !held.is_empty() {
            restart_for_edit(&what, &held);
        }
        for l in &o.lines {
            eprintln!("makepad live:   {}", l);
        }
        if o.rung != Rung::Patch {
            patch_refused_event(&what, o.rung == Rung::Refuse, &o.lines);
        }
        crate::tui::flush();
        if o.rung == Rung::Patch {
            patch_landed(which.iter().map(|&k| files[k].0.clone()), changed_items(up, &o));
            if o.group != 0 { landing.push((Instant::now() + LANDING, o.group, what.clone())); }
        }
        // what is left unverified reports when the borrow checker answers; else the verdict is now
        let patched = o.rung == Rung::Patch;
        let group = o.group;
        let mut awaits_check = false;
        if let Some(u) = o.undo {
            // SPEC-V: the slots this group installed without a clean verdict (all of them, unless
            // --check-first refused what failed before the install)
            let group = u.group;
            let ids: Vec<u32> = crate::owner::call(crate::owner::Class::Program, || unverified_patch(up, group));
            if !ids.is_empty() {
                UNVERIFIED_PATCHES.fetch_add(1, Ordering::AcqRel);
                pending.push(PendingPatch { group, what: what.clone(), ids, event_seq: crate::inspect::events::last_seq() });
                awaits_check = true;
            }
            undos.push(u);
            // rollback by patch group arrives with the runtime panic boundaries (M7)
            if undos.len() > 64 {
                undos.remove(0);
            }
        }
        if patched && !awaits_check {
            patch_verified_event(&what, group, &[]);
        }
    }
}

#[cfg(test)]
mod round32_tests {
    use super::*;

    #[test]
    fn enum_reload_snapshots() {
        const CHILD: &str = "MAKEPAD_RUN_ENUM_RELOAD_CASE";
        let Ok(case) = std::env::var(CHILD) else {
            for case in ["same", "changed", "order", "removed", "reordered", "added", "larger", "named", "executable", "callee", "named_code", "unrelated"] {
                let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "reload::round32_tests::enum_reload_snapshots", "--nocapture", "--test-threads=1"])
                    .env(CHILD, case).env("MAKEPAD_RUN_CTFE", "0").env("MAKEPAD_RUN_CACHE", "off").env("MAKEPAD_RUN_GANDALF", "0"));
                assert!(output.status.success(), "{case}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            }
            return;
        };
        std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
        crate::borrowck::lazy::disable_gandalf();
        let (old, new, accept) = match case.as_str() {
            "same" => ("enum E { A = 1, B }", "enum E { A = 1 + 0, B }", true),
            // M10: the running values keep their tags (AdtT::live_tags) and read the new numbers
            "changed" => ("enum E { A = 1, B }", "enum E { A = 2, B }", true),
            // ... unless the running variants' numbers change their order (a derived `Ord`)
            "order" => ("enum E { A = 1, B = 2 }", "enum E { A = 3, B = 2 }", false),
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
            assert_eq!(&values(&unit)[..2], if case == "changed" { vec![2, 3] } else { before.clone() });
            unsafe { rollback(up, outcome.undo.unwrap(), "test"); }
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
mod use_edit_tests {
    use super::*;

    /// Editing a `use` of a same-module enum / fn (the imported def's scope is the module itself)
    /// takes its old bindings out: no E0252 against its own earlier binding, and the edit patches.
    #[test]
    fn editing_a_use_of_a_same_module_item_patches() {
        const CHILD: &str = "MAKEPAD_RUN_USE_EDIT_CASE";
        let Ok(case) = std::env::var(CHILD) else {
            for case in ["enum_variants", "fn_rename", "use_in_closure", "closure_added"] {
                let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "reload::use_edit_tests::editing_a_use_of_a_same_module_item_patches", "--nocapture", "--test-threads=1"])
                    .env(CHILD, case).env("MAKEPAD_RUN_CTFE", "0").env("MAKEPAD_RUN_CACHE", "off").env("MAKEPAD_RUN_GANDALF", "0"));
                assert!(output.status.success(), "{case}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            }
            return;
        };
        std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
        crate::borrowck::lazy::disable_gandalf();
        let (old, new) = match case.as_str() {
            "enum_variants" => (
                "enum M { A, B }\nuse M::{A, B};\nfn main() { let _ = (A, B); }\n",
                "enum M { A, B }\nuse M::{B, A};\nfn main() { let _ = (A, B); }\n"),
            // a `use` and a fn added inside a closure body (LIVEDEBUG2: their defs were never
            // collected, so the declaration table the borrow checker reads stopped short of them)
            // a body edit that adds a closure: def_collector gives it a def at the commit
            "closure_added" => (
                "fn main() { let _ = 1u32; }\n",
                "fn main() { let f = || 2u32; let g = move |x: u32| x + f(); let _ = g(1); }\n"),
            "use_in_closure" => (
                "mod m { pub fn g() -> u32 { 1 } }\nfn main() { let f = || 1u32; let _ = f(); }\n",
                "mod m { pub fn g() -> u32 { 1 } }\nfn main() { let f = || { use self::m::g as h; fn k() -> u32 { 2 } h() + k() }; let _ = f(); }\n"),
            _ => (
                "fn f() {}\nuse self::f as g;\nfn main() { g(); }\n",
                "fn f() {}\nuse self::f as g;\nfn main() { g(); g(); }\n"),
        };
        let path = std::env::temp_dir().join(format!("relax-use-edit-{}-{}.rs", case, std::process::id()));
        std::fs::write(&path, old).unwrap();
        let mut session = crate::Session::default();
        session.options.ctfe = crate::options::Ctfe::Off;
        let session = std::sync::Arc::new(session);
        let (prog, core, krate) = crate::load_with_std(&session, path.to_str().unwrap(), "use_edit", false).unwrap();
        let file = prog.defs.iter().find(|d| d.krate == krate && d.kind == DefKind::Fn).map(|d| d.file).unwrap();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        // the declaration table exists before the edit (as in a running app)
        let main = (0..prog.defs.len() as u32).map(DefId).find(|d| prog.def(*d).krate == krate && prog.name(*d) == "main").unwrap();
        let _ = tcx.generic_indices_of(&prog, main);
        crate::layout::set_root_crate(krate);
        crate::layout::LIVE_SESSION.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut unit = Unit::new(prog, tcx, core);
        let up = &mut *unit as *mut Unit;
        let outcome = unsafe { apply(up, vec![Edit { file, path: path.to_string_lossy().into_owned(), src: new.as_bytes().to_vec() }]) };
        assert_eq!(outcome.rung, Rung::Patch, "{case}: {:?}", outcome.lines);
        if case == "use_in_closure" {
            // the body's new fn `k` has a def
            assert!((0..unit.prog.defs.len() as u32).map(DefId).any(|d| unit.prog.def(d).krate == krate && unit.prog.name(d) == "k"), "k collected");
        }
        if case == "use_in_closure" || case == "closure_added" {
            // every def of the crate, the edit's new ones too, has its declarations (the borrow
            // checker of the edited fn asks for its closures')
            for d in (0..unit.prog.defs.len() as u32).map(DefId).filter(|d| unit.prog.def(*d).krate == krate) {
                let _ = unit.tcx.generic_indices_of(&unit.prog, d);
            }
        }
        std::mem::forget(unit);
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod moved_type_tests {
    use super::*;

    /// A type moved out of a file whose kept fn still names it, now through a `use` (an image
    /// decoder moved to a crate of its own, 11-08): `read`, the fn's signature was asked before
    /// the edit; `built`, code for the fn exists. The edit's outcome and lines.
    fn move_type(name: &str, read: bool, built: bool) -> (Rung, Vec<String>) {
        std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
        crate::borrowck::lazy::disable_gandalf();
        let old = "pub struct Pic { pub w: u32 }\npub fn width<'a>(p: &'a Pic) -> &'a u32 { &p.w }\nfn main() { let p = Pic { w: 1 }; let _ = *width(&p); }\n";
        let new = "mod decode { pub struct Pic { pub w: u32 } }\nuse decode::Pic;\npub fn width<'a>(p: &'a Pic) -> &'a u32 { &p.w }\nfn main() { let p = Pic { w: 2 }; let _ = *width(&p); }\n";
        let path = std::env::temp_dir().join(format!("relax-moved-type-{}-{}.rs", name, std::process::id()));
        std::fs::write(&path, old).unwrap();
        let mut session = crate::Session::default();
        session.options.ctfe = crate::options::Ctfe::Off;
        let session = std::sync::Arc::new(session);
        let (prog, core, krate) = crate::load_with_std(&session, path.to_str().unwrap(), "moved_type", false).unwrap();
        let file = prog.defs.iter().find(|d| d.krate == krate && d.kind == DefKind::Fn).map(|d| d.file).unwrap();
        let width = (0..prog.defs.len() as u32).map(DefId).find(|d| prog.def(*d).krate == krate && prog.name(*d) == "width").unwrap();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        if read {
            assert!(tcx.fn_sig(&prog, width).is_some());
        }
        crate::layout::set_root_crate(krate);
        crate::layout::LIVE_SESSION.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut unit = Unit::new(prog, tcx, core);
        if built {
            unit.fn_id(FnKey::Inst(width, Vec::new()));
        }
        let up = &mut *unit as *mut Unit;
        let outcome = unsafe { apply(up, vec![Edit { file, path: path.to_string_lossy().into_owned(), src: new.as_bytes().to_vec() }]) };
        assert!(!outcome.lines.iter().any(|l| l.contains("internal compiler error")), "{:?}", outcome.lines);
        if outcome.rung == Rung::Patch {
            // the kept fn is declared with the moved type, as a fresh build declares it
            let param = unit.tcx.fn_sig(&unit.prog, width).unwrap().params[0];
            let moved = (0..unit.prog.defs.len() as u32).map(DefId).find(|d| unit.prog.def(*d).krate == krate && unit.prog.def_path(*d).ends_with("decode::Pic")).unwrap();
            assert!(ty_names_def(&unit, param, &[moved], 0), "width takes the moved type");
        } else {
            // nothing installed: the running declaration is back, the type before the move
            let param = unit.tcx.fn_sig(&unit.prog, width).unwrap().params[0];
            let running = (0..unit.prog.defs.len() as u32).map(DefId).find(|d| unit.prog.def(*d).krate == krate && unit.prog.def(*d).kind == DefKind::Struct && !unit.prog.def_path(*d).contains("decode")).unwrap();
            assert!(ty_names_def(&unit, param, &[running], 0), "width takes the running type again");
        }
        std::mem::forget(unit);
        std::fs::remove_file(path).unwrap();
        (outcome.rung, outcome.lines)
    }

    /// The edit's check of the declarations naming the removed type reads the kept fn's signature
    /// in the file's new version; with a lifetime in it, that resolves the fn's bound variables,
    /// which read the new version's file input. It must exist by then: the edit stopped with an
    /// internal compiler error (a missing query input), and the program it left half-made failed
    /// later compiles the same way.
    #[test]
    fn moving_a_type_its_lifetime_signatures_name_patches() {
        isolated_test!(moving_a_type_its_lifetime_signatures_name_patches);
        let (rung, lines) = move_type("unread", false, false);
        assert_eq!(rung, Rung::Patch, "{lines:?}");
    }

    /// A signature collected before the edit names the removed type, though the name means the
    /// moved one now: no E0412 for the name and no E0308 at the call. With no code built for the
    /// fn, it is declared again and the edit patches.
    #[test]
    fn a_signature_read_before_a_type_moved_is_declared_again() {
        isolated_test!(a_signature_read_before_a_type_moved_is_declared_again);
        let (rung, lines) = move_type("read", true, false);
        assert_eq!(rung, Rung::Patch, "{lines:?}");
    }

    /// The same with code built for the fn: built against the old declaration, it is not
    /// rebuilt live, so the edit asks for a restart (and refuses nothing).
    #[test]
    fn a_type_moved_under_built_code_asks_for_a_restart() {
        isolated_test!(a_type_moved_under_built_code_asks_for_a_restart);
        let (rung, lines) = move_type("built", true, true);
        assert_eq!(rung, Rung::Restart, "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("names `Pic`, which this edit moves")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("E0412") || l.contains("E0308")), "{lines:?}");
    }
}

#[cfg(test)]
mod watch_wait_tests {
    use super::*;

    #[test]
    fn an_inputs_ask_during_the_boot_waits_for_the_watcher() {
        isolated_test!(an_inputs_ask_during_the_boot_waits_for_the_watcher);
        assert!(input_paths(&|_| true).is_err(), "no live run: no watcher to wait for");
        // a live run that has not laid out its program yet
        crate::run::LIVE_SESSION.store(true, std::sync::atomic::Ordering::Release);
        assert!(!crate::layout::LIVE_SESSION.load(std::sync::atomic::Ordering::Acquire));
        let ask = std::thread::spawn(|| input_paths(&|_| true));
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!ask.is_finished(), "the ask waits for the watcher");
        set_watch_inputs(&Inputs::default(), &[]);
        assert_eq!(ask.join().unwrap(), Ok(Vec::new()));
    }

    /// A watcher that panicked reports the recovery it asked for while the restart is pending.
    /// A restart in place ends the run first: the watcher leaves with it, and the run's end finds
    /// no thread of it left. (It waited on, out of the run's reach: the end left it behind with
    /// the unit, and it watched that ended unit again once the next run's restore completed the
    /// recovery -- a fault outside JIT code, not contained, in a live app session.)
    #[test]
    fn a_watcher_awaiting_its_recovery_leaves_with_the_run() {
        isolated_test!(a_watcher_awaiting_its_recovery_leaves_with_the_run);
        // (its own home: the record is no record for this machine's ledger)
        let home = std::env::temp_dir().join(format!("relax-await-recovery-{}", std::process::id()));
        std::env::set_var("MAKEPAD_HOME", &home);
        crate::hot_failure::enable();
        assert!(crate::hot_failure::failed(makepad_remote::hot::Outcome::Panic, &["the live watcher panicked: test".to_string()], None));
        assert!(crate::hot_failure::recovering());
        let watcher = crate::run_threads::spawn("run-live", await_recovery).unwrap();
        // the recovery stays pending: its restart's next run never comes in this test
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!watcher.is_finished(), "the watcher reports the pending recovery");
        let stayed = crate::run_threads::end_all(std::time::Duration::from_secs(10));
        assert!(stayed.is_empty(), "the run's end left its watcher behind: {stayed:?}");
        assert!(!watcher.join().unwrap(), "the run ended: the watcher does not watch again");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A live edit that fails (here an internal compiler error applying it) leaves the app running
    /// its code: the restart that applies the edit is offered and waits for the person, it is not
    /// started. (A landing reached the person's running Fab, applying it panicked, and the
    /// recovery restarted the app in place on its own.)
    #[test]
    fn a_failed_edit_offers_its_restart_and_the_run_goes_on() {
        isolated_test!(a_failed_edit_offers_its_restart_and_the_run_goes_on);
        let home = std::env::temp_dir().join(format!("relax-failed-edit-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var("MAKEPAD_HOME", &home);
        std::env::set_var("MAKEPAD_RUN_LIVE_LOG", home.join("live.jsonl"));
        crate::hot_failure::enable();
        let why = "internal compiler error applying the edit: test -- makepad run bug, report it".to_string();
        recover_hot_failure(makepad_remote::hot::Outcome::Panic, std::slice::from_ref(&why), None);
        assert!(restart_pending().contains(&why), "the restart is offered: {:?}", restart_pending());
        assert!(!RESTART_CONFIRMED.load(Ordering::SeqCst), "nobody took the offer: no restart is under way");
        assert!(!RESTART_REQUESTED.load(Ordering::SeqCst), "the app is not asked to restart");
        assert!(!crate::run_threads::ending(), "the run goes on");
        assert!(crate::hot_failure::recovering(), "the recovery waits for the restart the person takes");
        let log = std::fs::read_to_string(home.join("live.jsonl")).unwrap_or_default();
        assert!(log.contains("\"event\":\"restart_offered\""), "the offer is in the live log (the WM's toast): {log}");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Every `session relaunch` says who asked, a repeat too: the run window's click or key, or
    /// the inspect client's process, which the command's connection names (this test's own, here).
    #[test]
    fn a_relaunch_says_who_asked() {
        let why = "the Splash VM changed";
        assert_eq!(relaunch_line("a click on the ring's dot (Update ready)", why, false), "makepad live: session relaunch, asked by a click on the ring's dot (Update ready): the Splash VM changed");
        assert_eq!(relaunch_line("the R key in the run window", "", true), "makepad live: session relaunch, asked by the R key in the run window again: the restart is on its way (nothing waits)");
        // no command is answered on this thread: its asker is not known
        assert_eq!(inspect_asker(), "an inspect client whose process its connection does not name");
    }
}

#[cfg(test)]
mod removed_module_tests {
    use super::*;

    /// A running module's file gone from disk while the file that declares it stays as it was is
    /// refused as rustc's E0583 (the app keeps the module's code; a "patched (0 items)" verdict
    /// was a silent wrong patch, 11-04). The declaring file's edit in the same batch decides
    /// instead (the compile unbinds the module or reports it), and a crate root or a file the
    /// program does not run is no such module.
    #[test]
    fn a_removed_module_file_its_parent_still_declares_is_refused() {
        isolated_test!(a_removed_module_file_its_parent_still_declares_is_refused);
        std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
        let dir = std::env::temp_dir().join(format!("relax-removed-module-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("zz")).unwrap();
        std::fs::write(dir.join("main.rs"), "mod zz;\nmod leaf;\nfn main() { let _ = zz::f() + leaf::g(); }\n").unwrap();
        std::fs::write(dir.join("zz/mod.rs"), "pub fn f() -> u32 { 1 }\n").unwrap();
        std::fs::write(dir.join("leaf.rs"), "pub fn g() -> u32 { 2 }\n").unwrap();
        let session = std::sync::Arc::new(crate::Session::default());
        let (prog, _, _) = crate::load_with_std(&session, dir.join("main.rs").to_str().unwrap(), "removed_module", false).unwrap();
        let path = |end: &str| prog.file_paths.iter().find(|p| p.ends_with(end)).cloned().unwrap();
        let (main, zz, leaf) = (path("/main.rs"), path("/zz/mod.rs"), path("/leaf.rs"));
        let none = std::collections::HashSet::new();
        let lines = undeclared_removals(&prog, &[zz.as_str(), leaf.as_str()], &none);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with("error[E0583]: file not found for module `zz`") && lines[0].contains(&main), "{lines:?}");
        assert!(lines[1].starts_with("error[E0583]: file not found for module `leaf`"), "{lines:?}");
        // the declaring file is in the batch: its compile decides
        let edited: std::collections::HashSet<&str> = [main.as_str()].into_iter().collect();
        assert!(undeclared_removals(&prog, &[zz.as_str()], &edited).is_empty());
        // the crate root, and a file the program does not run
        let other = dir.join("other.rs").to_string_lossy().into_owned();
        assert!(undeclared_removals(&prog, &[main.as_str(), other.as_str()], &none).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A read of the running code (prof, mem, eval, ..) while a saved edit waits for a restart
    /// heads its answer with STALE and the edit: it measures the code from before it (a layout
    /// change measured for hours as if it ran, 11-04). A run that loads the sources clears it.
    #[test]
    fn a_read_while_an_edit_waits_for_a_restart_says_stale() {
        isolated_test!(a_read_while_an_edit_waits_for_a_restart_says_stale);
        assert_eq!(crate::inspect::stale_line(), None);
        note_restart_pending("main.rs: the layout of WorkStats changed");
        let line = crate::inspect::stale_line().unwrap();
        assert!(line.contains("STALE: 1 edit waits for a restart (main.rs: the layout of WorkStats changed)") && line.contains("\"restart_pending\":1"), "{line}");
        clear_restart_pending();
        assert_eq!(crate::inspect::stale_line(), None);
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
mod split_operator_tests {
    #[test]
    fn split_glued_operators_read_as_written() {
        isolated_test!(split_glued_operators_read_as_written);
        let session = std::sync::Arc::new(crate::Session::default());
        // The outer closure's closing `|` glues to the inner closure's opening `|`.
        let source = br#"
            fn a(v: &[&String], s: &String) -> bool { v.contains(&&s) }
            fn b() { let v: Vec<Vec<u8>> = Vec::new(); }
            fn c() { let f = |_: bool||| true; }
            fn d() { let r = &&mut 1; }
        "#;
        let old = crate::parser::parse_owned(&session, source.to_vec(), 2021).unwrap();
        let mut new = crate::parser::parse_owned(&session, source.to_vec(), 2021).unwrap();
        new.toks = Default::default();
        let (pairs, removed, added) = super::pair_items(&old, &new);
        assert!(removed.is_empty(), "removed: {:?}", removed);
        assert!(added.is_empty(), "added: {:?}", added);
        assert_eq!(pairs.len(), 4);
        for (i, j) in pairs {
            let (a, b) = (&old.ast.items[i as usize], &new.ast.items[j as usize]);
            let crate::ast::ItemKind::Fn(_, Some(body), _) = &a.kind else { panic!("expected parsed fn body"); };
            let body = old.ast.block(*body);
            assert!(old.toks.iter().zip(old.toks.iter().skip(1)).any(|(prev, t)|
                t.lo >= body.lo && t.hi <= body.hi && t.lo > prev.hi && t.lo > 0
                && matches!(old.src.get(t.lo as usize - 1), Some(b'<' | b'>' | b'&' | b'|' | b'+'))), "no split in item {}", i);
            assert!(super::same_toks(&old, a.lo, a.hi, &[], &new, b.lo, b.hi, &[]), "item {} differs", i);
            let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
            let mut expected = Vec::new();
            for part in source[a.lo as usize..a.hi as usize].split(u8::is_ascii_whitespace).filter(|p| !p.is_empty()) {
                if expected.last().copied().is_some_and(word) && word(part[0]) { expected.push(b' '); }
                expected.extend_from_slice(part);
            }
            assert_eq!(super::toks_norm(&old, a.lo, a.hi, &[]), vec![expected], "item {} lost source text", i);
        }
    }
}

#[cfg(test)]
mod vtable_name_tests {
    use super::{vtable_name_types, vtable_types};

    /// A vtable linked from the disk cache is known by its data name only: the two types it gives
    /// (the checks of a trait's method edit find the trait objects of it by the dyn type).
    #[test]
    fn a_vtables_name_gives_its_two_types() {
        let t = |a: &str, b: &str| Some((a.to_string(), b.to_string()));
        assert_eq!(vtable_name_types("vtable:my::Foo#0123abcd:dyn core::fmt::Display#89abcdef"), t("my::Foo", "dyn core::fmt::Display"));
        assert_eq!(vtable_name_types("vtable:Vec<my::Foo>#0123abcd:dyn my::Tr<u8> + Send + Sync#89abcdef"), t("Vec<my::Foo>", "dyn my::Tr<u8> + Send + Sync"));
        assert_eq!(vtable_types("vtable:my::Foo#0123abcd:dyn my::Tr#89abcdef"), "my::Foo` as `dyn my::Tr");
        assert_eq!(vtable_name_types("const:my::K"), None);
    }
}

#[cfg(test)]
mod include_part_tests {
    use super::*;

    /// A file's lazily parsed bodies are parsed together into a part whose expansions sit past the
    /// file's text (ParsedFile::part). The include lookup reads the expansions of an item's macro
    /// calls: a part's through the part. (It sliced the file's own text there: "range start index
    /// .. out of range for slice of length .." panicked the live watcher, and an edit's apply, in
    /// an app's live sessions after a landing.)
    #[test]
    fn an_items_include_lookup_reads_its_parsed_bodys_expansions() {
        isolated_test!(an_items_include_lookup_reads_its_parsed_bodys_expansions);
        crate::parser::LAZY_AUTO.store(true, Ordering::Relaxed);
        // an expansion at the load whose text names an include (the lookup looks into expansions
        // then), and a body with built-in macros' calls (it stays lazy): a format with arguments
        // (an app's `format!("the demo {} has no {project}", self.key)`) and a concat
        let source = r#"
macro_rules! named {
    () => { pub const include_me: usize = 1; };
}
named!();
pub fn reads() -> usize {
    let said = format!("{}.{}", include_me, 2);
    concat!("do", "cs").len() + include_me + said.len()
}
"#;
        let path = std::env::temp_dir().join(format!("relax-include-part-{}.rs", std::process::id()));
        std::fs::write(&path, source).unwrap();
        let path_text = path.to_str().unwrap();
        let session = std::sync::Arc::new(crate::Session::default());
        let cfg = crate::cfg::CfgSet::host(&session, &[]);
        let mut prog = Program::new(&session);
        // the format family's built-in expansion, as a live session has it with core loaded
        std::sync::Arc::make_mut(&mut prog.macros).s2_fmt = true;
        prog.load_crate("include_part", path_text, 2021, cfg.clone(), vec![]).unwrap();
        let reads = (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|&d| prog.name(d) == "reads").unwrap();
        let pf = prog.files[prog.def(reads).file as usize].clone();
        assert!(expansions_include(&pf), "the expansion at the load names an include");
        let part = pf.part(&cfg).expect("the fn's body was skipped at the load");
        let regions: Vec<(u32, u32, u32)> = part.exp_map.iter().filter(|&(a, b, _)| a < b).collect();
        assert!(regions.len() >= 2, "the body's format! and concat! expand into the part: {regions:?}");
        let mut texts = String::new();
        for &(a, b, _) in &regions {
            assert!(a as usize >= pf.src.len(), "the part's expansions sit past the file's text ({a} < {})", pf.src.len());
            texts.push_str(&String::from_utf8_lossy(&toks(&pf, a, b, &[]).concat()));
        }
        assert!(texts.contains("docs"), "the expansions' own text: {texts:?}");
        let it = pf.ast.item(prog.def(reads).item);
        assert_eq!(item_includes(&pf, path_text, it, true), Vec::<String>::new(), "the body includes nothing");
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod derive_reload_tests {
    use super::*;

    #[test]
    fn derive_identity_survives_successive_patches() {
        isolated_test!(derive_identity_survives_successive_patches);
        let source = r#"#![no_std]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record { pub value: u32 }
mod nested {
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Record { pub value: u32 }
}
macro_rules! record {
    () => {
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct Expanded { pub value: u32 }
    };
}
record!();
pub fn revision() -> u32 { 0 }
"#;
        let path = std::env::temp_dir().join(format!("relax-derive-reload-{}.rs", std::process::id()));
        std::fs::write(&path, source).unwrap();
        let path_text = path.to_str().unwrap();
        let session = std::sync::Arc::new(crate::Session::default());
        let cfg = crate::cfg::CfgSet::host(&session, &[]);
        let mut prog = Program::new(&session);
        // parse_owned and a standalone load without core do not expand these derives.
        // Enable the same built-in expander that loading core enables in a live session.
        std::sync::Arc::make_mut(&mut prog.macros).s2_fmt = true;
        prog.load_crate("derive_reload", path_text, 2021, cfg.clone(), vec![]).unwrap();
        let revision = (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|&d| prog.name(d) == "revision").unwrap();
        let mut file = prog.def(revision).file;
        let first = source.replace("{ 0 }", "{ 1 }");
        // Move all expansion regions without editing any deriving item, and use the
        // installed first patch as the old version (including its expansion reuse).
        let second = format!("// an unrelated edit above the declarations\n{}", first.replace("{ 1 }", "{ 2 }"));
        for (patch, source) in [first, second].into_iter().enumerate() {
            let old = &prog.files[file as usize];
            let generated: Vec<_> = old.ast.items.iter().filter_map(|it| {
                let crate::ast::ItemKind::Impl(row) = it.kind else { return None };
                let trait_ = old.ast.row(row).trait_?;
                Some(old.text(old.ast.segs(trait_.segs).last()?.name))
            }).collect();
            for trait_ in ["Clone", "Debug", "Default", "PartialEq"] {
                assert_eq!(generated.iter().filter(|&&name| name == trait_).count(), 3, "{trait_} must really expand");
            }
            assert!(old.ast.items.iter().any(|it| matches!(it.kind, crate::ast::ItemKind::Struct(..)) && it.lo >= old.src_len),
                "one deriving type must itself live in an expansion region");
            let (text, kept) = compose_partial(&prog, file, source.as_bytes()).unwrap();
            assert_eq!(kept, 0, "patch {patch}: unchanged derives must not retain duplicate authored items");
            let mut version = prog.parse_version(file, path_text, text).unwrap();
            let (pairs, removed, added) = pair_items(old, &version.pf);
            assert!(removed.is_empty(), "patch {patch}: removed {removed:?}");
            assert!(added.is_empty(), "patch {patch}: added {added:?}");
            assert_eq!(pairs.len(), old.ast.items.len());
            // Check the same expanded tree with text-only storage as well. Keep the
            // full tokens for classification/installation (attributes use token indices).
            let tokens = std::mem::take(&mut version.pf.toks);
            let (text_pairs, removed, added) = pair_items(old, &version.pf);
            assert!(removed.is_empty(), "patch {patch}, text view: removed {removed:?}");
            assert!(added.is_empty(), "patch {patch}, text view: added {added:?}");
            assert_eq!(pairs, text_pairs);
            version.pf.toks = tokens;
            let plan = classify(&prog, file, &version.pf, path_text, &Default::default())
                .unwrap_or_else(|why| panic!("patch {patch}: unexpected restart reasons: {why:?}"));
            assert!(plan.removed.is_empty());
            assert!(plan.added.is_empty());
            assert_eq!(plan.changes, vec![Change::Body(revision)]);
            if patch == 0 {
                // Restore the first patch's file through the warm front-image reader before
                // installing it. The second patch must pair against that representation.
                let prep = crate::program::prepare_file(&mut version.pf, &cfg);
                let used = crate::macros::MacroScope::new(prog.macros.clone()).finish();
                let meta = crate::crate_image::file_meta(path_text, "derive_reload", &version.pf, &used, &prep, false);
                let mut meta = crate::crate_image::decode(&meta).unwrap();
                let bytes = crate::front::encode(&version.pf);
                let words = Box::leak(vec![0u64; bytes.len().div_ceil(8)].into_boxed_slice());
                let image = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, bytes.len()) };
                image.copy_from_slice(&bytes);
                version.pf = crate::front::restore(&mut meta, image).unwrap();
                assert!(version.pf.toks.is_mapped());
            }
            file = prog.commit_body_version(version, &plan.pairs).unwrap();
            assert_eq!(prog.def(revision).file, file);
        }
        // A real derive removal removes its impls with their fns (a nested module's and an
        // expansion's too); apply_locked checks the code that called them.
        let old = &prog.files[file as usize];
        let removed_debug = String::from_utf8(old.src[..old.src_len as usize].to_vec()).unwrap().replace("Clone, Debug, Default", "Clone, Default");
        let version = prog.parse_version(file, path_text, removed_debug.into_bytes()).unwrap();
        let plan = classify(&prog, file, &version.pf, path_text, &Default::default()).unwrap_or_else(|why| panic!("removing Debug: {why:?}"));
        let kinds: Vec<(DefKind, &str)> = plan.removed.iter().map(|&d| (prog.def(d).kind, prog.name(d))).collect();
        assert_eq!(kinds.iter().filter(|k| k.0 == DefKind::Impl).count(), 3, "{kinds:?}");
        assert_eq!(kinds.iter().filter(|k| k.1 == "fmt").count(), 3, "{kinds:?}");
        // (and `record!`, whose text changed: its callers are expanded again)
        assert!(plan.added.is_empty() && plan.changes.iter().all(|c| matches!(c, Change::Macro(_))), "{:?} {:?}", plan.added, plan.changes);
        std::fs::remove_file(path).unwrap();
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
                prog.lower_body_declarations(&[owner], &[new]);
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
        let plan = classify(prog, file, &version.pf, path, &Default::default()).unwrap();
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
pub const C_FIELD: usize = 10; struct Ga<const N: usize>; struct UsesField { _f: Ga<C_FIELD> }
pub const C_ALIAS_TY: usize = 11; type UsesAlias = Ga<C_ALIAS_TY>;
pub const C_IMPL: usize = 12; trait Tr {} impl Tr for Ga<C_IMPL> { }
pub const C_BODY_ONLY: usize = 13; impl Ga<0> { fn body_only() -> usize { C_BODY_ONLY } }
pub const C_THROUGH: usize = 14; const fn through() -> usize { C_THROUGH } struct UsesThrough { _a: [u8; through()] }
pub const C_VALUE: u32 = 15; const VALUE_READ: u32 = C_VALUE + 1; const fn value_read() -> u32 { VALUE_READ } fn reads_value() -> u32 { value_read() }
pub const C_NEW_OTHER: usize = 16; struct ViaOther(usize); impl ViaOther { const fn new() -> Self { ViaOther(C_NEW_OTHER) } } struct Unrelated; impl Unrelated { const fn new() -> Self { Unrelated } } struct UsesUnrelated { _a: [u8; { let _ = Unrelated::new(); 1 }] }
pub const C_NEW_VIA: usize = 17; struct ViaIt(usize); impl ViaIt { const fn make() -> Self { ViaIt(C_NEW_VIA) } } struct UsesViaIt { _a: [u8; { let _ = ViaIt::make(); 1 }] }
pub const C_NEW_RENAMED: usize = 18; struct ViaRen(usize); impl ViaRen { const fn build() -> Self { ViaRen(C_NEW_RENAMED) } } use ViaRen as Ren; struct UsesRen { _a: [u8; { let _ = Ren::build(); 1 }] }
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
        for (name, expected) in [("C_FN", false), ("C_CONST", false), ("C_LEN", true), ("C_REPEAT", true), ("C_BODY", false), ("C_STATIC", false),
            ("C_FIELD", true), ("C_ALIAS_TY", true), ("C_IMPL", true), ("C_BODY_ONLY", false), ("C_THROUGH", true), ("C_VALUE", false),
            ("C_NEW_OTHER", false), ("C_NEW_VIA", true), ("C_NEW_RENAMED", true)] {
            assert_eq!(read_in_a_type(&prog, &[named(name)]), expected, "{name}: read by a type");
        }
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

#[cfg(test)]
mod dock_gate_tests {
    use super::DockGate;

    /// A dock app's code changes only with a sync: edits in its tree (a lane's build or checkout
    /// there) are held, and the round after the sync wrote a new revision applies.
    #[test]
    fn only_a_sync_opens_the_dock_gate() {
        let dir = std::env::temp_dir().join(format!("relax-dock-gate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rev = dir.join("FLEET-REVISION.txt");
        std::fs::write(&rev, "makepad aaaa\nsynced 1 (devhub sync)\n").unwrap();
        let mut gate = DockGate::new(rev.clone());
        // files changed, no new revision: a lane's write into the tree
        assert!(!gate.synced(), "a change without a sync is held");
        gate.held(["apps/fab/src/main.rs"].into_iter());
        assert!(!gate.synced());
        // the sync moved the files, then wrote the revision: this round applies, once
        std::fs::write(&rev, "makepad bbbb\nsynced 2 (devhub sync)\n").unwrap();
        assert!(gate.synced(), "the round after a sync applies");
        assert!(!gate.synced(), "and the next one holds again");
        // a revision file that went missing never opens the gate
        std::fs::remove_file(&rev).unwrap();
        assert!(!gate.synced());
        let _ = std::fs::remove_dir(&dir);
    }
}

#[cfg(test)]
mod ledger_tests {
    /// The 11-03 overlay loss: an overlay edit rewrites the edits ledger, and the round that first
    /// sees it move must stat every virtual file, whatever woke it (a kick or an empty drain before
    /// the ledger's own event, a full round, the poll without native events); a round that finds
    /// it standing still stats none of them.
    #[test]
    fn every_round_that_sees_the_edits_ledger_move_looks_at_every_virtual_file() {
        let at = |s| Some((std::time::UNIX_EPOCH + std::time::Duration::from_secs(s), 10));
        // files 0 and 2 are on disk, 1 and 3 virtual; the polled hot set is 0 and 1
        let (virt, hot) = ([false, true, false, true], [0, 1]);
        let none = Vec::<usize>::new();
        let mut ledger = super::Ledger(at(1));
        assert_eq!(ledger.round(at(1), false, Some(vec![]), &hot, &virt), none, "at rest: an empty drain stats nothing");
        assert_eq!(ledger.round(at(1), true, None, &hot, &virt), [0, 2], "at rest: a full round stats the disk files only");
        assert_eq!(ledger.round(at(2), false, Some(vec![]), &hot, &virt), [1, 3], "a round before the ledger's event: every virtual file");
        assert_eq!(ledger.round(at(2), true, None, &hot, &virt), [0, 2], "the event's full round after that: the disk files");
        assert_eq!(ledger.round(at(2), false, None, &hot, &virt), [0], "no native events: the hot set's disk files");
        assert_eq!(ledger.round(at(3), false, None, &hot, &virt), [0, 1, 3], "no native events, the ledger moved: and every virtual file");
        assert_eq!(ledger.round(at(3), false, Some(vec![3]), &hot, &virt), [3], "a virtual file the watch named (an in-app edit leaves the ledger)");
    }
}

#[cfg(test)]
mod patched_tests {
    /// A program that keeps up hears each patch's files and the items whose bodies it changed;
    /// one that fell behind hears everything since it last asked; a patch that changed more than
    /// bodies changed every item for a program that has not seen it.
    #[test]
    fn a_program_hears_the_items_each_patch_changed() {
        let v = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let mut p = super::Patched::new();
        p.land(v(&["a.rs"]).into_iter(), Some(v(&["handle_event"])));
        assert_eq!(p.since(0), "1 0\na.rs\n\thandle_event\n");
        p.land(v(&["a.rs", "b.rs"]).into_iter(), Some(v(&["script_mod"])));
        assert_eq!(p.since(1), "2 0\na.rs\nb.rs\n\tscript_mod\n");
        assert_eq!(p.since(0), "2 0\na.rs\nb.rs\n\thandle_event\n\tscript_mod\n");
        p.land(v(&["b.rs"]).into_iter(), None);
        assert_eq!(p.since(2), "3 1\nb.rs\n");
        assert_eq!(p.since(3), "3 0\n");
    }
}

#[cfg(all(test, unix))]
mod commit_tests {
    use crate::value_tests::vfs_store;

    /// An overlay edit devhub's ship-done committed (its file, then the ledger renamed) is taken
    /// at one look; a file written after the ledger (an app's own write, or the next ship's
    /// before its ledger) still settles first.
    #[test]
    fn an_overlay_edit_is_whole_once_the_ledger_after_it_moved() {
        isolated_test!(an_overlay_edit_is_whole_once_the_ledger_after_it_moved);
        let fixture = vfs_store::Fixture::new(&[("app/a.rs", b"a"), ("app/b.rs", b"b")]);
        let root = "/relax-live-commit/vfs/m";
        crate::vfs::mount(root, fixture.store.clone(), &fixture.commit).unwrap();
        let edits = std::env::temp_dir().join(format!("relax-live-commit-{}", std::process::id()));
        std::fs::create_dir_all(edits.join("app")).unwrap();
        crate::vfs::set_edits(Some(edits.clone()));
        let (a, b) = (format!("{root}/app/a.rs"), format!("{root}/app/b.rs"));
        let put = |rel: &str, text: &str| {
            let tmp = edits.join(format!("{rel}.part"));
            std::fs::write(&tmp, text).unwrap();
            std::fs::rename(&tmp, edits.join(rel)).unwrap();
        };
        // ctimes are the clock's coarse ticks: a write meant to come later waits one out
        let tick = || std::thread::sleep(std::time::Duration::from_millis(20));
        put("app/a.rs", "a2");
        put(".makepad-run-edits", "app/a.rs\tnew\n");
        let whole = |paths: &[&str]| super::committed_by_ledger(paths.iter().copied());
        assert!(whole(&[a.as_str()]), "a shipped edit is whole once its ledger moved");
        tick();
        put("app/b.rs", "b2");
        assert!(!whole(&[a.as_str(), b.as_str()]), "a file written after the ledger settles first");
        tick();
        put(".makepad-run-edits", "app/a.rs\tnew\napp/b.rs\tnew\n");
        assert!(whole(&[a.as_str(), b.as_str()]), "its own ledger commits it");
        assert!(!whole(&[a.as_str(), "/tmp/not-in-a-store.rs"]), "a disk file settles as any save does");
        assert!(!whole(&[]), "nothing changed is nothing committed");
        crate::vfs::set_edits(None);
        let _ = std::fs::remove_dir_all(&edits);
    }

    /// An overlay edit seen between its file's rename and its ledger's goes on when the watch
    /// hears the ledger, not a quiet window later: the settle waits on the live watch.
    #[test]
    fn the_watch_wakes_at_the_ledger_that_commits_an_overlay_edit() {
        isolated_test!(the_watch_wakes_at_the_ledger_that_commits_an_overlay_edit);
        let fixture = vfs_store::Fixture::new(&[("app/a.rs", b"a")]);
        let root = "/relax-live-wake/vfs/m";
        crate::vfs::mount(root, fixture.store.clone(), &fixture.commit).unwrap();
        let edits = std::env::temp_dir().join(format!("relax-live-wake-{}", std::process::id()));
        std::fs::create_dir_all(edits.join("app")).unwrap();
        crate::vfs::set_edits(Some(edits.clone()));
        let a = format!("{root}/app/a.rs");
        let put = |rel: &str, text: &str| {
            let tmp = edits.join(format!("{rel}.part"));
            std::fs::write(&tmp, text).unwrap();
            std::fs::rename(&tmp, edits.join(rel)).unwrap();
        };
        let tick = || std::thread::sleep(std::time::Duration::from_millis(20));
        put(".makepad-run-edits", "");
        let mut watch = super::LiveWatch::new(std::iter::once(a.as_str())).expect("a native watch");
        tick();
        put("app/a.rs", "a2");
        let whole = || super::committed_by_ledger(std::iter::once(a.as_str()));
        assert!(!whole(), "its ledger has not moved yet");
        let start = std::time::Instant::now();
        std::thread::scope(|s| {
            s.spawn(|| {
                tick();
                put(".makepad-run-edits", "app/a.rs\tnew\n");
            });
            // the file's own event may come first: each wakeup looks again
            while !whole() {
                watch.wait(Some(std::time::Duration::from_secs(30)));
                let _ = watch.changed();
            }
        });
        assert!(start.elapsed() < std::time::Duration::from_secs(2), "woken by the ledger's rename, not a timeout ({:?})", start.elapsed());
        crate::vfs::set_edits(None);
        let _ = std::fs::remove_dir_all(&edits);
    }
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    host::reset_statics();
    reset(&LIVE_CRATES_NEXT, std::sync::Mutex::new(Vec::new()));
    reset(&EDITED_CRATES, relax_session::sync::Mutex::new(Vec::new()));
    reset(&INCLUDES_EDITED, relax_session::sync::Mutex::new(Vec::new()));
    reset(&PROGRAM_EDITS, std::sync::atomic::AtomicU64::new(0));
    reset(&RESTART_SUPPORTED, std::sync::atomic::AtomicBool::new(false));
    reset(&LAYOUT_GROUP, std::sync::atomic::AtomicU32::new(0));
    reset(&UNIT_SUSPECT, relax_session::sync::Mutex::new(None));
    reset(&RESTART_REQUESTED, std::sync::atomic::AtomicBool::new(false));
    reset(&PINNED, std::sync::atomic::AtomicBool::new(false));
    reset(&CATCH_UP, std::sync::atomic::AtomicBool::new(false));
    reset(&LANDING_HOLD, relax_session::sync::Mutex::new(std::collections::BTreeMap::new()));
    reset(&LANDING_FORCED, relax_session::sync::Mutex::new(Vec::new()));
    reset(&RETRY_NOW, std::sync::atomic::AtomicBool::new(false));
    reset(&RESTART_REASON, relax_session::sync::Mutex::new(String::new()));
    reset(&RESTART_SAVED_MS, std::sync::atomic::AtomicU64::new(0));
    reset(&DOMAIN_RESTORED, relax_session::sync::Mutex::new(None));
    reset(&DOMAIN_LOADING, std::sync::atomic::AtomicBool::new(false));
    reset(&DOMAIN_INPUTS, relax_session::sync::Mutex::new(None));
    reset(&WATCH_INPUTS, relax_session::sync::Mutex::new(None));
    reset(&WATCH_GONE, std::sync::atomic::AtomicBool::new(false));
    reset(&FIRST, std::sync::atomic::AtomicBool::new(true));
    reset(&PATCH_WAKER, relax_session::sync::Mutex::new(None));
    reset(&PATCHED, relax_session::sync::Mutex::new(Patched::new()));
    reset(&RESTORED_STATE, std::sync::OnceLock::new());
    reset(&UNVERIFIED_PATCHES, std::sync::atomic::AtomicUsize::new(0));
    reset(&SOURCES_WAIT, std::sync::atomic::AtomicBool::new(false));
    reset(&RESTART_CONFIRMED, std::sync::atomic::AtomicBool::new(false));
    reset(&RESTART_PENDING, std::sync::Mutex::new(Vec::new()));
    reset(&LOADED_HEADS, std::sync::Mutex::new(None));
    reset(&RELAUNCH_ASKED, std::sync::atomic::AtomicBool::new(false));
    reset(&STREAMS, relax_session::sync::Mutex::new(Vec::new()));
    reset(&STD, std::sync::OnceLock::new());
}
