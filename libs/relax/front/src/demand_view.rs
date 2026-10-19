//! The cold planner's symbol space and id-level name resolution (rustc's `Symbol`, applied to the
//! demand plan). A symbol is a `u32`: the id of a name in the frozen `Bindings` table, or, for the
//! few names the table does not hold, an id appended after it. A spelling is a chain of `::`
//! segments, so a symbol is canonical: two spellings are equal exactly when their symbols are.
//! Resolution works on segment ids and writes sorted id sets into reusable buffers; spellings are
//! rendered only where a string is persisted (the crate boundary) or traced.
use super::*;
use std::sync::atomic::{AtomicU16, Ordering::Relaxed};
use std::sync::OnceLock;

pub type Sym = u32;

const PRIMITIVES: [&str; 17] = ["bool", "char", "str", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize"];

/// Index of a primitive type's name in `PRIMITIVES` (`primitive`).
pub fn prim_index(name: &str) -> Option<usize> {
    Some(match name {
        "bool" => 0, "char" => 1, "str" => 2, "f32" => 3, "f64" => 4, "i8" => 5, "i16" => 6, "i32" => 7, "i64" => 8, "i128" => 9,
        "isize" => 10, "u8" => 11, "u16" => 12, "u32" => 13, "u64" => 14, "u128" => 15, "usize" => 16,
        _ => return None,
    })
}

// Facts about a name, computed once from the namespace tables (the spelling's membership), cached
// per base id. The binding bits describe `Names::binding(spelling)`: the targets reached by walking
// the spelling's segments through the module tables.
const F_DONE: u16 = 1;
const F_MACRO: u16 = 2;
const F_TYPE: u16 = 4;
const F_VALUE: u16 = 8;
const F_MODULE: u16 = 16;
const F_TRAIT: u16 = 32;
const F_EMPTY: u16 = 64;
const B_DONE: u16 = 128;
/// the binding has targets and every one is a module
const B_ALL_MODULES: u16 = 256;
/// the binding has targets and each is a module or a type-only name (a named block import shadows it)
const B_SHADOWED: u16 = 512;

pub const FLAG_MACRO: u16 = F_MACRO;
pub const FLAG_TYPE: u16 = F_TYPE;
pub const FLAG_VALUE: u16 = F_VALUE;
pub const FLAG_MODULE: u16 = F_MODULE;
pub const FLAG_EMPTY: u16 = F_EMPTY;
pub const FLAG_ALL_MODULES: u16 = B_ALL_MODULES;
pub const FLAG_SHADOWED: u16 = B_SHADOWED;

#[derive(Default)]
struct Ext {
    segs: HashMap<Box<str>, Sym>,
    pairs: HashMap<(Sym, Sym), Sym>,
    info: Vec<ExtInfo>,
}
enum ExtInfo { Seg(Box<str>), Pair(Sym, Sym) }

/// A worker's copy of the extension entries it has seen: the shared table is consulted (under its
/// lock) only for a name the worker has not met.
#[derive(Default)]
pub struct SymCache {
    segs: HashMap<Box<str>, Sym>,
    pairs: HashMap<(Sym, Sym), Sym>,
}

pub struct View {
    base: u32,
    ext: Mutex<Ext>,
    flags: Vec<AtomicU16>,
    prim: [Sym; 17],
    crate_seg: Vec<Option<Sym>>,
    deps: Vec<OnceLock<HashMap<Box<str>, Vec<Option<Sym>>>>>,
    core_prelude: Option<(Sym, Sym)>,
    std_family: [Option<Sym>; 3],
}

/// Reusable buffers of one worker's resolution.
#[derive(Default)]
pub struct Scratch {
    pub(super) t: Vec<Sym>,
    pub(super) v: Vec<Sym>,
    nt: Vec<Sym>,
    nv: Vec<Sym>,
    base: Vec<Option<Sym>>,
    full: Vec<Sym>,
    pub(super) segs: Vec<Sym>,
    names: Vec<Sym>,
    cands: Vec<Sym>,
}

/// One path segment: its spelling and, when the table holds it, its id.
#[derive(Clone, Copy)]
pub struct PathSeg<'a> { pub text: &'a str, pub id: Option<Sym> }

/// A module spelling's segment ids and the qualified ids of its prefixes.
pub struct ModuleCtx {
    segs: Vec<Option<Sym>>,
    chain: Vec<Option<Sym>>,
}
impl ModuleCtx {
    pub(super) fn qid(&self) -> Option<Sym> { self.chain.last().copied().flatten() }
    /// Whether this module is inside `candidate` (or is it): `within(module, candidate)`.
    pub(super) fn is_within(&self, candidate: Sym) -> bool { self.chain.contains(&Some(candidate)) }
}

pub fn sort_dedup(v: &mut Vec<Sym>) {
    if v.len() > 1 { v.sort_unstable(); v.dedup(); }
}

impl View {
    pub fn new(names: &Names) -> Self {
        let b = &names.bindings;
        let base = b.len();
        let mut v = View {
            base, ext: Mutex::new(Ext::default()),
            flags: (0..base).map(|_| AtomicU16::new(0)).collect(),
            prim: [0; 17], crate_seg: Vec::new(), deps: Vec::new(), core_prelude: None, std_family: [None; 3],
        };
        let mut cache = SymCache::default();
        let tag = v.seg(b, &mut cache, "<primitive>");
        for (i, name) in PRIMITIVES.iter().enumerate() {
            let s = v.seg(b, &mut cache, name);
            v.prim[i] = v.pair(b, &mut cache, tag, s);
        }
        v.crate_seg = names.crates.iter().map(|c| b.segment_id(&c.name)).collect();
        v.deps = names.crates.iter().map(|_| OnceLock::new()).collect();
        v.core_prelude = b.segment_id("core").zip(b.segment_id("prelude"));
        v.std_family = ["std", "core", "alloc"].map(|k| b.segment_id(k));
        v
    }

    pub fn base(&self) -> u32 { self.base }
    pub fn prim(&self, i: usize) -> Sym { self.prim[i] }

    /// The symbol of one segment, interned if the table does not hold it.
    pub fn seg(&self, b: &Bindings, cache: &mut SymCache, text: &str) -> Sym {
        if let Some(id) = b.segment_id(text) { return id; }
        if let Some(&id) = cache.segs.get(text) { return id; }
        let id = {
            let mut ext = self.ext.lock().unwrap();
            match ext.segs.get(text) {
                Some(&id) => id,
                None => {
                    let id = self.base + ext.info.len() as u32;
                    ext.info.push(ExtInfo::Seg(text.into()));
                    ext.segs.insert(text.into(), id);
                    id
                }
            }
        };
        cache.segs.insert(text.into(), id);
        id
    }
    /// The symbol of `module::name`, interned if the table does not hold it.
    pub fn pair(&self, b: &Bindings, cache: &mut SymCache, module: Sym, name: Sym) -> Sym {
        if module < self.base && name < self.base {
            if let Some(id) = b.pair_id(module, name) { return id; }
        }
        if let Some(&id) = cache.pairs.get(&(module, name)) { return id; }
        let id = {
            let mut ext = self.ext.lock().unwrap();
            match ext.pairs.get(&(module, name)) {
                Some(&id) => id,
                None => {
                    let id = self.base + ext.info.len() as u32;
                    ext.info.push(ExtInfo::Pair(module, name));
                    ext.pairs.insert((module, name), id);
                    id
                }
            }
        };
        cache.pairs.insert((module, name), id);
        id
    }
    pub fn pair_find(&self, b: &Bindings, module: Sym, name: Sym) -> Option<Sym> {
        if module < self.base && name < self.base { return b.pair_id(module, name); }
        self.ext.lock().unwrap().pairs.get(&(module, name)).copied()
    }
    /// The symbol of a whole spelling.
    pub fn sym(&self, b: &Bindings, cache: &mut SymCache, text: &str) -> Sym {
        let mut parts = text.split("::");
        let mut acc = self.seg(b, cache, parts.next().unwrap());
        for part in parts { let s = self.seg(b, cache, part); acc = self.pair(b, cache, acc, s); }
        acc
    }
    /// The (module, name) a qualified symbol was made of.
    pub fn parts(&self, b: &Bindings, s: Sym) -> Option<(Sym, Sym)> {
        if s < self.base { return b.pair_of(s); }
        match self.ext.lock().unwrap().info.get((s - self.base) as usize)? { ExtInfo::Pair(m, n) => Some((*m, *n)), ExtInfo::Seg(_) => None }
    }
    pub fn render(&self, b: &Bindings, s: Sym, out: &mut String) {
        if s < self.base { return b.render_into(s, out); }
        let (pair, text) = match self.ext.lock().unwrap().info.get((s - self.base) as usize) {
            Some(ExtInfo::Pair(m, n)) => (Some((*m, *n)), None),
            Some(ExtInfo::Seg(text)) => (None, Some(text.clone())),
            None => (None, None),
        };
        match (pair, text) {
            (Some((m, n)), _) => { self.render(b, m, out); out.push_str("::"); self.render(b, n, out); }
            (_, Some(text)) => out.push_str(&text),
            _ => {}
        }
    }
    pub fn spelling(&self, b: &Bindings, s: Sym) -> String {
        let mut out = String::new();
        self.render(b, s, &mut out);
        out
    }
    /// The segment ids of a symbol's spelling, appended to `out`.
    pub fn segs_of(&self, b: &Bindings, s: Sym, out: &mut Vec<Sym>) {
        match self.parts(b, s) {
            Some((m, n)) => { self.segs_of(b, m, out); out.push(n); }
            None => out.push(s),
        }
    }
    pub(super) fn dep_root(&self, names: &Names, ci: usize, alias: &str) -> Option<&[Option<Sym>]> {
        let map = self.deps[ci].get_or_init(|| {
            let c = &names.crates[ci];
            let mut map = HashMap::<Box<str>, Vec<Option<Sym>>>::default();
            for (alias, root) in c.deps.iter().chain(c.image_dep_names.iter().filter(|_| c.cfg.session.options.rs_lazy)) {
                map.entry(alias.as_str().into()).or_insert_with(|| root.split("::").map(|s| names.bindings.segment_id(s)).collect());
            }
            map
        });
        map.get(alias).map(Vec::as_slice)
    }
}

impl Names {
    /// The symbol view of the namespace as it is now (dropped by every mutation).
    pub(super) fn view(&self) -> &View {
        self.view.get_or_init(|| View::new(self))
    }
    pub(super) fn reset_view(&mut self) { self.view = OnceLock::new(); }

    fn compute_flags(&self, v: &View, s: Sym) -> u16 {
        let mut text = String::new();
        v.render(&self.bindings, s, &mut text);
        let t = text.as_str();
        F_DONE
            | if self.macros.contains(t) { F_MACRO } else { 0 }
            | if self.type_symbols.contains(t) { F_TYPE } else { 0 }
            | if self.value_symbols.contains(t) { F_VALUE } else { 0 }
            | if self.modules.contains(t) { F_MODULE } else { 0 }
            | if self.trait_symbols.contains(t) { F_TRAIT } else { 0 }
            | if self.empty_macros.contains(t) { F_EMPTY } else { 0 }
    }
    /// Whether the spelling of `s` is in the namespace table selected by `bit` (`FLAG_*`).
    pub(super) fn flag(&self, v: &View, s: Sym, bit: u16) -> bool {
        if s < v.base {
            let cell = &v.flags[s as usize];
            let mut f = cell.load(Relaxed);
            if f & F_DONE == 0 {
                let new = self.compute_flags(v, s);
                cell.fetch_or(new, Relaxed);
                f |= new;
            }
            f & bit != 0
        } else {
            self.compute_flags(v, s) & bit != 0
        }
    }
    /// The facts about `binding(spelling(s))`'s targets (`FLAG_ALL_MODULES`, `FLAG_SHADOWED`).
    pub(super) fn binding_flag(&self, v: &View, s: Sym, bit: u16, sc: &mut Scratch) -> bool {
        let compute = |sc: &mut Scratch| {
            let mut segs = std::mem::take(&mut sc.segs);
            segs.clear();
            v.segs_of(&self.bindings, s, &mut segs);
            self.walk(&segs, sc);
            sc.segs = segs;
            let nonempty = !sc.t.is_empty();
            let targets = std::mem::take(&mut sc.t);
            let all_modules = nonempty && targets.iter().all(|&t| self.flag(v, t, F_MODULE));
            let shadowed = nonempty && targets.iter().all(|&t| self.flag(v, t, F_MODULE)
                || self.flag(v, t, F_TYPE) && !self.flag(v, t, F_VALUE) && !self.flag(v, t, F_MACRO));
            sc.t = targets;
            B_DONE | if all_modules { B_ALL_MODULES } else { 0 } | if shadowed { B_SHADOWED } else { 0 }
        };
        if s < v.base {
            let cell = &v.flags[s as usize];
            let mut f = cell.load(Relaxed);
            if f & B_DONE == 0 { let new = compute(sc); cell.fetch_or(new, Relaxed); f |= new; }
            f & bit != 0
        } else {
            compute(sc) & bit != 0
        }
    }

    /// `Bindings::watched` over segment ids: the targets reached by following `segs` through the
    /// module tables (imports included) in `sc.t`, the routes taken in `sc.v`; both sorted.
    pub(super) fn walk(&self, segs: &[Sym], sc: &mut Scratch) {
        sc.t.clear();
        sc.v.clear();
        let Some((&root, rest)) = segs.split_first() else { return; };
        sc.t.push(root);
        for &part in rest {
            sc.nt.clear();
            sc.nv.clear();
            for &module in &sc.t {
                if let Some(&id) = self.bindings.modules.get(&(module as usize)).and_then(|names| names.get(&(part as usize))) {
                    let binding = &self.bindings.values[id];
                    sc.nt.extend_from_slice(binding.targets.ids());
                    sc.nv.extend_from_slice(binding.via.ids());
                }
            }
            sc.nv.extend_from_slice(&sc.v);
            sort_dedup(&mut sc.nt);
            sort_dedup(&mut sc.nv);
            std::mem::swap(&mut sc.t, &mut sc.nt);
            std::mem::swap(&mut sc.v, &mut sc.nv);
            if sc.t.is_empty() { break; }
        }
    }

    pub(super) fn module_ctx(&self, v: &View, module: &str) -> ModuleCtx {
        let segs: Vec<Option<Sym>> = module.split("::").map(|s| self.bindings.segment_id(s)).collect();
        let mut chain = Vec::with_capacity(segs.len());
        let mut acc = None;
        for (i, s) in segs.iter().enumerate() {
            acc = if i == 0 { *s } else { acc.zip(*s).and_then(|(m, n)| v.pair_find(&self.bindings, m, n)) };
            chain.push(acc);
        }
        ModuleCtx { segs, chain }
    }

    /// `absolute` over ids: the base segments of the path's first spelling in `base`, and the index
    /// where the rest of the path begins.
    fn absolute_core(&self, v: &View, ci: usize, m: &ModuleCtx, path: &[PathSeg], base: &mut Vec<Option<Sym>>) -> Option<usize> {
        base.clear();
        let mut at = 0;
        if path.first().is_some_and(|p| p.text.is_empty()) { at += 1; }
        let first = path.get(at)?;
        at += 1;
        let parent = |n: usize| if n > 1 { n - 1 } else { n };
        match first.text {
            "crate" => base.push(v.crate_seg[ci]),
            "self" => {
                let mut n = m.segs.len();
                while path.get(at).is_some_and(|x| x.text == "super") { at += 1; n = parent(n); }
                base.extend_from_slice(&m.segs[..n]);
            }
            "super" => {
                let mut n = parent(m.segs.len());
                while path.get(at).is_some_and(|x| x.text == "super") { at += 1; n = parent(n); }
                base.extend_from_slice(&m.segs[..n]);
            }
            name => {
                // A child module of this module shadows the extern prelude, as in rustc:
                // `pub use core::{EngineCore, ..}` in a module with a child `mod core` names that
                // child, not the core crate.
                let local = m.qid().zip(first.id).and_then(|(q, n)| v.pair_find(&self.bindings, q, n));
                if local.is_some_and(|l| self.flag(v, l, F_MODULE)) {
                    base.extend_from_slice(&m.segs);
                    base.push(first.id);
                }
                // The loader also supplies the implicit core/alloc/std graph.
                // Those names need their real root in Rust file-selection mode;
                // treating std::arch as a child of this module loses its import.
                else if let Some(root) = v.dep_root(self, ci, name) { base.extend_from_slice(root); }
                else if self.crates[ci].name == name { base.push(v.crate_seg[ci]); }
                else {
                    base.extend_from_slice(&m.segs);
                    base.push(first.id);
                }
            }
        }
        Some(at)
    }

    /// `resolve` over ids: the names `path` (as written in `m`) demands, appended to `out`
    /// (unsorted, possibly repeating).
    pub(super) fn resolve_core(&self, v: &View, cache: &mut SymCache, ci: usize, m: &ModuleCtx, path: &[PathSeg], out: &mut Vec<Sym>, sc: &mut Scratch) {
        if path.len() == 1 { if let Some(i) = prim_index(path[0].text) { out.push(v.prim[i]); } }
        let mut found = false;
        let mut base = std::mem::take(&mut sc.base);
        let mut full = std::mem::take(&mut sc.full);
        if let Some(tail) = self.absolute_core(v, ci, m, path, &mut base) {
            full.clear();
            let complete = base.iter().all(|s| s.is_some()) && path[tail..].iter().all(|p| p.id.is_some());
            if complete {
                full.extend(base.iter().map(|s| s.unwrap()));
                full.extend(path[tail..].iter().map(|p| p.id.unwrap()));
                self.walk(&full, sc);
                if !sc.t.is_empty() {
                    found = true;
                    out.push(self.chain_sym(v, cache, &full));
                    out.extend_from_slice(&sc.t);
                    out.extend_from_slice(&sc.v);
                }
            }
        }
        // Rust file selection has no forced core: an unbound first segment
        // falls back to the std prelude Relax puts in every crate (core's
        // `prelude` module, Program::prelude_crate), after local scopes.
        if !found && self.crates[ci].cfg.session.options.rs_lazy
            && path.first().is_some_and(|first| !matches!(first.text, "" | "crate" | "self" | "super")) {
            if let Some((core, prelude)) = v.core_prelude {
                if path.iter().all(|p| p.id.is_some()) {
                    full.clear();
                    full.extend([core, prelude]);
                    full.extend(path.iter().map(|p| p.id.unwrap()));
                    self.walk(&full, sc);
                    if !sc.t.is_empty() {
                        out.push(self.chain_sym(v, cache, &full));
                        out.extend_from_slice(&sc.t);
                        out.extend_from_slice(&sc.v);
                    }
                }
            }
        }
        sc.base = base;
        sc.full = full;
    }
    fn chain_sym(&self, v: &View, cache: &mut SymCache, segs: &[Sym]) -> Sym {
        let mut acc = segs[0];
        for &s in &segs[1..] { acc = v.pair(&self.bindings, cache, acc, s); }
        acc
    }

    /// `resolve_macro` over ids.
    pub(super) fn resolve_macro_core(&self, v: &View, cache: &mut SymCache, ci: usize, m: &ModuleCtx, path: &[PathSeg], out: &mut Vec<Sym>, sc: &mut Scratch) {
        let start = out.len();
        self.resolve_core(v, cache, ci, m, path, out, sc);
        if path.len() == 1 {
            // Textual macros are visible in child modules. Keep shadowed
            // candidates too; the real expansion pass retains source order.
            if let Some(name) = path[0].id {
                for len in (1..=m.segs.len()).rev() {
                    if m.segs[..len].iter().any(|s| s.is_none()) { continue; }
                    let mut full = std::mem::take(&mut sc.full);
                    full.clear();
                    full.extend(m.segs[..len].iter().map(|s| s.unwrap()));
                    full.push(name);
                    self.walk(&full, sc);
                    sc.full = full;
                    if sc.t.iter().any(|&t| self.flag(v, t, F_MACRO)) {
                        out.extend_from_slice(&sc.t);
                        out.extend_from_slice(&sc.v);
                    }
                }
            }
            // macros::MacroScope::std_prelude: the exported macros of std, core, alloc.
            if out.len() == start && self.crates[ci].cfg.session.options.rs_lazy {
                if let Some(name) = path[0].id {
                    for krate in v.std_family.iter().flatten() {
                        self.walk(&[*krate, name], sc);
                        if sc.t.iter().any(|&t| self.flag(v, t, F_MACRO)) {
                            out.extend_from_slice(&sc.t);
                            out.extend_from_slice(&sc.v);
                            break;
                        }
                    }
                }
            }
        }
    }

    /// `empty_macro` over ids. `fresh` reads the empty-rule table itself (it is changing: the
    /// fixed point in `finish_prepare`) instead of the cached flags.
    pub(super) fn empty_macro_core(&self, v: &View, cache: &mut SymCache, ci: usize, m: &ModuleCtx, path: &[PathSeg], fresh: bool, sc: &mut Scratch) -> bool {
        let mut resolved = std::mem::take(&mut sc.cands);
        resolved.clear();
        self.resolve_macro_core(v, cache, ci, m, path, &mut resolved, sc);
        sort_dedup(&mut resolved);
        let mut definitions = 0usize;
        let mut all_empty = true;
        for &name in &resolved {
            if !self.flag(v, name, F_MACRO) { continue; }
            definitions += 1;
            let empty = if fresh {
                let mut text = String::new();
                v.render(&self.bindings, name, &mut text);
                self.empty_macros.contains(text.as_str())
            } else { self.flag(v, name, F_EMPTY) };
            all_empty &= empty;
        }
        sc.cands = resolved;
        definitions != 0 && all_empty
    }

    /// `import_demands` over ids, appended to `out`.
    pub(super) fn import_demands_core(&self, v: &View, cache: &mut SymCache, ci: usize, m: &ModuleCtx, path: &[PathSeg], glob: bool, out: &mut Vec<Sym>, sc: &mut Scratch) {
        let mut resolved = std::mem::take(&mut sc.names);
        let mut end = path.len();
        while end != 0 {
            resolved.clear();
            self.resolve_core(v, cache, ci, m, &path[..end], &mut resolved, sc);
            sort_dedup(&mut resolved);
            if !resolved.is_empty() {
                // Enum variants/associated names demand the nearest indexed
                // owner. An import of a module, and every glob edge, needs only
                // that module's declaration (an inline module lives in its
                // parent's file): demand the declaring scope, never the module's
                // own file. Names later looked up through it demand themselves.
                // A named import of anything but a trait binds a name that
                // demands itself when looked up; the loader defers an import
                // into a skipped module. A trait import is observable without
                // a lookup (method resolution), so it demands its target.
                let parent = |n: Sym| v.parts(&self.bindings, n).map_or(n, |(parent, _)| parent);
                if !glob && !resolved.iter().any(|&name| self.flag(v, name, F_TRAIT)) {
                    out.extend(resolved.iter().filter(|&&name| self.flag(v, name, F_MODULE)).map(|&name| parent(name)));
                } else {
                    out.extend(resolved.iter().map(|&name| if self.flag(v, name, F_MODULE) { parent(name) } else { name }));
                }
                break;
            }
            end -= 1;
        }
        sc.names = resolved;
    }

    /// Path segments of spellings.
    pub(super) fn strs_path<'a, P: Borrow<str>>(&self, path: &'a [P]) -> Vec<PathSeg<'a>> {
        path.iter().map(|p| { let text = p.borrow(); PathSeg { text, id: self.bindings.segment_id(text) } }).collect()
    }
    /// `resolve` with spellings in and out.
    pub(super) fn resolve_strs<P: Borrow<str>>(&self, ci: usize, module: &str, path: &[P]) -> BTreeSet<String> {
        let v = self.view();
        let (mut cache, mut sc, mut out) = (SymCache::default(), Scratch::default(), Vec::new());
        let m = self.module_ctx(v, module);
        self.resolve_core(v, &mut cache, ci, &m, &self.strs_path(path), &mut out, &mut sc);
        out.into_iter().map(|s| v.spelling(&self.bindings, s)).collect()
    }
    pub(super) fn resolve_macro_strs<P: Borrow<str>>(&self, ci: usize, module: &str, path: &[P]) -> BTreeSet<String> {
        let v = self.view();
        let (mut cache, mut sc, mut out) = (SymCache::default(), Scratch::default(), Vec::new());
        let m = self.module_ctx(v, module);
        self.resolve_macro_core(v, &mut cache, ci, &m, &self.strs_path(path), &mut out, &mut sc);
        out.into_iter().map(|s| v.spelling(&self.bindings, s)).collect()
    }
    pub(super) fn empty_macro_strs<P: Borrow<str>>(&self, ci: usize, module: &str, path: &[P], fresh: bool) -> bool {
        let v = self.view();
        let (mut cache, mut sc) = (SymCache::default(), Scratch::default());
        let m = self.module_ctx(v, module);
        self.empty_macro_core(v, &mut cache, ci, &m, &self.strs_path(path), fresh, &mut sc)
    }
}
