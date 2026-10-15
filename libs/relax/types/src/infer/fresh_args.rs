// ported from N:rustc_infer/src/infer/mod.rs @8d1a764
// ported from N:rustc_middle/src/ty/generic_args.rs @8d1a764
use super::InferCtxt;
use crate::consts::{Const, ConstKind, GenericArg};
use crate::program::{DefId, Program};
use crate::types::{TyId, TyKind, RegionId, RegionKind, Types};
use crate::types::fold::{TypeFolder, super_fold_const, super_fold_ty};
use crate::types::variance::GenericIndices;

#[derive(Clone, Copy)]
pub enum GenericParamDefKind { Lifetime, Type, Const }
#[derive(Clone, Copy)]
pub struct GenericParamDef { pub index: u32, pub kind: GenericParamDefKind }

impl InferCtxt<'_> {
    /// infer/mod.rs:1037: each declaration kind has a distinct inference table.
    pub fn var_for_def(&self, param: GenericParamDef) -> GenericArg {
        match param.kind {
            GenericParamDefKind::Lifetime => GenericArg::Lifetime(self.next_region_var()),
            GenericParamDefKind::Type => GenericArg::Type(self.next_ty_var()),
            GenericParamDefKind::Const => GenericArg::Const(self.next_const_var()),
        }
    }

    pub fn fresh_args_for_item(&self, prog: &Program, def_id: DefId) -> Vec<GenericArg> {
        let indices = self.tcx.generic_indices_of(prog, def_id);
        let count = indices.lifetimes.len() + indices.types_consts.len();
        let mut args = Vec::with_capacity(count);
        for index in 0..count as u32 {
            let kind = if indices.lifetimes.contains(&index) { GenericParamDefKind::Lifetime }
                else if indices.consts.contains(&index) { GenericParamDefKind::Const }
                else { GenericParamDefKind::Type };
            args.push(self.var_for_def(GenericParamDef { index, kind }));
        }
        args
    }
}

impl crate::tcx::Tcx {
    pub fn generic_indices_of(&self, prog: &Program, def: DefId) -> &GenericIndices {
        self.declaration_queries(prog, def).generics.get_or_init(|| self.generic_indices_uncached(prog, def))
    }

    fn generic_indices_uncached(&self, prog: &Program, def: DefId) -> GenericIndices {
        if let Some(indices) = self.rpitit_generic_indices(prog, def) { return indices; }
        if matches!(prog.def(def).kind, crate::program::DefKind::Prim(_) | crate::program::DefKind::Mod | crate::program::DefKind::MacroRules) { return GenericIndices::default(); }
        // rustc_hir_analysis/src/collect/generics_of.rs:96-100: MCG has no
        // generic parent, unlike repeat counts and inline const bodies.
        if prog.def(def).kind == crate::program::DefKind::AnonConst && !prog.repeat_expr_counts.contains(&def)
            && matches!(prog.def_id_to_node_id.get(&def), Some((_, relax_front::def_collector::NodeId::AnonConst(_)))) {
            return GenericIndices::default();
        }
        self.resolve_bound_vars(prog, def).generic_indices.clone()
    }
}

/// generic_args.rs: EarlyBinder::instantiate: declaration lifetimes, types and
/// consts all use the same substitution; bound variables shift under binders.
/// Relax's compact type/const parameter indices use GenericIndices to find
/// their position in the complete declaration argument list.
pub struct ArgFolder<'a> { pub indices: &'a GenericIndices, pub args: &'a [GenericArg] }
impl TypeFolder for ArgFolder<'_> {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if let TyKind::Param(index) = tys.kind(ty) {
            let full = self.indices.types_consts[*index as usize];
            let GenericArg::Type(ty) = self.args[full as usize] else { panic!("wrong generic argument kind") };
            return tys.shift_in(ty, depth);
        }
        super_fold_ty(tys, ty, self, depth)
    }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
        if let ConstKind::Param(param) = tys.const_kind(ct) {
            let full = self.indices.types_consts[param.index as usize];
            let GenericArg::Const(ct) = self.args[full as usize] else { panic!("wrong generic argument kind") };
            return tys.shift_const(ct, depth, false);
        }
        super_fold_const(tys, ct, self, depth)
    }
    fn fold_region(&mut self, tys: &Types, region: RegionId, depth: u32) -> RegionId {
        if let RegionKind::ReEarlyParam(index) = tys.region_kind(region) {
            let GenericArg::Lifetime(region) = self.args[index as usize] else { panic!("wrong generic argument kind") };
            if let RegionKind::ReBound(db, var) = tys.region_kind(region) {
                return tys.intern_region(RegionKind::ReBound(db + depth, var));
            }
            return region;
        }
        region
    }
}

impl GenericIndices {
    pub fn identity_types_consts(&self, tys: &Types) -> Vec<TyId> {
        let mut args = Vec::with_capacity(self.types_consts.len());
        for (index, full) in self.types_consts.iter().enumerate() {
            let kind = if self.consts.contains(full) {
                TyKind::Const(tys.intern_const(ConstKind::Param(crate::consts::ParamConst { index: index as u32 })))
            } else { TyKind::Param(index as u32) };
            args.push(tys.intern(kind));
        }
        args
    }
    pub fn identity_args(&self, tys: &Types) -> Vec<GenericArg> {
        let compact = self.identity_types_consts(tys);
        let mut args = Vec::with_capacity(compact.len() + self.lifetimes.len());
        for full in 0..(compact.len() + self.lifetimes.len()) as u32 {
            if self.lifetimes.contains(&full) {
                args.push(GenericArg::Lifetime(tys.intern_region(RegionKind::ReEarlyParam(full))));
            } else {
                let i = self.types_consts.iter().position(|x| *x == full).unwrap();
                args.push(match tys.kind(compact[i]) { TyKind::Const(c) => GenericArg::Const(*c), _ => GenericArg::Type(compact[i]) });
            }
        }
        args
    }
}
