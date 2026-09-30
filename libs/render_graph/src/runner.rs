//! Runs a [`FramePlan`]'s passes on the GPU, one stage at a time, in the
//! slot the host gives each stage.
//!
//! The host renders its scene, calls [`GraphRunner::run_stage`] with the
//! stage's colour (and its attachments), and gets back the colour after the
//! stage plus the pass the producer of the stage's input must parent under
//! (the chain order is producer -> stage passes -> `parent`). An empty stage
//! returns the input unchanged and records nothing: with no nodes the host's
//! chain is exactly what it was.
//!
//! History passes (`history: true`) own a second target: each frame they
//! write one and read the other as `@history`, and the two swap. After the
//! passes change or [`GraphRunner::reset_history`] the history is cold: the
//! slot reads the pass's first input and `history_ready()` is 0, so a pass
//! never samples a texture it has not written in this run (a texture from
//! the pool may still hold another document's frame).

use crate::pass::PassDecl;
use crate::program::{DrawGraphPass, Programs};
use crate::plan::{Attachments, ColorPipeline, FramePlan, PlanError, PostGraph, PassNode, Resource, Source, Stage, Version};
use makepad_draw::*;

pub use crate::PassValues;

/// What the stage's passes may read from the host.
pub struct StageInputs<'a> {
    pub color: &'a Texture,
    pub depth: Option<&'a Texture>,
    pub normal: Option<&'a Texture>,
    pub velocity: Option<&'a Texture>,
    pub glow: Option<&'a Texture>,
    pub id: Option<&'a Texture>,
    /// Textures the host provides by name (LUTs, document images).
    pub named: &'a [(LiveId, Texture)],
}

impl StageInputs<'_> {
    fn get(&self, r: Resource) -> Option<&Texture> {
        match r {
            Resource::Color => Some(self.color),
            Resource::Depth => self.depth,
            Resource::Normal => self.normal,
            Resource::Velocity => self.velocity,
            Resource::Glow => self.glow,
            Resource::Id => self.id,
            Resource::Named(n) => self.named.iter().find(|(k, _)| *k == n).map(|(_, t)| t),
            Resource::History | Resource::Previous(_) => None,
        }
    }
}

/// The camera a host gives passes (raymarching): the inverse of
/// projection x view (column-major, as Makepad's `Mat4f::v`), the eye and
/// the view's forward axis, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassView {
    pub inv_view_proj: [f32; 16],
    pub eye: [f32; 3],
    pub forward: [f32; 3],
    /// Last frame's projection x view (realtime velocity: where a point was
    /// on screen a frame ago); the current one when there is none.
    pub prev_view_proj: [f32; 16],
}
/// The frame's standard-block values.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameUniforms {
    /// The canonical time (seconds).
    pub time: f32,
    /// The frame index.
    pub frame: f32,
    /// The supersampling tap of this sub-frame (-1 = none).
    pub ss_tap: f32,
    /// The frame's seed in 0..1.
    pub seed: f32,
    pub exposure: f32,
    /// The camera for depth reads: projection z terms `proj[10]`,
    /// `proj[14]`, 1 for orthographic, 1 when the depth buffer stores
    /// clip z / w directly (Metal) rather than z * 0.5 + 0.5.
    pub camera: [f32; 4],
}

struct PassSlot {
    pass: DrawPass,
    list: DrawList,
}

/// A history pass's second target and which of the two holds its latest
/// output.
struct HistorySlot {
    /// The plan node it belongs to.
    node: usize,
    twin: Texture,
    /// True: the latest output is in `twin` (the next run writes the plan's
    /// target and reads `twin` as its history).
    in_twin: bool,
    cold: bool,
}

/// The graph's GPU side for one host view.
#[derive(Default)]
pub struct GraphRunner {
    /// The camera passes see (`self.ray_dir()`, `self.eye()`).
    view: Option<PassView>,
    /// Frame pixels per output pixel (the host's supersampling; 0 = 1).
    px_scale: f32,
    decls: Vec<PassDecl>,
    graph: PostGraph,
    plan: Option<FramePlan>,
    plan_key: Option<((u32, u32), Attachments, Vec<LiveId>, bool)>,
    programs: Programs,
    slots: Vec<PassSlot>,
    targets: Vec<(crate::plan::Format, Texture)>,
    history: Vec<HistorySlot>,
    color: Option<ColorPipeline>,
    /// Per stage: the passes the last run recorded, in order.
    last: [Vec<DrawPassId>; 4],
    /// All zeros: a cold history's stand-in when it has no other input.
    zero: Option<Texture>,
    /// Errors to show the author (compile errors, refused plans), taken by
    /// the host.
    pub errors: Vec<String>,
}

impl GraphRunner {
    /// The colour pipeline the plan checks against (default HDR).
    pub fn set_color_pipeline(&mut self, color: ColorPipeline) {
        if self.color != Some(color) {
            self.color = Some(color);
            self.graph.color = color;
            self.plan = None;
            self.plan_key = None;
        }
    }

    /// Set the document's passes (cheap when unchanged). New passes start
    /// with a cold history.
    pub fn set_passes(&mut self, decls: &[PassDecl]) {
        if self.decls.as_slice() == decls {
            return;
        }
        self.decls = decls.to_vec();
        self.history.clear();
        self.graph = PostGraph {
            color: self.color.unwrap_or(ColorPipeline::Hdr),
            nodes: decls
                .iter()
                .map(|d| PassNode {
                    name: d.name.as_deref().map(LiveId::from_str),
                    stage: d.stage,
                    reads: d.resources(),
                    scale: d.scale,
                    size: d.size,
                    format: d.format(),
                    program: d.program_id(),
                    history: d.history,
                    outputs: d.outputs.iter().map(|o| (LiveId::from_str(&o.name), o.format)).collect(),
                    label: d.label.clone(),
                })
                .collect(),
        };
        self.plan = None;
        self.plan_key = None;
        let keep: Vec<_> = self.graph.nodes.iter().map(|n| n.program).collect();
        self.programs.retain(&keep);
    }

    /// The camera the passes see this frame (raymarch passes), or none.
    pub fn set_view(&mut self, view: Option<PassView>) {
        self.view = view;
    }

    /// How many frame pixels one output pixel is (the host's supersampling
    /// factor; 1 by default). Passes read it, times their own scale, as
    /// `self.px_scale()`, so a line N output pixels wide stays N wide at
    /// any supersampling.
    pub fn set_px_scale(&mut self, px_scale: f32) {
        self.px_scale = if px_scale.is_finite() && px_scale > 0.0 { px_scale } else { 1.0 };
    }

    pub fn is_empty(&self) -> bool {
        self.decls.is_empty()
    }

    pub fn has_stage(&self, stage: Stage) -> bool {
        self.decls.iter().any(|d| d.stage == stage)
    }

    /// Plan for this frame. False when the graph cannot run (the reason is
    /// in `errors`); the host then draws without it.
    pub fn prepare(&mut self, cx: &mut Cx, size: (u32, u32), attachments: Attachments, host_named: &[LiveId], locked: bool, budget: u64) -> bool {
        if self.decls.is_empty() {
            return true;
        }
        let key = (size, attachments, host_named.to_vec(), locked);
        let stale = self.plan.is_none() || self.plan_key.as_ref() != Some(&key);
        if stale {
            self.plan_key = Some(key);
            match FramePlan::compile(&self.graph, size, attachments, host_named, locked, budget) {
                Ok(p) => self.plan = Some(p),
                Err(e) => {
                    self.note(format!("post graph refused: {}", PlanError::to_string(&e)));
                    self.plan = None;
                    return false;
                }
            }
        }
        let mut ok = true;
        let mut failed = Vec::new();
        for d in &self.decls {
            match self.programs.compile(cx, d) {
                Ok(Some(_)) => {}
                Ok(None) => ok = false,
                Err(e) => {
                    failed.push(e);
                    ok = false;
                }
            }
        }
        for e in failed {
            self.note(e);
        }
        let plan = self.plan.as_ref().unwrap();
        self.targets.truncate(plan.targets.len());
        for (i, t) in plan.targets.iter().enumerate() {
            match self.targets.get(i) {
                Some((f, _)) if *f == t.format => {}
                _ => {
                    let tex = Texture::new_with_format(cx, t.format.texture_format());
                    if i < self.targets.len() {
                        self.targets[i] = (t.format, tex);
                    } else {
                        self.targets.push((t.format, tex));
                    }
                }
            }
        }
        while self.slots.len() < plan.passes.len() {
            let pass = DrawPass::new_with_name(cx, "graph pass");
            self.slots.push(PassSlot { pass, list: DrawList::new(cx) });
        }
        // Each pass named for what it runs, so a GPU profile says which.
        for p in &plan.passes {
            let label = &self.decls[p.node].label;
            let id = self.slots[p.id.0 as usize].pass.draw_pass_id();
            let named = &cx.passes[id].debug_name;
            if !(named.len() == label.len() + 6 && named.starts_with("graph ") && named.ends_with(label.as_str())) {
                cx.passes[id].debug_name = format!("graph {label}");
            }
        }
        // History twins, kept across plan rebuilds of the same passes (a
        // resize keeps the texture objects; their content is resized).
        let wanted: Vec<(usize, crate::plan::Format)> = plan
            .passes
            .iter()
            .filter(|p| self.graph.nodes[p.node].history)
            .map(|p| (p.node, plan.targets[p.output.0 as usize].format))
            .collect();
        self.history.retain(|h| wanted.iter().any(|(n, _)| *n == h.node));
        for (node, format) in wanted {
            if !self.history.iter().any(|h| h.node == node) {
                let twin = Texture::new_with_format(cx, format.texture_format());
                self.history.push(HistorySlot { node, twin, in_twin: false, cold: true });
            }
        }
        ok
    }

    /// The texture holding a planned version's latest write (a history
    /// pass alternates between its plan target and its twin).
    fn version_texture(&self, v: Version) -> Texture {
        let plan = self.plan.as_ref().unwrap();
        let writer = plan.targets[v.0 as usize].writer;
        let node = plan.passes[writer.0 as usize].node;
        match self.history.iter().find(|h| h.node == node) {
            Some(h) if h.in_twin => h.twin.clone(),
            _ => self.targets[v.0 as usize].1.clone(),
        }
    }

    /// A history pass's latest output (None while it is cold).
    fn latest_of(&self, node: usize) -> Option<Texture> {
        let h = self.history.iter().find(|h| h.node == node)?;
        if h.cold {
            return None;
        }
        let plan = self.plan.as_ref()?;
        let v = plan.passes.iter().find(|p| p.node == node)?.output;
        Some(if h.in_twin { h.twin.clone() } else { self.targets[v.0 as usize].1.clone() })
    }

    /// Start every history cold again (a live unit suspended, a document
    /// reloaded), and drop what the passes recorded, so nothing keeps the
    /// producers they sampled. Targets stay allocated.
    pub fn reset_history(&mut self, cx: &mut Cx) {
        for h in &mut self.history {
            h.in_twin = false;
            h.cold = true;
        }
        for slot in &self.slots {
            let pass = &mut cx.passes[slot.pass.draw_pass_id()];
            pass.main_draw_list_id = None;
            pass.parent = CxDrawPassParent::None;
            pass.attached_by = None;
            pass.paint_dirty = false;
            let items = &mut cx.draw_lists[slot.list.id()].draw_items;
            items.clear();
            items.finish_recording();
        }
        self.last = Default::default();
    }

    /// Whether any pass keeps history.
    pub fn has_history(&self) -> bool {
        self.decls.iter().any(|d| d.history)
    }

    /// What these passes need of locked time: a history pass is refused
    /// there (`locked::check`).
    pub fn locked_usage(&self) -> (crate::locked::Subsystem, crate::locked::Usage) {
        use crate::locked::{Subsystem, Usage};
        (Subsystem::PassHistory, if self.has_history() { Usage::History } else { Usage::Off })
    }

    /// A named pass's output this frame (the current one of a history
    /// pass): simulation state or a generated texture for the host's scene.
    pub fn output(&self, name: &str) -> Option<Texture> {
        let plan = self.plan.as_ref()?;
        let id = LiveId::from_str(name);
        let v = plan.named.iter().rev().find(|(n, _)| *n == id)?.1;
        Some(self.version_texture(v))
    }

    /// The passes the last run of `stage` recorded, first to last.
    pub fn last_run(&self, stage: Stage) -> &[DrawPassId] {
        let si = Stage::ALL.iter().position(|s| *s == stage).unwrap();
        &self.last[si]
    }

    /// Attach the passes of the last run of `stage` again, unchanged (a
    /// host retrying a submission that has not painted yet): nothing is
    /// recorded and no history advances.
    pub fn reattach(&self, cx: &mut Cx2d, stage: Stage, parent: Option<DrawPassId>) {
        let ids = self.last_run(stage);
        for w in ids.windows(2) {
            cx.cx.attach_child_pass(w[0], w[1], None);
        }
        if let Some(last) = ids.last() {
            if let Some(slot) = self.slots.iter().find(|s| s.pass.draw_pass_id() == *last) {
                crate::accum::attach(cx, &slot.pass, parent);
            }
        }
    }

    fn note(&mut self, e: String) {
        if !self.errors.contains(&e) {
            self.errors.push(e);
        }
    }

    /// Record one stage. Returns the colour after it and the first pass of
    /// the stage (the producer of `inputs.color` parents under it), or
    /// `None` when the stage has no passes (nothing recorded). The stage's
    /// last pass parents under `parent` (None: a child of the pass being
    /// drawn).
    pub fn run_stage(&mut self, cx: &mut Cx2d, stage: Stage, inputs: &StageInputs, values: &[PassValues], frame: FrameUniforms, parent: Option<DrawPassId>) -> Option<(Texture, DrawPassId)> {
        let plan = self.plan.as_ref()?;
        let passes: Vec<_> = plan.stage(stage).cloned().collect();
        if passes.is_empty() {
            return None;
        }
        let si = Stage::ALL.iter().position(|s| *s == stage).unwrap();
        let stage_color = plan.stage_color[si];
        let ids: Vec<DrawPassId> = passes.iter().map(|p| self.slots[p.id.0 as usize].pass.draw_pass_id()).collect();
        self.last[si] = ids.clone();
        for w in ids.windows(2) {
            cx.cx.attach_child_pass(w[0], w[1], None);
        }
        let last = passes.last().unwrap().id.0 as usize;
        crate::accum::attach(cx, &self.slots[last].pass, parent);
        let aspect = plan.size.0 as f32 / plan.size.1.max(1) as f32;
        let plan_w = plan.size.0 as f32;
        for p in &passes {
            let program = self.decls[p.node].program_id();
            // A history pass reads its latest output and writes the other
            // of its two textures, which then holds the latest.
            let mut prev = None;
            if let Some(h) = self.history.iter_mut().find(|h| h.node == p.node) {
                if !h.cold {
                    prev = Some(if h.in_twin { h.twin.clone() } else { self.targets[p.output.0 as usize].1.clone() });
                }
                h.in_twin = !h.in_twin;
                h.cold = false;
            }
            let target = self.version_texture(p.output);
            let mut textures: Vec<Option<Texture>> = p
                .inputs
                .iter()
                .map(|s| match *s {
                    Source::Host(r) => inputs.get(r).cloned(),
                    Source::Target(v) => Some(self.version_texture(v)),
                    Source::History => prev.clone(),
                    Source::Previous(node) => self.latest_of(node),
                })
                .collect();
            // A cold history reads the pass's first input instead (zeros
            // when that is the history itself).
            let mut history_ready = 1.0;
            let cold: Vec<usize> = p.inputs.iter().enumerate().filter(|(i, s)| matches!(s, Source::History | Source::Previous(_)) && textures[*i].is_none()).map(|(i, _)| i).collect();
            for i in cold {
                {
                    // `@history` falls back to the pass's first input, a
                    // cold `name.prev` to zeros.
                    let first = if matches!(p.inputs[i], Source::History) { textures[0].clone() } else { None };
                    textures[i] = match first {
                        Some(t) => Some(t),
                        None => Some(
                            self.zero
                                .get_or_insert_with(|| {
                                    Texture::new_with_format(
                                        cx.cx,
                                        TextureFormat::VecRGBAf32 { width: 1, height: 1, data: Some(vec![0.0; 4]), updated: TextureUpdated::Full },
                                    )
                                })
                                .clone(),
                        ),
                    };
                    history_ready = 0.0;
                }
            }
            let decl = &self.decls[p.node];
            let Some(draw) = self.programs.get_mut(program) else { continue };
            let dv = &mut draw.draw_super.draw_vars;
            // A pass whose uniforms do not fit a draw call is refused by
            // name instead of writing past the call's uniforms.
            if let Some(sid) = dv.draw_shader_id {
                let total = cx.cx.draw_shaders[sid.index].mapping.dyn_uniforms.total_slots;
                let room = dv.dyn_uniforms.len();
                if total > room {
                    let e = format!("{}: its uniforms (with the standard block's) take {total} floats, over the {room} a pass can have; pack parameters into fewer vectors, or read tables from a named texture", decl.label);
                    if !self.errors.contains(&e) {
                        self.errors.push(e);
                    }
                    continue;
                }
            }
            for (i, t) in textures.iter().enumerate() {
                if let Some(t) = t {
                    dv.set_texture(i, t);
                }
            }
            let (w, h) = (p.size.0 as f32, p.size.1 as f32);
            dv.set_uniform(cx.cx, live_id!(g_frame), &[frame.time, frame.frame, frame.ss_tap, frame.seed]);
            dv.set_uniform(cx.cx, live_id!(g_size), &[w, h, 1.0 / w, 1.0 / h]);
            let base = if self.px_scale > 0.0 { self.px_scale } else { 1.0 };
            let pass_px = base * w / plan_w.max(1.0);
            dv.set_uniform(cx.cx, live_id!(g_misc), &[frame.exposure, aspect, history_ready, pass_px]);
            dv.set_uniform(cx.cx, live_id!(g_cam), &frame.camera);
            if let Some(v) = &self.view {
                // Rows of the column-major matrix.
                let m = &v.inv_view_proj;
                dv.set_uniform(cx.cx, live_id!(g_ivp0), &[m[0], m[4], m[8], m[12]]);
                dv.set_uniform(cx.cx, live_id!(g_ivp1), &[m[1], m[5], m[9], m[13]]);
                dv.set_uniform(cx.cx, live_id!(g_ivp2), &[m[2], m[6], m[10], m[14]]);
                dv.set_uniform(cx.cx, live_id!(g_ivp3), &[m[3], m[7], m[11], m[15]]);
                dv.set_uniform(cx.cx, live_id!(g_eye), &[v.eye[0], v.eye[1], v.eye[2], 1.0]);
                dv.set_uniform(cx.cx, live_id!(g_fwd), &[v.forward[0], v.forward[1], v.forward[2], 0.0]);
                let p = &v.prev_view_proj;
                dv.set_uniform(cx.cx, live_id!(g_pvp0), &[p[0], p[4], p[8], p[12]]);
                dv.set_uniform(cx.cx, live_id!(g_pvp1), &[p[1], p[5], p[9], p[13]]);
                dv.set_uniform(cx.cx, live_id!(g_pvp2), &[p[2], p[6], p[10], p[14]]);
                dv.set_uniform(cx.cx, live_id!(g_pvp3), &[p[3], p[7], p[11], p[15]]);
            }
            if let Some(vals) = values.get(p.node) {
                for (u, v) in decl.uniforms.iter().zip(vals.iter()) {
                    dv.set_uniform(cx.cx, LiveId::from_str(&u.name), &v[..u.width as usize]);
                }
            }
            let extra: Vec<Texture> = p.extra.iter().map(|v| self.version_texture(*v)).collect();
            let Some(draw) = self.programs.get_mut(program) else { continue };
            let slot = &mut self.slots[p.id.0 as usize];
            record(cx, slot, dvec2(w as f64, h as f64), &target, &extra, draw);
        }
        let out = stage_color.map(|v| self.version_texture(v)).unwrap_or_else(|| inputs.color.clone());
        Some((out, ids[0]))
    }

    /// The passes of `stage` in the current plan, in order (for a host's
    /// painted check).
    pub fn pass_ids(&self, stage: Stage) -> Vec<DrawPassId> {
        self.plan.as_ref().map_or(Vec::new(), |p| p.stage(stage).map(|pp| self.slots[pp.id.0 as usize].pass.draw_pass_id()).collect())
    }

    /// One of this runner's passes by id (a host re-attaches the last).
    pub fn draw_pass(&self, id: DrawPassId) -> Option<&DrawPass> {
        self.slots.iter().map(|s| &s.pass).find(|p| p.draw_pass_id() == id)
    }

    /// The current plan.
    pub fn plan(&self) -> Option<&FramePlan> {
        self.plan.as_ref()
    }
}

/// Record one pass: `target` is attachment 0, `extra` the further outputs
/// (MRT attachments 1..).
fn record(cx: &mut Cx2d, slot: &mut PassSlot, size: DVec2, target: &Texture, extra: &[Texture], draw: &mut DrawGraphPass) {
    let id = slot.pass.draw_pass_id();
    let parent = cx.cx.passes[id].parent.clone();
    slot.pass.set_size(cx, size);
    slot.pass.clear_color_textures(cx.cx);
    slot.pass.set_color_texture(cx, target, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
    for t in extra {
        slot.pass.add_color_texture(cx.cx, t, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
    }
    cx.cx.passes[id].depth_texture = None;
    cx.cx.passes[id].parent = parent;
    cx.begin_pass(&slot.pass, Some(1.0));
    slot.pass.set_size(cx, size);
    slot.list.begin_always(cx);
    let pass_size = cx.current_pass_size();
    cx.begin_root_turtle(pass_size, Layout::flow_overlay());
    draw.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size });
    cx.end_pass_sized_turtle();
    slot.list.end(cx);
    cx.end_pass(&slot.pass);
}
