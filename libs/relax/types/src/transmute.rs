// ported from N:rustc_hir_typeck/src/intrinsicck.rs and rustc_middle/src/ty/layout.rs (SizeSkeleton)
//! `transmute::<T, U>` needs both types to have the same size, judged from their layouts or,
//! for generic types, their size skeletons (E0512).
use crate::consts::GenericArg;
use crate::layout::{LayoutCx, Layouts};
use crate::next_solver::Goal;
use crate::program::{DefId, DefKind, Program};
use crate::tcx::{Tcx, TypingEnv};
use crate::types::{AliasTyKind, TyId, TyKind};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Skeleton {
    Known(u32, u32),
    /// a potentially wide pointer whose metadata depends on `tail`
    Pointer { non_zero: bool, tail: TyId },
}

struct Cx<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    env: TypingEnv,
    lay: Layouts,
    /// a type the layout code cannot take apart was met: nothing is judged
    gave_up: std::cell::Cell<bool>,
}

/// Whether `d` is `mem::transmute` / `intrinsics::transmute` of the standard library.
pub fn is_transmute(prog: &Program, d: DefId) -> bool {
    let def = prog.def(d);
    matches!(def.kind, DefKind::Fn | DefKind::ForeignFn) && prog.name(d) == "transmute"
        && matches!(prog.syms.str(prog.crates[def.krate as usize].name), "core" | "std")
}

/// check_transmute: the E0512 message when the sizes of `from` and `to` differ or depend on a parameter.
pub fn check_transmute(prog: &Program, tcx: &Tcx, env: TypingEnv, from: TyId, to: TyId) -> Option<String> {
    // (a type that does not normalize becomes an error type, which has no layout: E0512)
    let norm = |t: TyId| tcx.try_normalize_erasing_regions(prog, env, t).ok();
    let (Some(from), Some(to)) = (norm(from), norm(to)) else {
        return Some("E0512: cannot transmute between types of different sizes, or dependently-sized types".to_string());
    };
    for t in [from, to] {
        // (what the layout code cannot judge is not judged)
        if tcx.tys.has_error(t) || tcx.tys.has_infer(t) || tcx.tys.has_opaque(t) { return None; }
    }
    if from == to { return None; }
    let cx = Cx { prog, tcx, env, lay: Layouts::erased_generics(), gave_up: Default::default() };
    let (a, b) = (cx.skeleton(from, 0), cx.skeleton(to, 0));
    if cx.gave_up.get() { return None; }
    if let (Some(a), Some(b)) = (a, b) {
        // same_size
        let same = match (a, b) {
            (Skeleton::Known(x, _), Skeleton::Known(y, _)) => x == y,
            (Skeleton::Pointer { tail: x, .. }, Skeleton::Pointer { tail: y, .. }) => x == y,
            _ => false,
        };
        if same { return None; }
    }
    Some("E0512: cannot transmute between types of different sizes, or dependently-sized types".to_string())
}

impl Cx<'_> {
    fn sized(&self, t: TyId) -> bool {
        let Some(sized) = self.tcx.lang_items.get(crate::lang_items::LangItem::Sized) else { return true };
        let infcx = crate::infer::InferCtxt::new(self.tcx, crate::tcx::TypingMode::non_body_analysis(), self.env.limit);
        let goal = Goal { param_env: self.env.param_env, predicate: crate::next_solver::collect::trait_predicate(sized, vec![GenericArg::Type(t)]) };
        infcx.evaluate_obligation(self.prog, goal).must_apply_modulo_regions()
    }

    /// The last field's type of a struct, repeatedly (rustc struct_tail).
    fn tail(&self, mut t: TyId) -> TyId {
        for _ in 0..64 {
            match self.tcx.tys.kind(t).clone() {
                TyKind::Adt(d, args, _) => {
                    let Some(a) = self.tcx.adts.get(&d) else { return t };
                    if a.is_enum || a.is_union { return t; }
                    let Some(f) = a.variants.first().and_then(|v| v.fields.last()) else { return t };
                    let ft = self.tcx.tys.subst(f.ty, self.tcx.tys.list(args));
                    t = self.tcx.try_normalize_erasing_regions(self.prog, self.env, ft).unwrap_or(ft);
                }
                TyKind::Tuple(l) => match self.tcx.tys.list(l).last() { Some(&x) => t = x, None => return t },
                _ => return t,
            }
        }
        t
    }

    /// Whether the layout of `t` depends on a type parameter (rustc layout_of: TooGeneric).
    fn too_generic(&self, t: TyId, depth: u32) -> bool {
        if depth > 24 { return false; }
        match self.tcx.tys.kind(t).clone() {
            TyKind::Param(_) | TyKind::Placeholder(_) => true,
            TyKind::Alias(..) => true,
            TyKind::Ref(_, p, _) | TyKind::Ptr(_, p) => {
                let tail = self.tail(p);
                match self.tcx.tys.kind(tail) {
                    TyKind::Param(_) | TyKind::Alias(..) => !self.sized(tail),
                    _ => false,
                }
            }
            TyKind::Array(e, n) => matches!(self.tcx.tys.const_kind(n), crate::consts::ConstKind::Param(_) | crate::consts::ConstKind::Alias(..)) || self.too_generic(e, depth + 1),
            TyKind::Tuple(l) => self.tcx.tys.list(l).iter().any(|&x| self.too_generic(x, depth + 1)),
            TyKind::Adt(d, args, _) => {
                let Some(a) = self.tcx.adts.get(&d) else { return false };
                let args = self.tcx.tys.list(args).to_vec();
                a.variants.iter().any(|v| v.fields.iter().any(|f| {
                    let ft = self.tcx.tys.subst(f.ty, &args);
                    let ft = self.tcx.try_normalize_erasing_regions(self.prog, self.env, ft).unwrap_or(ft);
                    self.too_generic(ft, depth + 1)
                }))
            }
            _ => false,
        }
    }

    /// No field type of `t` (substituted) still holds a projection.
    fn fields_clean(&self, t: TyId, depth: u32) -> bool {
        if depth > 12 { return true; }
        match self.tcx.tys.kind(t).clone() {
            TyKind::Adt(d, args, _) => {
                let Some(a) = self.tcx.adts.get(&d) else { return true };
                let args = self.tcx.tys.list(args).to_vec();
                a.variants.iter().all(|v| v.fields.iter().all(|f| {
                    let ft = self.tcx.tys.subst(f.ty, &args);
                    // (an ADT without fields, `PhantomData<<U as Tr>::Assoc>`, has the same layout whatever its arguments)
                    let no_data = matches!(self.tcx.tys.kind(ft), TyKind::Adt(fd, ..) if self.tcx.adts.get(fd).is_some_and(|fa| fa.variants.iter().all(|fv| fv.fields.is_empty())));
                    (no_data || !self.tcx.tys.flags(ft).intersects(crate::types::TypeFlags::HAS_ALIAS)) && self.fields_clean(ft, depth + 1)
                }))
            }
            TyKind::Tuple(l) => self.tcx.tys.list(l).iter().all(|&x| self.fields_clean(x, depth + 1)),
            TyKind::Array(e, _) | TyKind::Slice(e) => self.fields_clean(e, depth + 1),
            _ => true,
        }
    }

    fn erase_generics(&self, t: TyId) -> TyId {
        let tys = &self.tcx.tys;
        let unit = tys.unit;
        match tys.kind(t).clone() {
            TyKind::Param(_) | TyKind::Alias(..) => unit,
            TyKind::Adt(d, args, r) => { let a: Vec<TyId> = tys.list(args).to_vec().into_iter().map(|x| self.erase_generics(x)).collect(); tys.intern(TyKind::Adt(d, tys.mk_list(&a), r)) }
            TyKind::Ref(m, p, r) => tys.intern(TyKind::Ref(m, self.erase_generics(p), r)),
            TyKind::Ptr(m, p) => tys.intern(TyKind::Ptr(m, self.erase_generics(p))),
            TyKind::Slice(e) => tys.intern(TyKind::Slice(self.erase_generics(e))),
            TyKind::Array(e, n) => tys.intern(TyKind::Array(self.erase_generics(e), n)),
            TyKind::Tuple(l) => { let a: Vec<TyId> = tys.list(l).to_vec().into_iter().map(|x| self.erase_generics(x)).collect(); tys.intern(TyKind::Tuple(tys.mk_list(&a))) }
            _ => t,
        }
    }

    fn skeleton(&self, t: TyId, depth: u32) -> Option<Skeleton> {
        if depth > 32 { return None; }
        if !self.too_generic(t, 0) {
            let lcx = LayoutCx::new(self.prog);
            // (parameters and projections are only behind thin pointers or in PhantomData here:
            // the layout is computed with each of them as `()`)
            let t = self.erase_generics(t);
            if !self.fields_clean(t, 0) { self.gave_up.set(true); return None; }
            if self.lay.is_unsized(&lcx, self.tcx, t) { return None; }
            let l = self.lay.of(&lcx, self.tcx, t);
            return Some(Skeleton::Known(l.size, l.align));
        }
        match self.tcx.tys.kind(t).clone() {
            TyKind::Ref(_, p, _) | TyKind::Ptr(_, p) => {
                let non_zero = matches!(self.tcx.tys.kind(t), TyKind::Ref(..));
                let tail = self.tail(p);
                match self.tcx.tys.kind(tail) {
                    TyKind::Param(_) | TyKind::Alias(_, crate::types::AliasTy { kind: AliasTyKind::Projection { .. } | AliasTyKind::Inherent { .. }, .. }) =>
                        Some(Skeleton::Pointer { non_zero, tail: self.tcx.tys.erase_all_regions(tail) }),
                    _ => None,
                }
            }
            TyKind::Adt(d, args, _) => {
                // only newtypes and enums with the nullable pointer optimization
                let a = self.tcx.adts.get(&d)?;
                if a.is_union || a.variants.is_empty() || a.variants.len() > 2 { return None; }
                let args = self.tcx.tys.list(args).to_vec();
                let variant = |i: usize| -> Option<Option<Skeleton>> {
                    let mut ptr = None;
                    for f in &a.variants[i].fields {
                        let ft = self.tcx.tys.subst(f.ty, &args);
                        let ft = self.tcx.try_normalize_erasing_regions(self.prog, self.env, ft).unwrap_or(ft);
                        match self.skeleton(ft, depth + 1)? {
                            Skeleton::Known(size, align) => if !(size == 0 && align == 1) { return None; },
                            p @ Skeleton::Pointer { .. } => { if ptr.is_some() { return None; } ptr = Some(p); }
                        }
                    }
                    Some(ptr)
                };
                let v0 = variant(0)?;
                if a.variants.len() == 1 {
                    // (`#[rustc_layout_scalar_valid_range_start(1)]` is the `!null` pattern type of rustc's NonNull)
                    return match v0 { Some(Skeleton::Pointer { non_zero, tail }) => Some(Skeleton::Pointer { non_zero: non_zero || a.valid_start == Some(1), tail }), _ => None };
                }
                let v1 = variant(1)?;
                match (v0, v1) {
                    (Some(Skeleton::Pointer { non_zero: true, tail }), None) | (None, Some(Skeleton::Pointer { non_zero: true, tail })) =>
                        Some(Skeleton::Pointer { non_zero: false, tail }),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}
