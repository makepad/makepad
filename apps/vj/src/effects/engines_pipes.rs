//! Pipes — the classic 3D-pipes lattice screensaver, beat-grown.
//!
//! A turtle per pipe walks an integer lattice: mostly straight, sometimes a
//! right-angle turn, never through an occupied cell; a stuck pipe TELEPORTS
//! to a fresh free cell and keeps going (the screensaver's respawn). Pipes
//! step round-robin, so the GLOBAL birth order interleaves them and they
//! all grow simultaneously. The whole run is a STATIC mesh — every tube
//! segment and elbow ball carries its birth order (`a_aux`), and the shared
//! growth front (`grow: "loop"` + `grow_beats`, i.e. `u_growth`) replays
//! the build in tempo: segments POP in with an overshoot bulge as the
//! front passes, the newest length glows white-hot, and when the front
//! loops the lattice starts over — respawn as replay.
//!
//! Tube emission reuses the tunnel idiom: rings of `sides`+1 vertices
//! (seam duplicated), radial normals, elbow joints as bulged lat/long
//! balls where the direction changes.
//!
//! # Vertex channels (CubeVertex layout — documented in CONTRACT.md)
//!   a_id  = pipe id
//!   a_aux = birth order 0..1 (global step / steps — THE growth axis)
//!   uv    = (around 0..1, tube: along-segment 0..1 / ball: elevation 0..1)
//!   a_r0  = pipe hue01, a_r1 = local radius (ball rings are bulged)
//!   normal = radial (outward)
//!
//! # Document keys (`engine: "pipes"`)
//! `pipes` (6, ≤16), `bound` (6 — lattice half-extent in cells, ≤10),
//! `cell` (0.55 world per lattice step), `radius` (0.16), `sides` (10,
//! 3..16), `steps` (900, ≤2600 — total segments = the build budget),
//! `turn_chance` (0.35), `pop` (0.4 overshoot bulge 0..1), `hot` (2.5 —
//! white-hot tail length in % of the run). Growth: set `grow: "loop"` +
//! `grow_beats` (the beat clock of the build). Bindings: `p0` nudges the
//! growth front (kick bursts: `"0.06*pulse"`), `p1` adds hot-tail gain,
//! `p2` adds specular gain. Hook: `fx_color` (t = birth 0..1,
//! attr = (pipe, birth, hue, radius)).
//!
//! Content coupling (`content:` → `fog.z`): the glossy pipes reflect the
//! channel video — a fake env map by the mirror direction, folded into
//! the specular/rim term.

use super::engines::EngineUniforms;
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;
use std::collections::HashSet;

pub struct PipesConfig {
    pub pipes: usize,
    /// Lattice half-extent in cells (volume side = 2*bound+1).
    pub bound: i32,
    /// World size of one lattice step.
    pub cell: f32,
    pub radius: f32,
    pub sides: usize,
    /// Total tube segments across all pipes (the build budget).
    pub steps: usize,
    pub turn_chance: f32,
    /// Pop-in overshoot bulge 0..1.
    pub pop: f32,
    /// Hot-tail length, in percent of the whole run.
    pub hot: f32,
    pub seed: u64,
}

impl Default for PipesConfig {
    fn default() -> Self {
        Self {
            pipes: 6,
            bound: 6,
            cell: 0.55,
            radius: 0.16,
            sides: 10,
            steps: 900,
            turn_chance: 0.35,
            pop: 0.4,
            hot: 2.5,
            seed: 23,
        }
    }
}

const DIRS: [(i32, i32, i32); 6] =
    [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)];

struct PipeState {
    cell: (i32, i32, i32),
    dir: usize,
}

pub struct PipesEngine {
    pub cfg: PipesConfig,
    pub(crate) built: bool,
    /// World half-extent (bound * cell) for the camera default.
    pub extent: f32,
    pub segments: usize,
    pub teleports: usize,
}

impl PipesEngine {
    pub fn new(cfg: PipesConfig) -> Self {
        let bound = cfg.bound.clamp(2, 10);
        let cell = if cfg.cell.is_finite() { cfg.cell } else { 0.55 }.clamp(0.1, 4.0);
        Self { cfg, built: false, extent: bound as f32 * cell, segments: 0, teleports: 0 }
    }

    fn san(v: f32, d: f32) -> f32 {
        if v.is_finite() {
            v
        } else {
            d
        }
    }

    fn world(&self, c: (i32, i32, i32)) -> Vec3f {
        let cell = Self::san(self.cfg.cell, 0.55).clamp(0.1, 4.0);
        vec3f(c.0 as f32 * cell, c.1 as f32 * cell, c.2 as f32 * cell)
    }

    fn free_cell(occupied: &HashSet<(i32, i32, i32)>, bound: i32, rng: &mut FxRng)
        -> Option<(i32, i32, i32)> {
        for _ in 0..64 {
            let c = (
                (rng.range(-(bound as f32), bound as f32 + 0.999)).floor() as i32,
                (rng.range(-(bound as f32), bound as f32 + 0.999)).floor() as i32,
                (rng.range(-(bound as f32), bound as f32 + 0.999)).floor() as i32,
            );
            if !occupied.contains(&c) {
                return Some(c);
            }
        }
        None
    }

    /// A frame perpendicular to `d` (axis-aligned input, so this is exact).
    fn frame(d: Vec3f) -> (Vec3f, Vec3f) {
        let u = if d.y.abs() > 0.5 { vec3f(1.0, 0.0, 0.0) } else { vec3f(0.0, 1.0, 0.0) };
        // v = d × u, then u2 = v × d — both unit for axis-aligned d.
        let v = vec3f(
            d.y * u.z - d.z * u.y,
            d.z * u.x - d.x * u.z,
            d.x * u.y - d.y * u.x,
        );
        let u2 = vec3f(
            v.y * d.z - v.z * d.y,
            v.z * d.x - v.x * d.z,
            v.x * d.y - v.y * d.x,
        );
        (u2, v)
    }

    /// One straight tube segment a→b (both ring seams duplicated — the
    /// tunnel seam law).
    #[allow(clippy::too_many_arguments)]
    fn emit_tube(
        mesh: &mut FxMesh,
        a: Vec3f,
        b: Vec3f,
        radius: f32,
        sides: usize,
        pipe: f32,
        birth: f32,
        hue: f32,
    ) {
        let mut d = b - a;
        let l = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt().max(1e-5);
        d = d / l;
        let (u, v) = Self::frame(d);
        let mut ring_a = Vec::with_capacity(sides + 1);
        let mut ring_b = Vec::with_capacity(sides + 1);
        for s in 0..=sides {
            let around = s as f32 / sides as f32;
            let ang = around * std::f32::consts::TAU;
            let radial = u * ang.cos() + v * ang.sin();
            ring_a.push(mesh.push_vert(
                a + radial * radius, pipe, radial, birth, vec2f(around, 0.0), hue, radius,
            ));
            ring_b.push(mesh.push_vert(
                b + radial * radius, pipe, radial, birth, vec2f(around, 1.0), hue, radius,
            ));
        }
        for s in 0..sides {
            mesh.push_quad(ring_a[s], ring_a[s + 1], ring_b[s + 1], ring_b[s]);
        }
    }

    /// An elbow/end ball: a small lat/long sphere, slightly bulged past the
    /// tube radius (the classic joint).
    fn emit_ball(
        mesh: &mut FxMesh,
        c: Vec3f,
        radius: f32,
        pipe: f32,
        birth: f32,
        hue: f32,
    ) {
        let r = radius * 1.45;
        let lats = 4usize;
        let longs = 8usize;
        let mut rows: Vec<Vec<u32>> = Vec::with_capacity(lats + 1);
        for la in 0..=lats {
            let el = la as f32 / lats as f32;
            let phi = (el - 0.5) * std::f32::consts::PI;
            let (sp, cp) = phi.sin_cos();
            let mut row = Vec::with_capacity(longs + 1);
            for lo in 0..=longs {
                let az = lo as f32 / longs as f32;
                let th = az * std::f32::consts::TAU;
                let n = vec3f(cp * th.cos(), sp, cp * th.sin());
                row.push(mesh.push_vert(
                    c + n * r, pipe, n, birth, vec2f(az, el), hue, r,
                ));
            }
            rows.push(row);
        }
        for la in 0..lats {
            for lo in 0..longs {
                mesh.push_quad(
                    rows[la][lo], rows[la][lo + 1], rows[la + 1][lo + 1], rows[la + 1][lo],
                );
            }
        }
    }

    pub(crate) fn build(&mut self, mesh: &mut FxMesh) {
        let mut rng = FxRng::new(self.cfg.seed);
        let n_pipes = self.cfg.pipes.clamp(1, 16);
        let bound = self.cfg.bound.clamp(2, 10);
        let radius = Self::san(self.cfg.radius, 0.16).clamp(0.02, 1.5);
        let sides = self.cfg.sides.clamp(3, 16);
        let steps = self.cfg.steps.clamp(32, 2600);
        let turn = Self::san(self.cfg.turn_chance, 0.35).clamp(0.0, 1.0);

        let mut occupied: HashSet<(i32, i32, i32)> = HashSet::new();
        let mut pipes: Vec<PipeState> = Vec::with_capacity(n_pipes);
        let mut hues: Vec<f32> = Vec::with_capacity(n_pipes);
        for i in 0..n_pipes {
            let Some(c) = Self::free_cell(&occupied, bound, &mut rng) else { break };
            occupied.insert(c);
            pipes.push(PipeState { cell: c, dir: (rng.next_f32() * 6.0) as usize % 6 });
            // Distinct, well-spread hues; a little jitter keeps ties broken.
            hues.push(((i as f32 + 0.5) / n_pipes as f32 + rng.next_f32() * 0.04).fract());
        }
        let mut segs = 0usize;
        let mut teleports = 0usize;
        // Start balls (the classic seeds a ball at every pipe origin).
        for (i, p) in pipes.iter().enumerate() {
            let birth = 0.0;
            Self::emit_ball(mesh, self.world(p.cell), radius, i as f32, birth, hues[i]);
        }
        for step in 0..steps {
            if pipes.is_empty() {
                break;
            }
            let pi = step % pipes.len();
            let birth = step as f32 / steps as f32;
            let (hue, pipe_id) = (hues[pi], pi as f32);
            let cur = pipes[pi].cell;
            let prev_dir = pipes[pi].dir;
            // Candidate order: keep heading (unless a turn is rolled), then
            // the four perpendiculars in random order, then reverse.
            let mut order: Vec<usize> = Vec::with_capacity(6);
            let turning = rng.next_f32() < turn;
            if !turning {
                order.push(prev_dir);
            }
            let mut perps: Vec<usize> = (0..6)
                .filter(|&d| d != prev_dir && d != (prev_dir ^ 1))
                .collect();
            // Fisher-Yates on the four perpendiculars.
            for k in (1..perps.len()).rev() {
                let j = (rng.next_f32() * (k + 1) as f32) as usize % (k + 1);
                perps.swap(k, j);
            }
            order.extend(perps);
            if turning {
                order.push(prev_dir);
            }
            order.push(prev_dir ^ 1);
            let mut moved = false;
            for &di in &order {
                let d = DIRS[di];
                let next = (cur.0 + d.0, cur.1 + d.1, cur.2 + d.2);
                if next.0.abs() > bound || next.1.abs() > bound || next.2.abs() > bound {
                    continue;
                }
                if occupied.contains(&next) {
                    continue;
                }
                // Elbow ball where the heading changes.
                if di != prev_dir {
                    Self::emit_ball(mesh, self.world(cur), radius, pipe_id, birth, hue);
                }
                Self::emit_tube(
                    mesh, self.world(cur), self.world(next), radius, sides, pipe_id, birth, hue,
                );
                occupied.insert(next);
                pipes[pi].cell = next;
                pipes[pi].dir = di;
                segs += 1;
                moved = true;
                break;
            }
            if !moved {
                // Stuck: cap the dead end, teleport to a fresh cell (the
                // screensaver respawn), consume the step.
                Self::emit_ball(mesh, self.world(cur), radius, pipe_id, birth, hue);
                if let Some(c) = Self::free_cell(&occupied, bound, &mut rng) {
                    occupied.insert(c);
                    pipes[pi].cell = c;
                    pipes[pi].dir = (rng.next_f32() * 6.0) as usize % 6;
                    Self::emit_ball(mesh, self.world(c), radius, pipe_id, birth, hue);
                    teleports += 1;
                } else {
                    // Lattice genuinely full — stop the whole build.
                    break;
                }
            }
        }
        self.segments = segs;
        self.teleports = teleports;
    }

    pub fn uniforms(&self) -> EngineUniforms {
        let steps = self.cfg.steps.clamp(32, 2600) as f32;
        // Reveal sharpness: a segment pops over ~2.5 birth steps; hot tail
        // falls off over `hot` percent of the run (birth space).
        let reveal_k = steps / 2.5;
        let hot_q = 100.0 / Self::san(self.cfg.hot, 2.5).clamp(0.2, 40.0);
        EngineUniforms {
            shape: vec4(
                reveal_k,
                hot_q,
                Self::san(self.cfg.pop, 0.4).clamp(0.0, 1.0),
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
    // Pipes: solid depth-tested plastic tubes. The growth front (u_growth =
    // anim.z, swept by grow/grow_beats, nudged by p0) replays the build:
    // radius scales 0→1 with an overshoot bulge as the front passes a
    // vertex's birth order, and the newest length glows white-hot.
    // -----------------------------------------------------------------------
    mod.draw.DrawVjFxPipes = set_type_default() do #(DrawVjFxPipes::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        has_content: uniform(0.0)
        backface_culling: false
        alpha_blend: false
        depth_write: true

        v_color: varying(vec4f)
        v_normal: varying(vec3f)
        v_world: varying(vec3f)
        // (heat, birth, around, unused)
        v_misc: varying(vec4f)

        // Document hook: pipe material. t = birth 0..1,
        // attr = (pipe, birth, hue, radius).
        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            let base = self.col_a.mix(self.col_b, attr.z)
            let shade = 0.85 + 0.15 * sin(attr.z * 6.2831853)
            return vec4(base.xyz * shade, 1.0)
        }

        vertex: fn() {
            let attr = vec4(
                self.geom.geom_id,
                self.geom.geom_pad,
                self.geom.geom_tail_pad_0,
                self.geom.geom_tail_pad_1
            )
            let birth = attr.y
            let r = attr.w
            // Growth front: anim.z sweeps 0..1.3 on the beat clock
            // (grow/grow_beats); p0 nudges it (kick lurch).
            let front = self.anim.z + self.user.x
            let local = clamp((front - birth) * self.shape.x, 0.0, 1.0)
            // Pop-in: overshoot bulge while appearing, settle to 1.
            let popa = local * (1.0 + self.shape.z * sin(local * 3.14159265) * 0.6)
            let center = self.geom.geom_pos - self.geom.geom_normal * r
            let pos = center + self.geom.geom_normal * (r * popa)
            let world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_world = world.xyz
            self.v_normal = self.geom.geom_normal
            // Hot tail: the newest grown length burns; beat pulse feeds it.
            let heat = exp(0.0 - max(front - birth, 0.0) * self.shape.y)
                * step(birth, front)
            self.v_color = self.fx_color(birth, attr, self.geom.geom_normal, world.xyz)
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            // v_misc.zw = this fragment's SCREEN uv — the address the
            // content coupling mirrors the channel video from.
            let ndc = self.vertex_pos.xy / max(self.vertex_pos.w, 0.0001)
            self.v_misc = vec4(
                heat,
                birth,
                clamp(ndc.x * 0.5 + 0.5, 0.0, 1.0),
                clamp(0.5 - ndc.y * 0.5, 0.0, 1.0)
            )
            return self.vertex_pos
        }

        pixel: fn() {
            let n = normalize(self.v_normal)
            let cam = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            let cam_pos = cam.xyz / max(cam.w, 0.0001)
            let vd = normalize(cam_pos - self.v_world)
            let key = normalize(vec3(0.5, 0.75, 0.4))
            let lit = 0.30 + 0.70 * clamp(dot(n, key), 0.0, 1.0)
            let fill = 0.18 * clamp(dot(n, vec3(-0.4, -0.2, -0.6)), 0.0, 1.0)
            // Glossy plastic: half-vector specular + a fresnel rim.
            let hv = normalize(key + vd)
            let spec = pow(clamp(dot(n, hv), 0.0, 1.0), 28.0)
                * (0.9 + self.time_beat.w * 0.8 + self.user.z)
            let rim = pow(1.0 - abs(dot(n, vd)), 3.0) * 0.25
            let heat = self.v_misc.x * (1.0 + self.user.y)
            let hot = self.col_c.xyz * heat * (0.8 + self.time_beat.w * 0.9)
            let d = length(self.v_world - cam_pos)
            let fogf = exp(0.0 - d * self.fog.x)
            // CONTENT: the pipes turn to CHROME and mirror the channel
            // video. Addressing by the mirror direction alone (the first
            // pass) folded a whole frame into a few degrees of tube normal
            // and read as a smear; addressing by the fragment's own SCREEN
            // uv, pushed sideways by the normal, makes each tube reflect
            // what is behind it — the lattice assembles the picture and
            // stays curved metal. fog.z = pre-gated `content`.
            let cm = self.fog.z
            let suv = clamp(
                vec2(self.v_misc.z, self.v_misc.w) + vec2(n.x, 0.0 - n.y) * 0.13,
                vec2(0.0, 0.0),
                vec2(1.0, 1.0)
            )
            let env = self.tex0.sample_as_bgra(suv).xyz * (0.45 + 0.85 * lit)
            let body = (self.v_color.xyz * (lit + fill)).mix(env, clamp(cm * 1.25, 0.0, 1.0))
            let rgb = (body * self.fog.y
                + env * ((rim * 2.0 + spec * 1.2) * cm)
                + self.col_c.xyz * (spec + rim) + hot)
                .mix(self.col_bg.xyz, 1.0 - fogf)
            return vec4(rgb, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// Standard fx draw layout (see shaders.rs — the view writes these fields).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxPipes {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0 = front nudge, p1 = hot-tail gain, p2 = specular gain.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    /// (reveal_k, hot_q, pop, unused).
    #[live(vec4(360.0, 40.0, 0.4, 0.0))]
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
    fn pipes_build_finite_and_reaches_budget() {
        let mut e = PipesEngine::new(PipesConfig::default());
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert!(e.segments > 700, "only {} segments placed", e.segments);
        assert!(mesh.triangle_count() > 1000);
        for v in mesh.verts.chunks(super::super::mesh::VERT_FLOATS) {
            for f in v {
                assert!(f.is_finite(), "non-finite vertex data");
            }
        }
    }

    #[test]
    fn pipes_stay_on_lattice_in_bounds() {
        // Rebuild the walk logic's INVARIANT from the emitted stream: every
        // tube endpoint lies on the lattice, inside bounds, and no lattice
        // cell is entered twice (self-avoidance).
        let cfg = PipesConfig { steps: 600, ..Default::default() };
        let cell = cfg.cell;
        let bound = cfg.bound;
        let mut e = PipesEngine::new(cfg);
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        let floats = super::super::mesh::VERT_FLOATS;
        for v in mesh.verts.chunks(floats) {
            // Ring center = pos - normal*r (radius in a_r1).
            let cx = v[0] - v[4] * v[11];
            let cy = v[1] - v[5] * v[11];
            let cz = v[2] - v[6] * v[11];
            for c in [cx, cy, cz] {
                let l = c / cell;
                assert!(
                    (l - l.round()).abs() < 0.02,
                    "tube/ball center off-lattice: {c}"
                );
                assert!(l.round().abs() <= bound as f32 + 0.01, "out of bounds: {l}");
            }
        }
    }

    #[test]
    fn pipes_birth_order_is_monotone() {
        let mut e = PipesEngine::new(PipesConfig::default());
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        let floats = super::super::mesh::VERT_FLOATS;
        let mut last = 0.0f32;
        for v in mesh.verts.chunks(floats) {
            let birth = v[7];
            assert!(birth >= last - 1e-6, "birth order must never regress");
            last = last.max(birth);
        }
        assert!(last > 0.9, "the run must span the whole birth axis, got {last}");
    }

    #[test]
    fn pipes_degenerate_params_stay_safe() {
        let mut e = PipesEngine::new(PipesConfig {
            pipes: 0,
            bound: 0,
            cell: f32::NAN,
            radius: -1.0,
            sides: 0,
            steps: 0,
            turn_chance: f32::INFINITY,
            hot: 0.0,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert!(mesh.vertex_count() > 0, "even degenerate configs must render");
        let u = e.uniforms();
        for v in [u.shape.x, u.shape.y, u.shape.z, u.shape.w] {
            assert!(v.is_finite(), "uniform not sanitized");
        }
    }
}
