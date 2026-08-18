//! Native HY-Motion -> arbitrary humanoid-rig retarget and animated GLB export.
//!
//! This is the Blender-free equivalent of `python/retarget_multi.py`. It
//! consumes the native runtime's [`HyMotionDecoded`] directly: global root
//! rotations, translations, and 52 WoodenMesh keypoints are already all the
//! direction-based retarget needs. In particular, it never reconstructs an
//! NPZ or applies the root twice.
//!
//! The auto-rig is classified from hierarchy and geometry rather than bone
//! names. Each driven bone aligns the actual parent->child hierarchy edge,
//! not the bone's decorative local +Y/tail axis. The latter is wrong for
//! unconnected SkinTokens bones and was the cause of crossed limbs in the
//! first playable-character result.
//!
//! # Integration boundary
//!
//! A resident HY-Motion service supplies one [`HyMotionDecoded`] per named
//! clip and calls [`retarget_hy_motion_glb`] with the rigged GLB bytes. The
//! usual playable-character contract is four [`HyMotionClipRef`] entries
//! named `idle`, `walk`, `run`, and `jump`; no NPZ, Python, NumPy, or Blender process
//! is involved.
//!
//! The fixed 34-joint/120-frame Mario oracle runs parse + retarget + lossless
//! GLB augmentation in about 1.7 ms median in a local release build. Its
//! Blender reference takes 2.16 s including startup (252 ms export-only).
//! Channel parity is below 9e-7 radians rotation and 1e-7 translation.

use makepad_diffusion::hy_motion_decode::{
    HyMotionDecoded, HY_MOTION_WOODEN_JOINTS,
};
use makepad_gltf::{
    parse_glb_bytes, replace_glb_node_animations, GlbAnimPath,
    GlbNodeAnimChannel, GlbNodeAnimClip, GltfError, GltfNode,
};
use makepad_micro_serde::JsonValue;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

const EPS: f64 = 1.0e-12;
const MIN_BONE_LENGTH: f64 = 0.004;
const NOMINAL_HUMAN_HEIGHT: f64 = 1.75;

// Official WoodenMesh/SMPL-H joint indices. Only the retargeted subset is
// named here; decoded.keypoints_3d still retains all 52 joints.
const L_HIP: usize = 1;
const R_HIP: usize = 2;
const SPINE1: usize = 3;
const L_KNEE: usize = 4;
const R_KNEE: usize = 5;
const SPINE2: usize = 6;
const L_ANKLE: usize = 7;
const R_ANKLE: usize = 8;
const SPINE3: usize = 9;
const L_FOOT: usize = 10;
const R_FOOT: usize = 11;
const NECK: usize = 12;
const L_COLLAR: usize = 13;
const R_COLLAR: usize = 14;
const HEAD: usize = 15;
const L_SHOULDER: usize = 16;
const R_SHOULDER: usize = 17;
const L_ELBOW: usize = 18;
const R_ELBOW: usize = 19;
const L_WRIST: usize = 20;
const R_WRIST: usize = 21;
const L_MIDDLE1: usize = 25;
const R_MIDDLE1: usize = 40;

// WoodenMesh's neutral leg offsets. Direction-only retargeting must compare
// the generated foot frame against this source rest frame before applying it
// to an arbitrary target rig. Aligning a SkinTokens ankle->foot edge directly
// to the absolute WoodenMesh edge bakes the two skeletons' different bind-pose
// pitches into every frame (about 30 degrees on the current Mario rig).
const L_ANKLE_TO_KNEE_REST: V3 = V3 {
    x: -0.00123765,
    y: 0.39813763,
    z: 0.05212229,
};
const R_ANKLE_TO_KNEE_REST: V3 = V3 {
    x: -0.00085527,
    y: 0.39813870,
    z: 0.05212229,
};
const L_ANKLE_TO_FOOT_REST: V3 = V3 {
    x: 0.01255833,
    y: -0.07163954,
    z: 0.14270640,
};
const R_ANKLE_TO_FOOT_REST: V3 = V3 {
    x: -0.01218209,
    y: -0.07170463,
    z: 0.14270626,
};

#[derive(Debug)]
pub enum RetargetError {
    Gltf(GltfError),
    Rig(String),
    Motion(String),
}

impl fmt::Display for RetargetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gltf(error) => write!(formatter, "glTF: {error}"),
            Self::Rig(error) => write!(formatter, "rig: {error}"),
            Self::Motion(error) => write!(formatter, "motion: {error}"),
        }
    }
}

impl Error for RetargetError {}

impl From<GltfError> for RetargetError {
    fn from(value: GltfError) -> Self {
        Self::Gltf(value)
    }
}

/// One named native HY-Motion result. The normal character contract passes
/// four entries named `idle`, `walk`, `run`, and `jump`.
#[derive(Clone, Copy)]
pub struct HyMotionClipRef<'a> {
    pub name: &'a str,
    pub motion: &'a HyMotionDecoded,
}

#[derive(Clone, Copy, Debug)]
pub struct RetargetOptions {
    pub fps: f32,
    /// Remove X/Z root travel but keep Y (crouch/jump) so the game host owns
    /// horizontal locomotion.
    pub in_place: bool,
    /// Remove vertical root travel as well. Playable output uses a host-side
    /// ballistic controller for height, so retaining authored Y there would
    /// apply the jump twice. Kept opt-in to preserve oracle/export behavior.
    pub strip_vertical_root: bool,
}

impl Default for RetargetOptions {
    fn default() -> Self {
        Self {
            fps: 30.0,
            in_place: true,
            strip_vertical_root: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RetargetReport {
    pub joints: usize,
    pub mapped_joints: usize,
    pub clips: usize,
    pub frames: usize,
    pub mirrored: bool,
    pub rig_height: f32,
    pub motion_scale: f32,
}

pub struct RetargetOutput {
    pub glb: Vec<u8>,
    pub report: RetargetReport,
}

/// Humanoid limb regions classified only from skin hierarchy and bind-pose
/// geometry. Node indices remain in glTF document space so downstream skin
/// audits can map them through any `skin.joints` palette order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HumanoidBranches {
    pub arm_nodes: Vec<usize>,
    pub leg_nodes: Vec<usize>,
}

pub(crate) fn classify_humanoid_branches(
    rigged_glb: &[u8],
) -> Result<HumanoidBranches, RetargetError> {
    let parsed = parse_glb_bytes(rigged_glb)?;
    let rig = Rig::classify(&parsed.document.nodes_slice(), parsed.document.skins.as_deref())?;
    Ok(HumanoidBranches {
        arm_nodes: rig.arm_nodes,
        leg_nodes: rig.leg_nodes,
    })
}

/// Retarget native decoded clips and return the original rig GLB augmented
/// with named animation channels.
pub fn retarget_hy_motion_glb(
    rigged_glb: &[u8],
    clips: &[HyMotionClipRef<'_>],
    options: &RetargetOptions,
) -> Result<Vec<u8>, RetargetError> {
    Ok(retarget_hy_motion_glb_with_report(rigged_glb, clips, options)?.glb)
}

pub fn retarget_hy_motion_glb_with_report(
    rigged_glb: &[u8],
    clips: &[HyMotionClipRef<'_>],
    options: &RetargetOptions,
) -> Result<RetargetOutput, RetargetError> {
    if clips.is_empty() {
        return Err(RetargetError::Motion(
            "at least one decoded clip is required".to_string(),
        ));
    }
    if !options.fps.is_finite() || options.fps <= 0.0 {
        return Err(RetargetError::Motion(
            "retarget FPS must be finite and positive".to_string(),
        ));
    }
    let parsed = parse_glb_bytes(rigged_glb)?;
    let rig = Rig::classify(&parsed.document.nodes_slice(), parsed.document.skins.as_deref())?;
    let mut clip_names = HashSet::with_capacity(clips.len());
    for clip in clips {
        validate_motion(clip.name, clip.motion)?;
        if !clip_names.insert(clip.name) {
            return Err(RetargetError::Motion(format!(
                "duplicate clip name '{}'",
                clip.name
            )));
        }
    }

    let mut output_clips = Vec::with_capacity(clips.len());
    let mut total_frames = 0usize;
    for clip in clips {
        total_frames += clip.motion.frames;
        output_clips.push(rig.retarget_clip(*clip, options)?);
    }
    let glb = replace_glb_node_animations(rigged_glb, &output_clips)?;
    Ok(RetargetOutput {
        glb,
        report: RetargetReport {
            joints: rig.joints.len(),
            mapped_joints: rig.mapping.len(),
            clips: clips.len(),
            frames: total_frames,
            mirrored: rig.mirrored,
            rig_height: rig.height as f32,
            motion_scale: rig.scale as f32,
        },
    })
}

fn validate_motion(name: &str, motion: &HyMotionDecoded) -> Result<(), RetargetError> {
    if name.trim().is_empty() {
        return Err(RetargetError::Motion(
            "clip name must not be empty".to_string(),
        ));
    }
    let frames = motion.frames;
    let expected_keypoints = frames * HY_MOTION_WOODEN_JOINTS * 3;
    if frames == 0
        || motion.translations.len() != frames * 3
        || motion.root_rotation_matrices.len() != frames * 9
        || motion.keypoints_3d.len() != expected_keypoints
    {
        return Err(RetargetError::Motion(format!(
            "clip '{name}' has inconsistent decoded shapes: frames={frames}, translations={}, root_rotations={}, keypoints={}",
            motion.translations.len(),
            motion.root_rotation_matrices.len(),
            motion.keypoints_3d.len(),
        )));
    }
    if motion
        .translations
        .iter()
        .chain(&motion.root_rotation_matrices)
        .chain(&motion.keypoints_3d)
        .any(|value| !value.is_finite())
    {
        return Err(RetargetError::Motion(format!(
            "clip '{name}' contains non-finite decoded values"
        )));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
struct V3 {
    x: f64,
    y: f64,
    z: f64,
}

impl V3 {
    const X: Self = Self { x: 1.0, y: 0.0, z: 0.0 };
    const Y: Self = Self { x: 0.0, y: 1.0, z: 0.0 };

    fn new(value: [f32; 3]) -> Self {
        Self {
            x: value[0] as f64,
            y: value[1] as f64,
            z: value[2] as f64,
        }
    }

    fn from_slice(value: &[f32]) -> Self {
        Self::new([value[0], value[1], value[2]])
    }

    fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    fn normalized(self) -> Self {
        let length = self.length();
        if length > EPS {
            self / length
        } else {
            Self::default()
        }
    }

    fn component_mul(self, other: Self) -> Self {
        Self {
            x: self.x * other.x,
            y: self.y * other.y,
            z: self.z * other.z,
        }
    }

    fn to_f32(self) -> [f32; 3] {
        [self.x as f32, self.y as f32, self.z as f32]
    }
}

impl std::ops::Add for V3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self { x: self.x + rhs.x, y: self.y + rhs.y, z: self.z + rhs.z }
    }
}

impl std::ops::Sub for V3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self { x: self.x - rhs.x, y: self.y - rhs.y, z: self.z - rhs.z }
    }
}

impl std::ops::Mul<f64> for V3 {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self {
        Self { x: self.x * rhs, y: self.y * rhs, z: self.z * rhs }
    }
}

impl std::ops::Div<f64> for V3 {
    type Output = Self;
    fn div(self, rhs: f64) -> Self {
        Self { x: self.x / rhs, y: self.y / rhs, z: self.z / rhs }
    }
}

#[derive(Clone, Copy, Debug)]
struct M3 {
    // Row-major.
    m: [[f64; 3]; 3],
}

impl M3 {
    const IDENTITY: Self = Self {
        m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    fn from_quaternion(value: [f32; 4]) -> Self {
        let mut x = value[0] as f64;
        let mut y = value[1] as f64;
        let mut z = value[2] as f64;
        let mut w = value[3] as f64;
        let length = (x * x + y * y + z * z + w * w).sqrt();
        if length <= EPS {
            return Self::IDENTITY;
        }
        x /= length;
        y /= length;
        z /= length;
        w /= length;
        Self {
            m: [
                [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
                [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
                [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
            ],
        }
    }

    fn from_rows(values: &[f32]) -> Self {
        Self {
            m: [
                [values[0] as f64, values[1] as f64, values[2] as f64],
                [values[3] as f64, values[4] as f64, values[5] as f64],
                [values[6] as f64, values[7] as f64, values[8] as f64],
            ],
        }
    }

    fn yaw(angle: f64) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self {
            m: [[cos, 0.0, sin], [0.0, 1.0, 0.0], [-sin, 0.0, cos]],
        }
    }

    fn transpose(self) -> Self {
        Self {
            m: [
                [self.m[0][0], self.m[1][0], self.m[2][0]],
                [self.m[0][1], self.m[1][1], self.m[2][1]],
                [self.m[0][2], self.m[1][2], self.m[2][2]],
            ],
        }
    }

    fn mul_vec(self, value: V3) -> V3 {
        V3 {
            x: self.m[0][0] * value.x + self.m[0][1] * value.y + self.m[0][2] * value.z,
            y: self.m[1][0] * value.x + self.m[1][1] * value.y + self.m[1][2] * value.z,
            z: self.m[2][0] * value.x + self.m[2][1] * value.y + self.m[2][2] * value.z,
        }
    }

    fn mul(self, rhs: Self) -> Self {
        let mut out = [[0.0; 3]; 3];
        for (row, values) in out.iter_mut().enumerate() {
            for (column, value) in values.iter_mut().enumerate() {
                *value = (0..3).map(|index| self.m[row][index] * rhs.m[index][column]).sum();
            }
        }
        Self { m: out }
    }

    fn column(self, index: usize) -> V3 {
        V3 { x: self.m[0][index], y: self.m[1][index], z: self.m[2][index] }
    }

    fn from_columns(c0: V3, c1: V3, c2: V3) -> Self {
        Self {
            m: [[c0.x, c1.x, c2.x], [c0.y, c1.y, c2.y], [c0.z, c1.z, c2.z]],
        }
    }

    fn orthonormalized(self) -> Self {
        let x = self.column(0).normalized();
        let mut y = self.column(1) - x * x.dot(self.column(1));
        y = y.normalized();
        let z = x.cross(y).normalized();
        // Preserve a reflected third column as closely as a quaternion can:
        // rig transforms are expected to be proper rotations, but this makes
        // malformed/mirrored matrices deterministic rather than NaN.
        let y = z.cross(x).normalized();
        Self::from_columns(x, y, z)
    }

    fn quaternion_xyzw(self) -> [f32; 4] {
        let matrix = self.orthonormalized().m;
        let trace = matrix[0][0] + matrix[1][1] + matrix[2][2];
        let (x, y, z, w) = if trace > 0.0 {
            let s = (trace + 1.0).sqrt() * 2.0;
            (
                (matrix[2][1] - matrix[1][2]) / s,
                (matrix[0][2] - matrix[2][0]) / s,
                (matrix[1][0] - matrix[0][1]) / s,
                0.25 * s,
            )
        } else if matrix[0][0] > matrix[1][1] && matrix[0][0] > matrix[2][2] {
            let s = (1.0 + matrix[0][0] - matrix[1][1] - matrix[2][2]).sqrt() * 2.0;
            (
                0.25 * s,
                (matrix[0][1] + matrix[1][0]) / s,
                (matrix[0][2] + matrix[2][0]) / s,
                (matrix[2][1] - matrix[1][2]) / s,
            )
        } else if matrix[1][1] > matrix[2][2] {
            let s = (1.0 + matrix[1][1] - matrix[0][0] - matrix[2][2]).sqrt() * 2.0;
            (
                (matrix[0][1] + matrix[1][0]) / s,
                0.25 * s,
                (matrix[1][2] + matrix[2][1]) / s,
                (matrix[0][2] - matrix[2][0]) / s,
            )
        } else {
            let s = (1.0 + matrix[2][2] - matrix[0][0] - matrix[1][1]).sqrt() * 2.0;
            (
                (matrix[0][2] + matrix[2][0]) / s,
                (matrix[1][2] + matrix[2][1]) / s,
                0.25 * s,
                (matrix[1][0] - matrix[0][1]) / s,
            )
        };
        let length = (x * x + y * y + z * z + w * w).sqrt();
        [(x / length) as f32, (y / length) as f32, (z / length) as f32, (w / length) as f32]
    }
}

#[derive(Clone, Copy, Debug)]
struct Trs {
    t: V3,
    r: M3,
    s: V3,
}

impl Default for Trs {
    fn default() -> Self {
        Self { t: V3::default(), r: M3::IDENTITY, s: V3 { x: 1.0, y: 1.0, z: 1.0 } }
    }
}

impl Trs {
    fn compose(self, local: Self) -> Self {
        Self {
            t: self.t + self.r.mul_vec(self.s.component_mul(local.t)),
            r: self.r.mul(local.r),
            s: self.s.component_mul(local.s),
        }
    }

    fn inverse_point(self, world: V3) -> V3 {
        let rotated = self.r.transpose().mul_vec(world - self.t);
        V3 {
            x: rotated.x / self.s.x,
            y: rotated.y / self.s.y,
            z: rotated.z / self.s.z,
        }
    }
}

fn node_trs(node: &GltfNode) -> Result<Trs, RetargetError> {
    if let Some(matrix) = node.matrix {
        let t = V3 { x: matrix[12] as f64, y: matrix[13] as f64, z: matrix[14] as f64 };
        let c0 = V3 { x: matrix[0] as f64, y: matrix[1] as f64, z: matrix[2] as f64 };
        let c1 = V3 { x: matrix[4] as f64, y: matrix[5] as f64, z: matrix[6] as f64 };
        let c2 = V3 { x: matrix[8] as f64, y: matrix[9] as f64, z: matrix[10] as f64 };
        let s = V3 { x: c0.length(), y: c1.length(), z: c2.length() };
        if s.x <= EPS || s.y <= EPS || s.z <= EPS {
            return Err(RetargetError::Rig("joint matrix has a zero scale axis".to_string()));
        }
        return Ok(Trs {
            t,
            r: M3::from_columns(c0 / s.x, c1 / s.y, c2 / s.z).orthonormalized(),
            s,
        });
    }
    Ok(Trs {
        t: V3::new(node.translation.unwrap_or([0.0; 3])),
        r: M3::from_quaternion(node.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0])),
        s: V3::new(node.scale.unwrap_or([1.0; 3])),
    })
}

fn json_usize(value: &JsonValue) -> Option<usize> {
    match value {
        JsonValue::U64(value) => usize::try_from(*value).ok(),
        JsonValue::U128(value) => usize::try_from(*value).ok(),
        JsonValue::I64(value) if *value >= 0 => usize::try_from(*value).ok(),
        JsonValue::I128(value) if *value >= 0 => usize::try_from(*value).ok(),
        JsonValue::F64(value) if value.is_finite() && *value >= 0.0 && value.fract() == 0.0 => {
            Some(*value as usize)
        }
        _ => None,
    }
}

fn skin_joint_nodes(skins: Option<&[JsonValue]>, node_count: usize) -> Result<Vec<usize>, RetargetError> {
    let skin = skins
        .and_then(|skins| skins.first())
        .ok_or_else(|| RetargetError::Rig("GLB has no skin".to_string()))?;
    let joints = match skin.key("joints") {
        Some(JsonValue::Array(joints)) => joints,
        _ => return Err(RetargetError::Rig("skin 0 has no joints array".to_string())),
    };
    let mut result = Vec::with_capacity(joints.len());
    let mut unique = HashSet::new();
    for value in joints {
        let joint = json_usize(value)
            .ok_or_else(|| RetargetError::Rig("skin contains a non-integer joint".to_string()))?;
        if joint >= node_count {
            return Err(RetargetError::Rig(format!(
                "skin joint node {joint} is out of range ({node_count})"
            )));
        }
        if !unique.insert(joint) {
            return Err(RetargetError::Rig(format!("skin repeats joint node {joint}")));
        }
        result.push(joint);
    }
    if result.is_empty() {
        return Err(RetargetError::Rig("skin 0 has no joints".to_string()));
    }
    Ok(result)
}

#[derive(Clone, Copy)]
struct Mapping {
    from: usize,
    to: usize,
    rig_child: Option<usize>,
}

fn assign_chain(
    mapping: &mut HashMap<usize, Mapping>,
    chain: &[usize],
    pairs: &[(usize, usize)],
) {
    for (index, &(from, to)) in pairs.iter().enumerate().take(chain.len()) {
        mapping.insert(
            chain[index],
            Mapping {
                from,
                to,
                rig_child: chain.get(index + 1).copied(),
            },
        );
    }
}

struct Rig {
    joints: Vec<usize>,
    joint_set: HashSet<usize>,
    parents: Vec<Option<usize>>,
    local: Vec<Trs>,
    rest_global: Vec<Trs>,
    rig_base: Trs,
    root: usize,
    order: Vec<usize>,
    heads: Vec<V3>,
    tails: Vec<V3>,
    mapping: HashMap<usize, Mapping>,
    coordinate: M3,
    mirrored: bool,
    height: f64,
    scale: f64,
    arm_nodes: Vec<usize>,
    leg_nodes: Vec<usize>,
}

fn joint_children(
    node: usize,
    children: &[Vec<usize>],
    joint_set: &HashSet<usize>,
) -> Vec<usize> {
    children[node]
        .iter()
        .copied()
        .filter(|child| joint_set.contains(child))
        .collect()
}

fn subtree_min_head_y(
    start: usize,
    children: &[Vec<usize>],
    joint_set: &HashSet<usize>,
    heads: &[V3],
) -> f64 {
    let mut minimum = f64::INFINITY;
    let mut pending = vec![start];
    while let Some(node) = pending.pop() {
        minimum = minimum.min(heads[node].y);
        pending.extend(joint_children(node, children, joint_set));
    }
    minimum
}

fn subtree_max_head_y(
    start: usize,
    children: &[Vec<usize>],
    joint_set: &HashSet<usize>,
    heads: &[V3],
) -> f64 {
    let mut maximum = f64::NEG_INFINITY;
    let mut pending = vec![start];
    while let Some(node) = pending.pop() {
        maximum = maximum.max(heads[node].y);
        pending.extend(joint_children(node, children, joint_set));
    }
    maximum
}

fn humanoid_root_branches(
    root: usize,
    children: &[Vec<usize>],
    joint_set: &HashSet<usize>,
    heads: &[V3],
) -> Result<([usize; 2], usize), RetargetError> {
    let root_children = joint_children(root, children, joint_set);
    let direct_legs: Vec<usize> = root_children
        .iter()
        .copied()
        .filter(|child| heads[*child].y < heads[root].y - 1.0e-6)
        .collect();
    let direct_spine: Vec<usize> = root_children
        .iter()
        .copied()
        .filter(|child| !direct_legs.contains(child))
        .collect();
    if direct_legs.len() == 2 && direct_spine.len() == 1 {
        return Ok(([direct_legs[0], direct_legs[1]], direct_spine[0]));
    }

    // If the immediate hip heads do not point downward, retry with the
    // vertical extent of each whole root branch. Keep this strictly as a
    // fallback: on a normal human rig the arms/hands can extend below the
    // pelvis, so subtree extent must not override two already-valid direct
    // hip edges.
    let descending_branches: Vec<usize> = root_children
        .iter()
        .copied()
        .filter(|child| {
            subtree_min_head_y(*child, children, joint_set, heads)
                < heads[root].y - 1.0e-6
        })
        .collect();
    let upward_branches: Vec<usize> = root_children
        .iter()
        .copied()
        .filter(|child| !descending_branches.contains(child))
        .collect();
    if descending_branches.len() == 2 && upward_branches.len() == 1 {
        return Ok((
            [descending_branches[0], descending_branches[1]],
            upward_branches[0],
        ));
    }

    // Long A-pose arms may end below the pelvis, so a valid torso subtree can
    // be both the highest-rising and a descending branch.  When the root has
    // exactly three children, select a unique torso by upward reach and then
    // require the other two branches to form a mirrored, depth-matched leg
    // pair.  This keeps the fallback fail-closed for tail/accessory roots.
    if root_children.len() == 3 {
        let mut by_height: Vec<(usize, f64)> = root_children
            .iter()
            .copied()
            .map(|child| (child, subtree_max_head_y(child, children, joint_set, heads)))
            .collect();
        by_height.sort_by(|left, right| right.1.total_cmp(&left.1));
        let torso = by_height[0].0;
        let legs: Vec<usize> = root_children
            .iter()
            .copied()
            .filter(|child| *child != torso)
            .collect();
        let torso_rise = by_height[0].1 - heads[root].y;
        let next_rise = by_height[1].1 - heads[root].y;
        let leg_min = [
            subtree_min_head_y(legs[0], children, joint_set, heads),
            subtree_min_head_y(legs[1], children, joint_set, heads),
        ];
        let leg_depth = [heads[root].y - leg_min[0], heads[root].y - leg_min[1]];
        let lateral = [heads[legs[0]].x - heads[root].x, heads[legs[1]].x - heads[root].x];
        let lateral_abs = [lateral[0].abs(), lateral[1].abs()];
        let minimum_lateral = lateral_abs[0].min(lateral_abs[1]);
        let mean_lateral = (lateral_abs[0] + lateral_abs[1]) * 0.5;
        let torso_dx = heads[torso].x - heads[root].x;
        let hip_y_delta = (heads[legs[0]].y - heads[legs[1]].y).abs();
        let hip_z_delta = (heads[legs[0]].z - heads[legs[1]].z).abs();
        let mirrored = lateral[0] * lateral[1] < 0.0;
        let depth_ratio = leg_depth[0].max(leg_depth[1])
            / leg_depth[0].min(leg_depth[1]).max(MIN_BONE_LENGTH);
        let lateral_ratio = lateral_abs[0].max(lateral_abs[1])
            / minimum_lateral.max(MIN_BONE_LENGTH);
        if torso_rise > next_rise + MIN_BONE_LENGTH
            && leg_depth.iter().all(|depth| *depth > MIN_BONE_LENGTH)
            && mirrored
            && depth_ratio <= 2.0
            && torso_dx.abs() <= 0.35 * minimum_lateral
            && lateral_ratio <= 1.5
            && (lateral[0] + lateral[1]).abs()
                <= 0.25 * (lateral_abs[0] + lateral_abs[1])
            && hip_y_delta <= 0.25 * mean_lateral
            && hip_z_delta <= 0.25 * mean_lateral
        {
            return Ok(([legs[0], legs[1]], torso));
        }
    }

    // SkinTokens can emit a zero-length pelvis helper chain below the
    // skeleton root, with the two hips branching from the last helper. The
    // helpers are semantically transparent: they inherit root motion while
    // the first non-zero branches remain the driven hip joints.
    let mut pelvis_candidates = Vec::new();
    for &root_child in &root_children {
        let mut bridge = root_child;
        loop {
            if (heads[bridge] - heads[root]).length() > MIN_BONE_LENGTH {
                break;
            }
            let next = joint_children(bridge, children, joint_set);
            if next.len() == 1 {
                bridge = next[0];
                continue;
            }
            if next.len() == 2
                && next.iter().all(|leg| {
                    subtree_min_head_y(*leg, children, joint_set, heads)
                        < heads[root].y - 1.0e-6
                })
            {
                pelvis_candidates.push((root_child, [next[0], next[1]]));
            }
            break;
        }
    }
    if pelvis_candidates.len() == 1 {
        let (pelvis_child, legs) = pelvis_candidates[0];
        let spine: Vec<usize> = root_children
            .iter()
            .copied()
            .filter(|child| *child != pelvis_child)
            .collect();
        if spine.len() == 1 {
            return Ok((legs, spine[0]));
        }
    }

    Err(RetargetError::Rig(format!(
        "unexpected humanoid root children {root_children:?}"
    )))
}

fn humanoid_chain_down(
    start: usize,
    children: &[Vec<usize>],
    joint_set: &HashSet<usize>,
) -> Vec<usize> {
    let mut chain = vec![start];
    loop {
        let next = joint_children(*chain.last().unwrap(), children, joint_set);
        if next.len() != 1 {
            break;
        }
        chain.push(next[0]);
    }
    chain
}

fn joint_subtree_nodes(
    start: usize,
    children: &[Vec<usize>],
    joint_set: &HashSet<usize>,
) -> Vec<usize> {
    let mut result = Vec::new();
    let mut pending = vec![start];
    while let Some(node) = pending.pop() {
        result.push(node);
        pending.extend(joint_children(node, children, joint_set));
    }
    result
}

/// Number of arm joints that represent independently driven anatomical
/// segments. A terminal leaf after the forearm is the hand endpoint: it has
/// no real distal edge of its own, so it inherits the driven forearm/wrist
/// joint exactly like a terminal foot endpoint. Conversely, a chain that
/// stops because its wrist branches still maps that wrist; a real branch is
/// then selected as its hand-direction edge below.
fn mapped_arm_bones(
    chain: &[usize],
    children: &[Vec<usize>],
    joint_set: &HashSet<usize>,
) -> usize {
    let mut mapped = chain.len().min(4);
    if mapped == chain.len()
        && mapped > 0
        && joint_children(chain[mapped - 1], children, joint_set).is_empty()
    {
        mapped -= 1;
    }
    mapped
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CoincidentSplitAnkle {
    selected_branch: usize,
    selected_leaf: usize,
    sibling_branch: usize,
    sibling_leaf: usize,
}

/// Recognize SkinTokens' duplicated ankle-head topology at a knee fork.
///
/// The two branches must each be a real knee-to-ankle segment followed by a
/// terminal shoe-direction edge. The scale-relative gates keep a tiny
/// coincident accessory fork from being mistaken for the two halves of a
/// weighted shoe merely because it has the same child counts.
fn coincident_split_ankle(
    knee: usize,
    children: &[Vec<usize>],
    joint_set: &HashSet<usize>,
    heads: &[V3],
) -> Option<CoincidentSplitAnkle> {
    let branches = joint_children(knee, children, joint_set);
    if branches.len() != 2 {
        return None;
    }

    let mut leaves = [0usize; 2];
    let mut segment_lengths = [0.0f64; 2];
    let mut horizontal = [0.0f64; 2];
    for index in 0..2 {
        let branch = branches[index];
        let branch_children = joint_children(branch, children, joint_set);
        if branch_children.len() != 1
            || !joint_children(branch_children[0], children, joint_set).is_empty()
        {
            return None;
        }
        let leaf = branch_children[0];
        let segment = heads[branch] - heads[knee];
        let leaf_delta = heads[leaf] - heads[branch];
        leaves[index] = leaf;
        segment_lengths[index] = segment.length();
        horizontal[index] = (leaf_delta.x * leaf_delta.x + leaf_delta.z * leaf_delta.z).sqrt();
    }

    if segment_lengths
        .iter()
        .any(|length| *length <= MIN_BONE_LENGTH)
    {
        return None;
    }
    let segment = (segment_lengths[0] + segment_lengths[1]) * 0.5;
    if (heads[branches[0]] - heads[branches[1]]).length() > 0.05 * segment
        || horizontal
            .iter()
            .any(|distance| *distance <= MIN_BONE_LENGTH || *distance < 0.05 * segment)
        || horizontal[0].max(horizontal[1]) < 0.15 * segment
    {
        return None;
    }

    let selected = if horizontal[0] >= horizontal[1] { 0 } else { 1 };
    let sibling = 1 - selected;
    Some(CoincidentSplitAnkle {
        selected_branch: branches[selected],
        selected_leaf: leaves[selected],
        sibling_branch: branches[sibling],
        sibling_leaf: leaves[sibling],
    })
}

fn humanoid_leg_chain(
    start: usize,
    children: &[Vec<usize>],
    joint_set: &HashSet<usize>,
    heads: &[V3],
) -> Vec<usize> {
    let mut chain = humanoid_chain_down(start, children, joint_set);
    let ankle = *chain.last().unwrap();
    let endpoints = joint_children(ankle, children, joint_set);
    if let Some(split) = coincident_split_ankle(ankle, children, joint_set, heads) {
        // Keep the branch with the strongest genuine horizontal shoe edge so
        // the third driven segment remains ankle -> foot.
        chain.push(split.selected_branch);
        chain.push(split.selected_leaf);
        return chain;
    }
    if endpoints.len() > 1
        && endpoints
            .iter()
            .all(|endpoint| joint_children(*endpoint, children, joint_set).is_empty())
    {
        // A split terminal ankle commonly encodes heel/toe endpoints. Keep
        // the endpoint with the strongest horizontal displacement as the
        // foot direction instead of stopping at a directionless ankle.
        if let Some(foot) = endpoints.into_iter().max_by(|left, right| {
            let left_delta = heads[*left] - heads[ankle];
            let right_delta = heads[*right] - heads[ankle];
            (left_delta.x * left_delta.x + left_delta.z * left_delta.z)
                .total_cmp(&(right_delta.x * right_delta.x + right_delta.z * right_delta.z))
        }) {
            chain.push(foot);
        }
    }
    chain
}

/// Return a second coincident ankle branch when SkinTokens represents a
/// shoe with two weighted ankle->endpoint chains. Both branches must receive
/// the same bind-relative source foot delta or the un-driven half of the shoe
/// inherits only the knee rotation and shears away during locomotion.
fn split_ankle_sibling(
    chain: &[usize],
    children: &[Vec<usize>],
    joint_set: &HashSet<usize>,
    heads: &[V3],
) -> Option<(usize, usize)> {
    let knee = *chain.get(1)?;
    let selected = *chain.get(2)?;
    let split = coincident_split_ankle(knee, children, joint_set, heads)?;
    (split.selected_branch == selected).then_some((split.sibling_branch, split.sibling_leaf))
}

fn longest_nontrivial_joint_child(
    start: usize,
    children: &[Vec<usize>],
    joint_set: &HashSet<usize>,
    heads: &[V3],
) -> Option<usize> {
    joint_children(start, children, joint_set)
        .into_iter()
        .filter_map(|child| {
            let length = (heads[child] - heads[start]).length();
            (length > MIN_BONE_LENGTH).then_some((child, length))
        })
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(child, _)| child)
}

fn fill_terminal_branch_child(
    mapping: &mut HashMap<usize, Mapping>,
    chain: &[usize],
    mapped_bones: usize,
    children: &[Vec<usize>],
    joint_set: &HashSet<usize>,
    heads: &[V3],
) {
    let Some(&terminal) = chain.iter().take(mapped_bones).last() else {
        return;
    };
    let Some(entry) = mapping.get_mut(&terminal) else {
        return;
    };
    if entry.rig_child.is_none() {
        entry.rig_child =
            longest_nontrivial_joint_child(terminal, children, joint_set, heads);
    }
}

/// A mapped wrist must be driven by an actual hierarchy edge. Falling back
/// to the imported bone's synthetic tail is especially dangerous here: the
/// tail direction is presentation metadata, not the direction of the hand,
/// and can rotate an otherwise valid skin by nearly 180 degrees.
fn validate_mapped_hand_directions(
    mapping: &HashMap<usize, Mapping>,
    heads: &[V3],
) -> Result<(), RetargetError> {
    for (&node, mapped) in mapping {
        if !matches!(mapped.to, L_MIDDLE1 | R_MIDDLE1) {
            continue;
        }
        let child = mapped.rig_child.ok_or_else(|| {
            RetargetError::Rig(format!(
                "mapped wrist node {node} has no real hand-direction child"
            ))
        })?;
        let direction = heads
            .get(child)
            .zip(heads.get(node))
            .map(|(child, wrist)| *child - *wrist)
            .ok_or_else(|| {
                RetargetError::Rig(format!(
                    "mapped wrist node {node} has out-of-range hand child {child}"
                ))
            })?;
        if !direction.x.is_finite()
            || !direction.y.is_finite()
            || !direction.z.is_finite()
            || direction.length() <= MIN_BONE_LENGTH
        {
            return Err(RetargetError::Rig(format!(
                "mapped wrist node {node} has degenerate hand child {child}"
            )));
        }
    }
    Ok(())
}

impl Rig {
    fn classify(nodes: &[GltfNode], skins: Option<&[JsonValue]>) -> Result<Self, RetargetError> {
        let joints = skin_joint_nodes(skins, nodes.len())?;
        let joint_set: HashSet<usize> = joints.iter().copied().collect();
        let mut parents = vec![None; nodes.len()];
        let mut children = vec![Vec::new(); nodes.len()];
        for (parent, node) in nodes.iter().enumerate() {
            for &child in node.children.as_deref().unwrap_or(&[]) {
                if child >= nodes.len() {
                    return Err(RetargetError::Rig(format!(
                        "node {parent} has out-of-range child {child}"
                    )));
                }
                if parents[child].replace(parent).is_some() {
                    return Err(RetargetError::Rig(format!("node {child} has multiple parents")));
                }
                children[parent].push(child);
            }
        }
        let local: Vec<Trs> = nodes.iter().map(node_trs).collect::<Result<_, _>>()?;
        let mut rest_global = vec![Trs::default(); nodes.len()];
        let mut state = vec![0u8; nodes.len()];
        fn resolve(
            node: usize,
            parents: &[Option<usize>],
            local: &[Trs],
            global: &mut [Trs],
            state: &mut [u8],
        ) -> Result<Trs, RetargetError> {
            match state[node] {
                2 => return Ok(global[node]),
                1 => return Err(RetargetError::Rig("node hierarchy contains a cycle".to_string())),
                _ => {}
            }
            state[node] = 1;
            let value = match parents[node] {
                Some(parent) => resolve(parent, parents, local, global, state)?.compose(local[node]),
                None => local[node],
            };
            global[node] = value;
            state[node] = 2;
            Ok(value)
        }
        for node in 0..nodes.len() {
            resolve(node, &parents, &local, &mut rest_global, &mut state)?;
        }

        let roots: Vec<usize> = joints
            .iter()
            .copied()
            .filter(|joint| parents[*joint].map_or(true, |parent| !joint_set.contains(&parent)))
            .collect();
        if roots.len() != 1 {
            return Err(RetargetError::Rig(format!(
                "humanoid retarget expects one skeleton root, found {}",
                roots.len()
            )));
        }
        let root = roots[0];
        let rig_base = parents[root].map_or(Trs::default(), |parent| rest_global[parent]);
        let relative = |point: V3| rig_base.inverse_point(point);
        let heads: Vec<V3> = rest_global.iter().map(|transform| relative(transform.t)).collect();

        // Match Blender glTF import's default BLENDER bone heuristic: a bone
        // uses the nearest nontrivial child distance, or inherits its parent's
        // length at a leaf. The resulting head/tail extent is exactly what the
        // Python oracle uses for scale and chain endpoint classification.
        let mut lengths = vec![None; nodes.len()];
        fn bone_length(
            joint: usize,
            parents: &[Option<usize>],
            children: &[Vec<usize>],
            joints: &HashSet<usize>,
            local: &[Trs],
            cache: &mut [Option<f64>],
        ) -> f64 {
            if let Some(length) = cache[joint] {
                return length;
            }
            let mut child_lengths: Vec<f64> = children[joint]
                .iter()
                .filter(|child| joints.contains(child))
                .map(|child| local[*child].t.length())
                .filter(|length| *length > MIN_BONE_LENGTH)
                .collect();
            child_lengths.sort_by(f64::total_cmp);
            let length = child_lengths.first().copied().unwrap_or_else(|| {
                parents[joint]
                    .filter(|parent| joints.contains(parent))
                    .map(|parent| bone_length(parent, parents, children, joints, local, cache))
                    .unwrap_or_else(|| local[joint].t.length().max(1.0))
            });
            cache[joint] = Some(length);
            length
        }
        for &joint in &joints {
            bone_length(joint, &parents, &children, &joint_set, &local, &mut lengths);
        }
        let mut tails = heads.clone();
        for &joint in &joints {
            let local_tail = V3::Y * lengths[joint].unwrap();
            let world_tail = rest_global[joint].t
                + rest_global[joint]
                    .r
                    .mul_vec(rest_global[joint].s.component_mul(local_tail));
            tails[joint] = relative(world_tail);
        }

        let (legs, spine_start) =
            humanoid_root_branches(root, &children, &joint_set, &heads)?;
        let leg_chains = [
            humanoid_leg_chain(legs[0], &children, &joint_set, &heads),
            humanoid_leg_chain(legs[1], &children, &joint_set, &heads),
        ];
        let mut spine = vec![spine_start];
        loop {
            let next = joint_children(*spine.last().unwrap(), &children, &joint_set);
            if next.len() != 1 {
                break;
            }
            spine.push(next[0]);
        }
        let chest = *spine.last().unwrap();
        let chest_children = joint_children(chest, &children, &joint_set);
        let child_chains: Vec<Vec<usize>> = chest_children
            .iter()
            .map(|child| humanoid_chain_down(*child, &children, &joint_set))
            .collect();
        let mut ranked: Vec<(usize, f64)> = child_chains
            .iter()
            .enumerate()
            .map(|(index, chain)| {
                let end = *chain.last().unwrap();
                (index, (tails[end].x - heads[chest].x).abs())
            })
            .collect();
        ranked.sort_by(|left, right| right.1.total_cmp(&left.1));
        if ranked.len() < 2 {
            return Err(RetargetError::Rig(format!(
                "chest node {chest} has fewer than two arm chains"
            )));
        }
        let arm_indices = [ranked[0].0, ranked[1].0];
        let arm_chains = [child_chains[arm_indices[0]].clone(), child_chains[arm_indices[1]].clone()];
        let neck_chain = child_chains
            .iter()
            .enumerate()
            .filter(|(index, _)| !arm_indices.contains(index))
            .max_by(|(_, left), (_, right)| {
                tails[*left.last().unwrap()]
                    .y
                    .total_cmp(&tails[*right.last().unwrap()].y)
            })
            .map(|(_, chain)| chain.clone())
            .unwrap_or_default();
        let lr = |chains: [Vec<usize>; 2]| -> (Vec<usize>, Vec<usize>) {
            if heads[chains[0][0]].x > heads[chains[1][0]].x {
                (chains[0].clone(), chains[1].clone())
            } else {
                (chains[1].clone(), chains[0].clone())
            }
        };
        let (left_leg, right_leg) = lr(leg_chains);
        let (left_arm, right_arm) = lr(arm_chains);
        let mut arm_nodes = joint_subtree_nodes(left_arm[0], &children, &joint_set);
        arm_nodes.extend(joint_subtree_nodes(right_arm[0], &children, &joint_set));
        arm_nodes.sort_unstable();
        arm_nodes.dedup();
        let mut leg_nodes = joint_subtree_nodes(left_leg[0], &children, &joint_set);
        leg_nodes.extend(joint_subtree_nodes(right_leg[0], &children, &joint_set));
        leg_nodes.sort_unstable();
        leg_nodes.dedup();

        let mut mapping = HashMap::new();
        // Drive the three actual limb segments: hip -> knee, knee -> ankle,
        // and ankle -> foot. `humanoid_leg_chain` may append a fourth joint
        // solely to give the ankle a real hierarchy-edge direction. That
        // endpoint is a leaf, not another anatomical segment. Driving it a
        // second time with the same ankle -> foot vector would align its
        // synthetic imported tail to a direction already consumed by its
        // parent; near foot-contact singularities this made the leaf snap by
        // 60-70 degrees between adjacent frames. Leave the endpoint at its
        // rest-local transform so it inherits the correctly driven ankle.
        assign_chain(&mut mapping, &left_leg, &[(L_HIP, L_KNEE), (L_KNEE, L_ANKLE), (L_ANKLE, L_FOOT)]);
        assign_chain(&mut mapping, &right_leg, &[(R_HIP, R_KNEE), (R_KNEE, R_ANKLE), (R_ANKLE, R_FOOT)]);
        for (chain, (from, to)) in [
            (left_leg.as_slice(), (L_ANKLE, L_FOOT)),
            (right_leg.as_slice(), (R_ANKLE, R_FOOT)),
        ] {
            if let Some((ankle, endpoint)) =
                split_ankle_sibling(chain, &children, &joint_set, &heads)
            {
                mapping.insert(
                    ankle,
                    Mapping {
                        from,
                        to,
                        rig_child: Some(endpoint),
                    },
                );
            }
        }
        let spine_pairs = [(SPINE1, SPINE2), (SPINE2, SPINE3), (SPINE3, NECK)];
        if spine.len() >= 3 {
            let last = spine.len() - 1;
            // Python's round() uses ties-to-even; preserve it for rigs with
            // an even number of spine bones (where last / 2 is x.5).
            let middle_floor = last / 2;
            let middle = if last % 2 == 0 || middle_floor % 2 == 0 {
                middle_floor
            } else {
                middle_floor + 1
            };
            let indices = [0, middle, last];
            for (&(from, to), &index) in spine_pairs.iter().zip(&indices) {
                mapping.insert(
                    spine[index],
                    Mapping {
                        from,
                        to,
                        rig_child: spine.get(index + 1).copied().or_else(|| neck_chain.first().copied()),
                    },
                );
            }
        } else {
            for (index, &joint) in spine.iter().enumerate() {
                let (from, to) = spine_pairs[index];
                mapping.insert(
                    joint,
                    Mapping {
                        from,
                        to,
                        rig_child: spine.get(index + 1).copied().or_else(|| neck_chain.first().copied()),
                    },
                );
            }
        }
        let left_arm_pairs = [(L_COLLAR, L_SHOULDER), (L_SHOULDER, L_ELBOW), (L_ELBOW, L_WRIST), (L_WRIST, L_MIDDLE1)];
        let right_arm_pairs = [(R_COLLAR, R_SHOULDER), (R_SHOULDER, R_ELBOW), (R_ELBOW, R_WRIST), (R_WRIST, R_MIDDLE1)];
        let left_arm_bones = mapped_arm_bones(&left_arm, &children, &joint_set);
        let right_arm_bones = mapped_arm_bones(&right_arm, &children, &joint_set);
        assign_chain(&mut mapping, &left_arm, &left_arm_pairs[..left_arm_bones]);
        assign_chain(&mut mapping, &right_arm, &right_arm_pairs[..right_arm_bones]);
        // `humanoid_chain_down` deliberately stops at a branch. At a wrist
        // with multiple hand/finger branches this leaves the final mapped
        // joint without a direction child, causing `rest_direction` to use
        // the decorative local +Y tail and fold the hand back toward the hip.
        // Use the longest real branch solely when the mapped chain itself did
        // not already supply a successor.
        fill_terminal_branch_child(
            &mut mapping,
            &left_arm,
            left_arm_bones,
            &children,
            &joint_set,
            &heads,
        );
        fill_terminal_branch_child(
            &mut mapping,
            &right_arm,
            right_arm_bones,
            &children,
            &joint_set,
            &heads,
        );
        validate_mapped_hand_directions(&mapping, &heads)?;
        for (index, &joint) in neck_chain.iter().enumerate() {
            mapping.insert(
                joint,
                Mapping { from: NECK, to: HEAD, rig_child: neck_chain.get(index + 1).copied() },
            );
        }

        let foot_direction = [left_leg.as_slice(), right_leg.as_slice()]
            .iter()
            .fold(V3::default(), |sum, chain| {
                let ankle = chain.get(2).copied().unwrap_or(*chain.last().unwrap());
                let foot = chain.get(3).copied().unwrap_or(*chain.last().unwrap());
                sum + (tails[foot] - heads[ankle])
            });
        let planar = V3 { y: 0.0, ..foot_direction }.normalized();
        if planar.length() <= EPS {
            return Err(RetargetError::Rig("could not infer rig facing direction".to_string()));
        }
        let coordinate = M3::yaw(planar.x.atan2(planar.z));

        // Use the official template's left-hip rest offset, not a potentially
        // crossed first animation frame, for the mirror gate.
        let smpl_left_hip = V3 {
            x: 0.08057684 - (-0.00179506),
            y: -0.28076518 - (-0.19082700),
            z: 0.02511700 - 0.02821912,
        };
        let rig_left_hip = heads[left_leg[0]] - heads[root];
        let mirrored = coordinate.mul_vec(smpl_left_hip).x * rig_left_hip.x < 0.0;
        if mirrored {
            for value in mapping.values_mut() {
                value.from = flip_lr(value.from);
                value.to = flip_lr(value.to);
            }
        }

        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for &joint in &joints {
            min_y = min_y.min(heads[joint].y).min(tails[joint].y);
            max_y = max_y.max(heads[joint].y).max(tails[joint].y);
        }
        let height = max_y - min_y;
        if !height.is_finite() || height <= EPS {
            return Err(RetargetError::Rig("rig has no finite vertical extent".to_string()));
        }

        let mut order = Vec::with_capacity(joints.len());
        fn topo(node: usize, children: &[Vec<usize>], joints: &HashSet<usize>, out: &mut Vec<usize>) {
            out.push(node);
            for &child in &children[node] {
                if joints.contains(&child) {
                    topo(child, children, joints, out);
                }
            }
        }
        topo(root, &children, &joint_set, &mut order);
        if order.len() != joints.len() {
            return Err(RetargetError::Rig(
                "skin contains joints outside the root hierarchy".to_string(),
            ));
        }

        Ok(Self {
            joints,
            joint_set,
            parents,
            local,
            rest_global,
            rig_base,
            root,
            order,
            heads,
            tails,
            mapping,
            coordinate,
            mirrored,
            height,
            scale: height / NOMINAL_HUMAN_HEIGHT,
            arm_nodes,
            leg_nodes,
        })
    }

    fn rest_rotation_in_rig(&self, node: usize) -> M3 {
        self.rig_base.r.transpose().mul(self.rest_global[node].r)
    }

    fn rest_direction(&self, node: usize, mapping: Mapping) -> V3 {
        mapping
            .rig_child
            .map(|child| self.heads[child] - self.heads[node])
            .unwrap_or(self.tails[node] - self.heads[node])
    }

    fn retarget_clip(
        &self,
        clip: HyMotionClipRef<'_>,
        options: &RetargetOptions,
    ) -> Result<GlbNodeAnimClip, RetargetError> {
        let frames = clip.motion.frames;
        // `retarget_multi.py` creates each NLA strip at Blender frame 1. Its
        // exported GLB therefore samples motion frame 0 at 1/fps (rather than
        // time zero) and motion frame T-1 at T/fps. Preserve that observable
        // clip contract so native and Blender outputs have identical timing.
        let times: Vec<f32> = (0..frames)
            .map(|frame| (frame + 1) as f32 / options.fps)
            .collect();
        let mut translations: HashMap<usize, Vec<f32>> = self
            .joints
            .iter()
            .map(|joint| (*joint, Vec::with_capacity(frames * 3)))
            .collect();
        let mut rotations: HashMap<usize, Vec<f32>> = self
            .joints
            .iter()
            .map(|joint| (*joint, Vec::with_capacity(frames * 4)))
            .collect();
        let mut scales: HashMap<usize, Vec<f32>> = self
            .joints
            .iter()
            .map(|joint| (*joint, Vec::with_capacity(frames * 3)))
            .collect();
        let initial_translation = V3::from_slice(&clip.motion.translations[..3]);

        for frame in 0..frames {
            let root_rotation = M3::from_rows(
                &clip.motion.root_rotation_matrices[frame * 9..frame * 9 + 9],
            );
            let delta = self
                .coordinate
                .mul(root_rotation)
                .mul(self.coordinate.transpose());
            let mut desired_global: HashMap<usize, M3> = HashMap::with_capacity(self.joints.len());
            for &node in &self.order {
                let desired = if node == self.root {
                    delta.mul(self.rest_rotation_in_rig(node))
                } else if let Some(mapping) = self.mapping.get(&node).copied() {
                    if matches!((mapping.from, mapping.to),
                        (L_ANKLE, L_FOOT) | (R_ANKLE, R_FOOT))
                    {
                        // Retarget the generated ankle frame as motion
                        // relative to WoodenMesh's own bind pose. Directly
                        // aligning the target edge to the absolute generated
                        // ankle->foot vector made this rig's shoes sit about
                        // 30 degrees toe-up even in neutral idle.
                        let source_delta = source_foot_delta(
                            clip.motion,
                            frame,
                            mapping.from,
                        )
                        .unwrap_or(M3::IDENTITY);
                        self.coordinate
                            .mul(source_delta)
                            .mul(self.coordinate.transpose())
                            .mul(self.rest_rotation_in_rig(node))
                    } else {
                        let from = keypoint(clip.motion, frame, mapping.from);
                        let to = keypoint(clip.motion, frame, mapping.to);
                        let target = self.coordinate.mul_vec(to - from);
                        rot_between(self.rest_direction(node, mapping), target)
                            .mul(self.rest_rotation_in_rig(node))
                    }
                } else {
                    let parent = self.parents[node].filter(|parent| self.joint_set.contains(parent));
                    match parent {
                        Some(parent) => desired_global[&parent].mul(self.local[node].r),
                        None => self.rest_rotation_in_rig(node),
                    }
                };
                desired_global.insert(node, desired);
            }

            for &node in &self.order {
                let parent = self.parents[node].filter(|parent| self.joint_set.contains(parent));
                let local_rotation = match parent {
                    Some(parent) => desired_global[&parent].transpose().mul(desired_global[&node]),
                    None => desired_global[&node],
                };
                let mut quaternion = local_rotation.quaternion_xyzw();
                let rotation_values = rotations.get_mut(&node).unwrap();
                if rotation_values.len() >= 4 {
                    let prior = &rotation_values[rotation_values.len() - 4..];
                    let dot = prior[0] * quaternion[0]
                        + prior[1] * quaternion[1]
                        + prior[2] * quaternion[2]
                        + prior[3] * quaternion[3];
                    if dot < 0.0 {
                        for value in &mut quaternion {
                            *value = -*value;
                        }
                    }
                }
                rotation_values.extend_from_slice(&quaternion);

                let translation = if node == self.root {
                    let travel = root_travel(
                        V3::from_slice(
                            &clip.motion.translations[frame * 3..frame * 3 + 3],
                        ),
                        initial_translation,
                        options.in_place,
                        options.strip_vertical_root,
                    );
                    self.local[node].t + self.coordinate.mul_vec(travel * self.scale)
                } else {
                    self.local[node].t
                };
                translations.get_mut(&node).unwrap().extend_from_slice(&translation.to_f32());
                scales.get_mut(&node).unwrap().extend_from_slice(&self.local[node].s.to_f32());
            }
        }

        // Match Blender's sampled NLA export shape (T/R/S per joint), while
        // targeting the original node indices rather than rebuilt nodes.
        let mut channels = Vec::with_capacity(self.joints.len() * 3);
        for &node in &self.order {
            channels.push(GlbNodeAnimChannel {
                node,
                path: GlbAnimPath::Translation,
                times: times.clone(),
                values: translations.remove(&node).unwrap(),
            });
            channels.push(GlbNodeAnimChannel {
                node,
                path: GlbAnimPath::Rotation,
                times: times.clone(),
                values: rotations.remove(&node).unwrap(),
            });
            channels.push(GlbNodeAnimChannel {
                node,
                path: GlbAnimPath::Scale,
                times: times.clone(),
                values: scales.remove(&node).unwrap(),
            });
        }
        Ok(GlbNodeAnimClip { name: clip.name.to_string(), channels })
    }
}

fn keypoint(motion: &HyMotionDecoded, frame: usize, joint: usize) -> V3 {
    let offset = (frame * HY_MOTION_WOODEN_JOINTS + joint) * 3;
    V3::from_slice(&motion.keypoints_3d[offset..offset + 3])
}

/// Complete orientation frame at a foot, using its forward ankle->foot edge
/// and the lower leg as the up/roll reference. A single direction leaves an
/// unconstrained twist around the shoe and, more importantly, cannot remove
/// the bind-pose pitch difference between the source and target skeletons.
fn foot_frame(forward: V3, up_hint: V3) -> Option<M3> {
    let z = forward.normalized();
    let projected_up = up_hint - z * z.dot(up_hint);
    let y = projected_up.normalized();
    let x = y.cross(z).normalized();
    if x.length() <= EPS || y.length() <= EPS || z.length() <= EPS {
        return None;
    }
    Some(M3::from_columns(x, z.cross(x).normalized(), z))
}

/// Generated WoodenMesh ankle orientation as a delta from its own neutral
/// frame. Applying this delta to the target's rest frame preserves the shoe's
/// authored level orientation while transferring pitch, yaw, and roll from
/// the motion. This is the standard bind-pose correction omitted by the old
/// one-vector ankle mapping.
fn source_foot_delta(
    motion: &HyMotionDecoded,
    frame: usize,
    ankle: usize,
) -> Option<M3> {
    let (knee, foot, rest_up, rest_forward) = match ankle {
        L_ANKLE => (L_KNEE, L_FOOT, L_ANKLE_TO_KNEE_REST, L_ANKLE_TO_FOOT_REST),
        R_ANKLE => (R_KNEE, R_FOOT, R_ANKLE_TO_KNEE_REST, R_ANKLE_TO_FOOT_REST),
        _ => return None,
    };
    let ankle_point = keypoint(motion, frame, ankle);
    let current = foot_frame(
        keypoint(motion, frame, foot) - ankle_point,
        keypoint(motion, frame, knee) - ankle_point,
    )?;
    let rest = foot_frame(rest_forward, rest_up)?;
    Some(current.mul(rest.transpose()))
}

fn root_travel(
    current: V3,
    initial: V3,
    in_place: bool,
    strip_vertical_root: bool,
) -> V3 {
    let mut travel = current - initial;
    if in_place {
        travel.x = 0.0;
        travel.z = 0.0;
    }
    if strip_vertical_root {
        travel.y = 0.0;
    }
    travel
}

fn flip_lr(joint: usize) -> usize {
    match joint {
        L_HIP => R_HIP,
        R_HIP => L_HIP,
        L_KNEE => R_KNEE,
        R_KNEE => L_KNEE,
        L_ANKLE => R_ANKLE,
        R_ANKLE => L_ANKLE,
        L_FOOT => R_FOOT,
        R_FOOT => L_FOOT,
        L_COLLAR => R_COLLAR,
        R_COLLAR => L_COLLAR,
        L_SHOULDER => R_SHOULDER,
        R_SHOULDER => L_SHOULDER,
        L_ELBOW => R_ELBOW,
        R_ELBOW => L_ELBOW,
        L_WRIST => R_WRIST,
        R_WRIST => L_WRIST,
        L_MIDDLE1 => R_MIDDLE1,
        R_MIDDLE1 => L_MIDDLE1,
        _ => joint,
    }
}

fn skew(value: V3) -> M3 {
    M3 {
        m: [[0.0, -value.z, value.y], [value.z, 0.0, -value.x], [-value.y, value.x, 0.0]],
    }
}

fn rot_between(from: V3, to: V3) -> M3 {
    let from = from.normalized();
    let to = to.normalized();
    if from.length() <= EPS || to.length() <= EPS {
        return M3::IDENTITY;
    }
    let cross = from.cross(to);
    let dot = from.dot(to).clamp(-1.0, 1.0);
    if dot > 0.999999 {
        return M3::IDENTITY;
    }
    if dot < -0.999999 {
        let mut axis = from.cross(V3::X);
        if axis.length() < 1.0e-6 {
            axis = from.cross(V3::Y);
        }
        let k = skew(axis.normalized());
        return M3::IDENTITY.add(k.mul(k).scale(2.0));
    }
    let k = skew(cross);
    M3::IDENTITY
        .add(k)
        .add(k.mul(k).scale(1.0 / (1.0 + dot)))
}

impl M3 {
    fn add(self, rhs: Self) -> Self {
        let mut out = self;
        for row in 0..3 {
            for column in 0..3 {
                out.m[row][column] += rhs.m[row][column];
            }
        }
        out
    }

    fn scale(self, value: f64) -> Self {
        let mut out = self;
        for row in &mut out.m {
            for component in row {
                *component *= value;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quaternion_matrix_roundtrip() {
        let q = [0.2, -0.3, 0.4, 0.84261495];
        let got = M3::from_quaternion(q).quaternion_xyzw();
        let dot = q.iter().zip(got).map(|(left, right)| left * right).sum::<f32>().abs();
        assert!((dot - 1.0).abs() < 1.0e-5, "dot {dot}: {got:?}");
    }

    #[test]
    fn rot_between_places_direction() {
        let from = V3 { x: 0.2, y: -0.7, z: 0.4 };
        let to = V3 { x: -0.3, y: 0.1, z: 0.9 };
        let got = rot_between(from, to).mul_vec(from.normalized());
        assert!((got - to.normalized()).length() < 1.0e-9);
        let opposite = rot_between(V3::Y, V3::Y * -1.0).mul_vec(V3::Y);
        assert!((opposite - V3::Y * -1.0).length() < 1.0e-9);
    }

    #[test]
    fn neutral_source_foot_frame_does_not_bake_target_bind_pitch() {
        let source_rest = foot_frame(L_ANKLE_TO_FOOT_REST, L_ANKLE_TO_KNEE_REST).unwrap();
        let source_neutral = foot_frame(L_ANKLE_TO_FOOT_REST, L_ANKLE_TO_KNEE_REST).unwrap();
        let delta = source_neutral.mul(source_rest.transpose());
        let target_forward = V3 { x: 0.0665, y: -0.1369, z: 0.0900 };

        assert!((delta.mul_vec(V3::X) - V3::X).length() < 1.0e-9);
        assert!((delta.mul_vec(V3::Y) - V3::Y).length() < 1.0e-9);
        // The historical absolute direction alignment contains the bind-pose
        // mismatch: this target edge is roughly 30 degrees steeper than the
        // WoodenMesh neutral foot and therefore rotates a level shoe toe-up.
        let old = rot_between(target_forward, L_ANKLE_TO_FOOT_REST);
        let old_angle = old.quaternion_xyzw()[3].abs().clamp(-1.0, 1.0).acos() * 2.0;
        assert!(old_angle.to_degrees() > 25.0, "old angle {old_angle}");
    }

    #[test]
    fn two_vector_foot_frame_transfers_roll_as_well_as_forward() {
        let rest = foot_frame(L_ANKLE_TO_FOOT_REST, L_ANKLE_TO_KNEE_REST).unwrap();
        let motion = M3::yaw(0.35).mul(M3::from_quaternion([
            0.0,
            0.0,
            (0.2f32).sin(),
            (0.2f32).cos(),
        ]));
        let current = foot_frame(
            motion.mul_vec(L_ANKLE_TO_FOOT_REST),
            motion.mul_vec(L_ANKLE_TO_KNEE_REST),
        )
        .unwrap();
        let recovered = current.mul(rest.transpose());
        for axis in [V3::X, V3::Y, V3 { x: 0.0, y: 0.0, z: 1.0 }] {
            assert!((recovered.mul_vec(axis) - motion.mul_vec(axis)).length() < 1.0e-9);
        }
    }

    fn synthetic_three_branch_root() -> (usize, Vec<Vec<usize>>, HashSet<usize>, Vec<V3>) {
        // Deliberately shuffled node indices and root-child order: the
        // classifier must depend only on hierarchy and geometry.
        let root = 5;
        let children = vec![
            vec![],
            vec![2],
            vec![3],
            vec![],
            vec![],
            vec![6, 9, 1],
            vec![7],
            vec![8],
            vec![],
            vec![10],
            vec![11],
            vec![],
        ];
        let joint_set: HashSet<usize> = [1, 2, 3, 5, 6, 7, 8, 9, 10, 11]
            .into_iter()
            .collect();
        let mut heads = vec![V3::default(); children.len()];
        heads[1] = V3 {
            x: 0.10,
            y: 0.015,
            z: 0.006,
        };
        heads[2] = V3 {
            x: 0.10,
            y: -0.20,
            z: 0.010,
        };
        heads[3] = V3 {
            x: 0.10,
            y: -0.46,
            z: 0.020,
        };
        heads[6] = V3 {
            x: -0.105,
            y: 0.014,
            z: 0.005,
        };
        heads[7] = V3 {
            x: -0.105,
            y: -0.19,
            z: 0.012,
        };
        heads[8] = V3 {
            x: -0.105,
            y: -0.45,
            z: 0.021,
        };
        heads[9] = V3 {
            x: 0.010,
            y: 0.075,
            z: 0.003,
        };
        heads[10] = V3 {
            x: 0.0,
            y: 0.34,
            z: 0.0,
        };
        // A low A-pose extremity makes all three subtrees descend, forcing
        // the exactly-three-child fallback under test.
        heads[11] = V3 {
            x: 0.20,
            y: -0.08,
            z: 0.02,
        };
        (root, children, joint_set, heads)
    }

    #[test]
    fn root_classifier_three_branch_fallback_is_geometry_only() {
        let (root, children, joint_set, heads) = synthetic_three_branch_root();
        let (legs, spine) =
            humanoid_root_branches(root, &children, &joint_set, &heads).unwrap();
        assert_eq!(legs, [6, 1]);
        assert_eq!(spine, 9);
    }

    #[test]
    fn root_classifier_rejects_off_center_pseudo_torso() {
        let (root, children, joint_set, mut heads) = synthetic_three_branch_root();
        heads[9].x = 0.040;
        assert!(humanoid_root_branches(root, &children, &joint_set, &heads).is_err());
    }

    #[test]
    fn root_classifier_rejects_asymmetric_hip_head_y_or_z() {
        let (root, children, joint_set, heads) = synthetic_three_branch_root();

        let mut asymmetric_y = heads.clone();
        asymmetric_y[6].y = 0.070;
        assert!(
            humanoid_root_branches(root, &children, &joint_set, &asymmetric_y).is_err(),
            "a vertically asymmetric pair is not a bilateral hip pair"
        );

        let mut asymmetric_z = heads;
        asymmetric_z[6].z = 0.080;
        assert!(
            humanoid_root_branches(root, &children, &joint_set, &asymmetric_z).is_err(),
            "a depth-asymmetric pair is not a bilateral hip pair"
        );
    }

    #[test]
    fn root_classifier_accepts_zero_length_pelvis_helpers() {
        let children = vec![
            vec![1, 5],
            vec![2],
            vec![3],
            vec![],
            vec![],
            vec![6],
            vec![7, 10],
            vec![8],
            vec![9],
            vec![],
            vec![11],
            vec![12],
            vec![],
        ];
        let joint_set: HashSet<usize> = (0..children.len()).collect();
        let mut heads = vec![V3::default(); children.len()];
        heads[1] = V3 { x: 0.0, y: 0.06, z: 0.0 };
        heads[2] = V3 { x: 0.0, y: 0.12, z: 0.0 };
        heads[3] = V3 { x: 0.0, y: 0.18, z: 0.0 };
        heads[7] = V3 { x: 0.08, y: 0.02, z: 0.0 };
        heads[8] = V3 { x: 0.08, y: -0.08, z: 0.0 };
        heads[9] = V3 { x: 0.08, y: -0.24, z: 0.02 };
        heads[10] = V3 { x: -0.08, y: 0.02, z: 0.0 };
        heads[11] = V3 { x: -0.08, y: -0.08, z: 0.0 };
        heads[12] = V3 { x: -0.08, y: -0.24, z: 0.02 };

        let (legs, spine) =
            humanoid_root_branches(0, &children, &joint_set, &heads).unwrap();
        assert_eq!(legs, [7, 10]);
        assert_eq!(spine, 1);
    }

    #[test]
    fn root_classifier_accepts_raised_hip_heads_when_legs_descend() {
        // Some compact SkinTokens rigs place both hip heads a little above
        // the skeleton root even though the knee/ankle chains clearly travel
        // downward. Classifying only the first root->hip edge rejects that
        // otherwise ordinary three-way humanoid root.
        let children = vec![
            vec![1, 4, 7],
            vec![2],
            vec![3],
            vec![],
            vec![5],
            vec![6],
            vec![],
            vec![8],
            vec![9],
            vec![],
        ];
        let joint_set: HashSet<usize> = (0..children.len()).collect();
        let heads = vec![
            V3 { x: 0.0, y: 0.0, z: 0.0 },
            V3 { x: 0.0, y: 0.09, z: 0.03 },
            V3 { x: 0.0, y: 0.18, z: 0.04 },
            V3 { x: 0.0, y: 0.25, z: 0.03 },
            V3 { x: 0.11, y: 0.012, z: 0.0 },
            V3 { x: 0.11, y: -0.05, z: 0.01 },
            V3 { x: 0.10, y: -0.22, z: 0.03 },
            V3 { x: -0.11, y: 0.012, z: 0.0 },
            V3 { x: -0.11, y: -0.05, z: 0.01 },
            V3 { x: -0.10, y: -0.22, z: 0.03 },
        ];

        let (legs, spine) =
            humanoid_root_branches(0, &children, &joint_set, &heads).unwrap();
        assert_eq!(legs, [4, 7]);
        assert_eq!(spine, 1);
    }

    #[test]
    fn root_classifier_distinguishes_low_hands_from_symmetric_raised_hips() {
        // The torso's long A-pose arm leaves end below the pelvis, so all
        // three root branches have descending subtrees.  Its uniquely higher
        // upward reach still distinguishes it from the mirrored leg pair.
        let children = vec![
            vec![1, 4, 7],
            vec![2],
            vec![3],
            vec![],
            vec![5],
            vec![6],
            vec![],
            vec![8],
            vec![9],
            vec![],
        ];
        let joint_set: HashSet<usize> = (0..children.len()).collect();
        let heads = vec![
            V3 { x: 0.0, y: 0.0, z: 0.0 },
            V3 { x: 0.0, y: 0.067, z: 0.02 },
            V3 { x: 0.0, y: 0.30, z: 0.0 },
            V3 { x: 0.14, y: -0.012, z: 0.03 },
            V3 { x: 0.043, y: 0.016, z: 0.016 },
            V3 { x: 0.071, y: -0.20, z: 0.02 },
            V3 { x: 0.10, y: -0.48, z: 0.02 },
            V3 { x: -0.043, y: 0.016, z: 0.016 },
            V3 { x: -0.067, y: -0.20, z: 0.02 },
            V3 { x: -0.10, y: -0.48, z: 0.02 },
        ];

        let (legs, spine) =
            humanoid_root_branches(0, &children, &joint_set, &heads).unwrap();
        assert_eq!(legs, [4, 7]);
        assert_eq!(spine, 1);
    }

    #[test]
    fn leg_chain_uses_forward_leaf_of_coincident_ankle_fork() {
        let children = vec![vec![1], vec![2, 4], vec![3], vec![], vec![5], vec![]];
        let joint_set: HashSet<usize> = (0..children.len()).collect();
        let heads = vec![
            V3 { x: 0.04, y: 0.02, z: 0.0 },
            V3 { x: 0.07, y: -0.20, z: 0.02 },
            V3 { x: 0.11, y: -0.45, z: -0.03 },
            V3 { x: 0.13, y: -0.47, z: 0.04 },
            V3 { x: 0.11, y: -0.45, z: -0.03 },
            V3 { x: 0.10, y: -0.49, z: -0.02 },
        ];

        let chain = humanoid_leg_chain(0, &children, &joint_set, &heads);
        assert_eq!(chain, vec![0, 1, 2, 3]);
        assert_eq!(
            split_ankle_sibling(&chain, &children, &joint_set, &heads),
            Some((4, 5))
        );
    }

    #[test]
    fn negligible_horizontal_accessory_fork_is_not_a_split_ankle() {
        let children = vec![vec![1], vec![2, 4], vec![3], vec![], vec![5], vec![]];
        let joint_set: HashSet<usize> = (0..children.len()).collect();
        let heads = vec![
            V3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            V3 {
                x: 0.0,
                y: -0.20,
                z: 0.0,
            },
            V3 {
                x: 0.0,
                y: -0.45,
                z: 0.0,
            },
            V3 {
                x: 0.006,
                y: -0.48,
                z: 0.0,
            },
            V3 {
                x: 0.0,
                y: -0.45,
                z: 0.0,
            },
            V3 {
                x: 0.0,
                y: -0.48,
                z: 0.007,
            },
        ];

        // Both accessory offsets are longer than the absolute epsilon, but
        // are too small relative to the 0.25-unit knee-to-ankle segment.
        assert_eq!(
            humanoid_leg_chain(0, &children, &joint_set, &heads),
            vec![0, 1]
        );
        assert_eq!(
            split_ankle_sibling(&[0, 1, 2, 3], &children, &joint_set, &heads),
            None
        );
    }

    #[test]
    fn generated_lib_19_raised_hip_topology_classifies_if_present() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/ai_content_library/lib-19.glb");
        if !path.is_file() {
            eprintln!("generated lib-19 rig fixture absent; skipping");
            return;
        }
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let parsed = parse_glb_bytes(&bytes).unwrap();
        let nodes = parsed.document.nodes_slice();
        assert_eq!(nodes[1].children.as_deref(), Some(&[2, 15, 19][..]));

        let rig = Rig::classify(nodes, parsed.document.skins.as_deref()).unwrap();
        assert_eq!(rig.root, 1);
        assert_eq!(rig.joints.len(), 22);
        assert_eq!(rig.mapping.len(), 17);
        assert!(rig.heads[15].y > rig.heads[rig.root].y);
        assert!(rig.heads[19].y > rig.heads[rig.root].y);
        assert!(rig.heads[18].y < rig.heads[rig.root].y);
        assert!(rig.heads[22].y < rig.heads[rig.root].y);
    }

    #[test]
    fn generated_lib_34_terminal_hand_topology_classifies_if_present() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/ai_content_library/lib-34.glb");
        if !path.is_file() {
            eprintln!("generated lib-34 elf rig fixture absent; skipping");
            return;
        }
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let parsed = parse_glb_bytes(&bytes).unwrap();
        let nodes = parsed.document.nodes_slice();
        let rig = Rig::classify(nodes, parsed.document.skins.as_deref()).unwrap();

        assert_eq!(rig.joints.len(), 27);
        assert_eq!(rig.mapping.len(), 18);
        // The final leaf on each four-node arm is the hand endpoint. It must
        // inherit from its driven parent and must never be aligned using its
        // synthetic imported tail.
        assert!(!rig.mapping.contains_key(&11));
        assert!(!rig.mapping.contains_key(&15));
        assert_eq!(rig.mapping[&10].rig_child, Some(11));
        assert_eq!(rig.mapping[&14].rig_child, Some(15));
    }

    #[test]
    fn generated_lib_49_low_hand_and_split_ankle_topology_classifies_if_present() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/ai_content_library/lib-49.glb");
        if !path.is_file() {
            eprintln!("generated lib-49 elf rig fixture absent; skipping");
            return;
        }
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let parsed = parse_glb_bytes(&bytes).unwrap();
        let nodes = parsed.document.nodes_slice();
        assert_eq!(nodes[1].children.as_deref(), Some(&[2, 14, 20][..]));

        let rig = Rig::classify(nodes, parsed.document.skins.as_deref()).unwrap();
        assert_eq!(rig.root, 1);
        assert_eq!(rig.joints.len(), 25);
        assert_eq!(rig.mapping.len(), 18);
        assert_eq!(rig.mapping[&16].rig_child, Some(17));
        assert_eq!(rig.mapping[&18].rig_child, Some(19));
        assert_eq!(rig.mapping[&22].rig_child, Some(23));
        assert_eq!(rig.mapping[&24].rig_child, Some(25));
        assert!(!rig.mapping.contains_key(&17));
        assert!(!rig.mapping.contains_key(&19));
        assert!(!rig.mapping.contains_key(&23));
        assert!(!rig.mapping.contains_key(&25));
    }

    #[test]
    fn leg_chain_uses_forward_terminal_of_split_foot() {
        let children = vec![vec![1], vec![2], vec![3, 4], vec![], vec![]];
        let joint_set: HashSet<usize> = (0..children.len()).collect();
        let heads = vec![
            V3 { x: 0.08, y: 0.02, z: 0.0 },
            V3 { x: 0.08, y: -0.08, z: 0.0 },
            V3 { x: 0.08, y: -0.18, z: 0.0 },
            V3 { x: 0.09, y: -0.32, z: 0.01 },
            V3 { x: 0.09, y: -0.34, z: -0.09 },
        ];

        assert_eq!(
            humanoid_leg_chain(0, &children, &joint_set, &heads),
            vec![0, 1, 2, 4]
        );
    }

    #[test]
    fn terminal_foot_endpoint_is_direction_only_not_retargeted_twice() {
        let chain = vec![0, 1, 2, 3];
        let mut mapping = HashMap::new();

        assign_chain(
            &mut mapping,
            &chain,
            &[(L_HIP, L_KNEE), (L_KNEE, L_ANKLE), (L_ANKLE, L_FOOT)],
        );

        assert_eq!(mapping.len(), 3);
        assert_eq!(mapping[&2].rig_child, Some(3));
        assert!(
            !mapping.contains_key(&3),
            "the direction endpoint must inherit its parent's driven ankle"
        );
    }

    #[test]
    fn terminal_hand_endpoint_is_direction_only_not_retargeted_twice() {
        let children = vec![vec![1], vec![2], vec![3], vec![]];
        let joint_set: HashSet<usize> = (0..children.len()).collect();
        let chain = vec![0, 1, 2, 3];
        let mapped = mapped_arm_bones(&chain, &children, &joint_set);
        assert_eq!(mapped, 3);

        let mut mapping = HashMap::new();
        let pairs = [
            (L_COLLAR, L_SHOULDER),
            (L_SHOULDER, L_ELBOW),
            (L_ELBOW, L_WRIST),
            (L_WRIST, L_MIDDLE1),
        ];
        assign_chain(&mut mapping, &chain, &pairs[..mapped]);
        assert_eq!(mapping[&2].rig_child, Some(3));
        assert!(!mapping.contains_key(&3));
    }

    #[test]
    fn branched_wrist_remains_mapped_and_requires_real_branch() {
        let children = vec![vec![1], vec![2], vec![3], vec![4, 5], vec![], vec![]];
        let joint_set: HashSet<usize> = (0..children.len()).collect();
        assert_eq!(mapped_arm_bones(&[0, 1, 2, 3], &children, &joint_set), 4);
    }

    #[test]
    fn terminal_wrist_uses_longest_nontrivial_hand_branch() {
        let children = vec![vec![1], vec![2], vec![3], vec![4, 5], vec![], vec![]];
        let joint_set: HashSet<usize> = (0..children.len()).collect();
        let heads = vec![
            V3 { x: 0.0, y: 0.0, z: 0.0 },
            V3 { x: 0.1, y: 0.0, z: 0.0 },
            V3 { x: 0.2, y: 0.0, z: 0.0 },
            V3 { x: 0.3, y: 0.0, z: 0.0 },
            V3 { x: 0.33, y: 0.01, z: 0.0 },
            V3 { x: 0.38, y: 0.0, z: 0.0 },
        ];
        let chain = vec![0, 1, 2, 3];
        let mut mapping = HashMap::new();
        for (index, &joint) in chain.iter().enumerate() {
            mapping.insert(
                joint,
                Mapping {
                    from: index,
                    to: index + 1,
                    rig_child: chain.get(index + 1).copied(),
                },
            );
        }

        fill_terminal_branch_child(
            &mut mapping,
            &chain,
            4,
            &children,
            &joint_set,
            &heads,
        );

        assert_eq!(mapping[&3].rig_child, Some(5));
    }

    #[test]
    fn mapped_wrist_cannot_fall_back_to_synthetic_tail() {
        let heads = vec![
            V3 { x: 0.0, y: 0.0, z: 0.0 },
            V3 { x: 0.08, y: -0.01, z: 0.0 },
        ];
        let mut mapping = HashMap::from([(
            0,
            Mapping {
                from: L_WRIST,
                to: L_MIDDLE1,
                rig_child: None,
            },
        )]);
        let error = validate_mapped_hand_directions(&mapping, &heads).unwrap_err();
        assert!(error.to_string().contains("no real hand-direction child"));

        mapping.get_mut(&0).unwrap().rig_child = Some(1);
        validate_mapped_hand_directions(&mapping, &heads).unwrap();
    }

    #[test]
    fn playable_root_travel_is_fully_host_driven() {
        let initial = V3 { x: 1.0, y: 2.0, z: 3.0 };
        let current = V3 { x: 4.0, y: 7.0, z: 9.0 };
        let oracle_in_place = root_travel(current, initial, true, false);
        assert_eq!(oracle_in_place.x, 0.0);
        assert_eq!(oracle_in_place.y, 5.0);
        assert_eq!(oracle_in_place.z, 0.0);

        let playable = root_travel(current, initial, true, true);
        assert_eq!(playable.x, 0.0);
        assert_eq!(playable.y, 0.0);
        assert_eq!(playable.z, 0.0);
    }

}
