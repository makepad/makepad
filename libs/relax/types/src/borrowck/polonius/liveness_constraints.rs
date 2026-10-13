// ported from N:rustc_borrowck/src/polonius/liveness_constraints.rs @8d1a764
use super::{ConstraintDirection as D, LiveRegionVariances};
use crate::borrowck::universal_regions::UniversalRegions;
use crate::program::Program;
use crate::tcx::Tcx;
use crate::types::{RegionId, RegionKind, TyId, TyKind, variance::{V, xform}};

pub(crate) fn region_vid(tcx: &Tcx, universals: &UniversalRegions, r: RegionId) -> Option<usize> {
    match tcx.tys.region_kind(r) {
        RegionKind::ReBound(..) | RegionKind::ReErased => None,
        RegionKind::ReVar(v) => Some(v as usize),
        RegionKind::ReStatic => Some(universals.fr_static),
        _ => panic!("Polonius requires renumbered MIR regions"),
    }
}
pub(crate) fn regions_in(tcx: &Tcx, universals: &UniversalRegions, ty: TyId) -> Vec<usize> {
    let regions = std::cell::RefCell::new(Vec::new());
    tcx.tys.map_regions(ty, &|r, _| {
        if let Some(vid) = region_vid(tcx, universals, r) {
            if !regions.borrow().contains(&vid) { regions.borrow_mut().push(vid); }
        }
        r
    }, 0);
    regions.into_inner()
}
pub fn record_live_region_variance(tcx: &Tcx, prog: &Program, directions: &mut LiveRegionVariances,
    universals: &UniversalRegions, ty: TyId) {
    record_variance(tcx, prog, directions, universals, ty, V::Co);
}
pub(crate) fn record_variance(tcx: &Tcx, prog: &Program, directions: &mut LiveRegionVariances,
    universals: &UniversalRegions, ty: TyId, variance: V) {
    VarianceExtractor { tcx, prog, directions, universals }.ty(ty, variance);
}
pub(crate) fn record_region_variance(directions: &mut LiveRegionVariances, region: usize, variance: V) {
        let direction = match variance { V::Co => D::Forward, V::Contra => D::Backward, V::Inv => D::Bidirectional, V::Bi => return };
        directions.resize(directions.len().max(region + 1), None);
        let entry = &mut directions[region];
        *entry = Some(match *entry { Some(old) if old != direction => D::Bidirectional, _ => direction });
}
struct VarianceExtractor<'a> { tcx: &'a Tcx, prog: &'a Program, directions: &'a mut LiveRegionVariances, universals: &'a UniversalRegions }
impl VarianceExtractor<'_> {
    fn region(&mut self, r: RegionId, variance: V) {
        let Some(region) = region_vid(self.tcx, self.universals, r) else { return; };
        record_region_variance(self.directions, region, variance);
    }

    // rustc_type_ir/src/relate.rs: AliasConst arguments are invariant; value
    // valtrees recurse through their typed constants.
    fn constant(&mut self, ct: crate::consts::Const, v: V) {
        use crate::consts::{ConstKind, GenericArg, ValTreeKind};
        match self.tcx.tys.const_kind(ct) {
            ConstKind::Alias(_, a) => for arg in a.args.iter() { match *arg {
                GenericArg::Type(t) => self.ty(t, xform(v, V::Inv)),
                GenericArg::Lifetime(r) => self.region(r, xform(v, V::Inv)),
                GenericArg::Const(c) => self.constant(c, xform(v, V::Inv)),
            } },
            ConstKind::Value(value) => if let ValTreeKind::Branch(cs) = value.valtree.kind() {
                for &c in cs.iter() { self.constant(c, v); }
            },
            ConstKind::Param(_) | ConstKind::Infer(_) | ConstKind::Bound(..) | ConstKind::Placeholder(_) | ConstKind::Error => {}
        }
    }

    fn ty(&mut self, ty: TyId, v: V) {
        match self.tcx.tys.kind(ty).clone() {
            TyKind::Ref(m, t, r) => { self.region(r, v); self.ty(t, xform(v, if m { V::Inv } else { V::Co })); }
            TyKind::Ptr(m, t) => self.ty(t, xform(v, if m { V::Inv } else { V::Co })),
            TyKind::Slice(t) => self.ty(t, v),
            TyKind::Array(t, n) => { self.ty(t, v); self.constant(n, xform(v, V::Inv)); }
            TyKind::Tuple(ts) => { for t in self.tcx.tys.list(ts).iter().copied() { self.ty(t, v); } }
            TyKind::FnPtr(ps, ret, _, _) => { for t in self.tcx.tys.list(ps).iter().copied() { self.ty(t, xform(v, V::Contra)); } self.ty(ret, v); }
            TyKind::Adt(d, ts, rs) | TyKind::FnDef(d, ts, rs) => {
                let indices = self.tcx.adts.get(&d).map(|a| &a.generic_indices)
                    .or_else(|| self.tcx.fn_item_sig(self.prog, d).map(|s| &s.generic_indices));
                let variances = self.tcx.variances_of(self.prog, d);
                for (i, t) in self.tcx.tys.list(ts).iter().copied().enumerate() {
                    let arg = indices.and_then(|ix| ix.types_consts.get(i)).and_then(|&i| variances.get(i as usize)).copied().unwrap_or(V::Inv);
                    self.ty(t, xform(v, arg));
                }
                for (i, &r) in self.tcx.tys.region_args(rs).iter().enumerate() {
                    let arg = indices.and_then(|ix| ix.lifetimes.get(i)).and_then(|&i| variances.get(i as usize)).copied().unwrap_or(V::Inv);
                    self.region(r, xform(v, arg));
                }
            }
            // Relate<AliasTy> (rustc_type_ir relate.rs:224): opaques and RPITIT projections use
            // their declared variances (opt_alias_variances, rustc_middle/src/ty/util.rs:954-974),
            // so uncaptured (bivariant) lifetimes record no direction; other aliases are invariant.
            TyKind::Alias(_, crate::types::AliasTy { kind, args: ts, regions: rs }) => {
                let variances = self.tcx.alias_declared_variances(self.prog, kind);
                let arg = |region: bool, k: usize| variances.as_ref().and_then(|(indices, vs)| {
                    let indices = if region { &indices.lifetimes } else { &indices.types_consts };
                    indices.get(k).and_then(|&j| vs.get(j as usize)).copied()
                }).unwrap_or(V::Inv);
                for (k, t) in self.tcx.tys.list(ts).iter().copied().enumerate() { self.ty(t, xform(v, arg(false, k))); }
                for (k, &r) in self.tcx.tys.region_args(rs).iter().enumerate() { self.region(r, xform(v, arg(true, k))); }
            }
            TyKind::Dyn(_, ts, bindings, rs, r, _, _) => {
                self.region(r, v);
                for t in self.tcx.tys.list(ts).iter().copied() { self.ty(t, xform(v, V::Inv)); }
                for &r in self.tcx.tys.region_args(rs).iter() { self.region(r, xform(v, V::Inv)); }
                for (_, t, _) in self.tcx.tys.list(bindings).iter().copied() { self.ty(t, xform(v, V::Inv)); }
            }
            TyKind::Closure(_, _, args) => {
                for t in [args.parent_args, args.closure_kind_ty, args.closure_sig_as_fn_ptr_ty, args.tupled_upvars_ty] { self.ty(t, xform(v, V::Inv)); }
            }
            TyKind::Const(c) => self.constant(c, v),
            TyKind::Bool | TyKind::Char | TyKind::Str | TyKind::Int(_) | TyKind::Float(_)
            | TyKind::Never | TyKind::Foreign(_) | TyKind::Error | TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_)
            | TyKind::Bound(..) | TyKind::Placeholder(_) | TyKind::Param(_) => {}
        }
    }
}
