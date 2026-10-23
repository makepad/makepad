// ported from N:rustc_infer/src/infer/outlives/mod.rs @8d1a764
pub mod env;
pub mod components;
pub mod verify;
pub mod obligations;
pub mod test_type_match;
use super::{InferCtxt, free_regions::RegionRelations, lexical_region_resolve::{self, RegionResolutionError}, region_constraints::ConstraintKind};
use crate::program::Program;
use crate::types::fold::TypeFolder;

impl InferCtxt<'_> {
    /// outlives/mod.rs:43-96: consumes region storage; only resolution is legal afterward.
    pub fn resolve_regions_with_outlives_env(&self, prog: &Program, env: &env::OutlivesEnvironment) -> Vec<RegionResolutionError> {
        assert!(!self.in_snapshot());
        loop {
            let obligations = std::mem::take(&mut self.inner.borrow_mut().region_obligations);
            if obligations.is_empty() { break; }
            for (ty, region, span) in obligations {
                self.origin_span.set(span);
                let mut resolver = super::resolve::DeepVariableResolver::new(self);
                let ty = resolver.fold_ty(&self.tcx.tys, ty, 0);
                let region = resolver.fold_region(&self.tcx.tys, region, 0);
                // obligations.rs register_type_outlives_constraint: SubregionOrigin::RelateParamBound(span, sup_type)
                let mut delegate = obligations::LexicalOutlives { infcx: self, origin: super::region_constraints::SubregionOrigin::RelateParamBound(span, ty) };
                obligations::TypeOutlives::new(&mut delegate, self.tcx, prog, &env.region_bound_pairs, None, &env.known_type_outlives)
                    .type_must_outlive(ty, region, crate::borrowck::constraints::ConstraintCategory::Boring);
            }
        }
        let storage = self.inner.borrow_mut().region_constraint_storage.take().expect("regions already resolved");
        let mut constraints = Vec::new();
        for mut c in storage.constraints {
            let eq = matches!(c.kind, ConstraintKind::VarEqVar | ConstraintKind::VarEqReg | ConstraintKind::RegEqReg);
            c.kind = match c.kind {
                ConstraintKind::VarEqVar => ConstraintKind::VarSubVar,
                ConstraintKind::VarEqReg => ConstraintKind::VarSubReg,
                ConstraintKind::RegEqReg => ConstraintKind::RegSubReg,
                kind => kind,
            };
            constraints.push(c);
            if eq {
                std::mem::swap(&mut c.sub, &mut c.sup);
                if c.kind == ConstraintKind::VarSubReg { c.kind = ConstraintKind::RegSubVar; }
                constraints.push(c);
            }
        }
        let rels = RegionRelations { prog, tcx: self.tcx, tys: &self.tcx.tys, free_regions: &env.free_region_map };
        let (values, errors) = lexical_region_resolve::resolve(&rels, &self.errors, storage.var_infos, constraints, storage.verifys);
        assert!(self.lexical_region_resolutions.replace(Some(values)).is_none());
        errors
    }
}

#[cfg(test)]
mod tests;
