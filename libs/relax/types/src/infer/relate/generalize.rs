// ported from N:rustc_infer/src/infer/relate/generalize.rs @8d1a764
use super::*;
use crate::consts::{ConstKind, InferConst, IsRigid};
use crate::infer::type_variable::TypeVariableValue;
use crate::infer::unify_key::ConstVariableValue;
use crate::types::{RegionKind, UniverseIndex};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TermVid { Ty(u32), Const(u32) }

struct Generalizer<'a, 'tcx> {
    infcx: &'a InferCtxt<'tcx>,
    prog: &'a crate::program::Program,
    root_vid: TermVid,
    for_universe: UniverseIndex,
    ambient_variance: V,
    in_alias: bool,
    cache: HashMap<(TyId, V, bool), TyId>,
}

impl InferCtxt<'_> {
    pub(crate) fn instantiate_ty_var(&self, relation: &mut dyn TypeRelation<'_>, target_is_expected: bool, target_vid: u32, variance: V, source: TyId) -> Result<(), TypeError> {
        let generalized = self.generalize_ty_var(relation.prog(), target_vid, variance, source)?;
        if target_is_expected { relation.tys(generalized, source)?; } else { relation.tys(source, generalized)?; }
        Ok(())
    }
    /// generalize.rs:124-147: generalize, then union_var_term, without relating
    /// the source. Borrowck relates it with its location-aware region relation.
    pub fn generalize_ty_var(&self, prog: &crate::program::Program, target_vid: u32, variance: V, source: TyId) -> Result<TyId, TypeError> {
        let (universe, root) = {
            let mut inner = self.inner.borrow_mut();
            let mut table = inner.type_variables();
            let TypeVariableValue::Unknown { universe } = table.probe(target_vid) else { panic!("resolved target") };
            (universe, table.root_var(target_vid))
        };
        let mut generalizer = Generalizer { infcx: self, prog: prog, root_vid: TermVid::Ty(root), for_universe: universe, ambient_variance: variance, in_alias: false, cache: HashMap::new() };
        let generalized = generalizer.tys(source, source)?;
        if let TyKind::Infer(vid) = self.tcx.tys.kind(generalized) {
            self.inner.borrow_mut().type_variables().equate(target_vid, *vid);
        } else {
            self.inner.borrow_mut().type_variables().instantiate(target_vid, generalized);
        }
        Ok(generalized)
    }
    pub(crate) fn instantiate_const_var(&self, relation: &mut dyn TypeRelation<'_>, target_is_expected: bool, target_vid: u32, source: Const) -> Result<(), TypeError> {
        let (universe, root) = {
            let mut inner = self.inner.borrow_mut();
            let mut table = inner.const_unification_table();
            let ConstVariableValue::Unknown { universe } = table.probe_value(target_vid) else { panic!("resolved target") };
            (universe, table.find(target_vid))
        };
        let mut generalizer = Generalizer { infcx: self, prog: relation.prog(), root_vid: TermVid::Const(root), for_universe: universe, ambient_variance: V::Inv, in_alias: false, cache: HashMap::new() };
        let generalized = generalizer.consts(source, source)?;
        if let ConstKind::Infer(InferConst::Var(vid)) = self.tcx.tys.const_kind(generalized) {
            self.inner.borrow_mut().const_unification_table().union(target_vid, vid.0);
        } else {
            self.inner.borrow_mut().const_unification_table().union_value(target_vid, ConstVariableValue::Known { value: generalized });
        }
        if target_is_expected { relation.consts(generalized, source)?; } else { relation.consts(source, generalized)?; }
        Ok(())
    }
}

impl Generalizer<'_, '_> {
    fn generalize_alias_ty(&mut self, ty: TyId) -> Result<TyId, TypeError> {
        let tys = &self.infcx.tcx.tys;
        if !tys.has_vars_bound_at_or_above(ty, 0) && !self.in_alias {
            return Ok(self.infcx.next_ty_var_in_universe(self.for_universe));
        }
        let nested = std::mem::replace(&mut self.in_alias, true);
        let result = structurally_relate_tys(self, ty, ty);
        self.in_alias = nested;
        match result {
            Err(_) if !nested => Ok(self.infcx.next_ty_var_in_universe(self.for_universe)),
            result => result,
        }
    }
}

impl<'tcx> TypeRelation<'tcx> for Generalizer<'_, 'tcx> {
    fn tcx(&self) -> &'tcx crate::tcx::Tcx { self.infcx.tcx }
    fn prog(&self) -> &crate::program::Program { self.prog }
    fn variance(&self) -> V { self.ambient_variance }
    fn set_variance(&mut self, v: V) { self.ambient_variance = v; }
    fn tys(&mut self, ty: TyId, other: TyId) -> Result<TyId, TypeError> {
        assert_eq!(ty, other);
        let infcx = self.infcx;
        let key = (ty, self.ambient_variance, self.in_alias);
        if let Some(&ty) = self.cache.get(&key) { return Ok(ty); }
        let value = match infcx.tcx.tys.kind(ty) {
            TyKind::Infer(vid) => {
                let mut inner = infcx.inner.borrow_mut();
                let mut table = inner.type_variables();
                let root = table.root_var(*vid);
                if self.root_vid == TermVid::Ty(root) { return Err(TypeError::CyclicTy(ty)); }
                match table.probe(root) {
                    TypeVariableValue::Known { value } => { drop(inner); self.tys(value, value)? }
                    TypeVariableValue::Unknown { universe } => {
                        if self.ambient_variance == V::Inv && self.for_universe.can_name(universe) { return Ok(ty); }
                        let fresh = table.new_var(self.for_universe);
                        table.sub_unify(root, fresh);
                        if !infcx.typing_mode.get().is_coherence() && self.in_alias { table.equate(root, fresh); }
                        infcx.tcx.tys.intern(TyKind::Infer(fresh))
                    }
                }
            }
            TyKind::Placeholder(p) if !self.for_universe.can_name(p.universe) => return Err(TypeError::Mismatch),
            TyKind::Alias(crate::consts::IsRigid::No, _) => self.generalize_alias_ty(ty)?,
            _ => structurally_relate_tys(self, ty, ty)?,
        };
        self.cache.insert(key, value);
        Ok(value)
    }
    fn regions(&mut self, region: RegionId, other: RegionId) -> Result<RegionId, TypeError> {
        assert_eq!(region, other);
        let tys = &self.infcx.tcx.tys;
        if matches!(tys.region_kind(region), RegionKind::ReBound(..) | RegionKind::ReErased | RegionKind::ReError) { return Ok(region); }
        let universe = self.infcx.inner.borrow_mut().unwrap_region_constraints().universe(tys, region);
        if self.ambient_variance == V::Inv && self.for_universe.can_name(universe) { return Ok(region); }
        Ok(self.infcx.next_region_var_in_universe(self.for_universe))
    }
    fn consts(&mut self, ct: Const, other: Const) -> Result<Const, TypeError> {
        assert_eq!(ct, other);
        let infcx = self.infcx;
        let tys = &infcx.tcx.tys;
        let kind = match tys.const_kind(ct) {
            ConstKind::Infer(InferConst::Var(vid)) => {
                let mut inner = infcx.inner.borrow_mut();
                let mut table = inner.const_unification_table();
                let root = table.find(vid.0);
                if self.root_vid == TermVid::Const(root) { return Err(TypeError::CyclicConst(ct)); }
                return match table.probe_value(root) {
                    ConstVariableValue::Known { value } => { drop(inner); self.consts(value, value) }
                    ConstVariableValue::Unknown { universe } => {
                        if self.for_universe.can_name(universe) { return Ok(ct); }
                        let fresh = table.new_key(ConstVariableValue::Unknown { universe: self.for_universe });
                        if !infcx.typing_mode.get().is_coherence() && self.in_alias { table.union(root, fresh); }
                        Ok(tys.intern_const(ConstKind::Infer(InferConst::Var(crate::consts::ConstVid(fresh)))))
                    }
                };
            }
            ConstKind::Placeholder(p) if !self.for_universe.can_name(p.universe) => return Err(TypeError::Mismatch),
            ConstKind::Alias(rigid, mut alias) => {
                let escaping = alias_has_escaping_bound_vars(tys, &alias.args);
                if rigid == IsRigid::No && !escaping && !self.in_alias { return Ok(infcx.next_const_var_in_universe(self.for_universe)); }
                let nested = self.in_alias;
                if rigid == IsRigid::No { self.in_alias = true; }
                let args = relate_args(self, &alias.args, &alias.args);
                self.in_alias = nested;
                alias.args = match args {
                    Ok(args) => args.into(),
                    Err(_) if rigid == IsRigid::No && !nested => return Ok(infcx.next_const_var_in_universe(self.for_universe)),
                    Err(e) => return Err(e),
                };
                ConstKind::Alias(rigid, alias)
            }
            ConstKind::Value(value) => {
                if let Some(branches) = value.valtree.try_to_branch() {
                    for &branch in branches { self.consts(branch, branch)?; }
                }
                return Ok(ct);
            }
            kind => kind,
        };
        Ok(tys.intern_const(kind))
    }
    fn binders_args(&mut self, a: &[GenericArg], b: &[GenericArg]) -> Result<Vec<GenericArg>, TypeError> { relate_args(self, a, b) }
    fn binders(&mut self, a: TyId, b: TyId) -> Result<TyId, TypeError> { super::relate_fn_sig(self, a, b) }
}

pub(super) fn alias_has_escaping_bound_vars(tys: &crate::types::Types, args: &[GenericArg]) -> bool {
    for arg in args {
        match arg {
            GenericArg::Type(ty) => { if tys.has_vars_bound_at_or_above(*ty, 0) { return true; } }
            GenericArg::Lifetime(region) => { if matches!(tys.region_kind(*region), RegionKind::ReBound(..)) { return true; } }
            GenericArg::Const(ct) => match tys.const_kind(*ct) {
                ConstKind::Bound(..) => return true,
                ConstKind::Alias(_, alias) => { if alias_has_escaping_bound_vars(tys, &alias.args) { return true; } }
                ConstKind::Value(value) => { if tys.has_vars_bound_at_or_above(value.ty, 0) { return true; } }
                _ => {},
            },
        }
    }
    false
}
