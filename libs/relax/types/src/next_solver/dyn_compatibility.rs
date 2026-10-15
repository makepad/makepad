// ported from N:rustc_trait_selection/src/traits/dyn_compatibility.rs @8d1a764
use super::*;
use crate::tcx::Tcx;
use crate::program::{Program, DefKind};
use crate::types::{Types, TyKind, AliasTyKind, RegionKind, RegionScope, LateParamKind};
use crate::types::fold::{TypeFolder, super_fold_ty};
use crate::lang_items::LangItem;
impl Tcx {
    pub fn generics_require_sized_self(&self, prog: &Program, def: DefId) -> bool {
        let Some(sized) = self.lang_items.get(LangItem::Sized) else { return false; };
        for p in self.clauses_of(prog, def) { for p in self.supertraits(prog, p) {
            if let PredicateKind::Clause(ClauseKind::Trait(t)) = p.kind {
                if t.trait_ref.def_id == sized && matches!(t.trait_ref.args.first(), Some(GenericArg::Type(t)) if matches!(self.tys.kind(*t), TyKind::Param(0))) { return true; }
            }
        } }
        false
    }
    /// dyn_compatibility.rs:39-60 dyn_compatibility_violations (the query's provider; the
    /// query: Tcx::dyn_compatibility_violations, type_queries.rs).
    pub(crate) fn dyn_compatibility_violations_uncached(&self, prog: &Program, def: DefId) -> Vec<DefId> {
        let mut violations = Vec::new();
        let identity = collect::trait_predicate(def, self.generic_indices_of(prog, def).identity_args(&self.tys));
        let mut supers = Vec::new();
        for p in self.supertraits(prog, identity) { if let PredicateKind::Clause(ClauseKind::Trait(t)) = p.kind { if !supers.contains(&t.trait_ref.def_id) { supers.push(t.trait_ref.def_id); } } }
        for td in supers {
            if self.generics_require_sized_self(prog, td) { violations.push(td); }
            for p in self.clauses_of(prog, td) { if predicate_references_self(self, prog, td, &p, false) { violations.push(td); } }
            for &item in &prog.traits[prog.def(td).sub as usize] {
                if self.generics_require_sized_self(prog, item) { continue; }
                let d = prog.def(item);
                match d.kind {
                    DefKind::AssocConst => violations.push(item),
                    DefKind::AssocTy => {
                        let own = self.generic_indices_of(prog, item); let parent = self.generic_indices_of(prog, td);
                        if !prog.impl_trait_in_trait_data.contains_key(&item) && own.types_consts.len() + own.lifetimes.len() > parent.types_consts.len() + parent.lifetimes.len() { violations.push(item); }
                        for p in self.item_bounds(prog, item) { if predicate_references_self(self, prog, td, &p, true) { violations.push(item); } }
                    }
                    DefKind::AssocFn => {
                        let sig = self.fn_item_sig(prog, item).expect("trait method signature");
                        if sig.self_kind == 0 { violations.push(item); continue; }
                        let mut illegal = sig.header.c_variadic || sig.n_generics > sig.n_parent_generics || self.async_fns.get(&item).is_some();
                        for &t in &sig.params[1..] { illegal |= contains_illegal_self_type_reference(self, prog, td, t, true); }
                        illegal |= contains_illegal_self_type_reference(self, prog, td, sig.ret, true);
                        struct IllegalRpititVisitor<'a> { prog: &'a Program, illegal: bool }
                        impl TypeFolder for IllegalRpititVisitor<'_> {
                            fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
                                if let TyKind::Alias(_, a) = tys.kind(ty) { if matches!(a.kind, AliasTyKind::Projection { .. }) && self.prog.impl_trait_in_trait_data.contains_key(&a.kind.def_id()) { self.illegal = true; return ty; } }
                                super_fold_ty(tys, ty, self, depth)
                            }
                        }
                        let mut v = IllegalRpititVisitor { prog, illegal: false }; v.fold_ty(&self.tys, sig.ret, 0); illegal |= v.illegal;
                        // dyn_compatibility.rs:594-614: method where-clauses may not
                        // mention Self except outlives clauses and Self: AutoTrait.
                        for predicate in self.own_clauses_of(prog, item) {
                            match &predicate.kind {
                                PredicateKind::Clause(ClauseKind::TypeOutlives(..) | ClauseKind::RegionOutlives(..)) => continue,
                                PredicateKind::Clause(ClauseKind::Trait(t)) if eval_ctxt::trait_is_auto(prog, t.trait_ref.def_id) => continue,
                                _ => {}
                            }
                            struct FindSelf(bool);
                            impl TypeFolder for FindSelf {
                                fn fold_ty(&mut self, tys: &Types, t: TyId, depth: u32) -> TyId {
                                    if matches!(tys.kind(t), TyKind::Param(0)) { self.0 = true; return t; }
                                    super_fold_ty(tys, t, self, depth)
                                }
                            }
                            let mut visitor = FindSelf(false);
                            fold::fold_predicate(&self.tys, &predicate, &mut visitor);
                            illegal |= visitor.0;
                        }
                        let receiver = liberate_late_bound_regions(self, item, sig.params[0]);
                        if !matches!(self.tys.kind(receiver), TyKind::Param(0)) { illegal |= !self.receiver_is_dispatchable(prog, item, receiver); }
                        if illegal { violations.push(item); }
                    }
                    _ => {}
                }
            }
        }
        violations
    }
    fn receiver_is_dispatchable(&self, prog: &Program, method: DefId, receiver: TyId) -> bool {
        if self.tys.has_error(receiver) { return false; }
        let (Some(unsize), Some(dispatch)) = (self.lang_items.get(LangItem::Unsize), self.lang_items.get(LangItem::DispatchFromDyn)) else { return false; };
        let u = self.tys.intern(TyKind::Param(u32::MAX));
        let mut args = self.generic_indices_of(prog, method).identity_args(&self.tys); args[0] = GenericArg::Type(u);
        let target = self.instantiate_ty(prog, method, &args, receiver);
        let td = prog.def(method).parent;
        let mut clauses = self.clauses_of(prog, method);
        clauses.push(collect::trait_predicate(unsize, vec![GenericArg::Type(self.tys.intern(TyKind::Param(0))), GenericArg::Type(u)]));
        let mut trait_args = self.generic_indices_of(prog, td).identity_args(&self.tys); trait_args[0] = GenericArg::Type(u);
        clauses.push(collect::trait_predicate(td, trait_args.clone()));
        for p in self.clauses_of(prog, td) { clauses.push(self.instantiate_predicate(prog, td, &trait_args, &p)); }
        let infcx = crate::infer::InferCtxt::for_item(self, prog, crate::tcx::TypingMode::non_body_analysis(), method);

        // dyn_compatibility.rs:756-767: MetaSized is required; normalize this
        // newly constructed environment before testing receiver dispatch.
        let Some(meta_sized) = self.lang_items.get(LangItem::MetaSized) else {
            // require_lang_item (rustc_middle/src/middle/lang_items.rs) reports a fatal missing-lang-item error.
            self.dcx.report(infcx.cause, "requires `meta_sized` lang item".into());
            return false;
        };
        clauses.push(collect::trait_predicate(meta_sized, vec![GenericArg::Type(u)]));
        let env = super::param_env::normalize_param_env_or_error(self, prog,
            ParamEnv { clauses: clauses.into() }, infcx.cause);
        let env = self.intern_param_env(env);
        let goal = Goal { param_env: env, predicate: collect::trait_predicate(dispatch, vec![GenericArg::Type(receiver), GenericArg::Type(target)]) };
        infcx.evaluate_root_goal(prog, goal, None).is_ok_and(|r| r.certainty.is_yes())
    }
}
fn predicate_references_self(tcx: &Tcx, prog: &Program, td: DefId, p: &Predicate, allow: bool) -> bool {
    let args = match &p.kind {
        PredicateKind::Clause(ClauseKind::Trait(t)) => t.trait_ref.args[1..].to_vec(),
        PredicateKind::Clause(ClauseKind::Projection(p)) => match &p.projection_term {
            AliasTerm::Ty(t) => { let TyKind::Alias(_, a) = tcx.tys.kind(*t) else { unreachable!() }; tcx.complete_args(prog, a.kind.def_id(), tcx.tys.list(a.args), a.regions)[1..].to_vec() }
            AliasTerm::Const(a) => a.args[1..].to_vec(),
        },
        PredicateKind::Clause(ClauseKind::ConstArgHasType(_, t)) => vec![GenericArg::Type(*t)],
        _ => return false,
    };
    for a in args { let ty = match a { GenericArg::Type(t) => t, GenericArg::Const(c) => tcx.tys.intern(TyKind::Const(c)), GenericArg::Lifetime(_) => continue }; if contains_illegal_self_type_reference(tcx, prog, td, ty, allow) { return true; } }
    false
}
fn contains_illegal_self_type_reference(tcx: &Tcx, prog: &Program, td: DefId, ty: TyId, allow: bool) -> bool {
    struct IllegalSelfTypeVisitor<'a> { tcx: &'a Tcx, prog: &'a Program, supers: Vec<TraitRef>, allow: bool, illegal: bool }
    impl TypeFolder for IllegalSelfTypeVisitor<'_> {
        fn fold_ty(&mut self, tys: &Types, t: TyId, depth: u32) -> TyId {
            if matches!(tys.kind(t), TyKind::Param(0)) { self.illegal = true; return t; }
            if let TyKind::Alias(_, a) = tys.kind(t) { if let AliasTyKind::Projection { def_id } = a.kind {
                if self.prog.impl_trait_in_trait_data.contains_key(&def_id) { return t; }
                if self.allow {
                    let parent = self.prog.def(def_id).parent;
                    let mut args = self.tcx.complete_args(self.prog, def_id, tys.list(a.args), a.regions);
                    let n = self.tcx.generic_indices_of(self.prog, parent); args.truncate(n.types_consts.len() + n.lifetimes.len());
                    erase_args(tys, &mut args);
                    if self.supers.contains(&TraitRef { def_id: parent, args }) { return t; }
                }
            } }
            super_fold_ty(tys, t, self, depth)
        }
    }
    let mut supers = Vec::new();
    for p in tcx.supertraits(prog, collect::trait_predicate(td, tcx.generic_indices_of(prog, td).identity_args(&tcx.tys))) {
        if let PredicateKind::Clause(ClauseKind::Trait(mut p)) = p.kind { erase_args(&tcx.tys, &mut p.trait_ref.args); supers.push(p.trait_ref); }
    }
    let mut v = IllegalSelfTypeVisitor { tcx, prog, supers, allow, illegal: false }; v.fold_ty(&tcx.tys, ty, 0); v.illegal
}
fn erase_args(tys: &Types, args: &mut [GenericArg]) {
    for a in args { match a { GenericArg::Lifetime(r) => *r = crate::types::RegionId::ERASED, GenericArg::Type(t) => *t = tys.erase_regions(*t), GenericArg::Const(_) => {} } }
}
pub(super) fn liberate_late_bound_regions(tcx: &Tcx, owner: DefId, ty: TyId) -> TyId {
    let mut replacer = crate::types::fold::BoundVarReplacer::new(|var: crate::types::BoundVar| tcx.tys.intern_region(RegionKind::ReLateParam(RegionScope::Item(owner), LateParamKind::Anon(var.0))), |_| panic!("non-lifetime binder"));
    replacer.fold_ty(&tcx.tys, ty, 0)
}
impl eval_ctxt::EvalCtxt<'_, '_> {
    pub(super) fn check_dyn_compatible(&self, def: DefId) -> Result<(), NoSolution> {
        if self.delegate.tcx.dyn_compatibility_violations(self.solver.prog, def).is_empty() { Ok(()) } else { Err(NoSolution) }
    }
}
