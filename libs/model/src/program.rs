//! Whole-model programs: an ordered list of typed program operations, as
//! built by a front end such as the Splash `game.model_new` builder, applied
//! to a fresh document and compiled once. A program is pure data, so the same
//! list always yields the same model and is cached by the hash of its
//! canonical encoding ([`program_bytes`]). Besides every ordinary document
//! [`Operation`], a program has a few macro operations ([`ProgramOp`]).
#[path = "program_weathering.rs"]
mod weathering;
use crate::{canon::Writer, character::{CharacterSpec, FpsArms}, transform::*, Attachment, CompiledModel, Document, Error, Limits, Operation, RigOperation, SceneOperation, Socket, Transaction, WeightEdit};

/// One program step.
#[derive(Clone, Debug)]
pub enum ProgramOp {
    /// An ordinary document operation.
    Op(Operation),
    /// The humanoid template skeleton, its procedural clips (`idle`/`walk`/`run` when
    /// `clips` is None) and, with `sockets`, the `hand.r`/`hand.l` sockets.
    HumanoidRig { height: f64, clips: Option<Vec<String>>, sockets: bool },
    /// A whole generated character (see `character.rs`): skinned body, face rig, hair,
    /// clothing, gear, sockets and clip set.
    Character(Box<CharacterSpec>),
    /// First-person arms for a character spec and a hold.
    FpsArms(Box<CharacterSpec>, FpsArms),
    /// Rigidly binds an object to a joint by name.
    Bind { object: String, joint: String },
    /// Rigidly binds every still-unweighted object to the joint whose bone passes
    /// closest to the object's centre.
    BindAll,
    /// Bakes ambient occlusion and convex-edge wear into vertex colours (values are
    /// clamped to their ranges when applied).
    Weathering { ao: f64, distance: f64, edges: f64, samples: u32, objects: Option<Vec<String>> },
    /// Geometry, materials, skin and clips of a GLB the host resolves by alias.
    ImportGlb { alias: String, prefix: String, material_base: u32, transform: Option<Transform> },
    /// A request for a far stand-in, read by the build host (the program itself skips it).
    Impostor { distance: f64, cell: u32 },
    /// A mesh primitive, made when the program builds (not when it is written).
    Mesh(MeshPrimitive),
    /// A material layer of an image the host resolves by name when the program builds
    /// (`fetch`: PNG bytes), e.g. a generated texture the document keeps.
    ImageLayer { material: u32, channel: crate::SurfaceChannel, layer: String, image: String },
    /// A joint added under `parent` (by name) at a parent-local offset: the skeleton
    /// grows by one, after whatever rig the program made so far (a humanoid, a
    /// soft body's attachment frames).
    Joint { name: String, parent: String, translation: [f64; 3] },
    /// A rig control whose joint indices index `joints`, names the build resolves
    /// against the skeleton as it stands when the control runs.
    Rig { joints: Vec<String>, op: RigOperation },
    /// A keyframed clip on named joints: each channel's joint indexes `joints`, and a
    /// translation key is an offset from that joint's rest. Joints a clip leaves out
    /// keep their rest (a soft body's solver-owned frames stay unkeyed).
    Clip { joints: Vec<String>, clip: crate::AnimationClip },
}

/// Mesh primitives a program describes and the build makes ([`Operation::sphere`] ...).
#[derive(Clone, Debug, PartialEq)]
pub enum MeshPrimitive {
    Sphere { object: String, radius: f64, segments: u32, rings: u32, smooth: bool },
    Cylinder { object: String, radius: f64, height: f64, segments: u32, smooth: bool },
    Polygons { object: String, positions: Vec<[f64; 3]>, polygons: Vec<crate::mesh::Polygon> },
}
impl MeshPrimitive {
    fn operation(&self, limits: &Limits) -> crate::Result<Operation> {
        match self {
            Self::Sphere { object, radius, segments, rings, smooth } => Operation::sphere(object.clone(), *radius, *segments, *rings, *smooth, limits),
            Self::Cylinder { object, radius, height, segments, smooth } => Operation::cylinder(object.clone(), *radius, *height, *segments, *smooth, limits),
            Self::Polygons { object, positions, polygons } => Operation::polygon_mesh(object.clone(), positions, polygons, limits),
        }
    }
    fn write(&self, w: &mut Writer) -> crate::Result<()> {
        match self {
            Self::Sphere { object, radius, segments, rings, smooth } => { w.u8(0)?; w.string(object)?; w.f64(*radius)?; w.u32(*segments)?; w.u32(*rings)?; w.u8(*smooth as u8) }
            Self::Cylinder { object, radius, height, segments, smooth } => { w.u8(1)?; w.string(object)?; w.f64(*radius)?; w.f64(*height)?; w.u32(*segments)?; w.u8(*smooth as u8) }
            Self::Polygons { object, positions, polygons } => {
                w.u8(2)?; w.string(object)?;
                w.count(positions.len())?; for p in positions { for v in p { w.f64(*v)?; } }
                w.count(polygons.len())?;
                for p in polygons {
                    w.count(p.vertices.len())?; for v in &p.vertices { w.u32(*v)?; }
                    w.count(p.uvs.len())?; for uv in &p.uvs { w.f64(uv[0])?; w.f64(uv[1])?; }
                    w.u32(p.material)?;
                }
                Ok(())
            }
        }
    }
}

impl ProgramOp {
    /// A short name for failure messages.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Op(op) => op.family_name(),
            Self::HumanoidRig { .. } => "humanoid_rig",
            Self::Character(_) => "character",
            Self::FpsArms(..) => "fps_arms",
            Self::Bind { .. } => "bind",
            Self::BindAll => "bind_all",
            Self::Weathering { .. } => "weathering",
            Self::ImportGlb { .. } => "import_glb",
            Self::Impostor { .. } => "impostor",
            Self::Mesh(_) => "mesh",
            Self::ImageLayer { .. } => "image_layer",
            Self::Joint { .. } => "joint",
            Self::Rig { .. } => "rig",
            Self::Clip { .. } => "clip",
        }
    }
    fn write(&self, w: &mut Writer) -> crate::Result<()> {
        let strings = |w: &mut Writer, v: &Option<Vec<String>>| -> crate::Result<()> {
            match v { None => w.u8(0), Some(v) => { w.u8(1)?; w.count(v.len())?; for s in v { w.string(s)?; } Ok(()) } }
        };
        match self {
            Self::Op(op) => { w.u8(0)?; op.write(w) }
            Self::HumanoidRig { height, clips, sockets } => { w.u8(1)?; w.f64(*height)?; strings(w, clips)?; w.u8(*sockets as u8) }
            Self::Character(spec) => { w.u8(2)?; spec.write(w) }
            Self::FpsArms(spec, arms) => { w.u8(3)?; spec.write(w)?; arms.write(w) }
            Self::Bind { object, joint } => { w.u8(4)?; w.string(object)?; w.string(joint) }
            Self::BindAll => w.u8(5),
            Self::Weathering { ao, distance, edges, samples, objects } => { w.u8(6)?; w.f64(*ao)?; w.f64(*distance)?; w.f64(*edges)?; w.u32(*samples)?; strings(w, objects) }
            Self::ImportGlb { alias, prefix, material_base, transform } => {
                w.u8(7)?; w.string(alias)?; w.string(prefix)?; w.u32(*material_base)?;
                match transform { None => w.u8(0), Some(t) => { w.u8(1)?; for v in t.translation.iter().chain(&t.rotation).chain(&t.scale) { w.f64(*v)?; } Ok(()) } }
            }
            Self::Impostor { distance, cell } => { w.u8(8)?; w.f64(*distance)?; w.u32(*cell) }
            Self::Mesh(m) => { w.u8(9)?; m.write(w) }
            Self::ImageLayer { material, channel, layer, image } => { w.u8(10)?; w.u32(*material)?; w.u8(*channel as u8)?; w.string(layer)?; w.string(image) }
            Self::Joint { name, parent, translation } => { w.u8(11)?; w.string(name)?; w.string(parent)?; for v in translation { w.f64(*v)?; } Ok(()) }
            Self::Rig { joints, op } => { w.u8(12)?; w.count(joints.len())?; for j in joints { w.string(j)?; } op.write(w) }
            Self::Clip { joints, clip } => { w.u8(13)?; w.count(joints.len())?; for j in joints { w.string(j)?; } Operation::SetClip { clip: clip.clone() }.write(w) }
        }
    }
}

/// The canonical encoding of a program: equal programs, equal bytes (the build cache key).
pub fn program_bytes(ops: &[ProgramOp]) -> crate::Result<Vec<u8>> {
    let mut w = Writer::new(usize::MAX);
    w.raw(b"MPPROG\0\x01")?;
    w.count(ops.len())?;
    for op in ops { op.write(&mut w)?; }
    Ok(w.bytes)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProgramFailure { pub index: usize, pub op: String, pub message: String }
impl std::fmt::Display for ProgramFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "operation {} ({}): {}", self.index, self.op, self.message) }
}
pub struct BuiltProgram { pub document: Document, pub compiled: CompiledModel, pub warnings: Vec<String> }

/// Readable text for engine errors, for authors rather than engine tests.
pub fn describe_error(error: &Error) -> String {
    match error {
        Error::Invalid(s) | Error::Corrupt(s) => (*s).into(),
        Error::Budget(s) => format!("over budget: {s}"),
        Error::MissingObject(n) => format!("unknown object '{n}'"),
        Error::DuplicateObject(n) => format!("object '{n}' already exists"),
        Error::Mesh(e) => format!("{e:?}"),
        other => format!("{other:?}"),
    }
}

const CHUNK: usize = 64;

struct Runner<'a> {
    doc: Document,
    limits: Limits,
    pending: Vec<(usize, Operation)>,
    serial: u64,
    cancelled: Option<&'a dyn Fn() -> bool>,
}
impl Runner<'_> {
    fn flush(&mut self, names: &[String]) -> Result<(), ProgramFailure> {
        while !self.pending.is_empty() {
            let take = self.pending.len().min(CHUNK.min(self.limits.max_operations));
            let batch: Vec<(usize, Operation)> = self.pending.drain(..take).collect();
            if self.apply(batch.iter().map(|(_, op)| op.clone()).collect()).is_err() {
                // Replay one by one to attribute the failure to its operation.
                for (index, op) in batch {
                    if let Err(error) = self.apply(vec![op]) {
                        return Err(ProgramFailure { index, op: names.get(index).cloned().unwrap_or_default(), message: describe_error(&error) });
                    }
                }
            }
            let head = self.doc.head();
            self.doc.checkpoint(head).map_err(|e| ProgramFailure { index: 0, op: "checkpoint".into(), message: describe_error(&e) })?;
        }
        Ok(())
    }
    fn apply(&mut self, operations: Vec<Operation>) -> crate::Result<()> {
        self.serial += 1;
        self.doc.apply(Transaction { request_id: format!("program-{}", self.serial), expected: self.doc.head(), operations }, self.cancelled).map(|_| ())
    }
}

fn joint_world(doc: &Document) -> Vec<(String, Option<usize>, [f64; 3])> {
    let Some(skeleton) = doc.skeleton() else { return Vec::new() };
    let mut out: Vec<(String, Option<usize>, [f64; 3])> = Vec::new();
    for j in &skeleton.joints {
        let parent = j.parent.map(|p| p as usize);
        let base = parent.and_then(|p| out.get(p)).map_or([0.; 3], |p| p.2);
        out.push((j.name.clone(), parent, add(base, j.translation)));
    }
    out
}
fn segment_distance(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let ab = sub(b, a); let l = dot(ab, ab);
    let t = if l <= 1e-12 { 0. } else { (dot(sub(p, a), ab) / l).clamp(0., 1.) };
    length(sub(p, add(a, mul(ab, t))))
}
fn weight_edit(object: &str, joint: usize) -> Operation {
    Operation::Rig(RigOperation::Weights { object: object.into(), vertices: Vec::new(), edit: WeightEdit::Assign { joint: joint as u32 } })
}

/// Builds and compiles a program. `fetch` resolves `import_glb` aliases to
/// GLB bytes and `ImageLayer` images to PNG bytes (it runs on the calling worker; the host decides where bytes
/// come from). Nothing here blocks on anything but that callback.
pub fn build_program(ops: &[ProgramOp], limits: Limits, fetch: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>, cancelled: Option<&dyn Fn() -> bool>) -> Result<BuiltProgram, ProgramFailure> {
    let names: Vec<String> = ops.iter().map(|op| op.name().to_string()).collect();
    let fail = |index: usize, message: String| ProgramFailure { index, op: names.get(index).cloned().unwrap_or_default(), message };
    let doc = Document::new(limits.clone()).map_err(|e| fail(0, describe_error(&e)))?;
    let mut run = Runner { doc, limits: limits.clone(), pending: Vec::new(), serial: 0, cancelled };
    let mut warnings = Vec::new();
    let mut bound = std::collections::BTreeSet::new();
    for (index, op) in ops.iter().enumerate() {
        if cancelled.is_some_and(|c| c()) { return Err(fail(index, "cancelled".into())); }
        match op {
            ProgramOp::Op(op) => run.pending.push((index, op.clone())),
            ProgramOp::Impostor { .. } => {}
            ProgramOp::Mesh(m) => run.pending.push((index, m.operation(&run.limits).map_err(|e| fail(index, describe_error(&e)))?)),
            ProgramOp::ImageLayer { material, channel, layer, image } => {
                use crate::{SurfaceChannel as Ch, SurfaceOperation as S};
                let bytes = fetch(image).map_err(|e| fail(index, format!("image '{image}': {e}")))?;
                let pixels = crate::RgbaImage::from_png(&bytes, &run.limits).map_err(|e| fail(index, format!("image '{image}': {}", describe_error(&e))))?;
                // The image carries the channel: its factor goes neutral (white base colour
                // keeping alpha, full roughness and metalness, white emission).
                run.flush(&names)?;
                let mut factors = match run.doc.surface().materials.get(material) {
                    Some(m) => m.clone(),
                    None => {
                        let legacy = run.doc.materials().get(material).ok_or_else(|| fail(index, format!("image '{image}': material {material} is not declared")))?;
                        crate::SurfaceMaterial::from_legacy(legacy, &run.limits).map_err(|e| fail(index, describe_error(&e)))?
                    }
                };
                factors.channels.clear();
                match channel {
                    Ch::BaseColor => factors.base_color = [1.0, 1.0, 1.0, factors.base_color[3]],
                    Ch::MetallicRoughness => { factors.roughness = 1.0; factors.metallic = 1.0; }
                    Ch::Emissive => { factors.emissive = [1.0; 3]; factors.emissive_strength = factors.emissive_strength.max(1.0); }
                    Ch::Normal | Ch::Occlusion => {}
                }
                let layer = crate::SurfaceLayer::new(layer.clone(), pixels);
                run.pending.push((index, Operation::Surface(S::Material { material: *material, value: factors })));
                run.pending.push((index, Operation::Surface(S::Layer { material: *material, channel: *channel, layer })));
            }
            ProgramOp::HumanoidRig { height, clips, sockets } => {
                let height = *height;
                if !(0.2..=20.).contains(&height) { return Err(fail(index, "humanoid height must be 0.2..20 m".into())); }
                run.pending.push((index, Operation::SetSkeleton { skeleton: crate::templates::humanoid_skeleton(height) }));
                let clips: Vec<String> = clips.clone().unwrap_or_else(|| crate::templates::HUMANOID_CLIPS.iter().map(|s| s.to_string()).collect());
                for clip in clips {
                    let clip = crate::templates::humanoid_clip(&clip, height).ok_or_else(|| fail(index, format!("unknown humanoid clip '{clip}' ({}, {})", crate::templates::HUMANOID_CLIPS.join(", "), crate::templates::HUMANOID_ACTION_CLIPS.join(", "))))?;
                    run.pending.push((index, Operation::SetClip { clip }));
                }
                if *sockets {
                    let joints = crate::templates::humanoid_joints(height);
                    for (socket, joint) in [("hand.r", "hand_r"), ("hand.l", "hand_l")] {
                        let j = joints.iter().position(|x| x.0 == joint).unwrap();
                        let transform = Transform { translation: [0., -0.045 * height, 0.], rotation: [0., 0., 0., 1.], scale: [1.; 3] };
                        run.pending.push((index, Operation::Scene(SceneOperation::Socket(Socket { name: socket.into(), attachment: Attachment::Joint(j as u32), transform }))));
                    }
                }
            }
            ProgramOp::Character(spec) => {
                let built = crate::character::build_from_spec(spec, &run.limits).map_err(|e| fail(index, describe_error(&e)))?;
                warnings.extend(built.warnings);
                run.pending.extend(built.early.into_iter().chain(built.operations).chain(built.late).map(|o| (index, o)));
            }
            ProgramOp::FpsArms(spec, arms) => {
                let built = crate::character::build_fps_arms(spec, arms, &run.limits).map_err(|e| fail(index, describe_error(&e)))?;
                warnings.extend(built.warnings);
                run.pending.extend(built.early.into_iter().chain(built.operations).chain(built.late).map(|o| (index, o)));
            }
            ProgramOp::Bind { object, joint } => {
                run.flush(&names)?;
                let joints = joint_world(&run.doc);
                if joints.is_empty() { return Err(fail(index, "bind needs a skeleton first (humanoid_rig or skeleton)".into())); }
                let j = joints.iter().position(|x| x.0 == *joint).ok_or_else(|| fail(index, format!("unknown joint '{joint}'")))?;
                run.pending.push((index, weight_edit(object, j)));
                bound.insert(object.clone());
            }
            ProgramOp::BindAll => {
                run.flush(&names)?;
                let joints = joint_world(&run.doc);
                if joints.is_empty() { return Err(fail(index, "bind_all needs a skeleton first".into())); }
                let mut edits = Vec::new();
                for (name, mesh) in run.doc.objects() {
                    if bound.contains(name) || mesh.vertices().iter().any(|v| !v.weights.is_empty()) || mesh.vertices().is_empty() { continue; }
                    let world = run.doc.scene().world_matrix(name).unwrap_or(IDENTITY_MATRIX);
                    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                    for vtx in mesh.vertices() { let p = transform_point(world, vtx.position); for d in 0..3 { lo[d] = lo[d].min(p[d]); hi[d] = hi[d].max(p[d]); } }
                    let c = mul(add(lo, hi), 0.5);
                    // A bone runs from a joint to each child; leaves are points.
                    let best = (0..joints.len()).min_by(|&a, &b| {
                        let d = |i: usize| { let children: Vec<_> = joints.iter().filter(|j| j.1 == Some(i)).collect();
                            if children.is_empty() { length(sub(c, joints[i].2)) } else { children.iter().map(|ch| segment_distance(c, joints[i].2, ch.2)).fold(f64::INFINITY, f64::min) } };
                        d(a).total_cmp(&d(b))
                    }).unwrap();
                    edits.push(weight_edit(name, best));
                }
                run.pending.extend(edits.into_iter().map(|e| (index, e)));
            }
            ProgramOp::Weathering { ao, distance, edges, samples, objects } => {
                run.flush(&names)?;
                let (ao, distance, edges, samples) = (ao.clamp(0., 1.), distance.clamp(0.005, 10.), edges.clamp(0., 1.), (*samples).clamp(1, 64) as usize);
                for op in weathering::weathering_ops(&run.doc, objects.as_deref(), ao, distance, edges, samples) {
                    run.pending.push((index, op));
                }
            }
            ProgramOp::Joint { name, parent, translation } => {
                run.flush(&names)?;
                let mut skeleton = run.doc.skeleton().cloned().ok_or_else(|| fail(index, format!("joint '{name}' needs a skeleton first (rig_humanoid, character or soft_body)")))?;
                if skeleton.joints.iter().any(|j| j.name == *name) { return Err(fail(index, format!("joint '{name}' already exists"))); }
                let p = skeleton.joints.iter().position(|j| j.name == *parent).ok_or_else(|| fail(index, format!("joint '{name}': unknown parent joint '{parent}'")))?;
                skeleton.joints.push(crate::Joint { name: name.clone(), parent: Some(p as u32), translation: *translation });
                run.pending.push((index, Operation::SetSkeleton { skeleton }));
            }
            ProgramOp::Rig { joints, op } => {
                run.flush(&names)?;
                let skeleton = run.doc.skeleton().ok_or_else(|| fail(index, "rig controls need a skeleton first (rig_humanoid, character or soft_body)".into()))?;
                let mut resolved = Vec::with_capacity(joints.len());
                for j in joints {
                    let at = skeleton.joints.iter().position(|x| x.name == *j).ok_or_else(|| fail(index, format!("unknown joint '{j}'")))?;
                    resolved.push(at as u32);
                }
                let op = op.map_joints(&|i| resolved.get(i as usize).copied().ok_or(Error::Invalid("rig joint outside the program's joint table"))).map_err(|e| fail(index, describe_error(&e)))?;
                run.pending.push((index, Operation::Rig(op)));
            }
            ProgramOp::Clip { joints, clip } => {
                run.flush(&names)?;
                let skeleton = run.doc.skeleton().ok_or_else(|| fail(index, format!("clip '{}' needs a skeleton first", clip.name)))?;
                let mut clip = clip.clone();
                for channel in &mut clip.channels {
                    let name = joints.get(channel.joint as usize).ok_or_else(|| fail(index, "clip joint outside the program's joint table".into()))?;
                    let at = skeleton.joints.iter().position(|x| x.name == *name).ok_or_else(|| fail(index, format!("clip '{}': unknown joint '{name}'", clip.name)))?;
                    if channel.path == crate::AnimationPath::Translation {
                        let rest = skeleton.joints[at].translation;
                        for key in &mut channel.keys { for d in 0..3 { key.value[d] += rest[d]; } }
                    }
                    channel.joint = at as u32;
                }
                run.pending.push((index, Operation::SetClip { clip }));
            }
            ProgramOp::ImportGlb { alias, prefix, material_base, transform } => {
                let bytes = fetch(alias).map_err(|e| fail(index, format!("import_glb '{alias}': {e}")))?;
                let root = match transform { Some(t) => t.matrix().map_err(|e| fail(index, describe_error(&e)))?, None => IDENTITY_MATRIX };
                let imported = crate::import::import_glb(&bytes, prefix, *material_base, root, &run.limits).map_err(|e| fail(index, describe_error(&e)))?;
                warnings.extend(imported.warnings);
                run.pending.extend(imported.operations.into_iter().map(|o| (index, o)));
            }
        }
    }
    run.flush(&names)?;
    let compiled = run.doc.compile(cancelled).map_err(|e| ProgramFailure { index: ops.len(), op: "compile".into(), message: describe_error(&e) })?;
    Ok(BuiltProgram { document: run.doc, compiled, warnings })
}
