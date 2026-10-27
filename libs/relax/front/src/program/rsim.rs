//! A replay of rustc_resolve's import resolution over the checked crates (imports.rs
//! `resolve_imports` / `finalize_imports`, ident.rs `resolve_ident_in_scope_set_inner` and the
//! module lookups), `makepad check` only.
//!
//! Relax resolves a `use` optimistically: an import finds its target as soon as the target exists,
//! whatever else could still define the name. rustc's resolution is a fixed point in which a name
//! that a glob provides is *undetermined* while a single import of the same name may still define
//! it, an import path through such a name is undetermined too, and an import left undetermined when
//! no progress is made is an error ("cannot determine resolution for the import"). The same walk
//! over the scopes (module, extern prelude, std prelude, primitive types) reports `E0659` when two
//! scopes provide different items to an import path. This module re-runs that algorithm on the
//! imports and items Relax collected, and reports only what rustc reports on top of Relax's own
//! import errors; modules of crates that are not checked are answered by Relax's tables (a crate
//! the checked crates import from is resolved completely).
//!
//! Not modelled (nothing in these crates' imports needs them): macro-namespace bindings, macro
//! expansion rounds (every item exists from the start; `expansion` only orders the output of
//! different macros), hygiene, tool modules, derive helpers.
use super::*;
use std::cmp::Ordering;

type DeclId = u32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Det { Determined, Undetermined }
use Det::*;

/// rustc's `ControlFlow<Determinacy, Determinacy>` of a scope lookup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ctl { Break(Det), Continue(Det) }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scope { NonGlobs(u32), Globs(u32), ExternItems, ExternFlags, StdPrelude, Builtin }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ScopeSet { All(usize), Module(usize, u32), ModuleAndExternPrelude(usize, u32), ExternPrelude }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shadowing { Restricted, Unrestricted }

/// rustc's `ModuleOrUniformRoot`; a module is a module index, an enum or trait a container.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MoU { Module(u32), Container(DefId), ExternPrelude, ModuleAndExternPrelude(u32), CurrentScope }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Ps { module: u32, exp: u32 }

#[derive(Clone, Copy, Debug)]
struct Summ { vis: BindVis, npm: u32, is_single: bool }

#[derive(Clone, Copy, Debug)]
struct Fin { imp: Option<Summ>, report_private: bool }

enum PathRes { Module(MoU), NonModule, Failed(bool), Indeterminate }

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pend { Pending, Ready(Option<DeclId>) }

impl Pend {
    fn decl(self) -> Option<DeclId> { if let Pend::Ready(d) = self { d } else { None } }
}

#[derive(Clone)]
struct Decl {
    /// `Some`: a definition; None with no import: the dummy `Res::Err` binding
    def: Option<DefId>,
    import: Option<(u32, DeclId)>,
    ambiguity: Option<DeclId>,
    warn_ambiguity: bool,
    vis: BindVis,
    vis_max: Option<DeclId>,
    vis_min: Option<DeclId>,
    exp: u32,
}

enum IK {
    Single { source: Sym, target: Sym, decls: [Pend; 3] },
    Glob { max_vis: Option<BindVis> },
}

struct SImp {
    pi: usize,
    module: u32,
    vis: BindVis,
    exp: u32,
    kind: IK,
    path: Vec<(SegKind, Sym)>,
    global: bool,
    imported: Option<MoU>,
    edition: u16,
}

#[derive(Default, Clone)]
struct NameRes {
    single_imports: Vec<u32>,
    non_glob: Option<DeclId>,
    glob: Option<DeclId>,
}

impl NameRes {
    fn determined_decl(&self) -> Option<DeclId> {
        if self.non_glob.is_some() { self.non_glob } else if self.glob.is_some() && self.single_imports.is_empty() { self.glob } else { None }
    }
    fn best_decl(&self) -> Option<DeclId> { self.non_glob.or(self.glob) }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Warn { Glob, Vis }

pub enum SimKind { Error, Lint(&'static str) }

/// A macro the replay resolved: a definition, or a std-family built-in (name, std's `panic`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MacRes { Def(DefId), Builtin(Sym, bool) }

/// What a checked module binds in the macro namespace: its own binding (an item or a single import)
/// and its glob binding, with the glob's ambiguity (0 none, 1 an error, 2 the lint ambiguous_glob_imports).
#[derive(Clone, Copy, Default, Debug)]
pub struct MacBinding { pub non_glob: Option<MacRes>, pub glob: Option<MacRes>, pub glob_amb: u8 }

pub struct SimDiag {
    pub file: u32,
    pub pos: u32,
    pub kind: SimKind,
    pub msg: String,
}

struct AmbErr { name: Sym, imp: u32, warning: Option<Warn> }

struct Sim<'a> {
    p: &'a Program,
    own: &'a [bool],
    decls: Vec<Decl>,
    res: crate::fxhash::HashMap<(u32, Sym, usize), NameRes>,
    order: crate::fxhash::HashMap<u32, Vec<(Sym, usize)>>,
    imps: Vec<SImp>,
    /// every glob import of a module (resolved or not), in order
    all_globs: crate::fxhash::HashMap<u32, Vec<u32>>,
    glob_importers: crate::fxhash::HashMap<u32, Vec<u32>>,
    propagating: crate::fxhash::HashSet<u32>,
    busy: crate::fxhash::HashSet<(u32, Sym, usize)>,
    ext_names: crate::fxhash::HashMap<u32, std::rc::Rc<Vec<(Sym, usize)>>>,
    ext_decls: crate::fxhash::HashMap<(u32, Sym, usize), Option<DeclId>>,
    amb: Vec<AmbErr>,
    cannot_determine: Vec<u32>,
    unresolved: Vec<u32>,
    privacy_errors: usize,
    priv_list: Vec<(u32, Sym, DeclId)>,
    hack_applied: bool,
    cur_imp: u32,
    no_prelude: crate::fxhash::HashMap<u32, bool>,
    extern_items: crate::fxhash::HashMap<(u32, Sym), DeclId>,
    dummy: DeclId,
    /// modules whose macro-expanded output is not defined yet
    unexpanded: crate::fxhash::HashSet<u32>,
    indeterminate: Vec<u32>,
    determined: Vec<u32>,
    /// the std-family built-in macros given an identity so far: (name, std's `panic`)
    builtin: Vec<(Sym, bool)>,
}

fn ns_type(ns: usize) -> bool { ns == 0 }

/// The bang macros of the std family's modules as rustc's library defines them (Relax implements them
/// by name, builtins.rs): core's prelude (library/core/src/prelude/v1.rs), std's (std/src/prelude/v1.rs),
/// and the crate roots. A macro's identity is its name -- std's and alloc's re-export core's (or
/// alloc's) macro -- except `panic`: std's `std_panic` and core's `core_panic` are two macros (rust#147319).
const CORE_PRELUDE_MACROS: &[&str] = &[
    "assert", "assert_eq", "assert_ne", "cfg", "column", "compile_error", "concat", "debug_assert", "debug_assert_eq",
    "debug_assert_ne", "file", "format_args", "include", "include_bytes", "include_str", "line", "matches", "module_path",
    "option_env", "stringify", "todo", "try", "unimplemented", "unreachable", "write", "writeln", "env", "panic",
    "cfg_select", "concat_bytes", "const_format_args", "log_syntax", "pattern_type", "trace_macros",
];
const STD_PRELUDE_MACROS: &[&str] = &[
    "assert", "assert_eq", "assert_ne", "cfg", "column", "compile_error", "concat", "debug_assert", "debug_assert_eq",
    "debug_assert_ne", "env", "file", "format_args", "include", "include_bytes", "include_str", "line", "matches",
    "module_path", "option_env", "stringify", "todo", "try", "unimplemented", "unreachable", "write", "writeln",
    "dbg", "eprint", "eprintln", "format", "is_x86_feature_detected", "print", "println", "thread_local", "vec", "panic",
    "cfg_select", "concat_bytes", "const_format_args", "log_syntax", "trace_macros",
];
const ALLOC_ROOT_MACROS: &[&str] = &["format", "vec"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum StdMacros { Core, Std, Alloc }

impl StdMacros {
    fn names(self) -> &'static [&'static str] {
        match self { StdMacros::Core => CORE_PRELUDE_MACROS, StdMacros::Std => STD_PRELUDE_MACROS, StdMacros::Alloc => ALLOC_ROOT_MACROS }
    }
}

/// A synthetic definition id of a std-family built-in macro in the replay (index `k` of `Sim::builtin`).
fn builtin_macro_id(k: usize) -> DefId { DefId(u32::MAX - 1 - k as u32) }

impl Program {
    /// What rustc's import resolution reports for the imports of the checked crates beyond the
    /// errors Relax's own resolution found (see the module comment); the glob ambiguities it
    /// found become the answer of `glob_ambiguity` for the checked modules.
    pub fn run_import_sim(&mut self, own: &[bool]) {
        let prev = record_import_uses();
        set_record_import_uses(false);
        let (out, amb, macros) = {
            let mut sim = Sim::new(self, own);
            sim.init(false);
            sim.resolve_imports();
            sim.unexpanded.clear();
            sim.init(true);
            sim.resolve_imports();
            let out = sim.finalize_imports();
            (out, sim.ambiguity_table(), sim.macro_table())
        };
        self.sim_macros = Some(macros);
        set_record_import_uses(prev);
        self.sim_ambiguity = Some(amb);
        self.sim_diags = Some(out);
        self.glob_amb_cache.clear();
    }
}

/// A finding of `macro_call_ambiguities`: (file, position, the lint's name or None for an error, message).
pub type MacroCallDiag = (u32, u32, Option<&'static str>, String);

impl Program {
    /// rustc_resolve ident.rs `resolve_ident_in_scope_set_inner` for the single-name bang macro calls of
    /// the checked crates (`calls`: the parser's record), over the replay's macro namespace: the scopes
    /// are the call's module and its lexical parents (non-glob, then glob bindings), then the injected
    /// prelude. A first result from a glob that a later scope answers with another macro is ambiguous
    /// (GlobVsOuter, E0659) -- the lint `ambiguous_panic_imports` when that is core's against std's
    /// `panic` from the prelude (rust#147319) --, and a glob binding that is itself ambiguous is E0659 or
    /// the lint `ambiguous_glob_imports`. Not modelled: hygiene, the macro_use prelude, the
    /// expansion-order kinds (MoreExpandedVsOuter, GlobVsExpanded).
    pub fn macro_call_ambiguities(&self, calls: Vec<(String, u32, String)>, own: &[bool]) -> Vec<MacroCallDiag> {
        let mut out = Vec::new();
        let Some(table) = self.sim_macros.as_ref() else { return out };
        if table.is_empty() { return out; }
        // names some checked module binds through a glob: only their calls can be ambiguous
        let globbed: crate::fxhash::HashSet<Sym> = table.iter().filter(|(_, b)| b.glob.is_some()).map(|(&(_, n), _)| n).collect();
        if globbed.is_empty() { return out; }
        let mut seen = crate::fxhash::HashSet::default();
        let mut items_of: crate::fxhash::HashMap<u32, Vec<(u32, u32, u32)>> = Default::default();
        let mut blocks_of: crate::fxhash::HashMap<u32, Vec<(u32, u32, u32)>> = Default::default();
        for (path, pos, name) in calls {
            let Some(name) = self.syms.get(&name) else { continue };
            if !globbed.contains(&name) { continue; }
            let Some(file) = self.file_paths.iter().position(|p| *p == path) else { continue };
            let file = file as u32;
            let krate = self.file_crate.get(file as usize).copied().unwrap_or(u32::MAX);
            if !own.get(krate as usize).copied().unwrap_or(false) || !seen.insert((file, pos)) { continue; }
            let Some(module) = self.call_module(file, pos, &mut items_of, &mut blocks_of) else { continue };
            if self.textual_macro_at(module, file, pos, name) { continue; }
            if let Some(d) = self.macro_call_ambiguity(table, module, krate, name) { out.push((file, pos, d.0, d.1)); }
        }
        out
    }

    /// The module whose scope a call at `pos` of `file` resolves in: the innermost body block that is a
    /// module, else the scope of the innermost item holding it.
    fn call_module(&self, file: u32, pos: u32, items_of: &mut crate::fxhash::HashMap<u32, Vec<(u32, u32, u32)>>, blocks_of: &mut crate::fxhash::HashMap<u32, Vec<(u32, u32, u32)>>) -> Option<u32> {
        let ast = &self.files.get(file as usize)?.ast;
        let blocks = blocks_of.entry(file).or_insert_with(|| {
            self.block_mods.iter().filter(|((f, _), _)| *f == file).map(|(&(_, b), &m)| { let bl = ast.block(BlockId(b)); (bl.lo, bl.hi, m) }).collect()
        });
        if let Some(&(_, _, m)) = blocks.iter().filter(|(lo, hi, _)| *lo <= pos && pos < *hi).min_by_key(|(lo, hi, _)| hi - lo) { return Some(m); }
        let items = items_of.entry(file).or_insert_with(|| {
            // (an inline `mod` item: the calls written in it resolve in that module)
            self.defs.iter().filter(|d| d.file == file && d.item.0 != u32::MAX && !matches!(d.kind, DefKind::Closure | DefKind::AnonConst | DefKind::OpaqueTy | DefKind::Prim(_) | DefKind::Field | DefKind::Variant | DefKind::MacroRules))
                .map(|d| { let it = ast.item(d.item); (it.lo, it.hi, if d.kind == DefKind::Mod { d.sub } else { d.scope }) }).collect()
        });
        if let Some(&(_, _, m)) = items.iter().filter(|(lo, hi, _)| *lo <= pos && pos < *hi).min_by_key(|(lo, hi, _)| hi - lo) { return Some(m); }
        (0..self.mods.len()).find(|&i| { let md = &self.mods[i]; md.file == file && md.lexical_parent.is_none() && self.def(md.def).kind == DefKind::Mod && self.def(md.def).sub as usize == i }).map(|m| m as u32)
    }

    /// A `macro_rules! name` written before `pos` of `file` in the call's module or an enclosing one
    /// (rustc's MacroRules scopes, searched first: a result there is never ambiguous with a module's).
    fn textual_macro_at(&self, module: u32, file: u32, pos: u32, name: Sym) -> bool {
        let mut chain = Vec::new();
        let mut m = Some(module);
        while let Some(x) = m { chain.push(self.mods[x as usize].def); m = self.mods[x as usize].lexical_parent.or(self.mods[x as usize].parent_mod); }
        let Some(f) = self.files.get(file as usize) else { return false };
        self.defs.iter().any(|d| d.kind == DefKind::MacroRules && d.name == name && d.file == file && chain.contains(&d.parent) && f.ast.item(d.item).lo < pos)
    }

    fn macro_call_ambiguity(&self, table: &HashMap<(u32, Sym), MacBinding>, module: u32, krate: u32, name: Sym) -> Option<(Option<&'static str>, String)> {
        #[derive(Clone, Copy, PartialEq)]
        enum S { NonGlobs(u32), Globs(u32), Prelude }
        let n = self.syms.str(name);
        let mut found: Vec<(MacRes, S, u8)> = Vec::new();
        let mut m = Some(module);
        let mut results: Vec<(MacRes, S, u8)> = Vec::new();
        while let Some(x) = m {
            if let Some(b) = table.get(&(x, name)) {
                if let Some(r) = b.non_glob { results.push((r, S::NonGlobs(x), 0)); }
                if let Some(r) = b.glob { results.push((r, S::Globs(x), b.glob_amb)); }
            }
            m = self.mods[x as usize].lexical_parent;
        }
        // the injected prelude: std's, core's under #![no_std], none under #![no_core]
        let root = self.crates[krate as usize].root_mod;
        if let Some(f) = self.files.get(self.mods[root as usize].file as usize) {
            let has = |a: &str| f.ast.root_attrs.iter().any(|x| x.inner && x.toks.hi > x.toks.lo && f.tok_text(x.toks.lo) == a);
            if !has("no_core") {
                let std = !has("no_std");
                let set: &[&str] = if std { STD_PRELUDE_MACROS } else { CORE_PRELUDE_MACROS };
                if set.contains(&n) { results.push((MacRes::Builtin(name, std && n == "panic"), S::Prelude, 0)); }
            }
        }
        for (r, sc, amb) in results {
            let Some(&(inner, inner_sc, inner_amb)) = found.first() else {
                found.push((r, sc, amb));
                // the binding a use finds is a glob's ambiguous one: record_use's error
                if amb == 1 { return Some((None, format!("E0659: `{n}` is ambiguous"))); }
                if amb == 2 { return Some((Some("ambiguous_glob_imports"), format!("`{n}` is ambiguous"))); }
                continue;
            };
            let _ = inner_amb;
            if r != inner && matches!(inner_sc, S::Globs(_)) {
                // issue_149681_hack: a glob binding behind a non-glob binding of the same module
                if let S::Globs(m1) = sc { if found[1..].iter().any(|(_, s, _)| *s == S::NonGlobs(m1)) { found.push((r, sc, amb)); continue; } }
                let pair = |a: MacRes, b: MacRes| matches!((a, b), (MacRes::Builtin(x, true), MacRes::Builtin(y, false)) if x == y);
                let panic_hack = self.crates[krate as usize].edition <= 2024 && n == "panic" && sc == S::Prelude && (pair(r, inner) || pair(inner, r));
                return Some(if panic_hack { (Some("ambiguous_panic_imports"), format!("`{n}` is ambiguous")) } else { (None, format!("E0659: `{n}` is ambiguous")) });
            }
            found.push((r, sc, amb));
        }
        None
    }
}

impl<'a> Sim<'a> {
    fn new(p: &'a Program, own: &'a [bool]) -> Self {
        let mut s = Sim {
            p, own,
            decls: Vec::new(),
            res: Default::default(), order: Default::default(), imps: Vec::new(),
            all_globs: Default::default(), glob_importers: Default::default(), propagating: Default::default(),
            busy: Default::default(), ext_names: Default::default(), ext_decls: Default::default(),
            amb: Vec::new(), cannot_determine: Vec::new(), unresolved: Vec::new(), privacy_errors: 0, priv_list: Vec::new(), hack_applied: false, cur_imp: 0,
            no_prelude: Default::default(), extern_items: Default::default(), dummy: 0, unexpanded: Default::default(),
            indeterminate: Vec::new(), determined: Vec::new(), builtin: Vec::new(),
        };
        s.dummy = s.alloc(Decl { def: None, import: None, ambiguity: None, warn_ambiguity: false, vis: None, vis_max: None, vis_min: None, exp: 0 });
        s
    }

    fn alloc(&mut self, d: Decl) -> DeclId {
        self.decls.push(d);
        self.decls.len() as u32 - 1
    }

    fn local(&self, m: u32) -> bool {
        self.own.get(self.p.mods[m as usize].krate as usize).copied().unwrap_or(false)
    }

    fn edition_of(&self, m: u32) -> u16 { self.p.crates[self.p.mods[m as usize].krate as usize].edition }

    fn root_of(&self, m: u32) -> u32 { self.p.crates[self.p.mods[m as usize].krate as usize].root_mod }

    // ------------------------------------------------------------------ visibility

    fn in_module(&self, to: u32, x: u32) -> bool { self.p.in_module(to, x) }

    fn accessible(&self, v: BindVis, from: u32) -> bool { self.p.vis_visible(v, from) }

    fn vis_cmp(&self, a: BindVis, b: BindVis) -> Option<Ordering> {
        match (a, b) {
            (None, None) => Some(Ordering::Equal),
            (None, Some(_)) => Some(Ordering::Greater),
            (Some(_), None) => Some(Ordering::Less),
            (Some(x), Some(y)) => {
                if x == y { Some(Ordering::Equal) }
                else if self.in_module(y, x) { Some(Ordering::Greater) }
                else if self.in_module(x, y) { Some(Ordering::Less) }
                else { None }
            }
        }
    }

    fn greater_than(&self, a: BindVis, b: BindVis) -> bool { self.vis_cmp(a, b) == Some(Ordering::Greater) }

    fn dvis(&self, d: DeclId) -> BindVis {
        match self.decls[d as usize].vis_max { Some(m) => self.dvis(m), None => self.decls[d as usize].vis }
    }

    fn dmin_vis(&self, d: DeclId) -> BindVis {
        match self.decls[d as usize].vis_min { Some(m) => self.dvis(m), None => self.decls[d as usize].vis }
    }

    // ------------------------------------------------------------------ declarations

    fn dres(&self, d: DeclId) -> Option<DefId> {
        let dd = &self.decls[d as usize];
        match dd.import { Some((_, src)) => self.dres(src), None => dd.def }
    }

    fn is_glob_decl(&self, d: DeclId) -> bool {
        match self.decls[d as usize].import { Some((i, _)) => matches!(self.imps[i as usize].kind, IK::Glob { .. }), None => false }
    }

    fn is_ambiguity_recursive(&self, d: DeclId) -> bool {
        let dd = &self.decls[d as usize];
        dd.ambiguity.is_some() || dd.import.map_or(false, |(_, s)| self.is_ambiguity_recursive(s))
    }

    fn warn_ambiguity_recursive(&self, d: DeclId) -> bool {
        let dd = &self.decls[d as usize];
        dd.warn_ambiguity || dd.import.map_or(false, |(_, s)| self.warn_ambiguity_recursive(s))
    }

    fn is_extern_crate_import(&self, d: DeclId) -> bool {
        // `extern crate` imports are planted with an `ext` Program import
        match self.decls[d as usize].import {
            Some((i, _)) => self.p.imports[self.imps_pi(i)].ext,
            None => false,
        }
    }

    fn imps_pi(&self, i: u32) -> usize { self.imps[i as usize].pi }

    fn ext_decl(&mut self, m: u32, name: Sym, ns: usize) -> Option<DeclId> {
        if let Some(r) = self.ext_decls.get(&(m, name, ns)) { return *r; }
        let r = if ns == 2 {
            self.ext_macro(m, name).map(|d| self.alloc(Decl { def: Some(d), import: None, ambiguity: None, warn_ambiguity: false, vis: None, vis_max: None, vis_min: None, exp: 0 }))
        } else {
            let mut q = GlobQuery { precise: true, ..Default::default() };
            self.p.lookup_in_module_vis(m, name, ns_type(ns), &mut q).map(|(d, v)| {
                self.alloc(Decl { def: Some(d), import: None, ambiguity: None, warn_ambiguity: false, vis: v, vis_max: None, vis_min: None, exp: 0 })
            })
        };
        self.ext_decls.insert((m, name, ns), r);
        r
    }

    /// Which std-family macro set module `m` holds: the roots of core / std / alloc and the modules of
    /// their `prelude` (v1, rust_20NN).
    fn std_macro_module(&self, m: u32) -> Option<StdMacros> {
        let md = &self.p.mods[m as usize];
        if md.lexical_parent.is_some() { return None; }
        let krate = match self.p.syms.str(self.p.crates[md.krate as usize].name) { "core" => StdMacros::Core, "std" => StdMacros::Std, "alloc" => StdMacros::Alloc, _ => return None };
        let root = self.root_of(m);
        if m == root { return Some(krate); }
        let par = md.parent_mod?;
        let pd = self.p.def(self.p.mods[par as usize].def);
        let in_prelude = self.p.mods[par as usize].parent_mod == Some(root) && self.p.syms.str(pd.name) == "prelude";
        let own = self.p.syms.str(self.p.def(md.def).name);
        (krate != StdMacros::Alloc && in_prelude && (own == "v1" || own.starts_with("rust_20"))).then_some(krate)
    }

    /// The macro `name` of the prelude rustc injects into `module`'s crate: `std::prelude::rust_20NN`,
    /// core's under `#![no_std]`, none under `#![no_core]` (rustc_builtin_macros standard_library_imports).
    fn macro_prelude(&mut self, module: u32, name: Sym) -> Option<DeclId> {
        let krate = self.p.mods[module as usize].krate;
        let root = self.p.crates[krate as usize].root_mod;
        let f = self.p.files.get(self.p.mods[root as usize].file as usize)?;
        let has = |n: &str| f.ast.root_attrs.iter().any(|a| a.inner && a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == n);
        if has("no_core") { return None; }
        let set = if has("no_std") { StdMacros::Core } else { StdMacros::Std };
        let n = self.p.syms.str(name);
        if !set.names().contains(&n) { return None; }
        let d = self.builtin_macro(name, set == StdMacros::Std && n == "panic");
        Some(self.alloc(Decl { def: Some(d), import: None, ambiguity: None, warn_ambiguity: false, vis: None, vis_max: None, vis_min: None, exp: 0 }))
    }

    fn builtin_macro(&mut self, name: Sym, std_panic: bool) -> DefId {
        let key = (name, std_panic);
        let k = match self.builtin.iter().position(|x| *x == key) { Some(k) => k, None => { self.builtin.push(key); self.builtin.len() - 1 } };
        builtin_macro_id(k)
    }

    /// The macro `name` of an unchecked module: a `#[macro_export]` macro_rules! of a crate root, or a
    /// built-in of the std family.
    fn ext_macro(&mut self, m: u32, name: Sym) -> Option<DefId> {
        if let Some(&d) = self.p.mods[m as usize].macros.get(&name) { return Some(d); }
        let set = self.std_macro_module(m)?;
        let n = self.p.syms.str(name);
        if !set.names().contains(&n) { return None; }
        Some(self.builtin_macro(name, set == StdMacros::Std && n == "panic"))
    }

    /// rustc's MacroRules scopes at import `i`: a `macro_rules! name` written before it in its file, in its
    /// module or an enclosing one.
    fn textual_macro_rules(&mut self, i: u32, name: Sym) -> Option<DeclId> {
        let pim = &self.p.imports[self.imps[i as usize].pi];
        let (file, pos) = (pim.file, pim.pos);
        let mut chain = Vec::new();
        let mut m = Some(self.imps[i as usize].module);
        while let Some(x) = m { chain.push(self.p.mods[x as usize].def); m = self.p.mods[x as usize].lexical_parent.or(self.p.mods[x as usize].parent_mod); }
        let f = self.p.files.get(file as usize)?;
        let mut best: Option<(u32, DefId)> = None;
        for (k, d) in self.p.defs.iter().enumerate() {
            if d.kind != DefKind::MacroRules || d.name != name || d.file != file || !chain.contains(&d.parent) { continue; }
            let lo = f.ast.item(d.item).lo;
            if lo < pos && best.map_or(true, |(b, _)| lo > b) { best = Some((lo, DefId(k as u32))); }
        }
        let (_, d) = best?;
        Some(self.alloc(Decl { def: Some(d), import: None, ambiguity: None, warn_ambiguity: false, vis: None, vis_max: None, vis_min: None, exp: 0 }))
    }

    fn container_decl(&mut self, c: DefId, name: Sym, ns: usize) -> Option<DeclId> {
        let d = self.p.lookup_in_container(c, name, ns_type(ns))?;
        Some(self.alloc(Decl { def: Some(d), import: None, ambiguity: None, warn_ambiguity: false, vis: None, vis_max: None, vis_min: None, exp: 0 }))
    }

    fn import_decl_vis_ext(&self, decl: DeclId, imp: u32, min: bool) -> BindVis {
        let im = &self.imps[imp as usize];
        let decl_vis = if min { self.dmin_vis(decl) } else { self.dvis(decl) };
        let ord = self.vis_cmp(decl_vis, im.vis);
        let is_single = matches!(im.kind, IK::Single { .. });
        let hack = is_single && im.vis.is_none() && self.is_extern_crate_import(decl);
        let npm = self.p.mods[im.module as usize].normal;
        if ord == Some(Ordering::Less) && self.accessible(decl_vis, npm) && !hack { decl_vis } else { im.vis }
    }

    fn import_decl_vis(&self, decl: DeclId, imp: u32) -> BindVis { self.import_decl_vis_ext(decl, imp, false) }

    fn new_import_decl(&mut self, decl: DeclId, imp: u32) -> DeclId {
        let vis = self.import_decl_vis(decl, imp);
        let im = &self.imps[imp as usize];
        let exp = im.exp;
        let own_vis = im.vis;
        let update = match im.kind { IK::Glob { max_vis } => Some(max_vis), _ => None };
        if let Some(max) = update {
            if vis == own_vis || max.map_or(true, |mv| self.greater_than(vis, mv)) {
                if let IK::Glob { max_vis } = &mut self.imps[imp as usize].kind { *max_vis = Some(vis); }
            }
        }
        self.alloc(Decl { def: None, import: Some((imp, decl)), ambiguity: None, warn_ambiguity: false, vis, vis_max: None, vis_min: None, exp })
    }

    fn remove_same_import(&self, d1: DeclId, d2: DeclId) -> (DeclId, DeclId) {
        if let (Some((i1, n1)), Some((i2, n2))) = (self.decls[d1 as usize].import, self.decls[d2 as usize].import) {
            if i1 == i2 { return self.remove_same_import(n1, n2); }
        }
        (d1, d2)
    }

    fn select_glob_decl(&mut self, old: DeclId, new: DeclId, warn: bool) -> DeclId {
        let (old_deep, deep) = self.remove_same_import(old, new);
        if deep != new {
            let _ = old_deep;
            if self.decls[old as usize].ambiguity.is_some() && self.decls[new as usize].ambiguity.is_none() {
                self.decls[new as usize].ambiguity = self.decls[old as usize].ambiguity;
            }
            if self.is_ambiguity_recursive(new) { self.decls[new as usize].warn_ambiguity = true; }
            new
        } else if self.dres(new) != self.dres(old) {
            self.decls[old as usize].ambiguity = Some(new);
            self.decls[old as usize].warn_ambiguity = warn;
            if warn { old } else { let c = self.decls[old as usize].clone(); self.alloc(c) }
        } else if self.dvis(old) != self.dvis(new) {
            let (old_vis, vis) = (self.dvis(old), self.dvis(new));
            if self.greater_than(vis, old_vis) {
                self.decls[old as usize].vis_max = Some(new);
            } else {
                let old_min = self.dmin_vis(old);
                if old_min != vis && self.greater_than(old_min, vis) { self.decls[old as usize].vis_min = Some(new); }
            }
            old
        } else if self.is_ambiguity_recursive(new) && !self.is_ambiguity_recursive(old) {
            self.decls[old as usize].ambiguity = Some(new);
            self.decls[old as usize].warn_ambiguity = true;
            old
        } else {
            old
        }
    }

    fn resolution_mut(&mut self, key: (u32, Sym, usize)) -> &mut NameRes {
        if !self.res.contains_key(&key) {
            self.order.entry(key.0).or_default().push((key.1, key.2));
            self.res.insert(key, NameRes::default());
        }
        self.res.get_mut(&key).unwrap()
    }

    /// `update_local_resolution`: `f` changes the resolution; a new or changed determined declaration
    /// is defined in the module's glob importers.
    fn update_local_resolution<T>(&mut self, module: u32, name: Sym, ns: usize, warn: bool, f: impl FnOnce(&mut Self, &mut NameRes) -> T) -> T {
        let key = (module, name, ns);
        self.resolution_mut(key);
        let mut r = self.res.remove(&key).unwrap();
        let old_decl = r.determined_decl();
        let old_vis = old_decl.map(|d| self.dvis(d));
        let t = f(self, &mut r);
        let new_decl = r.determined_decl();
        self.res.insert(key, r);
        let binding = match new_decl {
            Some(b) if old_decl != Some(b) || old_vis != Some(self.dvis(b)) => b,
            _ => return t,
        };
        let warn = warn || old_decl.is_some();
        if !self.propagating.insert(module) { return t; }
        let importers = self.glob_importers.get(&module).cloned().unwrap_or_default();
        for imp in importers {
            let scope = self.imps[imp as usize].module;
            if self.accessible(self.dvis(binding), scope) {
                let d = self.new_import_decl(binding, imp);
                let _ = self.try_plant(scope, name, ns, d, warn);
            }
        }
        self.propagating.remove(&module);
        t
    }

    /// `try_plant_decl_into_local_module`; the module is the declaration's parent.
    fn try_plant(&mut self, module: u32, name: Sym, ns: usize, decl: DeclId, warn: bool) -> Result<(), DeclId> {
        let is_glob = self.is_glob_decl(decl);
        let is_err = self.dres(decl).is_none();
        self.update_local_resolution(module, name, ns, warn, |this, r| {
            // error recovery does not override a real declaration
            if is_err {
                if let Some(old) = r.best_decl() {
                    if this.dres(old).is_some() { return Ok(()); }
                }
            }
            if is_glob {
                r.glob = Some(match r.glob {
                    Some(old) => this.select_glob_decl(old, decl, warn && r.non_glob.is_none()),
                    None => decl,
                });
                Ok(())
            } else {
                match r.non_glob {
                    Some(old) => Err(old),
                    None => { r.non_glob = Some(decl); Ok(()) }
                }
            }
        })
    }

    // ------------------------------------------------------------------ initial state

    fn exp_of(&self, def: DefId) -> u32 {
        let d = self.p.def(def);
        if d.item.0 == u32::MAX { return 0; }
        match self.p.files.get(d.file as usize).and_then(|f| f.ast.items.get(d.item.0 as usize).map(|it| (f, it))) {
            Some((f, it)) => {
                let id = f.expansion_region_id(it.lo);
                if id != 0 { return id; }
                // an item with a `#[derive]` is output by the derive's expansion: it exists once
                // that has run
                let derived = f.ast.list(it.attrs).iter().any(|a| a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "derive");
                if derived { 1 } else { 0 }
            }
            None => 0,
        }
    }

    /// Defines the items and imports written in the source (`expanded` false) or produced by macro
    /// expansion (true). rustc expands macros while imports resolve: what an expansion makes is not
    /// there while the written imports resolve, and a module that has such output has "unexpanded
    /// invocations" until then.
    fn init(&mut self, expanded: bool) {
        let p = self.p;
        let nmods = p.mods.len() as u32;
        // items
        for m in 0..nmods {
            if !self.local(m) { continue; }
            let md = &p.mods[m as usize];
            for ns in 0..2 {
                let tbl = if ns == 0 { &md.types } else { &md.values };
                let mut names: Vec<(Sym, DefId)> = tbl.iter().map(|(n, d)| (*n, *d)).collect();
                names.sort_unstable();
                for (name, d) in names {
                    if md.import_pub.contains_key(&(name, ns_type(ns))) { continue; }
                    let vis = p.precise_vis(m, name, ns_type(ns), d);
                    let exp = self.exp_of(d);
                    if (exp != 0) != expanded {
                        if exp != 0 { self.unexpanded.insert(m); }
                        continue;
                    }
                    let decl = self.alloc(Decl { def: Some(d), import: None, ambiguity: None, warn_ambiguity: false, vis, vis_max: None, vis_min: None, exp });
                    let _ = self.try_plant(m, name, ns, decl, false);
                }
            }
            // the macro namespace: a `#[macro_export]` macro_rules! is a binding of its crate root
            let mut names: Vec<(Sym, DefId)> = md.macros.iter().map(|(n, d)| (*n, *d)).collect();
            names.sort_unstable();
            for (name, d) in names {
                let exp = self.exp_of(d);
                if (exp != 0) != expanded {
                    if exp != 0 { self.unexpanded.insert(m); }
                    continue;
                }
                let decl = self.alloc(Decl { def: Some(d), import: None, ambiguity: None, warn_ambiguity: false, vis: None, vis_max: None, vis_min: None, exp });
                let _ = self.try_plant(m, name, 2, decl, false);
            }
        }
        // imports
        for pi in 0..p.imports.len() {
            let im = &p.imports[pi];
            if !self.local(im.module) || im.bound & BOUND_PREFIX_ONLY != 0 { continue; }
            let module = im.module;
            let edition = self.edition_of(module);
            let (vis, exp) = self.import_vis_exp(im);
            if (exp != 0) != expanded {
                if exp != 0 { self.unexpanded.insert(module); }
                continue;
            }
            if im.ext {
                // `extern crate`: planted resolved
                let Some(bind) = im.bind else { continue };
                if p.syms.str(bind) == "_" { continue; }
                let Some(&(kind, name)) = im.segs.last() else { continue };
                let krate = if kind == SegKind::Crate {
                    Some(p.mods[module as usize].krate)
                } else {
                    let cur_krate = p.mods[module as usize].krate;
                    p.crates[cur_krate as usize].deps.iter().find(|x| x.0 == name).map(|x| x.1)
                        .or_else(|| p.syms.str(name).strip_prefix("__relax_crate_").and_then(|cn| p.dollar_crate(cur_krate, cn)))
                };
                let Some(c) = krate else { continue };
                let root = p.mods[p.crates[c as usize].root_mod as usize].def;
                let src = self.alloc(Decl { def: Some(root), import: None, ambiguity: None, warn_ambiguity: false, vis: None, vis_max: None, vis_min: None, exp: 0 });
                let idx = self.imps.len() as u32;
                self.imps.push(SImp { pi, module, vis, exp, kind: IK::Single { source: name, target: bind, decls: [Pend::Ready(None), Pend::Ready(None), Pend::Ready(None)] }, path: Vec::new(), global: false, imported: None, edition });
                let d = self.new_import_decl(src, idx);
                let _ = self.try_plant(module, bind, 0, d, false);
                if let IK::Single { decls, .. } = &mut self.imps[idx as usize].kind { decls[0] = Pend::Ready(Some(d)); }
                if module == p.crates[p.mods[module as usize].krate as usize].root_mod {
                    self.extern_items.entry((p.mods[module as usize].krate, bind)).or_insert(d);
                }
                continue;
            }
            let mut path: Vec<(SegKind, Sym)> = im.segs.clone();
            // 2015 `use` paths are crate-relative: an implicit `::` before the first name
            let global = im.global || (edition < 2018 && (im.segs.first().map_or(im.glob, |s| s.0 == SegKind::Ident)));
            if im.glob {
                let idx = self.imps.len() as u32;
                self.imps.push(SImp { pi, module, vis, exp, kind: IK::Glob { max_vis: None }, path, global, imported: None, edition });
                self.all_globs.entry(module).or_default().push(idx);
                self.indeterminate.push(idx);
                continue;
            }
            let Some(target) = im.bind else { continue };
            let Some(&(lk, last)) = path.last() else { continue };
            let mut type_only = im.bound & BOUND_SELF_ONLY != 0;
            let source;
            if lk != SegKind::Ident && (path.len() == 1 || lk != SegKind::SelfValue) {
                // `use crate as c;` / `use self as s;` / `use super as p;`: a module, resolved at once
                if let Some(def) = p.resolve_mod_path(module, im.global, &im.segs, edition < 2018, true) {
                    if p.def(def).kind == DefKind::Mod {
                        let src = self.alloc(Decl { def: Some(def), import: None, ambiguity: None, warn_ambiguity: false, vis: None, vis_max: None, vis_min: None, exp: 0 });
                        let idx = self.imps.len() as u32;
                        self.imps.push(SImp { pi, module, vis, exp, kind: IK::Single { source: last, target, decls: [Pend::Ready(None), Pend::Ready(None), Pend::Ready(None)] }, path: Vec::new(), global: false, imported: None, edition });
                        let d = self.new_import_decl(src, idx);
                        if p.syms.str(target) != "_" { let _ = self.try_plant(module, target, 0, d, false); }
                        if let IK::Single { decls, .. } = &mut self.imps[idx as usize].kind { decls[0] = Pend::Ready(Some(d)); }
                    }
                }
                continue;
            }
            if lk == SegKind::SelfValue {
                // `use a::{self}` is `use a` of the type namespace
                if path.len() < 2 { continue; }
                path.pop();
                let (k2, s2) = path.pop().unwrap();
                if k2 != SegKind::Ident { continue; }
                source = s2;
                type_only = true;
            } else if lk != SegKind::Ident {
                continue;
            } else {
                path.pop();
                source = last;
            }
            let idx = self.imps.len() as u32;
            let decls = [Pend::Pending, if type_only { Pend::Ready(None) } else { Pend::Pending }, if type_only { Pend::Ready(None) } else { Pend::Pending }];
            self.imps.push(SImp { pi, module, vis, exp, kind: IK::Single { source, target, decls }, path, global, imported: None, edition });
            if p.syms.str(target) != "_" {
                for ns in 0..3 {
                    if type_only && ns != 0 { continue; }
                    let key = (module, target, ns);
                    self.resolution_mut(key).single_imports.push(idx);
                }
            }
            self.indeterminate.push(idx);
        }
    }

    /// The visibility and expansion of the `use` item an import came from.
    fn import_vis_exp(&self, im: &Import) -> (BindVis, u32) {
        let p = self.p;
        let Some(f) = p.files.get(im.file as usize) else { return (Some(p.mods[im.module as usize].normal), 0) };
        let exp = f.expansion_region_id(im.pos);
        let item = f.ast.items.iter().find(|it| it.lo == im.pos && matches!(it.kind, ItemKind::Use { .. } | ItemKind::ExternCrate(_)));
        match item {
            Some(it) => (p.vis_module(im.file, im.module, &it.vis), exp),
            None => (Some(p.mods[im.module as usize].normal), exp),
        }
    }

    // ------------------------------------------------------------------ globs of unchecked modules

    /// Every (name, namespace) an unchecked module provides, through its own bindings and its globs.
    fn ext_names(&mut self, t: u32) -> std::rc::Rc<Vec<(Sym, usize)>> {
        if let Some(r) = self.ext_names.get(&t) { return r.clone(); }
        let mut seen = crate::fxhash::HashSet::default();
        let mut stack = vec![t];
        let mut names: Vec<(Sym, usize)> = Vec::new();
        let mut have = crate::fxhash::HashSet::default();
        while let Some(m) = stack.pop() {
            if !seen.insert(m) { continue; }
            let md = &self.p.mods[m as usize];
            let mut ks: Vec<(Sym, usize)> = md.types.keys().map(|&n| (n, 0)).chain(md.values.keys().map(|&n| (n, 1))).chain(md.macros.keys().map(|&n| (n, 2))).collect();
            if let Some(set) = self.std_macro_module(m) {
                ks.extend(set.names().iter().filter_map(|n| self.p.syms.get(n)).map(|n| (n, 2)));
            }
            ks.sort_unstable();
            for k in ks { if have.insert(k) { names.push(k); } }
            for &(g, _) in &md.globs {
                let gd = self.p.def(g);
                match gd.kind {
                    DefKind::Mod => stack.push(gd.sub),
                    DefKind::Enum => {
                        for &v in &self.p.variants[gd.sub as usize] {
                            let n = self.p.def(v).name;
                            for ns in 0..2 { if have.insert((n, ns)) { names.push((n, ns)); } }
                        }
                    }
                    _ => {}
                }
            }
        }
        let r = std::rc::Rc::new(names);
        self.ext_names.insert(t, r.clone());
        r
    }

    // ------------------------------------------------------------------ scopes

    fn no_implicit_prelude(&mut self, m: u32) -> bool {
        if let Some(&r) = self.no_prelude.get(&m) { return r; }
        let p = self.p;
        let md = &p.mods[m as usize];
        let mut r = false;
        let has = |f: &ParsedFile, attrs: &[Attr]| attrs.iter().any(|a| a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "no_implicit_prelude");
        if let Some(f) = p.files.get(md.file as usize) {
            let def = p.def(md.def);
            if def.kind == DefKind::Mod && def.sub == m {
                if def.item.0 != u32::MAX && def.file == md.file {
                    // the module's own item (inline `mod`), or a file module declared elsewhere
                    if let Some(it) = f.ast.items.get(def.item.0 as usize) { r = has(f, f.ast.list(it.attrs)); }
                }
                if !r { r = has(f, &f.ast.root_attrs.iter().copied().filter(|a| a.inner).collect::<Vec<_>>()) && md.parent_mod.is_none() && md.lexical_parent.is_none(); }
            }
        }
        if !r {
            if let Some(par) = md.lexical_parent.or(md.parent_mod) { r = self.no_implicit_prelude(par); }
        }
        self.no_prelude.insert(m, r);
        r
    }

    fn scope_ns_of(set: ScopeSet) -> usize {
        match set { ScopeSet::All(ns) | ScopeSet::Module(ns, _) | ScopeSet::ModuleAndExternPrelude(ns, _) => ns, ScopeSet::ExternPrelude => 0 }
    }

    fn resolve_ident_in_scope_set(&mut self, name: Sym, set: ScopeSet, ps: Ps, fin: Option<Fin>, ign_decl: Option<DeclId>, ign_imp: Option<u32>) -> Result<DeclId, Det> {
        let ns = Self::scope_ns_of(set);
        let module_only = matches!(set, ScopeSet::Module(..));
        let module_and_ext = matches!(set, ScopeSet::ModuleAndExternPrelude(..));
        let ext_only = matches!(set, ScopeSet::ExternPrelude);
        let module = match set { ScopeSet::Module(_, m) | ScopeSet::ModuleAndExternPrelude(_, m) => m, _ => ps.module };
        let mut scope = if module_only || module_and_ext { Scope::NonGlobs(module) } else if ext_only { Scope::ExternItems } else { Scope::NonGlobs(module) };
        let mut use_prelude = !self.no_implicit_prelude(module);
        let mut innermost: Vec<(DeclId, Scope)> = Vec::new();
        let mut determinacy = Determined;
        let mut cur_module = module;
        loop {
            let visit = match scope {
                Scope::NonGlobs(_) | Scope::Globs(_) => true,
                Scope::ExternItems | Scope::ExternFlags => use_prelude || module_and_ext || ext_only,
                // (the macro namespace looks there under #![no_implicit_prelude] too: built-in macros stay)
                Scope::StdPrelude => use_prelude || ns == 2,
                Scope::Builtin => true,
            };
            if visit {
                let r = match self.resolve_ident_in_scope(name, ns, scope, use_prelude, set, ps, if innermost.is_empty() { fin } else { None }, ign_decl, ign_imp) {
                    Ok(d) => Ok(d),
                    Err(Ctl::Break(det)) if innermost.is_empty() => return Err(det),
                    Err(c) => Err(match c { Ctl::Break(d) | Ctl::Continue(d) => d }),
                };
                match r {
                    Ok(decl) => {
                        let imp = match fin {
                            None => return Ok(decl),
                            Some(f) => f.imp,
                        };
                        self.maybe_push_glob_vs_glob_vis_ambiguity(name, decl, imp);
                        if let Some(&(innermost_decl, _)) = innermost.first() {
                            if self.maybe_push_ambiguity(name, ns, set, ps, decl, scope, &innermost, imp) {
                                return Ok(innermost_decl);
                            }
                        }
                        innermost.push((decl, scope));
                    }
                    Err(Determined) => {}
                    Err(Undetermined) => determinacy = Undetermined,
                }
            }
            scope = match scope {
                Scope::NonGlobs(m) => Scope::Globs(m),
                Scope::Globs(_) if module_only => break,
                Scope::Globs(_) if module_and_ext => if ns == 0 { Scope::ExternItems } else { break },
                Scope::Globs(m) => {
                    use_prelude = !self.no_implicit_prelude(m);
                    cur_module = m;
                    match self.p.mods[m as usize].lexical_parent {
                        Some(par) => Scope::NonGlobs(par),
                        None => if ns == 0 { Scope::ExternItems } else { Scope::StdPrelude },
                    }
                }
                Scope::ExternItems => Scope::ExternFlags,
                Scope::ExternFlags if module_and_ext || ext_only => break,
                Scope::ExternFlags => Scope::StdPrelude,
                Scope::StdPrelude => if ns == 0 { Scope::Builtin } else { break },
                Scope::Builtin => break,
            };
        }
        let _ = cur_module;
        match innermost.first() {
            Some(&(d, _)) => Ok(d),
            None => Err(determinacy),
        }
    }

    fn resolve_ident_in_scope(&mut self, name: Sym, ns: usize, scope: Scope, use_prelude: bool, set: ScopeSet, ps: Ps, fin: Option<Fin>, ign_decl: Option<DeclId>, ign_imp: Option<u32>) -> Result<DeclId, Ctl> {
        let restricted = if matches!(set, ScopeSet::Module(..)) { Shadowing::Unrestricted } else { Shadowing::Restricted };
        match scope {
            Scope::NonGlobs(m) => {
                let aps = if matches!(set, ScopeSet::Module(..) | ScopeSet::ModuleAndExternPrelude(..)) { ps } else { Ps { module: m, exp: ps.exp } };
                if !self.local(m) {
                    return match self.ext_decl(m, name, ns) {
                        Some(d) if Some(d) != ign_decl => if self.accessible(self.dvis(d), aps.module) { Ok(d) } else { Err(Ctl::Break(Determined)) },
                        _ => Err(Ctl::Continue(Determined)),
                    };
                }
                self.lookup_non_globs(m, name, ns, aps, restricted, fin, ign_decl, ign_imp)
            }
            Scope::Globs(m) => {
                if !self.local(m) { return Err(Ctl::Continue(Determined)); }
                let aps = if matches!(set, ScopeSet::Module(..) | ScopeSet::ModuleAndExternPrelude(..)) { ps } else { Ps { module: m, exp: ps.exp } };
                self.lookup_globs(m, name, ns, aps, restricted, fin, ign_decl, ign_imp)
            }
            Scope::ExternItems => {
                let krate = self.p.mods[ps.module as usize].krate;
                match self.extern_items.get(&(krate, name)) {
                    Some(&d) => Ok(d),
                    None => Err(Ctl::Continue(Determined)),
                }
            }
            Scope::ExternFlags => {
                let krate = self.p.mods[ps.module as usize].krate;
                let found = self.p.crates[krate as usize].deps.iter().find(|x| x.0 == name).map(|x| x.1)
                    .or_else(|| self.p.syms.str(name).strip_prefix("__relax_crate_").and_then(|cn| self.p.dollar_crate(krate, cn)));
                match found {
                    Some(c) => {
                        let root = self.p.mods[self.p.crates[c as usize].root_mod as usize].def;
                        let d = self.alloc(Decl { def: Some(root), import: None, ambiguity: None, warn_ambiguity: false, vis: None, vis_max: None, vis_min: None, exp: 0 });
                        Ok(d)
                    }
                    None => Err(Ctl::Continue(Determined)),
                }
            }
            Scope::StdPrelude if ns == 2 => {
                let _ = use_prelude;
                match self.macro_prelude(ps.module, name) {
                    Some(d) if Some(d) != ign_decl => Ok(d),
                    _ => Err(Ctl::Continue(Determined)),
                }
            }
            Scope::StdPrelude => {
                let _ = use_prelude;
                if let Some(pm) = self.p.edition_prelude(ps.module) {
                    if let Some(d) = self.ext_decl(pm, name, ns) {
                        if Some(d) != ign_decl { return Ok(d); }
                    }
                }
                Err(Ctl::Continue(Determined))
            }
            Scope::Builtin => {
                for i in 0..PRIMS.len() {
                    if self.p.syms.get(PRIMS[i].0) == Some(name) {
                        let d = self.alloc(Decl { def: Some(self.p.prim_defs[i]), import: None, ambiguity: None, warn_ambiguity: false, vis: None, vis_max: None, vis_min: None, exp: 0 });
                        return Ok(d);
                    }
                }
                Err(Ctl::Continue(Determined))
            }
        }
    }

    fn lookup_non_globs(&mut self, m: u32, name: Sym, ns: usize, ps: Ps, shadowing: Shadowing, fin: Option<Fin>, ign_decl: Option<DeclId>, ign_imp: Option<u32>) -> Result<DeclId, Ctl> {
        let key = (m, name, ns);
        if self.busy.contains(&key) { return Err(Ctl::Continue(Determined)); }
        let r = self.res.get(&key).cloned().unwrap_or_default();
        let binding = r.non_glob.filter(|b| Some(*b) != ign_decl);
        if let Some(f) = fin {
            return self.finalize_module_binding(name, binding, ps, f, shadowing);
        }
        if let Some(b) = binding {
            return if self.accessible(self.dvis(b), ps.module) { Ok(b) } else { Err(Ctl::Break(Determined)) };
        }
        self.busy.insert(key);
        let blocked = self.single_import_can_define_name(&r, None, ns, ign_imp, ign_decl, ps);
        self.busy.remove(&key);
        if blocked { return Err(Ctl::Break(Undetermined)); }
        // an unexpanded macro may still define the name
        if self.unexpanded.contains(&m) { return Err(Ctl::Continue(Undetermined)); }
        Err(Ctl::Continue(Determined))
    }

    fn lookup_globs(&mut self, m: u32, name: Sym, ns: usize, ps: Ps, shadowing: Shadowing, fin: Option<Fin>, ign_decl: Option<DeclId>, ign_imp: Option<u32>) -> Result<DeclId, Ctl> {
        let key = (m, name, ns);
        if self.busy.contains(&key) { return Err(Ctl::Continue(Determined)); }
        let r = self.res.get(&key).cloned().unwrap_or_default();
        let binding = r.glob.filter(|b| Some(*b) != ign_decl);
        if let Some(f) = fin {
            return self.finalize_module_binding(name, binding, ps, f, shadowing);
        }
        self.busy.insert(key);
        let blocked = self.single_import_can_define_name(&r, binding, ns, ign_imp, ign_decl, ps);
        if blocked {
            self.busy.remove(&key);
            return Err(Ctl::Break(Undetermined));
        }
        if let Some(b) = binding {
            self.busy.remove(&key);
            return if self.decl_determined(b) || shadowing == Shadowing::Restricted {
                if self.accessible(self.dvis(b), ps.module) { Ok(b) } else { Err(Ctl::Break(Determined)) }
            } else {
                Err(Ctl::Break(Undetermined))
            };
        }
        if self.unexpanded.contains(&m) {
            self.busy.remove(&key);
            return Err(Ctl::Continue(Undetermined));
        }
        let globs = self.all_globs.get(&m).cloned().unwrap_or_default();
        for g in globs {
            if ign_imp == Some(g) { continue; }
            let gv = self.imps[g as usize].vis;
            if !self.accessible(gv, ps.module) { continue; }
            let target = match self.imps[g as usize].imported {
                Some(MoU::Module(t)) => t,
                Some(_) => continue,
                None => { self.busy.remove(&key); return Err(Ctl::Continue(Undetermined)); }
            };
            let result = self.resolve_ident_in_scope_set(name, ScopeSet::Module(ns, target), ps, None, ign_decl, None);
            match result {
                Err(Determined) => continue,
                Ok(b) if !self.accessible(self.dvis(b), self.imps[g as usize].module) => continue,
                Ok(_) | Err(Undetermined) => { self.busy.remove(&key); return Err(Ctl::Continue(Undetermined)); }
            }
        }
        self.busy.remove(&key);
        Err(Ctl::Continue(Determined))
    }

    /// `DeclData::determined`: a glob declaration of a module with unexpanded macros may still be
    /// shadowed.
    fn decl_determined(&self, d: DeclId) -> bool {
        match self.decls[d as usize].import {
            Some((i, src)) if matches!(self.imps[i as usize].kind, IK::Glob { .. }) => {
                !self.unexpanded.contains(&self.imps[i as usize].module) && self.decl_determined(src)
            }
            _ => true,
        }
    }

    fn finalize_module_binding(&mut self, name: Sym, binding: Option<DeclId>, ps: Ps, fin: Fin, _shadowing: Shadowing) -> Result<DeclId, Ctl> {
        let Some(b) = binding else { return Err(Ctl::Continue(Determined)) };
        if !self.accessible(self.dvis(b), ps.module) {
            if fin.report_private {
                self.privacy_errors += 1;
                if self.dres(b).is_some() { self.priv_list.push((self.cur_imp, name, b)); }
            } else {
                return Err(Ctl::Break(Determined));
            }
        }
        self.record_use(name, b);
        Ok(b)
    }

    /// `record_use_inner`'s walk down a declaration's import chain: the ambiguity errors it records,
    /// as the `warning` of each (None: an error).
    fn chain_ambiguities(&self, used: DeclId) -> Vec<Option<Warn>> {
        let mut out = Vec::new();
        let mut cur = used;
        let mut warn = self.decls[used as usize].warn_ambiguity;
        loop {
            if self.decls[cur as usize].ambiguity.is_some() { out.push(if warn { Some(Warn::Glob) } else { None }); }
            match self.decls[cur as usize].import {
                Some((_, src)) => { warn = warn || self.decls[src as usize].warn_ambiguity; cur = src; }
                None => break,
            }
        }
        out
    }

    fn record_use(&mut self, name: Sym, used: DeclId) {
        for w in self.chain_ambiguities(used) { self.push_amb(name, w); }
    }

    fn push_amb(&mut self, name: Sym, warning: Option<Warn>) {
        let imp = self.cur_imp;
        if self.amb.iter().any(|e| e.name == name && e.imp == imp && e.warning == warning) { return; }
        self.amb.push(AmbErr { name, imp, warning });
    }

    fn single_import_can_define_name(&mut self, r: &NameRes, binding: Option<DeclId>, ns: usize, ign_imp: Option<u32>, ign_decl: Option<DeclId>, ps: Ps) -> bool {
        for &si in &r.single_imports {
            if let Some(nd) = r.non_glob {
                if let Some((i, _)) = self.decls[nd as usize].import { if i == si { continue; } }
            }
            if ign_imp == Some(si) { continue; }
            if !self.accessible(self.imps[si as usize].vis, ps.module) { continue; }
            if let Some(ig) = ign_decl {
                if let Some((i, _)) = self.decls[ig as usize].import { if i == si { continue; } }
            }
            let Some(module) = self.imps[si as usize].imported else { return true };
            let (source, target, decls) = match &self.imps[si as usize].kind {
                IK::Single { source, target, decls } => (*source, *target, *decls),
                _ => unreachable!(),
            };
            if source != target {
                if decls.iter().all(|d| d.decl().is_none()) { return true; }
                else if decls[ns].decl().is_none() && binding.is_some() { return true; }
            }
            let sps = Ps { module: self.imps[si as usize].module, exp: self.imps[si as usize].exp };
            match self.resolve_ident_in_module(module, source, ns, sps, None, ign_decl, None) {
                Err(Determined) => continue,
                Ok(b) if !self.accessible(self.dvis(b), self.imps[si as usize].module) => continue,
                Ok(_) | Err(Undetermined) => return true,
            }
        }
        false
    }

    fn maybe_push_glob_vs_glob_vis_ambiguity(&mut self, name: Sym, decl: DeclId, imp: Option<Summ>) {
        let Some(s) = imp else { return };
        let vis1 = self.import_decl_vis_summ(decl, s, false);
        let vis2 = self.import_decl_vis_summ(decl, s, true);
        if vis1 != vis2 { self.push_amb(name, Some(Warn::Vis)); }
    }

    fn import_decl_vis_summ(&self, decl: DeclId, s: Summ, min: bool) -> BindVis {
        let decl_vis = if min { self.dmin_vis(decl) } else { self.dvis(decl) };
        let ord = self.vis_cmp(decl_vis, s.vis);
        let hack = s.is_single && s.vis.is_none() && self.is_extern_crate_import(decl);
        if ord == Some(Ordering::Less) && self.accessible(decl_vis, s.npm) && !hack { decl_vis } else { s.vis }
    }

    fn maybe_push_ambiguity(&mut self, name: Sym, ns: usize, set: ScopeSet, ps: Ps, decl: DeclId, scope: Scope, innermost: &[(DeclId, Scope)], imp: Option<Summ>) -> bool {
        let (innermost_decl, innermost_scope) = innermost[0];
        let (res, innermost_res) = (self.dres(decl), self.dres(innermost_decl));
        let ambig_vis = if res != innermost_res {
            None
        } else if let Some(s) = imp {
            let (v1, v2) = (self.import_decl_vis_summ(decl, s, false), self.import_decl_vis_summ(innermost_decl, s, false));
            if v1 != v2 { Some((v1, v2)) } else { return false }
        } else {
            return false;
        };
        let module_only = matches!(set, ScopeSet::Module(..));
        let in_modules = |s: Scope| matches!(s, Scope::NonGlobs(_) | Scope::Globs(_));
        let kind = if self.decls[innermost_decl as usize].import.is_some() && self.is_glob_decl(innermost_decl) {
            Some("GlobVsOuter")
        } else if !module_only && self.may_appear_after(innermost_decl, ps.exp, decl) {
            Some("MoreExpandedVsOuter")
        } else if self.decls[innermost_decl as usize].exp != 0
            && (!module_only || ns == 2)
            && matches!((scope, innermost_scope), (Scope::Globs(a), Scope::NonGlobs(b)) if a == b)
        {
            Some("GlobVsExpanded")
        } else {
            None
        };
        let _ = in_modules;
        let Some(_kind) = kind else { return false };
        let hack_145575 = matches!(scope, Scope::ExternFlags)
            && innermost[1..].iter().any(|(b, s)| matches!(s, Scope::ExternItems) && *b != innermost_decl);
        let hack_149681 = match scope {
            Scope::Globs(m1) => innermost[1..].iter().any(|(_, s)| matches!(*s, Scope::NonGlobs(m2) if m1 == m2)),
            _ => false,
        };
        if hack_145575 || hack_149681 {
            self.hack_applied = true;
            return false;
        }
        let warning = if ambig_vis.is_some() { Some(Warn::Vis) } else { None };
        self.push_amb(name, warning);
        let _ = ps;
        true
    }

    /// `DeclData::may_appear_after`.
    fn may_appear_after(&self, this: DeclId, invoc_exp: u32, decl: DeclId) -> bool {
        let desc = |a: u32, b: u32| a == b || b == 0;
        let self_exp = self.decls[this as usize].exp;
        let other_exp = self.decls[decl as usize].exp;
        !(desc(other_exp, self_exp) || desc(invoc_exp, self_exp))
    }

    // ------------------------------------------------------------------ modules and paths

    fn module_of_def(&self, d: DefId) -> Option<MoU> {
        let def = self.p.def(d);
        match def.kind {
            DefKind::Mod => Some(MoU::Module(def.sub)),
            DefKind::Enum | DefKind::Trait => Some(MoU::Container(d)),
            _ => None,
        }
    }

    fn resolve_ident_in_module(&mut self, module: MoU, name: Sym, ns: usize, ps: Ps, fin: Option<Fin>, ign_decl: Option<DeclId>, ign_imp: Option<u32>) -> Result<DeclId, Det> {
        match module {
            MoU::Module(m) => self.resolve_ident_in_scope_set(name, ScopeSet::Module(ns, m), ps, fin, ign_decl, ign_imp),
            MoU::Container(c) => {
                match self.container_decl(c, name, ns) {
                    Some(d) => {
                        if let Some(f) = fin { self.record_use(name, d); let _ = f; }
                        Ok(d)
                    }
                    None => Err(Determined),
                }
            }
            MoU::ModuleAndExternPrelude(m) => self.resolve_ident_in_scope_set(name, ScopeSet::ModuleAndExternPrelude(ns, m), ps, fin, ign_decl, ign_imp),
            MoU::ExternPrelude => {
                if ns != 0 { Err(Determined) } else { self.resolve_ident_in_scope_set(name, ScopeSet::ExternPrelude, ps, fin, ign_decl, ign_imp) }
            }
            MoU::CurrentScope => self.resolve_ident_in_scope_set(name, ScopeSet::All(ns), ps, fin, ign_decl, ign_imp),
        }
    }

    fn resolve_path(&mut self, path: &[(SegKind, Sym)], global: bool, edition: u16, ps: Ps, fin: Option<Fin>, ign_decl: Option<DeclId>, ign_imp: Option<u32>) -> PathRes {
        let mut module: Option<MoU> = None;
        let mut allow_super = true;
        let root = self.root_of(ps.module);
        if global {
            module = Some(if edition >= 2018 { MoU::ExternPrelude } else { MoU::ModuleAndExternPrelude(root) });
        }
        let n = path.len();
        for (idx, &(kind, name)) in path.iter().enumerate() {
            let is_last = idx + 1 == n;
            allow_super &= matches!(kind, SegKind::SelfValue | SegKind::Super);
            match kind {
                SegKind::Super if allow_super => {
                    let cur = match module {
                        None => Some(self.p.mods[ps.module as usize].normal),
                        Some(MoU::Module(m)) => Some(m),
                        _ => None,
                    };
                    let par = cur.and_then(|m| self.p.mods[self.p.mods[m as usize].normal as usize].parent_mod).map(|pm| self.p.mods[pm as usize].normal);
                    match par {
                        Some(pm) => { module = Some(MoU::Module(pm)); continue; }
                        None => return PathRes::Failed(false),
                    }
                }
                SegKind::SelfValue if idx == 0 => {
                    module = Some(MoU::Module(self.p.mods[ps.module as usize].normal));
                    continue;
                }
                SegKind::Crate if idx == 0 => {
                    module = Some(MoU::Module(root));
                    continue;
                }
                SegKind::Ident => {}
                _ => return PathRes::Failed(false),
            }
            let _ = is_last;
            let binding = match module {
                Some(m) => self.resolve_ident_in_module(m, name, 0, ps, fin, ign_decl, ign_imp),
                None => self.resolve_ident_in_scope_set(name, ScopeSet::All(0), ps, fin, ign_decl, ign_imp),
            };
            match binding {
                Ok(b) => {
                    match self.dres(b) {
                        Some(d) => match self.module_of_def(d) {
                            Some(m) => module = Some(m),
                            None => return PathRes::Failed(is_last),
                        },
                        None => return PathRes::NonModule,
                    }
                }
                Err(Undetermined) if fin.is_none() => return PathRes::Indeterminate,
                Err(_) => return PathRes::Failed(is_last),
            }
        }
        PathRes::Module(match module {
            // (the bare 2015 root: `use *;`, `use ::{a, b};`)
            Some(MoU::ModuleAndExternPrelude(r)) if path.is_empty() => MoU::Module(r),
            Some(m) => m,
            None => MoU::CurrentScope,
        })
    }

    // ------------------------------------------------------------------ the fixed point

    fn ps_of(&self, i: u32) -> Ps { Ps { module: self.imps[i as usize].module, exp: self.imps[i as usize].exp } }

    fn resolve_imports(&mut self) {
        let mut prev = usize::MAX;
        let mut count = self.indeterminate.len() * 3;
        while count < prev {
            prev = count;
            count = 0;
            for i in std::mem::take(&mut self.indeterminate) {
                let c = self.resolve_import(i);
                count += c;
                if c == 0 { self.determined.push(i); } else { self.indeterminate.push(i); }
            }
        }
    }

    fn resolve_import(&mut self, i: u32) -> usize {
        let module = match self.imps[i as usize].imported {
            Some(m) => m,
            None => {
                let (path, global, edition) = (self.imps[i as usize].path.clone(), self.imps[i as usize].global, self.imps[i as usize].edition);
                let ps = self.ps_of(i);
                match self.resolve_path(&path, global, edition, ps, None, None, Some(i)) {
                    PathRes::Module(m) => m,
                    PathRes::Indeterminate => return 3,
                    PathRes::NonModule | PathRes::Failed(_) => return 0,
                }
            }
        };
        self.imps[i as usize].imported = Some(module);
        let (source, target, decls) = match &self.imps[i as usize].kind {
            IK::Single { source, target, decls } => (*source, *target, *decls),
            IK::Glob { .. } => { self.resolve_glob_import(i); return 0; }
        };
        let ps = self.ps_of(i);
        let mut count = 0;
        let mut out = decls;
        for ns in 0..3 {
            if decls[ns] != Pend::Pending { continue; }
            // (macro namespace: a single-name import in the current scope finds a `macro_rules!` of its
            // textual scope first -- rustc's MacroRules scopes come before the module scopes)
            let textual = if ns == 2 && module == MoU::CurrentScope { self.textual_macro_rules(i, source) } else { None };
            let r = match textual { Some(d) => Ok(d), None => self.resolve_ident_in_module(module, source, ns, ps, None, None, Some(i)) };
            out[ns] = match r {
                Ok(binding) => {
                    let d = self.new_import_decl(binding, i);
                    let m = ps.module;
                    if self.p.syms.str(target) != "_" { let _ = self.try_plant(m, target, ns, d, false); }
                    Pend::Ready(Some(d))
                }
                Err(Determined) => {
                    if self.p.syms.str(target) != "_" {
                        let m = ps.module;
                        self.update_local_resolution(m, target, ns, false, |_, r| { r.single_imports.retain(|&x| x != i); });
                    }
                    Pend::Ready(None)
                }
                Err(Undetermined) => { count += 1; Pend::Pending }
            };
        }
        if let IK::Single { decls, .. } = &mut self.imps[i as usize].kind { *decls = out; }
        count
    }

    fn resolve_glob_import(&mut self, i: u32) {
        let here = self.imps[i as usize].module;
        match self.imps[i as usize].imported {
            Some(MoU::Module(module)) if module != here => {
                if self.local(module) {
                    self.glob_importers.entry(module).or_default().push(i);
                    let keys = self.order.get(&module).cloned().unwrap_or_default();
                    for (name, ns) in keys {
                        let Some(binding) = self.res.get(&(module, name, ns)).and_then(|r| r.determined_decl()) else { continue };
                        self.plant_glob(i, here, name, ns, binding);
                    }
                } else {
                    for &(name, ns) in self.ext_names(module).iter() {
                        if let Some(binding) = self.ext_decl(module, name, ns) { self.plant_glob(i, here, name, ns, binding); }
                    }
                }
            }
            Some(MoU::Container(c)) => {
                let d = self.p.def(c);
                if d.kind == DefKind::Enum {
                    for &v in &self.p.variants[d.sub as usize] {
                        let name = self.p.def(v).name;
                        for ns in 0..2 {
                            if let Some(binding) = self.container_decl(c, name, ns) { self.plant_glob(i, here, name, ns, binding); }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn plant_glob(&mut self, i: u32, here: u32, name: Sym, ns: usize, binding: DeclId) {
        if !self.accessible(self.dvis(binding), here) { return; }
        let d = self.new_import_decl(binding, i);
        let warn = self.res.get(&(here, name, ns)).and_then(|r| r.determined_decl()).map_or(false, |b| self.warn_ambiguity_recursive(b));
        let _ = self.try_plant(here, name, ns, d, warn);
    }

    // ------------------------------------------------------------------ finalization

    /// The ambiguities recorded on the glob declarations of the modules: (module, name, type ns).
    fn ambiguity_table(&self) -> HashMap<(u32, Sym, bool), u8> {
        let mut t = HashMap::default();
        for (&(m, name, ns), r) in &self.res {
            if ns == 2 { continue; }
            // low bits: what a use of the name finds (a name a module binds directly -- an item or a
            // single import -- has no glob ambiguity); bits 2..: the glob declaration's own
            let glob = r.glob.map_or(0, |g| { let amb = self.chain_ambiguities(g); if amb.is_empty() { 0 } else if amb.iter().any(|w| w.is_none()) { 1 } else { 2 } });
            if r.non_glob.is_none() && r.glob.is_none() { continue; }
            t.insert((m, name, ns_type(ns)), (if r.non_glob.is_some() { 0 } else { glob }) | (glob << 2));
        }
        t
    }

    fn mac_res(&self, d: DeclId) -> Option<MacRes> {
        let def = self.dres(d)?;
        if def.0 < u32::MAX && ((u32::MAX - 1 - def.0) as usize) < self.builtin.len() {
            let (n, std) = self.builtin[(u32::MAX - 1 - def.0) as usize];
            return Some(MacRes::Builtin(n, std));
        }
        Some(MacRes::Def(def))
    }

    /// The macro namespace of the checked modules for the macro-call check (lint.rs `macro_call_ambiguities`).
    fn macro_table(&self) -> HashMap<(u32, Sym), MacBinding> {
        let mut t = HashMap::default();
        for (&(m, name, ns), r) in &self.res {
            if ns != 2 { continue; }
            let glob_amb = r.glob.map_or(0, |g| { let amb = self.chain_ambiguities(g); if amb.is_empty() { 0 } else if amb.iter().any(|w| w.is_none()) { 1 } else { 2 } });
            let b = MacBinding { non_glob: r.non_glob.and_then(|d| self.mac_res(d)), glob: r.glob.and_then(|d| self.mac_res(d)), glob_amb };
            if b.non_glob.is_some() || b.glob.is_some() { t.insert((m, name), b); }
        }
        t
    }

    fn summ(&self, i: u32) -> Summ {
        let im = &self.imps[i as usize];
        Summ { vis: im.vis, npm: self.p.mods[im.module as usize].normal, is_single: matches!(im.kind, IK::Single { .. }) }
    }

    fn finalize_imports(&mut self) -> Vec<SimDiag> {
        let det = std::mem::take(&mut self.determined);
        let indet = std::mem::take(&mut self.indeterminate);
        for (is_indet, &i) in det.iter().map(|i| (false, i)).chain(indet.iter().map(|i| (true, i))) {
            self.cur_imp = i;
            self.finalize_import(i);
            self.import_dummy_binding(i, is_indet);
        }
        let mut out = Vec::new();
        let p = self.p;
        let at = |i: u32| { let im = &p.imports[self.imps[i as usize].pi]; (im.file, im.pos) };
        let mut seen = crate::fxhash::HashSet::default();
        for &i in &self.cannot_determine {
            if !seen.insert(i) { continue; }
            let (file, pos) = at(i);
            // invariant: an import the replay cannot determine is one the real resolver left unresolved
            let pim = &p.imports[self.imps[i as usize].pi];
            debug_assert!(!pim.done || !(pim.glob || pim.bound & (BOUND_TYPES | BOUND_VALUES) != 0),
                "replay cannot determine an import the real resolver resolved (file {file}, pos {pos})");
            out.push(SimDiag { file, pos, kind: SimKind::Error, msg: "cannot determine resolution for the import".to_string() });
        }
        // rustc throws the unresolved errors of the imports, and only when there are none the
        // indeterminate imports whose path has a `::`
        let text_of = |i: u32| {
            let pim = &p.imports[self.imps[i as usize].pi];
            let mut text = String::new();
            for (k, s) in &pim.segs {
                if !text.is_empty() { text.push_str("::"); }
                text.push_str(match k { SegKind::Crate => "crate", SegKind::SelfValue => "self", SegKind::Super => "super", _ => p.syms.str(*s) });
            }
            if pim.glob { text.push_str("::*"); }
            text
        };
        // (only an import Relax resolved is news: the others it reports itself)
        let news = |i: u32| { let pim = &p.imports[self.imps[i as usize].pi]; pim.done && (pim.glob || pim.bound & (BOUND_TYPES | BOUND_VALUES) != 0) };
        let mut unresolved: Vec<u32> = self.unresolved.iter().copied().filter(|&i| news(i)).collect();
        unresolved.dedup();
        if self.unresolved.is_empty() {
            for &i in &indet {
                if text_of(i).contains("::") { unresolved.push(i); }
            }
        }
        for i in unresolved {
            let pim = &p.imports[self.imps[i as usize].pi];
            out.push(SimDiag { file: pim.file, pos: pim.pos, kind: SimKind::Error, msg: format!("E0432: unresolved import `{}`", text_of(i)) });
        }
        let mut seen_priv = crate::fxhash::HashSet::default();
        for &(i, name, b) in &self.priv_list {
            let Some(def) = self.dres(b) else { continue };
            if !seen_priv.insert((i, name)) { continue; }
            let (file, pos) = at(i);
            let pe = PrivacyError { name, def, import: self.decls[b as usize].import.is_some(), ext: false, ambiguous: false, lint: false };
            out.push(SimDiag { file, pos, kind: SimKind::Error, msg: p.privacy_message(&pe) });
        }
        for e in &self.amb {
            let (file, pos) = at(e.imp);
            let n = p.syms.str(e.name);
            let (kind, msg) = match e.warning {
                None => (SimKind::Error, format!("E0659: `{n}` is ambiguous")),
                Some(Warn::Glob) => (SimKind::Lint("ambiguous_glob_imports"), format!("`{n}` is ambiguous")),
                Some(Warn::Vis) => (SimKind::Lint("ambiguous_import_visibilities"), format!("ambiguous import visibility")),
            };
            out.push(SimDiag { file, pos, kind, msg });
        }
        out
    }

    fn finalize_import(&mut self, i: u32) {
        let (path, global, edition) = (self.imps[i as usize].path.clone(), self.imps[i as usize].global, self.imps[i as usize].edition);
        let ps = self.ps_of(i);
        let summ = self.summ(i);
        let ignore_decl = match &self.imps[i as usize].kind { IK::Single { decls, .. } => decls[0].decl(), _ => None };
        // (the path is resolved under a `Finalize` without the import's summary)
        let fin = Fin { imp: None, report_private: true };
        let path_res = self.resolve_path(&path, global, edition, ps, Some(fin), ignore_decl, Some(i));
        let module = match path_res {
            PathRes::Module(m) => {
                if self.imps[i as usize].imported.is_none() && self.privacy_errors == 0 { self.cannot_determine.push(i); }
                m
            }
            PathRes::Failed(true) => { self.unresolved.push(i); return; }
            PathRes::Failed(false) | PathRes::NonModule | PathRes::Indeterminate => return,
        };
        let (source, decls) = match &self.imps[i as usize].kind {
            IK::Single { source, decls, .. } => (*source, *decls),
            IK::Glob { .. } => return,
        };
        // `use a::{self}` has the source `self` in rustc, which names the module in the type namespace only
        let self_only = self.p.imports[self.imps[i as usize].pi].bound & BOUND_SELF_ONLY != 0;
        let mut all_ns_err = true;
        for ns in 0..2 {
            if self_only && ns == 1 { continue; }
            let fin = Fin { imp: Some(summ), report_private: false };
            let binding = self.resolve_ident_in_module(module, source, ns, ps, Some(fin), decls[ns].decl(), Some(i));
            if let Ok(b) = binding {
                if decls[ns].decl().is_some() { all_ns_err = false; }
                let has_ambiguity_error = self.amb.iter().any(|e| e.warning.is_none());
                if self.dres(b).is_none() || has_ambiguity_error { continue; }
                if decls[ns].decl().is_none() && self.privacy_errors == 0 { self.cannot_determine.push(i); }
            }
        }
        if all_ns_err {
            let mut all_failed = true;
            for ns in 0..2 {
                if self_only && ns == 1 { continue; }
                let fin = Fin { imp: None, report_private: true };
                if self.resolve_ident_in_module(module, source, ns, ps, Some(fin), None, None).is_ok() { all_failed = false; }
            }
            if all_failed { self.unresolved.push(i); }
        }
    }

    fn import_dummy_binding(&mut self, i: u32, is_indeterminate: bool) {
        let (target, decls) = match &self.imps[i as usize].kind {
            IK::Single { target, decls, .. } => (*target, *decls),
            IK::Glob { .. } => return,
        };
        if !(is_indeterminate || decls.iter().all(|d| d.decl().is_none())) { return; }
        let dummy = self.dummy;
        let d = self.new_import_decl(dummy, i);
        let m = self.imps[i as usize].module;
        for ns in 0..3 {
            if self.p.syms.str(target) != "_" {
                let _ = self.try_plant(m, target, ns, d, false);
                self.update_local_resolution(m, target, ns, false, |_, r| { r.single_imports.retain(|&x| x != i); });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The diagnostics of the replay over a crate written in `source` (no std: nothing to import from it).
    fn replay(source: &str, edition: u16) -> Vec<(String, usize)> {
        let sess = &std::sync::Arc::new(relax_session::Session::default());
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("relax-rsim-{}-{n}.rs", std::process::id()));
        std::fs::write(&path, source).unwrap();
        let mut prog = Program::new(sess);
        let k = prog.load_crate("rsim_test", path.to_str().unwrap(), edition, crate::cfg::CfgSet::host(sess, &[]), vec![]).unwrap();
        std::fs::remove_file(path).unwrap();
        prog.resolve_imports();
        let mut own = vec![false; prog.crates.len()];
        own[k as usize] = true;
        prog.own_crates = own.clone();
        prog.run_import_sim(&own);
        let diags = prog.sim_diags.take().unwrap();
        diags.into_iter().map(|d| (match d.kind { SimKind::Error => d.msg, SimKind::Lint(l) => format!("lint {l}: {}", d.msg) }, prog.files[d.file as usize].line_col(d.pos).0)).collect()
    }

    fn has(v: &[(String, usize)], text: &str, line: usize) -> bool { v.iter().any(|(m, l)| m.contains(text) && *l == line) }

    #[test]
    fn a_self_import_of_a_module_whose_name_is_also_a_re_exported_fn_is_determined() {
        // `use dep::render::{self, Timing}` where `dep` re-exports a fn `render` at its root
        let d = replay("mod dep { pub mod render { pub struct Timing; pub fn render() {} } pub use self::render::render; }\nuse dep::render::{self, Timing};\nfn main() { let _ = Timing; render::render(); }\n", 2021);
        assert!(!has(&d, "cannot determine", 2), "{d:?}");
        let d = replay("mod dep { pub mod render { pub struct Timing; } pub fn render() {} }\nuse dep::{render::{self, Timing}};\nuse dep::render::{self as r2};\nfn main() { let _ = Timing; }\n", 2021);
        assert!(d.iter().all(|(m, _)| !m.contains("cannot determine")), "{d:?}");
    }

    #[test]
    fn a_glob_shadowed_by_a_pending_import_cannot_be_determined() {
        // rustc tests/ui/imports/shadow-glob-module-resolution-1.rs
        let d = replay("mod a { pub mod b { pub mod c {} } }\nuse a::*;\nuse b::c;\nuse c as b;\nfn main() {}\n", 2015);
        assert!(has(&d, "cannot determine resolution for the import", 3), "{d:?}");
        assert!(has(&d, "unresolved import `b::c`", 3), "{d:?}");
        assert!(!d.iter().any(|(_, l)| *l == 4), "{d:?}");
    }

    #[test]
    fn two_globs_of_different_items_are_an_ambiguity_lint() {
        let d = replay("mod b { pub fn x() {} }\nmod c { pub fn x() {} }\nmod a { pub use super::b::*; pub use super::c::*; }\nmod user { use super::a::*; pub fn f() { x(); } }\nfn main() {}\n", 2018);
        // (the use site is judged by the checks that read `sim_ambiguity`; the replay itself has no complaint)
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn a_glob_name_against_an_outer_scope_item_is_ambiguous() {
        // rustc: E0659 "a conflict between a name from a glob import and an outer scope"
        let d = replay("mod x { pub mod y {} }\nmod a { pub mod x { pub mod y {} } }\nfn main() {\n    use a::*;\n    use x::y;\n}\n", 2018);
        assert!(has(&d, "E0659: `x` is ambiguous", 5), "{d:?}");
    }

    #[test]
    fn plain_imports_report_nothing() {
        let d = replay("mod m { pub struct S; pub fn f() {} }\nuse m::{S, f};\nuse m::*;\nmod n { pub use crate::m::S as T; }\nfn main() { f(); }\n", 2018);
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn an_import_cycle_through_single_imports_is_reported() {
        // rustc tests/ui/imports/cycle-import-in-std-1.rs with a module in place of `std`
        let d = replay("mod m { pub mod collections {} }\nfn main() {\n    use ops::{self as m};\n    use m::collections::{self as ops};\n}\n", 2018);
        assert!(has(&d, "unresolved import `ops`", 3), "{d:?}");
    }

    #[test]
    fn a_statement_macro_expanding_to_a_macro_call_has_no_trailing_semicolon_lint() {
        // rustc tests/ui/type-alias-enum-variants/enum-variant-generic-args-pass.rs: the `;` after the
        // outer call goes to the inner call's statement
        let src = "macro_rules! inner { ($e:expr) => ( assert!($e, \"x\"); ) }\nmacro_rules! outer { ($e:expr) => ( inner!($e) ) }\nfn main() { outer!(true); }\n";
        crate::macros::uses::record(true);
        let _ = crate::macros::uses::take_trailing_semis();
        replay(src, 2021);
        let t = crate::macros::uses::take_trailing_semis();
        crate::macros::uses::record(false);
        assert!(t.is_empty(), "{}", t.len());
    }

    #[test]
    fn a_block_ending_macro_call_is_expanded_once_while_recording_lint_uses() {
        // the trailing-semicolon probe parsed a block-ending call's expansion with macro expansion
        // on: a chain of statement macros nested through blocks (nom's alt_trait_inner!/succ!)
        // expanded 3^depth times under `makepad check` (9 calls here, 6561 expansions). A const
        // initializer so the body is parsed at load whatever the lazy-body mode.
        let mut src = String::from("macro_rules! step { (0) => { () };");
        for n in 1..=8 {
            src.push_str(&format!(" ({n}) => {{ {{ step!({}) }} }};", n - 1));
        }
        src.push_str(" }\nconst X: () = { step!(8) };\n");
        let sess = &std::sync::Arc::new(relax_session::Session::default());
        let path = std::env::temp_dir().join(format!("relax-rsim-expansions-{}.rs", std::process::id()));
        std::fs::write(&path, &src).unwrap();
        crate::macros::uses::record(true);
        let mut prog = Program::new(sess);
        let k = prog.load_crate("rsim_test", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), vec![]);
        crate::macros::uses::record(false);
        std::fs::remove_file(path).unwrap();
        let k = k.unwrap();
        let expansions: usize = prog.files.iter().zip(&prog.file_crate).filter(|(_, &c)| c == k).map(|(f, _)| f.exp_map.len()).sum();
        assert_eq!(expansions, 9, "expansions of a chain of 9 calls");
    }
}
