// ported from N:rustc_ty_utils/src/ty.rs @8d1a764
// ported from N:rustc_ty_utils/src/representability.rs @8d1a764
use super::{ClauseKind, GenericArg, PredicateKind};
use crate::lang_items::LangItem;
use crate::program::{DefId, Program};
use crate::query::{Fingerprint, QueryError, QueryId, QueryTable};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};
use crate::type_queries::Providers;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SizedTraitKind { Sized, MetaSized }
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SizednessKey { def: DefId, kind: SizedTraitKind }
fn def_fp(def: &DefId) -> Fingerprint { Fingerprint::from_bytes(&def.0.to_le_bytes()) }
fn ty_fp(ty: &TyId) -> Fingerprint { Fingerprint::from_bytes(&ty.0.to_le_bytes()) }
fn sizedness_fp(key: &SizednessKey) -> Fingerprint {
    let mut bytes = key.def.0.to_le_bytes().to_vec(); bytes.push(key.kind as u8); Fingerprint::from_bytes(&bytes)
}
fn constraint_fp(ty: &Option<TyId>) -> Fingerprint {
    let mut bytes = vec![ty.is_some() as u8]; if let Some(ty) = ty { bytes.extend_from_slice(&ty.0.to_le_bytes()); } Fingerprint::from_bytes(&bytes)
}
fn params_fp(params: &Vec<u32>) -> Fingerprint {
    let mut bytes = Vec::new(); for p in params { bytes.extend_from_slice(&p.to_le_bytes()); } Fingerprint::from_bytes(&bytes)
}
fn unit_fp(_: &()) -> Fingerprint { Fingerprint::from_bytes(&[]) }
pub(crate) struct DeclarationQueries {
    sizedness: QueryTable<SizednessKey, Option<TyId>>,
    representability: QueryTable<DefId, ()>,
    representability_adt_ty: QueryTable<TyId, ()>,
    params_in_repr: QueryTable<DefId, Vec<u32>>,
}
impl DeclarationQueries {
    pub(crate) fn new() -> Self {
        Self { sizedness: QueryTable::new(5, sizedness_fp, constraint_fp), representability: QueryTable::new(6, def_fp, unit_fp),
            representability_adt_ty: QueryTable::new(7, ty_fp, unit_fp), params_in_repr: QueryTable::new(8, def_fp, params_fp) }
    }
}
impl Providers<'_> {
    pub(crate) fn force_declaration(&self, id: QueryId) -> Result<(), QueryError> {
        let q = &self.tcx.queries.declarations;
        match id.kind {
            5 => { self.adt_sizedness_constraint(q.sizedness.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            6 => self.check_representability(q.representability.key(id.key).ok_or(QueryError::MissingQuery(id))?)?,
            7 => self.check_representability_adt_ty(q.representability_adt_ty.key(id.key).ok_or(QueryError::MissingQuery(id))?)?,
            8 => { self.params_in_repr(q.params_in_repr.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            _ => return Err(QueryError::MissingQuery(id)),
        }
        Ok(())
    }
    fn check_representability(&self, def: DefId) -> Result<(), QueryError> {
        let q = &self.tcx.queries;
        q.declarations.representability.get(&q.engine, def, &mut |def| {
            q.read_def_declarations(self.prog, def)?;
            for variant in &self.tcx.adts[&def].variants { for field in &variant.fields { self.check_representability_ty(field.ty)?; } }
            Ok(())
        }, &|id| self.force(id))
    }
    fn check_representability_ty(&self, ty: TyId) -> Result<(), QueryError> {
        match self.tcx.tys.kind(ty) {
            TyKind::Adt(..) => self.check_representability_adt_ty(ty)?,
            TyKind::Array(ty, _) => self.check_representability_ty(*ty)?,
            TyKind::Tuple(tys) => { for &ty in self.tcx.tys.list(*tys) { self.check_representability_ty(ty)?; } }
            _ => {}
        }
        Ok(())
    }
    // representability.rs:60-91: separate ADT-instance query preserves the
    // distinction between recursion in a definition and in represented args.
    fn check_representability_adt_ty(&self, ty: TyId) -> Result<(), QueryError> {
        let q = &self.tcx.queries;
        q.declarations.representability_adt_ty.get(&q.engine, ty, &mut |ty| {
            q.read_declarations()?;
            let TyKind::Adt(def, args, _) = self.tcx.tys.kind(ty) else { unreachable!() };
            self.check_representability(*def)?;
            let params = self.params_in_repr(*def)?;
            for &index in &params { self.check_representability_ty(self.tcx.tys.list(*args)[index as usize])?; }
            Ok(())
        }, &|id| self.force(id))
    }
    fn params_in_repr(&self, def: DefId) -> Result<Vec<u32>, QueryError> {
        let q = &self.tcx.queries;
        q.declarations.params_in_repr.get(&q.engine, def, &mut |def| {
            q.read_def_declarations(self.prog, def)?;
            let mut params = Vec::new();
            for variant in &self.tcx.adts[&def].variants { for field in &variant.fields { self.params_in_repr_ty(field.ty, &mut params)?; } }
            params.sort_unstable(); Ok(params)
        }, &|id| self.force(id))
    }
    fn params_in_repr_ty(&self, ty: TyId, params: &mut Vec<u32>) -> Result<(), QueryError> {
        match self.tcx.tys.kind(ty) {
            TyKind::Adt(def, args, _) => { for index in self.params_in_repr(*def)? { self.params_in_repr_ty(self.tcx.tys.list(*args)[index as usize], params)?; } }
            TyKind::Array(ty, _) => self.params_in_repr_ty(*ty, params)?,
            TyKind::Tuple(tys) => { for &ty in self.tcx.tys.list(*tys) { self.params_in_repr_ty(ty, params)?; } }
            TyKind::Param(index) => { if !params.contains(index) { params.push(*index); } }
            _ => {}
        }
        Ok(())
    }
    fn sizedness_constraint_for_ty(&self, kind: SizedTraitKind, ty: TyId) -> Result<Option<TyId>, QueryError> {
        Ok(match self.tcx.tys.kind(ty) {
            TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) | TyKind::Ptr(..) | TyKind::Ref(..) | TyKind::FnDef(..)
            | TyKind::FnPtr(..) | TyKind::Array(..) | TyKind::Closure(..) | TyKind::Never => None,
            TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..) => if kind == SizedTraitKind::Sized { Some(ty) } else { None },
            TyKind::Foreign(_) | TyKind::Param(_) | TyKind::Alias(..) | TyKind::Error => Some(ty),
            TyKind::Tuple(tys) => match self.tcx.tys.list(*tys).last() { Some(&ty) => self.sizedness_constraint_for_ty(kind, ty)?, None => None },
            TyKind::Adt(def, args, regions) => match self.adt_sizedness_constraint(SizednessKey { def: *def, kind })? {
                Some(ty) => {
                    let full = self.tcx.complete_args(self.prog, *def, self.tcx.tys.list(*args), *regions);
                    let ty = self.tcx.instantiate_ty(self.prog, *def, &full, ty);
                    self.sizedness_constraint_for_ty(kind, ty)?
                }
                None => None,
            },
            _ => panic!("unexpected type in sizedness_constraint_for_ty"),
        })
    }
    fn adt_sizedness_constraint(&self, key: SizednessKey) -> Result<Option<TyId>, QueryError> {
        let q = &self.tcx.queries;
        q.declarations.sizedness.get(&q.engine, key, &mut |key| {
            q.read_def_declarations(self.prog, key.def)?;
            let adt = &self.tcx.adts[&key.def];
            // rustc_middle/src/ty/adt.rs:766-772: enums/unions are sized.
            if adt.is_enum || adt.is_union { return Ok(None); }
            self.check_representability(key.def)?;
            let Some(field) = adt.variants[0].fields.last() else { return Ok(None); };
            let Some(ty) = self.sizedness_constraint_for_ty(key.kind, field.ty)? else { return Ok(None); };
            let lang = match key.kind { SizedTraitKind::Sized => LangItem::Sized, SizedTraitKind::MetaSized => LangItem::MetaSized };
            if let Some(trait_def) = self.tcx.lang_items.get(lang) {
                for p in self.tcx.clauses_of(self.prog, key.def) {
                    if let PredicateKind::Clause(ClauseKind::Trait(p)) = p.kind {
                        if p.trait_ref.def_id == trait_def && p.trait_ref.args.first() == Some(&GenericArg::Type(ty)) { return Ok(None); }
                    }
                }
            }
            Ok(Some(ty))
        }, &|id| self.force(id))
    }
}
impl Tcx {
    pub fn check_representability(&self, prog: &Program, def: DefId) -> Result<(), QueryError> { Providers { tcx: self, prog }.check_representability(def) }
    pub fn adt_sizedness_constraint(&self, prog: &Program, def: DefId, kind: SizedTraitKind) -> Result<Option<TyId>, QueryError> {
        Providers { tcx: self, prog }.adt_sizedness_constraint(SizednessKey { def, kind })
    }
}
