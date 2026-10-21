// ported from N:rustc_next_trait_solver/src/canonical/mod.rs @8d1a764
use super::*;
use crate::infer::relate::{TypeRelation, TypeError, structurally_relate_tys, structurally_relate_consts, relate_fn_sig, relate_args};
use crate::infer::unify_key::{ConstVariableValue, IntVarValue, FloatVarValue};
use crate::infer::opaque_types::ProvisionalHiddenType;
use crate::types::variance::V;
use super::super::fold::fold_response;

/// canonical/mod.rs:290: replay relates aliases structurally. Normalizing an
/// alias again here would discard constraints established by the query.
pub struct ResponseRelating<'a, 'tcx> { pub infcx: &'a InferCtxt<'tcx>, pub prog: &'a crate::program::Program }
impl<'tcx> TypeRelation<'tcx> for ResponseRelating<'_, 'tcx> {
    fn tcx(&self) -> &'tcx crate::tcx::Tcx { self.infcx.tcx }
    fn prog(&self) -> &crate::program::Program { self.prog }
    fn variance(&self) -> V { V::Inv }
    fn set_variance(&mut self, _: V) {}
    fn tys(&mut self, a: TyId, b: TyId) -> Result<TyId, TypeError> {
        if a == b { return Ok(a); }
        let infcx = self.infcx;
        let a = infcx.shallow_resolve(a);
        let b = infcx.shallow_resolve(b);
        if a == b { return Ok(a); }
        match (infcx.tcx.tys.kind(a), infcx.tcx.tys.kind(b)) {
            (TyKind::Infer(a), TyKind::Infer(b)) => infcx.inner.borrow_mut().type_variables().equate(*a, *b),
            (TyKind::Infer(vid), _) => infcx.inner.borrow_mut().type_variables().instantiate(*vid, b),
            (_, TyKind::Infer(vid)) => infcx.inner.borrow_mut().type_variables().instantiate(*vid, a),
            (TyKind::Error, _) | (_, TyKind::Error) => { infcx.set_tainted_by_errors(); return Ok(infcx.tcx.tys.error); }
            (TyKind::IntVar(a), TyKind::IntVar(b)) => infcx.inner.borrow_mut().int_unification_table().union(*a, *b),
            (TyKind::FloatVar(a), TyKind::FloatVar(b)) => infcx.inner.borrow_mut().float_unification_table().union(*a, *b),
            (TyKind::IntVar(vid), TyKind::Int(p)) | (TyKind::Int(p), TyKind::IntVar(vid)) => {
                use crate::program::Prim;
                let value = if matches!(p, Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64 | Prim::I128 | Prim::Isize) {
                    IntVarValue::IntType(*p)
                } else { IntVarValue::UintType(*p) };
                infcx.inner.borrow_mut().int_unification_table().union_value(*vid, value);
            }
            (TyKind::FloatVar(vid), TyKind::Float(p)) | (TyKind::Float(p), TyKind::FloatVar(vid)) => {
                infcx.inner.borrow_mut().float_unification_table().union_value(*vid, FloatVarValue::Known(*p));
            }
            _ => { structurally_relate_tys(self, a, b)?; }
        }
        Ok(a)
    }
    fn consts(&mut self, a: Const, b: Const) -> Result<Const, TypeError> {
        if a == b { return Ok(a); }
        let infcx = self.infcx;
        let a = infcx.shallow_resolve_const(a);
        let b = infcx.shallow_resolve_const(b);
        if a == b { return Ok(a); }
        match (infcx.tcx.tys.const_kind(a), infcx.tcx.tys.const_kind(b)) {
            (ConstKind::Infer(InferConst::Var(a)), ConstKind::Infer(InferConst::Var(b))) => {
                infcx.inner.borrow_mut().const_unification_table().union(a.0, b.0);
            }
            (ConstKind::Infer(InferConst::Var(vid)), _) => {
                infcx.inner.borrow_mut().const_unification_table().union_value(vid.0, ConstVariableValue::Known { value: b });
            }
            (_, ConstKind::Infer(InferConst::Var(vid))) => {
                infcx.inner.borrow_mut().const_unification_table().union_value(vid.0, ConstVariableValue::Known { value: a });
            }
            _ => { structurally_relate_consts(self, a, b)?; }
        }
        Ok(a)
    }
    fn regions(&mut self, a: RegionId, b: RegionId) -> Result<RegionId, TypeError> {
        // Bound variables in response binders have already been matched by the
        // query; avoid passing identical bound regions to the region collector.
        if a != b { self.infcx.inner.borrow_mut().unwrap_region_constraints().make_eqregion(&self.infcx.tcx.tys, a, b, true); }
        Ok(a)
    }
    fn binders(&mut self, a: TyId, b: TyId) -> Result<TyId, TypeError> { relate_fn_sig(self, a, b) }
    fn binders_args(&mut self, a: &[GenericArg], b: &[GenericArg]) -> Result<Vec<GenericArg>, TypeError> { relate_args(self, a, b) }
}

pub fn canonicalize_query_response(infcx: &InferCtxt<'_>, max_input_universe: UniverseIndex, response: &Response) -> Canonical<Response> {
    let mut resolved = fold_response(infcx.tcx, response, &mut DeepVariableResolver { infcx });
    filter_irrelevant_region_constraints(infcx, &mut resolved);
    let mut c = Canonicalizer::new(infcx, CanonicalizeMode::Response { max_input_universe });
    let value = fold_response(infcx.tcx, &resolved, &mut c);
    let (max_universe, _, var_kinds) = c.finalize();
    Canonical { value, max_universe, var_kinds }
}

fn compute_query_response_instantiation_values(infcx: &InferCtxt<'_>, original: &[GenericArg], response: &Canonical<Response>) -> Vec<GenericArg> {
    let previous = infcx.universe();
    compute_query_response_instantiation_values_in_universe(infcx, original, response, previous)
}
pub(super) fn compute_query_response_instantiation_values_in_universe(infcx: &InferCtxt<'_>, original: &[GenericArg], response: &Canonical<Response>, previous: UniverseIndex) -> Vec<GenericArg> {
    while infcx.universe().0 < previous.0 + response.max_universe.0 { infcx.create_next_universe(); }
    assert_eq!(original.len(), response.value.var_values.len());
    let tys = &infcx.tcx.tys;
    let mut guessed = vec![None; response.var_kinds.len()];
    for (&orig, &value) in original.iter().zip(&response.value.var_values) {
        let index = match value {
            GenericArg::Type(ty) => match tys.kind(ty) {
                TyKind::Bound(0, b) if !matches!(response.var_kinds[b.var.0 as usize], CanonicalVarKind::Ty { .. }) => Some(b.var),
                _ => None,
            },
            GenericArg::Lifetime(region) => match tys.region_kind(region) { RegionKind::ReBound(0, b) => Some(b), _ => None },
            GenericArg::Const(ct) => match tys.const_kind(ct) { ConstKind::Bound(0, b) => Some(b.var), _ => None },
        };
        if let Some(index) = index { guessed[index.0 as usize] = Some(orig); }
    }
    let mut values = Vec::with_capacity(response.var_kinds.len());
    for (index, &kind) in response.var_kinds.iter().enumerate() {
        let universe = UniverseIndex(previous.0 + kind.universe().0);
        let placeholder_index = match kind {
            CanonicalVarKind::PlaceholderTy(p) => Some(p.bound),
            CanonicalVarKind::PlaceholderRegion { var, .. } => Some(var),
            CanonicalVarKind::PlaceholderConst(p) => Some(p.bound.var),
            _ => None,
        };
        if kind.universe() == UniverseIndex::ROOT {
            if let Some(var) = placeholder_index { values.push(original[var.0 as usize]); continue; }
            if let Some(value) = guessed[index] { values.push(value); continue; }
        }
        values.push(match kind {
            CanonicalVarKind::Ty { sub_root, .. } => {
                let ty = infcx.next_ty_var_in_universe(universe);
                if sub_root.0 < index as u32 {
                    let GenericArg::Type(other) = values[sub_root.0 as usize] else { panic!("sub-root kind") };
                    let (TyKind::Infer(a), TyKind::Infer(b)) = (tys.kind(ty), tys.kind(other)) else { panic!("sub-root value") };
                    infcx.inner.borrow_mut().type_variables().sub_unify(*a, *b);
                }
                GenericArg::Type(ty)
            }
            CanonicalVarKind::Int => GenericArg::Type(infcx.next_int_var()),
            CanonicalVarKind::Float => GenericArg::Type(infcx.next_float_var()),
            CanonicalVarKind::Const(_) => GenericArg::Const(infcx.next_const_var_in_universe(universe)),
            CanonicalVarKind::Region(_) => GenericArg::Lifetime(infcx.next_region_var_in_universe(universe)),
            CanonicalVarKind::PlaceholderTy(p) => GenericArg::Type(tys.intern(TyKind::Placeholder(PlaceholderType { universe, ..p }))),
            CanonicalVarKind::PlaceholderRegion { var, .. } => GenericArg::Lifetime(tys.intern_region(RegionKind::RePlaceholder { universe, var })),
            CanonicalVarKind::PlaceholderConst(p) => GenericArg::Const(tys.intern_const(ConstKind::Placeholder(PlaceholderConst { universe, ..p }))),
        });
    }
    values
}

pub fn instantiate_and_apply_query_response(infcx: &InferCtxt<'_>, prog: &crate::program::Program, original: &[GenericArg], response: &Canonical<Response>) -> (Vec<(GoalSource, Goal)>, Certainty) {
    let values = compute_query_response_instantiation_values(infcx, original, response);
    let response = fold_response(infcx.tcx, &response.value, &mut CanonicalInstantiator { values: &values });
    let mut relation = ResponseRelating { infcx, prog };
    relate_args(&mut relation, original, &response.var_values).expect("invalid canonical response");
    for (constraint, _) in response.external_constraints.region_constraints {
        match constraint {
            RegionConstraint::RegionOutlives(a, b) => infcx.inner.borrow_mut().unwrap_region_constraints().make_subregion(&infcx.tcx.tys, b, a, false),
            RegionConstraint::Eq(a, b) => infcx.inner.borrow_mut().unwrap_region_constraints().make_eqregion(&infcx.tcx.tys, a, b, false),
            RegionConstraint::TypeOutlives(ty, region) => infcx.register_ty_outlives(ty, region),
        }
    }
    for (key, ty) in response.external_constraints.opaque_types {
        let mut inner = infcx.inner.borrow_mut();
        let mut table = inner.opaque_types();
        if let Some(previous) = table.register(key.clone(), ProvisionalHiddenType::new(ty)) {
            table.add_duplicate(key, ProvisionalHiddenType::new(previous));
        }
    }
    (response.external_constraints.normalization_nested_goals, response.certainty)
}

/// eval_ctxt/mod.rs:1701-1712,1827-1902; solve/delegate.rs:386-401.
/// Resolve first, combine leak-check visibility, then drop existential RHS-only vars.
fn filter_irrelevant_region_constraints(infcx: &InferCtxt<'_>, response: &mut Response) {
    use crate::next_solver::RegionConstraint;
    let tys = &infcx.tcx.tys;
    let mut unique = Vec::<(RegionConstraint, bool)>::new();
    for &(constraint, visible) in &response.external_constraints.region_constraints {
        let trivial = match constraint {
            RegionConstraint::Eq(a, b) => a == b,
            RegionConstraint::RegionOutlives(a, b) => a == b,
            RegionConstraint::TypeOutlives(..) => false,
        };
        if trivial { continue; }
        if let Some((_, prior)) = unique.iter_mut().find(|(c, _)| *c == constraint) { *prior |= visible; }
        else { unique.push((constraint, visible)); }
    }
    response.external_constraints.region_constraints = unique;
    struct NonTrivialVars(crate::fxhash::HashSet<u32>);
    impl TypeFolder for NonTrivialVars {
        fn fold_flags(&self) -> Option<crate::types::TypeFlags> { Some(crate::types::TypeFlags::HAS_RE_INFER) }
        fn fold_region(&mut self, tys: &Types, r: RegionId, _: u32) -> RegionId {
            if let RegionKind::ReVar(vid) = tys.region_kind(r) { self.0.insert(vid); } r
        }
    }
    let mut visitor = NonTrivialVars(Default::default());
    let mut remaining = response.clone();
    remaining.external_constraints.region_constraints.clear();
    fold_response(infcx.tcx, &remaining, &mut visitor);
    for &(constraint, _) in &response.external_constraints.region_constraints {
        match constraint {
            RegionConstraint::Eq(a, b) => { visitor.fold_region(tys, a, 0); visitor.fold_region(tys, b, 0); }
            RegionConstraint::RegionOutlives(a, _) => { visitor.fold_region(tys, a, 0); }
            RegionConstraint::TypeOutlives(t, _) => { visitor.fold_ty(tys, t, 0); }
        }
    }
    response.external_constraints.region_constraints.retain(|&(constraint, _)| {
        if let RegionConstraint::RegionOutlives(a, b) = constraint {
            if let RegionKind::ReVar(vid) = tys.region_kind(b) {
                let mut inner = infcx.inner.borrow_mut();
                let mut regions = inner.unwrap_region_constraints();
                if regions.universe(tys, b).can_name(regions.universe(tys, a)) { return visitor.0.contains(&vid); }
            }
        }
        true
    });
}
