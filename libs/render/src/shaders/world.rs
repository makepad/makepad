//! Foliage, shadow meshes, SDF shadow quads and terrain.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // Generated foliage: the OPT-IN variant that adds growth and wind.
    //
    // Deliberately a sibling of DrawSceneSkinned rather than a flag inside it.
    // Wind costs ~20 vertex ALU and growth ~6; the cube shader draws most of
    // the world and must not pay either. A plant opts in by being drawn with
    // this shader; everything else keeps the cheap path untouched.
    //
    // Both animation weights ride in ONE unorm8 lane (the colour's alpha, high
    // nibble = growth order, low = wind flex), so the variant costs zero extra
    // vertex BYTES over the shared 24-byte layout — which is the bottleneck we
    // actually measured.
    mod.draw.DrawSceneFoliage = mod.std.set_type_default() do #(DrawSceneFoliage::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertex, geom.GameMeshGeom)
        v_ambient: varying(vec3f)
        v_direct: varying(vec3f)
        v_color: varying(vec3f)
        world: varying(vec4f)
        v_fog: varying(float)

        oct_decode: fn(e: vec2f) -> vec3f {
            let nz = 1.0 - abs(e.x) - abs(e.y)
            let t = max(0.0 - nz, 0.0)
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return normalize(vec3(e.x - t * sx, e.y - t * sy, nz))
        }

        vertex: fn() {
            let pos = vec3(self.geom.px, self.geom.py, self.geom.pz)
            let rgba = unpack4u8(self.geom.color)
            // High nibble = growth order along the skeleton, low = wind flex.
            let packed = floor(rgba.w * 255.0 + 0.5)
            let growth_t = floor(packed / 16.0) / 15.0
            let flex = (packed - floor(packed / 16.0) * 16.0) / 15.0

            // Growth reveal: each vertex has its own threshold, so the plant
            // unfurls root-first instead of scaling up as a whole. The band
            // hides the 16-level quantisation of growth_t.
            let reveal = smoothstep(growth_t - self.growth_band, growth_t, self.growth)
            let grown = pos * reveal

            // Per-instance phase from the instance's world position, so a
            // forest sways individually rather than in lockstep — this is the
            // detail that makes it read as wind rather than a global wobble.
            let origin = (self.transform * vec4(0.0, 0.0, 0.0, 1.0)).xyz
            let phase = origin.x * 0.7 + origin.z * 1.3
            let t = self.wind_time
            // Two frequencies: a slow sway plus a faster flutter.
            let sway = sin(t * 1.1 + phase) * self.wind_strength
            let flutter = sin(t * 3.7 + phase * 1.7) * self.wind_gust
            // Clamped so a strong gust bends the plant instead of shearing it.
            let amount = clamp((sway + flutter) * flex, 0.0 - 0.6, 0.6)
            let bent = grown + self.wind_dir * amount

            let normal_in = self.oct_decode(unpack2f16(self.geom.nrm))
            let model_view = self.draw_list.view_transform * self.transform
            let world_normal = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            self.world = model_view * vec4(bent.x, bent.y, bent.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            // Two-sided foliage: cards are lit by the absolute facing so a
            // leaf seen from behind is not black.
            let dp = abs(dot(world_normal, normalize(self.light_dir)))
            let hemi = clamp(world_normal.y * 0.5 + 0.5, 0.0, 1.0)
            self.v_ambient = mix(self.sun_ground, self.sun_sky, hemi)
            self.v_direct = self.sun_color * dp
            self.v_color = rgba.xyz
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            let lit = self.v_color * (self.v_ambient + self.v_direct)
            return vec4(mix(lit, self.fog_color, self.v_fog), 1.0)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // Silhouette shadow mesh (shadow_mesh.rs): every caster's hull for the
    // whole frame, in ONE geometry and ONE draw call.
    //
    // Z-fighting is handled structurally rather than by tuning:
    //   * geometry is offset along the RECEIVER's normal on the CPU, with a
    //     slope-scaled term (world-up would slide the shadow along a slope),
    //   * `depth_write: false` — shadows never occlude each other or anything
    //     else, so overlapping casters cannot fight for the depth buffer,
    //   * depth TEST stays on, so a shadow is still hidden by geometry in
    //     front of it.
    // Per-vertex alpha (colour.w) gives the soft rim for free.
    // Shadow + contact-AO geometry, draped on whatever it lands on.
    mod.draw.DrawSceneShadow = mod.std.set_type_default() do #(DrawSceneShadow::script_shader(vm)){
        alpha_blend: true
        depth_write: false
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertex, geom.GameMeshGeom)
        v_alpha: varying(float)
        world: varying(vec4f)

        vertex: fn() {
            // Packed layout: 6 f32 slots instead of PbrVertex's 16. A shadow
            // needs a position and a coverage value, nothing else.
            let pos = vec3(self.geom.px, self.geom.py, self.geom.pz)
            self.world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_alpha = unpack4u8(self.geom.color).w
            let view_pos = self.draw_pass.camera_view * self.world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            // SHADOW DEBUG: saturated magenta at boosted alpha. Overlapping
            // geometry compounds toward white-pink and sliver triangles are
            // unmistakable — structure a black-on-ground shadow hides.
            if self.shadow_debug > 0.5 {
                return vec4(1.0, 0.0, 0.6, clamp(self.v_alpha * 2.0, 0.0, 1.0))
            }
            // Premultiplied black: RGB 0 leaves exactly ground*(1-a), a true
            // multiplicative shadow. Unpremultiplied dark RGB would ADD light.
            return vec4(0.0, 0.0, 0.0, self.v_alpha * self.shadow_scale)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // SDF silhouette shadow — THE dynamic shadow shader: ONE ground-aligned
    // quad per caster (character or driven car); the PIXEL stage samples
    // the caster's baked silhouette-SDF atlas (shadow_sdf.rs — 16 relative
    // yaws x (idle + 8 walk phases) of 32x32 R8 distance cells; rigid
    // models carry one yaw row) at the 2 yaw-neighbour x 2 phase-neighbour
    // cells and LERPS THE DISTANCES. Lerping distances MORPHS the
    // silhouette between poses — a sprite crossfade would double-expose a
    // mid-stride walker into four ghost legs; the moving iso-line cannot.
    // The atlas is baked against a canonical light azimuth, so the quad
    // rotates the sprite into the owning light's world frame (instance
    // axis) and one atlas serves the sun from any direction and any lamp.
    // Edge width widens with distance toward the shadow tip — the far
    // texels were cast by high body parts, which is contact hardening for
    // free. Blend/depth conventions match DrawSceneShadow: premultiplied
    // dark, depth-tested, never depth-written, receiver lift on the CPU.
    mod.draw.DrawSceneShadowSdf = mod.std.set_type_default() do #(DrawSceneShadowSdf::script_shader(vm)){
        alpha_blend: true
        depth_write: false
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        // The shared one-quad sheet (flare geometry): geom_pos.xy is the
        // corner in -0.5..0.5.
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        sdf_tex: texture_2d(float)
        world: varying(vec4f)
        // Cell-local uv (0..1 across the sprite window) and the fragment's
        // window-local coordinates in sprite units (for contact hardening).
        v_uv: varying(vec2f)
        v_local: varying(vec2f)

        vertex: fn() {
            let corner = self.geom.geom_pos.xy + vec2(0.5, 0.5)
            self.v_uv = corner
            // Window-local position in unscaled sprite units.
            let local = self.sdf_d.xy + corner * self.sdf_d.zw
            self.v_local = local
            // Rotate into the owning light's frame: local +x axis = the
            // horizontal direction TOWARD the light, so the baked shadow
            // (which extends toward -x) lands away from it. perp is the
            // frame's +z image; scale is footprint x anchor compression.
            let axis = self.sdf_b.xy
            let perp = vec2(0.0 - axis.y, axis.x)
            let s = self.sdf_b.z
            let xz = self.sdf_a.xz + axis * (local.x * s) + perp * (local.y * s)
            self.world = self.draw_list.view_transform
                * vec4(xz.x, self.sdf_a.y + self.sdf_a.w, xz.y, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            return self.vertex_pos
        }

        // One atlas cell tap: bilinear WITHIN the cell (uv clamped half a
        // texel inside so neighbouring cells never bleed), returning the
        // encoded distance (0.5 = silhouette boundary, > 0.5 outside).
        cell_d: fn(k: float, row: float) -> float {
            let dim = self.sdf_tex.size()
            let t = clamp(self.v_uv * 32.0, vec2(0.5, 0.5), vec2(31.5, 31.5))
            let uv = (vec2(k, row) * 32.0 + t) / dim
            return self.sdf_tex.sample(uv).x
        }

        // Yaw-pair lerped distance of one pose row.
        yaw_d: fn(row: float, k0: float, k1: float, kf: float) -> float {
            return mix(self.cell_d(k0, row), self.cell_d(k1, row), kf)
        }

        pixel: fn() {
            // Relative yaw -> two stations + blend. The wrap test must see
            // k0 + 1: k0 itself tops out at exactly 15, so step(15.5, k0)
            // never fired and the last sector lerped toward cell 16 — off
            // the atlas' right edge, where the clamp-to-edge sampler reads
            // the border padding ("far outside") and the shadow faded to
            // nothing across one 22.5-degree heading band. Same pattern as
            // the phase wrap below: compare the SUCCESSOR against the last
            // valid index + 0.5.
            let station = fract(self.sdf_c.x / 6.2831855) * 16.0
            let k0 = floor(station)
            let kf = station - k0
            let k1 = k0 + 1.0 - 16.0 * step(15.5, k0 + 1.0)
            // Idle row, then the walk-phase pair (rows 1..rows-1, wrapping)
            // mixed in by the gait blend — THE DISTANCES are what lerp, at
            // every step.
            var d = self.yaw_d(0.0, k0, k1, kf)
            let rows = self.sdf_c.w
            let blend = self.sdf_c.z
            if blend > 0.001 {
                let g = rows - 1.0
                if g > 0.5 {
                    let pp = fract(self.sdf_c.y) * g
                    let p0 = floor(pp)
                    let pf = pp - p0
                    let p1 = p0 + 1.0 - g * step(g - 0.5, p0 + 1.0)
                    let dw = mix(
                        self.yaw_d(1.0 + p0, k0, k1, kf),
                        self.yaw_d(1.0 + p1, k0, k1, kf),
                        pf
                    )
                    d = mix(d, dw, blend)
                }
            }
            // Contact hardening: widen the edge band with distance toward
            // the shadow tip (cast by high sources). w is in encoded-d
            // units, precomputed by the CPU from the atlas band.
            let w = self.sdf_e.x + self.sdf_e.y * max(0.0 - self.v_local.x, 0.0)
            let a = 1.0 - smoothstep(0.5 - w, 0.5 + w, d)
            if self.shadow_debug > 0.5 {
                return vec4(1.0, 0.0, 0.6, clamp(a * 2.0, 0.0, 1.0))
            }
            // Premultiplied black — the shadow layer's multiplicative
            // convention (see DrawSceneShadow).
            return vec4(0.0, 0.0, 0.0, a * self.sdf_b.w * self.shadow_scale)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // The smooth terrain mesh: per-vertex colored triangles, flat normals.
    mod.draw.DrawSceneTerrain = mod.std.set_type_default() do #(DrawSceneTerrain::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)
        // The scene's baked-light atlas: A = sun-visibility SDF, RGB = lamps.
        light_map: texture_2d(float)
        // Realtime cascades (see DrawSceneCube's block — same contract).
        csm_map: texture_depth(float)
        // Ambient and direct split into separate varyings so the PIXEL stage
        // can gate the direct term by the sampled sun SDF — folded together
        // (the old lit_color) there is nothing left to gate.
        lit_color: varying(vec4f)
        v_direct_col: varying(vec3f)
        v_albedo: varying(vec3f)
        v_lm_uv: varying(vec2f)
        v_lm_in: varying(float)
        world: varying(vec4f)
        v_fog: varying(float)
        // Per-frame TRANSIENT lights only (firework flashes, host lights) —
        // street lamps are baked into the atlas RGB, adding them here would
        // double-light the ground. PIXEL stage: terrain vertices are coarse,
        // a vertex-lit flash pops whole cells (see DrawSceneCube).
        dl_pos0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        v_dl_pos: varying(vec3f)
        v_dl_nrm: varying(vec3f),

        ..mod.draw.ClusteredLighting,
        ..mod.draw.FastGiSampling,

        dl_term: fn(wp: vec3, n: vec3, lp: vec4, lc: vec4) -> vec3 {
            if lp.w <= 0.0 {
                return vec3(0.0, 0.0, 0.0)
            }
            let l = lp.xyz - wp
            let d = max(length(l), 0.0001)
            if d >= lp.w {
                return vec3(0.0, 0.0, 0.0)
            }
            let att = 1.0 - d / lp.w
            let ndl = max(dot(n, l * (1.0 / d)), 0.0)
            let cone = clamp((l.y * (1.0 / d) + 0.35) / 1.35, 0.0, 1.0)
            let s = ndl * att * att * (cone * cone * lc.w + (1.0 - lc.w))
            return lc.xyz * s
        }

        dl_sum: fn(wp: vec3, n: vec3) -> vec3 {
            if self.cluster_on > 0.5 { return vec3(0.0, 0.0, 0.0) }
            var dl = vec3(0.0, 0.0, 0.0)
            // The eight slots in one loop: one dl_term call site (a D3D
            // compile inlines every site).
            var k = 0.0
            while k < 7.5 {
                var lp = self.dl_pos0
                var lc = self.dl_col0
                if k > 0.5 {
                    lp = self.dl_pos1
                    lc = self.dl_col1
                }
                if k > 1.5 {
                    lp = self.dl_pos2
                    lc = self.dl_col2
                }
                if k > 2.5 {
                    lp = self.dl_pos3
                    lc = self.dl_col3
                }
                if k > 3.5 {
                    lp = self.dl_pos4
                    lc = self.dl_col4
                }
                if k > 4.5 {
                    lp = self.dl_pos5
                    lc = self.dl_col5
                }
                if k > 5.5 {
                    lp = self.dl_pos6
                    lc = self.dl_col6
                }
                if k > 6.5 {
                    lp = self.dl_pos7
                    lc = self.dl_col7
                }
                dl = dl + self.dl_term(wp, n, lp, lc)
                k = k + 1.0
            }
            return dl
        }

        // Sun visibility with a lamp pool's fill of its own shadow folded in.
        // See lightmap::lamp_shadow_fill for the law and why it cannot blow
        // anything out; 0.180 is lightmap::LM_LAMP_SHADOW_FILL_AT, the pool
        // strength at which the fill is complete.
        sun_filled: fn(sun_vis: float, local: vec3) -> float {
            if self.cluster_on > 0.5 { return sun_vis }
            let fill = clamp(max(max(local.x, local.y), local.z) / 0.180, 0.0, 1.0)
            return sun_vis + (1.0 - sun_vis) * fill
        },

        ..mod.draw.SunCascades,

        vertex: fn() {
            let pos = vec3(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z)
            let normal_in = vec3(self.geom.pos_nx.w, self.geom.ny_nz_uv.x, self.geom.ny_nz_uv.y)
            let model_view = self.draw_list.view_transform * self.transform
            let world_normal = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(world_normal, normalize(self.light_dir)), 0.0)
            let hemi = clamp(world_normal.y * 0.5 + 0.5, 0.0, 1.0)
            let ambient = mix(self.sun_ground, self.sun_sky, hemi)
            let albedo = self.to_lin(self.geom.color.xyz)
            self.lit_color = vec4(albedo * ambient, self.geom.color.w)
            self.v_direct_col = albedo * (self.sun_color * dp)
            self.v_albedo = albedo
            // A heightfield is its own lightmap parameterisation: uv comes
            // straight from the MESH's world xz (pre-view — the stage
            // transform must not move the map), remapped into the terrain's
            // atlas window.
            let lw = max(self.lm_world.zw, vec2(0.000001, 0.000001))
            let lraw = (pos.xz - self.lm_world.xy) / lw
            let lf = clamp(lraw, vec2(0.0, 0.0), vec2(1.0, 1.0))
            self.v_lm_uv = self.lm_rect.xy + lf * self.lm_rect.zw
            // Terrain past the field's rect is fully lit — the field covers
            // the statics, not the whole map.
            self.v_lm_in = step(0.0, lraw.x) * step(lraw.x, 1.0)
                * step(0.0, lraw.y) * step(lraw.y, 1.0)
            // TRUE world position + normal for the pixel-stage transient
            // lights (pre view/stage — the terrain transform is identity in
            // practice, but stay principled). Vertex normals are unit and
            // terrain is smooth, so the interpolated normal is close enough
            // to skip a per-fragment renormalize.
            self.v_dl_pos = (self.transform * vec4(pos.x, pos.y, pos.z, 1.0)).xyz
            self.v_dl_nrm = (self.transform * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            let lm = self.light_map.sample(self.v_lm_uv)
            let has_lm = step(0.000001, self.lm_rect.z) * self.v_lm_in
            // Realtime: the cascades replace the baked A channel.
            let ndl_t = max(dot(normalize(self.v_dl_nrm), normalize(self.light_dir)), 0.0)
            let sun_vis = mix(
                mix(1.0, smoothstep(0.2, 0.8, lm.w), has_lm),
                self.csm_vis(self.v_dl_pos, self.v_dl_nrm, ndl_t),
                self.csm_p.x
            )
            // 0.9 = lightmap::LM_LAMP_CEIL — the atlas RGB decode.
            let lamps = lm.xyz * (0.9 * has_lm) * (1.0 - self.cluster_on)
            // Local light reaches the ground WITHOUT the sun's shadow term,
            // and over its bright core a pool fills that shadow back in —
            // this is the surface a street lamp's own pole shadow lands on,
            // so it is where the fill has to read: lightmap::lamp_shadow_fill.
            let dl = self.dl_sum(self.v_dl_pos, self.v_dl_nrm)
            let local = lamps + dl + self.cluster_sum(self.v_dl_pos, self.v_dl_nrm)
            let sun_lit = self.sun_filled(sun_vis, local)
            if self.lm_debug > 0.5 {
                return vec4(
                    mix(vec3(0.6, 0.1, 0.1), vec3(0.1, 0.6, 0.1), sun_lit) + lamps,
                    1.0
                )
            }
            let ambient=mix(self.sun_ground,self.sun_sky,clamp(self.v_dl_nrm.y*0.5+0.5,0.0,1.0))
            let c = self.lit_color.xyz + self.v_albedo*(self.gi_ambient(self.v_dl_pos,self.v_dl_nrm,ambient)-ambient) + self.v_direct_col * sun_lit
                + self.v_albedo * local
            return self.csm_debug_view(self.gi_display(vec4(self.scene_fogged(c, self.v_fog, self.v_dl_pos, self.fog_density), self.lit_color.w),self.v_dl_pos,self.v_dl_nrm),self.v_dl_pos,self.v_dl_nrm)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // ------------------------------------------------------------------
    // GPU lightmap baker passes (gpu_lightmap.rs). Every shader below is a
    // BAKE pass, never a scene pass: they render into scratch targets and
    // the light atlas itself, and the material shaders above consume the
    // result unchanged. Coordinate conventions used throughout:
    //   * target position: uv in [0,1] with (0,0) the TOP-LEFT texel, so
    //     clip = (u*2-1, 1-2v). Sampling the produced texture with the same
    //     uv reads the texel that was written.
    //   * sun cameras are three row vec4s (rx, ry, rz): row.xyz dotted with
    //     a world point + row.w gives ndc x / ndc y / z01 directly.
    //   * depth scratches are R32F (`color_format: @Rf32`), sampled with
    //     sample_nearest — a shadow test is one exact texel read, never a
    //     filtered one.
}
