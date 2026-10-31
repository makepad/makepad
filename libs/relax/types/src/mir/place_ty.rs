//! Place typing: rustc_middle/src/mir/statement.rs:97,124,202.
use super::*;
use crate::program::Program;
use crate::tcx::{Tcx, TypingEnv};
use crate::types::{TyKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaceTy {
    pub ty: TyId,
    pub variant_index: Option<VariantIdx>,
}

impl PlaceTy {
    pub fn from_ty(ty: TyId) -> Self { Self { ty, variant_index: None } }

    /// ADT fields are substituted and normalized at the shared Tcx boundary.
    /// rustc statement.rs:124; rustc_middle/src/ty/mod.rs:1050: caller-owned typing environment.
    pub fn field_ty(self, prog: &Program, tcx: &Tcx, env: TypingEnv, field: FieldIdx) -> Option<TyId> {
        if self.ty.0 as usize >= tcx.tys.len() { return None; }
        match tcx.tys.kind(self.ty) {
            TyKind::Adt(def, args, _) => {
                let adt = tcx.adts.get(def)?;
                let variant = if adt.is_enum() { self.variant_index?.0 as usize }
                    else { if self.variant_index.is_some() { return None; } 0 };
                adt.variants.get(variant)?.fields.get(field.0 as usize)?;
                Some(tcx.field_ty(prog, env, *def, variant as u32, field.0, tcx.tys.list(*args)))
            }
            TyKind::Tuple(fields) if self.variant_index.is_none() => tcx.tys.list(*fields).get(field.0 as usize).copied(),
            TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: captures, .. }) if self.variant_index.is_none() => {
                let TyKind::Tuple(fields) = tcx.tys.kind(*captures) else { return None; };
                tcx.tys.list(*fields).get(field.0 as usize).copied()
            }
            _ => None,
        }
    }

    pub fn projection_ty_normalized(self, prog: &Program, tcx: &Tcx, env: TypingEnv, elem: &PlaceElem) -> Option<Self> {
        if let PlaceElem::Field(field, _) = elem {
            let ty = self.field_ty(prog, tcx, env, *field)?;
            self.projection_ty(tcx, &PlaceElem::Field(*field, ty))
        } else { self.projection_ty(tcx, elem) }
    }

    /// Field projections already carry their instantiated type. Move-path construction
    /// obtains that type through field_ty before applying this shared projection rule.
    /// Invalid/incomplete MIR yields None so the structural validator can also use it.
    pub fn projection_ty(self, tcx: &Tcx, elem: &PlaceElem) -> Option<Self> {
        if self.ty.0 as usize >= tcx.tys.len() { return None; }
        if self.variant_index.is_some() && !matches!(elem, PlaceElem::Field(..)) { return None; }
        let ty = match elem {
            PlaceElem::Deref => match tcx.tys.kind(self.ty) {
                TyKind::Ref(_, ty, _) | TyKind::Ptr(_, ty) => *ty,
                TyKind::Adt(def, args, _) if tcx.adts.get(def)?.is_box() => *tcx.tys.list(*args).first()?,
                _ => return None,
            },
            PlaceElem::Field(_, ty) | PlaceElem::OpaqueCast(ty) | PlaceElem::Subtype(ty) => *ty,
            PlaceElem::Index(_) | PlaceElem::ConstantIndex { .. } => match tcx.tys.kind(self.ty) {
                TyKind::Array(ty, _) | TyKind::Slice(ty) => *ty,
                _ => return None,
            },
            PlaceElem::Subslice { from, to, from_end } => match tcx.tys.kind(self.ty) {
                TyKind::Slice(_) => self.ty,
                TyKind::Array(inner, size) => {
                    let len = if *from_end {
                        let size = size.try_to_target_usize(&tcx.tys)?;
                        size.checked_sub(*from)?.checked_sub(*to)?
                    } else { to.checked_sub(*from)? };
                    tcx.tys.intern(TyKind::Array(*inner, tcx.tys.const_usize(len)))
                }
                _ => return None,
            },
            PlaceElem::Downcast(_, variant) => return Some(Self { ty: self.ty, variant_index: Some(*variant) }),
        };
        if ty.0 as usize >= tcx.tys.len() { return None; }
        Some(Self::from_ty(ty))
    }
}

impl Place {
    pub fn ty_normalized(&self, body: &Body, prog: &Program, tcx: &Tcx, env: TypingEnv) -> Option<PlaceTy> {
        let mut ty = PlaceTy::from_ty(body.local_decls.get(self.local.0 as usize)?.ty);
        if ty.ty.0 as usize >= tcx.tys.len() { return None; }
        for elem in &self.projection { ty = ty.projection_ty_normalized(prog, tcx, env, elem)?; }
        Some(ty)
    }

    pub fn ty(&self, body: &Body, tcx: &Tcx) -> Option<PlaceTy> {
        let mut ty = PlaceTy::from_ty(body.local_decls.get(self.local.0 as usize)?.ty);
        if ty.ty.0 as usize >= tcx.tys.len() { return None; }
        for elem in &self.projection { ty = ty.projection_ty(tcx, elem)?; }
        Some(ty)
    }
}
