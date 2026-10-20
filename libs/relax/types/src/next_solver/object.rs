// ported from N:rustc_next_trait_solver/src/solve/assembly/mod.rs @8d1a764
// ported from N:rustc_next_trait_solver/src/solve/assembly/structural_traits.rs @8d1a764
use super::*;
use super::assembly::*;
use super::eval_ctxt::EvalCtxt;
use crate::program::{Program, DefKind, NO_DEF};
use crate::tcx::Tcx;
use crate::types::{TyKind, AliasTyKind, AliasTy, Types};
use crate::types::fold::{TypeFolder, super_fold_ty};
use crate::consts::IsRigid;
impl Tcx {
    /// Does `t` mention `needle` (a type, structurally)?
    fn mentions_ty(&self, t: TyId, needle: TyId) -> bool {
        struct Finder { needle: TyId, found: bool }
        impl TypeFolder for Finder {
            fn fold_ty(&mut self, tys: &Types, t: TyId, depth: u32) -> TyId {
                if t == self.needle { self.found = true; return t; }
                if !self.found { super_fold_ty(tys, t, self, depth); }
                t
            }
        }
        let mut f = Finder { needle, found: false };
        f.fold_ty(&self.tys, t, 0);
        f.found
    }
    pub fn alias_ty_with_args(&self, kind: AliasTyKind, args: &[GenericArg]) -> TyId {
        let mut types = Vec::new(); let mut regions = Vec::new();
        for &a in args { match a { GenericArg::Lifetime(r) => regions.push(r), GenericArg::Type(t) => types.push(t), GenericArg::Const(c) => types.push(self.tys.intern(TyKind::Const(c))) } }
        self.tys.intern(TyKind::Alias(IsRigid::No, AliasTy { kind, args: self.tys.mk_list(&types), regions: self.tys.intern_region_args(&regions) }))
    }
    pub fn object_predicates(&self, prog: &Program, ty: TyId) -> Vec<Predicate> {
        let TyKind::Dyn(principal, args, bounds, rs, _, n, autos) = self.tys.kind(ty) else { unreachable!() };
        let mut predicates = Vec::new();
        if *principal != NO_DEF {
            let mut compact = vec![ty]; compact.extend_from_slice(self.tys.list(*args));
            let args = self.complete_args(prog, *principal, &compact, *rs);
            let mut p = collect::trait_predicate(*principal, args); p.bound_vars = vec![BoundVariableKind::Region(crate::types::BoundRegionKind::Anon); *n as usize].into();
            predicates.push(p);
        }
        for &auto in self.tys.list(*autos) { predicates.push(collect::trait_predicate(auto, vec![GenericArg::Type(ty)])); }
        for &(def, term, n) in self.tys.list(*bounds) {
            let owner = prog.def(def).parent;
            let mut tr = None;
            if *principal != NO_DEF { for p in self.supertraits(prog, predicates[0].clone()) {
                if let PredicateKind::Clause(ClauseKind::Trait(t)) = p.kind { if t.trait_ref.def_id == owner { tr = Some(t.trait_ref); break; } }
            } }
            let tr = tr.expect("object projection's defining trait");
            let alias = self.alias_ty_with_args(AliasTyKind::Projection { def_id: def }, &tr.args);
            predicates.push(Predicate { kind: PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: AliasTerm::Ty(alias), term: Term::Ty(term) })), bound_vars: vec![BoundVariableKind::Region(crate::types::BoundRegionKind::Anon); n as usize].into() });
        }
        predicates
    }
}
impl EvalCtxt<'_, '_> {
    pub(super) fn assemble_object_bound_candidates(&mut self, env: &ParamEnvId, goal: &GoalKind, candidates: &mut Vec<Candidate>) -> Result<(), NoSolutionOrRerunNonErased> {
        let tcx = self.delegate.tcx;
        if [crate::lang_items::LangItem::Sized, crate::lang_items::LangItem::MetaSized, crate::lang_items::LangItem::PointeeSized].iter().any(|&item| tcx.lang_items.get(item) == Some(goal.trait_ref(self).def_id)) || matches!(self.delegate.typing_mode.get(), crate::tcx::TypingMode::Reflection) { return Ok(()); }
        let ty = goal.self_ty(self);
        let TyKind::Dyn(principal, ..) = tcx.tys.kind(ty) else { return Ok(()); };
        if *principal != NO_DEF && self.check_dyn_compatible(*principal).is_err() { return Ok(()); }
        let bounds = tcx.object_predicates(self.solver.prog, ty);
        // rustc lowers the projections the principal's supertraits fix into the object type itself
        // (dyn_trait.rs:225-285, unless the term mentions Self); the candidate's requirements are
        // rewritten with all of them, so `<dyn Foo as Sup>::A` of `trait Foo: Sup<A = u8>` is `u8`.
        let mut replacements = bounds.clone();
        if *principal != NO_DEF {
            for p in tcx.supertraits(self.solver.prog, bounds[0].clone()) {
                let PredicateKind::Clause(ClauseKind::Projection(proj)) = &p.kind else { continue };
                if replacements.iter().any(|r| matches!(&r.kind, PredicateKind::Clause(ClauseKind::Projection(q)) if q.projection_term == proj.projection_term)) { continue; }
                let Term::Ty(term) = proj.term else { continue };
                if tcx.mentions_ty(term, ty) { continue; }
                replacements.push(p);
            }
        }
        for (i, p) in bounds.iter().enumerate() {
            if i == 0 && *principal != NO_DEF {
                for (index, p) in tcx.supertraits(self.solver.prog, p.clone()).into_iter().enumerate() { self.probe_and_consider_object_bound_candidate(env, goal, &p, CandidateSource::BuiltinImpl(BuiltinImplSource::Object(index)), &replacements, candidates)?; }
            } else { self.probe_and_consider_object_bound_candidate(env, goal, p, CandidateSource::BuiltinImpl(BuiltinImplSource::Misc), &replacements, candidates)?; }
        }
        Ok(())
    }
    fn probe_and_consider_object_bound_candidate(&mut self, env: &ParamEnvId, goal: &GoalKind, p: &Predicate, source: CandidateSource, bounds: &[Predicate], candidates: &mut Vec<Candidate>) -> Result<(), NoSolutionOrRerunNonErased> {
        if !self.assumption_matches_kind(goal, p) { return Ok(()); }
        let (result, head_usages) = self.probe().enter_single_candidate(source, |ecx| {
            if matches!(goal, GoalKind::NormalizesTo(_)) {
                ecx.match_assumption(env, goal, p)?;
                // normalizes_to.rs:235-243 deliberately overrides the trait
                // candidate's supertrait checks for old-solver compatibility.
                return ecx.evaluate_added_goals_and_make_canonical_response(Certainty::Yes);
            }
            let PredicateKind::Clause(ClauseKind::Trait(t)) = ecx.delegate.instantiate_predicate_binder_with_infer(p) else { unreachable!() };
            ecx.eq_args(env, &goal.trait_ref(ecx).args, &t.trait_ref.args)?;
            let certainty = if ecx.predicates_for_object_candidate(env, &t.trait_ref, bounds)? { Certainty::Yes } else { Certainty::AMBIGUOUS };
            ecx.evaluate_added_goals_and_make_canonical_response(certainty)
        });
        if let Ok(result) = map_err_to_rerun(result)? { candidates.push(Candidate { source, result, head_usages }); }
        Ok(())
    }
    fn predicates_for_object_candidate(&mut self, env: &ParamEnvId, tr: &TraitRef, bounds: &[Predicate]) -> Result<bool, NoSolutionOrRerunNonErased> {
        let tcx = self.delegate.tcx; let prog = self.solver.prog; let mut requirements = Vec::new();
        for p in tcx.explicit_super_predicates_of(prog, tr.def_id) {
            let p = tcx.instantiate_predicate(prog, tr.def_id, &tr.args, &p);
            requirements.extend(tcx.supertraits(prog, p));
        }
        for &item in &prog.traits[prog.def(tr.def_id).sub as usize] {
            if prog.def(item).kind != DefKind::AssocTy || tcx.generics_require_sized_self(prog, item) { continue; }
            for p in tcx.item_bounds(prog, item) { requirements.push(tcx.instantiate_predicate(prog, item, &tr.args, &p)); }
        }
        let GenericArg::Type(self_ty) = tr.args[0] else { unreachable!() };
        let mut folder = ReplaceProjectionWith { ecx: self, env, self_ty, mapping: bounds, ambiguous: false, failed: false };
        let mut mapped = Vec::new();
        for p in requirements { mapped.push(fold::fold_predicate(&tcx.tys, &p, &mut folder)); }
        if folder.failed { return Err(NoSolution.into()); }
        if folder.ambiguous { return Ok(false); }
        for p in mapped { self.add_goal(GoalSource::ImplWhereBound, Goal { param_env: env.clone(), predicate: p })?; }
        Ok(true)
    }
}
struct ReplaceProjectionWith<'a, 'b, 'tcx> { ecx: &'a mut EvalCtxt<'b, 'tcx>, env: &'a ParamEnvId, self_ty: TyId, mapping: &'a [Predicate], ambiguous: bool, failed: bool }
impl TypeFolder for ReplaceProjectionWith<'_, '_, '_> {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if let TyKind::Alias(_, a) = tys.kind(ty) {
            if matches!(a.kind, AliasTyKind::Projection { .. }) && tys.list(a.args)[0] == self.self_ty {
                // rustc ReplaceProjectionWith::try_eagerly_replace_alias: the projection bounds of
                // the object that may match the alias; two are ambiguous, one replaces it. (A
                // projection no bound matches is left alone: the object type is already in error.)
                let mut matching = None; let target = self.ecx.alias_args(&AliasTerm::Ty(ty));
                for p in self.mapping {
                    let PredicateKind::Clause(ClauseKind::Projection(source)) = &p.kind else { continue; };
                    if self.ecx.alias_args(&source.projection_term).0 != target.0 { continue; }
                    let result = self.ecx.probe().enter_without_propagated_nested_goals(|ecx| {
                        let PredicateKind::Clause(ClauseKind::Projection(p)) = ecx.delegate.instantiate_predicate_binder_with_infer(p) else { unreachable!() };
                        ecx.eq_args(self.env, &ecx.alias_args(&p.projection_term).1, &target.1)?;
                        ecx.try_evaluate_added_goals()
                    });
                    if result.is_ok() { if matching.is_some() { self.ambiguous = true; return ty; } matching = Some(p); }
                }
                if let Some(p) = matching {
                    let PredicateKind::Clause(ClauseKind::Projection(p)) = self.ecx.delegate.instantiate_predicate_binder_with_infer(p) else { unreachable!() };
                    if self.ecx.eq_args(self.env, &self.ecx.alias_args(&p.projection_term).1, &target.1).is_err() { self.failed = true; return ty; }
                    let Term::Ty(value) = p.term else { unreachable!() }; return value;
                }
            }
        }
        super_fold_ty(tys, ty, self, depth)
    }
}
