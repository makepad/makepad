//! Optional, worker-local measurements. Nothing is retained in model data or
//! cache keys, and no measurement allocates inside a vertex or pixel loop.
use crate::{program::ProgramInstant, Operation, SurfaceChannel, SurfaceOperation, SurfaceState};
use std::{cell::RefCell, collections::BTreeMap, time::Duration};

#[derive(Clone, Debug, Default)]
pub struct OperationTiming {
    pub count: usize,
    pub elapsed: Duration,
}

#[derive(Clone, Debug)]
pub struct TextureTiming {
    /// None for a shared fallback or atlas.
    pub material: Option<u32>,
    pub channel: SurfaceChannel,
    pub kind: &'static str,
    pub width: u32,
    pub height: u32,
    /// Generation, layer preparation and mip work, excluding export PNG encoding.
    pub elapsed: Duration,
}

/// Times texture work performed by a front end before it queues a program.
pub struct TextureGenerationTimer(ProgramInstant);
impl TextureGenerationTimer {
    pub fn start() -> Self { Self(ProgramInstant::now()) }
    pub fn finish(self, material: u32, channel: SurfaceChannel, kind: &'static str, width: u32, height: u32) -> TextureTiming {
        TextureTiming { material: Some(material), channel, kind, width, height, elapsed: self.0.elapsed() }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WeatheringTiming {
    pub vertices: usize,
    pub rays: usize,
    pub setup: Duration,
    pub acceleration: Duration,
    /// The sampling loop, including direction transforms and edge wear.
    pub queries: Duration,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CompileTimings {
    pub evaluation: Duration,
    pub mesh_packing: Duration,
    pub tangents: Duration,
    pub png: Duration,
    pub glb_assembly: Duration,
    pub texture_work: Duration,
    pub other: Duration,
}
impl CompileTimings {
    pub(crate) fn total(self) -> Duration {
        self.evaluation + self.mesh_packing + self.tangents + self.png + self.glb_assembly + self.texture_work + self.other
    }
}

/// Detail for a single program. The original `ProgramTimings` remains the
/// authoritative coarse split; in particular PNG time still belongs to its
/// textures field. These finer compile phases account for the whole export.
#[derive(Clone, Debug, Default)]
pub struct ProgramBuildDetail {
    pub operations: BTreeMap<&'static str, OperationTiming>,
    pub geometry: GeometryTimings,
    pub geometry_other: Duration,
    pub vertices: usize,
    pub triangles: usize,
    pub weathering: WeatheringTiming,
    pub textures: Vec<TextureTiming>,
    pub compile: CompileTimings,
}

/// Exclusive overhead: operation bodies and texture generation are excluded.
#[derive(Clone, Copy, Debug, Default)]
pub struct GeometryTimings {
    pub flush_apply: Duration,
    pub validate: Duration,
    pub history: Duration,
    pub clone: Duration,
}
impl GeometryTimings {
    pub(crate) fn total(self) -> Duration { self.flush_apply + self.validate + self.history + self.clone }
}

#[derive(Clone, Copy)]
pub(crate) enum GeometryPhase { FlushApply, Validate, History, Clone }
pub(crate) struct GeometryTimer {
    started: ProgramInstant,
    before: Duration,
    textures: Duration,
    phase: GeometryPhase,
}
fn geometry_accounted(detail: &ProgramBuildDetail) -> Duration {
    detail.geometry.total() + detail.operations.values().map(|op| op.elapsed).sum::<Duration>()
}
impl GeometryTimer {
    pub(crate) fn new(phase: GeometryPhase) -> Option<Self> {
        CAPTURE.with(|current| current.borrow().as_ref().filter(|c| c.program).map(|c| Self {
            started: ProgramInstant::now(), before: geometry_accounted(&c.detail),
            textures: crate::program::texture_time(), phase,
        }))
    }
}
impl Drop for GeometryTimer {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed().saturating_sub(crate::program::texture_time() - self.textures);
        CAPTURE.with(|current| {
            if let Some(capture) = current.borrow_mut().as_mut() {
                let elapsed = elapsed.saturating_sub(geometry_accounted(&capture.detail) - self.before);
                let target = match self.phase {
                    GeometryPhase::FlushApply => &mut capture.detail.geometry.flush_apply,
                    GeometryPhase::Validate => &mut capture.detail.geometry.validate,
                    GeometryPhase::History => &mut capture.detail.geometry.history,
                    GeometryPhase::Clone => &mut capture.detail.geometry.clone,
                };
                *target += elapsed;
            }
        });
    }
}

#[derive(Default)]
struct Capture {
    detail: ProgramBuildDetail,
    primitive_kinds: BTreeMap<String, &'static str>,
    program: bool,
}
thread_local! {
    static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) };
}

/// Restores a parent's collector on errors, cancellation and nested builds.
pub(crate) struct DetailScope(Option<Capture>);
impl DetailScope {
    pub(crate) fn new(ops: &[crate::ProgramOp]) -> Self {
        let mut capture = Capture { program: true, ..Default::default() };
        for op in ops {
            if let crate::ProgramOp::Mesh(mesh) = op {
                let (object, kind) = primitive_kind(mesh);
                capture.primitive_kinds.insert(object.to_owned(), kind);
            }
        }
        Self(CAPTURE.with(|current| current.replace(Some(capture))))
    }
    pub(crate) fn finish(self) -> ProgramBuildDetail {
        CAPTURE.with(|current| current.borrow_mut().take().unwrap().detail)
    }
}
impl Drop for DetailScope {
    fn drop(&mut self) {
        CAPTURE.with(|current| { current.replace(self.0.take()); });
    }
}

pub(crate) fn primitive_kind(mesh: &crate::MeshPrimitive) -> (&str, &'static str) {
    match mesh {
        crate::MeshPrimitive::Sphere { object, .. } => (object, "sphere"),
        crate::MeshPrimitive::Cylinder { object, .. } => (object, "cylinder"),
        crate::MeshPrimitive::Polygons { object, .. } => (object, "polygons"),
    }
}

pub(crate) fn record_operation(kind: &'static str, count: usize, elapsed: Duration) {
    CAPTURE.with(|current| {
        if let Some(capture) = current.borrow_mut().as_mut() {
            let entry = capture.detail.operations.entry(kind).or_default();
            entry.count += count;
            entry.elapsed += elapsed;
        }
    });
}

pub(crate) struct OperationTimer {
    started: ProgramInstant,
    textures: Duration,
    before: Duration,
    kind: &'static str,
}
impl OperationTimer {
    pub(crate) fn new(op: &Operation) -> Option<Self> {
        CAPTURE.with(|current| {
            let current = current.borrow();
            let capture = current.as_ref()?;
            if !capture.program { return None; }
            let kind = if let Operation::ImportMesh { object, .. } = op {
                capture.primitive_kinds.get(object).copied().unwrap_or("import_mesh")
            } else { operation_kind(op) };
            Some(Self { started: ProgramInstant::now(), textures: crate::program::texture_time(), before: geometry_accounted(&capture.detail), kind })
        })
    }
}
impl Drop for OperationTimer {
    fn drop(&mut self) {
        let nested = CAPTURE.with(|current| current.borrow().as_ref()
            .map_or(Duration::ZERO, |c| geometry_accounted(&c.detail).saturating_sub(self.before)));
        record_operation(self.kind, 1, self.started.elapsed()
            .saturating_sub(crate::program::texture_time() - self.textures).saturating_sub(nested));
    }
}

fn operation_kind(op: &Operation) -> &'static str {
    use crate::{Action, Construction as C, SceneOperation as S, Shape};
    match op {
        Operation::Construction(op) => match &op.kind {
            C::Lathe { .. } => "lathe", C::Sweep { .. } => "sweep", C::Loft { .. } => "loft",
            C::Decal { .. } => "decal", C::Boolean { .. } => "boolean", C::Voxel { .. } => "voxel",
            C::Fibers { .. } => "fibers", C::QuadStrip { .. } => "quad_strip",
            C::Shape { shape, .. } => match shape {
                Shape::Box { radius, .. } if *radius > 0. => "box_radius",
                Shape::Box { .. } => "box", Shape::Capsule { .. } => "capsule",
                Shape::Cone { .. } => "cone", Shape::Torus { .. } => "torus",
                Shape::Wedge { .. } => "wedge", Shape::Tube { .. } => "tube",
                Shape::Extrude { bevel, corner_radius, .. } if *bevel > 0. || *corner_radius > 0. => "extrude_bevel_radius",
                Shape::Extrude { .. } => "extrude", Shape::Wall(_) => "wall",
                Shape::Stair { .. } => "stair", Shape::Roof(_) => "roof",
            },
        },
        Operation::MeshEditing(op) => match &op.action {
            Action::Bevel { .. } | Action::RoundBevel { .. } => "bevel",
            Action::Normals { .. } => "smooth", Action::Decimate { .. } => "decimate",
            Action::BoxUv { .. } | Action::CylinderUv { .. } | Action::Unwrap { .. } | Action::Pack { .. } => "uv",
            Action::Region { .. } => "region", Action::Split { .. } => "split",
            Action::Collapse { .. } => "collapse", Action::Dissolve { .. } => "dissolve",
            Action::Fill { .. } => "fill", Action::Bridge { .. } => "bridge",
            Action::LoopCut { .. } => "loop_cut", Action::Slide { .. } => "slide",
            Action::Solidify { .. } => "solidify", Action::Array { .. } => "array",
            Action::Deform { .. } => "deform", Action::Lattice { .. } => "lattice",
            Action::Shrinkwrap { .. } => "shrinkwrap", Action::Sculpt { .. } => "sculpt",
            Action::Subdivide { .. } => "subdivide", Action::Mirror { .. } => "mirror",
            Action::FlipFaces { .. } => "flip_faces", Action::Relax { .. } => "relax",
            Action::Pin { .. } => "pin", Action::Crease { .. } => "crease",
        },
        Operation::Scene(op) => match op {
            S::Collider { .. } => "collider", S::Node { .. } => "node",
            S::Instance { .. } => "instance", S::Duplicate { .. } => "duplicate",
            S::Lods { .. } => "lod", S::Modifier { .. } | S::ApplyModifiers { .. } => "modifier",
            S::MakeUnique { .. } => "make_unique", S::Join { .. } => "join",
            S::Separate { .. } => "separate", S::Snap { .. } => "snap",
            S::Pivot { .. } => "pivot", S::Dimensions { .. } => "dimensions",
            S::Light(_) | S::DeleteLight { .. } => "light",
            S::Socket(_) | S::DeleteSocket { .. } => "socket",
            S::RemoveModifier { .. } | S::ModifierOrder { .. } => "modifier",
            S::VehicleWheel { .. } | S::DeleteVehicleWheel { .. } => "vehicle_wheel",
        },
        Operation::SetMaterial { .. } | Operation::AssignMaterial { .. } => "material",
        Operation::Surface(SurfaceOperation::Material { .. }) => "material",
        Operation::Surface(SurfaceOperation::VertexColors { .. } | SurfaceOperation::VertexPaint { .. }) => "vertex_colors",
        Operation::Surface(_) | Operation::TextureSolid { .. } | Operation::PaintTexture { .. } => "surface",
        Operation::Cube { .. } => "box", Operation::Plane { .. } => "plane",
        Operation::ImportMesh { .. } => "import_mesh", Operation::Transform { .. } => "transform",
        Operation::Smooth { .. } => "smooth", Operation::Extrude { .. } => "extrude",
        Operation::Inset { .. } => "inset", Operation::Weld { .. } => "weld",
        Operation::Mirror { .. } => "mirror", Operation::Subdivide { .. } => "subdivide",
        Operation::ProjectUv { .. } | Operation::SetUv { .. } => "uv",
        Operation::DeleteObject { .. } => "delete_object", Operation::DeleteFaces { .. } => "delete_faces",
        Operation::EdgeAttributes { .. } => "edge_attributes", Operation::CornerNormal { .. } => "normal",
        Operation::Brush { .. } => "brush", Operation::SetWeights { .. } | Operation::AutoWeights { .. } => "weights",
        Operation::SetSkeleton { .. } => "skeleton", Operation::SetClip { .. } | Operation::DeleteClip { .. } => "clip",
        _ => op.family_name(),
    }
}

pub(crate) fn record_texture(material: Option<u32>, channel: SurfaceChannel, kind: &'static str, width: u32, height: u32, elapsed: Duration) {
    CAPTURE.with(|current| {
        if let Some(capture) = current.borrow_mut().as_mut() {
            if let Some(entry) = capture.detail.textures.iter_mut().find(|t| t.material == material && t.channel == channel && (material.is_some() || t.kind == kind)) {
                entry.elapsed += elapsed;
                entry.width = width;
                entry.height = height;
                entry.kind = kind;
            } else {
                capture.detail.textures.push(TextureTiming { material, channel, kind, width, height, elapsed });
            }
        }
    });
}

pub(crate) fn texture_kind(channel: SurfaceChannel, layer: &crate::SurfaceLayer) -> &'static str {
    if let Some(pattern) = &layer.pattern { pattern.kind.name() }
    else if layer.id == "sign" || layer.id == "sign_glow" { "sign" }
    else if channel == SurfaceChannel::Occlusion { "ao" }
    else if layer.derived.is_some() { "derived" }
    else { "image" }
}

pub(crate) fn record_surface(op: &SurfaceOperation, state: &SurfaceState, elapsed: Duration) {
    use SurfaceOperation::*;
    if !CAPTURE.with(|current| current.borrow().is_some()) { return; }
    let (material, channel, layer) = match op {
        Layer { material, channel, layer } => (*material, *channel, layer.id.as_str()),
        Pattern { material, channel, layer, .. } | Derive { material, channel, layer, .. }
        | Bake { material, channel, layer, .. } | Mips { material, channel, layer }
        | Dilate { material, channel, layer, .. } | Stroke { material, channel, layer, .. }
        | ProjectedStroke { material, channel, layer, .. } => (*material, *channel, layer.as_str()),
        _ => return,
    };
    if let Some(layer) = state.materials.get(&material).and_then(|m| m.channels.get(&channel)).and_then(|ls| ls.iter().find(|l| l.id == layer)) {
        record_texture(Some(material), channel, texture_kind(channel, layer), layer.image.width, layer.image.height, elapsed);
    }
}

#[derive(Clone, Copy)]
pub(crate) enum CompilePhase { Evaluation, MeshPacking, Tangents, Png, GlbAssembly, Textures }
pub(crate) struct CompileTimer { started: ProgramInstant, before: Duration, phase: CompilePhase }
impl CompileTimer {
    pub(crate) fn new(phase: CompilePhase) -> Option<Self> {
        CAPTURE.with(|current| current.borrow().as_ref().map(|c| Self {
            started: ProgramInstant::now(), before: c.detail.compile.total(), phase,
        }))
    }
}
impl Drop for CompileTimer {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed();
        CAPTURE.with(|current| {
            if let Some(capture) = current.borrow_mut().as_mut() {
                let compile = &mut capture.detail.compile;
                let elapsed = elapsed.saturating_sub(compile.total() - self.before);
                let target = match self.phase {
                    CompilePhase::Evaluation => &mut compile.evaluation,
                    CompilePhase::MeshPacking => &mut compile.mesh_packing,
                    CompilePhase::Tangents => &mut compile.tangents,
                    CompilePhase::Png => &mut compile.png,
                    CompilePhase::GlbAssembly => &mut compile.glb_assembly,
                    CompilePhase::Textures => &mut compile.texture_work,
                };
                *target += elapsed;
            }
        });
    }
}

pub(crate) fn png_time() -> Duration {
    CAPTURE.with(|current| current.borrow().as_ref().map_or(Duration::ZERO, |c| c.detail.compile.png))
}

pub(crate) fn reset_compile_timings() {
    CAPTURE.with(|current| {
        if let Some(capture) = current.borrow_mut().as_mut() { capture.detail.compile = CompileTimings::default(); capture.program = false; }
    });
}
