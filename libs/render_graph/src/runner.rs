//! Runs a [`FramePlan`]'s passes on the GPU, one stage at a time, in the
//! slot the host gives each stage.
//!
//! The host renders its scene, calls [`GraphRunner::run_stage`] with the
//! stage's colour (and its attachments), and gets back the colour after the
//! stage plus the pass the producer of the stage's input must parent under
//! (the chain order is producer -> stage passes -> `parent`). An empty stage
//! returns the input unchanged and records nothing: with no nodes the host's
//! chain is exactly what it was.

use crate::pass::PassDecl;
use crate::program::{DrawGraphPass, Programs};
use crate::plan::{Attachments, FramePlan, PlanError, PostGraph, PassNode, Resource, Source, Stage};
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
        }
    }
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

/// The graph's GPU side for one host view.
#[derive(Default)]
pub struct GraphRunner {
    decls: Vec<PassDecl>,
    graph: PostGraph,
    plan: Option<FramePlan>,
    plan_key: Option<((u32, u32), Attachments, Vec<LiveId>, bool)>,
    programs: Programs,
    slots: Vec<PassSlot>,
    targets: Vec<(crate::plan::Format, Texture)>,
    /// Errors to show the author (compile errors, refused plans), taken by
    /// the host.
    pub errors: Vec<String>,
}

impl GraphRunner {
    /// Set the document's passes (cheap when unchanged).
    pub fn set_passes(&mut self, decls: &[PassDecl]) {
        if self.decls.as_slice() == decls {
            return;
        }
        self.decls = decls.to_vec();
        self.graph = PostGraph {
            color: crate::plan::ColorPipeline::Hdr,
            nodes: decls
                .iter()
                .map(|d| PassNode {
                    name: d.name.as_deref().map(LiveId::from_str),
                    stage: d.stage,
                    reads: d.resources(),
                    scale: d.scale,
                    format: d.format(),
                    program: d.program_id(),
                    label: d.label.clone(),
                })
                .collect(),
        };
        self.plan = None;
        self.plan_key = None;
        let keep: Vec<_> = self.graph.nodes.iter().map(|n| n.program).collect();
        self.programs.retain(&keep);
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
        ok
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
        let out = plan.stage_color[si].map(|v| self.targets[v.0 as usize].1.clone()).unwrap_or_else(|| inputs.color.clone());
        let ids: Vec<DrawPassId> = passes.iter().map(|p| self.slots[p.id.0 as usize].pass.draw_pass_id()).collect();
        for w in ids.windows(2) {
            cx.cx.attach_child_pass(w[0], w[1], None);
        }
        let last = passes.last().unwrap().id.0 as usize;
        crate::accum::attach(cx, &self.slots[last].pass, parent);
        let aspect = plan.size.0 as f32 / plan.size.1.max(1) as f32;
        for p in &passes {
            let decl = &self.decls[p.node];
            let program = decl.program_id();
            let target = self.targets[p.output.0 as usize].1.clone();
            let textures: Vec<Option<Texture>> = p
                .inputs
                .iter()
                .map(|s| match *s {
                    Source::Host(r) => inputs.get(r).cloned(),
                    Source::Target(v) => Some(self.targets[v.0 as usize].1.clone()),
                })
                .collect();
            let Some(draw) = self.programs.get_mut(program) else { continue };
            let dv = &mut draw.draw_super.draw_vars;
            for (i, t) in textures.iter().enumerate() {
                if let Some(t) = t {
                    dv.set_texture(i, t);
                }
            }
            let (w, h) = (p.size.0 as f32, p.size.1 as f32);
            dv.set_uniform(cx.cx, live_id!(g_frame), &[frame.time, frame.frame, frame.ss_tap, frame.seed]);
            dv.set_uniform(cx.cx, live_id!(g_size), &[w, h, 1.0 / w, 1.0 / h]);
            dv.set_uniform(cx.cx, live_id!(g_misc), &[frame.exposure, aspect, 0.0, 0.0]);
            dv.set_uniform(cx.cx, live_id!(g_cam), &frame.camera);
            if let Some(vals) = values.get(p.node) {
                for (u, v) in decl.uniforms.iter().zip(vals.iter()) {
                    dv.set_uniform(cx.cx, LiveId::from_str(&u.name), &v[..u.width as usize]);
                }
            }
            let slot = &mut self.slots[p.id.0 as usize];
            record(cx, slot, dvec2(w as f64, h as f64), &target, draw);
        }
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

fn record(cx: &mut Cx2d, slot: &mut PassSlot, size: DVec2, target: &Texture, draw: &mut DrawGraphPass) {
    let id = slot.pass.draw_pass_id();
    let parent = cx.cx.passes[id].parent.clone();
    slot.pass.set_size(cx, size);
    slot.pass.clear_color_textures(cx.cx);
    slot.pass.set_color_texture(cx, target, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
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
