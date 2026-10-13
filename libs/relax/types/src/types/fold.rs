// ported from N:rustc_type_ir/src/fold.rs @8d1a764
// ported from N:rustc_type_ir/src/binder.rs @8d1a764
//! Binder-aware type folding, shifting, and instantiation. FnPtr and each dyn
//! predicate introduce a binder even when their bound-region count is zero.
use crate::types::{AliasTy};
use super::*;
use crate::consts::{Const, ConstKind, GenericArg, Value, ValTree};
use std::collections::HashMap;

/// rustc_type_ir/src/binder.rs:345-349,650-668: declaration values lose
/// context-local rigidness before substitution into a consumer environment.
pub struct EarlyBinder { value: TyId }
impl EarlyBinder {
    pub fn bind(tys: &Types, value: TyId) -> Self {
        Self { value: tys.set_aliases_to_non_rigid(value) }
    }
    pub fn instantiate(self, tys: &Types, indices: &crate::types::variance::GenericIndices, args: &[GenericArg]) -> TyId {
        if args.is_empty() { assert!(!tys.flags(self.value).contains(TypeFlags::HAS_TY_PARAM)); return self.value; }
        crate::infer::fresh_args::ArgFolder { indices, args }.fold_ty(tys, self.value, 0)
    }
}

/// A structural folder. `depth` counts binders entered since the root value.
/// Overridden type callbacks can recurse with `super_fold_ty`.
pub trait TypeFolder {
    /// Flags this folder needs to visit; None means it must visit every environment.
    fn fold_flags(&self) -> Option<TypeFlags> { None }

    fn fold_const(&mut self, tys: &Types, ct: crate::consts::Const, depth: DebruijnIndex) -> crate::consts::Const {
        super_fold_const(tys, ct, self, depth)
    }
    fn fold_ty(&mut self, tys: &Types, t: TyId, depth: DebruijnIndex) -> TyId {
        super_fold_ty(tys, t, self, depth)
    }
    /// Consts share TyId storage; callers identify them by argument position.
    /// rustc_type_ir/src/fold.rs:144 (TypeFolder::fold_const).
    fn fold_const_arg(&mut self, tys: &Types, t: TyId, depth: DebruijnIndex) -> TyId {
        self.fold_ty(tys, t, depth)
    }
    fn fold_args(&mut self, tys: &Types, _def: DefId, args: &[TyId], _offset: u32, depth: DebruijnIndex) -> Vec<TyId> {
        let mut result = Vec::with_capacity(args.len());
        for t in args { result.push(self.fold_ty(tys, *t, depth)); }
        result
    }
    fn fold_region(&mut self, _tys: &Types, r: RegionId, _depth: DebruijnIndex) -> RegionId { r }
}

pub fn super_fold_ty<F: TypeFolder + ?Sized>(tys: &Types, t: TyId, f: &mut F, depth: DebruijnIndex) -> TyId {
    let mut list = |v: &TyList, depth| tys.mk_list(&tys.list(*v).iter().map(|t| f.fold_ty(tys, *t, depth)).collect::<Vec<_>>());
    let k = match tys.kind(t) {
        TyKind::Tuple(v) => TyKind::Tuple(list(v, depth)),
        TyKind::Const(ct) => TyKind::Const(f.fold_const(tys, *ct, depth)),
        TyKind::Array(t, n) => TyKind::Array(f.fold_ty(tys, *t, depth), f.fold_const(tys, *n, depth)),
        TyKind::Slice(t) => TyKind::Slice(f.fold_ty(tys, *t, depth)),
        TyKind::Ptr(m, t) => TyKind::Ptr(*m, f.fold_ty(tys, *t, depth)),
        TyKind::Ref(m, t, r) => TyKind::Ref(*m, f.fold_ty(tys, *t, depth), f.fold_region(tys, *r, depth)),
        TyKind::Adt(d, v, rs) => TyKind::Adt(*d, tys.mk_list(&f.fold_args(tys, *d, tys.list(*v), 0, depth)), fold_regions(tys, *rs, f, depth)),
        TyKind::FnDef(d, v, rs) => TyKind::FnDef(*d, tys.mk_list(&f.fold_args(tys, *d, tys.list(*v), 0, depth)), fold_regions(tys, *rs, f, depth)),
        TyKind::Alias(rigid, alias) => TyKind::Alias(*rigid, AliasTy { kind: alias.kind,
            args: tys.mk_list(&f.fold_args(tys, alias.kind.def_id(), tys.list(alias.args), 0, depth)), regions: fold_regions(tys, alias.regions, f, depth) }),
        TyKind::FnPtr(ps, r, c, n) => TyKind::FnPtr(list(ps, depth + 1), f.fold_ty(tys, *r, depth + 1), *c, *n),
        TyKind::Dyn(d, v, bs, rs, r, n, autos) => {
            let v = tys.mk_list(&f.fold_args(tys, *d, tys.list(*v), 1, depth + 1));
            let bs: Vec<_> = tys.list(*bs).iter().map(|(d, t, n)| (*d, f.fold_ty(tys, *t, depth + 1), *n)).collect();
            TyKind::Dyn(*d, v, tys.mk_list(&bs), fold_regions(tys, *rs, f, depth + 1), f.fold_region(tys, *r, depth), *n, *autos)
        }
        TyKind::Closure(fi, e, crate::types::ClosureArgs { parent_args: o, closure_kind_ty, closure_sig_as_fn_ptr_ty: s, tupled_upvars_ty: u }) => TyKind::Closure(*fi, *e, crate::types::ClosureArgs { parent_args: f.fold_ty(tys, *o, depth), closure_kind_ty: f.fold_ty(tys, *closure_kind_ty, depth), closure_sig_as_fn_ptr_ty: f.fold_ty(tys, *s, depth), tupled_upvars_ty: f.fold_ty(tys, *u, depth) }),
        TyKind::Bool | TyKind::Char | TyKind::Str | TyKind::Int(_) | TyKind::Float(_) |
        TyKind::Never | TyKind::Param(_) | TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) |
        TyKind::Foreign(_) | TyKind::Error | TyKind::Bound(..) | TyKind::Placeholder(_) => return t,
    };
    tys.intern(k)
}

fn fold_regions<F: TypeFolder + ?Sized>(tys: &Types, rs: RegionArgsId, f: &mut F, depth: DebruijnIndex) -> RegionArgsId {
    if rs == RegionArgsId::EMPTY { return rs; }
    tys.intern_region_args(&tys.region_args(rs).iter().map(|r| f.fold_region(tys, *r, depth)).collect::<Vec<_>>())
}

pub(crate) struct Shifter {
    pub(crate) amount: u32,
    pub(crate) out: bool,
}

impl Shifter {
    fn index(&self, d: DebruijnIndex, depth: DebruijnIndex) -> DebruijnIndex {
        if self.out {
            d.checked_sub(self.amount).filter(|d| *d >= depth)
                .expect("cannot shift a bound variable out through its binder")
        } else {
            d.checked_add(self.amount).expect("bound variable index overflow")
        }
    }
}

impl TypeFolder for Shifter {
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
        match tys.const_kind(ct) {
            ConstKind::Bound(d, b) if d >= depth => tys.intern_const(ConstKind::Bound(self.index(d, depth), b)),
            _ => super_fold_const(tys, ct, self, depth),
        }
    }
    fn fold_ty(&mut self, tys: &Types, t: TyId, depth: DebruijnIndex) -> TyId {
        if !tys.has_vars_bound_at_or_above(t, depth) { return t; }
        match tys.kind(t) {
            TyKind::Bound(d, b) if *d >= depth => tys.intern(TyKind::Bound(self.index(*d, depth), *b)),
            _ => super_fold_ty(tys, t, self, depth),
        }
    }
    fn fold_region(&mut self, tys: &Types, r: RegionId, depth: DebruijnIndex) -> RegionId {
        match tys.region_kind(r) {
            RegionKind::ReBound(d, v) if d >= depth => tys.intern_region(RegionKind::ReBound(self.index(d, depth), v)),
            _ => r,
        }
    }
}

/// Replaces the variables of one removed binder. Callbacks return values in the
/// scope *outside* that binder; escaping variables in them are shifted under any
/// nested binders. Each distinct variable is passed to its callback once.
pub struct BoundVarReplacer<R, T> {
    regions: R,
    types: T,
    region_cache: HashMap<BoundVar, RegionId>,
    type_cache: HashMap<BoundTy, TyId>,
    folded: HashMap<(DebruijnIndex, TyId), TyId>,
}

impl<R, T> BoundVarReplacer<R, T>
where R: FnMut(BoundVar) -> RegionId, T: FnMut(BoundTy) -> TyId {
    pub fn new(regions: R, types: T) -> Self {
        Self { regions, types, region_cache: HashMap::new(), type_cache: HashMap::new(), folded: HashMap::new() }
    }

    /// `value` is the binder's payload (the binder itself has been skipped).
    /// For FnPtr, pass its inputs/output individually using this same replacer,
    /// or pack them in a tuple. Passing a FnPtr visits its binder as a nested one.
    /// A replacer belongs to one binder and one Types interner.
    pub fn instantiate(&mut self, tys: &Types, value: TyId) -> TyId {
        self.fold_ty(tys, value, 0)
    }
}

impl<R, T> TypeFolder for BoundVarReplacer<R, T>
where R: FnMut(BoundVar) -> RegionId, T: FnMut(BoundTy) -> TyId {
    fn fold_ty(&mut self, tys: &Types, t: TyId, depth: DebruijnIndex) -> TyId {
        if !tys.has_vars_bound_at_or_above(t, depth) { return t; }
        match tys.kind(t) {
            TyKind::Bound(d, b) if *d == depth => {
                let replacement = *self.type_cache.entry(*b).or_insert_with(|| (self.types)(*b));
                tys.shift_in(replacement, depth)
            }
            TyKind::Bound(d, b) if *d > depth => tys.intern(TyKind::Bound(d - 1, *b)),
            _ => {
                if let Some(t) = self.folded.get(&(depth, t)) { return *t; }
                let result = super_fold_ty(tys, t, self, depth);
                self.folded.insert((depth, t), result);
                result
            }
        }
    }
    fn fold_region(&mut self, tys: &Types, r: RegionId, depth: DebruijnIndex) -> RegionId {
        match tys.region_kind(r) {
            RegionKind::ReBound(d, v) if d == depth => {
                let replacement = *self.region_cache.entry(v).or_insert_with(|| (self.regions)(v));
                match tys.region_kind(replacement) {
                    RegionKind::ReBound(d, v) => tys.intern_region(RegionKind::ReBound(
                        d.checked_add(depth).expect("bound region index overflow"), v)),
                    _ => replacement,
                }
            }
            RegionKind::ReBound(d, v) if d > depth => tys.intern_region(RegionKind::ReBound(d - 1, v)),
            _ => r,
        }
    }
}

impl Types {
    pub fn fold_with(&self, t: TyId, folder: &mut impl TypeFolder) -> TyId { folder.fold_ty(self, t, 0) }

    /// Introduce binders outside `t`, shifting only its escaping type/region vars.
    pub fn shift_in(&self, t: TyId, amount: u32) -> TyId {
        if amount == 0 { t } else { self.fold_with(t, &mut Shifter { amount, out: false }) }
    }

    /// rustc's `shift_vars` operation.
    pub fn shift_vars(&self, t: TyId, amount: u32) -> TyId { self.shift_in(t, amount) }

    /// Remove empty binder levels outside `t`. Panics if a variable refers to
    /// a removed level; use instantiation when those levels bind variables.
    pub fn shift_out(&self, t: TyId, amount: u32) -> TyId {
        if amount == 0 { t } else { self.fold_with(t, &mut Shifter { amount, out: true }) }
    }

    /// Instantiate one skipped binder's payload; nested binders remain intact.
    pub fn instantiate_bound_vars(&self, value: TyId, regions: impl FnMut(BoundVar) -> RegionId,
        types: impl FnMut(BoundTy) -> TyId) -> TyId {
        BoundVarReplacer::new(regions, types).instantiate(self, value)
    }

    /// Region-only instantiation, like rustc's instantiate_bound_regions.
    /// Panics if the removed binder also binds a type variable.
    pub fn instantiate_bound_regions(&self, value: TyId, regions: impl FnMut(BoundVar) -> RegionId) -> TyId {
        self.instantiate_bound_vars(value, regions, |b| panic!("unexpected bound type in region-only instantiation: {b:?}"))
    }

    /// Instantiate one skipped binder with universally quantified type and
    /// region placeholders. The caller allocates/passes the fresh universe.
    pub fn instantiate_binder_with_placeholders(&self, value: TyId, universe: UniverseIndex) -> TyId {
        self.instantiate_bound_vars(value,
            |var| self.intern_region(RegionKind::RePlaceholder { universe, var }),
            |b| self.intern(TyKind::Placeholder(PlaceholderType { universe, bound: b.var })))
    }
}

pub fn super_fold_const<F: TypeFolder + ?Sized>(tys: &Types, ct: Const, f: &mut F, depth: DebruijnIndex) -> Const {
    let kind = match tys.const_kind(ct) {
        ConstKind::Alias(rigid, mut alias) => {
            let mut args = Vec::with_capacity(alias.args.len());
            for arg in &*alias.args {
                args.push(match *arg {
                    GenericArg::Type(ty) => GenericArg::Type(f.fold_ty(tys, ty, depth)),
                    GenericArg::Lifetime(region) => GenericArg::Lifetime(f.fold_region(tys, region, depth)),
                    GenericArg::Const(ct) => GenericArg::Const(f.fold_const(tys, ct, depth)),
                });
            }
            alias.args = args.into();
            ConstKind::Alias(rigid, alias)
        }
        ConstKind::Value(value) => {
            let ty = f.fold_ty(tys, value.ty, depth);
            let valtree = if let Some(branches) = value.valtree.try_to_branch() {
                let mut result = Vec::with_capacity(branches.len());
                for &ct in branches { result.push(f.fold_const(tys, ct, depth)); }
                ValTree::from_branches(tys, &result)
            } else { value.valtree };
            ConstKind::Value(Value { ty, valtree })
        }
        _ => return ct,
    };
    tys.intern_const(kind)
}

// ported from N:rustc_type_ir/src/fold.rs:594-722 @8d1a764
impl Types {
    pub fn set_aliases_to_non_rigid(&self, ty: TyId) -> TyId {
        struct RigidnessFolder;
        impl TypeFolder for RigidnessFolder {
            fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
                // fold.rs:643-656 RigidnessFoldMode::needs_change: nothing to change in a type
                // without aliases (nightly's has_rigid_aliases; HAS_ALIAS records any alias, a
                // superset, so the answer is the same)
                if !tys.flags(ty).intersects(crate::types::TypeFlags::HAS_ALIAS) { return ty; }
                let ty = super_fold_ty(tys, ty, self, depth);
                match tys.kind(ty) { TyKind::Alias(_, alias) => tys.intern(TyKind::Alias(crate::consts::IsRigid::No, alias.clone())), _ => ty }
            }
            fn fold_const(&mut self, tys: &Types, ct: crate::consts::Const, depth: u32) -> crate::consts::Const {
                let ct = super_fold_const(tys, ct, self, depth);
                match tys.const_kind(ct) { crate::consts::ConstKind::Alias(_, alias) => tys.intern_const(crate::consts::ConstKind::Alias(crate::consts::IsRigid::No, alias)), _ => ct }
            }
        }
        RigidnessFolder.fold_ty(self, ty, 0)
    }
}
