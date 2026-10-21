//! rustc_mir_build/src/thir/pattern/mod.rs: AST patterns (with the typeck facts) to `thir::Pat`.
//! Owned by lane C (S4). The shapes are fixed in `thir/mod.rs` at I1.
//!
//! Where the typeck facts differ from rustc's (`pat_adjustments`, `node_type`), this file adapts:
//! - `Body.pat_ty[p]` is the type of the value the pattern is matched against, before the
//!   references match ergonomics peels; `Body.pat_derefs[p]` is how many were peeled. The peeled
//!   references become `PatKind::Deref` nodes around the unadjusted pattern (rustc `lower_pattern`).
//! - A binding's `LocalVarId` is its typeck local index (`Body.pat_local`), the same for every
//!   alternative of an or-pattern; the primary binding is the one whose pattern id is `Local.pat`.
//! - `Body.pat_bind_ref[p]` is a binding made by-reference through the default binding mode.
//! - The `Pat::Ident` of a unit struct / variant / const is a path pattern (`Body.pat_res`).
use crate::ast::{ExprId, Pat as AstPat, PatId};
use crate::consts::{GenericArg, Value};
use crate::mir::build::thir::{Ascription, Cx, DerefPatBorrowMode, ErrorGuaranteed, FieldPat, LocalVarId, Pat, PatKind, PatRange, PatRangeBoundary, Pinnedness, RangeEnd};
use crate::mir::build::{BuildResult, UnsupportedReason};
use crate::mir::{BindingMode, ByRef, CanonicalUserTypeAnnotation, FieldIdx, Mutability, Span, Variance, VariantIdx};
use crate::program::{DefId, DefKind};
use crate::typeck::user_types::UserTypeKey;
use crate::typeck::{Res, PAT_KEY};
use crate::types::user_type::UserTypeKind;
use crate::types::{TyId, TyKind, TypeFlags};
use std::cmp::Ordering;
use std::sync::Arc;

mod const_to_pat;
pub(crate) mod range;
mod span;
#[cfg(test)]
pub(crate) mod tests;


impl<'a> Cx<'a> {
    fn pat_unresolved<T>(&self, span: Span, what: &'static str) -> BuildResult<T> {
        Err(self.error(span, UnsupportedReason::UnresolvedFact(what)))
    }

    /// The type a pattern is matched against (`Body.pat_ty`), None for an untyped or erroneous one.
    fn matched_ty(&self, p: PatId, span: Span) -> BuildResult<TyId> {
        let b = self.root.typeck;
        let i = b.pat_ix(p);
        if i >= b.pat_ty.len() || !b.pat_typed.get(i) {
            return self.pat_unresolved(span, "pattern type");
        }
        let t = b.pat_ty[i];
        let f = self.root.cx.tcx.tys.flags(t);
        if f.intersects(TypeFlags::HAS_ERROR) || f.intersects(TypeFlags::HAS_NON_REGION_INFER) {
            return Err(self.error(span, UnsupportedReason::TypeckTainted));
        }
        Ok(t)
    }

    /// The annotation `key` names, with the span and inferred type of the annotated node.
    fn annotation(&self, key: UserTypeKey, span: Span, inferred_ty: TyId) -> Option<CanonicalUserTypeAnnotation> {
        let user_ty = self.root.typeck.user_provided_types.get(&key)?;
        Some(CanonicalUserTypeAnnotation { user_ty: Box::new((**user_ty).clone()), span, inferred_ty })
    }

    /// thir/util.rs:`user_args_applied_to_ty_of_hir_id`: the annotation of `key` for a node of type
    /// `ty`; the annotation of a constructor path names the ADT itself.
    fn user_args_applied_to_ty(&self, key: UserTypeKey, span: Span, ty: TyId) -> Option<CanonicalUserTypeAnnotation> {
        let mut annotation = self.annotation(key, span, ty)?;
        match *self.root.cx.tcx.tys.kind(ty) {
            TyKind::Adt(adt, ..) => {
                // `type_of(ctor_did)` is a FnDef, but the ADT itself is annotated; consts and fns
                // keep their own definition
                if let UserTypeKind::TypeOf(did, _) = &mut annotation.user_ty.value.kind {
                    *did = adt;
                }
                Some(annotation)
            }
            TyKind::FnDef(..) => Some(annotation),
            _ => None,
        }
    }

    /// Pushes an ascription for the annotation `key` names, if any.
    fn ascribe(&self, pat: &mut Pat, annotation: Option<CanonicalUserTypeAnnotation>, variance: Variance) {
        if let Some(annotation) = annotation {
            pat.extra.get_or_insert_default().ascriptions.push(Ascription { annotation, variance });
        }
    }

    /// pattern/mod.rs:`pat_from_hir`: the pattern of a `let`, with the user type of its explicit type
    /// annotation (`let x: T = ..`) as an ascription. `let_ty` is the annotation's syntax node and span.
    pub fn lower_pattern_for_let(&mut self, p: PatId, let_ty: Option<(crate::ast::TyId, Span)>) -> BuildResult<Pat> {
        let mut pat = self.lower_pattern(p)?;
        if let Some((ty, span)) = let_ty {
            let inferred_ty = self.matched_ty(p, span)?;
            let annotation = self.annotation(UserTypeKey::Type(ty.0), span, inferred_ty);
            self.ascribe(&mut pat, annotation, Variance::Covariant);
        }
        Ok(pat)
    }

    /// pattern/mod.rs:`lower_pattern`: the pattern and its implicit dereferences.
    pub fn lower_pattern(&mut self, p: PatId) -> BuildResult<Pat> {
        let span = self.pat_span(p);
        let ty = self.matched_ty(p, span)?;
        let tys = &self.root.cx.tcx.tys;
        let n = self.root.typeck.pat_derefs.get(&p.0).copied().unwrap_or(0);
        // adjustment k: source type tys[k], peeling one reference
        let mut peeled = vec![ty];
        for _ in 0..n {
            match *tys.kind(*peeled.last().unwrap()) {
                TyKind::Ref(_, inner, _) => peeled.push(inner),
                _ => return self.pat_unresolved(span, "implicit dereference of a non-reference"),
            }
        }
        let mut pat = self.lower_pattern_unadjusted(p, *peeled.last().unwrap())?;
        for k in (0..n).rev() {
            let span = pat.span;
            pat = Pat { ty: peeled[k], span, extra: None, kind: PatKind::Deref { pin: Pinnedness::Not, subpattern: Box::new(pat) } };
        }
        Ok(pat)
    }

    fn lower_patterns(&mut self, pats: &[PatId]) -> BuildResult<Box<[Pat]>> {
        pats.iter().map(|&p| self.lower_pattern(p)).collect::<BuildResult<Vec<_>>>().map(Vec::into_boxed_slice)
    }

    pub(crate) fn adt_variant_fields(&self, adt: DefId, variant: u32) -> usize {
        self.root.cx.tcx.adts.get(&adt).and_then(|a| a.variants.get(variant as usize)).map_or(0, |v| v.fields.len())
    }

    /// pattern/mod.rs:`lower_tuple_subpats` over `EnumerateAndAdjustIterator`: the fields a `..`
    /// leaves out are not listed.
    fn lower_seq_subpats(&mut self, subs: &[PatId], arity: usize, span: Span) -> BuildResult<Vec<FieldPat>> {
        let ast = self.ast();
        let rest = subs.iter().position(|x| matches!(ast.pat(*x), AstPat::Rest));
        let n = subs.len();
        let mut out = Vec::with_capacity(n);
        for (i, &x) in subs.iter().enumerate() {
            if Some(i) == rest {
                continue;
            }
            let idx = match rest {
                Some(r) if i > r => (arity + i).checked_sub(n),
                _ => Some(i),
            };
            let Some(idx) = idx.filter(|idx| *idx < arity) else { return self.pat_unresolved(span, "sub-pattern count does not match the fields") };
            out.push(FieldPat { field: FieldIdx(idx as u32), pattern: self.lower_pattern(x)? });
        }
        Ok(out)
    }

    pub(crate) fn field_index(&self, adt: DefId, variant: u32, name: crate::ast::Ident) -> Option<u32> {
        let prog = self.root.cx.prog;
        let pf = &prog.files[self.root.typeck.file as usize];
        let text = std::str::from_utf8(pf.bytes(name.lo, name.hi)).ok()?;
        let text = text.strip_prefix("r#").unwrap_or(text);
        let sym = if text.as_bytes().first().is_some_and(|c| c.is_ascii_digit()) {
            crate::ast::tuple_index(text)? | 0x8000_0000
        } else {
            prog.syms.get(text)?
        };
        let pos = self.root.cx.tcx.adts.get(&adt)?.variants.get(variant as usize)?.fields.iter().position(|f| f.name == sym)?;
        Some(pos as u32)
    }

    /// pattern/mod.rs:`lower_variant_or_leaf`: a variant (or struct) constructor pattern of the ADT `ty`.
    pub(crate) fn lower_variant_or_leaf(&self, key: UserTypeKey, ty: TyId, res: Option<DefId>, subpatterns: Vec<FieldPat>, span: Span) -> BuildResult<Pat> {
        let (prog, tcx) = (self.root.cx.prog, self.root.cx.tcx);
        let TyKind::Adt(adt, args, _) = *tcx.tys.kind(ty) else { return self.pat_unresolved(span, "constructor pattern of a non-ADT type") };
        let Some(a) = tcx.adts.get(&adt) else { return self.pat_unresolved(span, "constructor pattern of an unknown ADT") };
        let kind = if a.is_enum() {
            let Some(variant) = res.filter(|d| prog.def(*d).kind == DefKind::Variant) else {
                return self.pat_unresolved(span, "enum pattern without a variant");
            };
            PatKind::Variant { adt_def: adt, args: self.generic_args_of(args), variant_index: VariantIdx(prog.def(variant).sub), subpatterns }
        } else {
            PatKind::Leaf { subpatterns }
        };
        let mut pat = Pat { ty, span, extra: None, kind };
        let annotation = self.user_args_applied_to_ty(key, span, ty);
        self.ascribe(&mut pat, annotation, Variance::Covariant);
        Ok(pat)
    }

    /// pattern/mod.rs:`lower_path`: a path (or identifier) pattern that names a constant, unit
    /// variant or unit struct.
    fn lower_path_pat(&self, key: u32, d: DefId, ty: TyId, span: Span) -> BuildResult<Pat> {
        let user_key = UserTypeKey::of_node(key);
        match self.root.cx.prog.def(d).kind {
            DefKind::Const | DefKind::AssocConst => {
                let mut pat = self.const_path_to_pat(d, key, ty, span)?;
                // an associated constant with an explicit user-written type (`<Foo<'a> as Tr>::C`);
                // note the variance (see `Ascription::variance`)
                let annotation = self.annotation(user_key, span, ty);
                self.ascribe(&mut pat, annotation, Variance::Contravariant);
                Ok(pat)
            }
            DefKind::Variant | DefKind::Struct => self.lower_variant_or_leaf(user_key, ty, Some(d), Vec::new(), span),
            // statics and others are errors E0532 (reported by typeck)
            _ => Err(self.error(span, UnsupportedReason::TypeckTainted)),
        }
    }

    /// pattern/mod.rs:`lower_pattern_range`.
    fn lower_pattern_range(&mut self, lo: Option<ExprId>, hi: Option<ExprId>, inclusive: bool, ty: TyId, span: Span) -> BuildResult<Pat> {
        let tys = &self.root.cx.tcx.tys;
        let end = if inclusive { RangeEnd::Included } else { RangeEnd::Excluded };
        if lo.is_none() && hi.is_none() {
            return self.pat_unresolved(span, "twice-open range pattern");
        }
        let endpoint = |this: &Self, e: Option<ExprId>, default: PatRangeBoundary| -> BuildResult<PatRangeBoundary> {
            match e {
                Some(e) => Ok(PatRangeBoundary::Finite(this.endpoint_value(e, ty, span)?.valtree)),
                None => Ok(default),
            }
        };
        let lo_b = endpoint(self, lo, PatRangeBoundary::NegInfinity)?;
        // `lo..` is an inclusive range to the type's maximum
        let hi_b = endpoint(self, hi, PatRangeBoundary::PosInfinity)?;
        let end = if hi.is_none() { RangeEnd::Included } else { end };
        let cmp = lo_b.compare_with(&hi_b, ty, tys);
        let mut kind = PatKind::Range(Arc::new(PatRange { lo: lo_b.clone(), hi: hi_b.clone(), end, ty }));
        match (end, cmp) {
            (RangeEnd::Excluded, Some(Ordering::Less)) => {}
            (RangeEnd::Included, Some(Ordering::Less)) => {}
            // `x..=y` where `x == y` and both are finite
            (RangeEnd::Included, Some(Ordering::Equal)) if lo_b.is_finite() && hi_b.is_finite() => {
                kind = PatKind::Constant { value: Value { ty, valtree: lo_b.as_finite().unwrap().clone() } };
            }
            // `..=x` where `x == ty::MIN`, `x..` where `x == ty::MAX`
            (RangeEnd::Included, Some(Ordering::Equal)) if !lo_b.is_finite() || !hi_b.is_finite() => {}
            // an empty range is an error (E0030 / E0579), reported nowhere else in Relax yet
            _ => kind = PatKind::Error(ErrorGuaranteed),
        }
        let mut pat = Pat { ty, span, extra: None, kind };
        // associated constants with explicit user-written types (`Foo::<'a>::A..=Foo::B`): their
        // ascriptions go on the range pattern
        let ast = self.ast();
        for e in [lo, hi].into_iter().flatten() {
            let e = crate::lit::strip_paren(ast, e);
            if matches!(ast.expr(e).kind, crate::ast::ExprKind::Path(_)) {
                let annotation = self.annotation(UserTypeKey::Expr(e.0), span, ty);
                self.ascribe(&mut pat, annotation, Variance::Contravariant);
            }
        }
        Ok(pat)
    }

    fn lower_slice_pattern(&mut self, subs: &[PatId], ty: TyId, span: Span) -> BuildResult<Pat> {
        let tcx = self.root.cx.tcx;
        let ast = self.ast();
        let rest = subs.iter().position(|x| crate::typeck::is_rest_pat(ast, *x));
        let (prefix_ids, slice_id, suffix_ids): (&[PatId], Option<PatId>, &[PatId]) = match rest {
            Some(r) => (&subs[..r], Some(subs[r]), &subs[r + 1..]),
            None => (subs, None, &[]),
        };
        let (elem, array_len) = match *tcx.tys.kind(ty) {
            TyKind::Slice(e) => (e, None),
            TyKind::Array(e, len) => match len.try_to_target_usize(&tcx.tys) {
                Some(len) => (e, Some(len)),
                None => return self.pat_unresolved(span, "array length in a pattern"),
            },
            _ => return self.pat_unresolved(span, "slice pattern of a non-sequence type"),
        };
        let prefix = self.lower_patterns(prefix_ids)?;
        let suffix = self.lower_patterns(suffix_ids)?;
        let slice = match slice_id {
            None => None,
            Some(s) => {
                // `x @ ..` is a binding with its own type; a bare `..` has the subslice type
                let has_own_ty = matches!(ast.pat(s), AstPat::Ident { .. });
                if has_own_ty {
                    Some(Box::new(self.lower_pattern(s)?))
                } else {
                    let fixed = (prefix_ids.len() + suffix_ids.len()) as u64;
                    let sub_ty = match array_len {
                        Some(n) => tcx.tys.intern(TyKind::Array(elem, tcx.tys.const_usize(n.saturating_sub(fixed)))),
                        None => tcx.tys.intern(TyKind::Slice(elem)),
                    };
                    let rest_span = self.pat_span(s);
                    Some(Box::new(Pat { ty: sub_ty, span: rest_span, extra: None, kind: PatKind::Wild }))
                }
            }
        };
        let kind = match array_len {
            None => PatKind::Slice { prefix, slice, suffix },
            Some(len) => {
                if len < (prefix.len() + suffix.len()) as u64 {
                    return self.pat_unresolved(span, "array pattern longer than its array");
                }
                PatKind::Array { prefix, slice, suffix }
            }
        };
        Ok(Pat { ty, span, extra: None, kind })
    }

    /// pattern/mod.rs:`lower_pattern_unadjusted`: `ty` is the type after the implicit dereferences.
    fn lower_pattern_unadjusted(&mut self, p: PatId, ty: TyId) -> BuildResult<Pat> {
        let ast = self.ast();
        let tb = self.root.typeck;
        let tcx = self.root.cx.tcx;
        let span = self.pat_span(p);
        let node = |kind| Pat { ty, span, extra: None, kind };
        match *ast.pat(p) {
            AstPat::Wild | AstPat::Rest => Ok(node(PatKind::Wild)),
            AstPat::Mac(_) => self.pat_unresolved(span, "macro in pattern position"),
            AstPat::Paren(x) => self.lower_pattern(x),
            AstPat::Ident { by_ref, mutbl, sub, .. } => {
                if let Some(Res::Def(d)) = tb.pat_res.get(&p.0).copied() {
                    if matches!(self.root.cx.prog.def(d).kind, DefKind::Const | DefKind::AssocConst | DefKind::Variant | DefKind::Struct) {
                        return self.lower_path_pat(p.0 | PAT_KEY, d, ty, span);
                    }
                }
                let Some(&li) = tb.pat_local.get(&p.0) else { return self.pat_unresolved(span, "binding without a local") };
                let local = &tb.locals[li as usize];
                let mode = if by_ref {
                    BindingMode(ByRef::Yes(if mutbl { Mutability::Mut } else { Mutability::Not }), Mutability::Not)
                } else if let Some(&m) = tb.pat_bind_ref.get(&p.0) {
                    BindingMode(ByRef::Yes(if m == 2 { Mutability::Mut } else { Mutability::Not }), Mutability::Not)
                } else {
                    BindingMode(ByRef::No, if mutbl { Mutability::Mut } else { Mutability::Not })
                };
                let subpattern = match sub {
                    Some(s) => Some(Box::new(self.lower_pattern(s)?)),
                    None => None,
                };
                Ok(node(PatKind::Binding {
                    name: local.name,
                    mode,
                    var: LocalVarId(li),
                    ty: local.ty,
                    subpattern,
                    is_primary: local.pat == p.0,
                    is_shorthand: false,
                }))
            }
            AstPat::Lit(e) => {
                let value = self.lit_value(e, ty, span)?;
                self.valtree_to_pat(&value, span)
            }
            AstPat::Range(lo, hi, inclusive) => self.lower_pattern_range(lo, hi, inclusive, ty, span),
            AstPat::Path(_) => match tb.pat_res.get(&p.0).copied() {
                Some(Res::Def(d)) => self.lower_path_pat(p.0 | PAT_KEY, d, ty, span),
                _ => self.pat_unresolved(span, "unresolved path pattern"),
            },
            AstPat::TupleStruct(_, subs) => {
                let Some(Res::Def(d)) = tb.pat_res.get(&p.0).copied() else { return self.pat_unresolved(span, "unresolved tuple struct pattern") };
                let TyKind::Adt(adt, _, _) = *tcx.tys.kind(ty) else { return self.pat_unresolved(span, "tuple struct pattern of a non-ADT type") };
                let variant = if self.root.cx.prog.def(d).kind == DefKind::Variant { self.root.cx.prog.def(d).sub } else { 0 };
                let arity = self.adt_variant_fields(adt, variant);
                let subpatterns = self.lower_seq_subpats(ast.list(subs), arity, span)?;
                self.lower_variant_or_leaf(UserTypeKey::Pattern(p.0), ty, Some(d), subpatterns, span)
            }
            AstPat::Struct(_, fields, _) => {
                let Some(Res::Def(d)) = tb.pat_res.get(&p.0).copied() else { return self.pat_unresolved(span, "unresolved struct pattern") };
                let TyKind::Adt(adt, _, _) = *tcx.tys.kind(ty) else { return self.pat_unresolved(span, "struct pattern of a non-ADT type") };
                let variant = if self.root.cx.prog.def(d).kind == DefKind::Variant { self.root.cx.prog.def(d).sub } else { 0 };
                let mut subpatterns = Vec::new();
                for fp in ast.list(fields) {
                    let Some(field) = self.field_index(adt, variant, fp.name) else { return self.pat_unresolved(span, "unknown field in a struct pattern") };
                    let mut pattern = self.lower_pattern(fp.pat)?;
                    if let PatKind::Binding { is_shorthand, .. } = &mut pattern.kind {
                        *is_shorthand = fp.shorthand;
                    }
                    subpatterns.push(FieldPat { field: FieldIdx(field), pattern });
                }
                self.lower_variant_or_leaf(UserTypeKey::Pattern(p.0), ty, Some(d), subpatterns, span)
            }
            AstPat::Tuple(subs) => {
                let TyKind::Tuple(l) = *tcx.tys.kind(ty) else { return self.pat_unresolved(span, "tuple pattern of a non-tuple type") };
                let arity = tcx.tys.list(l).len();
                let subpatterns = self.lower_seq_subpats(ast.list(subs), arity, span)?;
                Ok(node(PatKind::Leaf { subpatterns }))
            }
            AstPat::Slice(subs) => self.lower_slice_pattern(ast.list(subs), ty, span),
            AstPat::Ref(_, x) => Ok(node(PatKind::Deref { pin: Pinnedness::Not, subpattern: Box::new(self.lower_pattern(x)?) })),
            AstPat::Box(x) => Ok(node(PatKind::DerefPattern { subpattern: Box::new(self.lower_pattern(x)?), borrow: DerefPatBorrowMode::Box })),
            AstPat::Or(alts) => {
                let pats = self.lower_patterns(ast.list(alts))?;
                Ok(node(PatKind::Or { pats }))
            }
        }
    }
}

/// Generic arguments of a type list, for tests.
#[allow(dead_code)]
pub(crate) fn args_of(cx: &Cx<'_>, ty: TyId) -> Vec<GenericArg> {
    match *cx.root.cx.tcx.tys.kind(ty) {
        TyKind::Adt(_, args, _) => cx.generic_args_of(args),
        _ => Vec::new(),
    }
}
