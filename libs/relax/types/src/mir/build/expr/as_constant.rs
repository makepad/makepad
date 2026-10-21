//! rustc_mir_build/src/builder/expr/as_constant.rs. Owner: lane E.
use crate::ast::{ExprKind as AstExprKind, LitKind};
use crate::consts::ConstKind;
use crate::mir::build::builder::Builder;
use crate::mir::build::thir::{ExprId, ExprKind};
use crate::mir::build::{BuildResult, UnsupportedReason};
use crate::mir::{
    AllocId, Allocation, CanonicalUserTypeAnnotation, Const, ConstOperand, ConstValue, CtfeProvenance, Mutability, Scalar, Span,
    UserTypeAnnotationIndex,
};
use crate::program::Prim;
use crate::types::{TyId, TyKind};

impl Builder<'_> {
    /// as_constant.rs:`as_constant`. Compile `expr`, yielding a compile-time constant. Assumes that
    /// `expr` is a valid compile-time constant.
    pub fn as_constant(&mut self, expr: ExprId) -> BuildResult<ConstOperand> {
        let e = &self.thir.thir[expr];
        if let ExprKind::Scope { value, .. } = e.kind {
            return self.as_constant(value);
        }
        self.as_constant_inner(expr)
    }

    /// A user type written on a constant becomes a body annotation (rustc `push_cuta`).
    fn push_cuta(&mut self, user_ty: &Option<Box<crate::types::user_type::CanonicalUserType>>, span: Span, inferred_ty: TyId) -> Option<UserTypeAnnotationIndex> {
        let user_ty = user_ty.as_ref()?;
        self.canonical_user_type_annotations.push(CanonicalUserTypeAnnotation { span, user_ty: user_ty.clone(), inferred_ty });
        Some(UserTypeAnnotationIndex(self.canonical_user_type_annotations.len() as u32 - 1))
    }

    /// as_constant.rs:`as_constant_inner`.
    fn as_constant_inner(&mut self, expr: ExprId) -> BuildResult<ConstOperand> {
        let e = &self.thir.thir[expr];
        let (ty, span) = (e.ty, e.span);
        let kind = e.kind.clone();
        match kind {
            ExprKind::Literal { expr: lit, neg } => {
                let const_ = self.lit_to_mir_constant(lit, ty, neg, span)?;
                Ok(ConstOperand { span, user_ty: None, const_ })
            }
            ExprKind::NonHirLiteral { lit, user_ty } => {
                let user_ty = self.push_cuta(&user_ty, span, ty);
                let const_ = Const::Val(ConstValue::Scalar(Scalar::Int { bits: lit.to_bits(lit.size()), size: lit.size() }), ty);
                Ok(ConstOperand { span, user_ty, const_ })
            }
            ExprKind::ZstLiteral { user_ty } => {
                let user_ty = self.push_cuta(&user_ty, span, ty);
                Ok(ConstOperand { span, user_ty, const_: Const::Val(ConstValue::ZeroSized, ty) })
            }
            ExprKind::NamedConst { def, args, user_ty } => {
                let user_ty = self.push_cuta(&user_ty, span, ty);
                Ok(ConstOperand { span, user_ty, const_: Const::Unevaluated { def, args, promoted: None, ty } })
            }
            ExprKind::ConstParam { param, def: _ } => {
                let ct = self.root.cx.tcx.tys.intern_const(ConstKind::Param(param));
                Ok(ConstOperand { span, user_ty: None, const_: Const::Ty(ty, ct) })
            }
            ExprKind::ConstBlock { def, args } => {
                Ok(ConstOperand { span, user_ty: None, const_: Const::Unevaluated { def, args, promoted: None, ty } })
            }
            ExprKind::StaticRef { def, ty } => {
                let alloc = self.allocs.static_alloc(def);
                let value = ConstValue::Scalar(Scalar::Ptr { provenance: CtfeProvenance::new(alloc), offset: 0 });
                Ok(ConstOperand { span, user_ty: None, const_: Const::Val(value, ty) })
            }
            _ => Err(self.error(span, UnsupportedReason::UnresolvedFact("expression is not a valid constant"))),
        }
    }

    fn alloc_bytes(&mut self, bytes: Vec<u8>) -> AllocId {
        let init_mask = vec![true; bytes.len()];
        self.allocs.insert(Allocation {
            bytes, align: 1, init_mask, deallocated: false, provenance: Vec::new(), provenance_fragments: Vec::new(), mutability: Mutability::Not,
        })
    }

    /// as_constant.rs:`lit_to_mir_constant`. The literal's value is read from its token text (the AST
    /// keeps kinds, not values), truncated to its type exactly as rustc does.
    fn lit_to_mir_constant(&mut self, lit: crate::ast::ExprId, ty: TyId, neg: bool, span: Span) -> BuildResult<Const> {
        let tcx = self.root.cx.tcx;
        let tys = &tcx.tys;
        let pf = &self.root.cx.prog.files[self.root.typeck.file as usize];
        let ex = pf.ast.expr(lit);
        let AstExprKind::Lit(kind) = ex.kind else {
            return Err(self.error(span, UnsupportedReason::UnresolvedFact("literal expression is not a literal")));
        };
        let text = String::from_utf8_lossy(pf.bytes(ex.lo, ex.hi)).into_owned();
        let bad = |this: &Self| this.error(span, UnsupportedReason::UnresolvedFact("invalid literal/type combination"));
        let ty_kind = tys.kind(ty).clone();
        let int_scalar = |bits: u128, size: u8| ConstValue::Scalar(Scalar::Int { bits, size });
        let trunc = |n: u128, size: u8| {
            let bits = if size == 16 { n } else { n & ((1u128 << (size as u32 * 8)) - 1) };
            int_scalar(bits, size)
        };
        let value = match (kind, &ty_kind) {
            (LitKind::Str | LitKind::RawStr, TyKind::Ref(_, inner, _)) if matches!(tys.kind(*inner), TyKind::Str) => {
                let bytes = relax_front::lit::unescape_bytes(&text);
                let meta = bytes.len() as u64;
                ConstValue::Slice { alloc: self.alloc_bytes(bytes), meta }
            }
            (LitKind::ByteStr, TyKind::Ref(_, inner, _)) if matches!(tys.kind(*inner), TyKind::Slice(_)) => {
                let bytes = relax_front::lit::unescape_bytes(&text);
                let meta = bytes.len() as u64;
                ConstValue::Slice { alloc: self.alloc_bytes(bytes), meta }
            }
            (LitKind::ByteStr, TyKind::Ref(_, inner, _)) if matches!(tys.kind(*inner), TyKind::Array(..)) => {
                let bytes = relax_front::lit::unescape_bytes(&text);
                let id = self.alloc_bytes(bytes);
                ConstValue::Scalar(Scalar::Ptr { provenance: CtfeProvenance::new(id), offset: 0 })
            }
            (LitKind::CStr, TyKind::Ref(..)) => {
                let mut bytes = relax_front::lit::unescape_bytes(&text);
                bytes.push(0);
                let meta = bytes.len() as u64;
                ConstValue::Slice { alloc: self.alloc_bytes(bytes), meta }
            }
            (LitKind::Byte, TyKind::Int(Prim::U8)) => {
                let b = relax_front::lit::unescape_bytes(&text).first().copied().ok_or_else(|| bad(self))?;
                int_scalar(b as u128, 1)
            }
            (LitKind::Int, TyKind::Int(p)) => {
                let (size, signed) = tys.int_info(*p);
                let n = crate::const_fold::parse_int_lit(&text).ok_or_else(|| bad(self))?;
                if neg {
                    // Unsigned "negation" has the same bitwise effect as signed negation.
                    trunc(n.wrapping_neg(), size as u8)
                } else if signed || !neg {
                    trunc(n, size as u8)
                } else {
                    return Err(bad(self));
                }
            }
            (LitKind::Int | LitKind::Float, TyKind::Float(p)) => {
                let num = text.replace('_', "");
                let num = num.strip_suffix("f32").or_else(|| num.strip_suffix("f64")).unwrap_or(&num).to_string();
                let sign = |bits: u128, top: u32| if neg { bits ^ (1u128 << top) } else { bits };
                match p {
                    Prim::F32 => int_scalar(sign(num.parse::<f32>().map_err(|_| bad(self))?.to_bits() as u128, 31), 4),
                    Prim::F64 => int_scalar(sign(num.parse::<f64>().map_err(|_| bad(self))?.to_bits() as u128, 63), 8),
                    _ => return Err(self.error(span, UnsupportedReason::UnresolvedFact("f16/f128 literal"))),
                }
            }
            (LitKind::Bool(b), TyKind::Bool) => int_scalar(b as u128, 1),
            (LitKind::Char, TyKind::Char) => {
                let c = relax_front::lit::unescape_str(&text).chars().next().ok_or_else(|| bad(self))?;
                int_scalar(c as u128, 4)
            }
            _ => return Err(bad(self)),
        };
        Ok(Const::Val(value, ty))
    }
}
