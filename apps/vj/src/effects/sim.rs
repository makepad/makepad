//! SIM FIELDS — the float simulation-texture primitive.
//!
//! A **sim field** is a named, ping-ponged pair of float render targets
//! (`TextureFormat::RenderRGBAf32` + `color_format: @Rgba32F` pipelines —
//! the four-channel sibling of the gpu_lightmap `Rf32` depth-scratch path)
//! updated once per frame by an update shader: previous state + beat/signal
//! uniforms in, next state out. Any engine's vertex or pixel shader can
//! sample a field by name (`sample_nearest(uv, 0.0)` — the vertex-legal
//! fetch, and on Apple GPUs 32-bit float textures are NEVER filtered, so
//! smooth consumers do their own 4-tap bilinear).
//!
//! This is what unlocks STATE on the GPU: living wind that gusts travel
//! across, particles with real velocities and histories, stable-fluids dye.
//!
//! # Document contract (the `fields:` list)
//!
//! ```text
//! fields: [
//!     {
//!         name: "wind"          // REQUIRED — the handle consumers use
//!         kind: "wind"          // wind | particles | fluid
//!         res: 64               // texels per side (fluid: the grid)
//!         format: "rgba32f"     // the only shipped format (16f documented
//!                               // platform-side, not exposed here yet)
//!         // kind parameters — every one ACCEPTS A NUMBER OR A BINDING
//!         // STRING, evaluated per frame like stage params:
//!         //   wind:      flow gust travel decay scale + extent (const,
//!         //              world half-extent the field spans)
//!         //   particles: curl attract swirl gravity impulse drag
//!         //              max_speed life bound + spawn ("volume"|"shell")
//!         //   fluid:     force dissipation swirl splat dye_fade radius
//!         //              vortex + iters (const, jacobi budget 4..40)
//!         // optional update-shader SUBCLASS (same law as `shader:` — a
//!         // subclass of the kind's registered base, never a plain object):
//!         update: draw.DrawVjFxSimWind {
//!             sim: fn(uv: vec2, prev: vec4) -> vec4 { ... }
//!         }
//!     }
//! ]
//! ```
//!
//! Consumers:
//! * mesh engines (grass/lsystem/metaballs): `wind_field: "wind"` swaps the
//!   analytic sway for the field (the `DrawVjFxMeshField` shader).
//! * `engine: "simswarm"`: draws its particles FROM a particles field
//!   (state texel = particle; the draw shader fetches by instance id).
//! * `engine: "fluid"`: renders a fluid field's dye gorgeously.
//! * ANY texture-input effect: `input0: "field:<name>"` binds a field's
//!   current texture as input 0 (heightmap displaced by dye, image
//!   particles carrying dye colors…). A fluid field named X also exposes
//!   `X.flow` (velocity+pressure).
//!
//! # State layouts
//! * wind (res × res): texel = (wind.x, wind.z, gust energy, ψ-spare).
//!   Wind xy is the BASE curl field; gust is beat-injected energy that
//!   ADVECTS DOWNWIND each frame — that is why a gust visibly travels.
//! * particles (2·res × res): LEFT half texel = (pos.xyz, age); RIGHT half
//!   texel = (vel.xyz, seed/cycle marker). One update pass writes both
//!   halves (each output texel reads its pos+vel pair).
//! * fluid (res × res, TWO pairs): "flow" pair texel = (vel.x, vel.y,
//!   pressure, divergence) in uv/sec; "dye" pair texel = (r, g, b, amount).
//!   Frame = advect → divergence → jacobi×iters → project → dye advect,
//!   splats injected on the beat (position hashed per beat index).
//!
//! All frame work is GPU passes; the CPU cost is uniform writes + pass
//! encodes (measured in `last_ms`). Frame path is panic-free: every doc
//! value is clamped, dt is clamped, and a missing field name simply
//! resolves to no texture (consumers fall back).

use super::doc::Reader;
use super::expr::{Animatable, Signals};
use makepad_widgets::*;

/// Kind parameter slot counts (documented order — see module docs).
const WIND_PARS: usize = 5;
const SWARM_PARS: usize = 9;
const FLUID_PARS: usize = 7;

#[derive(Clone, Copy, PartialEq)]
pub enum SimKind {
    Wind,
    Swarm,
    Fluid,
}

/// A parsed field declaration (everything clamped/forgiving at parse).
#[derive(Clone)]
pub struct SimFieldCfg {
    pub name: String,
    pub kind: SimKind,
    pub res: usize,
    /// Wind: world half-extent the field maps ([-extent, extent] → uv 0..1).
    pub extent: f32,
    /// Fluid: jacobi iteration budget.
    pub iters: usize,
    /// Particles: spawn on a shell instead of in the volume.
    pub spawn_shell: bool,
    /// Kind-specific animatable parameters (slot order in module docs).
    pub par: Vec<Animatable>,
    /// Optional update-shader subclass from the document.
    pub update_hook: Option<ScriptObjectRef>,
    pub seed: f32,
}

impl SimFieldCfg {
    pub fn wind(name: &str) -> Self {
        let cfg = Self {
            name: name.to_string(),
            kind: SimKind::Wind,
            res: 64,
            extent: 12.0,
            iters: 0,
            spawn_shell: false,
            par: vec![
                Animatable::Const(0.8),  // flow
                Animatable::Const(1.6),  // gust
                Animatable::Const(0.22), // travel
                Animatable::Const(0.55), // decay
                Animatable::Const(3.0),  // scale
            ],
            update_hook: None,
            seed: 1.0,
        };
        debug_assert_eq!(cfg.par.len(), WIND_PARS);
        cfg
    }

    pub fn swarm(name: &str, res: usize) -> Self {
        Self {
            name: name.to_string(),
            kind: SimKind::Swarm,
            res: res.clamp(16, 160),
            extent: 6.0,
            iters: 0,
            spawn_shell: false,
            par: vec![
                Animatable::Const(1.0),  // curl
                Animatable::Const(0.0),  // attract
                Animatable::Const(0.0),  // swirl
                Animatable::Const(0.0),  // gravity
                Animatable::Const(2.0),  // impulse
                Animatable::Const(0.35), // drag
                Animatable::Const(9.0),  // speed
                Animatable::Const(7.0),  // life
                Animatable::Const(6.0),  // bound
            ],
            update_hook: None,
            seed: 1.0,
        }
    }

    pub fn fluid(name: &str, res: usize) -> Self {
        let cfg = Self {
            name: name.to_string(),
            kind: SimKind::Fluid,
            res: res.clamp(32, 256),
            extent: 1.0,
            iters: 20,
            spawn_shell: false,
            par: vec![
                Animatable::Const(2.2),   // force
                Animatable::Const(0.12),  // dissipation
                Animatable::Const(0.35),  // swirl
                Animatable::Const(1.4),   // splat
                Animatable::Const(0.28),  // dye_fade
                Animatable::Const(0.085), // radius
                Animatable::Const(2.4),   // vortex (confinement strength)
            ],
            update_hook: None,
            seed: 1.0,
        };
        debug_assert_eq!(cfg.par.len(), FLUID_PARS);
        cfg
    }
}

/// Read the document's `fields:` list (forgiving — bad rows warn + skip).
pub fn parse_fields(r: &mut Reader) -> Vec<SimFieldCfg> {
    let objs = r.object_list(live_id!(fields));
    let mut out: Vec<SimFieldCfg> = Vec::new();
    for obj in objs {
        if out.len() >= 4 {
            r.warnings.push("more than 4 fields — extras dropped".to_string());
            break;
        }
        let outer = r.obj;
        r.obj = obj;
        let name = r.string(live_id!(name)).unwrap_or_else(|| format!("field{}", out.len()));
        let kind = r.string(live_id!(kind)).unwrap_or_default();
        let mut cfg = match kind.as_str() {
            "wind" => SimFieldCfg::wind(&name),
            "particles" | "swarm" => SimFieldCfg::swarm(&name, 96),
            "fluid" => SimFieldCfg::fluid(&name, 128),
            other => {
                r.warnings
                    .push(format!("field kind '{other}' unknown (wind/particles/fluid) — skipped"));
                r.obj = outer;
                continue;
            }
        };
        read_field_keys(r, &mut cfg);
        r.obj = outer;
        if out.iter().any(|f| f.name == cfg.name) {
            r.warnings.push(format!("duplicate field name '{}' — skipped", cfg.name));
            continue;
        }
        out.push(cfg);
    }
    out
}

/// Read one field object's keys onto `cfg` (r.obj must be the field object).
/// Also used by the engine synthesizers, where r.obj is the DOCUMENT — the
/// same key names configure the auto-created field.
pub fn read_field_keys(r: &mut Reader, cfg: &mut SimFieldCfg) {
    match cfg.kind {
        SimKind::Wind => {
            cfg.res = r.usize(live_id!(res), cfg.res).clamp(8, 256);
            cfg.extent = r.f32(live_id!(extent), cfg.extent).clamp(1.0, 200.0);
            let keys = [
                live_id!(flow),
                live_id!(gust),
                live_id!(travel),
                live_id!(decay),
                live_id!(scale),
            ];
            for (i, key) in keys.iter().enumerate() {
                let d = anim_default(&cfg.par[i]);
                cfg.par[i] = r.anim(*key, d);
            }
        }
        SimKind::Swarm => {
            cfg.res = r.usize(live_id!(res), cfg.res).clamp(16, 160);
            cfg.spawn_shell = matches!(r.string(live_id!(spawn)).as_deref(), Some("shell"));
            let keys = [
                live_id!(curl),
                live_id!(attract),
                live_id!(swirl),
                live_id!(gravity),
                live_id!(impulse),
                live_id!(drag),
                live_id!(max_speed),
                live_id!(life),
                live_id!(bound),
            ];
            for (i, key) in keys.iter().enumerate() {
                let d = anim_default(&cfg.par[i]);
                cfg.par[i] = r.anim(*key, d);
            }
        }
        SimKind::Fluid => {
            cfg.res = r.usize(live_id!(res), cfg.res).clamp(32, 256);
            cfg.iters = r.usize(live_id!(iters), cfg.iters).clamp(4, 40);
            let keys = [
                live_id!(force),
                live_id!(dissipation),
                live_id!(swirl),
                live_id!(splat),
                live_id!(dye_fade),
                live_id!(radius),
                live_id!(vortex),
            ];
            for (i, key) in keys.iter().enumerate() {
                let d = anim_default(&cfg.par[i]);
                cfg.par[i] = r.anim(*key, d);
            }
        }
    }
    cfg.seed = r.f32(live_id!(seed), cfg.seed).max(0.0);
    // Optional update-shader subclass: must subclass the kind's base (same
    // law as `shader:` — an inherited `pixel` proves the chain).
    let v = r.value(live_id!(update));
    if let Some(obj) = v.as_object() {
        let has_pixel = !r.vm.bx.heap.value(obj, live_id!(pixel).into(), NoTrap).is_err();
        if has_pixel {
            cfg.update_hook = Some(r.vm.bx.heap.new_object_ref(obj));
        } else {
            r.warnings.push(format!(
                "field '{}': update must subclass the kind's update shader \
                 (draw.DrawVjFxSimWind {{ sim: fn()... }}) — ignored",
                cfg.name
            ));
        }
    }
}

fn anim_default(a: &Animatable) -> f32 {
    match a {
        Animatable::Const(v) => *v,
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Runtime: fixed-resolution float passes, ping-ponged.
// ---------------------------------------------------------------------------

struct SimPass {
    pass: DrawPass,
    draw_list: DrawList2d,
    texture: Texture,
}

impl SimPass {
    fn new_bgra(cx: &mut Cx, name: &str) -> Self {
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true },
        );
        let pass = DrawPass::new_with_name(cx, name);
        let draw_list = DrawList2d::new(cx);
        pass.set_color_texture(
            cx,
            &texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
        );
        Self { pass, draw_list, texture }
    }
}

/// One per-frame pass slot. A `DrawPass` begun twice in one frame executes
/// only its LAST contents — so every step a field runs per frame owns its
/// OWN pass object, and the ping-ponged STATE TEXTURES are re-attached to
/// the step's pass each frame (the multi-jacobi lesson: never re-begin).
struct StepPass {
    pass: DrawPass,
    draw_list: DrawList2d,
}

impl StepPass {
    fn new(cx: &mut Cx, name: &str) -> Self {
        Self { pass: DrawPass::new_with_name(cx, name), draw_list: DrawList2d::new(cx) }
    }

    /// One fullscreen pass into `color` at `size` logical units, dpi 1.0 —
    /// exact texel resolution for Fixed textures (the gpu_lightmap idiom).
    fn run(
        &mut self,
        cx: &mut Cx2d,
        size: DVec2,
        color: &Texture,
        draw: &mut dyn FnMut(&mut Cx2d, Rect),
    ) {
        self.pass.set_size(cx, size);
        self.pass.set_color_texture(
            cx.cx,
            color,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
        );
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, Some(1.0));
        self.draw_list.begin_always(cx);
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());
        draw(cx, Rect { pos: dvec2(0.0, 0.0), size: pass_size });
        cx.end_pass_sized_turtle();
        self.draw_list.end(cx);
        cx.end_pass(&self.pass);
    }
}

/// The ping-ponged float state pair (textures only — passes live in the
/// per-frame step pool).
struct PingPongTex {
    tex: [Texture; 2],
    front: usize,
}

impl PingPongTex {
    fn new(cx: &mut Cx, w: usize, h: usize) -> Self {
        let mk = |cx: &mut Cx| {
            Texture::new_with_format(
                cx,
                TextureFormat::RenderRGBAf32 {
                    size: TextureSize::Fixed { width: w, height: h },
                    initial: true,
                },
            )
        };
        Self { tex: [mk(cx), mk(cx)], front: 0 }
    }

    /// The texture holding the CURRENT state (last written).
    fn front(&self) -> Texture {
        self.tex[self.front].clone()
    }

    /// The texture the next step writes.
    fn back(&self) -> Texture {
        self.tex[1 - self.front].clone()
    }

    fn flip(&mut self) {
        self.front = 1 - self.front;
    }
}

struct SimFieldRt {
    cfg: SimFieldCfg,
    state: PingPongTex,
    /// Fluid only: the dye pair (`state` is the flow field).
    dye: Option<PingPongTex>,
    /// One pass object per step this field runs per frame.
    steps: Vec<StepPass>,
    /// Fluid only: the pretty pass output (lazy, display-sized BGRA).
    view: Option<SimPass>,
}

/// The per-widget sim runtime: owns every declared field's targets and runs
/// the update passes each frame BEFORE the scene pass consumes them.
#[derive(Default)]
pub struct SimFields {
    fields: Vec<SimFieldRt>,
    last_ms: f32,
}

/// The sim update draw shaders, borrowed from the widget for one frame.
pub struct SimDraws<'a> {
    pub wind: &'a mut DrawVjFxSimWind,
    pub swarm: &'a mut DrawVjFxSimSwarm,
    pub fluid_advect: &'a mut DrawVjFxFluidAdvect,
    pub fluid_div: &'a mut DrawVjFxFluidDiv,
    pub fluid_jacobi: &'a mut DrawVjFxFluidJacobi,
    pub fluid_project: &'a mut DrawVjFxFluidProject,
    pub fluid_dye: &'a mut DrawVjFxFluidDye,
}

fn hash1(x: f32) -> f32 {
    ((x * 12.9898).sin() * 43758.5453).fract().abs()
}

impl SimFields {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// CPU encode cost of the last `update` (uniform writes + pass encodes).
    pub fn last_ms(&self) -> f32 {
        self.last_ms
    }

    /// (Re)build the runtime for a document's field list. Every pass a
    /// field runs per frame gets its OWN pass object (see [`StepPass`]).
    pub fn configure(&mut self, cx: &mut Cx, cfgs: &[SimFieldCfg]) {
        self.fields.clear();
        for cfg in cfgs {
            let n = &cfg.name;
            let (state, dye, step_count) = match cfg.kind {
                SimKind::Wind => (PingPongTex::new(cx, cfg.res, cfg.res), None, 1),
                // Particle state is double-wide: left half pos, right vel.
                SimKind::Swarm => (PingPongTex::new(cx, cfg.res * 2, cfg.res), None, 1),
                // advect + divergence + jacobi*iters + project + dye.
                SimKind::Fluid => (
                    PingPongTex::new(cx, cfg.res, cfg.res),
                    Some(PingPongTex::new(cx, cfg.res, cfg.res)),
                    cfg.iters + 4,
                ),
            };
            let steps = (0..step_count)
                .map(|k| StepPass::new(cx, &format!("vjsim_{n}_{k}")))
                .collect();
            self.fields.push(SimFieldRt { cfg: cfg.clone(), state, dye, steps, view: None });
        }
    }

    /// A field's CURRENT consumable texture by name. Fluid fields resolve to
    /// their dye; `<name>.flow` resolves to the velocity/pressure field.
    pub fn texture(&self, name: &str) -> Option<Texture> {
        let (base, flow) = match name.strip_suffix(".flow") {
            Some(b) => (b, true),
            None => (name, false),
        };
        let f = self.fields.iter().find(|f| f.cfg.name == base)?;
        match (&f.dye, flow) {
            (Some(dye), false) => Some(dye.front()),
            _ => Some(f.state.front()),
        }
    }

    /// The wind mapping uniform for a mesh consumer:
    /// (world→uv scale, 1.0, gust gain, 0). None when the field is missing.
    pub fn wind_uniform(&self, name: &str) -> Option<[f32; 4]> {
        let f = self.fields.iter().find(|f| f.cfg.name == name && f.cfg.kind == SimKind::Wind)?;
        Some([0.5 / f.cfg.extent.max(0.5), 1.0, 1.0, 0.0])
    }

    /// Run every field's update passes for this frame. Call inside the
    /// widget's draw, BEFORE the scene pass that samples the fields.
    pub fn update(
        &mut self,
        cx: &mut Cx2d,
        sig: &Signals,
        palette: &[Vec4f; 4],
        draws: &mut SimDraws,
    ) {
        let t0 = std::time::Instant::now();
        let dt = sig.0[Signals::DT].clamp(0.0, 1.0 / 30.0);
        let time = sig.0[Signals::TIME];
        let beat = sig.0[Signals::BEAT];
        let pulse = sig.0[Signals::PULSE];
        let state = [dt, time, beat, pulse];
        for f in &mut self.fields {
            let res = f.cfg.res as f32;
            let mut p = [0.0f32; SWARM_PARS];
            for (i, a) in f.cfg.par.iter().enumerate().take(SWARM_PARS) {
                p[i] = a.value(sig);
                if !p[i].is_finite() {
                    p[i] = 0.0;
                }
            }
            // Per-beat impulse anchor, deterministic per beat index.
            let bi = beat.floor();
            let sd = f.cfg.seed;
            let (ox, oy) = (
                0.2 + 0.6 * hash1(bi * 1.7 + sd * 13.1),
                0.2 + 0.6 * hash1(bi * 2.3 + sd * 7.7),
            );
            let state_pp = &mut f.state;
            let dye_pp = &mut f.dye;
            let mut steps = f.steps.iter_mut();
            match f.cfg.kind {
                SimKind::Wind => {
                    let size = dvec2(res as f64, res as f64);
                    let d = &mut *draws.wind;
                    d.draw_vars.set_texture(0, &state_pp.front());
                    d.draw_vars.set_uniform(cx, live_id!(u_state), &state);
                    d.draw_vars
                        .set_uniform(cx, live_id!(u_grid), &[res, res, 1.0 / res, 1.0 / res]);
                    d.draw_vars.set_uniform(cx, live_id!(u_par), &[p[0], p[1], p[2], p[3]]);
                    d.draw_vars.set_uniform(cx, live_id!(u_par2), &[p[4], 0.0, 0.0, 0.0]);
                    d.draw_vars.set_uniform(cx, live_id!(u_aux), &[ox, oy, bi, sd]);
                    if let Some(sp) = steps.next() {
                        sp.run(cx, size, &state_pp.back(), &mut |cx, rect| d.draw_abs(cx, rect));
                        state_pp.flip();
                    }
                }
                SimKind::Swarm => {
                    let (w, h) = (res * 2.0, res);
                    let size = dvec2(w as f64, h as f64);
                    let d = &mut *draws.swarm;
                    d.draw_vars.set_texture(0, &state_pp.front());
                    d.draw_vars.set_uniform(cx, live_id!(u_state), &state);
                    d.draw_vars.set_uniform(cx, live_id!(u_grid), &[w, h, 1.0 / w, 1.0 / h]);
                    // (curl, attract, swirl, gravity)
                    d.draw_vars.set_uniform(cx, live_id!(u_par), &[p[0], p[1], p[2], p[3]]);
                    // (impulse, drag, speed, life)
                    d.draw_vars.set_uniform(
                        cx,
                        live_id!(u_par2),
                        &[p[4], p[5].max(0.0), p[6].max(0.5), p[7].clamp(0.5, 60.0)],
                    );
                    d.draw_vars.set_uniform(
                        cx,
                        live_id!(u_aux),
                        &[
                            if f.cfg.spawn_shell { 1.0 } else { 0.0 },
                            p[8].clamp(0.5, 100.0),
                            bi,
                            sd,
                        ],
                    );
                    if let Some(sp) = steps.next() {
                        sp.run(cx, size, &state_pp.back(), &mut |cx, rect| d.draw_abs(cx, rect));
                        state_pp.flip();
                    }
                }
                SimKind::Fluid => {
                    let size = dvec2(res as f64, res as f64);
                    let texel = [res, res, 1.0 / res, 1.0 / res];
                    // Splat direction: hashed per beat index; the anchor
                    // DRIFTS along it within the beat, so each injection
                    // paints a brush stroke instead of inflating one dot.
                    let ang = hash1(bi * 3.1 + sd) * std::f32::consts::TAU;
                    let (dx, dy) = (ang.cos(), ang.sin());
                    let ph = (beat - bi).clamp(0.0, 1.0);
                    let (ox, oy) = (ox + dx * ph * 0.12, oy + dy * ph * 0.12);
                    // 1. advect velocity + beat force splat + ambient swirl.
                    {
                        let d = &mut *draws.fluid_advect;
                        d.draw_vars.set_texture(0, &state_pp.front());
                        d.draw_vars.set_uniform(cx, live_id!(u_state), &state);
                        d.draw_vars.set_uniform(cx, live_id!(u_grid), &texel);
                        // (force, dissipation, swirl, radius)
                        d.draw_vars
                            .set_uniform(cx, live_id!(u_par), &[p[0], p[1], p[2], p[5]]);
                        // (vortex confinement strength, ...)
                        d.draw_vars.set_uniform(
                            cx,
                            live_id!(u_par2),
                            &[p[6].clamp(0.0, 12.0), 0.0, 0.0, 0.0],
                        );
                        d.draw_vars.set_uniform(cx, live_id!(u_aux), &[ox, oy, dx, dy]);
                        if let Some(sp) = steps.next() {
                            sp.run(cx, size, &state_pp.back(), &mut |cx, rect| {
                                d.draw_abs(cx, rect)
                            });
                            state_pp.flip();
                        }
                    }
                    // 2. divergence into .w.
                    {
                        let d = &mut *draws.fluid_div;
                        d.draw_vars.set_texture(0, &state_pp.front());
                        d.draw_vars.set_uniform(cx, live_id!(u_grid), &texel);
                        if let Some(sp) = steps.next() {
                            sp.run(cx, size, &state_pp.back(), &mut |cx, rect| {
                                d.draw_abs(cx, rect)
                            });
                            state_pp.flip();
                        }
                    }
                    // 3. jacobi pressure relaxation, budgeted.
                    for _ in 0..f.cfg.iters {
                        let d = &mut *draws.fluid_jacobi;
                        d.draw_vars.set_texture(0, &state_pp.front());
                        d.draw_vars.set_uniform(cx, live_id!(u_grid), &texel);
                        if let Some(sp) = steps.next() {
                            sp.run(cx, size, &state_pp.back(), &mut |cx, rect| {
                                d.draw_abs(cx, rect)
                            });
                            state_pp.flip();
                        }
                    }
                    // 4. project: subtract the pressure gradient.
                    {
                        let d = &mut *draws.fluid_project;
                        d.draw_vars.set_texture(0, &state_pp.front());
                        d.draw_vars.set_uniform(cx, live_id!(u_grid), &texel);
                        if let Some(sp) = steps.next() {
                            sp.run(cx, size, &state_pp.back(), &mut |cx, rect| {
                                d.draw_abs(cx, rect)
                            });
                            state_pp.flip();
                        }
                    }
                    // 5. dye: advect by the projected velocity + beat splat.
                    if let Some(dye) = dye_pp {
                        let col = palette[1 + (bi.max(0.0) as usize) % 3];
                        let d = &mut *draws.fluid_dye;
                        d.draw_vars.set_texture(0, &dye.front());
                        d.draw_vars.set_texture(1, &state_pp.front());
                        d.draw_vars.set_uniform(cx, live_id!(u_state), &state);
                        d.draw_vars.set_uniform(cx, live_id!(u_grid), &texel);
                        // (splat, dye_fade, radius, 0)
                        d.draw_vars
                            .set_uniform(cx, live_id!(u_par), &[p[3], p[4], p[5], 0.0]);
                        d.draw_vars.set_uniform(cx, live_id!(u_aux), &[ox, oy, dx, dy]);
                        d.draw_vars
                            .set_uniform(cx, live_id!(u_dye_col), &[col.x, col.y, col.z, 1.0]);
                        if let Some(sp) = steps.next() {
                            sp.run(cx, size, &dye.back(), &mut |cx, rect| d.draw_abs(cx, rect));
                            dye.flip();
                        }
                    }
                }
            }
        }
        self.last_ms = t0.elapsed().as_secs_f32() * 1000.0;
    }

    /// Render a fluid field's pretty view (dye through the palette, over an
    /// optional warped input) at `size`, returning the output texture. The
    /// fluid engine's whole scene pass.
    #[allow(clippy::too_many_arguments)]
    pub fn fluid_view(
        &mut self,
        cx: &mut Cx2d,
        name: &str,
        size: DVec2,
        draw: &mut DrawVjFxFluidView,
        input0: &Texture,
        view_par: [f32; 4],
        palette: &[Vec4f; 4],
    ) -> Option<Texture> {
        let f = self
            .fields
            .iter_mut()
            .find(|f| f.cfg.name == name && f.cfg.kind == SimKind::Fluid)?;
        let dye = f.dye.as_ref()?.front();
        let flow = f.state.front();
        let res = f.cfg.res as f32;
        let view = f
            .view
            .get_or_insert_with(|| SimPass::new_bgra(cx.cx, &format!("vjsim_{name}_view")));
        draw.draw_vars.set_texture(0, &dye);
        draw.draw_vars.set_texture(1, &flow);
        draw.draw_vars.set_texture(2, input0);
        draw.draw_vars
            .set_uniform(cx, live_id!(u_grid), &[res, res, 1.0 / res, 1.0 / res]);
        draw.draw_vars.set_uniform(cx, live_id!(u_view), &view_par);
        let pal = [palette[0], palette[1], palette[2], palette[3]];
        draw.draw_vars
            .set_uniform(cx, live_id!(u_bg), &[pal[0].x, pal[0].y, pal[0].z, 1.0]);
        // Display pass runs at widget dpi like a post stage.
        let dpi = cx.current_dpi_factor();
        view.pass.set_size(cx, size);
        cx.make_child_pass(&view.pass);
        cx.begin_pass(&view.pass, Some(dpi));
        view.draw_list.begin_always(cx);
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());
        draw.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: pass_size });
        cx.end_pass_sized_turtle();
        view.draw_list.end(cx);
        cx.end_pass(&view.pass);
        Some(view.texture.clone())
    }
}

// ---------------------------------------------------------------------------
// The sim shaders. Update shaders render 1 fullscreen quad into an
// RGBA32F target (`color_format: @Rgba32F` — the pipeline format this
// build added platform-side, mirroring the gpu_lightmap Rf32 path).
// Float textures are ALWAYS read with sample_nearest; smooth reads do a
// manual 4-tap bilinear (Apple GPUs never filter 32-bit floats).
// ---------------------------------------------------------------------------

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // -----------------------------------------------------------------
    // WIND field update. State: (wind.x, wind.z, gust, spare).
    // u_par = (flow, gust, travel, decay); u_par2 = (scale, 0, 0, 0);
    // u_aux = (origin.x, origin.y, beat index, seed);
    // u_state = (dt, time, beat, pulse); u_grid = (w, h, 1/w, 1/h).
    // The base wind is an ANALYTIC curl (divergence-free) of a sum-of-
    // sines potential; the gust channel is beat-injected energy ADVECTED
    // DOWNWIND every frame — a gust is a traveling object, not a flicker.
    // Document hook: `sim` (the whole state function).
    // -----------------------------------------------------------------
    mod.draw.DrawVjFxSimWind = set_type_default() do #(DrawVjFxSimWind::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        tex_prev: texture_2d(float)
        u_state: uniform(vec4(0.016, 0.0, 0.0, 0.0))
        u_grid: uniform(vec4(64.0, 64.0, 0.015625, 0.015625))
        u_par: uniform(vec4(0.8, 1.6, 0.22, 0.55))
        u_par2: uniform(vec4(3.0, 0.0, 0.0, 0.0))
        u_aux: uniform(vec4(0.5, 0.5, 0.0, 1.0))

        bil: fn(uv: vec2) -> vec4 {
            let g = self.u_grid
            let p = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)) * vec2(g.x, g.y)
                - vec2(0.5, 0.5)
            let i = floor(p)
            let f = p - i
            let c = (i + vec2(0.5, 0.5)) * vec2(g.z, g.w)
            let s00 = self.tex_prev.sample_nearest(c, 0.0)
            let s10 = self.tex_prev.sample_nearest(c + vec2(g.z, 0.0), 0.0)
            let s01 = self.tex_prev.sample_nearest(c + vec2(0.0, g.w), 0.0)
            let s11 = self.tex_prev.sample_nearest(c + vec2(g.z, g.w), 0.0)
            return mix(mix(s00, s10, f.x), mix(s01, s11, f.x), f.y)
        }

        sim: fn(uv: vec2, prev: vec4) -> vec4 {
            let t = self.u_state.y * 0.31
            let p = uv * self.u_par2.x
            // Analytic curl of psi = sum of two sine products: wind =
            // (d psi/dy, -d psi/dx) — smooth, loops, divergence-free.
            let a1 = p.x * 1.7 + t
            let b1 = p.y * 1.3 - t * 0.7
            let a2 = p.x * 2.9 - t * 1.3 + 4.0
            let b2 = p.y * 2.3 + t
            let dpy = (0.0 - 1.3) * sin(a1) * sin(b1) - 1.15 * sin(a2) * sin(b2)
            let dpx = 1.7 * cos(a1) * cos(b1) + 1.45 * cos(a2) * cos(b2)
            let wind = vec2(dpy, 0.0 - dpx) * self.u_par.x * 0.5
            // Gust: fetch upwind (bilinear), decay, inject on the beat.
            let dir = wind / max(length(wind), 0.0001)
            let upwind = uv - dir * (self.u_par.z * self.u_state.x)
            let mut g = self.bil(upwind).z * exp(0.0 - self.u_par.w * self.u_state.x)
            let d = uv - self.u_aux.xy
            let inject = self.u_state.w * self.u_state.w * self.u_state.w
            g = g + inject * self.u_par.y * self.u_state.x
                * 3.0 * exp(0.0 - dot(d, d) * 42.0)
            g = clamp(g, 0.0, 4.0)
            return vec4(wind.x, wind.y, g, 0.0)
        }

        pixel: fn() {
            let uv = self.pos
            let prev = self.tex_prev.sample_nearest(uv, 0.0)
            return self.sim(uv, prev)
        }
    }

    // -----------------------------------------------------------------
    // PARTICLE state update. Texture is 2S x S: LEFT half = (pos, age),
    // RIGHT half = (vel, seed). Each output texel reads its own pos+vel
    // pair, integrates, and writes its half.
    // u_par = (curl, attract, swirl, gravity);
    // u_par2 = (impulse, drag, speed cap, life);
    // u_aux = (spawn shell?, bound, beat index, seed).
    // Document hook: `force(p, v, id)` — the acceleration field.
    // -----------------------------------------------------------------
    mod.draw.DrawVjFxSimSwarm = set_type_default() do #(DrawVjFxSimSwarm::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        tex_prev: texture_2d(float)
        u_state: uniform(vec4(0.016, 0.0, 0.0, 0.0))
        u_grid: uniform(vec4(192.0, 96.0, 0.0052, 0.0104))
        u_par: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        u_par2: uniform(vec4(2.0, 0.35, 9.0, 7.0))
        u_aux: uniform(vec4(0.0, 6.0, 0.0, 1.0))

        hash1: fn(x: float) -> float {
            return fract(sin(x * 12.9898) * 43758.5453)
        }

        // Analytic curl of the vector potential (A, B, C) — exactly
        // divergence-free, two octaves.
        curl3: fn(p: vec3, t: float) -> vec3 {
            let k = 0.55
            let ca = cos(k * (p.y + p.z) * 0.9 + t)
            let cb = cos(k * (p.z + p.x) * 1.1 - t * 0.7)
            let cc = cos(k * (p.x + p.y) * 1.3 + t * 1.3)
            let mut c = vec3(
                1.3 * cc - 1.1 * cb,
                0.9 * ca - 1.3 * cc,
                1.1 * cb - 0.9 * ca
            )
            let k2 = 1.9
            let ca2 = cos(k2 * (p.y - p.z) * 1.0 + t * 1.7)
            let cb2 = cos(k2 * (p.z - p.x) * 0.8 - t)
            let cc2 = cos(k2 * (p.x - p.y) * 1.2 + t * 0.6)
            c = c + vec3(cc2 - cb2, ca2 - cc2, cb2 - ca2) * 0.45
            return c
        }

        spawn_pos: fn(id: float, cyc: float) -> vec3 {
            let bound = self.u_aux.y
            let h1 = self.hash1(id * 1.31 + cyc * 17.7 + self.u_aux.w)
            let h2 = self.hash1(id * 2.71 + cyc * 11.3 + self.u_aux.w * 3.0)
            let h3 = self.hash1(id * 3.17 + cyc * 7.9)
            let th = h1 * 6.2831853
            let z = h2 * 2.0 - 1.0
            let rxy = sqrt(max(1.0 - z * z, 0.0))
            let mut r = bound * (0.25 + 0.55 * h3)
            if self.u_aux.x > 0.5 {
                r = bound * (0.92 + 0.08 * h3)
            }
            return vec3(
                rxy * cos(th) * r,
                z * r * 0.75,
                rxy * sin(th) * r
            )
        }

        // Document hook: acceleration at (p, v) for particle `id`.
        force: fn(p: vec3, v: vec3, id: float) -> vec3 {
            let t = self.u_state.y * 0.4
            let mut acc = self.curl3(p, t) * self.u_par.x
            // Attractor: a target orbiting the origin + tangential swirl.
            let bound = self.u_aux.y
            let target = vec3(
                cos(t * 1.3) * bound * 0.45,
                sin(t * 0.9) * bound * 0.20,
                sin(t * 1.3) * bound * 0.45
            )
            let rel = target - p
            acc = acc + rel * self.u_par.y
            let tang = vec3(0.0 - rel.z, 0.0, rel.x)
            acc = acc + tang * self.u_par.z
            acc = acc + vec3(0.0, 0.0 - self.u_par.w, 0.0)
            // Beat impulse: outward kick, eased by the pulse envelope.
            let pl = self.u_state.w
            let away = p / max(length(p), 0.001)
            acc = acc + away * (pl * pl * pl * self.u_par2.x * 8.0)
            return acc
        }

        pixel: fn() {
            let uv = self.pos
            let half = step(0.5, uv.x)
            let pos_uv = vec2(uv.x - half * 0.5, uv.y)
            let vel_uv = vec2(pos_uv.x + 0.5, uv.y)
            let pos = self.tex_prev.sample_nearest(pos_uv, 0.0)
            let vel = self.tex_prev.sample_nearest(vel_uv, 0.0)
            let side = self.u_grid.y
            let col = floor(pos_uv.x * self.u_grid.x)
            let row = floor(uv.y * side)
            let id = row * side + col
            let dt = self.u_state.x
            let life = self.u_par2.w
            let mut p = pos.xyz
            let mut v = vel.xyz
            let mut age = pos.w
            let mut cyc = vel.w
            // Unseeded texel (cleared texture): scatter mid-life so the
            // swarm is alive on frame one.
            if cyc < 0.5 {
                cyc = 1.0
                p = self.spawn_pos(id, cyc)
                v = self.curl3(p, 0.0) * 0.4
                age = self.hash1(id * 7.7) * life
            }
            age = age + dt
            if age > life {
                cyc = cyc + 1.0
                p = self.spawn_pos(id, cyc)
                v = self.curl3(p, self.u_state.y * 0.4) * 0.4
                age = age - life
            }
            let acc = self.force(p, v, id)
            v = (v + acc * dt) * exp(0.0 - self.u_par2.y * dt)
            let sp = length(v)
            if sp > self.u_par2.z {
                v = v * (self.u_par2.z / sp)
            }
            p = p + v * dt
            // Soft bound: fold escapees back toward the volume.
            let bound = self.u_aux.y
            let r = length(p)
            if r > bound * 1.6 {
                p = p * (bound * 1.6 / r)
                v = v * 0.5
            }
            if half < 0.5 {
                return vec4(p.x, p.y, p.z, age)
            }
            return vec4(v.x, v.y, v.z, cyc)
        }
    }

    // -----------------------------------------------------------------
    // FLUID: stable fluids on a packed field (vel.x, vel.y, pressure,
    // divergence), velocities in uv/sec. Passes: advect -> divergence ->
    // jacobi xN -> project. Neighbor math in texel units (the classic
    // GPU-gems constants).
    // -----------------------------------------------------------------
    mod.draw.DrawVjFxFluidAdvect = set_type_default() do #(DrawVjFxFluidAdvect::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        tex_prev: texture_2d(float)
        u_state: uniform(vec4(0.016, 0.0, 0.0, 0.0))
        u_grid: uniform(vec4(128.0, 128.0, 0.0078125, 0.0078125))
        u_par: uniform(vec4(2.2, 0.12, 0.35, 0.085))
        u_par2: uniform(vec4(2.4, 0.0, 0.0, 0.0))
        u_aux: uniform(vec4(0.5, 0.5, 1.0, 0.0))

        bil: fn(uv: vec2) -> vec4 {
            let g = self.u_grid
            let p = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)) * vec2(g.x, g.y)
                - vec2(0.5, 0.5)
            let i = floor(p)
            let f = p - i
            let c = (i + vec2(0.5, 0.5)) * vec2(g.z, g.w)
            let s00 = self.tex_prev.sample_nearest(c, 0.0)
            let s10 = self.tex_prev.sample_nearest(c + vec2(g.z, 0.0), 0.0)
            let s01 = self.tex_prev.sample_nearest(c + vec2(0.0, g.w), 0.0)
            let s11 = self.tex_prev.sample_nearest(c + vec2(g.z, g.w), 0.0)
            return mix(mix(s00, s10, f.x), mix(s01, s11, f.x), f.y)
        }

        // Curl of the velocity at uv (central differences, texel units).
        vort: fn(uv: vec2) -> float {
            let g = self.u_grid
            let l = self.tex_prev.sample_nearest(uv - vec2(g.z, 0.0), 0.0)
            let r = self.tex_prev.sample_nearest(uv + vec2(g.z, 0.0), 0.0)
            let b = self.tex_prev.sample_nearest(uv - vec2(0.0, g.w), 0.0)
            let t = self.tex_prev.sample_nearest(uv + vec2(0.0, g.w), 0.0)
            return 0.5 * ((r.y - l.y) - (t.x - b.x))
        }

        pixel: fn() {
            let uv = self.pos
            let dt = self.u_state.x
            let v0 = self.tex_prev.sample_nearest(uv, 0.0)
            let adv = self.bil(uv - v0.xy * dt)
            let mut v = adv.xy * exp(0.0 - self.u_par.y * dt)
            // Vorticity confinement: push velocity toward local curl
            // maxima, reviving the eddies that semi-Lagrangian advection
            // diffuses away — THE difference between drifting blobs and
            // curling ink (u_par2.x = strength).
            let g = self.u_grid
            let wc = self.vort(uv)
            let wl = abs(self.vort(uv - vec2(g.z, 0.0)))
            let wr = abs(self.vort(uv + vec2(g.z, 0.0)))
            let wb = abs(self.vort(uv - vec2(0.0, g.w)))
            let wt = abs(self.vort(uv + vec2(0.0, g.w)))
            let grad = vec2(wr - wl, wt - wb) * 0.5
            let n = grad / max(length(grad), 0.0001)
            v = v + vec2(n.y, 0.0 - n.x) * (wc * self.u_par2.x * dt * 1.2)
            // Ambient swirl: analytic 2D curl keeps the dye alive between
            // beats (psychedelic wash).
            let t = self.u_state.y * 0.23
            let q = uv * 4.1
            let a1 = q.x * 1.5 + t
            let b1 = q.y * 1.7 - t * 0.8
            let sw = vec2(
                (0.0 - 1.7) * sin(a1) * sin(b1),
                (0.0 - 1.5) * cos(a1) * cos(b1)
            )
            v = v + sw * (self.u_par.z * dt)
            // Beat splat: a directional JET at the anchor (the radial term
            // stays small so the impulse shears the dye into filaments
            // instead of inflating a blob).
            let d = uv - self.u_aux.xy
            let r = max(self.u_par.w, 0.01)
            let gsp = exp(0.0 - dot(d, d) / (r * r))
            let pl = self.u_state.w
            let dirv = self.u_aux.zw * 1.2 + d * 0.8
            v = v + dirv * (pl * pl * self.u_par.x * dt * 42.0 * gsp)
            let spd = length(v)
            if spd > 3.0 {
                v = v * (3.0 / spd)
            }
            // Zero the border so the box behaves.
            let g = self.u_grid
            let edge = step(g.z, uv.x) * step(uv.x, 1.0 - g.z)
                * step(g.w, uv.y) * step(uv.y, 1.0 - g.w)
            v = v * edge
            return vec4(v.x, v.y, adv.z, 0.0)
        }
    }

    mod.draw.DrawVjFxFluidDiv = set_type_default() do #(DrawVjFxFluidDiv::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        tex_prev: texture_2d(float)
        u_grid: uniform(vec4(128.0, 128.0, 0.0078125, 0.0078125))

        pixel: fn() {
            let uv = self.pos
            let g = self.u_grid
            let c = self.tex_prev.sample_nearest(uv, 0.0)
            let l = self.tex_prev.sample_nearest(uv - vec2(g.z, 0.0), 0.0)
            let r = self.tex_prev.sample_nearest(uv + vec2(g.z, 0.0), 0.0)
            let b = self.tex_prev.sample_nearest(uv - vec2(0.0, g.w), 0.0)
            let t = self.tex_prev.sample_nearest(uv + vec2(0.0, g.w), 0.0)
            let div = 0.5 * ((r.x - l.x) + (t.y - b.y))
            return vec4(c.x, c.y, c.z, div)
        }
    }

    mod.draw.DrawVjFxFluidJacobi = set_type_default() do #(DrawVjFxFluidJacobi::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        tex_prev: texture_2d(float)
        u_grid: uniform(vec4(128.0, 128.0, 0.0078125, 0.0078125))

        pixel: fn() {
            let uv = self.pos
            let g = self.u_grid
            let c = self.tex_prev.sample_nearest(uv, 0.0)
            let l = self.tex_prev.sample_nearest(uv - vec2(g.z, 0.0), 0.0)
            let r = self.tex_prev.sample_nearest(uv + vec2(g.z, 0.0), 0.0)
            let b = self.tex_prev.sample_nearest(uv - vec2(0.0, g.w), 0.0)
            let t = self.tex_prev.sample_nearest(uv + vec2(0.0, g.w), 0.0)
            let pr = (l.z + r.z + b.z + t.z - c.w) * 0.25
            return vec4(c.x, c.y, pr, c.w)
        }
    }

    mod.draw.DrawVjFxFluidProject = set_type_default() do #(DrawVjFxFluidProject::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        tex_prev: texture_2d(float)
        u_grid: uniform(vec4(128.0, 128.0, 0.0078125, 0.0078125))

        pixel: fn() {
            let uv = self.pos
            let g = self.u_grid
            let c = self.tex_prev.sample_nearest(uv, 0.0)
            let l = self.tex_prev.sample_nearest(uv - vec2(g.z, 0.0), 0.0)
            let r = self.tex_prev.sample_nearest(uv + vec2(g.z, 0.0), 0.0)
            let b = self.tex_prev.sample_nearest(uv - vec2(0.0, g.w), 0.0)
            let t = self.tex_prev.sample_nearest(uv + vec2(0.0, g.w), 0.0)
            let v = c.xy - vec2(r.z - l.z, t.z - b.z) * 0.5
            return vec4(v.x, v.y, c.z, c.w)
        }
    }

    // Dye: advected by the projected velocity, fading, beat splats in the
    // frame's palette color. tex_prev = dye, tex_vel = flow.
    // u_par = (splat, dye_fade, radius, 0).
    mod.draw.DrawVjFxFluidDye = set_type_default() do #(DrawVjFxFluidDye::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        tex_prev: texture_2d(float)
        tex_vel: texture_2d(float)
        u_state: uniform(vec4(0.016, 0.0, 0.0, 0.0))
        u_grid: uniform(vec4(128.0, 128.0, 0.0078125, 0.0078125))
        u_par: uniform(vec4(1.4, 0.28, 0.085, 0.0))
        u_aux: uniform(vec4(0.5, 0.5, 1.0, 0.0))
        u_dye_col: uniform(vec4(0.3, 0.9, 1.0, 1.0))

        bil: fn(uv: vec2) -> vec4 {
            let g = self.u_grid
            let p = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)) * vec2(g.x, g.y)
                - vec2(0.5, 0.5)
            let i = floor(p)
            let f = p - i
            let c = (i + vec2(0.5, 0.5)) * vec2(g.z, g.w)
            let s00 = self.tex_prev.sample_nearest(c, 0.0)
            let s10 = self.tex_prev.sample_nearest(c + vec2(g.z, 0.0), 0.0)
            let s01 = self.tex_prev.sample_nearest(c + vec2(0.0, g.w), 0.0)
            let s11 = self.tex_prev.sample_nearest(c + vec2(g.z, g.w), 0.0)
            return mix(mix(s00, s10, f.x), mix(s01, s11, f.x), f.y)
        }

        pixel: fn() {
            let uv = self.pos
            let dt = self.u_state.x
            let vel = self.tex_vel.sample_nearest(uv, 0.0)
            let mut dye = self.bil(uv - vel.xy * dt)
            dye = dye * exp(0.0 - self.u_par.y * dt)
            // Dye splat: TIGHTER than the velocity splat (0.55x) so the jet
            // it rides visibly stretches it; injection stays modest — the
            // view pass tone-maps, whites mean real density.
            let d = uv - self.u_aux.xy
            let r = max(self.u_par.z * 0.55, 0.008)
            let gsp = exp(0.0 - dot(d, d) / (r * r))
            let pl = self.u_state.w
            let inj = pl * pl * pl * self.u_par.x * dt * 10.0 * gsp
            dye = dye + vec4(
                self.u_dye_col.x * inj,
                self.u_dye_col.y * inj,
                self.u_dye_col.z * inj,
                inj
            )
            return clamp(dye, vec4(0.0, 0.0, 0.0, 0.0), vec4(6.0, 6.0, 6.0, 6.0))
        }
    }

    // -----------------------------------------------------------------
    // FLUID VIEW: the pretty pass (BGRA, display-sized). Dye is already
    // palette-colored; this pass adds glow, velocity shimmer, an optional
    // dye-warped input0 underneath, and screens dye over the base.
    // u_view = (glow, warp, texmix, unused). Document hook: `fx_shade`.
    // -----------------------------------------------------------------
    mod.draw.DrawVjFxFluidView = set_type_default() do #(DrawVjFxFluidView::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_dye: texture_2d(float)
        tex_flow: texture_2d(float)
        tex_in: texture_2d(float)
        u_grid: uniform(vec4(128.0, 128.0, 0.0078125, 0.0078125))
        u_view: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        u_bg: uniform(vec4(0.01, 0.012, 0.03, 1.0))

        bil_dye: fn(uv: vec2) -> vec4 {
            let g = self.u_grid
            let p = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)) * vec2(g.x, g.y)
                - vec2(0.5, 0.5)
            let i = floor(p)
            let f = p - i
            let c = (i + vec2(0.5, 0.5)) * vec2(g.z, g.w)
            let s00 = self.tex_dye.sample_nearest(c, 0.0)
            let s10 = self.tex_dye.sample_nearest(c + vec2(g.z, 0.0), 0.0)
            let s01 = self.tex_dye.sample_nearest(c + vec2(0.0, g.w), 0.0)
            let s11 = self.tex_dye.sample_nearest(c + vec2(g.z, g.w), 0.0)
            return mix(mix(s00, s10, f.x), mix(s01, s11, f.x), f.y)
        }

        bil_flow: fn(uv: vec2) -> vec4 {
            let g = self.u_grid
            let p = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)) * vec2(g.x, g.y)
                - vec2(0.5, 0.5)
            let i = floor(p)
            let f = p - i
            let c = (i + vec2(0.5, 0.5)) * vec2(g.z, g.w)
            let s00 = self.tex_flow.sample_nearest(c, 0.0)
            let s10 = self.tex_flow.sample_nearest(c + vec2(g.z, 0.0), 0.0)
            let s01 = self.tex_flow.sample_nearest(c + vec2(0.0, g.w), 0.0)
            let s11 = self.tex_flow.sample_nearest(c + vec2(g.z, g.w), 0.0)
            return mix(mix(s00, s10, f.x), mix(s01, s11, f.x), f.y)
        }

        // Document hook: final shade from (dye, flow, base).
        fx_shade: fn(uv: vec2, dye: vec4, flow: vec4, base: vec3) -> vec4 {
            let glow = self.u_view.x
            let ink = vec3(dye.x, dye.y, dye.z) * glow
            let shimmer = clamp(length(flow.xy) * 0.35, 0.0, 0.6)
            let mut lit = ink + ink * shimmer
            // Soft-knee tone map: color survives density; only genuinely
            // dense ink approaches white.
            lit = lit / (vec3(1.0, 1.0, 1.0) + lit * 0.45)
            let rgb = vec3(1.0, 1.0, 1.0)
                - (vec3(1.0, 1.0, 1.0) - base)
                * clamp(vec3(1.0, 1.0, 1.0) - lit, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
            return vec4(rgb, 1.0)
        }

        pixel: fn() {
            let uv = self.pos
            let dye = self.bil_dye(uv)
            let flow = self.bil_flow(uv)
            // Optional input0 underneath, warped by the velocity field —
            // the psychedelic wash. texmix 0 = pure ink on background.
            let wuv = clamp(uv + flow.xy * self.u_view.y, vec2(0.0, 0.0), vec2(1.0, 1.0))
            let src = self.tex_in.sample_as_bgra(wuv)
            let base = mix(self.u_bg.xyz, src.xyz, clamp(self.u_view.z, 0.0, 1.0))
            return self.fx_shade(uv, dye, flow, base)
        }
    }

    // -----------------------------------------------------------------
    // SWARM DRAW: billboard quads whose VERTEX shader fetches this
    // particle's state texel by instance id (pos left half, vel right).
    // Velocity-stretched sprites; standard fx signal block; hooks:
    // fx_color(t = age01, attr = (id, speed, r0, r1)), fx_sprite.
    // shape = (state side S, size, stretch, life); flow = (bound,
    // speed color gain, 0, 0).
    // -----------------------------------------------------------------
    mod.draw.DrawVjFxSimSwarmDraw = set_type_default() do #(DrawVjFxSimSwarmDraw::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        state_tex: texture_2d(float)
        // Content coupling: state_tex owns slot 0, so input0 rides slot 1.
        tex0: texture_2d(float)
        has_content: uniform(0.0)
        backface_culling: false
        alpha_blend: true
        depth_write: false

        v_color: varying(vec4f)
        v_uv: varying(vec2f)

        fx_sprite: fn(uv: vec2, tint: vec4) -> vec4 {
            let d = length(uv - vec2(0.5, 0.5)) * 2.0
            let core = 1.0 - smoothstep(0.0, 0.45, d)
            let halo = 1.0 - smoothstep(0.12, 1.0, d)
            let a = clamp(core + halo * 0.55, 0.0, 1.0) * tint.w
            return vec4(tint.x * a, tint.y * a, tint.z * a, 0.0)
        }

        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            let sp = clamp(attr.y * self.flow.y, 0.0, 1.0)
            let base = self.col_a.mix(self.col_b, sp)
            let hot = base.xyz + self.col_c.xyz * pow(sp, 3.0) * 0.6
            let fade = smoothstep(0.0, 0.06, t) * (1.0 - smoothstep(0.75, 1.0, t))
            let boost = 0.8 + self.time_beat.w * 0.9
            return vec4(hot * boost, fade)
        }

        vertex: fn() {
            let id = self.geom.geom_id
            let side = max(self.shape.x, 1.0)
            let col = modf(id, side)
            let row = floor(id / side)
            let tw = 0.5 / side
            let th = 1.0 / side
            let pos_uv = vec2((col + 0.5) * tw, (row + 0.5) * th)
            let vel_uv = vec2(pos_uv.x + 0.5, pos_uv.y)
            let st = self.state_tex.sample_nearest(pos_uv, 0.0)
            let vl = self.state_tex.sample_nearest(vel_uv, 0.0)
            let life = max(self.shape.w, 0.5)
            let age01 = clamp(st.w / life, 0.0, 1.0)
            let speed = length(vl.xyz)
            let world = self.draw_list.view_transform * vec4(st.x, st.y, st.z, 1.0)
            let view_pos = self.draw_pass.camera_view * world
            // Velocity-aligned stretch in view space.
            let vv = (self.draw_pass.camera_view
                * (self.draw_list.view_transform * vec4(vl.x, vl.y, vl.z, 0.0))).xyz
            let vdir = (vv.xy + vec2(0.0001, 0.0)) / max(length(vv.xy), 0.0001)
            let size = self.shape.y * (0.7 + 0.6 * fract(id * 0.618))
                * (1.0 + self.time_beat.w * 0.45)
            let stretch = 1.0 + clamp(speed * self.shape.z, 0.0, 4.0)
            let corner = self.geom.geom_pos
            let along = vdir * (corner.y * size * stretch)
            let across = vec2(0.0 - vdir.y, vdir.x) * (corner.x * size)
            let billboard = vec4(
                view_pos.x + along.x + across.x,
                view_pos.y + along.y + across.y,
                view_pos.z,
                view_pos.w
            )
            let attr = vec4(id, speed, fract(id * 0.618), st.w)
            let c0 = self.fx_color(age01, attr, self.geom.geom_normal, world.xyz)
            // CONTENT COUPLING (fog.z, pre-gated): each agent glows in the
            // channel video's color at its screen position — the flock
            // sweeps the clip's palette around while its motion, sprite
            // and fade stay the classic look's. tex0 = input0 (slot 1).
            let cc = self.draw_pass.camera_projection * view_pos
            let suv = clamp(
                vec2(cc.x, 0.0 - cc.y) / max(cc.w, 0.0001) * 0.5 + vec2(0.5, 0.5),
                vec2(0.0, 0.0),
                vec2(1.0, 1.0)
            )
            let vtex = self.tex0.sample_nearest(suv, 0.0)
            self.v_color = vec4(
                c0.xyz.mix(vtex.xyz * (0.85 + 0.55 * self.time_beat.w), self.fog.z * 0.85),
                c0.w
            )
            self.v_uv = self.geom.geom_uv
            self.vertex_pos = self.draw_pass.camera_projection * billboard
            return self.vertex_pos
        }

        pixel: fn() {
            return self.fx_sprite(self.v_uv, self.v_color)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // -----------------------------------------------------------------
    // MESH + WIND FIELD: the solid-mesh family (grass/lsystem/metaballs)
    // with the analytic wind REPLACED by the wind field — a doc opts in
    // with `wind_field: "<name>"`. The gust channel also LIGHTS what it
    // bends, so a traveling gust is visible twice over. Same hooks as
    // DrawVjFxMesh (fx_displace, fx_color).
    // u_wind = (world to uv scale, base gain, gust gain, unused).
    // -----------------------------------------------------------------
    mod.draw.DrawVjFxMeshField = set_type_default() do #(DrawVjFxMeshField::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        wind_tex: texture_2d(float)
        // Content coupling: wind_tex owns slot 0, so input0 rides slot 1.
        tex0: texture_2d(float)
        has_content: uniform(0.0)
        u_wind: uniform(vec4(0.0416, 1.0, 1.6, 0.0))
        backface_culling: false
        alpha_blend: false
        depth_write: true

        v_color: varying(vec4f)
        v_normal: varying(vec3f)
        v_world: varying(vec3f)
        v_gust: varying(float)

        windbil: fn(uv: vec2) -> vec4 {
            let dim = self.wind_tex.size()
            let p = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)) * dim - vec2(0.5, 0.5)
            let i = floor(p)
            let f = p - i
            let tx = vec2(1.0 / dim.x, 1.0 / dim.y)
            let c = (i + vec2(0.5, 0.5)) * tx
            let s00 = self.wind_tex.sample_nearest(c, 0.0)
            let s10 = self.wind_tex.sample_nearest(c + vec2(tx.x, 0.0), 0.0)
            let s01 = self.wind_tex.sample_nearest(c + vec2(0.0, tx.y), 0.0)
            let s11 = self.wind_tex.sample_nearest(c + vec2(tx.x, tx.y), 0.0)
            return mix(mix(s00, s10, f.x), mix(s01, s11, f.x), f.y)
        }

        fx_displace: fn(pos: vec3, normal: vec3, attr: vec4) -> vec3 {
            return vec3(0.0, 0.0, 0.0)
        }

        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            let base = self.col_a.mix(self.col_b, clamp(attr.z, 0.0, 1.0))
            let tip = self.col_c * (clamp(t, 0.0, 1.0) * 0.35)
            let flash = self.col_c * (self.time_beat.w * 0.5)
            return vec4(base.xyz + tip.xyz + flash.xyz, 1.0)
        }

        vertex: fn() {
            let attr = vec4(
                self.geom.geom_id,
                self.geom.geom_pad,
                self.geom.geom_tail_pad_0,
                self.geom.geom_tail_pad_1
            )
            let depth = attr.x
            let arc = attr.y
            let branch = self.geom.geom_uv.y
            let center = self.geom.geom_pos - self.geom.geom_normal * attr.w
            let g = clamp((self.anim.z - arc) * 9.0, 0.12, 1.0)
            let grown = center + self.geom.geom_normal * (attr.w * g)
            // THE FIELD: wind sampled at the REST position (continuous —
            // joints can never tear), amplitude grows along the arc.
            let fuv = center.xz * self.u_wind.x + vec2(0.5, 0.5)
            let w = self.windbil(fuv)
            let gust = w.z
            let bend = self.anim.x * arc * arc * 1.4
            let gain = self.u_wind.y + gust * self.u_wind.z
            let wind = vec3(w.x, 0.0 - abs(w.x + w.y) * 0.12, w.y) * (bend * gain)
            let wf = self.time_beat.x * self.anim.y
            let flutter = sin(wf * 3.1 + branch * 41.0) * depth * 0.012
                * self.anim.x * (1.0 + gust * 1.5)
            let tw = self.anim.w * grown.y * 0.35
            let cs = cos(tw)
            let sn = sin(tw)
            let twisted = vec3(
                grown.x * cs - grown.z * sn,
                grown.y,
                grown.x * sn + grown.z * cs
            )
            let scale = 1.0 + self.time_beat.w * 0.14
            let mut pos = twisted * scale + wind + vec3(flutter, 0.0, flutter * 0.7)
            pos = pos + self.fx_displace(pos, self.geom.geom_normal, attr)

            let world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_world = world.xyz
            self.v_normal = self.geom.geom_normal
            self.v_gust = clamp(gust, 0.0, 3.0)
            let grow_dim = 0.20 + 0.80 * g
            let c = self.fx_color(arc, attr, self.geom.geom_normal, world.xyz)
            self.v_color = vec4(c.xyz * grow_dim, c.w)
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            return self.vertex_pos
        }

        pixel: fn() {
            let n = normalize(self.v_normal)
            let cam = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            let cam_pos = cam.xyz / max(cam.w, 0.0001)
            let view_dir = normalize(cam_pos - self.v_world)
            let rim = pow(1.0 - abs(dot(n, view_dir)), 2.0)
            let lit = 0.55 + 0.45 * abs(dot(n, normalize(vec3(0.4, 0.8, 0.35))))
            let d = length(self.v_world - cam_pos)
            let fog = exp(0.0 - d * self.fog.x)
            // The gust LIGHTS the meadow it moves through.
            let gustlight = 1.0 + self.v_gust * 0.45
            let glow = self.v_color.xyz * (lit * self.fog.y * gustlight)
                + self.col_c.xyz * rim * 0.55
            let final_rgb = glow.mix(self.col_bg.xyz, 1.0 - fog)
            return vec4(final_rgb, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

// ---------------------------------------------------------------------------
// Rust structs. Update + view shaders are DrawQuad subclasses (uniforms
// only, set via draw_vars); the swarm draw + mesh-field shaders carry the
// STANDARD fx signal block as instance fields (vec4 only, after deref).
// ---------------------------------------------------------------------------

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxSimWind {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxSimSwarm {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxFluidAdvect {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxFluidDiv {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxFluidJacobi {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxFluidProject {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxFluidDye {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxFluidView {
    #[deref]
    pub draw_super: DrawQuad,
}

macro_rules! fx_sim_mesh_struct {
    ($name:ident) => {
        #[derive(Script, ScriptHook)]
        #[repr(C)]
        pub struct $name {
            #[deref]
            pub draw_vars: DrawVars,
            /// (time, beat position, beat phase 0..1, eased pulse).
            #[live(vec4(0.0, 0.0, 0.0, 0.0))]
            pub time_beat: Vec4f,
            /// (bar phase 0..1, bpm, audio energy 0..1, dt).
            #[live(vec4(0.0, 120.0, 0.0, 0.0))]
            pub sig: Vec4f,
            /// Document-bound user parameters p0..p3.
            #[live(vec4(0.0, 0.0, 0.0, 0.0))]
            pub user: Vec4f,
            /// (sway, sway_freq, growth 0..1, twist).
            #[live(vec4(0.0, 1.0, 1.0, 0.0))]
            pub anim: Vec4f,
            /// Engine-specific (see engines_simfx.rs).
            #[live(vec4(0.0, 0.0, 0.0, 0.0))]
            pub shape: Vec4f,
            #[live(vec4(0.0, 0.0, 0.0, 0.0))]
            pub flow: Vec4f,
            #[live(vec4(0.28, 0.94, 1.0, 1.0))]
            pub col_a: Vec4f,
            #[live(vec4(1.0, 0.25, 0.63, 1.0))]
            pub col_b: Vec4f,
            #[live(vec4(1.0, 1.0, 1.0, 1.0))]
            pub col_c: Vec4f,
            #[live(vec4(0.01, 0.012, 0.03, 1.0))]
            pub col_bg: Vec4f,
            /// (fog density, emissive gain, tex mix, unused).
            #[live(vec4(0.05, 1.0, 0.0, 0.0))]
            pub fog: Vec4f,
        }
    };
}

fx_sim_mesh_struct!(DrawVjFxSimSwarmDraw);
fx_sim_mesh_struct!(DrawVjFxMeshField);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_defaults_are_sane_and_clamped() {
        let w = SimFieldCfg::wind("wind");
        assert_eq!(w.res, 64);
        assert!(w.extent > 0.0);
        assert_eq!(w.par.len(), WIND_PARS);
        let s = SimFieldCfg::swarm("state", 9999);
        assert_eq!(s.res, 160, "swarm side must clamp");
        assert_eq!(s.par.len(), SWARM_PARS);
        let f = SimFieldCfg::fluid("dye", 8);
        assert_eq!(f.res, 32, "fluid grid must clamp");
        assert_eq!(f.par.len(), FLUID_PARS);
        assert!(f.iters >= 4 && f.iters <= 40);
    }

    #[test]
    fn fluid_pass_budget_is_bounded() {
        // advect + div + jacobi*iters + project + dye must stay bounded by
        // the iters clamp: worst case 4 + 40 passes at 256^2.
        let f = SimFieldCfg::fluid("dye", 256);
        let passes = 4 + f.iters;
        assert!(passes <= 44);
    }
}
