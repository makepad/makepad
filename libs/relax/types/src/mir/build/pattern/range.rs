//! rustc_middle/src/thir.rs:915-1125: `PatRange` and `PatRangeBoundary` queries over valtrees.
use crate::consts::{ValTree, Value};
use crate::mir::build::thir::{PatRange, PatRangeBoundary, RangeEnd};
use crate::types::{TyId, TyKind, Types};
use std::cmp::Ordering;

/// Size in bytes and signedness of a scalar type of a pattern constant.
pub(crate) fn scalar_size(tys: &Types, ty: TyId) -> Option<(u8, bool)> {
    Some(match *tys.kind(ty) {
        TyKind::Bool => (1, false),
        TyKind::Char => (4, false),
        TyKind::Int(p) => {
            let (bytes, signed) = Types::int_info(p);
            (bytes as u8, signed)
        }
        TyKind::Float(p) => (if p == crate::program::Prim::F32 { 4 } else { 8 }, false),
        _ => return None,
    })
}

pub(crate) fn leaf_bits(tree: &ValTree) -> Option<u128> {
    let leaf = tree.try_to_leaf()?;
    Some(leaf.to_bits(leaf.size()))
}

fn sign_extend(bits: u128, size: u8) -> i128 {
    let shift = 128 - 8 * size as u32;
    ((bits << shift) as i128) >> shift
}

/// rustc `Ty::numeric_min_and_max_as_bits`.
fn numeric_min_and_max_as_bits(tys: &Types, ty: TyId) -> Option<(u128, u128)> {
    let (size, signed) = scalar_size(tys, ty)?;
    let bits = 8 * size as u32;
    let mask = if bits == 128 { u128::MAX } else { (1u128 << bits) - 1 };
    Some(match *tys.kind(ty) {
        TyKind::Int(_) if signed => (1u128 << (bits - 1), (1u128 << (bits - 1)) - 1),
        TyKind::Int(_) => (0, mask),
        TyKind::Char => (0, char::MAX as u128),
        TyKind::Float(_) if size == 4 => (f32::NEG_INFINITY.to_bits() as u128, f32::INFINITY.to_bits() as u128),
        TyKind::Float(_) => (f64::NEG_INFINITY.to_bits() as u128, f64::INFINITY.to_bits() as u128),
        _ => return None,
    })
}

impl PatRangeBoundary {
    pub fn is_finite(&self) -> bool {
        matches!(self, Self::Finite(..))
    }
    pub fn as_finite(&self) -> Option<&ValTree> {
        match self {
            Self::Finite(value) => Some(value),
            Self::NegInfinity | Self::PosInfinity => None,
        }
    }
    pub fn to_bits(&self, ty: TyId, tys: &Types) -> Option<u128> {
        match self {
            Self::Finite(value) => leaf_bits(value),
            Self::NegInfinity => Some(numeric_min_and_max_as_bits(tys, ty)?.0),
            Self::PosInfinity => Some(numeric_min_and_max_as_bits(tys, ty)?.1),
        }
    }
    /// thir.rs:1059.
    pub fn compare_with(&self, other: &Self, ty: TyId, tys: &Types) -> Option<Ordering> {
        use PatRangeBoundary::*;
        match (self, other) {
            (PosInfinity, PosInfinity) => return Some(Ordering::Equal),
            (NegInfinity, NegInfinity) => return Some(Ordering::Equal),
            _ => {}
        }
        let a = self.to_bits(ty, tys)?;
        let b = other.to_bits(ty, tys)?;
        let (size, signed) = scalar_size(tys, ty)?;
        match *tys.kind(ty) {
            TyKind::Float(_) if size == 4 => f32::from_bits(a as u32).partial_cmp(&f32::from_bits(b as u32)),
            TyKind::Float(_) => f64::from_bits(a as u64).partial_cmp(&f64::from_bits(b as u64)),
            TyKind::Int(_) if signed => Some(sign_extend(a, size).cmp(&sign_extend(b, size))),
            TyKind::Int(_) | TyKind::Char => Some(a.cmp(&b)),
            _ => None,
        }
    }
}

impl PatRange {
    /// thir.rs:918: whether this range covers the full extent of possible values (best effort,
    /// floats ignored).
    pub fn is_full_range(&self, tys: &Types) -> Option<bool> {
        let (size, signed) = match *tys.kind(self.ty) {
            TyKind::Char => (4u8, false),
            TyKind::Int(_) => scalar_size(tys, self.ty)?,
            _ => return None,
        };
        let bits = 8 * size as u32;
        let (min, max, bias): (u128, u128, u128) = match *tys.kind(self.ty) {
            TyKind::Char => (0, char::MAX as u128, 0),
            _ if signed => (0, if bits == 128 { u128::MAX } else { (1u128 << bits) - 1 }, 1u128 << (bits - 1)),
            _ => (0, if bits == 128 { u128::MAX } else { (1u128 << bits) - 1 }, 0),
        };
        let lo_is_min = match &self.lo {
            PatRangeBoundary::NegInfinity => true,
            PatRangeBoundary::Finite(value) => (leaf_bits(value)? ^ bias) <= min,
            PatRangeBoundary::PosInfinity => false,
        };
        if lo_is_min {
            let hi_is_max = match &self.hi {
                PatRangeBoundary::NegInfinity => false,
                PatRangeBoundary::Finite(value) => {
                    let hi = leaf_bits(value)? ^ bias;
                    hi > max || hi == max && self.end == RangeEnd::Included
                }
                PatRangeBoundary::PosInfinity => true,
            };
            if hi_is_max {
                return Some(true);
            }
        }
        Some(false)
    }

    /// thir.rs:967.
    pub fn contains(&self, value: &Value, tys: &Types) -> Option<bool> {
        use Ordering::*;
        debug_assert_eq!(value.ty, self.ty);
        let ty = self.ty;
        let value = PatRangeBoundary::Finite(value.valtree.clone());
        Some(
            match self.lo.compare_with(&value, ty, tys)? {
                Less | Equal => true,
                Greater => false,
            } && match value.compare_with(&self.hi, ty, tys)? {
                Less => true,
                Equal => self.end == RangeEnd::Included,
                Greater => false,
            },
        )
    }

    /// thir.rs:986.
    pub fn overlaps(&self, other: &Self, tys: &Types) -> Option<bool> {
        use Ordering::*;
        debug_assert_eq!(self.ty, other.ty);
        Some(
            match other.lo.compare_with(&self.hi, self.ty, tys)? {
                Less => true,
                Equal => self.end == RangeEnd::Included,
                Greater => false,
            } && match self.lo.compare_with(&other.hi, self.ty, tys)? {
                Less => true,
                Equal => other.end == RangeEnd::Included,
                Greater => false,
            },
        )
    }
}
