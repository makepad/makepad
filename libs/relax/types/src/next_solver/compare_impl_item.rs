// ported from N:rustc_hir_analysis/src/check/compare_impl_item.rs @8d1a764
use super::*;
use crate::infer::InferCtxt;
use crate::program::{Program, DefKind};
use crate::tcx::{Tcx, TypingMode};
use crate::types::TyKind;
use super::fulfill::ObligationCtxt;

impl Tcx {
    /// :37-97,174-412,2141-2317. Diagnostics and const-trait/GCA gates are cut.
    pub fn compare_impl_item(&self, prog: &Program, method: DefId) -> Result<(), NoSolution> {
        crate::type_queries::Providers { tcx: self, prog }.compare_impl_item(method).expect("compare_impl_item query")
    }

    pub(crate) fn compare_impl_item_uncached(&self, prog: &Program, method: DefId) -> Result<(), NoSolution> {
        let trait_method = *prog.trait_item_def_id.get(&method).ok_or(NoSolution)?;
        let imp = prog.def(method).parent;
        let trait_def = prog.def(trait_method).parent;
        let own = self.generic_indices_of(prog, method);
        let parent = self.generic_indices_of(prog, imp);
        let trait_own = self.generic_indices_of(prog, trait_method);
        let trait_parent = self.generic_indices_of(prog, trait_def);
        let identity = own.identity_args(&self.tys);
        let trait_identity = trait_own.identity_args(&self.tys);
        let parent_count = parent.types_consts.len() + parent.lifetimes.len();
        let trait_parent_count = trait_parent.types_consts.len() + trait_parent.lifetimes.len();
        let a = &identity[parent_count..];
        let b = &trait_identity[trait_parent_count..];
        // :1590,2062,1097: counts, ordered kinds and early-bound region count.
        if a.len() != b.len() || own.lifetimes.len() - parent.lifetimes.len() != trait_own.lifetimes.len() - trait_parent.lifetimes.len() { return Err(NoSolution); }
        for (a, b) in a.iter().zip(b) {
            if std::mem::discriminant(a) != std::mem::discriminant(b) { return Err(NoSolution); }
        }
        // :2087-2093: matching const parameter kinds must also have the
        // same declared type. Generic declarations retain their AST type here.
        let imp_env = self.declaration_env(prog, method);
        let trait_env = self.declaration_env(prog, trait_method);
        let declared_const_ty = |def, _indices: &crate::types::variance::GenericIndices, env: &crate::tcx::GenEnv, ct| {
            let crate::consts::ConstKind::Param(param) = self.tys.const_kind(ct) else { unreachable!() };
            let compact = param.index;
            let &(_, file, ty) = env.consts.iter().find(|p| p.0 == compact).expect("const parameter declaration");
            let state = crate::tcx::EvalState::new();
            let cx = crate::tcx::EvalCx::new(prog, self, crate::tcx::TypingEnv { limit: crate::tcx::RecursionLimit::for_item(prog, method), param_env: Default::default(), mode: TypingMode::non_body_analysis() }, &state);
            self.lower_ty(&cx, prog, file, prog.def(def).scope, env, ty)
        };
        for (&a, &b) in a.iter().zip(b) {
            if let (GenericArg::Const(a), GenericArg::Const(b)) = (a, b) {
                if declared_const_ty(method, &own, &imp_env, a) != declared_const_ty(trait_method, &trait_own, &trait_env, b) { return Err(NoSolution); }
            }
        }
        if prog.def(method).kind == DefKind::AssocFn {
            let imp_sig = self.fn_item_sig(prog, method).ok_or(NoSolution)?;
            let trait_sig = self.fn_item_sig(prog, trait_method).ok_or(NoSolution)?;
            if (imp_sig.self_kind == 0) != (trait_sig.self_kind == 0) || imp_sig.params.len() != trait_sig.params.len() { return Err(NoSolution); }
            // :1910: synthetic and explicitly named type parameters differ.
            for i in 0..own.types_consts.len() - parent.types_consts.len() {
                let ia = (parent.types_consts.len() + i) as u32;
                let ta = (trait_parent.types_consts.len() + i) as u32;
                if imp_env.impl_params.iter().any(|(_, p)| *p == ia) != trait_env.impl_params.iter().any(|(_, p)| *p == ta) { return Err(NoSolution); }
            }
        }
        if prog.def(method).kind == DefKind::AssocTy && self.own_clauses_of(prog, method).is_empty() { return self.check_type_bounds(prog, method, trait_method); }
        let mut trait_args = self.impl_trait_ref(prog, imp, &parent.identity_args(&self.tys)).args;
        trait_args.extend_from_slice(a);
        let mut clauses = self.clauses_of(prog, imp);
        for p in self.own_clauses_of(prog, trait_method) { clauses.push(self.instantiate_predicate(prog, trait_method, &trait_args, &p)); }
        let mode = TypingMode::non_body_analysis();
        let cause = super::fulfill::ObligationCause {
            body_id: method,
            span: prog.files[prog.def(method).file as usize].ast.item(prog.def(method).item).lo,
        };
        // traits/mod.rs:330-580: resolve the hybrid environment's regions
        // before moving its clauses into the signature comparison context.
        let env = super::param_env::normalize_param_env_or_error(
            self, prog, ParamEnv { clauses: clauses.into() }, cause);
        let env = self.intern_param_env(env);

        let infcx = InferCtxt::for_item(self, prog, mode, method);

        let mut ocx = ObligationCtxt::new(&infcx, prog);
        for p in self.own_clauses_of(prog, method) {
            let p = ocx.normalize_predicate(&env, &p);
            ocx.register_obligation(Goal { param_env: env.clone(), predicate: p });
        }
        let mut wf_tys = Vec::new();
        match prog.def(method).kind {
            DefKind::AssocFn => {
                let imp_sig = self.fn_item_sig(prog, method).ok_or(NoSolution)?;
                let trait_sig = self.fn_item_sig(prog, trait_method).ok_or(NoSolution)?;
                let raw_impl = self.tys.intern(TyKind::FnPtr(self.tys.mk_list(&imp_sig.params), imp_sig.ret, imp_sig.header, imp_sig.bound_regions));
                let raw_impl = infcx.instantiate_binder_with_infer(raw_impl);
                let impl_fn = ocx.normalize_ty(&env, raw_impl);
                let mut trait_types = trait_sig.params.clone(); trait_types.push(trait_sig.ret);
                let raw_trait = self.instantiate_ty(prog, trait_method, &trait_args, self.tys.intern(TyKind::Tuple(self.tys.mk_list(&trait_types))));
                let raw_trait = dyn_compatibility::liberate_late_bound_regions(self, method, raw_trait);
                let TyKind::Tuple(trait_types) = self.tys.kind(raw_trait) else { unreachable!() };
                let trait_types = self.tys.list(*trait_types);
                let raw_trait = self.tys.intern(TyKind::FnPtr(self.tys.mk_list(&trait_types[..trait_types.len() - 1]), *trait_types.last().unwrap(), trait_sig.header, 0));
                let trait_fn = ocx.normalize_ty(&env, raw_trait);
                for ty in [raw_trait, trait_fn] {
                    let TyKind::FnPtr(inputs, output, _, _) = self.tys.kind(ty) else { unreachable!() };
                    for &ty in self.tys.list(*inputs) { if !wf_tys.contains(&ty) { wf_tys.push(ty); } }
                    if !wf_tys.contains(output) { wf_tys.push(*output); }
                }
                let goals = infcx.at(prog, env.clone()).sub(impl_fn, trait_fn).map_err(|_| NoSolution)?;
                ocx.register_obligations(goals);
                if !self.tys.has_error(impl_fn) && !self.tys.has_error(trait_fn) {
                    let TyKind::FnPtr(inputs, output, _, _) = self.tys.kind(raw_impl) else { unreachable!() };
                    let mut inputs = self.tys.list(*inputs).to_vec(); inputs.push(*output);
                    for ty in inputs { ocx.register_obligation(Goal { param_env: env.clone(), predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(ty)))) }); }
                }
            }
            DefKind::AssocConst => {
                let impl_ty = ocx.normalize_ty(&env, self.type_of(prog, method)?);
                let trait_ty = self.instantiate_ty(prog, trait_method, &trait_args, self.type_of(prog, trait_method)?);
                let trait_ty = ocx.normalize_ty(&env, trait_ty);
                ocx.register_obligations(infcx.at(prog, env.clone()).sub(impl_ty, trait_ty).map_err(|_| NoSolution)?);
            }
            // :2333-2468: associated types prove their own clauses against
            // the hybrid environment above, without a signature relation.
            DefKind::AssocTy => {}
            _ => unreachable!("compare_impl_item requires an associated item"),
        }
        ocx.evaluate_obligations_error_on_ambiguity()?;
        if !infcx.resolve_regions(prog, env, &wf_tys).is_empty() { return Err(NoSolution); }
        if prog.def(method).kind == DefKind::AssocTy { self.check_type_bounds(prog, method, trait_method)?; }
        Ok(())
    }
}

impl Tcx {
    /// compare_impl_item.rs:2485-2597. Specialization/default associated types
    /// and const-trait conditions are cut; the associated value is a final leaf.
    fn check_type_bounds(&self, prog: &Program, item: DefId, trait_item: DefId) -> Result<(), NoSolution> {
        let imp = prog.def(item).parent;
        let td = prog.def(trait_item).parent;
        if !(crate::type_queries::Providers { tcx: self, prog }).specialization_graph((prog.def(imp).krate, td)).expect("coherent trait query") { return Err(NoSolution); }
        let parent = self.generic_indices_of(prog, imp);
        let own = self.generic_indices_of(prog, item);
        let identity = own.identity_args(&self.tys);
        let mut args = self.impl_trait_ref(prog, imp, &parent.identity_args(&self.tys)).args;
        args.extend_from_slice(&identity[parent.types_consts.len() + parent.lifetimes.len()..]);
        let env = self.param_env_id(prog, item);
        let normalize_env = self.intern_param_env(self.param_env_with_gat_bounds(prog, item)?);
        let infcx = InferCtxt::for_item(self, prog, TypingMode::non_body_analysis(), item);
        let mut ocx = ObligationCtxt::new(&infcx, prog);
        let mut obligations = Vec::new();
        for p in self.explicit_item_bounds(prog, trait_item) {
            let p = self.instantiate_predicate(prog, trait_item, &args, &p);
            for p in self.supertraits(prog, p) {
                let predicate = ocx.deeply_normalize_predicate(&normalize_env, &p)?;
                obligations.push(Goal { param_env: env.clone(), predicate });
            }
        }
        ocx.register_obligations(obligations);
        ocx.evaluate_obligations_error_on_ambiguity()?;
        // rustc_ty_utils/src/implied_bounds.rs: impl headers are assumed WF.
        let mut assumed = vec![self.impl_self[&imp]];
        if let Some(args) = self.impl_trait_args.get(&imp) { assumed.extend_from_slice(args); }
        let mut normalized = Vec::new();
        for &ty in &assumed { normalized.push(ocx.deeply_normalize_ty(&env, ty)?); }
        assumed.extend(normalized);
        if !infcx.resolve_regions(prog, env, &assumed).is_empty() { return Err(NoSolution); }
        Ok(())
    }

    /// compare_impl_item.rs:2635-2757: quantified normalization equalities for
    /// the GAT's own variables, and every RPITIT from the same function body.
    fn param_env_with_gat_bounds(&self, prog: &Program, item: DefId) -> Result<ParamEnv, NoSolution> {
        use crate::types::{BoundVar, BoundTy, BoundTyKind, RegionKind, BoundRegionKind, AliasTyKind};
        let mut clauses = self.param_env(prog, item).clauses.to_vec();
        let items = match prog.impl_trait_in_trait_data.get(&item) {
            Some(crate::assoc::ImplTraitInTraitData::Impl { fn_def_id } | crate::assoc::ImplTraitInTraitData::Trait { fn_def_id, .. }) => prog.associated_types_for_impl_traits.get(fn_def_id).cloned().unwrap_or_default(),
            None => vec![item],
        };
        for item in items {
            let imp = prog.def(item).parent;
            let trait_item = if prog.def(imp).kind == DefKind::Trait { item } else { *prog.trait_item_def_id.get(&item).ok_or(NoSolution)? };
            let parent = self.generic_indices_of(prog, imp);
            let own = self.generic_indices_of(prog, item);
            let mut full = parent.identity_args(&self.tys);
            let parent_count = full.len();
            let mut bound_vars = Vec::new();
            for i in parent_count..own.types_consts.len() + own.lifetimes.len() {
                let var = BoundVar(bound_vars.len() as u32);
                let arg = if own.lifetimes.contains(&(i as u32)) {
                    bound_vars.push(BoundVariableKind::Region(BoundRegionKind::Anon));
                    GenericArg::Lifetime(self.tys.intern_region(RegionKind::ReBound(0, var)))
                } else if own.consts.contains(&(i as u32)) {
                    bound_vars.push(BoundVariableKind::Const);
                    GenericArg::Const(self.tys.intern_const(crate::consts::ConstKind::Bound(0, crate::consts::BoundConst { var })))
                } else {
                    bound_vars.push(BoundVariableKind::Ty);
                    GenericArg::Type(self.tys.intern(TyKind::Bound(0, BoundTy { var, kind: BoundTyKind::Anon })))
                };
                full.push(arg);
            }
            let value = self.instantiate_ty(prog, item, &full, self.type_of(prog, item)?);
            let mut args = if prog.def(imp).kind == DefKind::Trait { parent.identity_args(&self.tys) } else { self.impl_trait_ref(prog, imp, &parent.identity_args(&self.tys)).args };
            args.extend_from_slice(&full[parent_count..]);
            let projection = self.alias_ty_with_args(AliasTyKind::Projection { def_id: trait_item }, &args);
            if self.tys.set_aliases_to_non_rigid(value) == projection { continue; }
            clauses.push(Predicate { bound_vars: bound_vars.into(), kind: PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: AliasTerm::Ty(projection), term: Term::Ty(value) })) });
        }
        Ok(ParamEnv { clauses: clauses.into() })
    }
}

impl Tcx {
    /// rustc_hir_analysis/src/check/check.rs:1302-1483. Specialization and
    /// unstable default-body attributes are cut; each impl is its own leaf.
    pub fn check_impl_items_against_trait(&self, prog: &Program, imp: DefId) -> Vec<String> {
        let Some(&td) = self.impl_trait.get(&imp) else { return Vec::new(); };
        let indices = self.generic_indices_of(prog, imp);
        let trait_ref = self.impl_trait_ref(prog, imp, &indices.identity_args(&self.tys));
        if trait_ref.args.iter().any(|arg| match *arg {
            GenericArg::Type(ty) => self.tys.has_error(ty),
            GenericArg::Const(ct) => self.tys.has_error(self.tys.intern(TyKind::Const(ct))),
            GenericArg::Lifetime(_) => false,
        }) { return Vec::new(); }
        let data = &prog.impls[prog.def(imp).sub as usize];
        let def = prog.def(imp);
        let ast = &prog.files[def.file as usize].ast;
        let mut errors = Vec::new();
        let error = |item: DefId, message: String| {
            let d = prog.def(item);
            prog.record_error(d.file, prog.files[d.file as usize].ast.item(d.item).lo, "typeck", message)
        };
        if ast.as_impl(&ast.item(def.item).kind).is_some_and(|i| i.negative) {
            if let Some(&item) = data.items.first() { errors.push(error(item, "E0749: negative impls cannot have any items".into())); }
            return errors;
        }
        for &item in &data.items {
            if !prog.trait_item_def_id.contains_key(&item) || prog.impl_trait_in_trait_data.contains_key(&item) { continue; }
            if matches!(prog.def(item).kind, DefKind::AssocFn | DefKind::AssocConst | DefKind::AssocTy) && self.compare_impl_item(prog, item).is_err() {
                let code = match prog.def(item).kind { DefKind::AssocConst => "E0326", DefKind::AssocTy => "E0277", _ => "E0053" };
                errors.push(error(item, format!("{code}: item `{}` has an incompatible type for trait", prog.name(item))));
            }
        }
        let unsized_self = match self.impl_self_is_guaranteed_unsized(prog, imp) {
            Ok(value) => value,
            Err(message) => { errors.push(error(imp, message)); false }
        };
        let mut missing = Vec::new();
        for &item in &prog.traits[prog.def(td).sub as usize] {
            if prog.impl_trait_in_trait_data.contains_key(&item) { continue; }
            let leaf = self.fetch_eligible_assoc_item(prog, item, imp);
            let d = prog.def(leaf);
            let pf = &prog.files[d.file as usize];
            let has_value = match pf.ast.item(d.item).kind {
                crate::ast::ItemKind::Fn(sig, body, _) => body.is_some() || pf.ast.fn_sig(sig).lazy_body.is_some(),
                crate::ast::ItemKind::Const(_, value) => value.is_some(),
                crate::ast::ItemKind::TypeAlias(r) => pf.ast.row(r).ty.is_some(),
                _ => false,
            };
            if !has_value && !(unsized_self && self.generics_require_sized_self(prog, item)) { missing.push(format!("`{}`", prog.name(item))); }
        }
        if !missing.is_empty() { errors.push(error(imp, format!("E0046: not all trait items implemented, missing: {}", missing.join(", ")))); }
        errors
    }

    /// rustc_ty_utils/src/ty.rs:348-398: normalize, then inspect the structural tail;
    /// an unconstrained parameter is not guaranteed unsized.
    fn impl_self_is_guaranteed_unsized(&self, prog: &Program, imp: DefId) -> Result<bool, String> {
        let infcx = InferCtxt::for_item(self, prog, TypingMode::non_body_analysis(), imp);
        let mut ocx = ObligationCtxt::new(&infcx, prog);
        let env = self.param_env_id(prog, imp);
        let mut ty = ocx.normalize_ty(&env, self.impl_self[&imp]);
        let limit = infcx.recursion_limit(prog);
        for _ in 0..=limit {
            match self.tys.kind(ty) {
                TyKind::Tuple(fields) => match self.tys.list(*fields).last() { Some(&tail) => ty = tail, None => return Ok(false) },
                TyKind::Adt(def, args, regions) => {
                    let adt = &self.adts[def];
                    if adt.is_enum || adt.is_union { return Ok(false); }
                    let Some(field) = adt.variants[0].fields.last() else { return Ok(false); };
                    let args = self.complete_args(prog, *def, self.tys.list(*args), *regions);
                    ty = ocx.normalize_ty(&env, self.instantiate_ty(prog, *def, &args, field.ty));
                }
                TyKind::Dyn(..) | TyKind::Slice(_) | TyKind::Str => return Ok(true),
                _ => return Ok(false),
            }
        }
        // rustc_middle/src/ty/util.rs:266-279: exhausting the structural-tail
        // recursion limit is an error, not a sizedness answer.
        Err("reached the recursion limit finding the struct tail".into())
    }
}
