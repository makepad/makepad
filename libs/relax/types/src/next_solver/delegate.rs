// ported from N:rustc_trait_selection/src/solve/delegate.rs @8d1a764
use super::*;
use crate::infer::fresh_args::ArgFolder;
use crate::program::Program;
use crate::tcx::Tcx;
use crate::types::{TyKind, RegionArgsId};

impl Tcx {
    pub fn complete_args(&self, prog: &Program, def: DefId, compact: &[TyId], regions: RegionArgsId) -> Vec<GenericArg> {
        let indices = self.generic_indices_of(prog, def);
        let regions = self.tys.region_args(regions);
        assert_eq!(compact.len(), indices.types_consts.len(), "generic type/const slots for {}", prog.def_path(def));
        assert_eq!(regions.len(), indices.lifetimes.len(), "generic lifetime slots for {}", prog.def_path(def));
        let mut args = vec![GenericArg::Lifetime(crate::types::RegionId::ERASED); compact.len() + regions.len()];
        for (&full, &region) in indices.lifetimes.iter().zip(regions.iter()) {
            args[full as usize] = GenericArg::Lifetime(region);
        }
        for (&full, &ty) in indices.types_consts.iter().zip(compact) {
            args[full as usize] = match self.tys.kind(ty) {
                TyKind::Const(ct) => GenericArg::Const(*ct),
                _ => GenericArg::Type(ty),
            };
        }
        args
    }
    pub fn instantiate_ty(&self, prog: &Program, def: DefId, args: &[GenericArg], ty: TyId) -> TyId {
        crate::types::fold::EarlyBinder::bind(&self.tys, ty).instantiate(&self.tys, &self.generic_indices_of(prog, def), args)
    }
    pub fn instantiate_predicate(&self, prog: &Program, def: DefId, args: &[GenericArg], p: &Predicate) -> Predicate {
        fold::fold_predicate(&self.tys, p, &mut ArgFolder { indices: &self.generic_indices_of(prog, def), args })
    }
    pub(crate) fn identity_impl_trait_ref(&self, prog: &Program, imp: DefId) -> &TraitRef {
        self.declaration_queries(prog, imp).impl_trait_ref.get_or_init(|| {
            let def_id = self.impl_trait[&imp];
            let mut compact = vec![self.impl_self[&imp]];
            compact.extend_from_slice(self.impl_trait_args.get(&imp).map_or(&[], |a| a.as_slice()));
            let regions = self.impl_trait_regions.get(&imp).copied().unwrap_or(RegionArgsId::EMPTY);
            TraitRef { def_id, args: self.complete_args(prog, def_id, &compact, regions) }
        })
    }
    pub fn impl_trait_ref(&self, prog: &Program, imp: DefId, args: &[GenericArg]) -> TraitRef {
        let identity = self.identity_impl_trait_ref(prog, imp);
        let mut f = ArgFolder { indices: self.generic_indices_of(prog, imp), args };
        let args = identity.args.iter().map(|arg| fold::fold_arg(&self.tys, *arg, &mut f, 0)).collect();
        TraitRef { def_id: identity.def_id, args }
    }

    /// rustc_hir_analysis/src/collect/item_bounds.rs:526: elaborate in the impl's
    /// identity binder once, then instantiate for each candidate.
    pub fn impl_super_outlives(&self, prog: &Program, imp: DefId) -> &[Predicate] {
        self.declaration_queries(prog, imp).impl_super_outlives.get_or_init(|| {
            let identity = self.identity_impl_trait_ref(prog, imp);
            self.supertraits(prog, collect::trait_predicate(identity.def_id, identity.args.clone()))
                .into_iter().filter(|p| matches!(p.kind,
                    PredicateKind::Clause(ClauseKind::RegionOutlives(..) | ClauseKind::TypeOutlives(..)))).collect()
        })
    }

}

impl Tcx {
    /// rustc_trait_selection/src/solve/delegate.rs:439-488, final LeafDef path.
    /// DEBLOAT(round9): collection rejects specialization, so every impl is a
    /// leaf and an omitted item uses the trait's definition in every typing mode.
    pub fn fetch_eligible_assoc_item(&self, prog: &Program, trait_assoc_def_id: DefId, impl_def_id: DefId) -> DefId {
        let trait_item = prog.def(trait_assoc_def_id);
        debug_assert_eq!(self.impl_trait.get(&impl_def_id), Some(&trait_item.parent));
        let imp = &prog.impls[prog.def(impl_def_id).sub as usize];
        for &item in &imp.items {
            if prog.trait_item_def_id.get(&item) == Some(&trait_assoc_def_id) { return item; }
        }
        trait_assoc_def_id
    }
}

#[cfg(test)]
mod tests;
