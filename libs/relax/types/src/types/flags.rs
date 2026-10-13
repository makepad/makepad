// ported from N:rustc_type_ir/src/flags.rs @8d1a764
//! Cached flags and escaping-binder depth (rustc_type_ir::flags).
use crate::types::{AliasTy};
use super::*;

/// The existing query bits retain their semantics; the new region/binder bits
/// visit every child, including closure owners and type-valued array lengths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeFlags(u32);

impl TypeFlags {
    pub const HAS_TY_INFER: Self = Self(F_INFER);
    pub const HAS_NON_REGION_ERROR: Self = Self(F_ERROR);
    pub const HAS_TY_OPAQUE: Self = Self(F_OPAQUE);
    pub const HAS_TY_PROJECTION: Self = Self(F_PROJ);
    pub const HAS_TY_PARAM: Self = Self(F_PARAM);
    pub const HAS_CT_BOUND: Self = Self(1 << 17);
    pub const HAS_CT_PLACEHOLDER: Self = Self(1 << 18);
    /// rustc_type_ir flags.rs HAS_ALIAS (any type or const alias, rigid or not, at any depth,
    /// closure parent args included): set_aliases_to_non_rigid's needs_change
    pub const HAS_ALIAS: Self = Self(1 << 19);
    /// Unlike the legacy F_INFER, structural inference visits closure parent args too.
    pub const HAS_NON_REGION_INFER: Self = Self(1 << 20);
    pub const HAS_ANY_INFER: Self = Self(Self::HAS_NON_REGION_INFER.0 | Self::HAS_RE_INFER.0);
    /// rustc_next_trait_solver/canonical/canonicalizer.rs: NEEDS_CANONICAL.
    pub const NEEDS_CANONICAL: Self = Self(Self::HAS_ANY_INFER.0 | Self::HAS_PLACEHOLDER.0
        | Self::HAS_PARAM.0 | Self::HAS_FREE_REGIONS.0 | Self::HAS_RE_ERASED.0);
    pub const HAS_TY_BOUND: Self = Self(1 << 8);
    pub const HAS_TY_PLACEHOLDER: Self = Self(1 << 9);
    pub const HAS_RE_BOUND: Self = Self(1 << 10);
    pub const HAS_RE_PLACEHOLDER: Self = Self(1 << 11);
    pub const HAS_RE_PARAM: Self = Self(1 << 12);
    pub const HAS_RE_INFER: Self = Self(1 << 13);
    pub const HAS_RE_ERROR: Self = Self(1 << 14);
    pub const HAS_RE_ERASED: Self = Self(1 << 15);
    pub const HAS_FREE_REGIONS: Self = Self(1 << 16);
    pub const HAS_ERROR: Self = Self(Self::HAS_NON_REGION_ERROR.0 | Self::HAS_RE_ERROR.0);
    pub const HAS_INFER: Self = Self(Self::HAS_TY_INFER.0 | Self::HAS_RE_INFER.0);
    pub const HAS_PARAM: Self = Self(Self::HAS_TY_PARAM.0 | Self::HAS_RE_PARAM.0);
    pub const HAS_PLACEHOLDER: Self = Self(Self::HAS_TY_PLACEHOLDER.0 | Self::HAS_RE_PLACEHOLDER.0 | Self::HAS_CT_PLACEHOLDER.0);
    pub const HAS_BOUND_VARS: Self = Self(Self::HAS_TY_BOUND.0 | Self::HAS_RE_BOUND.0 | Self::HAS_CT_BOUND.0);
    pub const HAS_REGIONS: Self = Self(Self::HAS_FREE_REGIONS.0 | Self::HAS_RE_BOUND.0 | Self::HAS_RE_ERASED.0);

    /// Every bit a type's flags can hold (types.rs packs them below bit 21).
    pub(crate) const ALL_BITS: u32 = F_INFER | F_ERROR | F_OPAQUE | F_PROJ | F_PARAM | F_NONCONC | F_ASSOC | F_STUCK
        | Self::HAS_CT_BOUND.0 | Self::HAS_CT_PLACEHOLDER.0 | Self::HAS_ALIAS.0 | Self::HAS_NON_REGION_INFER.0
        | Self::HAS_TY_BOUND.0 | Self::HAS_TY_PLACEHOLDER.0 | Self::HAS_RE_BOUND.0 | Self::HAS_RE_PLACEHOLDER.0
        | Self::HAS_RE_PARAM.0 | Self::HAS_RE_INFER.0 | Self::HAS_RE_ERROR.0 | Self::HAS_RE_ERASED.0 | Self::HAS_FREE_REGIONS.0;

    pub fn contains(self, other: Self) -> bool { self.0 & other.0 == other.0 }
    pub fn intersects(self, other: Self) -> bool { self.0 & other.0 != 0 }
    pub fn bits(self) -> u32 { self.0 }
}

impl std::ops::BitOr for TypeFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
}

#[derive(Default)]
struct Computation {
    flags: u32,
    outer: DebruijnIndex,
}

impl Computation {
    fn add(&mut self, flags: u32, outer: DebruijnIndex, binders: u32) {
        self.flags |= flags;
        self.outer = self.outer.max(outer.saturating_sub(binders));
    }

    fn ty(&mut self, tys: &Types, ty: TyId, binders: u32) {
        self.add(tys.fl(ty) & !0xff, tys.outer_exclusive_binder(ty), binders);
    }

    fn region(&mut self, tys: &Types, r: RegionId, binders: u32) {
        use TypeFlags as F;
        let (flags, outer) = match tys.region_kind(r) {
            RegionKind::ReBound(d, _) => (F::HAS_RE_BOUND, d.checked_add(1).expect("bound region index overflow")),
            RegionKind::RePlaceholder { .. } => (F::HAS_RE_PLACEHOLDER | F::HAS_FREE_REGIONS, 0),
            RegionKind::ReEarlyParam(_) => (F::HAS_RE_PARAM | F::HAS_FREE_REGIONS, 0),
            RegionKind::ReVar(_) => (F::HAS_RE_INFER | F::HAS_FREE_REGIONS, 0),
            RegionKind::ReError => (F::HAS_RE_ERROR | F::HAS_FREE_REGIONS, 0),
            RegionKind::ReErased => (F::HAS_RE_ERASED, 0),
            RegionKind::ReStatic | RegionKind::ReLateParam(..) => (F::HAS_FREE_REGIONS, 0),
        };
        self.add(flags.0, outer, binders);
    }

    fn ct(&mut self, tys: &Types, ct: Const, binders: u32) {
        use crate::consts::{ConstKind, GenericArg};
        use TypeFlags as F;
        match tys.const_kind(ct) {
            ConstKind::Param(_) => self.flags |= F_PARAM | F_NONCONC,
            ConstKind::Infer(_) => self.flags |= F_INFER | F_NONCONC | F::HAS_NON_REGION_INFER.0,
            ConstKind::Error => self.flags |= F_ERROR | F_NONCONC,
            ConstKind::Bound(d, _) => self.add(F::HAS_CT_BOUND.0 | F_NONCONC, d + 1, binders),
            ConstKind::Placeholder(_) => self.flags |= F::HAS_CT_PLACEHOLDER.0 | F_NONCONC,
            ConstKind::Alias(_, alias) => {
                self.flags |= F_PROJ | F::HAS_ALIAS.0;
                for arg in &*alias.args {
                    match *arg {
                        GenericArg::Type(t) => { self.flags |= tys.fl(t); self.ty(tys, t, binders); }
                        GenericArg::Const(ct) => self.ct(tys, ct, binders),
                        GenericArg::Lifetime(r) => self.region(tys, r, binders),
                    }
                }
            }
            ConstKind::Value(value) => {
                self.flags |= tys.fl(value.ty);
                self.ty(tys, value.ty, binders);
                if let Some(branches) = value.valtree.try_to_branch() {
                    for &ct in branches { self.ct(tys, ct, binders); }
                }
            }
        }
    }

    fn regions(&mut self, tys: &Types, rs: RegionArgsId, binders: u32) {
        if rs != RegionArgsId::EMPTY {
            for r in tys.region_args(rs).iter() { self.region(tys, *r, binders); }
        }
    }
}

impl Types {
    pub(crate) fn region_flags(&self, region: RegionId) -> u32 {
        let mut c = Computation::default();
        c.region(self, region, 0);
        c.flags
    }

    pub(crate) fn const_flags(&self, ct: Const) -> (u32, DebruijnIndex) {
        let mut c = Computation::default();
        c.ct(self, ct, 0);
        (c.flags, c.outer)
    }

    pub(crate) fn const_depth(&self, ct: Const) -> u16 {
        use crate::consts::{ConstKind, GenericArg};
        let mut depth = 0;
        match self.const_kind(ct) {
            ConstKind::Alias(_, alias) => for arg in &*alias.args {
                depth = depth.max(match *arg {
                    GenericArg::Type(ty) => self.depth(ty) as u16,
                    GenericArg::Const(ct) => self.const_depth(ct),
                    GenericArg::Lifetime(_) => 0,
                });
            },
            ConstKind::Value(value) => {
                depth = self.depth(value.ty) as u16;
                if let Some(branches) = value.valtree.try_to_branch() {
                    for &ct in branches { depth = depth.max(self.const_depth(ct)); }
                }
            }
            _ => {}
        }
        depth.saturating_add(1)
    }

    pub fn flags(&self, t: TyId) -> TypeFlags { TypeFlags(self.fl(t)) }

    /// One past the greatest escaping de Bruijn index, or zero when all bound
    /// variables are captured internally. Even an empty FnPtr/dyn binder counts.
    pub fn outer_exclusive_binder(&self, t: TyId) -> DebruijnIndex {
        let b = self.entry(t).meta >> META_BINDER_SHIFT;
        if b < BINDER_SAT { b } else { self.binder_flags_of(self.kind(t)).1 }
    }

    pub fn has_vars_bound_at_or_above(&self, t: TyId, depth: DebruijnIndex) -> bool {
        self.outer_exclusive_binder(t) > depth
    }

    pub fn has_escaping_bound_vars(&self, t: TyId) -> bool {
        self.has_vars_bound_at_or_above(t, 0)
    }

    pub fn has_placeholders(&self, t: TyId) -> bool {
        self.flags(t).intersects(TypeFlags::HAS_PLACEHOLDER)
    }

    pub(super) fn binder_flags_of(&self, k: &TyKind) -> (u32, DebruijnIndex) {
        let mut c = Computation::default();
        match k {
            TyKind::Bound(d, _) => c.add(TypeFlags::HAS_TY_BOUND.0, d.checked_add(1).expect("bound type index overflow"), 0),
            TyKind::Placeholder(_) => c.flags |= TypeFlags::HAS_TY_PLACEHOLDER.0,
            TyKind::Tuple(v) => for t in self.list(*v) { c.ty(self, *t, 0); },
            TyKind::Array(t, n) => {
                c.ty(self, *t, 0);
                c.ct(self, *n, 0);
            }
            TyKind::Slice(t) | TyKind::Ptr(_, t) => c.ty(self, *t, 0),
            TyKind::Ref(_, t, r) => { c.ty(self, *t, 0); c.region(self, *r, 0); }
            TyKind::Adt(_, v, rs) | TyKind::FnDef(_, v, rs) | TyKind::Alias(_, AliasTy { args: v, regions: rs, .. }) => {
                if matches!(k, TyKind::Alias(..)) { c.flags |= TypeFlags::HAS_ALIAS.0; }
                for t in self.list(*v) { c.ty(self, *t, 0); }
                c.regions(self, *rs, 0);
            }
            TyKind::FnPtr(ps, r, _, _) => {
                for t in self.list(*ps) { c.ty(self, *t, 1); }
                c.ty(self, *r, 1);
            }
            TyKind::Dyn(_, v, bs, rs, r, _, _) => {
                for t in self.list(*v) { c.ty(self, *t, 1); }
                for (_, t, _) in self.list(*bs) { c.ty(self, *t, 1); }
                c.regions(self, *rs, 1);
                c.region(self, *r, 0);
            }
            TyKind::Closure(_, _, crate::types::ClosureArgs { parent_args: o, closure_kind_ty, closure_sig_as_fn_ptr_ty: s, tupled_upvars_ty: u }) => for t in [s, u, o, closure_kind_ty] { c.ty(self, *t, 0); },
            TyKind::Const(ct) => c.ct(self, *ct, 0),
            TyKind::Bool | TyKind::Char | TyKind::Str | TyKind::Int(_) | TyKind::Float(_) |
            TyKind::Foreign(_) | TyKind::Never | TyKind::Param(_) | TyKind::Error => {}
            TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) => c.flags |= TypeFlags::HAS_NON_REGION_INFER.0,
        }
        (c.flags, c.outer)
    }
}
