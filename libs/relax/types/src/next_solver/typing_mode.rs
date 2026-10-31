// ported from N:rustc_next_trait_solver/src/solve/eval_ctxt/mod.rs @8d1a764
use crate::infer::opaque_types::OpaqueTypeKey;
use crate::program::DefId;
use crate::tcx::{Tcx, TypingMode};
use crate::types::{TyId, TyKind};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RerunCondition {
    Never,
    Always,
    OpaqueInStorage(Vec<DefId>),
    AnyOpaqueHasInferAsHidden,
    OpaqueInStorageOrAnyOpaqueHasInferAsHidden(Vec<DefId>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RerunDecision { Yes, No, EagerlyPropagateToParent }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoSolutionOrRerunNonErased { NoSolution, RerunNonErased }
impl From<super::NoSolution> for NoSolutionOrRerunNonErased {
    fn from(_: super::NoSolution) -> Self { Self::NoSolution }
}
impl NoSolutionOrRerunNonErased {
    pub fn assert_no_rerun(self) -> super::NoSolution {
        assert_eq!(self, Self::NoSolution); super::NoSolution
    }
}
pub type QueryResultOrRerunNonErased = Result<super::canonical::Canonical<super::Response>, NoSolutionOrRerunNonErased>;
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AccessedOpaques { pub rerun: RerunCondition }
impl Default for AccessedOpaques { fn default() -> Self { Self { rerun: RerunCondition::Never } } }
impl AccessedOpaques {
    pub fn update(&mut self, other: Self) -> Result<(), NoSolutionOrRerunNonErased> {
        use RerunCondition::*;
        self.rerun = match (std::mem::replace(&mut self.rerun, Never), other.rerun) {
            (Never, b) | (b, Never) => b,
            (Always, _) | (_, Always) => Always,
            (AnyOpaqueHasInferAsHidden, AnyOpaqueHasInferAsHidden) => AnyOpaqueHasInferAsHidden,
            (OpaqueInStorage(a), AnyOpaqueHasInferAsHidden) | (AnyOpaqueHasInferAsHidden, OpaqueInStorage(a)) => OpaqueInStorageOrAnyOpaqueHasInferAsHidden(a),
            (OpaqueInStorageOrAnyOpaqueHasInferAsHidden(a), AnyOpaqueHasInferAsHidden) | (AnyOpaqueHasInferAsHidden, OpaqueInStorageOrAnyOpaqueHasInferAsHidden(a)) => OpaqueInStorage(a),
            (OpaqueInStorage(mut a), OpaqueInStorage(b)) => { for d in b { if !a.contains(&d) { a.push(d); } } OpaqueInStorage(a) }
            (OpaqueInStorageOrAnyOpaqueHasInferAsHidden(mut a), OpaqueInStorageOrAnyOpaqueHasInferAsHidden(b)) |
            (OpaqueInStorageOrAnyOpaqueHasInferAsHidden(mut a), OpaqueInStorage(b)) |
            (OpaqueInStorage(mut a), OpaqueInStorageOrAnyOpaqueHasInferAsHidden(b)) => {
                for d in b { if !a.contains(&d) { a.push(d); } } OpaqueInStorageOrAnyOpaqueHasInferAsHidden(a)
            }
        };
        self.should_bail()
    }
    pub fn should_bail(&self) -> Result<(), NoSolutionOrRerunNonErased> {
        if self.rerun == RerunCondition::Always { Err(NoSolutionOrRerunNonErased::RerunNonErased) } else { Ok(()) }
    }
    pub fn rerun_always(&mut self) -> NoSolutionOrRerunNonErased { self.rerun = RerunCondition::Always; NoSolutionOrRerunNonErased::RerunNonErased }
    pub fn might_rerun(&self) -> bool { self.rerun != RerunCondition::Never }
}

/// eval_ctxt/mod.rs:1917: erased results may be reused only when the accesses
/// cannot depend on the original owner set or on predefined hidden types.
pub fn should_rerun_after_erased_canonicalization(tcx: &Tcx, rerun: &RerunCondition, mode: TypingMode, parent: &[(OpaqueTypeKey, TyId)]) -> RerunDecision {
    use RerunCondition::*;
    use RerunDecision::*;
    if *rerun == Never { return No; }
    let owners = match mode {
        TypingMode::ErasedNotCoherence => return EagerlyPropagateToParent,
        TypingMode::Coherence => panic!("coherence does not erase its typing mode"),
        TypingMode::PostAnalysis | TypingMode::Codegen | TypingMode::Reflection => None,
        TypingMode::Typeck { defining_opaque_types_and_generators } => Some(defining_opaque_types_and_generators),
        TypingMode::PostTypeckUntilBorrowck { defining_opaque_types } => Some(defining_opaque_types),
        TypingMode::PostBorrowck { defined_opaque_types } => Some(defined_opaque_types),
    };
    let def_ids = match rerun {
        Never => return No,
        Always => return Yes,
        OpaqueInStorage(def_ids) | OpaqueInStorageOrAnyOpaqueHasInferAsHidden(def_ids) => Some(def_ids),
        AnyOpaqueHasInferAsHidden => None,
    };
    if let Some(def_ids) = def_ids {
        let Some(owners) = owners else { return Yes; };
        for owner in tcx.defining_opaques(owners) { if def_ids.contains(owner) { return Yes; } }
        for (key, _) in parent { if def_ids.contains(&key.def_id) { return Yes; } }
    }
    if matches!(rerun, AnyOpaqueHasInferAsHidden | OpaqueInStorageOrAnyOpaqueHasInferAsHidden(_)) {
        match mode {
            TypingMode::Typeck { .. } => {
                for (_, ty) in parent { if matches!(tcx.tys.kind(*ty), TyKind::Infer(_)) { return Yes; } }
            }
            TypingMode::PostTypeckUntilBorrowck { .. } | TypingMode::PostBorrowck { .. } |
            TypingMode::PostAnalysis | TypingMode::Codegen | TypingMode::Reflection => {},
            TypingMode::Coherence | TypingMode::ErasedNotCoherence => unreachable!(),
        }
    }
    No
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infer::InferCtxt;
    #[test]
    fn rerun_uses_owner_sets_hidden_variables_and_parent_propagation() {
        let tcx = Tcx::new();
        let owner = DefId(1);
        let mode = TypingMode::Typeck { defining_opaque_types_and_generators: tcx.intern_defining_opaques(vec![owner]) };
        let infcx = InferCtxt::new(&tcx, mode, Default::default());
        let parent = vec![(OpaqueTypeKey { def_id: DefId(2), args: Vec::new().into() }, infcx.next_ty_var())];
        assert_eq!(should_rerun_after_erased_canonicalization(&tcx, &RerunCondition::OpaqueInStorage(vec![owner]), mode, &[]), RerunDecision::Yes);
        assert_eq!(should_rerun_after_erased_canonicalization(&tcx, &RerunCondition::OpaqueInStorage(vec![DefId(2)]), mode, &parent), RerunDecision::Yes);
        assert_eq!(should_rerun_after_erased_canonicalization(&tcx, &RerunCondition::OpaqueInStorage(vec![DefId(3)]), mode, &parent), RerunDecision::No);
        assert_eq!(should_rerun_after_erased_canonicalization(&tcx, &RerunCondition::AnyOpaqueHasInferAsHidden, mode, &parent), RerunDecision::Yes);
        assert_eq!(should_rerun_after_erased_canonicalization(&tcx, &RerunCondition::AnyOpaqueHasInferAsHidden, TypingMode::PostAnalysis, &parent), RerunDecision::No);
        assert_eq!(should_rerun_after_erased_canonicalization(&tcx, &RerunCondition::Always, TypingMode::ErasedNotCoherence, &[]), RerunDecision::EagerlyPropagateToParent);
        assert_eq!(should_rerun_after_erased_canonicalization(&tcx, &RerunCondition::Never, TypingMode::Coherence, &[]), RerunDecision::No);
    }
}
