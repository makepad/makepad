//! The cube/terrain family's primitive batches: DrawSceneCube and its alpha variant.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // The game cube: DrawCube + per-instance emission and distance fog.
    //
    // The sun and fog COLOUR are uniforms, not instances. They are identical
    // for every instance in a batch, so as instance fields they cost 12
    // floats (48 bytes) per cube of pure duplication — the single largest
    // waste in the stream on a bandwidth-bound tiler. `fog_density` stays
    // per-instance because shadows switch it off individually.
    mod.draw.DrawSceneCube = mod.std.set_type_default() do #(DrawSceneCube::script_shader(vm)){
        ..mod.draw.DrawCube,
        ..mod.draw.SceneColorAdjust,
        // The platform default is OFF (draw_shader.rs:41) and DrawCube does not
        // override it, so every slab, crate, ground plane and rigid body was
        // rasterising its BACK faces too — invisible, and double the fill. A
        // tiler pays per fragment and a headset pays twice again for stereo,
        // so this is the single cheapest win available on the geometry that
        // covers most of the screen. Safe because shape_geometry_data winds
        // every primitive outward and `shape_windings_face_outward` asserts it.
        backface_culling: true
        v_fog: varying(float)
        v_direct: varying(vec3f)
        v_up: varying(float)
        v_lm_uv: varying(vec2f)
        v_lm_in: varying(float)
        v_albedo: varying(vec3f)
        fog_color: uniform(vec3(0.75, 0.87, 0.96))
        sun_color: uniform(vec3(0.72, 0.72, 0.72))
        sun_sky: uniform(vec3(0.28, 0.28, 0.28))
        sun_ground: uniform(vec3(0.28, 0.28, 0.28))
        // The scene's ground light field: one planar lightmap region over
        // the whole static footprint (terrain heights ∪ box tops), addressed
        // by world xz — no per-cube data, so the packed slab layout is
        // untouched. Zero lm_rect = no lightmap, the pre-bake path.
        light_map: texture_2d(float)
        // The field's shadow-top plane (R8, same uv as light_map): per
        // texel the ABSOLUTE world height its sun ray was blocked at,
        // decoded base + byte * range from lm_top_decode (255 = lit / no
        // blocker measured). The field stores shadow at GROUND level; a
        // fragment ABOVE the blocker is out of that shadow — a raised
        // dirt ramp must not wear the fence shadows that land on the
        // grass under its footprint, and a crate carried over a rail's
        // shadow stays lit.
        top_map: texture_2d(float)
        lm_rect: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        lm_world: uniform(vec4(0.0, 0.0, 1.0, 1.0))
        lm_top_decode: uniform(vec4(0.0, 8.0, 0.0, 0.0))
        // Realtime cascaded shadow maps (shadow_csm.rs): 3 sun-depth tiles
        // side by side in one Rf32 strip. csm_p = (tier on, one tile's
        // inverse resolution, cascade-0 texel_world, cascade-1 texel_world);
        // csm_r*N are cascade N's world->map
        // rows; csm_bias.xyz = z01 depth bias, .w = cascade-2 texel_world.
        // When the tier
        // is on, `csm_vis` REPLACES every baked sun-visibility path — one
        // receive path for statics, dynamics and characters alike.
        csm_map: texture_depth(float),

        ..mod.draw.SunCascades,
        // Per-frame dynamic lights, up to 8 (renderer.rs write_light_uniforms):
        // dl_posN = xyz + radius (0 = empty slot), dl_colN = rgb + spot amount.
        // The cube family receives TRANSIENT lights only (firework flashes,
        // host frame lights) — street lamps are already baked into the atlas
        // RGB, so summing them here would double-light every static. Computed
        // in the PIXEL stage: slabs are 8-vertex boxes, and a vertex-lit flash
        // pops whole road segments on and off.
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

        // One dynamic light's contribution at world point `wp` with world
        // normal `n`. Attenuation (1 - d/r)^2; the spot factor mirrors
        // lightmap.rs's lamp pass exactly (SPILL = 0.35, squared, mixed by
        // the spot amount) with the emission axis fixed straight DOWN — the
        // harvested street lamps' convention. Empty slots (radius 0) and
        // out-of-radius fragments return early, so the common case of zero
        // active transients costs 8 uniform reads and 8 branches.
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
        }

        vertex: fn() {
            let pos = self.get_size() * self.geom.geom_pos + self.get_pos()
            // TRUE world position first (the stage/view transform must not
            // move the light field), then the view-space chain on top.
            let wpos = self.transform * vec4(pos.x, pos.y, pos.z, 1.0)
            let model_view = self.draw_list.view_transform * self.transform
            let normal4 = model_view * vec4(
                self.geom.geom_normal.x,
                self.geom.geom_normal.y,
                self.geom.geom_normal.z,
                0.0
            )
            let normal = normalize(normal4.xyz)
            self.world = self.draw_list.view_transform * wpos
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(normal, normalize(self.light_dir)), 0.0)
            self.v_albedo = self.color_adjust(
                self.to_lin(self.color.xyz),
                vec4(1.0, 1.0, 1.0, 1.0),
                self.color_adjust_ctl
            )
            self.lit_color = self.get_color(dp, normal.y)
            // The direct sun term rides its own varying so the PIXEL stage
            // can gate it by the baked sun-visibility SDF.
            self.v_direct = self.v_albedo * (self.sun_color * dp)
            self.v_up = normal.y
            let lw = max(self.lm_world.zw, vec2(0.000001, 0.000001))
            let lraw = (wpos.xz - self.lm_world.xy) / lw
            let lf = clamp(lraw, vec2(0.0, 0.0), vec2(1.0, 1.0))
            self.v_lm_uv = self.lm_rect.xy + lf * self.lm_rect.zw
            // Outside the field: fully lit, never a clamp-smeared border.
            self.v_lm_in = step(0.0, lraw.x) * step(lraw.x, 1.0)
                * step(0.0, lraw.y) * step(lraw.y, 1.0)
            // TRUE world position + normal for the pixel-stage dynamic
            // lights (the stage/view transform must not move a light).
            // Cube faces are flat, so the interpolated normal is constant
            // per face and needs no per-fragment renormalize.
            self.v_dl_pos = wpos.xyz
            self.v_dl_nrm = normalize((self.transform * vec4(
                self.geom.geom_normal.x,
                self.geom.geom_normal.y,
                self.geom.geom_normal.z,
                0.0
            )).xyz)
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        // One lighting model for every game shader (sun.rs): hemisphere
        // ambient by surface-up-ness plus emission. The sun's DIRECT term
        // moved to v_direct (gated per fragment by the light field); with
        // the default flat sun this composes to exactly the old constants.
        get_color: fn(dp: float, nrm_y: float) {
            let hemi = clamp(nrm_y * 0.5 + 0.5, 0.0, 1.0)
            let ambient = mix(self.sun_ground, self.sun_sky, hemi)
            let lit = self.v_albedo * ambient
            // Emission: glowing eyes, beacons, bolts (energy ramps at runtime).
            let glowing = lit + self.v_albedo * (self.glow * 0.6 * self.lin_ctl.w)
            return vec4(glowing, self.color.w)
        }

        pixel: fn() {
            // Baked ground light on UP-facing fragments only (feathered by
            // up-ness): the field stores what lands on top surfaces at that
            // xz — walls would smear it vertically. Dynamics sample it too,
            // deliberately: a crate rolling through a house's shadow should
            // darken, and this is the only shadow-receiving dynamics get.
            let lm = self.light_map.sample(self.v_lm_uv)
            let has_lm = step(0.000001, self.lm_rect.z)
                * clamp(self.v_up * 4.0, 0.0, 1.0) * self.v_lm_in
            // Shadow-top comparison: the A channel says what reaches the
            // GROUND at this xz, the R8 plane says how high its blocker
            // sits. A fragment above the blocker rejects the ground's
            // shadow; at ground level top_h is above the fragment and this
            // collapses to the old behaviour.
            let top_h = self.lm_top_decode.x
                + self.top_map.sample(self.v_lm_uv).x * self.lm_top_decode.y
            let occ = 1.0 - smoothstep(top_h - 0.15, top_h + 0.15, self.v_dl_pos.y)
            // Realtime: the cascades replace the whole baked ground path.
            let ndl_c = max(dot(normalize(self.v_dl_nrm), normalize(self.light_dir)), 0.0)
            let sun_vis = mix(
                mix(1.0, smoothstep(0.2, 0.8, lm.w), has_lm * occ),
                self.csm_vis(self.v_dl_pos, self.v_dl_nrm, ndl_c),
                self.csm_p.x
            )
            // 0.9 = lightmap::LM_LAMP_CEIL — the atlas RGB decode.
            let lamps = lm.xyz * (0.9 * has_lm) * (1.0 - self.cluster_on)
            let dl = self.dl_sum(self.v_dl_pos, self.v_dl_nrm)
            // The lamps light this fragment WITHOUT the sun's shadow (a shadow
            // is the absence of sun, not of light), and a strong enough pool
            // fills that shadow back in — lightmap::lamp_shadow_fill.
            let local = lamps + dl + self.cluster_sum(self.v_dl_pos, self.v_dl_nrm)
            let ambient=mix(self.sun_ground,self.sun_sky,clamp(self.v_dl_nrm.y*0.5+0.5,0.0,1.0))
            let gi=self.gi_ambient(self.v_dl_pos,self.v_dl_nrm,ambient)-ambient
            let c = self.lit_color.xyz + self.v_albedo*gi + self.v_direct * self.sun_filled(sun_vis, local)
                + self.v_albedo * local
            let fogged = self.scene_fogged(c, self.v_fog, self.v_dl_pos, self.fog_density)
            return self.csm_debug_view(self.gi_display(vec4(fogged, self.lit_color.w),self.v_dl_pos,self.v_dl_nrm),self.v_dl_pos,self.v_dl_nrm)
        }
    }

    // Same shading, alpha-blended: water, sensor ghosts, blob shadows, and the
    // particle batch.
    mod.draw.DrawSceneAlpha = mod.std.set_type_default() do #(DrawSceneAlpha::script_shader(vm)){
        ..mod.draw.DrawSceneCube,
        alpha_blend: true
        // DELIBERATE, do not "fix": this batch carries flat single-sided
        // geometry — blob shadows and water surfaces — whose winding is not
        // guaranteed to face the viewer, and culling a blended surface changes
        // the composite rather than merely hiding a hidden face. Overriding the
        // `true` now inherited from DrawSceneCube.
        backface_culling: false
    }
}
