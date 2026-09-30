//! The post graph and its frame plan (KERNELS.md §3.4.1-§3.4.2).
//!
//! A [`PostGraph`] is what a document asks for: [`PassNode`]s placed in one
//! of three stages, each a Splash fullscreen program reading named
//! resources. [`FramePlan::compile`] turns it into what runs:
//!
//! * **Order.** Stages run `@hdr` (linear, before exposure and the tone
//!   map), `@display` (after the tone map) and `@final` (after AA); within a
//!   stage passes run in the author's order. The host owns what sits
//!   between the stages (its tone map, its AA): the plan gives it *slots*.
//! * **Pass ids and resource versions.** Every pass gets a [`PassId`];
//!   every write makes a new [`Version`] of its resource. A read names the
//!   version current at that point. Reads of a resource nothing produces
//!   (a named pass that does not exist or comes later, an attachment the
//!   host does not render) are rejected with a diagnostic naming the pass,
//!   so cycles cannot be written.
//! * **Targets.** Every written version gets its own target for the frame:
//!   a pass never samples a texture a later pass of the same paint writes
//!   (the platform rule), so versions are not recycled within a frame.
//! * **Memory is admitted, not assumed.** The plan sums its target bytes
//!   (plus whatever the host adds, such as accumulation) and a host admits
//!   it before allocating anything ([`FramePlan::bytes`]).
//! * **Recompiles.** [`FramePlan::structure_hash`] covers what changes the
//!   plan (nodes, reads, scales, formats, programs, size, mode kind);
//!   parameter changes are uniforms and never recompile.
//!
//! With no nodes the plan is empty and a host's chain runs exactly as it
//! did before the graph existed (the zero-cost check).

use makepad_script::*;
use std::hash::{Hash, Hasher};

/// Where a pass sits in the chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// Before the scene: simulation state and generated textures the scene
    /// and later passes read by name (no frame colour yet).
    Pre,
    /// Linear scene-referred light, before exposure and the tone map.
    Hdr,
    /// Display-encoded, after the tone map (grades, LUTs, outlines).
    Display,
    /// After anti-aliasing: grain, vignette, frame-point post.
    Final,
}

impl Stage {
    pub const ALL: [Stage; 4] = [Stage::Pre, Stage::Hdr, Stage::Display, Stage::Final];

    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "pre" => Some(Stage::Pre),
            "hdr" => Some(Stage::Hdr),
            "display" => Some(Stage::Display),
            "final" => Some(Stage::Final),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Stage::Pre => "pre",
            Stage::Hdr => "hdr",
            Stage::Display => "display",
            Stage::Final => "final",
        }
    }
}

/// A pass output's texel format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// Linear half float (the default for `@hdr`).
    Rgba16f,
    /// Full float (data passes, sums).
    Rgba32f,
    /// 8-bit, display-encoded (the default after the tone map).
    Rgba8,
    /// One full-float channel (a raymarch's view distance, an id).
    R32f,
}

impl Format {
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "rgba16f" | "hdr" | "f16" => Some(Format::Rgba16f),
            "rgba32f" | "f32" | "data" => Some(Format::Rgba32f),
            "rgba8" | "display" | "u8" => Some(Format::Rgba8),
            "r32f" | "depth" | "id" => Some(Format::R32f),
            _ => None,
        }
    }

    pub fn bytes_per_texel(self) -> u64 {
        match self {
            Format::Rgba16f => 8,
            Format::Rgba32f => 16,
            Format::Rgba8 => 4,
            Format::R32f => 4,
        }
    }

    /// The format a stage's passes write unless they say otherwise.
    pub fn default_for(stage: Stage) -> Self {
        match stage {
            Stage::Pre | Stage::Hdr => Format::Rgba16f,
            Stage::Display | Stage::Final => Format::Rgba8,
        }
    }

    #[cfg(feature = "gpu")]
    pub fn texture_format(self) -> makepad_draw::TextureFormat {
        use makepad_draw::{TextureFormat, TextureSize};
        match self {
            Format::Rgba16f => TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true },
            Format::Rgba32f => TextureFormat::RenderRGBAf32 { size: TextureSize::Auto, initial: true },
            Format::Rgba8 => TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true },
            Format::R32f => TextureFormat::RenderRf32 { size: TextureSize::Auto, initial: true },
        }
    }

    /// The draw shader's `color_format` for a target of this format.
    pub fn shader_color_format(self) -> &'static str {
        match self {
            Format::Rgba16f => "@Rgba16F",
            Format::Rgba32f => "@Rgba32F",
            Format::Rgba8 => "@Bgra8NoBlend",
            Format::R32f => "@Rf32",
        }
    }
}

/// A resource a pass reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Resource {
    /// The frame colour at this point of the chain (linear in `@hdr`,
    /// display-encoded after).
    Color,
    /// The scene's depth (D32, sampled).
    Depth,
    /// The scene's normals.
    Normal,
    /// Screen-space velocity (realtime motion blur).
    Velocity,
    /// The emission / glow attachment: what glows, linear.
    Glow,
    /// Object ids (outlines).
    Id,
    /// The output of an earlier named pass, or a texture the host provides
    /// by name (a LUT, a document image).
    Named(LiveId),
    /// The reading pass's own output of the previous frame (a history
    /// pass only).
    History,
    /// `name.prev`: the named history pass's latest output when the
    /// reading pass runs (last frame's when that pass comes later: a
    /// multi-pass simulation closes its loop).
    Previous(LiveId),
}

impl Resource {
    /// `@color`, `@depth`, ... or a pass name.
    pub fn by_name(name: &str) -> Self {
        match name {
            "color" => Resource::Color,
            "depth" => Resource::Depth,
            "normal" => Resource::Normal,
            "velocity" => Resource::Velocity,
            "glow" | "emission" => Resource::Glow,
            "id" => Resource::Id,
            "history" => Resource::History,
            other => match other.strip_suffix(".prev") {
                Some(name) => Resource::Previous(LiveId::from_str(name)),
                None => Resource::Named(LiveId::from_str(other)),
            },
        }
    }

    fn is_attachment(self) -> bool {
        !matches!(self, Resource::Color | Resource::Named(_) | Resource::History | Resource::Previous(_))
    }
}

/// A compiled program's identity: the hash of its shader source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProgramId(pub u64);

/// One pass as the document gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct PassNode {
    /// A name later passes read it by; unnamed passes replace the frame
    /// colour.
    pub name: Option<LiveId>,
    pub stage: Stage,
    /// The texture slots, in the order the program declares them.
    pub reads: Vec<Resource>,
    /// Output size relative to the frame (0.5 = half resolution).
    pub scale: f32,
    /// A fixed size in pixels instead (simulation state, a lookup).
    pub size: Option<(u32, u32)>,
    pub format: Format,
    pub program: ProgramId,
    /// It keeps its output across frames (it may read `@history`).
    pub history: bool,
    /// Further named outputs the pass writes in the same draw (MRT: a
    /// raymarch's G-buffer, its depth), each at the pass's size.
    pub outputs: Vec<(LiveId, Format)>,
    /// For diagnostics: the pass's label (its kit and index, or `Pass`).
    pub label: String,
}

impl PassNode {
    /// Whether this pass writes the frame colour (unnamed) or its own
    /// resource (named).
    pub fn writes_color(&self) -> bool {
        self.name.is_none()
    }
}

/// How the colour is managed through the chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorPipeline {
    /// Linear scene-referred f16 until the tone map (the default).
    Hdr,
    /// The legacy 8-bit display-space lane (vj_fx parity, devices without
    /// f16 blending): no `@hdr` stage.
    Display8,
}

/// The document's post graph.
#[derive(Clone, Debug, PartialEq)]
pub struct PostGraph {
    pub color: ColorPipeline,
    pub nodes: Vec<PassNode>,
}

impl Default for PostGraph {
    fn default() -> Self {
        Self { color: ColorPipeline::Hdr, nodes: Vec::new() }
    }
}

impl PostGraph {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// A pass's index in the plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PassId(pub u32);

/// One write of a resource: the plan's targets are indexed by version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Version(pub u32);

/// Where a pass's input comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// What the host hands the stage: the colour at the slot, or one of its
    /// scene attachments.
    Host(Resource),
    /// An earlier pass's output.
    Target(Version),
    /// This pass's own output of the previous frame.
    History,
    /// A history pass's latest output (the plan node).
    Previous(usize),
}

/// A planned target: one per written version.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetDesc {
    pub format: Format,
    pub scale: f32,
    /// Size in pixels for the plan's frame size.
    pub size: (u32, u32),
    /// The pass that writes it.
    pub writer: PassId,
}

/// One pass in the plan.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedPass {
    pub id: PassId,
    /// Its node in the graph.
    pub node: usize,
    pub stage: Stage,
    pub inputs: Vec<Source>,
    pub output: Version,
    /// The versions of its further outputs, in order (MRT attachments
    /// 1..).
    pub extra: Vec<Version>,
    pub size: (u32, u32),
}

/// Which attachments the host's scene pass renders (and so may be read).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Attachments {
    pub depth: bool,
    pub normal: bool,
    pub velocity: bool,
    pub glow: bool,
    pub id: bool,
}

impl Attachments {
    fn has(&self, r: Resource) -> bool {
        match r {
            Resource::Depth => self.depth,
            Resource::Normal => self.normal,
            Resource::Velocity => self.velocity,
            Resource::Glow => self.glow,
            Resource::Id => self.id,
            Resource::Color | Resource::Named(_) | Resource::History | Resource::Previous(_) => true,
        }
    }
}

/// Why a graph did not compile.
#[derive(Clone, Debug, PartialEq)]
pub enum PlanError {
    /// A read of a named resource nothing wrote before it.
    UnproducedRead { pass: String, resource: String },
    /// A read of an attachment the host does not render.
    MissingAttachment { pass: String, resource: String },
    /// Two passes with one name.
    DuplicateName { pass: String, name: String },
    /// `@hdr` passes in a display-space (8-bit) pipeline.
    NoHdrStage { pass: String },
    /// A scale outside (0, 4].
    BadScale { pass: String, scale: f32 },
    /// A read of `@history` in a pass that keeps none.
    NoHistory { pass: String },
    /// A `@pre` pass reads the frame colour or writes it.
    PreColor { pass: String },
    /// The targets do not fit the budget the host gave.
    OverBudget { bytes: u64, budget: u64 },
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::UnproducedRead { pass, resource } => write!(f, "{pass} reads `{resource}`, which no earlier pass writes (name a pass that comes before it)"),
            PlanError::MissingAttachment { pass, resource } => write!(f, "{pass} reads @{resource}, which this renderer does not provide"),
            PlanError::DuplicateName { pass, name } => write!(f, "{pass}: a pass named `{name}` already exists"),
            PlanError::NoHdrStage { pass } => write!(f, "{pass} is placed at @hdr, but this pipeline is display-space 8-bit (no @hdr stage)"),
            PlanError::BadScale { pass, scale } => write!(f, "{pass}: scale {scale} is outside 0..4"),
            PlanError::NoHistory { pass } => write!(f, "{pass} reads @history but keeps none (add `history: true`)"),
            PlanError::PreColor { pass } => write!(f, "{pass} runs @pre, before the scene: it has no @color to read, and it needs a `name` later passes and the scene read it by"),
            PlanError::OverBudget { bytes, budget } => write!(f, "the post graph needs {} MiB of targets, over the {} MiB budget", bytes >> 20, budget >> 20),
        }
    }
}

/// A compiled graph for one frame size and mode kind.
#[derive(Clone, Debug, PartialEq)]
pub struct FramePlan {
    pub passes: Vec<PlannedPass>,
    pub targets: Vec<TargetDesc>,
    /// Per stage: the colour at the stage's end (None = unchanged: what the
    /// host handed in).
    pub stage_color: [Option<Version>; 4],
    /// The last version of each named resource.
    pub named: Vec<(LiveId, Version)>,
    pub size: (u32, u32),
    structure: u64,
    bytes: u64,
}

fn scaled(size: (u32, u32), scale: f32) -> (u32, u32) {
    let s = |v: u32| ((v as f64 * scale as f64).round() as u32).max(1);
    (s(size.0), s(size.1))
}

impl FramePlan {
    /// Compile `graph` for a frame of `size` pixels. `locked` is the mode
    /// kind (part of the structure); `budget` caps the target bytes.
    pub fn compile(graph: &PostGraph, size: (u32, u32), attachments: Attachments, host_named: &[LiveId], locked: bool, budget: u64) -> Result<FramePlan, PlanError> {
        let mut passes = Vec::new();
        let mut targets: Vec<TargetDesc> = Vec::new();
        let mut named: Vec<(LiveId, Version)> = Vec::new();
        let mut stage_color = [None; 4];
        let mut color: Option<Version> = None;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        size.hash(&mut hasher);
        locked.hash(&mut hasher);
        attachments.hash(&mut hasher);
        host_named.hash(&mut hasher);
        std::mem::discriminant(&graph.color).hash(&mut hasher);
        for (si, stage) in Stage::ALL.into_iter().enumerate() {
            // The colour entering a stage is what the host hands in: its
            // tone map sits between the stages, so a later stage never
            // reads the earlier stage's colour version as its own.
            color = None;
            for (ni, node) in graph.nodes.iter().enumerate().filter(|(_, n)| n.stage == stage) {
                if stage == Stage::Hdr && graph.color == ColorPipeline::Display8 {
                    return Err(PlanError::NoHdrStage { pass: node.label.clone() });
                }
                if node.size.is_none() && !(node.scale > 0.0 && node.scale <= 4.0) {
                    return Err(PlanError::BadScale { pass: node.label.clone(), scale: node.scale });
                }
                if stage == Stage::Pre && (node.name.is_none() || node.reads.contains(&Resource::Color)) {
                    return Err(PlanError::PreColor { pass: node.label.clone() });
                }
                let mut inputs = Vec::with_capacity(node.reads.len());
                for &r in &node.reads {
                    let src = match r {
                        Resource::Color => color.map_or(Source::Host(Resource::Color), Source::Target),
                        Resource::Named(n) => match named.iter().rev().find(|(k, _)| *k == n) {
                            Some((_, v)) => Source::Target(*v),
                            None if host_named.contains(&n) => Source::Host(r),
                            None => return Err(PlanError::UnproducedRead { pass: node.label.clone(), resource: n.to_string() }),
                        },
                        Resource::History => {
                            if !node.history {
                                return Err(PlanError::NoHistory { pass: node.label.clone() });
                            }
                            Source::History
                        }
                        Resource::Previous(n) => match graph.nodes.iter().position(|o| o.name == Some(n) && o.history) {
                            Some(at) => Source::Previous(at),
                            None => return Err(PlanError::NoHistory { pass: format!("{} (reads {n}.prev: no history pass is named {n})", node.label) }),
                        },
                        a if a.is_attachment() => {
                            if !attachments.has(a) {
                                return Err(PlanError::MissingAttachment { pass: node.label.clone(), resource: format!("{a:?}").to_lowercase() });
                            }
                            Source::Host(a)
                        }
                        _ => unreachable!(),
                    };
                    inputs.push(src);
                }
                let id = PassId(passes.len() as u32);
                let v = Version(targets.len() as u32);
                let px = match node.size {
                    Some((w, h)) => (w.clamp(1, 16384), h.clamp(1, 16384)),
                    None => scaled(size, node.scale),
                };
                targets.push(TargetDesc { format: node.format, scale: node.scale, size: px, writer: id });
                match node.name {
                    Some(n) => {
                        if named.iter().any(|(k, _)| *k == n) {
                            return Err(PlanError::DuplicateName { pass: node.label.clone(), name: n.to_string() });
                        }
                        named.push((n, v));
                    }
                    None => color = Some(v),
                }
                let mut extra = Vec::new();
                for &(n, format) in &node.outputs {
                    if named.iter().any(|(k, _)| *k == n) {
                        return Err(PlanError::DuplicateName { pass: node.label.clone(), name: n.to_string() });
                    }
                    let ev = Version(targets.len() as u32);
                    targets.push(TargetDesc { format, scale: node.scale, size: px, writer: id });
                    named.push((n, ev));
                    extra.push(ev);
                }
                (stage, &node.reads, node.name, node.format, node.program, node.history, node.size, &node.outputs).hash(&mut hasher);
                node.scale.to_bits().hash(&mut hasher);
                passes.push(PlannedPass { id, node: ni, stage, inputs, output: v, extra, size: px });
            }
            stage_color[si] = color;
        }
        let _ = color;
        // A history pass keeps a second target (last frame's output).
        let bytes = targets
            .iter()
            .map(|t| {
                let twin = if graph.nodes[passes[t.writer.0 as usize].node].history { 2 } else { 1 };
                twin * t.size.0 as u64 * t.size.1 as u64 * t.format.bytes_per_texel()
            })
            .sum();
        if bytes > budget {
            return Err(PlanError::OverBudget { bytes, budget });
        }
        Ok(FramePlan { passes, targets, stage_color, named, size, structure: hasher.finish(), bytes })
    }

    /// What changes the plan; equal hashes run the same passes.
    pub fn structure_hash(&self) -> u64 {
        self.structure
    }

    /// Target bytes the plan allocates.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }

    /// The passes of one stage, in order.
    pub fn stage(&self, stage: Stage) -> impl Iterator<Item = &PlannedPass> {
        self.passes.iter().filter(move |p| p.stage == stage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(name: Option<&str>, stage: Stage, reads: &[&str], scale: f32) -> PassNode {
        PassNode {
            name: name.map(LiveId::from_str),
            stage,
            reads: reads.iter().map(|r| Resource::by_name(r)).collect(),
            scale,
            format: Format::default_for(stage),
            program: ProgramId(reads.len() as u64),
            history: false,
            outputs: Vec::new(),
            size: None,
            label: name.unwrap_or("Pass").to_string(),
        }
    }

    const BIG: u64 = 1 << 40;

    #[test]
    fn no_nodes_is_an_empty_plan() {
        let plan = FramePlan::compile(&PostGraph::default(), (1920, 1080), Attachments::default(), &[], false, BIG).unwrap();
        assert!(plan.is_empty());
        assert_eq!(plan.bytes(), 0);
        assert_eq!(plan.stage_color, [None; 4]);
    }

    #[test]
    fn versions_follow_writes_and_stages_order_passes() {
        let g = PostGraph {
            color: ColorPipeline::Hdr,
            nodes: vec![
                pass(None, Stage::Final, &["color"], 1.0),
                pass(Some("half"), Stage::Hdr, &["glow"], 0.5),
                pass(None, Stage::Hdr, &["color", "half"], 1.0),
                pass(None, Stage::Hdr, &["color"], 1.0),
                pass(None, Stage::Display, &["color", "half"], 1.0),
            ],
        };
        let at = Attachments { glow: true, ..Default::default() };
        let plan = FramePlan::compile(&g, (1280, 720), at, &[], true, BIG).unwrap();
        let order: Vec<usize> = plan.passes.iter().map(|p| p.node).collect();
        assert_eq!(order, vec![1, 2, 3, 4, 0]);
        // half is v0; the first colour pass reads the host's colour and
        // half, writes v1; the next reads v1.
        assert_eq!(plan.passes[0].inputs, vec![Source::Host(Resource::Glow)]);
        assert_eq!(plan.passes[0].size, (640, 360));
        assert_eq!(plan.passes[1].inputs, vec![Source::Host(Resource::Color), Source::Target(Version(0))]);
        assert_eq!(plan.passes[2].inputs, vec![Source::Target(Version(1))]);
        // @display starts from the host's tone-mapped colour again, and
        // can still read @hdr's named output.
        assert_eq!(plan.passes[3].inputs, vec![Source::Host(Resource::Color), Source::Target(Version(0))]);
        assert_eq!(plan.stage_color, [None, Some(Version(2)), Some(Version(3)), Some(Version(4))]);
        assert_eq!(plan.targets.len(), 5);
        assert_eq!(plan.bytes(), 640 * 360 * 8 + 2 * 1280 * 720 * 8 + 2 * 1280 * 720 * 4);
    }

    #[test]
    fn bad_graphs_are_refused_with_the_pass_named() {
        let at = Attachments::default();
        let later = PostGraph { color: ColorPipeline::Hdr, nodes: vec![pass(None, Stage::Hdr, &["color", "blur"], 1.0), pass(Some("blur"), Stage::Hdr, &["color"], 0.5)] };
        assert!(matches!(FramePlan::compile(&later, (64, 64), at, &[], false, BIG), Err(PlanError::UnproducedRead { .. })));
        let glow = PostGraph { color: ColorPipeline::Hdr, nodes: vec![pass(None, Stage::Hdr, &["glow"], 1.0)] };
        let e = FramePlan::compile(&glow, (64, 64), at, &[], false, BIG).unwrap_err();
        assert!(e.to_string().contains("@glow"), "{e}");
        let dup = PostGraph { color: ColorPipeline::Hdr, nodes: vec![pass(Some("a"), Stage::Hdr, &["color"], 1.0), pass(Some("a"), Stage::Hdr, &["color"], 1.0)] };
        assert!(matches!(FramePlan::compile(&dup, (64, 64), at, &[], false, BIG), Err(PlanError::DuplicateName { .. })));
        let d8 = PostGraph { color: ColorPipeline::Display8, nodes: vec![pass(None, Stage::Hdr, &["color"], 1.0)] };
        assert!(matches!(FramePlan::compile(&d8, (64, 64), at, &[], false, BIG), Err(PlanError::NoHdrStage { .. })));
        let scale = PostGraph { color: ColorPipeline::Hdr, nodes: vec![pass(None, Stage::Hdr, &["color"], 0.0)] };
        assert!(matches!(FramePlan::compile(&scale, (64, 64), at, &[], false, BIG), Err(PlanError::BadScale { .. })));
    }

    #[test]
    fn history_reads_its_own_last_frame_and_is_charged_twice() {
        let mut fb = pass(None, Stage::Display, &["color", "history"], 1.0);
        let g = PostGraph { color: ColorPipeline::Display8, nodes: vec![fb.clone()] };
        assert!(matches!(FramePlan::compile(&g, (64, 64), Attachments::default(), &[], false, BIG), Err(PlanError::NoHistory { .. })));
        fb.history = true;
        let g = PostGraph { color: ColorPipeline::Display8, nodes: vec![fb] };
        let plan = FramePlan::compile(&g, (64, 64), Attachments::default(), &[], false, BIG).unwrap();
        assert_eq!(plan.passes[0].inputs, vec![Source::Host(Resource::Color), Source::History]);
        assert_eq!(plan.bytes(), 2 * 64 * 64 * 4);
    }

    #[test]
    fn pre_passes_run_first_at_their_own_size_and_are_read_by_name() {
        let mut state = pass(Some("state"), Stage::Pre, &["history"], 1.0);
        state.history = true;
        state.size = Some((256, 128));
        state.format = Format::Rgba32f;
        let g = PostGraph { color: ColorPipeline::Hdr, nodes: vec![pass(None, Stage::Hdr, &["color", "state"], 1.0), state] };
        let plan = FramePlan::compile(&g, (1920, 1080), Attachments::default(), &[], false, BIG).unwrap();
        assert_eq!(plan.passes[0].node, 1);
        assert_eq!(plan.passes[0].size, (256, 128));
        assert_eq!(plan.passes[1].inputs, vec![Source::Host(Resource::Color), Source::Target(Version(0))]);
        assert_eq!(plan.named, vec![(LiveId::from_str("state"), Version(0))]);
        let bad = PostGraph { color: ColorPipeline::Hdr, nodes: vec![pass(Some("s"), Stage::Pre, &["color"], 1.0)] };
        assert!(matches!(FramePlan::compile(&bad, (64, 64), Attachments::default(), &[], false, BIG), Err(PlanError::PreColor { .. })));
    }

    #[test]
    fn a_pass_reads_a_later_history_pass_s_last_frame() {
        let mut advect = pass(Some("advect"), Stage::Pre, &["project.prev"], 1.0);
        advect.size = Some((64, 64));
        let mut project = pass(Some("project"), Stage::Pre, &["advect"], 1.0);
        project.history = true;
        project.size = Some((64, 64));
        let g = PostGraph { color: ColorPipeline::Hdr, nodes: vec![advect.clone(), project] };
        let plan = FramePlan::compile(&g, (64, 64), Attachments::default(), &[], false, BIG).unwrap();
        assert_eq!(plan.passes[0].inputs, vec![Source::Previous(1)]);
        let bad = PostGraph { color: ColorPipeline::Hdr, nodes: vec![advect] };
        assert!(matches!(FramePlan::compile(&bad, (64, 64), Attachments::default(), &[], false, BIG), Err(PlanError::NoHistory { .. })));
    }

    #[test]
    fn further_outputs_are_named_versions_of_the_same_size() {
        let mut march = pass(Some("march"), Stage::Hdr, &["color"], 0.5);
        march.outputs = vec![(LiveId::from_str("gbuf"), Format::Rgba16f), (LiveId::from_str("march_depth"), Format::R32f)];
        let g = PostGraph { color: ColorPipeline::Hdr, nodes: vec![march, pass(None, Stage::Hdr, &["color", "march", "march_depth"], 1.0)] };
        let plan = FramePlan::compile(&g, (200, 100), Attachments::default(), &[], false, BIG).unwrap();
        assert_eq!(plan.passes[0].extra, vec![Version(1), Version(2)]);
        assert_eq!(plan.targets[2].format, Format::R32f);
        assert_eq!(plan.targets[2].size, (100, 50));
        assert_eq!(plan.passes[1].inputs, vec![Source::Host(Resource::Color), Source::Target(Version(0)), Source::Target(Version(2))]);
        assert_eq!(plan.bytes(), 100 * 50 * (8 + 8 + 4) + 200 * 100 * 8);
    }

    #[test]
    fn memory_is_admitted_before_allocation() {
        // One 8192² RGBA16F target is 512 MiB.
        let g = PostGraph { color: ColorPipeline::Hdr, nodes: vec![pass(None, Stage::Hdr, &["color"], 1.0)] };
        let e = FramePlan::compile(&g, (8192, 8192), Attachments::default(), &[], true, 256 << 20).unwrap_err();
        assert_eq!(e, PlanError::OverBudget { bytes: 512 << 20, budget: 256 << 20 });
    }

    #[test]
    fn parameters_do_not_change_the_structure() {
        let g = PostGraph { color: ColorPipeline::Hdr, nodes: vec![pass(None, Stage::Hdr, &["color"], 1.0)] };
        let a = FramePlan::compile(&g, (100, 100), Attachments::default(), &[], false, BIG).unwrap();
        let b = FramePlan::compile(&g.clone(), (100, 100), Attachments::default(), &[], false, BIG).unwrap();
        assert_eq!(a.structure_hash(), b.structure_hash());
        let c = FramePlan::compile(&g, (101, 100), Attachments::default(), &[], false, BIG).unwrap();
        assert_ne!(a.structure_hash(), c.structure_hash());
        let d = FramePlan::compile(&g, (100, 100), Attachments::default(), &[], true, BIG).unwrap();
        assert_ne!(a.structure_hash(), d.structure_hash());
    }
}
