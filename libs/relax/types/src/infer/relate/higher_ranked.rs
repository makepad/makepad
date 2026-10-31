// ported from N:rustc_infer/src/infer/relate/higher_ranked.rs @8d1a764
// ported from N:rustc_infer/src/infer/mod.rs @8d1a764
use super::*;
use crate::types::{PlaceholderType, RegionKind, UniverseIndex};
use crate::types::fold::TypeFolder;
use crate::infer::snapshot::CombinedSnapshot;

impl InferCtxt<'_> {
    pub(crate) fn fn_has_bound_vars(&self, ty: TyId) -> bool {
        let TyKind::FnPtr(params, ret, _, _) = self.tcx.tys.kind(ty) else { panic!("expected fn binder") };
        for &ty in self.tcx.tys.list(*params) { if self.tcx.tys.has_vars_bound_at_or_above(ty, 0) { return true; } }
        self.tcx.tys.has_vars_bound_at_or_above(*ret, 0)
    }
    /// higher_ranked.rs:24: only escaping variables allocate a new universe.
    pub fn enter_forall_and_leak_universe(&self, ty: TyId) -> TyId {
        if !self.fn_has_bound_vars(ty) { return ty; }
        let universe = self.create_next_universe();
        self.instantiate_fn_binder(ty, Some(universe))
    }
    pub fn instantiate_binder_with_infer(&self, ty: TyId) -> TyId { self.instantiate_fn_binder(ty, None) }
    fn instantiate_fn_binder(&self, ty: TyId, universe: Option<UniverseIndex>) -> TyId {
        let tys = &self.tcx.tys;
        let TyKind::FnPtr(params, ret, header, _) = tys.kind(ty) else { panic!("expected fn binder") };
        let mut replacer = BinderInstantiator { infcx: self, universe, values: Default::default() };
        let mut inputs = Vec::with_capacity(tys.list(*params).len());
        for &ty in tys.list(*params) { inputs.push(replacer.fold_ty(tys, ty, 0)); }
        let output = replacer.fold_ty(tys, *ret, 0);
        tys.intern(TyKind::FnPtr(tys.mk_list(&inputs), output, *header, 0))
    }
    pub fn leak_check(&self, outer_universe: UniverseIndex, snapshot: Option<&CombinedSnapshot>) -> Result<(), TypeError> {
        if self.skip_leak_check { return Ok(()); }
        self.inner.borrow_mut().unwrap_region_constraints().leak_check(&self.tcx.tys, outer_universe, self.universe(), snapshot).map_err(|_| TypeError::Regions)
    }
}

struct BinderInstantiator<'a, 'tcx> {
    infcx: &'a InferCtxt<'tcx>,
    universe: Option<UniverseIndex>,
    values: std::collections::HashMap<crate::consts::GenericArg, crate::consts::GenericArg>,
}
impl crate::types::fold::TypeFolder for BinderInstantiator<'_, '_> {
    fn fold_const(&mut self, tys: &crate::types::Types, ct: Const, depth: u32) -> Const {
        use crate::consts::{GenericArg, ConstKind, PlaceholderConst};
        match tys.const_kind(ct) {
            ConstKind::Bound(d, bound) if d == depth => {
                let key = GenericArg::Const(tys.intern_const(ConstKind::Bound(0, bound)));
                let result = *self.values.entry(key).or_insert_with(|| GenericArg::Const(match self.universe {
                    Some(universe) => tys.intern_const(ConstKind::Placeholder(PlaceholderConst { universe, bound })),
                    None => self.infcx.next_const_var(),
                }));
                let GenericArg::Const(ct) = result else { unreachable!() };
                ct
            }
            ConstKind::Bound(d, bound) if d > depth => tys.intern_const(ConstKind::Bound(d - 1, bound)),
            _ => crate::types::fold::super_fold_const(tys, ct, self, depth),
        }
    }

    fn fold_ty(&mut self, tys: &crate::types::Types, ty: TyId, depth: u32) -> TyId {
        use crate::consts::GenericArg;
        match *tys.kind(ty) {
            TyKind::Bound(d, b) if d == depth => {
                let key = GenericArg::Type(tys.intern(TyKind::Bound(0, b)));
                let result = *self.values.entry(key).or_insert_with(|| GenericArg::Type(match self.universe {
                    Some(universe) => tys.intern(TyKind::Placeholder(PlaceholderType { universe, bound: b.var })),
                    None => self.infcx.next_ty_var(),
                }));
                let GenericArg::Type(ty) = result else { unreachable!() };
                ty
            }
            TyKind::Bound(d, b) if d > depth => tys.intern(TyKind::Bound(d - 1, b)),
            _ => crate::types::fold::super_fold_ty(tys, ty, self, depth),
        }
    }
    fn fold_region(&mut self, tys: &crate::types::Types, region: RegionId, depth: u32) -> RegionId {
        use crate::consts::GenericArg;
        match tys.region_kind(region) {
            RegionKind::ReBound(d, var) if d == depth => {
                let key = GenericArg::Lifetime(tys.intern_region(RegionKind::ReBound(0, var)));
                let result = *self.values.entry(key).or_insert_with(|| GenericArg::Lifetime(match self.universe {
                    Some(universe) => tys.intern_region(RegionKind::RePlaceholder { universe, var }),
                    None => self.infcx.next_region_var(),
                }));
                let GenericArg::Lifetime(region) = result else { unreachable!() };
                region
            }
            RegionKind::ReBound(d, var) if d > depth => tys.intern_region(RegionKind::ReBound(d - 1, var)),
            _ => region,
        }
    }
}


impl InferCtxt<'_> {
    pub fn instantiate_predicate_binder_with_infer(&self, predicate: &crate::next_solver::Predicate) -> crate::next_solver::PredicateKind {
        let mut f = BinderInstantiator { infcx: self, universe: None, values: Default::default() };
        crate::next_solver::fold::fold_predicate_kind(&self.tcx.tys, &predicate.kind, &mut f, 0)
    }
    pub fn enter_forall_predicate_and_leak_universe(&self, predicate: &crate::next_solver::Predicate) -> crate::next_solver::PredicateKind {
        let universe = if !predicate.has_bound_vars(&self.tcx.tys) { None } else { Some(self.create_next_universe()) };
        let mut f = BinderInstantiator { infcx: self, universe, values: Default::default() };
        crate::next_solver::fold::fold_predicate_kind(&self.tcx.tys, &predicate.kind, &mut f, 0)
    }
    pub fn instantiate_binder_args_with_infer(&self, args: &[GenericArg]) -> Vec<GenericArg> {
        self.instantiate_binder_args(args, None)
    }
    pub fn enter_forall_args_and_leak_universe(&self, args: &[GenericArg]) -> Vec<GenericArg> {
        if !super::generalize::alias_has_escaping_bound_vars(&self.tcx.tys, args) { return args.to_vec(); }
        self.instantiate_binder_args(args, Some(self.create_next_universe()))
    }
    fn instantiate_binder_args(&self, args: &[GenericArg], universe: Option<UniverseIndex>) -> Vec<GenericArg> {
        let mut f = BinderInstantiator { infcx: self, universe, values: Default::default() };
        let mut result = Vec::with_capacity(args.len());
        for &arg in args { result.push(crate::next_solver::fold::fold_arg(&self.tcx.tys, arg, &mut f, 0)); }
        result
    }
}
