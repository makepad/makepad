//! Skinned character loading + animation (M1c, game.md).
//!
//! Parses a GLB (binary glTF v2) with skins and animation clips into a
//! [`SkinnedModel`], samples/blends clips into a [`PoseBuffer`], and computes
//! the joint palette. Rendering skins on the GPU: the rest mesh uploads once
//! ([`SkinnedModel::rest_gpu_packed`]) and the palette rides a texture
//! ([`palette_texels`]); the CPU path ([`SkinnedModel::skin_to_packed`])
//! remains for the shadow bake and for pinning the shader formula in tests.
//! Skinning is Derived-tier presentation: exact IEEE ops only
//! (lerp/nlerp/sqrt — no libm calls).
//!
//! Scope (KayKit/Blender-style exports): GLB container only, dense accessors,
//! JOINTS_0 u8/u16, WEIGHTS_0 f32/normalized u8/u16, TRS node transforms,
//! linear (or step) samplers. Unskinned primitives (hand props) are counted
//! and skipped. Materials are ignored — the caller binds its own texture.

use makepad_draw::makepad_math::{Mat4f, Quat, Vec3f};

// ------------------------------------------------------------------- JSON

/// Minimal owned JSON value — glTF headers are small (a few hundred KB max),
/// clarity beats speed here.
pub(crate) enum Val {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Val>),
    Obj(Vec<(String, Val)>),
}

impl Val {
    pub(crate) fn get(&self, key: &str) -> Option<&Val> {
        match self {
            Val::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub(crate) fn idx(&self, i: usize) -> Option<&Val> {
        match self {
            Val::Arr(items) => items.get(i),
            _ => None,
        }
    }
    pub(crate) fn arr(&self) -> &[Val] {
        match self {
            Val::Arr(items) => items,
            _ => &[],
        }
    }
    /// An object's fields in file order — for `extras`, whose keys are
    /// whatever the exporter chose to write.
    pub(crate) fn obj(&self) -> &[(String, Val)] {
        match self {
            Val::Obj(fields) => fields,
            _ => &[],
        }
    }
    pub(crate) fn f64(&self) -> Option<f64> {
        match self {
            Val::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub(crate) fn usize(&self) -> Option<usize> {
        self.f64().map(|n| n as usize)
    }
    pub(crate) fn str(&self) -> Option<&str> {
        match self {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }
}

pub(crate) struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    pub(crate) fn parse(bytes: &'a [u8]) -> Result<Val, String> {
        let mut p = JsonParser { bytes, pos: 0 };
        let v = p.value()?;
        Ok(v)
    }
    fn err(&self, what: &str) -> String {
        format!("gltf json: {what} at byte {}", self.pos)
    }
    fn peek(&self) -> u8 {
        *self.bytes.get(self.pos).unwrap_or(&0)
    }
    fn skip_ws(&mut self) {
        while matches!(self.peek(), b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
    }
    fn expect(&mut self, b: u8) -> Result<(), String> {
        self.skip_ws();
        if self.peek() != b {
            return Err(self.err(&format!("expected '{}'", b as char)));
        }
        self.pos += 1;
        Ok(())
    }
    fn value(&mut self) -> Result<Val, String> {
        self.skip_ws();
        match self.peek() {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => Ok(Val::Str(self.string()?)),
            b't' => self.lit("true", Val::Bool(true)),
            b'f' => self.lit("false", Val::Bool(false)),
            b'n' => self.lit("null", Val::Null),
            _ => self.number(),
        }
    }
    fn lit(&mut self, s: &str, v: Val) -> Result<Val, String> {
        if self.bytes[self.pos..].starts_with(s.as_bytes()) {
            self.pos += s.len();
            Ok(v)
        } else {
            Err(self.err("bad literal"))
        }
    }
    fn object(&mut self) -> Result<Val, String> {
        self.expect(b'{')?;
        let mut fields = Vec::new();
        self.skip_ws();
        if self.peek() == b'}' {
            self.pos += 1;
            return Ok(Val::Obj(fields));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.expect(b':')?;
            let value = self.value()?;
            fields.push((key, value));
            self.skip_ws();
            match self.peek() {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    return Ok(Val::Obj(fields));
                }
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
    }
    fn array(&mut self) -> Result<Val, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == b']' {
            self.pos += 1;
            return Ok(Val::Arr(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                b',' => self.pos += 1,
                b']' => {
                    self.pos += 1;
                    return Ok(Val::Arr(items));
                }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
    }
    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let b = self.peek();
            self.pos += 1;
            match b {
                0 => return Err(self.err("unterminated string")),
                b'"' => return Ok(out),
                b'\\' => {
                    let e = self.peek();
                    self.pos += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'u' => {
                            let hex = self
                                .bytes
                                .get(self.pos..self.pos + 4)
                                .ok_or_else(|| self.err("bad \\u"))?;
                            let code = u32::from_str_radix(
                                std::str::from_utf8(hex).map_err(|_| self.err("bad \\u"))?,
                                16,
                            )
                            .map_err(|_| self.err("bad \\u"))?;
                            self.pos += 4;
                            // Asset names are BMP-only; surrogate pairs are out of scope.
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                        _ => return Err(self.err("bad escape")),
                    }
                }
                _ => {
                    // Copy raw UTF-8 bytes through.
                    let start = self.pos - 1;
                    while self.peek() != b'"' && self.peek() != b'\\' && self.peek() != 0 {
                        self.pos += 1;
                    }
                    out.push_str(
                        std::str::from_utf8(&self.bytes[start..self.pos])
                            .map_err(|_| self.err("bad utf8"))?,
                    );
                }
            }
        }
    }
    fn number(&mut self) -> Result<Val, String> {
        let start = self.pos;
        while matches!(self.peek(), b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
            self.pos += 1;
        }
        let s = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.err("bad number"))?;
        s.parse::<f64>()
            .map(Val::Num)
            .map_err(|_| self.err("bad number"))
    }
}

// ------------------------------------------------------------------ model

#[derive(Clone, Copy)]
pub struct NodeTrs {
    pub t: Vec3f,
    pub r: Quat,
    pub s: Vec3f,
}

impl Default for NodeTrs {
    fn default() -> Self {
        Self {
            t: Vec3f::default(),
            r: Quat::default(),
            s: Vec3f { x: 1.0, y: 1.0, z: 1.0 },
        }
    }
}

/// One sampled skeleton pose: local TRS per glTF node.
pub type PoseBuffer = Vec<NodeTrs>;

/// Source-neutral collider attached to one skeleton node. Coordinates are in
/// that node's local bind frame and model units; a game scales them with the
/// same character instance scale used for the visible skin.
#[derive(Clone, Debug)]
pub enum RagdollCollider {
    Capsule { point_a: Vec3f, point_b: Vec3f, radius: f32 },
    Sphere { center: Vec3f, radius: f32 },
    Box { center: Vec3f, half_extents: Vec3f },
}

/// One generic articulated body parsed from `extras.kind="ragdoll_body"`.
/// `parent` indexes [`RagdollRig::bodies`], not glTF nodes.
#[derive(Clone, Debug)]
pub struct RagdollBody {
    pub connection: String,
    pub node: usize,
    pub parent: Option<usize>,
    pub collider: RagdollCollider,
    pub mass_fraction: f32,
    pub cone_angle: f32,
    pub twist_min: f32,
    pub twist_max: f32,
}

#[derive(Clone, Debug)]
pub struct RagdollRig {
    pub bodies: Vec<RagdollBody>,
}

/// Current world-space skeleton-node frame used to seed or draw an
/// articulation. The connection id is the only lookup handle exposed to the
/// game; source bone names never leave the importer.
#[derive(Clone, Debug)]
pub struct RagdollBodyPose {
    pub connection: String,
    pub transform: Mat4f,
}

struct Node {
    name: String,
    parent: Option<usize>,
    rest: NodeTrs,
}

#[derive(Clone, Copy, PartialEq)]
enum ChannelPath {
    Translation,
    Rotation,
    Scale,
}

struct Channel {
    node: usize,
    path: ChannelPath,
    times: Vec<f32>,
    /// 3 floats per key (T/S) or 4 (R).
    values: Vec<f32>,
}

impl Channel {
    fn apply_at(&self, t: f32, trs: &mut NodeTrs) {
        if self.times.is_empty() {
            return;
        }
        // Key pair straddling t + interpolation factor.
        let (k0, k1, f) = match self.times.iter().position(|kt| *kt > t) {
            Some(0) => (0, 0, 0.0),
            None => (self.times.len() - 1, self.times.len() - 1, 0.0),
            Some(k) => {
                let (t0, t1) = (self.times[k - 1], self.times[k]);
                let span = t1 - t0;
                (k - 1, k, if span > 0.0 { (t - t0) / span } else { 0.0 })
            }
        };
        match self.path {
            ChannelPath::Translation | ChannelPath::Scale => {
                let a = &self.values[k0 * 3..k0 * 3 + 3];
                let b = &self.values[k1 * 3..k1 * 3 + 3];
                let v = Vec3f {
                    x: a[0] + (b[0] - a[0]) * f,
                    y: a[1] + (b[1] - a[1]) * f,
                    z: a[2] + (b[2] - a[2]) * f,
                };
                if self.path == ChannelPath::Translation {
                    trs.t = v;
                } else {
                    trs.s = v;
                }
            }
            ChannelPath::Rotation => {
                let a = quat_at(&self.values, k0);
                let b = quat_at(&self.values, k1);
                trs.r = nlerp(a, b, f);
            }
        }
    }
}

pub struct AnimClip {
    pub name: String,
    pub duration: f32,
    channels: Vec<Channel>,
}

/// One skinned vertex, model space rest pose.
struct SkinVertex {
    pos: Vec3f,
    normal: Vec3f,
    uv: [f32; 2],
    joints: [u16; 4],
    weights: [f32; 4],
}

pub struct SkinnedModel {
    nodes: Vec<Node>,
    /// skin.joints — indices into `nodes`.
    joint_nodes: Vec<usize>,
    inverse_bind: Vec<Mat4f>,
    /// Node holding the skinned mesh (its inverse global premultiplies the palette).
    mesh_node: usize,
    vertices: Vec<SkinVertex>,
    indices: Vec<u32>,
    pub clips: Vec<AnimClip>,
    /// Primitives without JOINTS_0 (hand props etc.) — skipped, reported.
    pub skipped_unskinned: usize,
    /// Per joint: bind-pose position in mesh space + the largest distance to
    /// any vertex it influences. `posed_bounds` turns a palette into a
    /// conservative posed AABB from these — a joint's transform is rigid, so
    /// every vertex it moves stays within its rest radius of the joint, and a
    /// blended vertex stays inside the union of its joints' spheres.
    joint_bounds: Vec<(Vec3f, f32)>,
    ragdoll: Option<RagdollRig>,
}

/// Pre-render deformation audit over animation samples. This observes the
/// authored skin and topology without mutating either, so callers can reject
/// a bad generated character instead of hiding it by deleting triangles.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkinDeformationAudit {
    pub triangles: usize,
    pub samples: usize,
    pub over_2x: usize,
    pub over_3x: usize,
    pub p95_stretch: f32,
    pub p99_stretch: f32,
    pub max_stretch: f32,
}

/// Rest-pose topology audit for an automatically generated humanoid rig.
///
/// Callers provide hierarchy-classified arm and leg node sets. A bridge is a
/// non-degenerate triangle with at least one vertex confidently controlled by
/// an arm branch and another confidently controlled by a leg branch. The
/// audit is deliberately observational: it never edits weights or topology.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkinSemanticBridgeAudit {
    pub triangles: usize,
    pub nondegenerate_triangles: usize,
    pub bridge_triangles: usize,
    pub bridge_rest_area: f32,
    pub bridge_rest_area_fraction: f32,
    pub first_bridge_face: Option<usize>,
    pub max_arm_confidence: f32,
    pub max_leg_confidence: f32,
}

/// Fail-closed CPU deformation audit over every authored animation key.
///
/// A triangle is visibly bad only when both its diameter grows beyond 3x and
/// its absolute extension exceeds 2% of the rest mesh's vertical extent. The
/// conjunction keeps harmless high ratios on microscopic triangles from
/// failing while still catching long hand/hip or leg/leg webbing. Bad-face
/// area is a union over topology, not multiplied by the number of samples.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkinMotionQualityAudit {
    pub triangles: usize,
    pub clips: usize,
    pub authored_samples: usize,
    pub bad_triangles: usize,
    pub bad_rest_area: f32,
    pub bad_rest_area_fraction: f32,
    pub rest_bbox_height: f32,
    pub max_stretch: f32,
    pub max_extension_height: f32,
    pub worst_clip: Option<usize>,
    pub worst_authored_frame: Option<usize>,
    pub worst_face: Option<usize>,
    pub worst_time_seconds: f32,
}

/// Consecutive authored-frame motion audit for one clip. Joint rotation is
/// local angular travel; vertex travel is measured after CPU skinning in
/// model space. `seam_*` compares the last authored frame to the first and
/// is reported separately because one-shot clips need not loop.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkinTemporalAudit {
    pub frames: usize,
    pub frame_pairs: usize,
    pub max_joint_angle_degrees: f32,
    pub max_joint_node: usize,
    pub max_joint_frame: usize,
    pub p99_joint_angle_degrees: f32,
    pub max_vertex_delta: f32,
    pub max_vertex: usize,
    pub max_vertex_frame: usize,
    pub p99_vertex_delta: f32,
    pub seam_joint_angle_degrees: f32,
    pub seam_vertex_delta: f32,
}

/// Runtime continuity audit for a looping clip sampled with
/// [`SkinnedModel::sample_clip_loop_blended`]. Unlike
/// [`SkinTemporalAudit`], this follows the exact clock path used by a game
/// for multiple cycles and includes every wrap in the consecutive-pair
/// maxima. `wrap_*` isolates pairs that cross a cycle boundary so a bad raw
/// tail-to-head seam cannot hide behind otherwise-smooth authored frames.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkinLoopTemporalAudit {
    pub cycles: usize,
    pub frames: usize,
    pub frame_pairs: usize,
    pub wraps: usize,
    pub max_joint_angle_degrees: f32,
    pub max_joint_node: usize,
    pub max_joint_frame: usize,
    pub max_vertex_delta: f32,
    pub max_vertex: usize,
    pub max_vertex_frame: usize,
    pub wrap_joint_angle_degrees: f32,
    pub wrap_vertex_delta: f32,
}

/// One node's largest local-rotation jump between consecutive authored
/// frames. Useful for locating an outlier reported by [`SkinTemporalAudit`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkinJointTemporalOutlier {
    pub node: usize,
    pub frame: usize,
    pub angle_degrees: f32,
}

pub(crate) fn trs_to_mat4(trs: &NodeTrs) -> Mat4f {
    let q = trs.r;
    let (x2, y2, z2) = (q.x + q.x, q.y + q.y, q.z + q.z);
    let (xx, yy, zz) = (q.x * x2, q.y * y2, q.z * z2);
    let (xy, xz, yz) = (q.x * y2, q.x * z2, q.y * z2);
    let (wx, wy, wz) = (q.w * x2, q.w * y2, q.w * z2);
    let (sx, sy, sz) = (trs.s.x, trs.s.y, trs.s.z);
    // Column-major, columns scaled, translation in v[12..15].
    Mat4f {
        v: [
            (1.0 - (yy + zz)) * sx,
            (xy + wz) * sx,
            (xz - wy) * sx,
            0.0,
            (xy - wz) * sy,
            (1.0 - (xx + zz)) * sy,
            (yz + wx) * sy,
            0.0,
            (xz + wy) * sz,
            (yz - wx) * sz,
            (1.0 - (xx + yy)) * sz,
            0.0,
            trs.t.x,
            trs.t.y,
            trs.t.z,
            1.0,
        ],
    }
}

pub(crate) fn mat4_mul_point(m: &Mat4f, p: Vec3f) -> Vec3f {
    Vec3f {
        x: m.v[0] * p.x + m.v[4] * p.y + m.v[8] * p.z + m.v[12],
        y: m.v[1] * p.x + m.v[5] * p.y + m.v[9] * p.z + m.v[13],
        z: m.v[2] * p.x + m.v[6] * p.y + m.v[10] * p.z + m.v[14],
    }
}

pub(crate) fn mat4_mul_dir(m: &Mat4f, p: Vec3f) -> Vec3f {
    Vec3f {
        x: m.v[0] * p.x + m.v[4] * p.y + m.v[8] * p.z,
        y: m.v[1] * p.x + m.v[5] * p.y + m.v[9] * p.z,
        z: m.v[2] * p.x + m.v[6] * p.y + m.v[10] * p.z,
    }
}

// -------------------------------------------------------------- accessors

pub(crate) struct Accessors<'a> {
    pub(crate) json: &'a Val,
    pub(crate) bin: &'a [u8],
}

impl<'a> Accessors<'a> {
    fn component_size(ct: usize) -> usize {
        match ct {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => 0,
        }
    }
    fn type_lanes(ty: &str) -> usize {
        match ty {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            "MAT4" => 16,
            _ => 0,
        }
    }

    /// Read accessor `index` as f32 lanes (integers are normalized when the
    /// accessor says so, else converted — the glTF rules for weights/joints).
    pub(crate) fn read_f32(&self, index: usize) -> Result<(Vec<f32>, usize), String> {
        let acc = self
            .json
            .get("accessors")
            .and_then(|a| a.idx(index))
            .ok_or(format!("missing accessor {index}"))?;
        let ct = acc.get("componentType").and_then(Val::usize).unwrap_or(0);
        let ty = acc.get("type").and_then(Val::str).unwrap_or("");
        let count = acc.get("count").and_then(Val::usize).unwrap_or(0);
        let normalized = matches!(acc.get("normalized"), Some(Val::Bool(true)));
        let lanes = Self::type_lanes(ty);
        let csize = Self::component_size(ct);
        if lanes == 0 || csize == 0 {
            return Err(format!("accessor {index}: unsupported {ty}/{ct}"));
        }
        let view_index = acc
            .get("bufferView")
            .and_then(Val::usize)
            .ok_or(format!("accessor {index}: sparse/no view unsupported"))?;
        let view = self
            .json
            .get("bufferViews")
            .and_then(|v| v.idx(view_index))
            .ok_or(format!("missing bufferView {view_index}"))?;
        let view_off = view.get("byteOffset").and_then(Val::usize).unwrap_or(0);
        let stride = view
            .get("byteStride")
            .and_then(Val::usize)
            .unwrap_or(lanes * csize);
        let acc_off = acc.get("byteOffset").and_then(Val::usize).unwrap_or(0);
        let base = view_off + acc_off;
        let mut out = Vec::with_capacity(count * lanes);
        for i in 0..count {
            let elem = base + i * stride;
            for lane in 0..lanes {
                let at = elem + lane * csize;
                let bytes = self
                    .bin
                    .get(at..at + csize)
                    .ok_or(format!("accessor {index}: out of range"))?;
                let v = match ct {
                    5126 => f32::from_le_bytes(bytes.try_into().unwrap()),
                    5121 => {
                        let u = bytes[0] as f32;
                        if normalized {
                            u / 255.0
                        } else {
                            u
                        }
                    }
                    5123 => {
                        let u = u16::from_le_bytes(bytes.try_into().unwrap()) as f32;
                        if normalized {
                            u / 65535.0
                        } else {
                            u
                        }
                    }
                    5125 => u32::from_le_bytes(bytes.try_into().unwrap()) as f32,
                    5120 => bytes[0] as i8 as f32,
                    5122 => i16::from_le_bytes(bytes.try_into().unwrap()) as f32,
                    _ => unreachable!(),
                };
                out.push(v);
            }
        }
        Ok((out, lanes))
    }
}

// ------------------------------------------------------------------ parse

fn ragdoll_vec3(value: Option<&Val>) -> Option<Vec3f> {
    let value = value?;
    let out = Vec3f {
        x: value.idx(0).and_then(Val::f64)? as f32,
        y: value.idx(1).and_then(Val::f64)? as f32,
        z: value.idx(2).and_then(Val::f64)? as f32,
    };
    out.x.is_finite().then_some(())?;
    out.y.is_finite().then_some(())?;
    out.z.is_finite().then_some(())?;
    Some(out)
}

fn parse_ragdoll(
    json: &Val,
    node_vals: &[Val],
    nodes: &[Node],
    joint_nodes: &[usize],
) -> Result<Option<RagdollRig>, String> {
    struct Raw {
        connection: String,
        parent_connection: Option<String>,
        root: bool,
        node: usize,
        collider: RagdollCollider,
        mass_fraction: f32,
        cone_angle: f32,
        twist_min: f32,
        twist_max: f32,
    }

    let mut raw = Vec::new();
    for (node, value) in node_vals.iter().enumerate() {
        let Some(extras) = value.get("extras") else { continue };
        if extras.get("kind").and_then(Val::str) != Some("ragdoll_body") {
            continue;
        }
        if !joint_nodes.contains(&node) {
            return Err(format!("ragdoll node {node} is not in the skin"));
        }
        let connection = extras
            .get("connection")
            .and_then(Val::str)
            .filter(|value| !value.is_empty())
            .ok_or("ragdoll body missing connection")?
            .to_string();
        if raw.iter().any(|body: &Raw| body.connection == connection) {
            return Err(format!("duplicate ragdoll connection {connection}"));
        }
        let root = matches!(extras.get("root"), Some(Val::Bool(true)));
        let parent_connection = extras
            .get("parent_connection")
            .and_then(Val::str)
            .map(str::to_string);
        let positive = |name: &str| -> Result<f32, String> {
            let value = extras.get(name).and_then(Val::f64).unwrap_or(0.0) as f32;
            if value.is_finite() && value > 0.0 {
                Ok(value)
            } else {
                Err(format!("ragdoll {connection} has invalid {name}"))
            }
        };
        let collider = match extras.get("shape").and_then(Val::str) {
            Some("capsule") => RagdollCollider::Capsule {
                point_a: ragdoll_vec3(extras.get("point_a"))
                    .ok_or_else(|| format!("ragdoll {connection} has invalid point_a"))?,
                point_b: ragdoll_vec3(extras.get("point_b"))
                    .ok_or_else(|| format!("ragdoll {connection} has invalid point_b"))?,
                radius: positive("radius")?,
            },
            Some("sphere") => RagdollCollider::Sphere {
                center: ragdoll_vec3(extras.get("position"))
                    .ok_or_else(|| format!("ragdoll {connection} has invalid position"))?,
                radius: positive("radius")?,
            },
            Some("box") => RagdollCollider::Box {
                center: ragdoll_vec3(extras.get("position"))
                    .ok_or_else(|| format!("ragdoll {connection} has invalid position"))?,
                half_extents: {
                    let half = ragdoll_vec3(extras.get("half_extents"))
                        .ok_or_else(|| format!("ragdoll {connection} has invalid half_extents"))?;
                    if half.x <= 0.0 || half.y <= 0.0 || half.z <= 0.0 {
                        return Err(format!("ragdoll {connection} has non-positive half_extents"));
                    }
                    half
                },
            },
            Some(other) => return Err(format!("ragdoll {connection} has unknown shape {other}")),
            None => return Err(format!("ragdoll {connection} is missing shape")),
        };
        let mass_fraction = positive("mass_fraction")?;
        let (cone_angle, twist_min, twist_max) = if root {
            (0.0, 0.0, 0.0)
        } else {
            let cone = positive("cone_angle")?;
            let lower = extras.get("twist_min").and_then(Val::f64).unwrap_or(f64::NAN) as f32;
            let upper = extras.get("twist_max").and_then(Val::f64).unwrap_or(f64::NAN) as f32;
            if !cone.is_finite()
                || cone > std::f32::consts::PI
                || !lower.is_finite()
                || !upper.is_finite()
                || lower > upper
            {
                return Err(format!("ragdoll {connection} has invalid joint limits"));
            }
            (cone, lower, upper)
        };
        raw.push(Raw {
            connection,
            parent_connection,
            root,
            node,
            collider,
            mass_fraction,
            cone_angle,
            twist_min,
            twist_max,
        });
    }
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.iter().filter(|body| body.root).count() != 1 {
        return Err("ragdoll rig must have exactly one root".into());
    }

    // Every skin in a multi-mesh character must use the same joint set. A
    // body rig bound to the torso skin but not the head skin is a partial rig,
    // not something runtime can safely guess around.
    for skin in json.get("skins").map(Val::arr).unwrap_or(&[]) {
        let joints: Vec<usize> = skin
            .get("joints")
            .map(|value| value.arr().iter().filter_map(Val::usize).collect())
            .unwrap_or_default();
        if joints != joint_nodes {
            return Err("ragdoll character skins use incompatible joint sets".into());
        }
    }

    let mut bodies = Vec::with_capacity(raw.len());
    for body in &raw {
        if body.root != body.parent_connection.is_none() {
            return Err(format!(
                "ragdoll {} root/parent declaration is inconsistent",
                body.connection
            ));
        }
        let parent = body
            .parent_connection
            .as_deref()
            .map(|parent| {
                raw.iter()
                    .position(|candidate| candidate.connection == parent)
                    .ok_or_else(|| format!("ragdoll {} has unresolved parent {parent}", body.connection))
            })
            .transpose()?;

        // The declared graph must match the skeleton graph after skipping
        // ordinary socket/group nodes. This makes it impossible for metadata
        // to attach an arm constraint to the wrong physical body.
        let mut ancestor = nodes[body.node].parent;
        let mut actual_parent = None;
        for _ in 0..nodes.len() {
            let Some(node) = ancestor else { break };
            if let Some(index) = raw.iter().position(|candidate| candidate.node == node) {
                actual_parent = Some(index);
                break;
            }
            ancestor = nodes[node].parent;
        }
        if actual_parent != parent {
            return Err(format!("ragdoll {} parent does not match skeleton", body.connection));
        }
        bodies.push(RagdollBody {
            connection: body.connection.clone(),
            node: body.node,
            parent,
            collider: body.collider.clone(),
            mass_fraction: body.mass_fraction,
            cone_angle: body.cone_angle,
            twist_min: body.twist_min,
            twist_max: body.twist_max,
        });
    }
    // Parent chains must terminate at the one root rather than cycle.
    for start in 0..bodies.len() {
        let mut at = Some(start);
        for depth in 0..=bodies.len() {
            let Some(index) = at else { break };
            if depth == bodies.len() {
                return Err("ragdoll parent graph contains a cycle".into());
            }
            at = bodies[index].parent;
        }
    }
    Ok(Some(RagdollRig { bodies }))
}

impl SkinnedModel {
    pub fn parse_glb(bytes: &[u8]) -> Result<SkinnedModel, String> {
        if bytes.len() < 12 || &bytes[0..4] != b"glTF" {
            return Err("not a GLB (magic mismatch)".into());
        }
        let mut json_chunk: Option<&[u8]> = None;
        let mut bin_chunk: &[u8] = &[];
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind = &bytes[at + 4..at + 8];
            let data = bytes
                .get(at + 8..at + 8 + len)
                .ok_or("GLB chunk out of range")?;
            match kind {
                b"JSON" => json_chunk = Some(data),
                b"BIN\0" => bin_chunk = data,
                _ => {}
            }
            at += 8 + len + (4 - len % 4) % 4;
        }
        let json = JsonParser::parse(json_chunk.ok_or("GLB has no JSON chunk")?)?;
        let acc = Accessors {
            json: &json,
            bin: bin_chunk,
        };

        // Nodes: rest TRS now, parents in a second pass over children lists.
        let node_vals = json.get("nodes").map(|n| n.arr()).unwrap_or(&[]);
        let mut nodes: Vec<Node> = node_vals
            .iter()
            .map(|n| {
                let mut rest = NodeTrs::default();
                if let Some(t) = n.get("translation") {
                    rest.t = Vec3f {
                        x: t.idx(0).and_then(Val::f64).unwrap_or(0.0) as f32,
                        y: t.idx(1).and_then(Val::f64).unwrap_or(0.0) as f32,
                        z: t.idx(2).and_then(Val::f64).unwrap_or(0.0) as f32,
                    };
                }
                if let Some(r) = n.get("rotation") {
                    rest.r = Quat {
                        x: r.idx(0).and_then(Val::f64).unwrap_or(0.0) as f32,
                        y: r.idx(1).and_then(Val::f64).unwrap_or(0.0) as f32,
                        z: r.idx(2).and_then(Val::f64).unwrap_or(0.0) as f32,
                        w: r.idx(3).and_then(Val::f64).unwrap_or(1.0) as f32,
                    };
                }
                if let Some(s) = n.get("scale") {
                    rest.s = Vec3f {
                        x: s.idx(0).and_then(Val::f64).unwrap_or(1.0) as f32,
                        y: s.idx(1).and_then(Val::f64).unwrap_or(1.0) as f32,
                        z: s.idx(2).and_then(Val::f64).unwrap_or(1.0) as f32,
                    };
                }
                Node {
                    name: n.get("name").and_then(Val::str).unwrap_or("").to_string(),
                    parent: None,
                    rest,
                }
            })
            .collect();
        for (parent_index, n) in node_vals.iter().enumerate() {
            if let Some(children) = n.get("children") {
                for c in children.arr() {
                    if let Some(ci) = c.usize() {
                        if ci < nodes.len() {
                            nodes[ci].parent = Some(parent_index);
                        }
                    }
                }
            }
        }

        // Skin 0: joints + inverse bind matrices.
        let skin = json
            .get("skins")
            .and_then(|s| s.idx(0))
            .ok_or("no skins in GLB")?;
        let joint_nodes: Vec<usize> = skin
            .get("joints")
            .map(|j| j.arr().iter().filter_map(Val::usize).collect())
            .unwrap_or_default();
        if joint_nodes.is_empty() {
            return Err("skin has no joints".into());
        }
        let inverse_bind = match skin.get("inverseBindMatrices").and_then(Val::usize) {
            Some(ibm_acc) => {
                let (floats, _) = acc.read_f32(ibm_acc)?;
                floats
                    .chunks_exact(16)
                    .map(|c| Mat4f {
                        v: c.try_into().unwrap(),
                    })
                    .collect()
            }
            None => vec![Mat4f::identity(); joint_nodes.len()],
        };
        let ragdoll = parse_ragdoll(&json, node_vals, &nodes, &joint_nodes)?;

        // All skinned mesh primitives, concatenated (they share skin 0).
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut skipped_unskinned = 0;
        let mut mesh_node = 0;
        for (node_index, n) in node_vals.iter().enumerate() {
            let (Some(mesh_index), Some(_)) =
                (n.get("mesh").and_then(Val::usize), n.get("skin"))
            else {
                continue;
            };
            mesh_node = node_index;
            let mesh = json
                .get("meshes")
                .and_then(|m| m.idx(mesh_index))
                .ok_or("bad mesh index")?;
            for prim in mesh.get("primitives").map(|p| p.arr()).unwrap_or(&[]) {
                let attrs = prim.get("attributes").ok_or("primitive without attributes")?;
                let Some(joints_acc) = attrs.get("JOINTS_0").and_then(Val::usize) else {
                    skipped_unskinned += 1;
                    continue;
                };
                let pos_acc = attrs
                    .get("POSITION")
                    .and_then(Val::usize)
                    .ok_or("primitive without POSITION")?;
                let (pos, _) = acc.read_f32(pos_acc)?;
                let normal = attrs
                    .get("NORMAL")
                    .and_then(Val::usize)
                    .map(|a| acc.read_f32(a))
                    .transpose()?
                    .map(|(v, _)| v);
                let uv = attrs
                    .get("TEXCOORD_0")
                    .and_then(Val::usize)
                    .map(|a| acc.read_f32(a))
                    .transpose()?
                    .map(|(v, _)| v);
                let (joints, _) = acc.read_f32(joints_acc)?;
                let weights = attrs
                    .get("WEIGHTS_0")
                    .and_then(Val::usize)
                    .map(|a| acc.read_f32(a))
                    .transpose()?
                    .map(|(v, _)| v);
                let base = vertices.len() as u32;
                let count = pos.len() / 3;
                for i in 0..count {
                    let g = |src: &Option<Vec<f32>>, lanes: usize, lane: usize, dflt: f32| {
                        src.as_ref()
                            .and_then(|v| v.get(i * lanes + lane).copied())
                            .unwrap_or(dflt)
                    };
                    let mut w = [
                        g(&weights, 4, 0, 1.0),
                        g(&weights, 4, 1, 0.0),
                        g(&weights, 4, 2, 0.0),
                        g(&weights, 4, 3, 0.0),
                    ];
                    let total = w[0] + w[1] + w[2] + w[3];
                    if total > 0.0 {
                        for wv in w.iter_mut() {
                            *wv /= total;
                        }
                    }
                    vertices.push(SkinVertex {
                        pos: Vec3f {
                            x: pos[i * 3],
                            y: pos[i * 3 + 1],
                            z: pos[i * 3 + 2],
                        },
                        normal: Vec3f {
                            x: g(&normal, 3, 0, 0.0),
                            y: g(&normal, 3, 1, 1.0),
                            z: g(&normal, 3, 2, 0.0),
                        },
                        uv: [g(&uv, 2, 0, 0.0), g(&uv, 2, 1, 0.0)],
                        joints: [
                            joints[i * 4] as u16,
                            joints[i * 4 + 1] as u16,
                            joints[i * 4 + 2] as u16,
                            joints[i * 4 + 3] as u16,
                        ],
                        weights: w,
                    });
                }
                if let Some(idx_acc) = prim.get("indices").and_then(Val::usize) {
                    let (idx, _) = acc.read_f32(idx_acc)?;
                    indices.extend(idx.iter().map(|v| base + *v as u32));
                } else {
                    indices.extend((0..count as u32).map(|i| base + i));
                }
            }
        }
        if vertices.is_empty() {
            return Err("no skinned primitives found".into());
        }

        // Animations.
        let mut clips = Vec::new();
        for (clip_index, a) in json
            .get("animations")
            .map(|a| a.arr())
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            let name = a
                .get("name")
                .and_then(Val::str)
                .map(str::to_string)
                .unwrap_or(format!("clip_{clip_index}"));
            let samplers = a.get("samplers").map(|s| s.arr()).unwrap_or(&[]);
            let mut channels = Vec::new();
            let mut duration = 0.0f32;
            for ch in a.get("channels").map(|c| c.arr()).unwrap_or(&[]) {
                let Some(sampler) = ch.get("sampler").and_then(Val::usize) else {
                    continue;
                };
                let Some(target) = ch.get("target") else { continue };
                let Some(node) = target.get("node").and_then(Val::usize) else {
                    continue;
                };
                let path = match target.get("path").and_then(Val::str) {
                    Some("translation") => ChannelPath::Translation,
                    Some("rotation") => ChannelPath::Rotation,
                    Some("scale") => ChannelPath::Scale,
                    _ => continue, // weights (morph targets) unsupported
                };
                let Some(s) = samplers.get(sampler) else { continue };
                let Some(input) = s.get("input").and_then(Val::usize) else {
                    continue;
                };
                let Some(output) = s.get("output").and_then(Val::usize) else {
                    continue;
                };
                let (times, _) = acc.read_f32(input)?;
                let (values, _) = acc.read_f32(output)?;
                if let Some(last) = times.last() {
                    duration = duration.max(*last);
                }
                channels.push(Channel {
                    node,
                    path,
                    times,
                    values,
                });
            }
            clips.push(AnimClip {
                name,
                duration: duration.max(1.0e-4),
                channels,
            });
        }

        // Bind position of each joint in mesh space (the point its IBM maps
        // to the joint origin) + the farthest vertex it influences.
        let mut joint_bounds: Vec<(Vec3f, f32)> = inverse_bind
            .iter()
            .map(|ibm| {
                let inv = ibm.invert();
                (Vec3f { x: inv.v[12], y: inv.v[13], z: inv.v[14] }, 0.0f32)
            })
            .collect();
        for v in &vertices {
            for k in 0..4 {
                if v.weights[k] == 0.0 {
                    continue;
                }
                let Some(jb) = joint_bounds.get_mut(v.joints[k] as usize) else {
                    continue;
                };
                let (dx, dy, dz) = (v.pos.x - jb.0.x, v.pos.y - jb.0.y, v.pos.z - jb.0.z);
                jb.1 = jb.1.max((dx * dx + dy * dy + dz * dz).sqrt());
            }
        }

        Ok(SkinnedModel {
            nodes,
            joint_nodes,
            inverse_bind,
            mesh_node,
            vertices,
            indices,
            clips,
            skipped_unskinned,
            joint_bounds,
            ragdoll,
        })
    }

    pub fn joint_count(&self) -> usize {
        self.joint_nodes.len()
    }

    pub fn ragdoll_rig(&self) -> Option<&RagdollRig> {
        self.ragdoll.as_ref()
    }

    /// Bounds of vertices predominantly controlled by `node`, expressed in
    /// that skeleton node's local bind frame. Asset importers use this generic
    /// measurement to fit colliders; no vendor naming lives here.
    pub fn dominant_joint_local_bounds(&self, node: usize) -> Option<(Vec3f, Vec3f)> {
        let joint = self.joint_nodes.iter().position(|candidate| *candidate == node)? as u16;
        let rest = self.rest_pose();
        let bone_in_mesh = self.node_mesh_transform(&rest, node)?;
        let mesh_in_bone = bone_in_mesh.invert();
        let mut min = Vec3f { x: f32::MAX, y: f32::MAX, z: f32::MAX };
        let mut max = Vec3f { x: f32::MIN, y: f32::MIN, z: f32::MIN };
        let mut count = 0usize;
        for vertex in &self.vertices {
            let mut dominant = 0usize;
            for lane in 1..4 {
                if vertex.weights[lane] > vertex.weights[dominant] {
                    dominant = lane;
                }
            }
            if vertex.joints[dominant] != joint || vertex.weights[dominant] <= 0.0 {
                continue;
            }
            let point = mat4_mul_point(&mesh_in_bone, vertex.pos);
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            min.z = min.z.min(point.z);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
            max.z = max.z.max(point.z);
            count += 1;
        }
        (count >= 3).then_some((min, max))
    }

    /// World-space node frames for seeding the physics articulation from the
    /// exact animation pose visible in the previous frame.
    pub fn ragdoll_body_poses(
        &self,
        pose: &PoseBuffer,
        model_world: &Mat4f,
    ) -> Vec<RagdollBodyPose> {
        let Some(rig) = &self.ragdoll else { return Vec::new() };
        rig.bodies
            .iter()
            .filter_map(|body| {
                self.node_mesh_transform(pose, body.node).map(|node| RagdollBodyPose {
                    connection: body.connection.clone(),
                    transform: Mat4f::mul(model_world, &node),
                })
            })
            .collect()
    }

    /// Replace ordinary animation palette entries with authoritative
    /// world-space ragdoll body frames. The renderer still consumes the same
    /// palette format; it never needs a second skinned draw path.
    pub fn palette_from_ragdoll(
        &self,
        base_pose: &PoseBuffer,
        model_world: &Mat4f,
        bodies: &[RagdollBodyPose],
        out: &mut Vec<Mat4f>,
    ) {
        self.palette(base_pose, out);
        let Some(rig) = &self.ragdoll else { return };
        let model_from_world = model_world.invert();
        for body in &rig.bodies {
            let Some(pose) = bodies.iter().find(|pose| pose.connection == body.connection) else {
                continue;
            };
            let Some(joint) = self.joint_nodes.iter().position(|node| *node == body.node) else {
                continue;
            };
            let bone_in_mesh = Mat4f::mul(&model_from_world, &pose.transform);
            out[joint] = Mat4f::mul(&bone_in_mesh, &self.inverse_bind[joint]);
        }
    }

    /// Resolve an animated glTF node by its authored name.
    ///
    /// Node names are presentation metadata rather than skin-joint indices:
    /// attachment sockets may be ordinary nodes, and a joint palette cannot
    /// be used here because its matrices also contain inverse-bind transforms.
    pub fn node_index(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|node| node.name == name)
    }

    /// Authored node name, for diagnostics that report a node index.
    pub fn node_name(&self, node: usize) -> Option<&str> {
        self.nodes.get(node).map(|node| node.name.as_str())
    }

    /// Mask containing `root` and every node parented below it.
    ///
    /// Cache this alongside a resolved animation clip when applying a
    /// per-frame upper-body overlay. An invalid root returns `None` rather
    /// than an all-false mask, so a miss cannot silently select stale data.
    pub fn descendant_mask(&self, root: usize) -> Option<Vec<bool>> {
        if root >= self.nodes.len() {
            return None;
        }
        let mut mask = vec![false; self.nodes.len()];
        for node in 0..self.nodes.len() {
            let mut ancestor = Some(node);
            // glTF nodes form an acyclic forest. The bound also makes this
            // fail safe if malformed input ever slips through the parser.
            for _ in 0..self.nodes.len() {
                let Some(index) = ancestor else { break };
                if index == root {
                    mask[node] = true;
                    break;
                }
                ancestor = self.nodes[index].parent;
            }
        }
        Some(mask)
    }

    /// Current transform of `node` in the skinned mesh's model space.
    ///
    /// Multiplying the character's world transform by this result places a
    /// held prop at the animated node. Unlike [`Self::palette`], this is the
    /// raw pose transform and deliberately does not include an inverse bind.
    pub fn node_mesh_transform(&self, pose: &PoseBuffer, node: usize) -> Option<Mat4f> {
        if node >= self.nodes.len() {
            return None;
        }
        fn global(
            nodes: &[Node],
            pose: &PoseBuffer,
            globals: &mut [Option<Mat4f>],
            index: usize,
        ) -> Mat4f {
            if let Some(matrix) = globals[index] {
                return matrix;
            }
            let local = trs_to_mat4(pose.get(index).unwrap_or(&nodes[index].rest));
            let matrix = match nodes[index].parent {
                Some(parent) => Mat4f::mul(&global(nodes, pose, globals, parent), &local),
                None => local,
            };
            globals[index] = Some(matrix);
            matrix
        }
        let mut globals: Vec<Option<Mat4f>> = vec![None; self.nodes.len()];
        let mesh_global = global(&self.nodes, pose, &mut globals, self.mesh_node);
        let socket_global = global(&self.nodes, pose, &mut globals, node);
        Some(Mat4f::mul(&mesh_global.invert(), &socket_global))
    }

    /// Webbing cull for AUTO-rigged meshes: drop triangles whose edges
    /// stretch more than `max_stretch`× their rest length in any sampled
    /// pose of the given clips.
    ///
    /// Why: mesh reconstruction + auto-skinning leaves thin bridges whose
    /// endpoints are dominated by DIFFERENT limbs (crotch/shin webbing on
    /// the campaign robot). No weight edit fixes topology — the bridge
    /// spans the legs, so it shears into glitch bars the moment the legs
    /// separate. Legitimate deformation (knees, elbows) stays well under
    /// 2× edge stretch; webbing across separating limbs hits 3-10×. Each
    /// clip is sampled at `samples` evenly spaced phases. Returns culled
    /// triangle count. Call BEFORE `rest_gpu`/`rest_gpu_flat` (they
    /// snapshot the index buffer).
    pub fn cull_stretched_triangles(
        &mut self,
        clips: &[usize],
        samples: usize,
        max_stretch: f32,
    ) -> usize {
        let rest: Vec<Vec3f> = self.vertices.iter().map(|v| v.pos).collect();
        // Max posed edge length per triangle, over all sampled poses.
        let mut worst: Vec<f32> = vec![0.0; self.indices.len() / 3];
        let mut pose = PoseBuffer::new();
        let mut palette: Vec<Mat4f> = Vec::new();
        let mut posed: Vec<Vec3f> = Vec::new();
        for &clip in clips {
            let Some(duration) = self.clips.get(clip).map(|c| c.duration) else {
                continue;
            };
            for s in 0..samples.max(1) {
                let t = s as f32 / samples.max(1) as f32 * duration;
                self.sample_clip(clip, t, &mut pose);
                self.palette(&pose, &mut palette);
                posed.clear();
                posed.reserve(self.vertices.len());
                for v in &self.vertices {
                    let mut p = Vec3f::default();
                    for k in 0..4 {
                        let w = v.weights[k];
                        if w == 0.0 {
                            continue;
                        }
                        let Some(m) = palette.get(v.joints[k] as usize) else {
                            continue;
                        };
                        let q = mat4_mul_point(m, v.pos);
                        p.x += q.x * w;
                        p.y += q.y * w;
                        p.z += q.z * w;
                    }
                    posed.push(p);
                }
                let dist = |a: &Vec3f, b: &Vec3f| {
                    let (dx, dy, dz) = (a.x - b.x, a.y - b.y, a.z - b.z);
                    (dx * dx + dy * dy + dz * dz).sqrt()
                };
                for (slot, tri) in self.indices.chunks_exact(3).enumerate() {
                    let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
                    let m = dist(&posed[a], &posed[b])
                        .max(dist(&posed[b], &posed[c]))
                        .max(dist(&posed[c], &posed[a]));
                    if m > worst[slot] {
                        worst[slot] = m;
                    }
                }
            }
        }
        let dist = |a: &Vec3f, b: &Vec3f| {
            let (dx, dy, dz) = (a.x - b.x, a.y - b.y, a.z - b.z);
            (dx * dx + dy * dy + dz * dz).sqrt()
        };
        let mut kept = Vec::with_capacity(self.indices.len());
        let mut culled = 0;
        for (slot, tri) in self.indices.chunks_exact(3).enumerate() {
            let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let rest_edge = dist(&rest[a], &rest[b])
                .max(dist(&rest[b], &rest[c]))
                .max(dist(&rest[c], &rest[a]))
                .max(1.0e-6);
            if worst[slot] > max_stretch * rest_edge {
                culled += 1;
            } else {
                kept.extend_from_slice(tri);
            }
        }
        self.indices = kept;
        culled
    }

    /// Measure worst triangle-edge stretch across evenly spaced clip phases.
    ///
    /// This is the non-destructive counterpart to
    /// [`Self::cull_stretched_triangles`]. Each triangle contributes its
    /// worst sampled ratio; percentiles and counts make the result suitable
    /// for a generation quality gate and a human-readable audit report.
    pub fn audit_deformation(
        &self,
        clips: &[usize],
        samples_per_clip: usize,
    ) -> SkinDeformationAudit {
        let triangles = self.indices.len() / 3;
        if triangles == 0 {
            return SkinDeformationAudit::default();
        }
        let distance = |a: &Vec3f, b: &Vec3f| {
            let (dx, dy, dz) = (a.x - b.x, a.y - b.y, a.z - b.z);
            (dx * dx + dy * dy + dz * dz).sqrt()
        };
        let rest: Vec<Vec3f> = self.vertices.iter().map(|vertex| vertex.pos).collect();
        let rest_edges: Vec<f32> = self
            .indices
            .chunks_exact(3)
            .map(|triangle| {
                let (a, b, c) = (
                    triangle[0] as usize,
                    triangle[1] as usize,
                    triangle[2] as usize,
                );
                distance(&rest[a], &rest[b])
                    .max(distance(&rest[b], &rest[c]))
                    .max(distance(&rest[c], &rest[a]))
                    .max(1.0e-6)
            })
            .collect();
        let mut worst = vec![1.0f32; triangles];
        let samples_per_clip = samples_per_clip.max(1);
        let mut sampled = 0usize;
        let mut pose = PoseBuffer::new();
        let mut palette = Vec::new();
        let mut packed = Vec::new();
        for &clip in clips {
            let Some(duration) = self.clips.get(clip).map(|clip| clip.duration) else {
                continue;
            };
            for phase in 0..samples_per_clip {
                let time = phase as f32 / samples_per_clip as f32 * duration;
                self.sample_clip(clip, time, &mut pose);
                self.palette(&pose, &mut palette);
                self.skin_to_packed(&palette, &mut packed);
                for (slot, triangle) in self.indices.chunks_exact(3).enumerate() {
                    let point = |index: u32| {
                        let offset = index as usize * SKIN_VERTEX_FLOATS;
                        Vec3f {
                            x: packed[offset],
                            y: packed[offset + 1],
                            z: packed[offset + 2],
                        }
                    };
                    let (a, b, c) = (
                        point(triangle[0]),
                        point(triangle[1]),
                        point(triangle[2]),
                    );
                    let posed_edge = distance(&a, &b)
                        .max(distance(&b, &c))
                        .max(distance(&c, &a));
                    worst[slot] = worst[slot].max(posed_edge / rest_edges[slot]);
                }
                sampled += 1;
            }
        }
        worst.sort_by(f32::total_cmp);
        let percentile = |percent: usize| {
            let index = ((worst.len() - 1) * percent + 99) / 100;
            worst[index]
        };
        SkinDeformationAudit {
            triangles,
            samples: sampled,
            over_2x: worst.iter().filter(|stretch| **stretch > 2.0).count(),
            over_3x: worst.iter().filter(|stretch| **stretch > 3.0).count(),
            p95_stretch: percentile(95),
            p99_stretch: percentile(99),
            max_stretch: *worst.last().unwrap(),
        }
    }

    /// Find rest-pose faces that directly connect hierarchy-classified arm
    /// and leg regions.
    ///
    /// `arm_nodes` and `leg_nodes` are glTF node indices, not palette slots;
    /// this keeps the hierarchy classifier independent from the order in
    /// `skin.joints`. Confidence is the sum of a vertex's skin weights whose
    /// palette joints belong to the corresponding semantic set.
    pub fn audit_semantic_bridges(
        &self,
        arm_nodes: &[usize],
        leg_nodes: &[usize],
        confidence: f32,
    ) -> Result<SkinSemanticBridgeAudit, String> {
        if !confidence.is_finite() || !(0.5..=1.0).contains(&confidence) {
            return Err("semantic bridge confidence must be finite and in 0.5..=1.0".into());
        }
        if arm_nodes.is_empty() || leg_nodes.is_empty() {
            return Err("semantic bridge audit needs non-empty arm and leg branches".into());
        }
        let mut arm_mask = vec![false; self.nodes.len()];
        let mut leg_mask = vec![false; self.nodes.len()];
        for &node in arm_nodes {
            let Some(slot) = arm_mask.get_mut(node) else {
                return Err(format!("arm branch node {node} is out of range"));
            };
            *slot = true;
        }
        for &node in leg_nodes {
            let Some(slot) = leg_mask.get_mut(node) else {
                return Err(format!("leg branch node {node} is out of range"));
            };
            *slot = true;
        }
        if arm_mask
            .iter()
            .zip(&leg_mask)
            .any(|(arm, leg)| *arm && *leg)
        {
            return Err("arm and leg semantic branches overlap".into());
        }
        if self
            .joint_nodes
            .iter()
            .any(|node| *node >= self.nodes.len())
        {
            return Err("skin palette contains an out-of-range node".into());
        }

        let finite_position = |point: &Vec3f| {
            point.x.is_finite() && point.y.is_finite() && point.z.is_finite()
        };
        if self.vertices.iter().any(|vertex| !finite_position(&vertex.pos)) {
            return Err("rest mesh contains a non-finite position".into());
        }
        let mut min = Vec3f {
            x: f32::INFINITY,
            y: f32::INFINITY,
            z: f32::INFINITY,
        };
        let mut max = Vec3f {
            x: f32::NEG_INFINITY,
            y: f32::NEG_INFINITY,
            z: f32::NEG_INFINITY,
        };
        for vertex in &self.vertices {
            min.x = min.x.min(vertex.pos.x);
            min.y = min.y.min(vertex.pos.y);
            min.z = min.z.min(vertex.pos.z);
            max.x = max.x.max(vertex.pos.x);
            max.y = max.y.max(vertex.pos.y);
            max.z = max.z.max(vertex.pos.z);
        }
        let diagonal_squared = (max.x - min.x).powi(2)
            + (max.y - min.y).powi(2)
            + (max.z - min.z).powi(2);
        let minimum_area = (diagonal_squared * 1.0e-12).max(1.0e-20);
        let triangle_area = |a: &Vec3f, b: &Vec3f, c: &Vec3f| {
            let ab = Vec3f {
                x: b.x - a.x,
                y: b.y - a.y,
                z: b.z - a.z,
            };
            let ac = Vec3f {
                x: c.x - a.x,
                y: c.y - a.y,
                z: c.z - a.z,
            };
            let cross = Vec3f {
                x: ab.y * ac.z - ab.z * ac.y,
                y: ab.z * ac.x - ab.x * ac.z,
                z: ab.x * ac.y - ab.y * ac.x,
            };
            0.5 * (cross.x * cross.x + cross.y * cross.y + cross.z * cross.z).sqrt()
        };
        let semantic_confidence = |vertex: &SkinVertex, mask: &[bool]| -> Result<f32, String> {
            let mut value = 0.0f32;
            for influence in 0..4 {
                let weight = vertex.weights[influence];
                if !weight.is_finite() || weight < 0.0 {
                    return Err("skin contains an invalid weight".into());
                }
                let palette = vertex.joints[influence] as usize;
                if weight > 0.0 {
                    let node = *self
                        .joint_nodes
                        .get(palette)
                        .ok_or_else(|| format!("vertex references palette joint {palette} out of range"))?;
                    if mask[node] {
                        value += weight;
                    }
                }
            }
            Ok(value)
        };

        let mut audit = SkinSemanticBridgeAudit {
            triangles: self.indices.len() / 3,
            ..Default::default()
        };
        let mut total_rest_area = 0.0f32;
        for (face, triangle) in self.indices.chunks_exact(3).enumerate() {
            let vertex = |corner: usize| -> Result<&SkinVertex, String> {
                self.vertices.get(triangle[corner] as usize).ok_or_else(|| {
                    format!("face {face} references vertex {} out of range", triangle[corner])
                })
            };
            let vertices = [vertex(0)?, vertex(1)?, vertex(2)?];
            let area = triangle_area(
                &vertices[0].pos,
                &vertices[1].pos,
                &vertices[2].pos,
            );
            if !area.is_finite() {
                return Err(format!("face {face} has non-finite rest area"));
            }
            total_rest_area += area;
            if area <= minimum_area {
                continue;
            }
            audit.nondegenerate_triangles += 1;
            let arm = [
                semantic_confidence(vertices[0], &arm_mask)?,
                semantic_confidence(vertices[1], &arm_mask)?,
                semantic_confidence(vertices[2], &arm_mask)?,
            ];
            let leg = [
                semantic_confidence(vertices[0], &leg_mask)?,
                semantic_confidence(vertices[1], &leg_mask)?,
                semantic_confidence(vertices[2], &leg_mask)?,
            ];
            let arm_max = arm.into_iter().fold(0.0f32, f32::max);
            let leg_max = leg.into_iter().fold(0.0f32, f32::max);
            if arm_max >= confidence && leg_max >= confidence {
                audit.bridge_triangles += 1;
                audit.bridge_rest_area += area;
                audit.first_bridge_face.get_or_insert(face);
                audit.max_arm_confidence = audit.max_arm_confidence.max(arm_max);
                audit.max_leg_confidence = audit.max_leg_confidence.max(leg_max);
            }
        }
        if total_rest_area > 0.0 {
            audit.bridge_rest_area_fraction = audit.bridge_rest_area / total_rest_area;
        }
        Ok(audit)
    }

    /// CPU-skin every distinct authored key time in every clip, including
    /// each terminal key without the looping sampler's modulo wrap.
    pub fn audit_authored_motion_quality(&self) -> Result<SkinMotionQualityAudit, String> {
        const MAX_STRETCH: f32 = 3.0;
        const MAX_EXTENSION_HEIGHT: f32 = 0.02;

        if self.vertices.is_empty() || self.indices.len() < 3 {
            return Err("motion quality audit needs a non-empty skinned mesh".into());
        }
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for vertex in &self.vertices {
            if !vertex.pos.x.is_finite()
                || !vertex.pos.y.is_finite()
                || !vertex.pos.z.is_finite()
            {
                return Err("rest mesh contains a non-finite position".into());
            }
            min_y = min_y.min(vertex.pos.y);
            max_y = max_y.max(vertex.pos.y);
        }
        let height = max_y - min_y;
        if !height.is_finite() || height <= 1.0e-6 {
            return Err("rest mesh has no finite vertical extent".into());
        }
        let distance = |left: &Vec3f, right: &Vec3f| {
            let (dx, dy, dz) = (
                left.x - right.x,
                left.y - right.y,
                left.z - right.z,
            );
            (dx * dx + dy * dy + dz * dz).sqrt()
        };
        let triangle_area = |a: &Vec3f, b: &Vec3f, c: &Vec3f| {
            let ab = (b.x - a.x, b.y - a.y, b.z - a.z);
            let ac = (c.x - a.x, c.y - a.y, c.z - a.z);
            let cross = (
                ab.1 * ac.2 - ab.2 * ac.1,
                ab.2 * ac.0 - ab.0 * ac.2,
                ab.0 * ac.1 - ab.1 * ac.0,
            );
            0.5 * (cross.0 * cross.0 + cross.1 * cross.1 + cross.2 * cross.2).sqrt()
        };

        let triangles = self.indices.len() / 3;
        let mut rest_diameter = Vec::with_capacity(triangles);
        let mut rest_area = Vec::with_capacity(triangles);
        let mut total_rest_area = 0.0f32;
        for (face, triangle) in self.indices.chunks_exact(3).enumerate() {
            let point = |corner: usize| -> Result<Vec3f, String> {
                self.vertices
                    .get(triangle[corner] as usize)
                    .map(|vertex| vertex.pos)
                    .ok_or_else(|| {
                        format!("face {face} references vertex {} out of range", triangle[corner])
                    })
            };
            let (a, b, c) = (point(0)?, point(1)?, point(2)?);
            let diameter = distance(&a, &b)
                .max(distance(&b, &c))
                .max(distance(&c, &a));
            let area = triangle_area(&a, &b, &c);
            if !diameter.is_finite() || !area.is_finite() {
                return Err(format!("face {face} has non-finite rest geometry"));
            }
            rest_diameter.push(diameter);
            rest_area.push(area);
            total_rest_area += area;
        }

        let mut audit = SkinMotionQualityAudit {
            triangles,
            clips: self.clips.len(),
            rest_bbox_height: height,
            ..Default::default()
        };
        let mut bad_faces = vec![false; triangles];
        let mut worst_bad_extension = 0.0f32;
        let mut pose = PoseBuffer::new();
        let mut palette = Vec::new();
        let mut packed = Vec::new();
        for (clip_index, animation) in self.clips.iter().enumerate() {
            let mut times = vec![0.0f32, animation.duration];
            for channel in &animation.channels {
                for &time in &channel.times {
                    if !time.is_finite() {
                        return Err(format!("clip {clip_index} contains a non-finite key time"));
                    }
                    times.push(time.clamp(0.0, animation.duration));
                }
            }
            times.sort_by(f32::total_cmp);
            times.dedup_by(|left, right| left.to_bits() == right.to_bits());
            for (authored_frame, &time) in times.iter().enumerate() {
                self.sample_clip_clamped(clip_index, time, &mut pose);
                self.palette(&pose, &mut palette);
                self.skin_to_packed(&palette, &mut packed);
                if packed.len() != self.vertices.len() * SKIN_VERTEX_FLOATS
                    || packed.chunks_exact(SKIN_VERTEX_FLOATS).any(|vertex| {
                        !vertex[0].is_finite()
                            || !vertex[1].is_finite()
                            || !vertex[2].is_finite()
                    })
                {
                    return Err(format!(
                        "clip {clip_index} authored frame {authored_frame} skins to invalid vertices"
                    ));
                }
                for (face, triangle) in self.indices.chunks_exact(3).enumerate() {
                    let point = |index: u32| {
                        let offset = index as usize * SKIN_VERTEX_FLOATS;
                        Vec3f {
                            x: packed[offset],
                            y: packed[offset + 1],
                            z: packed[offset + 2],
                        }
                    };
                    let (a, b, c) = (
                        point(triangle[0]),
                        point(triangle[1]),
                        point(triangle[2]),
                    );
                    let posed_diameter = distance(&a, &b)
                        .max(distance(&b, &c))
                        .max(distance(&c, &a));
                    let rest = rest_diameter[face];
                    if rest <= 1.0e-12 || rest_area[face] <= 1.0e-20 {
                        continue;
                    }
                    let stretch = posed_diameter / rest;
                    let extension_height = (posed_diameter - rest).max(0.0) / height;
                    audit.max_stretch = audit.max_stretch.max(stretch);
                    audit.max_extension_height =
                        audit.max_extension_height.max(extension_height);
                    if stretch > MAX_STRETCH && extension_height > MAX_EXTENSION_HEIGHT {
                        bad_faces[face] = true;
                        if audit.worst_face.is_none() || extension_height > worst_bad_extension {
                            worst_bad_extension = extension_height;
                            audit.worst_clip = Some(clip_index);
                            audit.worst_authored_frame = Some(authored_frame);
                            audit.worst_face = Some(face);
                            audit.worst_time_seconds = time;
                        }
                    }
                }
                audit.authored_samples += 1;
            }
        }
        for (face, bad) in bad_faces.into_iter().enumerate() {
            if bad {
                audit.bad_triangles += 1;
                audit.bad_rest_area += rest_area[face];
            }
        }
        if total_rest_area > 0.0 {
            audit.bad_rest_area_fraction = audit.bad_rest_area / total_rest_area;
        }
        Ok(audit)
    }

    /// Audit every consecutive authored frame at `fps` without wrapping the
    /// final sample onto the first. The closing seam is measured separately.
    pub fn audit_temporal_motion(&self, clip: usize, fps: f32) -> SkinTemporalAudit {
        let Some(animation) = self.clips.get(clip) else {
            return SkinTemporalAudit::default();
        };
        if !fps.is_finite() || fps <= 0.0 {
            return SkinTemporalAudit::default();
        }
        let frames = (animation.duration * fps).round().max(1.0) as usize;
        let frame_time = |frame: usize| {
            // Retargeted character clips key frame zero at 1/fps. Stay just
            // below duration on the final sample so sample_clip cannot wrap.
            let before_wrap =
                animation.duration - (1.0 / fps).min(animation.duration) * 1.0e-4;
            ((frame + 1) as f32 / fps).min(before_wrap).max(0.0)
        };
        let quaternion_angle_degrees = |left: &Quat, right: &Quat| {
            let left_len = (left.x * left.x + left.y * left.y + left.z * left.z + left.w * left.w)
                .sqrt()
                .max(f32::EPSILON);
            let right_len =
                (right.x * right.x + right.y * right.y + right.z * right.z + right.w * right.w)
                    .sqrt()
                    .max(f32::EPSILON);
            let dot = ((left.x * right.x
                + left.y * right.y
                + left.z * right.z
                + left.w * right.w)
                / (left_len * right_len))
                .abs()
                .clamp(-1.0, 1.0);
            2.0 * dot.acos() * 180.0 / std::f32::consts::PI
        };
        let point_distance = |left: &[f32], right: &[f32]| {
            let (dx, dy, dz) = (left[0] - right[0], left[1] - right[1], left[2] - right[2]);
            (dx * dx + dy * dy + dz * dz).sqrt()
        };
        let mut poses = Vec::with_capacity(frames);
        let mut meshes = Vec::with_capacity(frames);
        let mut pose = PoseBuffer::new();
        let mut palette = Vec::new();
        for frame in 0..frames {
            self.sample_clip(clip, frame_time(frame), &mut pose);
            self.palette(&pose, &mut palette);
            let mut packed = Vec::new();
            self.skin_to_packed(&palette, &mut packed);
            poses.push(pose.clone());
            meshes.push(packed);
        }
        let mut joint_deltas = Vec::with_capacity(frames.saturating_sub(1) * self.nodes.len());
        let mut vertex_deltas =
            Vec::with_capacity(frames.saturating_sub(1) * self.vertices.len());
        let mut audit = SkinTemporalAudit {
            frames,
            frame_pairs: frames.saturating_sub(1),
            ..Default::default()
        };
        for frame in 1..frames {
            for node in 0..poses[frame].len() {
                let angle = quaternion_angle_degrees(
                    &poses[frame - 1][node].r,
                    &poses[frame][node].r,
                );
                joint_deltas.push(angle);
                if angle > audit.max_joint_angle_degrees {
                    audit.max_joint_angle_degrees = angle;
                    audit.max_joint_node = node;
                    audit.max_joint_frame = frame;
                }
            }
            for vertex in 0..self.vertices.len() {
                let offset = vertex * SKIN_VERTEX_FLOATS;
                let delta = point_distance(
                    &meshes[frame - 1][offset..offset + 3],
                    &meshes[frame][offset..offset + 3],
                );
                vertex_deltas.push(delta);
                if delta > audit.max_vertex_delta {
                    audit.max_vertex_delta = delta;
                    audit.max_vertex = vertex;
                    audit.max_vertex_frame = frame;
                }
            }
        }
        let percentile_99 = |values: &mut Vec<f32>| {
            if values.is_empty() {
                return 0.0;
            }
            values.sort_by(f32::total_cmp);
            values[((values.len() - 1) * 99 + 99) / 100]
        };
        audit.p99_joint_angle_degrees = percentile_99(&mut joint_deltas);
        audit.p99_vertex_delta = percentile_99(&mut vertex_deltas);
        if frames > 1 {
            for node in 0..poses[0].len() {
                audit.seam_joint_angle_degrees = audit.seam_joint_angle_degrees.max(
                    quaternion_angle_degrees(&poses[frames - 1][node].r, &poses[0][node].r),
                );
            }
            for vertex in 0..self.vertices.len() {
                let offset = vertex * SKIN_VERTEX_FLOATS;
                audit.seam_vertex_delta = audit.seam_vertex_delta.max(point_distance(
                    &meshes[frames - 1][offset..offset + 3],
                    &meshes[0][offset..offset + 3],
                ));
            }
        }
        audit
    }

    /// Audit the actual loop-blended runtime path over `cycles` complete
    /// cycles. Sampling includes the final endpoint of the last cycle and
    /// therefore measures (rather than accidentally skipping) every wrap.
    pub fn audit_loop_blended_motion(
        &self,
        clip: usize,
        fps: f32,
        blend_seconds: f32,
        cycles: usize,
    ) -> SkinLoopTemporalAudit {
        let Some(animation) = self.clips.get(clip) else {
            return SkinLoopTemporalAudit::default();
        };
        if !fps.is_finite()
            || fps <= 0.0
            || !blend_seconds.is_finite()
            || blend_seconds < 0.0
            || cycles == 0
        {
            return SkinLoopTemporalAudit::default();
        }
        let total_time = animation.duration * cycles as f32;
        let frame_pairs = (total_time * fps).ceil().max(1.0) as usize;
        let mut audit = SkinLoopTemporalAudit {
            cycles,
            frames: frame_pairs + 1,
            frame_pairs,
            ..Default::default()
        };
        let quaternion_angle_degrees = |left: &Quat, right: &Quat| {
            let left_len = (left.x * left.x + left.y * left.y + left.z * left.z + left.w * left.w)
                .sqrt()
                .max(f32::EPSILON);
            let right_len =
                (right.x * right.x + right.y * right.y + right.z * right.z + right.w * right.w)
                    .sqrt()
                    .max(f32::EPSILON);
            let dot = ((left.x * right.x
                + left.y * right.y
                + left.z * right.z
                + left.w * right.w)
                / (left_len * right_len))
                .abs()
                .clamp(-1.0, 1.0);
            2.0 * dot.acos() * 180.0 / std::f32::consts::PI
        };
        let point_distance = |left: &[f32], right: &[f32]| {
            let (dx, dy, dz) = (left[0] - right[0], left[1] - right[1], left[2] - right[2]);
            (dx * dx + dy * dy + dz * dz).sqrt()
        };

        let mut previous_pose = PoseBuffer::new();
        let mut current_pose = PoseBuffer::new();
        let mut loop_scratch = PoseBuffer::new();
        let mut palette = Vec::new();
        let mut previous_mesh = Vec::new();
        let mut current_mesh = Vec::new();
        self.sample_clip_loop_blended(
            clip,
            0.0,
            blend_seconds,
            &mut previous_pose,
            &mut loop_scratch,
        );
        self.palette(&previous_pose, &mut palette);
        self.skin_to_packed(&palette, &mut previous_mesh);
        let mut previous_time = 0.0f32;
        for frame in 1..=frame_pairs {
            let time = (frame as f32 / fps).min(total_time);
            self.sample_clip_loop_blended(
                clip,
                time,
                blend_seconds,
                &mut current_pose,
                &mut loop_scratch,
            );
            self.palette(&current_pose, &mut palette);
            self.skin_to_packed(&palette, &mut current_mesh);
            let crossed_wrap = (time / animation.duration).floor()
                > (previous_time / animation.duration).floor();
            if crossed_wrap {
                audit.wraps += 1;
            }
            for (node, (left, right)) in previous_pose.iter().zip(&current_pose).enumerate() {
                let angle = quaternion_angle_degrees(&left.r, &right.r);
                if angle > audit.max_joint_angle_degrees {
                    audit.max_joint_angle_degrees = angle;
                    audit.max_joint_node = node;
                    audit.max_joint_frame = frame;
                }
                if crossed_wrap {
                    audit.wrap_joint_angle_degrees = audit.wrap_joint_angle_degrees.max(angle);
                }
            }
            for vertex in 0..self.vertices.len() {
                let offset = vertex * SKIN_VERTEX_FLOATS;
                let delta = point_distance(
                    &previous_mesh[offset..offset + 3],
                    &current_mesh[offset..offset + 3],
                );
                if delta > audit.max_vertex_delta {
                    audit.max_vertex_delta = delta;
                    audit.max_vertex = vertex;
                    audit.max_vertex_frame = frame;
                }
                if crossed_wrap {
                    audit.wrap_vertex_delta = audit.wrap_vertex_delta.max(delta);
                }
            }
            std::mem::swap(&mut previous_pose, &mut current_pose);
            std::mem::swap(&mut previous_mesh, &mut current_mesh);
            previous_time = time;
        }
        audit
    }

    /// Rank each node by its largest consecutive authored-frame rotation.
    pub fn temporal_joint_outliers(
        &self,
        clip: usize,
        fps: f32,
    ) -> Vec<SkinJointTemporalOutlier> {
        let Some(animation) = self.clips.get(clip) else {
            return Vec::new();
        };
        if !fps.is_finite() || fps <= 0.0 {
            return Vec::new();
        }
        let frames = (animation.duration * fps).round().max(1.0) as usize;
        let before_wrap = animation.duration - (1.0 / fps).min(animation.duration) * 1.0e-4;
        let mut prior = PoseBuffer::new();
        let mut current = PoseBuffer::new();
        let mut outliers = vec![SkinJointTemporalOutlier::default(); self.nodes.len()];
        for (node, outlier) in outliers.iter_mut().enumerate() {
            outlier.node = node;
        }
        for frame in 0..frames {
            let time = ((frame + 1) as f32 / fps).min(before_wrap).max(0.0);
            self.sample_clip(clip, time, &mut current);
            if frame > 0 {
                for (node, (left, right)) in prior.iter().zip(&current).enumerate() {
                    let left_length = (left.r.x * left.r.x
                        + left.r.y * left.r.y
                        + left.r.z * left.r.z
                        + left.r.w * left.r.w)
                        .sqrt()
                        .max(f32::EPSILON);
                    let right_length = (right.r.x * right.r.x
                        + right.r.y * right.r.y
                        + right.r.z * right.r.z
                        + right.r.w * right.r.w)
                        .sqrt()
                        .max(f32::EPSILON);
                    let dot = ((left.r.x * right.r.x
                        + left.r.y * right.r.y
                        + left.r.z * right.r.z
                        + left.r.w * right.r.w)
                        / (left_length * right_length))
                        .abs()
                        .clamp(-1.0, 1.0);
                    let angle = 2.0 * dot.acos() * 180.0 / std::f32::consts::PI;
                    if angle > outliers[node].angle_degrees {
                        outliers[node].angle_degrees = angle;
                        outliers[node].frame = frame;
                    }
                }
            }
            std::mem::swap(&mut prior, &mut current);
        }
        outliers.sort_by(|left, right| right.angle_degrees.total_cmp(&left.angle_degrees));
        outliers
    }

    /// Weight hygiene for AUTO-rigged meshes (SkinTokens/UniRig): prune
    /// influences below `threshold`, and when one joint clearly dominates
    /// (weight ≥ `harden_at`) drop the stragglers entirely; renormalize.
    ///
    /// Why: auto-skinning leaves faint cross-limb influences — a crotch
    /// vertex 0.9 left-hip / 0.1 right-hip sags toward the other leg the
    /// moment the legs separate, reading as glitch webbing between the
    /// thighs mid-stride. Hand-authored rigs don't need this (their weights
    /// are already clean), so it is an opt-in host call, not a parse step.
    /// Returns how many influences were culled.
    pub fn prune_weights(&mut self, threshold: f32, harden_at: f32) -> usize {
        let mut culled = 0;
        for v in &mut self.vertices {
            let dominant_index = (0..4)
                .max_by(|a, b| v.weights[*a].partial_cmp(&v.weights[*b]).unwrap())
                .unwrap_or(0);
            let dominant = v.weights[dominant_index];
            for (k, w) in v.weights.iter_mut().enumerate() {
                if *w > 0.0
                    && k != dominant_index
                    && (*w < threshold || dominant >= harden_at)
                {
                    *w = 0.0;
                    culled += 1;
                }
            }
            let total: f32 = v.weights.iter().sum();
            if total > 0.0 {
                for w in v.weights.iter_mut() {
                    *w /= total;
                }
            } else {
                // Everything pruned (degenerate input): keep the dominant.
                v.weights = [0.0; 4];
                v.weights[dominant_index] = 1.0;
            }
        }
        culled
    }
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// First clip whose name contains `needle` (ASCII case-insensitive).
    pub fn clip_index(&self, needle: &str) -> Option<usize> {
        let needle = needle.to_ascii_lowercase();
        self.clips
            .iter()
            .position(|c| c.name.to_ascii_lowercase().contains(&needle))
    }

    /// Clip whose authored name exactly matches `name`, ignoring ASCII case.
    ///
    /// Action overlays use this resolver: substring matching can select an
    /// aiming loop when the caller requested its similarly named fire shot.
    pub fn clip_index_exact(&self, name: &str) -> Option<usize> {
        self.clips
            .iter()
            .position(|clip| clip.name.eq_ignore_ascii_case(name))
    }

    /// First clip matching any of `names`, tried in order — the resolver for
    /// an animation STATE against this rig's vocabulary (P4: hosts feed it
    /// `AnimState::clip_candidates()`; a miss across the whole list means
    /// "use the part-pose choreography fallback"). Order matters: the caller
    /// lists its best fit first, so a rig with both `jump_start` and `jump`
    /// lands on the intended one.
    pub fn clip_index_any(&self, names: &[&str]) -> Option<usize> {
        names.iter().find_map(|n| self.clip_index(n))
    }

    /// The rig's gait pair — (idle, walk) clip indices resolved through
    /// [`GAIT_IDLE_CLIPS`]/[`GAIT_WALK_CLIPS`]. One resolver shared by the
    /// sandbox cast loader AND the offline shadow baker (tools/ao_bake), so
    /// the pose set a `.shadowsdf` sidecar was baked from is the pose set
    /// the runtime animates — the two picking different clips would be an
    /// invisible wrong-shadow bug, not an error. `None` = no recognisable
    /// gait; such a rig gets no baked silhouette atlas.
    pub fn gait_clips(&self) -> Option<(usize, usize)> {
        Some((
            self.clip_index_any(GAIT_IDLE_CLIPS)?,
            self.clip_index_any(GAIT_WALK_CLIPS)?,
        ))
    }

    /// CPU-skin the SDF-shadow-bake pose set: the idle stance at t = 0 plus
    /// the walk cycle at [`crate::shadow_sdf::SDF_GAIT_PHASES`] evenly
    /// spaced stations, each as a packed [`SKIN_VERTEX_FLOATS`] stream
    /// ([`Self::skin_to_packed`]). This IS the `.shadowsdf` rig sidecar's
    /// bake input (tools/ao_bake via [`crate::shadow_sdf::bake_rig_atlas`]);
    /// the parity tests reconstruct the same set to prove a sidecar equals
    /// the bake byte for byte.
    pub fn shadow_pose_meshes(&self, idle: usize, walk: usize) -> Vec<Vec<f32>> {
        let walk_dur = self.clips.get(walk).map_or(0.01, |c| c.duration).max(0.01);
        let mut pose = PoseBuffer::new();
        let mut pal: Vec<Mat4f> = Vec::new();
        let mut meshes = Vec::with_capacity(1 + crate::shadow_sdf::SDF_GAIT_PHASES);
        let mut skin_at = |clip: usize, t: f32, meshes: &mut Vec<Vec<f32>>| {
            self.sample_clip(clip, t, &mut pose);
            self.palette(&pose, &mut pal);
            let mut vertices = Vec::new();
            self.skin_to_packed(&pal, &mut vertices);
            meshes.push(vertices);
        };
        skin_at(idle, 0.0, &mut meshes);
        for k in 0..crate::shadow_sdf::SDF_GAIT_PHASES {
            let t = k as f32 / crate::shadow_sdf::SDF_GAIT_PHASES as f32 * walk_dur;
            skin_at(walk, t, &mut meshes);
        }
        meshes
    }

    pub fn rest_pose(&self) -> PoseBuffer {
        self.nodes.iter().map(|n| n.rest).collect()
    }

    /// Ground speed the walk `clip` depicts at playback rate 1, in the
    /// model's own units per second — the number that makes stride-matched
    /// locomotion possible for ANY rig: playback rate = ground_speed / this,
    /// and the feet stay planted at every travel speed (play-session-1
    /// entry 20, "the walking ANIMATION is too fast" — for every walker).
    ///
    /// Method: the support foot, whatever the rig calls it. Candidates are
    /// the clip-animated nodes whose rest origin sits in the lower body;
    /// each carries a probe at its rest ground reach, so a hip-pivoted
    /// block leg (kenney mini rigs have no foot bones) measures exactly
    /// like a real foot bone. Across one sampled cycle the lowest probe is
    /// the support; the median planar speed of the support probe between
    /// consecutive samples with the SAME support (excludes swap frames) is
    /// the depicted ground speed. `None` when the clip animates no
    /// lower-body node or the measurement is degenerate — callers keep
    /// their heuristic.
    pub fn walk_clip_ground_speed(&self, clip_index: usize) -> Option<f32> {
        let clip = self.clips.get(clip_index)?;
        if clip.duration <= 1.0e-3 {
            return None;
        }
        let rest = self.rest_pose();
        let origins: Vec<Vec3f> = (0..self.nodes.len())
            .map(|i| {
                self.node_mesh_transform(&rest, i)
                    .map(|m| Vec3f { x: m.v[12], y: m.v[13], z: m.v[14] })
                    .unwrap_or_default()
            })
            .collect();
        let min_y = origins.iter().map(|o| o.y).fold(f32::MAX, f32::min);
        let max_y = origins.iter().map(|o| o.y).fold(f32::MIN, f32::max);
        let height = max_y - min_y;
        if !(height > 1.0e-3) {
            return None;
        }
        let animated: std::collections::HashSet<usize> =
            clip.channels.iter().map(|c| c.node).collect();
        // (node, rest ground reach below its origin)
        let candidates: Vec<(usize, f32)> = origins
            .iter()
            .enumerate()
            .filter(|(i, o)| animated.contains(i) && o.y - min_y < height * 0.45)
            .map(|(i, o)| (i, o.y - min_y))
            .collect();
        if candidates.is_empty() {
            return None;
        }
        const STEPS: usize = 48;
        let dt = clip.duration / STEPS as f32;
        let mut pose = self.rest_pose();
        let mut prev: Option<(usize, Vec3f)> = None;
        let mut speeds: Vec<f32> = Vec::new();
        for s in 0..=STEPS {
            self.sample_clip(clip_index, s as f32 * dt, &mut pose);
            let mut support: Option<(usize, Vec3f)> = None;
            for (node, reach) in &candidates {
                let Some(m) = self.node_mesh_transform(&pose, *node) else {
                    continue;
                };
                let p = mat4_mul_point(&m, Vec3f { x: 0.0, y: -reach, z: 0.0 });
                if support.map_or(true, |(_, sp)| p.y < sp.y) {
                    support = Some((*node, p));
                }
            }
            let Some((node, p)) = support else { continue };
            if let Some((prev_node, pp)) = prev {
                if prev_node == node {
                    let (dx, dz) = (p.x - pp.x, p.z - pp.z);
                    speeds.push((dx * dx + dz * dz).sqrt() / dt);
                }
            }
            prev = Some((node, p));
        }
        if speeds.len() < STEPS / 4 {
            return None;
        }
        speeds.sort_by(|a, b| a.total_cmp(b));
        let v = speeds[speeds.len() / 2];
        (v > 1.0e-3).then_some(v)
    }

    /// Reset translation on every skeleton root to its rest value while
    /// preserving all authored rotations and child motion.
    ///
    /// Playable controllers use this after sampling a jump so root travel is
    /// applied exactly once by the host transform. Multiple disconnected
    /// skin roots are supported; malformed pose buffers fail closed.
    pub fn strip_skeleton_root_translation(&self, pose: &mut PoseBuffer) {
        if pose.len() != self.nodes.len() {
            return;
        }
        for &node in &self.joint_nodes {
            let parent_is_joint = self.nodes[node]
                .parent
                .is_some_and(|parent| self.joint_nodes.contains(&parent));
            if !parent_is_joint {
                pose[node].t = self.nodes[node].rest.t;
            }
        }
    }

    /// Sample `clip` at time `t` (wrapping) over the rest pose.
    pub fn sample_clip(&self, clip: usize, t: f32, out: &mut PoseBuffer) {
        out.clear();
        out.extend(self.nodes.iter().map(|n| n.rest));
        let Some(clip) = self.clips.get(clip) else { return };
        // Wrap with exact ops (floor is IEEE-exact).
        let t = if t.is_finite() {
            t - (t / clip.duration).floor() * clip.duration
        } else {
            0.0
        };
        for ch in &clip.channels {
            if ch.node >= out.len() || ch.times.is_empty() {
                continue;
            }
            ch.apply_at(t, &mut out[ch.node]);
        }
    }

    /// Sample a looping clip while smoothing its authored tail into its head.
    ///
    /// Generated locomotion clips are not guaranteed to key an identical
    /// first and last pose. A plain modulo clock exposes that mismatch as one
    /// violent frame on every cycle. During the final `blend_seconds` this
    /// sampler smoothly converges on the first pose, making the value at the
    /// end of the cycle equal to the value after wrap. The easing has zero
    /// slope at both ends, so entering the repair window does not add another
    /// corner. The source model, channels and key data are never modified.
    ///
    /// The repair window is capped at half the clip duration. Invalid or
    /// non-positive blend lengths preserve [`Self::sample_clip`] exactly.
    /// `scratch` is caller-owned so steady-state game sampling allocates no
    /// temporary pose per frame.
    pub fn sample_clip_loop_blended(
        &self,
        clip: usize,
        t: f32,
        blend_seconds: f32,
        out: &mut PoseBuffer,
        scratch: &mut PoseBuffer,
    ) {
        self.sample_clip(clip, t, out);
        let Some(animation) = self.clips.get(clip) else {
            return;
        };
        if !blend_seconds.is_finite() || blend_seconds <= 0.0 {
            return;
        }
        let blend_seconds = blend_seconds.min(animation.duration * 0.5);
        if blend_seconds <= f32::EPSILON {
            return;
        }
        let wrapped = if t.is_finite() {
            t - (t / animation.duration).floor() * animation.duration
        } else {
            0.0
        };
        let blend_start = animation.duration - blend_seconds;
        if wrapped < blend_start {
            return;
        }

        self.sample_clip(clip, 0.0, scratch);
        let linear = ((wrapped - blend_start) / blend_seconds).clamp(0.0, 1.0);
        let weight = linear * linear * (3.0 - 2.0 * linear);
        for (pose, head) in out.iter_mut().zip(scratch.iter()) {
            pose.t.x += (head.t.x - pose.t.x) * weight;
            pose.t.y += (head.t.y - pose.t.y) * weight;
            pose.t.z += (head.t.z - pose.t.z) * weight;
            pose.r = nlerp(pose.r, head.r, weight);
            pose.s.x += (head.s.x - pose.s.x) * weight;
            pose.s.y += (head.s.y - pose.s.y) * weight;
            pose.s.z += (head.s.z - pose.s.z) * weight;
        }
    }

    /// Sample a one-shot clip over the rest pose, holding its terminal key
    /// after completion instead of wrapping to the beginning.
    pub fn sample_clip_clamped(&self, clip: usize, t: f32, out: &mut PoseBuffer) {
        out.clear();
        out.extend(self.nodes.iter().map(|node| node.rest));
        let Some(animation) = self.clips.get(clip) else {
            return;
        };
        let t = if t.is_nan() {
            0.0
        } else {
            t.clamp(0.0, animation.duration)
        };
        for channel in &animation.channels {
            if channel.node >= out.len() || channel.times.is_empty() {
                continue;
            }
            channel.apply_at(t, &mut out[channel.node]);
        }
    }

    /// Overlay keyed channels from `clip` onto `pose` inside `mask`.
    ///
    /// Time is clamped rather than wrapped, which keeps a one-shot action on
    /// its final authored key. Only the TRS component actually keyed by the
    /// clip is written: unkeyed components and every node outside `mask`
    /// remain bit-for-bit identical to the base pose. Callers that want a
    /// rest-pose base can pass [`Self::rest_pose`].
    ///
    /// Invalid clip indices or buffers not built for this model fail closed
    /// and leave `pose` unchanged.
    pub fn overlay_clip_masked(
        &self,
        clip: usize,
        t: f32,
        mask: &[bool],
        pose: &mut PoseBuffer,
    ) -> bool {
        let Some(clip) = self.clips.get(clip) else {
            return false;
        };
        if mask.len() != self.nodes.len() || pose.len() != self.nodes.len() {
            return false;
        }
        let t = if t.is_nan() {
            0.0
        } else {
            t.clamp(0.0, clip.duration)
        };
        for channel in &clip.channels {
            if channel.node < pose.len()
                && mask[channel.node]
                && !channel.times.is_empty()
            {
                channel.apply_at(t, &mut pose[channel.node]);
            }
        }
        true
    }

    /// `out = a*(1-w) + b*w` per node (positions/scales lerp, rotations nlerp).
    pub fn blend_pose(a: &PoseBuffer, b: &PoseBuffer, w: f32, out: &mut PoseBuffer) {
        out.clear();
        for (pa, pb) in a.iter().zip(b.iter()) {
            out.push(NodeTrs {
                t: Vec3f {
                    x: pa.t.x + (pb.t.x - pa.t.x) * w,
                    y: pa.t.y + (pb.t.y - pa.t.y) * w,
                    z: pa.t.z + (pb.t.z - pa.t.z) * w,
                },
                r: nlerp(pa.r, pb.r, w),
                s: Vec3f {
                    x: pa.s.x + (pb.s.x - pa.s.x) * w,
                    y: pa.s.y + (pb.s.y - pa.s.y) * w,
                    z: pa.s.z + (pb.s.z - pa.s.z) * w,
                },
            });
        }
    }

    /// Joint palette for a pose: `inv(global(mesh_node)) * global(joint) * ibm`.
    pub fn palette(&self, pose: &PoseBuffer, out: &mut Vec<Mat4f>) {
        let mut globals: Vec<Option<Mat4f>> = vec![None; self.nodes.len()];
        fn global(
            nodes: &[Node],
            pose: &PoseBuffer,
            globals: &mut Vec<Option<Mat4f>>,
            i: usize,
        ) -> Mat4f {
            if let Some(m) = &globals[i] {
                return *m;
            }
            let local = trs_to_mat4(pose.get(i).unwrap_or(&nodes[i].rest));
            let m = match nodes[i].parent {
                Some(p) => Mat4f::mul(&global(nodes, pose, globals, p), &local),
                None => local,
            };
            globals[i] = Some(m);
            m
        }
        let mesh_inv = global(&self.nodes, pose, &mut globals, self.mesh_node).invert();
        out.clear();
        for (j, node) in self.joint_nodes.iter().enumerate() {
            let g = global(&self.nodes, pose, &mut globals, *node);
            out.push(Mat4f::mul(&Mat4f::mul(&mesh_inv, &g), &self.inverse_bind[j]));
        }
    }

    /// CPU-skin into the packed `geom.GameMeshVertexAo` layout the prop
    /// shader draws ([`SKIN_VERTEX_FLOATS`] floats/vertex).
    ///
    /// No longer the per-frame path — characters skin on the GPU against
    /// [`Self::rest_gpu_packed`] + a palette texture. This stays for the
    /// consumers that want an actual posed mesh on the CPU: the rotational
    /// shadow bake (one skin per character per sun era) and the tests that
    /// pin the GPU formula to this one.
    pub fn skin_to_packed(&self, palette: &[Mat4f], out: &mut Vec<f32>) {
        out.clear();
        out.reserve(self.vertices.len() * SKIN_VERTEX_FLOATS);
        for v in &self.vertices {
            let mut pos = Vec3f::default();
            let mut normal = Vec3f::default();
            for k in 0..4 {
                let w = v.weights[k];
                if w == 0.0 {
                    continue;
                }
                let Some(m) = palette.get(v.joints[k] as usize) else {
                    continue;
                };
                let p = mat4_mul_point(m, v.pos);
                let n = mat4_mul_dir(m, v.normal);
                pos.x += p.x * w;
                pos.y += p.y * w;
                pos.z += p.z * w;
                normal.x += n.x * w;
                normal.y += n.y * w;
                normal.z += n.z * w;
            }
            let len = (normal.x * normal.x + normal.y * normal.y + normal.z * normal.z).sqrt();
            if len > 1.0e-8 {
                normal.x /= len;
                normal.y /= len;
                normal.z /= len;
            }
            let (ox, oy) = oct_encode(normal);
            out.extend_from_slice(&[
                pos.x,
                pos.y,
                pos.z,
                makepad_draw::pack_pair_f16(ox, oy),
                makepad_draw::pack_pair_f16(v.uv[0], v.uv[1]),
                makepad_draw::pack_unorm8x4(1.0, 1.0, 1.0, 1.0),
                // AO-atlas uv. Characters carry no baked occlusion — they
                // deform, so a bake would be wrong the moment they moved —
                // but they SHARE the prop shader, so they must match its
                // vertex layout exactly. Omitting this lane made every
                // character render at the wrong stride: a shattered fan of
                // spikes, because each vertex read one float into the next.
                // `ao_enabled` is 0 for them, so the value is never sampled.
                0.0,
            ]);
        }
    }

    /// Cheap identity of the rest mesh (FNV-1a over positions, normals, uvs,
    /// influences and topology). Stamped into [`SkinRestGpu`] so a disk-cached
    /// bake is ignored the moment the asset changes.
    pub fn rest_hash(&self) -> u64 {
        fn eat(mut h: u64, bytes: &[u8]) -> u64 {
            for b in bytes {
                h = (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3);
            }
            h
        }
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for v in &self.vertices {
            for f in [
                v.pos.x, v.pos.y, v.pos.z, v.normal.x, v.normal.y, v.normal.z, v.uv[0], v.uv[1],
            ] {
                h = eat(h, &f.to_le_bytes());
            }
            for j in v.joints {
                h = eat(h, &j.to_le_bytes());
            }
            for w in v.weights {
                h = eat(h, &w.to_le_bytes());
            }
        }
        for i in &self.indices {
            h = eat(h, &i.to_le_bytes());
        }
        h
    }

    /// Build the rig's GPU rest bundle: the REST mesh chart-split and packed
    /// into the `geom.GameMeshVertexSkin` layout ([`SKIN_GPU_VERTEX_FLOATS`]
    /// floats/vertex — position, octahedral normal, colormap uv, 4 u8 joint
    /// indices, 4 unorm8 weights, unorm16x2 AO-atlas uv) plus the rest-pose
    /// AO atlas those uvs index. Uploaded ONCE per rig; the vertex shader
    /// blends it against a palette texture, so a character's per-frame cost
    /// is its joint palette.
    ///
    /// AO is a CHART TEXTURE ([`crate::ao_atlas::bake_into`]), not per-vertex:
    /// per-vertex occlusion interpolates across whole triangles, so on
    /// low-poly heads an ear's darkness smeared over the skull dome — the
    /// same failure that moved the props to the atlas. Baked on the REST pose
    /// once per rig: topology never changes, so the occlusion rides the
    /// skinned surface through every pose — an armpit stays an armpit
    /// mid-swing. Deterministic, but NOT free (hundreds of ms for a hero
    /// rig): callers that load at runtime should cache the bundle on disk
    /// ([`SkinRestGpu::to_bytes`]) keyed by [`Self::rest_hash`].
    ///
    /// Weight quantization is compensated: the largest weight absorbs the
    /// rounding so the four u8s sum to exactly 255, keeping the blended
    /// position an affine combination (a lossy sum visibly shrinks or grows
    /// the mesh where four influences meet).
    pub fn rest_gpu(&self) -> SkinRestGpu {
        // u8 joint indices; a rig this large would need a wider lane, and
        // silently wrapping would scramble the mesh.
        assert!(self.joint_nodes.len() <= 256, "rig exceeds u8 joint indices");
        let mut positions: Vec<Vec3f> = self.vertices.iter().map(|v| v.pos).collect();
        let mut normals: Vec<Vec3f> = self.vertices.iter().map(|v| v.normal).collect();
        let mut indices = self.indices.clone();
        let (mut min, mut max) = (
            Vec3f { x: f32::MAX, y: f32::MAX, z: f32::MAX },
            Vec3f { x: f32::MIN, y: f32::MIN, z: f32::MIN },
        );
        for p in &positions {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            min.z = min.z.min(p.z);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
            max.z = max.z.max(p.z);
        }
        let mut atlas = crate::ao_atlas::AoAtlas::new(SKIN_AO_ATLAS);
        // Single-rig atlas: scale charts up until the texture is spent.
        atlas.fill = true;
        let baked = crate::ao_atlas::bake_into(
            &mut atlas, &mut positions, &mut normals, &mut indices, min, max,
        );
        let mut out = Vec::with_capacity(positions.len() * SKIN_GPU_VERTEX_FLOATS);
        for (i, src) in baked.source_vertex.iter().enumerate() {
            // Chart seams duplicate vertices; influences, uv and weights are
            // carried from the source vertex so a split vertex blends to the
            // exact same position its original did.
            let v = &self.vertices[*src as usize];
            let mut q = [0u32; 4];
            let mut total = 0u32;
            for k in 0..4 {
                q[k] = (v.weights[k].clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
                total += q[k];
            }
            if total > 0 {
                let largest = (0..4).max_by_key(|k| q[*k]).unwrap();
                q[largest] = (q[largest] + 255).saturating_sub(total).min(255);
            }
            let (ox, oy) = oct_encode(normals[i]);
            out.extend_from_slice(&[
                positions[i].x,
                positions[i].y,
                positions[i].z,
                makepad_draw::pack_pair_f16(ox, oy),
                makepad_draw::pack_pair_f16(v.uv[0], v.uv[1]),
                makepad_draw::pack_unorm8x4(
                    v.joints[0] as f32 / 255.0,
                    v.joints[1] as f32 / 255.0,
                    v.joints[2] as f32 / 255.0,
                    v.joints[3] as f32 / 255.0,
                ),
                makepad_draw::pack_unorm8x4(
                    q[0] as f32 / 255.0,
                    q[1] as f32 / 255.0,
                    q[2] as f32 / 255.0,
                    q[3] as f32 / 255.0,
                ),
                // unorm16x2, NOT an f16 pair: f16 spacing near 1.0 is a full
                // texel of the atlas (see model::pack_ao_uv).
                crate::model::pack_ao_uv(baked.ao_uv[i][0], baked.ao_uv[i][1]),
            ]);
        }
        SkinRestGpu {
            vertices: out,
            indices,
            source: baked.source_vertex,
            ao_size: atlas.size,
            ao_pixels: atlas.pixels,
            source_hash: self.rest_hash(),
        }
    }

    /// [`Self::rest_gpu`] without the AO bake: same packed layout, no chart
    /// split, every `ao_uv` pointing into a small all-open (white) atlas —
    /// so the shader's per-fragment AO sample reads 1.0 everywhere and the
    /// character renders correctly, just without baked self-occlusion.
    ///
    /// Exists because the bake's cost scales with mesh size and a generated
    /// mesh can be two orders of magnitude denser than the hand-authored
    /// rigs the budget was set against: a 6k-vert KayKit hero bakes in ~6s,
    /// a 412k-vert Trellis character was measured at 20+ MINUTES — an app
    /// launch cannot pay that. Callers that want real AO for a heavy rig
    /// should bake offline/off-thread and cache the bundle
    /// ([`SkinRestGpu::to_bytes`]); this is the instant fallback until that
    /// sidecar exists.
    pub fn rest_gpu_flat(&self) -> SkinRestGpu {
        assert!(self.joint_nodes.len() <= 256, "rig exceeds u8 joint indices");
        let mut out = Vec::with_capacity(self.vertices.len() * SKIN_GPU_VERTEX_FLOATS);
        for v in &self.vertices {
            // Same compensated weight quantization as rest_gpu: the largest
            // weight absorbs the rounding so the four u8s sum to 255.
            let mut q = [0u32; 4];
            let mut total = 0u32;
            for k in 0..4 {
                q[k] = (v.weights[k].clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
                total += q[k];
            }
            if total > 0 {
                let largest = (0..4).max_by_key(|k| q[*k]).unwrap();
                q[largest] = (q[largest] + 255).saturating_sub(total).min(255);
            }
            let (ox, oy) = oct_encode(v.normal);
            out.extend_from_slice(&[
                v.pos.x,
                v.pos.y,
                v.pos.z,
                makepad_draw::pack_pair_f16(ox, oy),
                makepad_draw::pack_pair_f16(v.uv[0], v.uv[1]),
                makepad_draw::pack_unorm8x4(
                    v.joints[0] as f32 / 255.0,
                    v.joints[1] as f32 / 255.0,
                    v.joints[2] as f32 / 255.0,
                    v.joints[3] as f32 / 255.0,
                ),
                makepad_draw::pack_unorm8x4(
                    q[0] as f32 / 255.0,
                    q[1] as f32 / 255.0,
                    q[2] as f32 / 255.0,
                    q[3] as f32 / 255.0,
                ),
                // Every vertex samples the middle of the open atlas.
                crate::model::pack_ao_uv(0.5, 0.5),
            ]);
        }
        SkinRestGpu {
            source: (0..self.vertices.len() as u32).collect(),
            vertices: out,
            indices: self.indices.clone(),
            ao_size: 64,
            ao_pixels: vec![255; 64 * 64],
            source_hash: self.rest_hash(),
        }
    }

    /// Conservative model-space AABB of the mesh under `palette`, from joint
    /// positions ± each joint's rest radius (see `joint_bounds`). This is what
    /// frustum culling uses now that no posed vertices exist on the CPU: it
    /// never under-covers — a blended vertex is a convex combination of
    /// per-joint rigid images, each inside its joint's sphere.
    pub fn posed_bounds(&self, palette: &[Mat4f]) -> Option<(Vec3f, Vec3f)> {
        let mut min = Vec3f { x: f32::MAX, y: f32::MAX, z: f32::MAX };
        let mut max = Vec3f { x: f32::MIN, y: f32::MIN, z: f32::MIN };
        let mut any = false;
        for (j, (bind, radius)) in self.joint_bounds.iter().enumerate() {
            let Some(m) = palette.get(j) else { continue };
            let p = mat4_mul_point(m, *bind);
            // Largest basis-column length: the palette is rigid for these
            // rigs, but a scaling clip must still cull conservatively.
            let col = |a: usize| {
                (m.v[a] * m.v[a] + m.v[a + 1] * m.v[a + 1] + m.v[a + 2] * m.v[a + 2]).sqrt()
            };
            let r = radius * col(0).max(col(4)).max(col(8));
            min.x = min.x.min(p.x - r);
            min.y = min.y.min(p.y - r);
            min.z = min.z.min(p.z - r);
            max.x = max.x.max(p.x + r);
            max.y = max.y.max(p.y + r);
            max.z = max.z.max(p.z + r);
            any = true;
        }
        any.then_some((min, max))
    }
}

/// Clip names that mean "idle", tried in order, matched case-insensitively
/// by substring ([`SkinnedModel::clip_index`]): the vocabulary across the
/// shipped rigs — Kenney mini-characters say `idle`, KayKit heroes
/// `Idle`/`Unarmed_Idle`. One list, used by hosts picking animation clips
/// AND by the offline shadow baker identifying rigs, so both agree on what
/// a rig's idle stance is.
pub const GAIT_IDLE_CLIPS: &[&str] = &["idle", "unarmed_idle", "static"];

/// Clip names that mean "walk" — see [`GAIT_IDLE_CLIPS`]. Ordered best-fit
/// first: a rig with both `walk` and `run` gaits its shadow from the walk.
pub const GAIT_WALK_CLIPS: &[&str] = &["walk", "walking_a", "walking_b", "run", "running_a"];

/// Texels one joint matrix occupies in the palette texture.
pub const PALETTE_TEXELS_PER_JOINT: usize = 3;

/// Flatten a joint palette into RGBA32F texels: [`PALETTE_TEXELS_PER_JOINT`]
/// vec4 rows per joint — the top three ROWS of the column-major matrix — so
/// the vertex shader reconstructs `m * p` as three dot products against
/// `vec4(pos, 1)`.
pub fn palette_texels(palette: &[Mat4f], out: &mut Vec<f32>) {
    for m in palette {
        let v = &m.v;
        out.extend_from_slice(&[
            v[0], v[4], v[8], v[12], //
            v[1], v[5], v[9], v[13], //
            v[2], v[6], v[10], v[14],
        ]);
    }
}

/// Floats per vertex in the packed CPU-skinned stream (`skin_to_packed`).
///
/// MUST equal `model::MODEL_VERTEX_FLOATS`: characters and props share
/// `DrawSceneSkinned`, so they share its vertex layout. Asserted below,
/// because the failure mode is a garbled mesh rather than a compile error.
pub const SKIN_VERTEX_FLOATS: usize = 7;

/// Floats per vertex in the packed GPU-skinning rest stream
/// ([`SkinnedModel::rest_gpu`], `geom.GameMeshVertexSkin`).
pub const SKIN_GPU_VERTEX_FLOATS: usize = 8;

/// AO atlas edge for one rig, texels. 128^2 holds a 1-4k-vert character's
/// charts at crease resolution; `fill` mode scales charts up to spend
/// whatever is left, so smaller rigs get denser texels, not wasted ones.
pub const SKIN_AO_ATLAS: usize = 128;

/// Magic + version for the [`SkinRestGpu`] disk sidecar. Bumped whenever the
/// vertex layout or atlas semantics change, so a stale cache is IGNORED
/// rather than drawn with the wrong stride.
const SKIN_REST_MAGIC: &[u8; 8] = b"SKINAO\x01\x00";

/// One rig's GPU rest bundle: chart-split packed vertices, topology, and the
/// rest-pose AO atlas the `ao_uv` lane indexes. Built by
/// [`SkinnedModel::rest_gpu`]; cacheable on disk via
/// [`Self::to_bytes`]/[`Self::from_bytes`] because the bake costs real time.
pub struct SkinRestGpu {
    /// [`SKIN_GPU_VERTEX_FLOATS`] floats per vertex.
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
    /// For each output vertex, the source [`SkinnedModel`] vertex it came
    /// from — chart seams duplicate vertices, and this is what proves the
    /// duplicates carry identical skinning data.
    pub source: Vec<u32>,
    /// R8 atlas, `ao_size` square, 255 = open.
    pub ao_size: usize,
    pub ao_pixels: Vec<u8>,
    /// [`SkinnedModel::rest_hash`] of the mesh this was baked from.
    pub source_hash: u64,
}

impl SkinRestGpu {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            32 + self.vertices.len() * 4
                + self.indices.len() * 4
                + self.source.len() * 4
                + self.ao_pixels.len(),
        );
        out.extend_from_slice(SKIN_REST_MAGIC);
        out.extend_from_slice(&self.source_hash.to_le_bytes());
        let nverts = self.vertices.len() / SKIN_GPU_VERTEX_FLOATS;
        out.extend_from_slice(&(nverts as u32).to_le_bytes());
        out.extend_from_slice(&(self.indices.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.ao_size as u32).to_le_bytes());
        for f in &self.vertices {
            out.extend_from_slice(&f.to_le_bytes());
        }
        for i in self.indices.iter().chain(self.source.iter()) {
            out.extend_from_slice(&i.to_le_bytes());
        }
        out.extend_from_slice(&self.ao_pixels);
        out
    }

    /// `None` on any mismatch — magic, version, or truncation. The caller
    /// must still compare `source_hash` against the live mesh's
    /// [`SkinnedModel::rest_hash`] before trusting the bundle.
    pub fn from_bytes(bytes: &[u8]) -> Option<SkinRestGpu> {
        let mut at = 0usize;
        let take = |at: &mut usize, n: usize| -> Option<&[u8]> {
            let s = bytes.get(*at..*at + n)?;
            *at += n;
            Some(s)
        };
        if take(&mut at, 8)? != SKIN_REST_MAGIC {
            return None;
        }
        let source_hash = u64::from_le_bytes(take(&mut at, 8)?.try_into().ok()?);
        let nverts = u32::from_le_bytes(take(&mut at, 4)?.try_into().ok()?) as usize;
        let nidx = u32::from_le_bytes(take(&mut at, 4)?.try_into().ok()?) as usize;
        let ao_size = u32::from_le_bytes(take(&mut at, 4)?.try_into().ok()?) as usize;
        if ao_size == 0 || ao_size > 4096 || nverts > 4_000_000 {
            return None;
        }
        let mut vertices = Vec::with_capacity(nverts * SKIN_GPU_VERTEX_FLOATS);
        for _ in 0..nverts * SKIN_GPU_VERTEX_FLOATS {
            vertices.push(f32::from_le_bytes(take(&mut at, 4)?.try_into().ok()?));
        }
        let read_u32s = |at: &mut usize, n: usize| -> Option<Vec<u32>> {
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(u32::from_le_bytes(take(at, 4)?.try_into().ok()?));
            }
            Some(v)
        };
        let indices = read_u32s(&mut at, nidx)?;
        let source = read_u32s(&mut at, nverts)?;
        let ao_pixels = take(&mut at, ao_size * ao_size)?.to_vec();
        if at != bytes.len() {
            return None;
        }
        Some(SkinRestGpu { vertices, indices, source, ao_size, ao_pixels, source_hash })
    }
}

const _: () = assert!(
    SKIN_VERTEX_FLOATS == crate::model::MODEL_VERTEX_FLOATS,
    "skinned and static meshes share one shader and must share its stride"
);

/// Octahedral normal encoding: a unit vector into two components in [-1, 1].
/// Standard sphere->octahedron->square unfolding; ~1 degree of error at f16,
/// far below what flat-shaded game geometry can show.
/// f16-pair unpack, the shader's `unpack2f16` on the CPU (tests/oracles).
#[allow(dead_code)]
pub(crate) fn unpack2f16_pub(f: f32) -> (f32, f32) {
    let half = |h: u32| -> f32 {
        let (s, e, m) = ((h >> 15) & 1, (h >> 10) & 0x1f, h & 0x3ff);
        let v = match e {
            0 => (m as f32) * (-24f32).exp2(),
            0x1f => f32::INFINITY,
            _ => (1.0 + m as f32 / 1024.0) * ((e as i32 - 15) as f32).exp2(),
        };
        if s == 1 { -v } else { v }
    };
    let b = f.to_bits();
    (half(b & 0xffff), half(b >> 16))
}

/// Octahedral decode, the shader's `oct_decode` on the CPU (tests/oracles).
#[allow(dead_code)]
pub(crate) fn oct_decode_pub(ox: f32, oy: f32) -> makepad_draw::makepad_math::Vec3f {
    let nz = 1.0 - ox.abs() - oy.abs();
    let t = (0.0 - nz).max(0.0);
    let sx = if ox >= 0.0 { 1.0 } else { -1.0 };
    let sy = if oy >= 0.0 { 1.0 } else { -1.0 };
    let v = [ox - t * sx, oy - t * sy, nz];
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    makepad_draw::makepad_math::Vec3f {
        x: v[0] / len,
        y: v[1] / len,
        z: v[2] / len,
    }
}

pub(crate) fn oct_encode(n: Vec3f) -> (f32, f32) {
    let l1 = n.x.abs() + n.y.abs() + n.z.abs();
    if l1 < 1.0e-8 {
        return (0.0, 0.0);
    }
    let (x, y, z) = (n.x / l1, n.y / l1, n.z / l1);
    if z >= 0.0 {
        (x, y)
    } else {
        // Fold the lower hemisphere out across the diagonals.
        let sx = if x >= 0.0 { 1.0 } else { -1.0 };
        let sy = if y >= 0.0 { 1.0 } else { -1.0 };
        ((1.0 - y.abs()) * sx, (1.0 - x.abs()) * sy)
    }
}

fn quat_at(values: &[f32], k: usize) -> Quat {
    Quat {
        x: values[k * 4],
        y: values[k * 4 + 1],
        z: values[k * 4 + 2],
        w: values[k * 4 + 3],
    }
}

fn nlerp(a: Quat, mut b: Quat, f: f32) -> Quat {
    if a.dot(b) < 0.0 {
        b = b.neg();
    }
    let q = Quat {
        x: a.x + (b.x - a.x) * f,
        y: a.y + (b.y - a.y) * f,
        z: a.z + (b.z - a.z) * f,
        w: a.w + (b.w - a.w) * f,
    };
    let len = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
    if len > 1.0e-8 {
        Quat {
            x: q.x / len,
            y: q.y / len,
            z: q.z / len,
            w: q.w / len,
        }
    } else {
        Quat::default()
    }
}

// ------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-assembled minimal skinned GLB: 2 joints (root→child), a 2-triangle
    /// quad fully weighted to the child, one 1s clip "spin" rotating the child
    /// 90° about +Z. Keeps parser/sampler tests hermetic — no downloads.
    fn build_test_glb() -> Vec<u8> {
        let mut bin: Vec<u8> = Vec::new();
        let push_f32s = |bin: &mut Vec<u8>, v: &[f32]| {
            let at = bin.len();
            for f in v {
                bin.extend_from_slice(&f.to_le_bytes());
            }
            at
        };
        // POSITION (4 verts)
        let pos_off = push_f32s(
            &mut bin,
            &[1.0, 0.0, 0.0, 2.0, 0.0, 0.0, 1.0, 1.0, 0.0, 2.0, 1.0, 0.0],
        );
        // NORMAL
        let nrm_off = push_f32s(
            &mut bin,
            &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
        );
        // TEXCOORD_0
        let uv_off = push_f32s(&mut bin, &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        // WEIGHTS_0 (all on joint 1)
        let w_off = push_f32s(
            &mut bin,
            &[
                0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0,
            ],
        );
        // JOINTS_0 as u8 vec4 × 4 verts
        let j_off = bin.len();
        bin.extend_from_slice(&[0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0]);
        // indices u16: two triangles
        let i_off = bin.len();
        for i in [0u16, 1, 2, 2, 1, 3] {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        // IBM: two identity mats
        let ibm_off = push_f32s(
            &mut bin,
            &{
                let mut m = Vec::new();
                for _ in 0..2 {
                    m.extend_from_slice(&Mat4f::identity().v);
                }
                m
            },
        );
        // anim: times [0, 0.5, 1.0], rotations identity → 45° → 90° about Z
        let t_off = push_f32s(&mut bin, &[0.0, 0.5, 1.0]);
        let s225 = (std::f32::consts::PI / 8.0).sin();
        let c225 = (std::f32::consts::PI / 8.0).cos();
        let s45 = (std::f32::consts::PI / 4.0).sin();
        let c45 = (std::f32::consts::PI / 4.0).cos();
        let r_off = push_f32s(
            &mut bin,
            &[
                0.0, 0.0, 0.0, 1.0, // identity
                0.0, 0.0, s225, c225, // 45°
                0.0, 0.0, s45, c45, // 90°
            ],
        );
        while bin.len() % 4 != 0 {
            bin.push(0);
        }

        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},
"scenes":[{{"nodes":[0]}}],
"nodes":[
 {{"name":"root","translation":[10,2,0],"children":[1,2]}},
 {{"name":"bone","translation":[0,0,0],"children":[3]}},
 {{"name":"body","mesh":0,"skin":0}},
 {{"name":"handslot.r","translation":[0.25,0.5,0]}}],
"skins":[{{"joints":[0,1],"inverseBindMatrices":6}}],
"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2,"WEIGHTS_0":3,"JOINTS_0":4}},"indices":5}}]}}],
"animations":[{{"name":"spin","samplers":[{{"input":7,"output":8,"interpolation":"LINEAR"}}],"channels":[{{"sampler":0,"target":{{"node":1,"path":"rotation"}}}}]}}],
"accessors":[
 {{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3"}},
 {{"bufferView":1,"componentType":5126,"count":4,"type":"VEC3"}},
 {{"bufferView":2,"componentType":5126,"count":4,"type":"VEC2"}},
 {{"bufferView":3,"componentType":5126,"count":4,"type":"VEC4"}},
 {{"bufferView":4,"componentType":5121,"count":4,"type":"VEC4"}},
 {{"bufferView":5,"componentType":5123,"count":6,"type":"SCALAR"}},
 {{"bufferView":6,"componentType":5126,"count":2,"type":"MAT4"}},
 {{"bufferView":7,"componentType":5126,"count":3,"type":"SCALAR"}},
 {{"bufferView":8,"componentType":5126,"count":3,"type":"VEC4"}}],
"bufferViews":[
 {{"buffer":0,"byteOffset":{pos_off},"byteLength":48}},
 {{"buffer":0,"byteOffset":{nrm_off},"byteLength":48}},
 {{"buffer":0,"byteOffset":{uv_off},"byteLength":32}},
 {{"buffer":0,"byteOffset":{w_off},"byteLength":64}},
 {{"buffer":0,"byteOffset":{j_off},"byteLength":16}},
 {{"buffer":0,"byteOffset":{i_off},"byteLength":12}},
 {{"buffer":0,"byteOffset":{ibm_off},"byteLength":128}},
 {{"buffer":0,"byteOffset":{t_off},"byteLength":12}},
 {{"buffer":0,"byteOffset":{r_off},"byteLength":48}}],
"buffers":[{{"byteLength":{}}}]}}"#,
            bin.len()
        );
        let mut json_bytes = json.into_bytes();
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }

        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json_bytes);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&bin);
        glb
    }

    fn overlay_test_model() -> SkinnedModel {
        let node = |name: &str, parent: Option<usize>| Node {
            name: name.to_string(),
            parent,
            rest: NodeTrs::default(),
        };
        let rotation_keys = {
            let s45 = (std::f32::consts::PI / 4.0).sin();
            let c45 = (std::f32::consts::PI / 4.0).cos();
            vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, s45, c45]
        };
        SkinnedModel {
            nodes: vec![
                node("root", None),
                node("spine", Some(0)),
                node("hand.r", Some(1)),
                node("handslot.r", Some(2)),
                node("leg", Some(0)),
                node("mesh", Some(0)),
            ],
            joint_nodes: Vec::new(),
            inverse_bind: Vec::new(),
            mesh_node: 5,
            vertices: Vec::new(),
            indices: Vec::new(),
            clips: vec![AnimClip {
                name: "UpperBody_Shoot".to_string(),
                duration: 1.0,
                channels: vec![
                    // Authored but outside the upper-body mask.
                    Channel {
                        node: 0,
                        path: ChannelPath::Translation,
                        times: vec![0.0, 1.0],
                        values: vec![100.0, 101.0, 102.0, 200.0, 201.0, 202.0],
                    },
                    Channel {
                        node: 1,
                        path: ChannelPath::Rotation,
                        times: vec![0.0, 1.0],
                        values: rotation_keys,
                    },
                    Channel {
                        node: 2,
                        path: ChannelPath::Translation,
                        times: vec![0.0, 1.0],
                        values: vec![1.0, 2.0, 3.0, 7.0, 8.0, 9.0],
                    },
                    // Authored but outside the upper-body mask.
                    Channel {
                        node: 4,
                        path: ChannelPath::Scale,
                        times: vec![0.0, 1.0],
                        values: vec![2.0, 2.0, 2.0, 3.0, 3.0, 3.0],
                    },
                ],
            }],
            skipped_unskinned: 0,
            joint_bounds: Vec::new(),
            ragdoll: None,
        }
    }

    fn quality_vertex(pos: [f32; 3], joint: u16) -> SkinVertex {
        SkinVertex {
            pos: Vec3f {
                x: pos[0],
                y: pos[1],
                z: pos[2],
            },
            normal: Vec3f {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            uv: [0.0; 2],
            joints: [joint, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        }
    }

    fn semantic_quality_model(scale: f32, bridge: bool) -> SkinnedModel {
        let node = |name: &str| Node {
            name: name.to_string(),
            parent: None,
            rest: NodeTrs::default(),
        };
        SkinnedModel {
            nodes: vec![node("arm"), node("leg"), node("torso"), node("mesh")],
            joint_nodes: vec![0, 1, 2],
            inverse_bind: vec![Mat4f::identity(); 3],
            mesh_node: 3,
            vertices: vec![
                quality_vertex([0.0, 0.0, 0.0], 0),
                quality_vertex([scale, 0.0, 0.0], 0),
                quality_vertex([0.0, scale, 0.0], if bridge { 1 } else { 2 }),
                // Establish character-scale bounds without another face.
                quality_vertex([0.0, 1.0, 0.0], 1),
            ],
            indices: vec![0, 1, 2],
            clips: Vec::new(),
            skipped_unskinned: 0,
            joint_bounds: Vec::new(),
            ragdoll: None,
        }
    }

    fn motion_quality_model(triangle_size: f32, terminal_displacement: f32) -> SkinnedModel {
        let node = |name: &str, parent: Option<usize>| Node {
            name: name.to_string(),
            parent,
            rest: NodeTrs::default(),
        };
        SkinnedModel {
            nodes: vec![
                node("root", None),
                node("moving_limb", Some(0)),
                node("mesh", None),
            ],
            joint_nodes: vec![0, 1],
            inverse_bind: vec![Mat4f::identity(); 2],
            mesh_node: 2,
            vertices: vec![
                quality_vertex([0.0, 0.0, 0.0], 0),
                quality_vertex([triangle_size, 0.0, 0.0], 0),
                quality_vertex([0.0, triangle_size, 0.0], 1),
                // An unreferenced vertex establishes the rest character height.
                quality_vertex([0.0, 1.0, 0.0], 0),
            ],
            indices: vec![0, 1, 2],
            clips: vec![AnimClip {
                name: "terminal_bridge".to_string(),
                duration: 1.0,
                channels: vec![Channel {
                    node: 1,
                    path: ChannelPath::Translation,
                    times: vec![0.0, 1.0],
                    values: vec![
                        0.0,
                        0.0,
                        0.0,
                        terminal_displacement,
                        0.0,
                        0.0,
                    ],
                }],
            }],
            skipped_unskinned: 0,
            joint_bounds: Vec::new(),
            ragdoll: None,
        }
    }

    #[test]
    fn semantic_bridge_audit_accepts_separated_regions_without_mutation() {
        let model = semantic_quality_model(0.1, false);
        let rest_hash = model.rest_hash();
        let indices = model.indices.clone();
        let audit = model.audit_semantic_bridges(&[0], &[1], 0.55).unwrap();
        assert_eq!(audit.bridge_triangles, 0);
        assert_eq!(audit.first_bridge_face, None);
        assert_eq!(model.rest_hash(), rest_hash);
        assert_eq!(model.indices, indices);
    }

    #[test]
    fn semantic_bridge_audit_finds_microscopic_and_visible_cross_limb_faces() {
        for scale in [1.0e-5, 0.1] {
            let model = semantic_quality_model(scale, true);
            let audit = model.audit_semantic_bridges(&[0], &[1], 0.55).unwrap();
            assert_eq!(audit.nondegenerate_triangles, 1, "scale {scale}");
            assert_eq!(audit.bridge_triangles, 1, "scale {scale}");
            assert_eq!(audit.first_bridge_face, Some(0));
            assert_eq!(audit.max_arm_confidence, 1.0);
            assert_eq!(audit.max_leg_confidence, 1.0);
        }
    }

    #[test]
    fn authored_motion_quality_allows_microscopic_high_ratio_extension() {
        let model = motion_quality_model(0.001, 0.006);
        let audit = model.audit_authored_motion_quality().unwrap();
        assert!(audit.max_stretch > 3.0, "{audit:?}");
        assert!(audit.max_extension_height < 0.02, "{audit:?}");
        assert_eq!(audit.bad_triangles, 0, "{audit:?}");
    }

    #[test]
    fn authored_motion_quality_catches_visible_bridge_at_terminal_key() {
        let model = motion_quality_model(0.01, 0.06);
        let rest_hash = model.rest_hash();
        let indices = model.indices.clone();
        let audit = model.audit_authored_motion_quality().unwrap();
        assert_eq!(audit.authored_samples, 2, "{audit:?}");
        assert_eq!(audit.bad_triangles, 1, "{audit:?}");
        assert_eq!(audit.worst_clip, Some(0));
        assert_eq!(audit.worst_authored_frame, Some(1));
        assert_eq!(audit.worst_face, Some(0));
        assert_eq!(audit.worst_time_seconds, 1.0);
        assert_eq!(model.rest_hash(), rest_hash);
        assert_eq!(model.indices, indices);
    }

    fn distinctive_pose(node_count: usize) -> PoseBuffer {
        (0..node_count)
            .map(|index| {
                let n = index as f32;
                NodeTrs {
                    t: Vec3f { x: n + 0.1, y: n + 0.2, z: n + 0.3 },
                    r: Quat { x: n + 0.4, y: n + 0.5, z: n + 0.6, w: n + 0.7 },
                    s: Vec3f { x: n + 0.8, y: n + 0.9, z: n + 1.0 },
                }
            })
            .collect()
    }

    fn trs_bits(trs: &NodeTrs) -> [u32; 10] {
        [
            trs.t.x.to_bits(),
            trs.t.y.to_bits(),
            trs.t.z.to_bits(),
            trs.r.x.to_bits(),
            trs.r.y.to_bits(),
            trs.r.z.to_bits(),
            trs.r.w.to_bits(),
            trs.s.x.to_bits(),
            trs.s.y.to_bits(),
            trs.s.z.to_bits(),
        ]
    }

    #[test]
    fn parses_minimal_skinned_glb() {
        let model = SkinnedModel::parse_glb(&build_test_glb()).unwrap();
        assert_eq!(model.joint_count(), 2);
        assert_eq!(model.vertex_count(), 4);
        assert_eq!(model.indices().len(), 6);
        assert_eq!(model.clips.len(), 1);
        assert_eq!(model.clips[0].name, "spin");
        assert!((model.clips[0].duration - 1.0).abs() < 1.0e-6);
        assert_eq!(model.clip_index("SPIN"), Some(0));
        assert_eq!(model.skipped_unskinned, 0);
    }

    #[test]
    fn exact_clip_lookup_does_not_accept_substrings() {
        let model = overlay_test_model();
        assert_eq!(model.clip_index_exact("UpperBody_Shoot"), Some(0));
        assert_eq!(model.clip_index_exact("upperbody_shoot"), Some(0));
        assert_eq!(model.clip_index_exact("UpperBody"), None);
        assert_eq!(model.clip_index_exact("Shoot"), None);
        assert_eq!(model.clip_index_exact("UpperBody_Shooting"), None);
        assert_eq!(model.clip_index_exact(""), None);
    }

    #[test]
    fn masked_overlay_is_clamped_and_preserves_every_unkeyed_bit() {
        let model = overlay_test_model();
        let spine = model.node_index("spine").unwrap();
        let mask = model.descendant_mask(spine).unwrap();
        assert_eq!(mask, vec![false, true, true, true, false, false]);
        assert!(model.descendant_mask(usize::MAX).is_none());

        let before = distinctive_pose(model.nodes.len());
        let mut at_end = before.clone();
        assert!(model.overlay_clip_masked(0, 1.0, &mask, &mut at_end));
        let mut after_end = before.clone();
        assert!(model.overlay_clip_masked(0, 100.0, &mask, &mut after_end));

        // Clamping holds the terminal action key instead of wrapping to zero.
        for node in 0..model.nodes.len() {
            assert_eq!(trs_bits(&at_end[node]), trs_bits(&after_end[node]));
        }
        assert_eq!(at_end[2].t.x.to_bits(), 7.0f32.to_bits());
        assert_eq!(at_end[2].t.y.to_bits(), 8.0f32.to_bits());
        assert_eq!(at_end[2].t.z.to_bits(), 9.0f32.to_bits());

        // Root/leg have authored channels but are outside the mask. The
        // socket is inside the mask but has no channel. All stay bit-exact.
        for node in [0, 3, 4, 5] {
            assert_eq!(trs_bits(&before[node]), trs_bits(&at_end[node]));
        }
        // Keyed components change; their unkeyed components remain exact.
        assert_ne!(at_end[1].r.w.to_bits(), before[1].r.w.to_bits());
        assert_eq!(at_end[1].t.x.to_bits(), before[1].t.x.to_bits());
        assert_eq!(at_end[1].s.z.to_bits(), before[1].s.z.to_bits());
        assert_ne!(at_end[2].t.x.to_bits(), before[2].t.x.to_bits());
        assert_eq!(at_end[2].r.y.to_bits(), before[2].r.y.to_bits());
        assert_eq!(at_end[2].s.x.to_bits(), before[2].s.x.to_bits());

        // Starting with rest is the explicit rest-fallback path; unkeyed
        // descendants retain their rest transforms.
        let mut over_rest = model.rest_pose();
        let socket_rest = trs_bits(&over_rest[3]);
        assert!(model.overlay_clip_masked(0, 0.5, &mask, &mut over_rest));
        assert_eq!(trs_bits(&over_rest[3]), socket_rest);
    }

    #[test]
    fn masked_overlay_fails_closed_without_mutating_pose() {
        let model = overlay_test_model();
        let mask = model.descendant_mask(model.node_index("spine").unwrap()).unwrap();

        let before = distinctive_pose(model.nodes.len());
        let mut pose = before.clone();
        assert!(!model.overlay_clip_masked(99, 0.5, &mask, &mut pose));
        for node in 0..pose.len() {
            assert_eq!(trs_bits(&pose[node]), trs_bits(&before[node]));
        }

        let mut pose = before.clone();
        assert!(!model.overlay_clip_masked(0, 0.5, &mask[..mask.len() - 1], &mut pose));
        for node in 0..pose.len() {
            assert_eq!(trs_bits(&pose[node]), trs_bits(&before[node]));
        }

        let mut short_pose = before[..before.len() - 1].to_vec();
        let short_before = short_pose.clone();
        assert!(!model.overlay_clip_masked(0, 0.5, &mask, &mut short_pose));
        for node in 0..short_pose.len() {
            assert_eq!(trs_bits(&short_pose[node]), trs_bits(&short_before[node]));
        }
    }

    #[test]
    fn samples_and_skins_rotation_clip() {
        let model = SkinnedModel::parse_glb(&build_test_glb()).unwrap();
        let mut pose = PoseBuffer::new();
        let mut palette = Vec::new();
        let mut verts = Vec::new();

        // t=0: identity — vertex 0 stays at (1,0,0).
        model.sample_clip(0, 0.0, &mut pose);
        model.palette(&pose, &mut palette);
        model.skin_to_packed(&palette, &mut verts);
        assert!((verts[0] - 1.0).abs() < 1.0e-5, "x at rest: {}", verts[0]);
        assert!(verts[1].abs() < 1.0e-5);

        // t just shy of the clip end: 90° about Z — (1,0,0) → (0,1,0).
        // (t=1.0 wraps to 0 by design.)
        model.sample_clip(0, 0.999999, &mut pose);
        model.palette(&pose, &mut palette);
        model.skin_to_packed(&palette, &mut verts);
        assert!(verts[0].abs() < 1.0e-3, "x after spin: {}", verts[0]);
        assert!((verts[1] - 1.0).abs() < 1.0e-3, "y after spin: {}", verts[1]);

        // Midpoint between key 0 and key 1 (t=0.25 → 22.5°).
        model.sample_clip(0, 0.25, &mut pose);
        model.palette(&pose, &mut palette);
        model.skin_to_packed(&palette, &mut verts);
        let angle = verts[1].atan2(verts[0]);
        assert!(
            (angle - std::f32::consts::PI / 8.0).abs() < 1.0e-3,
            "angle {angle}"
        );

        // Blend rest↔sampled at w=0.5 halves the rotation.
        let rest = model.rest_pose();
        let mut spun = PoseBuffer::new();
        model.sample_clip(0, 0.999999, &mut spun);
        let mut blended = PoseBuffer::new();
        SkinnedModel::blend_pose(&rest, &spun, 0.5, &mut blended);
        model.palette(&blended, &mut palette);
        model.skin_to_packed(&palette, &mut verts);
        let angle = verts[1].atan2(verts[0]);
        assert!(
            (angle - std::f32::consts::PI / 4.0).abs() < 1.0e-2,
            "blended angle {angle}"
        );
    }

    #[test]
    fn playable_root_strip_preserves_child_and_rotation() {
        let model = SkinnedModel::parse_glb(&build_test_glb()).unwrap();
        let mut pose = model.rest_pose();
        pose[0].t = Vec3f {
            x: 11.0,
            y: 22.0,
            z: 33.0,
        };
        pose[0].r = Quat {
            x: 0.1,
            y: 0.2,
            z: 0.3,
            w: 0.9,
        };
        pose[1].t = Vec3f {
            x: 4.0,
            y: 5.0,
            z: 6.0,
        };
        let root_rotation = pose[0].r;
        let child_translation = pose[1].t;

        model.strip_skeleton_root_translation(&mut pose);
        assert_eq!(pose[0].t.x.to_bits(), model.nodes[0].rest.t.x.to_bits());
        assert_eq!(pose[0].t.y.to_bits(), model.nodes[0].rest.t.y.to_bits());
        assert_eq!(pose[0].t.z.to_bits(), model.nodes[0].rest.t.z.to_bits());
        assert_eq!(pose[0].r.x.to_bits(), root_rotation.x.to_bits());
        assert_eq!(pose[0].r.w.to_bits(), root_rotation.w.to_bits());
        assert_eq!(pose[1].t.x.to_bits(), child_translation.x.to_bits());
        assert_eq!(pose[1].t.y.to_bits(), child_translation.y.to_bits());
        assert_eq!(pose[1].t.z.to_bits(), child_translation.z.to_bits());
    }

    #[test]
    fn loop_blended_sampler_removes_wrap_snap_without_mutating_keys() {
        let model = SkinnedModel::parse_glb(&build_test_glb()).unwrap();
        let keys_before: Vec<Vec<u32>> = model.clips[0]
            .channels
            .iter()
            .map(|channel| channel.values.iter().map(|value| value.to_bits()).collect())
            .collect();
        let angle = |left: &Quat, right: &Quat| {
            let dot = (left.x * right.x
                + left.y * right.y
                + left.z * right.z
                + left.w * right.w)
                .abs()
                .clamp(-1.0, 1.0);
            2.0 * dot.acos() * 180.0 / std::f32::consts::PI
        };
        let mut raw_tail = PoseBuffer::new();
        let mut raw_head = PoseBuffer::new();
        model.sample_clip(0, 0.9999, &mut raw_tail);
        model.sample_clip(0, 1.0, &mut raw_head);
        assert!(angle(&raw_tail[1].r, &raw_head[1].r) > 89.0);

        let mut blended_tail = PoseBuffer::new();
        let mut blended_head = PoseBuffer::new();
        let mut scratch = PoseBuffer::new();
        model.sample_clip_loop_blended(0, 0.9999, 0.2, &mut blended_tail, &mut scratch);
        model.sample_clip_loop_blended(0, 1.0, 0.2, &mut blended_head, &mut scratch);
        assert!(
            angle(&blended_tail[1].r, &blended_head[1].r) < 0.01,
            "tail must converge continuously onto head"
        );

        // The sampler owns only output buffers; authored key bytes stay exact.
        let keys_after: Vec<Vec<u32>> = model.clips[0]
            .channels
            .iter()
            .map(|channel| channel.values.iter().map(|value| value.to_bits()).collect())
            .collect();
        assert_eq!(keys_before, keys_after);
    }

    #[test]
    fn runtime_loop_audit_crosses_two_smooth_wraps() {
        let model = SkinnedModel::parse_glb(&build_test_glb()).unwrap();
        let authored = model.audit_temporal_motion(0, 60.0);
        assert!(authored.seam_joint_angle_degrees > 80.0);

        let runtime = model.audit_loop_blended_motion(0, 60.0, 0.2, 2);
        assert_eq!(runtime.cycles, 2);
        assert_eq!(runtime.wraps, 2);
        assert_eq!(runtime.frame_pairs, 120);
        assert!(
            runtime.wrap_joint_angle_degrees < authored.seam_joint_angle_degrees * 0.05,
            "loop joint wrap remained visible: {} degrees",
            runtime.wrap_joint_angle_degrees
        );
        assert!(
            runtime.wrap_vertex_delta < authored.seam_vertex_delta * 0.05,
            "loop vertex wrap remained visible: {}",
            runtime.wrap_vertex_delta
        );
        assert!(runtime.max_joint_angle_degrees < 15.0);
    }

    #[test]
    fn clamped_sampler_holds_one_shot_terminal_key() {
        let model = SkinnedModel::parse_glb(&build_test_glb()).unwrap();
        let mut terminal = PoseBuffer::new();
        let mut held = PoseBuffer::new();
        let mut wrapped = PoseBuffer::new();
        model.sample_clip_clamped(0, 1.0, &mut terminal);
        model.sample_clip_clamped(0, 100.0, &mut held);
        model.sample_clip(0, 1.0, &mut wrapped);
        assert_eq!(trs_bits(&terminal[1]), trs_bits(&held[1]));
        assert_ne!(terminal[1].r.z.to_bits(), wrapped[1].r.z.to_bits());
    }

    #[test]
    fn named_node_transform_is_raw_animated_mesh_space() {
        let model = SkinnedModel::parse_glb(&build_test_glb()).unwrap();
        let socket = model.node_index("handslot.r").expect("named socket");
        assert_eq!(model.node_index("missing"), None);
        assert!(model.node_mesh_transform(&Vec::new(), usize::MAX).is_none());

        let mut pose = PoseBuffer::new();
        model.sample_clip(0, 0.0, &mut pose);
        let rest = model.node_mesh_transform(&pose, socket).unwrap();
        let rest_tip = mat4_mul_point(&rest, Vec3f::default());
        // Root carries a large translation shared by the mesh and socket.
        // Mesh-space conversion must cancel it rather than leaking it into
        // every attached prop.
        assert!((rest_tip.x - 0.25).abs() < 1.0e-5);
        assert!((rest_tip.y - 0.5).abs() < 1.0e-5);
        let fallback = model.node_mesh_transform(&Vec::new(), socket).unwrap();
        let fallback_tip = mat4_mul_point(&fallback, Vec3f::default());
        assert!((fallback_tip.x - rest_tip.x).abs() < 1.0e-5);
        assert!((fallback_tip.y - rest_tip.y).abs() < 1.0e-5);

        // The socket is an ordinary child node, not an inverse-bind matrix.
        // Its parent animation rotates the raw attachment point with the hand.
        model.sample_clip(0, 0.999999, &mut pose);
        let turned = model.node_mesh_transform(&pose, socket).unwrap();
        let turned_tip = mat4_mul_point(&turned, Vec3f::default());
        assert!((turned_tip.x + 0.5).abs() < 1.0e-3, "x={}", turned_tip.x);
        assert!((turned_tip.y - 0.25).abs() < 1.0e-3, "y={}", turned_tip.y);
    }

    // ---- CPU references for the shader-side unpack, bit-for-bit ----

    fn unpack4u8(f: f32) -> [f32; 4] {
        let b = f.to_bits();
        [
            (b & 0xff) as f32 / 255.0,
            ((b >> 8) & 0xff) as f32 / 255.0,
            ((b >> 16) & 0xff) as f32 / 255.0,
            ((b >> 24) & 0xff) as f32 / 255.0,
        ]
    }

    fn unpack2f16(f: f32) -> (f32, f32) {
        let half = |h: u32| -> f32 {
            let (s, e, m) = ((h >> 15) & 1, (h >> 10) & 0x1f, h & 0x3ff);
            let v = match e {
                0 => (m as f32) * (-24f32).exp2(),
                0x1f => f32::INFINITY,
                _ => (1.0 + m as f32 / 1024.0) * ((e as i32 - 15) as f32).exp2(),
            };
            if s == 1 {
                -v
            } else {
                v
            }
        };
        let b = f.to_bits();
        (half(b & 0xffff), half(b >> 16))
    }

    /// The exact blend `DrawSceneSkinnedGpu.vertex` performs, on the CPU:
    /// unpack joints/weights from the packed lanes, fetch the three palette
    /// rows per joint from the flat texel stream, accumulate weighted dot
    /// products. Any drift between this and the shader text is a bug in one
    /// of them.
    fn gpu_skin_vertex(rest: &[f32], texels: &[f32], joint_base: usize) -> (Vec3f, Vec3f) {
        let p = [rest[0], rest[1], rest[2], 1.0];
        let (ox, oy) = unpack2f16(rest[3]);
        let n3 = {
            // oct_decode, as in the shader.
            let nz = 1.0 - ox.abs() - oy.abs();
            let t = (0.0 - nz).max(0.0);
            let sx = if ox >= 0.0 { 1.0 } else { -1.0 };
            let sy = if oy >= 0.0 { 1.0 } else { -1.0 };
            let v = [ox - t * sx, oy - t * sy, nz];
            let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / len, v[1] / len, v[2] / len]
        };
        let joints = unpack4u8(rest[5]);
        let weights = unpack4u8(rest[6]);
        let mut pos = [0.0f32; 3];
        let mut nrm = [0.0f32; 3];
        for k in 0..4 {
            let w = weights[k];
            if w == 0.0 {
                continue;
            }
            let j = (joints[k] * 255.0 + 0.5).floor() as usize;
            let base = (joint_base + j * PALETTE_TEXELS_PER_JOINT) * 4;
            for (row, r) in texels[base..base + 12].chunks_exact(4).enumerate() {
                pos[row] += (r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3] * p[3]) * w;
                nrm[row] += (r[0] * n3[0] + r[1] * n3[1] + r[2] * n3[2]) * w;
            }
        }
        (
            Vec3f { x: pos[0], y: pos[1], z: pos[2] },
            Vec3f { x: nrm[0], y: nrm[1], z: nrm[2] },
        )
    }

    /// Two plates, a floor and a lid 0.06 above it, all on joint 0 — the
    /// smallest rig where the rest-pose AO bake must produce contrast: the
    /// floor sees a blocked hemisphere at close range, the lid corners an
    /// open sky. The lid's authored side faces DOWN, at the floor: the
    /// evaluator honours sidedness (a surface occludes what its FRONT
    /// faces, the way a real rig's torso fronts its own armpit), so a lid
    /// authored skyward would read as the floor staring at a backface —
    /// i.e. a sample buried inside geometry — and be rejected, not shaded.
    fn build_plates_glb() -> Vec<u8> {
        let mut bin: Vec<u8> = Vec::new();
        let push_f32s = |bin: &mut Vec<u8>, v: &[f32]| {
            let at = bin.len();
            for f in v {
                bin.extend_from_slice(&f.to_le_bytes());
            }
            at
        };
        // The lid overhangs the floor on every side, so every floor vertex —
        // they are all corners — sees lid, not sky, straight up and around.
        let pos_off = push_f32s(
            &mut bin,
            &[
                0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, // floor
                -1.0, 0.06, -1.0, 2.0, 0.06, -1.0, -1.0, 0.06, 2.0, 2.0, 0.06, 2.0, // lid
            ],
        );
        let nrm_off = push_f32s(&mut bin, &{
            let mut n = Vec::new();
            for _ in 0..4 {
                n.extend_from_slice(&[0.0, 1.0, 0.0]); // floor faces up
            }
            for _ in 0..4 {
                n.extend_from_slice(&[0.0, -1.0, 0.0]); // lid faces DOWN, at it
            }
            n
        });
        let uv_off = push_f32s(&mut bin, &[0.0; 16]);
        let w_off = push_f32s(&mut bin, &{
            let mut w = Vec::new();
            for _ in 0..8 {
                w.extend_from_slice(&[1.0, 0.0, 0.0, 0.0]);
            }
            w
        });
        let j_off = bin.len();
        bin.extend_from_slice(&[0u8; 32]);
        let i_off = bin.len();
        // Winding agrees with the authored facing (floor up, lid down): the
        // evaluators orient by winding after twin dedup, so a fixture whose
        // winding lies about its side would measure the dedup pass, not AO.
        for i in [0u16, 2, 1, 2, 3, 1, 4, 5, 6, 6, 5, 7] {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let ibm_off = push_f32s(&mut bin, &Mat4f::identity().v);
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},
"scenes":[{{"nodes":[0,1]}}],
"nodes":[{{"name":"root"}},{{"name":"body","mesh":0,"skin":0}}],
"skins":[{{"joints":[0],"inverseBindMatrices":6}}],
"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2,"WEIGHTS_0":3,"JOINTS_0":4}},"indices":5}}]}}],
"accessors":[
 {{"bufferView":0,"componentType":5126,"count":8,"type":"VEC3"}},
 {{"bufferView":1,"componentType":5126,"count":8,"type":"VEC3"}},
 {{"bufferView":2,"componentType":5126,"count":8,"type":"VEC2"}},
 {{"bufferView":3,"componentType":5126,"count":8,"type":"VEC4"}},
 {{"bufferView":4,"componentType":5121,"count":8,"type":"VEC4"}},
 {{"bufferView":5,"componentType":5123,"count":12,"type":"SCALAR"}},
 {{"bufferView":6,"componentType":5126,"count":1,"type":"MAT4"}}],
"bufferViews":[
 {{"buffer":0,"byteOffset":{pos_off},"byteLength":96}},
 {{"buffer":0,"byteOffset":{nrm_off},"byteLength":96}},
 {{"buffer":0,"byteOffset":{uv_off},"byteLength":64}},
 {{"buffer":0,"byteOffset":{w_off},"byteLength":128}},
 {{"buffer":0,"byteOffset":{j_off},"byteLength":32}},
 {{"buffer":0,"byteOffset":{i_off},"byteLength":24}},
 {{"buffer":0,"byteOffset":{ibm_off},"byteLength":64}}],
"buffers":[{{"byteLength":{}}}]}}"#,
            bin.len()
        );
        let mut json_bytes = json.into_bytes();
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json_bytes);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&bin);
        glb
    }

    /// The AO the shader will see at output vertex `i`: unpack the shipped
    /// ao_uv lane and read the atlas texel under it.
    fn sample_rig_ao(rest: &SkinRestGpu, i: usize) -> f32 {
        let uv = crate::model::unpack_ao_uv(rest.vertices[i * SKIN_GPU_VERTEX_FLOATS + 7]);
        let x = ((uv[0] * rest.ao_size as f32) as usize).min(rest.ao_size - 1);
        let y = ((uv[1] * rest.ao_size as f32) as usize).min(rest.ao_size - 1);
        rest.ao_pixels[y * rest.ao_size + x] as f32 / 255.0
    }

    #[test]
    fn rest_ao_darkens_the_covered_floor() {
        let model = SkinnedModel::parse_glb(&build_plates_glb()).unwrap();
        let rest = model.rest_gpu();
        let (mut floor, mut nf, mut lid, mut nl) = (0.0f32, 0usize, 0.0f32, 0usize);
        for (i, src) in rest.source.iter().enumerate() {
            let ao = sample_rig_ao(&rest, i);
            if *src < 4 {
                floor += ao;
                nf += 1;
            } else {
                lid += ao;
                nl += 1;
            }
        }
        let (floor, lid) = (floor / nf.max(1) as f32, lid / nl.max(1) as f32);
        assert!(lid > 0.9, "open lid must stay unshaded: {lid}");
        assert!(
            floor < lid - 0.15,
            "covered floor must darken: floor {floor} vs lid {lid}"
        );
    }

    #[test]
    fn rest_gpu_bundle_survives_the_sidecar_roundtrip() {
        let model = SkinnedModel::parse_glb(&build_plates_glb()).unwrap();
        let rest = model.rest_gpu();
        let back = SkinRestGpu::from_bytes(&rest.to_bytes()).expect("roundtrip");
        assert_eq!(back.source_hash, model.rest_hash());
        assert_eq!(back.vertices.len(), rest.vertices.len());
        assert!(back.vertices.iter().zip(&rest.vertices).all(|(a, b)| a.to_bits() == b.to_bits()));
        assert_eq!(back.indices, rest.indices);
        assert_eq!(back.source, rest.source);
        assert_eq!(back.ao_size, rest.ao_size);
        assert_eq!(back.ao_pixels, rest.ao_pixels);
        // A stale or foreign file must be rejected, not drawn.
        assert!(SkinRestGpu::from_bytes(&rest.to_bytes()[..40]).is_none());
        let mut wrong = rest.to_bytes();
        wrong[6] = 9; // version byte
        assert!(SkinRestGpu::from_bytes(&wrong).is_none());
    }

    #[test]
    fn gpu_rest_pack_roundtrips_joints_and_weights() {
        let model = SkinnedModel::parse_glb(&build_test_glb()).unwrap();
        let rest = model.rest_gpu();
        // Chart splitting may grow the vertex count, never shrink it, and
        // every output vertex must name a real source.
        assert!(rest.vertices.len() >= model.vertex_count() * SKIN_GPU_VERTEX_FLOATS);
        assert_eq!(rest.vertices.len(), rest.source.len() * SKIN_GPU_VERTEX_FLOATS);
        for (src, packed) in rest
            .source
            .iter()
            .zip(rest.vertices.chunks_exact(SKIN_GPU_VERTEX_FLOATS))
        {
            let v = &model.vertices[*src as usize];
            // Split copies must keep the exact source position.
            assert_eq!(packed[0], v.pos.x);
            assert_eq!(packed[1], v.pos.y);
            assert_eq!(packed[2], v.pos.z);
            let joints = unpack4u8(packed[5]);
            let weights = unpack4u8(packed[6]);
            let mut sum = 0u32;
            for k in 0..4 {
                let j = (joints[k] * 255.0 + 0.5).floor() as u16;
                if weights[k] > 0.0 {
                    assert_eq!(j, v.joints[k], "joint index must survive the u8 lane");
                }
                assert!(
                    (weights[k] - v.weights[k]).abs() <= 1.5 / 255.0,
                    "weight drifted: {} vs {}",
                    weights[k],
                    v.weights[k]
                );
                sum += (weights[k] * 255.0 + 0.5).floor() as u32;
            }
            assert_eq!(sum, 255, "quantized weights must stay affine");
            let ao_uv = crate::model::unpack_ao_uv(packed[7]);
            assert!((0.0..=1.0).contains(&ao_uv[0]) && (0.0..=1.0).contains(&ao_uv[1]));
        }
    }

    #[test]
    fn gpu_skin_formula_matches_cpu_skinning() {
        let model = SkinnedModel::parse_glb(&build_test_glb()).unwrap();
        let rest = model.rest_gpu();
        let mut pose = PoseBuffer::new();
        let mut palette = Vec::new();
        let mut cpu = Vec::new();
        let mut texels = Vec::new();
        for t in [0.0, 0.25, 0.6, 0.999999] {
            model.sample_clip(0, t, &mut pose);
            model.palette(&pose, &mut palette);
            model.skin_to_packed(&palette, &mut cpu);
            texels.clear();
            // Non-zero base: the shader offsets into a shared frame texture.
            texels.extend_from_slice(&[0.0; 8]);
            palette_texels(&palette, &mut texels);
            for (out_i, rv) in rest.vertices.chunks_exact(SKIN_GPU_VERTEX_FLOATS).enumerate() {
                // A chart-split duplicate must blend to the exact position
                // its source vertex does — the split may never move a vertex.
                let i = rest.source[out_i] as usize;
                let (pos, nrm) = gpu_skin_vertex(rv, &texels, 2);
                let c = &cpu[i * SKIN_VERTEX_FLOATS..];
                assert!(
                    (pos.x - c[0]).abs() < 1.0e-4
                        && (pos.y - c[1]).abs() < 1.0e-4
                        && (pos.z - c[2]).abs() < 1.0e-4,
                    "t={t} vertex {i}: gpu {:?} cpu ({}, {}, {})",
                    (pos.x, pos.y, pos.z),
                    c[0],
                    c[1],
                    c[2]
                );
                // Normals: compare directions after the same normalize.
                let (cx, cy) = unpack2f16(c[3]);
                let nz = 1.0 - cx.abs() - cy.abs();
                let tt = (0.0 - nz).max(0.0);
                let cd = [
                    cx - tt * if cx >= 0.0 { 1.0 } else { -1.0 },
                    cy - tt * if cy >= 0.0 { 1.0 } else { -1.0 },
                    nz,
                ];
                let cl = (cd[0] * cd[0] + cd[1] * cd[1] + cd[2] * cd[2]).sqrt();
                let nl = (nrm.x * nrm.x + nrm.y * nrm.y + nrm.z * nrm.z).sqrt();
                let dot =
                    (nrm.x * cd[0] + nrm.y * cd[1] + nrm.z * cd[2]) / (cl * nl).max(1.0e-8);
                assert!(dot > 0.995, "t={t} vertex {i}: normal diverged (dot {dot})");
            }
        }
    }

    #[test]
    fn posed_bounds_contain_every_skinned_vertex() {
        let model = SkinnedModel::parse_glb(&build_test_glb()).unwrap();
        let mut pose = PoseBuffer::new();
        let mut palette = Vec::new();
        let mut cpu = Vec::new();
        for t in [0.0, 0.3, 0.7] {
            model.sample_clip(0, t, &mut pose);
            model.palette(&pose, &mut palette);
            model.skin_to_packed(&palette, &mut cpu);
            let (min, max) = model.posed_bounds(&palette).expect("bounds");
            for v in cpu.chunks_exact(SKIN_VERTEX_FLOATS) {
                for (a, (lo, hi)) in [
                    (v[0], (min.x, max.x)),
                    (v[1], (min.y, max.y)),
                    (v[2], (min.z, max.z)),
                ] {
                    assert!(a >= lo - 1.0e-4 && a <= hi + 1.0e-4, "t={t}: {a} outside {lo}..{hi}");
                }
            }
        }
    }

    #[test]
    fn parses_vendored_knight_if_present() {
        // Real-asset smoke: skips (with a hint) when the download hasn't run.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/sandbox/resources/characters/knight.glb"
        );
        let Ok(bytes) = std::fs::read(path) else {
            println!("SKIP: knight.glb absent — run apps/sandbox/download_assets.sh");
            return;
        };
        let model = SkinnedModel::parse_glb(&bytes).expect("knight.glb must parse");
        assert!(model.joint_count() > 10, "joints: {}", model.joint_count());
        assert!(model.vertex_count() > 500, "verts: {}", model.vertex_count());
        assert!(!model.clips.is_empty());
        assert!(
            model.clip_index("idle").is_some(),
            "clips: {:?}",
            model.clips.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
        assert!(model.clip_index("walk").is_some());
        // Every joint index used by a vertex must be inside the palette.
        let jc = model.joint_count() as u16;
        for v in &model.vertices {
            for (j, w) in v.joints.iter().zip(v.weights) {
                assert!(w == 0.0 || *j < jc, "joint {j} out of range");
            }
        }
        // GPU path on the real 41-joint rig: the shader formula must track
        // the CPU skin through a multi-influence pose, and the joint-sphere
        // bounds must cover it. Tolerance is the u8 weight step scaled by
        // the rig's size.
        let rest = model.rest_gpu();
        // Rest-pose AO must show real contrast on a real character: open
        // surfaces (helmet crown, pauldrons) stay near 1 while crevices
        // (armpits, under the skirt, between the legs) drop well below.
        // Sampled through the shipped ao_uv lane, exactly as the shader will.
        let out_count = rest.source.len();
        let (mut ao_min, mut ao_max, mut shaded) = (1.0f32, 0.0f32, 0usize);
        for i in 0..out_count {
            let ao = sample_rig_ao(&rest, i);
            ao_min = ao_min.min(ao);
            ao_max = ao_max.max(ao);
            if ao < 0.9 {
                shaded += 1;
            }
        }
        assert!(ao_max > 0.97, "some surface must be fully open: max {ao_max}");
        assert!(ao_min < 0.7, "some crevice must darken: min {ao_min}");
        assert!(
            shaded * 50 > out_count,
            "at least 2% of a real rig sits in crevices: {shaded} of {out_count}"
        );
        let mut pose = PoseBuffer::new();
        let mut palette = Vec::new();
        let mut cpu = Vec::new();
        let walk = model.clip_index("walk").unwrap();
        model.sample_clip(walk, 0.4, &mut pose);
        model.palette(&pose, &mut palette);
        model.skin_to_packed(&palette, &mut cpu);
        let mut texels = Vec::new();
        palette_texels(&palette, &mut texels);
        let (min, max) = model.posed_bounds(&palette).expect("bounds");
        for (out_i, rv) in rest.vertices.chunks_exact(SKIN_GPU_VERTEX_FLOATS).enumerate() {
            let i = rest.source[out_i] as usize;
            let (pos, _) = gpu_skin_vertex(rv, &texels, 0);
            let c = &cpu[i * SKIN_VERTEX_FLOATS..];
            assert!(
                (pos.x - c[0]).abs() < 2.0e-2
                    && (pos.y - c[1]).abs() < 2.0e-2
                    && (pos.z - c[2]).abs() < 2.0e-2,
                "vertex {i}: gpu ({}, {}, {}) cpu ({}, {}, {})",
                pos.x,
                pos.y,
                pos.z,
                c[0],
                c[1],
                c[2]
            );
            for (a, (lo, hi)) in [
                (c[0], (min.x, max.x)),
                (c[1], (min.y, max.y)),
                (c[2], (min.z, max.z)),
            ] {
                assert!(a >= lo - 1.0e-3 && a <= hi + 1.0e-3, "{a} outside {lo}..{hi}");
            }
        }
    }
}
