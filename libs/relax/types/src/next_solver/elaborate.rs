// ported from N:rustc_type_ir/src/elaborate.rs @8d1a764
// ported from N:rustc_middle/src/ty/predicate.rs @8d1a764
use super::*;
use crate::infer::fresh_args::ArgFolder;
use crate::program::Program;
use crate::tcx::Tcx;
use crate::types::{Types, RegionKind, RegionId, BoundVar, BoundTy, TyKind, TyId};
use crate::types::fold::{TypeFolder, super_fold_ty, super_fold_const};

struct BoundVarShifter { offset: u32 }
impl TypeFolder for BoundVarShifter {
    fn fold_region(&mut self, tys: &Types, r: RegionId, depth: u32) -> RegionId {
        match tys.region_kind(r) {
            RegionKind::ReBound(d, var) if d == depth => tys.intern_region(RegionKind::ReBound(d, BoundVar(var.0 + self.offset))),
            _ => r,
        }
    }
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if let TyKind::Bound(d, b) = *tys.kind(ty) {
            if d == depth { return tys.intern(TyKind::Bound(d, BoundTy { var: BoundVar(b.var.0 + self.offset), kind: b.kind })); }
        }
        super_fold_ty(tys, ty, self, depth)
    }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
        if let crate::consts::ConstKind::Bound(d, b) = tys.const_kind(ct) {
            if d == depth { return tys.intern_const(crate::consts::ConstKind::Bound(d, crate::consts::BoundConst { var: BoundVar(b.var.0 + self.offset) })); }
        }
        super_fold_const(tys, ct, self, depth)
    }
}

impl Tcx {
    pub fn instantiate_supertrait(&self, prog: &Program, pred: &Predicate, parent: &Predicate) -> Predicate {
        let PredicateKind::Clause(ClauseKind::Trait(tr)) = &parent.kind else { panic!("supertrait parent") };
        let kind = fold::fold_predicate_kind(&self.tys, &pred.kind, &mut BoundVarShifter { offset: parent.bound_vars.len() as u32 }, 0);
        let kind = fold::fold_predicate_kind(&self.tys, &kind, &mut ArgFolder { indices: &self.generic_indices_of(prog, tr.trait_ref.def_id), args: &tr.trait_ref.args }, 0);
        let mut bound_vars = parent.bound_vars.to_vec();
        bound_vars.extend_from_slice(&pred.bound_vars);
        Predicate { kind, bound_vars: bound_vars.into() }
    }

    /// rustc_type_ir/src/elaborate.rs:101-112 (extend_deduped): a predicate is pushed only if its
    /// kind with anonymized bound variables (anonymize_bound_vars) was not seen before; :140-249
    /// pops, elaborates and yields.
    pub fn supertraits(&self, prog: &Program, initial: Predicate) -> Vec<Predicate> {
        fn anonymized(pred: &Predicate) -> (PredicateKind, Vec<BoundVariableKind>) {
            let vars = pred.bound_vars.iter().map(|var| match var {
                BoundVariableKind::Region(_) => BoundVariableKind::Region(crate::types::BoundRegionKind::Anon),
                other => other.clone(),
            }).collect();
            (pred.kind.clone(), vars)
        }
        let mut visited = crate::fxhash::HashSet::default();
        let mut stack = Vec::new();
        let mut push = |stack: &mut Vec<Predicate>, pred: Predicate| { if visited.insert(anonymized(&pred)) { stack.push(pred); } };
        push(&mut stack, initial);
        let mut result = Vec::new();
        while let Some(pred) = stack.pop() {
            if let PredicateKind::Clause(ClauseKind::Trait(tr)) = &pred.kind {
                if tr.polarity == ClausePolarity::Positive {
                for bound in self.explicit_super_predicates_of(prog, tr.trait_ref.def_id) {
                    push(&mut stack, self.instantiate_supertrait(prog, &bound, &pred));
                }
                }
            }
            if let PredicateKind::Clause(ClauseKind::TypeOutlives(ty, region)) = pred.kind {
                if !matches!(self.tys.region_kind(region), RegionKind::ReBound(..)) {
                    use crate::infer::outlives::components::{push_outlives_components, Component};
                    let mut parts = Vec::new();
                    push_outlives_components(self, prog, ty, &mut parts);
                    for part in parts {
                        let clause = match part {
                            Component::Region(r) => ClauseKind::RegionOutlives(r, region),
                            Component::Param(t) | Component::Placeholder(t) | Component::Alias(t) => ClauseKind::TypeOutlives(t, region),
                            Component::EscapingAlias(_) | Component::UnresolvedInferenceVariable(_) => continue,
                        };
                        push(&mut stack, Predicate { kind: PredicateKind::Clause(clause), bound_vars: pred.bound_vars.clone() });
                    }
                }
            }
            result.push(pred);
        }
        result
    }
}
