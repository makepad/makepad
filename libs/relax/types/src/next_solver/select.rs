// ported from N:rustc_trait_selection/src/solve/select.rs @8d1a764
use super::*;
use super::assembly::{BuiltinImplSource, CandidateSource};
use super::inspect::{InspectCandidate, InspectGoal, ProbeKind};
use crate::infer::InferCtxt;
use crate::program::{DefId, Program};

#[derive(Clone, Debug, Hash)]
pub enum ImplSource {
    UserDefined { impl_def_id: DefId, args: Vec<GenericArg>, nested: Vec<Goal> },
    Builtin(BuiltinImplSource, Vec<Goal>), Param(Vec<Goal>),
}
impl InferCtxt<'_> {
    pub fn select_in_new_trait_solver(&self, prog: &Program, goal: Goal) -> Result<Option<ImplSource>, NoSolution> {
        let root = self.evaluate_root_goal_for_proof_tree(prog, goal);
        Select.visit_goal(&InspectGoal::new(self, prog, root))
    }
}
struct Select;
impl Select {
    fn visit_goal(&self, goal: &InspectGoal<'_, '_>) -> Result<Option<ImplSource>, NoSolution> {
        let mut candidates = goal.candidates(); candidates.retain(|c| c.result.is_ok());
        if candidates.is_empty() { return Err(NoSolution); }
        if candidates.len() == 1 { return Ok(to_selection(candidates.remove(0))); }
        if !goal.result?.is_yes() { return Ok(None); }
        let mut i = 0;
        while i < candidates.len() {
            let mut drop = false;
            for j in 0..candidates.len() {
                if i != j && candidate_should_be_dropped_in_favor_of(&candidates[i], &candidates[j]) { drop = true; break; }
            }
            if drop { candidates.swap_remove(i); } else { i += 1; if i > 1 { return Ok(None); } }
        }
        Ok(to_selection(candidates.remove(0)))
    }
}
fn candidate_should_be_dropped_in_favor_of(victim: &InspectCandidate<'_, '_>, other: &InspectCandidate<'_, '_>) -> bool {
    if !other.result.is_ok_and(|c| c.is_yes()) { return false; }
    let (ProbeKind::TraitCandidate { source: a, .. }, ProbeKind::TraitCandidate { source: b, .. }) = (victim.kind, other.kind) else { return false; };
    match (a, b) {
        (CandidateSource::BuiltinImpl(BuiltinImplSource::Object(a)), CandidateSource::BuiltinImpl(BuiltinImplSource::Object(b))) |
        (CandidateSource::BuiltinImpl(BuiltinImplSource::TraitUpcasting(a)), CandidateSource::BuiltinImpl(BuiltinImplSource::TraitUpcasting(b))) => a >= b,
        (CandidateSource::Impl(_) | CandidateSource::ParamEnv(_) | CandidateSource::AliasBound(_), CandidateSource::BuiltinImpl(BuiltinImplSource::Object(_))) => true,
        (CandidateSource::ParamEnv(true), CandidateSource::Impl(_) | CandidateSource::BuiltinImpl(_)) => true,
        // DEBLOAT(round9): collection rejects specialization; no impl specializes another.
        _ => false,
    }
}
fn to_selection(candidate: InspectCandidate<'_, '_>) -> Option<ImplSource> {
    if !candidate.shallow_certainty.is_yes() { return None; }
    let nested = if candidate.result.expect("positive candidate").is_yes() { Vec::new() } else { candidate.instantiate_nested_goals() };
    Some(match candidate.kind {
        ProbeKind::TraitCandidate { source: CandidateSource::Impl(impl_def_id), .. } => ImplSource::UserDefined { impl_def_id: *impl_def_id, args: candidate.instantiate_impl_args(), nested },
        ProbeKind::TraitCandidate { source: CandidateSource::BuiltinImpl(source), .. } => ImplSource::Builtin(*source, nested),
        ProbeKind::TraitCandidate { source: CandidateSource::ParamEnv(_) | CandidateSource::AliasBound(_), .. } => ImplSource::Param(nested),
        _ => panic!("non-trait selection candidate"),
    })
}
