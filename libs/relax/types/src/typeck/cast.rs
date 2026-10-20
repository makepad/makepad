// ported from N:rustc_hir_typeck/src/cast.rs
//! The checks of `e as T` (rustc `CastCheck`): a cast is a coercion, or one of the primitive
//! casts of the table in rustc's module docs. Run once per cast after inference fallback; it
//! only validates (the pointer/unsizing adjustments codegen needs are recorded where the cast
//! is checked, in the `Cast` expression arm and the deferred cast loop).
use super::*;
use crate::next_solver::collect::trait_predicate;
use crate::consts::GenericArg;

/// rustc `PointerKind`: the kind of metadata a pointer to `T` carries.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PointerKind {
    Thin,
    /// the (region-erased) trait object type of the pointee tail
    VTable(TyId),
    Length,
    OfAlias(TyId),
    OfParam(u32),
}

/// rustc `CastTy::Int` payloads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum IntKind { Bool, Char, CEnum, U8, Other }

/// rustc `CastTy` (a reference is not one: the cast splits into a coercion to a raw pointer).
#[derive(Clone, Copy, PartialEq, Eq)]
enum CastTy { Int(IntKind), Float, FnPtr, Ptr(bool, TyId) }

enum CastError {
    /// an error has been reported already
    Reported,
    NeedDeref,
    NeedViaPtr,
    NeedViaThinPtr,
    NeedViaInt,
    IllegalCast,
    NonScalar,
    ForeignNonExhaustiveAdt,
    CastToBool,
    CastToChar,
    SizedUnsizedCast,
    DifferingKinds,
    UnknownCastPtrKind,
    UnknownExprPtrKind,
    IntToWideCast(Option<&'static str>),
    PtrPtrAddingAutoTrait(Vec<DefId>),
}

impl Fcx<'_> {
    fn cast_ty_of(&mut self, t: TyId) -> Option<CastTy> {
        let t = self.shallow(t);
        Some(match self.tcx.tys.kind(t).clone() {
            TyKind::Int(p) => CastTy::Int(if p == Prim::U8 { IntKind::U8 } else { IntKind::Other }),
            TyKind::IntVar(_) => CastTy::Int(IntKind::Other),
            TyKind::Float(_) | TyKind::FloatVar(_) => CastTy::Float,
            TyKind::Bool => CastTy::Int(IntKind::Bool),
            TyKind::Char => CastTy::Int(IntKind::Char),
            // a C-like enum: no variant has fields (rustc `is_payloadfree`)
            TyKind::Adt(d, _, _) if self.tcx.adts.get(&d).is_some_and(|a| a.is_enum
                && !a.variants.iter().any(|v| matches!(v.discr, crate::tcx::VariantDiscr::Explicit(_)) && v.shape != 0)
                && a.variants.iter().all(|v| v.fields.is_empty())) => CastTy::Int(IntKind::CEnum),
            TyKind::Ptr(m, p) => CastTy::Ptr(m, p),
            TyKind::FnPtr(..) => CastTy::FnPtr,
            _ => return None,
        })
    }

    fn type_is_sized_modulo_regions(&mut self, ty: TyId) -> bool {
        let Some(sized) = self.tcx.lang_items.get(crate::lang_items::LangItem::Sized) else { return true };
        let goal = Goal { param_env: self.param_env.clone(), predicate: trait_predicate(sized, vec![GenericArg::Type(ty)]) };
        self.infcx.evaluate_obligation(self.prog, goal).must_apply_modulo_regions()
    }

    /// rustc `pointer_kind`: Err when the type holds an error, Ok(None) when not yet known.
    fn pointer_kind(&mut self, t: TyId) -> Result<Option<PointerKind>, CastError> {
        let t = self.deeply_resolve_ignoring_regions_with_obligations(t);
        if self.tcx.tys.has_error(t) { return Err(CastError::Reported); }
        if self.type_is_sized_modulo_regions(t) { return Ok(Some(PointerKind::Thin)); }
        let t = self.shallow(t);
        Ok(match self.tcx.tys.kind(t).clone() {
            TyKind::Slice(_) | TyKind::Str => Some(PointerKind::Length),
            TyKind::Dyn(..) => Some(PointerKind::VTable(self.tcx.tys.erase_all_regions(t))),
            TyKind::Adt(d, args, _) => {
                let tail = match self.tcx.adts.get(&d) {
                    Some(a) if !a.is_enum && !a.is_union => match a.variants.first() {
                        Some(v) => v.fields.last().map(|f| f.ty),
                        None => None,
                    },
                    _ => return Err(CastError::Reported),
                };
                match tail {
                    None => Some(PointerKind::Thin),
                    Some(f) => { let ft = self.tcx.tys.subst(f, self.tcx.tys.list(args)); self.pointer_kind(ft)? }
                }
            }
            TyKind::Tuple(fields) => match self.tcx.tys.list(fields).last() {
                None => Some(PointerKind::Thin),
                Some(&f) => self.pointer_kind(f)?,
            },
            // pointers to foreign types are thin, despite being unsized
            TyKind::Foreign(_) => Some(PointerKind::Thin),
            TyKind::Alias(..) => Some(PointerKind::OfAlias(self.tcx.tys.erase_all_regions(t))),
            TyKind::Param(p) => Some(PointerKind::OfParam(p)),
            TyKind::Infer(_) => None,
            _ => return Err(CastError::Reported),
        })
    }

    /// try_coercion_cast: does `e` coerce to the cast type? Leaves no trace.
    fn cast_is_coercion(&mut self, e: ExprId, from: TyId, to: TyId) -> bool {
        let snap = self.start_snapshot();
        let n_dd = self.deferred_dyn.len();
        let old = self.save_value_adjustments(e.0);
        let mut ok = self.coerce(from, to, Some(e), self.last_lo, false).is_some();
        if !ok {
            // the object and unsizing coercions the `Cast` arm applies on its own
            let (sf, st) = (self.shallow(from), self.shallow(to));
            let (kf, kt) = (self.tcx.tys.kind(sf).clone(), self.tcx.tys.kind(st).clone());
            ok = self.dyn_coercion(&kf, &kt).is_some() || self.dyn_upcast(&kf, &kt).is_some() || self.unsize_coercion(&kf, &kt) || self.drops_principal(&kf, &kt);
        }
        self.rollback_to(snap);
        self.deferred_dyn.truncate(n_dd);
        self.restore_value_adjustments(e.0, old);
        ok
    }

    /// `dyn Trait + Send` unsizes to `dyn Send`: the principal may be dropped, the auto traits
    /// may only shrink (a pointer or reference to either).
    fn drops_principal(&mut self, from: &TyKind, to: &TyKind) -> bool {
        let (a, d) = match (from, to) {
            (TyKind::Ref(m1, a, _), TyKind::Ref(m2, d, _)) | (TyKind::Ptr(m1, a), TyKind::Ptr(m2, d)) | (TyKind::Ref(m1, a, _), TyKind::Ptr(m2, d)) if *m1 || !*m2 => (*a, *d),
            _ => return false,
        };
        let (a, d) = (self.shallow(a), self.shallow(d));
        let (TyKind::Dyn(_, _, _, _, _, _, sautos), TyKind::Dyn(td, _, _, _, _, _, tautos)) = (self.tcx.tys.kind(a).clone(), self.tcx.tys.kind(d).clone()) else { return false };
        crate::next_solver::eval_ctxt::trait_is_auto(self.prog, td) && self.tcx.tys.list(tautos).iter().all(|t| self.tcx.tys.list(sautos).contains(t))
    }

    pub(super) fn can_eq(&mut self, a: TyId, b: TyId) -> bool {
        let snap = self.start_snapshot();
        let ok = self.infcx.at(self.prog, self.param_env.clone()).eq(a, b).is_ok();
        self.rollback_to(snap);
        ok
    }

    /// `CastCheck::check`, reporting at `lo`.
    pub(super) fn check_cast_final(&mut self, e: ExprId, expr_ty: TyId, cast_ty: TyId, lo: u32) {
        let expr_ty = self.deeply_resolve_ignoring_regions_with_obligations(expr_ty);
        let cast_ty = self.deeply_resolve_ignoring_regions_with_obligations(cast_ty);
        // rustc CastCheck::check: both types are structurally resolved first (E0282 at the
        // offending subexpression / the cast target)
        for (t, span) in [(expr_ty, self.ast().expr(e).lo), (cast_ty, lo)] {
            if matches!(self.tcx.tys.kind(self.shallow(t)), TyKind::Infer(_)) {
                if !self.infcx.tainted_by_errors() { self.err(span, "E0282: type annotations needed".into()); }
                return;
            }
        }
        if self.tcx.tys.has_error(expr_ty) || self.tcx.tys.has_error(cast_ty) { return; }
        let (es, cs) = (self.cast_ty_str(expr_ty), self.cast_ty_str(cast_ty));
        if !self.type_is_sized_modulo_regions(cast_ty) && !self.tcx.tys.has_infer(cast_ty) {
            self.err(lo, format!("E0620: cast to unsized type: `{}` as `{}`", es, cs));
            return;
        }
        if self.cast_is_coercion(e, expr_ty, cast_ty) {
            // cast.rs: a raw pointer to raw pointer cast is not recorded as a coercion
            let raw = |t: TyId| matches!(self.tcx.tys.kind(t), TyKind::Ptr(..));
            if crate::lint::enabled() && !(raw(expr_ty) && raw(cast_ty)) { self.b.coercion_casts.insert(e.0, ()); }
            return;
        }
        match self.do_cast_check(e, expr_ty, cast_ty, lo) {
            Ok(()) => {}
            Err(err) => self.report_cast_error(err, &es, &cs, lo),
        }
    }

    fn cast_ty_str(&self, t: TyId) -> String { crate::typeck::ty_to_string(false, self.prog, self.tcx, t) }

    fn report_cast_error(&mut self, err: CastError, es: &str, cs: &str, lo: u32) {
        let invalid = format!("E0606: casting `{}` as `{}` is invalid", es, cs);
        match err {
            CastError::Reported => {}
            CastError::NeedDeref | CastError::NeedViaPtr | CastError::NeedViaThinPtr | CastError::NeedViaInt | CastError::IllegalCast
            | CastError::DifferingKinds => self.err(lo, invalid),
            CastError::ForeignNonExhaustiveAdt => self.err(lo, invalid),
            CastError::CastToBool => self.err(lo, format!("E0054: cannot cast `{}` as `bool`", es)),
            CastError::CastToChar => self.err(lo, format!("E0604: only `u8` can be cast as `char`, not `{}`", es)),
            CastError::NonScalar => self.err(lo, format!("E0605: non-primitive cast: `{}` as `{}`", es, cs)),
            CastError::SizedUnsizedCast => self.err(lo, format!("E0607: cannot cast thin pointer `{}` to wide pointer `{}`", es, cs)),
            CastError::IntToWideCast(meta) => self.err(lo, format!("E0606: cannot cast `{}` to a pointer that {} wide", es, match meta { Some(m) => format!("is wide (it carries {})", m), None => "may be wide".to_string() })),
            CastError::UnknownCastPtrKind | CastError::UnknownExprPtrKind => self.err(lo, "E0641: cannot cast to a pointer of an unknown kind".to_string()),
            CastError::PtrPtrAddingAutoTrait(added) => {
                let mut names: Vec<String> = added.iter().map(|d| self.prog.def_path(*d)).collect();
                names.sort();
                self.err(lo, format!("E0804: cannot add {} to dyn bound via pointer cast", names.join(", ")));
            }
        }
    }

    fn do_cast_check(&mut self, e: ExprId, expr_ty: TyId, cast_ty: TyId, lo: u32) -> Result<(), CastError> {
        use CastTy::*;
        let (t_from, t_cast) = match (self.cast_ty_of(expr_ty), self.cast_ty_of(cast_ty)) {
            (Some(f), Some(c)) => (f, c),
            // function item types may need to be reified before casts
            (None, Some(c)) => match self.tcx.tys.kind(expr_ty).clone() {
                TyKind::FnDef(d, _, _) => {
                    if self.tcx.fn_item_sig(self.prog, d).is_some_and(|s| s.header.abi == Abi::RelaxIntrinsic) { return Err(CastError::IllegalCast); }
                    match self.fn_ptr_of(expr_ty) {
                        Some(fp) if self.cast_is_coercion(e, expr_ty, fp) => (FnPtr, c),
                        _ => return Err(CastError::NonScalar),
                    }
                }
                // `&T` is not a cast type: its cast is a coercion to a raw pointer plus a cast
                TyKind::Ref(m, inner, _) => {
                    return match c {
                        Int(_) | Float => match self.tcx.tys.kind(self.shallow(inner)) {
                            TyKind::Int(_) | TyKind::Float(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) => Err(CastError::NeedDeref),
                            _ => Err(CastError::NeedViaPtr),
                        },
                        // array-ptr-cast
                        Ptr(cm, pointee) => {
                            if !self.type_is_sized_modulo_regions(pointee) { return Err(CastError::IllegalCast); }
                            let inner = self.shallow(inner);
                            let pointee = self.shallow(pointee);
                            if m || !cm {
                                if let TyKind::Array(ety, _) = self.tcx.tys.kind(inner).clone() {
                                    if self.can_eq(ety, pointee) { return Ok(()); }
                                }
                            }
                            Err(CastError::IllegalCast)
                        }
                        _ => Err(CastError::NonScalar),
                    };
                }
                _ => return Err(CastError::NonScalar),
            },
            _ => return Err(CastError::NonScalar),
        };
        // an enum with a non-exhaustive variant of another crate
        if let TyKind::Adt(d, _, _) = self.tcx.tys.kind(expr_ty).clone() {
            if self.prog.def(d).krate != self.prog.def(self.b.def).krate {
                let hit = self.tcx.adts.get(&d).is_some_and(|a| a.variants.iter().any(|v| {
                    let vd = self.prog.def(v.def);
                    let ast = &self.prog.files[vd.file as usize].ast;
                    // (a struct's own attributes are its variant's)
                    let attrs = if vd.kind == DefKind::Variant { ast.list(ast.item(vd.item).attrs).to_vec() } else { ast.list(ast.item(self.prog.def(d).item).attrs).to_vec() };
                    crate::lang_items::has_attr(self.prog, vd.file, &attrs, "non_exhaustive")
                }));
                if hit { return Err(CastError::ForeignNonExhaustiveAdt); }
            }
        }
        match (t_from, t_cast) {
            // these types have invariants: nothing casts into them
            (_, Int(IntKind::CEnum) | FnPtr) => Err(CastError::NonScalar),
            (_, Int(IntKind::Bool)) => Err(CastError::CastToBool),
            (Int(IntKind::U8), Int(IntKind::Char)) => Ok(()),
            (_, Int(IntKind::Char)) => Err(CastError::CastToChar),
            (Int(IntKind::Bool | IntKind::CEnum | IntKind::Char), Float) => Err(CastError::NeedViaInt),
            (Int(IntKind::Bool | IntKind::CEnum | IntKind::Char) | Float, Ptr(..)) | (Ptr(..) | FnPtr, Float) => Err(CastError::IllegalCast),
            (Ptr(_, src), Ptr(_, dst)) => self.check_ptr_ptr_cast(src, dst),
            (Ptr(_, pointee), Int(_)) => match self.pointer_kind(pointee)? {
                None => Err(CastError::UnknownExprPtrKind),
                Some(PointerKind::Thin) => Ok(()),
                _ => Err(CastError::NeedViaThinPtr),
            },
            (FnPtr, Int(_)) => Ok(()),
            (Int(_), Ptr(_, pointee)) => match self.pointer_kind(pointee)? {
                None => Err(CastError::UnknownCastPtrKind),
                Some(PointerKind::Thin) => Ok(()),
                Some(PointerKind::VTable(_)) => Err(CastError::IntToWideCast(Some("a vtable"))),
                Some(PointerKind::Length) => Err(CastError::IntToWideCast(Some("a length"))),
                Some(PointerKind::OfAlias(_) | PointerKind::OfParam(_)) => Err(CastError::IntToWideCast(None)),
            },
            (FnPtr, Ptr(_, pointee)) => match self.pointer_kind(pointee)? {
                None => Err(CastError::UnknownCastPtrKind),
                Some(PointerKind::Thin) => Ok(()),
                _ => Err(CastError::IllegalCast),
            },
            (Int(IntKind::CEnum), Int(_)) => {
                // err_if_cenum_impl_drop
                if let TyKind::Adt(d, _, _) = self.tcx.tys.kind(expr_ty).clone() {
                    if self.tcx.adt_destructor(d).is_some() {
                        let (es, cs) = (self.cast_ty_str(expr_ty), self.cast_ty_str(cast_ty));
                        self.err(lo, format!("E0604: cannot cast enum `{}` into integer `{}` because it implements `Drop`", es, cs));
                    }
                }
                Ok(())
            }
            (Int(IntKind::Char | IntKind::Bool), Int(_)) => Ok(()),
            (Int(_) | Float, Int(_) | Float) => Ok(()),
        }
    }

    fn check_ptr_ptr_cast(&mut self, src: TyId, dst: TyId) -> Result<(), CastError> {
        let src_kind = self.pointer_kind(src)?;
        let dst_kind = self.pointer_kind(dst)?;
        // we can't cast if the target pointer kind is unknown
        let Some(dst_kind) = dst_kind else { return Err(CastError::UnknownCastPtrKind) };
        // cast to thin pointer is ok
        if dst_kind == PointerKind::Thin { return Ok(()); }
        // we can't cast to a wide pointer if the source pointer kind is unknown
        let Some(src_kind) = src_kind else { return Err(CastError::UnknownCastPtrKind) };
        match (src_kind, dst_kind) {
            // thin -> fat? report invalid cast (don't complain about vtable kinds)
            (PointerKind::Thin, _) => Err(CastError::SizedUnsizedCast),
            // trait object -> trait object: need additional checks
            (PointerKind::VTable(src_obj), PointerKind::VTable(dst_obj)) => {
                let (TyKind::Dyn(std_, sargs, sbinds, srs, _, sbound, sautos), TyKind::Dyn(dtd, dargs, dbinds, drs, _, dbound, dautos)) =
                    (self.tcx.tys.kind(src_obj).clone(), self.tcx.tys.kind(dst_obj).clone()) else { return Err(CastError::Reported) };
                let src_principal = !crate::next_solver::eval_ctxt::trait_is_auto(self.prog, std_);
                let dst_principal = !crate::next_solver::eval_ctxt::trait_is_auto(self.prog, dtd);
                match (src_principal, dst_principal) {
                    (true, true) => {
                        // the same principal, generic arguments and projections
                        let erased = self.tcx.tys.erase_all_regions(src_obj);
                        let region = match self.tcx.tys.kind(erased).clone() { TyKind::Dyn(_, _, _, _, r, _, _) => r, _ => unreachable!() };
                        let without = |d: DefId, a, b, r, bound| self.tcx.tys.intern(TyKind::Dyn(d, a, b, r, region, bound, crate::types::List::EMPTY));
                        let (s, d) = (without(std_, sargs, sbinds, srs, sbound), without(dtd, dargs, dbinds, drs, dbound));
                        if !self.can_eq(s, d) { return Err(CastError::DifferingKinds); }
                        // SrcAuto (and the auto traits its principal implies) is a superset of DstAuto
                        let mut src_auto: Vec<DefId> = self.tcx.tys.list(sautos).to_vec();
                        let pred = trait_predicate(std_, self.tcx.generic_indices_of(self.prog, std_).identity_args(&self.tcx.tys));
                        for p in self.tcx.supertraits(self.prog, pred) {
                            if let PredicateKind::Clause(ClauseKind::Trait(t)) = &p.kind {
                                if crate::next_solver::eval_ctxt::trait_is_auto(self.prog, t.trait_ref.def_id) { src_auto.push(t.trait_ref.def_id); }
                            }
                        }
                        let added: Vec<DefId> = self.tcx.tys.list(dautos).iter().copied().filter(|t| !src_auto.contains(t)).collect();
                        if !added.is_empty() { return Err(CastError::PtrPtrAddingAutoTrait(added)); }
                        Ok(())
                    }
                    // dyn Auto -> dyn Auto'? ok
                    (false, false) => Ok(()),
                    // dropping or adding a principal is not allowed by a pointer cast
                    _ => Err(CastError::DifferingKinds),
                }
            }
            // fat -> fat? the metadata kinds must match
            (s, d) if s == d => Ok(()),
            _ => Err(CastError::DifferingKinds),
        }
    }
}
