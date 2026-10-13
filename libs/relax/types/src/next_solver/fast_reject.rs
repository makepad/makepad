// ported from N:rustc_type_ir/src/fast_reject.rs @8d1a764
use crate::consts::{Const, ConstKind, GenericArg};
use crate::program::{DefId, Prim, NO_DEF};
use crate::types::{TyId, TyKind, Types};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SimplifiedType {
    Bool, Char, Int(Prim), Float(Prim), Adt(DefId), Foreign(DefId), Str, Array, Slice,
    Ref(bool), Ptr(bool), Never, Tuple(usize), MarkerTraitObject, Trait(DefId),
    Closure(u32, u32), Function(usize), Placeholder, Error,
}
/// fast_reject.rs:98-149, TreatParams::InstantiateWithInfer for impl indexing.
pub fn simplify_type(tys: &Types, ty: TyId) -> Option<SimplifiedType> {
    Some(match tys.kind(ty) {
        TyKind::Bool => SimplifiedType::Bool, TyKind::Char => SimplifiedType::Char,
        TyKind::Int(p) => SimplifiedType::Int(*p), TyKind::Float(p) => SimplifiedType::Float(*p),
        TyKind::Adt(d, ..) => SimplifiedType::Adt(*d), TyKind::Foreign(d) => SimplifiedType::Foreign(*d),
        TyKind::Str => SimplifiedType::Str, TyKind::Array(..) => SimplifiedType::Array,
        TyKind::Slice(..) => SimplifiedType::Slice, TyKind::Ref(m, ..) => SimplifiedType::Ref(*m),
        TyKind::Ptr(m, _) => SimplifiedType::Ptr(*m), TyKind::Never => SimplifiedType::Never,
        TyKind::Tuple(ts) => SimplifiedType::Tuple(tys.list(*ts).len()),
        TyKind::Dyn(d, ..) => if *d == NO_DEF { SimplifiedType::MarkerTraitObject } else { SimplifiedType::Trait(*d) },
        TyKind::FnDef(d, ..) => SimplifiedType::Closure(u32::MAX, d.0),
        TyKind::Closure(f, e, _) => SimplifiedType::Closure(*f, *e),
        TyKind::FnPtr(ts, ..) => SimplifiedType::Function(tys.list(*ts).len()),
        TyKind::Placeholder(_) => SimplifiedType::Placeholder, TyKind::Error => SimplifiedType::Error,
        TyKind::Param(_) | TyKind::Alias(..) | TyKind::Infer(_) | TyKind::IntVar(_) |
        TyKind::FloatVar(_) | TyKind::Bound(..) => return None,
        TyKind::Const(_) => panic!("const argument used as an impl self type"),
    })
}

/// fast_reject.rs:190-566, concrete relate_infer_infer specialization.
#[derive(Clone, Copy)]
pub struct DeepRejectCtxt<'a> { tys: &'a Types, instantiate_lhs: bool }
impl<'a> DeepRejectCtxt<'a> {
    pub fn relate_infer_infer(tys: &'a Types) -> Self { Self { tys, instantiate_lhs: true } }
    /// Goal parameters are rigid; impl parameters will be instantiated by the probe.
    pub fn relate_rigid_infer(tys: &'a Types) -> Self { Self { tys, instantiate_lhs: false } }
    pub fn args_may_unify(self, a: &[GenericArg], b: &[GenericArg]) -> bool { self.args_may_unify_inner(a, b, 8) }
    pub fn types_may_unify(self, a: TyId, b: TyId) -> bool { self.types_may_unify_inner(a, b, 8) }
    fn args_may_unify_inner(self, a: &[GenericArg], b: &[GenericArg], depth: usize) -> bool {
        for (&a, &b) in a.iter().zip(b) {
            let ok = match (a,b) {
                (GenericArg::Lifetime(_), GenericArg::Lifetime(_)) => true,
                (GenericArg::Type(a), GenericArg::Type(b)) => self.types_may_unify_inner(a,b,depth),
                (GenericArg::Const(a), GenericArg::Const(b)) => self.consts_may_unify_inner(a,b),
                _ => panic!("generic argument kind mismatch"),
            };
            if !ok { return false; }
        }
        true
    }
    fn compact_args_may_unify(self, a: &[TyId], b: &[TyId], depth: usize) -> bool {
        for (&a, &b) in a.iter().zip(b) { if !self.types_may_unify_inner(a,b,depth) { return false; } }
        true
    }
    fn types_may_unify_inner(self, a: TyId, b: TyId, depth: usize) -> bool {
        if a == b { return true; }
        match self.tys.kind(b) {
            TyKind::Param(_) | TyKind::Alias(..) | TyKind::Error | TyKind::Bound(..) => return true,
            TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) => return self.var_and_ty_may_unify(b,a),
            _ => {}
        }
        let Some(depth) = depth.checked_sub(1) else { return true; };
        match (self.tys.kind(a), self.tys.kind(b)) {
            (TyKind::Param(a), TyKind::Param(b)) => self.instantiate_lhs || a == b,
            (TyKind::Param(_), _) => self.instantiate_lhs,
            (TyKind::Alias(..) | TyKind::Error | TyKind::Bound(..), _) => true,
            (TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_), _) => self.var_and_ty_may_unify(a,b),
            (TyKind::Ref(am,a,_), TyKind::Ref(bm,b,_)) | (TyKind::Ptr(am,a), TyKind::Ptr(bm,b)) => am==bm && self.types_may_unify_inner(*a,*b,depth),
            (TyKind::Adt(ad,a,_), TyKind::Adt(bd,b,_)) | (TyKind::FnDef(ad,a,_), TyKind::FnDef(bd,b,_)) => ad==bd && self.compact_args_may_unify(self.tys.list(*a),self.tys.list(*b),depth),
            (TyKind::Tuple(a), TyKind::Tuple(b)) => self.tys.list(*a).len()==self.tys.list(*b).len() && self.compact_args_may_unify(self.tys.list(*a),self.tys.list(*b),depth),
            (TyKind::Array(a,ac), TyKind::Array(b,bc)) => self.types_may_unify_inner(*a,*b,depth) && self.consts_may_unify_inner(*ac,*bc),
            (TyKind::Slice(a), TyKind::Slice(b)) => self.types_may_unify_inner(*a,*b,depth),
            (TyKind::Dyn(a,..), TyKind::Dyn(b,..)) => a==b,
            (TyKind::FnPtr(a,ar,ah,_), TyKind::FnPtr(b,br,bh,_)) => ah==bh && self.tys.list(*a).len()==self.tys.list(*b).len() && self.compact_args_may_unify(self.tys.list(*a),self.tys.list(*b),depth) && self.types_may_unify_inner(*ar,*br,depth),
            (TyKind::Closure(af,ae,a), TyKind::Closure(bf,be,b)) => af==bf && ae==be && self.compact_args_may_unify(
                &[a.parent_args,a.closure_kind_ty,a.closure_sig_as_fn_ptr_ty,a.tupled_upvars_ty],
                &[b.parent_args,b.closure_kind_ty,b.closure_sig_as_fn_ptr_ty,b.tupled_upvars_ty],depth),
            (TyKind::Const(a), TyKind::Const(b)) => self.consts_may_unify_inner(*a,*b),
            _ => false,
        }
    }
    fn consts_may_unify_inner(self, a: Const, b: Const) -> bool {
        let bk=self.tys.const_kind(b);
        if matches!(bk, ConstKind::Param(_) | ConstKind::Alias(..) | ConstKind::Error | ConstKind::Infer(_) | ConstKind::Bound(..)) { return true; }
        match (self.tys.const_kind(a),bk) {
            (ConstKind::Value(a),ConstKind::Value(b)) => a.valtree==b.valtree,
            (ConstKind::Param(a),ConstKind::Param(b)) => self.instantiate_lhs || a==b,
            (ConstKind::Param(_),_) => self.instantiate_lhs,
            (ConstKind::Placeholder(a),ConstKind::Placeholder(b)) => a==b,
            (ConstKind::Value(_) | ConstKind::Placeholder(_),_) => false,
            _ => true,
        }
    }
    fn var_and_ty_may_unify(self, var: TyId, ty: TyId) -> bool {
        if matches!(self.tys.kind(ty), TyKind::Param(_) | TyKind::Alias(..) | TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Bound(..) | TyKind::Error) { return true; }
        match self.tys.kind(var) { TyKind::IntVar(_) => matches!(self.tys.kind(ty),TyKind::Int(_)), TyKind::FloatVar(_) => matches!(self.tys.kind(ty),TyKind::Float(_)), _=>true }
    }
}
