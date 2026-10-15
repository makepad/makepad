// ported from N:rustc_trait_selection/src/traits/vtable.rs @8d1a764
use super::*;
use crate::program::{Program, DefKind};
use crate::tcx::{Tcx, TypingEnv};
use crate::types::{TyKind, RegionId};

#[derive(Clone, Debug)]
pub enum VtblSegment {
    MetadataDSA,
    TraitOwnEntries { trait_ref: TraitRef, emit_vptr: bool },
}

impl Tcx {
    /// rustc_middle/src/ty/fold.rs:321-326 @8d1a764, specialized to a
    /// trait binder's payload. Nested binders retain their own regions.
    pub fn instantiate_bound_regions_with_erased(&self, value: TraitRef) -> TraitRef {
        let mut replacer = crate::types::fold::BoundVarReplacer::new(
            |_| RegionId::ERASED, |_| panic!("bound type in a trait region binder"));
        let mut args = Vec::with_capacity(value.args.len());
        for arg in value.args { args.push(fold::fold_arg(&self.tys, arg, &mut replacer, 0)); }
        TraitRef { def_id: value.def_id, args }
    }

    /// vtable.rs own_existential_vtable_entries (the query's provider; the query:
    /// Tcx::own_existential_vtable_entries, type_queries.rs).
    pub(crate) fn own_existential_vtable_entries_uncached(&self, prog: &Program, td: DefId) -> Vec<DefId> {
        let violations = self.dyn_compatibility_violations(prog, td);
        let mut result = Vec::new();
        for &method in &prog.traits[prog.def(td).sub as usize] {
            if prog.def(method).kind != DefKind::AssocFn { continue; }
            // Final trait methods are feature-gated; specialization is rejected at collection.
            if self.generics_require_sized_self(prog, method) || violations.contains(&method) { continue; }
            result.push(method);
        }
        result
    }

    fn direct_supertraits(&self, prog: &Program, tr: &TraitRef) -> Vec<TraitRef> {
        let mut supers = Vec::new();
        for predicate in self.explicit_super_predicates_of(prog, tr.def_id) {
            let predicate = self.instantiate_predicate(prog, tr.def_id, &tr.args, &predicate);
            let PredicateKind::Clause(ClauseKind::Trait(clause)) = predicate.kind else { continue };
            let mut tr = clause.trait_ref;
            for arg in &mut tr.args {
                *arg = match *arg {
                    GenericArg::Lifetime(_) => GenericArg::Lifetime(RegionId::ERASED),
                    GenericArg::Type(t) => GenericArg::Type(self.normalize_erasing_late_bound_regions(prog, TypingEnv::fully_monomorphized(&prog), t)),
                    GenericArg::Const(c) => GenericArg::Const(self.tys.expect_const(self.normalize_erasing_late_bound_regions(prog, TypingEnv::fully_monomorphized(&prog), self.tys.intern(TyKind::Const(c))))),
                };
            }
            supers.push(tr);
        }
        supers
    }

    /// vtable.rs:24-197: the first supertrait is a prefix; later nonempty branches
    /// carry vptrs. The stack retains each node's unvisited siblings.
    pub fn prepare_vtable_segments(&self, prog: &Program, tr: TraitRef) -> Vec<VtblSegment> {
        let mut result = vec![VtblSegment::MetadataDSA];
        let mut emit_vptr_on_new_entry = false;
        let mut visited = vec![tr.clone()];
        let mut stack = vec![(tr, false, Vec::<TraitRef>::new())];
        loop {
            loop {
                let tr = &stack.last().unwrap().0;
                let mut supers = self.direct_supertraits(prog, tr);
                let Some(index) = supers.iter().position(|tr| !visited.contains(tr)) else { break };
                let next = supers[index].clone();
                supers.drain(..=index);
                visited.push(next.clone());
                stack.push((next, emit_vptr_on_new_entry, supers));
            }
            let mut sibling = false;
            while let Some((tr, emit_vptr, mut siblings)) = stack.pop() {
                let mut has_entries = false;
                for predicate in self.supertraits(prog, collect::trait_predicate(tr.def_id, tr.args.clone())) {
                    if let PredicateKind::Clause(ClauseKind::Trait(t)) = predicate.kind {
                        has_entries |= !self.own_existential_vtable_entries(prog, t.trait_ref.def_id).is_empty();
                    }
                }
                result.push(VtblSegment::TraitOwnEntries { trait_ref: tr, emit_vptr: emit_vptr && has_entries });
                emit_vptr_on_new_entry |= has_entries;
                if let Some(index) = siblings.iter().position(|tr| !visited.contains(tr)) {
                    let next = siblings[index].clone(); siblings.drain(..=index);
                    visited.push(next.clone()); stack.push((next, emit_vptr_on_new_entry, siblings));
                    sibling = true; break;
                }
            }
            if !sibling { return result; }
        }
    }

    /// vtable.rs:300-335 first_method_vtable_slot (the query's provider; the query:
    /// Tcx::first_method_vtable_slot, type_queries.rs).
    pub(crate) fn first_method_vtable_slot_uncached(&self, prog: &Program, key: &TraitRef) -> usize {
        let GenericArg::Type(self_ty) = key.args[0] else { unreachable!() };
        let TyKind::Dyn(def, args, _, regions, _, _, _) = self.tys.kind(self_ty) else { panic!("virtual receiver") };
        let mut compact = vec![self_ty]; compact.extend_from_slice(self.tys.list(*args));
        // vtable.rs:319-321: open the principal's binder before slot lookup.
        let source = self.instantiate_bound_regions_with_erased(TraitRef { def_id: *def, args: self.complete_args(prog, *def, &compact, *regions) });
        if self.instantiate_and_check_impossible_clauses(prog, source.def_id, &source.args) { return 0; }
        let mut offset = 0;
        for segment in self.prepare_vtable_segments(prog, source) {
            match segment {
                VtblSegment::MetadataDSA => offset += 3,
                VtblSegment::TraitOwnEntries { trait_ref, emit_vptr } => {
                    if trait_ref == *key { return offset; }
                    offset += self.own_existential_vtable_entries(prog, trait_ref.def_id).len();
                    if emit_vptr { offset += 1; }
                }
            }
        }
        panic!("virtual trait not in principal supertraits");
    }

    /// rustc_trait_selection/src/traits/vtable.rs:361-442.
    pub fn supertrait_vtable_slot(&self, prog: &Program, source: TyId, target: TyId) -> Option<usize> {
        let TyKind::Dyn(td, ta, _, tr, _, _, _) = self.tys.kind(target) else { panic!("upcast target") };
        if *td == crate::program::NO_DEF || super::eval_ctxt::trait_is_auto(prog, *td) { return None; }
        let TyKind::Dyn(sd, sa, _, sr, _, _, _) = self.tys.kind(source) else { panic!("upcast source") };
        let mut args = vec![source]; args.extend_from_slice(self.tys.list(*sa));
        let principal = self.instantiate_bound_regions_with_erased(TraitRef { def_id: *sd, args: self.complete_args(prog, *sd, &args, *sr) });
        if self.instantiate_and_check_impossible_clauses(prog, principal.def_id, &principal.args) { return None; }
        let mut args = vec![source]; args.extend_from_slice(self.tys.list(*ta));
        let target = self.instantiate_bound_regions_with_erased(TraitRef { def_id: *td, args: self.complete_args(prog, *td, &args, *tr) });
        let mut offset = 0;
        for segment in self.prepare_vtable_segments(prog, principal) {
            match segment {
                VtblSegment::MetadataDSA => offset += 3,
                VtblSegment::TraitOwnEntries { trait_ref, emit_vptr } => {
                    offset += self.own_existential_vtable_entries(prog, trait_ref.def_id).len();
                    if trait_ref == target { return emit_vptr.then_some(offset); }
                    if emit_vptr { offset += 1; }
                }
            }
        }
        self.dcx.delayed_bug(Default::default(), "supertrait vtable slot missing for a well-formed upcast".into());
        None
    }

    /// traits/mod.rs:846-901: errors prove impossibility; ambiguity does not.
    pub fn instantiate_and_check_impossible_clauses(&self, prog: &Program, def: DefId, args: &[GenericArg]) -> bool {
        let mut clauses = Vec::new();
        for p in self.clauses_of(prog, def) { clauses.push(self.instantiate_predicate(prog, def, args, &p)); }
        let parent = prog.def(def).parent;
        if parent != crate::program::NO_DEF && prog.def(parent).kind == DefKind::Trait {
            let indices = self.generic_indices_of(prog, parent);
            clauses.push(collect::trait_predicate(parent, args[..indices.types_consts.len() + indices.lifetimes.len()].to_vec()));
        }
        let infcx = crate::infer::InferCtxt::new(self, crate::tcx::TypingMode::PostAnalysis, TypingEnv::fully_monomorphized(prog).limit);

        let mut ocx = fulfill::ObligationCtxt::new(&infcx, prog);
        let env = ParamEnvId::EMPTY;
        for p in clauses {
            struct HasParam(bool);
            impl crate::types::fold::TypeFolder for HasParam {
                fn fold_ty(&mut self, tys: &crate::types::Types, t: TyId, depth: u32) -> TyId {
                    self.0 |= matches!(tys.kind(t), TyKind::Param(_));
                    crate::types::fold::super_fold_ty(tys, t, self, depth)
                }
                fn fold_region(&mut self, tys: &crate::types::Types, r: RegionId, _: u32) -> RegionId {
                    self.0 |= matches!(tys.region_kind(r), crate::types::RegionKind::ReEarlyParam(_));
                    r
                }
                fn fold_const(&mut self, tys: &crate::types::Types, c: crate::consts::Const, depth: u32) -> crate::consts::Const {
                    self.0 |= matches!(tys.const_kind(c), crate::consts::ConstKind::Param(_));
                    crate::types::fold::super_fold_const(tys, c, self, depth)
                }
            }
            let mut has = HasParam(false); fold::fold_predicate(&self.tys, &p, &mut has);
            if has.0 { continue; }
            let p = ocx.normalize_predicate(&env, &p);
            ocx.register_obligation(Goal { param_env: env.clone(), predicate: p });
        }
        ocx.engine.try_evaluate_obligations(&infcx, prog).iter().any(|e| matches!(e, fulfill::NextSolverError::TrueError(_)))
    }
}
