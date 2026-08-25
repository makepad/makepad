//! Screen-space ambient occlusion, as three chained offscreen passes.
//!
//! [`SsaoPass`] is a self-contained pass chain a host viewport parents into
//! its own: `raw` (hemisphere taps) → `blur h` → `blur v` (separable
//! bilateral). It reads ONE input: a linear-depth target whose red channel
//! is the VIEW-SPACE DISTANCE in metres (0 = background) — a CAD host's aux
//! G-buffer already is one, and any depth prepass can produce one. View
//! position is reconstructed by unprojecting that distance along the pixel's
//! ray; the surface normal comes from depth differences (the tighter side
//! per axis, so silhouettes do not bleed a false slope inward).
//!
//! What the result MEANS is pinned by the consumers, not here: occlusion is
//! ambient light failing to arrive, so the factor multiplies the AMBIENT /
//! sky fill only — never the direct sun, never the shadow-mapped light
//! (`DrawSceneSkinned`'s `ssao_ctl` hook, and a CAD composite's unlit
//! shading). A sunlit facade keeps its brightness while its creases darken.
//!
//! Laws, in order of how much debugging they encode:
//!
//! 1. **Coincident surfaces never occlude each other.** Buildings carry
//!    coplanar layers millimetres apart (a roof's build-up); a per-vertex
//!    bake gave those layers different values and turned an invisible depth
//!    fight into visible flicker. Here the tap test only counts an occluder
//!    that stands in front of the sample point by more than
//!    [`SsaoParams::bias`] (metres, floor well above any coplanar gap), and
//!    both layers read the same screen texel anyway — whichever face wins
//!    the depth fight, the factor is the same. `tap_occlusion` below is the
//!    exact shader expression, and the tests hold it to this law.
//! 2. **The radius is WORLD-space metres** (~0.3 m): a crease's shadow is a
//!    property of the building, so zooming must not grow it.
//! 3. **The rotation is a hash of the pixel coordinate** — stable across
//!    frames. A frame-varying rotation crawls; noise is only acceptable
//!    because the bilateral blur behind it is depth-aware and stops at
//!    silhouettes.
//! 4. **Distant occluders contribute nothing**: contribution fades to zero
//!    once the occluder stands more than `2 × radius` in front — a roof
//!    edge three metres before the lawn is a different object, not a
//!    crevice in the lawn.

use makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // =====================================================================
    // RAW — a dozen hemisphere taps against the reconstructed surface
    // =====================================================================
    mod.draw.DrawSsaoRaw = mod.std.set_type_default() do #(DrawSsaoRaw::script_shader(vm)){
        ..mod.draw.DrawQuad
        // (ao, view distance) — the depth rides along so the blur reads ONE
        // 8-byte texture per tap instead of this pass's 16-byte G-buffer;
        // that halved the blur's GPU time. f16 depth is for blur WEIGHTS
        // only (0.1% relative is plenty for a sigma), never for the taps.
        color_format: @Rgba16F
        // r = view-space distance in metres, 0 = background.
        depth_tex: texture_2d(float)
        // xy = 1 / pass pixels, zw = pass pixels
        u_texel: uniform(vec4(0.001, 0.001, 1000.0, 1000.0))
        // Camera terms for unproject/reproject:
        //   x = tan(fov_x / 2)   (perspective)   or half width in metres (ortho)
        //   y = tan(fov_y / 2)   (perspective)   or half height in metres (ortho)
        //   z = 1 for orthographic
        u_proj: uniform(vec4(0.5773, 0.5773, 0.0, 0.0))
        // x = radius (m), y = bias floor (m), z = bias slope per metre of
        // view distance, w unused
        u_ao: uniform(vec4(0.3, 0.015, 0.002, 0.0))
        // Nothing to clip against: the quad IS the pass. Straight to clip
        // space, exactly like a CAD composite quad — the pass keeps no
        // camera of its own.
        depth_clip: 0.0

        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(
                self.geom.pos.x * 2.0 - 1.0
                1.0 - self.geom.pos.y * 2.0
                0.0
                1.0
            )
        }

        // uv + stored distance → view-space position. View space is metric
        // (the view matrix is rigid), camera at the origin looking down -z.
        view_pos: fn(uv: vec2f, d: float) -> vec3f {
            let nx = uv.x * 2.0 - 1.0
            let ny = 1.0 - uv.y * 2.0
            if self.u_proj.z > 0.5 {
                let x = nx * self.u_proj.x
                let y = ny * self.u_proj.y
                let z2 = max(d * d - x * x - y * y, 0.000001)
                return vec3(x, y, 0.0 - sqrt(z2))
            }
            let ray = vec3(nx * self.u_proj.x, ny * self.u_proj.y, 0.0 - 1.0)
            return normalize(ray) * d
        }

        // view-space position → uv, the inverse of view_pos.
        project_uv: fn(p: vec3f) -> vec2f {
            var nx = 0.0
            var ny = 0.0
            if self.u_proj.z > 0.5 {
                nx = p.x / self.u_proj.x
                ny = p.y / self.u_proj.y
            } else {
                let w = max(0.0 - p.z, 0.000001)
                nx = p.x / (self.u_proj.x * w)
                ny = p.y / (self.u_proj.y * w)
            }
            return vec2(nx * 0.5 + 0.5, 0.5 - ny * 0.5)
        }

        pixel: fn() -> vec4f {
            let uv = self.pos
            let d = self.depth_tex.sample_nearest(uv).x
            if d <= 0.0 {
                return vec4(1.0, 0.0, 0.0, 1.0)
            }
            let p = self.view_pos(uv, d)

            // ---- normal from depth differences -------------------------
            // Four neighbour taps; per axis, the side whose depth is CLOSER
            // to the centre wins, so a silhouette's far side never smears a
            // false slope across the edge.
            let tx = self.u_texel.x
            let ty = self.u_texel.y
            let dl = self.depth_tex.sample_nearest(uv + vec2(0.0 - tx, 0.0)).x
            let dr = self.depth_tex.sample_nearest(uv + vec2(tx, 0.0)).x
            let du = self.depth_tex.sample_nearest(uv + vec2(0.0, 0.0 - ty)).x
            let dd = self.depth_tex.sample_nearest(uv + vec2(0.0, ty)).x
            var ddx = self.view_pos(uv + vec2(tx, 0.0), dr) - p
            if dr <= 0.0 || (dl > 0.0 && abs(dl - d) < abs(dr - d)) {
                ddx = p - self.view_pos(uv + vec2(0.0 - tx, 0.0), dl)
            }
            var ddy = self.view_pos(uv + vec2(0.0, ty), dd) - p
            if dd <= 0.0 || (du > 0.0 && abs(du - d) < abs(dd - d)) {
                ddy = p - self.view_pos(uv + vec2(0.0, 0.0 - ty), du)
            }
            var n = cross(ddy, ddx)
            if length(n) < 0.0000001 {
                // A one-pixel sliver: face the camera rather than divide by
                // zero — its occlusion is meaningless either way.
                n = 0.0 - normalize(p)
            }
            n = normalize(n)
            if dot(n, p) > 0.0 {
                n = 0.0 - n
            }

            // ---- tangent frame + stable per-pixel rotation -------------
            var up = vec3(0.0, 1.0, 0.0)
            if abs(n.y) > 0.9 {
                up = vec3(1.0, 0.0, 0.0)
            }
            let t = normalize(cross(up, n))
            let b = cross(n, t)
            // Hash of the PIXEL coordinate, never of time: a rotation that
            // varies per frame crawls over every surface.
            let pix = floor(uv * self.u_texel.zw)
            let rot = fract(sin(dot(pix, vec2(12.9898, 78.233))) * 43758.5453) * 6.2831853

            // ---- the hemisphere ----------------------------------------
            let radius = self.u_ao.x
            // The coincident-plane law: an occluder must stand in front of
            // the sample point by more than this, and the floor sits well
            // above any coplanar build-up gap. Slope-scaled with distance
            // so grazing walls at range stay clean.
            let bias = self.u_ao.y + d * self.u_ao.z
            var occ = 0.0
            var k = 0.0
            while k < 12.0 {
                let fi = (k + 0.5) / 12.0
                let ang = k * 2.3999632 + rot
                // Lifted off the tangent plane (z >= ~0.2) so a flat
                // surface's own rasterization noise never occludes it.
                let z = mix(0.18, 0.92, fi)
                let r = sqrt(max(1.0 - z * z, 0.0))
                // Denser near the centre: contact shadows carry the look.
                let scale = mix(0.2, 1.0, fi * fi) * radius
                let s = p + (t * (cos(ang) * r) + b * (sin(ang) * r) + n * z) * scale
                let suv = self.project_uv(s)
                if suv.x > 0.0 && suv.x < 1.0 && suv.y > 0.0 && suv.y < 1.0 {
                    let sd = self.depth_tex.sample_nearest(suv).x
                    if sd > 0.0 {
                        // How far the geometry there stands IN FRONT of the
                        // sample point, metres. tap_occlusion in ssao.rs is
                        // this exact pair of lines; the tests hold it to the
                        // coincident-plane and range laws.
                        let ahead = length(s) - sd
                        let range = 1.0 - smoothstep(radius, radius * 2.0, abs(d - sd))
                        occ = occ + smoothstep(bias, bias * 2.0, ahead) * range
                    }
                }
                k = k + 1.0
            }
            let ao = clamp(1.0 - occ / 12.0, 0.0, 1.0)
            return vec4(ao, d, 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // =====================================================================
    // BLUR — separable bilateral, run twice (u_dir picks the axis)
    // =====================================================================
    mod.draw.DrawSsaoBlur = mod.std.set_type_default() do #(DrawSsaoBlur::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba16F
        // (ao, view distance) from the raw pass / the previous blur axis —
        // one 8-byte read per tap, which is what keeps this pass cheap.
        src_tex: texture_2d(float)
        // xy = one-texel step along the blur axis, zw unused
        u_dir: uniform(vec4(0.001, 0.0, 0.0, 0.0))
        depth_clip: 0.0

        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(
                self.geom.pos.x * 2.0 - 1.0
                1.0 - self.geom.pos.y * 2.0
                0.0
                1.0
            )
        }

        pixel: fn() -> vec4f {
            let uv = self.pos
            let c = self.src_tex.sample_nearest(uv)
            let d = c.y
            if d <= 0.0 {
                return vec4(c.x, d, 0.0, 1.0)
            }
            // Depth sigma grows with distance so the weight is a SURFACE
            // question, not a screen one; the blur never averages across a
            // silhouette or a step, which is what keeps the noise from
            // bleeding a halo off every edge.
            let sigma = 0.03 * d + 0.02
            var sum = 0.0
            var wsum = 0.0
            var k = 0.0 - 4.0
            while k < 4.5 {
                let s = self.src_tex.sample_nearest(uv + self.u_dir.xy * k)
                let dz = (s.y - d) / sigma
                // Gaussian sigma 2.2 px: exp(-k^2 / (2 * 2.2^2)); the depth
                // term is exp(-dz^2).
                var w = exp((0.0 - k * k) * 0.1033) * exp(0.0 - dz * dz)
                if s.y <= 0.0 {
                    w = 0.0
                }
                sum = sum + s.x * w
                wsum = wsum + w
                k = k + 1.0
            }
            return vec4(sum / max(wsum, 0.0001), d, 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSsaoRaw {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSsaoBlur {
    #[deref]
    pub draw_super: DrawQuad,
}

/// The knobs. Metres throughout — the whole point is that none of this is
/// a function of the window or the zoom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SsaoParams {
    /// Hemisphere radius in metres. A doorstep-scale contact shadow.
    pub radius: f32,
    /// Occluder exclusion floor in metres (law 1): surfaces closer together
    /// than this never occlude one another. Keep it well above the
    /// millimetre gaps of coplanar construction layers.
    pub bias: f32,
    /// Extra bias per metre of view distance, so grazing walls at range
    /// stay clean.
    pub bias_slope: f32,
    /// How dark full occlusion goes at the consumer (0 = off, 1 = black).
    /// The consumers apply it — the pass itself outputs raw visibility.
    pub strength: f32,
}

impl Default for SsaoParams {
    fn default() -> Self {
        Self {
            radius: 0.3,
            bias: 0.015,
            bias_slope: 0.002,
            strength: 0.7,
        }
    }
}

/// Camera terms for reconstruction, matching the projection that produced
/// the depth target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SsaoProjection {
    pub ortho: bool,
    /// Perspective: tan(fov_x / 2). Ortho: half the view width in metres.
    pub half_x: f32,
    /// Perspective: tan(fov_y / 2). Ortho: half the view height in metres.
    pub half_y: f32,
}

struct SsaoStage {
    pass: DrawPass,
    list: DrawList,
}

/// The chain. Owns its passes, draw lists and both RGBA16F ping-pong
/// targets (x = occlusion, y = view distance);
/// the host parents it between its depth-producing pass and the pass that
/// consumes [`SsaoPass::output`].
#[derive(Default)]
pub struct SsaoPass {
    stages: Vec<SsaoStage>,
    /// raw target, and the final blurred output (the v blur writes back).
    tex_a: Option<Texture>,
    /// h-blur intermediate.
    tex_b: Option<Texture>,
    draw_raw: Option<Box<DrawSsaoRaw>>,
    draw_blur: Option<Box<DrawSsaoBlur>>,
    /// Latest GPU duration per stage (raw, blur h, blur v), milliseconds.
    /// Metal command-buffer times, arriving a frame or two behind.
    pub stage_gpu_ms: [f64; 3],
    /// Minimum observed per stage — the honest per-pass cost on a GPU that
    /// is also running other work: command-buffer wall time includes the
    /// neighbours' timeslices, and the floor is what the pass itself costs.
    pub stage_gpu_min_ms: [f64; 3],
    pub gpu_samples: u64,
}

const STAGE_NAMES: [&str; 3] = ["ssao raw", "ssao blur h", "ssao blur v"];

impl SsaoPass {
    /// Create passes, targets and draw structs. Returns false while the
    /// script VM is held elsewhere (try again next frame) — the pbr-lane
    /// convention for lazily built draw structs.
    pub fn ensure(&mut self, cx: &mut Cx) -> bool {
        while self.stages.len() < 3 {
            let name = STAGE_NAMES[self.stages.len()];
            let pass = DrawPass::new_with_name(cx, name);
            pass.set_gpu_timing_enabled(cx, true);
            self.stages.push(SsaoStage {
                pass,
                list: DrawList::new(cx),
            });
        }
        let rgba16f = |cx: &mut Cx| {
            Texture::new_with_format(
                cx,
                TextureFormat::RenderRGBAf16 {
                    size: TextureSize::Auto,
                    initial: true,
                },
            )
        };
        if self.tex_a.is_none() {
            self.tex_a = Some(rgba16f(cx));
        }
        if self.tex_b.is_none() {
            self.tex_b = Some(rgba16f(cx));
        }
        if self.draw_raw.is_none() {
            self.draw_raw = cx.try_with_vm(|vm| Box::new(DrawSsaoRaw::script_new_with_default(vm)));
        }
        if self.draw_blur.is_none() {
            self.draw_blur =
                cx.try_with_vm(|vm| Box::new(DrawSsaoBlur::script_new_with_default(vm)));
        }
        self.draw_raw.is_some() && self.draw_blur.is_some()
    }

    /// The deepest pass of the chain — the host parents its depth-producing
    /// pass under this, so the chain runs after it.
    pub fn first_pass_id(&self) -> Option<DrawPassId> {
        self.stages.first().map(|s| s.pass.draw_pass_id())
    }

    /// The blurred occlusion target (RGBA16F; x = occlusion with 1 =
    /// unoccluded, y = view distance). Valid after [`SsaoPass::run`] in the
    /// same frame.
    pub fn output(&self) -> Option<&Texture> {
        self.tex_a.as_ref()
    }

    /// Sum of the latest per-stage GPU times, ms.
    pub fn gpu_ms(&self) -> f64 {
        self.stage_gpu_ms.iter().sum()
    }

    /// Sum of the minimum per-stage GPU times, ms (the uncontended floor).
    pub fn gpu_min_ms(&self) -> f64 {
        self.stage_gpu_min_ms.iter().sum()
    }

    /// Record the three passes, parenting them raw → h → v → `parent` so
    /// the sort runs them between the depth producer and the consumer.
    /// `depth_src`'s red channel is view distance in metres (0 = none).
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        cx: &mut Cx2d,
        size: DVec2,
        depth_src: &Texture,
        proj: SsaoProjection,
        params: SsaoParams,
        parent: DrawPassId,
    ) {
        if !self.ensure(cx.cx) {
            return;
        }
        let tex_a = self.tex_a.clone().unwrap();
        let tex_b = self.tex_b.clone().unwrap();
        let dpi = cx.current_dpi_factor() as f32;
        let px = vec2(
            (size.x as f32 * dpi).max(1.0),
            (size.y as f32 * dpi).max(1.0),
        );

        let run_stage = |cx: &mut Cx2d,
                             stages: &mut [SsaoStage],
                             i: usize,
                             target: &Texture,
                             draw: &mut dyn FnMut(&mut Cx2d, Rect)| {
            let parent_id = if i + 1 < stages.len() {
                stages[i + 1].pass.draw_pass_id()
            } else {
                parent
            };
            {
                let st = &mut stages[i];
                st.pass.set_size(cx, size);
                st.pass.clear_color_textures(cx.cx);
                st.pass
                    .set_color_texture(cx, target, DrawPassClearColor::ClearWith(vec4(1.0, 1.0, 1.0, 1.0)));
                let id = st.pass.draw_pass_id();
                cx.cx.passes[id].depth_texture = None;
                cx.cx.passes[id].parent = CxDrawPassParent::DrawPass(parent_id);
                cx.begin_pass(&st.pass, None);
                st.pass.set_size(cx, size);
                st.list.begin_always(cx);
            }
            let pass_size = cx.current_pass_size();
            cx.begin_root_turtle(pass_size, Layout::flow_overlay());
            draw(
                cx,
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size,
                },
            );
            cx.end_pass_sized_turtle();
            {
                let st = &mut stages[i];
                st.list.end(cx);
                cx.end_pass(&st.pass);
            }
        };

        // ---- raw ----------------------------------------------------------
        {
            let draw_raw = self.draw_raw.as_mut().unwrap();
            let dv = &mut draw_raw.draw_super.draw_vars;
            dv.set_texture(0, depth_src);
            dv.set_uniform(
                cx.cx,
                live_id!(u_texel),
                &[1.0 / px.x, 1.0 / px.y, px.x, px.y],
            );
            dv.set_uniform(
                cx.cx,
                live_id!(u_proj),
                &[
                    proj.half_x,
                    proj.half_y,
                    if proj.ortho { 1.0 } else { 0.0 },
                    0.0,
                ],
            );
            dv.set_uniform(
                cx.cx,
                live_id!(u_ao),
                &[params.radius, params.bias, params.bias_slope, 0.0],
            );
            run_stage(cx, &mut self.stages, 0, &tex_a, &mut |cx, r| {
                draw_raw.draw_abs(cx, r)
            });
        }

        // ---- blur h / v ---------------------------------------------------
        for (i, (src, dst, dir)) in [
            (&tex_a, &tex_b, [1.0 / px.x, 0.0]),
            (&tex_b, &tex_a, [0.0, 1.0 / px.y]),
        ]
        .into_iter()
        .enumerate()
        {
            let draw_blur = self.draw_blur.as_mut().unwrap();
            let dv = &mut draw_blur.draw_super.draw_vars;
            dv.set_texture(0, src);
            dv.set_uniform(cx.cx, live_id!(u_dir), &[dir[0], dir[1], 0.0, 0.0]);
            run_stage(cx, &mut self.stages, 1 + i, dst, &mut |cx, r| {
                draw_blur.draw_abs(cx, r)
            });
        }

        // ---- timing (async, a frame or two behind) ------------------------
        for (i, st) in self.stages.iter().enumerate() {
            for ms in st.pass.take_gpu_times_ms(cx.cx) {
                self.stage_gpu_ms[i] = ms;
                let m = &mut self.stage_gpu_min_ms[i];
                if *m <= 0.0 || ms < *m {
                    *m = ms;
                }
                self.gpu_samples = self.gpu_samples.saturating_add(1);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The tap laws, in Rust, so the shader's occlusion expression is pinned by
// tests. `tap_occlusion` mirrors the raw pass verbatim: smoothstep over
// `ahead` (how far the occluder stands in front of the sample point) gated
// by the range window on the centre-to-occluder distance.
// ---------------------------------------------------------------------------

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One tap's contribution. `ahead` = sample-point distance minus occluder
/// distance (metres); `center_gap` = |centre depth − occluder depth|.
pub fn tap_occlusion(ahead: f32, center_gap: f32, params: &SsaoParams, view_dist: f32) -> f32 {
    let bias = params.bias + view_dist * params.bias_slope;
    let range = 1.0 - smoothstep(params.radius, params.radius * 2.0, center_gap);
    smoothstep(bias, bias * 2.0, ahead) * range
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Law 1: coincident construction layers, millimetres apart, must never
    /// occlude one another — at any view distance a coplanar layer's
    /// `ahead` is its physical gap, and the bias floor stands above it.
    #[test]
    fn coincident_layers_never_occlude() {
        let p = SsaoParams::default();
        for gap_mm in [0.0f32, 1.0, 2.0, 3.0, 5.0, 8.0] {
            for dist in [1.0f32, 5.0, 20.0, 80.0] {
                let occ = tap_occlusion(gap_mm * 0.001, gap_mm * 0.001, &p, dist);
                assert_eq!(
                    occ, 0.0,
                    "a {gap_mm} mm layer gap at {dist} m must not occlude"
                );
            }
        }
    }

    /// A real ledge a decimetre in front occludes fully.
    #[test]
    fn a_real_ledge_occludes() {
        let p = SsaoParams::default();
        let occ = tap_occlusion(0.1, 0.1, &p, 3.0);
        assert!(occ > 0.95, "a 10 cm ledge at 3 m reads {occ}");
    }

    /// Law 4: an occluder far in front of the surface is a different
    /// object, and contributes nothing.
    #[test]
    fn distant_occluders_contribute_nothing() {
        let p = SsaoParams::default();
        let occ = tap_occlusion(1.0, 1.0, &p, 10.0);
        assert_eq!(occ, 0.0, "an occluder a metre in front must not shade");
    }

    /// The defaults ARE the law: the bias floor stays above coplanar gaps
    /// and the radius stays a world measure.
    #[test]
    fn default_params_pin_the_laws() {
        let p = SsaoParams::default();
        assert!(p.bias >= 0.01, "bias floor {} under the mm law", p.bias);
        assert!(
            (0.1..=1.0).contains(&p.radius),
            "radius {} is not doorstep-scale",
            p.radius
        );
        assert!(p.strength <= 0.8, "default strength {} immodest", p.strength);
    }
}
