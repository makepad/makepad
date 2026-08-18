//! Canonical native HY-Motion service backend.
//!
//! Artifact preparation is intentionally delegated to the common service
//! lifecycle. This module never downloads files and never consults process
//! environment variables: it resolves immutable, verified artifacts by
//! semantic registry roles, then constructs the full native runtime on a
//! dedicated persistent worker. The Python/Torch/Blender reference remains a
//! separately named oracle backend and is never a fallback for this one.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::motion_backend::check_motion_output;
use crate::motion_retarget::{
    classify_humanoid_branches, retarget_hy_motion_glb_with_report, HyMotionClipRef,
    RetargetOptions,
};
use makepad_diffusion::hy_motion_pipeline::{
    HyMotionGenerateParams, HyMotionModelPaths, HyMotionPipeline, HyMotionRunControl,
};
use makepad_diffusion::hy_motion::{HY_MOTION_BODY_JOINTS, HY_MOTION_CFG};
use makepad_diffusion::hy_motion_decode::HyMotionDecoded;
use makepad_diffusion::DiffusionError;
use makepad_gltf::parse_glb_bytes;
use makepad_render::skin::SkinnedModel;
use makepad_micro_serde::JsonValue;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

pub const HY_MOTION_NATIVE_BACKEND: &str = "motion-native";
pub const HY_MOTION_NATIVE_MODEL: &str = "hy-motion";

pub const ROLE_QWEN_SHARDS: [&str; 5] = [
    "qwen-shard-1",
    "qwen-shard-2",
    "qwen-shard-3",
    "qwen-shard-4",
    "qwen-shard-5",
];
pub const ROLE_QWEN_TOKENIZER: &str = "qwen-tokenizer";
pub const ROLE_CLIP_MODEL: &str = "clip-model";
pub const ROLE_MOTION_CHECKPOINT: &str = "motion-checkpoint";
pub const ROLE_WOODEN_FILES: [&str; 6] = [
    "wooden-v-template",
    "wooden-j-template",
    "wooden-kintree",
    "wooden-skin-weights",
    "wooden-skin-indices",
    "wooden-joint-names",
];

/// The playable-character motion contract. Appearance/style never belongs in
/// these prompts: HY-Motion was trained for body actions, while the generated
/// character's appearance is already fixed in the input GLB.
pub const HY_MOTION_CLIP_RECIPES: [HyMotionClipRecipe; 5] = [
    HyMotionClipRecipe {
        name: "idle",
        prompt: "A person stands in a relaxed neutral idle pose, with subtle breathing, feet apart and arms resting at their sides",
        frames: 120,
    },
    HyMotionClipRecipe {
        name: "walk",
        // Released HY-Motion example prompt/frame pair. Keeping the command
        // simple avoids asking the model to choreograph gait constraints that
        // its non-looping training contract does not promise.
        prompt: "A person walks forward.",
        frames: 120,
    },
    HyMotionClipRecipe {
        name: "jump",
        // Released HY-Motion single-jump example prompt/frame pair.
        prompt: "A person jumps up.",
        frames: 90,
    },
    HyMotionClipRecipe {
        name: "run",
        // Keep this as a single canonical action, like the released walk
        // prompt. It produces a genuinely different gait instead of speeding
        // up the walk keys in the game host. Appending it preserves the seed
        // streams of the first three already-accepted idle/walk/jump clips.
        prompt: "A person runs forward.",
        frames: 120,
    },
    HyMotionClipRecipe {
        name: "dance",
        // Body-action only, one person, no environment/appearance — the same
        // contract as the locomotion prompts. Unlike them, dance is a finite
        // performance clip: it is NOT closed into a native cycle (the VJ
        // playback layer loops the finite clip). Appending it preserves the
        // seed streams of the four already-accepted idle/walk/jump/run clips.
        prompt: "A person dances energetically in place, stepping side to side with rhythmic arm swings and hip sways.",
        // 5 seconds at the model's 30fps: inside the released short-clip
        // band (the example pairs are 3-4s) and far under the 360-frame
        // training window, where short single actions are strongest.
        frames: 150,
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HyMotionClipRecipe {
    pub name: &'static str,
    pub prompt: &'static str,
    pub frames: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResolvedPaths {
    paths: HyMotionModelPaths,
}

impl ResolvedPaths {
    fn from_ctx(ctx: &BackendCtx) -> Result<Self, AssetAiError> {
        let qwen_shards: Vec<PathBuf> = ROLE_QWEN_SHARDS
            .iter()
            .map(|role| ctx.path_by_role(role))
            .collect::<Result<_, _>>()?;
        let qwen_tokenizer = ctx.path_by_role(ROLE_QWEN_TOKENIZER)?;
        let qwen_dir = require_same_parent("Qwen", &qwen_shards, &qwen_tokenizer)?;

        let clip = ctx.path_by_role(ROLE_CLIP_MODEL)?;
        let checkpoint = ctx.path_by_role(ROLE_MOTION_CHECKPOINT)?;
        let wooden_files: Vec<PathBuf> = ROLE_WOODEN_FILES
            .iter()
            .map(|role| ctx.path_by_role(role))
            .collect::<Result<_, _>>()?;
        let wooden_dir = require_common_parent("WoodenMesh", &wooden_files)?;
        Ok(Self {
            paths: HyMotionModelPaths::new(qwen_dir, clip, checkpoint, wooden_dir),
        })
    }
}

fn require_same_parent(
    label: &str,
    siblings: &[PathBuf],
    extra: &Path,
) -> Result<PathBuf, AssetAiError> {
    let mut all = siblings.to_vec();
    all.push(extra.to_path_buf());
    require_common_parent(label, &all)
}

fn require_common_parent(label: &str, paths: &[PathBuf]) -> Result<PathBuf, AssetAiError> {
    let Some(parent) = paths.first().and_then(|path| path.parent()) else {
        return Err(AssetAiError::Backend(format!(
            "HY-Motion {label} artifact has no parent directory"
        )));
    };
    if paths.iter().any(|path| path.parent() != Some(parent)) {
        return Err(AssetAiError::Backend(format!(
            "HY-Motion {label} artifacts must share one cache directory"
        )));
    }
    Ok(parent.to_path_buf())
}

fn diffusion_error(error: DiffusionError) -> WorkerError {
    match error {
        DiffusionError::Cancelled => WorkerError::Cancelled,
        other => WorkerError::Other(other.to_string()),
    }
}

fn validate_rigged_glb(bytes: &[u8]) -> Result<(), AssetAiError> {
    let parsed = parse_glb_bytes(bytes)
        .map_err(|error| AssetAiError::Params(format!("input_b64 is not a valid GLB: {error}")))?;
    let has_joints = parsed.document.skins.as_deref().is_some_and(|skins| {
        skins.iter().any(|skin| {
            matches!(skin.key("joints"), Some(JsonValue::Array(joints)) if !joints.is_empty())
        })
    });
    if !has_joints {
        return Err(AssetAiError::Params(
            "input GLB is not rigged (no skin joints) — run the rig domain first".to_string(),
        ));
    }
    let branches = classify_humanoid_branches(bytes).map_err(|error| {
        AssetAiError::Params(format!(
            "character-rig-quality: humanoid hierarchy classification failed: {error}"
        ))
    })?;
    let model = SkinnedModel::parse_glb(bytes).map_err(|error| {
        AssetAiError::Params(format!(
            "character-rig-quality: skinned mesh audit could not parse input: {error}"
        ))
    })?;
    let audit = model
        .audit_semantic_bridges(&branches.arm_nodes, &branches.leg_nodes, 0.55)
        .map_err(|error| {
            AssetAiError::Params(format!(
                "character-rig-quality: semantic bridge audit failed closed: {error}"
            ))
        })?;
    if audit.bridge_triangles > 0 {
        return Err(AssetAiError::Params(format!(
            "character-rig-quality: arm/leg topology bridge: faces={} first_face={} rest_area={:.8} area_fraction={:.8} confidence=0.55 arm_confidence={:.4} leg_confidence={:.4}",
            audit.bridge_triangles,
            audit.first_bridge_face.unwrap_or(usize::MAX),
            audit.bridge_rest_area,
            audit.bridge_rest_area_fraction,
            audit.max_arm_confidence,
            audit.max_leg_confidence,
        )));
    }
    Ok(())
}

fn validate_animated_glb_quality(bytes: &[u8]) -> Result<(), AssetAiError> {
    let model = SkinnedModel::parse_glb(bytes).map_err(|error| {
        AssetAiError::Params(format!(
            "character-motion-quality: animated skin audit could not parse output: {error}"
        ))
    })?;
    let audit = model.audit_authored_motion_quality().map_err(|error| {
        AssetAiError::Params(format!(
            "character-motion-quality: authored-key deformation audit failed closed: {error}"
        ))
    })?;
    if audit.bad_triangles == 0 {
        return Ok(());
    }
    let clip_index = audit.worst_clip.unwrap_or(usize::MAX);
    let clip_name = model
        .clips
        .get(clip_index)
        .map(|clip| clip.name.as_str())
        .unwrap_or("<unknown>");
    Err(AssetAiError::Params(format!(
        "character-motion-quality: visible skin stretch: bad_faces={} union_rest_area={:.8} union_area_fraction={:.8} max_stretch={:.4} max_extension_height={:.6} clip={} clip_index={} authored_frame={} time={:.6} face={}",
        audit.bad_triangles,
        audit.bad_rest_area,
        audit.bad_rest_area_fraction,
        audit.max_stretch,
        audit.max_extension_height,
        clip_name,
        clip_index,
        audit.worst_authored_frame.unwrap_or(usize::MAX),
        audit.worst_time_seconds,
        audit.worst_face.unwrap_or(usize::MAX),
    )))
}

fn clip_seed(request_seed: u64, clip_index: usize) -> u64 {
    // Stable SplitMix64 derivation: each canonical clip gets an independent
    // stream while identical requests remain bit-deterministic.
    let mut value = request_seed
        .wrapping_add((clip_index as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Reserve 2% for request setup and 11% for retarget/validation. The rest is
/// divided evenly between however many native clips the recipe contract has,
/// so appending a clip (`run`, `dance`) cannot make progress exceed 1.0.
fn clip_generation_progress(clip_index: usize, clip_count: usize, local: f64) -> f64 {
    const START: f64 = 0.02;
    const END: f64 = 0.89;
    if clip_count == 0 {
        return START;
    }
    let span = (END - START) / clip_count as f64;
    START + span * (clip_index.min(clip_count - 1) as f64 + local.clamp(0.0, 1.0))
}

/// HY-Motion pads a short jump with a standing lead-in and recovery. Find
/// the physical action from the decoded root-Y curve: deepest pre-apex crouch
/// through deepest post-apex landing. This is measured before playable root
/// translation is stripped, and keeps the body pose and controller clock on
/// the same takeoff/apex/landing event.
fn trim_jump_action(motion: &HyMotionDecoded) -> HyMotionDecoded {
    let frames = motion.frames;
    if frames < 20 || motion.translations.len() != frames * 3 {
        return motion.clone();
    }
    let y = |frame: usize| motion.translations[frame * 3 + 1];
    let apex = (0..frames)
        .max_by(|&left, &right| y(left).total_cmp(&y(right)))
        .unwrap_or(0);
    // A boundary "apex" is a flat/non-jump result; preserve it for quality
    // auditing rather than manufacturing an arbitrary crop.
    if apex < frames / 5 || apex + frames / 5 >= frames {
        return motion.clone();
    }
    let crouch = (0..=apex)
        .min_by(|&left, &right| y(left).total_cmp(&y(right)))
        .unwrap_or(0);
    let landing = (apex..frames)
        .min_by(|&left, &right| y(left).total_cmp(&y(right)))
        .unwrap_or(frames - 1);
    let end = landing + 1;
    if crouch >= apex || apex >= landing || end - crouch < 20 {
        return motion.clone();
    }

    fn rows(values: &[f32], frames: usize, start: usize, end: usize) -> Vec<f32> {
        if frames == 0 || values.len() % frames != 0 {
            return values.to_vec();
        }
        let stride = values.len() / frames;
        values[start * stride..end * stride].to_vec()
    }
    HyMotionDecoded {
        frames: end - crouch,
        latent_denorm: rows(&motion.latent_denorm, frames, crouch, end),
        rotations_6d: rows(&motion.rotations_6d, frames, crouch, end),
        translations: rows(&motion.translations, frames, crouch, end),
        local_rotation_matrices: rows(
            &motion.local_rotation_matrices,
            frames,
            crouch,
            end,
        ),
        root_rotation_matrices: rows(
            &motion.root_rotation_matrices,
            frames,
            crouch,
            end,
        ),
        keypoints_3d: rows(&motion.keypoints_3d, frames, crouch, end),
    }
}

const WOODEN_JOINTS: usize = 52;
const MIN_LOOP_FRAMES: usize = 60;

#[derive(Clone, Copy, Debug)]
struct LoopWindow {
    start: usize,
    end: usize,
    score: f32,
}

/// HY-Motion generates finite actions, not loop clips. Select a repeated gait
/// phase in the source skeleton before retargeting, then sample it as a closed
/// cycle. All measurements live in WoodenMesh space, so this does not depend
/// on the generated character's proportions, topology, or joint naming.
fn close_cyclic_motion(motion: &HyMotionDecoded, output_frames: usize) -> Option<HyMotionDecoded> {
    if output_frames < 2 || !decoded_rows_are_consistent(motion) {
        return None;
    }
    for window in find_loop_windows(motion)? {
        if let Some(closed) = close_cyclic_motion_window(motion, output_frames, window) {
            return Some(closed);
        }
    }
    None
}

fn close_cyclic_motion_window(
    motion: &HyMotionDecoded,
    output_frames: usize,
    window: LoopWindow,
) -> Option<HyMotionDecoded> {
    let span = window.end.checked_sub(window.start)?;
    if span < 2 {
        return None;
    }
    // Keep the model's cadence instead of stretching a two-cycle source
    // window across the whole four-second GLB clip. An integer repeat count
    // guarantees that the exported clip itself remains cyclic.
    let repeats = (((output_frames - 1) as f32 / span as f32).round() as usize).max(1);
    let mut out = HyMotionDecoded {
        frames: output_frames,
        latent_denorm: resample_projected_rows(
            &motion.latent_denorm, motion.frames, output_frames, window, repeats,
        )?,
        rotations_6d: vec![0.0; output_frames * HY_MOTION_BODY_JOINTS * 6],
        translations: resample_projected_rows(
            &motion.translations, motion.frames, output_frames, window, repeats,
        )?,
        local_rotation_matrices: resample_projected_rotations(
            &motion.local_rotation_matrices,
            motion.frames,
            output_frames,
            HY_MOTION_BODY_JOINTS,
            window,
            repeats,
        )?,
        root_rotation_matrices: resample_projected_rotations(
            &motion.root_rotation_matrices,
            motion.frames,
            output_frames,
            1,
            window,
            repeats,
        )?,
        keypoints_3d: resample_projected_rows(
            &motion.keypoints_3d, motion.frames, output_frames, window, repeats,
        )?,
    };
    sync_rotation_6d(&mut out);
    sync_latent_active_channels(&mut out);
    copy_first_row_to_last(&mut out.latent_denorm, output_frames);
    copy_first_row_to_last(&mut out.rotations_6d, output_frames);
    copy_first_row_to_last(&mut out.translations, output_frames);
    copy_first_row_to_last(&mut out.local_rotation_matrices, output_frames);
    copy_first_row_to_last(&mut out.root_rotation_matrices, output_frames);
    copy_first_row_to_last(&mut out.keypoints_3d, output_frames);
    cyclic_postcondition(&out, repeats).then_some(out)
}

fn decoded_rows_are_consistent(motion: &HyMotionDecoded) -> bool {
    let frames = motion.frames;
    frames >= MIN_LOOP_FRAMES
        && motion.latent_denorm.len() % frames == 0
        && motion.rotations_6d.len() == frames * HY_MOTION_BODY_JOINTS * 6
        && motion.translations.len() == frames * 3
        && motion.local_rotation_matrices.len() == frames * HY_MOTION_BODY_JOINTS * 9
        && motion.root_rotation_matrices.len() == frames * 9
        && motion.keypoints_3d.len() == frames * WOODEN_JOINTS * 3
        && motion
            .latent_denorm
            .iter()
            .chain(&motion.rotations_6d)
            .chain(&motion.translations)
            .chain(&motion.local_rotation_matrices)
            .chain(&motion.root_rotation_matrices)
            .chain(&motion.keypoints_3d)
            .all(|value| value.is_finite())
}

fn find_loop_windows(motion: &HyMotionDecoded) -> Option<Vec<LoopWindow>> {
    let frames = motion.frames;
    let min_span = (frames / 2).max(MIN_LOOP_FRAMES).min(frames - 1);
    let height = wooden_motion_height(motion).max(1.0e-4);
    let mut candidates = Vec::new();
    for start in 0..frames - min_span {
        for end in start + min_span..frames {
            let pose = rotation_pose_distance(motion, start, end)?;
            let keypoints = centered_keypoint_distance(motion, start, end) / height;
            let velocity = rotation_velocity_distance(motion, start, end)?;
            let key_velocity = centered_keypoint_velocity_distance(motion, start, end) / height;
            // Reject a phase coincidence whose adjacent motion disagrees: it
            // would merely move the pop from position to velocity.
            if pose > 0.35 || keypoints > 0.08 || velocity > 0.20 || key_velocity > 0.04 {
                continue;
            }
            let trim = (frames - 1 - (end - start)) as f32 / (frames - 1) as f32;
            let score = pose / 0.20
                + keypoints / 0.04
                + velocity / 0.12
                + key_velocity / 0.02
                + trim * 0.30;
            candidates.push(LoopWindow { start, end, score });
        }
    }
    candidates.sort_by(|left, right| {
        left.score
            .total_cmp(&right.score)
            .then_with(|| (right.end - right.start).cmp(&(left.end - left.start)))
            .then_with(|| left.start.cmp(&right.start))
            .then_with(|| left.end.cmp(&right.end))
    });
    (!candidates.is_empty()).then_some(candidates)
}

fn wooden_motion_height(motion: &HyMotionDecoded) -> f32 {
    let frame = motion.frames / 2;
    let row = &motion.keypoints_3d
        [frame * WOODEN_JOINTS * 3..(frame + 1) * WOODEN_JOINTS * 3];
    let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
    for point in row.chunks_exact(3) {
        min_y = min_y.min(point[1]);
        max_y = max_y.max(point[1]);
    }
    max_y - min_y
}

fn rotation_pose_distance(motion: &HyMotionDecoded, a: usize, b: usize) -> Option<f32> {
    let mut sum = 0.0;
    for joint in 0..HY_MOTION_BODY_JOINTS {
        let left = matrix_at(&motion.local_rotation_matrices, a, joint, HY_MOTION_BODY_JOINTS)?;
        let right = matrix_at(&motion.local_rotation_matrices, b, joint, HY_MOTION_BODY_JOINTS)?;
        let angle = rotation_distance(left, right);
        sum += angle * angle;
    }
    Some((sum / HY_MOTION_BODY_JOINTS as f32).sqrt())
}

fn rotation_velocity_distance(motion: &HyMotionDecoded, start: usize, end: usize) -> Option<f32> {
    if end == 0 || start + 1 >= motion.frames {
        return None;
    }
    let mut sum = 0.0;
    for joint in 0..HY_MOTION_BODY_JOINTS {
        let a0 = matrix_at(&motion.local_rotation_matrices, start, joint, HY_MOTION_BODY_JOINTS)?;
        let a1 = matrix_at(&motion.local_rotation_matrices, start + 1, joint, HY_MOTION_BODY_JOINTS)?;
        let b0 = matrix_at(&motion.local_rotation_matrices, end - 1, joint, HY_MOTION_BODY_JOINTS)?;
        let b1 = matrix_at(&motion.local_rotation_matrices, end, joint, HY_MOTION_BODY_JOINTS)?;
        let da = matrix_mul(matrix_transpose(a0), a1);
        let db = matrix_mul(matrix_transpose(b0), b1);
        let angle = rotation_distance(da, db);
        sum += angle * angle;
    }
    Some((sum / HY_MOTION_BODY_JOINTS as f32).sqrt())
}

fn centered_keypoint_distance(motion: &HyMotionDecoded, a: usize, b: usize) -> f32 {
    let mut sum = 0.0;
    for joint in 0..HY_MOTION_BODY_JOINTS {
        for axis in 0..3 {
            let av = centered_keypoint(motion, a, joint, axis);
            let bv = centered_keypoint(motion, b, joint, axis);
            sum += (av - bv) * (av - bv);
        }
    }
    (sum / (HY_MOTION_BODY_JOINTS * 3) as f32).sqrt()
}

fn centered_keypoint_velocity_distance(
    motion: &HyMotionDecoded,
    start: usize,
    end: usize,
) -> f32 {
    let mut sum = 0.0;
    for joint in 0..HY_MOTION_BODY_JOINTS {
        for axis in 0..3 {
            let av = centered_keypoint(motion, start + 1, joint, axis)
                - centered_keypoint(motion, start, joint, axis);
            let bv = centered_keypoint(motion, end, joint, axis)
                - centered_keypoint(motion, end - 1, joint, axis);
            sum += (av - bv) * (av - bv);
        }
    }
    (sum / (HY_MOTION_BODY_JOINTS * 3) as f32).sqrt()
}

fn centered_keypoint(motion: &HyMotionDecoded, frame: usize, joint: usize, axis: usize) -> f32 {
    let stride = WOODEN_JOINTS * 3;
    motion.keypoints_3d[frame * stride + joint * 3 + axis]
        - motion.keypoints_3d[frame * stride + axis]
}

fn loop_sample(
    frame: usize,
    output_frames: usize,
    window: LoopWindow,
    repeats: usize,
) -> (f32, f32) {
    let global_phase = repeats as f32 * frame as f32 / (output_frames - 1) as f32;
    let phase = if frame + 1 == output_frames {
        1.0
    } else {
        global_phase.fract()
    };
    let source = window.start as f32 + (window.end - window.start) as f32 * phase;
    (source, phase)
}

fn smoothstep(value: f32) -> f32 {
    value * value * (3.0 - 2.0 * value)
}

fn resample_projected_rows(
    values: &[f32],
    input_frames: usize,
    output_frames: usize,
    window: LoopWindow,
    repeats: usize,
) -> Option<Vec<f32>> {
    let stride = values.len().checked_div(input_frames)?;
    if stride * input_frames != values.len() {
        return None;
    }
    let start_row = &values[window.start * stride..(window.start + 1) * stride];
    let end_row = &values[window.end * stride..(window.end + 1) * stride];
    let mut out = Vec::with_capacity(output_frames * stride);
    for frame in 0..output_frames {
        let (at, phase) = loop_sample(frame, output_frames, window, repeats);
        let left = at.floor() as usize;
        let right = (left + 1).min(input_frames - 1);
        let t = at - left as f32;
        let closure = smoothstep(phase);
        for lane in 0..stride {
            let a = values[left * stride + lane];
            let b = values[right * stride + lane];
            out.push(a + (b - a) * t + (start_row[lane] - end_row[lane]) * closure);
        }
    }
    Some(out)
}

fn resample_projected_rotations(
    values: &[f32],
    input_frames: usize,
    output_frames: usize,
    joints: usize,
    window: LoopWindow,
    repeats: usize,
) -> Option<Vec<f32>> {
    if values.len() != input_frames * joints * 9 {
        return None;
    }
    let identity = [0.0, 0.0, 0.0, 1.0];
    let corrections: Vec<[f32; 4]> = (0..joints)
        .map(|joint| {
            let start = matrix_quaternion(matrix_at(values, window.start, joint, joints)?);
            let end = matrix_quaternion(matrix_at(values, window.end, joint, joints)?);
            Some(quaternion_mul(start, quaternion_inverse(end)))
        })
        .collect::<Option<_>>()?;
    let mut out = Vec::with_capacity(output_frames * joints * 9);
    for frame in 0..output_frames {
        let (at, phase) = loop_sample(frame, output_frames, window, repeats);
        let left = at.floor() as usize;
        let right = (left + 1).min(input_frames - 1);
        let t = at - left as f32;
        for joint in 0..joints {
            let a = matrix_at(values, left, joint, joints)?;
            let b = matrix_at(values, right, joint, joints)?;
            let source = quaternion_slerp(matrix_quaternion(a), matrix_quaternion(b), t);
            let correction = quaternion_slerp(identity, corrections[joint], smoothstep(phase));
            let q = quaternion_mul(correction, source);
            for row in quaternion_matrix(q) {
                out.extend_from_slice(&row);
            }
        }
    }
    Some(out)
}

fn sync_rotation_6d(motion: &mut HyMotionDecoded) {
    for frame in 0..motion.frames {
        for joint in 0..HY_MOTION_BODY_JOINTS {
            let matrix = matrix_at(
                &motion.local_rotation_matrices,
                frame,
                joint,
                HY_MOTION_BODY_JOINTS,
            ).unwrap();
            let offset = (frame * HY_MOTION_BODY_JOINTS + joint) * 6;
            motion.rotations_6d[offset..offset + 6].copy_from_slice(&[
                matrix[0][0], matrix[0][1], matrix[1][0],
                matrix[1][1], matrix[2][0], matrix[2][1],
            ]);
        }
    }
}

fn sync_latent_active_channels(motion: &mut HyMotionDecoded) {
    let stride = motion.latent_denorm.len() / motion.frames;
    if stride < 3 + HY_MOTION_BODY_JOINTS * 6 {
        return;
    }
    for frame in 0..motion.frames {
        let row = &mut motion.latent_denorm[frame * stride..(frame + 1) * stride];
        row[..3].copy_from_slice(&motion.translations[frame * 3..frame * 3 + 3]);
        row[3..3 + HY_MOTION_BODY_JOINTS * 6].copy_from_slice(
            &motion.rotations_6d[frame * HY_MOTION_BODY_JOINTS * 6
                ..(frame + 1) * HY_MOTION_BODY_JOINTS * 6],
        );
    }
}

fn cyclic_postcondition(motion: &HyMotionDecoded, repeats: usize) -> bool {
    if repeats == 0 || motion.frames < 3 || !decoded_rows_are_consistent(motion) {
        return false;
    }
    let transitions = motion.frames - 1;
    let is_boundary = |to_frame: usize| {
        ((to_frame - 1) * repeats) / transitions != (to_frame * repeats) / transitions
    };
    let height = wooden_motion_height(motion).max(1.0e-4);

    for joint in 0..HY_MOTION_BODY_JOINTS {
        let mut ordinary = Vec::new();
        let mut boundaries = Vec::new();
        for to_frame in 1..motion.frames {
            let left = match matrix_at(
                &motion.local_rotation_matrices,
                to_frame - 1,
                joint,
                HY_MOTION_BODY_JOINTS,
            ) { Some(value) => value, None => return false };
            let right = match matrix_at(
                &motion.local_rotation_matrices,
                to_frame,
                joint,
                HY_MOTION_BODY_JOINTS,
            ) { Some(value) => value, None => return false };
            let angle = rotation_distance(left, right);
            if is_boundary(to_frame) { boundaries.push(angle); } else { ordinary.push(angle); }
        }
        if ordinary.is_empty() || boundaries.is_empty() {
            return false;
        }
        let p95 = percentile(&ordinary, 0.95);
        let max = ordinary.iter().copied().fold(0.0f32, f32::max);
        if boundaries.into_iter().any(|angle| {
            !(angle <= 0.05 || (angle <= 2.0 * p95 && angle <= 1.5 * max))
                || !(angle * 30.0 <= 4.0
                    || (angle <= 1.5 * p95 && angle <= 1.25 * max))
        }) {
            return false;
        }

        let mut ordinary_acceleration = Vec::new();
        let mut boundary_acceleration = Vec::new();
        for frame in 1..motion.frames - 1 {
            let previous = match matrix_at(
                &motion.local_rotation_matrices,
                frame - 1,
                joint,
                HY_MOTION_BODY_JOINTS,
            ) { Some(value) => value, None => return false };
            let current = match matrix_at(
                &motion.local_rotation_matrices,
                frame,
                joint,
                HY_MOTION_BODY_JOINTS,
            ) { Some(value) => value, None => return false };
            let next = match matrix_at(
                &motion.local_rotation_matrices,
                frame + 1,
                joint,
                HY_MOTION_BODY_JOINTS,
            ) { Some(value) => value, None => return false };
            let left_velocity = matrix_mul(matrix_transpose(previous), current);
            let right_velocity = matrix_mul(matrix_transpose(current), next);
            // Comparing the relative rotation matrices retains the angular
            // direction; subtracting only their scalar speeds would miss an
            // equal-speed reversal at the seam.
            let acceleration = rotation_distance(left_velocity, right_velocity) * 30.0 * 30.0;
            if is_boundary(frame) || is_boundary(frame + 1) {
                boundary_acceleration.push(acceleration);
            } else {
                ordinary_acceleration.push(acceleration);
            }
        }
        if ordinary_acceleration.is_empty() || boundary_acceleration.is_empty() {
            return false;
        }
        let p95_acceleration = percentile(&ordinary_acceleration, 0.95);
        let max_acceleration = ordinary_acceleration.iter().copied().fold(0.0f32, f32::max);
        if boundary_acceleration.into_iter().any(|acceleration| {
            acceleration > 0.5
                && acceleration > 1.5 * p95_acceleration
                && acceleration > 1.25 * max_acceleration
        }) {
            return false;
        }
    }

    let mut ordinary = Vec::new();
    let mut boundaries = Vec::new();
    for to_frame in 1..motion.frames {
        let mut sum = 0.0;
        for joint in 0..HY_MOTION_BODY_JOINTS {
            for axis in 0..3 {
                let delta = centered_keypoint(motion, to_frame, joint, axis)
                    - centered_keypoint(motion, to_frame - 1, joint, axis);
                sum += delta * delta;
            }
        }
        let distance = (sum / (HY_MOTION_BODY_JOINTS * 3) as f32).sqrt() / height;
        if is_boundary(to_frame) { boundaries.push(distance); } else { ordinary.push(distance); }
    }
    let p95 = percentile(&ordinary, 0.95);
    let max = ordinary.iter().copied().fold(0.0f32, f32::max);
    if boundaries.into_iter().any(|distance| {
        !(distance <= 0.01 || (distance <= 1.5 * p95 && distance <= 1.25 * max))
    }) {
        return false;
    }


    // RMS protects the overall Wooden pose, but could dilute one distal
    // discontinuity by sqrt(joints * axes). Gate every Wooden joint as well,
    // including the finger joints whose transforms affect hand vertices.
    for joint in 0..WOODEN_JOINTS {
        let mut ordinary = Vec::new();
        let mut boundaries = Vec::new();
        for to_frame in 1..motion.frames {
            let mut squared = 0.0;
            for axis in 0..3 {
                let delta = centered_keypoint(motion, to_frame, joint, axis)
                    - centered_keypoint(motion, to_frame - 1, joint, axis);
                squared += delta * delta;
            }
            let distance = squared.sqrt() / height;
            if is_boundary(to_frame) {
                boundaries.push(distance);
            } else {
                ordinary.push(distance);
            }
        }
        let p95 = percentile(&ordinary, 0.95);
        let max = ordinary.iter().copied().fold(0.0f32, f32::max);
        if boundaries.into_iter().any(|distance| {
            !(distance <= 0.01 || (distance <= 1.5 * p95 && distance <= 1.25 * max))
        }) {
            return false;
        }
    }

    let mut ordinary_acceleration = Vec::new();
    let mut boundary_acceleration = Vec::new();
    for frame in 1..motion.frames - 1 {
        let mut sum = 0.0;
        for joint in 0..HY_MOTION_BODY_JOINTS {
            for axis in 0..3 {
                let acceleration = centered_keypoint(motion, frame + 1, joint, axis)
                    - 2.0 * centered_keypoint(motion, frame, joint, axis)
                    + centered_keypoint(motion, frame - 1, joint, axis);
                sum += acceleration * acceleration;
            }
        }
        let acceleration = (sum / (HY_MOTION_BODY_JOINTS * 3) as f32).sqrt()
            / height
            * 30.0
            * 30.0;
        if is_boundary(frame) || is_boundary(frame + 1) {
            boundary_acceleration.push(acceleration);
        } else {
            ordinary_acceleration.push(acceleration);
        }
    }
    if ordinary_acceleration.is_empty() || boundary_acceleration.is_empty() {
        return false;
    }
    let p95_acceleration = percentile(&ordinary_acceleration, 0.95);
    let max_acceleration = ordinary_acceleration.iter().copied().fold(0.0f32, f32::max);
    if boundary_acceleration.into_iter().any(|acceleration| {
        acceleration > 0.05
            && acceleration > 1.5 * p95_acceleration
            && acceleration > 1.25 * max_acceleration
    }) {
        return false;
    }
    for joint in 0..WOODEN_JOINTS {
        let mut ordinary = Vec::new();
        let mut boundaries = Vec::new();
        for frame in 1..motion.frames - 1 {
            let mut squared = 0.0;
            for axis in 0..3 {
                let acceleration = centered_keypoint(motion, frame + 1, joint, axis)
                    - 2.0 * centered_keypoint(motion, frame, joint, axis)
                    + centered_keypoint(motion, frame - 1, joint, axis);
                squared += acceleration * acceleration;
            }
            let acceleration = squared.sqrt() / height * 30.0 * 30.0;
            if is_boundary(frame) || is_boundary(frame + 1) {
                boundaries.push(acceleration);
            } else {
                ordinary.push(acceleration);
            }
        }
        let p95 = percentile(&ordinary, 0.95);
        let max = ordinary.iter().copied().fold(0.0f32, f32::max);
        if boundaries.into_iter().any(|acceleration| {
            acceleration > 0.05 && acceleration > 1.5 * p95 && acceleration > 1.25 * max
        }) {
            return false;
        }
    }
    true
}

fn percentile(values: &[f32], fraction: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    let index = ((sorted.len() - 1) as f32 * fraction).ceil() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn copy_first_row_to_last(values: &mut [f32], frames: usize) {
    let stride = values.len() / frames;
    let (head, tail) = values.split_at_mut((frames - 1) * stride);
    tail[..stride].copy_from_slice(&head[..stride]);
}

fn matrix_at(values: &[f32], frame: usize, joint: usize, joints: usize) -> Option<[[f32; 3]; 3]> {
    let offset = (frame.checked_mul(joints)?.checked_add(joint)?).checked_mul(9)?;
    let v = values.get(offset..offset + 9)?;
    Some([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]])
}

fn matrix_transpose(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    [[m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]]
}

fn matrix_mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            out[row][col] = (0..3).map(|k| a[row][k] * b[k][col]).sum();
        }
    }
    out
}

fn rotation_distance(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> f32 {
    let relative = matrix_mul(matrix_transpose(a), b);
    (((relative[0][0] + relative[1][1] + relative[2][2] - 1.0) * 0.5).clamp(-1.0, 1.0)).acos()
}

fn matrix_quaternion(m: [[f32; 3]; 3]) -> [f32; 4] {
    let trace = m[0][0] + m[1][1] + m[2][2];
    let (x, y, z, w) = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        ((m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s, 0.25 * s)
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        (0.25 * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s, (m[2][1] - m[1][2]) / s)
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        ((m[0][1] + m[1][0]) / s, 0.25 * s, (m[1][2] + m[2][1]) / s, (m[0][2] - m[2][0]) / s)
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        ((m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, 0.25 * s, (m[1][0] - m[0][1]) / s)
    };
    normalize_quaternion([x, y, z, w])
}

fn quaternion_matrix(q: [f32; 4]) -> [[f32; 3]; 3] {
    let [x, y, z, w] = normalize_quaternion(q);
    [[1.0 - 2.0 * (y*y + z*z), 2.0 * (x*y - z*w), 2.0 * (x*z + y*w)],
     [2.0 * (x*y + z*w), 1.0 - 2.0 * (x*x + z*z), 2.0 * (y*z - x*w)],
     [2.0 * (x*z - y*w), 2.0 * (y*z + x*w), 1.0 - 2.0 * (x*x + y*y)]]
}

fn quaternion_inverse(q: [f32; 4]) -> [f32; 4] {
    let q = normalize_quaternion(q);
    [-q[0], -q[1], -q[2], q[3]]
}

fn quaternion_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    normalize_quaternion([
        a[3]*b[0] + a[0]*b[3] + a[1]*b[2] - a[2]*b[1],
        a[3]*b[1] - a[0]*b[2] + a[1]*b[3] + a[2]*b[0],
        a[3]*b[2] + a[0]*b[1] - a[1]*b[0] + a[2]*b[3],
        a[3]*b[3] - a[0]*b[0] - a[1]*b[1] - a[2]*b[2],
    ])
}

fn normalize_quaternion(q: [f32; 4]) -> [f32; 4] {
    let inv = (q.iter().map(|v| v*v).sum::<f32>()).sqrt().max(1.0e-12).recip();
    [q[0]*inv, q[1]*inv, q[2]*inv, q[3]*inv]
}

fn quaternion_slerp(mut a: [f32; 4], mut b: [f32; 4], t: f32) -> [f32; 4] {
    let mut dot = a.iter().zip(b).map(|(x,y)| x*y).sum::<f32>();
    if dot < 0.0 { b = [-b[0], -b[1], -b[2], -b[3]]; dot = -dot; }
    if dot > 0.9995 {
        return normalize_quaternion([a[0]+(b[0]-a[0])*t, a[1]+(b[1]-a[1])*t,
            a[2]+(b[2]-a[2])*t, a[3]+(b[3]-a[3])*t]);
    }
    let angle = dot.clamp(-1.0, 1.0).acos();
    let denom = angle.sin();
    let left = ((1.0-t)*angle).sin()/denom;
    let right = (t*angle).sin()/denom;
    a = [a[0]*left+b[0]*right, a[1]*left+b[1]*right,
        a[2]*left+b[2]*right, a[3]*left+b[3]*right];
    normalize_quaternion(a)
}

pub struct MotionNativeBackend {
    model_id: String,
    worker: Option<MotionWorker>,
    ready: Option<ResolvedPaths>,
}

impl MotionNativeBackend {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            worker: None,
            ready: None,
        }
    }
}

impl ContentBackend for MotionNativeBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        ctx.cancel.check()?;
        let resolved = ResolvedPaths::from_ctx(ctx)?;
        if self.worker.as_ref().is_some_and(|worker| worker.is_alive())
            && self.ready.as_ref() == Some(&resolved)
        {
            return Ok(());
        }
        // A dead worker or a changed artifact identity must be retired before
        // a replacement can allocate another ~20 GiB resident set. This is a
        // bounded, acknowledged teardown; merely dropping the sender would
        // allow the old CUDA allocations to overlap the new load.
        if self.worker.is_some() {
            self.unload()?;
        }
        (ctx.progress)("motion load: worker", 0.01);
        let worker = MotionWorker::spawn(&resolved.paths, ctx.cancel.clone(), ctx.progress)?;
        self.worker = Some(worker);
        self.ready = Some(resolved);
        (ctx.progress)("motion load: resident", 1.0);
        Ok(())
    }

    fn is_resident(&self) -> bool {
        self.worker.as_ref().is_some_and(|worker| worker.is_alive())
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        if let Some(worker) = self.worker.as_mut() {
            worker.shutdown()?;
        }
        self.worker = None;
        self.ready = None;
        Ok(())
    }

    fn resident_is_healthy_after_error(&self, error: &AssetAiError) -> bool {
        // Parameter validation happens before a worker job is submitted, and
        // cooperative cancellation leaves only immutable weights plus a
        // complete conditioning cache. Model/kernel/retarget failures remain
        // conservative and retire the worker through the lifecycle manager.
        matches!(error, AssetAiError::Cancelled | AssetAiError::Params(_))
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        if params.input_bytes.is_empty() {
            return Err(AssetAiError::Params(format!(
                "{} needs an input rigged mesh (input_b64 GLB)",
                self.model_id
            )));
        }
        validate_rigged_glb(&params.input_bytes)?;
        cancel.check()?;
        let worker = self.worker.as_ref().ok_or_else(|| {
            AssetAiError::Backend("native HY-Motion used before ensure_loaded".to_string())
        })?;
        let job = GenerateJob {
            rigged_glb: params.input_bytes.clone(),
            seed: params.seed,
            steps: params.steps.unwrap_or(50) as usize,
            // GenerateParams' cross-domain default is 3.5, while the
            // released HY-Motion contract is CFG 5. Keep the official value
            // rather than silently changing model behavior on ordinary
            // motion requests.
            guidance: HY_MOTION_CFG,
        };
        let output = match worker.generate(job, cancel.clone(), progress) {
            Ok(bytes) => bytes,
            Err(WorkerError::Cancelled) => return Err(AssetAiError::Cancelled),
            Err(WorkerError::Other(message)) => {
                return Err(AssetAiError::Backend(format!("native HY-Motion: {message}")))
            }
            Err(WorkerError::WorkerGone(message)) => {
                self.worker = None;
                return Err(AssetAiError::Backend(format!("native HY-Motion: {message}")));
            }
        };
        cancel.check()?;
        check_motion_output(&output)?;
        validate_animated_glb_quality(&output)?;
        Ok(vec![ArtifactData {
            content_type: "model/gltf-binary",
            ext: "glb",
            bytes: output,
        }])
    }
}

struct MotionWorker {
    tx: Option<mpsc::Sender<WorkerMsg>>,
    join: Option<JoinHandle<()>>,
}

struct GenerateJob {
    rigged_glb: Vec<u8>,
    seed: u64,
    steps: usize,
    guidance: f32,
}

enum WorkerCommand {
    Generate(GenerateJob),
    Ping,
    Shutdown,
}

struct WorkerMsg {
    command: WorkerCommand,
    cancel: CancelToken,
    events: mpsc::Sender<WorkerEvent>,
}

enum WorkerEvent {
    Progress(String, f64),
    Ready(Result<(), WorkerError>),
    Done(Result<Vec<u8>, WorkerError>),
}

#[derive(Debug)]
enum WorkerError {
    Cancelled,
    Other(String),
    WorkerGone(String),
}

impl MotionWorker {
    fn spawn(
        paths: &HyMotionModelPaths,
        cancel: CancelToken,
        progress: ProgressSink,
    ) -> Result<Self, AssetAiError> {
        let (tx, rx) = mpsc::channel::<WorkerMsg>();
        let (ready_tx, ready_rx) = mpsc::channel::<WorkerEvent>();
        let paths = paths.clone();
        let join = std::thread::Builder::new()
            .name("hy-motion-native".to_string())
            .spawn(move || {
                let mut phase = |name: &str, done: usize, total: usize| {
                    let local = if total == 0 { 0.0 } else { done as f64 / total as f64 };
                    let (offset, span) = match name {
                        "load-qwen" => (0.02, 0.16),
                        "load-clip" => (0.18, 0.08),
                        "load-dit" => (0.26, 0.68),
                        "load-wooden" => (0.94, 0.05),
                        _ => (0.01, 0.98),
                    };
                    let _ = ready_tx.send(WorkerEvent::Progress(
                        format!("motion load: {name}"),
                        offset + span * local,
                    ));
                };
                let cancelled = || cancel.is_cancelled();
                let mut control = HyMotionRunControl {
                    on_phase: Some(&mut phase),
                    cancel: Some(&cancelled),
                };
                let pipeline = HyMotionPipeline::load_with_control(&paths, &mut control)
                    .map_err(diffusion_error);
                let mut pipeline = match pipeline {
                    Ok((pipeline, _)) => {
                        let _ = ready_tx.send(WorkerEvent::Ready(Ok(())));
                        pipeline
                    }
                    Err(error) => {
                        let _ = ready_tx.send(WorkerEvent::Ready(Err(error)));
                        return;
                    }
                };
                drop(ready_tx);
                let mut shutdown_reply = None;
                while let Ok(message) = rx.recv() {
                    match message.command {
                        WorkerCommand::Ping => {
                            let _ = message.events.send(WorkerEvent::Done(Ok(Vec::new())));
                        }
                        WorkerCommand::Generate(job) => {
                            let result = run_generate(
                                &mut pipeline,
                                job,
                                &message.cancel,
                                &message.events,
                            );
                            let _ = message.events.send(WorkerEvent::Done(result));
                        }
                        WorkerCommand::Shutdown => {
                            shutdown_reply = Some(message.events);
                            break;
                        }
                    }
                }
                // Qwen and CLIP use the thread-local named CUDA weight cache;
                // evict those explicitly before dropping the directly-owned
                // DiT tensors. Finally clear idle activation pools on this
                // same worker thread. An unload acknowledgement is emitted
                // only after all three release boundaries complete.
                let conditioner_evict = pipeline
                    .evict_conditioner_device_weights()
                    .map_err(diffusion_error);
                drop(pipeline);
                makepad_diffusion::backend::gpu_pool_clear();
                if let Some(events) = shutdown_reply {
                    let result = conditioner_evict.map(|_| Vec::new());
                    let _ = events.send(WorkerEvent::Done(result));
                }
            })
            .map_err(|error| AssetAiError::Backend(format!("spawn HY-Motion worker: {error}")))?;
        loop {
            match ready_rx.recv() {
                Ok(WorkerEvent::Progress(stage, fraction)) => progress(&stage, fraction),
                Ok(WorkerEvent::Ready(Ok(()))) => {
                    return Ok(Self {
                        tx: Some(tx),
                        join: Some(join),
                    })
                }
                Ok(WorkerEvent::Ready(Err(WorkerError::Cancelled))) => {
                    return Err(AssetAiError::Cancelled)
                }
                Ok(WorkerEvent::Ready(Err(error))) => {
                    return Err(AssetAiError::Backend(format!(
                        "load native HY-Motion: {error:?}"
                    )))
                }
                Ok(WorkerEvent::Done(_)) => continue,
                Err(_) => {
                    return Err(AssetAiError::Backend(
                        "HY-Motion worker exited during load".to_string(),
                    ))
                }
            }
        }
    }

    fn is_alive(&self) -> bool {
        let Some(tx) = &self.tx else {
            return false;
        };
        let (events, replies) = mpsc::channel();
        tx
            .send(WorkerMsg {
                command: WorkerCommand::Ping,
                cancel: CancelToken::new(),
                events,
            })
            .is_ok()
            && replies.recv_timeout(Duration::from_secs(2)).is_ok()
    }

    fn generate(
        &self,
        job: GenerateJob,
        cancel: CancelToken,
        progress: ProgressSink,
    ) -> Result<Vec<u8>, WorkerError> {
        let (events, replies) = mpsc::channel();
        self.tx
            .as_ref()
            .ok_or_else(|| WorkerError::WorkerGone("worker is shut down".to_string()))?
            .send(WorkerMsg {
                command: WorkerCommand::Generate(job),
                cancel,
                events,
            })
            .map_err(|_| WorkerError::WorkerGone("worker channel is gone".to_string()))?;
        loop {
            match replies.recv() {
                Ok(WorkerEvent::Progress(stage, fraction)) => progress(&stage, fraction),
                Ok(WorkerEvent::Done(result)) => return result,
                Ok(WorkerEvent::Ready(_)) => continue,
                Err(_) => {
                    return Err(WorkerError::WorkerGone(
                        "worker dropped its generation reply".to_string(),
                    ))
                }
            }
        }
    }

    fn shutdown(&mut self) -> Result<(), AssetAiError> {
        let Some(tx) = self.tx.take() else {
            return Ok(());
        };
        let (events, replies) = mpsc::channel();
        let sent = tx
            .send(WorkerMsg {
            command: WorkerCommand::Shutdown,
            cancel: CancelToken::new(),
            events,
        })
            .is_ok();
        drop(tx);
        if sent {
            match replies.recv_timeout(Duration::from_secs(120)) {
                Ok(WorkerEvent::Done(Ok(_))) => {}
                Ok(WorkerEvent::Done(Err(error))) => {
                    return Err(AssetAiError::Backend(format!(
                        "HY-Motion unload failed: {error:?}"
                    )))
                }
                Ok(_) => {
                    return Err(AssetAiError::Backend(
                        "HY-Motion unload received an invalid acknowledgement".to_string(),
                    ))
                }
                Err(error) => {
                    return Err(AssetAiError::Backend(format!(
                        "HY-Motion unload acknowledgement timed out: {error}"
                    )))
                }
            }
        }
        if let Some(join) = self.join.take() {
            join.join().map_err(|_| {
                AssetAiError::Backend("HY-Motion worker panicked during unload".to_string())
            })?;
        }
        Ok(())
    }
}

fn run_generate(
    pipeline: &mut HyMotionPipeline,
    job: GenerateJob,
    cancel: &CancelToken,
    events: &mpsc::Sender<WorkerEvent>,
) -> Result<Vec<u8>, WorkerError> {
    if job.steps == 0 {
        return Err(WorkerError::Other("step count must be non-zero".to_string()));
    }
    if !job.guidance.is_finite() {
        return Err(WorkerError::Other("guidance must be finite".to_string()));
    }
    let mut clips = Vec::with_capacity(HY_MOTION_CLIP_RECIPES.len());
    for (index, recipe) in HY_MOTION_CLIP_RECIPES.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(WorkerError::Cancelled);
        }
        let clip_count = HY_MOTION_CLIP_RECIPES.len();
        let mut on_phase = |name: &str, done: usize, total: usize| {
            let local = if total == 0 { 0.0 } else { done as f64 / total as f64 };
            let _ = events.send(WorkerEvent::Progress(
                format!("motion {}/{} {}: {name}", index + 1, clip_count, recipe.name),
                clip_generation_progress(index, clip_count, local),
            ));
        };
        let cancelled = || cancel.is_cancelled();
        let mut control = HyMotionRunControl {
            on_phase: Some(&mut on_phase),
            cancel: Some(&cancelled),
        };
        let run = pipeline
            .generate_with_control(
                recipe.prompt,
                &HyMotionGenerateParams {
                    frames: recipe.frames,
                    steps: job.steps,
                    guidance: job.guidance,
                    seed: clip_seed(job.seed, index),
                    initial_latent: None,
                    smooth: true,
                },
                &mut control,
            )
            .map_err(diffusion_error)?;
        clips.push(match recipe.name {
            "jump" => trim_jump_action(&run.decoded),
            // A dance take has no gait cycle to close: forcing it through
            // cyclic closure would either reject the take or author a fake
            // seam. Ship the finite performance as generated; the VJ
            // playback layer loops it.
            "dance" => run.decoded,
            _ => close_cyclic_motion(&run.decoded, recipe.frames).ok_or_else(|| {
                WorkerError::Other(format!(
                    "{} generation has no bounded cyclic motion window",
                    recipe.name
                ))
            })?,
        });
    }
    if cancel.is_cancelled() {
        return Err(WorkerError::Cancelled);
    }
    let _ = events.send(WorkerEvent::Progress("motion: native retarget".to_string(), 0.90));
    let refs: Vec<HyMotionClipRef<'_>> = HY_MOTION_CLIP_RECIPES
        .iter()
        .zip(&clips)
        .map(|(recipe, motion)| HyMotionClipRef {
            name: recipe.name,
            motion,
        })
        .collect();
    let output = retarget_hy_motion_glb_with_report(
        &job.rigged_glb,
        &refs,
        &RetargetOptions {
            fps: 30.0,
            in_place: true,
            // The AI Content play controller supplies its own ballistic Y;
            // baking HY-Motion's root Y here would make every jump happen
            // twice (and at two independently retimed rates).
            strip_vertical_root: true,
        },
    )
    .map_err(|error| WorkerError::Other(format!("retarget: {error}")))?;
    if output.report.clips != HY_MOTION_CLIP_RECIPES.len() {
        return Err(WorkerError::Other(format!(
            "retarget emitted {} clips; native contract requires {}",
            output.report.clips,
            HY_MOTION_CLIP_RECIPES.len()
        )));
    }
    let _ = events.send(WorkerEvent::Progress(
        format!(
            "motion: retargeted {} clips / {} mapped joints",
            output.report.clips, output.report.mapped_joints
        ),
        0.98,
    ));
    Ok(output.glb)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cyclic_fixture(frames: usize, period: f32, corrupt_tail: bool) -> HyMotionDecoded {
        let mut latent_denorm = vec![0.0; frames * 201];
        let mut rotations_6d = vec![0.0; frames * HY_MOTION_BODY_JOINTS * 6];
        let mut translations = vec![0.0; frames * 3];
        let mut local_rotation_matrices = vec![0.0; frames * HY_MOTION_BODY_JOINTS * 9];
        let mut root_rotation_matrices = vec![0.0; frames * 9];
        let mut keypoints_3d = vec![0.0; frames * WOODEN_JOINTS * 3];
        for frame in 0..frames {
            let phase = frame as f32 * std::f32::consts::TAU / period;
            let tail = if corrupt_tail && frame > frames * 4 / 5 {
                (frame - frames * 4 / 5) as f32 * 0.12
            } else { 0.0 };
            translations[frame * 3] = frame as f32 * 0.03;
            for joint in 0..HY_MOTION_BODY_JOINTS {
                let angle = phase.sin() * (0.08 + joint as f32 * 0.002) + tail;
                let c = angle.cos();
                let s = angle.sin();
                let matrix = [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]];
                let offset = (frame * HY_MOTION_BODY_JOINTS + joint) * 9;
                for (row_index, row) in matrix.into_iter().enumerate() {
                    local_rotation_matrices[offset + row_index * 3..offset + row_index * 3 + 3]
                        .copy_from_slice(&row);
                }
                let six = [c, -s, s, c, 0.0, 0.0];
                rotations_6d[(frame * HY_MOTION_BODY_JOINTS + joint) * 6
                    ..(frame * HY_MOTION_BODY_JOINTS + joint + 1) * 6].copy_from_slice(&six);
            }
            root_rotation_matrices[frame*9..frame*9+9]
                .copy_from_slice(&local_rotation_matrices[frame*HY_MOTION_BODY_JOINTS*9
                    ..frame*HY_MOTION_BODY_JOINTS*9+9]);
            for joint in 0..WOODEN_JOINTS {
                let offset = (frame * WOODEN_JOINTS + joint) * 3;
                keypoints_3d[offset] = joint as f32 * 0.01 + phase.sin() * 0.02;
                keypoints_3d[offset + 1] = joint as f32 / (WOODEN_JOINTS - 1) as f32 * 1.7;
                keypoints_3d[offset + 2] = phase.cos() * (joint as f32 * 0.0005);
            }
            latent_denorm[frame*201] = phase;
        }
        HyMotionDecoded { frames, latent_denorm, rotations_6d, translations,
            local_rotation_matrices, root_rotation_matrices, keypoints_3d }
    }

    #[test]
    fn clip_names_and_frame_contract_are_canonical() {
        assert_eq!(
            HY_MOTION_CLIP_RECIPES.map(|clip| clip.name),
            ["idle", "walk", "jump", "run", "dance"]
        );
        assert_eq!(
            HY_MOTION_CLIP_RECIPES.map(|clip| clip.frames),
            [120, 120, 90, 120, 150]
        );
        assert!(HY_MOTION_CLIP_RECIPES
            .iter()
            .all(|clip| clip.prompt.starts_with("A person")));
    }

    #[test]
    fn dance_recipe_is_appended_finite_and_short() {
        use makepad_diffusion::hy_motion::{
            HY_MOTION_FPS, HY_MOTION_MAX_FRAMES, HY_MOTION_MIN_FRAMES,
        };
        // Append-only: the locomotion clips keep their indices, so their
        // SplitMix64 seed streams are bit-identical to the pre-dance contract.
        let dance = HY_MOTION_CLIP_RECIPES.last().unwrap();
        assert_eq!(dance.name, "dance");
        assert!(HY_MOTION_CLIP_RECIPES[..4]
            .iter()
            .map(|clip| clip.name)
            .eq(["idle", "walk", "jump", "run"]));
        // A finite short performance: valid for the native pipeline and
        // within the model's short-single-action band (<= 6 seconds). The
        // clip is not a seamless native loop — the VJ layer loops it.
        assert!((HY_MOTION_MIN_FRAMES..=HY_MOTION_MAX_FRAMES).contains(&dance.frames));
        assert!(dance.frames <= 6 * HY_MOTION_FPS);
        // One person, body action only; the prompt never promises a loop.
        assert!(!dance.prompt.contains("loop"));
    }

    #[test]
    fn clip_seed_is_stable_and_separates_streams() {
        let left = [
            clip_seed(42, 0),
            clip_seed(42, 1),
            clip_seed(42, 2),
            clip_seed(42, 3),
            clip_seed(42, 4),
        ];
        let right = [
            clip_seed(42, 0),
            clip_seed(42, 1),
            clip_seed(42, 2),
            clip_seed(42, 3),
            clip_seed(42, 4),
        ];
        assert_eq!(left, right);
        assert_ne!(left[0], left[1]);
        assert_ne!(left[1], left[2]);
        assert_ne!(left[2], left[3]);
        assert_ne!(left[3], left[4]);
        assert_ne!(left[0], clip_seed(43, 0));
    }

    #[test]
    fn clip_progress_partition_is_monotonic_and_bounded() {
        let count = HY_MOTION_CLIP_RECIPES.len();
        assert_eq!(clip_generation_progress(0, count, 0.0), 0.02);
        let mut previous = 0.0;
        for index in 0..count {
            let start = clip_generation_progress(index, count, 0.0);
            let end = clip_generation_progress(index, count, 1.0);
            assert!(start >= previous);
            assert!(end >= start);
            previous = end;
        }
        assert!((previous - 0.89).abs() < f64::EPSILON);
        assert!(clip_generation_progress(count - 1, count, 2.0) <= 0.89);
    }

    #[test]
    fn artifact_roles_are_unique_and_complete() {
        let mut roles = Vec::new();
        roles.extend(ROLE_QWEN_SHARDS);
        roles.push(ROLE_QWEN_TOKENIZER);
        roles.push(ROLE_CLIP_MODEL);
        roles.push(ROLE_MOTION_CHECKPOINT);
        roles.extend(ROLE_WOODEN_FILES);
        let mut unique = std::collections::HashSet::new();
        assert_eq!(roles.len(), 14);
        assert!(roles.into_iter().all(|role| unique.insert(role)));
    }

    #[test]
    fn jump_trim_uses_crouch_through_landing_and_keeps_rows_aligned() {
        let frames = 60;
        let mut translations = Vec::with_capacity(frames * 3);
        // Apex at 30, pre-apex crouch at 20, post-apex landing at 40.
        let ys: Vec<f32> = (0..frames)
            .map(|frame| {
                if frame <= 20 {
                    -(frame as f32) * 0.005
                } else if frame <= 30 {
                    -0.1 + (frame - 20) as f32 * 0.03
                } else if frame <= 40 {
                    0.2 - (frame - 30) as f32 * 0.025
                } else {
                    -0.05 + (frame - 40) as f32 * 0.0025
                }
            })
            .collect();
        for (frame, y) in ys.into_iter().enumerate() {
            translations.extend_from_slice(&[frame as f32, y, 0.0]);
        }
        let tagged = |stride: usize| {
            (0..frames)
                .flat_map(|frame| std::iter::repeat(frame as f32).take(stride))
                .collect::<Vec<_>>()
        };
        let motion = HyMotionDecoded {
            frames,
            latent_denorm: tagged(201),
            rotations_6d: tagged(22 * 6),
            translations,
            local_rotation_matrices: tagged(22 * 9),
            root_rotation_matrices: tagged(9),
            keypoints_3d: tagged(52 * 3),
        };
        let trimmed = trim_jump_action(&motion);
        assert_eq!(trimmed.frames, 21);
        assert_eq!(trimmed.translations[0], 20.0);
        assert_eq!(trimmed.translations[(trimmed.frames - 1) * 3], 40.0);
        assert_eq!(trimmed.latent_denorm.len(), trimmed.frames * 201);
        assert_eq!(trimmed.keypoints_3d.len(), trimmed.frames * 52 * 3);
    }

    #[test]
    fn cyclic_window_discards_nonperiodic_tail_and_authors_exact_seam() {
        let source = cyclic_fixture(120, 32.0, true);
        let candidates = find_loop_windows(&source).expect("periodic core must be found");
        let (window, closed) = candidates
            .into_iter()
            .find_map(|window| {
                close_cyclic_motion_window(&source, 120, window)
                    .map(|closed| (window, closed))
            })
            .expect("a bounded projected cycle must be found");
        assert!(window.end < 119, "corrupt recovery tail must not become the seam");
        assert!(window.end - window.start >= 60);
        assert_eq!(closed.frames, 120);
        for values in [&closed.latent_denorm, &closed.rotations_6d, &closed.translations,
            &closed.local_rotation_matrices, &closed.root_rotation_matrices, &closed.keypoints_3d] {
            let stride = values.len() / closed.frames;
            assert_eq!(&values[..stride], &values[(closed.frames-1)*stride..]);
            assert!(values.iter().all(|value| value.is_finite()));
        }
        let repeats = (((closed.frames - 1) as f32 / (window.end - window.start) as f32)
            .round() as usize).max(1);
        assert!(cyclic_postcondition(&closed, repeats));
        let boundary = if repeats > 1 {
            (closed.frames - 1) / repeats
        } else {
            closed.frames - 2
        };
        assert!(boundary > 1 && boundary + 1 < closed.frames);
        let before = rotation_pose_distance(&closed, boundary - 1, boundary).unwrap();
        let across = rotation_pose_distance(&closed, boundary, boundary + 1).unwrap();
        assert!(before < 0.15, "closure pop moved one frame earlier: {before}");
        assert!(across < 0.15, "closure pop moved one frame later: {across}");
        let velocity_jump = rotation_velocity_distance(&closed, boundary - 1, boundary + 1).unwrap();
        assert!(velocity_jump < 0.15, "closure velocity is discontinuous: {velocity_jump}");
    }

    #[test]
    fn cyclic_window_is_topology_independent_and_deterministic() {
        let source = cyclic_fixture(120, 30.0, false);
        let first = close_cyclic_motion(&source, 120).unwrap();
        let second = close_cyclic_motion(&source, 120).unwrap();
        assert_eq!(first.local_rotation_matrices, second.local_rotation_matrices);
        assert_eq!(first.keypoints_3d, second.keypoints_3d);
    }

    #[test]
    fn cyclic_window_rejects_nonfinite_values_in_every_decoded_field() {
        let assert_rejected = |mut motion: HyMotionDecoded, field: usize| {
            match field {
                0 => motion.latent_denorm[7] = f32::NAN,
                1 => motion.rotations_6d[7] = f32::INFINITY,
                2 => motion.translations[7] = f32::NEG_INFINITY,
                3 => motion.local_rotation_matrices[7] = f32::NAN,
                4 => motion.root_rotation_matrices[7] = f32::INFINITY,
                5 => motion.keypoints_3d[7] = f32::NEG_INFINITY,
                _ => unreachable!(),
            }
            assert!(close_cyclic_motion(&motion, 120).is_none());
        };
        for field in 0..6 {
            assert_rejected(cyclic_fixture(120, 30.0, false), field);
        }
    }

    #[test]
    fn cyclic_postcondition_cannot_dilute_one_distal_keypoint_pop() {
        let mut closed = close_cyclic_motion(&cyclic_fixture(120, 30.0, false), 120).unwrap();
        let height = wooden_motion_height(&closed);
        let offset = ((closed.frames - 1) * WOODEN_JOINTS + (WOODEN_JOINTS - 1)) * 3;
        closed.keypoints_3d[offset] += height * 0.03;
        assert!(
            !cyclic_postcondition(&closed, 1),
            "a single distal joint discontinuity must not be hidden by pose RMS"
        );
    }

    #[test]
    fn cyclic_window_fails_closed_for_aperiodic_motion() {
        let mut source = cyclic_fixture(120, 10_000.0, false);
        for frame in 0..source.frames {
            for joint in 0..HY_MOTION_BODY_JOINTS {
                let angle = frame as f32 * (0.025 + joint as f32 * 0.001);
                let matrix = quaternion_matrix([0.0, 0.0, (angle*0.5).sin(), (angle*0.5).cos()]);
                let offset = (frame * HY_MOTION_BODY_JOINTS + joint) * 9;
                for (row_index, row) in matrix.into_iter().enumerate() {
                    source.local_rotation_matrices[offset + row_index * 3..offset + row_index * 3 + 3]
                        .copy_from_slice(&row);
                }
            }
        }
        assert!(close_cyclic_motion(&source, 120).is_none());
    }

    #[test]
    fn generated_elf_regression_is_rejected_by_both_quality_boundaries_if_present() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local/character_verify/elf_clean_lib49_animated_v1.glb"
        );
        let Ok(bytes) = std::fs::read(path) else {
            println!("SKIP: local lib49 animated regression fixture is absent");
            return;
        };
        let rig_error = validate_rigged_glb(&bytes).expect_err("fused rig must fail pre-HY");
        let motion_error =
            validate_animated_glb_quality(&bytes).expect_err("visible webbing must fail post-HY");
        println!("{rig_error}");
        println!("{motion_error}");
        assert!(
            matches!(&rig_error, AssetAiError::Params(message) if message.starts_with("character-rig-quality:")),
            "{rig_error}"
        );
        assert!(
            matches!(&motion_error, AssetAiError::Params(message) if message.starts_with("character-motion-quality:")),
            "{motion_error}"
        );
        let backend = MotionNativeBackend::new(HY_MOTION_NATIVE_MODEL);
        assert!(backend.resident_is_healthy_after_error(&rig_error));
        assert!(backend.resident_is_healthy_after_error(&motion_error));
    }
}
