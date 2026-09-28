//! DrawScenePbr (Cook-Torrance props) and the view-model shader.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // Static props whose glTF material actually carries SHININESS: a
    // metallic-roughness texture, or a roughness the author narrowed below 1
    // (model.rs PbrMaterial::is_shiny). Everything else keeps DrawSceneSkinned.
    //
    // A SIBLING, for the reason written above DrawSceneSkyAnalytic and
    // DrawSceneSkyMap: DrawSceneSkinned's pixel fn already sits at the
    // script-shader's capacity, and a specular branch inside it would make
    // every Kenney wall in the world execute a GGX lobe it can never show.
    // Which shader a model draws with is decided ONCE, at load, from its
    // material (renderer.rs LoadedModel::pbr).
    //
    // It INHERITS the whole of DrawSceneSkinned — vertex stage, cascades,
    // dynamic lights, octahedral decode — and replaces only `pixel`. That is
    // what keeps the two lanes from drifting: a fix to the CSM receive path
    // or the dl slot gate lands in both by construction. The replaced pixel
    // fn drops what a generated PBR mesh provably never has, and those drops
    // are what pays for the lobe:
    //   * the Q3/Unreal detail overlay (detail maps come with prelit worlds,
    //     which are excluded from this lane by definition),
    //   * the prelit COLOR_0-is-a-lightmap mix (same reason: prelit maps are
    //     the retro look and get no specular),
    //   * the BUILD magenta punch-through key (a Duke overlay convention),
    //   * the AO and lightmap DEBUG views (SANDBOX_AO_DEBUG / SANDBOX_LM_DEBUG
    //     still work on every OTHER prop in the scene; a shiny generated mesh
    //     simply keeps its lit look while they are on).
    mod.draw.DrawScenePbr = mod.std.set_type_default() do #(DrawScenePbr::script_shader(vm)){
        ..mod.draw.DrawSceneSkinned,
        // The glTF metallicRoughnessTexture: G = roughness, B = metallic,
        // and the factors multiply what it says. Declared AFTER the whole
        // inherited set, so it takes the slot past ssao_map (slot 8).
        orm_map: texture_2d(float)
        normal_map: texture_2d(float)
        occlusion_map: texture_2d(float)
        emissive_map: texture_2d(float)
        // TRUE world camera position (w unused). A specular lobe is the one
        // term in this file that needs the eye: everything else here is
        // view-independent. TRUE world, not stage world, so it matches
        // v_csm.xyz and v_csm_n — the same convention the cascades and the
        // dynamic lights already use.
        eye: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // x^5, the Schlick exponent. Written out rather than pow(x, 5.0):
        // two multiplies and a square beat a transcendental on every tiler.
        pow5: fn(x: float) -> float {
            let x2 = x * x
            return x2 * x2 * x
        }

        pixel: fn() {
            // Streamed-LOD crossfade (stream.rs `Dither`): a screen-space
            // noise window. 0 = no dither; else 1 + lo8 * 256 + hi8 and a
            // pixel shows only while lo <= noise < hi, so the two LODs of a
            // transition cover complementary pixels.
            if self.color_adjust_ctl.w > 0.5 {
                let dsp = self.v_spos.xy / max(self.v_spos.w, 0.000001)
                let dpx = floor(vec2(dsp.x * 0.5 + 0.5, 0.5 - dsp.y * 0.5) * vec2(1920.0, 1080.0))
                let dn = fract(52.9829189 * fract(dot(dpx, vec2(0.06711056, 0.00583715))))
                let dv = self.color_adjust_ctl.w - 1.0
                let dlo = floor(dv / 256.0)
                let dhi = dv - dlo * 256.0
                if dn < dlo / 255.0 || dn >= dhi / 255.0 { discard() }
            }
            if self.fur_mask() < 0.5 { discard() }
            let tex = self.base_texel()
            let alpha=tex.w*self.material_alpha*mix(1.0,self.v_tint.w,self.surface_on)
            if self.surface_on<0.5 && tex.w<0.5 {discard()}
            if self.alpha_mode>0.5 && self.alpha_mode<1.5 && alpha<self.alpha_cutoff {discard()}
            let base = self.to_scene(vec3(tex.x, tex.y, tex.z))
            let albedo = self.color_adjust(
                base * self.to_lin(self.v_tint.xyz),
                self.tint,
                self.color_adjust_ctl
            )
            // Occlusion, sun visibility and lamps: verbatim from
            // DrawSceneSkinned, so a PBR prop sits in the same light as the
            // wall behind it.
            let baked = self.ao_map.sample(self.v_ao_uv).x
            let hash = fract(
                sin(dot(self.world.xy + self.world.zz, vec2(12.9898, 78.233))) * 43758.5453
            )
            let ao = clamp(
                mix(mix(self.v_tint.w,1.0,self.surface_on), baked, self.ao_enabled) + (hash - 0.5) * 0.03,
                0.0, 1.0
            )
            // AO takes some of the DIRECT term too (a crease reads in sunlight);
            // the HDR lane keeps less of that stylisation — its real shadows
            // and fill already carry the contrast.
            let ao_direct = mix(1.0, ao, mix(0.75, 0.35, self.lin_ctl.x))
            // Screen-space AO on the ambient terms only, exactly as
            // DrawSceneSkinned: the diffuse fill below and the ambient
            // specular — never the sun's diffuse or its lobe.
            var sao = 1.0
            if self.ssao_ctl.x > 0.001 {
                let sp = self.v_spos.xy / max(self.v_spos.w, 0.000001)
                let suv = vec2(sp.x * 0.5 + 0.5, 0.5 - sp.y * 0.5)
                sao = 1.0 - (1.0 - self.ssao_map.sample_nearest(suv).x) * self.ssao_ctl.x
            }
            let lm = self.light_map.sample_as_bgra(self.v_lm_uv)
            let has_lm = step(0.000001, self.lm_rect.z)
            let sun_vis = mix(1.0, smoothstep(0.2, 0.8, lm.w), has_lm)
            let lmg = self.light_map.sample_as_bgra(self.v_lmg.xy)
            let top_g = self.lm_top_decode.x
                + self.top_map.sample(self.v_lmg.xy).x * self.lm_top_decode.y
            let occ_g = 1.0 - smoothstep(top_g - 0.15, top_g + 0.15, self.v_lmg.w)
            let sun_vis_g = mix(1.0, smoothstep(0.2, 0.8, lmg.w), self.v_lmg.z * occ_g)
            let sun_all = mix(
                sun_vis * sun_vis_g,
                self.csm_vis(self.v_csm.xyz, self.v_csm_n, self.v_csm.w),
                self.csm_p.x
            )
            // 0.9 = lightmap::LM_LAMP_CEIL — the atlas RGB decode.
            let lamps = lm.xyz * (0.9 * has_lm) * (1.0 - self.cluster_on)
            // Legacy local diffuse; clustered PBR evaluates the same light
            // list once below, including each lamp's GGX specular response.
            var local = lamps + self.v_dl
            if self.cluster_on > 0.5 { local = vec3(0.0,0.0,0.0) }
            let sun_lit = self.sun_filled(sun_all, local)

            // The material. Factor x map, per the glTF spec; orm_on is 0 for
            // a factors-only material so the sample folds out to 1.
            let orm = self.orm_map.sample_as_bgra_repeat(self.v_uv)
            // Roughness floored at 0.045: a2 goes to zero below that and the
            // GGX denominator collapses to a single blown-out pixel that
            // aliases into a crawling white dot as the camera moves.
            let rough = clamp(self.roughness * mix(1.0, orm.y, self.orm_on), 0.045, 1.0)
            let metal = clamp(self.metallic * mix(1.0, orm.z, self.orm_on), 0.0, 1.0)

            // TRUE world space for all three vectors.
            var n=normalize(self.v_csm_n)
            if self.double_sided>0.5 && dot(n,self.eye.xyz-self.v_csm.xyz)<0.0 {n=n*(-1.0)}
            if self.surface_on>0.5 && abs(self.normal_scale)>0.00001 {
                let dp1=dFdx(self.v_csm.xyz)
                let dp2=dFdy(self.v_csm.xyz)
                let du1=dFdx(self.v_uv)
                let du2=dFdy(self.v_uv)
                let determinant=du1.x*du2.y-du1.y*du2.x
                if abs(determinant)>0.00000001 {
                    let orientation=sign(determinant)
                    let tangent=normalize(dp1*du2.y-dp2*du1.y)*orientation
                    let bitangent=normalize(dp2*du1.x-dp1*du2.x)*orientation
                    let mapped=self.normal_map.sample_as_bgra_repeat(self.v_uv).xyz*2.0-vec3(1.0,1.0,1.0)
                    n=normalize(tangent*(mapped.x*self.normal_scale)+bitangent*(mapped.y*self.normal_scale)+n*mapped.z)
                }
            }
            let l = normalize(self.light_dir)
            let v = normalize(self.eye.xyz - self.v_csm.xyz)
            let h = normalize(l + v)
            let ndv = max(dot(n, v), 0.0001)
            let ndl = max(dot(n, l), 0.0001)
            let ndh = max(dot(n, h), 0.0001)
            let vdh = max(dot(v, h), 0.0)

            // Cook-Torrance: GGX distribution, Schlick-GGX geometry with the
            // direct-lighting k, Schlick Fresnel off an F0 that a metal takes
            // from its own albedo (that colour shift IS what reads as metal).
            let f0 = mix(vec3(0.04, 0.04, 0.04), albedo, metal)
            let f = f0 + (vec3(1.0, 1.0, 1.0) - f0) * self.pow5(1.0 - vdh)
            let a2 = rough * rough * rough * rough
            let den = ndh * ndh * (a2 - 1.0) + 1.0
            let dist = a2 / max(3.14159265 * den * den, 0.0001)
            let k = (rough + 1.0) * (rough + 1.0) * 0.125
            let geo = (ndv / max(ndv * (1.0 - k) + k, 0.0001))
                * (ndl / max(ndl * (1.0 - k) + k, 0.0001))
            // v_direct is already sun_color * N.L, so the BRDF here carries
            // no cosine of its own — the two compose to the standard
            // radiance * BRDF * N.L. Shadowed exactly like the diffuse sun:
            // a highlight surviving inside a shadow is the classic tell.
            let surface_direct=mix(self.v_direct,self.sun_color*ndl,self.surface_on)
            let surface_ambient=self.gi_ambient(self.v_csm.xyz,n,mix(self.v_ambient,mix(self.sun_ground,self.sun_sky,clamp(n.y*0.5+0.5,0.0,1.0)),self.surface_on))
            // Light arrives as irradiance/pi units (the diffuse lobe carries
            // no 1/pi), so the HDR lane lifts the lobe by pi to match.
            let sun_spec = surface_direct * (dist * geo / max(4.0 * ndv * ndl, 0.0001)) * mix(1.0, 3.14159265, self.lin_ctl.x)
                * (sun_lit * ao_direct)

            // Ambient specular WITHOUT an environment map: the hemisphere
            // ambient this renderer already computes, looked up along the
            // reflected view ray instead of along the normal, gated by the
            // roughness-aware Fresnel (Karis' EnvBRDF fallback). A polished
            // surface therefore picks up sky above and ground below and the
            // highlight slides as the camera orbits, which is the whole
            // point; a rough one collapses back to the flat ambient.
            let refl = n * (2.0 * ndv) - v
            let env = mix(self.sun_ground, self.sun_sky, clamp(refl.y * 0.5 + 0.5, 0.0, 1.0))
            let smooth3 = 1.0 - rough
            let fr = max(vec3(smooth3, smooth3, smooth3), f0)
            let f_env = f0 + (fr - f0) * self.pow5(1.0 - ndv)
            let amb_spec = env * f_env * (ao * sao)

            // A metal has no diffuse lobe at all; a dielectric keeps all of
            // it. (The (1 - F) half of the split is deliberately dropped:
            // at grazing angles it only ever darkens, and without an
            // environment probe there is nothing to hand the energy to.)
            let analytic = surface_ambient * (ao * sao)
                + surface_direct * (ao_direct * sun_lit)
                + local * ao_direct
            let local_pbr = self.cluster_pbr(self.v_csm.xyz, n, self.eye.xyz, albedo, rough, metal)
            let occlusion=mix(1.0,self.occlusion_map.sample_as_bgra_repeat(self.v_uv).x,self.occlusion_strength*self.surface_on)
            let emission=self.to_scene(self.emissive_map.sample_as_bgra_repeat(self.v_uv).xyz)*self.emissive
            let lit = self.fur_shade(albedo * ((1.0 - metal) * (surface_ambient*(ao*sao*occlusion)+surface_direct*(ao_direct*sun_lit)+local*ao_direct)) + sun_spec*f + amb_spec*occlusion + local_pbr*ao_direct, n, self.eye.xyz-self.v_csm.xyz) + emission
            let coverage=mix(1.0,alpha,step(1.5,self.alpha_mode))
            return self.csm_debug_view(self.gi_display(vec4(mix(self.to_display(lit), self.fog_color, self.scene_fog(self.v_fog, self.v_csm.xyz, self.fog_density))*coverage,coverage),self.v_csm.xyz,n),self.v_csm.xyz,n)
        }

        // Re-declared rather than inherited so the depth-clip wrapper is
        // provably calling THIS pixel fn.
        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // Camera-space FPS held mesh. This is intentionally a small sibling of
    // DrawSceneSkinned, not a mode inside the world shader: view geometry gets
    // one texture sample and analytic daylight, and has no lightmap, top-map,
    // CSM, fog or dynamic-light instructions to execute on low-end devices.
    mod.draw.DrawSceneViewModel = mod.std.set_type_default() do #(DrawSceneViewModel::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexAo, geom.GameMeshAoGeom)
        tex: texture_2d(float)
        v_uv: varying(vec2f)
        v_color: varying(vec3f)

        oct_decode: fn(e: vec2f) -> vec3f {
            let nz = 1.0 - abs(e.x) - abs(e.y)
            let t = max(0.0 - nz, 0.0)
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return normalize(vec3(e.x - t * sx, e.y - t * sy, nz))
        }

        vertex: fn() {
            let pos = vec3(self.geom.px, self.geom.py, self.geom.pz)
            let normal_in = self.oct_decode(unpack2f16(self.geom.nrm))
            let model_view = self.draw_list.view_transform * self.transform
            let normal = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            let world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * world
            let clip = self.draw_pass.camera_projection * view_pos
            let dp = max(dot(normal, normalize(self.light_dir)), 0.0)
            let hemi = clamp(normal.y * 0.5 + 0.5, 0.0, 1.0)
            let vc = unpack4u8(self.geom.color)
            self.v_color = vc.xyz * (
                mix(self.sun_ground, self.sun_sky, hemi) * vc.w
                + self.sun_color * dp * mix(1.0, vc.w, 0.35)
            )
            self.v_uv = unpack2f16(self.geom.uv)
            // Portable late overlay: 0..w clip depth is valid on Metal/D3D/
            // Vulkan and also inside GL's -w..w range. Retaining a tiny slice
            // of original depth preserves the pistol's triangle ordering.
            let original_01 = clamp(clip.z / clip.w * 0.5 + 0.5, 0.0, 1.0)
            self.vertex_pos = vec4(
                clip.x,
                clip.y,
                clip.w * (0.0001 + original_01 * 0.0008),
                clip.w
            )
        }

        pixel: fn() {
            let tex = self.tex.sample_as_bgra(self.v_uv)
            return vec4(tex.xyz * self.v_color, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}
