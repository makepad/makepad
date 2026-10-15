// ported from N:rustc_hir_analysis/src/collect/type_of.rs @8d1a764
// ported from N:rustc_hir_analysis/src/check/compare_impl_item.rs @8d1a764
use super::*;
use super::fulfill::ObligationCtxt;
use crate::infer::InferCtxt;
use crate::program::{Program, DefKind};
use crate::tcx::{Tcx, EvalCx, EvalState, TypingEnv, TypingMode};
use crate::types::{Types, TyKind, AliasTyKind, RegionKind, PlaceholderType, BoundVar};
use crate::types::fold::{TypeFolder, super_fold_ty};
use relax_front::assoc::ImplTraitInTraitData;
impl Tcx {
    pub fn type_of(&self, prog: &Program, def: DefId) -> Result<TyId, NoSolution> {
        match prog.impl_trait_in_trait_data.get(&def) {
            Some(ImplTraitInTraitData::Impl { fn_def_id }) => {
                let values = self.collect_return_position_impl_trait_in_trait_tys(prog, *fn_def_id)?;
                return values.into_iter().find(|(d, _)| *d == prog.trait_item_def_id[&def]).map(|(_, ty)| ty).ok_or(NoSolution);
            }
            Some(ImplTraitInTraitData::Trait { opaque_def_id, .. }) => {
                let args = self.generic_indices_of(prog, *opaque_def_id).identity_args(&self.tys);
                return Ok(self.alias_ty_with_args(AliasTyKind::Opaque { def_id: *opaque_def_id }, &args));
            }
            None => {}
        }
        let state = EvalState::new(); let cx = EvalCx::new(prog, self, TypingEnv::fully_monomorphized(&prog), &state);
        match prog.def(def).kind {
            DefKind::TypeAlias | DefKind::AssocTy => Ok(self.alias_ty(&cx, prog, def)),
            DefKind::OpaqueTy => crate::type_queries::Providers { tcx: self, prog }.type_of_opaque_hir_typeck(def).map_err(|_| NoSolution)?,
            DefKind::Const | DefKind::AssocConst | DefKind::AnonConst | DefKind::Static => self.const_ty(prog, def).ok_or(NoSolution),
            _ => panic!("type_of unsupported definition"),
        }
    }
    /// compare_impl_item.rs:457-794: infer hidden RPITIT types from both signatures,
    /// prove their WF using the trait signature's assumptions, then resolve regions.
    pub fn collect_return_position_impl_trait_in_trait_tys(&self, prog: &Program, method: DefId) -> Result<Vec<(DefId, TyId)>, NoSolution> {
        let trait_method = *prog.trait_item_def_id.get(&method).ok_or(NoSolution)?;
        let imp = prog.def(method).parent; let trait_def = prog.def(trait_method).parent;
        let impl_sig = self.fn_item_sig(prog, method).ok_or(NoSolution)?;
        let trait_sig = self.fn_item_sig(prog, trait_method).ok_or(NoSolution)?;
        if impl_sig.params.len() != trait_sig.params.len() || impl_sig.self_kind != trait_sig.self_kind || impl_sig.n_generics - impl_sig.n_parent_generics != trait_sig.n_generics - trait_sig.n_parent_generics { return Err(NoSolution); }
        let impl_parent = self.generic_indices_of(prog, imp);
        let parent_count = impl_parent.types_consts.len() + impl_parent.lifetimes.len();
        let trait_parent = self.generic_indices_of(prog, trait_def);
        let trait_own = &trait_sig.generic_indices;
        let impl_own = &impl_sig.generic_indices;
        if trait_own.lifetimes.len() - trait_parent.lifetimes.len() != impl_own.lifetimes.len() - impl_parent.lifetimes.len() { return Err(NoSolution); }
        let trait_parent_len = trait_parent.types_consts.len() + trait_parent.lifetimes.len();
        let impl_args_identity = impl_own.identity_args(&self.tys);
        let trait_args_identity = trait_own.identity_args(&self.tys);
        for (a, b) in impl_args_identity[parent_count..].iter().zip(&trait_args_identity[trait_parent_len..]) {
            if std::mem::discriminant(a) != std::mem::discriminant(b) { return Err(NoSolution); }
        }

        let mut trait_to_impl_args = self.impl_trait_ref(prog, imp, &impl_parent.identity_args(&self.tys)).args;
        let trait_parent_count = trait_to_impl_args.len();
        let method_args = self.generic_indices_of(prog, method).identity_args(&self.tys);
        trait_to_impl_args.extend_from_slice(&method_args[parent_count..]);
        let mut clauses = self.clauses_of(prog, imp);
        for p in self.own_clauses_of(prog, trait_method) { clauses.push(self.instantiate_predicate(prog, trait_method, &trait_to_impl_args, &p)); }
        let infcx = InferCtxt::for_item(self, prog, TypingMode::non_body_analysis(), method);

        let env = super::param_env::normalize_param_env_or_error(
            self, prog, ParamEnv { clauses: clauses.into() }, infcx.cause);
        let env = self.intern_param_env(env);
        let mut ocx = ObligationCtxt::new(&infcx, prog);
        for p in self.own_clauses_of(prog, method) { let p = ocx.normalize_predicate(&env, &p); ocx.register_obligation(Goal { param_env: env.clone(), predicate: p }); }
        let impl_fn = self.tys.intern(TyKind::FnPtr(self.tys.mk_list(&impl_sig.params), impl_sig.ret, impl_sig.header, impl_sig.bound_regions));
        let impl_fn = infcx.instantiate_binder_with_infer(impl_fn);
        let impl_fn = ocx.normalize_ty(&env, impl_fn);
        let mut trait_types = trait_sig.params.clone(); trait_types.push(trait_sig.ret);
        let trait_types = self.instantiate_ty(prog, trait_method, &trait_to_impl_args, self.tys.intern(TyKind::Tuple(self.tys.mk_list(&trait_types))));
        let trait_types = super::dyn_compatibility::liberate_late_bound_regions(self, method, trait_types);
        let mut collector = ImplTraitInTraitCollector { ocx: &mut ocx, env: &env, types: Vec::new() };
        let raw = collector.fold_ty(&self.tys, trait_types, 0);
        let collected = std::mem::take(&mut collector.types); drop(collector);
        let normalized = ocx.normalize_ty(&env, raw);
        let universe = infcx.create_next_universe();
        let mut substitutions = Vec::new();
        for (index, (_, ty, _)) in collected.iter().enumerate() {
            assert_eq!(infcx.deeply_resolve_ignoring_regions(*ty), *ty);
            let placeholder = self.tys.intern(TyKind::Placeholder(PlaceholderType { universe, bound: BoundVar(index as u32 + 1) }));
            substitutions.push((*ty, placeholder));
        }
        let mut wf_tys = Vec::new();
        for ty in [raw, normalized] { let TyKind::Tuple(ts) = self.tys.kind(ty) else { unreachable!() }; for &ty in self.tys.list(*ts) { let mapped = self.tys.replace(ty, &substitutions); if !wf_tys.contains(&mapped) { wf_tys.push(mapped); } } }
        let TyKind::FnPtr(_, impl_ret, _, _) = self.tys.kind(impl_fn) else { unreachable!() };
        let TyKind::Tuple(trait_types) = self.tys.kind(normalized) else { unreachable!() };
        let trait_types = self.tys.list(*trait_types);
        ocx.eq(&env, *trait_types.last().unwrap(), *impl_ret)?;
        let trait_fn = self.tys.intern(TyKind::FnPtr(self.tys.mk_list(&trait_types[..trait_types.len() - 1]), *trait_types.last().unwrap(), trait_sig.header, 0));
        ocx.eq(&env, trait_fn, impl_fn)?;
        for (_, ty, _) in &collected { ocx.register_obligation(Goal { param_env: env.clone(), predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(*ty)))) }); }
        ocx.evaluate_obligations_error_on_ambiguity()?;
        if !infcx.resolve_regions(prog, env, &wf_tys).is_empty() { return Err(NoSolution); }
        let mut result = Vec::new();
        // compare_impl_item.rs:717-719: only the RPITIT's own (captured) lifetimes are mapped;
        // the trait method's full generics (parent + own) are skipped.
        let trait_method_count = trait_own.lifetimes.len() + trait_own.types_consts.len();
        let cause = super::fulfill::ObligationCause { body_id: method, span: infcx.cause.span };
        for (def, ty, args) in collected {
            let ty = match infcx.deeply_resolve_via_region_graph(ty) {
                Ok(ty) => ty,
                // compare_impl_item.rs:760-766 is a span_bug whose comment asks for a delayed bug
                // once reached; keep the session alive with that delayed bug and an error type.
                Err(_) => { self.dcx.delayed_bug(cause, "could not fully resolve the hidden type of an RPITIT".into()); self.tys.error }
            };
            let identity = self.generic_indices_of(prog, def).identity_args(&self.tys);
            let mut map = Vec::new();
            for (&a, &b) in args.iter().zip(&identity).skip(trait_method_count) { if let (GenericArg::Lifetime(a), GenericArg::Lifetime(b)) = (a,b) { map.push((a,b)); } }
            let mut remap = RemapHiddenTyRegions { map, trait_count: trait_parent_count, impl_count: parent_count, failed: false };
            let ty = remap.fold_ty(&self.tys, ty, 0);
            // compare_impl_item.rs:929-958: an impl lifetime the RPITIT does not capture.
            if remap.failed { self.dcx.report(cause, "return type captures more lifetimes than trait definition".into()); }
            result.push((def, if remap.failed { self.tys.error } else { ty }));
        }
        for def in prog.associated_types_for_impl_traits.get(&trait_method).into_iter().flatten() { if !result.iter().any(|(d,_)| d == def) { result.push((*def, self.tys.error)); } }
        Ok(result)
    }
}
struct ImplTraitInTraitCollector<'a, 'b, 'tcx> { ocx: &'a mut ObligationCtxt<'b, 'tcx>, env: &'a ParamEnvId, types: Vec<(DefId, TyId, Vec<GenericArg>)> }
impl TypeFolder for ImplTraitInTraitCollector<'_, '_, '_> {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        let tcx = self.ocx.infcx.tcx;
        if let TyKind::Alias(_, a) = tys.kind(ty) { if let AliasTyKind::Projection { def_id } = a.kind {
            if self.ocx.prog.impl_trait_in_trait_data.contains_key(&def_id) {
                if let Some((_, ty, _)) = self.types.iter().find(|(d,_,_)| *d == def_id) { return *ty; }
                assert!(!tys.has_vars_bound_at_or_above(ty, 0));
                let args = tcx.complete_args(self.ocx.prog, def_id, tys.list(a.args), a.regions); let var = self.ocx.infcx.next_ty_var();
                self.types.push((def_id, var, args.clone()));
                for p in tcx.item_bounds(self.ocx.prog, def_id) {
                    let p = tcx.instantiate_predicate(self.ocx.prog, def_id, &args, &p);
                    let p = fold::fold_predicate(tys, &p, self);
                    let p = self.ocx.normalize_predicate(self.env, &p);
                    self.ocx.register_obligation(Goal { param_env: self.env.clone(), predicate: p });
                }
                return var;
            }
        } }
        super_fold_ty(tys, ty, self, depth)
    }
}
struct RemapHiddenTyRegions { map: Vec<(RegionId,RegionId)>, trait_count: usize, impl_count: usize, failed: bool }
impl TypeFolder for RemapHiddenTyRegions {
    fn fold_region(&mut self, tys: &Types, r: RegionId, _: u32) -> RegionId {
        match tys.region_kind(r) {
            RegionKind::ReBound(..) | RegionKind::ReStatic | RegionKind::ReError => return r,
            RegionKind::ReEarlyParam(i) if (i as usize) < self.impl_count => return r,
            RegionKind::ReEarlyParam(_) | RegionKind::ReLateParam(..) => {},
            _ => { self.failed = true; return r; }
        }
        // FxIndexMap collected from the arguments: a later duplicate key replaces the value.
        let Some((_, mapped)) = self.map.iter().rev().find(|(orig, _)| *orig == r) else { self.failed = true; return r; };
        let RegionKind::ReEarlyParam(index) = tys.region_kind(*mapped) else { unreachable!() };
        tys.intern_region(RegionKind::ReEarlyParam(index - self.trait_count as u32 + self.impl_count as u32))
    }
}
impl eval_ctxt::EvalCtxt<'_, '_> {
    pub(super) fn type_of(&self, def: DefId, args: &[GenericArg]) -> Result<TyId, NoSolution> {
        let ty = self.delegate.tcx.type_of(self.solver.prog, def)?;
        Ok(self.delegate.tcx.instantiate_ty(self.solver.prog, def, args, ty))
    }
}

impl crate::type_queries::Providers<'_> {
    /// collect/type_of.rs:325-345 + collect/type_of/opaque.rs:237-268
    /// (find_opaque_ty_constraints_for_rpit, DefiningScopeKind::HirTypeck): the
    /// hidden type HIR typeck of the defining owner published, an error type for a
    /// stranded opaque or an owner tainted by errors. Q1 handles cycles.
    pub(crate) fn type_of_opaque_hir_typeck(&self, def: DefId) -> Result<Result<TyId, NoSolution>, crate::query::QueryError> {
        let q = &self.tcx.queries;
        q.opaque_hidden_type.get(&q.engine, def, &mut |def| {
            q.read_def_declarations(self.prog, def)?;
            let owner = self.prog.opaque_owner(def);
            if !self.tcx.defining_opaques(self.tcx.opaque_types_defined_by(self.prog, owner)).contains(&def) { return Ok(Ok(self.tcx.tys.error)); }
            let body = self.tcx.typeck(self.prog, owner);
            if !body.errors.is_empty() { return Ok(Ok(self.tcx.tys.error)); }
            // Under the next solver every defining owner publishes a hidden type or
            // reports an error (opaque.rs:256-266).
            Ok(Ok(*body.hidden_types.get(&def).expect("defining owner published no hidden type")))
        }, &|id| self.force(id))
    }
}
