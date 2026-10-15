// ported from N:rustc_next_trait_solver/src/solve/trait_goals.rs @8d1a764
use super::*;
use super::assembly::*;
use super::eval_ctxt::EvalCtxt;
use crate::lang_items::LangItem;
use crate::types::{TyKind, AliasTyKind};
use crate::consts::IsRigid;
impl EvalCtxt<'_, '_> {
    pub(super) fn assemble_builtin_impl_candidates(&mut self, env: &ParamEnvId, goal: &GoalKind, candidates: &mut Vec<Candidate>) -> Result<(), NoSolutionOrRerunNonErased> {
        if matches!(self.delegate.typing_mode, crate::tcx::TypingMode::Reflection) { return Ok(()); }
        if matches!(goal, GoalKind::Trait(TraitClause { polarity: ClausePolarity::Negative, .. })) { return Ok(()); }
        let tcx = self.delegate.tcx; let tr = goal.trait_ref(self); let ty = goal.self_ty(self);
        let lang = tcx.lang_items.by_def.get(&tr.def_id).copied();
        if lang == Some(LangItem::Unsize) {
            if matches!(goal, GoalKind::Trait(_)) { self.consider_structural_builtin_unsize_candidates(env,&tr,candidates)?; }
            return Ok(());
        }
        let mut source = BuiltinImplSource::Misc;
        if matches!(lang, Some(LangItem::Sized | LangItem::MetaSized)) { source = BuiltinImplSource::Trivial; }
        let (result, head_usages) = self.probe().enter_single_candidate(CandidateSource::BuiltinImpl(source), |ecx| {
            if matches!(tcx.tys.kind(ty), TyKind::Error) {
                if let GoalKind::NormalizesTo(p) = goal { ecx.instantiate_normalizes_to_term(env, p, Term::Ty(tcx.tys.error))?; }
                return ecx.evaluate_added_goals_and_make_canonical_response(Certainty::Yes);
            }
            match lang {
                Some(LangItem::Fn) => return ecx.consider_builtin_fn_trait_candidates(env, goal, &tr, ty, crate::upvar::ClosureKind::Fn),
                Some(LangItem::FnMut) => return ecx.consider_builtin_fn_trait_candidates(env, goal, &tr, ty, crate::upvar::ClosureKind::FnMut),
                Some(LangItem::FnOnce) => return ecx.consider_builtin_fn_trait_candidates(env, goal, &tr, ty, crate::upvar::ClosureKind::FnOnce),
                Some(LangItem::AsyncFn) => return ecx.consider_builtin_async_fn_trait_candidates(env, goal, &tr, ty, crate::upvar::ClosureKind::Fn),
                Some(LangItem::AsyncFnMut) => return ecx.consider_builtin_async_fn_trait_candidates(env, goal, &tr, ty, crate::upvar::ClosureKind::FnMut),
                Some(LangItem::AsyncFnOnce) => return ecx.consider_builtin_async_fn_trait_candidates(env, goal, &tr, ty, crate::upvar::ClosureKind::FnOnce),
                Some(LangItem::AsyncFnKindHelper) if matches!(goal, GoalKind::Trait(_)) => return ecx.consider_builtin_async_fn_kind_helper_candidate(&tr),
                Some(LangItem::PointeeTrait) => return ecx.consider_builtin_pointee_candidate(env, goal, ty),
                Some(LangItem::DiscriminantKind) => return ecx.consider_builtin_discriminant_kind_candidate(env, goal, ty),
                Some(LangItem::Destruct) => {
                    assert!(matches!(goal, GoalKind::Trait(_)), "Destruct has no associated type");
                    return ecx.evaluate_added_goals_and_make_canonical_response(Certainty::Yes);
                }
                Some(LangItem::PointeeSized) if matches!(goal, GoalKind::Trait(_)) => return ecx.evaluate_added_goals_and_make_canonical_response(Certainty::Yes),
                Some(LangItem::Future) => return ecx.consider_builtin_future_candidate(env, goal, ty),
                _ => {}
            }
            if matches!(goal, GoalKind::NormalizesTo(_)) { return Err(NoSolution.into()); }
            let parts = if super::eval_ctxt::trait_is_auto(ecx.solver.prog, tr.def_id) {
                if matches!(tcx.tys.kind(ty), TyKind::IntVar(_) | TyKind::FloatVar(_)) { return ecx.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS); }
                if matches!(tcx.tys.kind(ty), TyKind::Foreign(_)) && !matches!(lang, Some(LangItem::DefaultTrait1 | LangItem::DefaultTrait2 | LangItem::DefaultTrait3 | LangItem::DefaultTrait4)) { return Err(NoSolution.into()); }
                if tcx.is_coroutine(ty) && lang == Some(LangItem::Unpin) { return Err(NoSolution.into()); }
                if let TyKind::Alias(IsRigid::Yes, alias) = tcx.tys.kind(ty) {
                    if let AliasTyKind::Opaque { def_id } = alias.kind {
                        for bound in tcx.item_bounds(ecx.solver.prog,def_id) {
                            if let PredicateKind::Clause(ClauseKind::Trait(t)) = &bound.kind {
                                if t.trait_ref.def_id == tr.def_id && matches!(t.trait_ref.args.first(),Some(GenericArg::Type(t)) if matches!(tcx.tys.kind(*t),TyKind::Alias(_,a) if a.kind.def_id()==def_id)) { return Err(NoSolution.into()); }
                            }
                        }
                        let args = tcx.complete_args(ecx.solver.prog,def_id,tcx.tys.list(alias.args),alias.regions);
                        let hidden = ecx.type_of(def_id,&args)?;
                        ecx.add_trait_goal(env,tr.def_id,hidden)?;
                        let response = ecx.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)?;
                        if !response_values_are_identity(&tcx.tys, &response, true) {
                            return Ok(ecx.make_ambiguous_response_no_constraints(MaybeInfo::AMBIGUOUS));
                        }
                        if !response.value.external_constraints.opaque_types.is_empty() && matches!(ecx.delegate.typing_mode,crate::tcx::TypingMode::Typeck { .. }) {
                            return Ok(ecx.make_ambiguous_response_no_constraints(MaybeInfo::AMBIGUOUS));
                        }
                        return Ok(response);
                    }
                }
                let key = crate::tcx::ty_key(&tcx.tys, ty);
                for key in [key, crate::tcx::TyKey::Other] { if tcx.trait_impls.get(&(key, tr.def_id)).is_some_and(|v| !v.is_empty()) { return Err(NoSolution.into()); } }
                structural_traits::instantiate_constituent_tys_for_auto_trait(ecx, ty)?
            } else { match lang {
                Some(LangItem::Sized) => structural_traits::instantiate_constituent_tys_for_sizedness_trait(ecx, super::sizedness::SizedTraitKind::Sized, ty)?,
                Some(LangItem::MetaSized) if matches!(tcx.tys.kind(ty),TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..)) => vec![],
                Some(LangItem::MetaSized) => structural_traits::instantiate_constituent_tys_for_sizedness_trait(ecx, super::sizedness::SizedTraitKind::MetaSized, ty)?,
                Some(LangItem::Copy | LangItem::Clone | LangItem::TrivialClone) => structural_traits::instantiate_constituent_tys_for_copy_clone_trait(ecx, ty)?,
                // trait_goals.rs:324-339: every function pointer implements FnPtr.
                Some(LangItem::FnPtrTrait) if matches!(tcx.tys.kind(ty), TyKind::FnPtr(..)) => vec![],
                Some(LangItem::Tuple) if matches!(tcx.tys.kind(ty), TyKind::Tuple(_)) => vec![],
                _ => return Err(NoSolution.into()),
            } };
            for part in parts { ecx.add_trait_goal(env, tr.def_id, part)?; }
            ecx.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
        });
        if let Ok(result) = map_err_to_rerun(result)? { candidates.push(Candidate { source: CandidateSource::BuiltinImpl(source), result, head_usages }); }
        Ok(())
    }
    /// trait_goals.rs:355-391 / normalizes_to.rs:533-572.
    fn consider_builtin_fn_trait_candidates(&mut self, env: &ParamEnvId, goal: &GoalKind, tr: &TraitRef, ty: TyId, goal_kind: crate::upvar::ClosureKind) -> QueryResultOrRerunNonErased {
        let tcx = self.delegate.tcx;
        let Some(sig) = structural_traits::extract_tupled_inputs_and_output_from_callable(self, ty, goal_kind)? else { return self.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS); };
        let sig = self.delegate.instantiate_binder_with_infer(sig);
        let TyKind::FnPtr(params, output, _, _) = tcx.tys.kind(sig).clone() else { unreachable!() };
        self.eq_args(env, &tr.args, &[GenericArg::Type(ty), GenericArg::Type(tcx.tys.intern(TyKind::Tuple(params)))])?;
        if let Some(sized) = tcx.lang_items.get(LangItem::Sized) { self.add_trait_goal(env, sized, output)?; }
        if let GoalKind::NormalizesTo(p) = goal { self.instantiate_normalizes_to_term(env, p, Term::Ty(output))?; }
        self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
    }
    /// trait_goals.rs:393-441 / normalizes_to.rs:574-656. For closures
    /// returning futures, the environment region does not change the future type;
    /// the lending CoroutineClosure arm remains EXCEPTION(M7).
    fn consider_builtin_async_fn_trait_candidates(&mut self, env: &ParamEnvId, goal: &GoalKind, tr: &TraitRef, ty: TyId, goal_kind: crate::upvar::ClosureKind) -> QueryResultOrRerunNonErased {
        let tcx = self.delegate.tcx;
        let (sig, nested) = structural_traits::extract_tupled_inputs_and_output_from_async_callable(self, ty, goal_kind)?;
        let sig = self.delegate.instantiate_binder_with_infer(sig);
        let structural_traits::AsyncCallableRelevantTypes { tupled_inputs_ty, output_coroutine_ty, coroutine_return_ty } = structural_traits::async_callable_relevant_types(self, sig)?;
        self.eq_args(env, &tr.args, &[GenericArg::Type(ty), GenericArg::Type(tupled_inputs_ty)])?;
        if let Some(sized) = tcx.lang_items.get(LangItem::Sized) { self.add_trait_goal(env, sized, output_coroutine_ty)?; }
        for predicate in nested { self.add_goal(GoalSource::ImplWhereBound, Goal { param_env: env.clone(), predicate })?; }
        if let GoalKind::NormalizesTo(p) = goal {
            let (def, _) = self.alias_args(&p.alias);
            let term = match tcx.lang_items.by_def.get(&def) {
                Some(LangItem::CallOnceFuture | LangItem::CallRefFuture) => output_coroutine_ty,
                Some(LangItem::AsyncFnOnceOutput) => coroutine_return_ty,
                _ => panic!("no such associated type in `AsyncFn*`"),
            };
            self.instantiate_normalizes_to_term(env, p, Term::Ty(term))?;
        }
        self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
    }
    /// trait_goals.rs:443-462: `AsyncFnKindHelper<GoalKind>` for a closure kind type.
    fn consider_builtin_async_fn_kind_helper_candidate(&mut self, tr: &TraitRef) -> QueryResultOrRerunNonErased {
        let tys = &self.delegate.tcx.tys;
        let [GenericArg::Type(closure_fn_kind_ty), GenericArg::Type(goal_kind_ty)] = tr.args[..] else { panic!("AsyncFnKindHelper arguments") };
        let Some(closure_kind) = tys.to_opt_closure_kind(closure_fn_kind_ty) else { return Err(NoSolution.into()); };
        let goal_kind = tys.to_opt_closure_kind(goal_kind_ty).expect("goal closure kind");
        if closure_kind > goal_kind { return Err(NoSolution.into()); }
        self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
    }
    /// trait_goals.rs:498-520 / normalizes_to.rs:819-845: async coroutines implement
    /// `Future` with their return type as `Output`.
    fn consider_builtin_future_candidate(&mut self, env: &ParamEnvId, goal: &GoalKind, ty: TyId) -> QueryResultOrRerunNonErased {
        let tcx = self.delegate.tcx;
        if !tcx.is_coroutine(ty) { return Err(NoSolution.into()); }
        if let GoalKind::NormalizesTo(p) = goal {
            let TyKind::Closure(_, _, args) = tcx.tys.kind(ty) else { unreachable!() };
            let TyKind::FnPtr(_, return_ty, _, _) = tcx.tys.kind(args.sig_as_fn_ptr_ty()) else { unreachable!() };
            self.instantiate_normalizes_to_term(env, p, Term::Ty(*return_ty))?;
        }
        self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
    }
    pub(super) fn add_trait_goal(&mut self, env: &ParamEnvId, def: crate::program::DefId, ty: TyId) -> Result<(), NoSolutionOrRerunNonErased> {
        self.add_goal(GoalSource::ImplWhereBound, Goal { param_env: env.clone(), predicate: collect::trait_predicate(def, vec![GenericArg::Type(ty)]) })
    }
}

impl EvalCtxt<'_, '_> {
    /// normalizes_to.rs:712-817: tail metadata, Sized fallback, then rigid alias.
    fn consider_builtin_pointee_candidate(&mut self, env: &ParamEnvId, goal: &GoalKind, ty: TyId) -> QueryResultOrRerunNonErased {
        let GoalKind::NormalizesTo(p) = goal else { return self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes); };
        let tcx = self.delegate.tcx;
        let metadata = tcx.lang_items.get(LangItem::Metadata).ok_or(NoSolution)?;
        assert_eq!(self.alias_args(&p.alias).0, metadata);
        let projection = |tail| tcx.alias_ty_with_args(AliasTyKind::Projection { def_id: metadata }, &[GenericArg::Type(tail)]);
        let value = match tcx.tys.kind(ty) {
            TyKind::Error => tcx.tys.error,
            TyKind::Str | TyKind::Slice(_) => tcx.tys.usize_,
            TyKind::Dyn(..) => {
                let def = tcx.lang_items.get(LangItem::DynMetadata).ok_or(NoSolution)?;
                tcx.tys.intern(TyKind::Adt(def, tcx.tys.mk_list(&[ty]), crate::types::RegionArgsId::EMPTY))
            }
            TyKind::Alias(IsRigid::Yes, _) | TyKind::Param(_) | TyKind::Placeholder(_) => {
                let result = self.probe().enter(|ecx| {
                    let sized = tcx.lang_items.get(LangItem::Sized).ok_or(NoSolution)?;
                    ecx.add_trait_goal(env, sized, ty)?;
                    ecx.instantiate_normalizes_to_term(env, p, Term::Ty(tcx.tys.unit))?;
                    ecx.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
                });
                match map_err_to_rerun(result)? {
                    Ok(response) => return Ok(response),
                    Err(NoSolution) => {
                        self.instantiate_normalizes_to_term(env, p, self.rigid_alias_term(&p.alias))?;
                        return self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes);
                    }
                }
            }
            TyKind::Adt(def, args, rs) => {
                let adt = &tcx.adts[def];
                if adt.is_enum || adt.is_union { tcx.tys.unit }
                else if let Some(field) = adt.variants[0].fields.last() {
                    projection(tcx.instantiate_ty(self.solver.prog, *def, &tcx.complete_args(self.solver.prog, *def, tcx.tys.list(*args), *rs), field.ty))
                } else { tcx.tys.unit }
            }
            TyKind::Tuple(ts) => tcx.tys.list(*ts).last().map_or(tcx.tys.unit, |&tail| projection(tail)),
            TyKind::Infer(_) | TyKind::Alias(IsRigid::No, _) | TyKind::Bound(..) => panic!("non-rigid metadata self type"),
            _ => tcx.tys.unit,
        };
        self.instantiate_normalizes_to_term(env, p, Term::Ty(value))?;
        self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
    }
    /// normalizes_to.rs:979-1037; rustc_middle/src/ty/sty.rs:1710-1761.
    fn consider_builtin_discriminant_kind_candidate(&mut self, env: &ParamEnvId, goal: &GoalKind, ty: TyId) -> QueryResultOrRerunNonErased {
        if let GoalKind::NormalizesTo(p) = goal {
            let tcx = self.delegate.tcx;
            let value = match tcx.tys.kind(ty) {
                TyKind::Alias(IsRigid::Yes, _) | TyKind::Param(_) | TyKind::Placeholder(_) => self.rigid_alias_term(&p.alias),
                TyKind::Adt(def, ..) if tcx.adts[def].is_enum => Term::Ty(tcx.tys.prim(tcx.adts[def].repr_int.unwrap_or(crate::program::Prim::Isize))),
                TyKind::Closure(..) if tcx.is_coroutine(ty) => Term::Ty(tcx.tys.u32_),
                TyKind::Infer(_) | TyKind::Alias(IsRigid::No, _) | TyKind::Bound(..) => panic!("non-rigid discriminant self type"),
                _ => Term::Ty(tcx.tys.u8_),
            };
            self.instantiate_normalizes_to_term(env, p, value)?;
        }
        self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
    }
}
