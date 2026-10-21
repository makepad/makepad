//! Item-level checks of `makepad check` that need no body: duplicate definitions (E0428,
//! rustc_resolve build_reduced_graph / report_conflict) and recursive types without
//! indirection (E0072, rustc_ty_utils representability.rs).

use crate::ast;
use crate::program::{DefId, DefKind, Program, NO_DEF};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};

/// The namespaces a definition takes in its module: (type, value).
fn namespaces(prog: &Program, d: DefId) -> (bool, bool) {
    let def = prog.def(d);
    match def.kind {
        DefKind::Mod | DefKind::Enum | DefKind::Union | DefKind::Trait | DefKind::TypeAlias => (true, false),
        // (foreign items count only in an `extern` block written at module level: one inside a fn
        // body is collected into the enclosing module, not the body's block, so two bodies'
        // declarations would collide)
        DefKind::Fn | DefKind::Const | DefKind::Static | DefKind::ForeignFn | DefKind::ForeignStatic => (false, true),
        // the items of a trait (`duplicate_items` only asks for those of traits)
        DefKind::AssocFn | DefKind::AssocConst => (false, true),
        DefKind::AssocTy => (true, false),
        DefKind::Struct => {
            // tuple and unit structs also live in the value namespace
            let ast = &prog.files[def.file as usize].ast;
            let tuple_or_unit = ast.as_struct(&ast.item(def.item).kind).is_some_and(|s| !matches!(s.data, crate::ast::VariantData::Struct(_)));
            (true, tuple_or_unit)
        }
        _ => (false, false),
    }
}

/// E0428: two items of one module (or block) with the same name in the same namespace. The
/// second is reported, as rustc does ("the name `X` is defined multiple times").
pub fn duplicate_items(prog: &Program, own: &[bool]) -> Vec<String> {
    let mut seen: crate::fxhash::HashMap<(DefId, bool, u32), DefId> = Default::default();
    let mut errors = Vec::new();
    let mut top_foreign: crate::fxhash::HashSet<(u32, u32)> = Default::default();
    for (m, md) in prog.mods.iter().enumerate() {
        if !own.get(md.krate as usize).copied().unwrap_or(false) || md.lexical_parent.is_some() { continue; }
        let (file, items) = crate::lint_dead::module_items(prog, m as u32);
        let ast = &prog.files[file as usize].ast;
        for it in items {
            if let ast::ItemKind::ForeignMod(r) = ast.item(it).kind {
                for &c in ast.list(ast.row(r).items) { top_foreign.insert((file, c.0)); }
            }
        }
    }
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if !own.get(def.krate as usize).copied().unwrap_or(false) || def.parent == NO_DEF { continue; }
        if !matches!(prog.def(def.parent).kind, DefKind::Mod | DefKind::Trait) { continue; }
        // only items the module's source declares (not the compiler's own definitions)
        if prog.item_defs.get(&(def.file, def.item.0)) != Some(&d) { continue; }
        let name = prog.syms.str(def.name);
        if name == "_" || name.is_empty() { continue; }
        let foreign = matches!(def.kind, DefKind::ForeignFn | DefKind::ForeignStatic);
        if foreign && !top_foreign.contains(&(def.file, def.item.0)) { continue; }
        // a module file configured away by its own inner `#![cfg]` defines nothing
        if def.kind == DefKind::Mod {
            let m = &prog.mods[def.sub as usize];
            let attrs: Vec<crate::ast::Attr> = prog.files[m.file as usize].ast.root_attrs.iter().copied().collect();
            if m.file != def.file && !prog.active(m.file, &attrs) { continue; }
        }
        let (ty_ns, value_ns) = namespaces(prog, d);
        for (ns, on) in [(true, ty_ns), (false, value_ns)] {
            if !on { continue; }
            match seen.get(&(def.parent, ns, def.name)) {
                Some(&first) if first != d => {
                    let ast = &prog.files[def.file as usize].ast;
                    let pos = if foreign { ast.item(def.item).lo } else { ast.item(def.item).name.lo };
                    let ns_name = if ns { "type" } else { "value" };
                    let module = if prog.def(def.parent).kind == DefKind::Trait { "this trait" } else if prog.block_owner.get(&prog.def(def.parent).sub).is_some() { "this block" } else { "this module" };
                    errors.push(prog.record_error(def.file, pos, "resolve", format!(
                        "E0428: the name `{}` is defined multiple times -- note: `{}` must be defined only once in the {} namespace of {}",
                        name, name, ns_name, module)));
                    break;
                }
                Some(_) => {}
                None => { seen.insert((def.parent, ns, def.name), d); }
            }
        }
    }
    errors
}

/// E0428 for `#[macro_export]` macros: two exported `macro_rules!` of one name in a crate share the
/// crate root's macro namespace (rustc_resolve build_reduced_graph define). The later one, in
/// source order, is reported; a macro the definition of which another crate wrote is not ours.
pub fn duplicate_exported_macros(prog: &Program, own: &[bool]) -> Vec<String> {
    let mut errors = Vec::new();
    for (name, defs) in prog.macros.defs.iter() {
        let mut seen: Vec<(u32, u32, u32)> = Vec::new(); // (krate, file, pos)
        let mut mine: Vec<&crate::macros::MacroDef> = defs.iter().map(|d| &**d).filter(|d| d.exported && d.builtin == 0 && !d.def_file.is_empty()).collect();
        // (source order: a macro an expansion defines counts at its call)
        let key = |d: &crate::macros::MacroDef| prog.file_paths.iter().position(|p| *p == d.def_file).map_or(d.def_pos, |f| prog.files[f].diagnostic_pos(d.def_pos));
        mine.sort_by(|a, b| (a.def_file.as_str(), key(a)).cmp(&(b.def_file.as_str(), key(b))));
        for d in mine {
            let Some(file) = prog.file_paths.iter().position(|p| *p == d.def_file) else { continue };
            let krate = prog.file_crate.get(file).copied().unwrap_or(u32::MAX);
            if !own.get(krate as usize).copied().unwrap_or(false) || d.def_pos >= prog.files[file].src_len { continue; }
            let kpos = key(d);
            if d.def_pos < prog.files[file].src_len && seen.iter().any(|&(k, f, p)| k == krate && !(f == file as u32 && p == kpos)) {
                errors.push(prog.record_error(file as u32, d.def_pos, "resolve", format!(
                    "E0428: the name `{}` is defined multiple times -- note: `{}` must be defined only once in the macro namespace of this module", name, name)));
            }
            seen.push((krate, file as u32, kpos));
        }
        // a macro the crate root imports under the same name (`pub use std::panic;`) is the same name
        // in the macro namespace (E0255)
        let sym = prog.syms.get(name);
        for d in defs.iter().map(|d| &**d).filter(|d| d.exported && d.builtin == 0 && !d.def_file.is_empty()) {
            let Some(file) = prog.file_paths.iter().position(|p| *p == d.def_file) else { continue };
            let krate = prog.file_crate.get(file).copied().unwrap_or(u32::MAX);
            if !own.get(krate as usize).copied().unwrap_or(false) || d.def_pos >= prog.files[file].src_len { continue; }
            let root = prog.crates[krate as usize].root_mod;
            let clash = prog.imports_iter().any(|im| im.module == root && !im.glob && !im.ext && im.bind == sym && sym.is_some()
                && im.segs.len() > 1 && im.segs.last().map(|s| s.1) == sym
                && (crate::program::BUILTIN_MACROS.contains(&name.as_str()) || relax_front::builtins::is_macro_name(name)));
            if clash {
                errors.push(prog.record_error(file as u32, d.def_pos, "resolve", format!(
                    "E0255: the name `{}` is defined multiple times -- note: `{}` must be defined only once in the macro namespace of this module", name, name)));
            }
        }
    }
    errors
}

/// E0446 (rustc_privacy `check_assoc_item` with `hard_error` for associated types): the type of an
/// associated type of a trait impl is no less visible than the impl itself (the least visible of
/// the trait and the types of its trait reference).
pub fn private_assoc_types(prog: &Program, tcx: &Tcx, own: &[bool]) -> Vec<String> {
    fn adts(tcx: &Tcx, t: TyId, out: &mut Vec<DefId>, depth: u32) {
        if depth > 24 { return; }
        match tcx.tys.kind(t).clone() {
            TyKind::Adt(d, args, _) => { out.push(d); for &a in tcx.tys.list(args) { adts(tcx, a, out, depth + 1); } }
            TyKind::Dyn(td, args, binds, ..) => {
                out.push(td);
                for &a in tcx.tys.list(args) { adts(tcx, a, out, depth + 1); }
                for b in tcx.tys.list(binds) { adts(tcx, b.1, out, depth + 1); }
            }
            TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) | TyKind::Slice(x) | TyKind::Array(x, _) => adts(tcx, x, out, depth + 1),
            TyKind::Tuple(l) => for &x in tcx.tys.list(l) { adts(tcx, x, out, depth + 1); },
            _ => {}
        }
    }
    let mut errors = Vec::new();
    // (a definition an expansion wrote has no visibility the AST states: its checks are skipped)
    let written = |d: DefId| { let def = prog.def(d); def.item.0 != u32::MAX && prog.files.get(def.file as usize).is_some_and(|f| f.ast.items.get(def.item.0 as usize).is_some_and(|it| it.lo < f.src_len)) };
    for i in 0..prog.defs.len() {
        let imp = DefId(i as u32);
        let idef = prog.def(imp);
        if idef.kind != DefKind::Impl || !own.get(idef.krate as usize).copied().unwrap_or(false) || !written(imp) { continue; }
        let Some(&tr) = tcx.impl_trait.get(&imp) else { continue };
        if !own.get(prog.def(tr).krate as usize).copied().unwrap_or(false) && prog.def(tr).krate != idef.krate { /* foreign trait: its vis is public */ }
        // the impl's visibility
        let mut vis = prog.def_item_vis(tr);
        if !own.get(prog.def(tr).krate as usize).copied().unwrap_or(false) { vis = None; }
        let mut tys: Vec<DefId> = Vec::new();
        let args = tcx.generic_indices_of(prog, imp).identity_args(&tcx.tys);
        for a in tcx.impl_trait_ref(prog, imp, &args).args.iter() {
            if let crate::consts::GenericArg::Type(t) = a { adts(tcx, *t, &mut tys, 0); }
        }
        let mut known = written(tr) || !own.get(prog.def(tr).krate as usize).copied().unwrap_or(false);
        for d in tys {
            if own.get(prog.def(d).krate as usize).copied().unwrap_or(false) {
                known &= written(d);
                vis = prog.vis_min(vis, prog.def_item_vis(d));
            }
        }
        if !known { continue; }
        for j in 0..prog.defs.len() {
            let ad = DefId(j as u32);
            let adef = prog.def(ad);
            if adef.kind != DefKind::AssocTy || adef.parent != imp { continue; }
            let Ok(t) = tcx.type_of(prog, ad) else { continue };
            let mut used = Vec::new();
            adts(tcx, t, &mut used, 0);
            for u in used {
                if !own.get(prog.def(u).krate as usize).copied().unwrap_or(false) || !written(u) { continue; }
                if prog.vis_wider(vis, prog.def_item_vis(u)) {
                    let pos = prog.files[adef.file as usize].ast.item(adef.item).name.lo;
                    errors.push(prog.record_error(adef.file, pos, "typeck", format!("E0446: private type `{}` in public interface", prog.syms.str(prog.def(u).name))));
                    break;
                }
            }
        }
    }
    errors
}

/// The path of an impl's trait is privacy-checked like any path (rustc_resolve: E0603 for a
/// private module or trait on the way).
pub fn impl_trait_path_privacy(prog: &Program, tcx: &Tcx, own: &[bool]) -> Vec<String> {
    let mut errors = Vec::new();
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if def.kind != DefKind::Impl || !own.get(def.krate as usize).copied().unwrap_or(false) { continue; }
        let ast = &prog.files[def.file as usize].ast;
        let Some(imp) = ast.as_impl(&ast.item(def.item).kind) else { continue };
        let Some(p) = imp.trait_ else { continue };
        if p.segs.len() < 2 || !p.no_qself() || p.lo >= prog.files[def.file as usize].src_len { continue; }
        let segs = crate::program::path_segs(prog, def.file, &p);
        let module = tcx.item_module(prog, d);
        if let Some(e) = prog.path_privacy(module, p.global(), &segs, Some(true), false, false) {
            if let Some(m) = crate::lint::privacy_diag(prog, def.file, p.lo, &e) { errors.push(prog.record_error(def.file, p.lo, "resolve", m)); }
        }
    }
    errors
}

/// A `pub(in path)` visibility path with a segment that names no module in the middle (rustc
/// `resolve_path`: "cannot find module `String` in `string`" when it is a struct).
fn vis_path_segments(prog: &Program, file: u32, scope: u32, p: crate::ast::Row<crate::ast::Path>, errors: &mut Vec<String>) {
    use crate::ast::SegKind;
    let ast = &prog.files[file as usize].ast;
    let pf = &prog.files[file as usize];
    let p = ast.row(p);
    let segs = crate::program::path_segs(prog, file, p);
    let written = ast.segs(p.segs);
    let from = prog.mods[scope as usize].normal;
    for k in 1..segs.len().saturating_sub(1) {
        if written[k].kind() != SegKind::Ident { continue; }
        match prog.resolve_mod_path(from, p.global(), &segs[..=k], false, false) {
            Some(m) if prog.def(m).kind != DefKind::Mod => {
                let at = written[k].name.lo;
                if at < pf.src_len {
                    errors.push(prog.record_error(file, at, "resolve", format!("E0433: cannot find module `{}` in `{}`", pf.text(written[k].name), pf.text(written[k - 1].name))));
                }
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
}

/// E0433 `too many leading `super` keywords`: a `pub(super)` / `pub(in super::super)` visibility
/// (also one a macro expansion wrote) with fewer enclosing modules than `super`s; and a global
/// `::super::x` path in a `use` (rustc_resolve: global paths cannot start with `super`).
pub fn super_paths(prog: &Program, own: &[bool]) -> Vec<String> {
    use crate::ast::{SegKind, Vis};
    let mut errors = Vec::new();
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if !own.get(def.krate as usize).copied().unwrap_or(false) || def.item.0 == u32::MAX { continue; }
        if prog.item_defs.get(&(def.file, def.item.0)) != Some(&d) { continue; }
        let ast = &prog.files[def.file as usize].ast;
        let item = ast.item(def.item);
        let depth = |m: u32| { let mut n = 0; let mut cur = prog.mods[prog.mods[m as usize].normal as usize].parent_mod; while let Some(p) = cur { n += 1; cur = prog.mods[p as usize].parent_mod; } n };
        let scope = if matches!(def.kind, DefKind::AssocFn | DefKind::AssocConst | DefKind::AssocTy) { prog.def(def.parent).scope } else { def.scope };
        let too_many = match item.vis {
            Vis::Super => depth(scope) == 0,
            Vis::In(p) => {
                let p = ast.row(p);
                let supers = ast.segs(p.segs).iter().take_while(|s| s.kind() == SegKind::Super).count();
                supers > depth(scope)
            }
            _ => false,
        };
        if too_many {
            errors.push(prog.record_error(def.file, item.lo, "resolve", "E0433: failed to resolve: too many leading `super` keywords".to_string()));
        } else if let Vis::In(p) = item.vis {
            vis_path_segments(prog, def.file, scope, p, &mut errors);
        }
    }
    let mut seen_items = crate::fxhash::HashSet::default();
    for im in prog.imports_iter() {
        let krate = prog.mods[im.module as usize].krate;
        if !own.get(krate as usize).copied().unwrap_or(false) { continue; }
        // the visibility path of the `use` / `extern crate` item this import came from
        if im.pos < prog.files[im.file as usize].src_len && seen_items.insert((im.file, im.pos)) {
            let ast = &prog.files[im.file as usize].ast;
            if let Some(item) = ast.items.iter().find(|it| it.lo == im.pos && matches!(it.kind, crate::ast::ItemKind::Use { .. } | crate::ast::ItemKind::ExternCrate(_))) {
                if let Vis::In(p) = item.vis { vis_path_segments(prog, im.file, im.module, p, &mut errors); }
            }
        }
        if im.ext || !im.global || im.segs.first().map(|s| s.0) != Some(SegKind::Super) { continue; }
        if im.pos >= prog.files[im.file as usize].src_len { continue; }
        errors.push(prog.record_error(im.file, im.pos, "resolve", "E0433: failed to resolve: global paths cannot start with `super`".to_string()));
    }
    errors
}

/// A query of rustc_ty_utils/src/representability.rs, the nodes of its cycle detection.
#[derive(Clone, PartialEq, Eq, Hash)]
enum ReprKey {
    /// `check_representability(adt)`
    Adt(DefId),
    /// `check_representability(field)`: the ADT, variant and field index
    Field(DefId, usize, usize),
    /// `check_representability_adt_ty(ty)`
    AdtTy(TyId),
}

struct Representability<'a> {
    tcx: &'a Tcx,
    /// the queries being computed, innermost last
    active: Vec<ReprKey>,
    done: crate::fxhash::HashSet<ReprKey>,
    /// the first cycle met: its queries, outermost first
    cycle: Option<Vec<ReprKey>>,
    /// `params_in_repr` results that no cycle cut short (rustc caches the query)
    params_memo: std::cell::RefCell<crate::fxhash::HashMap<DefId, crate::fxhash::HashSet<u32>>>,
    /// a cycle cut the `params_in_repr` computation now running
    truncated: std::cell::Cell<bool>,
}

impl Representability<'_> {
    fn enter(&mut self, key: &ReprKey) -> bool {
        if self.cycle.is_some() || self.done.contains(key) { return false; }
        if let Some(at) = self.active.iter().position(|k| k == key) {
            self.cycle = Some(self.active[at..].to_vec());
            return false;
        }
        self.active.push(key.clone());
        true
    }

    fn leave(&mut self, key: ReprKey) {
        self.active.pop();
        if self.cycle.is_none() { self.done.insert(key); }
    }

    fn check_adt(&mut self, d: DefId) {
        let key = ReprKey::Adt(d);
        if !self.enter(&key) { return; }
        if let Some(adt) = self.tcx.adts.get(&d) {
            for (vi, v) in adt.variants.iter().enumerate() {
                for fi in 0..v.fields.len() { self.check_field(d, vi, fi); }
            }
        }
        self.leave(key);
    }

    fn check_field(&mut self, d: DefId, vi: usize, fi: usize) {
        let key = ReprKey::Field(d, vi, fi);
        if !self.enter(&key) { return; }
        let ty = self.tcx.adts[&d].variants[vi].fields[fi].ty;
        self.check_ty(ty);
        self.leave(key);
    }

    fn check_ty(&mut self, t: TyId) {
        match self.tcx.tys.kind(t).clone() {
            TyKind::Adt(..) => self.check_adt_ty(t),
            TyKind::Array(e, _) => self.check_ty(e),
            TyKind::Tuple(ts) => for x in self.tcx.tys.list(ts).to_vec() { self.check_ty(x); },
            _ => {}
        }
    }

    fn check_adt_ty(&mut self, t: TyId) {
        let key = ReprKey::AdtTy(t);
        if !self.enter(&key) { return; }
        let TyKind::Adt(d, args, _) = self.tcx.tys.kind(t).clone() else { unreachable!() };
        // the item itself is representable; its parameters may still cycle through a type argument
        self.check_adt(d);
        if self.cycle.is_none() {
            let used = self.params_in_repr(d, &mut Vec::new());
            for (i, a) in self.tcx.tys.list(args).to_vec().into_iter().enumerate() {
                if used.contains(&(i as u32)) { self.check_ty(a); }
            }
        }
        self.leave(key);
    }

    /// `params_in_repr`: the type parameters (by compact index) an ADT's fields hold by value.
    fn params_in_repr(&self, d: DefId, visiting: &mut Vec<DefId>) -> crate::fxhash::HashSet<u32> {
        let mut out = crate::fxhash::HashSet::default();
        if visiting.contains(&d) { self.truncated.set(true); return out; }
        if let Some(done) = self.params_memo.borrow().get(&d) { return done.clone(); }
        let outer = self.truncated.replace(false);
        visiting.push(d);
        if let Some(adt) = self.tcx.adts.get(&d) {
            for v in &adt.variants {
                for f in &v.fields { self.params_in_repr_ty(f.ty, &mut out, visiting); }
            }
        }
        visiting.pop();
        if !self.truncated.get() { self.params_memo.borrow_mut().insert(d, out.clone()); }
        self.truncated.set(outer || self.truncated.get());
        out
    }

    fn params_in_repr_ty(&self, t: TyId, out: &mut crate::fxhash::HashSet<u32>, visiting: &mut Vec<DefId>) {
        match self.tcx.tys.kind(t).clone() {
            TyKind::Adt(d, args, _) => {
                let inner = self.params_in_repr(d, visiting);
                for (i, a) in self.tcx.tys.list(args).to_vec().into_iter().enumerate() {
                    if inner.contains(&(i as u32)) { self.params_in_repr_ty(a, out, visiting); }
                }
            }
            TyKind::Array(e, _) => self.params_in_repr_ty(e, out, visiting),
            TyKind::Tuple(ts) => for x in self.tcx.tys.list(ts).to_vec() { self.params_in_repr_ty(x, out, visiting); },
            TyKind::Param(i) => { out.insert(i); }
            _ => {}
        }
    }
}

/// E0072: an ADT that contains itself by value (through fields, tuples, arrays and the
/// parameters other ADTs hold by value) has no finite size (rustc_ty_utils
/// representability.rs: a cycle among its queries; rustc_middle values.rs reports the ADTs of
/// the cycle once, at the first). The cycle is reported by the first of its ADTs.
pub fn representability(prog: &Program, tcx: &Tcx, d: DefId) -> Option<String> {
    let mut r = Representability { tcx, active: Vec::new(), done: Default::default(), cycle: None, params_memo: Default::default(), truncated: Default::default() };
    r.check_adt(d);
    let cycle = r.cycle?;
    let mut items: Vec<DefId> = Vec::new();
    for k in &cycle {
        if let ReprKey::Field(a, _, _) = k { if !items.contains(a) { items.push(*a); } }
    }
    // reported once: by the first ADT of the cycle
    if items.first().is_none() || items.iter().min_by_key(|x| x.0) != Some(&d) || !items.contains(&d) { return None; }
    let at = items.iter().position(|x| *x == d).unwrap();
    items.rotate_left(at);
    let names: Vec<String> = items.iter().map(|x| format!("`{}`", prog.syms.str(prog.def(*x).name))).collect();
    let list = match names.len() {
        1 => names[0].clone(),
        2 => format!("{} and {}", names[0], names[1]),
        _ => format!("{} and {}", names[..names.len() - 1].join(", "), names[names.len() - 1]),
    };
    let def = prog.def(d);
    let ast = &prog.files[def.file as usize].ast;
    let pos = ast.item(def.item).name.lo;
    Some(prog.record_error(def.file, pos, "typeck", format!(
        "E0072: recursive type{} {} {} infinite size -- help: insert some indirection (e.g., a `Box`, `Rc`, or `&`) to break the cycle",
        if items.len() == 1 { "" } else { "s" }, list, if items.len() == 1 { "has" } else { "have" })))
}

/// E0391 for a bound that equates an associated type with another of the same parameter
/// (`fn f<T: Trait<A = T::B>>()`): lowering `T::B` needs T's bounds, which are being lowered
/// (rustc: cycle detected when computing the bounds for type parameter `T`).
pub fn bound_assoc_cycles(prog: &Program, tcx: &Tcx, d: DefId) -> Vec<String> {
    let def = prog.def(d);
    let ast = &prog.files[def.file as usize].ast;
    let Some(g) = ast.kind_generics(&ast.item(def.item).kind) else { return Vec::new() };
    let mut out = Vec::new();
    let mut check = |param: &str, bounds: &[ast::Bound], pos: u32| {
        for b in bounds {
            let ast::Bound::Trait { path, .. } = b else { continue };
            let Some(bt) = tcx.resolve_type_path_def(prog, def.file, def.scope, path) else { continue };
            for seg in ast.segs(path.segs) {
                let Some(ast::GenericArgs::Angle(v)) = ast.gargs(seg.args()) else { continue };
                for a in ast.list(*v) {
                    let ast::GenericArg::Binding(_, _, t) = a else { continue };
                    let ast::Ty::Path(p) = ast.ty(*t) else { continue };
                    let segs = ast.segs(p.segs);
                    if segs.len() == 2 && p.no_qself() && prog.files[def.file as usize].text(segs[0].name) == param {
                        let sym = prog.syms.get(prog.files[def.file as usize].text(segs[1].name)).unwrap_or(u32::MAX);
                        if tcx.trait_assoc(prog, bt, sym).is_some() {
                            out.push(prog.record_error(def.file, pos, "typeck", format!("E0391: cycle detected when computing the bounds for type parameter `{}`", param)));
                        }
                    }
                }
            }
        }
    };
    for p in ast.list(g.params) {
        if let ast::GenericParamKind::Type(bounds, _) = &p.kind {
            let name = prog.files[def.file as usize].text(p.name).to_string();
            check(&name, ast.list(*bounds), p.name.lo);
        }
    }
    for w in ast.list(g.where_) {
        if let ast::WherePred::Bound { ty, bounds, .. } = w {
            if let ast::Ty::Path(p) = ast.ty(*ty) {
                let segs = ast.segs(p.segs);
                if segs.len() == 1 && p.no_qself() {
                    let name = prog.files[def.file as usize].text(segs[0].name).to_string();
                    check(&name, ast.list(*bounds), segs[0].name.lo);
                }
            }
        }
    }
    out
}

/// E0391 for a trait's supertrait bounds (rustc_hir_analysis collect/predicates_of.rs
/// super_predicates_of): a trait that is its own supertrait, directly or through others, and a
/// bound that names an associated type through `Self` (`trait T: Iterator<Item = Self::Item>`),
/// which asks for the super traits that define it while they are being computed.
pub fn supertrait_cycles(prog: &Program, tcx: &Tcx, td: DefId) -> Vec<String> {
    let def = prog.def(td);
    let ast = &prog.files[def.file as usize].ast;
    let Some(ast::TraitItem { supers, .. }) = ast.as_trait(&ast.item(def.item).kind) else { return Vec::new() };
    let mut out = Vec::new();
    // a cycle through the super bounds: reported once, at its lowest member
    let mut seen = std::collections::HashSet::new();
    let mut stack = tcx.trait_supers(prog, td);
    let mut cyclic = false;
    let mut members = vec![td];
    while let Some(t) = stack.pop() {
        if t == td { cyclic = true; continue; }
        if !seen.insert(t) { continue; }
        members.push(t);
        stack.extend(tcx.trait_supers(prog, t));
    }
    if cyclic {
        // members that are on the cycle: those that reach `td` again
        let on_cycle = |m: DefId| -> bool {
            let mut seen = std::collections::HashSet::new();
            let mut stack = tcx.trait_supers(prog, m);
            while let Some(t) = stack.pop() {
                if t == m { return true; }
                if !seen.insert(t) { continue; }
                stack.extend(tcx.trait_supers(prog, t));
            }
            false
        };
        if members.iter().copied().filter(|m| on_cycle(*m)).all(|m| m.0 >= td.0) {
            out.push(prog.record_error(def.file, ast.item(def.item).lo, "typeck",
                format!("E0391: cycle detected when computing the super predicates of `{}`", prog.def_path(td))));
        }
    }
    // `Self::X` in the arguments of a super bound whose trait (or its supertraits) defines an
    // associated type X: resolving it asks for exactly the bounds being lowered
    // `trait B: A<T: B>`: an associated type bounded by the trait being defined (implied predicates)
    for b in ast.list(*supers) {
        if let ast::Bound::Trait { path, .. } = b {
            for seg in ast.segs(path.segs) {
                let Some(ast::GenericArgs::Angle(v)) = ast.gargs(seg.args()) else { continue };
                for a in ast.list(*v) {
                    let ast::GenericArg::Constraint(_, _, cbs) = a else { continue };
                    for cb in ast.list(*cbs) {
                        if let ast::Bound::Trait { path: cp, .. } = cb {
                            if tcx.resolve_type_path_def(prog, def.file, def.scope, cp) == Some(td) {
                                out.push(prog.record_error(def.file, ast.item(def.item).lo, "typeck",
                                    format!("E0391: cycle detected when computing the implied predicates of `{}`", prog.def_path(td))));
                            }
                        }
                    }
                }
            }
        }
    }
    let mut self_assoc = false;
    for b in ast.list(*supers) {
        if let ast::Bound::Trait { path, .. } = b {
            let Some(bt) = tcx.resolve_type_path_def(prog, def.file, def.scope, path) else { continue };
            let mut names = Vec::new();
            for seg in ast.segs(path.segs) {
                if let Some(ast::GenericArgs::Angle(v)) = ast.gargs(seg.args()) {
                    for g in ast.list(*v) {
                        if let ast::GenericArg::Type(t) | ast::GenericArg::Binding(_, _, t) = g { self_assoc_names(ast, *t, 0, &mut names); }
                    }
                }
            }
            for n in names {
                let sym = prog.syms.get(prog.files[def.file as usize].text(n)).unwrap_or(u32::MAX);
                if tcx.trait_assoc(prog, bt, sym).is_some() { self_assoc = true; }
            }
        }
    }
    if self_assoc {
        out.push(prog.record_error(def.file, ast.item(def.item).lo, "typeck",
            format!("E0391: cycle detected when computing the super traits of `{}` with an associated type name", prog.def_path(td))));
    }
    out
}

/// The names of the associated types the type reaches through `Self` (`Self::Item`).
fn self_assoc_names(ast: &ast::Ast, t: ast::TyId, depth: u32, out: &mut Vec<ast::Ident>) {
    if depth > 16 { return; }
    match ast.ty(t) {
        ast::Ty::Path(p) => {
            let segs = ast.segs(p.segs);
            if segs.len() >= 2 && p.no_qself() && segs[0].kind() == ast::SegKind::SelfType { out.push(segs[1].name); }
            for s in segs {
                if let Some(ast::GenericArgs::Angle(v)) = ast.gargs(s.args()) {
                    for g in ast.list(*v) {
                        if let ast::GenericArg::Type(x) | ast::GenericArg::Binding(_, _, x) = g { self_assoc_names(ast, *x, depth + 1, out); }
                    }
                }
            }
        }
        ast::Ty::Ref(_, _, x) | ast::Ty::Ptr(_, x) | ast::Ty::Slice(x) | ast::Ty::Array(x, _) | ast::Ty::Paren(x) => self_assoc_names(ast, *x, depth + 1, out),
        ast::Ty::Tuple(v) => for x in ast.list(*v) { self_assoc_names(ast, *x, depth + 1, out); },
        _ => {}
    }
}

/// rustc_hir_analysis coherence/builtin.rs `visit_implementation_of_copy`: `impl Copy for T`
/// needs an ADT (E0206) whose every field is `Copy` (E0204) and that has no destructor (E0184).
pub fn copy_impl(prog: &Program, tcx: &Tcx, imp: DefId) -> Vec<String> {
    use crate::consts::GenericArg;
    use crate::next_solver::Goal;
    let Some(&td) = tcx.impl_trait.get(&imp) else { return Vec::new() };
    if tcx.copy_trait != Some(td) { return Vec::new(); }
    let def = prog.def(imp);
    let ast = &prog.files[def.file as usize].ast;
    let Some(item) = ast.as_impl(&ast.item(def.item).kind) else { return Vec::new() };
    if item.negative { return Vec::new(); }
    let indices = tcx.generic_indices_of(prog, imp);
    let trait_ref = tcx.impl_trait_ref(prog, imp, &indices.identity_args(&tcx.tys));
    let Some(GenericArg::Type(self_ty)) = trait_ref.args.first().copied() else { return Vec::new() };
    if tcx.tys.has_error(self_ty) { return Vec::new(); }
    let pos = ast.item(def.item).lo;
    let report = |code: &str, msg: &str| prog.record_error(def.file, pos, "typeck", format!("{}: {}", code, msg));
    let (adt, args) = match tcx.tys.kind(self_ty).clone() {
        TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Ptr(..) | TyKind::Never | TyKind::Ref(false, _, _) | TyKind::Array(..) => return Vec::new(),
        TyKind::Adt(d, args, _) => (d, tcx.tys.list(args).to_vec()),
        _ => return vec![report("E0206", "the trait `Copy` cannot be implemented for this type; type is not a structure or enumeration")],
    };
    let Some(def_adt) = tcx.adts.get(&adt) else { return Vec::new() };
    let env = tcx.param_env_id(prog, imp);
    let infcx = crate::infer::InferCtxt::for_item(tcx, prog, crate::tcx::TypingMode::non_body_analysis(), imp);
    for v in &def_adt.variants {
        for f in &v.fields {
            let ft = tcx.tys.subst(f.ty, &args);
            if tcx.tys.has_error(ft) { continue; }
            let predicate = crate::next_solver::collect::trait_predicate(td, vec![GenericArg::Type(ft)]);
            if !infcx.evaluate_obligation(prog, Goal { param_env: env, predicate }).must_apply_modulo_regions() {
                return vec![report("E0204", "the trait `Copy` cannot be implemented for this type")];
            }
        }
    }
    if tcx.adt_destructor(adt).is_some() {
        return vec![report("E0184", "the trait `Copy` cannot be implemented for this type; the type has a destructor")];
    }
    Vec::new()
}

/// rustc_resolve late resolution of generic parameter ribs: a type or const parameter named
/// like an earlier one of the same list, or like one of the enclosing trait or impl, is E0403.
/// (Lifetime parameters are checked by the lifetime resolution.)
pub fn duplicate_generic_params(prog: &Program, d: DefId) -> Vec<String> {
    use crate::ast::GenericParamKind;
    let def = prog.def(d);
    let ast = &prog.files[def.file as usize].ast;
    let Some(g) = ast.kind_generics(&ast.item(def.item).kind) else { return Vec::new() };
    let f = &prog.files[def.file as usize];
    let mut outer: Vec<&str> = Vec::new();
    let parent = prog.def(def.parent);
    if def.parent != NO_DEF && matches!(parent.kind, DefKind::Trait | DefKind::Impl) {
        let pf = &prog.files[parent.file as usize];
        if let Some(pg) = pf.ast.kind_generics(&pf.ast.item(parent.item).kind) {
            for p in pf.ast.list(pg.params) {
                if !matches!(p.kind, GenericParamKind::Lifetime(_)) { outer.push(pf.text(p.name)); }
            }
        }
    }
    let mut errors = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for p in ast.list(g.params) {
        if matches!(p.kind, GenericParamKind::Lifetime(_)) { continue; }
        let name = f.text(p.name);
        if seen.contains(&name) || outer.contains(&name) {
            errors.push(prog.record_error(def.file, p.name.lo, "resolve", format!("E0403: the name `{}` is already used for a generic parameter in this item's generic parameters", name)));
        }
        seen.push(name);
    }
    errors
}

/// rustc_ast_passes ast_validation validate_generic_param_order and the default rules: lifetime
/// parameters come before type and const parameters; defaults of type and const parameters are
/// trailing (structs, enums, unions, aliases, traits) and absent on fns and impls.
pub fn generic_param_rules(prog: &Program, d: DefId) -> Vec<String> {
    use crate::ast::{GenericParamKind, ItemKind};
    let def = prog.def(d);
    let ast = &prog.files[def.file as usize].ast;
    let item = ast.item(def.item);
    let Some(g) = ast.kind_generics(&item.kind) else { return Vec::new() };
    let params = ast.list(g.params);
    let mut errors = Vec::new();
    let mut seen_non_lifetime = false;
    let mut order_reported = false;
    for p in params {
        match p.kind {
            GenericParamKind::Lifetime(_) => {
                if seen_non_lifetime && !order_reported {
                    errors.push(prog.record_error(def.file, p.name.lo, "parse", "lifetime parameters must be declared prior to type and const parameters".to_string()));
                    order_reported = true;
                }
            }
            _ => seen_non_lifetime = true,
        }
    }
    // a const parameter's type is an integer, bool or char (adt_const_params lifts the rule)
    if let ItemKind::Fn(s, _, _) = item.kind {
        let sig = ast.fn_sig(s);
        if sig.konst && sig.asyncness {
            errors.push(prog.record_error(def.file, item.lo, "parse", "functions cannot be both `const` and `async`".to_string()));
        }
    }
    let krate = &prog.crates[def.krate as usize];
    if !krate.features(prog).has("adt_const_params") && !krate.features(prog).has("unsized_const_params") {
        let f = &prog.files[def.file as usize];
        for p in params {
            let GenericParamKind::Const(ty, _) = p.kind else { continue };
            let forbidden = match ast.ty(ty) {
                ast::Ty::Path(path) if path.no_qself() && path.segs.len() == 1 => {
                    let n = f.text(ast.segs(path.segs)[0].name);
                    match n {
                        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "bool" | "char" => false,
                        "f32" | "f64" | "str" => true,
                        _ => {
                            let sym = prog.syms.get(n).unwrap_or(u32::MAX);
                            !params.iter().any(|q| f.text(q.name) == n)
                                && prog.lookup_in_scope(prog.mods[def.scope as usize].normal, sym, true).is_some_and(|t| matches!(prog.def(t).kind, DefKind::Struct | DefKind::Enum | DefKind::Union))
                        }
                    }
                }
                ast::Ty::Ref(..) | ast::Ty::Tuple(_) | ast::Ty::Slice(_) | ast::Ty::Array(..) => true,
                _ => false,
            };
            if forbidden {
                let n = match ast.ty(ty) { ast::Ty::Path(path) => f.text(ast.segs(path.segs)[0].name).to_string(), _ => "this type".to_string() };
                errors.push(prog.record_error(def.file, p.name.lo, "parse", format!("`{}` is forbidden as the type of a const generic parameter", n)));
            }
        }
    }
    let has_default = |p: &crate::ast::GenericParam| matches!(p.kind, GenericParamKind::Type(_, Some(_)) | GenericParamKind::Const(_, Some(_)));
    if matches!(item.kind, ItemKind::Fn(..) | ItemKind::Impl(..)) {
        for p in params {
            if has_default(p) {
                errors.push(prog.record_error(def.file, p.name.lo, "parse", "defaults for generic parameters are not allowed here".to_string()));
            }
        }
    } else {
        let mut trailing_from = params.len();
        for (i, p) in params.iter().enumerate().rev() {
            if matches!(p.kind, GenericParamKind::Lifetime(_)) { continue; }
            if has_default(p) { trailing_from = i; } else { break; }
        }
        let mut reported = false;
        for (i, p) in params.iter().enumerate() {
            if has_default(p) && i < trailing_from && !reported && params[i + 1..].iter().any(|q| !matches!(q.kind, GenericParamKind::Lifetime(_)) && !has_default(q)) {
                errors.push(prog.record_error(def.file, p.name.lo, "parse", "generic parameters with a default must be trailing".to_string()));
                reported = true;
            }
        }
    }
    errors
}

/// rustc_hir_analysis `check_for_entry_fn` for a binary crate: `main` exists in the crate root
/// (E0601), is not `async` (E0752), has the type `fn()` (E0580), no generic parameters (E0131)
/// and no where clause (E0646). A crate declaring itself `#![no_main]` or a non-binary
/// `#![crate_type]` has no entry point to check.
pub fn entry_point(prog: &Program, root: u32) -> Vec<String> {
    use crate::ast::ItemKind;
    let c = &prog.crates[root as usize];
    let m = &prog.mods[c.root_mod as usize];
    let pf = &prog.files[m.file as usize];
    for a in pf.ast.root_attrs.iter() {
        let t = a.toks;
        if t.hi <= t.lo { continue; }
        match pf.tok_text(t.lo) {
            "no_main" | "no_core" => return Vec::new(),
            "crate_type" if t.hi == t.lo + 3 && pf.tok_text(t.lo + 2).trim_matches('"') != "bin" => return Vec::new(),
            _ => {}
        }
    }
    let Some(sym) = prog.syms.get("main") else { return vec![no_main_error(prog, root, m.file)] };
    // `main` bound by two globs that disagree: E0659 (rustc resolves the entry point by name)
    if let Some(e) = prog.name_ambiguity(c.root_mod, sym, false, false) {
        if let Some(msg) = crate::lint::privacy_diag(prog, m.file, 0, &e) { return vec![prog.record_error(m.file, 0, "resolve", msg)]; }
    }
    let main = match prog.lookup_in_module(c.root_mod, sym, false, &mut Vec::new()) {
        Some(d) if prog.def(d).kind == DefKind::Fn => d,
        _ => return vec![no_main_error(prog, root, m.file)],
    };
    let def = prog.def(main);
    let ast = &prog.files[def.file as usize].ast;
    let item = ast.item(def.item);
    let ItemKind::Fn(sig, _, _) = item.kind else { return Vec::new() };
    let sig = ast.fn_sig(sig);
    let at = item.name.lo;
    let err = |msg: &str| vec![prog.record_error(def.file, at, "typeck", msg.to_string())];
    if sig.asyncness {
        return err("E0752: `main` function is not allowed to be `async`");
    }
    let rust_abi = match sig.abi {
        None => true,
        Some(None) => false,
        Some(Some(name)) => prog.files[def.file as usize].text(name) == "\"Rust\"",
    };
    if !sig.params.is_empty() || sig.self_param.is_some() || sig.variadic || sig.unsafe_ || !rust_abi {
        return err("E0580: `main` function has wrong type");
    }
    if !sig.generics.params.is_empty() {
        return err("E0131: `main` function is not allowed to have generic parameters");
    }
    if !sig.generics.where_.is_empty() {
        return err("E0646: `main` function is not allowed to have a `where` clause");
    }
    Vec::new()
}

fn no_main_error(prog: &Program, root: u32, file: u32) -> String {
    let name = prog.syms.str(prog.crates[root as usize].name);
    prog.record_error(file, 0, "typeck", format!("E0601: `main` function not found in crate `{}`", name))
}

/// rustc_monomorphize: a binary exporting a second `main` symbol (`#[no_mangle]`, `#[export_name
/// = "main"]` or a foreign `main` declaration) clashes with the entry symbol.
pub fn entry_symbol_conflict(prog: &Program, own: &[bool], root: u32) -> Vec<String> {
    let Some(main) = entry_main(prog, root) else { return Vec::new() };
    // (`#![no_main]`: the program supplies the entry symbol itself)
    {
        let m = &prog.mods[prog.crates[root as usize].root_mod as usize];
        let pf = &prog.files[m.file as usize];
        if pf.ast.root_attrs.iter().any(|a| a.toks.hi > a.toks.lo && pf.tok_text(a.toks.lo) == "no_main") { return Vec::new(); }
    }
    let mut clash = false;
    for (i, def) in prog.defs.iter().enumerate() {
        if def.krate != root || !own.get(def.krate as usize).copied().unwrap_or(false) { continue; }
        let d = DefId(i as u32);
        match def.kind {
            DefKind::ForeignFn => clash |= prog.name(d) == "main",
            DefKind::Fn | DefKind::AssocFn | DefKind::Static => {
                if prog.item_defs.get(&(def.file, def.item.0)) != Some(&d) { continue; }
                let ast = &prog.files[def.file as usize].ast;
                let p = prog.src(def.file);
                for a in ast.list(ast.item(def.item).attrs) {
                    let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
                    if hi <= lo { continue; }
                    let first = if crate::cfg::tok_str(p, lo) == b"unsafe" && hi > lo + 2 { lo + 2 } else { lo };
                    match crate::cfg::tok_str(p, first) {
                        b"no_mangle" => clash |= prog.name(d) == "main",
                        b"export_name" if first + 2 < hi && crate::cfg::tok_str(p, first + 1) == b"=" => clash |= crate::cfg::tok_str(p, first + 2) == b"\"main\"",
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if !clash { return Vec::new(); }
    let def = prog.def(main);
    let at = prog.files[def.file as usize].ast.item(def.item).name.lo;
    vec![prog.record_error(def.file, at, "typeck", "entry symbol `main` declared multiple times".to_string())]
}

/// rustc_ast_passes `extern` block validation and the lints on them: `unsafe extern` blocks
/// (`unsafe_code`), plain `extern` blocks (`missing_unsafe_on_extern`, an error from edition
/// 2024), and a safety qualifier on an item of a plain block.
pub fn foreign_blocks(prog: &Program, own: &[bool]) -> Vec<String> {
    use crate::ast::ItemKind;
    let mut errors = Vec::new();
    let mut scope: crate::fxhash::HashMap<u32, DefId> = Default::default();
    for (i, d) in prog.defs.iter().enumerate() {
        if own.get(d.krate as usize).copied().unwrap_or(false) { scope.entry(d.file).or_insert(DefId(i as u32)); }
    }
    for (&file, &def) in &scope {
        let pf = &prog.files[file as usize];
        let ast = &pf.ast;
        // the blocks of the module tree that survive cfg stripping (the item arena also holds the
        // items of cfg'd-out code, which have no definitions)
        let mut stack: Vec<crate::ast::ItemId> = ast.root_items.iter().copied().collect();
        let mut blocks: Vec<&crate::ast::Item> = Vec::new();
        while let Some(id) = stack.pop() {
            let item = ast.item(id);
            let attrs: Vec<crate::ast::Attr> = ast.list(item.attrs).to_vec();
            if !prog.active(file, &attrs) { continue; }
            match item.kind {
                ItemKind::ForeignMod(_) => blocks.push(item),
                ItemKind::Mod(Some(l)) => stack.extend(ast.list(l).iter().copied()),
                _ => {}
            }
        }
        // and the non-empty blocks inside fn bodies that survive cfg stripping: their items have
        // definitions (the arena also holds those of cfg'd-out code, which do not)
        for item in ast.items.iter() {
            let ItemKind::ForeignMod(r) = item.kind else { continue };
            if blocks.iter().any(|b| b.lo == item.lo) { continue; }
            if ast.list(ast.row(r).items).iter().any(|it| prog.item_defs.contains_key(&(file, it.0))) { blocks.push(item); }
        }
        blocks.sort_by_key(|i| i.lo);
        for item in blocks {
            let ItemKind::ForeignMod(r) = item.kind else { continue };
            let fm = ast.row(r);
            let attrs: Vec<crate::ast::Attr> = ast.list(item.attrs).to_vec();
            // rustc_ast_lowering lower_abi: the block's ABI string names an ABI
            if let Some(a) = fm.abi {
                let text = pf.text(a);
                if !crate::types::Abi::is_rustc_name(&crate::lit::unescape_str(text)) {
                    errors.push(prog.record_error(file, a.lo, "typeck", format!("E0703: invalid ABI: found `{}`", crate::lit::unescape_str(text))));
                }
            }
            // #[link(name = "..", kind = "..")] (rustc_attr_parsing link_attrs.rs)
            {
                let sp = prog.src(file);
                for a in &attrs {
                    let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
                    if hi <= lo { continue; }
                    let from_source = (sp.tok(lo).lo as usize) < pf.src_len as usize;
                    // the argument shapes of the attributes on an extern block (`link_name` here is a warning
                    // target, its malformed form still an error)
                    if from_source && crate::cfg::tok_str(sp, lo) == b"link_name" {
                        if let Some(m) = attribute_args_error(sp, lo, hi, b"link_name") {
                            errors.push(prog.record_error(file, sp.tok(m.0).lo, "typeck", format!("{}: malformed `link_name` attribute input", m.1)));
                        }
                    }
                    if crate::cfg::tok_str(sp, lo) == b"link" && (hi < lo + 3 || crate::cfg::tok_str(sp, lo + 1) != b"(") {
                        errors.push(prog.record_error(file, sp.tok(lo).lo, "typeck", "E0539: malformed `link` attribute input".to_string()));
                        continue;
                    }
                    if hi < lo + 3 || crate::cfg::tok_str(sp, lo) != b"link" || crate::cfg::tok_str(sp, lo + 1) != b"(" { continue; }
                    let pos = sp.tok(lo).lo;
                    // each key once (E0538); `modifiers` name each modifier once; `framework` only on Apple targets (E0455)
                    {
                        let mut seen: Vec<&[u8]> = Vec::new();
                        let mut i = lo + 2;
                        let mut depth = 0i32;
                        while i + 1 < hi {
                            let t = crate::cfg::tok_str(sp, i);
                            match t {
                                b"(" | b"[" | b"{" => depth += 1,
                                b")" | b"]" | b"}" => depth -= 1,
                                _ => {}
                            }
                            if depth == 0 && crate::cfg::tok_str(sp, i + 1) == b"=" && t.first().is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') {
                                if seen.contains(&t) {
                                    errors.push(prog.record_error(file, sp.tok(lo).lo, "typeck", "E0538: malformed `link` attribute input".to_string()));
                                }
                                seen.push(t);
                                if t == b"modifiers" && i + 2 < hi {
                                    if let Some(v) = crate::lint_early::string_value(crate::cfg::tok_str(sp, i + 2)) {
                                        let mut names: Vec<&str> = Vec::new();
                                        for m in v.split(',') {
                                            let n = m.trim_start_matches(['+', '-']);
                                            if names.contains(&n) {
                                                errors.push(prog.record_uncoded_error(file, sp.tok(i).lo, "typeck", format!("multiple `{}` modifiers in a single `modifiers` argument", n)));
                                            }
                                            names.push(n);
                                        }
                                    }
                                }
                                if t == b"kind" && crate::cfg::tok_str(sp, i + 2) == b"\"framework\"" {
                                    let target = &prog.session.options.target;
                                    let apple = if target.is_empty() { cfg!(target_vendor = "apple") } else { target.contains("apple") };
                                    if !apple {
                                        errors.push(prog.record_error(file, sp.tok(lo).lo, "typeck", "E0455: link kind `framework` is only supported on Apple targets".to_string()));
                                    }
                                }
                            }
                            i += 1;
                        }
                    }
                    let (mut name, mut kind_bad, mut wasm_module) = (None::<&[u8]>, false, false);
                    let mut i = lo + 2;
                    while i + 2 < hi {
                        let key = crate::cfg::tok_str(sp, i);
                        if crate::cfg::tok_str(sp, i + 1) == b"=" {
                            let val = crate::cfg::tok_str(sp, i + 2);
                            if key == b"name" { name = Some(val); }
                            if key == b"wasm_import_module" { wasm_module = true; }
                            if key == b"kind" && !matches!(val, b"\"static\"" | b"\"dylib\"" | b"\"framework\"" | b"\"raw-dylib\"" | b"\"link-arg\"") { kind_bad = true; }
                            i += 3;
                        } else { i += 1; }
                    }
                    if kind_bad { errors.push(prog.record_error(file, pos, "typeck", "E0539: malformed `link` attribute input".to_string())); }
                    match name {
                        None if wasm_module => {}
                        None => errors.push(prog.record_error(file, pos, "typeck", "E0459: `#[link]` attribute requires a `name = \"string\"` argument".to_string())),
                        Some(b"\"\"") => errors.push(prog.record_error(file, pos, "typeck", "E0454: link name must not be empty".to_string())),
                        _ => {}
                    }
                }
            }
            // (declarations of an extern block, plain or unsafe)
            for &it in ast.list(fm.items) {
                if let ItemKind::Const(_, init) = ast.item(it).kind {
                    errors.push(prog.record_error(file, ast.item(it).lo, "parse", "extern items cannot be `const`".to_string()));
                    if init.is_some() {
                        errors.push(prog.record_error(file, ast.item(it).lo, "parse", "incorrect `static` inside `extern` block".to_string()));
                    }
                }
                {
                    // `#[link_ordinal(N)]`: N fits 16 bits, and the block links as `raw-dylib`
                    let sp = prog.src(file);
                    for a in ast.list(ast.item(it).attrs) {
                        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
                        if hi > lo + 3 && crate::cfg::tok_str(sp, lo) == b"link_ordinal" && crate::cfg::tok_str(sp, lo + 1) == b"(" {
                            let n = crate::cfg::tok_str(sp, lo + 2);
                            if std::str::from_utf8(n).ok().and_then(|n| n.replace('_', "").parse::<u128>().ok()).is_some_and(|v| v > u16::MAX as u128) {
                                errors.push(prog.record_uncoded_error(file, sp.tok(lo).lo, "typeck", format!("ordinal value in `link_ordinal` is too large: `{}`", String::from_utf8_lossy(n))));
                                continue;
                            }
                            // a block with `#[link(..)]` whose kind is not raw-dylib
                            let mut linked = false;
                            let mut raw = false;
                            for b in &attrs {
                                let (blo, bhi) = (b.toks.lo as usize, b.toks.hi as usize);
                                if bhi > blo + 1 && crate::cfg::tok_str(sp, blo) == b"link" && crate::cfg::tok_str(sp, blo + 1) == b"(" {
                                    linked = true;
                                    for k in blo..bhi.saturating_sub(2) {
                                        if crate::cfg::tok_str(sp, k) == b"kind" && crate::cfg::tok_str(sp, k + 2) == b"\"raw-dylib\"" { raw = true; }
                                    }
                                }
                            }
                            if linked && !raw {
                                errors.push(prog.record_uncoded_error(file, sp.tok(lo).lo, "typeck", "`#[link_ordinal]` is only supported if link kind is `raw-dylib`".to_string()));
                            }
                        }
                    }
                }
                {
                    // `#[track_caller]` on an item of a block whose ABI is not "Rust" (E0737)
                    let abi = fm.abi.map_or("C".to_string(), |a| crate::lit::unescape_str(pf.text(a)));
                    let sp = prog.src(file);
                    for a in ast.list(ast.item(it).attrs) {
                        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
                        if hi > lo && crate::cfg::tok_str(sp, lo) == b"track_caller" && abi != "Rust" && matches!(ast.item(it).kind, ItemKind::Fn(..)) {
                            errors.push(prog.record_error(file, sp.tok(lo).lo, "typeck", "E0737: `#[track_caller]` can only be used with the Rust ABI".to_string()));
                        }
                    }
                }
                if let ItemKind::Fn(sig, body, _) = ast.item(it).kind {
                    let sig = ast.fn_sig(sig);
                    if body.is_some() || sig.lazy_body.is_some() {
                        errors.push(prog.record_error(file, ast.item(it).lo, "parse", "incorrect function inside `extern` block".to_string()));
                    }
                    // check_abi: a C-variadic foreign function has a C-like calling convention
                    if sig.variadic {
                        let abi = fm.abi.map_or(&b"\"C\""[..], |a| pf.bytes(a.lo, a.hi));
                        let abi = abi.strip_prefix(b"\"").and_then(|a| a.strip_suffix(b"\"")).unwrap_or(abi);
                        if !matches!(abi, b"C" | b"C-unwind" | b"cdecl" | b"cdecl-unwind" | b"system" | b"system-unwind" | b"aapcs" | b"aapcs-unwind" | b"win64" | b"win64-unwind" | b"sysv64" | b"sysv64-unwind" | b"efiapi") {
                            errors.push(prog.record_error(file, ast.item(it).lo, "typeck", format!("E0045: C-variadic functions with the \"{}\" calling convention are not supported", String::from_utf8_lossy(abi))));
                        }
                    }
                    if let Some(g) = ast.list(sig.generics.params).iter().find(|g| !matches!(g.kind, crate::ast::GenericParamKind::Lifetime(_))) {
                        let what = if matches!(g.kind, crate::ast::GenericParamKind::Const(..)) { "const" } else { "type" };
                        errors.push(prog.record_error(file, g.name.lo, "typeck", format!("E0044: foreign items may not have {} parameters", what)));
                    }
                    for prm in ast.list(sig.params) {
                        if !matches!(ast.pat(prm.pat), crate::ast::Pat::Wild | crate::ast::Pat::Ident { by_ref: false, mutbl: false, sub: None, .. }) {
                            errors.push(prog.record_error(file, ast.item(it).lo, "typeck", "E0130: patterns aren't allowed in foreign function declarations".to_string()));
                        }
                    }
                }
            }
            if fm.unsafe_ != 0 {
                crate::lint::emit_in(prog, def, &attrs, &crate::lint::UNSAFE_CODE, file, item.lo, "usage of an `unsafe extern` block");
                continue;
            }
            if pf.edition >= 2024 {
                errors.push(prog.record_uncoded_error(file, item.lo, "typeck", "extern blocks must be unsafe".to_string()));
            } else {
                crate::lint::emit_in(prog, def, &attrs, &crate::lint::MISSING_UNSAFE_ON_EXTERN, file, item.lo, "extern blocks should be unsafe");
            }
            for &it in ast.list(fm.items) {
                // (a foreign fn or static is unsafe by default: look for the written qualifier)
                if let ItemKind::Fn(..) | ItemKind::Static(..) = ast.item(it).kind {
                    let written = pf.bytes(ast.item(it).lo, ast.item(it).name.lo);
                    if written.split(|c| !c.is_ascii_alphanumeric() && *c != b'_').any(|w| w == b"unsafe" || w == b"safe") {
                        errors.push(prog.record_uncoded_error(file, ast.item(it).lo, "typeck", "items in `extern` blocks without an `unsafe` qualifier cannot have safety qualifiers".to_string()));
                    }
                }
            }
        }
    }
    errors
}

/// rustc_attr_parsing: the argument shapes of `inline`, `must_use`, `deprecated`, `link_name` and
/// `link_ordinal`. Some((token, code)): the token to blame and the code (E0539 malformed, E0538
/// repeated key, E0565 literal where a key is expected, E0805 wrong number of arguments).
fn attribute_args_error(p: crate::cfg::Src<'_>, lo: usize, hi: usize, name: &[u8]) -> Option<(usize, &'static str)> {
    use crate::cfg::tok_str;
    let t = |i: usize| if i < hi { tok_str(p, i) } else { b"" };
    let is_str = |i: usize| { let s = t(i); s.starts_with(b"\"") || s.starts_with(b"r\"") || s.starts_with(b"r#") };
    let is_int = |i: usize| t(i).first().is_some_and(|c| c.is_ascii_digit());
    let is_word = |i: usize| t(i).first().is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_');
    let n = lo + 1;
    let list = t(n) == b"(" && hi >= n + 2 && t(hi - 1) == b")";
    let mut entries: Vec<(usize, usize)> = Vec::new();
    if list {
        let (mut start, mut depth) = (n + 1, 0i32);
        for i in n + 1..hi - 1 {
            match t(i) {
                b"(" | b"[" | b"{" => depth += 1,
                b")" | b"]" | b"}" => depth -= 1,
                b"," if depth == 0 => { entries.push((start, i)); start = i + 1; }
                _ => {}
            }
        }
        if start < hi - 1 { entries.push((start, hi - 1)); }
    }
    // `NoArgsAttributeParser`s (+ `global_allocator`): any argument is malformed input
    const NO_ARGS: &[&[u8]] = &[b"non_exhaustive", b"no_core", b"no_std", b"no_main", b"panic_runtime", b"needs_panic_runtime", b"profiler_runtime", b"no_builtins",
        b"default_lib_allocator", b"may_dangle", b"coroutine", b"no_implicit_prelude", b"pin_v2", b"automatically_derived", b"macro_escape", b"allow_internal_unsafe",
        b"no_link", b"cold", b"track_caller", b"no_mangle", b"thread_local", b"marker", b"fundamental", b"export_stable", b"ffi_const", b"ffi_pure",
        b"needs_allocator", b"compiler_builtins", b"proc_macro", b"proc_macro_attribute", b"panic_handler", b"global_allocator"];
    if n < hi && NO_ARGS.contains(&name) { return Some((lo, "E0565")); }
    match name {
        b"inline" if list => {
            if entries.len() != 1 { return Some((lo, "E0805")); }
            let (a, b) = entries[0];
            if !(b == a + 1 && matches!(t(a), b"always" | b"never")) { return Some((a, "E0539")); }
        }
        b"must_use" if list => return Some((lo, "E0539")),
        b"link_name" if list || t(n) != b"=" => return Some((lo, "E0539")),
        b"link_ordinal" => {
            if !list { return Some((lo, "E0539")); }
            if entries.len() != 1 { return Some((lo, "E0805")); }
            let (a, b) = entries[0];
            if !(b == a + 1 && is_int(a)) { return Some((a, "E0539")); }
        }
        b"deprecated" => {
            if t(n) == b"=" {
                if !(hi == n + 2 && is_str(n + 1)) { return Some((n + 1, "E0539")); }
            } else if list {
                let mut seen: Vec<&[u8]> = Vec::new();
                for &(a, b) in &entries {
                    if b == a + 1 && is_str(a) { return Some((a, "E0565")); }
                    let key = t(a);
                    if !is_word(a) || !matches!(key, b"since" | b"note" | b"suggestion") { return Some((a, "E0539")); }
                    if !(b == a + 3 && t(a + 1) == b"=" && is_str(a + 2)) { return Some((a, "E0539")); }
                    if seen.contains(&key) { return Some((a, "E0538")); }
                    seen.push(key);
                }
            }
        }
        _ => {}
    }
    None
}

/// rustc_attr_parsing `check_attribute_safety`, on the attributes of an item that survived cfg
/// stripping. An unsafe builtin attribute (`no_mangle`, `export_name`, `link_section`; also
/// `naked`, `force_target_feature`, `ffi_const`, `ffi_pure`) is written `unsafe(..)` (an error
/// from edition 2024 for the first three, else the allow-by-default lint
/// `unsafe_attr_outside_unsafe`); a builtin attribute that is not unsafe is not (`unsafe(cfg)`).
/// The edition is the attribute path's: attributes a macro expansion wrote are not judged (the
/// expansion's tokens carry no edition).
pub fn attribute_safety(prog: &Program, d: DefId) -> Vec<String> {
    use crate::builtin_attrs::{attr_safety, AttrSafety};
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    let ast = &pf.ast;
    let p = prog.src(def.file);
    let mut errors = Vec::new();
    for a in ast.list(ast.item(def.item).attrs) {
        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
        if hi <= lo { continue; }
        let from_source = (p.tok(lo).lo as usize) < pf.src_len as usize;
        let wrapped = crate::cfg::tok_str(p, lo) == b"unsafe" && hi > lo + 1 && crate::cfg::tok_str(p, lo + 1) == b"(";
        let first = if wrapped { lo + 2 } else { lo };
        // the attribute's path, compared as bytes
        let mut name: Vec<u8> = Vec::new();
        let mut i = first;
        while i < hi && !matches!(crate::cfg::tok_str(p, i), b"(" | b"=" | b")" | b",") {
            name.extend_from_slice(crate::cfg::tok_str(p, i));
            i += 1;
        }
        let pos = p.tok(lo).lo;
        if wrapped {
            if attr_safety(&String::from_utf8_lossy(&name)) == AttrSafety::Normal {
                errors.push(prog.record_uncoded_error(def.file, pos, "typeck", format!("`{}` is not an unsafe attribute", String::from_utf8_lossy(&name))));
            }
            continue;
        }
        if from_source && matches!(name.as_slice(), b"track_caller" | b"target_feature") && def.kind == DefKind::Fn && prog.syms.str(def.name) == "main" && prog.def(def.parent).kind == DefKind::Mod && prog.mods[prog.def(def.parent).sub as usize].parent_mod.is_none() && crate::wfcheck::obligations::is_entry_main(prog, d) {
            let what = if name == b"track_caller" { "be `#[track_caller]`" } else { "have `#[target_feature]`" };
            errors.push(prog.record_uncoded_error(def.file, pos, "typeck", format!("`main` function is not allowed to {what}")));
        }
        if from_source && name == b"deprecated" && ast.list(ast.item(def.item).attrs).iter().take_while(|b| b.toks.lo != a.toks.lo).any(|b| b.toks.hi > b.toks.lo && crate::cfg::tok_str(p, b.toks.lo as usize) == b"deprecated") {
            errors.push(prog.record_uncoded_error(def.file, pos, "typeck", "multiple `deprecated` attributes".to_string()));
        }
        if from_source && name == b"macro_use" && hi > lo + 1 && crate::cfg::tok_str(p, lo + 1) == b"(" && !matches!(ast.item(def.item).kind, crate::ast::ItemKind::ExternCrate(_)) {
            errors.push(prog.record_uncoded_error(def.file, pos, "typeck", "arguments to `macro_use` are not allowed here".to_string()));
        }
        // check_attr.rs check_instruction_set: `arm::*` instruction sets are for the arm targets
        if from_source && name == b"instruction_set" && hi > first + 2 && crate::cfg::tok_str(p, first + 1) == b"(" && crate::cfg::tok_str(p, first + 2) == b"arm" {
            let target = &prog.session.options.target;
            let arch = if target.is_empty() { std::env::consts::ARCH } else { target.split('-').next().unwrap_or("") };
            if !arch.starts_with("arm") && arch != "thumbv7em" {
                let triple = if target.is_empty() { format!("{}-unknown-{}-gnu", arch, std::env::consts::OS) } else { target.clone() };
                errors.push(prog.record_uncoded_error(def.file, pos, "typeck", format!("target `{}` does not support `#[instruction_set(arm::*)]`", triple)));
            }
        }
        if from_source && name == b"export_name" && hi > first + 2 && crate::cfg::tok_str(p, first + 1) == b"=" {
            if let Some(v) = crate::lint_early::string_value(crate::cfg::tok_str(p, first + 2)) {
                if v.contains('\0') {
                    errors.push(prog.record_error(def.file, pos, "typeck", "E0648: `export_name` may not contain null characters".to_string()));
                } else if v.is_empty() {
                    errors.push(prog.record_uncoded_error(def.file, pos, "typeck", "`export_name` may not be empty".to_string()));
                }
            }
        }
        // ast_validation check_nomangle_item_asciionly (E0754) and `#[track_caller]` needing the Rust ABI (E0737)
        if from_source && name == b"no_mangle" && !prog.name(d).is_ascii() {
            errors.push(prog.record_error(def.file, ast.item(def.item).lo, "typeck", "E0754: `#[no_mangle]` requires ASCII identifier".to_string()));
        }
        if from_source && name == b"track_caller" {
            if let crate::ast::ItemKind::Fn(sig, _, _) = ast.item(def.item).kind {
                let abi = match ast.fn_sig(sig).abi { None => None, Some(None) => Some("C".to_string()), Some(Some(a)) => Some(crate::lit::unescape_str(pf.text(a))) };
                if abi.is_some_and(|a| a != "Rust") {
                    errors.push(prog.record_error(def.file, pos, "typeck", "E0737: `#[track_caller]` can only be used with the Rust ABI".to_string()));
                }
            }
        }
        if from_source {
            if let Some(m) = attribute_args_error(p, lo, hi, &name) {
                errors.push(prog.record_error(def.file, p.tok(m.0).lo, "typeck", format!("{}: malformed `{}` attribute input", m.1, String::from_utf8_lossy(&name))));
            }
            if let Some(what) = attribute_target_error(prog, d, &name) {
                errors.push(prog.record_uncoded_error(def.file, pos, "typeck", format!("`#[{}]` attribute cannot be used on {}", String::from_utf8_lossy(&name), what)));
            }
        }
        let AttrSafety::Unsafe { since_2024 } = attr_safety(&String::from_utf8_lossy(&name)) else { continue };
        if !since_2024 || pf.edition >= 2024 {
            if from_source { errors.push(prog.record_uncoded_error(def.file, pos, "typeck", "unsafe attribute used without unsafe".to_string())); }
        } else {
            crate::lint::emit(prog, d, &crate::lint::UNSAFE_ATTR_OUTSIDE_UNSAFE, def.file, pos, "unsafe attribute used without unsafe");
        }
    }
    errors
}

/// check_union_fields: a union field is `Copy` or otherwise guaranteed not to need dropping
/// (rustc `BikeshedGuaranteedNoDrop`: `ManuallyDrop<_>`, references, tuples and arrays of such)
/// -- E0740.
pub fn union_fields(prog: &Program, tcx: &Tcx, d: DefId) -> Vec<String> {
    use crate::consts::GenericArg;
    use crate::next_solver::Goal;
    let Some(adt) = tcx.adts.get(&d) else { return Vec::new() };
    if !adt.is_union { return Vec::new(); }
    let Some(copy) = tcx.copy_trait else { return Vec::new() };
    let env = tcx.param_env_id(prog, d);
    let infcx = crate::infer::InferCtxt::for_item(tcx, prog, crate::tcx::TypingMode::non_body_analysis(), d);
    fn no_drop(prog: &Program, tcx: &Tcx, infcx: &crate::infer::InferCtxt, env: crate::tcx::ParamEnvId, copy: DefId, t: TyId, depth: u32) -> bool {
        if depth > 16 { return true; }
        match tcx.tys.kind(t).clone() {
            TyKind::Error => true,
            TyKind::Ref(..) | TyKind::Ptr(..) => true,
            // (Relax's ManuallyDrop is an ordinary union of core)
            TyKind::Adt(d, _, _) if prog.name(d) == "ManuallyDrop" && matches!(prog.syms.str(prog.crates[prog.def(d).krate as usize].name), "core" | "std") => true,
            TyKind::Tuple(l) => tcx.tys.list(l).iter().all(|&x| no_drop(prog, tcx, infcx, env, copy, x, depth + 1)),
            TyKind::Array(e, _) => no_drop(prog, tcx, infcx, env, copy, e, depth + 1),
            _ => {
                let t = tcx.try_normalize_erasing_regions(prog, crate::tcx::TypingEnv::fully_monomorphized(prog), t).unwrap_or(t);
                let predicate = crate::next_solver::collect::trait_predicate(copy, vec![GenericArg::Type(t)]);
                infcx.evaluate_obligation(prog, Goal { param_env: env, predicate }).may_apply()
            }
        }
    }
    let def = prog.def(d);
    let ast = &prog.files[def.file as usize].ast;
    for f in &adt.variants[0].fields {
        if !no_drop(prog, tcx, &infcx, env, copy, f.ty, 0) {
            return vec![prog.record_error(def.file, ast.item(def.item).name.lo, "typeck", "E0740: field must implement `Copy` or be wrapped in `ManuallyDrop<...>` to be used in a union".to_string())];
        }
    }
    Vec::new()
}

/// rustc_passes check_attr.rs `check_repr` (and the packed/align checks of check/check.rs):
/// the `#[repr(..)]` hints of an item against its kind and against each other.
pub fn repr_checks(prog: &Program, tcx: &Tcx, d: DefId) -> Vec<String> {
    use crate::ast::ItemKind;
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    let ast = &pf.ast;
    let p = prog.src(def.file);
    let item = ast.item(def.item);
    let mut errors = Vec::new();
    let mut err = |pos: u32, msg: String| errors.push(prog.record_error(def.file, pos, "typeck", msg));
    const INTS: &[&[u8]] = &[b"u8", b"u16", b"u32", b"u64", b"u128", b"usize", b"i8", b"i16", b"i32", b"i64", b"i128", b"isize"];
    let (is_struct, is_enum, is_union) = (matches!(item.kind, ItemKind::Struct(_)), matches!(item.kind, ItemKind::Enum(_)), matches!(item.kind, ItemKind::Union(_)));
    let mut ints: Vec<Vec<u8>> = Vec::new();
    let (mut c, mut transparent, mut packed, mut aligned, mut other) = (false, false, Vec::<u32>::new(), false, false);
    let mut first_pos = item.lo;
    let mut first_repr: Option<u32> = None;
    for a in ast.list(item.attrs) {
        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
        if hi <= lo || crate::cfg::tok_str(p, lo) != b"repr" { continue; }
        let pos = p.tok(lo).lo;
        first_pos = pos;
        first_repr.get_or_insert(pos);
        if hi < lo + 3 || crate::cfg::tok_str(p, lo + 1) != b"(" {
            err(pos, "E0539: malformed `repr` attribute input".to_string());
            continue;
        }
        let mut i = lo + 2;
        while i < hi {
            let name = crate::cfg::tok_str(p, i);
            if name == b")" { break; }
            if name == b"," { i += 1; continue; }
            let hpos = p.tok(i).lo;
            // a literal where a hint belongs (`repr("C")`)
            if matches!(p.tok(i).kind, crate::lexer::T::Str | crate::lexer::T::RawStr | crate::lexer::T::Int | crate::lexer::T::Float | crate::lexer::T::Char) {
                err(hpos, "E0565: meta item in `repr` must be an identifier".to_string());
                i += 1;
                continue;
            }
            i += 1;
            // the hint's argument: `(..)` or `= ..`
            let mut args: Vec<&[u8]> = Vec::new();
            let mut assign = false;
            if i < hi && crate::cfg::tok_str(p, i) == b"(" {
                i += 1;
                while i < hi && crate::cfg::tok_str(p, i) != b")" { args.push(crate::cfg::tok_str(p, i)); i += 1; }
                i += 1;
            } else if i < hi && crate::cfg::tok_str(p, i) == b"=" {
                assign = true;
                i += 2;
            }
            match name {
                b"C" => { c = true; if !(is_struct || is_enum || is_union) { err(hpos, "E0517: attribute should be applied to a struct, enum, or union".to_string()); } }
                b"transparent" => {
                    transparent = true;
                    if !(is_struct || is_enum || is_union) { err(hpos, "E0517: attribute should be applied to a struct, enum, or union".to_string()); }
                }
                b"packed" => {
                    packed.push(args.first().and_then(|a| std::str::from_utf8(a).ok()).and_then(|a| a.parse().ok()).unwrap_or(1));
                    if !(is_struct || is_union) { err(hpos, "E0517: attribute should be applied to a struct or union".to_string()); }
                }
                b"align" => {
                    aligned = true;
                    // check_repr: `align` is for structs, enums and unions (fn/static align are feature-gated)
                    if args.is_empty() && !assign {
                        err(hpos, "E0589: invalid `repr(align)` attribute: `align` needs an argument".to_string());
                    } else if !(is_struct || is_enum || is_union) {
                        err(hpos, "E0517: attribute should be applied to a struct, enum, or union".to_string());
                    }
                    if assign {
                        err(hpos, "E0693: incorrect `repr(align)` attribute format".to_string());
                    } else if args.len() == 1 {
                        let t = args[0];
                        match std::str::from_utf8(t).ok().filter(|s| crate::const_fold::int_suffix(s).is_none()).and_then(|s| crate::const_fold::parse_int_lit(s)).map(|v| v as u128) {
                            None => err(hpos, "E0589: invalid `repr(align)` attribute: not an unsuffixed integer".to_string()),
                            Some(v) if v == 0 || !v.is_power_of_two() => err(hpos, "E0589: invalid `repr(align)` attribute: not a power of two".to_string()),
                            Some(v) if v > (1 << 29) => err(hpos, "E0589: invalid `repr(align)` attribute: larger than 2^29".to_string()),
                            _ => {}
                        }
                    }
                }
                n if INTS.contains(&n) => {
                    if !ints.iter().any(|x| x == n) { ints.push(n.to_vec()); }
                    if !is_enum { err(hpos, "E0517: attribute should be applied to an enum".to_string()); }
                }
                b"Rust" => { if !(is_struct || is_enum || is_union) { err(hpos, "E0517: attribute should be applied to a struct, enum, or union".to_string()); } }
                b"simd" => { other = true; if !is_struct { err(hpos, "E0517: attribute should be applied to a struct".to_string()); } }
                _ => {}
            }
        }
    }
    let _ = other;
    // check_enum: a zero-variant enum takes no representation
    if let (ItemKind::Enum(r), Some(pos)) = (item.kind, first_repr) {
        if ast.list(ast.row(r).variants).is_empty() { err(pos, "E0084: unsupported representation for zero-variant enum".to_string()); }
    }
    // check_enum: explicit discriminants next to non-unit variants need a primitive representation (E0732)
    if let ItemKind::Enum(r) = item.kind {
        if ints.is_empty() {
            let vs = ast.list(ast.row(r).variants);
            let unit = |v: &crate::ast::Variant| matches!(v.data, crate::ast::VariantData::Unit);
            let non_unit = vs.iter().any(|v| !unit(v));
            let disr_non_unit = vs.iter().any(|v| !unit(v) && v.disc.is_some());
            let disr_unit = vs.iter().any(|v| unit(v) && v.disc.is_some());
            if disr_non_unit || (disr_unit && non_unit) {
                err(item.name.lo, "E0732: `#[repr(inttype)]` must be specified for enums with explicit discriminants and non-unit variants".to_string());
            }
        }
    }
    if transparent && (c || !packed.is_empty() || aligned || !ints.is_empty()) {
        err(first_pos, "E0692: transparent struct cannot have other repr hints".to_string());
    }
    if transparent && is_enum {
        if let ItemKind::Enum(r) = item.kind {
            let n = ast.list(ast.row(r).variants).len();
            if n != 1 { err(item.name.lo, format!("E0731: transparent enum needs exactly one variant, but has {}", n)); }
        }
    }
    if ints.len() > 1 { err(first_pos, "E0566: conflicting representation hints".to_string()); }
    // `repr(C, u64)` on a fieldless enum
    if c && !ints.is_empty() && is_enum {
        if let Some(adt) = tcx.adts.get(&d) {
            if adt.variants.iter().all(|v| v.fields.is_empty()) { err(first_pos, "E0566: conflicting representation hints".to_string()); }
        }
    }
    if !packed.is_empty() && aligned { err(first_pos, "E0587: type has conflicting packed and align representation hints".to_string()); }
    if packed.len() > 1 && packed.iter().any(|x| *x != packed[0]) { err(first_pos, "E0634: type has conflicting packed representation hints".to_string()); }
    // a packed type holds no `repr(align)` type, however deep
    if !packed.is_empty() && (is_struct || is_union) {
        fn holds_align(tcx: &Tcx, t: TyId, seen: &mut Vec<DefId>, depth: u32) -> bool {
            if depth > 16 { return false; }
            match tcx.tys.kind(t).clone() {
                TyKind::Adt(d, args, _) => {
                    if seen.contains(&d) { return false; }
                    seen.push(d);
                    let Some(a) = tcx.adts.get(&d) else { return false };
                    if a.repr_align > 0 { return true; }
                    let args = tcx.tys.list(args).to_vec();
                    a.variants.iter().any(|v| v.fields.iter().any(|f| holds_align(tcx, tcx.tys.subst(f.ty, &args), seen, depth + 1)))
                }
                TyKind::Tuple(l) => tcx.tys.list(l).iter().any(|&x| holds_align(tcx, x, seen, depth + 1)),
                TyKind::Array(e, _) => holds_align(tcx, e, seen, depth + 1),
                _ => false,
            }
        }
        if let Some(adt) = tcx.adts.get(&d) {
            let mut seen = vec![d];
            if adt.variants.iter().any(|v| v.fields.iter().any(|f| holds_align(tcx, f.ty, &mut seen, 0))) {
                err(item.name.lo, "E0588: packed type cannot transitively contain a `#[repr(align)]` type".to_string());
            }
        }
    }
    // a variant takes no repr
    if let ItemKind::Enum(r) = item.kind {
        for v in ast.list(ast.row(r).variants) {
            for a in ast.list(v.attrs) {
                if a.toks.hi > a.toks.lo && crate::cfg::tok_str(p, a.toks.lo as usize) == b"repr" {
                    err(p.tok(a.toks.lo as usize).lo, "E0517: attribute should be applied to an enum".to_string());
                }
            }
        }
    }
    errors
}

/// rustc_resolve late.rs: an associated constant of an impl whose header names an anonymous
/// lifetime (`impl Foo<'_>`) with an elided `&` in its type -- the elided lifetime means
/// `'static` there (lint `elided_lifetimes_in_associated_constant`).
pub fn assoc_const_elision(prog: &Program, d: DefId) -> Vec<String> {
    use crate::ast::{GenericArg, GenericArgs, ItemKind, Ty};
    if !crate::lint::enabled() { return Vec::new(); }
    let def = prog.def(d);
    if def.kind != DefKind::AssocConst || def.parent == NO_DEF || prog.def(def.parent).kind != DefKind::Impl { return Vec::new(); }
    let ast = &prog.files[def.file as usize].ast;
    let pf = &prog.files[def.file as usize];
    let ItemKind::Const(oty, _) = ast.item(def.item).kind else { return Vec::new() };
    let Some(ty) = oty.get() else { return Vec::new() };
    let imp = prog.def(def.parent);
    let iast = &prog.files[imp.file as usize].ast;
    let Some(im) = iast.as_impl(&iast.item(imp.item).kind) else { return Vec::new() };
    // an anonymous lifetime written in a type
    fn anon<'t>(ast: &crate::ast::Ast, text: &dyn Fn(crate::ast::Ident) -> &'t str, t: crate::ast::TyId, refs_too: bool, depth: u32) -> bool {
        if depth > 16 { return false; }
        let args = |gs: crate::ast::GenericArgsId, depth: u32| -> bool {
            match ast.gargs(gs) {
                Some(GenericArgs::Angle(v)) => ast.list(*v).iter().any(|a| match a {
                    GenericArg::Lifetime(l) => text(*l) == "'_",
                    GenericArg::Type(x) => anon(ast, text, *x, refs_too, depth + 1),
                    _ => false,
                }),
                _ => false,
            }
        };
        match ast.ty(t) {
            Ty::Ref(lt, _, x) => (match lt { None => refs_too, Some(l) => text(*l) == "'_" }) || anon(ast, text, *x, refs_too, depth + 1),
            Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => anon(ast, text, *x, refs_too, depth + 1),
            Ty::Tuple(l) => ast.list(*l).iter().any(|x| anon(ast, text, *x, refs_too, depth + 1)),
            Ty::Path(p) => ast.segs(p.segs).iter().any(|s| args(s.args(), depth)),
            _ => false,
        }
    }
    let itext = |i: crate::ast::Ident| -> &str { std::str::from_utf8(prog.files[imp.file as usize].bytes(i.lo, i.hi)).unwrap_or("") };
    let ctext = |i: crate::ast::Ident| -> &str { std::str::from_utf8(pf.bytes(i.lo, i.hi)).unwrap_or("") };
    // a lifetime is in scope: one the impl declares (`impl<'a>`), or an anonymous one its header names
    let declared = iast.list(im.generics.params).iter().any(|p| matches!(p.kind, crate::ast::GenericParamKind::Lifetime(_)));
    let header = declared || anon(iast, &itext, im.self_ty, false, 0)
        || im.trait_.as_ref().is_some_and(|p| iast.segs(p.segs).iter().any(|s| match iast.gargs(s.args()) {
            Some(GenericArgs::Angle(v)) => iast.list(*v).iter().any(|a| matches!(a, GenericArg::Lifetime(l) if itext(*l) == "'_")),
            _ => false,
        }));
    if header && anon(ast, &ctext, ty, true, 0) {
        crate::lint::emit(prog, d, &crate::lint::ELIDED_LIFETIMES_IN_ASSOCIATED_CONSTANT, def.file, ast.item(def.item).lo, "`&` without an explicit lifetime name cannot be used here");
        // the elided lifetime is a parameter of the impl's constant, which the trait's does not have
        if im.trait_.is_some() {
            return vec![prog.record_error(def.file, ast.item(def.item).lo, "typeck", format!("E0195: lifetime parameters or bounds on associated constant `{}` do not match the trait declaration", prog.name(d)))];
        }
    }
    Vec::new()
}

/// rustc_attr_parsing `AllowedTargets::AllowList`: the targets a builtin attribute may be written
/// on (a `Warn` target is only the `unused_attributes` lint and is accepted here). Returns the
/// plural target name when the attribute is not allowed on definition `d`.
fn attribute_target_error(prog: &Program, d: DefId, name: &[u8]) -> Option<&'static str> {
    use crate::program::DefKind as K;
    use crate::ast::ItemKind;
    let def = prog.def(d);
    let ast = &prog.files[def.file as usize].ast;
    let item = ast.item(def.item);
    // (plural name, is a fn-like target with/without body, is static-like ...)
    let (what, fn_like, trait_fn_no_body, static_like): (&'static str, bool, bool, bool) = match def.kind {
        K::Fn | K::AssocFn => {
            let body = matches!(item.kind, ItemKind::Fn(_, Some(_), _));
            let in_trait = def.kind == K::AssocFn && prog.def(def.parent).kind == K::Trait;
            ("functions", true, in_trait && !body, false)
        }
        K::ForeignFn => ("foreign functions", true, false, false),
        K::Static => ("statics", false, false, true),
        K::ForeignStatic => ("foreign statics", false, false, true),
        K::Struct => ("structs", false, false, false),
        K::Enum => ("enums", false, false, false),
        K::Union => ("unions", false, false, false),
        K::Const => ("constants", false, false, false),
        K::AssocConst => ("associated constants", false, false, false),
        K::Trait => ("traits", false, false, false),
        K::Impl => ("implementation blocks", false, false, false),
        K::TypeAlias => ("type aliases", false, false, false),
        K::AssocTy => ("associated types", false, false, false),
        K::Mod => ("modules", false, false, false),
        _ => return None,
    };
    let ok = match name {
        // Fn, Method, Closure, Delegation (+ Warn: trait fn without body, foreign fn, assoc const, ...)
        b"inline" => fn_like || def.kind == K::AssocConst,
        b"track_caller" => fn_like,
        b"target_feature" => fn_like && def.kind != K::ForeignFn && !trait_fn_no_body,
        b"used" => def.kind == K::Static,
        b"export_name" => static_like && def.kind == K::Static || (fn_like && def.kind != K::ForeignFn && !trait_fn_no_body),
        b"debugger_visualizer" => def.kind == K::Mod,
        b"link_ordinal" => matches!(def.kind, K::ForeignFn | K::ForeignStatic),
        b"proc_macro" | b"proc_macro_attribute" | b"proc_macro_derive" => def.kind == K::Fn,
        _ => return None,
    };
    if ok { None } else { Some(what) }
}

/// rustc_codegen_ssa / rustc_monomorphize: two definitions of the checked crates that export
/// one symbol (`#[no_mangle]` names it after the item, `#[export_name = ".."]` as given) are
/// "symbol `x` is already defined". The second, in definition order, is reported.
pub fn duplicate_symbols(prog: &Program, own: &[bool]) -> Vec<String> {
    use crate::program::DefKind as K;
    let mut seen: crate::fxhash::HashMap<Vec<u8>, DefId> = Default::default();
    let mut errors = Vec::new();
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if !own.get(def.krate as usize).copied().unwrap_or(false) || !matches!(def.kind, K::Fn | K::AssocFn | K::Static) { continue; }
        if prog.item_defs.get(&(def.file, def.item.0)) != Some(&d) { continue; }
        // (an inherent method's symbol clash is left to a build: the check-pass test
        // lint/non-snake-case/non-snake-ffi-issue-31924 has one)
        if def.kind == K::AssocFn && prog.def(def.parent).kind == K::Impl && { let pd = prog.def(def.parent); let pa = &prog.files[pd.file as usize].ast; pa.as_impl(&pa.item(pd.item).kind).is_some_and(|i| i.trait_.is_none()) } { continue; }
        let pf = &prog.files[def.file as usize];
        let ast = &pf.ast;
        let p = prog.src(def.file);
        let mut symbol: Option<Vec<u8>> = None;
        for a in ast.list(ast.item(def.item).attrs) {
            let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
            if hi <= lo { continue; }
            // `unsafe(no_mangle)` and `no_mangle`
            let first = if crate::cfg::tok_str(p, lo) == b"unsafe" && hi > lo + 2 { lo + 2 } else { lo };
            match crate::cfg::tok_str(p, first) {
                b"no_mangle" => symbol = Some(prog.name(d).as_bytes().to_vec()),
                b"export_name" if first + 2 < hi && crate::cfg::tok_str(p, first + 1) == b"=" => {
                    let v = crate::cfg::tok_str(p, first + 2);
                    symbol = Some(v.trim_ascii().strip_prefix(b"\"").and_then(|v| v.strip_suffix(b"\"")).unwrap_or(v).to_vec());
                }
                _ => {}
            }
        }
        let Some(sym) = symbol else { continue };
        match seen.get(&sym) {
            Some(&first) if first != d => errors.push(prog.record_uncoded_error(def.file, ast.item(def.item).lo, "typeck", format!("symbol `{}` is already defined", String::from_utf8_lossy(&sym)))),
            Some(_) => {}
            None => { seen.insert(sym, d); }
        }
    }
    errors
}

/// rustc_attr_parsing: the shape of a few built-in attributes' arguments (the `template!` each
/// parser checks): E0539 "expected this to be of the form ..", E0805 a wrong number of
/// arguments, E0565 a literal where a meta item belongs.
pub fn attribute_shapes(prog: &Program, d: DefId) -> Vec<String> {
    let def = prog.def(d);
    let ast = &prog.files[def.file as usize].ast;
    let p = prog.src(def.file);
    let mut errors = Vec::new();
    for a in ast.list(ast.item(def.item).attrs) {
        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
        if hi <= lo { continue; }
        // `unsafe(name ..)` wraps
        let first = if crate::cfg::tok_str(p, lo) == b"unsafe" && hi > lo + 2 && crate::cfg::tok_str(p, lo + 1) == b"(" { lo + 2 } else { lo };
        let name = crate::cfg::tok_str(p, first).to_vec();
        let name_str = String::from_utf8_lossy(&name).into_owned();
        if !matches!(name.as_slice(), b"inline" | b"must_use" | b"deprecated" | b"link_name" | b"link_ordinal" | b"link" | b"derive") { continue; }
        let pos = p.tok(lo).lo;
        let end = if first > lo { hi.saturating_sub(1) } else { hi };
        let next = if first + 1 < end { crate::cfg::tok_str(p, first + 1) } else { b"" };
        // the arguments of `name(..)`: top-level comma separated token runs
        let mut args: Vec<Vec<usize>> = Vec::new();
        let mut list = false;
        let mut eq_value: Option<usize> = None;
        if next == b"(" {
            list = true;
            let mut depth = 0;
            let mut cur: Vec<usize> = Vec::new();
            let mut i = first + 2;
            while i < end {
                let t = crate::cfg::tok_str(p, i);
                if t == b"(" { depth += 1; }
                if t == b")" { if depth == 0 { break; } depth -= 1; }
                if t == b"," && depth == 0 { args.push(std::mem::take(&mut cur)); } else { cur.push(i); }
                i += 1;
            }
            if !cur.is_empty() { args.push(cur); }
        } else if next == b"=" && first + 2 < end { eq_value = Some(first + 2); }
        let is_str = |i: usize| p.tok(i).kind == crate::lexer::T::Str || p.tok(i).kind == crate::lexer::T::RawStr;
        let mut bad = |code: &str| errors.push(prog.record_error(def.file, pos, "typeck", format!("{}: malformed `{}` attribute input", code, name_str)));
        match name.as_slice() {
            // rustc_builtin_macros derive.rs: each entry is a path to a trait
            b"derive" => {
                for arg in &args {
                    if arg.len() == 1 && matches!(p.tok(arg[0]).kind, crate::lexer::T::Str | crate::lexer::T::RawStr | crate::lexer::T::Int | crate::lexer::T::Float | crate::lexer::T::Char | crate::lexer::T::Byte | crate::lexer::T::ByteStr | crate::lexer::T::KwTrue | crate::lexer::T::KwFalse) {
                        errors.push(prog.record_error(def.file, p.tok(arg[0]).lo, "typeck", "E0777: expected path to a trait, found literal".to_string()));
                    }
                }
            }
            b"inline" => {
                if list {
                    if args.len() != 1 { bad("E0805"); }
                    else if !(args[0].len() == 1 && matches!(crate::cfg::tok_str(p, args[0][0]), b"always" | b"never")) { bad("E0539"); }
                }
            }
            b"must_use" => {
                if list { bad("E0539"); }
                else if let Some(v) = eq_value { if !is_str(v) { bad("E0539"); } }
            }
            b"link_name" => {
                if eq_value.map_or(true, |v| !is_str(v)) { bad("E0539"); }
            }
            b"link" => {
                if !list { bad("E0539"); }
            }
            b"link_ordinal" => {
                if !list || args.is_empty() { bad("E0805"); }
                else if args.len() > 1 { bad("E0805"); }
                else if !(args[0].len() == 1 && p.tok(args[0][0]).kind == crate::lexer::T::Int) { bad("E0539"); }
            }
            b"deprecated" => {
                if list {
                    for arg in &args {
                        // a bare literal where `key = value` belongs
                        if arg.len() == 1 && matches!(p.tok(arg[0]).kind, crate::lexer::T::Str | crate::lexer::T::RawStr | crate::lexer::T::Int | crate::lexer::T::Float) { bad("E0565"); break; }
                        let key = arg.first().map(|&i| crate::cfg::tok_str(p, i)).unwrap_or(b"");
                        let ok_key = matches!(key, b"since" | b"note" | b"suggestion");
                        let valued = arg.len() == 3 && crate::cfg::tok_str(p, arg[1]) == b"=" && is_str(arg[2]);
                        if !ok_key || !valued { bad("E0539"); break; }
                    }
                } else if let Some(v) = eq_value { if !is_str(v) { bad("E0539"); } }
            }
            _ => {}
        }
    }
    errors
}

fn entry_main(prog: &Program, root: u32) -> Option<DefId> {
    let sym = prog.syms.get("main")?;
    let d = prog.lookup_in_module(prog.crates[root as usize].root_mod, sym, false, &mut Vec::new())?;
    (prog.def(d).kind == DefKind::Fn).then_some(d)
}

/// rustc_hir_analysis check_opaque_precise_captures: a `use<..>` list names every type and const parameter in
/// scope of the opaque (`Self` of a trait too). The syntax of the list (order, duplicates) is checked where its
/// names resolve (resolve_bound_vars).
pub fn precise_capture_params(prog: &Program, tcx: &Tcx, d: DefId) -> Vec<String> {
    use crate::ast::{Bound, GenericArg, GenericArgs, ItemKind, Ty};
    let def = prog.def(d);
    if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn) { return Vec::new(); }
    let pf = &prog.files[def.file as usize];
    let ast = &pf.ast;
    let ItemKind::Fn(sig, _, _) = ast.item(def.item).kind else { return Vec::new() };
    let Some(ret) = ast.fn_sig(sig).ret else { return Vec::new() };
    // the `use<..>` lists of the opaque types written in the return type
    fn walk(ast: &crate::ast::Ast, t: crate::ast::TyId, out: &mut Vec<crate::ast::GenericArgsId>, depth: u32) {
        if depth > 16 { return; }
        let bounds = |bs: crate::ast::List<Bound>, out: &mut Vec<crate::ast::GenericArgsId>| {
            for b in ast.list(bs) {
                match b {
                    Bound::Use(a) => out.push(*a),
                    Bound::Trait { path, .. } => for s in ast.segs(path.segs) {
                        if let Some(GenericArgs::Angle(v)) = ast.gargs(s.args()) {
                            for a in ast.list(*v) { if let GenericArg::Type(x) | GenericArg::Binding(_, _, x) = a { walk(ast, *x, out, depth + 1); } }
                        }
                    },
                    _ => {}
                }
            }
        };
        match ast.ty(t) {
            Ty::ImplTrait(bs) => bounds(*bs, out),
            Ty::DynTrait(bs, _) => bounds(*bs, out),
            Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => walk(ast, *x, out, depth + 1),
            Ty::Tuple(l) => for x in ast.list(*l) { walk(ast, *x, out, depth + 1); },
            Ty::Path(p) => for s in ast.segs(p.segs) {
                if let Some(GenericArgs::Angle(v)) = ast.gargs(s.args()) {
                    for a in ast.list(*v) { if let GenericArg::Type(x) | GenericArg::Binding(_, _, x) = a { walk(ast, *x, out, depth + 1); } }
                }
            },
            _ => {}
        }
    }
    let mut uses = Vec::new();
    walk(ast, ret, &mut uses, 0);
    if uses.is_empty() { return Vec::new(); }
    let env = tcx.declaration_env(prog, d);
    let mut errors = Vec::new();
    for args in uses {
        let Some(GenericArgs::Angle(list)) = ast.gargs(args) else { continue };
        let mut listed: Vec<&str> = Vec::new();
        for a in ast.list(*list) {
            match a {
                GenericArg::Lifetime(lt) => listed.push(pf.text(*lt)),
                GenericArg::Type(t) => if let Ty::Path(p) = ast.ty(*t) { if p.no_qself() && ast.segs(p.segs).len() == 1 { listed.push(pf.text(ast.segs(p.segs)[0].name)); } },
                _ => {}
            }
        }
        for (i, &name) in env.names.iter().enumerate() {
            let written = prog.syms.str(name);
            if listed.contains(&written) || env.impl_params.iter().any(|&(_, p)| p as usize == i) { continue; }
            let msg = if i == 0 && written == "Self" && env.trait_def.is_some() {
                "`impl Trait` must mention the `Self` type of the trait in `use<...>`"
            } else if env.consts.iter().any(|c| c.0 as usize == i) {
                "`impl Trait` must mention all const parameters in scope in `use<...>`"
            } else {
                "`impl Trait` must mention all type parameters in scope in `use<...>`"
            };
            errors.push(prog.record_uncoded_error(def.file, ast.item(def.item).lo, "typeck", msg.to_string()));
        }
    }
    errors
}

/// rustc_builtin_macros global_allocator.rs / rustc_metadata allocator.rs / rustc_passes weak_lang_items.rs /
/// check_attr.rs: `#[global_allocator]` is for statics and there is one per program; a `#[panic_handler]` of a
/// crate that links `std` duplicates `std`'s; `#[no_mangle]` is not for the language items.
pub fn global_allocators_and_lang_items(prog: &Program, own: &[bool]) -> Vec<String> {
    use crate::program::DefKind as K;
    let mut errors = Vec::new();
    let mut allocators = 0usize;
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if !own.get(def.krate as usize).copied().unwrap_or(false) || !matches!(def.kind, K::Fn | K::AssocFn | K::Static | K::Const | K::Struct | K::Enum | K::Impl | K::TypeAlias | K::Trait) { continue; }
        if prog.item_defs.get(&(def.file, def.item.0)) != Some(&d) { continue; }
        let pf = &prog.files[def.file as usize];
        let ast = &pf.ast;
        let p = prog.src(def.file);
        let item = ast.item(def.item);
        let (mut allocator, mut panic_handler, mut no_mangle) = (None, None, None);
        for a in ast.list(item.attrs) {
            let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
            if hi <= lo || (p.tok(lo).lo as usize) >= pf.src_len as usize { continue; }
            let first = if crate::cfg::tok_str(p, lo) == b"unsafe" && hi > lo + 2 { lo + 2 } else { lo };
            match crate::cfg::tok_str(p, first) {
                b"global_allocator" => allocator = Some(p.tok(lo).lo),
                b"panic_handler" => panic_handler = Some(p.tok(lo).lo),
                b"no_mangle" => no_mangle = Some(p.tok(lo).lo),
                _ => {}
            }
        }
        if allocator.is_some() {
            if def.kind != K::Static {
                errors.push(prog.record_uncoded_error(def.file, item.lo, "typeck", "allocators must be statics".to_string()));
            } else {
                allocators += 1;
                if allocators > 1 {
                    errors.push(prog.record_uncoded_error(def.file, item.lo, "typeck", "cannot define multiple global allocators".to_string()));
                }
            }
        }
        if panic_handler.is_some() {
            let root_file = prog.mods[prog.crates[def.krate as usize].root_mod as usize].file;
            let no_std = prog.files[root_file as usize].ast.root_attrs.iter().any(|a| a.toks.hi > a.toks.lo && prog.files[root_file as usize].tok_text(a.toks.lo) == "no_std");
            if !no_std {
                errors.push(prog.record_error(def.file, item.lo, "typeck", "E0152: found duplicate lang item `panic_impl`".to_string()));
            }
            if let Some(nm) = no_mangle {
                errors.push(prog.record_uncoded_error(def.file, nm, "typeck", "`#[no_mangle]` cannot be used on internal language items".to_string()));
            }
        }
    }
    errors
}

/// rustc_ast_passes feature_gate `gate_all`: syntax the parser recorded (ParseSess::gated_spans)
/// whose feature the crate does not enable is E0658. Text a macro expanded is not judged.
pub fn gated_syntax(prog: &Program, own: &[bool]) -> Vec<String> {
    let mut errors = Vec::new();
    for (f, pf) in prog.files.iter().enumerate() {
        if pf.ast.gated_syntax.is_empty() { continue; }
        let krate = prog.file_crate.get(f).copied().unwrap_or(u32::MAX);
        if !own.get(krate as usize).copied().unwrap_or(false) { continue; }
        let features = prog.crates[krate as usize].features(prog);
        for &(feature, pos) in &pf.ast.gated_syntax {
            if pos >= pf.src_len || features.has(feature) { continue; }
            let msg = match feature {
                "stmt_expr_attributes" => "attributes on expressions are experimental",
                "coroutines" => "coroutine syntax is experimental",
                "async_trait_bounds" => "`async` trait bounds are unstable",
                "closure_track_caller" => "`#[track_caller]` on closures is currently unstable",
                _ => continue,
            };
            errors.push(prog.record_error(f as u32, pos, "typeck", format!("E0658: {msg}")));
        }
    }
    errors
}

/// rustc_attr_parsing `AllowedTargets` for the attributes of nodes that have no definition of
/// their own: the targets of `#[inline]`, `#[track_caller]`, `#[target_feature]`, `#[used]`,
/// `#[export_name]` and `#[link_ordinal]` never include enum variants or the crate.
pub fn node_attribute_targets(prog: &Program, own: &[bool]) -> Vec<String> {
    use crate::ast::ItemKind;
    const NAMES: &[&[u8]] = &[b"inline", b"track_caller", b"target_feature", b"used", b"export_name", b"link_ordinal"];
    let mut errors = Vec::new();
    for (f, pf) in prog.files.iter().enumerate() {
        let krate = prog.file_crate.get(f).copied().unwrap_or(u32::MAX);
        if !own.get(krate as usize).copied().unwrap_or(false) { continue; }
        let ast = &pf.ast;
        let p = prog.src(f as u32);
        let mut check = |attrs: &[crate::ast::Attr], what: &str| {
            for a in attrs {
                let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
                if hi <= lo || (p.tok(lo).lo as usize) >= pf.src_len as usize { continue; }
                let name = crate::cfg::tok_str(p, lo);
                if NAMES.contains(&name) {
                    errors.push(prog.record_uncoded_error(f as u32, p.tok(lo).lo, "typeck", format!("`#[{}]` attribute cannot be used on {}", String::from_utf8_lossy(name), what)));
                }
            }
        };
        if ast.root_attrs.len() > 0 && prog.mods[prog.crates[krate as usize].root_mod as usize].file == f as u32 {
            let attrs: Vec<crate::ast::Attr> = ast.root_attrs.iter().copied().collect();
            check(&attrs, "crates");
        }
        for (i, item) in ast.items.iter().enumerate() {
            let ItemKind::Enum(r) = item.kind else { continue };
            if !prog.item_defs.contains_key(&(f as u32, i as u32)) { continue; }
            for v in ast.list(ast.row(r).variants) {
                check(ast.list(v.attrs), "enum variants");
            }
        }
    }
    errors
}

/// rustc_ast_passes `check_decl_attrs` (+ rustc_resolve): the attributes of a function parameter
/// are `cfg`, `cfg_attr` and the lint attributes, or non-builtin ones; a built-in attribute macro
/// (`#[test]`) is not a non-macro attribute.
pub fn param_attributes(prog: &Program, own: &[bool]) -> Vec<String> {
    let mut errors = Vec::new();
    for (f, pf) in prog.files.iter().enumerate() {
        let krate = prog.file_crate.get(f).copied().unwrap_or(u32::MAX);
        if !own.get(krate as usize).copied().unwrap_or(false) { continue; }
        let ast = &pf.ast;
        let p = prog.src(f as u32);
        // rustc_resolve: enum variants and struct fields take only non-macro attributes (struct-pattern
        // fields, match arms and struct-expression fields are the parser's)
        for a in ast.variants.iter().flat_map(|v| ast.list(v.attrs)).chain(ast.field_defs.iter().flat_map(|fd| ast.list(fd.attrs))) {
            let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
            if hi <= lo { continue; }
            let t = p.tok(lo);
            if t.lo >= pf.src_len { continue; }
            if hi > lo + 1 && !matches!(crate::cfg::tok_str(p, lo + 1), b"(" | b"=") { continue; }
            let name = String::from_utf8_lossy(crate::cfg::tok_str(p, lo)).into_owned();
            if matches!(name.as_str(), "test" | "bench" | "test_case" | "derive" | "global_allocator" | "cfg_eval" | "alloc_error_handler" | "derive_const") {
                errors.push(prog.record_uncoded_error(f as u32, t.lo, "resolve", format!("expected non-macro attribute, found attribute macro `{name}`")));
            }
        }
        for prm in ast.params.iter() {
            for a in ast.list(prm.attrs) {
                let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
                if hi <= lo { continue; }
                let t = p.tok(lo);
                if t.lo >= pf.src_len { continue; }
                // a plain one-segment path
                if hi > lo + 1 && !matches!(crate::cfg::tok_str(p, lo + 1), b"(" | b"=") { continue; }
                let name = String::from_utf8_lossy(crate::cfg::tok_str(p, lo)).into_owned();
                match name.as_str() {
                    "allow" | "cfg" | "cfg_attr" | "deny" | "expect" | "forbid" | "warn" => {}
                    "test" | "bench" | "test_case" | "derive" | "global_allocator" | "cfg_eval" | "alloc_error_handler" | "derive_const" =>
                        errors.push(prog.record_uncoded_error(f as u32, t.lo, "resolve", format!("expected non-macro attribute, found attribute macro `{name}`"))),
                    n if crate::builtin_attrs::is_builtin_attr(n) =>
                        errors.push(prog.record_uncoded_error(f as u32, t.lo, "typeck", "allow, cfg, cfg_attr, deny, expect, forbid, and warn are the only allowed built-in attributes in function parameters".to_string())),
                    _ => {}
                }
            }
        }
    }
    errors
}
