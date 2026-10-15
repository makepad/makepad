// ported from N:rustc_borrowck/src/implied_bounds.rs @8d1a764
use crate::program::Program;
use crate::types::{AliasTy, AliasTyKind};
use crate::tcx::{outlives::OutlivesPredicate, Tcx};
use crate::types::{RegionId, RegionKind, TyId, TyKind};

#[derive(Clone, Debug, Default)]
pub struct ImpliedBounds {
    pub outlives: Vec<(RegionId, RegionId)>,
    /// Generic and alias subjects are retained as types, not flattened slots.
    pub type_outlives: Vec<(TyId, RegionId)>,
    pub obligations: Vec<OutlivesPredicate>,
}

impl ImpliedBounds {
    pub fn create(
        tcx: &Tcx, prog: &Program,
        predicates: &[OutlivesPredicate],
        raw: &[TyId],
        normalized: &[TyId],
        obligations: Vec<OutlivesPredicate>,
    ) -> Self {
        let mut result = Self {
            obligations,
            ..Self::default()
        };
        for p in predicates {
            result.assume(tcx, *p);
        }
        for i in 0..raw.len().max(normalized.len()) {
            let work = raw.get(i).into_iter().chain(normalized.get(i)).copied().collect();
            result.implied_wf_args(tcx,prog,work);
        }
        // Relation insertion order determines nightly's SCC representatives.
        let mut seen = crate::fxhash::HashSet::default();
        result.outlives.retain(|p| seen.insert(*p));
        let mut seen = crate::fxhash::HashSet::default();
        result.type_outlives.retain(|p| seen.insert(*p));
        result
    }

    pub fn assume(&mut self, tcx: &Tcx, predicate: OutlivesPredicate) {
        match predicate {
            OutlivesPredicate::Region(a, b) => {
                if free(tcx, a) && free(tcx, b) {
                    self.outlives.push((a, b));
                }
            }
            OutlivesPredicate::Type(t, r) => {
                if free(tcx, r) {
                    self.components(tcx, t, r);
                }
            }
        }
    }

    fn components(&mut self, tcx: &Tcx, t: TyId, r: RegionId) {
        match tcx.tys.kind(t) {
            TyKind::Param(_) => self.type_outlives.push((t,r)),
            TyKind::Alias(..) => {
                // EscapingAlias components imply no bounds (nightly #25860).
                if !tcx.tys.has_vars_bound_at_or_above(t,0) { self.type_outlives.push((t,r)); }
            }
            TyKind::Ref(_, t, a) => {
                self.assume(tcx, OutlivesPredicate::Region(*a, r));
                self.components(tcx, *t, r);
            }
            TyKind::Adt(_, ts, rs) => {
                for a in tcx.tys.region_args(*rs).iter() {
                    self.assume(tcx, OutlivesPredicate::Region(*a, r));
                }
                for t in tcx.tys.list(*ts) {
                    self.components(tcx, *t, r);
                }
            }
            TyKind::FnDef(_, ts, _) => {
                // #70917: shallow lifetime arguments of function items do not count.
                for t in tcx.tys.list(*ts) { self.components(tcx,*t,r); }
            }
            TyKind::FnPtr(ts,out,_,_) => {
                for t in tcx.tys.list(*ts).iter().chain([out]) { self.components(tcx,*t,r); }
            }
            TyKind::Tuple(ts) => {
                for t in tcx.tys.list(*ts) {
                    self.components(tcx, *t, r);
                }
            }
            TyKind::Array(t, _) | TyKind::Slice(t) | TyKind::Ptr(_, t) => {
                self.components(tcx, *t, r)
            }
            TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: upvars, .. }) => self.components(tcx, *upvars, r),
            TyKind::Dyn(_, ts, bindings, rs, a, _, _) => {
                for t in tcx.tys.list(*ts) { self.components(tcx,*t,r); }
                for (_,t,_) in tcx.tys.list(*bindings) { self.components(tcx,*t,r); }
                for a in tcx.tys.region_args(*rs).iter().chain([a]) {
                    self.assume(tcx,OutlivesPredicate::Region(*a,r));
                }
            }
            _ => {}
        }
    }

    fn implied(&mut self, tcx: &Tcx, prog: &Program, t: TyId) {
        self.implied_wf_args(tcx,prog,vec![t]);
    }
    fn implied_wf_args(&mut self, tcx: &Tcx, prog: &Program, mut wf_args: Vec<TyId>) {
        let mut checked_wf_args = crate::fxhash::HashSet::default();
        while let Some(t) = wf_args.pop() {
            if !checked_wf_args.insert(t) { continue; }
            self.implied_wf_arg(tcx,prog,t,&mut wf_args);
        }
    }
    // Concrete WF obligations from Relax's collected RegionContract. Trait
    // selection and normalization-generated obligations are supplied by C2.
    fn implied_wf_arg(&mut self, tcx: &Tcx, prog: &Program, t: TyId, wf_args: &mut Vec<TyId>) {
        match tcx.tys.kind(t) {
            TyKind::Ref(_, t, r) => {
                if !tcx.tys.has_vars_bound_at_or_above(*t,0) {
                    self.assume(tcx, OutlivesPredicate::Type(*t, *r));
                }
                wf_args.push(*t);
            }
            TyKind::Adt(d, args, rs) => {
                if let Some(adt) = tcx.adts.get(d) {
                    self.nominal(tcx,&adt.region_contract,tcx.tys.list(*args),&tcx.tys.region_args(adt.region_params),*rs);
                }
                for t in tcx.tys.list(*args) {
                    wf_args.push(*t);
                }
            }
            TyKind::Tuple(ts) => {
                for t in tcx.tys.list(*ts) {
                    wf_args.push(*t);
                }
            }
            TyKind::Array(t, _) | TyKind::Slice(t) | TyKind::Ptr(_, t) => wf_args.push(*t),
            TyKind::FnPtr(ts,out,_,_) => wf_args.extend(tcx.tys.list(*ts).iter().chain([out]).copied()),
            TyKind::FnDef(d,args,rs) => {
                if let Some(sig) = tcx.fn_item_sig(prog, *d) {
                    let params: Vec<_> = sig.generic_indices.lifetimes.iter()
                        .map(|&i| tcx.tys.intern_region(RegionKind::ReEarlyParam(i))).collect();
                    self.nominal(tcx,&sig.region_contract,tcx.tys.list(*args),&params,*rs);
                    let OutlivesPredicate::Type(output,_) = instantiate(tcx,OutlivesPredicate::Type(sig.ret,RegionId::ERASED),tcx.tys.list(*args),&params,&tcx.tys.region_args(*rs)) else { unreachable!() };
                    wf_args.push(output);
                }
                wf_args.extend(tcx.tys.list(*args));
            }
            TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { def_id: d }, args, regions: rs }) => {
                if let Some(contract) = tcx.nominal_region_contracts.get(d) {
                    let params: Vec<_> = contract.regions.iter().map(|p| p.1).collect();
                    self.nominal(tcx,contract,tcx.tys.list(*args),&params,*rs);
                }
                wf_args.extend(tcx.tys.list(*args));
            }
            TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { def_id: d }, args, regions: rs }) => {
                if let Some(decl) = tcx.opaques.get(d) {
                    let params: Vec<_> = decl.region_contract.regions.iter().map(|p| p.1).collect();
                    self.nominal(tcx,&decl.region_contract,tcx.tys.list(*args),&params,*rs);
                }
                wf_args.extend(tcx.tys.list(*args));
            }
            TyKind::Closure(_, _, crate::types::ClosureArgs { parent_args: owner, tupled_upvars_ty: upvars, .. }) => {
                if let TyKind::FnDef(d,args,rs) = tcx.tys.kind(*owner) {
                    if let Some(sig) = tcx.fn_item_sig(prog, *d) {
                        let params: Vec<_> = sig.generic_indices.lifetimes.iter()
                            .map(|&i| tcx.tys.intern_region(RegionKind::ReEarlyParam(i))).collect();
                        self.nominal(tcx,&sig.region_contract,tcx.tys.list(*args),&params,*rs);
                    }
                }
                wf_args.push(*upvars);
            }
            TyKind::Dyn(d,args,_,rs,r,_,_) => {
                // Object WF: the explicit bound implies each elaborated Self
                // bound. Generic argument WF is deliberately NOT required.
                if !tcx.tys.has_vars_bound_at_or_above(t,0) {
                    if let Some(contract) = tcx.nominal_region_contracts.get(d) {
                        let params: Vec<_> = contract.regions.iter().map(|p| p.1).collect();
                        let mut types = vec![t]; types.extend(tcx.tys.list(*args));
                        for &p in &contract.predicates {
                            if let OutlivesPredicate::Type(subject,_) = p {
                                if tcx.tys.kind(subject) != &TyKind::Param(0) { continue; }
                                if let OutlivesPredicate::Type(_,bound) = instantiate(tcx,p,&types,&params,&tcx.tys.region_args(*rs)) {
                                    self.assume(tcx,OutlivesPredicate::Region(*r,bound));
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    fn nominal(&mut self, tcx: &Tcx, contract: &crate::tcx::outlives::RegionContract,
        args: &[TyId], params: &[RegionId], rs: crate::types::RegionArgsId) {
        for &p in &contract.predicates {
            let p = instantiate(tcx,p,args,params,&tcx.tys.region_args(rs));
            if matches!(p,OutlivesPredicate::Type(t,_) if tcx.tys.has_vars_bound_at_or_above(t,0)) { continue; }
            self.assume(tcx,p);
        }
    }

    /// The BIR adapter substitutes external query variables into its own SR
    /// arena. Keep only those explicitly bound by that adapter; all remaining
    /// region/type placeholders are residual, including ROOT placeholders.
    pub fn retain_instantiated_query_bounds(&mut self, tcx: &Tcx, external: &[RegionId]) {
        let residual = |r| matches!(tcx.tys.region_kind(r),RegionKind::RePlaceholder { .. }) && !external.contains(&r);
        self.outlives.retain(|&(a,b)| !residual(a) && !residual(b));
        self.type_outlives.retain(|&(t,r)| {
            let found = std::cell::Cell::new(residual(r));
            tcx.tys.map_regions(t,&|r,_| { found.set(found.get() || residual(r)); r },0);
            !found.get() && !contains_type_placeholder(tcx,t)
        });
    }

    // rustc_trait_selection::traits::implied_outlives_bounds @8d1a764.
    // Match only ParamSet and references to it, not arbitrary nested arguments.
    pub fn consider_implied_bounds_compat(&mut self, tcx: &Tcx, prog: &crate::program::Program,
        mut ty: TyId, registered_since_snapshot: &[OutlivesPredicate]) {
        loop {
            match tcx.tys.kind(ty) {
                TyKind::Ref(_,inner,_) => ty = *inner,
                TyKind::Adt(d,_,_) => {
                    let def = prog.def(*d);
                    if prog.name(*d) == "ParamSet" && prog.syms.str(prog.crates[def.krate as usize].name) == "bevy_ecs" {
                        for &p in registered_since_snapshot {
                            if matches!(p,OutlivesPredicate::Type(..)) { self.assume(tcx,p); }
                        }
                    }
                    break;
                }
                _ => break,
            }
        }
    }

    /// Apply external query substitutions first, then discard residual placeholders.
    pub fn instantiate_query_bounds(&mut self, tcx: &Tcx, replacements: &[(RegionId,RegionId)]) {
        let region = |r| replacements.iter().find(|p| p.0 == r).map_or(r,|p| p.1);
        let placeholder = |r| matches!(tcx.tys.region_kind(r),RegionKind::RePlaceholder { .. });
        self.outlives = self.outlives.iter().map(|&(a,b)| (region(a),region(b)))
            .filter(|&(a,b)| !placeholder(a) && !placeholder(b)).collect();
        self.type_outlives = self.type_outlives.iter().filter_map(|&(t,r)| {
            let r = region(r);
            let has_placeholder = std::cell::Cell::new(placeholder(r));
            let t = tcx.tys.map_regions(t,&|r,_| { let r = region(r); has_placeholder.set(has_placeholder.get() || placeholder(r)); r },0);
            if has_placeholder.get() || contains_type_placeholder(tcx,t) { None } else { Some((t,r)) }
        }).collect();
    }
}
fn contains_type_placeholder(tcx: &Tcx, t: TyId) -> bool {
    tcx.tys.flags(t).intersects(crate::types::TypeFlags::HAS_TY_PLACEHOLDER)
}

fn free(tcx: &Tcx, r: RegionId) -> bool {
    !matches!(
        tcx.tys.region_kind(r),
        RegionKind::ReBound(..) | RegionKind::ReErased | RegionKind::ReError
    )
}

pub fn instantiate(
    tcx: &Tcx,
    p: OutlivesPredicate,
    args: &[TyId],
    params: &[RegionId],
    regions: &[RegionId],
) -> OutlivesPredicate {
    let region = |r| match params.iter().position(|p| *p == r) {
        Some(i) => regions.get(i).copied().unwrap_or(RegionId::ERASED),
        None => r,
    };
    match p {
        OutlivesPredicate::Region(a, b) => OutlivesPredicate::Region(region(a), region(b)),
        OutlivesPredicate::Type(t, r) => OutlivesPredicate::Type(
            tcx.tys
                .subst(tcx.tys.map_regions(t, &|r, _| region(r), 0), args),
            region(r),
        ),
    }
}

/// Implied bounds are computed outside a defining root's opaque scope; nested
/// bodies retain the parent's environment. The normalized signature is returned
/// together with the assumptions, as required for subsequent I/O equality.
/// Normalization is the query's own (rustc_borrowck/src/implied_bounds.rs:27-118:
/// `ocx.deeply_normalize` in a separate inference context); its region constraints are
/// produced again by the body's renormalization through fully_perform_op (nll.rs). A
/// region variable left in a result is erased, since this context's variables cannot
/// leave it. A type that fails to normalize fails the whole query (implied_bounds.rs:59,95
/// `.map_err(|_| NoSolution)?`); the caller then keeps only `predicates` and the
/// unnormalized signature (free_region_relations.rs:523-529, see `query_failed`).
/// A type `in_signature_binder` cannot place under the signature's binder stays
/// unnormalized without failing (that is not a normalization result).
#[allow(clippy::too_many_arguments)]
pub fn mir_borrowck_implied_outlives_bounds(
    tcx: &Tcx, prog: &crate::program::Program, owner: crate::program::DefId, body_typing_mode: crate::tcx::TypingMode, param_env: &crate::next_solver::ParamEnvId,
    nested: bool, predicates: &[OutlivesPredicate], inputs_and_output: &[TyId], assumed_wf: &[TyId],
) -> Result<(ImpliedBounds, Vec<TyId>), crate::next_solver::NoSolution> {
    let typing_mode = if nested { body_typing_mode } else { crate::tcx::TypingMode::non_body_analysis() };
    let infcx = crate::infer::InferCtxt::for_item(tcx, prog, typing_mode, owner);

    let mut ocx = crate::next_solver::fulfill::ObligationCtxt::new(&infcx, prog);
    let erase_vars = |ty: TyId| tcx.tys.map_regions(ty, &|r, _| if matches!(tcx.tys.region_kind(r), RegionKind::ReVar(_)) { RegionId::ERASED } else { r }, 0);
    let registered_since = |snapshot: usize| -> Vec<OutlivesPredicate> {
        infcx.inner.borrow().region_obligations()[snapshot..].iter().map(|&(t, r)| OutlivesPredicate::Type(erase_vars(t), if matches!(tcx.tys.region_kind(r), RegionKind::ReVar(_)) { RegionId::ERASED } else { r })).collect()
    };
    let mut bounds = ImpliedBounds::create(tcx,prog,predicates,&[],&[],vec![]);
    let mut normalized_inputs_and_output = vec![];
    let failed = std::cell::Cell::new(false);
    for &ty in inputs_and_output {
        let snapshot = infcx.inner.borrow().region_obligations().len();
        let normalized_ty = crate::borrowck::tyx::with_foreign_region_vars(&infcx, ty, |ty| crate::borrowck::tyx::in_signature_binder(&tcx.tys, ty, |ty| ocx.deeply_normalize_ty(param_env, ty).map_err(|_| failed.set(true)).ok())).unwrap_or(ty);
        if failed.get() { return Err(crate::next_solver::NoSolution); }
        bounds.implied_wf_args(tcx,prog,vec![ty,normalized_ty]);
        bounds.consider_implied_bounds_compat(tcx,prog,normalized_ty,&registered_since(snapshot));
        normalized_inputs_and_output.push(normalized_ty);
    }
    for &ty in assumed_wf {
        let normalized_ty = crate::borrowck::tyx::with_foreign_region_vars(&infcx, ty, |ty| crate::borrowck::tyx::in_signature_binder(&tcx.tys, ty, |ty| ocx.deeply_normalize_ty(param_env, ty).map_err(|_| failed.set(true)).ok())).unwrap_or(ty);
        if failed.get() { return Err(crate::next_solver::NoSolution); }
        // Impl-header normalization obligations do not enter the compatibility rule.
        let snapshot = infcx.inner.borrow().region_obligations().len();
        bounds.implied(tcx,prog,normalized_ty);
        bounds.consider_implied_bounds_compat(tcx,prog,normalized_ty,&registered_since(snapshot));
    }
    Ok((bounds,normalized_inputs_and_output))
}

/// free_region_relations.rs:523-529: the query failed -- a delayed bug (an error must
/// have been reported for the signature), no implied bounds, the unnormalized signature.
/// The declared outlives predicates are the ParamEnv's (added before the query there).
pub fn query_failed(tcx: &Tcx, prog: &Program, body: crate::program::DefId,
    predicates: &[OutlivesPredicate], inputs_and_output: &[TyId]) -> (ImpliedBounds, Vec<TyId>) {
    tcx.dcx.delayed_bug(crate::next_solver::fulfill::ObligationCause { span: 0, body_id: body },
        format!("error computing implied bounds {}", prog.def_path(body)));
    (ImpliedBounds::create(tcx,prog,predicates,&[],&[],vec![]), inputs_and_output.to_vec())
}
pub fn implied_bounds_query_var_values(tcx: &Tcx, inputs_and_output: &[TyId], is_external_region: impl Fn(RegionId) -> bool) -> Vec<RegionId> {
    let values = std::cell::RefCell::new(vec![]);
    for &ty in inputs_and_output {
        tcx.tys.map_regions(ty,&|r,_| {
            if !matches!(tcx.tys.region_kind(r),RegionKind::ReBound(..)) && is_external_region(r) { values.borrow_mut().push(r); } r
        },0);
    }
    values.into_inner()
}
pub fn replace_erased_regions_with_placeholders(tcx: &Tcx, inputs_and_output: &[TyId]) -> Vec<TyId> {
    use crate::types::fold::TypeFolder;
    struct Replacer { next: u32 }
    impl TypeFolder for Replacer {
        fn fold_region(&mut self, tys: &crate::types::Types, r: RegionId, _: u32) -> RegionId {
            if tys.region_kind(r) == RegionKind::ReErased {
                let var = crate::types::BoundVar(self.next); self.next += 1;
                tys.intern_region(RegionKind::RePlaceholder { universe:crate::types::UniverseIndex::ROOT,var })
            } else { r }
        }
    }
    let mut replacer = Replacer { next:0 };
    inputs_and_output.iter().map(|&ty| replacer.fold_ty(&tcx.tys,ty,0)).collect()
}
