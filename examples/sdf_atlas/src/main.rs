//! makepad-example-sdf-atlas: render-target atlases for engines built from draw
//! passes (no compute): the data formats, partial-rect writes that persist
//! across frames, ping-pong between two targets, a mixed-format MRT pass, a
//! filtered read of a half-float copy, and `precise_math` kernels that match
//! the CPU bit for bit.
//!
//! ```text
//! makepad-example-sdf-atlas [--selftest]
//! ```
//! `--bench` instead times the kernel over a 4096 x 4096 R32F target (16.8 M
//! samples: 32 k bricks of 8^3), fast and precise, with the HAL's GPU timer,
//! and prints `ATLAS-BENCH fast=<ms> precise=<ms>` (medians).
//!
//! `--selftest` runs the cases below, reads every target back in its native
//! texel format, compares each texel with the CPU model, prints
//! `ATLAS-OK ...` or `ATLAS-FAIL ...` and exits 0 / 1.
//!
//! Phases (each recorded once, then waited on until its passes painted whole):
//! - 0: stamp 1 into every third 8x8 slot of the R32F, R16F and R8 atlases;
//!   the MRT pass (R32F distance + R8 slot); the kernels (fast and precise);
//!   the ping-pong source filled.
//! - 1: stamp 2 into every fifth slot (overwriting some of stamp 1: the
//!   targets keep stamp 1 elsewhere); one ping-pong step: B = 2 A + 1 over
//!   some slots, then A = B over the same slots.
//! - 2: the filtered read of the R16F atlas (hardware bilinear midpoints).
//! - then every target is read back and compared.
//!
//! The precise kernel is the shape of a sculpt bake: a lattice point
//! q = g * cell + origin from integer texel coordinates, a sphere and a box
//! (sqrt), a smooth union (a division by the blend radius) and a stroke
//! falloff (a division and products that a fused multiply-add would round
//! differently). With `precise_math: true` the GPU must agree with the CPU
//! on every texel; the fast-math twin's differing texels are reported (not
//! failed) as a measure of what the flag buys on this backend.

pub use makepad_widgets;

use makepad_widgets::*;
use makepad_widgets::makepad_platform::makepad_hal::gpu_prof;
use std::sync::{Arc, Mutex};

app_main!(App);

/// The slot atlases: 32 x 8 slots of 8 x 8 texels.
const AW: usize = 256;
const AH: usize = 64;
const SLOT: usize = 8;
const SLOTS_X: usize = AW / SLOT;
const SLOTS: usize = SLOTS_X * (AH / SLOT);
/// The kernel targets: a 64 x 64 x 16 lattice laid out as 4 x 4 tiles of 64 x 64.
const KW: usize = 256;
const KH: usize = 256;
const CELL: f32 = 1.0 / 32.0;
const ORIGIN: f32 = -1.0;
/// The ping-pong pair and the filter target.
const PW: usize = 64;
const PH: usize = 64;
const FW: usize = 64;
const FH: usize = 8;

const INIT_F32: f32 = -1.0;
const INIT_F16: f32 = -2.0;

fn in_s1(slot: usize) -> bool {
    slot % 3 == 0
}
fn in_s2(slot: usize) -> bool {
    slot % 5 == 0
}
/// The ping-pong step's slots (8 x 8 of a 64 x 64 target).
fn in_pp(slot: usize) -> bool {
    slot % 4 == 1
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.pod.*
    use mod.math.*
    use mod.shader.*

    // A raw data write: one quad per slot, each texel a function of its own
    // integer coordinates and the quad's stamp. `kind` 0: R32F, 1: R16F, 2: R8.
    mod.draw.AtlasWrite = mod.std.set_type_default() do #(DrawAtlasWrite::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: false
        color_format: @Rf32
        fragment: fn() {
            let g = floor(self.world.xy)
            if self.kind < 0.5 {
                self.fb0 = vec4(g.x * 1024.0 + g.y + self.stamp * 0.25, 0.0, 0.0, 1.0)
            } else if self.kind < 1.5 {
                self.fb0 = vec4(g.x * 0.5 + g.y + self.stamp, 0.0, 0.0, 1.0)
            } else {
                let n = g.x * 7.0 + g.y * 3.0 + self.stamp * 11.0
                let m = n - floor(n * 0.00390625) * 256.0
                self.fb0 = vec4(m / 255.0, 0.0, 0.0, 1.0)
            }
        }
    }

    // Distance and slot in one pass: attachment 0 R32F, attachment 1 R8.
    mod.draw.AtlasMrt = mod.std.set_type_default() do #(DrawAtlasMrt::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: false
        color_format: @Rf32
        fb1: fragment_output(1, vec4f)
        fragment: fn() {
            let g = floor(self.world.xy)
            self.fb0 = vec4(g.x + g.y * 512.0 + 0.5, 0.0, 0.0, 1.0)
            let n = g.x + g.y * 5.0
            let m = n - floor(n * 0.00390625) * 256.0
            self.fb1 = vec4(m / 255.0, 0.0, 0.0, 1.0)
        }
    }

    // out = src(texel) * scale + bias: the ping-pong step and its copy back.
    mod.draw.AtlasCopy = mod.std.set_type_default() do #(DrawAtlasCopy::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: false
        color_format: @Rf32
        src: texture_2d(float)
        src_inv: uniform(vec2(1.0, 1.0))
        fragment: fn() {
            let g = floor(self.world.xy)
            let v = self.src.sample_nearest((g + vec2(0.5, 0.5)) * self.src_inv).x
            self.fb0 = vec4(v * self.scale + self.bias, 0.0, 0.0, 1.0)
        }
    }

    // A filtered read: the midpoint between texels (2 g.x, g.y) and (2 g.x + 1, g.y).
    mod.draw.AtlasFilter = mod.std.set_type_default() do #(DrawAtlasFilter::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: false
        color_format: @Rf32
        src: texture_2d(float)
        src_inv: uniform(vec2(1.0, 1.0))
        fragment: fn() {
            let g = floor(self.world.xy)
            let uv = vec2(g.x * 2.0 + 1.0, g.y + 0.5) * self.src_inv
            self.fb0 = vec4(self.src.sample(uv).x, 0.0, 0.0, 1.0)
        }
    }

    // The bake-shaped kernel (the CPU twin is `kernel` below; the same ops in
    // the same order).
    mod.draw.AtlasKernel = mod.std.set_type_default() do #(DrawAtlasKernel::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: false
        color_format: @Rf32
        cell: uniform(1.0)
        origin: uniform(0.0)
        probe: uniform(0.0)
        fragment: fn() {
            let pix = floor(self.world.xy)
            let tx = floor(pix.x * 0.015625)
            let ty = floor(pix.y * 0.015625)
            let gx = pix.x - tx * 64.0
            let gy = pix.y - ty * 64.0
            let gz = tx + ty * 4.0
            let qx = gx * self.cell + self.origin
            let qy = gy * self.cell + self.origin
            let qz = gz * self.cell + self.origin
            let a = sqrt(qx * qx + qy * qy + qz * qz) - 0.61
            let bx = abs(qx - 0.13) - 0.37
            let by = abs(qy + 0.05) - 0.29
            let bz = abs(qz - 0.21) - 0.33
            let ox = max(bx, 0.0)
            let oy = max(by, 0.0)
            let oz = max(bz, 0.0)
            let b = sqrt(ox * ox + oy * oy + oz * oz) + min(max(bx, max(by, bz)), 0.0)
            let k = 0.17
            let h = max(k - abs(a - b), 0.0) / k
            let u = min(a, b) - h * h * k * 0.25
            let dx = qx - 0.3
            let dy = qy + 0.2
            let dz = qz - 0.45
            let t = (dx * dx + dy * dy + dz * dz) / 0.2025
            var w = 0.0
            if t < 1.0 {
                let s = 1.0 - t
                w = s * s
            }
            var d = u - w * 0.05 + qx * qy * 0.3 + qz * 0.01
            if self.probe > 0.5 && self.probe < 1.5 { d = sqrt(qx * qx + qy * qy + qz * qz) }
            if self.probe > 1.5 && self.probe < 2.5 { d = (dx * dx + dy * dy + dz * dz) / 0.2025 }
            if self.probe > 2.5 && self.probe < 3.5 { d = qx * qy * 0.3 + qz * 0.01 }
            if self.probe > 3.5 && self.probe < 4.5 { d = qx * qx + qy * qy + qz * qz }
            if self.probe > 4.5 && self.probe < 5.5 { d = (qx * qy) / (qz + 1.37) }
            if self.probe > 5.5 {
                // operands over 31 binades from integer hashes of the texel (the CPU makes the same bits)
                let h0 = (u32(pix.x) * 7919u + u32(pix.y) * 104729u) * 2654435761u
                let h1 = (h0 ^ (h0 >> 15u)) * 2246822519u
                let x0 = asfloat(((((h0 >> 23u) & 31u) + 107u) << 23u) | (h0 & 8388607u))
                let x1 = asfloat(((((h1 >> 23u) & 31u) + 107u) << 23u) | (h1 & 8388607u))
                if self.probe < 6.5 { d = x0 / x1 } else { d = sqrt(x0) }
            }
            self.fb0 = vec4(d, 0.0, 0.0, 1.0)
        }
    }

    // The targets shown in the window (a target nothing samples is never painted).
    mod.draw.AtlasShow = mod.std.set_type_default() do #(DrawAtlasShow::script_shader(vm)){
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        gain: uniform(1.0)
        pixel: fn() {
            let v = self.image.sample_nearest(self.pos).x * self.gain
            return vec4(fract(v), fract(v * 0.37), fract(v * 0.11), 1.0)
        }
    }

    mod.widgets.AtlasView = set_type_default() do #(AtlasView::register_widget(vm)){
        width: Fill
        height: Fill
        write_f16 +: { color_format: @Rf16 }
        write_u8 +: { color_format: @Ru8 }
        kernel_precise +: { precise_math: true }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(640, 360)
                window.title: "sdf atlas"
                body +: {
                    view := mod.widgets.AtlasView{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAtlasWrite {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub stamp: f32,
    #[live]
    pub kind: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAtlasMrt {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAtlasCopy {
    #[deref]
    pub draw_super: DrawQuad,
    #[live(1.0)]
    pub scale: f32,
    #[live]
    pub bias: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAtlasFilter {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAtlasShow {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAtlasKernel {
    #[deref]
    pub draw_super: DrawQuad,
}

/// One render target and the pass that writes it.
struct Target {
    name: &'static str,
    w: usize,
    h: usize,
    tex: Texture,
    /// a second attachment (the MRT case)
    extra: Option<Texture>,
    pass: DrawPass,
    list: DrawList,
}

impl Target {
    fn new(cx: &mut Cx, name: &'static str, w: usize, h: usize, format: TextureFormat) -> Self {
        Target {
            name,
            w,
            h,
            tex: Texture::new_with_format(cx, format),
            extra: None,
            pass: DrawPass::new_with_name(cx, name),
            list: DrawList::new(cx),
        }
    }
}

fn fixed(w: usize, h: usize) -> TextureSize {
    TextureSize::Fixed { width: w, height: h }
}

fn slot_rect(slot: usize, slots_x: usize) -> Rect {
    Rect {
        pos: dvec2(((slot % slots_x) * SLOT) as f64, ((slot / slots_x) * SLOT) as f64),
        size: dvec2(SLOT as f64, SLOT as f64),
    }
}

type Readbacks = Arc<Mutex<Vec<(&'static str, Result<(usize, usize, Arc<[u8]>), String>)>>>;

#[derive(Script, ScriptHook, Widget)]
pub struct AtlasView {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[rust]
    area: Area,
    #[live]
    write_f32: DrawAtlasWrite,
    #[live]
    write_f16: DrawAtlasWrite,
    #[live]
    write_u8: DrawAtlasWrite,
    #[live]
    mrt: DrawAtlasMrt,
    #[live]
    copy_step: DrawAtlasCopy,
    #[live]
    copy_back: DrawAtlasCopy,
    #[live]
    filter: DrawAtlasFilter,
    #[live]
    kernel_fast: DrawAtlasKernel,
    #[live]
    kernel_precise: DrawAtlasKernel,
    #[live]
    show: DrawAtlasShow,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    targets: Vec<Target>,
    #[rust]
    frame: u64,
    #[rust]
    requested: bool,
    #[rust]
    phase: usize,
    #[rust]
    wait: Option<u64>,
    #[rust]
    recorded: Vec<usize>,
    #[rust]
    bench: Vec<(Target, Vec<f64>)>,
    #[rust]
    prof_from: u64,
    #[rust]
    readbacks: Readbacks,
}

/// Target indices.
const T_F32: usize = 0;
const T_F16: usize = 1;
const T_U8: usize = 2;
const T_MRT: usize = 3;
const T_KFAST: usize = 4;
const T_KPRECISE: usize = 5;
const T_PP_A: usize = 6;
const T_PP_B: usize = 7;
const T_FILTER: usize = 8;

impl AtlasView {
    fn make_targets(&mut self, cx: &mut Cx) {
        let t = &mut self.targets;
        t.push(Target::new(cx, "atlas_r32f", AW, AH, TextureFormat::RenderRf32 { size: fixed(AW, AH), initial: true }));
        t.push(Target::new(cx, "atlas_r16f", AW, AH, TextureFormat::RenderRf16 { size: fixed(AW, AH), initial: true }));
        t.push(Target::new(cx, "atlas_r8", AW, AH, TextureFormat::RenderRu8 { size: fixed(AW, AH), initial: true }));
        let mut mrt = Target::new(cx, "mrt_r32f", AW, AH, TextureFormat::RenderRf32 { size: fixed(AW, AH), initial: true });
        mrt.extra = Some(Texture::new_with_format(cx, TextureFormat::RenderRu8 { size: fixed(AW, AH), initial: true }));
        t.push(mrt);
        t.push(Target::new(cx, "kernel_fast", KW, KH, TextureFormat::RenderRf32 { size: fixed(KW, KH), initial: true }));
        t.push(Target::new(cx, "kernel_precise", KW, KH, TextureFormat::RenderRf32 { size: fixed(KW, KH), initial: true }));
        t.push(Target::new(cx, "pingpong_a", PW, PH, TextureFormat::RenderRf32 { size: fixed(PW, PH), initial: true }));
        t.push(Target::new(cx, "pingpong_b", PW, PH, TextureFormat::RenderRf32 { size: fixed(PW, PH), initial: true }));
        t.push(Target::new(cx, "filter", FW, FH, TextureFormat::RenderRf32 { size: fixed(FW, FH), initial: true }));
    }

    /// Runs target `i`'s pass with `draw` (a child of `parent`, or of the window).
    fn run(&mut self, cx: &mut Cx2d, i: usize, parent: Option<usize>, draw: impl FnOnce(&mut Self, &mut Cx2d)) {
        let parent_id = parent.map(|p| self.targets[p].pass.draw_pass_id());
        {
            let t = &mut self.targets[i];
            let size = dvec2(t.w as f64, t.h as f64);
            let init = match i {
                T_F16 => vec4(INIT_F16, 0.0, 0.0, 1.0),
                T_U8 => vec4(0.0, 0.0, 0.0, 0.0),
                _ => vec4(INIT_F32, 0.0, 0.0, 1.0),
            };
            t.pass.set_size(cx, size);
            t.pass.clear_color_textures(cx.cx);
            t.pass.set_color_texture(cx, &t.tex, DrawPassClearColor::InitWith(init));
            if let Some(extra) = &t.extra {
                t.pass.add_color_texture(cx, extra, DrawPassClearColor::InitWith(vec4(0.0, 0.0, 0.0, 0.0)));
            }
            t.pass.clear_depth_texture(cx.cx);
            match parent_id {
                Some(p) => t.pass.set_pass_parent(cx.cx, p),
                None => cx.make_child_pass(&t.pass),
            }
            cx.begin_pass(&t.pass, Some(1.0));
            t.pass.set_size(cx, size);
            t.pass.set_dpi_factor(cx, 1.0);
        }
        self.targets[i].list.begin_always(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_overlay());
        draw(self, cx);
        cx.end_pass_sized_turtle();
        let t = &mut self.targets[i];
        t.list.end(cx);
        cx.end_pass(&t.pass);
    }

    fn stamp_atlases(&mut self, cx: &mut Cx2d, stamp: f32, slots: fn(usize) -> bool) {
        for (i, kind) in [(T_F32, 0.0), (T_F16, 1.0), (T_U8, 2.0)] {
            self.run(cx, i, None, |s, cx| {
                let draw = match i {
                    T_F32 => &mut s.write_f32,
                    T_F16 => &mut s.write_f16,
                    _ => &mut s.write_u8,
                };
                draw.stamp = stamp;
                draw.kind = kind;
                for slot in (0..SLOTS).filter(|s| slots(*s)) {
                    draw.draw_super.draw_abs(cx, slot_rect(slot, SLOTS_X));
                }
            });
        }
    }

    /// Records the next phase's draws once; then, until every pass it recorded painted whole
    /// (its pipelines compile asynchronously: a draw left out is painted when they are ready, from
    /// the same recording), keeps the passes attached without recording them again.
    fn draw_frame(&mut self, cx: &mut Cx2d) {
        if let Some(serial) = self.wait {
            for t in &self.targets {
                cx.make_child_pass(&t.pass);
            }
            let painted = self.recorded.iter().all(|i| self.targets[*i].pass.painted_whole_since(cx.cx, serial));
            if self.frame % 300 == 0 {
                let st: Vec<String> = self.recorded.iter().map(|i| {
                    let p = &cx.cx.passes[self.targets[*i].pass.draw_pass_id()];
                    format!("{}:dirty={} painted={}", self.targets[*i].name, p.paint_dirty, p.painted_serial)
                }).collect();
                println!("ATLAS waiting phase={} serial={serial} pending={} {}", self.phase, cx.cx.draw_shaders_pending(), st.join(" "));
            }
            if painted {
                println!("ATLAS phase={} painted (frame {})", self.phase, self.frame);
                self.phase += 1;
                self.wait = None;
            }
            return;
        }
        if self.phase > 2 {
            for t in &self.targets {
                cx.make_child_pass(&t.pass);
            }
            return;
        }
        self.recorded = match self.phase {
            0 => vec![T_F32, T_F16, T_U8, T_MRT, T_KFAST, T_KPRECISE, T_PP_A],
            1 => vec![T_F32, T_F16, T_U8, T_PP_B],
            _ => vec![T_PP_A, T_FILTER],
        };
        self.record_phase(cx);
        // Every other pass is recorded empty: a recording runs again whenever the pass repaints
        // (a pipeline answering repaints every pass), so a pass keeps a recording only in the
        // phase it belongs to. That also keeps the ping-pong pair acyclic: the step (B from A)
        // and the copy back (A from B) are recorded in different phases, never both at once.
        for i in 0..self.targets.len() {
            if !self.recorded.contains(&i) {
                self.run(cx, i, None, |_, _| {});
            }
        }
        self.wait = Some(cx.cx.repaint_id());
    }

    fn record_phase(&mut self, cx: &mut Cx2d) {
        match self.phase {
            0 => {
                self.stamp_atlases(cx, 1.0, in_s1);
                self.run(cx, T_MRT, None, |s, cx| {
                    s.mrt.draw_super.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(AW as f64, AH as f64) });
                });
                for (i, precise) in [(T_KFAST, false), (T_KPRECISE, true)] {
                    self.run(cx, i, None, |s, cx| {
                        let k = if precise { &mut s.kernel_precise } else { &mut s.kernel_fast };
                        let c = &*cx.cx;
                        k.draw_super.draw_vars.set_uniform(c, id!(cell), &[CELL]);
                        k.draw_super.draw_vars.set_uniform(c, id!(origin), &[ORIGIN]);
                        k.draw_super.draw_vars.set_uniform(c, id!(probe), &[probe() as f32]);
                        k.draw_super.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(KW as f64, KH as f64) });
                    });
                }
                // the ping-pong source: A = 1024 g.x + g.y everywhere
                self.run(cx, T_PP_A, None, |s, cx| {
                    s.write_f32.stamp = 0.0;
                    s.write_f32.kind = 0.0;
                    s.write_f32.draw_super.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(PW as f64, PH as f64) });
                });
            }
            1 => {
                self.stamp_atlases(cx, 2.0, in_s2);
                // B = 2 A + 1 over the step's slots (the copy back is the next phase)
                let a = self.targets[T_PP_A].tex.clone();
                let inv = [1.0 / PW as f32, 1.0 / PH as f32];
                self.run(cx, T_PP_B, None, |s, cx| {
                    let c = &*cx.cx;
                    s.copy_step.draw_super.draw_vars.set_texture(0, &a);
                    s.copy_step.draw_super.draw_vars.set_uniform(c, id!(src_inv), &inv);
                    s.copy_step.scale = 2.0;
                    s.copy_step.bias = 1.0;
                    for slot in (0..(PW / SLOT) * (PH / SLOT)).filter(|s| in_pp(*s)) {
                        s.copy_step.draw_super.draw_abs(cx, slot_rect(slot, PW / SLOT));
                    }
                });
            }
            _ => {
                let b = self.targets[T_PP_B].tex.clone();
                let inv = [1.0 / PW as f32, 1.0 / PH as f32];
                self.run(cx, T_PP_A, None, |s, cx| {
                    let c = &*cx.cx;
                    s.copy_back.draw_super.draw_vars.set_texture(0, &b);
                    s.copy_back.draw_super.draw_vars.set_uniform(c, id!(src_inv), &inv);
                    s.copy_back.scale = 1.0;
                    s.copy_back.bias = 0.0;
                    for slot in (0..(PW / SLOT) * (PH / SLOT)).filter(|s| in_pp(*s)) {
                        s.copy_back.draw_super.draw_abs(cx, slot_rect(slot, PW / SLOT));
                    }
                });
                let src = self.targets[T_F16].tex.clone();
                self.run(cx, T_FILTER, None, |s, cx| {
                    let c = &*cx.cx;
                    s.filter.draw_super.draw_vars.set_texture(0, &src);
                    s.filter.draw_super.draw_vars.set_uniform(c, id!(src_inv), &[1.0 / AW as f32, 1.0 / AH as f32]);
                    s.filter.draw_super.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(FW as f64, FH as f64) });
                });
            }
        }
    }

    /// Both kernels over the bench target, every frame, timed per pass.
    fn bench_frame(&mut self, cx: &mut Cx2d) {
        const N: usize = 4096;
        if self.bench.is_empty() {
            for name in ["bench_fast", "bench_precise"] {
                let t = Target::new(cx.cx.cx, name, N, N, TextureFormat::RenderRf32 { size: fixed(N, N), initial: true });
                self.bench.push((t, Vec::new()));
            }
            gpu_prof::start(gpu_prof::StartOptions { keep_s: 20, split_every: 0, ..Default::default() });
        }
        // GPU ms per bench pass of the frames the profiler recorded since the last look
        if let Some(rec) = gpu_prof::recording_of(None) {
            for f in rec.frames.iter().filter(|f| f.id > self.prof_from && !f.split) {
                for p in &f.passes {
                    let name = rec.s(p.name);
                    if p.end > p.start {
                        if let Some((_, times)) = self.bench.iter_mut().find(|(t, _)| t.name == name) {
                            times.push((p.end - p.start) as f64 / 1.0e6);
                        }
                    }
                }
            }
            self.prof_from = rec.frames.last().map_or(self.prof_from, |f| f.id);
        }
        for b in 0..2 {
            let (t, _) = &mut self.bench[b];
            let size = dvec2(N as f64, N as f64);
            t.pass.set_size(cx, size);
            t.pass.clear_color_textures(cx.cx);
            t.pass.set_color_texture(cx, &t.tex, DrawPassClearColor::InitWith(vec4(0.0, 0.0, 0.0, 1.0)));
            t.pass.clear_depth_texture(cx.cx);
            cx.make_child_pass(&t.pass);
            cx.begin_pass(&t.pass, Some(1.0));
            t.pass.set_size(cx, size);
            t.pass.set_dpi_factor(cx, 1.0);
            t.list.begin_always(cx);
            cx.begin_root_turtle(size, Layout::flow_overlay());
            let k = if b == 1 { &mut self.kernel_precise } else { &mut self.kernel_fast };
            let c = &*cx.cx;
            k.draw_super.draw_vars.set_uniform(c, id!(cell), &[CELL]);
            k.draw_super.draw_vars.set_uniform(c, id!(origin), &[ORIGIN]);
            k.draw_super.draw_vars.set_uniform(c, id!(probe), &[probe() as f32]);
            k.draw_super.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size });
            cx.end_pass_sized_turtle();
            t.list.end(cx);
            cx.end_pass(&t.pass);
        }
        // shown, so the passes paint
        let rect = cx.turtle().rect();
        for b in 0..2 {
            let r = Rect { pos: dvec2(rect.pos.x + b as f64 * rect.size.x * 0.5, rect.pos.y), size: dvec2(rect.size.x * 0.5 - 2.0, rect.size.y) };
            self.show.draw_super.draw_vars.set_texture(0, &self.bench[b].0.tex);
            self.show.draw_super.draw_abs(cx, r);
        }
        if self.bench.iter().all(|(_, t)| t.len() >= 60) {
            let median = |t: &Vec<f64>| {
                let mut v = t[10..].to_vec();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                v[v.len() / 2]
            };
            println!("ATLAS-BENCH fast={:.3} precise={:.3} (ms per {N}x{N} R32F kernel pass, medians)", median(&self.bench[0].1), median(&self.bench[1].1));
            std::process::exit(0);
        }
        if self.frame > 5000 {
            println!("ATLAS-BENCH-FAIL no GPU timings after {} frames ({} / {})", self.frame, self.bench[0].1.len(), self.bench[1].1.len());
            std::process::exit(1);
        }
    }

    fn selftest_step(&mut self, cx: &mut Cx) {
        if !self.requested && self.phase > 2 {
            let mut list: Vec<(&'static str, Texture)> = self.targets.iter().map(|t| (t.name, t.tex.clone())).collect();
            list.push(("mrt_r8", self.targets[T_MRT].extra.clone().unwrap()));
            for (name, tex) in list {
                let out = self.readbacks.clone();
                let r = cx.read_render_texture(&tex, move |res| {
                    out.lock().unwrap().push((name, res.map_err(|e| format!("{e:?}"))));
                });
                if let Err(e) = r {
                    println!("ATLAS-FAIL readback request {name}: {e:?}");
                    std::process::exit(1);
                }
            }
            self.requested = true;
        }
        let done = self.readbacks.try_lock().map(|r| r.len() == self.targets.len() + 1).unwrap_or(false);
        if done {
            let got = std::mem::take(&mut *self.readbacks.lock().unwrap());
            std::process::exit(if check(&got) { 0 } else { 1 });
        }
        // a frame budget, not a wall-clock one
        if self.frame > 3000 {
            println!("ATLAS-FAIL readbacks did not finish in {} frames", self.frame);
            std::process::exit(1);
        }
    }
}

impl Widget for AtlasView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if matches!(event, Event::Startup) {
            self.next_frame = cx.new_next_frame();
        }
        if let Event::NextFrame(ne) = event {
            if ne.set.contains(&self.next_frame) {
                self.frame += 1;
                if selftest() && !bench() {
                    self.selftest_step(cx);
                }
                self.area.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        if self.targets.is_empty() {
            self.make_targets(cx.cx.cx);
        }
        if bench() {
            self.bench_frame(cx);
            cx.end_turtle_with_area(&mut self.area);
            return DrawStep::done();
        }
        self.draw_frame(cx);
        let rect = cx.turtle().rect();
        let n = self.targets.len() + 1;
        let cols = 4;
        let (cw, ch) = (rect.size.x / cols as f64, rect.size.y / ((n + cols - 1) / cols) as f64);
        let mut texs: Vec<Texture> = self.targets.iter().map(|t| t.tex.clone()).collect();
        texs.extend(self.targets[T_MRT].extra.clone());
        for (i, tex) in texs.iter().enumerate() {
            let r = Rect { pos: dvec2(rect.pos.x + (i % cols) as f64 * cw, rect.pos.y + (i / cols) as f64 * ch), size: dvec2(cw - 2.0, ch - 2.0) };
            self.show.draw_super.draw_vars.set_texture(0, tex);
            self.show.draw_super.draw_abs(cx, r);
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

fn bench() -> bool {
    std::env::args().any(|a| a == "--bench")
}

fn selftest() -> bool {
    std::env::args().any(|a| a == "--selftest") || std::env::var_os("ATLAS_SELFTEST").is_some()
}

// ---------------------------------------------------------------------------
// The CPU model of every target.
// ---------------------------------------------------------------------------

fn slot_of(x: usize, y: usize, slots_x: usize) -> usize {
    (y / SLOT) * slots_x + x / SLOT
}

fn atlas_stamp(x: usize, y: usize) -> Option<f32> {
    let s = slot_of(x, y, SLOTS_X);
    if in_s2(s) {
        Some(2.0)
    } else if in_s1(s) {
        Some(1.0)
    } else {
        None
    }
}

fn u8_value(x: usize, y: usize, stamp: f32) -> u8 {
    let n = x as f32 * 7.0 + y as f32 * 3.0 + stamp * 11.0;
    (n - (n * 0.00390625).floor() * 256.0) as u8
}

fn f16_value(x: usize, y: usize, stamp: f32) -> f32 {
    x as f32 * 0.5 + y as f32 + stamp
}

/// The kernel's CPU twin: the shader's ops in the shader's order, plain f32 (Rust never fuses).
fn kernel(gx: f32, gy: f32, gz: f32) -> f32 {
    let qx = gx * CELL + ORIGIN;
    let qy = gy * CELL + ORIGIN;
    let qz = gz * CELL + ORIGIN;
    let a = (qx * qx + qy * qy + qz * qz).sqrt() - 0.61;
    let bx = (qx - 0.13).abs() - 0.37;
    let by = (qy + 0.05).abs() - 0.29;
    let bz = (qz - 0.21).abs() - 0.33;
    let ox = bx.max(0.0);
    let oy = by.max(0.0);
    let oz = bz.max(0.0);
    let b = (ox * ox + oy * oy + oz * oz).sqrt() + bx.max(by.max(bz)).min(0.0);
    let k = 0.17f32;
    let h = (k - (a - b).abs()).max(0.0) / k;
    let u = a.min(b) - h * h * k * 0.25;
    let dx = qx - 0.3;
    let dy = qy + 0.2;
    let dz = qz - 0.45;
    let t = (dx * dx + dy * dy + dz * dz) / 0.2025;
    let mut w = 0.0f32;
    if t < 1.0 {
        let s = 1.0 - t;
        w = s * s;
    }
    match probe() {
        1 => (qx * qx + qy * qy + qz * qz).sqrt(),
        2 => (dx * dx + dy * dy + dz * dz) / 0.2025,
        3 => qx * qy * 0.3 + qz * 0.01,
        4 => qx * qx + qy * qy + qz * qz,
        5 => (qx * qy) / (qz + 1.37),
        6 | 7 => {
            let h0 = (gx_px(gx, gz) .wrapping_mul(7919)).wrapping_add(gy_px(gy, gz).wrapping_mul(104729)).wrapping_mul(2654435761);
            let h1 = (h0 ^ (h0 >> 15)).wrapping_mul(2246822519);
            let x0 = f32::from_bits(((((h0 >> 23) & 31) + 107) << 23) | (h0 & 8388607));
            let x1 = f32::from_bits(((((h1 >> 23) & 31) + 107) << 23) | (h1 & 8388607));
            if probe() == 6 { x0 / x1 } else { x0.sqrt() }
        }
        _ => u - w * 0.05 + qx * qy * 0.3 + qz * 0.01,
    }
}

/// The kernel texel's pixel coordinates back from its lattice point.
fn gx_px(gx: f32, gz: f32) -> u32 {
    gx as u32 + (gz as u32 % 4) * 64
}
fn gy_px(gy: f32, gz: f32) -> u32 {
    gy as u32 + (gz as u32 / 4) * 64
}

/// `ATLAS_PROBE=<n>`: the kernel targets hold one sub-expression instead (precision diagnosis).
fn probe() -> u32 {
    std::env::var("ATLAS_PROBE").ok().and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn half_to_f32(h: u16) -> f32 {
    let s = ((h >> 15) as u32) << 31;
    let e = ((h >> 10) & 0x1f) as u32;
    let m = (h & 0x3ff) as u32;
    let bits = if e == 0 {
        if m == 0 {
            s
        } else {
            // subnormal: normalise
            let mut e = 127 - 15 + 1;
            let mut m = m;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            s | (e << 23) | ((m & 0x3ff) << 13)
        }
    } else if e == 31 {
        s | 0x7f80_0000 | (m << 13)
    } else {
        s | ((e + 127 - 15) << 23) | (m << 13)
    };
    f32::from_bits(bits)
}

fn f32s(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn f16s(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(2).map(|c| half_to_f32(u16::from_le_bytes([c[0], c[1]]))).collect()
}

/// Compares every read-back target with its model; prints one line per target.
fn check(got: &[(&'static str, Result<(usize, usize, Arc<[u8]>), String>)]) -> bool {
    let mut fails = Vec::new();
    for (name, res) in got {
        let (w, h, bytes) = match res {
            Ok(v) => v.clone(),
            Err(e) => {
                println!("ATLAS case={name} readback error {e}");
                fails.push(*name);
                continue;
            }
        };
        let mut bad = 0usize;
        let mut first = String::new();
        let mut note = |x: usize, y: usize, want: String, got: String, bad: &mut usize| {
            if *bad == 0 {
                first = format!(" first=({x},{y}) want {want} got {got}");
            }
            *bad += 1;
        };
        match *name {
            "atlas_r32f" | "atlas_r16f" | "atlas_r8" => {
                let vals: Vec<f32> = match *name {
                    "atlas_r32f" => f32s(&bytes),
                    "atlas_r16f" => f16s(&bytes),
                    _ => bytes.iter().map(|b| *b as f32).collect(),
                };
                if vals.len() != w * h || w != AW || h != AH {
                    println!("ATLAS case={name} size {w}x{h} with {} texels", vals.len());
                    fails.push(*name);
                    continue;
                }
                for y in 0..AH {
                    for x in 0..AW {
                        let want = match (*name, atlas_stamp(x, y)) {
                            ("atlas_r32f", Some(k)) => x as f32 * 1024.0 + y as f32 + k * 0.25,
                            ("atlas_r32f", None) => INIT_F32,
                            ("atlas_r16f", Some(k)) => f16_value(x, y, k),
                            ("atlas_r16f", None) => INIT_F16,
                            (_, Some(k)) => u8_value(x, y, k) as f32,
                            (_, None) => 0.0,
                        };
                        let v = vals[y * AW + x];
                        if v.to_bits() != want.to_bits() {
                            note(x, y, want.to_string(), v.to_string(), &mut bad);
                        }
                    }
                }
            }
            "mrt_r32f" | "mrt_r8" => {
                let r8 = *name == "mrt_r8";
                let vals: Vec<f32> = if r8 { bytes.iter().map(|b| *b as f32).collect() } else { f32s(&bytes) };
                for y in 0..h.min(AH) {
                    for x in 0..w.min(AW) {
                        let want = if r8 { ((x + y * 5) % 256) as f32 } else { x as f32 + y as f32 * 512.0 + 0.5 };
                        let v = vals[y * w + x];
                        if v.to_bits() != want.to_bits() {
                            note(x, y, want.to_string(), v.to_string(), &mut bad);
                        }
                    }
                }
            }
            "kernel_fast" | "kernel_precise" => {
                let vals = f32s(&bytes);
                let mut max_ulp = 0u32;
                for y in 0..KH {
                    for x in 0..KW {
                        let (tx, ty) = (x / 64, y / 64);
                        let want = kernel((x % 64) as f32, (y % 64) as f32, (tx + ty * 4) as f32);
                        let v = vals[y * KW + x];
                        if v.to_bits() != want.to_bits() {
                            max_ulp = max_ulp.max((v.to_bits() as i64 - want.to_bits() as i64).unsigned_abs() as u32);
                            note(x, y, format!("{want:e}"), format!("{v:e}"), &mut bad);
                        }
                    }
                }
                println!("ATLAS case={name} differing={bad} of {} max_ulp={max_ulp}{first}", KW * KH);
                // the fast-math twin is a measurement, not a requirement
                if *name == "kernel_precise" && bad != 0 {
                    fails.push(*name);
                }
                continue;
            }
            "pingpong_a" | "pingpong_b" => {
                let vals = f32s(&bytes);
                for y in 0..PH {
                    for x in 0..PW {
                        let a0 = x as f32 * 1024.0 + y as f32;
                        let step = in_pp(slot_of(x, y, PW / SLOT));
                        let want = match (*name, step) {
                            (_, true) => a0 * 2.0 + 1.0,
                            ("pingpong_a", false) => a0,
                            _ => INIT_F32,
                        };
                        let v = vals[y * PW + x];
                        if v.to_bits() != want.to_bits() {
                            note(x, y, want.to_string(), v.to_string(), &mut bad);
                        }
                    }
                }
            }
            "filter" => {
                let vals = f32s(&bytes);
                let texel = |x: usize, y: usize| atlas_stamp(x, y).map_or(INIT_F16, |k| f16_value(x, y, k));
                for y in 0..FH {
                    for x in 0..FW {
                        let want = (texel(2 * x, y) + texel(2 * x + 1, y)) * 0.5;
                        let v = vals[y * FW + x];
                        if (v - want).abs() > 0.07 {
                            note(x, y, want.to_string(), v.to_string(), &mut bad);
                        }
                    }
                }
            }
            _ => {}
        }
        println!("ATLAS case={name} {w}x{h} differing={bad}{first}");
        if bad != 0 {
            fails.push(*name);
        }
    }
    if fails.is_empty() {
        println!("ATLAS-OK cases={}", got.len());
        true
    } else {
        println!("ATLAS-FAIL {}", fails.join(" "));
        false
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    /// The selftest's own frame count: if the view never runs (a script
    /// error), the test still ends.
    #[rust]
    frames: u64,
    #[rust]
    next_frame: NextFrame,
}

impl MatchEvent for App {}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if selftest() {
            if matches!(event, Event::Startup) {
                self.next_frame = cx.new_next_frame();
            }
            if let Event::NextFrame(ne) = event {
                if ne.set.contains(&self.next_frame) {
                    self.frames += 1;
                    if self.frames > 4000 {
                        println!("ATLAS-FAIL the view did not finish in {} frames", self.frames);
                        std::process::exit(1);
                    }
                    self.next_frame = cx.new_next_frame();
                }
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
