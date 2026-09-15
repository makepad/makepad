//! Translation-only rest skeletons in model metres (Y up). A joint owns the
//! segment from its parent to itself; roots own a point. Selected segments
//! receive inverse squared distance weights, softened by a capsule radius.
//! This is a constrained geometric binder, not anatomical auto-rigging.
use super::*;

const MAX_JOINTS: usize = 64;
const MAX_CLIPS: usize = 16;
const MAX_KEYS: usize = 4096;
const MAX_CHANNEL_KEYS: usize = 128;

#[derive(Clone, Debug)]
struct Joint {
    name: String,
    parent: Option<String>,
    pos: [f64; 3],
}

#[derive(Clone, Debug)]
enum Binding {
    Auto { joints: Vec<String>, radius: f64 },
    Exact(Vec<(String, f32)>),
}

#[derive(Clone, Debug)]
struct Channel {
    joint: String,
    axis: usize,
    keys: Vec<[f32; 2]>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct RigDraft {
    joints: Vec<Joint>,
    bindings: Vec<(String, Binding)>,
    clips: Vec<(String, Vec<Channel>)>,
}

fn invalid(message: impl Into<String>) -> CsgError { CsgError::Invalid(message.into()) }

impl RigDraft {
    fn joint(&self, name: &str) -> Result<usize, CsgError> {
        self.joints.iter().position(|j| j.name == name)
            .ok_or_else(|| invalid(format!("rig: missing joint '{name}'")))
    }

    pub(super) fn validate(&self, parts: &[Part]) -> Result<(), CsgError> {
        if self.joints.is_empty() {
            return if self.bindings.is_empty() && self.clips.is_empty() { Ok(()) }
                else { Err(invalid("rig: bind/clip requires csg.joint declarations")) };
        }
        for joint in &self.joints {
            let mut path = vec![joint.name.as_str()];
            let mut parent = joint.parent.as_deref();
            while let Some(name) = parent {
                if path.contains(&name) {
                    path.push(name);
                    return Err(invalid(format!("rig: joint cycle {}", path.join(" -> "))));
                }
                path.push(name);
                parent = self.joints[self.joint(name)?].parent.as_deref();
            }
        }
        for (name, binding) in &self.bindings {
            if !parts.iter().any(|p| p.name == *name) {
                return Err(invalid(format!("rig: binding references missing part '{name}'")));
            }
            match binding {
                Binding::Auto { joints, .. } => for name in joints { self.joint(name)?; },
                Binding::Exact(weights) => for (name, _) in weights { self.joint(name)?; },
            }
        }
        for part in parts {
            if !self.bindings.iter().any(|(name, _)| *name == part.name) {
                return Err(invalid(format!("rig: part '{}' needs csg.bind (use rigid for shells)", part.name)));
            }
            if part.animation.is_some() || part.parent.is_some() || part.pivot.is_some() {
                return Err(invalid(format!("rig: part '{}' uses rigid parent/pivot/anim; use joints, bind and clip instead", part.name)));
            }
            if part.color.iter().any(|c| !c.is_finite()) || part.color[3] != 1.0 {
                return Err(invalid("rig: this slice requires finite opaque part colors"));
            }
        }
        for (_, channels) in &self.clips {
            for channel in channels { self.joint(&channel.joint)?; }
        }
        Ok(())
    }
}

// Rig options are fail-closed: typos must never become plausible defaults.
fn strict_keys(vm: &mut ScriptVm, o: ScriptObject, allowed: &[LiveId]) -> Result<(), CsgError> {
    for index in 0..vm.bx.heap.iter_len(o) {
        let kv = vm.bx.heap.iter_key_value(o, index, NoTrap);
        if let Some(key) = kv.key.as_id() {
            if !allowed.contains(&key) { return Err(invalid(format!("rig: unknown option '{key}'"))); }
        }
    }
    Ok(())
}

fn name(vm: &mut ScriptVm, value: ScriptValue) -> Result<String, CsgError> {
    let name = value_string(vm, value);
    if !valid_name(&name) { return Err(invalid("rig: names must match [a-z0-9-]{1,24}")); }
    Ok(name)
}

pub(super) fn c_joint(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let result = (|| {
        let n = arg(vm, args, 0);
        let name = name(vm, n)?;
        let o = opts(vm, state, args, 1, "joint").ok_or_else(|| invalid("csg.joint: options required"))?;
        strict_keys(vm, o, &[id!(pos), id!(parent)])?;
        let p = field(vm, o, id!(pos));
        let pos = vec3(vm, p).filter(|v| v.iter().all(|x| x.is_finite() && x.abs() <= 50.0))
            .ok_or_else(|| invalid("csg.joint: pos must be finite model-space vec3 within +/-50 metres"))?;
        let p = field(vm, o, id!(parent));
        let parent = if p.is_nil() { None } else { Some(self::name(vm, p)?) };
        let mut state = state.borrow_mut();
        if state.rig.joints.len() >= MAX_JOINTS { return Err(invalid("rig: maximum 64 joints")); }
        if state.rig.joints.iter().any(|j| j.name == name) { return Err(invalid(format!("rig: duplicate joint '{name}'"))); }
        state.rig.joints.push(Joint { name, pos, parent });
        Ok(())
    })();
    if let Err(error) = result { return state.borrow_mut().fail(error.to_string()); }
    NIL
}

pub(super) fn c_bind(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let result = (|| {
        let n = arg(vm, args, 0);
        let part = name(vm, n)?;
        let o = opts(vm, state, args, 1, "bind").ok_or_else(|| invalid("csg.bind: options required"))?;
        strict_keys(vm, o, &[id!(joints), id!(radius), id!(rigid), id!(weights)])?;
        let rigid = field(vm, o, id!(rigid));
        let weights = field(vm, o, id!(weights));
        let selected = field(vm, o, id!(joints));
        let radius = field(vm, o, id!(radius));
        let modes = [!rigid.is_nil(), !weights.is_nil(), !selected.is_nil()].into_iter().filter(|v| *v).count();
        if modes != 1 { return Err(invalid("csg.bind: choose exactly one of joints, rigid, weights; a later exact bind overrides auto")); }
        if selected.is_nil() && !radius.is_nil() { return Err(invalid("csg.bind: radius is only for automatic joints")); }
        let binding = if !rigid.is_nil() {
            Binding::Exact(vec![(name(vm, rigid)?, 1.0)])
        } else if !weights.is_nil() {
            let count = list_len(vm, weights);
            if !(1..=4).contains(&count) { return Err(invalid("csg.bind: exact weights need 1..4 influences")); }
            let mut entries = Vec::new();
            for i in 0..count {
                let item = list_value(vm, weights, i).as_object().ok_or_else(|| invalid("csg.bind: weights entries need {joint, weight}"))?;
                strict_keys(vm, item, &[id!(joint), id!(weight)])?;
                let j = field(vm, item, id!(joint));
                let j = name(vm, j)?;
                let w = option_f64(vm, item, id!(weight), f64::NAN);
                if !w.is_finite() || !(0.0..=1.0).contains(&w) || entries.iter().any(|(n, _)| *n == j) {
                    return Err(invalid("csg.bind: unique joints and finite nonnegative weights required"));
                }
                entries.push((j, w as f32));
            }
            let sum: f32 = entries.iter().map(|(_, w)| *w).sum();
            if (sum - 1.0).abs() > 1e-6 { return Err(invalid("csg.bind: exact weights must sum to one")); }
            for (_, w) in &mut entries { *w /= sum; }
            Binding::Exact(entries)
        } else {
            let count = list_len(vm, selected);
            if !(1..=MAX_JOINTS).contains(&count) { return Err(invalid("csg.bind: select 1..64 joints")); }
            let mut joints = Vec::new();
            for i in 0..count {
                let v = list_value(vm, selected, i);
                let n = name(vm, v)?;
                if joints.contains(&n) { return Err(invalid(format!("csg.bind: duplicate selected joint '{n}'"))); }
                joints.push(n);
            }
            let radius = option_f64(vm, o, id!(radius), 0.1);
            if !dimension(radius) { return Err(invalid("csg.bind: radius must be 0.001..50 metres")); }
            Binding::Auto { joints, radius }
        };
        let mut state = state.borrow_mut();
        if let Some((_, old)) = state.rig.bindings.iter_mut().find(|(n, _)| *n == part) {
            // Automatic rebinding can never erase an authored override.
            if !matches!((&*old, &binding), (Binding::Exact(_), Binding::Auto { .. })) { *old = binding; }
        } else {
            if state.rig.bindings.len() >= state.budgets.max_parts { return Err(invalid("rig: binding part budget exceeded")); }
            state.rig.bindings.push((part, binding));
        }
        Ok(())
    })();
    if let Err(error) = result { return state.borrow_mut().fail(error.to_string()); }
    NIL
}

pub(super) fn c_clip(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let result = (|| {
        let n = arg(vm, args, 0);
        let clip = name(vm, n)?;
        let list = arg(vm, args, 1);
        let count = list_len(vm, list);
        if !(1..=MAX_JOINTS).contains(&count) { return Err(invalid("csg.clip: expected 1..64 joint channels")); }
        let mut channels: Vec<Channel> = Vec::new();
        for i in 0..count {
            let o = list_value(vm, list, i).as_object().ok_or_else(|| invalid("csg.clip: expected {joint, axis, keys}"))?;
            strict_keys(vm, o, &[id!(joint), id!(axis), id!(keys)])?;
            let j = field(vm, o, id!(joint));
            let joint = name(vm, j)?;
            if channels.iter().any(|c| c.joint == joint) { return Err(invalid(format!("csg.clip: duplicate channel for '{joint}'"))); }
            let a = field(vm, o, id!(axis));
            let axis = match value_string(vm, a).as_str() {
                "x" => 0, "y" => 1, "z" => 2, _ => return Err(invalid("csg.clip: axis must be x, y or z")),
            };
            let list = field(vm, o, id!(keys));
            let count = list_len(vm, list);
            if !(2..=MAX_CHANNEL_KEYS).contains(&count) { return Err(invalid("csg.clip: expected 2..128 vec2(seconds,degrees) keys")); }
            let mut keys: Vec<[f32; 2]> = Vec::new();
            for k in 0..count {
                let v = list_value(vm, list, k);
                let key = vec2(vm, v).ok_or_else(|| invalid("csg.clip: key must be vec2(seconds,degrees)"))?;
                let key = [key[0] as f32, key[1] as f32];
                if key.iter().any(|v| !v.is_finite()) || !(0.0..=60.0).contains(&key[0]) || key[1].abs() > 180.0
                    || (k == 0 && key[0] != 0.0)
                    || keys.last().is_some_and(|prev| key[0] <= prev[0] || (key[1] - prev[1]).abs() > 180.0) {
                    return Err(invalid("csg.clip: finite keys, start 0, increasing times <=60s, degrees +/-180 and steps <=180 required"));
                }
                keys.push(key);
            }
            if channels.first().is_some_and(|c| c.keys.last().unwrap()[0] != keys.last().unwrap()[0]) {
                return Err(invalid("csg.clip: channels must end at the same time"));
            }
            channels.push(Channel { joint, axis, keys });
        }
        let mut state = state.borrow_mut();
        if state.rig.clips.len() >= MAX_CLIPS { return Err(invalid("rig: maximum 16 clips")); }
        if state.rig.clips.iter().any(|(n, _)| *n == clip) { return Err(invalid(format!("rig: duplicate clip '{clip}'"))); }
        let keys: usize = state.rig.clips.iter().flat_map(|(_, c)| c).chain(channels.iter()).map(|c| c.keys.len()).sum();
        if keys > MAX_KEYS { return Err(invalid("rig: maximum 4096 total keys")); }
        state.rig.clips.push((clip, channels));
        Ok(())
    })();
    if let Err(error) = result { return state.borrow_mut().fail(error.to_string()); }
    NIL
}

/// Validated binding in flattened part/vertex order. All heavy work is done
/// by `mesh_document` on the existing worker, before final publication.
#[derive(Clone, Debug)]
pub struct MeshedRig {
    draft: RigDraft,
    joints_0: Vec<[u16; 4]>,
    weights_0: Vec<[f32; 4]>,
}

fn distance_squared(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let ab = std::array::from_fn::<_, 3, _>(|i| b[i] - a[i]);
    let length: f64 = ab.iter().map(|x| x * x).sum();
    let dot: f64 = (0..3).map(|i| (p[i] - a[i]) * ab[i]).sum();
    let t = if length == 0.0 { 0.0 } else { (dot / length).clamp(0.0, 1.0) };
    (0..3).map(|i| (p[i] - a[i] - t * ab[i]).powi(2)).sum()
}

pub(super) fn bind_document(document: &CsgDocument, model: &MeshedModel) -> Result<Option<MeshedRig>, CsgError> {
    let draft = &document.rig;
    if draft.joints.is_empty() { return Ok(None); }
    let mut rig = MeshedRig { draft: draft.clone(), joints_0: Vec::new(), weights_0: Vec::new() };
    for part in &model.parts {
        let binding = &draft.bindings.iter().find(|(n, _)| *n == part.name).ok_or_else(|| invalid("rig: missing binding"))?.1;
        let mut candidates = Vec::new();
        if let Binding::Auto { joints, .. } = binding {
            for name in joints {
                let i = draft.joint(name)?;
                let joint = &draft.joints[i];
                let a = match &joint.parent { Some(p) => draft.joints[draft.joint(p)?].pos, None => joint.pos };
                candidates.push((i, a, joint.pos));
            }
        }
        let mut scores = Vec::with_capacity(MAX_JOINTS);
        for (vi, point) in part.mesh.vertices.iter().enumerate() {
            if vi % 256 == 0 { check_running(document)?; }
            let p = [point.x, point.y, point.z];
            if p.iter().any(|v| !v.is_finite() || v.abs() > 50.0) { return Err(invalid("rig: mesh must be finite within +/-50 model metres")); }
            scores.clear();
            match binding {
                Binding::Auto { radius, .. } => {
                    for &(i, a, b) in &candidates {
                        scores.push((i, 1.0 / (distance_squared(p, a, b) + radius * radius)));
                    }
                    // Equal distances use declaration order, independent of
                    // selected-list order. Only selected joints participate.
                    scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                    scores.truncate(4);
                }
                Binding::Exact(weights) => for (name, w) in weights { scores.push((draft.joint(name)?, *w as f64)); },
            }
            let sum: f64 = scores.iter().map(|(_, w)| *w).sum();
            let mut joints = [0; 4];
            let mut weights = [0.0; 4];
            for (k, &(i, w)) in scores.iter().enumerate() { joints[k] = i as u16; weights[k] = (w / sum) as f32; }
            if weights.iter().any(|w| !w.is_finite() || *w < 0.0) || (weights.iter().sum::<f32>() - 1.0).abs() > 1e-6 {
                return Err(invalid("rig: invalid computed weights"));
            }
            rig.joints_0.push(joints);
            rig.weights_0.push(weights);
        }
    }
    Ok(Some(rig))
}

impl MeshedRig {
    pub fn joint_count(&self) -> usize { self.draft.joints.len() }
    pub fn clip_names(&self) -> impl Iterator<Item = &str> { self.draft.clips.iter().map(|(n, _)| n.as_str()) }
    pub fn influences(&self) -> impl Iterator<Item = (&[u16; 4], &[f32; 4])> { self.joints_0.iter().zip(&self.weights_0) }

    /// Existing skeleton/clip writer, with a constant UV per part pointing
    /// at its own palette texel. No vertex-color dependency in the renderer.
    pub fn to_glb(&self, model: &MeshedModel) -> Result<Vec<u8>, CsgError> {
        use makepad_gltf::{GlbJoint, GlbAnimClip, GlbAnimChannel, GlbAnimPath, GlbSkinnedMesh};
        use makepad_zune_png::{PngEncoder, makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions}};
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        let mut uvs = Vec::new();
        // Power-of-two palette: texel centers are exactly representable by
        // the renderer's packed f16 UVs; bilinear sampling cannot bleed.
        let width = model.parts.len().max(1).next_power_of_two();
        let mut colors = vec![255; width * 4];
        for (pi, part) in model.parts.iter().enumerate() {
            let offset = positions.len() as u32;
            for p in &part.mesh.vertices {
                positions.push([p.x as f32, p.y as f32, p.z as f32]);
                uvs.push([(pi as f32 + 0.5) / width as f32, 0.5]);
            }
            indices.extend(part.mesh.triangles.iter().flatten().map(|i| offset + i));
            for c in 0..4 { colors[pi * 4 + c] = (part.color[c].clamp(0.0, 1.0) * 255.0).round() as u8; }
        }
        if positions.len() != self.joints_0.len() { return Err(invalid("rig: model changed after binding")); }
        let options = EncoderOptions::default().set_width(width).set_height(1).set_depth(BitDepth::Eight).set_colorspace(ColorSpace::RGBA);
        let mut png = Vec::new();
        PngEncoder::new(&colors, options).encode(&mut png).map_err(|e| invalid(format!("rig palette: {e:?}")))?;
        let mut joints = Vec::new();
        for j in &self.draft.joints {
            let parent = j.parent.as_ref().map(|p| self.draft.joint(p)).transpose()?;
            // Quantize globals before subtracting, matching inverse binds.
            let global = j.pos.map(|v| v as f32);
            let local = std::array::from_fn(|i| global[i] - parent.map_or(0.0, |p| self.draft.joints[p].pos[i] as f32));
            joints.push(GlbJoint::at(&j.name, parent, local, global));
        }
        let mut clips = Vec::new();
        for (name, source) in &self.draft.clips {
            let mut channels = Vec::new();
            for c in source {
                let mut values = Vec::new();
                for key in &c.keys {
                    let half = key[1].to_radians() * 0.5;
                    let mut q = [0.0, 0.0, 0.0, half.cos()];
                    q[c.axis] = half.sin();
                    values.extend(q);
                }
                channels.push(GlbAnimChannel { joint: self.draft.joint(&c.joint)?, path: GlbAnimPath::Rotation, times: c.keys.iter().map(|k| k[0]).collect(), values });
            }
            clips.push(GlbAnimClip { name: name.clone(), channels });
        }
        Ok(makepad_gltf::write_glb_mesh_skinned(&GlbSkinnedMesh {
            positions: &positions, normals: None, uvs: Some(&uvs), indices: &indices,
            joints_0: &self.joints_0, weights_0: &self.weights_0, joints: &joints, clips: &clips, base_color_png: Some(&png),
        }))
    }
}
