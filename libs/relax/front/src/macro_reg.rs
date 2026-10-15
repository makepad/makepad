//! The proc-macro registry of the front end: the crate graph's proc-macro crates and the
//! macros they define (`#[proc_macro_derive]`, `#[proc_macro]`, `#[proc_macro_attribute]`,
//! found by a token scan of their files), the app graph with those crates stripped, and the
//! dispatch of a macro call to the runner. Running a macro is not the front end's: an embedder
//! installs a [`MacroRunner`] (Relax's JIT macro world, pmhost.rs); without one a call reports
//! that the macro needs a host.

use crate::crates::CrateUnit;
use crate::pm::{Span, TokenStream};
use std::cell::{Cell, RefCell};

/// Runs proc macros (an embedder's macro host).
pub trait MacroRunner: Send + Sync {
    /// Whether this exact registered entry has a source-matched native implementation.
    fn native(&self, _entry: &Entry) -> bool { false }
    /// A source-matched token transform with no untracked external inputs.
    fn reusable(&self, _entry: &Entry) -> bool { false }
    /// Opt-in declaration contract for this exact source-matched entry.
    fn declaration_surface(&self, _entry: &Entry) -> bool { false }
    fn declare(&self, _sess: &std::sync::Arc<crate::Session>, _entry: &Entry,
        _attr: Option<&TokenStream>, _input: &TokenStream, _span: Span) -> Result<TokenStream, String> {
        Err("macro has no declaration surface".into())
    }
    /// The crate graph was registered (its source-matched native entries are known from now).
    fn registered(&self, _sess: &std::sync::Arc<crate::Session>) -> Result<(), String> { Ok(()) }
    /// The registered graph's load will expand macros: a runner may start building what it needs.
    fn prewarm(&self, sess: &std::sync::Arc<crate::Session>) -> Result<(), String>;
    /// Runs macro fn `entry` on `input`.
    fn call(&self, sess: &std::sync::Arc<crate::Session>, entry: &Entry, input: &TokenStream, call_span: Span) -> Result<TokenStream, String>;
    /// Runs attribute macro fn `entry` on its arguments `attr` and the item `item`.
    fn call_attr(&self, sess: &std::sync::Arc<crate::Session>, entry: &Entry, attr: &TokenStream, item: &TokenStream, call_span: Span) -> Result<TokenStream, String>;
}

thread_local! {
    static DECLARATIONS: Cell<bool> = const { Cell::new(false) };
}
pub(crate) fn declaring() -> bool { DECLARATIONS.with(Cell::get) }
pub(crate) fn with_declarations<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore { fn drop(&mut self) { DECLARATIONS.with(|v| v.set(self.0)); } }
    let _restore = Restore(DECLARATIONS.with(|v| v.replace(true)));
    f()
}

static RUNNER: std::sync::OnceLock<&'static dyn MacroRunner> = std::sync::OnceLock::new();

/// Installs the macro runner (once per process; later calls are ignored).
pub fn set_runner(r: &'static dyn MacroRunner) {
    let _ = RUNNER.set(r);
}

fn no_runner(e: &Entry) -> String {
    format!("proc macro `{}` needs a macro host to run (this front end runs no code)", e.fname)
}

#[derive(Clone)]
pub struct UnitInfo {
    pub name: String,
    pub src_path: String,
    pub edition: u16,
    pub features: Vec<String>,
    pub deps: Vec<(usize, String)>,
    pub proc_macro: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Derive,
    Bang,
    Attr,
}

#[derive(Clone)]
pub struct Entry {
    pub unit: usize,
    pub fname: String,
    pub kind: Kind,
}

#[derive(Clone, Default)]
pub struct Reg {
    pub units: Vec<UnitInfo>,
    pub std_dir: String,
    /// derive name -> macro fn
    pub derives: Vec<(String, Entry)>,
    /// function-like and attribute macros
    pub macros: Vec<(String, Entry)>,
    /// derive name -> its helper attributes (`attributes(..)` of proc_macro_derive)
    pub helpers: Vec<(String, Vec<String>)>,
}

/// Boot cost of the host (for the measurements in builtins-lane.md).
#[derive(Default, Clone, Copy, Debug)]
pub struct HostStats {
    pub world_ns: u64,
    pub calls: u64,
    pub call_ns: u64,
    pub encode_ns: u64,
    pub out_bytes: u64,
    pub failed: u64,
    /// JIT compile of the macro crates' fns (CPU: typeck + lower + opt + codegen), compiled count
    pub compile_ns: u64,
    pub compiled: u64,
    /// output bytes -> tokens
    pub decode_ns: u64,
    /// tokens -> source text, and lex + parse of the expansions (builtins / parser)
    pub render_ns: u64,
    pub parse_ns: u64,
    pub text_bytes: u64,
    /// derive sites the parser found expanded ahead (in parallel) / expanded on the spot
    pub pre_hits: u64,
    pub pre_misses: u64,
    /// wall time of the parallel pre-expansion
    pub pre_wall_ns: u64,
}

pub fn add_pre(hit: bool) {
    STATS.with(|s| {
        let mut x = s.get();
        if hit {
            x.pre_hits += 1;
        } else {
            x.pre_misses += 1;
        }
        s.set(x);
    });
}

pub fn add_pre_wall(ns: u64) {
    STATS.with(|s| {
        let mut x = s.get();
        x.pre_wall_ns += ns;
        s.set(x);
    });
}

/// Derive expansions that failed, distinct by (file, item position, derive): a failed derive
/// leaves only a `compile_error!` at the item, so what it generates is missing from the program
/// (rustc would reject the program). Reported by `makepad run` / `--check` (run.rs).
static DERIVE_FAILED: relax_session::sync::Mutex<Option<(std::collections::HashSet<(String, u32, String)>, Vec<String>)>> = relax_session::sync::Mutex::new(None);

pub fn derive_failed(path: &str, item_lo: u32, line: u32, col: u32, name: &str, msg: &str) {
    if declaring() { return; }
    let mut g = DERIVE_FAILED.lock().unwrap_or_else(|e| e.into_inner());
    let (seen, msgs) = g.get_or_insert_with(Default::default);
    if seen.insert((path.to_string(), item_lo, name.to_string())) {
        let text = format!("derive({}) failed: {}", name, msg);
        msgs.push(crate::diag::record(format!("{}: {}",path,text), crate::diag::Diagnostic::new("macro",path,line,col,text)));
    }
}

/// Every derive failure so far (one line each).
pub fn derive_failures() -> Vec<String> {
    DERIVE_FAILED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or_else(Vec::new, |x| x.1.clone())
}

/// A warm check replaces failures belonging to reparsed files, including their dedup keys.
pub fn clear_derive_failures(paths: &[String]) {
    if let Some((seen, msgs)) = DERIVE_FAILED.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        seen.retain(|(p, _, _)| !paths.contains(p));
        msgs.retain(|m| !paths.iter().any(|p| m.starts_with(&format!("{}:", p))));
    }
}

/// Adds to the host's stats (render/parse time is measured by builtins and the parser).
pub fn add_stats(render_ns: u64, parse_ns: u64, text_bytes: u64) {
    STATS.with(|s| {
        let mut x = s.get();
        x.render_ns += render_ns;
        x.parse_ns += parse_ns;
        x.text_bytes += text_bytes;
        s.set(x);
    });
}

thread_local! {
    /// this thread builds the world, or parses its crates (the builder's parse pool): a hosted
    /// macro met there is left alone (it would wait for the world being built)
    static IN_WORLD: Cell<bool> = const { Cell::new(false) };
}

/// The registry and the stats are process-wide: files parse on worker threads too.
pub struct Shared<T>(relax_session::sync::Mutex<RefCell<Option<T>>>);

impl<T> Shared<T> {
    pub fn with<R>(&self, f: impl FnOnce(&RefCell<Option<T>>) -> R) -> R {
        f(&self.0.lock().unwrap())
    }
}

pub static REG: Shared<Reg> = Shared(relax_session::sync::Mutex::new(RefCell::new(None)));

pub(crate) struct ZoneTaken(Option<Reg>);

pub(crate) fn zone_take() -> ZoneTaken {
    // The warm path must call register_and_strip again before expanding macros;
    // registry lookups do not load a missing graph themselves.
    let mut reg = REG.0.lock().unwrap_or_else(|e| e.into_inner());
    let taken = ZoneTaken(std::mem::take(reg.get_mut()));
    #[cfg(test)]
    assert!(reg.get_mut().is_none());
    taken
}

pub(crate) fn zone_put_back(taken: ZoneTaken) {
    let mut reg = REG.0.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(reg.get_mut(), taken.0));
}

/// The next compiler generation registers the running program's proc-macro graph again: a
/// copy of the taken registry, made in the caller's (new) generation, and the runner's
/// `registered` (the native macro graph is rebuilt there). A live edit after the swap expands
/// the same derives and macro calls as before it; with the registry left empty its file
/// reparsed without them (every derive's items "removed", every item macro "added": a
/// restart for any edit).
pub(crate) fn zone_reinstall(sess: &std::sync::Arc<crate::Session>, taken: &ZoneTaken) -> Result<(), String> {
    let Some(reg) = taken.0.clone() else { return Ok(()) };
    REG.with(|r| *r.borrow_mut() = Some(reg));
    match RUNNER.get() {
        Some(r) => r.registered(sess),
        None => Ok(()),
    }
}

static STATS_CELL: Shared<HostStats> = Shared(relax_session::sync::Mutex::new(RefCell::new(None)));

/// `STATS.with(|s| ..)` over the shared stats (a copy in a Cell, written back).
pub struct StatsAccess;
pub static STATS: StatsAccess = StatsAccess;

impl StatsAccess {
    pub fn with<R>(&self, f: impl FnOnce(&Cell<HostStats>) -> R) -> R {
        STATS_CELL.with(|c| {
            let mut c = c.borrow_mut();
            let cell = Cell::new(c.unwrap_or_default());
            let r = f(&cell);
            *c = Some(cell.get());
            r
        })
    }
}

/// Does this thread build the world (its parse pool inherits it: preload::start)?
pub fn in_world() -> bool {
    IN_WORLD.with(|w| w.get())
}

pub fn set_in_world(v: bool) {
    IN_WORLD.with(|w| w.set(v));
}

/// Index of function-like proc macro `name` of the registered graph (builtins' HOSTED ids).
pub fn bang_index(name: &str) -> Option<usize> {
    REG.with(|r| r.borrow().as_ref().and_then(|r| r.macros.iter().position(|(n, e)| n == name && e.kind == Kind::Bang)))
}

pub fn bang_name(i: usize) -> Option<String> {
    REG.with(|r| r.borrow().as_ref().and_then(|r| r.macros.get(i).map(|x| x.0.clone())))
}

/// Hosted proc macros re-run on every edited input unless their package is a
/// pure token transformation: makepad's shader derives (makepad_shader_derive +
/// derive_impl + micro_proc_macro read no filesystem, environment, clock or
/// process state; audited 2026-10-09), or the runner certifies the entry as a
/// token transform. Their expansion is a function of the item's tokens and
/// spans, which the reuse key covers.
const PURE_DERIVE_UNITS: &[&str] = &["makepad_shader_derive"];
pub fn reusable_bang(i: usize) -> bool {
    let entry = REG.with(|r| r.borrow().as_ref().and_then(|r| r.macros.get(i).map(|(_, e)| e.clone())));
    entry.as_ref().is_some_and(|e| RUNNER.get().is_some_and(|r| r.reusable(e)))
}
pub fn reusable_derive(name: &str) -> bool {
    let (entry, pure) = REG.with(|r| r.borrow().as_ref().and_then(|r| r.derives.iter().find(|(n, _)| n == name)
        .map(|(_, e)| (Some(e.clone()), PURE_DERIVE_UNITS.contains(&r.units[e.unit].name.as_str())))).unwrap_or((None, false)));
    pure || entry.as_ref().is_some_and(|e| RUNNER.get().is_some_and(|r| r.reusable(e)))
}

/// Is attribute macro `name` (as run_attr finds it) a pure token transform (derive rule)?
pub fn reusable_attr(name: &str) -> bool {
    let (entry, pure) = REG.with(|r| r.borrow().as_ref().and_then(|r| r.macros.iter().find(|(n, e)| n == name && e.kind == Kind::Attr)
        .map(|(_, e)| (Some(e.clone()), PURE_DERIVE_UNITS.contains(&r.units[e.unit].name.as_str())))).unwrap_or((None, false)));
    pure || entry.as_ref().is_some_and(|e| RUNNER.get().is_some_and(|r| r.reusable(e)))
}

/// Runs function-like proc macro `i` (bang_index) on `input`.
pub fn run_bang(sess: &std::sync::Arc<crate::Session>, i: usize, input: TokenStream, call_span: Span) -> Result<TokenStream, String> {
    let e = REG.with(|r| r.borrow().as_ref().and_then(|r| r.macros.get(i).map(|x| x.1.clone()))).ok_or("unknown proc macro")?;
    if in_world() {
        return Err("a proc macro used while the macro world itself is being built".to_string());
    }
    match RUNNER.get() {
        Some(r) if declaring() => r.declare(sess, &e, None, &input, call_span),
        Some(r) => r.call(sess, &e, &input, call_span),
        None => Err(no_runner(&e)),
    }
}
pub fn trace() -> bool {
    crate::preload::relax_trace().map_or(false, |t| t.split(',').any(|x| x == "pmhost"))
}

pub fn stats() -> HostStats {
    STATS.with(|s| s.get())
}

/// The loader's crate graph and std dir (before loading crates).
///
/// `additive`: another program's graph joins the one registered (a domain started into a running
/// unit, domain.rs): units the registry has already (same name, source, edition, features) keep
/// their index, new ones are appended with their dependencies mapped, and only new proc-macro
/// units are scanned for entries. The index of every earlier unit and entry stays valid.
fn register(sess: &std::sync::Arc<crate::Session>, units: &[CrateUnit], std_dir: &str, additive: bool) -> Result<(), String> {
    let mut reg = if additive { REG.with(|r| r.borrow().clone()).unwrap_or_default() } else { Reg::default() };
    reg.std_dir = std_dir.to_string();
    // unit index of the graph -> unit index in the registry
    let mut map = vec![usize::MAX; units.len()];
    let mut fresh = Vec::new();
    for (i, u) in units.iter().enumerate() {
        let same = if additive {
            reg.units.iter().position(|r| r.name == u.name && r.src_path == u.src_path && r.edition == u.edition && r.features == u.features && r.proc_macro == u.proc_macro)
        } else {
            None
        };
        map[i] = same.unwrap_or_else(|| {
            reg.units.push(UnitInfo { name: String::new(), src_path: String::new(), edition: 0, features: Vec::new(), deps: Vec::new(), proc_macro: false });
            fresh.push(i);
            reg.units.len() - 1
        });
    }
    for &i in &fresh {
        let u = &units[i];
        reg.units[map[i]] = UnitInfo { name: u.name.clone(), src_path: u.src_path.clone(), edition: u.edition, features: u.features.clone(), deps: u.deps.iter().map(|(d, n)| (map[*d], n.clone())).collect(), proc_macro: u.proc_macro };
    }
    for &i in &fresh {
        let u = reg.units[map[i]].clone();
        if u.proc_macro {
            scan_entries(&mut reg, map[i], &u.src_path, u.edition);
        }
    }
    if trace() {
        for (n, e) in &reg.derives {
            eprintln!("pmhost: derive {} -> {}::{}", n, reg.units[e.unit].name, e.fname);
        }
        for (n, e) in &reg.macros {
            eprintln!("pmhost: macro {}! ({:?}) -> {}", n, e.kind, reg.units[e.unit].name);
        }
    }
    REG.with(|r| *r.borrow_mut() = Some(reg));
    if let Some(r) = RUNNER.get() {
        // a world built during the program's load takes its core/alloc/std from that load
        r.registered(sess)?;
        // (a program started into a running one has its std loaded already)
        if !additive {
            crate::program::share_std_begin();
        }
    }
    Ok(())
}

/// The boot driver's load will expand macros (it is not a warm restore of the whole program):
/// the runner starts building what it needs now. Without this call the world is built on the
/// first expansion (a warm start's first live edit that adds a derive).
pub fn prewarm(sess: &std::sync::Arc<crate::Session>) -> Result<(), String> {
    if let Some(r) = RUNNER.get() {
        r.prewarm(sess)?;
    }
    Ok(())
}

/// Registers the graph with the host (when std provides `proc_macro`), then makes the app
/// graph see proc-macro crates, and the crates only they depend on, as empty crates: they run in
/// the macro world, never in the app (rustc also links them into the compiler, not the app).
pub fn register_and_strip(sess: &std::sync::Arc<crate::Session>, units: &mut [CrateUnit], std_dir: &str, additive: bool) -> Result<(), String> {
    let empty = format!("{}/proc_macro/empty.rs", std_dir);
    if trace() {
        eprintln!("pmhost: graph of {} units ({} proc-macro), std {}", units.len(), units.iter().filter(|u| u.proc_macro).count(), std_dir);
    }
    register(sess, units, std_dir, additive)?;
    if !crate::vfs::exists(&empty) {
        return Ok(());
    }
    // units reachable from a non-proc-macro unit stay; the rest is host-only
    let n = units.len();
    let mut app = vec![false; n];
    let mut stack: Vec<usize> = (0..n).filter(|&i| !units[i].proc_macro).collect();
    // roots: units no other unit depends on, plus everything they reach without passing a
    // proc-macro crate
    let mut is_dep = vec![false; n];
    for u in units.iter() {
        for (d, _) in &u.deps {
            is_dep[*d] = true;
        }
    }
    stack.retain(|&i| !is_dep[i]);
    while let Some(i) = stack.pop() {
        if app[i] || units[i].proc_macro {
            continue;
        }
        app[i] = true;
        for (d, _) in &units[i].deps {
            stack.push(*d);
        }
    }
    for i in 0..n {
        if !app[i] {
            units[i].src_path = empty.clone();
            units[i].deps.clear();
            units[i].features.clear();
        }
    }
    Ok(())
}

/// Is `name` a derive or proc macro of a registered proc-macro crate?
pub fn has_macro(name: &str) -> bool {
    REG.with(|r| r.borrow().as_ref().map_or(false, |r| r.derives.iter().chain(r.macros.iter()).any(|(n, _)| n == name)))
}

/// Is `name` an attribute macro of a registered proc-macro crate?
pub fn has_attr(name: &str) -> bool {
    REG.with(|r| r.borrow().as_ref().map_or(false, |r| r.macros.iter().any(|(n, e)| n == name && e.kind == Kind::Attr)))
}

/// Outlining may run only source-matched linked implementations. Other hosts
/// can observe arbitrary external state and retain the conservative fallback.
pub fn outline_native(name: &str, kind: Kind) -> bool {
    let entry = REG.with(|r| r.borrow().as_ref().and_then(|r| {
        r.derives.iter().chain(&r.macros).find(|(n, e)| n == name && e.kind == kind).map(|(_, e)| e.clone())
    }));
    entry.as_ref().is_some_and(|e| RUNNER.get().is_some_and(|r| if declaring() { r.declaration_surface(e) } else { r.native(e) }))
}

/// The outline can run only source-matched native entries. A source edit can
/// revoke that certification without changing the macro's exported name; demand
/// records must then be rebuilt using the current expansion policy.
pub fn outline_identity() -> u128 {
    let (entries, helpers) = REG.with(|r| r.borrow().as_ref().map(|r| (
        r.derives.iter().chain(&r.macros).map(|(name, entry)|
            (name.clone(), entry.clone(), r.units[entry.unit].src_path.clone())).collect::<Vec<_>>(),
        r.helpers.clone(),
    )).unwrap_or_default());
    let mut hash = crate::sip::Sip128::new();
    for (name, entry, source) in entries {
        hash.str(&name); hash.str(&source); hash.str(&entry.fname); hash.u8(entry.kind as u8);
        hash.u8(RUNNER.get().is_some_and(|r| r.native(&entry)) as u8);
    }
    for (name, attributes) in helpers {
        hash.str(&name); hash.u32(attributes.len() as u32);
        for attribute in attributes { hash.str(&attribute); }
    }
    hash.finish128()
}

/// Is `name` a helper attribute of one of `derives` (then `#[name]` is inert there, not a macro)?
pub fn is_derive_helper(name: &str, derives: &[String]) -> bool {
    REG.with(|r| r.borrow().as_ref().map_or(false, |r| r.helpers.iter().any(|(d, hs)| derives.contains(d) && hs.iter().any(|h| h == name))))
}

/// Runs attribute macro `name` on its arguments and item (any thread but the world's).
pub fn run_attr(sess: &std::sync::Arc<crate::Session>, name: &str, attr: &TokenStream, item: &TokenStream, call_span: Span) -> Result<TokenStream, String> {
    if in_world() {
        return Err("an attribute macro used while the macro world itself is being built".to_string());
    }
    let e = REG.with(|r| r.borrow().as_ref().and_then(|r| r.macros.iter().find(|(n, e)| n == name && e.kind == Kind::Attr).map(|x| x.1.clone()))).ok_or("unknown attribute macro")?;
    match RUNNER.get() {
        Some(r) if declaring() => r.declare(sess, &e, Some(attr), item, call_span),
        Some(r) => r.call_attr(sess, &e, attr, item, call_span),
        None => Err(no_runner(&e)),
    }
}

/// Is `name` a derive of a registered proc-macro crate?
pub fn has_derive(name: &str) -> bool {
    REG.with(|r| r.borrow().as_ref().map_or(false, |r| r.derives.iter().any(|(n, _)| n == name)))
}

/// `#[proc_macro_derive(Name, ..)]` / `#[proc_macro]` / `#[proc_macro_attribute]` fns of a
/// proc-macro crate (token scan of its files, following `mod x;`).
fn scan_entries(reg: &mut Reg, unit: usize, root: &str, edition: u16) {
    use crate::lexer::T;
    crate::why::edge(root, crate::why::Cause::new(root, 1, "proc-macro-index", "scan_entries"));
    let mut stack = vec![root.to_string()];
    let mut seen = Vec::new();
    while let Some(path) = stack.pop() {
        if seen.contains(&path) {
            continue;
        }
        seen.push(path.clone());
        let src = match crate::vfs::read(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut toks = Vec::new();
        if crate::lexer::lex(&src, edition, &mut toks).is_err() {
            continue;
        }
        let text = |i: usize| -> &[u8] { &src[toks[i].lo as usize..toks[i].hi as usize] };
        let is_root = path == root;
        let dir = crate::program::dir_of(&path).to_string();
        let mut i = 0;
        while i < toks.len() {
            match toks[i].kind {
                T::KwMod if i + 2 < toks.len() && toks[i + 2].kind == T::Semi => {
                    let name = String::from_utf8_lossy(text(i + 1)).into_owned();
                    let base = if is_root || path.ends_with("/mod.rs") { dir.clone() } else { format!("{}/{}", dir, crate::program::stem_of(&path)) };
                    for c in [format!("{}/{}.rs", base, name), format!("{}/{}/mod.rs", base, name)] {
                        if crate::vfs::exists(&c) {
                            if crate::why::enabled() { crate::why::edge(&c, crate::why::Cause::new(&path, crate::lexer::line_col(&src, toks[i].lo).0, "proc-macro-module", &name)); }
                            stack.push(c);
                            break;
                        }
                    }
                }
                T::Pound if i + 2 < toks.len() && toks[i + 1].kind == T::OpenBracket => {
                    let a = text(i + 2);
                    let kind = match a {
                        b"proc_macro_derive" => Some(Kind::Derive),
                        b"proc_macro" => Some(Kind::Bang),
                        b"proc_macro_attribute" => Some(Kind::Attr),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        // derive name: first ident inside `(..)`
                        let mut name = String::new();
                        let mut helpers: Vec<String> = Vec::new();
                        let mut j = i + 3;
                        if kind == Kind::Derive && j < toks.len() && toks[j].kind == T::OpenParen && j + 1 < toks.len() {
                            name = String::from_utf8_lossy(text(j + 1)).into_owned();
                            // `attributes(a, b)`: the derive's helper attributes
                            let mut k = j + 2;
                            while k + 1 < toks.len() && toks[k].kind != T::CloseBracket {
                                if text(k) == b"attributes" && toks[k + 1].kind == T::OpenParen {
                                    let mut m = k + 2;
                                    while m < toks.len() && toks[m].kind != T::CloseParen {
                                        if matches!(toks[m].kind, T::Ident | T::RawIdent) {
                                            helpers.push(String::from_utf8_lossy(text(m)).trim_start_matches("r#").to_string());
                                        }
                                        m += 1;
                                    }
                                    break;
                                }
                                k += 1;
                            }
                        }
                        // skip to the end of the attribute, then to `fn name`
                        let mut d = 0i32;
                        j = i + 1;
                        while j < toks.len() {
                            match toks[j].kind {
                                T::OpenBracket | T::OpenParen | T::OpenBrace => d += 1,
                                T::CloseBracket | T::CloseParen | T::CloseBrace => {
                                    d -= 1;
                                    if d == 0 {
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            j += 1;
                        }
                        while j < toks.len() && toks[j].kind != T::KwFn {
                            j += 1;
                        }
                        if j + 1 < toks.len() {
                            let fname = String::from_utf8_lossy(text(j + 1)).into_owned();
                            let e = Entry { unit, fname: fname.clone(), kind };
                            match kind {
                                Kind::Derive => {
                                    if !helpers.is_empty() {
                                        reg.helpers.push((name.clone(), helpers));
                                    }
                                    reg.derives.push((name, e))
                                }
                                _ => reg.macros.push((fname, e)),
                            }
                        }
                        i = j;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// Runs derive `name` on `item` (any thread). `None`: not a registered derive, or this thread
/// builds the macro world (a derive inside the macro crates themselves is left alone).
pub fn run_derive(sess: &std::sync::Arc<crate::Session>, name: &str, item: &TokenStream, call_span: Span) -> Option<Result<TokenStream, String>> {
    if in_world() {
        return None;
    }
    let e = REG.with(|r| r.borrow().as_ref().and_then(|r| r.derives.iter().find(|(n, _)| n == name).map(|x| x.1.clone())))?;
    let native = RUNNER.get().map_or(false, |r| r.native(&e));
    let _profile = crate::load_profile::span(sess.runtime.profile, if native { "derive-native" } else { "derive-jit" }, name);
    Some(match RUNNER.get() {
        Some(r) if declaring() => r.declare(sess, &e, None, item, call_span),
        Some(r) => r.call(sess, &e, item, call_span),
        None => Err(no_runner(&e)),
    })
}

#[cfg(test)]
mod zone_tests {
    use super::*;

    /// The compiler-generation swap takes the registry with the other compiler statics; the new
    /// generation registers the same proc-macro graph again (zone_reinstall), so a live edit
    /// after the swap still expands the file's derives and macro calls. Without it the registry
    /// stayed empty and every edit of a file with a derive classified as items removed (a restart).
    #[test]
    fn a_swapped_generation_registers_the_proc_macro_graph_again() {
        const CHILD: &str = "RELAX_FRONT_MACRO_REG_ZONE_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let unit = UnitInfo { name: "derive_crate".into(), src_path: "/d/lib.rs".into(), edition: 2021, features: Vec::new(), deps: Vec::new(), proc_macro: true };
            let reg = Reg {
                units: vec![unit],
                std_dir: "/std".into(),
                derives: vec![("Script".into(), Entry { unit: 0, fname: "derive_script".into(), kind: Kind::Derive })],
                macros: vec![("script_mod".into(), Entry { unit: 0, fname: "script_mod".into(), kind: Kind::Bang })],
                helpers: Vec::new(),
            };
            REG.with(|r| *r.borrow_mut() = Some(reg));
            assert!(has_macro("Script") && has_macro("script_mod"));
            let taken = zone_take();
            assert!(!has_macro("Script"), "the swap takes the registry");
            let sess = std::sync::Arc::new(crate::Session::default());
            zone_reinstall(&sess, &taken).unwrap();
            assert!(has_macro("Script") && has_macro("script_mod"), "the new generation has the program's macros");
            assert_eq!(bang_index("script_mod"), Some(0));
            // the taken registry stays intact for a rollback
            zone_put_back(taken);
            assert!(has_macro("Script"));
            return;
        }
        // REG is process-wide: isolate the take/reinstall in a child of this test binary
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "macro_reg::zone_tests::a_swapped_generation_registers_the_proc_macro_graph_again"])
            .env(CHILD, "1")
            .status().unwrap();
        assert!(status.success());
    }
}
