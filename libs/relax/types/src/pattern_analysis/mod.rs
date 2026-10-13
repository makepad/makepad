//! Match checking of `makepad check` (rustc_mir_build thir/pattern/check_match.rs over
//! rustc_pattern_analysis): E0004 (a `match` that does not cover every value), E0005 (a
//! refutable pattern in a `let`, `for`, or fn / closure parameter), the `unreachable_patterns`
//! lint and the `irrefutable_let_patterns` lint. The patterns are lowered from the typed AST
//! (types, match ergonomics derefs and resolutions from the typeck side tables) into
//! deconstructed patterns, which usefulness.rs analyses.
//!
//! Check-only: `makepad run` never calls it.

pub mod constructor;
pub mod usefulness;

use crate::ast::*;
use crate::lang_items::LangItem;
use crate::program::{DefId, DefKind, Prim, Program};
use crate::tcx::{Tcx, TypingEnv};
use crate::typeck::{Body, Res, PAT_KEY};
use crate::types::{TyId, TyKind, TypeFlags};
use constructor::{ConstructorSet, Ctor, IntRange, MaybeInfiniteInt, RangeEnd, Slice, SliceKind, VariantVisibility};
use std::cell::RefCell;
use std::collections::HashMap;
use usefulness::{MatchArm, PlaceValidity, Usefulness, WitnessPat};

/// A deconstructed pattern's index in `Cx::pats`.
pub type PIx = usize;

/// A pattern as the analysis sees it: a constructor with its (indexed) fields; missing
/// fields are wildcards.
pub struct DPat {
    pub ctor: Ctor,
    pub fields: Vec<(usize, PIx)>,
    pub arity: usize,
    pub ty: TyId,
    /// the source position diagnostics point at (None: a `_` / `..` with no position of its own)
    pub pos: Option<u32>,
}

/// The analysis context of one body: its types, the module it is in, the lowered patterns.
pub struct Cx<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    body: &'a Body,
    env: TypingEnv,
    module: u32,
    krate: u32,
    pats: Vec<DPat>,
    next_opaque: u32,
    inhabited: RefCell<HashMap<TyId, bool>>,
}

impl<'a> Cx<'a> {
    pub fn pat(&self, p: PIx) -> &DPat {
        &self.pats[p]
    }

    fn ast(&self) -> &'a Ast {
        &self.prog.files[self.body.file as usize].ast
    }

    fn kind(&self, t: TyId) -> TyKind {
        *self.tcx.tys.kind(t)
    }

    /// An opaque type as its hidden type (rustc reveal_opaque_ty).
    fn reveal(&self, t: TyId) -> TyId {
        if let TyKind::Alias(_, crate::types::AliasTy { kind: crate::types::AliasTyKind::Opaque { def_id }, args, .. }) = self.kind(t) {
            if let Some(h) = self.body.hidden_types.get(&def_id) {
                return self.tcx.tys.subst(*h, self.tcx.tys.list(args));
            }
        }
        t
    }

    fn normalize(&self, t: TyId) -> TyId {
        let t = self.tcx.try_normalize_erasing_regions(self.prog, self.env, t).unwrap_or(t);
        self.reveal(t)
    }

    /// The field types of variant `vi` of the ADT type `t`.
    fn variant_field_tys(&self, adt: DefId, vi: u32, args: crate::types::TyList) -> Vec<TyId> {
        let Some(a) = self.tcx.adts.get(&adt) else { return Vec::new() };
        let Some(v) = a.variants.get(vi as usize) else { return Vec::new() };
        let args = self.tcx.tys.list(args);
        v.fields.iter().map(|f| self.normalize(self.tcx.tys.subst(f.ty, args))).collect()
    }

    /// Whether field `f` of ADT `d` is visible from this body's module (typeck field_visible).
    fn field_visible(&self, d: DefId, f: &crate::tcx::FieldT) -> bool {
        if f.vis_mod != u32::MAX && f.is_pub {
            return self.prog.mod_within(self.module, f.vis_mod);
        }
        if f.is_pub {
            return true;
        }
        let target = self.prog.def(d).scope;
        let mut m = Some(self.module);
        let mut guard = 0;
        while let Some(cur) = m {
            if cur == target || self.prog.mods[cur as usize].normal == target {
                return true;
            }
            let md = &self.prog.mods[cur as usize];
            m = md.lexical_parent.or(md.parent_mod);
            guard += 1;
            if guard > 256 {
                break;
            }
        }
        false
    }

    /// An enum marked `#[non_exhaustive]` in another crate.
    fn is_foreign_non_exhaustive_enum(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Adt(d, _, _) => self.tcx.adts.get(&d).is_some_and(|a| a.is_enum && a.is_variant_list_non_exhaustive()) && self.prog.def(d).krate != self.krate,
            _ => false,
        }
    }

    /// rustc inhabitedness (ty/inhabitedness): uninhabited from this body's module. Private
    /// fields of an uninhabited type make a struct uninhabited only inside their module.
    pub fn is_uninhabited(&self, t: TyId) -> bool {
        !self.inhabited_at(t, 0)
    }

    fn inhabited_at(&self, t: TyId, depth: u32) -> bool {
        if depth > 32 {
            return true;
        }
        if let Some(v) = self.inhabited.borrow().get(&t) {
            return *v;
        }
        // a cycle (recursive types are inhabited unless proven otherwise)
        self.inhabited.borrow_mut().insert(t, true);
        let v = match self.kind(t) {
            TyKind::Never => false,
            TyKind::Tuple(l) => self.tcx.tys.list(l).iter().all(|x| self.inhabited_at(*x, depth + 1)),
            TyKind::Array(e, n) => match n.try_to_target_usize(&self.tcx.tys) {
                Some(0) | None => true,
                Some(_) => self.inhabited_at(e, depth + 1),
            },
            TyKind::Adt(d, args, _) => match self.tcx.adts.get(&d) {
                Some(a) if a.is_union => true,
                Some(_) if self.is_foreign_non_exhaustive_enum(t) => true,
                Some(a) => (0..a.variants.len()).any(|vi| self.variant_inhabited(d, vi as u32, args, depth)),
                None => true,
            },
            TyKind::Alias(..) => {
                let r = self.normalize(t);
                r == t || self.inhabited_at(r, depth + 1)
            }
            _ => true,
        };
        self.inhabited.borrow_mut().insert(t, v);
        v
    }

    fn variant_inhabited(&self, d: DefId, vi: u32, args: crate::types::TyList, depth: u32) -> bool {
        let a = &self.tcx.adts[&d];
        let tys = self.variant_field_tys(d, vi, args);
        a.variants[vi as usize].fields.iter().zip(tys).all(|(f, ft)| {
            self.inhabited_at(ft, depth + 1) || (!a.is_enum && !self.field_visible(d, f))
        })
    }

    pub fn ctor_arity(&self, ctor: &Ctor, t: TyId) -> usize {
        match ctor {
            Ctor::Struct | Ctor::Variant(_) | Ctor::UnionField => match self.kind(t) {
                TyKind::Tuple(l) => self.tcx.tys.list(l).len(),
                TyKind::Adt(d, _, _) => {
                    let vi = if let Ctor::Variant(i) = ctor { *i } else { 0 };
                    self.tcx.adts.get(&d).and_then(|a| a.variants.get(vi as usize)).map_or(0, |v| v.fields.len())
                }
                _ => 0,
            },
            Ctor::Ref => 1,
            Ctor::Slice(s) => s.arity(),
            _ => 0,
        }
    }

    /// The types of a constructor's fields, each with whether it is a private field of an
    /// uninhabited type (skipped by the analysis).
    pub fn ctor_sub_tys(&self, ctor: &Ctor, t: TyId) -> Vec<(TyId, bool)> {
        match ctor {
            Ctor::Struct | Ctor::Variant(_) | Ctor::UnionField => match self.kind(t) {
                TyKind::Tuple(l) => self.tcx.tys.list(l).iter().map(|x| (self.normalize(*x), false)).collect(),
                TyKind::Adt(d, args, _) => {
                    let vi = if let Ctor::Variant(i) = ctor { *i } else { 0 };
                    let a = &self.tcx.adts[&d];
                    let tys = self.variant_field_tys(d, vi, args);
                    a.variants[vi as usize].fields.iter().zip(tys).map(|(f, ft)| {
                        let visible = a.is_enum || self.field_visible(d, f);
                        let skip = !visible && self.is_uninhabited(ft);
                        (ft, skip)
                    }).collect()
                }
                _ => Vec::new(),
            },
            Ctor::Ref => match self.kind(t) {
                TyKind::Ref(_, i, _) => vec![(self.normalize(i), false)],
                _ => Vec::new(),
            },
            Ctor::Slice(s) => match self.kind(t) {
                TyKind::Array(e, _) | TyKind::Slice(e) => vec![(self.normalize(e), false); s.arity()],
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// The constructors of a type (rustc.rs ctors_for_ty). None: an error or inference type.
    pub fn ctors_for_ty(&self, t: TyId) -> Option<ConstructorSet> {
        let uint_range = |lo: u128, hi: u128| IntRange::from_range(MaybeInfiniteInt::new_finite_uint(lo), MaybeInfiniteInt::new_finite_uint(hi), RangeEnd::Included).expect("nonempty");
        Some(match self.kind(t) {
            TyKind::Error | TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) => return None,
            TyKind::Bool => ConstructorSet::Bool,
            TyKind::Char => ConstructorSet::Integers { range_1: uint_range(0, 0xD7FF), range_2: Some(uint_range(0xE000, 0x10FFFF)) },
            TyKind::Int(p) => {
                let (bytes, signed) = crate::types::Types::int_info(p);
                let range = if matches!(p, Prim::Isize) {
                    IntRange { lo: MaybeInfiniteInt::NegInfinity, hi: MaybeInfiniteInt::PosInfinity }
                } else if matches!(p, Prim::Usize) {
                    IntRange { lo: MaybeInfiniteInt::new_finite_uint(0), hi: MaybeInfiniteInt::PosInfinity }
                } else if signed {
                    let size = bytes * 8;
                    let min = 1u128 << (size - 1);
                    let max = min - 1;
                    IntRange::from_range(MaybeInfiniteInt::new_finite_int(min, size), MaybeInfiniteInt::new_finite_int(max, size), RangeEnd::Included).expect("nonempty")
                } else {
                    uint_range(0, if bytes == 16 { u128::MAX } else { (1u128 << (bytes * 8)) - 1 })
                };
                ConstructorSet::Integers { range_1: range, range_2: None }
            }
            TyKind::Slice(e) => ConstructorSet::Slice { array_len: None, subtype_is_empty: self.is_uninhabited(e) },
            TyKind::Array(e, n) => ConstructorSet::Slice {
                array_len: n.try_to_target_usize(&self.tcx.tys).map(|l| l as usize),
                subtype_is_empty: self.is_uninhabited(e),
            },
            TyKind::Adt(d, args, _) => {
                let Some(a) = self.tcx.adts.get(&d) else { return Some(ConstructorSet::Unlistable) };
                if a.is_enum {
                    let non_exhaustive = self.is_foreign_non_exhaustive_enum(t);
                    if a.variants.is_empty() && !non_exhaustive {
                        ConstructorSet::NoConstructors
                    } else {
                        let variants = (0..a.variants.len()).map(|vi| {
                            if self.variant_inhabited(d, vi as u32, args, 0) { VariantVisibility::Visible } else { VariantVisibility::Empty }
                        }).collect();
                        ConstructorSet::Variants { variants, non_exhaustive }
                    }
                } else if a.is_union {
                    ConstructorSet::Union
                } else {
                    ConstructorSet::Struct { empty: self.is_uninhabited(t) }
                }
            }
            TyKind::Tuple(_) => ConstructorSet::Struct { empty: self.is_uninhabited(t) },
            TyKind::Ref(..) => ConstructorSet::Ref,
            TyKind::Never => ConstructorSet::NoConstructors,
            _ => ConstructorSet::Unlistable,
        })
    }

    fn push(&mut self, ctor: Ctor, fields: Vec<(usize, PIx)>, arity: usize, ty: TyId, pos: Option<u32>) -> PIx {
        self.pats.push(DPat { ctor, fields, arity, ty, pos });
        self.pats.len() - 1
    }

    fn opaque(&mut self, ty: TyId, pos: Option<u32>) -> PIx {
        self.next_opaque += 1;
        self.push(Ctor::Opaque(self.next_opaque), Vec::new(), 0, ty, pos)
    }

    // ---------------------------------------------------------------- lowering

    /// A pattern's source position (patterns carry none; their parts do).
    pub fn pat_pos(&self, p: PatId) -> Option<u32> {
        let ast = self.ast();
        match ast.pat(p) {
            Pat::Wild | Pat::Rest => None,
            Pat::Ident { name, .. } => Some(name.lo),
            Pat::Lit(e) => Some(ast.expr(*e).lo),
            Pat::Range(a, b, _) => a.or(*b).map(|e| ast.expr(e).lo),
            Pat::Path(path) | Pat::TupleStruct(path, _) | Pat::Struct(path, _, _) => Some(path.lo),
            Pat::Tuple(v) | Pat::Slice(v) | Pat::Or(v) => ast.list(*v).iter().find_map(|x| self.pat_pos(*x)),
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.pat_pos(*x),
            Pat::Mac(m) => Some(ast.row(*m).path.lo),
        }
    }

    fn pat_ty(&self, p: PatId) -> Option<TyId> {
        let i = self.body.pat_ix(p);
        if i >= self.body.pat_ty.len() || !self.body.pat_typed.get(i) {
            return None;
        }
        let t = self.body.pat_ty[i];
        let f = self.tcx.tys.flags(t);
        if f.intersects(TypeFlags::HAS_ERROR) || f.intersects(TypeFlags::HAS_NON_REGION_INFER) {
            return None;
        }
        Some(self.reveal(t))
    }

    /// Lowers a pattern (rustc.rs lower_pat over the THIR pattern). None: a pattern the
    /// analysis cannot take (a type error, a macro): the match is not checked.
    fn lower(&mut self, p: PatId) -> Option<PIx> {
        let ty0 = self.pat_ty(p)?;
        let pos = self.pat_pos(p);
        let n = self.body.pat_derefs.get(&p.0).copied().unwrap_or(0);
        // match ergonomics: the implicit derefs are `&` patterns around the written one
        let mut tys = vec![ty0];
        for _ in 0..n {
            match self.kind(*tys.last().unwrap()) {
                TyKind::Ref(_, i, _) => tys.push(self.reveal(i)),
                _ => return None,
            }
        }
        let mut cur = self.lower_inner(p, *tys.last().unwrap(), pos)?;
        for k in (0..n).rev() {
            cur = self.push(Ctor::Ref, vec![(0, cur)], 1, tys[k], pos);
        }
        Some(cur)
    }

    fn lower_inner(&mut self, p: PatId, t: TyId, pos: Option<u32>) -> Option<PIx> {
        let ast = self.ast();
        match *ast.pat(p) {
            Pat::Wild | Pat::Rest | Pat::Box(_) => Some(self.push(Ctor::Wildcard, Vec::new(), 0, t, pos)),
            Pat::Ident { sub, .. } => {
                if let Some(Res::Def(d)) = self.body.pat_res.get(&p.0).copied() {
                    if matches!(self.prog.def(d).kind, DefKind::Const | DefKind::AssocConst | DefKind::Variant | DefKind::Struct) {
                        return self.lower_path_res(p, d, t, pos);
                    }
                }
                match sub {
                    Some(s) => self.lower(s),
                    None => Some(self.push(Ctor::Wildcard, Vec::new(), 0, t, pos)),
                }
            }
            Pat::Paren(x) => self.lower(x),
            Pat::Lit(e) => self.lower_lit(e, t, pos),
            Pat::Range(a, b, incl) => self.lower_range(a, b, incl, t, pos),
            Pat::Path(_) => match self.body.pat_res.get(&p.0).copied() {
                Some(Res::Def(d)) => self.lower_path_res(p, d, t, pos),
                _ => None,
            },
            Pat::TupleStruct(_, subs) => {
                let Some(Res::Def(d)) = self.body.pat_res.get(&p.0).copied() else { return None };
                let (ctor, arity) = self.adt_ctor(t, d)?;
                let fields = self.seq_fields(ast.list(subs), arity)?;
                Some(self.push(ctor, fields, arity, t, pos))
            }
            Pat::Struct(_, fps, _) => {
                let Some(Res::Def(d)) = self.body.pat_res.get(&p.0).copied() else { return None };
                let (ctor, arity) = self.adt_ctor(t, d)?;
                let TyKind::Adt(ad, _, _) = self.kind(t) else { return None };
                let vi = if let Ctor::Variant(i) = ctor { i } else { 0 };
                let mut fields = Vec::new();
                for fp in ast.list(fps) {
                    let idx = self.field_index(ad, vi, fp.name)?;
                    let f = self.lower(fp.pat)?;
                    fields.push((idx, f));
                }
                Some(self.push(ctor, fields, arity, t, pos))
            }
            Pat::Tuple(subs) => {
                let TyKind::Tuple(l) = self.kind(t) else { return None };
                let arity = self.tcx.tys.list(l).len();
                let fields = self.seq_fields(ast.list(subs), arity)?;
                Some(self.push(Ctor::Struct, fields, arity, t, pos))
            }
            Pat::Slice(subs) => {
                let array_len = match self.kind(t) {
                    TyKind::Array(_, n) => Some(n.try_to_target_usize(&self.tcx.tys)? as usize),
                    TyKind::Slice(_) => None,
                    _ => return None,
                };
                let subs = ast.list(subs);
                let is_rest = |x: PatId| match ast.pat(x) {
                    Pat::Rest => true,
                    Pat::Ident { sub: Some(s), .. } => matches!(ast.pat(*s), Pat::Rest),
                    _ => false,
                };
                let rest = subs.iter().position(|x| is_rest(*x));
                let (prefix, suffix): (&[PatId], &[PatId]) = match rest {
                    Some(r) => (&subs[..r], &subs[r + 1..]),
                    None => (subs, &[]),
                };
                let kind = if rest.is_some() { SliceKind::VarLen(prefix.len(), suffix.len()) } else { SliceKind::FixedLen(prefix.len()) };
                let slice = Slice::new(array_len, kind)?;
                let mut fields = Vec::new();
                for (i, x) in prefix.iter().chain(suffix.iter()).enumerate() {
                    fields.push((i, self.lower(*x)?));
                }
                Some(self.push(Ctor::Slice(slice), fields, kind.arity(), t, pos))
            }
            Pat::Ref(_, x) => {
                let f = self.lower(x)?;
                Some(self.push(Ctor::Ref, vec![(0, f)], 1, t, pos))
            }
            Pat::Or(alts) => {
                // nested or-patterns flattened (rustc expand_or_pat)
                let mut flat = Vec::new();
                let mut todo: Vec<PatId> = ast.list(alts).iter().rev().copied().collect();
                while let Some(x) = todo.pop() {
                    match ast.pat(x) {
                        Pat::Or(v) => todo.extend(ast.list(*v).iter().rev().copied()),
                        Pat::Paren(y) if matches!(ast.pat(*y), Pat::Or(_)) => todo.push(*y),
                        _ => flat.push(x),
                    }
                }
                let mut fields = Vec::new();
                for (i, x) in flat.into_iter().enumerate() {
                    fields.push((i, self.lower(x)?));
                }
                let arity = fields.len();
                Some(self.push(Ctor::Or, fields, arity, t, pos))
            }
            Pat::Mac(_) => None,
        }
    }

    /// The fields of a tuple / tuple-struct pattern with an optional `..`.
    fn seq_fields(&mut self, subs: &[PatId], arity: usize) -> Option<Vec<(usize, PIx)>> {
        let ast = self.ast();
        let rest = subs.iter().position(|x| matches!(ast.pat(*x), Pat::Rest));
        let n = subs.len();
        let mut fields = Vec::new();
        for (i, x) in subs.iter().enumerate() {
            if Some(i) == rest {
                continue;
            }
            let idx = match rest {
                Some(r) if i > r => (arity + i).checked_sub(n)?,
                _ => i,
            };
            if idx >= arity {
                return None;
            }
            fields.push((idx, self.lower(*x)?));
        }
        Some(fields)
    }

    fn field_index(&self, adt: DefId, vi: u32, name: Ident) -> Option<usize> {
        let pf = &self.prog.files[self.body.file as usize];
        let text = std::str::from_utf8(pf.bytes(name.lo, name.hi)).ok()?;
        let text = text.strip_prefix("r#").unwrap_or(text);
        let sym = if text.as_bytes().first().is_some_and(|c| c.is_ascii_digit()) {
            crate::ast::tuple_index(text)? | 0x8000_0000
        } else {
            self.prog.syms.get(text)?
        };
        self.tcx.adts.get(&adt)?.variants.get(vi as usize)?.fields.iter().position(|f| f.name == sym)
    }

    /// The constructor a struct / variant definition makes of the ADT type `t`.
    fn adt_ctor(&self, t: TyId, d: DefId) -> Option<(Ctor, usize)> {
        let TyKind::Adt(ad, _, _) = self.kind(t) else { return None };
        let a = self.tcx.adts.get(&ad)?;
        if a.is_enum {
            let def = self.prog.def(d);
            if def.kind != DefKind::Variant {
                return None;
            }
            let v = a.variants.get(def.sub as usize)?;
            Some((Ctor::Variant(def.sub), v.fields.len()))
        } else {
            let n = a.variants.first().map_or(0, |v| v.fields.len());
            Some((if a.is_union { Ctor::UnionField } else { Ctor::Struct }, n))
        }
    }

    /// A path pattern: a unit variant / struct, or a constant (rustc const_to_pat for the
    /// scalar types; other constants are opaque).
    fn lower_path_res(&mut self, p: PatId, d: DefId, t: TyId, pos: Option<u32>) -> Option<PIx> {
        match self.prog.def(d).kind {
            DefKind::Variant | DefKind::Struct => {
                let (ctor, arity) = self.adt_ctor(t, d)?;
                Some(self.push(ctor, Vec::new(), arity, t, pos))
            }
            DefKind::Const | DefKind::AssocConst => match self.const_value(d, p.0 | PAT_KEY, t) {
                Some(v) => match self.valtree_to_pat(&v, t, pos, 0) {
                    Some(x) => Some(x),
                    None => Some(self.opaque(t, pos)),
                },
                None => Some(self.opaque(t, pos)),
            },
            _ => None,
        }
    }

    /// The value bits of a scalar constant `d` (resolution args under `key`) of type `t`.
    fn const_bits(&self, d: DefId, key: u32, t: TyId) -> Option<u128> {
        if !matches!(self.kind(t), TyKind::Bool | TyKind::Char | TyKind::Int(_)) {
            return None;
        }
        let leaf = self.const_value(d, key, t)?.try_to_leaf()?;
        Some(leaf.to_bits(leaf.size()))
    }

    /// The value of constant `d` (resolution args under `key`) of type `t`: only for a
    /// constant declared with that type (no alias for the evaluator to normalize).
    fn const_value(&self, d: DefId, key: u32, t: TyId) -> Option<crate::consts::ValTree> {
        let declared = self.tcx.type_of(self.prog, d).ok()?;
        if self.tcx.tys.flags(declared).intersects(TypeFlags::HAS_ALIAS) || self.tcx.tys.flags(t).intersects(TypeFlags::HAS_ALIAS) || !self.valtree_type(t, 0) {
            return None;
        }
        let args: Vec<crate::consts::GenericArg> = match self.body.res_args.get(&key) {
            Some(l) => self.tcx.tys.list(*l).iter().map(|x| match self.kind(*x) {
                TyKind::Const(c) => crate::consts::GenericArg::Const(c),
                _ => crate::consts::GenericArg::Type(*x),
            }).collect(),
            None => Vec::new(),
        };
        for a in &args {
            let infer = match a {
                crate::consts::GenericArg::Type(x) => self.tcx.tys.has_infer(*x),
                crate::consts::GenericArg::Const(c) => c.has_non_region_infer(&self.tcx.tys),
                crate::consts::GenericArg::Lifetime(_) => false,
            };
            if infer {
                return None;
            }
        }
        let env = TypingEnv { mode: crate::tcx::TypingMode::PostAnalysis, ..self.env };
        self.tcx.const_eval_resolve(self.prog, env, d, &args).ok()
    }

    /// Whether a value of type `t` has a value tree (valtrees.rs: no raw pointers, unions,
    /// trait objects, ..): a constant of another type is opaque to the analysis, never
    /// evaluated for it (a failed evaluation would be reported as E0080).
    fn valtree_type(&self, t: TyId, depth: u32) -> bool {
        if depth > 16 {
            return false;
        }
        match self.kind(t) {
            TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Str => true,
            TyKind::Ref(_, i, _) => self.valtree_type(i, depth + 1),
            TyKind::Array(e, _) | TyKind::Slice(e) => self.valtree_type(e, depth + 1),
            TyKind::Tuple(l) => self.tcx.tys.list(l).iter().all(|x| self.valtree_type(*x, depth + 1)),
            TyKind::Adt(d, args, _) => {
                let Some(a) = self.tcx.adts.get(&d) else { return false };
                use crate::tcx::AdtFlags;
                if a.is_union || [AdtFlags::IS_MANUALLY_DROP, AdtFlags::IS_UNSAFE_CELL, AdtFlags::IS_UNSAFE_PINNED, AdtFlags::IS_BOX].iter().any(|f| a.flags.contains(*f)) {
                    return false;
                }
                (0..a.variants.len()).all(|vi| self.variant_field_tys(d, vi as u32, args).into_iter().all(|ft| self.valtree_type(ft, depth + 1)))
            }
            _ => false,
        }
    }

    /// A constant's value as a pattern (const_to_pat.rs valtree_to_pat); None: a value the
    /// analysis cannot take apart (the pattern is opaque).
    fn valtree_to_pat(&mut self, v: &crate::consts::ValTree, t: TyId, pos: Option<u32>, depth: u32) -> Option<PIx> {
        if depth > 64 {
            return None;
        }
        let child = |cx: &Self, c: &crate::consts::Const| c.try_to_value(&cx.tcx.tys);
        match self.kind(t) {
            TyKind::Bool | TyKind::Char | TyKind::Int(_) => {
                let leaf = v.try_to_leaf()?;
                let c = self.scalar_ctor(t, leaf.to_bits(leaf.size()))?;
                Some(self.push(c, Vec::new(), 0, t, pos))
            }
            TyKind::Ref(_, inner, _) => {
                let inner = self.reveal(inner);
                let f = self.valtree_to_pat(v, inner, pos, depth + 1)?;
                Some(self.push(Ctor::Ref, vec![(0, f)], 1, t, pos))
            }
            TyKind::Str => {
                let bytes: Vec<u8> = v.try_to_branch()?.iter().map(|c| {
                    let l = child(self, c)?.valtree.try_to_leaf()?;
                    Some(l.to_bits(l.size()) as u8)
                }).collect::<Option<_>>()?;
                Some(self.push(Ctor::Str(String::from_utf8(bytes).ok()?), Vec::new(), 0, t, pos))
            }
            TyKind::Tuple(_) | TyKind::Adt(..) | TyKind::Array(..) | TyKind::Slice(_) => {
                let branches: Vec<crate::consts::Value> = v.try_to_branch()?.iter().map(|c| child(self, c)).collect::<Option<_>>()?;
                let (ctor, arity, fields): (Ctor, usize, &[crate::consts::Value]) = match self.kind(t) {
                    TyKind::Tuple(l) => (Ctor::Struct, self.tcx.tys.list(l).len(), &branches[..]),
                    TyKind::Adt(ad, _, _) => {
                        let a = self.tcx.adts.get(&ad)?;
                        if a.is_union {
                            return None;
                        }
                        if a.is_enum {
                            let (first, rest) = branches.split_first()?;
                            let l = first.valtree.try_to_leaf()?;
                            let vi = l.to_bits(l.size()) as u32;
                            (Ctor::Variant(vi), a.variants.get(vi as usize)?.fields.len(), rest)
                        } else {
                            (Ctor::Struct, a.variants.first()?.fields.len(), &branches[..])
                        }
                    }
                    _ => {
                        let alen = match self.kind(t) {
                            TyKind::Array(_, n) => Some(n.try_to_target_usize(&self.tcx.tys)? as usize),
                            _ => None,
                        };
                        (Ctor::Slice(Slice::new(alen, SliceKind::FixedLen(branches.len()))?), branches.len(), &branches[..])
                    }
                };
                if fields.len() != arity {
                    return None;
                }
                let sub_tys = self.ctor_sub_tys(&ctor, t);
                let mut out = Vec::new();
                for (i, (f, (ft, _))) in fields.iter().zip(sub_tys).enumerate() {
                    out.push((i, self.valtree_to_pat(&f.valtree, ft, pos, depth + 1)?));
                }
                Some(self.push(ctor, out, arity, t, pos))
            }
            _ => None,
        }
    }

    /// The constructor of a scalar value (bits of the type's size).
    fn scalar_ctor(&self, t: TyId, bits: u128) -> Option<Ctor> {
        Some(match self.kind(t) {
            TyKind::Bool => Ctor::Bool(bits != 0),
            TyKind::Char => Ctor::IntRange(IntRange::from_singleton(MaybeInfiniteInt::new_finite_uint(bits))),
            TyKind::Int(_) => Ctor::IntRange(IntRange::from_singleton(self.int_value(t, bits)?)),
            _ => return None,
        })
    }

    /// An integer (bits of the type's size, or a negated literal's wrapped bits) as an
    /// ordered value.
    fn int_value(&self, t: TyId, bits: u128) -> Option<MaybeInfiniteInt> {
        match self.kind(t) {
            TyKind::Int(p) => {
                let (bytes, signed) = crate::types::Types::int_info(p);
                let size = bytes * 8;
                let bits = if size == 128 { bits } else { bits & ((1u128 << size) - 1) };
                Some(if signed { MaybeInfiniteInt::new_finite_int(bits, size) } else { MaybeInfiniteInt::new_finite_uint(bits) })
            }
            TyKind::Char => Some(MaybeInfiniteInt::new_finite_uint(bits)),
            _ => None,
        }
    }

    /// The bits of a range bound / literal expression of scalar type `t`: a literal, a
    /// negated literal, a constant.
    fn scalar_expr_bits(&self, e: ExprId, t: TyId) -> Option<u128> {
        let ast = self.ast();
        let pf = &self.prog.files[self.body.file as usize];
        let e = crate::lit::strip_paren(ast, e);
        let ex = ast.expr(e);
        let text = || std::str::from_utf8(pf.bytes(ex.lo, ex.hi)).ok();
        match ex.kind {
            ExprKind::Lit(LitKind::Int) => crate::const_fold::parse_int_lit(text()?),
            ExprKind::Lit(LitKind::Bool(b)) => Some(b as u128),
            ExprKind::Lit(LitKind::Char) => Some(crate::lit::unescape_str(text()?).chars().next()? as u128),
            ExprKind::Lit(LitKind::Byte) => Some(*crate::lit::unescape_bytes(text()?).first()? as u128),
            ExprKind::Unary(UnOp::Neg, x) => Some(self.scalar_expr_bits(x, t)?.wrapping_neg()),
            ExprKind::Path(_) => match self.body.res.get(&e.0).copied() {
                Some(Res::Def(d)) if matches!(self.prog.def(d).kind, DefKind::Const | DefKind::AssocConst) => self.const_bits(d, e.0, t),
                _ => None,
            },
            _ => None,
        }
    }

    fn float_expr(&self, e: ExprId) -> Option<f64> {
        let ast = self.ast();
        let pf = &self.prog.files[self.body.file as usize];
        let e = crate::lit::strip_paren(ast, e);
        let ex = ast.expr(e);
        match ex.kind {
            ExprKind::Lit(LitKind::Float | LitKind::Int) => {
                let text: String = std::str::from_utf8(pf.bytes(ex.lo, ex.hi)).ok()?.chars().filter(|c| *c != '_').collect();
                let text = text.trim_end_matches("f32").trim_end_matches("f64");
                text.parse::<f64>().ok()
            }
            ExprKind::Unary(UnOp::Neg, x) => Some(-self.float_expr(x)?),
            _ => None,
        }
    }

    fn lower_lit(&mut self, e: ExprId, t: TyId, pos: Option<u32>) -> Option<PIx> {
        let ast = self.ast();
        let pf = &self.prog.files[self.body.file as usize];
        let ex = ast.expr(crate::lit::strip_paren(ast, e));
        let ctor = match (ex.kind, self.kind(t)) {
            (ExprKind::Lit(LitKind::Str | LitKind::RawStr), TyKind::Ref(_, inner, _)) if matches!(self.kind(inner), TyKind::Str) => {
                // `&str` literals: a `&` around the `str` constant
                let s = crate::lit::unescape_str(std::str::from_utf8(pf.bytes(ex.lo, ex.hi)).ok()?);
                let f = self.push(Ctor::Str(s), Vec::new(), 0, inner, pos);
                return Some(self.push(Ctor::Ref, vec![(0, f)], 1, t, pos));
            }
            (ExprKind::Lit(LitKind::ByteStr), TyKind::Ref(_, inner, _)) => {
                // byte strings: `&` around an array / slice pattern of the bytes
                let bytes = crate::lit::unescape_bytes(std::str::from_utf8(pf.bytes(ex.lo, ex.hi)).ok()?);
                let (elem, array_len) = match self.kind(inner) {
                    TyKind::Array(el, n) => (el, Some(n.try_to_target_usize(&self.tcx.tys)? as usize)),
                    TyKind::Slice(el) => (el, None),
                    _ => return Some(self.opaque(t, pos)),
                };
                let slice = Slice::new(array_len, SliceKind::FixedLen(bytes.len()))?;
                let mut fields = Vec::new();
                for (i, b) in bytes.iter().enumerate() {
                    let c = Ctor::IntRange(IntRange::from_singleton(MaybeInfiniteInt::new_finite_uint(*b as u128)));
                    fields.push((i, self.push(c, Vec::new(), 0, elem, pos)));
                }
                let s = self.push(Ctor::Slice(slice), fields, bytes.len(), inner, pos);
                return Some(self.push(Ctor::Ref, vec![(0, s)], 1, t, pos));
            }
            (_, TyKind::Float(_)) => match self.float_expr(e) {
                Some(v) => Some(Ctor::FloatRange(v, v, RangeEnd::Included)),
                None => None,
            },
            (_, TyKind::Bool | TyKind::Char | TyKind::Int(_)) => match self.scalar_expr_bits(e, t) {
                Some(bits) => self.scalar_ctor(t, bits),
                None => None,
            },
            _ => None,
        };
        match ctor {
            Some(c) => Some(self.push(c, Vec::new(), 0, t, pos)),
            None => Some(self.opaque(t, pos)),
        }
    }

    fn lower_range(&mut self, a: Option<ExprId>, b: Option<ExprId>, incl: bool, t: TyId, pos: Option<u32>) -> Option<PIx> {
        let end = if incl { RangeEnd::Included } else { RangeEnd::Excluded };
        let ctor = match self.kind(t) {
            TyKind::Float(_) => {
                let lo = match a { Some(e) => self.float_expr(e), None => Some(f64::NEG_INFINITY) };
                let hi = match b { Some(e) => self.float_expr(e), None => Some(f64::INFINITY) };
                match (lo, hi) {
                    (Some(lo), Some(hi)) => Some(Ctor::FloatRange(lo, hi, end)),
                    _ => None,
                }
            }
            TyKind::Char | TyKind::Int(_) => {
                let lo = match a {
                    Some(e) => self.scalar_expr_bits(e, t).and_then(|v| self.int_value(t, v)),
                    None => Some(MaybeInfiniteInt::NegInfinity),
                };
                // `a..` (no end) runs to the type's end
                let (hi, end) = match b {
                    Some(e) => (self.scalar_expr_bits(e, t).and_then(|v| self.int_value(t, v)), end),
                    None => (Some(MaybeInfiniteInt::PosInfinity), RangeEnd::Excluded),
                };
                match (lo, hi) {
                    (Some(lo), Some(hi)) => IntRange::from_range(lo, hi, end).map(Ctor::IntRange),
                    _ => None,
                }
            }
            _ => None,
        };
        match ctor {
            Some(c) => Some(self.push(c, Vec::new(), 0, t, pos)),
            None => Some(self.opaque(t, pos)),
        }
    }

    // ---------------------------------------------------------------- printing

    fn ty_str(&self, t: TyId) -> String {
        crate::typeck::ty_to_string(false, self.prog, self.tcx, t)
    }

    fn is_option_or_result(&self, d: DefId) -> bool {
        let li = &self.tcx.lang_items;
        li.get(LangItem::Option) == Some(d) || li.get(LangItem::ResultOk).is_some_and(|ok| self.prog.def(ok).parent == d)
    }

    /// The bounds of a range as rustc prints them (rustc.rs print_pat_range).
    fn print_range(&self, r: &IntRange, t: TyId) -> String {
        use MaybeInfiniteInt::*;
        if matches!((r.lo, r.hi), (NegInfinity, PosInfinity)) {
            return "_".to_string();
        }
        if r.is_singleton() {
            return self.print_bound(r.lo, t).unwrap_or_default();
        }
        let lo = match self.hoist(r.lo, t) {
            PosInfinity => Finite(self.max_bits(t)),
            x => x,
        };
        let (hi, end) = match r.hi.minus_one() {
            Some(h) => (h, "..="),
            None => (r.hi, ".."),
        };
        let mut s = String::new();
        if let Finite(_) = lo {
            s.push_str(&self.print_value(lo, t));
        }
        match self.hoist(hi, t) {
            Finite(b) => {
                s.push_str(end);
                s.push_str(&self.print_value(Finite(b), t));
            }
            _ => s.push_str(".."),
        }
        s
    }

    fn int_size(&self, t: TyId) -> (u32, bool, bool) {
        match self.kind(t) {
            TyKind::Int(p) => {
                let (b, s) = crate::types::Types::int_info(p);
                (b * 8, s, matches!(p, Prim::Isize | Prim::Usize))
            }
            _ => (32, false, false),
        }
    }

    fn max_bits(&self, t: TyId) -> u128 {
        let (size, signed, _) = self.int_size(t);
        let m = if size == 128 { u128::MAX } else { (1u128 << size) - 1 };
        if signed { m >> 1 } else { m }
    }

    /// A range bound as the type's value bits (Finite), or infinite when out of the type.
    fn hoist(&self, v: MaybeInfiniteInt, t: TyId) -> MaybeInfiniteInt {
        use MaybeInfiniteInt::*;
        match v {
            Finite(_) => {
                let (size, signed, _) = self.int_size(t);
                let bits = if signed && matches!(self.kind(t), TyKind::Int(_)) { v.as_finite_int(size).unwrap() } else { v.as_finite_uint().unwrap() };
                if size < 128 && bits >> size != 0 { PosInfinity } else { Finite(bits) }
            }
            x => x,
        }
    }

    fn print_bound(&self, v: MaybeInfiniteInt, t: TyId) -> Option<String> {
        match self.hoist(v, t) {
            MaybeInfiniteInt::Finite(b) => Some(self.print_value(MaybeInfiniteInt::Finite(b), t)),
            _ => None,
        }
    }

    /// A value (its bits in the type's size) as rustc prints a typed constant: `5_u8`,
    /// `i32::MIN`, `'a'`.
    fn print_value(&self, v: MaybeInfiniteInt, t: TyId) -> String {
        let MaybeInfiniteInt::Finite(raw) = v else { return "_".to_string() };
        match self.kind(t) {
            TyKind::Char => match char::from_u32(raw as u32) {
                Some(c) => format!("{:?}", c),
                None => raw.to_string(),
            },
            TyKind::Int(p) => {
                let (size, signed, ptr) = self.int_size(t);
                let name = crate::types::Types::prim_name(p);
                let mask = if size == 128 { u128::MAX } else { (1u128 << size) - 1 };
                if signed {
                    let min = 1u128 << (size - 1);
                    if raw == min {
                        return format!("{}::MIN", if ptr { "isize" } else { name });
                    }
                    if raw == min - 1 {
                        return format!("{}::MAX", if ptr { "isize" } else { name });
                    }
                    let sv = if raw & min != 0 { (raw | !mask) as i128 } else { raw as i128 };
                    format!("{}_{}", sv, name)
                } else {
                    if raw == mask {
                        return format!("{}::MAX", name);
                    }
                    format!("{}_{}", raw, name)
                }
            }
            _ => raw.to_string(),
        }
    }

    fn would_print_as_wildcard(p: &WitnessPat) -> bool {
        matches!(p.ctor, Ctor::Wildcard | Ctor::NonExhaustive | Ctor::Hidden | Ctor::PrivateUninhabited | Ctor::Never)
            || matches!(p.ctor, Ctor::IntRange(IntRange { lo: MaybeInfiniteInt::NegInfinity, hi: MaybeInfiniteInt::PosInfinity }))
    }

    /// A witness as rustc prints it (rustc.rs print_witness_pat, print.rs).
    pub fn print_witness(&self, w: &WitnessPat) -> String {
        match &w.ctor {
            Ctor::Bool(b) => b.to_string(),
            Ctor::Str(s) => format!("{:?}", s),
            Ctor::IntRange(r) => self.print_range(r, w.ty),
            Ctor::Struct | Ctor::Variant(_) | Ctor::UnionField => {
                let fields: Vec<(String, bool)> = w.fields.iter().map(|f| (self.print_witness(f), Self::would_print_as_wildcard(f))).collect();
                let mut s = String::new();
                let (variant, is_union) = match self.kind(w.ty) {
                    TyKind::Adt(d, _, _) => {
                        let a = &self.tcx.adts[&d];
                        let vi = if let Ctor::Variant(i) = w.ctor { i } else { 0 };
                        let name = if a.is_enum {
                            if self.is_option_or_result(d) {
                                self.prog.syms.str(a.variants[vi as usize].name).to_string()
                            } else {
                                format!("{}::{}", self.prog.name(d), self.prog.syms.str(a.variants[vi as usize].name))
                            }
                        } else {
                            self.prog.name(d).to_string()
                        };
                        s.push_str(&name);
                        (a.variants.get(vi as usize), a.is_union)
                    }
                    _ => (None, false),
                };
                if let Some(v) = variant {
                    if v.shape == 2 {
                        s.push_str(" { ");
                        let mut printed = 0;
                        for (i, (p, wild)) in fields.iter().enumerate() {
                            if *wild {
                                continue;
                            }
                            if printed > 0 {
                                s.push_str(", ");
                            }
                            let f = v.fields[i].name;
                            let fname = if f & 0x8000_0000 != 0 { (f & 0x7fff_ffff).to_string() } else { self.prog.syms.str(f).to_string() };
                            s.push_str(&format!("{}: {}", fname, p));
                            printed += 1;
                        }
                        if printed < v.fields.len() && (!is_union || printed == 0) {
                            if printed > 0 {
                                s.push_str(", ");
                            }
                            s.push_str("..");
                        }
                        s.push_str(" }");
                        return s;
                    }
                }
                let num_fields = variant.map_or(fields.len(), |v| v.fields.len());
                if num_fields != 0 || variant.is_none() {
                    s.push('(');
                    s.push_str(&fields.iter().map(|f| f.0.as_str()).collect::<Vec<_>>().join(", "));
                    if matches!(self.kind(w.ty), TyKind::Tuple(_)) && num_fields == 1 {
                        s.push(',');
                    }
                    s.push(')');
                }
                s
            }
            Ctor::Ref => {
                let m = matches!(self.kind(w.ty), TyKind::Ref(true, _, _));
                format!("&{}{}", if m { "mut " } else { "" }, w.fields.first().map_or("_".to_string(), |f| self.print_witness(f)))
            }
            Ctor::Slice(slice) => {
                let (prefix_len, has_dot_dot) = match slice.kind {
                    SliceKind::FixedLen(len) => (len, false),
                    SliceKind::VarLen(prefix_len, _) => (prefix_len, true),
                };
                let (mut prefix, mut suffix) = w.fields.split_at(prefix_len.min(w.fields.len()));
                if has_dot_dot && slice.array_len.is_some() {
                    while let [rest @ .., last] = prefix {
                        if !Self::would_print_as_wildcard(last) { break; }
                        prefix = rest;
                    }
                    while let [first, rest @ ..] = suffix {
                        if !Self::would_print_as_wildcard(first) { break; }
                        suffix = rest;
                    }
                }
                let mut parts: Vec<String> = prefix.iter().map(|p| self.print_witness(p)).collect();
                if has_dot_dot {
                    parts.push("..".to_string());
                }
                parts.extend(suffix.iter().map(|p| self.print_witness(p)));
                format!("[{}]", parts.join(", "))
            }
            _ => "_".to_string(),
        }
    }

    fn joined_uncovered_patterns(&self, witnesses: &[WitnessPat]) -> String {
        const LIMIT: usize = 3;
        let s: Vec<String> = witnesses.iter().map(|w| self.print_witness(w)).collect();
        match s.len() {
            0 => String::new(),
            1 => format!("`{}`", s[0]),
            n if n - 1 < LIMIT => format!("`{}` and `{}`", s[..n - 1].join("`, `"), s[n - 1]),
            n => format!("`{}` and {} more", s[..LIMIT].join("`, `"), n - LIMIT),
        }
    }
}

// ---------------------------------------------------------------- the visitor

/// Where a `let` expression stands (check_match.rs LetSource).
#[derive(Clone, Copy, PartialEq, Eq)]
enum LetSource {
    IfLet,
    WhileLet,
    /// in a `&&` chain or elsewhere: never linted as irrefutable
    Chain,
}

struct Checker<'a> {
    cx: Cx<'a>,
    errors: Vec<String>,
}

/// Checks the matches, lets and parameters of a type-checked body: the errors (E0004 /
/// E0005, located); lints go to the lint sink.
pub fn check_body(prog: &Program, tcx: &Tcx, body: &Body) -> Vec<String> {
    let d = prog.def(body.def);
    let env = TypingEnv { limit: crate::tcx::RecursionLimit::for_item(prog, body.def), param_env: body.param_env.unwrap_or_default(), mode: crate::tcx::TypingMode::non_body_analysis() };
    let cx = Cx { prog, tcx, body, env, module: d.scope, krate: d.krate, pats: Vec::new(), next_opaque: 0, inhabited: RefCell::new(HashMap::new()) };
    let mut ck = Checker { cx, errors: Vec::new() };
    ck.run();
    ck.errors
}

impl<'a> Checker<'a> {
    fn ast(&self) -> &'a Ast {
        self.cx.ast()
    }

    fn run(&mut self) {
        let body = self.cx.body;
        let ast = self.ast();
        let n = body.expr_ty.len();
        let typed = |i: usize| body.expr_typed.get(i);
        // the `let` expressions of `if` / `while` conditions
        let mut let_source: HashMap<u32, LetSource> = HashMap::new();
        let mut blocks: Vec<BlockId> = body.body.into_iter().collect();
        for i in 0..n {
            if !typed(i) { continue; }
            let e = body.expr_at(i);
            match ast.expr(e).kind {
                ExprKind::If(c, b, _) => {
                    blocks.push(b);
                    let c = crate::lit::strip_paren(ast, c);
                    if let ExprKind::Let(..) = ast.expr(c).kind { let_source.insert(c.0, LetSource::IfLet); }
                }
                ExprKind::While(row) => {
                    let w = ast.row(row);
                    blocks.push(w.body);
                    let c = crate::lit::strip_paren(ast, w.cond);
                    if let ExprKind::Let(..) = ast.expr(c).kind { let_source.insert(c.0, LetSource::WhileLet); }
                }
                ExprKind::Block(b, _) | ExprKind::Unsafe(b) | ExprKind::Async(_, b) | ExprKind::ConstBlock(b) | ExprKind::Loop(b, _) => blocks.push(b),
                ExprKind::For(row) => blocks.push(ast.row(row).body),
                _ => {}
            }
        }
        let fn_arg = if self.cx.prog.def(body.def).kind == DefKind::Closure { "closure argument" } else { "function argument" };
        let item_lo = ast.item(self.cx.prog.def(body.def).item).lo;
        for p in body.param_pats.iter().copied() {
            self.check_binding_is_irrefutable(p, fn_arg, None, Some(item_lo));
        }
        for b in blocks {
            for s in ast.stmts(b) {
                let Stmt::Let(row) = s else { continue };
                let l = *ast.row(*row);
                match l.else_ {
                    None => self.check_binding_is_irrefutable(l.pat, "local binding", l.init, l.init.map(|e| ast.expr(e).lo)),
                    Some(_) => {
                        if let Some(true) = self.is_let_irrefutable(l.pat, l.init) {
                            let pos = self.cx.pat_pos(l.pat).or(l.init.map(|e| ast.expr(e).lo));
                            if let Some(pos) = pos {
                                self.lint(&crate::lint::IRREFUTABLE_LET_PATTERNS, pos, "unreachable `else` clause -- this pattern always matches, so the else clause is unreachable");
                            }
                        }
                    }
                }
            }
        }
        for i in 0..n {
            if !typed(i) { continue; }
            let e = body.expr_at(i);
            let ex = ast.expr(e);
            match ex.kind {
                ExprKind::Match(scrut, arms) => {
                    let arms: Vec<(PatId, bool, u32, List<Attr>)> = ast.list(arms).iter().map(|a| (a.pat, a.guard.is_some(), ast.expr(a.body).lo, a.attrs)).collect();
                    self.check_match(scrut, &arms, false);
                }
                ExprKind::Let(pat, init) => {
                    let source = let_source.get(&e.0).copied().unwrap_or(LetSource::Chain);
                    if let Some(true) = self.is_let_irrefutable(pat, Some(init)) {
                        let msg = match source {
                            LetSource::IfLet => Some("irrefutable `if let` pattern -- this pattern will always match, so the `if let` is useless"),
                            LetSource::WhileLet => Some("irrefutable `while let` pattern -- this pattern will always match, so the loop will never exit"),
                            LetSource::Chain => None,
                        };
                        if let Some(msg) = msg {
                            self.lint(&crate::lint::IRREFUTABLE_LET_PATTERNS, ex.lo, msg);
                        }
                    }
                }
                ExprKind::For(row) => {
                    let f = ast.row(row);
                    self.check_binding_is_irrefutable(f.pat, "`for` loop binding", None, Some(ast.expr(f.iter).lo));
                }
                ExprKind::Closure(row) => {
                    for cp in ast.list(ast.row(row).params) {
                        self.check_binding_is_irrefutable(cp.pat, "closure argument", None, Some(ex.lo));
                    }
                }
                ExprKind::Mac(m) => {
                    let mc = ast.row(m);
                    if mc.args == NO_MAC_ARGS { continue; }
                    let args = ast.mac_arg(mc.args);
                    // `matches!(e, pat if guard)`: `match e { pat if guard => true, _ => false }`
                    if let (Some(pat), Some(&scrut)) = (args.pat, ast.list(args.exprs).first()) {
                        self.check_match(scrut, &[(pat, args.guard.is_some(), ex.lo, List::default())], true);
                    }
                }
                _ => {}
            }
        }
    }

    /// Whether `pos` lies in the expansion of a macro defined in another file (lints from
    /// external macros are not reported).
    fn in_external_macro(&self, pos: u32) -> bool {
        let pf = &self.cx.prog.files[self.cx.body.file as usize];
        if pos < pf.src_len {
            return false;
        }
        pf.hyg_ctxt(pos).iter().any(|m| pf.hyg_def_pos(*m) == u32::MAX)
    }

    fn lint(&self, lint: &crate::lint::Lint, pos: u32, msg: &str) {
        self.lint_in(lint, &[], pos, msg);
    }

    fn lint_in(&self, lint: &crate::lint::Lint, attrs: &[Attr], pos: u32, msg: &str) {
        if self.in_external_macro(pos) {
            return;
        }
        crate::lint::emit_in(self.cx.prog, self.cx.body.def, attrs, lint, self.cx.body.file, pos, msg);
    }

    /// check_match.rs is_known_valid_scrutinee: a place reached through a dereference (or a
    /// union field) may hold an invalid value.
    fn is_known_valid_scrutinee(&self, e: ExprId) -> bool {
        let ast = self.ast();
        let body = self.cx.body;
        match ast.expr(e).kind {
            ExprKind::Paren(x) | ExprKind::Cast(x, _) => self.is_known_valid_scrutinee(x),
            ExprKind::Unary(UnOp::Deref, _) => false,
            ExprKind::Field(lhs, _) | ExprKind::TupleField(lhs, _) => {
                if !body.has_expr(lhs) { return true; }
                match self.cx.kind(body.ty(lhs)) {
                    TyKind::Adt(d, _, _) => !self.cx.tcx.adts.get(&d).is_some_and(|a| a.is_union) && self.is_known_valid_scrutinee(lhs),
                    TyKind::Tuple(_) => self.is_known_valid_scrutinee(lhs),
                    // an autoderef
                    _ => false,
                }
            }
            ExprKind::Index(lhs, _) => {
                if body.ov_index.get(&e.0).is_some() || !body.has_expr(lhs) { return false; }
                match self.cx.kind(body.ty(lhs)) {
                    TyKind::Array(..) | TyKind::Slice(_) => self.is_known_valid_scrutinee(lhs),
                    _ => false,
                }
            }
            _ => true,
        }
    }

    fn scrut_ty(&self, e: ExprId) -> Option<TyId> {
        let body = self.cx.body;
        if !body.has_expr(e) { return None; }
        let i = body.expr_ix(e);
        if !body.expr_typed.get(i) { return None; }
        let t = body.expr_ty[i];
        let f = self.cx.tcx.tys.flags(t);
        if f.intersects(TypeFlags::HAS_ERROR) || f.intersects(TypeFlags::HAS_NON_REGION_INFER) { return None; }
        Some(self.cx.reveal(t))
    }

    /// arms: (pattern, has guard, fallback position, attributes)
    fn check_match(&mut self, scrut: ExprId, arms: &[(PatId, bool, u32, List<Attr>)], matches_macro: bool) {
        let ast = self.ast();
        let Some(scrut_ty) = self.scrut_ty(scrut) else { return };
        let mut tarms = Vec::new();
        for (p, g, _, _) in arms {
            let Some(pat) = self.cx.lower(*p) else { return };
            tarms.push(MatchArm { pat, has_guard: *g });
        }
        if matches_macro {
            let w = self.cx.push(Ctor::Wildcard, Vec::new(), 0, scrut_ty, None);
            tarms.push(MatchArm { pat: w, has_guard: false });
        }
        let validity = if self.is_known_valid_scrutinee(scrut) { PlaceValidity::ValidOnly } else { PlaceValidity::MaybeInvalid };
        let Some(report) = usefulness::compute_match_usefulness(&self.cx, &tarms, scrut_ty, validity) else { return };
        for (i, u) in report.arm_usefulness.iter().enumerate() {
            let Some(&(_, _, fallback, attrs)) = arms.get(i) else { continue };
            let attrs = ast.list(attrs);
            match u {
                Usefulness::Redundant(covered_by) => self.report_unreachable(tarms[i].pat, covered_by, fallback, attrs),
                Usefulness::Useful(subs) => {
                    for (p, covered_by) in subs {
                        self.report_unreachable(*p, covered_by, fallback, attrs);
                    }
                }
            }
        }
        let witnesses = report.non_exhaustiveness_witnesses;
        if witnesses.is_empty() {
            return;
        }
        let pos = ast.expr(scrut).lo;
        let ty = self.cx.ty_str(scrut_ty);
        let non_empty_enum = match self.cx.kind(scrut_ty) {
            TyKind::Adt(d, _, _) => self.cx.tcx.adts.get(&d).is_some_and(|a| a.is_enum && !a.variants.is_empty()),
            _ => false,
        };
        let msg = if arms.is_empty() && !non_empty_enum {
            format!("E0004: non-exhaustive patterns: type `{}` is non-empty -- the matched value is of type `{}`", ty, ty)
        } else {
            let joined = self.cx.joined_uncovered_patterns(&witnesses);
            let mut notes = format!("the matched value is of type `{}`", ty);
            let mut special = Vec::new();
            collect_special_tys(&self.cx, &witnesses[0], &mut special);
            for t in special {
                let s = self.cx.ty_str(t);
                match self.cx.kind(t) {
                    TyKind::Int(Prim::Usize) => notes.push_str(&format!("; `{}::MAX` is not treated as exhaustive, so half-open ranges are necessary to match exhaustively", s)),
                    TyKind::Int(Prim::Isize) => notes.push_str(&format!("; `{}::MIN` and `{}::MAX` are not treated as exhaustive, so half-open ranges are necessary to match exhaustively", s, s)),
                    TyKind::Str => notes.push_str("; `&str` cannot be matched exhaustively, so a wildcard `_` is necessary"),
                    _ if self.cx.is_foreign_non_exhaustive_enum(t) => notes.push_str(&format!("; `{}` is marked as non-exhaustive, so a wildcard `_` is necessary to match exhaustively", s)),
                    _ if self.cx.is_uninhabited(t) => notes.push_str(&format!("; `{}` is uninhabited but is not being matched by value, so a wildcard `_` is required", s)),
                    _ => {}
                }
            }
            format!("E0004: non-exhaustive patterns: {} not covered -- {}; ensure that all possible cases are being handled by adding a match arm with a wildcard pattern or an explicit pattern", joined, notes)
        };
        self.errors.push(self.cx.prog.record_error(self.cx.body.file, pos, "typeck", msg));
    }

    fn report_unreachable(&self, p: PIx, covered_by: &[PIx], fallback: u32, attrs: &[Attr]) {
        let pat = self.cx.pat(p);
        let pos = pat.pos.unwrap_or(fallback);
        let note = match covered_by {
            [] => format!("matches no values because `{}` is uninhabited", self.cx.ty_str(pat.ty)),
            [c] if pat_is_catchall(&self.cx, *c) => "matches any value".to_string(),
            [_] => "matches all the relevant values".to_string(),
            _ => "multiple earlier patterns match some of the same values".to_string(),
        };
        self.lint_in(&crate::lint::UNREACHABLE_PATTERNS, attrs, pos, &format!("unreachable pattern -- {}", note));
    }

    /// A refutable binding's analysis: (witnesses), None when it is not analysed.
    fn analyze_binding(&mut self, p: PatId, scrut: Option<ExprId>, fallback: u32) -> Option<(PIx, usefulness::UsefulnessReport)> {
        let pat = self.cx.lower(p)?;
        let ty = self.cx.pat(pat).ty;
        let validity = match scrut {
            Some(e) if !self.is_known_valid_scrutinee(e) => PlaceValidity::MaybeInvalid,
            _ => PlaceValidity::ValidOnly,
        };
        let report = usefulness::compute_match_usefulness(&self.cx, &[MatchArm { pat, has_guard: false }], ty, validity)?;
        // (check_match.rs analyze_patterns: the redundant parts of a useful pattern)
        if let Usefulness::Useful(subs) = &report.arm_usefulness[0] {
            for (sp, covered_by) in subs {
                self.report_unreachable(*sp, covered_by, fallback, &[]);
            }
        }
        Some((pat, report))
    }

    /// check_match.rs report_arm_reachability for a `let`: the whole pattern matching nothing.
    fn report_arm_reachability(&self, pat: PIx, report: &usefulness::UsefulnessReport, fallback: u32) {
        if let Usefulness::Redundant(covered_by) = &report.arm_usefulness[0] {
            self.report_unreachable(pat, covered_by, fallback, &[]);
        }
    }

    /// Some(true): irrefutable; reports the pattern's unreachable parts.
    fn is_let_irrefutable(&mut self, p: PatId, scrut: Option<ExprId>) -> Option<bool> {
        let fallback = self.cx.pat_pos(p).or(scrut.map(|e| self.ast().expr(e).lo))?;
        let (pat, report) = self.analyze_binding(p, scrut, fallback)?;
        self.report_arm_reachability(pat, &report, fallback);
        Some(report.non_exhaustiveness_witnesses.is_empty())
    }

    fn check_binding_is_irrefutable(&mut self, p: PatId, origin: &str, scrut: Option<ExprId>, fallback: Option<u32>) {
        let Some(pos) = self.cx.pat_pos(p).or(fallback) else { return };
        let Some((pat, report)) = self.analyze_binding(p, scrut, pos) else { return };
        let witnesses = &report.non_exhaustiveness_witnesses;
        if witnesses.is_empty() {
            return;
        }
        let ty = self.cx.ty_str(self.cx.pat(pat).ty);
        let joined = self.cx.joined_uncovered_patterns(witnesses);
        let msg = format!("E0005: refutable pattern in {} -- pattern{} {} not covered; the matched value is of type `{}`", origin, if witnesses.len() == 1 { "" } else { "s" }, joined, ty);
        self.errors.push(self.cx.prog.record_error(self.cx.body.file, pos, "typeck", msg));
    }
}

/// A pattern that matches every value (check_match.rs pat_is_catchall).
fn pat_is_catchall(cx: &Cx, p: PIx) -> bool {
    let p = cx.pat(p);
    match p.ctor {
        Ctor::Wildcard => true,
        Ctor::Struct | Ctor::Ref => p.fields.iter().all(|(_, f)| pat_is_catchall(cx, *f)),
        _ => false,
    }
}

/// The types of a witness that need a note (check_match.rs collect_special_tys).
fn collect_special_tys(cx: &Cx, w: &WitnessPat, out: &mut Vec<TyId>) {
    let special = match &w.ctor {
        Ctor::NonExhaustive | Ctor::Never => true,
        Ctor::IntRange(r) => matches!(cx.kind(w.ty), TyKind::Int(Prim::Isize | Prim::Usize))
            && (matches!(cx.hoist(r.lo, w.ty), MaybeInfiniteInt::PosInfinity) || matches!(r.hi, MaybeInfiniteInt::Finite(0))),
        _ => false,
    };
    if special && !out.contains(&w.ty) {
        out.push(w.ty);
    }
    for f in &w.fields {
        collect_special_tys(cx, f, out);
    }
}

#[cfg(test)]
mod tests;
