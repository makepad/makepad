// ported from N:rustc_type_ir/src/relate.rs @8d1a764
pub mod solver_relating;
pub mod lattice;
mod generalize;
mod higher_ranked;
use crate::consts::{Const, GenericArg};
use crate::program::DefId;
use crate::types::{RegionArgsId, RegionId, TyId, TyKind};
use crate::types::variance::{V, xform};
use super::InferCtxt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeError { Mismatch, CyclicTy(TyId), CyclicConst(Const), Regions }

/// relate.rs:41,210: generalization and inference share structural traversal.
pub trait TypeRelation<'tcx> {
    fn tcx(&self) -> &'tcx crate::tcx::Tcx;
    fn prog(&self) -> &crate::program::Program;
    fn variance(&self) -> V;
    fn set_variance(&mut self, variance: V);
    fn tys(&mut self, a: TyId, b: TyId) -> Result<TyId, TypeError>;
    fn regions(&mut self, a: RegionId, b: RegionId) -> Result<RegionId, TypeError>;
    fn consts(&mut self, a: Const, b: Const) -> Result<Const, TypeError>;
    fn binders(&mut self, a: TyId, b: TyId) -> Result<TyId, TypeError>;
    fn binders_args(&mut self, a: &[GenericArg], b: &[GenericArg]) -> Result<Vec<GenericArg>, TypeError>;
    fn well_formed(&mut self, _ty: TyId) {}
    fn has_unresolved_infer(&self, _ty: TyId) -> bool { false }
}

pub fn relate_with_variance(r: &mut dyn TypeRelation<'_>, variance: V, a: TyId, b: TyId) -> Result<TyId, TypeError> {
    let old = r.variance();
    r.set_variance(xform(old, variance));
    let result = r.tys(a, b);
    r.set_variance(old);
    result
}

fn relate_region_with_variance(r: &mut dyn TypeRelation<'_>, variance: V, a: RegionId, b: RegionId) -> Result<RegionId, TypeError> {
    let old = r.variance();
    r.set_variance(xform(old, variance));
    let result = r.regions(a, b);
    r.set_variance(old);
    result
}

pub fn relate_args(r: &mut dyn TypeRelation<'_>, a: &[GenericArg], b: &[GenericArg]) -> Result<Vec<GenericArg>, TypeError> {
    if a.len() != b.len() { return Err(TypeError::Mismatch); }
    let old = r.variance();
    r.set_variance(xform(old, V::Inv));
    let result = (|| {
        let mut args = Vec::with_capacity(a.len());
        for (&a, &b) in a.iter().zip(b) {
            args.push(match (a, b) {
                (GenericArg::Type(a), GenericArg::Type(b)) => GenericArg::Type(r.tys(a, b)?),
                (GenericArg::Lifetime(a), GenericArg::Lifetime(b)) => GenericArg::Lifetime(r.regions(a, b)?),
                (GenericArg::Const(a), GenericArg::Const(b)) => GenericArg::Const(r.consts(a, b)?),
                _ => return Err(TypeError::Mismatch),
            });
        }
        Ok(args)
    })();
    r.set_variance(old);
    result
}

fn relate_ty_args(r: &mut dyn TypeRelation<'_>, def: DefId, a_ty: TyId, b_ty: TyId, a: &[TyId], b: &[TyId], ar: RegionArgsId, br: RegionArgsId) -> Result<(Vec<TyId>, RegionArgsId), TypeError> {
    if a.len() != b.len() { return Err(TypeError::Mismatch); }
    let tcx = r.tcx();
    let ars = tcx.tys.region_args(ar);
    let brs = tcx.tys.region_args(br);
    if ars.len() != brs.len() { return Err(TypeError::Mismatch); }
    let mut args = Vec::with_capacity(a.len());
    let mut regions = Vec::with_capacity(ars.len());
    let mut bivariant_infer = false;
    if r.variance() == V::Inv {
        for i in 0..a.len() { args.push(r.tys(a[i], b[i])?); }
        for i in 0..ars.len() { regions.push(r.regions(ars[i], brs[i])?); }
    } else {
        let indices = if let Some(adt) = tcx.adts.get(&def) { adt.generic_indices.clone() }
            else { tcx.fn_item_sig(r.prog(), def).expect("variance declaration").generic_indices.clone() };
        let variances = tcx.variances_of(r.prog(), def).to_vec();
        for i in 0..a.len() {
            let variance = variances[indices.types_consts[i] as usize];
            if variance == V::Bi && (r.has_unresolved_infer(a[i]) || r.has_unresolved_infer(b[i])) { bivariant_infer = true; }
            args.push(relate_with_variance(r, variance, a[i], b[i])?);
        }
        for i in 0..ars.len() {
            regions.push(relate_region_with_variance(r, variances[indices.lifetimes[i] as usize], ars[i], brs[i])?);
        }
    }
    if bivariant_infer { r.well_formed(a_ty); r.well_formed(b_ty); }
    Ok((args, tcx.tys.intern_region_args(&regions)))
}

pub fn structurally_relate_tys(r: &mut dyn TypeRelation<'_>, a: TyId, b: TyId) -> Result<TyId, TypeError> {
    let tys = &r.tcx().tys;
    let ak = tys.kind(a).clone();
    let bk = tys.kind(b).clone();
    let k = match (ak, bk) {
        (TyKind::Tuple(a), TyKind::Tuple(b)) if tys.list(a).len() == tys.list(b).len() => {
            let (a, b) = (tys.list(a), tys.list(b));
            let mut ts = Vec::with_capacity(a.len());
            for i in 0..a.len() { ts.push(relate_with_variance(r, V::Co, a[i], b[i])?); }
            TyKind::Tuple(tys.mk_list(&ts))
        }
        (TyKind::Slice(a), TyKind::Slice(b)) => TyKind::Slice(relate_with_variance(r, V::Co, a, b)?),
        (TyKind::Const(a), TyKind::Const(b)) => TyKind::Const(r.consts(a, b)?),
        (TyKind::Array(a, n), TyKind::Array(b, m)) => TyKind::Array(relate_with_variance(r, V::Co, a, b)?, r.consts(n, m)?),
        (TyKind::Ref(am, a, ar), TyKind::Ref(bm, b, br)) if am == bm => {
            let region = relate_region_with_variance(r, V::Co, ar, br)?;
            TyKind::Ref(am, relate_with_variance(r, if am { V::Inv } else { V::Co }, a, b)?, region)
        }
        (TyKind::Ptr(am, a), TyKind::Ptr(bm, b)) if am == bm =>
            TyKind::Ptr(am, relate_with_variance(r, if am { V::Inv } else { V::Co }, a, b)?),
        (TyKind::Adt(ad, aa, ar), TyKind::Adt(bd, ba, br)) if ad == bd => {
            let (args, regions) = relate_ty_args(r, ad, a, b, tys.list(aa), tys.list(ba), ar, br)?;
            TyKind::Adt(ad, tys.mk_list(&args), regions)
        }
        (TyKind::FnDef(ad, aa, ar), TyKind::FnDef(bd, ba, br)) if ad == bd => {
            let (args, regions) = relate_ty_args(r, ad, a, b, tys.list(aa), tys.list(ba), ar, br)?;
            TyKind::FnDef(ad, tys.mk_list(&args), regions)
        }
        (TyKind::Alias(ar, aa), TyKind::Alias(br, ba)) if ar == br && aa.kind == ba.kind => {
            if tys.list(aa.args).len() != tys.list(ba.args).len() { return Err(TypeError::Mismatch); }
            let (aargs, bargs) = (tys.list(aa.args), tys.list(ba.args));
            let mut args = Vec::with_capacity(aargs.len());
            for i in 0..aargs.len() { args.push(relate_with_variance(r, V::Inv, aargs[i], bargs[i])?); }
            let ars = tys.region_args(aa.regions);
            let brs = tys.region_args(ba.regions);
            if ars.len() != brs.len() { return Err(TypeError::Mismatch); }
            let mut regions = Vec::with_capacity(ars.len());
            for i in 0..ars.len() {
                let variance = alias_region_variance(r.tcx(), r.prog(), &aa, i);
                regions.push(relate_region_with_variance(r, variance, ars[i], brs[i])?);
            }
            TyKind::Alias(ar, crate::types::AliasTy { kind: aa.kind, args: tys.mk_list(&args), regions: tys.intern_region_args(&regions) })
        }
        (TyKind::Dyn(ad, aa, ab, ar, al, an, autos), TyKind::Dyn(bd, ba, bb, br, bl, _, bautos)) if ad == bd && autos == bautos && tys.list(ab).len() == tys.list(bb).len() => {
            let mut args = Vec::new();
            let mut other = Vec::new();
            for &ty in tys.list(aa) { args.push(GenericArg::Type(ty)); }
            for &region in &*tys.region_args(ar) { args.push(GenericArg::Lifetime(region)); }
            for &ty in tys.list(ba) { other.push(GenericArg::Type(ty)); }
            for &region in &*tys.region_args(br) { other.push(GenericArg::Lifetime(region)); }
            let related = r.binders_args(&args, &other)?;
            let mut args = Vec::new();
            let mut regions = Vec::new();
            for arg in related {
                match arg { GenericArg::Type(ty) => args.push(ty), GenericArg::Lifetime(region) => regions.push(region), GenericArg::Const(_) => unreachable!() }
            }
            let (ab, bb) = (tys.list(ab), tys.list(bb));
            let mut bindings = Vec::with_capacity(ab.len());
            for i in 0..ab.len() {
                let (ad, a, n) = ab[i];
                let (bd, b, _) = bb[i];
                if ad != bd { return Err(TypeError::Mismatch); }
                let related = r.binders_args(&[GenericArg::Type(a)], &[GenericArg::Type(b)])?;
                let GenericArg::Type(ty) = related[0] else { unreachable!() };
                bindings.push((ad, ty, n));
            }
            let region = r.regions(al, bl)?;
            TyKind::Dyn(ad, tys.mk_list(&args), tys.mk_list(&bindings), tys.intern_region_args(&regions), region, an, autos)
        }
        (TyKind::FnPtr(..), TyKind::FnPtr(..)) => return r.binders(a, b),
        (TyKind::Closure(af, ae, crate::types::ClosureArgs { parent_args: ao, closure_kind_ty: ak, closure_sig_as_fn_ptr_ty: asig, tupled_upvars_ty: au }), TyKind::Closure(bf, be, crate::types::ClosureArgs { parent_args: bo, closure_kind_ty: bk, closure_sig_as_fn_ptr_ty: bsig, tupled_upvars_ty: bu })) if af == bf && ae == be =>
            TyKind::Closure(af, ae, crate::types::ClosureArgs { parent_args: relate_with_variance(r, V::Inv, ao, bo)?, closure_kind_ty: relate_with_variance(r, V::Inv, ak, bk)?, closure_sig_as_fn_ptr_ty: relate_with_variance(r, V::Inv, asig, bsig)?, tupled_upvars_ty: relate_with_variance(r, V::Inv, au, bu)? }),
        (a, b) if a == b => return Ok(tys.intern(a)),
        _ => return Err(TypeError::Mismatch),
    };
    Ok(tys.intern(k))
}

pub fn relate_fn_sig(r: &mut dyn TypeRelation<'_>, a: TyId, b: TyId) -> Result<TyId, TypeError> {
    let tys = &r.tcx().tys;
    let TyKind::FnPtr(ap, ar, ah, an) = tys.kind(a).clone() else { panic!("expected fn binder") };
    let TyKind::FnPtr(bp, br, bh, _) = tys.kind(b).clone() else { panic!("expected fn binder") };
    if tys.list(ap).len() != tys.list(bp).len() || ah != bh { return Err(TypeError::Mismatch); }
    let (ap, bp) = (tys.list(ap), tys.list(bp));
    let mut params = Vec::with_capacity(ap.len());
    for i in 0..ap.len() { params.push(relate_with_variance(r, V::Contra, ap[i], bp[i])?); }
    let ret = relate_with_variance(r, V::Co, ar, br)?;
    Ok(tys.intern(TyKind::FnPtr(tys.mk_list(&params), ret, ah, an)))
}

pub fn structurally_relate_consts(r: &mut dyn TypeRelation<'_>, a: Const, b: Const) -> Result<Const, TypeError> {
    use crate::consts::{ConstKind, ValTreeKind};
    if a == b { return Ok(a); }
    let tys = &r.tcx().tys;
    match (tys.const_kind(a), tys.const_kind(b)) {
        (ConstKind::Infer(_), _) | (_, ConstKind::Infer(_)) => panic!("unresolved const inference"),
        (ConstKind::Error, _) => return Ok(a),
        (_, ConstKind::Error) => return Ok(b),
        (ConstKind::Param(a), ConstKind::Param(b)) if a == b => {},
        (ConstKind::Placeholder(a), ConstKind::Placeholder(b)) if a == b => {},
        (ConstKind::Alias(ar, aa), ConstKind::Alias(br, ba)) if ar == br && aa.kind == ba.kind => {
            relate_args(r, &aa.args, &ba.args)?;
        }
        (ConstKind::Value(a), ConstKind::Value(b)) => match (a.valtree.kind(), b.valtree.kind()) {
            (ValTreeKind::Leaf(a), ValTreeKind::Leaf(b)) if a == b => {},
            (ValTreeKind::Branch(a), ValTreeKind::Branch(b)) if a.len() == b.len() => {
                for i in 0..a.len() { r.consts(a[i], b[i])?; }
            }
            _ => return Err(TypeError::Mismatch),
        },
        _ => return Err(TypeError::Mismatch),
    }
    Ok(a)
}

/// rustc_type_ir/src/relate.rs:202; rustc_middle/src/ty/util.rs:954-974.
/// Opaques and trait RPITIT projections share the captured-lifetime variances.
fn alias_region_variance(tcx: &crate::tcx::Tcx, prog: &crate::program::Program, alias: &crate::types::AliasTy, index: usize) -> V {
    if let Some(variances) = tcx.opt_alias_variances(prog, alias.kind) {
        let indices = tcx.generic_indices_of(prog, alias.kind.def_id());
        return variances[indices.lifetimes[index] as usize];
    }
    V::Inv
}
