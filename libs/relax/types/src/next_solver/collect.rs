// ported from N:rustc_hir_analysis/src/collect/clauses_of.rs @8d1a764
// ported from N:rustc_hir_analysis/src/hir_ty_lowering/bounds.rs @8d1a764
// ported from N:rustc_hir_analysis/src/collect/item_bounds.rs @8d1a764
use super::*;
use crate::ast::{self, GenericParamKind, Bound, GenericArgs, WherePred};
use crate::program::{Program, DefKind};
use crate::tcx::{Tcx, GenEnv, EvalCx, EvalState, TypingEnv};
use crate::types::{TyKind, AliasTy, AliasTyKind, TyId, RegionId, RegionKind};
use crate::types::fold::TypeFolder;
use crate::consts::GenericArg;
use crate::consts::{ConstKind, IsRigid};
use crate::tcx::resolve_bound_vars::BinderId;

#[derive(Clone, Copy)]
pub(crate) enum PredicateFilter { All, SelfOnly, SelfTraitThatDefines(crate::program::Sym) }

pub fn trait_predicate(def_id: DefId, args: Vec<GenericArg>) -> Predicate {
    Predicate::dummy(PredicateKind::Clause(ClauseKind::Trait(TraitClause { trait_ref: TraitRef { def_id, args }, polarity: ClausePolarity::Positive })))
}


/// The late-bound regions of the innermost binder (`ReBound(0, _)` at the root) a value mentions; `constrained` skips
/// what sits under an alias (rustc `LateBoundRegionsCollector`).
struct LateBoundRegions { out: Vec<u32>, constrained: bool }
impl crate::types::fold::TypeFolder for LateBoundRegions {
    fn fold_ty(&mut self, tys: &crate::types::Types, t: TyId, depth: crate::types::DebruijnIndex) -> TyId {
        if self.constrained && matches!(tys.kind(t), TyKind::Alias(..)) { return t; }
        crate::types::fold::super_fold_ty(tys, t, self, depth)
    }
    fn fold_region(&mut self, tys: &crate::types::Types, r: crate::types::RegionId, depth: crate::types::DebruijnIndex) -> crate::types::RegionId {
        if let RegionKind::ReBound(d, v) = tys.region_kind(r) { if d == depth && !self.out.contains(&v.0) { self.out.push(v.0); } }
        r
    }
}

impl Tcx {
    /// hir_ty_lowering/bounds.rs validate_late_bound_regions (E0582): a late-bound region the bound's value mentions
    /// appears in the trait's input types (not only under a projection).
    fn validate_late_bound_regions(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, pos: u32, trait_args: &[GenericArg], alias_regions: &[RegionId], value: TyId) {
        if !self.validates(prog, file) { return; }
        let mut constrained = LateBoundRegions { out: Vec::new(), constrained: true };
        for a in trait_args {
            match a {
                GenericArg::Type(t) => { constrained.fold_ty(&self.tys, *t, 0); }
                GenericArg::Lifetime(r) => { constrained.fold_region(&self.tys, *r, 0); }
                GenericArg::Const(_) => {}
            }
        }
        for r in alias_regions { constrained.fold_region(&self.tys, *r, 0); }
        let mut referenced = LateBoundRegions { out: Vec::new(), constrained: false };
        referenced.fold_ty(&self.tys, value, 0);
        if referenced.out.iter().any(|v| !constrained.out.contains(v)) {
            self.err(cx, prog, file, pos, "E0582: binding for associated type references a lifetime, which does not appear in the trait input types".to_string());
        }
    }
}

static DECLARATION_ENVS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static LOWER_BOUNDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn count_query(counter: &std::sync::atomic::AtomicU64) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("MAKEPAD_RUN_DECL_COUNTERS").is_some()) {
        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Tcx {
    pub fn report_declaration_counters() {
        if std::env::var_os("MAKEPAD_RUN_DECL_COUNTERS").is_some() {
            eprintln!("makepad-run declarations at first present: declaration_env={} lower_bounds={}",
                DECLARATION_ENVS.load(std::sync::atomic::Ordering::Relaxed),
                LOWER_BOUNDS.load(std::sync::atomic::Ordering::Relaxed));
        }
    }
    pub fn declaration_env(&self, prog: &Program, def_id: DefId) -> GenEnv {
        count_query(&DECLARATION_ENVS);
        let def = prog.def(def_id);
        if matches!(def.kind, DefKind::Prim(_) | DefKind::Mod | DefKind::MacroRules) { return GenEnv::default(); }
        let mut env = if def.kind == DefKind::Trait {
            let mut env = GenEnv::default();
            env.names.push(prog.syms.get("Self").unwrap());
            env.regions.next_param = 1;
            if let Some(crate::ast::TraitItem { generics, .. }) = { let ast = &prog.files[def.file as usize].ast; ast.as_trait(&ast.item(def.item).kind) } {
                crate::tcx::push_generics(prog, def.file, generics, &mut env);
            }
            env.self_ty = Some(self.tys.intern(TyKind::Param(0)));
            env.trait_def = Some(def_id);
            env
        } else { self.impl_generics(prog, def_id).unwrap_or_else(|| self.item_env(prog, def_id)) };
        env.regions = self.bound_region_env(prog, def_id);
        env
    }

    pub fn explicit_super_predicates_of(&self, prog: &Program, def_id: DefId) -> &[Predicate] {
        self.declaration_queries(prog, def_id).super_predicates.get_or_init_cyclic(||
            self.implied_clauses_with_filter(prog, def_id, PredicateFilter::SelfOnly))
    }

    pub(crate) fn explicit_supertraits_containing_assoc_item(&self, prog: &Program, def_id: DefId, name: crate::program::Sym) -> Vec<Predicate> {
        self.declaration_queries(prog, def_id).filtered_supers(name, ||
            self.implied_clauses_with_filter(prog, def_id, PredicateFilter::SelfTraitThatDefines(name)))
    }

    // context.rs:2090 and type_ir/elaborate.rs:324: traverse the lowered
    // supertrait clauses, including Self where-predicates, in declaration order.
    fn trait_may_define_assoc_item(&self, prog: &Program, def_id: DefId, name: crate::program::Sym) -> bool {
        let mut stack = vec![def_id];
        let mut visited = crate::fxhash::HashSet::default();
        visited.insert(def_id);
        while let Some(def) = stack.pop() {
            for bound in self.explicit_super_predicates_of(prog, def) {
                if let PredicateKind::Clause(ClauseKind::Trait(clause)) = &bound.kind {
                    if visited.insert(clause.trait_ref.def_id) { stack.push(clause.trait_ref.def_id); }
                }
            }
            let d = prog.def(def);
            if d.kind == DefKind::Trait {
                for &item in &prog.traits[d.sub as usize] {
                    if prog.def(item).name == name { return true; }
                }
            }
        }
        false
    }

    fn implied_clauses_with_filter(&self, prog: &Program, def_id: DefId, filter: PredicateFilter) -> Vec<Predicate> {
        let def = prog.def(def_id);
        let ast = &prog.files[def.file as usize].ast;
        let Some(&crate::ast::TraitItem { supers, generics, .. }) = ast.as_trait(&ast.item(def.item).kind) else { return Vec::new(); };
        let env = self.declaration_env(prog, def_id);
        let state = EvalState::new();
        let cx = EvalCx::new(prog, self, TypingEnv::fully_monomorphized(&prog), &state);
        let mut out = Vec::new();
        let mut binder = env.clone();
        binder.regions = env.regions.with_binder(None);
        let self_ty = env.self_ty.unwrap();
        self.add_implicit_sizedness_bounds(&cx, def_id, &binder, self_ty, supers, &[], true, &mut out);
        self.lower_bounds(&cx, def_id, &binder, self_ty, supers, false, filter, &mut out);
        for (where_index, &pred) in ast.list(generics.where_).iter().enumerate() {
            if let WherePred::Bound { ty, bounds, .. } = pred {
                // clauses_of.rs:1022-1040: SelfOnly filters the resolved subject
                // before lowering it; lowering Self::Assoc here cycles through
                // the very supertrait query used to resolve that projection.
                let mut subject = ty;
                while let ast::Ty::Paren(inner) = ast.ty(subject) { subject = *inner; }
                let ast::Ty::Path(path) = ast.ty(subject) else { continue; };
                if path.has_qself() || path.segs.len() != 1 || ast.segs(path.segs)[0].kind() != ast::SegKind::SelfType { continue; }
                let mut binder = env.clone();
                binder.regions = env.regions.with_binder(Some(BinderId::Where(generics.where_.lo + where_index as u32)));
                self.lower_bounds(&cx, def_id, &binder, self_ty, bounds, false, filter, &mut out);
            }
        }
        out
    }

    /// clauses_of.rs:865-1050: recursively lower each item's parameter bounds
    /// in that item's ItemCtxt. Re-lowering inherited AST bounds in a child
    /// owner's lifetime table loses the resolved lifetime uses of the parent.
    pub(crate) fn compute_type_param_clauses(&self, prog: &Program, owner: DefId, param: u32, assoc: crate::program::Sym) -> Vec<Predicate> {
        match prog.impl_trait_in_trait_data.get(&owner) {
            Some(relax_front::assoc::ImplTraitInTraitData::Trait { opaque_def_id, .. }) => {
                return self.type_param_clauses(prog, *opaque_def_id, param, assoc);
            }
            Some(relax_front::assoc::ImplTraitInTraitData::Impl { .. }) => unreachable!("lowering bounds on RPITIT in impl"),
            None => {}
        }
        let def = prog.def(owner);
        if def.kind == DefKind::OpaqueTy {
            return self.type_param_clauses(prog, prog.opaque_owner(owner), param, assoc);
        }
        let mut out = Vec::new();
        if matches!(def.kind, DefKind::AssocFn | DefKind::AssocTy | DefKind::AssocConst) {
            let parent_count = self.generic_indices_of(prog, def.parent).types_consts.len();
            if (param as usize) < parent_count {
                out.extend(self.type_param_clauses(prog, def.parent, param, assoc));
            }
        }
        let env = self.declaration_env(prog, owner);
        let ty = self.tys.intern(TyKind::Param(param));
        if def.kind == DefKind::Trait && param == 0 {
            out.push(trait_predicate(owner, self.generic_indices_of(prog, owner).identity_args(&self.tys)));
        }
        let ast = &prog.files[def.file as usize].ast;
        let generics = crate::tcx::resolve_bound_vars::owner_generics(prog, owner);
        let state = EvalState::new();
        let cx = EvalCx::new(prog, self, TypingEnv::fully_monomorphized(prog), &state);
        let filter = PredicateFilter::SelfTraitThatDefines(assoc);
        let own_count = ast.list(generics.params).iter().filter(|p| !matches!(p.kind, GenericParamKind::Lifetime(_))).count();
        let mut index = env.names.len() - own_count - env.impl_params.len();
        let mut binder = env.clone();
        binder.regions = env.regions.with_binder(None);
        for parameter in ast.list(generics.params) {
            match parameter.kind {
                GenericParamKind::Lifetime(_) => continue,
                GenericParamKind::Type(bounds, _) if index == param as usize => {
                    self.lower_bounds(&cx, owner, &binder, ty, bounds, false, filter, &mut out);
                }
                _ => {}
            }
            index += 1;
        }
        for &(node, index) in &env.impl_params {
            if index == param {
                if let ast::Ty::ImplTrait(bounds) = ast.ty(ast::TyId(node)) {
                    self.lower_bounds(&cx, owner, &binder, ty, *bounds, false, filter, &mut out);
                }
            }
        }
        for (index, predicate) in ast.list(generics.where_).iter().enumerate() {
            let WherePred::Bound { ty: subject, bounds, .. } = *predicate else { continue; };
            let mut subject = subject;
            while let ast::Ty::Paren(inner) = ast.ty(subject) { subject = *inner; }
            let ast::Ty::Path(path) = ast.ty(subject) else { continue; };
            if path.has_qself() || path.global() || path.segs.len() != 1 { continue; }
            let segment = ast.segs(path.segs)[0];
            if !segment.args().is_none() { continue; }
            let name = prog.syms.get(prog.files[def.file as usize].text(segment.name));
            if name != env.names.get(param as usize).copied() { continue; }
            let mut binder = env.clone();
            binder.regions = env.regions.with_binder(Some(BinderId::Where(generics.where_.lo + index as u32)));
            self.lower_bounds(&cx, owner, &binder, ty, bounds, false, filter, &mut out);
        }
        out
    }

    pub fn clauses_of(&self, prog: &Program, def_id: DefId) -> Vec<Predicate> {
        let def = prog.def(def_id);
        let mut out = if matches!(def.kind, DefKind::AssocFn | DefKind::AssocTy | DefKind::AssocConst) {
            self.clauses_of(prog, def.parent)
        } else { Vec::new() };
        out.extend_from_slice(self.own_clauses_of(prog, def_id));
        out
    }

    /// The definition's own clauses and the errors lowering them reported (rustc `predicates_of`
    /// reports a bound that names no trait, a wrong number of arguments, ...).
    pub(crate) fn own_clauses_uncached(&self, prog: &Program, def_id: DefId) -> (Vec<Predicate>, Vec<String>) {
        if let Some(mut clauses) = self.rpitit_clauses(prog, def_id) {
            let count = self.clauses_of(prog, prog.def(def_id).parent).len();
            clauses.drain(..count);
            return (clauses, Vec::new());
        }
        let def = prog.def(def_id);
        // rustc_hir_analysis/src/collect/generics_of.rs:96: MCG has no parent.
        if def.kind == DefKind::AnonConst && !prog.repeat_expr_counts.contains(&def_id)
            && matches!(prog.def_id_to_node_id.get(&def_id), Some((_, relax_front::def_collector::NodeId::AnonConst(_)))) { return (Vec::new(), Vec::new()); }
        if matches!(def.kind, DefKind::AnonConst | DefKind::OpaqueTy | DefKind::Closure | DefKind::Field) {
            return (self.clauses_of(prog, def.parent), Vec::new());
        }
        let env = self.declaration_env(prog, def_id);
        let state = EvalState::new();
        state.errors.replace(Some(Vec::new()));
        let cx = EvalCx::new(prog, self, TypingEnv::fully_monomorphized(&prog), &state);
        let ast = &prog.files[def.file as usize].ast;
        let generics = match ast.kind_generics(&ast.item(def.item).kind) {
            Some(g) => *g,
            None => Default::default(),
        };
        let mut out = Vec::new();
        if def.kind == DefKind::Trait {
            out.push(trait_predicate(def_id, self.generic_indices_of(prog, def_id).identity_args(&self.tys)));
            if let Some(&crate::ast::TraitItem { supers, .. }) = ast.as_trait(&ast.item(def.item).kind) {
                let mut binder = env.clone();
                binder.regions = env.regions.with_binder(None);
                self.add_implicit_sizedness_bounds(&cx, def_id, &binder, env.self_ty.unwrap(), supers, &[], true, &mut out);
                for &b in ast.list(supers) { self.lower_bound(&cx, def_id, &binder, env.self_ty.unwrap(), b, PredicateFilter::All, &mut out); }
            }
        }
        let mut own = 0;
        for p in ast.list(generics.params) { if !matches!(p.kind, GenericParamKind::Lifetime(_)) { own += 1; } }
        let mut index = env.names.len() - own - env.impl_params.len();
        let mut binder = env.clone();
        binder.regions = env.regions.with_binder(None);
        for p in ast.list(generics.params) {
            match p.kind {
                GenericParamKind::Lifetime(bounds) => {
                    let a = env.regions.resolve(cx.infcx, &self.tys, prog, def.file, Some(p.name));
                    for &b in ast.list(bounds) {
                        let b = env.regions.resolve(cx.infcx, &self.tys, prog, def.file, Some(b));
                        out.push(Predicate::dummy(PredicateKind::Clause(ClauseKind::RegionOutlives(a, b))));
                    }
                    continue;
                }
                GenericParamKind::Type(bounds, _) => {
                    let ty = self.tys.intern(TyKind::Param(index as u32));
                    let name = prog.syms.get(prog.files[def.file as usize].text(p.name));
                    let where_bounds = param_where_bounds(prog, def.file, &generics, name);
                    self.add_implicit_sizedness_bounds(&cx, def_id, &binder, ty, bounds, &where_bounds, false, &mut out);
                    self.lower_bounds(&cx, def_id, &binder, ty, bounds, false, PredicateFilter::All, &mut out);
                }
                GenericParamKind::Const(ty, _) => {
                    let ct = self.tys.intern_const(ConstKind::Param(crate::consts::ParamConst { index: index as u32 }));
                    let ty = self.lower_ty(&cx, prog, def.file, def.scope, &env, ty);
                    out.push(Predicate::dummy(PredicateKind::Clause(ClauseKind::ConstArgHasType(ct, ty))));
                }
            }
            index += 1;
        }
        for &(node, index) in &env.impl_params {
            if let ast::Ty::ImplTrait(bs) = ast.ty(ast::TyId(node)) {
                self.lower_bounds(&cx, def_id, &binder, self.tys.intern(TyKind::Param(index)), *bs, true, PredicateFilter::All, &mut out);
            }
        }
        for (where_index, &predicate) in ast.list(generics.where_).iter().enumerate() {
            match predicate {
                WherePred::Lifetime(a, bounds) => {
                    let a = env.regions.resolve(cx.infcx, &self.tys, prog, def.file, Some(a));
                    for &b in ast.list(bounds) {
                        let b = env.regions.resolve(cx.infcx, &self.tys, prog, def.file, Some(b));
                        out.push(Predicate::dummy(PredicateKind::Clause(ClauseKind::RegionOutlives(a, b))));
                    }
                }
                WherePred::Bound { ty, bounds, .. } => {
                    let mut binder = env.clone();
                    binder.regions = env.regions.with_binder(Some(BinderId::Where(generics.where_.lo + where_index as u32)));
                    let ty = self.lower_ty(&cx, prog, def.file, def.scope, &binder, ty);
                    self.lower_bounds(&cx, def_id, &binder, ty, bounds, false, PredicateFilter::All, &mut out);
                    // `where T:` (no bounds) is the clause "T is well formed" (rustc lowers it so)
                    if bounds.is_empty() && !self.tys.has_error(ty) && crate::wfcheck::obligations::arity_ok(self, prog, ty) {
                        out.push(Predicate { kind: PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(ty))), bound_vars: binder.regions.bound_vars().into() });
                    }
                }
            }
        }
        (out, state.errors.take().unwrap_or_default())
    }

    pub(crate) fn lower_bounds(&self, cx: &EvalCx<'_>, owner: DefId, env: &GenEnv, ty: TyId, bounds: ast::List<Bound>, sized: bool, filter: PredicateFilter, out: &mut Vec<Predicate>) {
        count_query(&LOWER_BOUNDS);
        let ast = &cx.prog.files[cx.prog.def(owner).file as usize].ast;
        if sized { self.add_implicit_sizedness_bounds(cx, owner, env, ty, bounds, &[], false, out); }
        for &b in ast.list(bounds) { self.lower_bound(cx, owner, env, ty, b, filter, out); }
    }

    /// hir_ty_lowering/bounds.rs:145-207: explicit sizedness replaces the default;
    /// `?Sized` means MetaSized, while PointeeSized imposes no predicate. A type
    /// parameter's sizedness bounds include its where-clause bounds
    /// (bounds.rs:61-85 `search_bounds_for` with `ImpliedBoundsContext::TyParam`).
    fn add_implicit_sizedness_bounds(&self, cx: &EvalCx<'_>, owner: DefId, _env: &GenEnv, ty: TyId, bounds: ast::List<Bound>, where_bounds: &[ast::List<Bound>], trait_def: bool, out: &mut Vec<Predicate>) {
        use crate::lang_items::LangItem;
        if trait_def && (self.lang_items.get(LangItem::PointeeSized) == Some(owner) || eval_ctxt::trait_is_auto(cx.prog, owner)) { return; }
        let d = cx.prog.def(owner);
        let ast = &cx.prog.files[d.file as usize].ast;
        let mut maybe_sized = false;
        let mut explicit = false;
        for &bound in std::iter::once(&bounds).chain(where_bounds).flat_map(|list| ast.list(*list)) {
            if let Bound::Trait { maybe, path, .. } = bound {
                if let Some(def) = self.resolve_type_path_def(cx.prog, d.file, d.scope, &path) {
                    match self.lang_items.by_def.get(&def) {
                        Some(LangItem::Sized) if maybe => maybe_sized = true,
                        Some(LangItem::Sized | LangItem::MetaSized | LangItem::PointeeSized) => explicit = true,
                        _ => {},
                    }
                }
            }
        }
        if explicit { return; }
        let item = if maybe_sized || trait_def { LangItem::MetaSized } else { LangItem::Sized };
        if let Some(def) = self.lang_items.get(item) { out.push(trait_predicate(def, vec![GenericArg::Type(ty)])); }
    }

    fn lower_bound(&self, cx: &EvalCx<'_>, owner: DefId, env: &GenEnv, ty: TyId, bound: Bound, filter: PredicateFilter, out: &mut Vec<Predicate>) {
        let prog = cx.prog;
        let def = prog.def(owner);
        // bounds.rs:321: filtering before lowering prevents resolving unrelated
        // associated bindings while discovering a supertrait's binder inputs.
        if let PredicateFilter::SelfTraitThatDefines(name) = filter {
            let Bound::Trait { path, .. } = bound else { return; };
            let Some(td) = self.resolve_type_path_def(prog, def.file, def.scope, &path) else { return; };
            if !self.trait_may_define_assoc_item(prog, td, name) { return; }
        }
        // `?Trait` relaxes Sized only
        if let Bound::Trait { maybe: true, path, .. } = bound {
            if self.validates(prog, def.file) {
                if let Some(td) = self.resolve_trait_path(cx, prog, def.file, def.scope, env, &path) {
                    if self.lang_items.get(crate::lang_items::LangItem::Sized) != Some(td) {
                        self.err(cx, prog, def.file, path.lo, format!("{}bound modifier `?` can only be applied to `Sized`", relax_front::diagnostics::UNCODED_PREFIX));
                    }
                    let ast = &prog.files[def.file as usize].ast;
                    if let Some(GenericArgs::Angle(v)) = ast.segs(path.segs).last().and_then(|s| ast.gargs(s.args())) {
                        for a in ast.list(*v) {
                            if let ast::GenericArg::Binding(n, _, _) | ast::GenericArg::Constraint(n, _, _) = *a {
                                let sym = prog.syms.get(prog.files[def.file as usize].text(n)).unwrap_or(u32::MAX);
                                if self.trait_assoc(prog, td, sym).is_none() {
                                    self.err(cx, prog, def.file, n.lo, format!("E0220: associated type `{}` not found for `{}`", prog.files[def.file as usize].text(n), prog.name(td)));
                                }
                            }
                        }
                    }
                }
            }
            return;
        }
        let Bound::Trait { maybe: false, path, .. } = bound else {
            if let Bound::Lifetime(lt) = bound {
                let region = env.regions.resolve(cx.infcx, &self.tys, prog, def.file, Some(lt));
                out.push(Predicate { kind: PredicateKind::Clause(ClauseKind::TypeOutlives(ty, region)), bound_vars: env.regions.bound_vars().into() });
            }
            return;
        };
        let Some(td) = self.resolve_trait_path(cx, prog, def.file, def.scope, env, &path) else { return; };
        // hir_ty_lowering/bounds.rs: PointeeSized is absence of a sizedness bound.
        if self.lang_items.get(crate::lang_items::LangItem::PointeeSized) == Some(td) { return; }
        let mut env = env.clone();
        env.regions = env.regions.with_binder(Some(BinderId::Trait(path.segs.lo)));
        let ast = &prog.files[def.file as usize].ast;
        let mut callable_output = None;
        let written = if let Some(GenericArgs::Paren(ps, output)) = ast.segs(path.segs).last().and_then(|s| ast.gargs(s.args())) {
            let (ps, output) = self.lower_fn_trait_bound_args(cx, prog, def.file, def.scope, &mut env, *ps, *output);
            callable_output = Some(output);
            vec![self.tys.intern(TyKind::Tuple(self.tys.mk_list(&ps)))]
        } else { self.lower_generic_args(cx, prog, def.file, def.scope, &env, &path) };
        let ta = self.fill_trait_defaults(cx, prog, td, ty, written);
        let regions = self.lower_region_args(cx.infcx, prog, def.file, def.scope, &env, &path);
        let mut compact = vec![ty]; compact.extend_from_slice(&ta);
        let args = self.complete_args(prog, td, &compact, regions);
        let vars: Arc<[BoundVariableKind]> = env.regions.bound_vars().into();
        let mut pred = trait_predicate(td, args);
        pred.bound_vars = vars.clone();
        out.push(pred.clone());
        // hir_ty_lowering/mod.rs:1104: this filter only needs trait clauses.
        if matches!(filter, PredicateFilter::SelfTraitThatDefines(_)) { return; }
        let ast = &prog.files[def.file as usize].ast;
        let Some(written) = ast.segs(path.segs).last().and_then(|s| ast.gargs(s.args())) else { return; };
        match written {
            GenericArgs::Paren(..) => {
                if let Some(ad) = self.callable_output(td) {
                    let value = callable_output.expect("parenthesized callable output");
                    let input_args: Vec<GenericArg> = compact.iter().map(|t| GenericArg::Type(*t)).collect();
                    self.validate_late_bound_regions(cx, prog, def.file, path.lo, &input_args, &self.tys.region_args(regions), value);
                    let alias = self.tys.intern(TyKind::Alias(IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: ad }, args: self.tys.mk_list(&compact), regions }));
                    out.push(Predicate { kind: PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: AliasTerm::Ty(alias), term: Term::Ty(value) })), bound_vars: vars });
                }
            }
            GenericArgs::Angle(bindings) => {
                for arg in ast.list(*bindings) {
                    let (name, own_args) = match *arg {
                        ast::GenericArg::Binding(name, args, _) => (name, args),
                        ast::GenericArg::Constraint(name, args, _) => (name, args),
                        _ => continue,
                    };
                    let name = prog.syms.get(prog.files[def.file as usize].text(name)).unwrap();
                    let Some(ad) = self.trait_assoc(prog, td, name) else {
                        let n = match *arg { ast::GenericArg::Binding(n, _, _) | ast::GenericArg::Constraint(n, _, _) => n, _ => continue };
                        self.err(cx, prog, def.file, n.lo, format!("E0220: associated type `{}` not found for `{}`", prog.files[def.file as usize].text(n), prog.name(td)));
                        continue;
                    };
                    // lower_assoc_item_constraint: the constraint's own arguments against the item's
                    let errs = if !self.validates(prog, def.file) { Vec::new() } else { self.check_generic_arg_count(prog, ad, ast.gargs(own_args), ast, self.own_generic_counts(prog, ad, None), crate::tcx::arg_count::ArgPosition::AssocType, false, false) };
                    if !errs.is_empty() || self.prohibit_constraints(cx, prog, def.file, own_args) {
                        let lo = match *arg { ast::GenericArg::Binding(n, _, _) | ast::GenericArg::Constraint(n, _, _) => n.lo, _ => 0 };
                        for e in errs { self.err(cx, prog, def.file, lo, e); }
                        continue;
                    }
                    let supertrait = if prog.def(ad).parent == td { pred.clone() } else { self.supertraits(prog, pred.clone()).into_iter().find(|p| matches!(&p.kind, PredicateKind::Clause(ClauseKind::Trait(c)) if c.trait_ref.def_id == prog.def(ad).parent)).expect("associated item's supertrait") };
                    let PredicateKind::Clause(ClauseKind::Trait(c)) = supertrait.kind else { unreachable!() };
                    let mut args = Vec::new();
                    let mut rs = Vec::new();
                    for a in c.trait_ref.args.clone() { match a {
                        GenericArg::Type(t) => args.push(t), GenericArg::Const(c) => args.push(self.tys.intern(TyKind::Const(c))), GenericArg::Lifetime(r) => rs.push(r),
                    } }
                    let vars = supertrait.bound_vars;
                    let (parent_args, parent_regions) = (args.len(), rs.len());
                    if let Some(GenericArgs::Angle(own)) = ast.gargs(own_args) {
                        for arg in ast.list(*own) {
                            match *arg {
                                ast::GenericArg::Lifetime(lt) => rs.push(env.regions.resolve(cx.infcx, &self.tys, prog, def.file, Some(lt))),
                                ast::GenericArg::Type(ty) => args.push(self.lower_ty(cx, prog, def.file, def.scope, &env, ty)),
                                ast::GenericArg::Const(e) => {
                                    let own_env = self.declaration_env(prog, ad);
                                    let expected = match own_env.consts.iter().find(|(index,_,_)| *index as usize == args.len()) {
                                        Some(&(_, file, ty)) => self.lower_ty(cx, prog, file, prog.def(ad).scope, &own_env, ty),
                                        None => self.tys.error,
                                    };
                                    args.push(self.tys.intern(TyKind::Const(self.lower_const_arg(prog, def.file, &env, e, expected))));
                                }
                                _ => {}
                            }
                        }
                    }
                    let (mut own_args, mut own_regions) = (args.split_off(parent_args), rs.split_off(parent_regions));
                    self.fit_assoc_own_args(prog, ad, &mut own_args, &mut own_regions);
                    args.extend(own_args); rs.extend(own_regions);
                    let alias = self.tys.intern(TyKind::Alias(IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: ad }, args: self.tys.mk_list(&args), regions: self.tys.intern_region_args(&rs) }));
                    match *arg {
                        ast::GenericArg::Binding(_, _, value) => {
                            let value = self.lower_ty(cx, prog, def.file, def.scope, &env, value);
                            let n_pos = match *arg { ast::GenericArg::Binding(n, _, _) | ast::GenericArg::Constraint(n, _, _) => n.lo, _ => 0 };
                            self.validate_late_bound_regions(cx, prog, def.file, n_pos, &c.trait_ref.args, &rs, value);
                            out.push(Predicate { kind: PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: AliasTerm::Ty(alias), term: Term::Ty(value) })), bound_vars: vars.clone() });
                        }
                        ast::GenericArg::Constraint(_, _, bounds) => {
                            if matches!(filter, PredicateFilter::All) { self.lower_bounds(cx, owner, &env, alias, bounds, false, filter, out); }
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }
    }
}

/// The bound lists of `generics`' where clauses whose subject is the type
/// parameter `name` itself (rustc_hir/src/hir.rs `WhereBoundPredicate::is_param_bound`:
/// a one-segment path resolving to that parameter).
fn param_where_bounds(prog: &Program, file: u32, generics: &ast::Generics, name: Option<crate::program::Sym>) -> Vec<ast::List<Bound>> {
    let ast = &prog.files[file as usize].ast;
    let mut out = Vec::new();
    for predicate in ast.list(generics.where_) {
        let WherePred::Bound { ty: mut subject, bounds, .. } = *predicate else { continue; };
        while let ast::Ty::Paren(inner) = ast.ty(subject) { subject = *inner; }
        let ast::Ty::Path(path) = ast.ty(subject) else { continue; };
        if path.has_qself() || path.global() || path.segs.len() != 1 { continue; }
        let segment = ast.segs(path.segs)[0];
        if !segment.args().is_none() || segment.kind() == ast::SegKind::SelfType { continue; }
        if prog.syms.get(prog.files[file as usize].text(segment.name)) == name { out.push(bounds); }
    }
    out
}
