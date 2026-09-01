//! Pipeline specs and the derived-state law.
//!
//! A pipeline is its stages: a small DAG where each stage names a pipe
//! domain, declares which earlier stages feed it, and carries the splices
//! that paste a dependency's output into its own request. The spec is data;
//! the engine that walks it against a hub comes with the chain migrations
//! (asset-ui's `pipeline.rs` execution half, vj's DREAM shape).

use std::collections::HashSet;

/// One named, versioned chain in the library.
#[derive(Clone, Debug)]
pub struct PipelineSpec {
    /// Library identity: `"expand-image-video"`, `"image-mesh-pbr"`, …
    pub name: String,
    pub stages: Vec<StageSpec>,
}

/// One stage of a chain.
#[derive(Clone, Debug)]
pub struct StageSpec {
    /// Stable key inside the pipeline (`"expand"`, `"image"`, `"video"`).
    pub key: String,
    /// The pipe domain this stage executes on (`"text"`, `"image"`, …).
    pub domain: String,
    /// Keys of the stages whose outputs this stage consumes. A splice may
    /// only reference a declared dependency.
    pub deps: Vec<String>,
    /// How much of the whole run this stage is worth in progress terms.
    /// The per-kind table lives HERE so two clients cannot disagree.
    pub weight: u64,
    /// Pinned entropy: a re-picked stage must regenerate identical content.
    pub seed: u64,
    /// A failure here skips the stage instead of failing the run; dependents
    /// proceed with their declared fallbacks (the DREAM expand law: the run
    /// carries on with the typed prompt).
    pub on_fail_skip: bool,
}

/// Default weight for a stage that declares none — the store's old neutral
/// fallback, kept so migrated chains derive identical progress.
pub const DEFAULT_STAGE_WEIGHT: u64 = 10;

/// A stage's observed execution state, as reported by the hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageState {
    Pending,
    Running,
    Done,
    Failed,
    /// Failed, but declared `on_fail: skip` — the run went on without it.
    Skipped,
    Cancelled,
}

/// The whole run's state — NEVER stored, always derived from the stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Pending,
    Running,
    Done,
    /// A failed stage reads as failed IMMEDIATELY, without waiting for its
    /// dependents to notice (the store's lazy-doom lesson).
    Failed,
    Cancelled,
}

/// Validate a spec's graph: unique keys, deps that exist and point backwards
/// (no cycles by construction), at least one stage.
pub fn validate(spec: &PipelineSpec) -> Result<(), String> {
    if spec.stages.is_empty() {
        return Err(format!("pipeline {}: no stages", spec.name));
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for stage in &spec.stages {
        if !seen.insert(&stage.key) {
            return Err(format!("pipeline {}: duplicate stage {}", spec.name, stage.key));
        }
        for dep in &stage.deps {
            if !spec
                .stages
                .iter()
                .take_while(|s| s.key != stage.key)
                .any(|s| &s.key == dep)
            {
                return Err(format!(
                    "pipeline {}: stage {} depends on {}, which is not an earlier stage",
                    spec.name, stage.key, dep
                ));
            }
        }
    }
    Ok(())
}

/// The derived-state law: the run IS its stages.
pub fn derive_state(stages: &[StageState]) -> RunState {
    if stages.iter().any(|s| *s == StageState::Failed) {
        return RunState::Failed;
    }
    if stages.iter().any(|s| *s == StageState::Cancelled) {
        return RunState::Cancelled;
    }
    if stages
        .iter()
        .all(|s| matches!(s, StageState::Done | StageState::Skipped))
    {
        return RunState::Done;
    }
    if stages.iter().all(|s| *s == StageState::Pending) {
        return RunState::Pending;
    }
    RunState::Running
}

/// Weighted progress in [0, 1]: done stages count whole, a running stage
/// counts half its weight (coarse and honest — per-stage fractions arrive
/// with the engine).
pub fn derive_progress(specs: &[StageSpec], states: &[StageState]) -> f64 {
    let total: u64 = specs.iter().map(|s| s.weight.max(1)).sum();
    if total == 0 || specs.len() != states.len() {
        return 0.0;
    }
    let mut earned = 0.0;
    for (spec, state) in specs.iter().zip(states) {
        let w = spec.weight.max(1) as f64;
        earned += match state {
            StageState::Done => w,
            StageState::Running => w * 0.5,
            _ => 0.0,
        };
    }
    earned / total as f64
}

/// Which stages may start now: pending, with every dependency done.
pub fn ready_stages(spec: &PipelineSpec, states: &[StageState]) -> Vec<usize> {
    let mut ready = Vec::new();
    for (i, stage) in spec.stages.iter().enumerate() {
        if states.get(i) != Some(&StageState::Pending) {
            continue;
        }
        let deps_done = stage.deps.iter().all(|dep| {
            matches!(
                spec.stages
                    .iter()
                    .position(|s| &s.key == dep)
                    .and_then(|j| states.get(j)),
                Some(StageState::Done) | Some(StageState::Skipped)
            )
        });
        if deps_done {
            ready.push(i);
        }
    }
    ready
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(key: &str, deps: &[&str]) -> StageSpec {
        StageSpec {
            key: key.into(),
            domain: "text".into(),
            deps: deps.iter().map(|d| d.to_string()).collect(),
            weight: DEFAULT_STAGE_WEIGHT,
            seed: 42,
            on_fail_skip: false,
        }
    }

    fn dream() -> PipelineSpec {
        PipelineSpec {
            name: "expand-image-video".into(),
            stages: vec![
                stage("expand", &[]),
                stage("image", &["expand"]),
                stage("video", &["image"]),
            ],
        }
    }

    #[test]
    fn a_valid_chain_validates_and_bad_ones_do_not() {
        assert!(validate(&dream()).is_ok());
        let mut dup = dream();
        dup.stages.push(stage("image", &[]));
        assert!(validate(&dup).is_err());
        let mut forward = dream();
        forward.stages[0].deps.push("video".into());
        assert!(validate(&forward).is_err(), "forward deps are cycles");
        assert!(validate(&PipelineSpec { name: "empty".into(), stages: vec![] }).is_err());
    }

    #[test]
    fn the_run_is_its_stages() {
        use StageState::*;
        assert_eq!(derive_state(&[Pending, Pending]), RunState::Pending);
        assert_eq!(derive_state(&[Done, Running]), RunState::Running);
        assert_eq!(derive_state(&[Done, Done]), RunState::Done);
        // A failure reads immediately, whatever the dependents are doing.
        assert_eq!(derive_state(&[Done, Failed, Pending]), RunState::Failed);
        assert_eq!(derive_state(&[Done, Cancelled]), RunState::Cancelled);
        // Failure outranks cancellation: it is the fact a person must see.
        assert_eq!(derive_state(&[Failed, Cancelled]), RunState::Failed);
    }

    #[test]
    fn progress_is_weighted_and_bounded() {
        use StageState::*;
        let spec = dream();
        assert_eq!(derive_progress(&spec.stages, &[Pending, Pending, Pending]), 0.0);
        let half = derive_progress(&spec.stages, &[Done, Running, Pending]);
        assert!(half > 0.49 && half < 0.51);
        assert_eq!(derive_progress(&spec.stages, &[Done, Done, Done]), 1.0);
    }

    #[test]
    fn the_deps_gate_opens_stage_by_stage() {
        use StageState::*;
        let spec = dream();
        assert_eq!(ready_stages(&spec, &[Pending, Pending, Pending]), vec![0]);
        assert_eq!(ready_stages(&spec, &[Done, Pending, Pending]), vec![1]);
        assert_eq!(ready_stages(&spec, &[Done, Running, Pending]), Vec::<usize>::new());
        assert_eq!(ready_stages(&spec, &[Done, Done, Pending]), vec![2]);
    }
}
