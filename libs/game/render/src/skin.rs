//! Skinned character loading + animation (M1c, game.md).
//!
//! Parses a GLB (binary glTF v2) with skins and animation clips into a
//! [`SkinnedModel`], samples/blends clips into a [`PoseBuffer`], computes the
//! joint palette, and CPU-skins the mesh into the PbrVertex float layout the
//! renderer already draws (16 floats/vertex). Skinning is Derived-tier
//! presentation: exact IEEE ops only (lerp/nlerp/sqrt — no libm calls).
//!
//! Scope (KayKit/Blender-style exports): GLB container only, dense accessors,
//! JOINTS_0 u8/u16, WEIGHTS_0 f32/normalized u8/u16, TRS node transforms,
//! linear (or step) samplers. Unskinned primitives (hand props) are counted
//! and skipped. Materials are ignored — the caller binds its own texture.

use makepad_draw::makepad_math::{Mat4f, Quat, Vec3f};

// ------------------------------------------------------------------- JSON

/// Minimal owned JSON value — glTF headers are small (a few hundred KB max),
/// clarity beats speed here.
enum Val {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Val>),
    Obj(Vec<(String, Val)>),
}

impl Val {
    fn get(&self, key: &str) -> Option<&Val> {
        match self {
            Val::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn idx(&self, i: usize) -> Option<&Val> {
        match self {
            Val::Arr(items) => items.get(i),
            _ => None,
        }
    }
    fn arr(&self) -> &[Val] {
        match self {
            Val::Arr(items) => items,
            _ => &[],
        }
    }
    fn f64(&self) -> Option<f64> {
        match self {
            Val::Num(n) => Some(*n),
            _ => None,
        }
    }
    fn usize(&self) -> Option<usize> {
        self.f64().map(|n| n as usize)
    }
    fn str(&self) -> Option<&str> {
        match self {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Val, String> {
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

struct Node {
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
}

fn trs_to_mat4(trs: &NodeTrs) -> Mat4f {
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

fn mat4_mul_point(m: &Mat4f, p: Vec3f) -> Vec3f {
    Vec3f {
        x: m.v[0] * p.x + m.v[4] * p.y + m.v[8] * p.z + m.v[12],
        y: m.v[1] * p.x + m.v[5] * p.y + m.v[9] * p.z + m.v[13],
        z: m.v[2] * p.x + m.v[6] * p.y + m.v[10] * p.z + m.v[14],
    }
}

fn mat4_mul_dir(m: &Mat4f, p: Vec3f) -> Vec3f {
    Vec3f {
        x: m.v[0] * p.x + m.v[4] * p.y + m.v[8] * p.z,
        y: m.v[1] * p.x + m.v[5] * p.y + m.v[9] * p.z,
        z: m.v[2] * p.x + m.v[6] * p.y + m.v[10] * p.z,
    }
}

// -------------------------------------------------------------- accessors

struct Accessors<'a> {
    json: &'a Val,
    bin: &'a [u8],
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
    fn read_f32(&self, index: usize) -> Result<(Vec<f32>, usize), String> {
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
                Node { parent: None, rest }
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

        Ok(SkinnedModel {
            nodes,
            joint_nodes,
            inverse_bind,
            mesh_node,
            vertices,
            indices,
            clips,
            skipped_unskinned,
        })
    }

    pub fn joint_count(&self) -> usize {
        self.joint_nodes.len()
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

    pub fn rest_pose(&self) -> PoseBuffer {
        self.nodes.iter().map(|n| n.rest).collect()
    }

    /// Sample `clip` at time `t` (wrapping) over the rest pose.
    pub fn sample_clip(&self, clip: usize, t: f32, out: &mut PoseBuffer) {
        out.clear();
        out.extend(self.nodes.iter().map(|n| n.rest));
        let Some(clip) = self.clips.get(clip) else { return };
        // Wrap with exact ops (floor is IEEE-exact).
        let t = t - (t / clip.duration).floor() * clip.duration;
        for ch in &clip.channels {
            if ch.node >= out.len() || ch.times.is_empty() {
                continue;
            }
            // Key pair straddling t + interpolation factor.
            let (k0, k1, f) = match ch.times.iter().position(|kt| *kt > t) {
                Some(0) => (0, 0, 0.0),
                None => (ch.times.len() - 1, ch.times.len() - 1, 0.0),
                Some(k) => {
                    let (t0, t1) = (ch.times[k - 1], ch.times[k]);
                    let span = t1 - t0;
                    (k - 1, k, if span > 0.0 { (t - t0) / span } else { 0.0 })
                }
            };
            let trs = &mut out[ch.node];
            match ch.path {
                ChannelPath::Translation | ChannelPath::Scale => {
                    let a = &ch.values[k0 * 3..k0 * 3 + 3];
                    let b = &ch.values[k1 * 3..k1 * 3 + 3];
                    let v = Vec3f {
                        x: a[0] + (b[0] - a[0]) * f,
                        y: a[1] + (b[1] - a[1]) * f,
                        z: a[2] + (b[2] - a[2]) * f,
                    };
                    if ch.path == ChannelPath::Translation {
                        trs.t = v;
                    } else {
                        trs.s = v;
                    }
                }
                ChannelPath::Rotation => {
                    let a = quat_at(&ch.values, k0);
                    let b = quat_at(&ch.values, k1);
                    trs.r = nlerp(a, b, f);
                }
            }
        }
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

    /// CPU-skin into the PbrVertex float layout (16 floats/vertex:
    /// pos+nx, ny+nz+uv, color, tangent) — same layout the terrain mesh uses.
    /// Skin the mesh into the packed `geom.GameMeshVertex` layout.
    ///
    /// 6 floats per vertex, not PbrVertex's 16. This is the single largest
    /// vertex stream in the app and, because skinning runs on the CPU, it is
    /// re-uploaded in full every frame — so the saving is per frame, not per
    /// load. Position stays f32; the normal is octahedral in two f16 lanes
    /// and the uv is another f16 pair.
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
            ]);
        }
    }
}

/// Floats per vertex in the packed skinned stream.
pub const SKIN_VERTEX_FLOATS: usize = 6;

/// Octahedral normal encoding: a unit vector into two components in [-1, 1].
/// Standard sphere->octahedron->square unfolding; ~1 degree of error at f16,
/// far below what flat-shaded game geometry can show.
fn oct_encode(n: Vec3f) -> (f32, f32) {
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
"scenes":[{{"nodes":[0,2]}}],
"nodes":[
 {{"name":"root","children":[1]}},
 {{"name":"bone","translation":[0,0,0]}},
 {{"name":"body","mesh":0,"skin":0}}],
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
    fn parses_vendored_knight_if_present() {
        // Real-asset smoke: skips (with a hint) when the download hasn't run.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/arcade/resources/characters/knight.glb"
        );
        let Ok(bytes) = std::fs::read(path) else {
            println!("SKIP: knight.glb absent — run apps/arcade/download_assets.sh");
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
    }
}
