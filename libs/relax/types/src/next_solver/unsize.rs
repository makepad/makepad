// ported from N:rustc_next_trait_solver/src/solve/trait_goals.rs @8d1a764
// ported from N:rustc_ty_utils/src/ty.rs @8d1a764
use super::*;
use super::assembly::*;
use super::eval_ctxt::EvalCtxt;
use crate::types::{Types, TyKind};
use crate::types::fold::{TypeFolder, super_fold_ty, super_fold_const};
use crate::lang_items::LangItem;
use crate::program::NO_DEF;

impl EvalCtxt<'_, '_> {
    pub(super) fn consider_structural_builtin_unsize_candidates(&mut self, env: &ParamEnvId, tr: &TraitRef, candidates: &mut Vec<Candidate>) -> Result<(), NoSolutionOrRerunNonErased> {
        let GenericArg::Type(a) = tr.args[0] else { unreachable!() };
        let GenericArg::Type(b) = tr.args[1] else { unreachable!() };
        let result = self.probe().enter_with_kind(|ecx| {
            ecx.inspect.probe_kind(super::inspect::ProbeKind::UnsizeAssembly);
            let Term::Ty(b) = ecx.structurally_normalize_term(env,Term::Ty(b))? else { unreachable!() };
            let tcx = ecx.delegate.tcx;
            let mut results = Vec::new();
            if let (TyKind::Dyn(source_principal,..), TyKind::Dyn(target_principal,..)) = (tcx.tys.kind(a),tcx.tys.kind(b)) {
                let source_bounds = tcx.object_predicates(ecx.solver.prog,a);
                // a principal-less object (`dyn Send + Sync`) carries its first auto trait as `principal`
                // (Dyn's encoding): rustc's `principal_def_id()` is None for it
                let prog = ecx.solver.prog;
                let real = |p: &DefId| *p != NO_DEF && !super::eval_ctxt::trait_is_auto(prog, *p);
                let (source_principal, target_principal) = (if real(source_principal) { *source_principal } else { NO_DEF }, if real(target_principal) { *target_principal } else { NO_DEF });
                let principal = source_bounds.iter().find(|p| matches!(&p.kind, PredicateKind::Clause(ClauseKind::Trait(t)) if t.trait_ref.def_id == source_principal));
                // trait_goals.rs:986-1040: identity/removing auto traits is Misc;
                // upcast indices count the full principal-supertrait traversal.
                let mut principals = Vec::new();
                if source_principal == target_principal || target_principal == NO_DEF {
                    principals.push((BuiltinImplSource::Misc, principal.cloned()));
                } else if let Some(principal) = principal {
                    let supers = tcx.supertraits(ecx.solver.prog, principal.clone());
                    let mut index = 0;
                    for p in supers {
                        if !matches!(p.kind, PredicateKind::Clause(ClauseKind::Trait(_))) { continue; }
                        if index != 0 { principals.push((BuiltinImplSource::TraitUpcasting(index), Some(p))); }
                        index += 1;
                    }
                }
                for (source, principal) in principals {
                    let source = CandidateSource::BuiltinImpl(source);
                    let (r,head_usages) = ecx.probe().enter_single_candidate(source, |ecx| ecx.consider_builtin_upcast_to_principal(env,a,b,&source_bounds,principal.as_ref()));
                    if let Ok(result) = map_err_to_rerun(r)? { results.push(Candidate { source, result, head_usages }); }
                }
                return Ok(results);
            }
            let (r,head_usages) = ecx.probe().enter_single_candidate(CandidateSource::BuiltinImpl(BuiltinImplSource::Misc), |ecx| {
                match (tcx.tys.kind(a),tcx.tys.kind(b)) {
                    (_,TyKind::Infer(_)) => return ecx.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS),
                    (_,TyKind::Dyn(principal,_,_,_,region,..)) => {
                        if *principal != NO_DEF { ecx.check_dyn_compatible(*principal)?; }
                        for p in tcx.object_predicates(ecx.solver.prog,b) {
                            let p = fold::fold_predicate(&tcx.tys,&p,&mut ReplaceObjectSelf { from:b,to:a });
                            ecx.add_goal(GoalSource::ImplWhereBound,Goal { param_env:env.clone(),predicate:p })?;
                        }
                        ecx.add_trait_goal(env,tcx.lang_items.get(LangItem::Sized).ok_or(NoSolution)?,a)?;
                        ecx.add_goal(GoalSource::Misc,Goal { param_env:env.clone(),predicate:Predicate::dummy(PredicateKind::Clause(ClauseKind::TypeOutlives(a,*region))) })?;
                    }
                    (TyKind::Array(a,_),TyKind::Slice(b)) => ecx.eq(env,Term::Ty(*a),Term::Ty(*b))?,
                    (TyKind::Adt(ad,aa,ar),TyKind::Adt(bd,ba,br)) if ad==bd && !tcx.adts[ad].is_enum && !tcx.adts[ad].is_union => {
                        let params = tcx.unsizing_params_for_adt(*ad);
                        if params.is_empty() { return Err(NoSolution.into()); }
                        let fields = &tcx.adts[ad].variants[0].fields;
                        let tail = fields.last().unwrap().ty;
                        let a_full = tcx.complete_args(ecx.solver.prog,*ad,tcx.tys.list(*aa),*ar); let b_full = tcx.complete_args(ecx.solver.prog,*bd,tcx.tys.list(*ba),*br);
                        let at = tcx.instantiate_ty(ecx.solver.prog,*ad,&a_full,tail); let bt = tcx.instantiate_ty(ecx.solver.prog,*bd,&b_full,tail);
                        let mut new_args = tcx.tys.list(*aa).to_vec(); for &i in &params { new_args[i as usize] = tcx.tys.list(*ba)[i as usize]; }
                        let changed = tcx.tys.intern(TyKind::Adt(*ad,tcx.tys.mk_list(&new_args),*ar));
                        ecx.eq(env,Term::Ty(changed),Term::Ty(b))?;
                        ecx.add_goal(GoalSource::ImplWhereBound,Goal { param_env:env.clone(),predicate:collect::trait_predicate(tr.def_id,vec![GenericArg::Type(at),GenericArg::Type(bt)]) })?;
                    }
                    _ => return Err(NoSolution.into()),
                }
                ecx.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
            });
            if let Ok(result) = map_err_to_rerun(r)? { results.push(Candidate { source:CandidateSource::BuiltinImpl(BuiltinImplSource::Misc),result,head_usages }); }
            Ok(results)
        });
        if let Ok(results) = map_err_to_rerun(result)? { candidates.extend(results); }
        Ok(())
    }
    fn consider_builtin_upcast_to_principal(&mut self, env: &ParamEnvId, a: TyId, b: TyId, source_bounds: &[Predicate], principal: Option<&Predicate>) -> QueryResultOrRerunNonErased {
        let tcx = self.delegate.tcx;
        let TyKind::Dyn(_,_,_,_,ar,..) = tcx.tys.kind(a) else { unreachable!() };
        let TyKind::Dyn(_,_,_,_,br,..) = tcx.tys.kind(b) else { unreachable!() };
        let mut source = Vec::new(); for p in source_bounds { source.extend(tcx.supertraits(self.solver.prog,p.clone())); }
        for target in tcx.object_predicates(self.solver.prog,b) {
            let target = fold::fold_predicate(&tcx.tys,&target,&mut ReplaceObjectSelf { from:b,to:a });
            let target_kind = self.delegate.enter_forall_predicate_and_leak_universe(&target);
            match target_kind {
                PredicateKind::Clause(ClauseKind::Trait(t)) if super::eval_ctxt::trait_is_auto(self.solver.prog,t.trait_ref.def_id) => {
                    if !source.iter().any(|p| matches!(&p.kind,PredicateKind::Clause(ClauseKind::Trait(s)) if s.trait_ref.def_id==t.trait_ref.def_id)) { return Err(NoSolution.into()); }
                }
                PredicateKind::Clause(ClauseKind::Trait(t)) => {
                    let p = principal.ok_or(NoSolution)?;
                    let PredicateKind::Clause(ClauseKind::Trait(s)) = self.delegate.instantiate_predicate_binder_with_infer(p) else { unreachable!() };
                    if s.trait_ref.def_id != t.trait_ref.def_id { return Err(NoSolution.into()); }
                    self.eq_args(env,&s.trait_ref.args,&t.trait_ref.args)?;
                    self.try_evaluate_added_goals()?;
                }
                PredicateKind::Clause(ClauseKind::Projection(t)) => {
                    let mut matching = None;
                    for p in source_bounds {
                        let PredicateKind::Clause(ClauseKind::Projection(s)) = &p.kind else { continue; };
                        if self.alias_args(&s.projection_term).0 != self.alias_args(&t.projection_term).0 { continue; }
                        let r = self.probe().enter(|ecx| {
                            let PredicateKind::Clause(ClauseKind::Projection(s)) = ecx.delegate.instantiate_predicate_binder_with_infer(p) else { unreachable!() };
                            ecx.eq_args(env,&ecx.alias_args(&s.projection_term).1,&ecx.alias_args(&t.projection_term).1)?;
                            ecx.eq(env,s.term,t.term)?; ecx.try_evaluate_added_goals()
                        });
                        if map_err_to_rerun(r)?.is_ok() {
                            if matching.is_some() { return self.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS); }
                            matching = Some(p);
                        }
                    }
                    let p = matching.ok_or(NoSolution)?;
                    let PredicateKind::Clause(ClauseKind::Projection(s)) = self.delegate.instantiate_predicate_binder_with_infer(p) else { unreachable!() };
                    self.eq_args(env,&self.alias_args(&s.projection_term).1,&self.alias_args(&t.projection_term).1)?;
                    self.eq(env,s.term,t.term)?; self.try_evaluate_added_goals()?;
                }
                _ => unreachable!(),
            }
        }
        self.add_goal(GoalSource::ImplWhereBound,Goal { param_env:env.clone(),predicate:Predicate::dummy(PredicateKind::Clause(ClauseKind::RegionOutlives(*ar,*br))) })?;
        self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
    }
}
struct ReplaceObjectSelf { from: TyId, to: TyId }
impl TypeFolder for ReplaceObjectSelf {
    fn fold_ty(&mut self, tys: &Types,t:TyId,d:u32)->TyId { if t==self.from { tys.shift_in(self.to,d) } else { super_fold_ty(tys,t,self,d) } }
}
impl crate::tcx::Tcx {
    pub fn unsizing_params_for_adt(&self,def:DefId)->Vec<u32> {
        let adt = &self.adts[&def];
        if adt.is_enum || adt.is_union { return Vec::new(); }
        let fields = &adt.variants[0].fields;
        let Some((tail,prefix)) = fields.split_last() else { return Vec::new(); };
        let mut visitor = UnsizingParams { params:Vec::new(),remove:false };
        visitor.fold_ty(&self.tys,tail.ty,0); visitor.remove=true;
        for field in prefix { visitor.fold_ty(&self.tys,field.ty,0); }
        visitor.params
    }
}
struct UnsizingParams { params:Vec<u32>,remove:bool }
impl UnsizingParams {
    fn visit(&mut self,i:u32) { if self.remove { self.params.retain(|p| *p!=i); } else if !self.params.contains(&i) { self.params.push(i); } }
}
impl TypeFolder for UnsizingParams {
    fn fold_ty(&mut self,tys:&Types,t:TyId,d:u32)->TyId { if let TyKind::Param(i)=tys.kind(t) { self.visit(*i); } super_fold_ty(tys,t,self,d) }
    fn fold_const(&mut self,tys:&Types,c:Const,d:u32)->Const { if let crate::consts::ConstKind::Param(p)=tys.const_kind(c) { self.visit(p.index); } super_fold_const(tys,c,self,d) }
}
