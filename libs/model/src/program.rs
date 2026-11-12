//! Whole-model programs: an ordered list of typed program operations, as
//! built by a front end such as the Splash `game.model_new` builder, applied
//! to a fresh document and compiled once. A program is pure data, so the same
//! list always yields the same model and is cached by the hash of its
//! canonical encoding ([`program_bytes`]). Besides every ordinary document
//! [`Operation`], a program has a few macro operations ([`ProgramOp`]).
#[path = "program_weathering.rs"]
mod weathering;
use crate::{canon::Writer, character::{CharacterSpec, FpsArms}, transform::*, Attachment, CompiledModel, Document, Error, Limits, Operation, RigOperation, SceneOperation, Socket, WeightEdit, Workspace};
use std::{cell::Cell, collections::BTreeMap, time::Duration};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use std::time::Instant as ProgramInstant;

// The standard library has no clock on wasm. Every Makepad web thread
// provides this monotonic host clock (also used by the git and box3d crates).
#[cfg(target_arch = "wasm32")]
pub(crate) struct ProgramInstant(f64);
#[cfg(target_arch = "wasm32")]
impl ProgramInstant {
    pub(crate) fn now() -> Self {
        #[link(wasm_import_module = "env")]
        extern "C" { fn js_monotonic_now() -> f64; }
        // SAFETY: the host import takes no arguments and returns seconds.
        Self(unsafe { js_monotonic_now() })
    }
    pub(crate) fn elapsed(&self) -> Duration {
        Duration::from_secs_f64((Self::now().0 - self.0).max(0.0))
    }
}

thread_local! {
    // Build and export run on the calling worker. A cumulative counter lets
    // callers account for texture work without changing transaction batches.
    static TEXTURE_TIME: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}
pub(crate) fn texture_time() -> Duration { TEXTURE_TIME.with(Cell::get) }
pub(crate) fn record_texture_time(elapsed: Duration) {
    TEXTURE_TIME.with(|time| time.set(time.get() + elapsed));
}

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
    Polygons { object: String, positions: Vec<[f64; 3]>, polygons: Vec<crate::mesh::Polygon>, normals: Vec<[f64; 3]>, colors: Vec<[f64; 4]> },
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PrimitiveKey {
    Sphere { radius: u64, segments: u32, rings: u32, smooth: bool },
    Cylinder { radius: u64, height: u64, segments: u32, smooth: bool },
}
struct CachedPrimitive { remaining: usize, source: Option<Vec<u8>> }
impl MeshPrimitive {
    fn cache_key(&self) -> Option<(PrimitiveKey, &str)> {
        match self {
            Self::Sphere { object, radius, segments, rings, smooth } => Some((PrimitiveKey::Sphere { radius: radius.to_bits(), segments: *segments, rings: *rings, smooth: *smooth }, object)),
            Self::Cylinder { object, radius, height, segments, smooth } => Some((PrimitiveKey::Cylinder { radius: radius.to_bits(), height: height.to_bits(), segments: *segments, smooth: *smooth }, object)),
            Self::Polygons { .. } => None,
        }
    }
    fn cached_operations(&self, limits: &Limits, cache: &mut BTreeMap<PrimitiveKey, CachedPrimitive>) -> crate::Result<Vec<Operation>> {
        let Some((key, object)) = self.cache_key() else { return self.operations(limits) };
        let Some(cached) = cache.get_mut(&key) else { return self.operations(limits) };
        cached.remaining -= 1;
        if let Some(source) = &mut cached.source {
            // Move the last use out so the cache does not retain completed inputs.
            let source = if cached.remaining == 0 { std::mem::take(source) } else { source.clone() };
            return Ok(vec![Operation::ImportMesh { object: object.into(), source }]);
        }
        let ops = self.operations(limits)?;
        if let Some(Operation::ImportMesh { source, .. }) = ops.first() { cached.source = Some(source.clone()); }
        Ok(ops)
    }
    fn operations(&self, limits: &Limits) -> crate::Result<Vec<Operation>> {
        let op = match self {
            Self::Sphere { object, radius, segments, rings, smooth } => Operation::sphere(object.clone(), *radius, *segments, *rings, *smooth, limits),
            Self::Cylinder { object, radius, height, segments, smooth } => Operation::cylinder(object.clone(), *radius, *height, *segments, *smooth, limits),
            Self::Polygons { object, positions, polygons, normals, colors } => {
                if normals.is_empty() && colors.is_empty() {
                    return Ok(vec![Operation::polygon_mesh(object.clone(), positions, polygons, limits)?]);
                }
                for (attribute, count) in [("normals", normals.len()), ("colors", colors.len())] {
                    if count != 0 && count != positions.len() {
                        return Err(Error::Mesh(crate::mesh::MeshError::InvalidInput(format!("mesh '{object}': {attribute} needs one value per vertex ({count} for {} vertices)", positions.len()))));
                    }
                }
                let mut ctx = crate::mesh::Context::new(limits.mesh.clone(), None);
                let mut mesh = crate::ops::polygon_mesh(positions, polygons, &mut ctx)?;
                if !normals.is_empty() {
                    // Polygon order is corner order, so shared vertices receive the
                    // same authored normal without changing topology or UV seams.
                    let values = mesh.corners().iter().zip(polygons.iter().flat_map(|p| &p.vertices))
                        .map(|(corner, &vertex)| (corner.id, Some(normals[vertex as usize]))).collect::<Vec<_>>();
                    mesh.set_corner_normals_bulk(&values, &mut ctx)?;
                }
                let mut ops = vec![Operation::ImportMesh { object: object.clone(), source: mesh.to_bytes(&mut ctx)? }];
                if !colors.is_empty() {
                    ops.push(Operation::Surface(crate::SurfaceOperation::VertexColors {
                        object: object.clone(), vertices: mesh.vertices().iter().map(|v| v.id).collect(), colors: colors.clone(),
                    }));
                }
                // UV edits and export welding keep these corner normals and vertex
                // colours. Explicit normal recalculation replaces the authored field;
                // topology welds and decimation clear normals on changed faces because
                // those faces no longer describe the authored surface. Derived LODs
                // may approximate that field; the full-detail source keeps it intact.
                // Surface reconciliation and LOD transfer retain or interpolate colours.
                return Ok(ops);
            }
        }?;
        Ok(vec![op])
    }
    fn write(&self, w: &mut Writer) -> crate::Result<()> {
        match self {
            Self::Sphere { object, radius, segments, rings, smooth } => { w.u8(0)?; w.string(object)?; w.f64(*radius)?; w.u32(*segments)?; w.u32(*rings)?; w.u8(*smooth as u8) }
            Self::Cylinder { object, radius, height, segments, smooth } => { w.u8(1)?; w.string(object)?; w.f64(*radius)?; w.f64(*height)?; w.u32(*segments)?; w.u8(*smooth as u8) }
            Self::Polygons { object, positions, polygons, normals, colors } => {
                // Keep the original encoding for meshes without authored attributes.
                let attributes = !normals.is_empty() || !colors.is_empty();
                w.u8(if attributes { 3 } else { 2 })?; w.string(object)?;
                w.count(positions.len())?; for p in positions { for v in p { w.f64(*v)?; } }
                w.count(polygons.len())?;
                for p in polygons {
                    w.count(p.vertices.len())?; for v in &p.vertices { w.u32(*v)?; }
                    w.count(p.uvs.len())?; for uv in &p.uvs { w.f64(uv[0])?; w.f64(uv[1])?; }
                    w.u32(p.material)?;
                }
                if attributes {
                    w.count(normals.len())?; for normal in normals { for v in normal { w.f64(*v)?; } }
                    w.count(colors.len())?; for color in colors { for v in color { w.f64(*v)?; } }
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
/// Disjoint wall-clock phases of one build, measured on its calling worker.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProgramTimings {
    /// Program expansion and document application, excluding texture synthesis
    /// and weathering generation. Includes application of the baked colours.
    pub geometry: Duration,
    /// BVH construction, ambient occlusion sampling and edge-wear generation.
    pub weathering: Duration,
    /// Image preparation, surface operations and final texture flattening/PNG.
    pub textures: Duration,
    /// Final model compilation, excluding texture work counted above.
    pub compile: Duration,
}
pub struct BuiltProgram { pub document: Document, pub compiled: CompiledModel, pub warnings: Vec<String>, pub timings: ProgramTimings, pub detail: crate::ProgramBuildDetail }

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
    doc: Workspace,
    limits: Limits,
    program: &'a [ProgramOp],
    pending: Vec<(usize, Operation)>,
    generation: u64,
    // Own the already-materialized inputs for error-only replay. Successful
    // builds neither clone document states nor re-execute these batches.
    replay_batches: Vec<Vec<ReplayInput>>,
    replay_bytes: usize,
    cancelled: Option<&'a dyn Fn() -> bool>,
}
enum ReplayInput { Program(usize), Expanded(Box<Operation>) }
impl Runner<'_> {
    fn remember(&mut self, indices: &[usize], operations: Vec<Operation>) {
        let batch = indices.iter().zip(operations).map(|(&index, operation)| {
            self.replay_bytes = self.replay_bytes.saturating_add(std::mem::size_of::<ReplayInput>());
            // Coloured polygons expand to two operations, which may straddle a
            // batch boundary. Retain each expansion so error replay applies it once.
            let expanded_mesh = matches!(&self.program[index], ProgramOp::Mesh(MeshPrimitive::Polygons { colors, .. }) if !colors.is_empty());
            if !expanded_mesh && matches!(self.program[index], ProgramOp::Op(_) | ProgramOp::Mesh(_)) {
                ReplayInput::Program(index)
            } else {
                self.replay_bytes = self.replay_bytes.saturating_add(operation.memory_bytes());
                ReplayInput::Expanded(Box::new(operation))
            }
        }).collect();
        self.replay_batches.push(batch);
    }
    fn materialize(&self, batch: &[ReplayInput]) -> crate::Result<Vec<Operation>> {
        batch.iter().try_fold(Vec::new(), |mut operations, input| {
            match input {
                ReplayInput::Expanded(op) => operations.push((**op).clone()),
                ReplayInput::Program(index) => match &self.program[*index] {
                    ProgramOp::Op(op) => operations.push(op.clone()),
                    ProgramOp::Mesh(mesh) => operations.extend(mesh.operations(&self.limits)?),
                    _ => unreachable!(),
                },
            }
            Ok(operations)
        })
    }
    fn flush(&mut self, names: &[String]) -> Result<(), ProgramFailure> {
        let _flush = crate::build_detail::GeometryTimer::new(crate::build_detail::GeometryPhase::FlushApply);
        let chunk = CHUNK.min(self.limits.max_operations);
        if chunk == 0 && !self.pending.is_empty() {
            let index = self.pending[0].0;
            return Err(ProgramFailure { index, op: names.get(index).cloned().unwrap_or_default(), message: "transaction operations budget exceeded".into() });
        }
        let mut pending = std::mem::take(&mut self.pending).into_iter();
        loop {
            let (indices, operations): (Vec<_>, Vec<_>) = pending.by_ref().take(chunk).unzip();
            if operations.is_empty() { break; }
            if let Err((failed, error)) = self.doc.execute_program(&operations, self.replay_bytes, self.cancelled) {
                let failure = |at: usize, error: Error| {
                    let index = indices[at];
                    ProgramFailure { index, op: names.get(index).cloned().unwrap_or_default(), message: describe_error(&error) }
                };
                if matches!(error, Error::Mesh(crate::mesh::MeshError::Cancelled)) { return Err(failure(failed, error)); }
                // End-of-batch validation can find an error introduced by an
                // earlier operation. Reconstruct the boundary only on failure,
                // then use the original one-at-a-time attribution behavior.
                // Drop the damaged workspace before reconstructing, so error
                // attribution does not pin an additional full model state.
                self.doc = Workspace::new(self.limits.clone()).map_err(|e| failure(failed, e))?;
                for batch in &self.replay_batches {
                    let batch = self.materialize(batch).map_err(|e| failure(failed, e))?;
                    self.doc.execute_program(&batch, self.replay_bytes, self.cancelled).map_err(|(_, e)| failure(failed, e))?;
                }
                let count = operations.len();
                for (at, op) in operations.into_iter().enumerate() {
                    let batch = vec![op];
                    self.doc.execute_program(&batch, self.replay_bytes, self.cancelled).map_err(|(_, e)| failure(at, e))?;
                    self.remember(&indices[at..at + 1], batch);
                }
                self.generation += count as u64 + 1;
            } else {
                self.generation += 2; // Former apply followed by checkpoint.
                self.remember(&indices, operations);
            }
        }
        Ok(())
    }
}

fn joint_world(doc: &Workspace) -> Vec<(String, Option<usize>, [f64; 3])> {
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
    build_program_inner(ops, limits, fetch, cancelled, None)
}

/// Builds a program and retains reusable compile data for later exports.
pub fn build_program_with_cache(ops: &[ProgramOp], limits: Limits, fetch: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>, cancelled: Option<&dyn Fn() -> bool>, cache: &mut crate::CompileCache) -> Result<BuiltProgram, ProgramFailure> {
    build_program_inner(ops, limits, fetch, cancelled, Some(cache))
}

fn build_program_inner(ops: &[ProgramOp], limits: Limits, fetch: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>, cancelled: Option<&dyn Fn() -> bool>, cache: Option<&mut crate::CompileCache>) -> Result<BuiltProgram, ProgramFailure> {
    let detail_scope = crate::build_detail::DetailScope::new(ops);
    let mut weathering_detail = crate::WeatheringTiming::default();
    let geometry_start = ProgramInstant::now();
    let texture_start = texture_time();
    let mut timings = ProgramTimings::default();
    let names: Vec<String> = ops.iter().map(|op| {
        let object = match op {
            ProgramOp::Op(op) => op.object_name(),
            ProgramOp::Mesh(mesh) => Some(crate::build_detail::primitive_kind(mesh).0),
            ProgramOp::Bind { object, .. } => Some(object.as_str()),
            _ => None,
        };
        match object {
            Some(object) => format!("{}, object '{object}'", op.name()),
            None => op.name().to_string(),
        }
    }).collect();
    let fail = |index: usize, message: String| ProgramFailure { index, op: names.get(index).cloned().unwrap_or_default(), message };
    if limits.max_materials == 0 || limits.max_receipts == 0 {
        return Err(fail(0, describe_error(&Error::Invalid("zero material or receipt limit"))));
    }
    let doc = Workspace::new(limits.clone()).map_err(|e| fail(0, describe_error(&e)))?;
    let mut run = Runner { doc, limits: limits.clone(), program: ops, pending: Vec::new(), generation: 0, replay_batches: Vec::new(), replay_bytes: 0, cancelled };
    let mut warnings = Vec::new();
    let mut bound = std::collections::BTreeSet::new();
    // All entries use this build's fixed Limits. Retain source bytes only for
    // repeated complete parameter keys, and only until their final occurrence.
    let mut primitives = BTreeMap::<PrimitiveKey, CachedPrimitive>::new();
    for op in ops {
        if let ProgramOp::Mesh(m) = op {
            if let Some((key, _)) = m.cache_key() {
                primitives.entry(key).or_insert(CachedPrimitive { remaining: 0, source: None }).remaining += 1;
            }
        }
    }
    primitives.retain(|_, cached| cached.remaining > 1);
    for (index, op) in ops.iter().enumerate() {
        if cancelled.is_some_and(|c| c()) { return Err(fail(index, "cancelled".into())); }
        match op {
            ProgramOp::Op(op) => run.pending.push((index, op.clone())),
            ProgramOp::Impostor { .. } => {}
            ProgramOp::Mesh(m) => {
                let started = ProgramInstant::now();
                let operations = m.cached_operations(&run.limits, &mut primitives).map_err(|e| fail(index, describe_error(&e)))?;
                crate::build_detail::record_operation(crate::build_detail::primitive_kind(m).1, 0, started.elapsed());
                run.pending.extend(operations.into_iter().map(|op| (index, op)));
            }
            ProgramOp::ImageLayer { material, channel, layer, image } => {
                use crate::{SurfaceChannel as Ch, SurfaceOperation as S};
                let image_start = ProgramInstant::now();
                let nested_textures = texture_time();
                let bytes = fetch(image).map_err(|e| fail(index, format!("image '{image}': {e}")))?;
                let pixels = crate::RgbaImage::from_png(&bytes, &run.limits).map_err(|e| fail(index, format!("image '{image}': {}", describe_error(&e))))?;
                record_texture_time(image_start.elapsed().saturating_sub(texture_time() - nested_textures));
                // The image carries the channel: its factor goes neutral (white base colour
                // keeping alpha, full roughness and metalness, white emission).
                run.flush(&names)?;
                let image_start = ProgramInstant::now();
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
                record_texture_time(image_start.elapsed());
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
                let weathering_start = ProgramInstant::now();
                let (ao, distance, edges, samples) = (ao.clamp(0., 1.), distance.clamp(0.005, 10.), edges.clamp(0., 1.), (*samples).clamp(1, 64) as usize);
                for op in weathering::weathering_ops(&run.doc, objects.as_deref(), ao, distance, edges, samples, &mut weathering_detail) {
                    run.pending.push((index, op));
                }
                timings.weathering += weathering_start.elapsed();
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
    drop(run.replay_batches);
    let document = run.doc.into_document(run.generation, cancelled)
        .map_err(|e| fail(ops.len(), describe_error(&e)))?;
    timings.textures = texture_time() - texture_start;
    timings.geometry = geometry_start.elapsed().saturating_sub(timings.weathering + timings.textures);
    crate::build_detail::reset_compile_timings();
    let compile_start = ProgramInstant::now();
    let texture_start = texture_time();
    let compiled = match cache {
        Some(cache) => document.compile_with_cache(cache, cancelled),
        None => document.compile(cancelled),
    }.map_err(|e| ProgramFailure { index: ops.len(), op: "compile".into(), message: describe_error(&e) })?;
    warnings.extend(compiled.warnings.iter().cloned());
    let compile_textures = texture_time() - texture_start;
    let compile_elapsed = compile_start.elapsed();
    timings.compile = compile_elapsed.saturating_sub(compile_textures);
    timings.textures += compile_textures;
    let mut detail = detail_scope.finish();
    detail.geometry_other = timings.geometry.saturating_sub(detail.operations.values().map(|op| op.elapsed).sum::<Duration>() + detail.geometry.total());
    detail.compile.other = compile_elapsed.saturating_sub(detail.compile.total());
    detail.vertices = compiled.vertices;
    detail.triangles = compiled.triangles;
    detail.weathering = weathering_detail;
    Ok(BuiltProgram { document, compiled, warnings, timings, detail })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_attributes_export_and_plain_mesh_keeps_legacy_bytes() {
        let positions = vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        let polygons = vec![crate::mesh::Polygon::new(vec![0, 1, 2]), crate::mesh::Polygon::new(vec![0, 3, 1])];
        let normals = vec![[2., 0., 0.], [0., 3., 0.], [0., 0., 4.], [-2., 0., 0.]];
        let colors = vec![[0., 0., 0., 1.], [0.25, 0.25, 0.25, 1.], [0.75, 0.75, 0.75, 1.], [1.; 4]];
        let primitive = |normals: Vec<[f64; 3]>, colors: Vec<[f64; 4]>| MeshPrimitive::Polygons {
            object: "card".into(), positions: positions.clone(), polygons: polygons.clone(), normals, colors,
        };
        let build = |ops: &[ProgramOp], limits| build_program(ops, limits, &mut |_| Err("no external inputs".into()), None);
        let legacy = build(&[ProgramOp::Op(Operation::polygon_mesh("card", &positions, &polygons, &Limits::default()).unwrap())], Limits::default()).unwrap();
        let plain = primitive(Vec::new(), Vec::new());
        let original = build(&[ProgramOp::Mesh(plain.clone())], Limits::default()).unwrap();
        assert_eq!(original.compiled.glb, legacy.compiled.glb, "no attributes keeps the original GLB bytes");
        let mut encodings = std::collections::BTreeSet::new();
        for (with_normals, with_colors) in [(false, false), (true, false), (false, true), (true, true)] {
            let mesh = primitive(if with_normals { normals.clone() } else { Vec::new() }, if with_colors { colors.clone() } else { Vec::new() });
            let ops = vec![ProgramOp::Mesh(mesh)];
            assert!(encodings.insert(program_bytes(&ops).unwrap()), "each attribute changes the cache key");
            let built = build(&ops, Limits::default()).unwrap();
            let loaded = makepad_gltf::load_gltf_from_bytes(&built.compiled.glb, None).unwrap();
            let decoded = makepad_gltf::decode_mesh_primitive(&loaded, 0, 0).unwrap();
            assert_eq!(decoded.colors0.is_some(), with_colors, "only authored colours add COLOR_0");
            let actual_normals = decoded.normals.as_ref().unwrap();
            for (i, position) in decoded.positions.iter().enumerate() {
                let vertex = positions.iter().position(|p| p.map(|v| v as f32) == *position).unwrap();
                if with_normals {
                    assert_eq!(actual_normals[i], normalized(normals[vertex]).unwrap().map(|v| v as f32), "normal of vertex {vertex}");
                }
                if let Some(actual) = &decoded.colors0 {
                    assert_eq!(actual[i], colors[vertex].map(|v| v as f32), "colour of vertex {vertex}");
                }
            }
            if !with_normals {
                for (face, triangle) in decoded.indices.chunks_exact(3).enumerate() {
                    for &corner in triangle {
                        assert_eq!(actual_normals[corner as usize], [[0., 0., 1.], [0., 1., 0.]][face], "legacy face normal");
                    }
                }
            }
        }
        // A colour operation can fall in the next transaction. Error replay must
        // not expand either half twice or lose the original failure attribution.
        let mut limits = Limits::default();
        limits.max_operations = 1;
        let mut ops = vec![ProgramOp::Mesh(primitive(normals.clone(), colors.clone()))];
        build(&ops, limits.clone()).unwrap();
        ops.push(ProgramOp::Op(Operation::DeleteObject { object: "missing".into() }));
        let error = build(&ops, limits).err().unwrap();
        assert_eq!(error.index, 1, "replay keeps the failing program index");
        assert!(error.message.contains("missing"), "{}", error.message);
        for (normals, colors, attribute) in [(vec![[1., 0., 0.]], Vec::new(), "normals"), (Vec::new(), vec![[1.; 4]], "colors")] {
            let error = build(&[ProgramOp::Mesh(primitive(normals, colors))], Limits::default()).err().unwrap();
            assert!(error.message.contains("card") && error.message.contains(attribute), "{}", error.message);
        }
    }
}
