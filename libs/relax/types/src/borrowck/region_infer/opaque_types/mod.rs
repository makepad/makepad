// ported from N:rustc_borrowck/src/region_infer/opaque_types/mod.rs @8d1a764
use crate::types::AliasTy;
mod member_constraints;
mod region_ctxt;
use region_ctxt::RegionCtxt;
use super::{NllRegionVariableOrigin, RegionDefinition};
use crate::borrowck::constraints::OutlivesConstraintSet;
use crate::borrowck::type_check::free_region_relations::UniversalRegionRelations;
use crate::borrowck::universal_regions::RegionClassification;
use crate::types::{RegionId, RegionKind, TyId, TyKind};
use crate::tcx::Tcx;
use crate::program::{DefId, Program};

/// Defining-use input after opaque argument validation and NLL renumbering.
/// Relax's inference storage producer is supplied by N0/C2.
pub struct DefiningUse {
    pub def_id: DefId,
    pub hidden_type: TyId,
    pub arg_regions: Vec<usize>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum DeferredOpaqueTypeError {
    UnexpectedHiddenRegion { def_id: DefId, member_region: usize },
    ConflictingHiddenTypes { def_id: DefId },
}
fn nll_var_to_universal_region(rcx: &RegionCtxt<'_>, r: usize) -> Option<RegionId> {
    let vid = rcx.representative(r).rvid();
    match rcx.definitions[vid].origin {
        NllRegionVariableOrigin::FreeRegion => rcx.universal_region_relations.universal_regions.universal_regions_iter()
            .filter(|&ur| rcx.universal_region_relations.universal_regions.region_classification(ur) != Some(RegionClassification::External))
            .find(|&ur| rcx.universal_region_relations.equal(vid,ur))
            .and_then(|ur| rcx.definitions[ur].external_name),
        NllRegionVariableOrigin::Placeholder(p) => Some(p),
        NllRegionVariableOrigin::Existential => None,
    }
}
pub fn compute_definition_site_hidden_types(tcx: &Tcx, prog: &Program, definitions: Vec<RegionDefinition>,
    relations: &UniversalRegionRelations, constraints: OutlivesConstraintSet, defining_uses: &[DefiningUse])
    -> (Vec<(DefId,TyId)>, Vec<DeferredOpaqueTypeError>) {
    let mut rcx = RegionCtxt::new(definitions, relations, constraints);
    member_constraints::apply_member_constraints(&mut rcx, tcx, prog, defining_uses);
    let mut hidden_types = vec![];
    let mut errors = vec![];
    for defining_use in defining_uses {
        let mut folder = ToArgRegionsFolder { tcx, prog, rcx: &rcx, arg_regions: &defining_use.arg_regions, erase_unknown_regions: false };
        let ty = match folder.fold_ty(defining_use.hidden_type) {
            Ok(ty) => ty,
            Err(member_region) => {
                errors.push(DeferredOpaqueTypeError::UnexpectedHiddenRegion { def_id: defining_use.def_id, member_region });
                continue;
            }
        };
        if let Some((_,previous)) = hidden_types.iter().find(|(d,_)| *d == defining_use.def_id) {
            if *previous != ty { errors.push(DeferredOpaqueTypeError::ConflictingHiddenTypes { def_id: defining_use.def_id }); }
        } else { hidden_types.push((defining_use.def_id,ty)); }
    }
    (hidden_types,errors)
}
type AliasVariances = Option<(crate::types::variance::GenericIndices, Vec<crate::types::variance::V>)>;
fn bivariant(variance: &AliasVariances, region: bool, index: usize) -> bool {
    variance.as_ref().is_some_and(|(indices, vs)| {
        let indices = if region { &indices.lifetimes } else { &indices.types_consts };
        indices.get(index).is_some_and(|&i| vs.get(i as usize) == Some(&crate::types::variance::V::Bi))
    })
}
struct ToArgRegionsFolder<'a,'b> {
    tcx: &'a Tcx, prog: &'a Program, rcx: &'a RegionCtxt<'b>, arg_regions: &'a [usize], erase_unknown_regions: bool,
}
impl ToArgRegionsFolder<'_, '_> {
    fn fold_region(&self, r: RegionId) -> Result<RegionId, usize> {
        let vid = match self.tcx.tys.region_kind(r) {
            RegionKind::ReBound(..) => return Ok(r),
            RegionKind::ReVar(v) => v as usize,
            _ => panic!("opaque hidden type was not renumbered"),
        };
        if let Some(region) = self.arg_regions.iter().copied().find(|&a| self.rcx.eval_equal(vid,a))
            .and_then(|a| nll_var_to_universal_region(self.rcx,a)) { Ok(region) }
        else if self.erase_unknown_regions { Ok(RegionId::ERASED) }
        else { Err(vid) }
    }
    fn list(&mut self, ts: &[TyId]) -> Result<Vec<TyId>, usize> {
        ts.iter().map(|&t| self.fold_ty(t)).collect()
    }
    fn args(&mut self, def: DefId, ts: &[TyId], rs: crate::types::RegionArgsId, variance: AliasVariances)
        -> Result<(Vec<TyId>, crate::types::RegionArgsId), usize> {
        let tys = &self.tcx.tys;
        let original_regions = tys.region_args(rs);
        let indices = variance.as_ref().map(|p| p.0.clone())
            .or_else(|| self.tcx.adts.get(&def).map(|d| d.generic_indices.clone()))
            .or_else(|| self.tcx.fn_item_sig(self.prog, def).map(|d| d.generic_indices.clone()))
            .or_else(|| self.tcx.nominal_region_contracts.get(&def).map(|c| {
                let lifetimes: Vec<_> = c.regions.iter().filter_map(|p| match tys.region_kind(p.1) {
                    RegionKind::ReEarlyParam(i) => Some(i), _ => None,
                }).collect();
                let mut full = 0;
                let types_consts = (0..ts.len()).map(|_| {
                    while lifetimes.contains(&full) { full += 1; }
                    let i = full; full += 1; i
                }).collect();
                crate::types::variance::GenericIndices { lifetimes,types_consts,consts:vec![] }
            }));
        let mut order = vec![];
        for i in 0..ts.len() {
            let full = indices.as_ref().and_then(|g| g.types_consts.get(i)).copied().unwrap_or(i as u32);
            order.push((full,false,i));
        }
        for i in 0..original_regions.len() {
            let full = indices.as_ref().and_then(|g| g.lifetimes.get(i)).copied().unwrap_or((ts.len()+i) as u32);
            order.push((full,true,i));
        }
        order.sort_by_key(|p| p.0);
        let mut types = ts.to_vec(); let mut regions = original_regions.to_vec();
        for (_,region,i) in order {
            let previous = self.erase_unknown_regions;
            self.erase_unknown_regions |= bivariant(&variance,region,i);
            let result = if region {
                self.fold_region(regions[i]).map(|r| { regions[i] = r; })
            } else {
                self.fold_ty(types[i]).map(|t| { types[i] = t; })
            };
            self.erase_unknown_regions = previous;
            result?;
        }
        Ok((types,self.tcx.tys.intern_region_args(&regions)))
    }
    // Fallible traversal, including nightly's region-before-referent ordering.
    // Every recursive operation uses ?, so later arguments are never visited
    // after an unknown member region has failed substitution.
    fn fold_ty(&mut self, ty: TyId) -> Result<TyId, usize> {
        let tys = &self.tcx.tys;
        if !tys.flags(ty).intersects(crate::types::TypeFlags::HAS_FREE_REGIONS) { return Ok(ty); }
        let kind = match tys.kind(ty) {
            TyKind::Ref(m,t,r) => { let r = self.fold_region(*r)?; TyKind::Ref(*m,self.fold_ty(*t)?,r) }
            TyKind::Ptr(m,t) => TyKind::Ptr(*m,self.fold_ty(*t)?),
            TyKind::Slice(t) => TyKind::Slice(self.fold_ty(*t)?),
            TyKind::Array(t,n) => {
                let t = self.fold_ty(*t)?;
                let n = self.fold_ty(tys.intern(TyKind::Const(*n)))?;
                return Ok(tys.mk_array(t,tys.expect_const(n)));
            }
            TyKind::Const(ct) => {
                use crate::consts::{ConstKind, GenericArg, Value, ValTree};
                let kind = match tys.const_kind(*ct) {
                    ConstKind::Alias(rigid, mut alias) => {
                        let mut args = Vec::new();
                        for arg in &*alias.args {
                            args.push(match *arg {
                                GenericArg::Lifetime(r) => GenericArg::Lifetime(self.fold_region(r)?),
                                GenericArg::Type(t) => GenericArg::Type(self.fold_ty(t)?),
                                GenericArg::Const(c) => GenericArg::Const(tys.expect_const(self.fold_ty(tys.intern(TyKind::Const(c)))?)),
                            });
                        }
                        alias.args = args.into(); ConstKind::Alias(rigid,alias)
                    }
                    ConstKind::Value(v) => {
                        let ty = self.fold_ty(v.ty)?;
                        let valtree = if let Some(branches) = v.valtree.try_to_branch() {
                            let mut folded = Vec::new();
                            for &c in branches { folded.push(tys.expect_const(self.fold_ty(tys.intern(TyKind::Const(c)))?)); }
                            ValTree::from_branches(tys,&folded)
                        } else { v.valtree };
                        ConstKind::Value(Value { ty, valtree })
                    }
                    _ => return Ok(ty),
                };
                TyKind::Const(tys.intern_const(kind))
            }
            TyKind::Tuple(ts) => TyKind::Tuple(tys.mk_list(&self.list(tys.list(*ts))?)),
            TyKind::Adt(d,ts,rs) | TyKind::FnDef(d,ts,rs) => {
                let (ts,rs) = self.args(*d,tys.list(*ts),*rs,None)?;
                match tys.kind(ty) {
                    TyKind::Adt(..) => TyKind::Adt(*d,tys.mk_list(&ts),rs),
                    TyKind::FnDef(..) => TyKind::FnDef(*d,tys.mk_list(&ts),rs),
                    _ => unreachable!(),
                }
            }
            TyKind::Alias(rigid, alias) => {
                let d = alias.kind.def_id();
                // mod.rs:502-514: opt_alias_variances (opaques and RPITITs).
                let variance = self.tcx.alias_declared_variances(self.prog, alias.kind);
                let (args,regions) = self.args(d,tys.list(alias.args),alias.regions,variance)?;
                TyKind::Alias(*rigid, AliasTy { kind: alias.kind, args: tys.mk_list(&args), regions })
            }
            TyKind::FnPtr(ts,r,h,n) => TyKind::FnPtr(tys.mk_list(&self.list(tys.list(*ts))?),self.fold_ty(*r)?,*h,*n),
            TyKind::Dyn(d,ts,bs,rs,r,n,autos) => {
                let (ts,rs) = self.args(*d,tys.list(*ts),*rs,None)?;
                let bs: Vec<_> = tys.list(*bs).iter().map(|&(d,t,n)| Ok((d,self.fold_ty(t)?,n))).collect::<Result<_,usize>>()?;
                TyKind::Dyn(*d,tys.mk_list(&ts),tys.mk_list(&bs),rs,self.fold_region(*r)?,*n,*autos)
            }
            TyKind::Closure(f, e, crate::types::ClosureArgs { parent_args: o, closure_kind_ty, closure_sig_as_fn_ptr_ty: s, tupled_upvars_ty: u }) => {
                let previous = self.erase_unknown_regions;
                self.erase_unknown_regions = true;
                let owner = self.fold_ty(*o);
                self.erase_unknown_regions = previous;
                let owner = owner?;
                TyKind::Closure(*f, *e, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty: self.fold_ty(*closure_kind_ty)?, closure_sig_as_fn_ptr_ty: self.fold_ty(*s)?, tupled_upvars_ty: self.fold_ty(*u)? })
            }
            _ => return Ok(ty),
        };
        Ok(tys.intern(kind))
    }
}

#[cfg(test)]
pub(super) fn test_fail_fast(tcx: &Tcx, ty: TyId) {
    use crate::types::UniverseIndex;
    use crate::borrowck::universal_regions::UniversalRegions;
    let definitions = (0..3).map(|_| RegionDefinition { origin:NllRegionVariableOrigin::FreeRegion,
        universe:UniverseIndex::ROOT,external_name:None }).collect();
    let relations = UniversalRegionRelations::create(UniversalRegions::new(0,None,vec![
        (0,RegionClassification::Global),(1,RegionClassification::Local),(2,RegionClassification::Local)]),3,&[]);
    let rcx = RegionCtxt::new(definitions,&relations,OutlivesConstraintSet::default());
    let mut folder = ToArgRegionsFolder { tcx, prog:&crate::program::Program::new(&std::sync::Arc::new(crate::Session::default())),rcx:&rcx,arg_regions:&[0,1],erase_unknown_regions:false };
    assert_eq!(folder.fold_ty(ty),Err(2));
}
