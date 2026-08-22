//! Flock — a CPU boids murmuration emitting oriented glider triangles every
//! frame (the ribbons-style regen family: small per-frame mesh, recycled
//! capacity-stable buffers).
//!
//! The CPU steps 200–400 boids (O(N²) neighbor pass — fine at these counts,
//! measured) with the classic separation / alignment / cohesion forces plus
//! a GOAL POINT that jumps to a new hashed position every `goal_beats`
//! beats — the murmuration swings across the volume in time with the music.
//! An optional PREDATOR dives through the flock centre on every bar,
//! scattering the field before it re-forms.
//!
//! Per bird the engine emits three oriented triangles (two wings + a tail
//! fin) built on the bird's banked flight frame. The WING FLAP lives on the
//! vertex stream: each vertex carries its flap amplitude (0 on the spine,
//! full at the wingtips) and its bird's flap-phase hash; the vertex shader
//! beats the wings — so a 60-vert bird flock flaps at pixel-perfect
//! per-bird phase with zero CPU cost.
//!
//! # Vertex channels (CubeVertex layout — documented in CONTRACT.md)
//!   geom_pos = vertex REST position on the flight frame (wings flat)
//!   a_id     = bird index
//!   normal   = the bird's banked UP vector (the flap displacement axis)
//!   a_aux    = speed01 (0 slow .. 1 at the speed ceiling — brightness)
//!   uv       = (along-body 0 nose..1 tail, flap phase hash)
//!   a_r0     = per-bird hue hash
//!   a_r1     = flap amplitude at THIS vertex (0 spine, wingspan at tips)
//!
//! # Document keys (`engine: "flock"`)
//! `birds` (320, 8..600), `size` (0.14 body scale), `flight_speed` (2.4
//! cruise — `speed` is the shared time multiplier, ribbons' `flow_speed`
//! convention), `flap` (3.0 flaps/sec base, ±25% per bird), `bound` (6.0),
//! `spacing` (0.45 separation radius), `vision` (1.6 neighbor radius),
//! `goal_beats` (2 — beats between goal jumps), `predator` (0..1 scatter
//! strength, dives on the bar), `additive` (0 solid silhouettes .. 1 neon
//! additive), `bank` (1.0 roll-into-turns gain). The shared `bar_beats` key
//! also feeds the predator's bar clock. Bindings: p0..p3 free for hooks.
//! Hook: `fx_color(t = speed01, attr = (id, speed01, hue, flap_amp))`.
//!
//! Content coupling (`content:` → `fog.z`): a dim clip-space backdrop of
//! the channel video behind the murmuration (invisible standalone).

use super::engines::EngineUniforms;
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

pub struct FlockConfig {
    pub birds: usize,
    pub size: f32,
    /// Cruise speed (world units/sec); actual speeds ride 0.45x..1.7x.
    pub speed: f32,
    /// Base flaps per second.
    pub flap: f32,
    /// Radius of the flight volume.
    pub bound: f32,
    /// Separation radius.
    pub spacing: f32,
    /// Alignment/cohesion neighbor radius.
    pub vision: f32,
    /// Beats between goal jumps.
    pub goal_beats: f32,
    /// Predator scatter strength 0..1 (0 = no predator).
    pub predator: f32,
    /// 0 = solid silhouettes, 1 = additive neon.
    pub additive: f32,
    /// Roll-into-turns gain.
    pub bank: f32,
    /// Mirror of the document's bar_beats (the predator's bar clock).
    pub bar_beats: f32,
    pub seed: u64,
}

impl Default for FlockConfig {
    fn default() -> Self {
        Self {
            birds: 320,
            size: 0.14,
            speed: 2.4,
            flap: 3.0,
            bound: 6.0,
            spacing: 0.45,
            vision: 1.6,
            goal_beats: 2.0,
            predator: 0.0,
            additive: 0.0,
            bank: 1.0,
            bar_beats: 4.0,
            seed: 21,
        }
    }
}

/// Seconds the predator's dive lasts.
const PREDATOR_DIVE_SECS: f32 = 1.15;
const VERTS_PER_BIRD: usize = 9;

pub struct FlockEngine {
    pub cfg: FlockConfig,
    pos: Vec<Vec3f>,
    vel: Vec<Vec3f>,
    steer: Vec<Vec3f>,
    hue: Vec<f32>,
    phase: Vec<f32>,
    goal: Vec3f,
    /// Beat-edge detector state (regen only sees the 0..1 phase).
    last_phase: f32,
    beat_count: u64,
    predator_age: f32,
    predator_from: Vec3f,
    predator_to: Vec3f,
    rng: FxRng,
    pub(crate) warmed: bool,
}

impl FlockEngine {
    pub fn new(cfg: FlockConfig) -> Self {
        let n = cfg.birds.clamp(8, 600);
        let mut rng = FxRng::new(cfg.seed);
        let bound = Self::san(cfg.bound, 6.0).clamp(1.0, 40.0);
        let speed = Self::san(cfg.speed, 2.4).clamp(0.2, 20.0);
        let mut pos = Vec::with_capacity(n);
        let mut vel = Vec::with_capacity(n);
        let mut hue = Vec::with_capacity(n);
        let mut phase = Vec::with_capacity(n);
        for _ in 0..n {
            pos.push(vec3f(
                rng.range(-bound * 0.4, bound * 0.4),
                rng.range(-bound * 0.2, bound * 0.2),
                rng.range(-bound * 0.4, bound * 0.4),
            ));
            let d = vec3f(rng.range(-1.0, 1.0), rng.range(-0.3, 0.3), rng.range(-1.0, 1.0));
            let dl = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt().max(1e-3);
            vel.push(d * (speed / dl));
            hue.push(rng.next_f32());
            phase.push(rng.next_f32());
        }
        let goal = vec3f(rng.range(-bound, bound) * 0.5, rng.range(-0.2, 0.4) * bound, 0.0);
        Self {
            cfg,
            steer: vec![Vec3f::default(); n],
            pos,
            vel,
            hue,
            phase,
            goal,
            last_phase: 0.0,
            beat_count: 0,
            predator_age: PREDATOR_DIVE_SECS + 1.0,
            predator_from: Vec3f::default(),
            predator_to: Vec3f::default(),
            rng,
            warmed: false,
        }
    }

    fn san(v: f32, d: f32) -> f32 {
        if v.is_finite() {
            v
        } else {
            d
        }
    }

    fn centroid(&self) -> Vec3f {
        let mut c = Vec3f::default();
        for p in &self.pos {
            c = c + *p;
        }
        c * (1.0 / self.pos.len().max(1) as f32)
    }

    fn new_goal(&mut self) {
        let bound = Self::san(self.cfg.bound, 6.0).clamp(1.0, 40.0);
        self.goal = vec3f(
            self.rng.range(-0.62, 0.62) * bound,
            self.rng.range(-0.22, 0.38) * bound,
            self.rng.range(-0.62, 0.62) * bound,
        );
    }

    fn launch_predator(&mut self) {
        let c = self.centroid();
        let bound = Self::san(self.cfg.bound, 6.0).clamp(1.0, 40.0);
        let a = self.rng.range(0.0, std::f32::consts::TAU);
        let dir = vec3f(a.cos(), self.rng.range(-0.35, 0.1), a.sin());
        self.predator_from = c + dir * (bound * 1.1);
        self.predator_to = c - dir * (bound * 1.1);
        self.predator_age = 0.0;
    }

    /// One boid step. Beat edges are detected from the wrapping 0..1 phase
    /// (regen's clock) — a goal jump every `goal_beats` beats, a predator
    /// dive on every bar when enabled. The goal anchor also DRIFTS on a slow
    /// lissajous (`time`), so the flock is always chasing, always streaming —
    /// never milling in a ball around a stationary point.
    pub(crate) fn step(&mut self, dt: f32, time: f32, beat_phase: f32) {
        let dt = dt.clamp(0.0, 0.05).max(1e-4);
        if beat_phase < self.last_phase - 0.5 {
            self.beat_count += 1;
            let goal_beats = Self::san(self.cfg.goal_beats, 2.0).clamp(0.5, 32.0) as u64;
            if self.beat_count % goal_beats.max(1) == 0 {
                self.new_goal();
            }
            let bar = Self::san(self.cfg.bar_beats, 4.0).clamp(1.0, 32.0) as u64;
            if self.cfg.predator > 0.001 && self.beat_count % bar.max(1) == 0 {
                self.launch_predator();
            }
        }
        self.last_phase = beat_phase;
        self.predator_age += dt;

        let n = self.pos.len();
        let speed = Self::san(self.cfg.speed, 2.4).clamp(0.2, 20.0);
        let bound = Self::san(self.cfg.bound, 6.0).clamp(1.0, 40.0);
        let sep_r = Self::san(self.cfg.spacing, 0.45).clamp(0.05, 4.0);
        let vis_r = Self::san(self.cfg.vision, 1.6).clamp(sep_r, 8.0);
        let sep_r2 = sep_r * sep_r;
        let vis_r2 = vis_r * vis_r;
        let (vmin, vmax) = (speed * 0.45, speed * 1.7);
        let predator_on = self.predator_age < PREDATOR_DIVE_SECS && self.cfg.predator > 0.001;
        let pred_pos = if predator_on {
            let k = (self.predator_age / PREDATOR_DIVE_SECS).clamp(0.0, 1.0);
            self.predator_from + (self.predator_to - self.predator_from) * k
        } else {
            Vec3f::default()
        };
        let scare_r = bound * 0.7;
        let scare = Self::san(self.cfg.predator, 0.0).clamp(0.0, 1.0) * 34.0;
        // The beat-jumped anchor plus a slow lissajous drift: the flock is
        // always chasing a moving point, so it streams instead of milling.
        let goal_dyn = self.goal
            + vec3f(
                (time * 0.47).sin(),
                0.35 * (time * 0.31).sin(),
                (time * 0.41).cos(),
            ) * (bound * 0.28);

        // O(N²) neighbor pass, symmetric halves.
        for s in self.steer.iter_mut() {
            *s = Vec3f::default();
        }
        for i in 0..n {
            let pi = self.pos[i];
            let mut sep = Vec3f::default();
            let mut ali = Vec3f::default();
            let mut coh = Vec3f::default();
            let mut cnt = 0.0f32;
            for j in 0..n {
                if j == i {
                    continue;
                }
                let d = self.pos[j] - pi;
                let d2 = d.x * d.x + d.y * d.y + d.z * d.z;
                if d2 > vis_r2 {
                    continue;
                }
                if d2 < sep_r2 {
                    sep = sep - d * (1.0 / (d2 + 1e-3));
                }
                ali = ali + self.vel[j];
                coh = coh + d;
                cnt += 1.0;
            }
            // Weights: separation strong, cohesion deliberately weak — the
            // GOAL is what gathers the flock, so it streams instead of
            // balling up into a fish-school knot.
            let mut st = sep * 1.15;
            if cnt > 0.0 {
                let inv = 1.0 / cnt;
                st = st + (ali * inv - self.vel[i]) * 0.55;
                st = st + coh * (inv * 0.20);
            }
            // Goal attraction — the beat-jumped, ever-drifting point the
            // flock swings to.
            st = st + (goal_dyn - pi) * 0.5;
            // Soft containment.
            let r = (pi.x * pi.x + pi.y * pi.y + pi.z * pi.z).sqrt();
            if r > bound * 0.85 {
                st = st - pi * ((r - bound * 0.85) / r.max(1e-3) * 3.0);
            }
            // Predator scatter.
            if predator_on {
                let dp = pi - pred_pos;
                let dl = (dp.x * dp.x + dp.y * dp.y + dp.z * dp.z).sqrt();
                if dl < scare_r {
                    st = st + dp * (scare * (1.0 - dl / scare_r) / dl.max(0.05));
                }
            }
            self.steer[i] = st;
        }
        for i in 0..n {
            let mut v = self.vel[i] + self.steer[i] * (dt * 2.4);
            let vl = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt().max(1e-4);
            let vc = vl.clamp(vmin, vmax);
            v = v * (vc / vl);
            self.vel[i] = v;
            self.pos[i] = self.pos[i] + v * dt;
        }
    }

    /// Emit three oriented triangles per bird on its banked flight frame.
    /// Wing flap is NOT baked here — tips carry their flap amplitude in
    /// a_r1 and the vertex shader beats them from the time uniform.
    pub(crate) fn emit(&mut self, mesh: &mut FxMesh) {
        mesh.clear();
        // CONTENT backdrop (FIRST = painted behind the flock): one
        // clip-space quad showing the channel video dimmed behind the
        // murmuration. a_aux = 2.0 marks it (birds carry speed01 0..1);
        // the pixel stage gates it with the pre-gated content strength,
        // so it is invisible standalone.
        {
            let n0 = vec3f(0.0, 0.0, 1.0);
            let corners = [
                (vec3f(-1.0, -1.0, 0.0), vec2f(0.0, 1.0)),
                (vec3f(1.0, -1.0, 0.0), vec2f(1.0, 1.0)),
                (vec3f(1.0, 1.0, 0.0), vec2f(1.0, 0.0)),
                (vec3f(-1.0, 1.0, 0.0), vec2f(0.0, 0.0)),
            ];
            let mut ids = [0u32; 4];
            for (k, (pos, uv)) in corners.iter().enumerate() {
                ids[k] = mesh.push_vert(*pos, 0.0, n0, 2.0, *uv, 0.0, 0.0);
            }
            mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
        }
        let size = Self::san(self.cfg.size, 0.14).clamp(0.01, 2.0);
        let speed = Self::san(self.cfg.speed, 2.4).clamp(0.2, 20.0);
        let bank_gain = Self::san(self.cfg.bank, 1.0).clamp(0.0, 4.0);
        let (vmin, vmax) = (speed * 0.45, speed * 1.7);
        let l = size * 1.7;
        let w = size * 2.1;
        let up0 = vec3f(0.0, 1.0, 0.0);
        for i in 0..self.pos.len() {
            let p = self.pos[i];
            let v = self.vel[i];
            let vl = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt().max(1e-4);
            let f = v * (1.0 / vl);
            // Right vector; degenerate (vertical flight) falls back to +X.
            let mut r = vec3f(
                f.y * up0.z - f.z * up0.y,
                f.z * up0.x - f.x * up0.z,
                f.x * up0.y - f.y * up0.x,
            );
            let rl = (r.x * r.x + r.y * r.y + r.z * r.z).sqrt();
            if rl < 1e-3 {
                r = vec3f(1.0, 0.0, 0.0);
            } else {
                r = r * (1.0 / rl);
            }
            // Bank: roll the up vector into the turn (lateral steer).
            let st = self.steer[i];
            let lat = (st.x * r.x + st.y * r.y + st.z * r.z) * 0.11 * bank_gain;
            let mut u = up0 - r * lat.clamp(-0.85, 0.85);
            let ul = (u.x * u.x + u.y * u.y + u.z * u.z).sqrt().max(1e-3);
            u = u * (1.0 / ul);

            let speed01 = ((vl - vmin) / (vmax - vmin).max(1e-3)).clamp(0.0, 1.0);
            let hue = self.hue[i];
            let ph = self.phase[i];
            let id = i as f32;
            let flap_amp = w * 0.55;

            let nose = p + f * l;
            let tail = p - f * (l * 0.75);
            let root = p - f * (l * 0.2);
            let tip_l = root - r * w;
            let tip_r = root + r * w;
            let fin_top = tail + u * (l * 0.55) - f * (l * 0.12);
            let mid = p - f * (l * 0.05);

            // Wing L: nose, tail (spine, no flap), tip (full flap).
            let a = mesh.push_vert(nose, id, u, speed01, vec2f(0.0, ph), hue, 0.0);
            let b = mesh.push_vert(tail, id, u, speed01, vec2f(1.0, ph), hue, 0.0);
            let c = mesh.push_vert(tip_l, id, u, speed01, vec2f(0.45, ph), hue, flap_amp);
            mesh.push_tri(a, b, c);
            // Wing R.
            let a2 = mesh.push_vert(nose, id, u, speed01, vec2f(0.0, ph), hue, 0.0);
            let b2 = mesh.push_vert(tail, id, u, speed01, vec2f(1.0, ph), hue, 0.0);
            let c2 = mesh.push_vert(tip_r, id, u, speed01, vec2f(0.45, ph), hue, flap_amp);
            mesh.push_tri(a2, b2, c2);
            // Tail fin (vertical stabilizer, no flap).
            let a3 = mesh.push_vert(mid, id, u, speed01, vec2f(0.6, ph), hue, 0.0);
            let b3 = mesh.push_vert(tail, id, u, speed01, vec2f(1.0, ph), hue, 0.0);
            let c3 = mesh.push_vert(fin_top, id, u, speed01, vec2f(0.85, ph), hue, 0.0);
            mesh.push_tri(a3, b3, c3);
        }
        debug_assert_eq!(mesh.vertex_count(), self.pos.len() * VERTS_PER_BIRD + 4);
        mesh.pad_to_high_water();
    }

    pub fn uniforms(&self) -> EngineUniforms {
        let san = Self::san;
        EngineUniforms {
            shape: vec4(
                san(self.cfg.flap, 3.0).clamp(0.1, 16.0),
                san(self.cfg.additive, 0.0).clamp(0.0, 1.0),
                san(self.cfg.bound, 6.0).clamp(1.0, 40.0),
                0.0,
            ),
            flow: vec4(0.0, 0.0, 0.0, 0.0),
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // -----------------------------------------------------------------------
    // Flock: flat oriented triangles, premultiplied blending. additive
    // (shape.y) crossfades the whole look: 0 = opaque dusk silhouettes
    // fogged into the sky, 1 = additive neon confetti. The wings BEAT here:
    // every vertex displaces along its bird's up vector by its baked flap
    // amplitude — spine verts carry 0 and stay put.
    // -----------------------------------------------------------------------
    mod.draw.DrawVjFxFlock = set_type_default() do #(DrawVjFxFlock::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        has_content: uniform(0.0)
        backface_culling: false
        alpha_blend: true
        depth_write: false

        v_color: varying(vec4f)

        // Document hook: bird tint. t = speed01 (fast birds run brighter),
        // attr = (bird id, speed01, hue, flap amplitude).
        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            let base = self.col_a.mix(self.col_b, attr.z)
            let gain = 0.5 + 0.6 * t + self.time_beat.w * 0.35
            return vec4(base.xyz * gain, 1.0)
        }

        vertex: fn() {
            let attr = vec4(
                self.geom.geom_id,
                self.geom.geom_pad,
                self.geom.geom_tail_pad_0,
                self.geom.geom_tail_pad_1
            )
            if attr.y > 1.5 {
                // CONTENT BACKDROP: clip-space passthrough; the pixel stage
                // shows the channel video dimmed behind the flock, gated by
                // the pre-gated content strength (invisible standalone).
                // v_color.w = -1 marks it (birds carry alpha 0..1).
                self.v_color = vec4(self.geom.geom_uv.x, self.geom.geom_uv.y, 0.0, -1.0)
                self.vertex_pos = vec4(self.geom.geom_pos.x, self.geom.geom_pos.y, 0.5, 1.0)
                return self.vertex_pos
            }
            let up = self.geom.geom_normal
            // Per-bird flap rate (±25%) + baked phase; pulse snaps the beat.
            let rate = self.shape.x * (0.75 + 0.5 * fract(attr.x * 0.6180339))
            let flap = sin((self.time_beat.x * rate + self.geom.geom_uv.y) * 6.2831853)
                * (1.0 + self.time_beat.w * 0.4)
            let pos = self.geom.geom_pos + up * (attr.w * flap)
            let world = self.draw_list.view_transform
                * vec4(pos.x, pos.y, pos.z, 1.0)
            let c = self.fx_color(attr.y, attr, up, world.xyz)
            // Body gradient: nose bright, tail dim.
            let body = 0.75 + 0.25 * (1.0 - self.geom.geom_uv.x)
            let cam = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            let d = length(world.xyz - cam.xyz / max(cam.w, 0.0001))
            let fogf = exp(0.0 - d * self.fog.x)
            let additive = clamp(self.shape.y, 0.0, 1.0)
            let lit = c.xyz * (body * self.fog.y)
            // Opaque path fogs toward the sky color; additive path just
            // fades with distance (never adds sky-colored haze).
            let faded = lit.mix(self.col_bg.xyz, 1.0 - fogf)
            let addrgb = lit * fogf
            let rgb = faded.mix(addrgb, additive)
            self.v_color = vec4(rgb, 1.0 - additive)
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            return self.vertex_pos
        }

        pixel: fn() {
            if self.v_color.w < -0.5 {
                // CONTENT BACKDROP: the video, dimmed so dusk silhouettes
                // still read; blends toward the sky color as fog.z falls.
                let cm = self.fog.z
                let tx = self.tex0.sample_as_bgra(vec2(self.v_color.x, self.v_color.y))
                let rgb = tx.xyz * (0.34 * (1.0 + self.time_beat.w * 0.10))
                return vec4(rgb * cm, cm)
            }
            return self.v_color
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// Standard fx draw layout (see shaders.rs — the view writes these fields).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxFlock {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0..p3 free (reachable from fx_color hooks).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    /// (flap rate, additive 0..1, bound, unused).
    #[live(vec4(3.0, 0.0, 6.0, 0.0))]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flock_emits_nine_verts_per_bird_with_stable_capacity() {
        let mut e = FlockEngine::new(FlockConfig { birds: 60, ..Default::default() });
        let mut mesh = FxMesh::default();
        e.step(1.0 / 60.0, 0.0, 0.0);
        e.emit(&mut mesh);
        // + 4: the content-backdrop quad emitted ahead of the birds.
        assert_eq!(mesh.vertex_count(), 60 * VERTS_PER_BIRD + 4);
        let len = mesh.verts.len();
        for k in 0..30 {
            e.step(1.0 / 60.0, k as f32 / 60.0, (k as f32 * 0.07).fract());
            e.emit(&mut mesh);
            assert_eq!(mesh.verts.len(), len, "flock buffer must be capacity-stable");
        }
    }

    #[test]
    fn flock_channels_carry_flap_and_speed() {
        let mut e = FlockEngine::new(FlockConfig { birds: 40, ..Default::default() });
        let mut mesh = FxMesh::default();
        e.step(1.0 / 60.0, 0.0, 0.0);
        e.emit(&mut mesh);
        let floats = super::super::mesh::VERT_FLOATS;
        let mut tips = 0;
        let mut backdrop = 0;
        for v in mesh.verts.chunks(floats) {
            for f in v {
                assert!(f.is_finite(), "non-finite vertex data");
            }
            if v[7] > 1.5 {
                // The content-backdrop quad (a_aux = 2.0, clip-space).
                backdrop += 1;
                assert!(v[0].abs() == 1.0 && v[1].abs() == 1.0, "backdrop off clip square");
                continue;
            }
            assert!((0.0..=1.0).contains(&v[7]), "speed01 out of range");
            // Up vector roughly unit.
            let ul = (v[4] * v[4] + v[5] * v[5] + v[6] * v[6]).sqrt();
            assert!((ul - 1.0).abs() < 0.05, "up vector must be unit, got {ul}");
            if v[11] > 0.0 {
                tips += 1;
            }
        }
        // Exactly the two wingtips per bird carry flap amplitude.
        assert_eq!(tips, 40 * 2);
        assert_eq!(backdrop, 4, "the content backdrop quad must be present");
    }

    #[test]
    fn flock_beats_move_the_goal_and_speeds_stay_bounded() {
        let mut e = FlockEngine::new(FlockConfig {
            birds: 80,
            goal_beats: 1.0,
            predator: 1.0,
            bar_beats: 2.0,
            ..Default::default()
        });
        let g0 = e.goal;
        // Simulate 8 beats at ~0.5s each.
        let mut phase = 0.0f32;
        for _ in 0..240 {
            phase += 1.0 / 30.0;
            if phase >= 1.0 {
                phase -= 1.0;
            }
            e.step(1.0 / 60.0, 0.0, phase);
        }
        assert!(e.beat_count >= 7, "beat edges must be detected, got {}", e.beat_count);
        let g1 = e.goal;
        assert!(
            (g0.x - g1.x).abs() + (g0.y - g1.y).abs() + (g0.z - g1.z).abs() > 1e-4,
            "goal must jump on the beat"
        );
        let speed = e.cfg.speed;
        for v in &e.vel {
            let vl = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
            assert!(vl.is_finite());
            assert!(vl <= speed * 1.75 + 1e-3, "speed ceiling violated: {vl}");
        }
        for p in &e.pos {
            assert!(p.x.is_finite() && p.y.is_finite() && p.z.is_finite());
        }
    }

    #[test]
    fn flock_degenerate_params_stay_safe() {
        let mut e = FlockEngine::new(FlockConfig {
            birds: 0,
            size: f32::NAN,
            speed: -3.0,
            flap: f32::INFINITY,
            bound: 0.0,
            spacing: f32::NAN,
            vision: 0.0,
            goal_beats: 0.0,
            predator: 99.0,
            bar_beats: 0.0,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        for k in 0..20 {
            e.step(0.5, k as f32 * 0.5, (k as f32 * 0.3).fract());
        }
        e.emit(&mut mesh);
        assert!(mesh.vertex_count() > 0, "bird floor keeps the effect alive");
        for v in mesh.verts.chunks(super::super::mesh::VERT_FLOATS) {
            for f in v {
                assert!(f.is_finite(), "degenerate cfg leaked non-finite data");
            }
        }
        let u = e.uniforms();
        assert!(u.shape.x.is_finite() && u.shape.y >= 0.0 && u.shape.z >= 1.0);
    }
}
