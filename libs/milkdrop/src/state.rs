//! The CPU half of a MilkDrop frame: the equations that must run on the
//! CPU, producing one [`FrameParams`] for the GPU passes.
//!
//! Per frame:
//! 1. the per-frame block runs on a context whose built-in variables start
//!    from the preset's values every frame and whose `q1..q32` start from
//!    the values `per_frame_init` left (MilkDrop's rules; user variables
//!    persist);
//! 2. each enabled custom wave runs its per-frame block (colour, `t1..t8`,
//!    sample count), then either hands its samples to the GPU (per-point
//!    code in the wave shader) or evaluates its per-point block here when
//!    it carries state from point to point ([`crate::eel_gpu`] decides);
//! 3. each enabled custom shape runs its per-frame block once per instance;
//! 4. per-vertex code runs here only when it cannot run on the GPU; the
//!    warped UV grid is then uploaded instead of computed by the mesh pass;
//! 5. the basic wave, motion vectors and borders are laid out.
//!
//! Variable names resolve to context slots once, at load; a frame only
//! reads and writes slots and runs programs.

use crate::audio::{AudioFrame, SPECTRUM_BINS, WAVE_SAMPLES};
use crate::eel_gpu::{per_point, per_vertex, GpuPlan};
use crate::expr::{CompileError, Context, Megabuf, Program};
use crate::parse::{Preset, SHAPES, WAVES};
use std::sync::Arc;

/// Instances drawn per custom shape at most.
pub const MAX_SHAPE_INSTANCES: usize = 1024;
/// Points per custom wave at most.
pub const MAX_WAVE_POINTS: usize = 512;

/// Per-frame values the frame's code starts from, with MilkDrop's
/// defaults for keys a preset leaves out.
const FRAME_BUILTINS: &[(&str, f64)] = &[
    ("zoom", 1.0),
    ("zoomexp", 1.0),
    ("rot", 0.0),
    ("warp", 1.0),
    ("cx", 0.5),
    ("cy", 0.5),
    ("dx", 0.0),
    ("dy", 0.0),
    ("sx", 1.0),
    ("sy", 1.0),
    ("decay", 0.98),
    ("gamma", 2.0),
    ("echo_zoom", 2.0),
    ("echo_alpha", 0.0),
    ("echo_orient", 0.0),
    ("wave_mode", 0.0),
    ("wave_r", 1.0),
    ("wave_g", 1.0),
    ("wave_b", 1.0),
    ("wave_a", 0.8),
    ("wave_x", 0.5),
    ("wave_y", 0.5),
    ("wave_mystery", 0.0),
    ("wave_scale", 1.0),
    ("wave_smoothing", 0.75),
    ("wave_usedots", 0.0),
    ("wave_thick", 0.0),
    ("wave_additive", 0.0),
    ("wave_brighten", 1.0),
    ("modwavealphabyvolume", 0.0),
    ("modwavealphastart", 0.75),
    ("modwavealphaend", 0.95),
    ("warpanimspeed", 1.0),
    ("warpscale", 1.0),
    ("ob_size", 0.01),
    ("ob_r", 0.0),
    ("ob_g", 0.0),
    ("ob_b", 0.0),
    ("ob_a", 0.0),
    ("ib_size", 0.01),
    ("ib_r", 0.25),
    ("ib_g", 0.25),
    ("ib_b", 0.25),
    ("ib_a", 0.0),
    ("mv_x", 12.0),
    ("mv_y", 9.0),
    ("mv_dx", 0.0),
    ("mv_dy", 0.0),
    ("mv_l", 0.9),
    ("mv_r", 1.0),
    ("mv_g", 1.0),
    ("mv_b", 1.0),
    ("mv_a", 0.0),
    ("darken_center", 0.0),
    ("wrap", 1.0),
    ("invert", 0.0),
    ("brighten", 0.0),
    ("darken", 0.0),
    ("solarize", 0.0),
    ("red_blue", 0.0),
    ("blur1_min", 0.0),
    ("blur1_max", 1.0),
    ("blur2_min", 0.0),
    ("blur2_max", 1.0),
    ("blur3_min", 0.0),
    ("blur3_max", 1.0),
    ("blur1_edge_darken", 0.25),
];

/// Read-only inputs the host sets every frame.
const FRAME_INPUTS: &[&str] = &[
    "time", "fps", "frame", "progress", "bass", "mid", "treb", "vol", "bass_att", "mid_att", "treb_att",
    "vol_att", "meshx", "meshy", "pixelsx", "pixelsy", "aspectx", "aspecty",
];

/// What the host tells the engine about this frame.
pub struct FrameInput<'a> {
    pub time: f64,
    pub fps: f64,
    pub frame: u64,
    /// 0..1 through the preset's time on screen (the host's playlist).
    pub progress: f64,
    pub audio: &'a AudioFrame,
    /// Render size in pixels.
    pub width: f32,
    pub height: f32,
    /// Warp mesh cells (x, y).
    pub mesh: (usize, usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motion {
    pub zoom: f32,
    pub zoomexp: f32,
    pub rot: f32,
    pub warp: f32,
    pub cx: f32,
    pub cy: f32,
    pub dx: f32,
    pub dy: f32,
    pub sx: f32,
    pub sy: f32,
}

/// A polyline (or dots) drawn from CPU-computed points: the basic wave,
/// motion vectors, custom waves evaluated here.
#[derive(Clone, Debug, Default)]
pub struct LineBatch {
    /// Points in wave space (0..1, y up).
    pub points: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    /// Draw each point alone instead of segments between neighbours.
    pub dots: bool,
    /// Pairs: segments are (0,1), (2,3), ... instead of a polyline.
    pub pairs: bool,
    pub thick: bool,
    pub additive: bool,
}

/// A custom wave whose per-point code runs in the wave shader.
#[derive(Clone, Debug, Default)]
pub struct GpuWave {
    /// Which custom wave (0..4): selects the wave shader subclass.
    pub slot: usize,
    /// (sample, value1, value2) per point.
    pub inputs: Vec<[f32; 3]>,
    pub rgba: [f32; 4],
    pub t: [f32; 8],
    pub q: [f32; 32],
    pub dots: bool,
    pub thick: bool,
    pub additive: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ShapeInstance {
    pub x: f32,
    pub y: f32,
    pub rad: f32,
    pub ang: f32,
    pub sides: f32,
    pub textured: bool,
    pub tex_zoom: f32,
    pub tex_ang: f32,
    pub additive: bool,
    pub thick: bool,
    pub inner: [f32; 4],
    pub outer: [f32; 4],
    pub border: [f32; 4],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Border {
    pub size: f32,
    pub rgba: [f32; 4],
}

/// Everything the GPU passes need for one frame.
#[derive(Clone, Debug, Default)]
pub struct FrameParams {
    pub motion: Motion,
    pub decay: f32,
    pub wrap: bool,
    pub warp_time: f32,
    pub warp_scale_inv: f32,
    pub warp_f: [f32; 4],
    pub q: [f32; 32],
    pub gamma: f32,
    pub echo_zoom: f32,
    pub echo_alpha: f32,
    pub echo_orient: f32,
    pub brighten: bool,
    pub darken: bool,
    pub solarize: bool,
    pub invert: bool,
    pub darken_center: bool,
    pub blur_min: [f32; 3],
    pub blur_max: [f32; 3],
    pub blur1_edge_darken: f32,
    pub outer_border: Border,
    pub inner_border: Border,
    pub lines: Vec<LineBatch>,
    pub gpu_waves: Vec<GpuWave>,
    pub shapes: Vec<ShapeInstance>,
    /// The warped UV per mesh vertex, row by row, when the per-vertex code
    /// ran here; `None` when the mesh pass computes it on the GPU.
    pub cpu_mesh: Option<Vec<[f32; 2]>>,
    pub rand_frame: [f32; 4],
    pub rand_preset: [f32; 4],
    pub roam_cos: [f32; 4],
    pub roam_sin: [f32; 4],
    pub slow_roam_cos: [f32; 4],
    pub slow_roam_sin: [f32; 4],
    pub hue: [[f32; 3]; 4],
}

/// How the per-vertex code runs.
enum VertexMode {
    /// No per-vertex code: the mesh pass applies the per-frame motion.
    None,
    /// The mesh shader carries the compiled code (see [`Engine::gpu_vertex`]).
    Gpu(String),
    /// The VM runs it per vertex ([`LoadReport::vertex_cpu`] says why).
    Cpu { ctx: Context, prog: Program, slots: VertexSlots },
}

struct VertexSlots {
    x: usize,
    y: usize,
    rad: usize,
    ang: usize,
    motion: [usize; 10],
    inputs: Vec<(usize, usize)>,
    q: [usize; 32],
}

struct WaveRt {
    ctx: Context,
    frame: Program,
    t_init: [f64; 8],
    t: [usize; 8],
    q: [usize; 32],
    inputs: Vec<(usize, usize)>,
    base: Vec<(usize, f64)>,
    out: WaveSlots,
    point: PointMode,
}

struct WaveSlots {
    r: usize,
    g: usize,
    b: usize,
    a: usize,
    samples: usize,
    sep: usize,
    scaling: usize,
    smoothing: usize,
    spectrum: usize,
    usedots: usize,
    thick: usize,
    additive: usize,
}

enum PointMode {
    None,
    Gpu(String),
    Cpu { ctx: Context, prog: Program, slots: PointSlots },
}

struct PointSlots {
    sample: usize,
    value1: usize,
    value2: usize,
    x: usize,
    y: usize,
    rgba: [usize; 4],
    t: [usize; 8],
    q: [usize; 32],
    inputs: Vec<(usize, usize)>,
}

struct ShapeRt {
    ctx: Context,
    frame: Program,
    t_init: [f64; 8],
    t: [usize; 8],
    q: [usize; 32],
    inputs: Vec<(usize, usize)>,
    base: Vec<(usize, f64)>,
    instance: usize,
    num_inst: usize,
    out: [usize; 22],
}

/// Shape outputs in the order of [`ShapeRt::out`].
const SHAPE_OUT: [&str; 22] = [
    "x", "y", "rad", "ang", "sides", "textured", "tex_zoom", "tex_ang", "additive", "thick", "r", "g", "b", "a", "r2",
    "g2", "b2", "a2", "border_r", "border_g", "border_b", "border_a",
];

const SHAPE_DEFAULTS: [f64; 22] =
    [0.5, 0.5, 0.1, 0.0, 4.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0];

/// Compile errors found while loading, kept for display; the preset still
/// renders with the parts that compiled.
#[derive(Clone, Debug, Default)]
pub struct LoadReport {
    pub errors: Vec<String>,
    /// Why per-vertex code runs on the CPU, if it does.
    pub vertex_cpu: Option<String>,
    /// Waves whose per-point code runs on the CPU, with the reason.
    pub waves_cpu: Vec<(usize, String)>,
}

/// CPU time of the last frame's stages, in ms.
#[derive(Clone, Copy, Debug, Default)]
pub struct StageTimes {
    pub per_frame: f64,
    /// Custom waves: per-frame code, and per-point code run on the CPU.
    pub waves: f64,
    pub shapes: f64,
    /// Per-vertex code on the CPU (the fallback); 0 when it runs on the GPU.
    pub per_vertex: f64,
    /// Basic wave and motion vectors.
    pub rest: f64,
}

/// One loaded preset's equation state.
pub struct Engine {
    pub preset: Arc<Preset>,
    frame_ctx: Context,
    frame_prog: Program,
    base: Vec<(usize, f64)>,
    inputs: Vec<(usize, usize)>,
    q: [usize; 32],
    q_init: [f64; 32],
    out: Vec<usize>,
    vertex: VertexMode,
    waves: Vec<Option<WaveRt>>,
    shapes: Vec<Option<ShapeRt>>,
    gmegabuf: Megabuf,
    rand_preset: [f32; 4],
    rng: u64,
    pub report: LoadReport,
    pub times: StageTimes,
}

fn compile(src: &str, ctx: &mut Context, what: &str, errors: &mut Vec<String>) -> Program {
    match Program::compile(src, ctx) {
        Ok(p) => p,
        Err(CompileError { message, offset }) => {
            errors.push(format!("{what}: {message} at byte {offset}"));
            Program::default()
        }
    }
}

fn q_slots(ctx: &mut Context) -> [usize; 32] {
    std::array::from_fn(|i| ctx.slot(&format!("q{}", i + 1)))
}

fn t_slots(ctx: &mut Context) -> [usize; 8] {
    std::array::from_fn(|i| ctx.slot(&format!("t{}", i + 1)))
}

fn input_slots(ctx: &mut Context) -> Vec<(usize, usize)> {
    FRAME_INPUTS.iter().enumerate().map(|(i, name)| (ctx.slot(name), i)).collect()
}

impl Engine {
    pub fn new(preset: Arc<Preset>, seed: u64) -> Engine {
        let mut errors = Vec::new();
        let mut frame_ctx = Context::new(seed ^ 0x9e37_79b9_7f4a_7c15);
        let base: Vec<(usize, f64)> = FRAME_BUILTINS
            .iter()
            .map(|(name, default)| (frame_ctx.slot(name), preset.get(name, *default)))
            .collect();
        let out: Vec<usize> = FRAME_BUILTINS.iter().map(|(name, _)| frame_ctx.slot(name)).collect();
        let inputs = input_slots(&mut frame_ctx);
        let q = q_slots(&mut frame_ctx);
        let init = compile(&preset.per_frame_init, &mut frame_ctx, "per_frame_init", &mut errors);
        let frame_prog = compile(&preset.per_frame, &mut frame_ctx, "per_frame", &mut errors);

        let mut rng = seed | 1;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng >> 40) as f32 / (1u64 << 24) as f32
        };
        let rand_preset = [next(), next(), next(), next()];

        let mut engine = Engine {
            preset: preset.clone(),
            frame_ctx,
            frame_prog,
            base,
            inputs,
            q,
            q_init: [0.0; 32],
            out,
            vertex: VertexMode::None,
            waves: Vec::new(),
            shapes: Vec::new(),
            gmegabuf: Megabuf::default(),
            rand_preset,
            rng,
            report: LoadReport::default(),
            times: StageTimes::default(),
        };
        // per_frame_init runs once with the base values in place; the q
        // values it leaves are where every frame's q values start.
        for (slot, value) in engine.base.clone() {
            engine.frame_ctx.values[slot] = value;
        }
        init.run(&mut engine.frame_ctx, &mut engine.gmegabuf);
        for i in 0..32 {
            engine.q_init[i] = engine.frame_ctx.values[engine.q[i]];
        }

        // Per-vertex code.
        engine.vertex = match per_vertex(&preset.per_pixel) {
            GpuPlan::Empty => VertexMode::None,
            GpuPlan::Gpu(src) => VertexMode::Gpu(src),
            GpuPlan::Cpu(reason) => {
                let mut ctx = Context::new(seed ^ 0x51);
                let slots = VertexSlots {
                    x: ctx.slot("x"),
                    y: ctx.slot("y"),
                    rad: ctx.slot("rad"),
                    ang: ctx.slot("ang"),
                    motion: ["zoom", "zoomexp", "rot", "warp", "cx", "cy", "dx", "dy", "sx", "sy"].map(|n| ctx.slot(n)),
                    inputs: input_slots(&mut ctx),
                    q: q_slots(&mut ctx),
                };
                let prog = compile(&preset.per_pixel, &mut ctx, "per_pixel", &mut errors);
                engine.report.vertex_cpu = Some(reason);
                VertexMode::Cpu { ctx, prog, slots }
            }
        };

        // Custom waves.
        for (i, def) in preset.waves.iter().enumerate().take(WAVES) {
            if !def.enabled() {
                engine.waves.push(None);
                continue;
            }
            let mut ctx = Context::new(seed ^ (0x100 + i as u64));
            let base_names = [
                ("r", 1.0),
                ("g", 1.0),
                ("b", 1.0),
                ("a", 1.0),
                ("samples", 512.0),
                ("sep", 0.0),
                ("scaling", 1.0),
                ("smoothing", 0.5),
                ("spectrum", 0.0),
                ("usedots", 0.0),
                ("thick", 0.0),
                ("additive", 0.0),
            ];
            let base: Vec<(usize, f64)> =
                base_names.iter().map(|(n, d)| (ctx.slot(n), def.get(n, *d))).collect();
            let out = WaveSlots {
                r: ctx.slot("r"),
                g: ctx.slot("g"),
                b: ctx.slot("b"),
                a: ctx.slot("a"),
                samples: ctx.slot("samples"),
                sep: ctx.slot("sep"),
                scaling: ctx.slot("scaling"),
                smoothing: ctx.slot("smoothing"),
                spectrum: ctx.slot("spectrum"),
                usedots: ctx.slot("usedots"),
                thick: ctx.slot("thick"),
                additive: ctx.slot("additive"),
            };
            let t = t_slots(&mut ctx);
            let q = q_slots(&mut ctx);
            let inputs = input_slots(&mut ctx);
            let init = compile(&def.init, &mut ctx, &format!("wave {i} init"), &mut errors);
            let frame = compile(&def.per_frame, &mut ctx, &format!("wave {i} per_frame"), &mut errors);
            for (slot, v) in &base {
                ctx.values[*slot] = *v;
            }
            for k in 0..32 {
                ctx.values[q[k]] = engine.q_init[k];
            }
            init.run(&mut ctx, &mut engine.gmegabuf);
            let t_init = std::array::from_fn(|k| ctx.values[t[k]]);
            let point = match per_point(&def.per_point) {
                GpuPlan::Empty => PointMode::None,
                GpuPlan::Gpu(src) => PointMode::Gpu(src),
                GpuPlan::Cpu(reason) => {
                    let mut pctx = Context::new(seed ^ (0x200 + i as u64));
                    let slots = PointSlots {
                        sample: pctx.slot("sample"),
                        value1: pctx.slot("value1"),
                        value2: pctx.slot("value2"),
                        x: pctx.slot("x"),
                        y: pctx.slot("y"),
                        rgba: ["r", "g", "b", "a"].map(|n| pctx.slot(n)),
                        t: t_slots(&mut pctx),
                        q: q_slots(&mut pctx),
                        inputs: input_slots(&mut pctx),
                    };
                    let prog = compile(&def.per_point, &mut pctx, &format!("wave {i} per_point"), &mut errors);
                    engine.report.waves_cpu.push((i, reason));
                    PointMode::Cpu { ctx: pctx, prog, slots }
                }
            };
            engine.waves.push(Some(WaveRt { ctx, frame, t_init, t, q, inputs, base, out, point }));
        }

        // Custom shapes.
        for (i, def) in preset.shapes.iter().enumerate().take(SHAPES) {
            if !def.enabled() {
                engine.shapes.push(None);
                continue;
            }
            let mut ctx = Context::new(seed ^ (0x300 + i as u64));
            let out: [usize; 22] = std::array::from_fn(|k| ctx.slot(SHAPE_OUT[k]));
            let base: Vec<(usize, f64)> =
                (0..22).map(|k| (out[k], def.get(SHAPE_OUT[k], SHAPE_DEFAULTS[k]))).collect();
            let t = t_slots(&mut ctx);
            let q = q_slots(&mut ctx);
            let inputs = input_slots(&mut ctx);
            let instance = ctx.slot("instance");
            let num_inst = (def.get("num_inst", 1.0).max(1.0) as usize).min(MAX_SHAPE_INSTANCES);
            let init = compile(&def.init, &mut ctx, &format!("shape {i} init"), &mut errors);
            let frame = compile(&def.per_frame, &mut ctx, &format!("shape {i} per_frame"), &mut errors);
            for (slot, v) in &base {
                ctx.values[*slot] = *v;
            }
            for k in 0..32 {
                ctx.values[q[k]] = engine.q_init[k];
            }
            init.run(&mut ctx, &mut engine.gmegabuf);
            let t_init = std::array::from_fn(|k| ctx.values[t[k]]);
            engine.shapes.push(Some(ShapeRt { ctx, frame, t_init, t, q, inputs, base, instance, num_inst, out }));
        }
        engine.report.errors = errors;
        engine
    }

    /// The mesh shader member for GPU per-vertex code, if the preset has it.
    pub fn gpu_vertex(&self) -> Option<&str> {
        match &self.vertex {
            VertexMode::Gpu(src) => Some(src),
            _ => None,
        }
    }

    /// The wave shader members for custom wave `slot`, when its per-point
    /// code runs on the GPU.
    pub fn gpu_wave(&self, slot: usize) -> Option<&str> {
        match self.waves.get(slot) {
            Some(Some(WaveRt { point: PointMode::Gpu(src), .. })) => Some(src),
            _ => None,
        }
    }

    fn next_rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Run one frame's CPU work into `params`.
    pub fn frame(&mut self, input: &FrameInput, params: &mut FrameParams) {
        let a = input.audio;
        let (aspect_x, aspect_y) = aspect(input.width, input.height);
        let input_values = [
            input.time,
            input.fps,
            input.frame as f64,
            input.progress,
            a.bass as f64,
            a.mid as f64,
            a.treb as f64,
            a.vol as f64,
            a.bass_att as f64,
            a.mid_att as f64,
            a.treb_att as f64,
            a.vol_att as f64,
            input.mesh.0 as f64,
            input.mesh.1 as f64,
            input.width as f64,
            input.height as f64,
            aspect_x as f64,
            aspect_y as f64,
        ];

        // 1. Per-frame.
        let mut clock = std::time::Instant::now();
        let mut lap = |to: &mut f64| {
            let now = std::time::Instant::now();
            *to = (now - clock).as_secs_f64() * 1000.0;
            clock = now;
        };
        let mut times = StageTimes::default();
        for &(slot, value) in &self.base {
            self.frame_ctx.values[slot] = value;
        }
        for &(slot, i) in &self.inputs {
            self.frame_ctx.values[slot] = input_values[i];
        }
        for i in 0..32 {
            self.frame_ctx.values[self.q[i]] = self.q_init[i];
        }
        self.frame_prog.run(&mut self.frame_ctx, &mut self.gmegabuf);
        let v = |name_index: usize, ctx: &Context, out: &[usize]| -> f32 {
            let x = ctx.values[out[name_index]];
            if x.is_finite() {
                x as f32
            } else {
                0.0
            }
        };
        let f = |name: &str| -> f32 {
            let i = FRAME_BUILTINS.iter().position(|(n, _)| *n == name).unwrap();
            v(i, &self.frame_ctx, &self.out)
        };
        let q: [f64; 32] = std::array::from_fn(|i| {
            let x = self.frame_ctx.values[self.q[i]];
            if x.is_finite() {
                x
            } else {
                0.0
            }
        });
        params.q = q.map(|x| x as f32);
        params.motion = Motion {
            zoom: f("zoom"),
            zoomexp: f("zoomexp"),
            rot: f("rot"),
            warp: f("warp"),
            cx: f("cx"),
            cy: f("cy"),
            dx: f("dx"),
            dy: f("dy"),
            sx: f("sx"),
            sy: f("sy"),
        };
        params.decay = f("decay").clamp(0.0, 1.0);
        params.wrap = f("wrap") != 0.0;
        let warp_speed = f("warpanimspeed");
        let warp_scale = f("warpscale");
        let time = input.time as f32;
        params.warp_time = time * warp_speed;
        params.warp_scale_inv = if warp_scale.abs() > 1e-6 { 1.0 / warp_scale } else { 1.0 };
        params.warp_f = [
            11.68 + 4.0 * (time * 1.413 + 10.0).cos(),
            8.77 + 3.0 * (time * 1.113 + 7.0).cos(),
            10.54 + 3.0 * (time * 1.233 + 3.0).cos(),
            11.49 + 4.0 * (time * 0.933 + 5.0).cos(),
        ];
        params.gamma = f("gamma");
        params.echo_zoom = f("echo_zoom");
        params.echo_alpha = f("echo_alpha").clamp(0.0, 1.0);
        params.echo_orient = f("echo_orient").rem_euclid(4.0).floor();
        params.brighten = f("brighten") != 0.0;
        params.darken = f("darken") != 0.0;
        params.solarize = f("solarize") != 0.0;
        params.invert = f("invert") != 0.0;
        params.darken_center = f("darken_center") != 0.0;
        params.blur_min = [f("blur1_min"), f("blur2_min"), f("blur3_min")];
        params.blur_max = [f("blur1_max"), f("blur2_max"), f("blur3_max")];
        params.blur1_edge_darken = f("blur1_edge_darken");
        params.outer_border = Border { size: f("ob_size"), rgba: [f("ob_r"), f("ob_g"), f("ob_b"), f("ob_a")] };
        params.inner_border = Border { size: f("ib_size"), rgba: [f("ib_r"), f("ib_g"), f("ib_b"), f("ib_a")] };
        params.rand_frame = [self.next_rand(), self.next_rand(), self.next_rand(), self.next_rand()];
        params.rand_preset = self.rand_preset;
        let roam = [0.3, 0.1435, 0.1233, 0.087];
        let slow = [0.005, 0.008, 0.013, 0.022];
        params.roam_cos = roam.map(|k| 0.5 + 0.5 * (time * k).cos());
        params.roam_sin = roam.map(|k| 0.5 + 0.5 * (time * k).sin());
        params.slow_roam_cos = slow.map(|k| 0.5 + 0.5 * (time * k).cos());
        params.slow_roam_sin = slow.map(|k| 0.5 + 0.5 * (time * k).sin());
        for (i, corner) in params.hue.iter_mut().enumerate() {
            let mut c = [0.0f32; 3];
            for (k, ch) in c.iter_mut().enumerate() {
                *ch = 0.6 + 0.3 * (time * 0.429 * (1.0 + k as f32 * 0.3) + i as f32 * 1.7 + k as f32 * 2.1).sin();
            }
            let max = c.iter().copied().fold(0.0f32, f32::max).max(1e-3);
            *corner = c.map(|x| x / max);
        }

        params.lines.clear();
        params.gpu_waves.clear();
        params.shapes.clear();

        lap(&mut times.per_frame);

        // 2. Custom waves.
        for (slot, wave) in self.waves.iter_mut().enumerate() {
            let Some(w) = wave else { continue };
            for &(s, value) in &w.base {
                w.ctx.values[s] = value;
            }
            for &(s, i) in &w.inputs {
                w.ctx.values[s] = input_values[i];
            }
            for i in 0..32 {
                w.ctx.values[w.q[i]] = q[i];
            }
            for i in 0..8 {
                w.ctx.values[w.t[i]] = w.t_init[i];
            }
            w.frame.run(&mut w.ctx, &mut self.gmegabuf);
            let g = |s: usize| -> f32 {
                let x = w.ctx.values[s];
                if x.is_finite() {
                    x as f32
                } else {
                    0.0
                }
            };
            let rgba = [g(w.out.r), g(w.out.g), g(w.out.b), g(w.out.a)];
            let n = (g(w.out.samples).max(0.0) as usize).min(MAX_WAVE_POINTS);
            if n < 2 {
                continue;
            }
            let spectrum = g(w.out.spectrum) != 0.0;
            let sep = g(w.out.sep).max(0.0) as usize;
            let scaling = g(w.out.scaling);
            let smoothing = g(w.out.smoothing).clamp(0.0, 0.99);
            let dots = g(w.out.usedots) != 0.0;
            let thick = g(w.out.thick) != 0.0;
            let additive = g(w.out.additive) != 0.0;
            let (left, right) = wave_values(a, spectrum, n, sep, scaling, smoothing);
            let t: [f64; 8] = std::array::from_fn(|i| w.ctx.values[w.t[i]]);
            match &mut w.point {
                PointMode::None => {
                    // No per-point code: MilkDrop's default placement.
                    let mut batch = LineBatch { dots, thick, additive, ..Default::default() };
                    for i in 0..n {
                        batch.points.push([0.5 + left[i], 0.5 + right[i]]);
                        batch.colors.push(rgba);
                    }
                    params.lines.push(batch);
                }
                PointMode::Gpu(_) => {
                    let inputs =
                        (0..n).map(|i| [i as f32 / (n - 1) as f32, left[i], right[i]]).collect();
                    params.gpu_waves.push(GpuWave {
                        slot,
                        inputs,
                        rgba,
                        t: t.map(|x| x as f32),
                        q: q.map(|x| x as f32),
                        dots,
                        thick,
                        additive,
                    });
                }
                PointMode::Cpu { ctx, prog, slots, .. } => {
                    for &(s, i) in &slots.inputs {
                        ctx.values[s] = input_values[i];
                    }
                    for i in 0..32 {
                        ctx.values[slots.q[i]] = q[i];
                    }
                    for i in 0..8 {
                        ctx.values[slots.t[i]] = t[i];
                    }
                    let mut batch = LineBatch { dots, thick, additive, ..Default::default() };
                    batch.points.reserve(n);
                    batch.colors.reserve(n);
                    for i in 0..n {
                        ctx.values[slots.sample] = i as f64 / (n - 1) as f64;
                        ctx.values[slots.value1] = left[i] as f64;
                        ctx.values[slots.value2] = right[i] as f64;
                        ctx.values[slots.x] = 0.5 + left[i] as f64;
                        ctx.values[slots.y] = 0.5 + right[i] as f64;
                        for k in 0..4 {
                            ctx.values[slots.rgba[k]] = rgba[k] as f64;
                        }
                        prog.run(ctx, &mut self.gmegabuf);
                        let r = |s: usize| {
                            let x = ctx.values[s];
                            if x.is_finite() {
                                x as f32
                            } else {
                                0.0
                            }
                        };
                        batch.points.push([r(slots.x), r(slots.y)]);
                        batch.colors.push([r(slots.rgba[0]), r(slots.rgba[1]), r(slots.rgba[2]), r(slots.rgba[3])]);
                    }
                    params.lines.push(batch);
                }
            }
        }

        lap(&mut times.waves);

        // 3. Custom shapes.
        for shape in self.shapes.iter_mut() {
            let Some(s) = shape else { continue };
            for &(slot, i) in &s.inputs {
                s.ctx.values[slot] = input_values[i];
            }
            for inst in 0..s.num_inst {
                for &(slot, value) in &s.base {
                    s.ctx.values[slot] = value;
                }
                for i in 0..32 {
                    s.ctx.values[s.q[i]] = q[i];
                }
                for i in 0..8 {
                    s.ctx.values[s.t[i]] = s.t_init[i];
                }
                s.ctx.values[s.instance] = inst as f64;
                s.frame.run(&mut s.ctx, &mut self.gmegabuf);
                let o: [f32; 22] = std::array::from_fn(|k| {
                    let x = s.ctx.values[s.out[k]];
                    if x.is_finite() {
                        x as f32
                    } else {
                        0.0
                    }
                });
                params.shapes.push(ShapeInstance {
                    x: o[0],
                    y: o[1],
                    rad: o[2],
                    ang: o[3],
                    sides: o[4].clamp(3.0, 100.0),
                    textured: o[5] != 0.0,
                    tex_zoom: o[6],
                    tex_ang: o[7],
                    additive: o[8] != 0.0,
                    thick: o[9] != 0.0,
                    inner: [o[10], o[11], o[12], o[13]],
                    outer: [o[14], o[15], o[16], o[17]],
                    border: [o[18], o[19], o[20], o[21]],
                });
            }
        }

        lap(&mut times.shapes);

        // 4. Per-vertex code on the CPU (fallback).
        params.cpu_mesh = None;
        if let VertexMode::Cpu { ctx, prog, slots, .. } = &mut self.vertex {
            let (gx, gy) = input.mesh;
            let mut mesh = Vec::with_capacity((gx + 1) * (gy + 1));
            for &(s, i) in &slots.inputs {
                ctx.values[s] = input_values[i];
            }
            let m = params.motion;
            let base = [m.zoom, m.zoomexp, m.rot, m.warp, m.cx, m.cy, m.dx, m.dy, m.sx, m.sy];
            for j in 0..=gy {
                for i in 0..=gx {
                    let fx = i as f32 / gx as f32 * 2.0 - 1.0;
                    let fy = 1.0 - j as f32 / gy as f32 * 2.0;
                    let rad = ((fx * aspect_x).powi(2) + (fy * aspect_y).powi(2)).sqrt();
                    let ang = (fy * aspect_y).atan2(fx * aspect_x);
                    ctx.values[slots.x] = (fx * 0.5 * aspect_x + 0.5) as f64;
                    ctx.values[slots.y] = (-fy * 0.5 * aspect_y + 0.5) as f64;
                    ctx.values[slots.rad] = rad as f64;
                    ctx.values[slots.ang] = ang as f64;
                    for k in 0..10 {
                        ctx.values[slots.motion[k]] = base[k] as f64;
                    }
                    for k in 0..32 {
                        ctx.values[slots.q[k]] = q[k];
                    }
                    prog.run(ctx, &mut self.gmegabuf);
                    let r = |s: usize| {
                        let x = ctx.values[s];
                        if x.is_finite() {
                            x as f32
                        } else {
                            0.0
                        }
                    };
                    let mv = Motion {
                        zoom: r(slots.motion[0]),
                        zoomexp: r(slots.motion[1]),
                        rot: r(slots.motion[2]),
                        warp: r(slots.motion[3]),
                        cx: r(slots.motion[4]),
                        cy: r(slots.motion[5]),
                        dx: r(slots.motion[6]),
                        dy: r(slots.motion[7]),
                        sx: r(slots.motion[8]),
                        sy: r(slots.motion[9]),
                    };
                    mesh.push(warp_uv([fx, fy], rad, &mv, params, aspect_x, aspect_y));
                }
            }
            params.cpu_mesh = Some(mesh);
        }

        lap(&mut times.per_vertex);

        // 5. Basic wave, motion vectors.
        basic_wave(self, a, input, params, aspect_x, aspect_y);
        motion_vectors(self, params, aspect_x, aspect_y);
        lap(&mut times.rest);
        self.times = times;
    }

    fn frame_value(&self, name: &str) -> f32 {
        let i = FRAME_BUILTINS.iter().position(|(n, _)| *n == name).unwrap();
        let x = self.frame_ctx.values[self.out[i]];
        if x.is_finite() {
            x as f32
        } else {
            0.0
        }
    }
}

/// MilkDrop's aspect factors: 1 on the longer axis, short/long on the other.
/// MilkDrop only knew wide screens and shrinks x by `ay`; a tall picture
/// (a phone upright) shrinks y by `ax` the same way, so a circle stays round.
pub fn aspect(width: f32, height: f32) -> (f32, f32) {
    let (w, h) = (width.max(1.0), height.max(1.0));
    if w >= h {
        (1.0, h / w)
    } else {
        (w / h, 1.0)
    }
}

/// The warp mesh formula (same as `DrawMilkBase::warp_uv`), for vertices
/// the CPU evaluates and for motion vectors.
pub fn warp_uv(f: [f32; 2], rad: f32, m: &Motion, p: &FrameParams, ax: f32, ay: f32) -> [f32; 2] {
    let zoom2 = m.zoom.max(0.0001).powf(m.zoomexp.max(0.0001).powf(rad * 2.0 - 1.0));
    let inv = if zoom2.abs() > 0.0001 { 1.0 / zoom2 } else { 1.0 };
    let mut u = f[0] * ax * 0.5 * inv + 0.5;
    let mut v = -f[1] * ay * 0.5 * inv + 0.5;
    let nz = |x: f32| if x.abs() < 0.0001 { 0.0001f32.copysign(x + 1e-7) } else { x };
    u = (u - m.cx) / nz(m.sx) + m.cx;
    v = (v - m.cy) / nz(m.sy) + m.cy;
    let (wt, ws, wf) = (p.warp_time, p.warp_scale_inv, p.warp_f);
    let w = m.warp * 0.0035;
    u += w * (wt * 0.333 + ws * (f[0] * wf[0] - f[1] * wf[3])).sin();
    v += w * (wt * 0.375 - ws * (f[0] * wf[2] + f[1] * wf[1])).cos();
    u += w * (wt * 0.753 - ws * (f[0] * wf[1] - f[1] * wf[2])).cos();
    v += w * (wt * 0.825 + ws * (f[0] * wf[0] + f[1] * wf[3])).sin();
    let (u2, v2) = (u - m.cx, v - m.cy);
    let (cr, sr) = (m.rot.cos(), m.rot.sin());
    u = u2 * cr - v2 * sr + m.cx;
    v = u2 * sr + v2 * cr + m.cy;
    u -= m.dx;
    v -= m.dy;
    [(u - 0.5) / ax + 0.5, (v - 0.5) / ay + 0.5]
}

/// A custom wave's value arrays: waveform or spectrum per channel, scaled,
/// smoothed along the samples, right channel offset by `sep`.
fn wave_values(a: &AudioFrame, spectrum: bool, n: usize, sep: usize, scaling: f32, smoothing: f32) -> (Vec<f32>, Vec<f32>) {
    let src_len = if spectrum { SPECTRUM_BINS } else { WAVE_SAMPLES };
    let pick = |ch: usize, i: usize| -> f32 {
        let i = i.min(src_len - 1);
        if spectrum {
            a.spectrum[ch][i] * 2.0
        } else {
            a.wave[ch][i]
        }
    };
    // Spread n samples over the source (spectrum: the low half, where the
    // music is).
    let span = if spectrum { src_len / 2 } else { src_len };
    let index = |i: usize| i * span / n.max(1);
    let mut left: Vec<f32> = (0..n).map(|i| pick(0, index(i)) * scaling).collect();
    let mut right: Vec<f32> = (0..n).map(|i| pick(1, index(i) + sep) * scaling).collect();
    for i in 1..n {
        left[i] = left[i] * (1.0 - smoothing) + left[i - 1] * smoothing;
        right[i] = right[i] * (1.0 - smoothing) + right[i - 1] * smoothing;
    }
    (left, right)
}

/// MilkDrop 1's built-in wave (`nWaveMode` 0..7), laid out from the
/// waveform. Positions follow MilkDrop's modes; the exact curves are
/// approximations.
fn basic_wave(e: &Engine, a: &AudioFrame, input: &FrameInput, p: &mut FrameParams, ax: f32, ay: f32) {
    let mut alpha = e.frame_value("wave_a");
    if e.frame_value("modwavealphabyvolume") != 0.0 {
        let (s, t) = (e.frame_value("modwavealphastart"), e.frame_value("modwavealphaend"));
        let k = if (t - s).abs() > 1e-4 { ((a.vol - s) / (t - s)).clamp(0.0, 1.0) } else { 1.0 };
        alpha *= k;
    }
    if alpha <= 0.001 {
        return;
    }
    let mut rgb = [e.frame_value("wave_r"), e.frame_value("wave_g"), e.frame_value("wave_b")];
    if e.frame_value("wave_brighten") != 0.0 {
        let m = rgb.iter().copied().fold(0.0f32, f32::max);
        if m > 1e-4 {
            rgb = rgb.map(|c| c / m);
        }
    }
    let color = [rgb[0], rgb[1], rgb[2], alpha.min(1.0)];
    let mode = e.frame_value("wave_mode").rem_euclid(8.0).floor() as u32;
    let (cx, cy) = (e.frame_value("wave_x"), e.frame_value("wave_y"));
    let scale = e.frame_value("wave_scale");
    let mystery = e.frame_value("wave_mystery");
    let smooth = e.frame_value("wave_smoothing").clamp(0.0, 0.99);
    let time = input.time as f32;
    let n = 256usize;
    let mut l: Vec<f32> = (0..n).map(|i| a.wave[0][i * WAVE_SAMPLES / n] * scale).collect();
    let mut r: Vec<f32> = (0..n).map(|i| a.wave[1][i * WAVE_SAMPLES / n] * scale).collect();
    for i in 1..n {
        l[i] = l[i] * (1.0 - smooth) + l[i - 1] * smooth;
        r[i] = r[i] * (1.0 - smooth) + r[i - 1] * smooth;
    }
    let mut batch = LineBatch {
        dots: e.frame_value("wave_usedots") != 0.0,
        thick: e.frame_value("wave_thick") != 0.0,
        additive: e.frame_value("wave_additive") != 0.0,
        ..Default::default()
    };
    let mut push = |x: f32, y: f32, c: [f32; 4]| {
        batch.points.push([x, y]);
        batch.colors.push(c);
    };
    match mode {
        0 => {
            // Circle, radius modulated by the wave, slowly turning.
            for i in 0..=n {
                let k = i % n;
                let ang = k as f32 / n as f32 * std::f32::consts::TAU + time * 0.2;
                let rad = 0.5 * (0.5 + 0.4 * r[k] + mystery);
                push(cx + rad * ang.cos() * ay, cy + rad * ang.sin() * ax, color);
            }
        }
        1 => {
            // X-Y oscilloscope in polar form.
            for i in 0..n {
                let rad = 0.53 + 0.43 * r[i] + mystery;
                let ang = l[i] * 1.57 + time * 2.3;
                push(cx + rad * 0.5 * ang.cos() * ay, cy + rad * 0.5 * ang.sin() * ax, color);
            }
        }
        2 | 3 => {
            // Centred spiro: left drives x, right drives y.
            let c = if mode == 3 { [color[0], color[1], color[2], (color[3] * a.treb * a.treb).min(1.0)] } else { color };
            for i in 0..n {
                push(cx + r[i] * 0.5 * ay, cy + l[i] * 0.5 * ax, c);
            }
        }
        4 => {
            // Horizontal line, displaced by the wave's derivative.
            for i in 0..n {
                let x = i as f32 / (n - 1) as f32;
                let d = if i > 0 { l[i] - l[i - 1] } else { 0.0 };
                push(x, cy + l[i] * 0.25 + d * 0.5 + mystery * (x - 0.5), color);
            }
        }
        5 => {
            // Explosive hash: both channels through a rotation.
            let (s, c) = ((time * 0.35).sin(), (time * 0.35).cos());
            for i in 0..n {
                let (x0, y0) = (r[i] * l[i] + mystery, r[i] * r[i] - l[i] * l[i]);
                push(cx + (x0 * c - y0 * s) * 0.5 * ay, cy + (x0 * s + y0 * c) * 0.5 * ax, color);
            }
        }
        6 | 7 => {
            // A line (or two) across the screen at angle `mystery`.
            let ang = mystery * std::f32::consts::FRAC_PI_2;
            let (dx, dy) = (ang.cos(), ang.sin());
            let lines: &[(f32, &Vec<f32>)] = if mode == 6 { &[(0.0, &l)] } else { &[(-0.1, &l), (0.1, &r)] };
            for (offset, chan) in lines {
                for i in 0..n {
                    let t = i as f32 / (n - 1) as f32 - 0.5;
                    let o = offset + chan[i] * 0.25;
                    push(cx + t * dx - o * dy * ay, cy + t * dy + o * dx * ax, color);
                }
                if mode == 7 && *offset < 0.0 {
                    // Break the polyline between the two lines.
                    push(f32::NAN, f32::NAN, color);
                }
            }
        }
        _ => {}
    }
    p.lines.push(batch);
}

/// MilkDrop's motion vectors: a grid of short lines showing where the warp
/// moves each point this frame (from the per-frame motion; per-vertex code
/// running on the GPU is not reflected, since the frame never reads the GPU
/// mesh back).
fn motion_vectors(e: &Engine, p: &mut FrameParams, ax: f32, ay: f32) {
    let a = e.frame_value("mv_a");
    if a <= 0.001 {
        return;
    }
    let nx = e.frame_value("mv_x").clamp(0.0, 64.0) as usize;
    let ny = e.frame_value("mv_y").clamp(0.0, 48.0) as usize;
    if nx == 0 || ny == 0 {
        return;
    }
    let (odx, ody) = (e.frame_value("mv_dx"), e.frame_value("mv_dy"));
    let len = e.frame_value("mv_l");
    let color = [e.frame_value("mv_r"), e.frame_value("mv_g"), e.frame_value("mv_b"), a.min(1.0)];
    let mut batch = LineBatch { pairs: true, ..Default::default() };
    for j in 0..ny {
        for i in 0..nx {
            let u = ((i as f32 + 0.25) / nx as f32 + odx).rem_euclid(1.0);
            let v = ((j as f32 + 0.25) / ny as f32 + ody).rem_euclid(1.0);
            let f = [u * 2.0 - 1.0, 1.0 - v * 2.0];
            let rad = ((f[0] * ax).powi(2) + (f[1] * ay).powi(2)).sqrt();
            let src = warp_uv(f, rad, &p.motion, p, ax, ay);
            // Wave space is y up; the mesh is in texture space (y down).
            let a0 = [u, 1.0 - v];
            let d = [(u - src[0]) * len, -(v - src[1]) * len];
            batch.points.push(a0);
            batch.points.push([a0[0] + d[0], a0[1] + d[1]]);
            batch.colors.push(color);
            batch.colors.push(color);
        }
    }
    p.lines.push(batch);
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESET: &str = "[preset00]\nfDecay=0.9\nzoom=1.02\nnWaveMode=2\nfWaveAlpha=1\nper_frame_init_1=q5 = 3;\nper_frame_1=zoom = zoom + 0.01; q1 = time; q5 = q5 + 1; count = count + 1;\nper_pixel_1=rot = rot + 0.1*rad;\nwavecode_0_enabled=1\nwavecode_0_samples=100\nwave_0_per_point1=x = sample; y = 0.5 + value1*0.1;\nwavecode_1_enabled=1\nwavecode_1_samples=50\nwave_1_per_point1=acc = acc + 0.01; x = acc; y = 0.5;\nshapecode_0_enabled=1\nshapecode_0_num_inst=3\nshape_0_per_frame1=x = 0.2 + instance*0.3; rad = 0.05;\n";

    fn run(e: &mut Engine, time: f64) -> FrameParams {
        let audio = AudioFrame::default();
        let input = FrameInput { time, fps: 60.0, frame: 0, progress: 0.0, audio: &audio, width: 1280.0, height: 720.0, mesh: (48, 36) };
        let mut params = FrameParams::default();
        e.frame(&input, &mut params);
        params
    }

    #[test]
    fn a_frame_follows_milkdrop_reset_rules() {
        let preset = Arc::new(Preset::parse(PRESET.as_bytes()));
        let mut e = Engine::new(preset, 1);
        assert!(e.report.errors.is_empty(), "{:?}", e.report.errors);
        let p1 = run(&mut e, 1.0);
        let p2 = run(&mut e, 2.0);
        // Built-ins restart from the preset every frame.
        assert!((p1.motion.zoom - 1.03).abs() < 1e-5 && (p2.motion.zoom - 1.03).abs() < 1e-5);
        // q values restart from what per_frame_init left.
        assert_eq!(p1.q[4], 4.0);
        assert_eq!(p2.q[4], 4.0);
        assert_eq!(p2.q[0], 2.0);
        // User variables persist.
        assert_eq!(e.frame_ctx.get("count"), 2.0);
        assert_eq!(p2.decay, 0.9);
    }

    #[test]
    fn waves_split_between_gpu_and_cpu() {
        let preset = Arc::new(Preset::parse(PRESET.as_bytes()));
        let mut e = Engine::new(preset, 1);
        assert!(e.gpu_wave(0).is_some());
        assert!(e.gpu_wave(1).is_none());
        assert_eq!(e.report.waves_cpu.len(), 1);
        let p = run(&mut e, 1.0);
        assert_eq!(p.gpu_waves.len(), 1);
        assert_eq!(p.gpu_waves[0].inputs.len(), 100);
        // The CPU wave accumulates across points, as MilkDrop does.
        let cpu = p.lines.iter().find(|l| l.points.len() == 50).expect("cpu wave");
        assert!((cpu.points[49][0] - 0.5).abs() < 1e-5);
        assert_eq!(p.shapes.len(), 3);
        assert!((p.shapes[2].x - 0.8).abs() < 1e-5);
        // Per-vertex code runs on the GPU for this preset.
        assert!(e.gpu_vertex().is_some() && p.cpu_mesh.is_none());
    }

    #[test]
    fn cpu_vertex_fallback_fills_the_mesh() {
        let preset = Arc::new(Preset::parse(b"per_pixel_1=n = n + 1; zoom = zoom + n*0.00001;\n"));
        let mut e = Engine::new(preset, 1);
        assert!(e.report.vertex_cpu.is_some());
        let p = run(&mut e, 0.5);
        let mesh = p.cpu_mesh.expect("mesh");
        assert_eq!(mesh.len(), 49 * 37);
        // The centre vertex maps to the centre with no motion.
        let c = mesh[18 * 49 + 24];
        assert!((c[0] - 0.5).abs() < 0.01 && (c[1] - 0.5).abs() < 0.01, "{c:?}");
    }

    #[test]
    fn warp_uv_identity_and_zoom() {
        let p = FrameParams { warp_scale_inv: 1.0, ..Default::default() };
        let m = Motion { zoom: 1.0, zoomexp: 1.0, cx: 0.5, cy: 0.5, sx: 1.0, sy: 1.0, ..Default::default() };
        let (ax, ay) = aspect(1280.0, 720.0);
        let uv = warp_uv([1.0, 1.0], 1.0, &m, &p, ax, ay);
        assert!((uv[0] - 1.0).abs() < 1e-5 && uv[1].abs() < 1e-5, "{uv:?}");
        let zoomed = warp_uv([1.0, 1.0], 1.0, &Motion { zoom: 2.0, ..m }, &p, ax, ay);
        assert!((zoomed[0] - 0.75).abs() < 1e-5, "{zoomed:?}");
    }
}
