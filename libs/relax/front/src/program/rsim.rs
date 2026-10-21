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
    Single { source: Sym, target: Sym, decls: [Pend; 2] },
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
}

fn ns_type(ns: usize) -> bool { ns == 0 }

impl Program {
    /// What rustc's import resolution reports for the imports of the checked crates beyond the
    /// errors Relax's own resolution found (see the module comment); the glob ambiguities it
    /// found become the answer of `glob_ambiguity` for the checked modules.
    pub fn run_import_sim(&mut self, own: &[bool]) {
        let prev = record_import_uses();
        set_record_import_uses(false);
        let (out, amb) = {
            let mut sim = Sim::new(self, own);
            sim.init(false);
            sim.resolve_imports();
            sim.unexpanded.clear();
            sim.init(true);
            sim.resolve_imports();
            let out = sim.finalize_imports();
            (out, sim.ambiguity_table())
        };
        set_record_import_uses(prev);
        self.sim_ambiguity = Some(amb);
        self.sim_diags = Some(out);
        self.glob_amb_cache.clear();
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
            indeterminate: Vec::new(), determined: Vec::new(),
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
        let mut q = GlobQuery { precise: true, ..Default::default() };
        let r = self.p.lookup_in_module_vis(m, name, ns_type(ns), &mut q).map(|(d, v)| {
            self.alloc(Decl { def: Some(d), import: None, ambiguity: None, warn_ambiguity: false, vis: v, vis_max: None, vis_min: None, exp: 0 })
        });
        self.ext_decls.insert((m, name, ns), r);
        r
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
                let krate = if kind == SegKind::Crate { Some(p.mods[module as usize].krate) } else { p.crates[p.mods[module as usize].krate as usize].deps.iter().find(|x| x.0 == name).map(|x| x.1) };
                let Some(c) = krate else { continue };
                let root = p.mods[p.crates[c as usize].root_mod as usize].def;
                let src = self.alloc(Decl { def: Some(root), import: None, ambiguity: None, warn_ambiguity: false, vis: None, vis_max: None, vis_min: None, exp: 0 });
                let idx = self.imps.len() as u32;
                self.imps.push(SImp { pi, module, vis, exp, kind: IK::Single { source: name, target: bind, decls: [Pend::Ready(None), Pend::Ready(None)] }, path: Vec::new(), global: false, imported: None, edition });
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
                        self.imps.push(SImp { pi, module, vis, exp, kind: IK::Single { source: last, target, decls: [Pend::Ready(None), Pend::Ready(None)] }, path: Vec::new(), global: false, imported: None, edition });
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
            let decls = [Pend::Pending, if type_only { Pend::Ready(None) } else { Pend::Pending }];
            self.imps.push(SImp { pi, module, vis, exp, kind: IK::Single { source, target, decls }, path, global, imported: None, edition });
            if p.syms.str(target) != "_" {
                for ns in 0..2 {
                    if type_only && ns == 1 { continue; }
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
            let mut ks: Vec<(Sym, usize)> = md.types.keys().map(|&n| (n, 0)).chain(md.values.keys().map(|&n| (n, 1))).collect();
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
                Scope::StdPrelude => use_prelude,
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
                match self.p.crates[krate as usize].deps.iter().find(|x| x.0 == name) {
                    Some(&(_, c)) => {
                        let root = self.p.mods[self.p.crates[c as usize].root_mod as usize].def;
                        let d = self.alloc(Decl { def: Some(root), import: None, ambiguity: None, warn_ambiguity: false, vis: None, vis_max: None, vis_min: None, exp: 0 });
                        Ok(d)
                    }
                    None => Err(Ctl::Continue(Determined)),
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
        for ns in 0..2 {
            if decls[ns] != Pend::Pending { continue; }
            let r = self.resolve_ident_in_module(module, source, ns, ps, None, None, Some(i));
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
            // low bits: what a use of the name finds (a name a module binds directly -- an item or a
            // single import -- has no glob ambiguity); bits 2..: the glob declaration's own
            let glob = r.glob.map_or(0, |g| { let amb = self.chain_ambiguities(g); if amb.is_empty() { 0 } else if amb.iter().any(|w| w.is_none()) { 1 } else { 2 } });
            if r.non_glob.is_none() && r.glob.is_none() { continue; }
            t.insert((m, name, ns_type(ns)), (if r.non_glob.is_some() { 0 } else { glob }) | (glob << 2));
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
        for ns in 0..2 {
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
