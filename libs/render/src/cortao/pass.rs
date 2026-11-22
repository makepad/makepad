// Source: src/shaders/CORTAOMainPass.h, CORTAO v1.0.2, commit f1b18d7772.
// MIT License
//
// Copyright (c) 2026 Daniel Isheden
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//
// Modified:
// - Generate per-tap constants on the CPU in f64 and read them from a uniform
//   array (two vec4 per tap), padded to 48 entries and rebuilt on count changes.
// - Choose sample/ray/filter counts through four runtime tiers; the main tap
//   loop runs to a runtime count under a literal bound of 24.
// - Mix both words of a frame counter and upload the seed as exact 16-bit halves.
// - Own a three-stage fragment chain, fixed-size atlas/MRT and R8 targets,
//   shared runtime LUTs, replaceable surface draw, readiness and GPU timings.
// - Reject invalid frame inputs before computing uniforms or recording passes.
// - A temporal resolve stage (not in CORTAO) with ping-pong RGBA16F history,
//   reprojected by the previous depth's camera; hosts opt in per frame.
// - An optional half-resolution chain with a full-resolution depth-aware
//   upsample, preserving the host's prepass surface hook and output size.

//! A self-contained CORTAO chain: packed depth and normals → visibility → filter
//! → (with history) the temporal resolve → (at half resolution) the upsample.

use super::params::*;
use super::shaders::{
    DrawCortaoFilter, DrawCortaoMain, DrawCortaoPrepass, DrawCortaoResolve, DrawCortaoUpsample,
};
use super::tables::{occluder_table_halves, occluder_table_rf32};
use makepad_draw::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CortaoTier {
    Low,
    Medium,
    High,
    Ultra,
}

impl CortaoTier {
    pub fn samples(self) -> u32 {
        match self {
            Self::Low => 4,
            Self::Medium => 8,
            Self::High => 12,
            Self::Ultra => 24,
        }
    }

    pub fn rays(self) -> u32 {
        if self == Self::Ultra {
            32
        } else {
            16
        }
    }

    pub fn large_filter(self) -> bool {
        self != Self::Low
    }

    pub fn symmetric_filter(self) -> bool {
        self != Self::Low
    }
}

/// The occluder table texture for one ray count (24×24×16), shared by every view.
pub struct CortaoTable {
    rays: u32,
    texture: Texture,
}

impl CortaoTable {
    /// Panics for ray counts other than the two supported by [`CortaoTier`].
    pub fn new(cx: &mut Cx, rays: u32) -> Self {
        assert!(matches!(rays, 16 | 32), "CORTAO tiers use 16 or 32 rays");
        let format = if rays == 16 {
            TextureFormat::VecRf32 {
                width: 24,
                height: 24 * 16,
                data: Some(occluder_table_rf32(24, 16, rays)),
                updated: TextureUpdated::Full,
            }
        } else {
            TextureFormat::VecRGBAf32 {
                width: 24,
                height: 24 * 16,
                data: Some(
                    occluder_table_halves(24, 16, rays)
                        .into_iter()
                        .flat_map(|[lo, hi]| [lo, hi, 0.0, 0.0])
                        .collect(),
                ),
                updated: TextureUpdated::Full,
            }
        };
        Self {
            rays,
            texture: Texture::new_with_format(cx, format),
        }
    }

    pub fn rays(&self) -> u32 {
        self.rays
    }
}

/// Which frame the depth holds. The resolve reprojects with that depth's own
/// camera either way; locked time refuses `Previous` (see `locked_usage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CortaoDepth {
    Current,
    Previous,
}

pub struct CortaoInput<'a> {
    /// Layout points; the depth's device pixels are `size * dpi`, rounded.
    pub size: DVec2,
    /// Hardware depth (clip z/w), sampled by the default surface callback.
    pub depth: &'a Texture,
    pub depth_frame: CortaoDepth,
    pub camera: CortaoCamera,
    pub tier: CortaoTier,
    pub config: CortaoConfig,
    /// The host's frame counter, mixed into the shader seed (never wall time,
    /// so a replay is exact; synced views pass the same counter).
    pub frame: u64,
    /// Blend this frame's AO into a history (realtime hosts). A host that
    /// accumulates itself (StillAa, locked time) passes false and moves `frame`.
    pub history: bool,
    /// Start the history again: a camera cut or a jump of the clip matrix (the
    /// host knows; the pass does not guess). A resize or the first frame resets
    /// it without being asked, as does a change of internal resolution.
    pub reset: bool,
    /// Must have the ray count selected by `tier`.
    pub table: &'a CortaoTable,
}

impl CortaoInput<'_> {
    /// What this frame asks of locked time: history or a previous frame's depth
    /// is refused there.
    pub fn locked_usage(&self) -> makepad_render_graph::locked::Usage {
        if self.history || self.depth_frame == CortaoDepth::Previous {
            makepad_render_graph::locked::Usage::History
        } else {
            makepad_render_graph::locked::Usage::Synchronous
        }
    }
}

pub enum CortaoOutput {
    /// At the input's device size, red is visibility (one means unoccluded):
    /// R8 at Half or without history; at Full with history, RGBA16F (green: depth).
    Visibility(Texture),
}

/// Murmur3's 32-bit finalizer, with wrapping arithmetic on both frame words.
pub fn frame_seed(frame: u64) -> u32 {
    fn mix(mut x: u32) -> u32 {
        x ^= x >> 16;
        x = x.wrapping_mul(0x85ebca6b);
        x ^= x >> 13;
        x = x.wrapping_mul(0xc2b2ae35);
        x ^ (x >> 16)
    }
    mix((frame as u32).wrapping_add(0x9e3779b9) ^ mix((frame >> 32) as u32 ^ 0x85ebca6b))
}

/// Two vec4 per tap: inner/outer spiral xy, then weight/mip/zero/zero.
/// An empty request returns an empty table; tiers always request at least four.
pub fn tap_table(samples: u32) -> Vec<[f32; 4]> {
    let mut taps = Vec::with_capacity(samples as usize * 2);
    for i in 0..samples {
        let angle = i as f64 * 2.3999632297286533222315555066336;
        // The skew pulls samples toward the centre of the sampling circle.
        let inner = (i as f64 / samples as f64).powf(1.75);
        let outer = ((i as f64 + 1.0) / samples as f64).powf(1.75);
        let (sin, cos) = angle.sin_cos();
        taps.push([inner * cos, inner * sin, outer * cos, outer * sin].map(|v| v as f32));
        // Square-root the area because the weight multiplies a radius.
        taps.push([
            (outer * outer - inner * inner).sqrt() as f32,
            outer.log2() as f32,
            0.0,
            0.0,
        ]);
    }
    taps
}

/// How much of the reprojected history a frame keeps.
const HISTORY_WEIGHT: f32 = 0.85;
const STAGE_NAMES: [&str; 5] = [
    "ssao prepass",
    "ssao main",
    "ssao filter",
    "ssao resolve",
    "ssao upsample",
];

#[derive(Clone, Copy)]
enum StageColor {
    R32F,
    R8,
    Rgba16F,
}

struct StageColors<const N: usize>([StageColor; N]);

const PREPASS_COLORS: StageColors<2> = StageColors([StageColor::R32F; 2]);
const MAIN_COLORS: StageColors<1> = StageColors([StageColor::R8]);
const FILTER_COLORS: StageColors<1> = StageColors([StageColor::R8]);
const RESOLVE_COLORS: StageColors<1> = StageColors([StageColor::Rgba16F]);
const UPSAMPLE_COLORS: StageColors<1> = StageColors([StageColor::R8]);

impl<const N: usize> StageColors<N> {
    fn formats(&self, size: TextureSize) -> [TextureFormat; N] {
        self.0.map(|color| {
            let size = size.clone();
            match color {
                StageColor::R32F => TextureFormat::RenderRf32 { size, initial: true },
                StageColor::R8 => TextureFormat::RenderRu8 { size, initial: true },
                StageColor::Rgba16F => TextureFormat::RenderRGBAf16 { size, initial: true },
            }
        })
    }

    fn layout(&self) -> PassLayout {
        PassLayout::offscreen(&self.formats(TextureSize::Auto), None)
    }
}

const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

struct CortaoStage {
    pass: DrawPass,
    list: DrawList,
}

struct CortaoTargets {
    full_size: [u32; 2],
    half: bool,
    atlas: CortaoAtlas,
    packed: Texture,
    normals: Texture,
    raw: Texture,
    output: Texture,
    /// Only Half allocates a separate, full-resolution R8 output.
    upsampled: Option<Texture>,
    /// The resolve's two RGBA16F targets, made on the first frame with history.
    history: Option<[Texture; 2]>,
}

impl CortaoTargets {
    fn new(
        cx: &mut Cx,
        full_size: [u32; 2],
        [width, height]: [u32; 2],
        half: bool,
    ) -> Self {
        let atlas = CortaoAtlas::new(width, height);
        let atlas_size = TextureSize::Fixed {
            width: atlas.width as usize,
            height: atlas.height as usize,
        };
        let size = TextureSize::Fixed {
            width: width as usize,
            height: height as usize,
        };
        let [packed, normals] = PREPASS_COLORS.formats(atlas_size);
        let [raw] = MAIN_COLORS.formats(size.clone());
        let [output] = FILTER_COLORS.formats(size);
        Self {
            full_size,
            half,
            atlas,
            history: None,
            upsampled: half.then(|| {
                let [upsampled] = UPSAMPLE_COLORS.formats(TextureSize::Fixed {
                    width: full_size[0] as usize,
                    height: full_size[1] as usize,
                });
                Texture::new_with_format(cx, upsampled)
            }),
            packed: Texture::new_with_format(cx, packed),
            normals: Texture::new_with_format(cx, normals),
            raw: Texture::new_with_format(cx, raw),
            output: Texture::new_with_format(cx, output),
        }
    }
}

/// Each stage owns its target and draw: no later stage writes an earlier input,
/// so a painted chain stays painted. Parent the depth producer to `first_pass_id`.
#[derive(Default)]
pub struct CortaoPass {
    scene_time: Option<f32>,
    stages: Vec<CortaoStage>,
    targets: Option<CortaoTargets>,
    tap_uniforms: Option<UniformBuffer>,
    tap_uniform_samples: u32,
    draw_prepass: Option<Box<DrawCortaoPrepass>>,
    draw_main: Option<Box<DrawCortaoMain>>,
    draw_filter: Option<Box<DrawCortaoFilter>>,
    draw_resolve: Option<Box<DrawCortaoResolve>>,
    draw_upsample: Option<Box<DrawCortaoUpsample>>,
    upsample_source: Option<(DrawShaderId, u64)>,
    /// The half-size upsample cannot run here: the host prepass binds so many
    /// textures that it (two more) would not fit a draw call's slots, or its
    /// pipeline failed to build. Said once, then the chain stays Full.
    upsample_unfit: bool,
    ready: bool,
    /// The history target written last and the camera of the depth it was
    /// resolved for; None until a frame with history ran on these targets.
    history_last: Option<(usize, CortaoCamera)>,
    /// The resolve's `u_current` as the last `run` set it ([`CortaoPass::forget_history`]).
    resolve_current: [f32; 4],
    /// The GPU cannot render float colour targets: said once, then no AO.
    no_float_targets_logged: bool,
    /// The last `run` drew nothing only because a pipeline (or the VM) was
    /// not ready yet: a host settling a still frame draws again.
    pipelines_pending: bool,
    /// A pipeline of the chain failed to build: it never answers, so there is
    /// no AO and nothing to wait for (said once per failure).
    pipelines_failed: bool,
    pub stage_gpu_ms: [f64; 5],
    pub stage_gpu_min_ms: [f64; 5],
    pub gpu_samples: u64,
}

impl CortaoPass {
    pub(crate) fn set_scene_time(&mut self, cx: &mut Cx, time: Option<f32>) {
        self.scene_time = time;
        for stage in &self.stages { stage.pass.set_own_time(cx, time); }
    }

    /// Prepare draw structs and ask whether their GPU pipelines can draw.
    /// False while the VM is held elsewhere or a pipeline is still compiling.
    pub fn prepare_draws(&mut self, cx: &mut Cx) -> bool {
        if self.draw_prepass.is_none() {
            self.draw_prepass =
                cx.try_with_vm(|vm| Box::new(DrawCortaoPrepass::script_new_with_default(vm)));
        }
        if self.draw_main.is_none() {
            self.draw_main =
                cx.try_with_vm(|vm| Box::new(DrawCortaoMain::script_new_with_default(vm)));
        }
        if self.draw_filter.is_none() {
            self.draw_filter =
                cx.try_with_vm(|vm| Box::new(DrawCortaoFilter::script_new_with_default(vm)));
        }
        if self.draw_resolve.is_none() {
            self.draw_resolve =
                cx.try_with_vm(|vm| Box::new(DrawCortaoResolve::script_new_with_default(vm)));
        }
        let stages = [
            (self.draw_prepass.as_ref().and_then(|d| d.draw_vars.draw_shader_id), PREPASS_COLORS.layout()),
            (self.draw_main.as_ref().and_then(|d| d.draw_vars.draw_shader_id), MAIN_COLORS.layout()),
            (self.draw_filter.as_ref().and_then(|d| d.draw_vars.draw_shader_id), FILTER_COLORS.layout()),
            (self.draw_resolve.as_ref().and_then(|d| d.draw_vars.draw_shader_id), RESOLVE_COLORS.layout()),
        ];
        // Probe every pipeline even when another is pending, to queue them together.
        self.ready = stages.iter().fold(true, |ready, (id, layout)| {
            id.is_some_and(|id| cx.draw_shader_ready_for(id, layout)) && ready
        });
        let failed = !self.ready && stages.iter().any(|(id, _)| id.is_some_and(|id| cx.draw_shader_failed(id)));
        if failed && !self.pipelines_failed {
            log!("CORTAO: a pipeline of the AO chain failed to build: screen AO is off");
        }
        self.pipelines_failed = failed;
        self.ready
    }

    /// The last `run` returned nothing only because its pipelines are still
    /// being made (native compiles them in the background): try again.
    pub fn pipelines_pending(&self) -> bool {
        self.pipelines_pending
    }

    /// A pipeline of the chain failed to build (as of the last
    /// `prepare_draws`/`ensure`/`run`): no AO comes, and no
    /// `Event::DrawShadersReady` will say otherwise, so a host stops waiting.
    pub fn pipelines_failed(&self) -> bool {
        self.pipelines_failed
    }

    /// Readiness at the last `prepare_draws`/`ensure`/`run` call.
    pub fn draws_ready(&self) -> bool {
        self.ready
    }

    pub fn ensure(&mut self, cx: &mut Cx) -> bool {
        self.ensure_stages(cx, 4);
        self.prepare_draws(cx)
    }

    fn ensure_stages(&mut self, cx: &mut Cx, count: usize) {
        while self.stages.len() < count {
            let pass = crate::renderer::new_scene_pass(cx, STAGE_NAMES[self.stages.len()], self.scene_time);
            pass.set_gpu_timing_enabled(cx, true);
            self.stages.push(CortaoStage {
                pass,
                list: DrawList::new(cx),
            });
        }
    }

    /// Derive the upsample only for Half, from the actual host shader's heap.
    fn prepare_upsample(&mut self, cx: &mut Cx) -> bool {
        let Some(prepass) = self.draw_prepass.as_mut() else {
            return false;
        };
        prepass.draw_vars.refresh_after_reload(cx);
        let Some(id) = prepass.draw_vars.draw_shader_id else {
            return false;
        };
        let key = (id, cx.reload_generation);
        if self.upsample_source != Some(key) || self.draw_upsample.is_none() {
            self.draw_upsample = None;
            let source = cx.draw_shaders[id.index].mapping.source.clone();
            if cx.is_script_vm_held() {
                return false;
            }
            cx.with_vm_of_heap(source.heap_key(), &mut |vm| {
                self.draw_upsample = DrawCortaoUpsample::from_prepass(vm, source.as_object());
            });
            self.upsample_source = Some(key);
            let textures = self
                .draw_upsample
                .as_ref()
                .and_then(|draw| draw.draw_vars.draw_shader_id)
                .map_or(0, |id| cx.draw_shaders[id.index].mapping.textures.len());
            if textures > DRAW_CALL_TEXTURE_SLOTS {
                log!(
                    "CORTAO: the host prepass binds too many textures for the half-size upsample ({textures} > {DRAW_CALL_TEXTURE_SLOTS}); AO runs at full size"
                );
                self.upsample_unfit = true;
                self.draw_upsample = None;
                return false;
            }
        }
        let prepass_ready = cx.draw_shader_ready_for(id, &PREPASS_COLORS.layout());
        let upsample = self
            .draw_upsample
            .as_ref()
            .and_then(|draw| draw.draw_vars.draw_shader_id);
        let ready = upsample.is_some_and(|id| cx.draw_shader_ready_for(id, &UPSAMPLE_COLORS.layout()))
            && prepass_ready;
        if !ready && upsample.is_some_and(|id| cx.draw_shader_failed(id)) {
            log!("CORTAO: the half-size upsample's pipeline failed to build; AO runs at full size");
            self.upsample_unfit = true;
            self.draw_upsample = None;
            return false;
        }
        ready
    }

    /// Replace the prepass by a host-derived draw with surface callbacks.
    /// Half mirrors its surface hook into the upsample automatically.
    pub fn set_prepass_draw(&mut self, draw: Box<DrawCortaoPrepass>) {
        self.draw_prepass = Some(draw);
        self.draw_upsample = None;
        self.upsample_source = None;
        self.upsample_unfit = false;
        self.history_last = None;
        self.ready = false;
    }

    /// Bind the host's additional surface textures and uniforms here.
    /// Half copies these bindings to the upsample on every run.
    pub fn prepass_draw_mut(&mut self) -> Option<&mut DrawCortaoPrepass> {
        self.draw_prepass.as_deref_mut()
    }

    /// What the retained passes read changed in place, without a new [`CortaoPass::run`] (a
    /// host moving a cut through its retained depth): their next paint is a reset run's at
    /// `frame`, its seed and no history blended, and the next run starts the history afresh.
    pub fn forget_history(&mut self, cx: &mut Cx, frame: u64) {
        let seed = frame_seed(frame);
        let seed = [(seed & 65535) as f32, (seed >> 16) as f32, 0.0, 0.0];
        let list = |stage: &CortaoStage| {
            Area::Rect(RectArea {
                draw_list_id: stage.list.id(),
                rect_id: 0,
                redraw_id: 0,
            })
        };
        if let (Some(draw), Some(stage)) = (self.draw_prepass.as_mut(), self.stages.get(0)) {
            let dv = &mut draw.draw_super.draw_vars;
            dv.set_uniform_on_draw_list(cx, list(stage), live_id!(u_seed), &seed);
        }
        if let (Some(draw), Some(stage)) = (self.draw_main.as_mut(), self.stages.get(1)) {
            let dv = &mut draw.draw_super.draw_vars;
            dv.set_uniform_on_draw_list(cx, list(stage), live_id!(u_seed), &seed);
        }
        if self.history_last.take().is_none() {
            return;
        }
        if let (Some(draw), Some(stage)) = (self.draw_resolve.as_mut(), self.stages.get(3)) {
            self.resolve_current[2] = 0.0;
            let dv = &mut draw.draw_super.draw_vars;
            dv.set_uniform_on_draw_list(cx, list(stage), live_id!(u_current), &self.resolve_current);
        }
    }

    pub fn first_pass_id(&self) -> Option<DrawPassId> {
        self.stages.first().map(|s| s.pass.draw_pass_id())
    }

    /// Sum of the latest per-stage GPU times, ms.
    pub fn gpu_ms(&self) -> f64 {
        self.stage_gpu_ms.iter().sum()
    }

    /// Sum of the minimum per-stage GPU times, ms (the uncontended floor).
    pub fn gpu_min_ms(&self) -> f64 {
        self.stage_gpu_min_ms.iter().sum()
    }

    /// Child passes of `parent`: prepass → main → filter → optional resolve/upsample.
    pub fn run(
        &mut self,
        cx: &mut Cx2d,
        input: &CortaoInput<'_>,
        parent: DrawPassId,
    ) -> Option<CortaoOutput> {
        self.pipelines_pending = false;
        // The atlas is R32F: a GPU that cannot render float targets (WebGL2 without
        // EXT_color_buffer_float) gets no AO rather than a pass that never draws.
        if !cx.cx.gpu_info().float_color_targets {
            if !self.no_float_targets_logged {
                self.no_float_targets_logged = true;
                log!("CORTAO: this GPU renders no float colour targets (EXT_color_buffer_float): screen AO is off");
            }
            return None;
        }
        let dpi = cx.current_dpi_factor();
        let full_size @ [full_width, full_height] = frame_size(input.size, dpi)?;
        let half = !self.upsample_unfit && input.config.resolution.is_half(full_size);
        let [width, height] = if half {
            [full_width.div_ceil(2), full_height.div_ceil(2)]
        } else {
            full_size
        };
        if !valid_parameters(&input.camera, &input.config) {
            return None;
        }
        debug_assert_eq!(
            input.table.rays(),
            input.tier.rays(),
            "CORTAO table/tier ray count"
        );
        if input.table.rays() != input.tier.rays() {
            return None;
        }
        let main = main_params(
            &input.camera,
            width,
            height,
            frame_seed(input.frame),
            &input.config,
        );
        let filter = filter_params(width, height, &input.config);
        // Finite inputs can still overflow when inverted or converted to f32.
        let geometry = [
            main.unproject_skew,
            main.projection_skew,
            main.normal_bias,
            main.radius,
        ];
        let limits = [
            main.max_apparent_circle_radius_coefficient,
            main.mip_bias,
            main.inverse_max_slope_compensation,
            main.backface_threshold,
        ];
        if !main
            .unproject
            .iter()
            .chain(&geometry)
            .chain(&limits)
            .all(|v| v.is_finite())
            || main.normal_bias <= 0.0
            || main.inverse_max_slope_compensation <= 0.0
            || (!input.camera.ortho && main.max_apparent_circle_radius_coefficient <= 0.0)
        {
            return None;
        }
        if !self.ensure(cx.cx) {
            // A failed pipeline never answers: nothing to wait for.
            self.pipelines_pending = !self.pipelines_failed;
            return None;
        }
        if half {
            self.ensure_stages(cx.cx, 5);
            self.ready = self.prepare_upsample(cx.cx);
            if !self.ready {
                // An upsample that cannot fit falls back to Full next frame.
                self.pipelines_pending = !self.upsample_unfit;
                return None;
            }
        } else {
            if self.stages.len() > 4 {
                self.stages[4].pass.withdraw(cx.cx);
                self.stages.truncate(4);
            }
            self.draw_upsample = None;
            self.upsample_source = None;
            self.stage_gpu_ms[4] = 0.0;
            self.stage_gpu_min_ms[4] = 0.0;
        }
        if self
            .targets
            .as_ref()
            .is_none_or(|t| t.full_size != full_size || t.half != half)
        {
            self.targets = Some(CortaoTargets::new(
                cx.cx,
                full_size,
                [width, height],
                half,
            ));
            self.history_last = None;
        }
        let samples = input.tier.samples();
        if self.tap_uniforms.is_none() || self.tap_uniform_samples != samples {
            let mut data = tap_table(samples);
            data.resize(48, [0.0; 4]);
            let buffer = UniformBuffer::new(cx.cx);
            buffer.set_struct_slice(cx.cx, &data);
            self.tap_uniforms = Some(buffer);
            self.tap_uniform_samples = samples;
        }
        let targets = self.targets.as_mut()?;
        if !input.history {
            targets.history = None;
        } else if targets.history.is_none() {
            let size = TextureSize::Fixed {
                width: width as usize,
                height: height as usize,
            };
            let [history] = RESOLVE_COLORS.formats(size);
            targets.history = Some([
                Texture::new_with_format(cx.cx, history.clone()),
                Texture::new_with_format(cx.cx, history),
            ]);
        }
        if !input.history {
            self.stages[3].pass.withdraw(cx.cx);
        }
        let after_resolve = if half {
            self.stages[4].pass.draw_pass_id()
        } else {
            parent
        };
        let after_filter = if input.history {
            self.stages[3].pass.draw_pass_id()
        } else {
            after_resolve
        };
        let after_main = self.stages[2].pass.draw_pass_id();
        let after_prepass = self.stages[1].pass.draw_pass_id();
        let targets = self.targets.as_ref()?;
        let atlas = &targets.atlas;
        let size = dvec2(width as f64 / dpi, height as f64 / dpi);
        let atlas_size = dvec2(atlas.width as f64 / dpi, atlas.height as f64 / dpi);
        let dims = [
            main.resolution[0],
            main.resolution[1],
            atlas.levels as f32,
            atlas.height as f32,
        ];
        let offsets_a = std::array::from_fn::<_, 4, _>(|i| atlas.offsets[i + 1][0] as f32);
        let offsets_b = [
            atlas.offsets[5][0] as f32,
            atlas.offsets[6][0] as f32,
            atlas.offsets[7][0] as f32,
            0.0,
        ];
        let seed = [
            (main.random_seed & 65535) as f32,
            (main.random_seed >> 16) as f32,
            0.0,
            0.0,
        ];
        let mode = [if input.camera.ortho { 1.0 } else { 0.0 }];
        let set_common = |cx: &Cx, dv: &mut DrawVars| {
            dv.set_uniform(cx, live_id!(u_size), &dims);
            dv.set_uniform(cx, live_id!(u_offsets_a), &offsets_a);
            dv.set_uniform(cx, live_id!(u_offsets_b), &offsets_b);
            dv.set_uniform(cx, live_id!(u_seed), &seed);
            dv.set_uniform(cx, live_id!(u_proj_mode), &mode);
            dv.set_uniform(cx, live_id!(u_depth_code), &input.camera.depth_code);
        };

        let prepass = self.draw_prepass.as_mut()?;
        let dv = &mut prepass.draw_super.draw_vars;
        set_common(cx.cx, dv);
        dv.set_texture(0, input.depth);
        dv.set_uniform(
            cx.cx,
            live_id!(u_view_from_clip),
            &input.camera.view_from_clip,
        );
        run_stage(
            cx,
            &mut self.stages,
            0,
            after_prepass,
            dpi,
            atlas_size,
            &[&targets.packed, &targets.normals],
            &mut |cx, rect| prepass.draw_abs(cx, rect),
        );

        let draw_main = &mut self.draw_main.as_mut()?.draw_super;
        let dv = &mut draw_main.draw_vars;
        set_common(cx.cx, dv);
        dv.set_texture(0, &targets.packed);
        dv.set_texture(1, &targets.normals);
        dv.set_texture(2, &input.table.texture);
        dv.set_uniform_buffer(0, self.tap_uniforms.as_ref()?);
        dv.set_uniform(
            cx.cx,
            live_id!(u_counts),
            &[samples as f32, input.tier.rays() as f32, 0.0, 0.0],
        );
        dv.set_uniform(cx.cx, live_id!(u_unproject), &main.unproject);
        dv.set_uniform(cx.cx, live_id!(u_projection), &main.projection);
        dv.set_uniform(cx.cx, live_id!(u_geometry), &geometry);
        dv.set_uniform(cx.cx, live_id!(u_limits), &limits);
        dv.set_uniform(cx.cx, live_id!(u_backface_boost), &[main.backface_boost]);
        dv.set_uniform(cx.cx, live_id!(u_min_occluder), &[input.config.min_occluder]);
        run_stage(
            cx,
            &mut self.stages,
            1,
            after_main,
            dpi,
            size,
            &[&targets.raw],
            &mut |cx, rect| draw_main.draw_abs(cx, rect),
        );

        let draw_filter = self.draw_filter.as_mut()?;
        let dv = &mut draw_filter.draw_super.draw_vars;
        dv.set_texture(0, &targets.packed);
        dv.set_texture(1, &targets.raw);
        dv.set_uniform(cx.cx, live_id!(u_depth_code), &input.camera.depth_code);
        dv.set_uniform(
            cx.cx,
            live_id!(u_size),
            &[filter.resolution[0], filter.resolution[1], dims[2], dims[3]],
        );
        dv.set_uniform(
            cx.cx,
            live_id!(u_filter),
            &[
                filter.depth_sensitivity,
                filter.normal_sensitivity / 32.0,
                if input.tier.large_filter() { 1.0 } else { 0.0 },
                if input.tier.symmetric_filter() {
                    1.0
                } else {
                    0.0
                },
            ],
        );
        run_stage(
            cx,
            &mut self.stages,
            2,
            after_filter,
            dpi,
            size,
            &[&targets.output],
            &mut |cx, rect| draw_filter.draw_abs(cx, rect),
        );

        let mut output = targets.output.clone();
        if input.history {
            let history = targets.history.as_ref()?;
            let (write, weight, previous_from_view, previous) = match self.history_last {
                Some((index, previous)) if !input.reset => match history_from_view(&previous, &input.camera) {
                    Some(m) => (1 - index, HISTORY_WEIGHT, m, previous),
                    None => (1 - index, 0.0, IDENTITY, input.camera),
                },
                Some((index, _)) => (1 - index, 0.0, IDENTITY, input.camera),
                None => (0, 0.0, IDENTITY, input.camera),
            };
            let draw_resolve = self.draw_resolve.as_mut()?;
            let dv = &mut draw_resolve.draw_super.draw_vars;
            dv.set_texture(0, &targets.packed);
            dv.set_texture(1, &targets.output);
            dv.set_texture(2, &history[1 - write]);
            dv.set_texture(3, &targets.normals);
            dv.set_uniform(cx.cx, live_id!(u_depth_code), &input.camera.depth_code);
            dv.set_uniform(cx.cx, live_id!(u_size), &dims);
            dv.set_uniform(cx.cx, live_id!(u_unproject), &main.unproject);
            self.resolve_current = [main.unproject_skew, mode[0], weight, 0.0];
            dv.set_uniform(cx.cx, live_id!(u_current), &self.resolve_current);
            dv.set_uniform(
                cx.cx,
                live_id!(u_previous),
                &[previous.px, previous.py, previous.pz, previous.pw],
            );
            dv.set_uniform(
                cx.cx,
                live_id!(u_previous_mode),
                &[previous.skew, if previous.ortho { 1.0 } else { 0.0 }, 0.0, 0.0],
            );
            dv.set_uniform(cx.cx, live_id!(u_previous_from_view), &previous_from_view);
            run_stage(
                cx,
                &mut self.stages,
                3,
                after_resolve,
                dpi,
                size,
                &[&history[write]],
                &mut |cx, rect| draw_resolve.draw_abs(cx, rect),
            );
            self.history_last = Some((write, input.camera));
            output = history[write].clone();
        } else {
            self.history_last = None;
        }

        if half {
            let draw_upsample = self.draw_upsample.as_mut()?;
            let dv = &mut draw_upsample.draw_super.draw_vars;
            copy_surface_bindings(cx.cx, &self.draw_prepass.as_ref()?.draw_vars, dv);
            set_texture_named(cx.cx, dv, live_id!(depth_tex), input.depth);
            set_texture_named(cx.cx, dv, live_id!(upsample_atlas_tex), &targets.packed);
            set_texture_named(cx.cx, dv, live_id!(upsample_ao_tex), &output);
            dv.set_uniform(
                cx.cx,
                live_id!(u_size),
                &[full_width as f32, full_height as f32, dims[2], dims[3]],
            );
            dv.set_uniform(cx.cx, live_id!(u_upsample_size), &dims);
            dv.set_uniform(cx.cx, live_id!(u_depth_code), &input.camera.depth_code);
            let upsampled = targets.upsampled.as_ref()?;
            run_stage(
                cx,
                &mut self.stages,
                4,
                parent,
                dpi,
                dvec2(full_width as f64 / dpi, full_height as f64 / dpi),
                &[upsampled],
                &mut |cx, rect| draw_upsample.draw_abs(cx, rect),
            );
            output = upsampled.clone();
        }

        for (i, stage) in self.stages.iter().enumerate() {
            for ms in stage.pass.gpu_times_ms(cx.cx) {
                if i == 3 && !input.history {
                    continue;
                }
                self.stage_gpu_ms[i] = ms;
                let min = &mut self.stage_gpu_min_ms[i];
                if *min <= 0.0 || ms < *min {
                    *min = ms;
                }
                self.gpu_samples = self.gpu_samples.saturating_add(1);
            }
            if i == 3 && !input.history {
                self.stage_gpu_ms[i] = 0.0;
                self.stage_gpu_min_ms[i] = 0.0;
            }
        }
        Some(CortaoOutput::Visibility(output))
    }
}

fn frame_size(size: DVec2, dpi: f64) -> Option<[u32; 2]> {
    if !dpi.is_finite() || dpi <= 0.0 {
        return None;
    }
    // The passes' own rounding: every target is exactly the size of the host's
    // depth and lit passes, at fractional scales too (100.25 device px is 101).
    let (w, h) = Cx::pass_pixel_size(size, dpi);
    if w < 2 || h < 2 {
        return None;
    }
    let [w, h] = [u32::try_from(w).ok()?, u32::try_from(h).ok()?];
    h.checked_add((h >> 1).max(1))?;
    Some([w, h])
}

fn valid_parameters(camera: &CortaoCamera, config: &CortaoConfig) -> bool {
    camera
        .view_from_clip
        .iter()
        .chain(&camera.view_from_world)
        .all(|v| v.is_finite())
        && [camera.px, camera.py, camera.pz, camera.pw, camera.skew]
            .iter()
            .all(|v| v.is_finite())
        && camera.px > 0.0
        && camera.py < 0.0
        && [
            config.radius,
            config.min_occluder,
            config.max_screen_radius,
            config.mip_bias,
            config.max_slope_compensation,
            config.backface_threshold,
            config.backface_boost,
            config.filter_depth_sensitivity,
            config.filter_normal_sensitivity,
        ]
        .iter()
        .all(|v| v.is_finite())
        && config.radius > 0.0
        && config.min_occluder >= 0.0
        && config.max_screen_radius > 0.0
        && (0.0..=1.0).contains(&config.backface_threshold)
        && (0.0..=1.0).contains(&config.backface_boost)
        && config.filter_depth_sensitivity >= 0.0
        && config.filter_normal_sensitivity >= 0.0
}

/// The host updates only its prepass. Mirror those values by name because the
/// upsample adds inputs, so neither uniform offsets nor texture slots must match.
fn copy_surface_bindings(cx: &Cx, source: &DrawVars, dest: &mut DrawVars) {
    let (Some(source_id), Some(dest_id)) = (source.draw_shader_id, dest.draw_shader_id) else {
        return;
    };
    let source_mapping = &cx.draw_shaders[source_id.index].mapping;
    let dest_mapping = &cx.draw_shaders[dest_id.index].mapping;
    for input in &source_mapping.dyn_uniforms.inputs {
        if let Some(values) = source
            .dyn_uniforms
            .get(input.offset..input.offset + input.slots)
        {
            dest.set_uniform(cx, input.id, values);
        }
    }
    let source_base = source.dyn_instances.len() - source_mapping.dyn_instances.total_slots;
    let dest_base = dest.dyn_instances.len() - dest_mapping.dyn_instances.total_slots;
    for input in &source_mapping.dyn_instances.inputs {
        if let Some(output) = dest_mapping.dyn_instances.find(input.id) {
            let slots = input.slots.min(output.slots);
            dest.dyn_instances[dest_base + output.offset..dest_base + output.offset + slots]
                .copy_from_slice(
                    &source.dyn_instances[source_base + input.offset..source_base + input.offset + slots],
                );
        }
    }
    for (slot, input) in dest_mapping.textures.iter().enumerate() {
        if let Some(from) = source_mapping.textures.iter().position(|t| t.id == input.id) {
            dest.texture_slots[slot] = source.texture_slots[from].clone();
        }
    }
    for (slot, input) in dest_mapping.uniform_buffers.iter().enumerate() {
        if let Some(from) = source_mapping
            .uniform_buffers
            .iter()
            .position(|b| b.id == input.id)
        {
            dest.uniform_buffer_slots[slot] = source.uniform_buffer_slots[from].clone();
        }
    }
}

fn set_texture_named(cx: &Cx, vars: &mut DrawVars, name: LiveId, texture: &Texture) {
    if let Some(id) = vars.draw_shader_id {
        if let Some(slot) = cx.draw_shaders[id.index]
            .mapping
            .textures
            .iter()
            .position(|t| t.id == name)
        {
            vars.set_texture(slot, texture);
        }
    }
}

/// Record stage `i` under its next active consumer (resolve may be skipped), at
/// the frame's `dpi`: stages are parented one by one as they record, so the
/// consumer may not reach the window yet (first frame, a mode change) and a
/// DPI inherited through it would be 1.
#[allow(clippy::too_many_arguments)]
fn run_stage(
    cx: &mut Cx2d,
    stages: &mut [CortaoStage],
    i: usize,
    parent: DrawPassId,
    dpi: f64,
    size: DVec2,
    targets: &[&Texture],
    draw: &mut dyn FnMut(&mut Cx2d, Rect),
) {
    let stage = &mut stages[i];
    stage.pass.set_size(cx, size);
    stage.pass.clear_color_textures(cx.cx);
    let clear = if i == 0 {
        vec4(0.0, 0.0, 0.0, 0.0)
    } else {
        vec4(1.0, 1.0, 1.0, 1.0)
    };
    for target in targets {
        stage
            .pass
            .add_color_texture(cx.cx, target, DrawPassClearColor::ClearWith(clear));
    }
    stage.pass.clear_depth_texture(cx.cx);
    stage.pass.set_pass_parent(cx.cx, parent);
    cx.begin_pass(&stage.pass, Some(dpi));
    stage.pass.set_size(cx, size);
    stage.list.begin_always(cx);
    cx.begin_root_turtle(cx.current_pass_size(), Layout::flow_overlay());
    draw(
        cx,
        Rect {
            pos: dvec2(0.0, 0.0),
            size,
        },
    );
    cx.end_pass_sized_turtle();
    stage.list.end(cx);
    cx.end_pass(&stage.pass);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spiral_taps_cover_the_disc_and_keep_the_first_mip_finite() {
        for tier in [
            CortaoTier::Low,
            CortaoTier::Medium,
            CortaoTier::High,
            CortaoTier::Ultra,
        ] {
            assert!(tier.samples() <= 24, "{tier:?}: fits the uniform tap array");
            let taps = tap_table(tier.samples());
            assert_eq!(
                taps.len(),
                tier.samples() as usize * 2,
                "{tier:?}: two vec4 per tap"
            );
            let mut previous = 0.0_f32;
            let mut area = 0.0;
            for pair in taps.chunks_exact(2) {
                assert!(
                    pair.iter().flatten().all(|v| v.is_finite()),
                    "{tier:?}: finite tap and mip"
                );
                let [ix, iy, ox, oy] = pair[0];
                let inner = ix.hypot(iy);
                let outer = ox.hypot(oy);
                assert!(
                    (inner - previous).abs() < 1e-6,
                    "{tier:?}: contiguous annuli"
                );
                assert!(outer > inner, "{tier:?}: positive annulus width");
                assert!(
                    (pair[1][1].exp2() - outer).abs() < 1e-6,
                    "{tier:?}: mip matches outer radius"
                );
                area += pair[1][0] * pair[1][0];
                previous = outer;
            }
            assert!(
                (previous - 1.0).abs() < 1e-6,
                "{tier:?}: reaches the unit disc rim"
            );
            assert!(
                (area - 1.0).abs() < 1e-6,
                "{tier:?}: weights partition disc area"
            );
            assert_eq!(
                taps[0][..2],
                [0.0, 0.0],
                "{tier:?}: first inner radius is zero"
            );
        }
        assert!(tap_table(0).is_empty());
    }

    #[test]
    fn frame_seeds_mix_both_words_and_survive_float_halves() {
        let mut unique = std::collections::HashSet::new();
        let mut changed = 0;
        for frame in 0..4096 {
            let seed = frame_seed(frame);
            assert!(unique.insert(seed), "adjacent frame seeds must differ");
            assert_ne!(
                seed,
                frame_seed(frame + (1u64 << 32)),
                "high frame word contributes"
            );
            let halves = [(seed & 65535) as f32, (seed >> 16) as f32];
            assert_eq!(
                seed,
                halves[0] as u32 | ((halves[1] as u32) << 16),
                "seed upload is exact"
            );
            changed += (seed ^ frame_seed(frame + 1)).count_ones();
        }
        assert!(
            (14.0..18.0).contains(&(changed as f64 / 4096.0)),
            "consecutive seeds decorrelate their bits"
        );
    }

    #[test]
    fn frame_dimensions_round_device_pixels_and_reject_invalid_extents() {
        assert_eq!(frame_size(dvec2(10.3, 20.8), 2.0), Some([21, 42]));
        assert_eq!(frame_size(dvec2(80.2, 40.0), 1.25), Some([101, 50]), "a pass's own size");
        for (size, dpi) in [
            (dvec2(1.0, 8.0), 1.0),
            (dvec2(f64::NAN, 8.0), 1.0),
            (dvec2(8.0, 8.0), 0.0),
            (dvec2(8.0, 8.0), f64::INFINITY),
            (dvec2(8.0, u32::MAX as f64), 1.0),
        ] {
            assert_eq!(
                frame_size(size, dpi),
                None,
                "invalid size or atlas overflow"
            );
        }
    }
}
