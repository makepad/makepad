// ported from N:rustc_borrowck/src/implied_bounds.rs @8d1a764
use crate::program::Program;
use crate::types::{AliasTy, AliasTyKind};
use crate::tcx::{outlives::OutlivesPredicate, Tcx};
use crate::types::{RegionId, RegionKind, TyId, TyKind};

#[derive(Clone, Debug, Default)]
pub struct ImpliedBounds {
    pub outlives: Vec<(RegionId, RegionId)>,
    /// How many leading `outlives` are the param-env's region-outlives clauses (rustc inserts those
    /// into the universal region relations before the regions themselves, the implied ones after).
    pub explicit_outlives: usize,
    /// Generic and alias subjects are retained as types, not flattened slots.
    pub type_outlives: Vec<(TyId, RegionId)>,
    pub obligations: Vec<OutlivesPredicate>,
    /// `inferred_outlives_of` per ADT, in terms of its identity parameters (computed on demand).
    inferred: crate::fxhash::HashMap<crate::program::DefId, Vec<OutlivesPredicate>>,
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
        for p in predicates.iter().filter(|p| matches!(p, OutlivesPredicate::Region(..))) {
            result.assume(tcx, *p);
        }
        result.explicit_outlives = result.outlives.len();
        for p in predicates.iter().filter(|p| matches!(p, OutlivesPredicate::Type(..))) {
            result.assume(tcx, *p);
        }
        for i in 0..raw.len().max(normalized.len()) {
            let work = raw.get(i).into_iter().chain(normalized.get(i)).copied().collect();
            result.implied_wf_args(tcx,prog,work);
        }
        // Relation insertion order determines nightly's SCC representatives.
        let mut seen = crate::fxhash::HashSet::default();
        let explicit = result.explicit_outlives;
        let mut index = 0;
        let mut kept_explicit = 0;
        result.outlives.retain(|p| { let keep = seen.insert(*p); if keep && index < explicit { kept_explicit += 1; } index += 1; keep });
        result.explicit_outlives = kept_explicit;
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

    /// `inferred_outlives_of`: the outlives requirements the fields of `def` put on its own parameters.
    pub fn inferred_outlives(tcx: &Tcx, prog: &Program, def: crate::program::DefId) -> Vec<OutlivesPredicate> {
        Self::default().inferred_outlives_of(tcx, prog, def, &mut vec![])
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
                    // The ADT is only well-formed where its fields' types are (RFC 2093): `struct S<'a, T>(&'a T)`
                    // implies `T: 'a` wherever an `S<'a, T>` is.
                    let inferred = self.inferred_outlives_of(tcx,prog,*d,&mut vec![]);
                    for p in inferred {
                        let p = instantiate(tcx,p,tcx.tys.list(*args),&tcx.tys.region_args(adt.region_params),&tcx.tys.region_args(*rs));
                        if matches!(p,OutlivesPredicate::Type(t,_) if tcx.tys.has_vars_bound_at_or_above(t,0)) { continue; }
                        self.assume(tcx,p);
                    }
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
                    // The alias's region arguments are the opaque's lifetime parameters by index (the
                    // parent's, then the captured ones); the contract also lists late-bound ones.
                    let params: Vec<_> = if (d.0 as usize) < prog.defs.len() { tcx.generic_indices_of(prog, *d).lifetimes.iter().map(|&i| tcx.tys.intern_region(RegionKind::ReEarlyParam(i))).collect() }
                        else { decl.region_contract.regions.iter().map(|p| p.1).collect() }; // (a fixture's opaque is no program item)
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
                // The projection bounds (the object's own and its supertraits') are well-formed terms:
                // `dyn Foo<'a, T>` with `Foo: Bar<Assoc = &'a T>` implies `T: 'a`.
                if !tcx.tys.has_vars_bound_at_or_above(t,0) && (d.0 as usize) < prog.defs.len() && tcx.tys.list(*args).len() + 1 == tcx.generic_indices_of(prog, *d).types_consts.len() {
                    use crate::next_solver::{ClauseKind, PredicateKind, Term};
                    let mut clauses = tcx.object_predicates(prog, t);
                    if let Some(principal) = clauses.first().cloned() { clauses.extend(tcx.supertraits(prog, principal)); }
                    for p in clauses {
                        if let PredicateKind::Clause(ClauseKind::Projection(pc)) = p.kind {
                            if let Term::Ty(term) = pc.term { if !tcx.tys.has_vars_bound_at_or_above(term,0) { wf_args.push(term); } }
                        }
                    }
                }
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
    /// rustc_hir_analysis/src/outlives/implicit_infer.rs: the outlives predicates the field types of `def`
    /// require, with those of the ADTs they name (`insert_required_predicates_to_be_wf`). A cycle through
    /// `visiting` contributes nothing on its way back (rustc iterates to a fixpoint).
    fn inferred_outlives_of(&mut self, tcx: &Tcx, prog: &Program, def: crate::program::DefId, visiting: &mut Vec<crate::program::DefId>) -> Vec<OutlivesPredicate> {
        if let Some(done) = self.inferred.get(&def) { return done.clone(); }
        if let Some(done) = tcx.inferred_outlives.get(&def) { return (*done).clone(); }
        if visiting.contains(&def) { return vec![]; }
        let Some(adt) = tcx.adts.get(&def) else { return vec![] };
        visiting.push(def);
        let mut required: Vec<OutlivesPredicate> = vec![];
        for variant in &adt.variants {
            for field in &variant.fields { self.required_to_be_wf(tcx, prog, field.ty, visiting, &mut required); }
        }
        visiting.pop();
        // Only the ADT's own early-bound parameters can be required of it.
        required.retain(|p| match *p {
            OutlivesPredicate::Region(a, b) => [a, b].iter().all(|&r| matches!(tcx.tys.region_kind(r), RegionKind::ReEarlyParam(_))),
            OutlivesPredicate::Type(t, r) => matches!(tcx.tys.kind(t), TyKind::Param(_) | TyKind::Alias(..)) && matches!(tcx.tys.region_kind(r), RegionKind::ReEarlyParam(_)),
        });
        if visiting.is_empty() { self.inferred.insert(def, required.clone()); tcx.inferred_outlives.insert(def, std::sync::Arc::new(required.clone())); }
        required
    }
    fn required_to_be_wf(&mut self, tcx: &Tcx, prog: &Program, ty: TyId, visiting: &mut Vec<crate::program::DefId>, out: &mut Vec<OutlivesPredicate>) {
        let tys = &tcx.tys;
        let push = |p: OutlivesPredicate, out: &mut Vec<OutlivesPredicate>| if !out.contains(&p) { out.push(p) };
        match tys.kind(ty).clone() {
            TyKind::Adt(d, args, rs) => {
                if let Some(adt) = tcx.adts.get(&d) {
                    let params = tys.region_args(adt.region_params);
                    let explicit = adt.region_contract.predicates.clone();
                    let inferred = self.inferred_outlives_of(tcx, prog, d, visiting);
                    for p in explicit.into_iter().chain(inferred) {
                        let p = instantiate(tcx, p, tys.list(args), &params, &tys.region_args(rs));
                        if !matches!(p, OutlivesPredicate::Type(t, _) if tys.has_vars_bound_at_or_above(t, 0)) { push(p, out); }
                    }
                }
                for t in tys.list(args).to_vec() { self.required_to_be_wf(tcx, prog, t, visiting, out); }
            }
            TyKind::Ref(_, inner, r) => {
                // `&'r T` requires `T: 'r`: the components of T each outlive 'r.
                if matches!(tys.region_kind(r), RegionKind::ReBound(..)) || tys.has_vars_bound_at_or_above(inner, 0) { return; }
                let mut components = ImpliedBounds::default();
                components.components(tcx, inner, r);
                for (a, b) in components.outlives { push(OutlivesPredicate::Region(a, b), out); }
                for (t, r) in components.type_outlives { push(OutlivesPredicate::Type(t, r), out); }
                self.required_to_be_wf(tcx, prog, inner, visiting, out);
            }
            TyKind::Tuple(ts) => { for t in tys.list(ts).to_vec() { self.required_to_be_wf(tcx, prog, t, visiting, out); } }
            TyKind::Array(t, _) | TyKind::Slice(t) | TyKind::Ptr(_, t) => self.required_to_be_wf(tcx, prog, t, visiting, out),
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
    pub fn consider_implied_bounds_compat(&mut self, tcx: &Tcx, prog: &crate::program::Program,
        ty: TyId, registered_since_snapshot: &[OutlivesPredicate]) {
        if !contains_bevy_param_set(tcx, prog, ty) { return; }
        for &p in registered_since_snapshot {
            if matches!(p,OutlivesPredicate::Type(..)) { self.assume(tcx,p); }
        }
    }

    /// Apply external query substitutions first, then discard residual placeholders.
    pub fn instantiate_query_bounds(&mut self, tcx: &Tcx, replacements: &[(RegionId,RegionId)]) {
        let region = |r| replacements.iter().find(|p| p.0 == r).map_or(r,|p| p.1);
        let placeholder = |r| matches!(tcx.tys.region_kind(r),RegionKind::RePlaceholder { .. });
        let explicit = self.explicit_outlives;
        let mut kept_explicit = 0;
        let mapped: Vec<(RegionId, RegionId)> = self.outlives.iter().enumerate().map(|(i, &(a,b))| (i, (region(a),region(b))))
            .filter(|&(_, (a,b))| !placeholder(a) && !placeholder(b))
            .map(|(i, p)| { if i < explicit { kept_explicit += 1; } p }).collect();
        self.outlives = mapped;
        self.explicit_outlives = kept_explicit;
        self.type_outlives = self.type_outlives.iter().filter_map(|&(t,r)| {
            let r = region(r);
            let has_placeholder = std::cell::Cell::new(placeholder(r));
            let t = tcx.tys.map_regions(t,&|r,_| { let r = region(r); has_placeholder.set(has_placeholder.get() || placeholder(r)); r },0);
            if has_placeholder.get() || contains_type_placeholder(tcx,t) { None } else { Some((t,r)) }
        }).collect();
    }
}
/// `ContainsBevyParamSet`: `ParamSet<T>` of crate `bevy_ecs`, or a reference to one (references only; the
/// arguments of other types are not searched).
fn contains_bevy_param_set(tcx: &Tcx, prog: &crate::program::Program, mut ty: TyId) -> bool {
    loop {
        match tcx.tys.kind(ty) {
            TyKind::Ref(_,inner,_) => ty = *inner,
            TyKind::Adt(d,_,_) => {
                let def = prog.def(*d);
                return prog.name(*d) == "ParamSet" && prog.syms.str(prog.crates[def.krate as usize].name) == "bevy_ecs";
            }
            _ => return false,
        }
    }
}

/// The search of `compute_implied_outlives_bounds` through `ty`'s well-formedness: every WF obligation is
/// deeply normalized (which selects impls and registers their outlives obligations), and each WF predicate it
/// yields is searched in turn. Only the registered region obligations matter to the bevy compatibility rule.
fn normalize_wf_obligations(infcx: &crate::infer::InferCtxt<'_>, ocx: &mut crate::next_solver::fulfill::ObligationCtxt<'_, '_>, prog: &Program, param_env: &crate::next_solver::ParamEnvId, tys: &[TyId]) -> Result<(), crate::next_solver::NoSolution> {
    use crate::next_solver::{ClauseKind, PredicateKind, Term};
    let mut checked: crate::fxhash::HashSet<Term> = Default::default();
    let mut wf_args: Vec<Term> = tys.iter().map(|&t| Term::Ty(t)).collect();
    while let Some(arg) = wf_args.pop() {
        if !checked.insert(arg) { continue; }
        for goal in crate::next_solver::wf::unnormalized_obligations(infcx, prog, param_env, arg)?.into_iter().flatten() {
            let pred = ocx.deeply_normalize_predicate(param_env, &goal.predicate)?;
            if let PredicateKind::Clause(ClauseKind::WellFormed(term)) = pred.kind { wf_args.push(term); }
        }
    }
    Ok(())
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
        infcx.inner.borrow().region_obligations()[snapshot..].iter().map(|&(t, r, _)| OutlivesPredicate::Type(erase_vars(t), if matches!(tcx.tys.region_kind(r), RegionKind::ReVar(_)) { RegionId::ERASED } else { r })).collect()
    };
    let mut bounds = ImpliedBounds::create(tcx,prog,predicates,&[],&[],vec![]);
    let mut normalized_inputs_and_output = vec![];
    let failed = std::cell::Cell::new(false);
    for &ty in inputs_and_output {
        let snapshot = infcx.inner.borrow().region_obligations().len();
        let normalized_ty = crate::type_queries::place_types::with_foreign_region_vars(&infcx, ty, |ty| crate::type_queries::place_types::in_signature_binder(&tcx.tys, ty, |ty| ocx.deeply_normalize_ty(param_env, ty).map_err(|_| failed.set(true)).ok())).unwrap_or(ty);
        if failed.get() { return Err(crate::next_solver::NoSolution); }
        bounds.implied_wf_args(tcx,prog,vec![ty,normalized_ty]);
        if contains_bevy_param_set(tcx, prog, ty) && normalize_wf_obligations(&infcx, &mut ocx, prog, param_env, &[ty, normalized_ty]).is_err() { return Err(crate::next_solver::NoSolution); }
        bounds.consider_implied_bounds_compat(tcx,prog,normalized_ty,&registered_since(snapshot));
        normalized_inputs_and_output.push(normalized_ty);
    }
    for &ty in assumed_wf {
        let normalized_ty = crate::type_queries::place_types::with_foreign_region_vars(&infcx, ty, |ty| crate::type_queries::place_types::in_signature_binder(&tcx.tys, ty, |ty| ocx.deeply_normalize_ty(param_env, ty).map_err(|_| failed.set(true)).ok())).unwrap_or(ty);
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
/// The placeholders are numbered from `base`: a nested body's are numbered after those it inherits from its creator,
/// so that its own external regions stay different from the creator's.
pub fn replace_erased_regions_with_placeholders(tcx: &Tcx, inputs_and_output: &[TyId], base: u32) -> Vec<TyId> {
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
    let mut replacer = Replacer { next:base };
    inputs_and_output.iter().map(|&ty| replacer.fold_ty(&tcx.tys,ty,0)).collect()
}
