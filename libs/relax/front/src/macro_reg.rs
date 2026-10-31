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
    /// a registry/git package (CrateUnit::registry): third-party, sandboxed under a strict
    /// proc-macro policy (pmhost::Policy)
    pub registry: bool,
    pub cfgs: Vec<String>,
    pub env: Vec<(String, String)>,
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
    /// crate name -> the units its unit(s) reach through `deps` (transitively; memoised)
    pub reach: std::collections::HashMap<String, Vec<bool>>,
}

/// The units in the dependency closure of the unit(s) named `krate`, themselves included. A
/// name no unit has (core, alloc, std, proc_macro: never cargo units) reaches nothing.
fn reach<'a>(reg: &'a mut Reg, krate: &str) -> &'a [bool] {
    if !reg.reach.contains_key(krate) {
        let mut seen = vec![false; reg.units.len()];
        let mut stack: Vec<usize> = (0..reg.units.len()).filter(|&i| reg.units[i].name == krate).collect();
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            stack.extend(reg.units[i].deps.iter().map(|d| d.0));
        }
        reg.reach.insert(krate.to_string(), seen);
    }
    &reg.reach[krate]
}

/// Resolves proc-macro `name` of `kind` as crate `krate` sees it:
/// Tier 1: direct proc-macro dependency of `krate`
/// Tier 2: 1-hop library wrapper dependency of `krate` (`krate -> wrapper -> proc_macro`)
/// Tier 3: transitive dependency closure of `krate`
fn resolve_entry(reg: &mut Reg, krate: &str, name: &str, kind: Kind) -> Option<Entry> {
    let hits: Vec<Entry> = match kind {
        Kind::Derive => reg.derives.iter().filter(|(n, _)| n == name).map(|x| x.1.clone()).collect(),
        _ => reg.macros.iter().filter(|(n, e)| n == name && e.kind == kind).map(|x| x.1.clone()).collect(),
    };
    if hits.is_empty() {
        return None;
    }
    let u_idxs: Vec<usize> = if krate.is_empty() {
        Vec::new()
    } else {
        (0..reg.units.len()).filter(|&i| reg.units[i].name == krate).collect()
    };
    if u_idxs.is_empty() {
        return if kind == Kind::Attr { None } else { hits.into_iter().next() };
    }
    if let Some(e) = hits.iter().find(|e| u_idxs.contains(&e.unit) || u_idxs.iter().any(|&u| reg.units[u].deps.iter().any(|&(d, _)| d == e.unit))) {
        return Some(e.clone());
    }
    if let Some(e) = hits.iter().find(|e| u_idxs.iter().any(|&u| reg.units[u].deps.iter().any(|&(d, _)| reg.units[d].deps.iter().any(|&(dd, _)| dd == e.unit)))) {
        return Some(e.clone());
    }
    let rc = reach(reg, krate);
    hits.into_iter().find(|e| rc[e.unit])
}

/// Attribute macro `name` as crate `krate` sees it: an entry of a proc-macro crate in `krate`'s
/// dependency closure (rustc: the macro has to be nameable in the invoking crate). A crate
/// whose closure holds no proc-macro crate, such as std's own crates, has no attribute macros
/// at all: core's inert `#[unstable]` stays an attribute when the graph holds the
/// `instability` crate, which exports an attribute macro of that name.
pub(crate) fn attr_entry_for(krate: &str, name: &str) -> Option<Entry> {
    if in_world() { return None; }
    REG.with(|r| {
        let mut b = r.borrow_mut();
        resolve_entry(b.as_mut()?, krate, name, Kind::Attr)
    })
}

/// An attribute exported by this crate itself, not by one of its dependencies.
pub(crate) fn exported_attr(krate: &str, name: &str) -> Option<Entry> {
    if in_world() { return None; }
    REG.with(|r| {
        let r = r.borrow();
        let r = r.as_ref()?;
        r.macros.iter().find(|(n, e)| n == name && e.kind == Kind::Attr && r.units[e.unit].name == krate).map(|(_, e)| e.clone())
    })
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
    /// this thread works for the macro world's build (the builder, or a parse worker on one of
    /// the world's crates): a macro the boot world lacks must not wait for that build (pmhost)
    static WORLD_BUILDER: Cell<bool> = const { Cell::new(false) };
    /// the invoking crate name during a proc-macro expansion (`run_derive_in`, `run_attr`, `run_bang_in`)
    static CALL_KRATE: RefCell<String> = const { RefCell::new(String::new()) };
}

pub struct CallKrateGuard(String);

impl Drop for CallKrateGuard {
    fn drop(&mut self) {
        CALL_KRATE.with(|c| *c.borrow_mut() = std::mem::take(&mut self.0));
    }
}

pub fn with_call_krate(krate: &str) -> CallKrateGuard {
    let prev = CALL_KRATE.with(|c| std::mem::replace(&mut *c.borrow_mut(), krate.to_string()));
    CallKrateGuard(prev)
}

pub fn current_call_krate() -> String {
    CALL_KRATE.with(|c| c.borrow().clone())
}

pub fn call_krate_env(key: &str) -> Option<String> {
    let krate = current_call_krate();
    if krate.is_empty() {
        return None;
    }
    if let Some(v) = crate::crates::crate_env_registered(&krate, key) {
        return Some(v);
    }
    REG.with(|r| {
        let b = r.borrow();
        let reg = b.as_ref()?;
        let u = reg.units.iter().find(|u| u.name == krate)?;
        if let Some((_, v)) = u.env.iter().find(|(k, _)| k == key) {
            return Some(v.clone());
        }
        match key {
            "CARGO_MANIFEST_DIR" => {
                let p = std::path::Path::new(&u.src_path);
                for dir in p.ancestors().skip(1) {
                    if crate::vfs::exists(dir.join("Cargo.toml")) {
                        return Some(dir.to_string_lossy().into_owned());
                    }
                }
                p.parent().map(|d| d.to_string_lossy().into_owned())
            }
            "CARGO_CRATE_NAME" => Some(crate::crates::base_crate_name(&krate).to_string()),
            "CARGO_PKG_NAME" => Some(crate::crates::base_crate_name(&krate).replace('_', "-")),
            _ => None,
        }
    })
}

/// The registry and the stats are process-wide: files parse on worker threads too.
pub struct Shared<T>(relax_session::sync::Mutex<RefCell<Option<T>>>);

impl<T> Shared<T> {
    pub fn with<R>(&self, f: impl FnOnce(&RefCell<Option<T>>) -> R) -> R {
        f(&self.0.lock().unwrap())
    }
}

pub static REG: Shared<Reg> = Shared(relax_session::sync::Mutex::new(RefCell::new(None)));

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

pub fn world_builder() -> bool {
    WORLD_BUILDER.with(|w| w.get())
}

pub fn set_world_builder(v: bool) {
    WORLD_BUILDER.with(|w| w.set(v));
}

/// Index of function-like proc macro `name` of the registered graph (builtins' HOSTED ids).
pub fn bang_index(name: &str) -> Option<usize> {
    if in_world() {
        return None;
    }
    REG.with(|r| r.borrow().as_ref().and_then(|r| r.macros.iter().position(|(n, e)| n == name && e.kind == Kind::Bang)))
}

/// Index of function-like proc macro `name` as crate `krate` sees it (`resolve_entry`).
pub fn bang_index_for(krate: &str, name: &str) -> Option<usize> {
    if in_world() {
        return None;
    }
    REG.with(|r| {
        let mut b = r.borrow_mut();
        let reg = b.as_mut()?;
        let e = resolve_entry(reg, krate, name, Kind::Bang)?;
        reg.macros.iter().position(|(n, ex)| n == name && ex.unit == e.unit && ex.fname == e.fname && ex.kind == Kind::Bang)
    })
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

/// Is the resolved attribute a pure token transform (derive rule)?
pub fn reusable_attr(entry: &Entry) -> bool {
    let pure = REG.with(|r| r.borrow().as_ref().is_some_and(|r| PURE_DERIVE_UNITS.contains(&r.units[entry.unit].name.as_str())));
    pure || RUNNER.get().is_some_and(|r| r.reusable(entry))
}

/// Runs function-like proc macro `i` (bang_index) on `input` in crate `krate`.
pub fn run_bang_in(sess: &std::sync::Arc<crate::Session>, krate: &str, i: usize, input: TokenStream, call_span: Span) -> Result<TokenStream, String> {
    let e = REG.with(|r| r.borrow().as_ref().and_then(|r| r.macros.get(i).map(|x| x.1.clone()))).ok_or("unknown proc macro")?;
    if in_world() {
        return Err("a proc macro used while the macro world itself is being built".to_string());
    }
    let _krate = with_call_krate(krate);
    match RUNNER.get() {
        Some(r) if declaring() => r.declare(sess, &e, None, &input, call_span),
        Some(r) => r.call(sess, &e, &input, call_span),
        None => Err(no_runner(&e)),
    }
}

/// Runs function-like proc macro `i` (bang_index) on `input`.
pub fn run_bang(sess: &std::sync::Arc<crate::Session>, i: usize, input: TokenStream, call_span: Span) -> Result<TokenStream, String> {
    run_bang_in(sess, "", i, input, call_span)
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
    // the dependency closures are computed again for the new units
    reg.reach.clear();
    // unit index of the graph -> unit index in the registry
    let mut map = vec![usize::MAX; units.len()];
    let mut fresh = Vec::new();
    for (i, u) in units.iter().enumerate() {
        let features = u.host.as_ref().map_or(&u.features, |h| &h.features);
        let same = if additive {
            reg.units.iter().position(|r| r.name == u.name && r.src_path == u.src_path && r.edition == u.edition && &r.features == features && r.proc_macro == u.proc_macro && r.registry == u.registry)
        } else {
            None
        };
        map[i] = same.unwrap_or_else(|| {
            reg.units.push(UnitInfo { name: String::new(), src_path: String::new(), edition: 0, features: Vec::new(), deps: Vec::new(), proc_macro: false, registry: false, cfgs: Vec::new(), env: Vec::new() });
            fresh.push(i);
            reg.units.len() - 1
        });
    }
    for &i in &fresh {
        let u = &units[i];
        let (features, deps) = u.host.as_ref().map_or((&u.features, &u.deps), |h| (&h.features, &h.deps));
        reg.units[map[i]] = UnitInfo { name: u.name.clone(), src_path: u.src_path.clone(), edition: u.edition, features: features.clone(), deps: deps.iter().map(|(d, n)| (map[*d], n.clone())).collect(), proc_macro: u.proc_macro, registry: u.registry, cfgs: Vec::new(), env: Vec::new() };
    }
    for &i in &fresh {
        let u = reg.units[map[i]].clone();
        if u.proc_macro {
            scan_entries(sess, &mut reg, map[i], &u.src_path, u.edition);
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

#[cfg(test)]
#[test]
fn merged_host_units_keep_features_and_dependencies() {
    use crate::resolve::{GraphUnit, Units};
    let root = std::env::temp_dir().join(format!("relax-host-units-{}", std::process::id()));
    std::fs::create_dir_all(root.join("proc_macro")).unwrap();
    std::fs::write(root.join("proc_macro/empty.rs"), "").unwrap();
    let dir = root.display().to_string();
    let unit = |name: &str, kind: &str, host, features: &[&str], deps: &[(usize, &str)]| GraphUnit {
        pkg_id: format!("path+file://{dir}/{name}#0.1.0"),
        name: name.to_string(), kind: vec![kind.to_string()],
        src_path: format!("{dir}/{name}/src/lib.rs"), edition: "2021".to_string(),
        host, mode: "build".to_string(), features: features.iter().map(|s| s.to_string()).collect(),
        deps: deps.iter().map(|(d, n)| (*d, n.to_string())).collect(),
    };
    let units = Units { units: vec![
        unit("app", "bin", false, &[], &[(1, "shared"), (2, "macros")]),
        unit("shared", "lib", false, &["target"], &[(3, "target_helper")]),
        unit("macros", "proc-macro", true, &[], &[(4, "shared")]),
        unit("target_helper", "lib", false, &[], &[]),
        unit("shared", "lib", true, &["full", "visit"], &[(5, "host_helper")]),
        unit("host_helper", "lib", true, &["walk"], &[]),
    ], roots: vec![0], bins: Vec::new() };
    let mut g = crate::unitgraph::load(&units, false).unwrap();
    let sess = &std::sync::Arc::new(crate::Session::default());
    let saved = REG.with(|r| r.borrow_mut().take());
    register_and_strip(sess, &mut g.crates, &dir, false).unwrap();
    let shared = g.crates.iter().find(|u| u.name == "shared").unwrap();
    assert_eq!(shared.features, ["target"]);
    assert_eq!(shared.deps.len(), 1);
    assert_eq!(g.crates[shared.deps[0].0].name, "target_helper");
    let helper = g.crates.iter().find(|u| u.name == "host_helper").unwrap();
    assert_eq!(helper.src_path, format!("{dir}/proc_macro/empty.rs"));
    // Re-registering the original graph matches the host feature set, not the app's.
    let again = crate::unitgraph::load(&units, false).unwrap();
    register(sess, &again.crates, &dir, true).unwrap();
    REG.with(|r| {
        let r = r.borrow();
        let reg = r.as_ref().unwrap();
        assert_eq!(reg.units.len(), 5);
        let shared = reg.units.iter().find(|u| u.name == "shared").unwrap();
        assert_eq!(shared.features, ["full", "visit"]);
        assert_eq!(shared.deps.len(), 1);
        let helper = &reg.units[shared.deps[0].0];
        assert_eq!(helper.name, "host_helper");
        assert_eq!(helper.features, ["walk"]);
        assert_eq!(helper.src_path, format!("{dir}/host_helper/src/lib.rs"));
        let pm = reg.units.iter().find(|u| u.name == "macros").unwrap();
        assert_eq!(reg.units[pm.deps[0].0].features, shared.features);
    });
    REG.with(|r| *r.borrow_mut() = saved);
    let _ = std::fs::remove_dir_all(&root);
}

/// Updates registered units with build-script `cfgs` and `env` outputs (`run.rs` after
/// `run_build_scripts`), and re-scans proc-macro units whose `cfgs` changed.
pub fn set_build_outputs(sess: &std::sync::Arc<crate::Session>, units: &[CrateUnit], cfgs: &[Vec<String>], env: &[Vec<(String, String)>]) {
    REG.with(|r| {
        let mut b = r.borrow_mut();
        let Some(reg) = b.as_mut() else { return };
        let mut rescan = Vec::new();
        for (ci, c) in units.iter().enumerate() {
            if c.bin {
                continue;
            }
            for (ui, u) in reg.units.iter_mut().enumerate() {
                if u.name == c.name {
                    if let Some(cs) = cfgs.get(ci) {
                        if u.cfgs != *cs {
                            u.cfgs = cs.clone();
                            if u.proc_macro {
                                rescan.push((ui, u.src_path.clone(), u.edition));
                            }
                        }
                    }
                    if let Some(ev) = env.get(ci) {
                        u.env = ev.clone();
                    }
                }
            }
        }
        for (ui, src_path, edition) in rescan {
            reg.derives.retain(|(_, e)| e.unit != ui);
            reg.macros.retain(|(_, e)| e.unit != ui);
            scan_entries(sess, reg, ui, &src_path, edition);
        }
    });
}

/// Whether a proc-macro crate of the registered graph depends (directly or not) on a crate named
/// in `names`: the macro world built before those crates' build outputs are known would lack them.
pub fn macro_closure_reaches(names: &[&str]) -> bool {
    macro_closure_names().iter().any(|n| names.contains(&n.as_str()))
}

/// The registered graph's proc-macro crates and everything they depend on (the crates a macro
/// world loads), by name.
pub fn macro_closure_names() -> Vec<String> {
    REG.with(|r| {
        let b = r.borrow();
        let Some(reg) = b.as_ref() else { return Vec::new() };
        let mut seen = vec![false; reg.units.len()];
        let mut stack: Vec<usize> = (0..reg.units.len()).filter(|&i| reg.units[i].proc_macro).collect();
        let mut out = Vec::new();
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) { continue; }
            out.push(reg.units[i].name.clone());
            stack.extend(reg.units[i].deps.iter().map(|(d, _)| *d));
        }
        out
    })
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
        for (d, _) in u.deps.iter().chain(u.host.iter().flat_map(|h| &h.deps)) {
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
            units[i].host = None;
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
    entry.as_ref().is_some_and(outline_native_entry)
}

pub(crate) fn outline_native_entry(entry: &Entry) -> bool {
    RUNNER.get().is_some_and(|r| if declaring() { r.declaration_surface(entry) } else { r.native(entry) })
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

/// Runs the resolved attribute macro, with `krate` as the calling crate, on its arguments
/// and item (any thread but the world's).
pub fn run_attr(sess: &std::sync::Arc<crate::Session>, krate: &str, e: &Entry, attr: &TokenStream, item: &TokenStream, call_span: Span) -> Result<TokenStream, String> {
    if in_world() {
        return Err("an attribute macro used while the macro world itself is being built".to_string());
    }
    let _krate = with_call_krate(krate);
    match RUNNER.get() {
        Some(r) if declaring() => r.declare(sess, e, Some(attr), item, call_span),
        Some(r) => r.call_attr(sess, e, attr, item, call_span),
        None => Err(no_runner(e)),
    }
}

/// Is `name` a derive of a registered proc-macro crate?
pub fn has_derive(name: &str) -> bool {
    REG.with(|r| r.borrow().as_ref().map_or(false, |r| r.derives.iter().any(|(n, _)| n == name)))
}

/// Collects `macro_rules!` definitions from a crate's root file for proc-macro entry discovery.
fn collect_macro_rules_defs(root: &str, krate: &str, edition: u16) -> Vec<crate::macros::MacroDef> {
    use crate::lexer::T;
    let Ok(src) = crate::vfs::read(root) else { return Vec::new() };
    let mut toks = Vec::new();
    if crate::lexer::lex(&src, edition, &mut toks).is_err() {
        return Vec::new();
    }
    let text = |i: usize| -> &[u8] { &src[toks[i].lo as usize..toks[i].hi as usize] };
    let mut defs = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if toks[i].kind == T::Ident && text(i) == b"macro_rules" && i + 3 < toks.len() && toks[i + 1].kind == T::Not {
            let name = String::from_utf8_lossy(text(i + 2)).trim_start_matches("r#").to_string();
            let open = i + 3;
            let mut d = 0i32;
            let mut close = open;
            while close < toks.len() {
                match toks[close].kind {
                    T::OpenParen | T::OpenBracket | T::OpenBrace => d += 1,
                    T::CloseParen | T::CloseBracket | T::CloseBrace => {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                close += 1;
            }
            if close > open {
                let mt = crate::macros::definition_tokens(&toks[open + 1..close.min(toks.len())],
                    |lo, hi| &src[lo as usize..hi as usize]);
                if let Ok(def) = crate::macros::parse_def(&name, krate, edition, &mt) {
                    defs.push(def);
                }
            }
            i = close + 1;
            continue;
        }
        i += 1;
    }
    defs
}

/// `#[proc_macro_derive(Name, ..)]` / `#[proc_macro]` / `#[proc_macro_attribute]` fns of a
/// proc-macro crate (token scan of its files, following `mod x;` and expanding top-level
/// `macro_rules!` invocations that generate proc-macro entry points).
fn scan_entries(sess: &std::sync::Arc<crate::Session>, reg: &mut Reg, unit: usize, root: &str, edition: u16) {
    crate::why::edge(root, crate::why::Cause::new(root, 1, "proc-macro-index", "scan_entries"));
    let u = &reg.units[unit];
    let mut cfg = crate::cfg::CfgSet::machine(sess, &u.features);
    cfg.set.push("relax".to_string());
    for c in &u.cfgs {
        cfg.set.push(c.replace('"', ""));
    }
    let mut local_defs = collect_macro_rules_defs(root, &u.name, edition);
    let deps: Vec<(usize, String)> = u.deps.clone();
    for (dep_idx, _) in deps {
        if let Some(dep_u) = reg.units.get(dep_idx) {
            if !dep_u.proc_macro {
                local_defs.extend(collect_macro_rules_defs(&dep_u.src_path, &dep_u.name, dep_u.edition));
            }
        }
    }
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
        let is_root = path == root;
        scan_tokens_for_entries(sess, reg, unit, &src, &toks, edition, &cfg, &path, is_root, &local_defs, &mut stack, 0);
    }
}

fn scan_tokens_for_entries(
    sess: &std::sync::Arc<crate::Session>,
    reg: &mut Reg,
    unit: usize,
    src: &[u8],
    toks: &[crate::lexer::Tok],
    edition: u16,
    cfg: &crate::cfg::CfgSet,
    path: &str,
    is_root: bool,
    local_defs: &[crate::macros::MacroDef],
    stack: &mut Vec<String>,
    depth: u32,
) {
    use crate::lexer::T;
    let text = |i: usize| -> &[u8] { &src[toks[i].lo as usize..toks[i].hi as usize] };
    let dir = if path.is_empty() { String::new() } else { crate::program::dir_of(path).to_string() };
    let p_src = crate::cfg::Src { src, plain: toks, ext: &[], parts: None };
    let mut brace_depth = 0i32;
    let mut i = 0;
    while i < toks.len() {
        match toks[i].kind {
            T::OpenBrace => brace_depth += 1,
            T::CloseBrace => brace_depth = (brace_depth - 1).max(0),
            T::Ident if text(i) == b"macro_rules" && i + 3 < toks.len() && toks[i + 1].kind == T::Not => {
                // Skip `macro_rules!` definition body so metavariables like `$trait` inside
                // `#[proc_macro_derive($trait)]` are never mistaken for real derive names.
                let mut d = 0i32;
                let mut close = i + 3;
                while close < toks.len() {
                    match toks[close].kind {
                        T::OpenParen | T::OpenBracket | T::OpenBrace => d += 1,
                        T::CloseParen | T::CloseBracket | T::CloseBrace => {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    close += 1;
                }
                i = close;
            }
            T::KwMod if !path.is_empty() && i + 2 < toks.len() && toks[i + 2].kind == T::Semi => {
                let name = String::from_utf8_lossy(text(i + 1)).into_owned();
                let base = if is_root || path.ends_with("/mod.rs") { dir.clone() } else { format!("{}/{}", dir, crate::program::stem_of(path)) };
                for c in [format!("{}/{}.rs", base, name), format!("{}/{}/mod.rs", base, name)] {
                    if crate::vfs::exists(&c) {
                        if crate::why::enabled() { crate::why::edge(&c, crate::why::Cause::new(path, crate::lexer::line_col(src, toks[i].lo).0, "proc-macro-module", &name)); }
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
                    if kind == Kind::Derive && j < toks.len() && toks[j].kind == T::OpenParen && j + 1 < toks.len() && matches!(toks[j + 1].kind, T::Ident | T::RawIdent) {
                        name = String::from_utf8_lossy(text(j + 1)).trim_start_matches("r#").to_string();
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
                    if j + 1 < toks.len() && (kind != Kind::Derive || !name.is_empty()) {
                        let item_attrs = crate::cfg::attrs_before(p_src, j, cfg);
                        if crate::cfg::active(p_src, &item_attrs, cfg) {
                            let fname = String::from_utf8_lossy(text(j + 1)).trim_start_matches("r#").to_string();
                            let e = Entry { unit, fname: fname.clone(), kind };
                            match kind {
                                Kind::Derive => {
                                    if !helpers.is_empty() && !reg.helpers.iter().any(|(n, hs)| n == &name && hs == &helpers) {
                                        reg.helpers.push((name.clone(), helpers));
                                    }
                                    if !reg.derives.iter().any(|(n, ex)| n == &name && ex.unit == unit && ex.fname == e.fname) {
                                        reg.derives.push((name, e));
                                    }
                                }
                                _ => {
                                    if !reg.macros.iter().any(|(n, ex)| n == &fname && ex.unit == unit && ex.kind == kind) {
                                        reg.macros.push((fname, e));
                                    }
                                }
                            }
                        }
                    }
                    i = j;
                }
            }
            T::Ident if brace_depth == 0 && depth < 8 && !local_defs.is_empty() => {
                // Top-level macro call `m!(...)` or `path::m!(...)` in a proc-macro crate
                let start = i;
                let mut k = i;
                while k + 2 < toks.len() && toks[k + 1].kind == T::PathSep && matches!(toks[k + 2].kind, T::Ident | T::RawIdent) {
                    k += 2;
                }
                if k + 2 < toks.len() && toks[k + 1].kind == T::Not && matches!(toks[k + 2].kind, T::OpenParen | T::OpenBracket | T::OpenBrace) {
                    let mname = String::from_utf8_lossy(text(k)).trim_start_matches("r#").to_string();
                    let open = k + 2;
                    let mut d = 0i32;
                    let mut close = open;
                    while close < toks.len() {
                        match toks[close].kind {
                            T::OpenParen | T::OpenBracket | T::OpenBrace => d += 1,
                            T::CloseParen | T::CloseBracket | T::CloseBrace => {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        close += 1;
                    }
                    if close < toks.len() {
                        let call_attrs = crate::cfg::attrs_before(p_src, start, cfg);
                        if crate::cfg::active(p_src, &call_attrs, cfg) {
                            if let Some(def) = local_defs.iter().rev().find(|d| d.name == mname) {
                                let mut p = crate::parser::Parser::new(sess, src, toks.to_vec(), edition);
                                if let Ok((exp_text, exp_toks, _, _, _)) = p.expand_rules(def, open + 1, close) {
                                    scan_tokens_for_entries(sess, reg, unit, exp_text.as_bytes(), &exp_toks, edition, cfg, "", false, local_defs, stack, depth + 1);
                                }
                            }
                        }
                        i = close;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// Runs derive `name` as crate `krate` sees it on `item` (any thread). `None`: not a registered
/// derive, or this thread builds the macro world (a derive inside the macro crates themselves is
/// left alone).
pub fn run_derive_in(sess: &std::sync::Arc<crate::Session>, krate: &str, name: &str, item: &TokenStream, call_span: Span) -> Option<Result<TokenStream, String>> {
    if in_world() {
        return None;
    }
    let e = REG.with(|r| {
        let mut b = r.borrow_mut();
        resolve_entry(b.as_mut()?, krate, name, Kind::Derive)
    })?;
    let native = RUNNER.get().map_or(false, |r| r.native(&e));
    let _profile = crate::load_profile::span(sess.runtime.profile, if native { "derive-native" } else { "derive-jit" }, name);
    let _krate = with_call_krate(krate);
    Some(match RUNNER.get() {
        Some(r) if declaring() => r.declare(sess, &e, None, item, call_span),
        Some(r) => r.call(sess, &e, item, call_span),
        None => Err(no_runner(&e)),
    })
}

/// Runs derive `name` on `item` (any thread).
pub fn run_derive(sess: &std::sync::Arc<crate::Session>, name: &str, item: &TokenStream, call_span: Span) -> Option<Result<TokenStream, String>> {
    run_derive_in(sess, "", name, item, call_span)
}
