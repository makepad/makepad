//! DrawSceneSkinned: the textured model lane (skinned characters and static props).

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // Skinned character mesh: PbrVertex stream (CPU-skinned per frame, uv in
    // ny_nz_uv.zw), textured, lit and fogged like the terrain.
    mod.draw.DrawSceneSkinned = mod.std.set_type_default() do #(DrawSceneSkinned::script_shader(vm)){
        ..mod.draw.SceneFurSurface,
        ..mod.draw.SceneColorAdjust,
        alpha_blend: false
        // Imported model layers may be deliberate sheets (roof soffits,
        // glazing, CAD faces). The shadow depth path is already two-sided;
        // the receiver path must show and light the same geometry.
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexAo, geom.GameMeshAoGeom)
        tex: texture_2d(float)
        // Baked occlusion for this pack, sampled per FRAGMENT. Per vertex it
        // would carry exactly as much information as a vertex bake, which is
        // the thing the atlas exists to escape.
        ao_map: texture_2d(float)
        // The scene's baked-light atlas (lightmap.rs): A = sun-visibility
        // SDF, RGB = lamp light / lightmap::LM_LAMP_CEIL. Every static draw
        // binds it (a 1x1 "fully lit" stand-in before the first bake
        // delivers).
        light_map: texture_2d(float)
        // The ground field's shadow-top plane (R8, same uv as the ground
        // region): the ABSOLUTE height each shadowed texel's sun ray was
        // blocked at, decoded via lm_top_decode. Dynamics compare their
        // vertex height against it so a crate lifted above a fence rail's
        // shadow comes out of it (see DrawSceneCube).
        top_map: texture_2d(float)
        v_ao_uv: varying(vec2f)
        v_lm_uv: varying(vec2f)
        v_ambient: varying(vec3f)
        v_direct: varying(vec3f)
        v_uv: varying(vec2f)
        v_tint: varying(vec4f)
        world: varying(vec4f)
        v_fog: varying(float)
        // Per-frame dynamic lights, up to 8, summed in the VERTEX stage
        // (props carry enough vertices for that to read smoothly). Slot
        // layout from renderer.rs write_light_uniforms: TRANSIENT lights
        // (firework flashes, host lights) occupy slots [0, dl_split);
        // baked street lamps fill the rest. Statics (dl_apply = 0) sum only
        // the transient prefix — their lamp light is already in the baked
        // atlas and adding it again would double-light every facade —
        // while dynamic instances (dl_apply = 1) sum everything.
        dl_split: uniform(0.0)
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
        v_dl: varying(vec3f)
        // The GROUND region of the light atlas, for DYNAMIC instances only
        // (dl_apply gates it): a driven car crossing a house's shadow
        // darkens. A channel only — statics gate their sun through their
        // own chart region, and nobody reads the ground RGB here (lamps for
        // dynamics arrive analytically through dl_*).
        lm_ground_rect: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        lm_ground_world: uniform(vec4(0.0, 0.0, 1.0, 1.0))
        // Decode for top_map: x = base world height, y = range; absolute
        // blocked height = x + byte * y.
        lm_top_decode: uniform(vec4(0.0, 8.0, 0.0, 0.0))
        // xy = ground-region uv, z = in-field gate, w = TRUE world height
        // of the vertex (for the shadow-top comparison).
        v_lmg: varying(vec4f)
        // Realtime cascades (see DrawSceneCube's block — same contract, same
        // uniforms, one receive path for every family). v_csm = (true world
        // position, N.L) for the pixel-stage compare.
        csm_map: texture_depth(float)
        // Q3 / Unreal detail overlay. Last texture so CSM stays slot 4.
        detail_map: texture_2d(float)
        // `detail_st` (the overlay's uv scale) and `prelit` (1 = COLOR_0 is a
        // baked lightmap, so the analytic sun must not multiply it again) are
        // the Rust struct's own #[live] instance fields — `script_shader`
        // already declares them. Re-declaring them here as `instance(...)`
        // applies a shader-descriptor OBJECT to a typed field, which fails
        // the apply: it is what logged the two `type mismatch for property`
        // errors every app on this shader printed at boot. See the HUD shader in makepad-game-hud
        // below for the same rule stated once.
        // ---- per-element lookup (CAD hosts) — slot 6, OFF by default ----
        // A viewer that hides, isolates or explodes PARTS of one merged
        // static model, every frame, without ever re-uploading its
        // geometry. `elem_ctl.w == 0` (the default, and what every game
        // path leaves it at) skips the whole block, so a model that does
        // not opt in renders byte-identically to before this lane.
        //
        // When it is ON, the `ao_uv` lane is NOT an atlas coordinate but a
        // 32-bit element index (unorm16x2, lo in xy / hi in zw), and
        // `elem_map` holds one RGBA texel per element:
        //   r = visible (< 0.5 collapses the vertex to the model origin, so
        //       the whole triangle is zero-area and rasterises nothing)
        //   gba = a model-space offset added to the vertex (exploded views)
        // `elem_ctl` = (map width, map height, unused, enable).
        // The host writes both with `DrawVars::set_texture(6, ..)` /
        // `set_uniform(live_id!(elem_ctl), ..)` on its own draw struct.
        elem_map: texture_2d(float)
        elem_ctl: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // ---- screen-space AO (ssao.rs) — slot 7, OFF by default ----
        // A host that runs `SsaoPass` hands its blurred output to
        // `Renderer::set_ssao`, which binds it here for BOTH model lanes.
        // The factor multiplies the AMBIENT fill only — never the direct
        // sun, never the shadow-mapped light — so a sunlit facade keeps its
        // brightness while its creases darken. `ssao_ctl.x` is the strength;
        // 0 (the default, and what every game path leaves it at) skips the
        // sample, so a scene that does not opt in shades exactly as before.
        ssao_map: texture_2d(float)
        ssao_ctl: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // ---- follow-camera occluder fade (occluder_fade.rs), OFF by default ----
        // occ_eye = the TRUE world eye; occ_focus = the filmed body's chest,
        // w = the clear length before it (0 = off, every other host).
        // occ_eye.w > 0: swaying foliage within that many metres of the
        // followed body (occ_focus.xyz, about 1 m over its feet) bends away
        // (occluder_fade.rs set_foliage_push); the uniform block is full,
        // so the push rides these two.
        // Pixels near the eye→focus line, or right at the lens, draw
        // screen-door dithered so nothing hides the player.
        occ_eye: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        occ_focus: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // The fragment's own clip position, for the screen-space AO lookup.
        v_spos: varying(vec4f)
        // Which SPACE this lane shades in. 0 (the default) is the game's:
        // sRGB texels times a display-referred sun rig, written raw. 1 is the
        // scene-referred lane — see `to_scene` / `to_display`.
        display: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        v_csm: varying(vec4f)
        v_csm_n: varying(vec3f),

        ..mod.draw.SunCascades,

        // One dynamic light at world point `wp`, world normal `n`.
        // Attenuation (1 - d/r)^2; the spot factor mirrors lightmap.rs's
        // lamp pass (SPILL = 0.35, squared, mixed by lc.w) with the
        // emission axis fixed straight DOWN — the street-lamp convention.
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

        // The 8-slot sum with the per-instance static gate: slot i counts
        // when the instance is dynamic (dl_apply = 1) OR i < dl_split.
        dl_sum_gated: fn(wp: vec3, n: vec3) -> vec3 {
            if self.cluster_on > 0.5 { return vec3(0.0, 0.0, 0.0) }
            var dl = vec3(0.0, 0.0, 0.0)
            let g = self.dl_apply
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
                    * clamp(g + step(k + 0.5, self.dl_split), 0.0, 1.0)
                k = k + 1.0
            }
            return dl
        }

        // Sun visibility with a lamp pool's fill of its own shadow folded in.
        // See lightmap::lamp_shadow_fill for the law and why it cannot blow
        // anything out; 0.180 is lightmap::LM_LAMP_SHADOW_FILL_AT, the pool
        // strength at which the fill is complete. Inherited by DrawScenePbr.
        sun_filled: fn(sun_vis: float, local: vec3) -> float {
            if self.cluster_on > 0.5 { return sun_vis }
            let fill = clamp(max(max(local.x, local.y), local.z) / 0.180, 0.0, 1.0)
            return sun_vis + (1.0 - sun_vis) * fill
        }

        // ---- the two ends of the scene-referred lane (display.x = 1) -----
        //
        // A GAME shades in display space and it is right to: its texels are
        // sRGB bytes used as-is, its sun rig sums to one for a fully lit white
        // surface, and the product goes straight into an 8-bit target. Off
        // (display.x = 0) both of these are the identity and that lane is
        // untouched, bit for bit.
        //
        // A host that shades PHYSICALLY needs the other convention, and needs
        // BOTH ends of it or neither. `to_scene` decodes a texel to linear
        // reflectance so it can be multiplied by a cosine; `to_display` puts
        // the result back through the same ACES fit and 1/2.2 gamma the
        // analytic sky, the fog colour and the path tracer already finish
        // with. Multiplying light by an sRGB-encoded albedo and writing the
        // product raw is what crushes the midtones — the reason a fully
        // sunlit charcoal building still read as a silhouette against a sky
        // that WAS tone mapped.
        to_scene: fn(encoded: vec3) -> vec3 {
            if self.lin_ctl.x > 0.5 { return self.to_lin(encoded) }
            return mix(encoded, pow(encoded, vec3(2.2, 2.2, 2.2)), self.display.x)
        }

        to_display: fn(linear: vec3) -> vec3 {
            // The HDR lane writes scene-referred light: the composite maps it.
            if self.lin_ctl.x > 0.5 { return linear }
            let mapped = clamp(
                (linear * (linear * 2.51 + vec3(0.03, 0.03, 0.03)))
                    / (linear * (linear * 2.43 + vec3(0.59, 0.59, 0.59))
                        + vec3(0.14, 0.14, 0.14)),
                vec3(0.0, 0.0, 0.0),
                vec3(1.0, 1.0, 1.0)
            )
            return mix(
                linear,
                pow(mapped, vec3(0.4545454, 0.4545454, 0.4545454)),
                self.display.x
            )
        }

        // Octahedral decode: the inverse of skin.rs's oct_encode. Two f16
        // lanes carry a unit normal that would otherwise cost three floats.
        // sign is inlined rather than shared: the builtin sign() returns 0 at
        // 0, which would collapse the fold on an axis-aligned normal.
        oct_decode: fn(e: vec2f) -> vec3f {
            let nz = 1.0 - abs(e.x) - abs(e.y)
            let t = max(0.0 - nz, 0.0)
            // step(0,v)*2-1 is +1 for v>=0 and -1 for v<0, branchless and
            // without the sign() builtin's zero case.
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return normalize(vec3(e.x - t * sx, e.y - t * sy, nz))
        }

        // Foliage wind (a layer with makepadShading.wind). A swaying layer
        // never morphs, so it rides the morph lanes: morph_ctl.w = -1 turns
        // it on (morph_delta only reads w > 0.5) and morph_weights0 = (sway
        // metres at the top, 1 / model height, wind clock s, leaf flutter).
        // No new instance lane: the model stream is at the vertex-attribute
        // limit. One world-space breeze for the whole forest: a slow sway
        // plus a faster gust, both phased by the copy's position so
        // neighbours never move in lockstep, bending more toward the top
        // (flex = height^2), and a flutter on the leaves. The bend is taken
        // into model space through the copy's own axes, so a yawed or
        // scaled tree leans the same way as its neighbours.
        wind_bend: fn(pos: vec3) -> vec3 {
            let ax = (self.transform * vec4(1.0, 0.0, 0.0, 0.0)).xyz
            let ay = (self.transform * vec4(0.0, 1.0, 0.0, 0.0)).xyz
            let az = (self.transform * vec4(0.0, 0.0, 1.0, 0.0)).xyz
            let origin = (self.transform * vec4(0.0, 0.0, 0.0, 1.0)).xyz
            let phase = origin.x * 0.11 + origin.z * 0.07
            let w = self.morph_weights0
            let t = w.z
            let h = clamp(pos.y * w.y, 0.0, 1.4)
            let sway = (sin(t * 0.83 + phase) * 0.7 + sin(t * 2.1 + phase * 1.9) * 0.3 + 0.35) * h * h
            let flutter = sin(t * 6.3 + dot(pos, vec3(3.7, 2.3, 4.1)) + phase) * w.w * h
            let wd = vec3(0.8, 0.0, 0.6) * (sway * w.x)
            // Stepped-through grass and flowers lean away from the feet.
            var pd = vec3(0.0, 0.0, 0.0)
            if self.occ_eye.w > 0.0 {
                let wp = (self.transform * vec4(pos.x, pos.y, pos.z, 1.0)).xyz - self.occ_focus.xyz
                let pl = length(vec2(wp.x, wp.z))
                let pk = clamp(1.0 - pl / self.occ_eye.w, 0.0, 1.0) * (1.0 - smoothstep(1.5, 2.5, abs(wp.y + 1.0)))
                pd = vec3(wp.x, 0.0, wp.z) * (pk * pk * h * self.occ_eye.w * 0.7 / max(pl, 0.05)) - vec3(0.0, pk * pk * h * 0.25, 0.0)
            }
            let s2 = max(dot(ax, ax), 0.000001)
            let bend = vec3(dot(ax, wd + pd), dot(ay, wd + pd), dot(az, wd + pd)) / s2
            return pos + bend + vec3(flutter * 0.6, flutter * 0.4, flutter * 0.5) - vec3(0.0, abs(sway * w.x) * 0.12, 0.0)
        }

        vertex: fn() {
            var pos = vec3(self.geom.px, self.geom.py, self.geom.pz)
            if self.morph_ctl.w>0.5{pos=pos+self.morph_delta(self.geom.ao_uv,0.0)}
            if self.morph_ctl.w < -0.5 { pos = self.wind_bend(pos) }
            // ao_uv is unorm16x2 (model.rs pack_ao_uv), NOT an f16 pair — f16
            // spacing near 1.0 is a full texel of a 1024 atlas. Each axis is
            // (lo + 256*hi)/257 of the two unpacked bytes: 255*257 = 65535.
            let ao_uv_b = unpack4u8(self.geom.ao_uv)
            var chart_uv = vec2(
                (ao_uv_b.x + ao_uv_b.y * 256.0) / 257.0,
                (ao_uv_b.z + ao_uv_b.w * 256.0) / 257.0
            )
            // The CAD lookup reads the same lane as an element index. See
            // the `elem_map` declaration above; off unless the host enabled it.
            if self.elem_ctl.w > 0.5 {
                let ew = max(self.elem_ctl.x, 1.0)
                let eh = max(self.elem_ctl.y, 1.0)
                let lo = (ao_uv_b.x + ao_uv_b.y * 256.0) * 255.0
                let hi = (ao_uv_b.z + ao_uv_b.w * 256.0) * 255.0
                let ei = floor(lo + hi * 65536.0 + 0.5)
                let ex = (modf(ei, ew) + 0.5) / ew
                let ey = (floor(ei / ew) + 0.5) / eh
                let f = self.elem_map.sample_nearest(vec2(ex, ey), 0.0)
                if f.x < 0.5 {
                    pos = vec3(0.0, 0.0, 0.0)
                } else {
                    pos = pos + f.yzw
                }
                chart_uv = vec2(0.0, 0.0)
            }
            if self.morph_ctl.w>0.5{chart_uv=vec2(0.0,0.0)}
            self.v_ao_uv = chart_uv
            // The lightmap REUSES the chart parameterisation: this instance's
            // atlas window is one offset/scale over the same uv.
            self.v_lm_uv = self.lm_rect.xy + self.v_ao_uv * self.lm_rect.zw
            var normal_in = self.oct_decode(unpack2f16(self.geom.nrm))
            if self.morph_ctl.w>0.5{normal_in=normalize(normal_in+self.morph_delta(self.geom.ao_uv,1.0))}
            // The material `vertex` hook (render-material): identity here.
            pos = self.mat_vertex(pos, normal_in, unpack2f16(self.geom.uv))
            self.v_fur_root = vec3(self.geom.px, self.geom.py, self.geom.pz)
            self.v_fur_normal = self.oct_decode(unpack2f16(self.geom.nrm))
            pos = pos + normal_in * (self.fur.x * self.fur_layer.x)
            let model_view = self.draw_list.view_transform * self.transform
            let raw_world_normal = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            // Face-forward only the side the camera actually sees. This is
            // the two-sided material convention: an authored back face gets
            // the opposite normal, so N.L, hemisphere fill, CSM bias and the
            // PBR lobe all agree instead of an underside rendering black.
            let raw_view_normal = (self.draw_pass.camera_view
                * vec4(raw_world_normal.x, raw_world_normal.y, raw_world_normal.z, 0.0)).xyz
            var face_sign = 1.0
            if dot(raw_view_normal, view_pos.xyz) > 0.0 {
                face_sign = 0.0 - 1.0
            }
            let world_normal = raw_world_normal * face_sign
            let dp = max(dot(world_normal, normalize(self.light_dir)), 0.0)
            let hemi = clamp(world_normal.y * 0.5 + 0.5, 0.0, 1.0)
            self.v_ambient = mix(self.sun_ground, self.sun_sky, hemi)
            self.v_direct = self.sun_color * dp
            // Dynamic lights in TRUE world space (pre stage/view transform —
            // light positions are world coordinates, and the stage must not
            // move them).
            let dl_wp = (self.transform * vec4(pos.x, pos.y, pos.z, 1.0)).xyz
            let dl_n = normalize((self.transform * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
                * face_sign
            self.v_dl = self.dl_sum_gated(dl_wp, dl_n)
            // Ground-field sun shadow for DYNAMIC instances (dl_apply = 1).
            // The field stores GROUND-level visibility, so the sample is
            // projected ALONG THE SUN RAY from this vertex down to the
            // instance's ground plane — a vertex at height h is shadowed
            // iff the sun ray through it lands on shadowed ground. This is
            // what slants the boundary across a body correctly and stops a
            // wall's shadow at its feet from climbing the whole object.
            let dl_h = max(dl_wp.y - self.ground_y, 0.0)
            let dl_sun = normalize(self.light_dir)
            let dl_gxz = dl_wp.xz - dl_sun.xz * (dl_h / max(dl_sun.y, 0.2))
            let lgw = max(self.lm_ground_world.zw, vec2(0.000001, 0.000001))
            let lgraw = (dl_gxz - self.lm_ground_world.xy) / lgw
            let lgf = clamp(lgraw, vec2(0.0, 0.0), vec2(1.0, 1.0))
            let lg_in = self.dl_apply * step(0.000001, self.lm_ground_rect.z)
                * step(0.0, lgraw.x) * step(lgraw.x, 1.0)
                * step(0.0, lgraw.y) * step(lgraw.y, 1.0)
            let lg_uv = self.lm_ground_rect.xy + lgf * self.lm_ground_rect.zw
            self.v_lmg = vec4(lg_uv.x, lg_uv.y, lg_in, dl_wp.y)
            self.v_csm = vec4(dl_wp.x, dl_wp.y, dl_wp.z, dp)
            self.v_csm_n = dl_n
            self.v_uv = unpack2f16(self.geom.uv)
            // RGB is the material tint. Instance tint/hue is applied once the
            // complete texture × vertex albedo exists in the pixel stage.
            // Alpha carries baked self-AO from model.rs.
            let vc = unpack4u8(self.geom.color)
            self.v_tint = vc
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            // Depth-tie breaker: uniform view-space scale toward the camera.
            // The perspective divide cancels it in x/y (the image does not
            // move); only the stored depth shifts, so coplanar stacked
            // pieces resolve by placement order instead of z-fighting.
            let zk = 1.0 - self.depth_bias
            let clip_out = self.draw_pass.camera_projection
                * vec4(view_pos.x * zk, view_pos.y * zk, view_pos.z * zk, view_pos.w)
            self.v_spos = clip_out
            self.vertex_pos = clip_out
        }

        // Every discard of the model lanes goes through here. A discard
        // anywhere in a pipeline costs a tile GPU its hidden-surface removal
        // for everything that pipeline draws (Apple GPUs shade every covered
        // layer), so a draw that can never cut a pixel (opaque texture, no
        // dither, no occluder fade) goes through the same shader with this
        // emptied (`Renderer::opaque_shader`).
        clip: fn() { discard() }
        // The material `vertex` hook's stock body: the model-space position
        // unchanged. Custom materials replace it (custom_material.rs), and
        // their shadow casters run the same function.
        mat_vertex: fn(p: vec3, n: vec3, uv: vec2) -> vec3 { return p }

        // The base-colour texel. With `tex_mag.x` set (the material's glTF
        // sampler says magFilter NEAREST) a MAGNIFIED texel — larger than a
        // pixel on screen — is read at its centre, so it draws as a hard
        // square the way the classic games' software renderers drew it; a
        // minified one keeps the filtered, mipmapped read (no shimmer in the
        // distance). The uv is chosen first and sampled once, outside the
        // branch, so the sampler's own derivatives stay well defined.
        base_texel: fn() -> vec4 {
            var suv = self.v_uv
            if self.tex_mag.x > 1.5 {
                // A number plate (libs/model stencil_plate): the top half is
                // a 7-glyph plate (6-dot glyph pitch from dot 2 of 45), the
                // last three glyphs left blank for the number; the bottom
                // half is "0123456789" on the same plate (from dot 2 of 63).
                // Each copy's number rides color_adjust_ctl.w = -(1 + n); a
                // blank glyph cell reads its digit from the strip.
                let pu = clamp(self.v_uv.x, 0.0, 0.9999)
                let pv = clamp(self.v_uv.y, 0.0, 0.9999)
                suv = vec2(pu, pv * 0.5)
                let cell = (pu * 45.0 - 1.5) / 6.0
                let n = max(0.0 - self.color_adjust_ctl.w - 1.0, 0.0)
                if cell >= 4.0 && cell < 7.0 && self.color_adjust_ctl.w < -0.5 {
                    let k = floor(cell) - 4.0
                    let digit = floor(n / pow(10.0, 2.0 - k)) - floor(n / pow(10.0, 3.0 - k)) * 10.0
                    suv = vec2((1.5 + 6.0 * digit + 6.0 * fract(cell)) / 63.0, 0.5 + pv * 0.5)
                }
                return self.tex.sample(suv)
            }
            if self.tex_mag.x > 0.5 {
                let tsz = self.tex.size()
                let tuv = self.v_uv * tsz
                let foot = max(length(dFdx(tuv)), length(dFdy(tuv)))
                if foot < 1.0 {
                    suv = (floor(tuv) + vec2(0.5, 0.5)) / tsz
                }
            }
            return self.tex.sample_repeat(suv)
        }

        pixel: fn() {
            // Streamed-LOD crossfade (stream.rs `Dither`): a screen-space
            // noise window. 0 = no dither; else 1 + lo8 * 256 + hi8 and a
            // pixel shows only while lo <= noise < hi, so the two LODs of a
            // transition cover complementary pixels.
            if self.color_adjust_ctl.w > 0.5 {
                let dsp = self.v_spos.xy / max(self.v_spos.w, 0.000001)
                // A cell grid commensurate with no real target: two draws
                // that must cover complementary pixels read the same cell
                // (at 1920 cells a 2720-px target put every 17th column's
                // centre exactly on a cell edge, where the two meshes'
                // interpolated positions round apart: a hole in both).
                let dpx = floor(vec2(dsp.x * 0.5 + 0.5, 0.5 - dsp.y * 0.5) * vec2(1913.37, 1071.93))
                let dn = fract(52.9829189 * fract(dot(dpx, vec2(0.06711056, 0.00583715))))
                let dv = self.color_adjust_ctl.w - 1.0
                let dlo = floor(dv / 256.0)
                let dhi = dv - dlo * 256.0
                if dn < dlo / 255.0 || dn >= dhi / 255.0 { self.clip() }
            }
            if self.occ_focus.w > 0.0 {
                let oe = self.occ_eye.xyz
                let of = self.occ_focus.xyz - oe
                let ol = max(length(of), 0.001)
                let op = self.v_csm.xyz - oe
                let ot = clamp(dot(op, of) / ol, 0.0, ol - self.occ_focus.w)
                let orad = length(op - of * (ot / ol))
                let ocone = 0.55 + 0.22 * (ol - ot)
                let ofade = max((1.0 - smoothstep(ocone, ocone + 0.5, orad)) * step(0.0, ol - self.occ_focus.w - dot(op, of) / ol), 1.0 - clamp(length(op) / 1.2, 0.0, 1.0))
                let osp = self.v_spos.xy / max(self.v_spos.w, 0.000001)
                let opx = floor(vec2(osp.x * 0.5 + 0.5, 0.5 - osp.y * 0.5) * vec2(1920.0, 1080.0))
                if fract(52.9829189 * fract(dot(opx, vec2(0.06711056, 0.00583715)))) < ofade * 0.8 { self.clip() }
            }
            // Atlas x vertex tint. Kenney ships both conventions — most packs
            // UV-map into one colormap (tint = white), nature-kit and friends
            // carry no texture and colour per material (atlas = white 1x1).
            // Multiplying serves both without a branch or a second shader.
            // REPEAT + raw UVs (not fract): fract() wraps in software but
            // explodes screen-space derivatives at every tile seam, so the
            // GPU picks the tiniest mip and distant walls turn to noise.
            let tex = self.base_texel()
            // BUILD punch-through: palette 255 / magenta is the overlay key.
            let magenta = (tex.x > 0.75) && (tex.z > 0.75) && (tex.y < 0.22)
            if tex.w < 0.5 || magenta {
                self.clip()
            }
            let base = self.to_scene(vec3(tex.x, tex.y, tex.z))
            var albedo = self.color_adjust(
                base * self.to_lin(self.v_tint.xyz),
                self.tint,
                self.color_adjust_ctl
            )
            // Detail: blendFunc GL_DST_COLOR GL_SRC_COLOR = 2 * dest * src.
            // Mean-127 overlay is identity; far mips go gray and drop out.
            if self.detail_st.x > 0.001 {
                let det = self.detail_map.sample_repeat(self.v_uv * self.detail_st)
                albedo = vec3(albedo.x * det.x * 2.0, albedo.y * det.y * 2.0, albedo.z * det.z * 2.0)
            }
            // AO scales AMBIENT only. Ambient is light arriving from
            // everywhere, which is exactly what a crevice blocks; direct
            // sunlight is already zero where the surface faces away. Folding
            // it into both would darken a lit wall twice for the same reason.
            // Occlusion from the ATLAS when the pack has one, else from the
            // vertex lane. Both live in [AO_FLOOR, 1].
            // A copy with no pack atlas (ao_enabled 0) never reads it.
            var baked = 0.0
            if self.ao_enabled > 0.5 { baked = self.ao_map.sample(self.v_ao_uv).x }
            // Dithered: the atlas is 8-bit and magnified well past a texel per
            // pixel, so a shallow wall gradient otherwise lands as visible
            // bands of piecewise-linear bilinear. Hash noise anchored in WORLD
            // space (screen-anchored grain swims when the camera moves) at
            // ±1.5% breaks the bands without reading as dirt on flat colour.
            let hash = fract(
                sin(dot(self.world.xy + self.world.zz, vec2(12.9898, 78.233))) * 43758.5453
            )
            let ao = clamp(
                mix(self.v_tint.w, baked, self.ao_enabled) + (hash - 0.5) * 0.03,
                0.0, 1.0
            )
            // AO scales ambient FULLY and direct partially. Ambient-only is
            // the physically tidy answer and it is why the bake was invisible:
            // ambient is about a quarter of the light here, so even a properly
            // dark corner moved the pixel by a few percent. Letting occlusion
            // take some of the direct term too is what every stylised renderer
            // does, and it is what makes a crease read as a crease.
            // AO takes some of the DIRECT term too (a crease reads in sunlight);
            // the HDR lane keeps less of that stylisation — its real shadows
            // and fill already carry the contrast.
            let ao_direct = mix(1.0, ao, mix(0.75, 0.35, self.lin_ctl.x))
            // Screen-space AO (ssao.rs), composed with the baked term on the
            // AMBIENT fill only — same law as `ao` above, stricter split:
            // a crevice blocks sky light, not the sun, so the direct and
            // shadow-mapped terms never see this factor at all.
            var sao = 1.0
            if self.ssao_ctl.x > 0.001 {
                let sp = self.v_spos.xy / max(self.v_spos.w, 0.000001)
                let suv = vec2(sp.x * 0.5 + 0.5, 0.5 - sp.y * 0.5)
                sao = 1.0 - (1.0 - self.ssao_map.sample_nearest(suv).x) * self.ssao_ctl.x
            }
            // Baked light: A gates the analytic sun through a smoothstep over
            // the signed-distance field — the penumbra width is the decode
            // WINDOW ([`LM_SUN_SOFT`]), a runtime knob, not a bake product.
            // RGB adds the lamps (x2: half range stored for overbright).
            //
            // Realtime cascades with clustered lamps use none of it: the
            // cascades replace both sun gates below (mixed in by csm_p.x = 1)
            // and the atlas lamps are multiplied out by cluster_on, so the
            // three reads are skipped (a uniform branch).
            var lm = vec4(0.0, 0.0, 0.0, 0.0)
            var sun_vis_g = 1.0
            if self.csm_p.x < 0.5 || self.cluster_on < 0.5 {
                lm = self.light_map.sample(self.v_lm_uv)
                // Dynamics gate their sun through the GROUND region instead
                // (statics have v_lmg.z = 0, dynamics have lm_rect = 0, so the
                // two gates never both engage). The shadow-top plane rejects
                // the ground's shadow for vertices ABOVE the blocker along the
                // sun ray: a fence rail shades shins, never the head over it.
                let lmg = self.light_map.sample(self.v_lmg.xy)
                let top_g = self.lm_top_decode.x
                    + self.top_map.sample(self.v_lmg.xy).x * self.lm_top_decode.y
                let occ_g = 1.0 - smoothstep(top_g - 0.15, top_g + 0.15, self.v_lmg.w)
                sun_vis_g = mix(1.0, smoothstep(0.2, 0.8, lmg.w), self.v_lmg.z * occ_g)
            }
            let has_lm = step(0.000001, self.lm_rect.z)
            let sun_vis = mix(1.0, smoothstep(0.2, 0.8, lm.w), has_lm)
            // Realtime: the cascades replace BOTH baked gates (own chart
            // and ground projection) — one receive path for every family.
            let sun_all = mix(
                sun_vis * sun_vis_g,
                self.csm_vis(self.v_csm.xyz, self.v_csm_n, self.v_csm.w),
                self.csm_p.x
            )
            // 0.9 = lightmap::LM_LAMP_CEIL — the atlas RGB decode.
            let lamps = lm.xyz * (0.9 * has_lm) * (1.0 - self.cluster_on)
            // Local light — baked pools plus the per-frame slots — reaches
            // this fragment WITHOUT the sun's shadow term, because a shadow
            // is the absence of SUN and of nothing else. Over its bright core
            // a pool additionally fills that shadow back in, so a lamp drowns
            // out the streak its own pole throws across its own pool:
            // lightmap::lamp_shadow_fill.
            var local = lamps + self.v_dl
            if self.cluster_on > 0.5 { local = self.cluster_sum(self.v_csm.xyz, self.v_csm_n) }
            let sun_lit = self.sun_filled(sun_all, local)
            let analytic = self.gi_ambient(self.v_csm.xyz,self.v_csm_n,self.v_ambient) * (ao * sao)
                + self.v_direct * (ao_direct * sun_lit)
                + local * ao_direct
            // prelit: albedo already carries COLOR_0 = LM×4. Multiplying
            // the sun again zeros any face that looks inward or down.
            // HDR: a prelit map keeps its authored brightness through the
            // exposure (1/exposure here, exposure in the composite).
            var lit = albedo * mix(analytic, vec3(self.lin_ctl.z, self.lin_ctl.z, self.lin_ctl.z), self.prelit)
            // Streamed-world window glow (stream_draw.rs): tint.w = 1 + n,
            // n the night factor. The albedo's alpha below 1 is a lit-window
            // mask (1 - a) * 2 in 0..1; a window glows once its mask clears a
            // threshold that falls from 0.82 by day (a few warm interiors) to
            // 0.42 at night (about a third of the windows lit, warm and a few
            // cold-white offices). Other lanes keep tint.w
            // = 1 and never glow.
            if self.tint.w > 1.0001 {
                let n = clamp(self.tint.w - 1.0, 0.0, 1.0)
                let m = clamp((1.0 - tex.w) * 2.0, 0.0, 1.0)
                let on = smoothstep(mix(0.82, 0.42, n), mix(0.86, 0.5, n), m)
                let hue = fract(m * 7.13)
                let warm = mix(mix(vec3(1.0, 0.62, 0.3), vec3(1.0, 0.8, 0.55), hue), vec3(0.7, 0.8, 1.0), step(0.8, hue))
                // Capped relative to the scene: the glow never exceeds about
                // three times the albedo-lit street (~1.5 EV over), so a lit
                // shop reads as a warm room, not a white slab.
                let glow = warm * (on * mix(0.3, 0.8, n) * mix(0.45, 1.0, m))
                let cap = max(max(lit.x, max(lit.y, lit.z)) * 3.0, 0.12)
                lit = lit + glow * min(1.0, cap / max(glow.x, 0.0001))
            }
            // AO DEBUG: show baked occlusion alone, contrast-stretched. AO
            // lives in [AO_FLOOR, 1] (0.52..1), so raw it is a wash of pale
            // greys and judging whether a 90-degree corner actually darkens is
            // guesswork. Remapped to full black-to-white, a corner that works
            // is unmistakable and one that does not is equally so.
            if self.ao_debug > 0.5 {
                // HARD PINK, heavily accentuated. Greyscale AO on grey-white
                // Kenney walls is unreadable — the whole reason the last three
                // bakes looked "a bit smudgy" is that a real defect and a
                // correct result differ by a few percent of luminance. Hue
                // separates occlusion from albedo completely, and the cube
                // curve pushes even slight darkening to saturation, so where
                // AO does anything at all it is obvious.
                // LINEAR over AO's actual range. An earlier version cubed this
                // to make faint occlusion obvious and that made the view lie:
                // a barely-shaded wall at ao=0.9 came out 37% pink, so the
                // whole house read as heavily occluded while the atlas was in
                // fact 74% unoccluded. A debug view that exaggerates is worse
                // than none — it hides the very problem it is there to show.
                let occ = clamp((1.0 - ao) / 0.70, 0.0, 1.0)
                return vec4(mix(vec3(1.0, 1.0, 1.0), vec3(1.0, 0.0, 0.55), occ), 1.0)
            }
            // LM DEBUG: the ACTIVE sun tier alone — red in shadow, green in
            // sun, lamps added as their own colour on top. Albedo suppressed
            // so a faint lamp or a misplaced shadow reads instantly.
            if self.lm_debug > 0.5 {
                return vec4(
                    mix(vec3(0.6, 0.1, 0.1), vec3(0.1, 0.6, 0.1), sun_lit) + lamps,
                    1.0
                )
            }
            return self.csm_debug_view(self.gi_display(vec4(mix(self.to_display(lit), self.fog_color, self.scene_fog(self.v_fog, self.v_csm.xyz, self.fog_density)), 1.0),self.v_csm.xyz,self.v_csm_n),self.v_csm.xyz,self.v_csm_n)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
        morph_map: texture_2d(float)
        // The weight of morph target k (0..31): morph_weights0..7, four each.
        morph_weight: fn(k: float) -> float {
            let i = floor(k / 4.0)
            var w = self.morph_weights0
            if i > 0.5 { w = self.morph_weights1 }
            if i > 1.5 { w = self.morph_weights2 }
            if i > 2.5 { w = self.morph_weights3 }
            if i > 3.5 { w = self.morph_weights4 }
            if i > 4.5 { w = self.morph_weights5 }
            if i > 5.5 { w = self.morph_weights6 }
            if i > 6.5 { w = self.morph_weights7 }
            let c = k - i * 4.0
            if c < 0.5 { return w.x }
            if c < 1.5 { return w.y }
            if c < 2.5 { return w.z }
            return w.w
        }
        // The first morph_ctl.w targets' deltas, weighted, in target order
        // (one loop: the body is compiled once, not per target).
        morph_delta: fn(vertex:float,lane:float)->vec3f {
            var delta=vec3(0.0,0.0,0.0)
            var k=0.0
            while k < 31.5 && self.morph_ctl.w > k + 0.5 {
                let index=(k*self.morph_ctl.z+vertex)*2.0+lane
                // Rows count from the top of the allocation size() reports,
                // which a backend may make taller than the rows uploaded.
                let size=self.morph_map.size()
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/size.x,(floor(index/self.morph_ctl.x)+0.5)/size.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weight(k)
                k=k+1.0
            }
            return delta
        }

    }
}
