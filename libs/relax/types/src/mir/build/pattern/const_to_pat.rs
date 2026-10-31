//! rustc_mir_build/src/thir/pattern/const_to_pat.rs + the literal half of pattern/mod.rs
//! (`lower_pat_expr`): constants and literals as patterns.
//!
//! Diagnostics of unusable constants (non-structural types, NaN, pointers, E0158...) are produced by
//! the match checker (`pattern_analysis`, D8); a constant that cannot be turned into a pattern here
//! makes the body "unchecked" (`BuildError`), never an error.
use super::range::{leaf_bits, scalar_size};
use crate::ast::{ExprId, ExprKind, LitKind, UnOp};
use crate::consts::{self, GenericArg, ScalarInt, ValTree, Value};
use crate::mir::build::thir::{Cx, ErrorGuaranteed, FieldPat, Pat, PatKind, Pinnedness};
use crate::mir::build::BuildResult;
use crate::mir::{FieldIdx, Span, VariantIdx};
use crate::program::{DefId, Prim};
use crate::tcx::{TypingEnv, TypingMode};
use crate::typeck::PAT_KEY;
use crate::types::{TyId, TyKind, TypeFlags};

impl Cx<'_> {
    /// Generic arguments of an ADT type's argument list (types and consts; lifetimes are region slots).
    pub(crate) fn generic_args_of(&self, args: crate::types::TyList) -> Vec<GenericArg> {
        let tys = &self.root.cx.tcx.tys;
        tys.list(args)
            .iter()
            .map(|&arg| match tys.kind(arg) {
                TyKind::Const(c) => GenericArg::Const(*c),
                _ => GenericArg::Type(arg),
            })
            .collect()
    }

    /// The value tree of constant `d` at the use `key` (a pattern id or'ed with `PAT_KEY`, or an
    /// expression id) of type `ty`: evaluated in a revealed environment, as rustc's
    /// `unevaluated_to_pat` does.
    pub(crate) fn const_valtree(&self, d: DefId, key: u32, ty: TyId, span: Span) -> BuildResult<ValTree> {
        let (prog, tcx) = (self.root.cx.prog, self.root.cx.tcx);
        let tb = self.root.typeck;
        let flags = tcx.tys.flags(ty);
        if flags.intersects(TypeFlags::HAS_ALIAS) || !self.valtree_type(ty, 0) {
            return self.unresolved(span, "constant pattern of a type without a value tree");
        }
        let args: Vec<GenericArg> = match tb.res_args.get(&key) {
            Some(l) => self.generic_args_of(*l),
            None => Vec::new(),
        };
        for a in &args {
            let infer = match a {
                GenericArg::Type(x) => tcx.tys.has_infer(*x),
                GenericArg::Const(c) => c.has_non_region_infer(&tcx.tys),
                GenericArg::Lifetime(_) => false,
            };
            if infer {
                return self.unresolved(span, "constant pattern with inference variables");
            }
        }
        let env = TypingEnv { mode: TypingMode::PostAnalysis, ..self.root.cx.env };
        match tcx.const_eval_resolve(prog, env, d, &args) {
            Ok(tree) => Ok(tree),
            Err(_) => self.unresolved(span, "constant pattern does not evaluate"),
        }
    }

    /// Whether a value of type `ty` has a value tree (rustc valtree creation: no raw pointers,
    /// unions, trait objects, ...).
    fn valtree_type(&self, ty: TyId, depth: u32) -> bool {
        let tcx = self.root.cx.tcx;
        if depth > 16 {
            return false;
        }
        match *tcx.tys.kind(ty) {
            TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) | TyKind::Str => true,
            TyKind::Ref(_, i, _) => self.valtree_type(i, depth + 1),
            TyKind::Array(e, _) | TyKind::Slice(e) => self.valtree_type(e, depth + 1),
            TyKind::Tuple(l) => tcx.tys.list(l).iter().all(|x| self.valtree_type(*x, depth + 1)),
            TyKind::Adt(d, args, _) => {
                let Some(a) = tcx.adts.get(&d) else { return false };
                // (a type that never has a value tree of its own may still sit in an unused variant:
                // `None::<Cell<_>>` is a constant; `valtree_to_pat` refuses one a value actually holds)
                if a.is_union() || a.is_manually_drop() || a.is_unsafe_cell() || a.is_unsafe_pinned() || a.is_box() {
                    return true;
                }
                let tys = tcx.tys.list(args);
                (0..a.variants.len()).all(|vi| {
                    a.variants[vi].fields.iter().all(|f| {
                        let ft = tcx.tys.subst(f.ty, tys);
                        let ft = tcx.try_normalize_erasing_regions(self.root.cx.prog, self.root.cx.env, ft).unwrap_or(ft);
                        self.valtree_type(ft, depth + 1)
                    })
                })
            }
            _ => false,
        }
    }

    /// const_to_pat.rs:`valtree_to_pat`: a constant's value as a pattern.
    pub(crate) fn valtree_to_pat(&self, value: &Value, span: Span) -> BuildResult<Pat> {
        let tcx = self.root.cx.tcx;
        let tys = &tcx.tys;
        let ty = value.ty;
        let fields = |branches: &[consts::Const]| -> BuildResult<Vec<FieldPat>> {
            branches
                .iter()
                .enumerate()
                .map(|(index, c)| {
                    let Some(v) = c.try_to_value(tys) else { return self.unresolved(span, "constant field is not a value") };
                    Ok(FieldPat { field: FieldIdx(index as u32), pattern: self.valtree_to_pat(&v, span)? })
                })
                .collect()
        };
        let elems = |branches: &[consts::Const]| -> BuildResult<Box<[Pat]>> {
            branches
                .iter()
                .map(|c| {
                    let Some(v) = c.try_to_value(tys) else { return self.unresolved(span, "constant element is not a value") };
                    self.valtree_to_pat(&v, span)
                })
                .collect::<BuildResult<Vec<_>>>()
                .map(Vec::into_boxed_slice)
        };
        let kind = match *tys.kind(ty) {
            TyKind::Adt(adt, args, _) => {
                let Some(a) = tcx.adts.get(&adt) else { return self.unresolved(span, "constant of an unknown ADT") };
                if a.is_union() || a.is_manually_drop() || a.is_unsafe_cell() || a.is_unsafe_pinned() || a.is_box() {
                    return self.unresolved(span, "constant of a type without a value tree in a pattern");
                }
                let Some(branches) = value.valtree.try_to_branch() else { return self.unresolved(span, "constant ADT is not a branch") };
                if a.is_enum() {
                    let Some((first, rest)) = branches.split_first() else { return self.unresolved(span, "constant enum without a variant") };
                    let Some(index) = first.try_to_value(tys).and_then(|v| leaf_bits(&v.valtree)) else {
                        return self.unresolved(span, "constant enum variant is not a leaf");
                    };
                    PatKind::Variant { adt_def: adt, args: self.generic_args_of(args), variant_index: VariantIdx(index as u32), subpatterns: fields(rest)? }
                } else {
                    PatKind::Leaf { subpatterns: fields(branches)? }
                }
            }
            TyKind::Tuple(_) => {
                let Some(branches) = value.valtree.try_to_branch() else { return self.unresolved(span, "constant tuple is not a branch") };
                PatKind::Leaf { subpatterns: fields(branches)? }
            }
            TyKind::Slice(_) => {
                let Some(branches) = value.valtree.try_to_branch() else { return self.unresolved(span, "constant slice is not a branch") };
                PatKind::Slice { prefix: elems(branches)?, slice: None, suffix: Box::new([]) }
            }
            TyKind::Array(..) => {
                let Some(branches) = value.valtree.try_to_branch() else { return self.unresolved(span, "constant array is not a branch") };
                PatKind::Array { prefix: elems(branches)?, slice: None, suffix: Box::new([]) }
            }
            // `str` constants are the leaves of `&str` patterns (a `Deref` around the constant).
            TyKind::Str => PatKind::Constant { value: value.clone() },
            TyKind::Ref(_, pointee, _) => {
                if matches!(tys.kind(pointee), TyKind::Dyn(..) | TyKind::Foreign(_)) {
                    return self.unresolved(span, "unsized constant pattern");
                }
                // references have the same valtree representation as their pointee
                let inner = self.valtree_to_pat(&Value { ty: pointee, valtree: value.valtree.clone() }, span)?;
                PatKind::Deref { pin: Pinnedness::Not, subpattern: Box::new(inner) }
            }
            TyKind::Float(p) => {
                let Some(bits) = leaf_bits(&value.valtree) else { return self.unresolved(span, "float constant is not a leaf") };
                let nan = if p == Prim::F32 { f32::from_bits(bits as u32).is_nan() } else { f64::from_bits(bits as u64).is_nan() };
                if nan {
                    PatKind::Error(ErrorGuaranteed)
                } else {
                    PatKind::Constant { value: value.clone() }
                }
            }
            TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Ptr(..) => PatKind::Constant { value: value.clone() },
            _ => return self.unresolved(span, "constant pattern of an unsupported type"),
        };
        Ok(Pat { ty, span, extra: None, kind })
    }

    /// pattern/mod.rs:`lower_pat_expr` for a path: a named constant as a pattern.
    pub(crate) fn const_path_to_pat(&self, d: DefId, key: u32, ty: TyId, span: Span) -> BuildResult<Pat> {
        let valtree = self.const_valtree(d, key, ty, span)?;
        let mut pat = self.valtree_to_pat(&Value { ty, valtree }, span)?;
        pat.extra.get_or_insert_default().expanded_const = Some(d);
        Ok(pat)
    }

    /// The value of a literal (or negated literal) pattern expression as a pattern of type `ty`
    /// (pattern/mod.rs:`lower_pat_expr`, `Lit` arm: the pattern's type, not the literal's, is used,
    /// so byte strings against `&[u8]` become slice patterns).
    pub(crate) fn lit_value(&self, e: ExprId, ty: TyId, span: Span) -> BuildResult<Value> {
        let prog = self.root.cx.prog;
        let tys = &self.root.cx.tcx.tys;
        let ast = &prog.files[self.root.typeck.file as usize].ast;
        let pf = &prog.files[self.root.typeck.file as usize];
        let e = crate::lit::strip_paren(ast, e);
        let ex = ast.expr(e);
        let (neg, ex) = match ex.kind {
            ExprKind::Unary(UnOp::Neg, x) => (true, ast.expr(crate::lit::strip_paren(ast, x))),
            _ => (false, ex),
        };
        let ExprKind::Lit(lit) = ex.kind else { return self.unresolved(span, "pattern expression is not a literal") };
        let Ok(text) = std::str::from_utf8(pf.bytes(ex.lo, ex.hi)) else { return self.unresolved(span, "literal is not utf-8") };
        let u8_ty = tys.prim(Prim::U8);
        let bytes_tree = |bytes: &[u8]| {
            let items: Vec<consts::Const> = bytes
                .iter()
                .map(|&b| consts::Const::new_value(tys, ValTree::from_scalar_int(tys, ScalarInt::try_from_uint(b as u128, 1).unwrap()), u8_ty))
                .collect();
            ValTree::from_branches(tys, &items)
        };
        let scalar = |bits: u128| -> BuildResult<Value> {
            let Some((size, _)) = scalar_size(tys, ty) else { return self.unresolved(span, "literal does not match its pattern type") };
            Ok(Value { ty, valtree: ValTree::from_scalar_int(tys, ScalarInt::truncate_from_uint(bits, size).0) })
        };
        match lit {
            LitKind::Int => {
                let Some(mut bits) = crate::const_fold::parse_int_lit(text) else { return self.unresolved(span, "integer literal") };
                if let TyKind::Float(p) = *tys.kind(ty) {
                    let f = if neg { -(bits as f64) } else { bits as f64 };
                    return scalar(if p == Prim::F32 { (f as f32).to_bits() as u128 } else { f.to_bits() as u128 });
                }
                if neg {
                    bits = bits.wrapping_neg();
                }
                scalar(bits)
            }
            LitKind::Bool(b) => scalar(b as u128),
            LitKind::Char => match crate::lit::unescape_str(text).chars().next() {
                Some(c) => scalar(c as u128),
                None => self.unresolved(span, "char literal"),
            },
            LitKind::Byte => match crate::lit::unescape_bytes(text).first() {
                Some(b) => scalar(*b as u128),
                None => self.unresolved(span, "byte literal"),
            },
            LitKind::Float => {
                let cleaned: String = text.chars().filter(|c| *c != '_').collect();
                let cleaned = cleaned.trim_end_matches("f32").trim_end_matches("f64");
                let Ok(mut f) = cleaned.parse::<f64>() else { return self.unresolved(span, "float literal") };
                if neg {
                    f = -f;
                }
                match *tys.kind(ty) {
                    TyKind::Float(Prim::F32) => scalar((f as f32).to_bits() as u128),
                    TyKind::Float(_) => scalar(f.to_bits() as u128),
                    _ => self.unresolved(span, "float literal against a non-float type"),
                }
            }
            LitKind::Str | LitKind::RawStr => Ok(Value { ty, valtree: bytes_tree(crate::lit::unescape_str(text).as_bytes()) }),
            LitKind::ByteStr => Ok(Value { ty, valtree: bytes_tree(&crate::lit::unescape_bytes(text)) }),
            LitKind::CStr => self.unresolved(span, "C string literal pattern"),
        }
    }

    /// A literal pattern expression, or a path to a constant (range endpoints), as a leaf value.
    pub(crate) fn endpoint_value(&self, e: ExprId, ty: TyId, span: Span) -> BuildResult<Value> {
        let prog = self.root.cx.prog;
        let ast = &prog.files[self.root.typeck.file as usize].ast;
        let e = crate::lit::strip_paren(ast, e);
        if let ExprKind::Path(_) = ast.expr(e).kind {
            use crate::program::DefKind;
            return match self.root.typeck.res.get(&e.0).copied() {
                Some(crate::typeck::Res::Def(d)) if matches!(prog.def(d).kind, DefKind::Const | DefKind::AssocConst) => {
                    let valtree = self.const_valtree(d, e.0, ty, span)?;
                    Ok(Value { ty, valtree })
                }
                _ => self.unresolved(span, "range endpoint path is not a constant"),
            };
        }
        self.lit_value(e, ty, span)
    }

    #[allow(dead_code)]
    pub(crate) fn pat_key(p: crate::ast::PatId) -> u32 {
        p.0 | PAT_KEY
    }
}
