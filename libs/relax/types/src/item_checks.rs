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
        // (foreign items are left out: an `extern` block inside a fn body is collected into the
        // enclosing module, not the body's block, so two bodies' declarations would collide)
        DefKind::Fn | DefKind::Const | DefKind::Static => (false, true),
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
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if !own.get(def.krate as usize).copied().unwrap_or(false) || def.parent == NO_DEF { continue; }
        if prog.def(def.parent).kind != DefKind::Mod { continue; }
        // only items the module's source declares (not the compiler's own definitions)
        if prog.item_defs.get(&(def.file, def.item.0)) != Some(&d) { continue; }
        let name = prog.syms.str(def.name);
        if name == "_" || name.is_empty() { continue; }
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
                    let pos = ast.item(def.item).name.lo;
                    let ns_name = if ns { "type" } else { "value" };
                    let module = if prog.block_owner.get(&prog.def(def.parent).sub).is_some() { "this block" } else { "this module" };
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
        if visiting.contains(&d) { return out; }
        visiting.push(d);
        if let Some(adt) = self.tcx.adts.get(&d) {
            for v in &adt.variants {
                for f in &v.fields { self.params_in_repr_ty(f.ty, &mut out, visiting); }
            }
        }
        visiting.pop();
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
    let mut r = Representability { tcx, active: Vec::new(), done: Default::default(), cycle: None };
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
            // #[link(name = "..", kind = "..")] (rustc_attr_parsing link_attrs.rs)
            {
                let sp = prog.src(file);
                for a in &attrs {
                    let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
                    if hi < lo + 3 || crate::cfg::tok_str(sp, lo) != b"link" || crate::cfg::tok_str(sp, lo + 1) != b"(" { continue; }
                    let pos = sp.tok(lo).lo;
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
                if let ItemKind::Fn(sig, _, _) = ast.item(it).kind {
                    let sig = ast.fn_sig(sig);
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
        if from_source {
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
    for a in ast.list(item.attrs) {
        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
        if hi <= lo || crate::cfg::tok_str(p, lo) != b"repr" { continue; }
        let pos = p.tok(lo).lo;
        first_pos = pos;
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
                b"Rust" | b"simd" => other = other || name == b"simd",
                _ => {}
            }
        }
    }
    let _ = other;
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
        _ => return None,
    };
    if ok { None } else { Some(what) }
}
