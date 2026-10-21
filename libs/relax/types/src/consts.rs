// ported from N:rustc_type_ir/src/const_kind.rs @8d1a764
// ported from N:rustc_middle/src/ty/consts/valtree.rs @8d1a764
// ported from N:rustc_middle/src/ty/consts/int.rs @8d1a764
use crate::program::DefId;
use crate::types::{BoundVar, DebruijnIndex, RegionId, TyId, Types, UniverseIndex};
use std::collections::HashMap;
use std::num::NonZeroU8;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Const(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GenericArg { Lifetime(RegionId), Type(TyId), Const(Const) }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParamConst { pub index: u32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConstVid(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InferConst { Var(ConstVid) }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BoundConst { pub var: BoundVar }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlaceholderConst { pub universe: UniverseIndex, pub bound: BoundConst }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IsRigid { Yes, No }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AliasConstKind {
    Projection { def_id: DefId },
    InherentSelf { def_id: DefId },
    InherentImpl { def_id: DefId },
    Free { def_id: DefId },
    Anon { def_id: DefId },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AliasConst { pub kind: AliasConstKind, pub args: Arc<[GenericArg]> }

impl AliasConstKind {
    pub fn def_id(self) -> DefId {
        match self {
            Self::Projection { def_id } | Self::InherentSelf { def_id } |
            Self::InherentImpl { def_id } | Self::Free { def_id } |
            Self::Anon { def_id } => def_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ConstKind {
    Param(ParamConst),
    Infer(InferConst),
    Bound(DebruijnIndex, BoundConst),
    Placeholder(PlaceholderConst),
    Alias(IsRigid, AliasConst),
    Value(Value),
    Error,
}

/// int.rs:146,232,268: bits above the scalar's byte size are always zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(packed)]
pub struct ScalarInt { data: u128, size: NonZeroU8 }

impl ScalarInt {
    fn raw(data: u128, size: u8) -> Self {
        assert!(size <= 16);
        Self { data, size: NonZeroU8::new(size).expect("zero-sized scalar") }
    }

    pub fn try_from_uint(data: u128, size: u8) -> Option<Self> {
        let (value, overflow) = Self::truncate_from_uint(data, size);
        if overflow { None } else { Some(value) }
    }

    pub fn truncate_from_uint(data: u128, size: u8) -> (Self, bool) {
        assert!(size <= 16);
        let bits = if size == 16 { data } else { data & ((1u128 << (size as u32 * 8)) - 1) };
        let value = Self::raw(bits, size);
        (value, bits != data)
    }

    pub fn size(self) -> u8 { self.size.get() }

    pub fn to_bits(self, size: u8) -> u128 {
        assert_eq!(self.size.get(), size);
        self.data
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ValTreeKind { Leaf(ScalarInt), Branch(Arc<[Const]>) }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ValTree(Arc<ValTreeKind>);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Value { pub ty: TyId, pub valtree: ValTree }

/// Const storage is separate from TyKind: u128 payloads do not enlarge type entries.
/// Branches contain typed constants (const_kind.rs:223), not untyped child valtrees.
#[derive(Default)]
pub(crate) struct ConstInterner {
    kinds: Vec<ConstKind>,
    ids: HashMap<ConstKind, Const>,
    valtrees: HashMap<ValTreeKind, ValTree>,
}

impl Types {
    pub fn intern_const(&self, kind: ConstKind) -> Const {
        let mut table = self.consts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&id) = table.ids.get(&kind) { return id; }
        let id = Const(u32::try_from(table.kinds.len()).expect("too many constants"));
        table.kinds.push(kind.clone());
        table.ids.insert(kind, id);
        id
    }

    pub fn const_kind(&self, ct: Const) -> ConstKind {
        self.consts.lock().unwrap_or_else(|e| e.into_inner()).kinds[ct.0 as usize].clone()
    }

    fn intern_valtree(&self, kind: ValTreeKind) -> ValTree {
        let mut table = self.consts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(value) = table.valtrees.get(&kind) { return value.clone(); }
        let value = ValTree(Arc::new(kind.clone()));
        table.valtrees.insert(kind, value.clone());
        value
    }
}

impl ValTree {
    pub fn kind(&self) -> &ValTreeKind { &self.0 }
    pub fn zst(tys: &Types) -> Self { Self::from_branches(tys, &[]) }
    pub fn from_scalar_int(tys: &Types, scalar: ScalarInt) -> Self {
        tys.intern_valtree(ValTreeKind::Leaf(scalar))
    }
    pub fn from_branches(tys: &Types, branches: &[Const]) -> Self {
        tys.intern_valtree(ValTreeKind::Branch(branches.into()))
    }
    pub fn try_to_leaf(&self) -> Option<ScalarInt> {
        match self.kind() { ValTreeKind::Leaf(scalar) => Some(*scalar), ValTreeKind::Branch(_) => None }
    }
    pub fn try_to_branch(&self) -> Option<&[Const]> {
        match self.kind() { ValTreeKind::Branch(branches) => Some(branches), ValTreeKind::Leaf(_) => None }
    }
}

impl Const {
    /// rustc_type_ir/src/flags.rs: cached inference flags in rustc; walk this
    /// concrete arena until consts participate in Relax's structural type flags.
    pub fn has_non_region_infer(self, tys: &Types) -> bool {
        let mut todo = vec![self];
        while let Some(ct) = todo.pop() {
            match tys.const_kind(ct) {
                ConstKind::Infer(_) => return true,
                ConstKind::Alias(_, alias) => {
                    for arg in &*alias.args {
                        match arg {
                            GenericArg::Type(ty) => { if tys.has_infer(*ty) { return true; } }
                            GenericArg::Const(ct) => todo.push(*ct),
                            GenericArg::Lifetime(_) => {}
                        }
                    }
                }
                ConstKind::Value(value) => {
                    if tys.has_infer(value.ty) { return true; }
                    if let Some(branches) = value.valtree.try_to_branch() { todo.extend_from_slice(branches); }
                }
                _ => {}
            }
        }
        false
    }

    pub fn new_value(tys: &Types, valtree: ValTree, ty: TyId) -> Self {
        tys.intern_const(ConstKind::Value(Value { ty, valtree }))
    }

    pub fn try_to_value(self, tys: &Types) -> Option<Value> {
        match tys.const_kind(self) { ConstKind::Value(value) => Some(value), _ => None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::Prim;
    use crate::types::{RegionKind, TyKind};

    fn scalar(tys: &Types, ty: TyId, bits: u128, size: u8) -> Const {
        Const::new_value(tys, ValTree::from_scalar_int(tys, ScalarInt::try_from_uint(bits, size).unwrap()), ty)
    }

    #[test]
    fn scalar_identity_preserves_type_size_and_all_128_bits() {
        let tys = Types::new();
        let u128_ = tys.intern(TyKind::Int(Prim::U128));
        let zero = scalar(&tys, u128_, 0, 16);
        let high = scalar(&tys, u128_, 1u128 << 127, 16);
        assert_ne!(zero, high);
        assert_eq!(high, scalar(&tys, u128_, 1u128 << 127, 16));
        assert_eq!(high.try_to_value(&tys).unwrap().valtree.try_to_leaf().unwrap().to_bits(16), 1u128 << 127);
        assert_ne!(scalar(&tys, tys.u8_, 1, 1), scalar(&tys, tys.bool_, 1, 1));
        assert!(ScalarInt::try_from_uint(256, 1).is_none());
        assert!(ScalarInt::try_from_uint(u128::MAX, 16).is_some());
        let (byte, overflow) = ScalarInt::truncate_from_uint(257, 1);
        assert!(overflow);
        assert_eq!(byte.to_bits(1), 1);
    }

    #[test]
    fn aggregate_branches_retain_typed_fields() {
        let tys = Types::new();
        let byte = scalar(&tys, tys.u8_, 7, 1);
        let boolean = scalar(&tys, tys.bool_, 1, 1);
        let fields = ValTree::from_branches(&tys, &[byte, boolean]);
        let value = Const::new_value(&tys, fields.clone(), tys.intern(TyKind::Tuple(tys.mk_list(&[tys.u8_, tys.bool_]))));
        assert_eq!(value.try_to_value(&tys).unwrap().valtree, fields);
        assert_eq!(fields.try_to_branch(), Some(&[byte, boolean][..]));
        assert_eq!(boolean.try_to_value(&tys).unwrap().ty, tys.bool_);
        assert_eq!(ValTree::zst(&tys).try_to_branch(), Some(&[][..]));
        assert!(fields.try_to_leaf().is_none());
    }

    #[test]
    fn aliases_include_lifetime_type_and_const_arguments() {
        let tys = Types::new();
        let region = tys.intern_region(RegionKind::ReStatic);
        let n = scalar(&tys, tys.usize_, 3, 8);
        let args: Arc<[GenericArg]> = vec![GenericArg::Lifetime(region), GenericArg::Type(tys.bool_), GenericArg::Const(n)].into();
        let alias = AliasConst { kind: AliasConstKind::Projection { def_id: DefId(1) }, args: args.clone() };
        let a = tys.intern_const(ConstKind::Alias(IsRigid::No, alias.clone()));
        assert_eq!(a, tys.intern_const(ConstKind::Alias(IsRigid::No, alias.clone())));
        assert_ne!(a, tys.intern_const(ConstKind::Alias(IsRigid::Yes, alias)));
        let b = tys.intern_const(ConstKind::Alias(IsRigid::No, AliasConst { kind: AliasConstKind::Anon { def_id: DefId(1) }, args }));
        assert_ne!(a, b);
        let ConstKind::Alias(_, alias) = tys.const_kind(a) else { panic!() };
        assert_eq!(alias.args[0], GenericArg::Lifetime(region));
        assert_eq!(alias.args[2], GenericArg::Const(n));
    }
}

impl Const {
    /// rustc_middle/src/ty/consts.rs: scalar usize constants use the target width.
    pub fn from_target_usize(tys: &Types, value: u64) -> Self {
        Self::new_value(tys, ValTree::from_scalar_int(tys, ScalarInt::try_from_uint(value as u128, tys.ptr_bytes as u8).unwrap()), tys.usize_)
    }

    pub fn try_to_target_usize(self, tys: &Types) -> Option<u64> {
        let value = self.try_to_value(tys)?;
        if value.ty != tys.usize_ { return None; }
        Some(value.valtree.try_to_leaf()?.to_bits(tys.ptr_bytes as u8) as u64)
    }
}

impl Types {
    pub fn const_usize(&self, value: u64) -> Const { Const::from_target_usize(self, value) }

    pub(crate) fn subst_const(&self, ct: Const, args: &[TyId], depth: u32) -> Const {
        struct Subst<'a> { args: &'a [TyId] }
        impl crate::types::fold::TypeFolder for Subst<'_> {
            fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId { tys.subst_at_depth(ty, self.args, depth) }
            fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
                if let ConstKind::Param(param) = tys.const_kind(ct) {
                    if let Some(&arg) = self.args.get(param.index as usize) {
                        let crate::types::TyKind::Const(value) = tys.kind(arg) else { panic!("type argument for const parameter") };
                        return tys.shift_const(*value, depth, false);
                    }
                }
                crate::types::fold::super_fold_const(tys, ct, self, depth)
            }
        }
        use crate::types::fold::TypeFolder;
        Subst { args }.fold_const(self, ct, depth)
    }

    pub(crate) fn replace_const(&self, ct: Const, pairs: &[(TyId, TyId)]) -> Const {
        struct Replace<'a> { pairs: &'a [(TyId, TyId)] }
        impl crate::types::fold::TypeFolder for Replace<'_> {
            fn fold_ty(&mut self, tys: &Types, ty: TyId, _: u32) -> TyId { tys.replace(ty, self.pairs) }
        }
        use crate::types::fold::TypeFolder;
        Replace { pairs }.fold_const(self, ct, 0)
    }

    pub(crate) fn erase_const_regions(&self, ct: Const, all: bool) -> Const {
        struct Erase { all: bool }
        impl crate::types::fold::TypeFolder for Erase {
            fn fold_ty(&mut self, tys: &Types, ty: TyId, _: u32) -> TyId {
                if self.all { tys.erase_all_regions(ty) } else { tys.erase_regions(ty) }
            }
            fn fold_region(&mut self, tys: &Types, region: RegionId, _: u32) -> RegionId {
                if self.all { RegionId::ERASED } else { tys.erase_region(region) }
            }
        }
        use crate::types::fold::TypeFolder;
        Erase { all }.fold_const(self, ct, 0)
    }

    pub fn shift_const(&self, ct: Const, amount: u32, out: bool) -> Const {
        use crate::types::fold::TypeFolder;
        crate::types::fold::Shifter { amount, out }.fold_const(self, ct, 0)
    }
}

impl Types {
    /// Const arguments occupy an interned slot in Relax's compact declaration
    /// argument list; the slot contains the native Const handle, never its bits.
    pub fn expect_const(&self, arg: TyId) -> Const {
        match self.kind(arg) {
            crate::types::TyKind::Const(ct) => *ct,
            _ => panic!("expected a const argument"),
        }
    }

    pub fn const_to_string(&self, ct: Const) -> String {
        match self.const_kind(ct) {
            ConstKind::Value(value) => match value.valtree.kind() {
                ValTreeKind::Leaf(bits) => format!("{}", bits.to_bits(bits.size())),
                ValTreeKind::Branch(_) => "{aggregate}".into(),
            },
            ConstKind::Param(p) => format!("N{}", p.index),
            ConstKind::Infer(InferConst::Var(v)) => format!("?C{}", v.0),
            ConstKind::Bound(d, b) => format!("^{}.C{}", d, b.var.0),
            ConstKind::Placeholder(p) => format!("!{}.C{}", p.universe.0, p.bound.var.0),
            ConstKind::Alias(_, alias) => format!("const#{}", alias.kind.def_id().0),
            ConstKind::Error => "{error}".into(),
        }
    }
}

#[cfg(test)]
mod array_tests {
    use super::*;
    use crate::types::{TyKind, RegionKind, TypeFlags};

    #[test]
    fn native_lengths_keep_usize_max_and_substitute_inside_nominal_args() {
        let tys = Types::new();
        let full = tys.mk_array(tys.unit, tys.const_usize(u64::MAX));
        let TyKind::Array(_, n) = tys.kind(full) else { panic!() };
        assert_eq!(n.try_to_target_usize(&tys), Some(u64::MAX));
        let param = tys.intern_const(ConstKind::Param(ParamConst { index: 0 }));
        let arr = tys.mk_array(tys.u32_, param);
        let nominal = tys.intern(TyKind::Adt(DefId(1), tys.mk_list(&[tys.intern(TyKind::Const(param)), arr]), crate::types::RegionArgsId::EMPTY));
        assert!(tys.flags(nominal).contains(TypeFlags::HAS_TY_PARAM));
        let arg = tys.intern(TyKind::Const(tys.const_usize(7)));
        let instantiated = tys.subst(nominal, &[arg]);
        let TyKind::Adt(_, args, _) = tys.kind(instantiated) else { panic!() };
        let args = tys.list(*args);
        assert_eq!(args[0], arg);
        assert_eq!(args[1], tys.mk_array(tys.u32_, tys.const_usize(7)));
    }

    #[test]
    fn array_const_flags_and_region_folding_follow_alias_arguments() {
        let tys = Types::new();
        let region = tys.intern_region(RegionKind::ReLateParam(crate::types::RegionScope::Item(DefId(1)), crate::types::LateParamKind::Anon(0)));
        let alias = AliasConst { kind: AliasConstKind::Anon { def_id: DefId(2) }, args: vec![GenericArg::Lifetime(region)].into() };
        let ct = tys.intern_const(ConstKind::Alias(IsRigid::No, alias));
        let arr = tys.mk_array(tys.u8_, ct);
        assert!(tys.flags(arr).contains(TypeFlags::HAS_FREE_REGIONS));
        assert!(tys.may_project(arr));
        let erased = tys.erase_regions(arr);
        assert_ne!(arr, erased);
        assert_eq!(tys.map_regions(arr, &|_, _| crate::types::RegionId::ERASED, 0), erased);
        let TyKind::Array(_, n) = tys.kind(erased) else { panic!() };
        let ConstKind::Alias(_, alias) = tys.const_kind(*n) else { panic!() };
        assert_eq!(alias.args[0], GenericArg::Lifetime(crate::types::RegionId::ERASED));
    }
}

// The collected graph retains arena IDs; the disk code cache uses structural keys.
mod collected {
    use super::*;
    use crate::crate_image::{Codec, Decode};
    crate::collected_id!(ConstVid);
    crate::collected_fields!(ParamConst, index);
    crate::collected_fields!(BoundConst, var);
    crate::collected_fields!(PlaceholderConst, universe, bound);
    crate::collected_fields!(AliasConst, kind, args);
    crate::collected_fields!(Value, ty, valtree);
    crate::collected_enum!(GenericArg; 0 => Lifetime(r), 1 => Type(t), 2 => Const(c));
    crate::collected_enum!(InferConst; 0 => Var(v));
    crate::collected_enum!(IsRigid; 0 => Yes, 1 => No);
    crate::collected_enum!(ConstKind; 0 => Param(p), 1 => Infer(v), 2 => Bound(d,b), 3 => Placeholder(p), 4 => Alias(r,a), 5 => Value(v), 6 => Error);
    crate::collected_enum!(ValTreeKind; 0 => Leaf(s), 1 => Branch(v));
    impl Codec for ScalarInt {
        fn put(&self, w: &mut Vec<u8>) { self.size().put(w); self.to_bits(self.size()).put(w); }
        fn get(r: &mut Decode<'_>) -> Option<Self> {
            let size = u8::get(r)?; if size == 0 || size > 16 { return None; }
            Self::try_from_uint(u128::get(r)?, size)
        }
    }
    impl Codec for ValTree {
        fn put(&self, w: &mut Vec<u8>) { self.kind().put(w); }
        fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Self(Arc::new(Codec::get(r)?))) }
    }
    impl Codec for AliasConstKind {
        fn put(&self, w: &mut Vec<u8>) {
            (match self { Self::Projection { .. } => 0u8, Self::InherentSelf { .. } => 1, Self::InherentImpl { .. } => 2, Self::Free { .. } => 3, Self::Anon { .. } => 4 }).put(w);
            self.def_id().put(w);
        }
        fn get(r: &mut Decode<'_>) -> Option<Self> {
            let tag = u8::get(r)?; let def_id = Codec::get(r)?;
            Some(match tag { 0 => Self::Projection { def_id }, 1 => Self::InherentSelf { def_id }, 2 => Self::InherentImpl { def_id }, 3 => Self::Free { def_id }, 4 => Self::Anon { def_id }, _ => return None })
        }
    }
    impl Codec for ConstInterner {
        fn put(&self, w: &mut Vec<u8>) { self.kinds.put(w); }
        fn get(r: &mut Decode<'_>) -> Option<Self> {
            let kinds = Vec::<ConstKind>::get(r)?;
            let mut table = Self::default();
            for (i, kind) in kinds.iter().enumerate() {
                if table.ids.insert(kind.clone(), Const(i as u32)).is_some() { return None; }
                if let ConstKind::Value(v) = kind { table.valtrees.insert(v.valtree.kind().clone(), v.valtree.clone()); }
            }
            table.kinds = kinds;
            Some(table)
        }
    }
}
