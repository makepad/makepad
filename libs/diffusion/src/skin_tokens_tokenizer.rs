//! Pure-Rust TokenizerPart grammar for the released SkinTokens rig.
//!
//! The TokenRig vocabulary has two phases. IDs 0..267 encode a skeleton with
//! a small deterministic grammar; skeleton EOS (258) switches generation to
//! exactly four FSQ IDs per decoded bone, after which global EOS (33035) is
//! the only legal token. This is the model's intended/decodable contract. The
//! released Python logits processor has an off-by-one and emits only `4J-1`
//! real FSQs, then accidentally consumes EOS as a modulo-wrapped FSQ index 0
//! for the final joint. Native production deliberately fixes that corruption;
//! oracle compatibility belongs in Qwen validation, not in this strict FSM.
//! Keeping this state machine independent of the Qwen
//! runtime lets beam search mask a compact valid-ID list instead of allocating
//! a full-vocabulary mask at every step.

use crate::skin_tokens::{
    SKIN_TOKENS_FSQ_VOCAB, SKIN_TOKENS_PER_BONE, SKIN_TOKENS_VOCAB,
};
use crate::{DiffusionError, Result};

pub const SKIN_TOKENS_COORD_BINS: usize = 256;
pub const SKIN_TOKENS_TOKEN_BRANCH: u32 = 256;
pub const SKIN_TOKENS_TOKEN_BOS: u32 = 257;
pub const SKIN_TOKENS_TOKEN_SKELETON_EOS: u32 = 258;
pub const SKIN_TOKENS_TOKEN_PAD: u32 = 259;
pub const SKIN_TOKENS_TOKEN_SPRING: u32 = 260;
pub const SKIN_TOKENS_TOKEN_PART_BODY: u32 = 261;
pub const SKIN_TOKENS_TOKEN_PART_HAND: u32 = 262;
pub const SKIN_TOKENS_TOKEN_CLASS_NONE: u32 = 263;
pub const SKIN_TOKENS_TOKEN_CLASS_VROID: u32 = 264;
pub const SKIN_TOKENS_TOKEN_CLASS_MIXAMO: u32 = 265;
pub const SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL: u32 = 266;
pub const SKIN_TOKENS_SKELETON_VOCAB: usize = 267;
pub const SKIN_TOKENS_FSQ_OFFSET: u32 = SKIN_TOKENS_SKELETON_VOCAB as u32;
pub const SKIN_TOKENS_TOKEN_GLOBAL_EOS: u32 = (SKIN_TOKENS_VOCAB - 1) as u32;
pub const SKIN_TOKENS_COORD_MIN: f32 = -1.0;
pub const SKIN_TOKENS_COORD_MAX: f32 = 1.0;

const COORDS: std::ops::Range<u32> = 0..SKIN_TOKENS_COORD_BINS as u32;
const PARTS: [u32; 3] = [
    SKIN_TOKENS_TOKEN_SPRING,
    SKIN_TOKENS_TOKEN_PART_BODY,
    SKIN_TOKENS_TOKEN_PART_HAND,
];
const CLASSES: [u32; 4] = [
    SKIN_TOKENS_TOKEN_CLASS_NONE,
    SKIN_TOKENS_TOKEN_CLASS_VROID,
    SKIN_TOKENS_TOKEN_CLASS_MIXAMO,
    SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkinTokensSkeletonState {
    ExpectBos,
    ExpectClassPartOrJoint,
    ExpectPartOrJoint,
    ExpectJoint2,
    ExpectJoint3,
    ExpectBranchPartOrJoint,
    ExpectJoint,
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkinTokensGenerationPhase {
    Skeleton,
    Skin { bones: usize, generated: usize },
    Complete { bones: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkinTokensSkeleton {
    /// Absolute decoded joint heads in the normalized Blender/model frame.
    pub joints: Vec<[f32; 3]>,
    pub parents: Vec<Option<usize>>,
    pub class_token: Option<u32>,
    /// Part annotations in emission order. They do not affect topology.
    pub parts: Vec<Option<u32>>,
}

#[derive(Clone, Copy, Debug)]
pub enum SkinTokensValidIds {
    One(u32),
    Coordinates,
    ClassPartOrCoordinates,
    PartCoordinatesOrSkeletonEos,
    BranchPartCoordinatesOrSkeletonEos,
    Fsq,
}

/// Incremental, clone-cheap cursor for constrained beam search. Advancing a
/// candidate is O(1), with no full-prefix rescan or full-vocabulary mask.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkinTokensGrammar {
    skeleton_state: SkinTokensSkeletonState,
    bones: usize,
    branch: bool,
    skin_generated: Option<usize>,
    complete: bool,
    offset: usize,
}

impl Default for SkinTokensGrammar {
    fn default() -> Self {
        Self {
            skeleton_state: SkinTokensSkeletonState::ExpectBos,
            bones: 0,
            branch: false,
            skin_generated: None,
            complete: false,
            offset: 0,
        }
    }
}

impl SkinTokensGrammar {
    pub fn from_tokens(ids: &[u32]) -> Result<Self> {
        let mut grammar = Self::default();
        for &id in ids {
            grammar.push(id)?;
        }
        Ok(grammar)
    }

    pub fn push(&mut self, id: u32) -> Result<()> {
        if self.complete {
            return Err(DiffusionError::workflow(format!(
                "SkinTokens token {id} follows global EOS at offset {}",
                self.offset
            )));
        }
        if let Some(generated) = self.skin_generated {
            let required = self.bones * SKIN_TOKENS_PER_BONE;
            if generated < required {
                if !(SKIN_TOKENS_FSQ_OFFSET
                    ..SKIN_TOKENS_FSQ_OFFSET + SKIN_TOKENS_FSQ_VOCAB as u32)
                    .contains(&id)
                {
                    return Err(DiffusionError::workflow(format!(
                        "SkinTokens expected FSQ token at offset {}, found {id}",
                        self.offset
                    )));
                }
                self.skin_generated = Some(generated + 1);
            } else if id == SKIN_TOKENS_TOKEN_GLOBAL_EOS {
                self.complete = true;
            } else {
                return Err(DiffusionError::workflow(format!(
                    "SkinTokens expected global EOS at offset {}, found {id}",
                    self.offset
                )));
            }
            self.offset += 1;
            return Ok(());
        }

        let prior = self.skeleton_state;
        self.skeleton_state = match prior {
            SkinTokensSkeletonState::ExpectBos if id == SKIN_TOKENS_TOKEN_BOS => {
                SkinTokensSkeletonState::ExpectClassPartOrJoint
            }
            SkinTokensSkeletonState::ExpectClassPartOrJoint if COORDS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint2
            }
            SkinTokensSkeletonState::ExpectClassPartOrJoint if CLASSES.contains(&id) => {
                SkinTokensSkeletonState::ExpectPartOrJoint
            }
            SkinTokensSkeletonState::ExpectClassPartOrJoint if PARTS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint
            }
            SkinTokensSkeletonState::ExpectPartOrJoint if COORDS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint2
            }
            SkinTokensSkeletonState::ExpectPartOrJoint if PARTS.contains(&id) => {
                SkinTokensSkeletonState::ExpectPartOrJoint
            }
            SkinTokensSkeletonState::ExpectPartOrJoint
                if id == SKIN_TOKENS_TOKEN_SKELETON_EOS =>
            {
                SkinTokensSkeletonState::Complete
            }
            SkinTokensSkeletonState::ExpectJoint2 if COORDS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint3
            }
            SkinTokensSkeletonState::ExpectJoint3 if COORDS.contains(&id) => {
                if !self.branch {
                    self.bones += 1;
                }
                self.branch = false;
                SkinTokensSkeletonState::ExpectBranchPartOrJoint
            }
            SkinTokensSkeletonState::ExpectBranchPartOrJoint
                if id == SKIN_TOKENS_TOKEN_BRANCH =>
            {
                self.branch = true;
                SkinTokensSkeletonState::ExpectJoint
            }
            SkinTokensSkeletonState::ExpectBranchPartOrJoint if COORDS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint2
            }
            SkinTokensSkeletonState::ExpectBranchPartOrJoint if PARTS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint
            }
            SkinTokensSkeletonState::ExpectBranchPartOrJoint
                if id == SKIN_TOKENS_TOKEN_SKELETON_EOS =>
            {
                SkinTokensSkeletonState::Complete
            }
            SkinTokensSkeletonState::ExpectJoint if COORDS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint2
            }
            _ => {
                return Err(DiffusionError::workflow(format!(
                    "SkinTokens token {id} is invalid for {prior:?} at offset {}",
                    self.offset
                )));
            }
        };
        if self.skeleton_state == SkinTokensSkeletonState::Complete {
            if self.bones == 0 {
                return Err(DiffusionError::workflow(
                    "SkinTokens skeleton EOS completed no bones",
                ));
            }
            self.skin_generated = Some(0);
        }
        self.offset += 1;
        Ok(())
    }

    pub fn valid_next(&self) -> Result<SkinTokensValidIds> {
        if self.complete {
            return Err(DiffusionError::workflow(
                "SkinTokens sequence is already complete",
            ));
        }
        if let Some(generated) = self.skin_generated {
            return Ok(if generated < self.bones * SKIN_TOKENS_PER_BONE {
                SkinTokensValidIds::Fsq
            } else {
                SkinTokensValidIds::One(SKIN_TOKENS_TOKEN_GLOBAL_EOS)
            });
        }
        Ok(match self.skeleton_state {
            SkinTokensSkeletonState::ExpectBos => {
                SkinTokensValidIds::One(SKIN_TOKENS_TOKEN_BOS)
            }
            SkinTokensSkeletonState::ExpectClassPartOrJoint => {
                SkinTokensValidIds::ClassPartOrCoordinates
            }
            SkinTokensSkeletonState::ExpectPartOrJoint => {
                SkinTokensValidIds::PartCoordinatesOrSkeletonEos
            }
            SkinTokensSkeletonState::ExpectJoint2
            | SkinTokensSkeletonState::ExpectJoint3
            | SkinTokensSkeletonState::ExpectJoint => SkinTokensValidIds::Coordinates,
            SkinTokensSkeletonState::ExpectBranchPartOrJoint => {
                SkinTokensValidIds::BranchPartCoordinatesOrSkeletonEos
            }
            SkinTokensSkeletonState::Complete => unreachable!(),
        })
    }

    pub fn phase(&self) -> SkinTokensGenerationPhase {
        if self.complete {
            SkinTokensGenerationPhase::Complete { bones: self.bones }
        } else if let Some(generated) = self.skin_generated {
            SkinTokensGenerationPhase::Skin {
                bones: self.bones,
                generated,
            }
        } else {
            SkinTokensGenerationPhase::Skeleton
        }
    }

    pub fn skeleton_state(&self) -> SkinTokensSkeletonState {
        self.skeleton_state
    }

    pub fn bones(&self) -> usize {
        self.bones
    }
}

impl SkinTokensValidIds {
    pub fn len(self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Coordinates => SKIN_TOKENS_COORD_BINS,
            Self::ClassPartOrCoordinates => CLASSES.len() + PARTS.len() + SKIN_TOKENS_COORD_BINS,
            Self::PartCoordinatesOrSkeletonEos => PARTS.len() + SKIN_TOKENS_COORD_BINS + 1,
            Self::BranchPartCoordinatesOrSkeletonEos => PARTS.len() + SKIN_TOKENS_COORD_BINS + 2,
            Self::Fsq => SKIN_TOKENS_FSQ_VOCAB,
        }
    }

    pub fn contains(self, id: u32) -> bool {
        match self {
            Self::One(only) => id == only,
            Self::Coordinates => COORDS.contains(&id),
            Self::ClassPartOrCoordinates => {
                COORDS.contains(&id) || CLASSES.contains(&id) || PARTS.contains(&id)
            }
            Self::PartCoordinatesOrSkeletonEos => {
                COORDS.contains(&id) || PARTS.contains(&id) || id == SKIN_TOKENS_TOKEN_SKELETON_EOS
            }
            Self::BranchPartCoordinatesOrSkeletonEos => {
                COORDS.contains(&id)
                    || PARTS.contains(&id)
                    || id == SKIN_TOKENS_TOKEN_BRANCH
                    || id == SKIN_TOKENS_TOKEN_SKELETON_EOS
            }
            Self::Fsq => {
                (SKIN_TOKENS_FSQ_OFFSET
                    ..SKIN_TOKENS_FSQ_OFFSET + SKIN_TOKENS_FSQ_VOCAB as u32)
                    .contains(&id)
            }
        }
    }

    pub fn for_each(self, mut emit: impl FnMut(u32)) {
        match self {
            Self::One(id) => emit(id),
            Self::Coordinates => COORDS.for_each(emit),
            Self::ClassPartOrCoordinates => {
                CLASSES.into_iter().for_each(&mut emit);
                PARTS.into_iter().for_each(&mut emit);
                COORDS.for_each(emit);
            }
            Self::PartCoordinatesOrSkeletonEos => {
                PARTS.into_iter().for_each(&mut emit);
                COORDS.for_each(&mut emit);
                emit(SKIN_TOKENS_TOKEN_SKELETON_EOS);
            }
            Self::BranchPartCoordinatesOrSkeletonEos => {
                PARTS.into_iter().for_each(&mut emit);
                emit(SKIN_TOKENS_TOKEN_BRANCH);
                COORDS.for_each(&mut emit);
                emit(SKIN_TOKENS_TOKEN_SKELETON_EOS);
            }
            Self::Fsq => {
                (SKIN_TOKENS_FSQ_OFFSET
                    ..SKIN_TOKENS_FSQ_OFFSET + SKIN_TOKENS_FSQ_VOCAB as u32)
                    .for_each(emit);
            }
        }
    }
}

pub fn skin_tokens_skeleton_state(ids: &[u32]) -> Result<(SkinTokensSkeletonState, usize)> {
    let mut state = SkinTokensSkeletonState::ExpectBos;
    let mut bones = 0usize;
    let mut branch = false;
    for (offset, &id) in ids.iter().enumerate() {
        if state == SkinTokensSkeletonState::Complete {
            return Err(DiffusionError::workflow(format!(
                "SkinTokens skeleton has token {id} after EOS at offset {offset}",
            )));
        }
        state = match state {
            SkinTokensSkeletonState::ExpectBos if id == SKIN_TOKENS_TOKEN_BOS => {
                SkinTokensSkeletonState::ExpectClassPartOrJoint
            }
            SkinTokensSkeletonState::ExpectClassPartOrJoint if COORDS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint2
            }
            SkinTokensSkeletonState::ExpectClassPartOrJoint if CLASSES.contains(&id) => {
                SkinTokensSkeletonState::ExpectPartOrJoint
            }
            SkinTokensSkeletonState::ExpectClassPartOrJoint if PARTS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint
            }
            SkinTokensSkeletonState::ExpectPartOrJoint if COORDS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint2
            }
            SkinTokensSkeletonState::ExpectPartOrJoint if PARTS.contains(&id) => {
                SkinTokensSkeletonState::ExpectPartOrJoint
            }
            SkinTokensSkeletonState::ExpectPartOrJoint if id == SKIN_TOKENS_TOKEN_SKELETON_EOS => {
                SkinTokensSkeletonState::Complete
            }
            SkinTokensSkeletonState::ExpectJoint2 if COORDS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint3
            }
            SkinTokensSkeletonState::ExpectJoint3 if COORDS.contains(&id) => {
                if !branch {
                    bones += 1;
                }
                branch = false;
                SkinTokensSkeletonState::ExpectBranchPartOrJoint
            }
            SkinTokensSkeletonState::ExpectBranchPartOrJoint if id == SKIN_TOKENS_TOKEN_BRANCH => {
                branch = true;
                SkinTokensSkeletonState::ExpectJoint
            }
            SkinTokensSkeletonState::ExpectBranchPartOrJoint if COORDS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint2
            }
            SkinTokensSkeletonState::ExpectBranchPartOrJoint if PARTS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint
            }
            SkinTokensSkeletonState::ExpectBranchPartOrJoint
                if id == SKIN_TOKENS_TOKEN_SKELETON_EOS =>
            {
                SkinTokensSkeletonState::Complete
            }
            SkinTokensSkeletonState::ExpectJoint if COORDS.contains(&id) => {
                SkinTokensSkeletonState::ExpectJoint2
            }
            _ => {
                return Err(DiffusionError::workflow(format!(
                    "SkinTokens token {id} is invalid for {state:?} at skeleton offset {offset}",
                )));
            }
        };
    }
    Ok((state, bones))
}

pub fn skin_tokens_generation_phase(
    start_tokens: &[u32],
    generated: &[u32],
) -> Result<SkinTokensGenerationPhase> {
    let mut grammar = SkinTokensGrammar::from_tokens(start_tokens)?;
    for &id in generated {
        grammar.push(id)?;
    }
    Ok(grammar.phase())
}

pub fn skin_tokens_valid_next(
    start_tokens: &[u32],
    generated: &[u32],
) -> Result<SkinTokensValidIds> {
    let mut grammar = SkinTokensGrammar::from_tokens(start_tokens)?;
    for &id in generated {
        grammar.push(id)?;
    }
    grammar.valid_next()
}

/// Mirror upstream `undiscretize`: bin center mapped from 0..256 to [-1, 1].
pub fn skin_tokens_undiscretize(id: u32) -> Result<f32> {
    if !COORDS.contains(&id) {
        return Err(DiffusionError::workflow(format!("SkinTokens coordinate token {id} is outside 0..256")));
    }
    Ok((id as f32 + 0.5) / SKIN_TOKENS_COORD_BINS as f32
        * (SKIN_TOKENS_COORD_MAX - SKIN_TOKENS_COORD_MIN)
        + SKIN_TOKENS_COORD_MIN)
}

pub fn skin_tokens_detokenize_skeleton(ids: &[u32]) -> Result<SkinTokensSkeleton> {
    let (state, expected_bones) = skin_tokens_skeleton_state(ids)?;
    if state != SkinTokensSkeletonState::Complete {
        return Err(DiffusionError::workflow(format!("SkinTokens skeleton is incomplete at {state:?}")));
    }
    let mut joints = Vec::with_capacity(expected_bones);
    let mut parent_positions = Vec::with_capacity(expected_bones);
    let mut parts = Vec::new();
    let mut class_token = None;
    let mut branch = false;
    let mut last_joint = None;
    let mut index = 1usize; // BOS
    while index + 1 < ids.len() {
        let id = ids[index];
        if id == SKIN_TOKENS_TOKEN_SKELETON_EOS {
            break;
        }
        if COORDS.contains(&id) {
            let read_point = |at: usize| -> Result<[f32; 3]> {
                Ok([
                    skin_tokens_undiscretize(ids[at])?,
                    skin_tokens_undiscretize(ids[at + 1])?,
                    skin_tokens_undiscretize(ids[at + 2])?,
                ])
            };
            let (parent_position, joint, consumed) = if branch {
                (read_point(index)?, read_point(index + 3)?, 6)
            } else {
                let joint = read_point(index)?;
                (last_joint.unwrap_or(joint), joint, 3)
            };
            joints.push(joint);
            parent_positions.push(parent_position);
            last_joint = Some(joint);
            branch = false;
            index += consumed;
        } else if id == SKIN_TOKENS_TOKEN_BRANCH {
            branch = true;
            last_joint = None;
            index += 1;
        } else if PARTS.contains(&id) {
            parts.push(if id == SKIN_TOKENS_TOKEN_SPRING { None } else { Some(id) });
            index += 1;
        } else if CLASSES.contains(&id) {
            class_token = (id != SKIN_TOKENS_TOKEN_CLASS_NONE).then_some(id);
            index += 1;
        } else {
            return Err(DiffusionError::workflow(format!("unexpected SkinTokens skeleton token {id}")));
        }
    }
    let mut parents = Vec::with_capacity(joints.len());
    for (joint_index, parent_position) in parent_positions.iter().enumerate() {
        if joint_index == 0 {
            parents.push(None);
            continue;
        }
        let mut parent = joint_index - 1;
        let mut best = f32::INFINITY;
        for candidate in (0..joint_index).rev() {
            let distance = squared_distance(joints[candidate], *parent_position);
            // Exact upstream behavior: reversed scan with strict `<`, so a
            // tie selects the most recently emitted prior joint.
            if distance < best {
                best = distance;
                parent = candidate;
            }
        }
        parents.push(Some(parent));
    }
    if joints.len() != expected_bones {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens decoded {} joints, grammar counted {expected_bones}", joints.len()
        )));
    }
    Ok(SkinTokensSkeleton { joints, parents, class_token, parts })
}

fn squared_distance(left: [f32; 3], right: [f32; 3]) -> f32 {
    left.into_iter().zip(right).map(|(left, right)| (left - right) * (left - right)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> Vec<u32> {
        vec![
            SKIN_TOKENS_TOKEN_BOS,
            SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL,
            SKIN_TOKENS_TOKEN_PART_BODY,
            128, 128, 128,
            128, 192, 128,
            SKIN_TOKENS_TOKEN_BRANCH,
            128, 128, 128,
            192, 128, 128,
            SKIN_TOKENS_TOKEN_SKELETON_EOS,
        ]
    }

    #[test]
    fn exact_vocab_layout_is_self_consistent() {
        assert_eq!(SKIN_TOKENS_SKELETON_VOCAB + SKIN_TOKENS_FSQ_VOCAB + 1, SKIN_TOKENS_VOCAB);
        assert_eq!(SKIN_TOKENS_TOKEN_GLOBAL_EOS, 33_035);
        assert_eq!(SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL, 266);
    }

    #[test]
    fn fixed_generation_oracle_skeleton_detokenizes_exactly() {
        // Official eval-mode, seed-424242 Mario generation oracle. Keeping
        // this compact token boundary in-tree prevents grammar/parent changes
        // from being hidden behind a later neural tolerance.
        let ids = [
            257, 266, 129, 135, 86, 129, 132, 99, 129, 131, 113, 129, 133, 128, 129,
            134, 146, 129, 131, 155, 256, 129, 133, 128, 140, 134, 143, 163, 134, 136,
            173, 132, 120, 179, 123, 100, 178, 118, 88, 177, 114, 78, 176, 109, 66,
            256, 179, 123, 100, 182, 117, 83, 183, 119, 75, 184, 121, 65, 256, 129,
            133, 128, 116, 134, 143, 94, 134, 136, 85, 135, 120, 76, 127, 100, 77,
            122, 89, 78, 117, 79, 78, 111, 69, 256, 76, 127, 100, 74, 124, 87,
            73, 125, 76, 72, 126, 67, 256, 129, 135, 86, 147, 134, 80, 151, 136,
            53, 154, 138, 37, 170, 115, 2, 256, 129, 135, 86, 111, 137, 80, 107,
            142, 53, 105, 146, 37, 86, 127, 2, 258,
        ];
        let skeleton = skin_tokens_detokenize_skeleton(&ids).unwrap();
        assert_eq!(skeleton.joints.len(), 34);
        assert_eq!(
            skeleton.parents,
            [
                None,
                Some(0), Some(1), Some(2), Some(3), Some(4), Some(3), Some(6),
                Some(7), Some(8), Some(9), Some(10), Some(11), Some(9), Some(13),
                Some(14), Some(3), Some(16), Some(17), Some(18), Some(19), Some(20),
                Some(21), Some(19), Some(23), Some(24), Some(0), Some(26), Some(27),
                Some(28), Some(0), Some(30), Some(31), Some(32),
            ]
        );
        assert_eq!(skeleton.joints[0], [0.01171875, 0.05859375, -0.32421875]);
        assert_eq!(skeleton.joints[33], [-0.32421875, -0.00390625, -0.98046875]);
    }

    #[test]
    fn grammar_counts_branches_and_emits_compact_valid_sets() {
        let ids = chain();
        assert_eq!(skin_tokens_skeleton_state(&ids).unwrap(), (SkinTokensSkeletonState::Complete, 3));
        let start = &ids[..2];
        let generated = &ids[2..ids.len() - 1];
        let valid = skin_tokens_valid_next(start, generated).unwrap();
        assert!(valid.contains(SKIN_TOKENS_TOKEN_SKELETON_EOS));
        assert!(valid.contains(SKIN_TOKENS_TOKEN_BRANCH));
        assert!(!valid.contains(SKIN_TOKENS_TOKEN_GLOBAL_EOS));
    }

    #[test]
    fn detokenizes_chain_and_nearest_prior_branch_parent() {
        let decoded = skin_tokens_detokenize_skeleton(&chain()).unwrap();
        assert_eq!(decoded.joints.len(), 3);
        assert_eq!(decoded.parents, vec![None, Some(0), Some(0)]);
        assert_eq!(decoded.class_token, Some(SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL));
        assert!((decoded.joints[1][1] - 0.50390625).abs() < 1.0e-7);
    }

    #[test]
    fn switches_to_exact_skin_count_then_global_eos() {
        let ids = chain();
        let start = &ids[..2];
        let mut generated = ids[2..].to_vec();
        assert_eq!(skin_tokens_generation_phase(start, &generated).unwrap(), SkinTokensGenerationPhase::Skin { bones: 3, generated: 0 });
        generated.extend(std::iter::repeat(SKIN_TOKENS_FSQ_OFFSET).take(12));
        assert!(matches!(skin_tokens_valid_next(start, &generated), Ok(SkinTokensValidIds::One(SKIN_TOKENS_TOKEN_GLOBAL_EOS))));
        generated.push(SKIN_TOKENS_TOKEN_GLOBAL_EOS);
        assert_eq!(skin_tokens_generation_phase(start, &generated).unwrap(), SkinTokensGenerationPhase::Complete { bones: 3 });
    }

    #[test]
    fn strict_mode_rejects_the_released_eos_as_last_fsq_bug() {
        let skeleton = chain();
        let mut grammar = SkinTokensGrammar::from_tokens(&skeleton).unwrap();
        let required = grammar.bones() * SKIN_TOKENS_PER_BONE;
        for _ in 0..required - 1 {
            grammar.push(SKIN_TOKENS_FSQ_OFFSET).unwrap();
        }
        let error = grammar.push(SKIN_TOKENS_TOKEN_GLOBAL_EOS).unwrap_err();
        assert!(error.to_string().contains("expected FSQ token"));
        grammar.push(SKIN_TOKENS_FSQ_OFFSET).unwrap();
        grammar.push(SKIN_TOKENS_TOKEN_GLOBAL_EOS).unwrap();
        assert!(matches!(
            grammar.phase(),
            SkinTokensGenerationPhase::Complete { bones: 3 }
        ));
    }

    #[test]
    fn incremental_cursor_matches_prefix_helpers() {
        let ids = chain();
        let mut grammar = SkinTokensGrammar::default();
        for (index, &id) in ids.iter().enumerate() {
            assert!(grammar.valid_next().unwrap().contains(id), "offset {index}");
            grammar.push(id).unwrap();
        }
        assert_eq!(grammar.phase(), SkinTokensGenerationPhase::Skin { bones: 3, generated: 0 });
        for _ in 0..12 {
            let mut candidate = grammar.clone();
            candidate.push(SKIN_TOKENS_FSQ_OFFSET + 17).unwrap();
            grammar = candidate;
        }
        assert!(matches!(grammar.valid_next(), Ok(SkinTokensValidIds::One(SKIN_TOKENS_TOKEN_GLOBAL_EOS))));
        grammar.push(SKIN_TOKENS_TOKEN_GLOBAL_EOS).unwrap();
        assert_eq!(grammar.phase(), SkinTokensGenerationPhase::Complete { bones: 3 });
    }

    #[test]
    fn malformed_streams_fail_closed() {
        assert!(skin_tokens_skeleton_state(&[SKIN_TOKENS_TOKEN_BOS, 1, 2, SKIN_TOKENS_TOKEN_SKELETON_EOS]).is_err());
        let mut bad = chain();
        bad[5] = SKIN_TOKENS_TOKEN_GLOBAL_EOS;
        assert!(skin_tokens_detokenize_skeleton(&bad).is_err());
    }
}
