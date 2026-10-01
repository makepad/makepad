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
    //   * the AO and lightmap DEBUG views (host settings)
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

        // The material hooks (makepad-render-material). Each stock body
        // returns its input, so the stock lane draws exactly as before and a
        // custom material (custom_material.rs) replaces only what it hooks.
        // `surface` is also game.material's albedo hook.
        surface: fn(base: vec4) -> vec4 { return base }
        mat_normal: fn(n: vec3) -> vec3 { return n }
        mat_metal_rough: fn(mr: vec2) -> vec2 { return mr }
        mat_emission: fn(e: vec3) -> vec3 { return e }
        mat_ambient: fn(n: vec3, a: vec3) -> vec3 { return a }
        mat_lighting: fn(direct: vec3, ambient: vec3) -> vec3 { return direct + ambient }
        mat_finish: fn(c: vec4) -> vec4 { return c }
        // The lit colour from its parts (see mat_compose_hooked in
        // render-material for what a light or lighting hook swaps in): the
        // ambient, sun and lamp diffuse (a, s, l) under the diffuse albedo,
        // plus the sun, ambient and clustered specular (ss, sa, sl). The
        // trailing arguments describe the sun for the hooks and fold away here.
        mat_compose: fn(albedo: vec3, metal: float, a: vec3, s: vec3, l: vec3, ss: vec3, sa: vec3, sl: vec3, n: vec3, ldir: vec3, v: vec3, sun_rad: vec3, f: vec3, lobe: float) -> vec3 {
            return albedo * ((1.0 - metal) * (a + s + l)) + ss + sa + sl
        }
        tn_hash: fn(p: vec2) -> float {
            return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453)
        }
        // Smooth value noise in 0..1 (terrain splat edges).
        tn_noise: fn(p: vec2) -> float {
            let i = floor(p)
            let f = fract(p)
            let u = f * f * (vec2(3.0, 3.0) - f * 2.0)
            let a = self.tn_hash(i)
            let b = self.tn_hash(i + vec2(1.0, 0.0))
            let c = self.tn_hash(i + vec2(0.0, 1.0))
            let d = self.tn_hash(i + vec2(1.0, 1.0))
            return mix(mix(a, b, u.x), mix(c, d, u.x), u.y)
        }

        // x^5, the Schlick exponent. Written out rather than pow(x, 5.0):
        // two multiplies and a square beat a transcendental on every tiler.
        pow5: fn(x: float) -> float {
            let x2 = x * x
            return x2 * x2 * x
        }

        // Specular environment without a probe: the sky this frame already
        // resolved. The horizon is the fog colour (in the HDR lane that IS
        // the horizon radiance the dome shows), the zenith the sky fill, a
        // soft aureole sits around the sun (the GGX lobe draws the disc
        // itself), and below the horizon the ground. Roughness widens the
        // horizon band and pulls the whole map toward the hemisphere
        // average, as a prefiltered mip chain would.
        sky_env: fn(r: vec3, rough: float) -> vec3 {
            let band = 0.06 + rough * 0.5
            let up = smoothstep(0.0 - band * 0.25, band + 0.3, r.y)
            var sky = mix(self.fog_color, self.sun_sky, up)
            let sd = max(dot(r, normalize(self.light_dir)), 0.0)
            let sd4 = sd * sd * sd * sd
            sky = sky + self.sun_color * (sd4 * sd4 * (0.12 - 0.08 * rough))
            let below = smoothstep(0.0, 0.0 - band - 0.04, r.y)
            let env = mix(sky, self.sun_ground * 0.7, below)
            let avg = mix(self.sun_ground, self.sun_sky, clamp(r.y * 0.5 + 0.5, 0.0, 1.0))
            return mix(env, avg, rough)
        }

        // Race paint and toy gloss (tex_mag.y packs rim * 255 * 65536 +
        // clearcoat * 255 * 256 + flake * 255): a mirror-smooth lacquer over
        // the base lobe. Its own Fresnel takes
        // light from the base, and it reflects the sky environment sharply
        // with a treeline silhouette along the horizon (dark, broken by
        // azimuth), so the reflection reads as a place and slides over the
        // body as the car turns. Flakes are hashed facets ~1 mm across that
        // catch the sun a little off the mirror direction.
        clear_coat: fn(base: vec3, n: vec3, v: vec3, l: vec3, albedo: vec3, sun: vec3, amb_occ: float) -> vec3 {
            let ndv = max(dot(n, v), 0.0001)
            let r = n * (2.0 * ndv) - v
            let packed_all = floor(self.tex_mag.y + 0.5)
            let rim = floor(packed_all / 65536.0)
            let packed = packed_all - rim * 65536.0
            let coat = floor(packed / 256.0) / 255.0
            let flake = (packed - floor(packed / 256.0) * 256.0) / 255.0
            let fc = (0.04 + 0.96 * self.pow5(1.0 - ndv)) * coat
            var env = self.sky_env(normalize(r), 0.03)
            // The treeline is car paint's; a toy (rim set) reflects a plain
            // studio sky, and skips the two noise taps.
            if rim < 0.5 {
                let az = atan2(r.z, r.x)
                let ridge = 0.035 + 0.05 * self.tn_noise(vec2(az * 9.0, 0.5)) + 0.03 * self.tn_noise(vec2(az * 37.0, 1.5))
                let tree = (1.0 - smoothstep(ridge - 0.01, ridge + 0.01, r.y)) * smoothstep(-0.02, 0.0, r.y)
                env = mix(env, self.sun_ground * 0.28 + self.fog_color * 0.12, tree * 0.85)
            }
            let h = normalize(l + v)
            let ndh = max(dot(n, h), 0.0)
            let a2 = 0.0016
            let den = ndh * ndh * (a2 - 1.0) + 1.0
            let glint = a2 / max(3.14159265 * den * den, 0.0001) * 0.25
            var out = base * (1.0 - fc) + env * (fc * amb_occ) + sun * (glint * fc)
            if flake > 0.0 {
                let cell = floor(self.v_csm.xyz * 900.0)
                let fh = fract(sin(dot(cell, vec3(12.9898, 78.233, 37.719))) * 43758.5453)
                let fh2 = fract(fh * 17.13 + 0.37)
                let tilt = normalize(n + (vec3(fh, fh2, fract(fh * 7.7)) - vec3(0.5, 0.5, 0.5)) * 0.5)
                let sparkle = pow(max(dot(tilt, h), 0.0), 220.0) * step(0.55, fh2)
                out = out + sun * albedo * (sparkle * flake * 6.0) + env * albedo * (flake * 0.12 * amb_occ)
            }
            if rim > 0.0 { out = out + self.toy_rim(n, v, l, amb_occ) * (rim / 255.0) }
            return out
        }

        // The studio rim a toy diorama is lit with: a soft Fresnel edge of
        // sky, brightened by the sun when it is behind the object, so a
        // silhouette separates from the set around it.
        toy_rim: fn(n: vec3, v: vec3, l: vec3, amb_occ: float) -> vec3 {
            let e = 1.0 - max(dot(n, v), 0.0)
            let back = 0.35 + 0.65 * max(0.0 - dot(l, v), 0.0)
            return (self.sun_color * (back * mix(1.0, 3.14159265, self.lin_ctl.x) * 0.45) + self.sun_sky * 0.8) * (e * e * e * amb_occ)
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
            if self.fur_mask() < 0.5 { self.clip() }
            var tex = vec4(0.0, 0.0, 0.0, 0.0)
            if self.triplanar != 0.0 {
                // World-position triplanar (generated terrain): three planar
                // samples blended by the geometric normal, so a cliff keeps
                // an unstretched texture with no per-triangle projection
                // seams. Height (alpha) blends the same way for the masks.
                let wp = self.v_csm.xyz * abs(self.triplanar)
                let gn = abs(normalize(self.v_csm_n))
                var bw = gn * gn
                bw = bw * bw
                bw = bw / max(bw.x + bw.y + bw.z, 0.0001)
                let ty = self.tex.sample_repeat(vec2(wp.x, wp.z))
                if self.triplanar < 0.0 {
                    // An up-facing part (negative scale, terrain_mesh
                    // FLAT_Y): the side projections weigh ~1% at most.
                    // A per-draw branch, so implicit mip selection holds.
                    tex = ty
                } else {
                    let tx = self.tex.sample_repeat(vec2(wp.z, 0.0 - wp.y))
                    let tz = self.tex.sample_repeat(vec2(wp.x, 0.0 - wp.y))
                    tex = tx * bw.x + ty * bw.y + tz * bw.z
                }
                // Anti-tiling: a texture that repeats every few metres reads
                // as a checker grid from the air. With distance, blend in a
                // second top-down read at an unrelated scale and offset, and
                // drift the brightness over ~60 m, so no period survives.
                let dist = length(self.eye.xyz - self.v_csm.xyz)
                let far = smoothstep(25.0, 140.0, dist)
                if far > 0.001 {
                    let t2 = self.tex.sample_repeat(vec2(wp.x * 0.173 + 0.31, wp.z * 0.173 - 0.47))
                    tex = vec4(mix(tex.xyz, t2.xyz, 0.5 * far * bw.y), tex.w)
                }
                let drift = self.tn_noise(self.v_csm.xz * 0.017) * 0.65 + self.tn_noise(self.v_csm.xz * 0.061) * 0.35
                tex = vec4(tex.xyz * (0.86 + 0.28 * drift), tex.w)
            } else {
                tex = self.base_texel()
            }
            var alpha=tex.w*self.material_alpha*mix(1.0,self.v_tint.w,self.surface_on)
            if self.triplanar != 0.0 && self.alpha_mode>0.5 && self.alpha_mode<1.5 {
                // A terrain splat overlay (gen terrain_mesh: vertex alpha is
                // 0.45 + 0.55 * weight, texture alpha is a height). Where two
                // layers share a cell at weight ~0.5 the product height *
                // alpha stays under the cutoff for BOTH overlays, so the
                // per-triangle dominant base shows raw: a sawtooth of
                // triangles. Cover instead by weight around 0.5, shifted by
                // the texture's height and by world noise (~2 m and ~0.5 m),
                // so the boundary follows the weight contour and frays into
                // an organic edge. Weight 1 always covers, weight 0 never.
                let w = clamp((self.v_tint.w - 0.45) / 0.55, 0.0, 1.0)
                let np = self.v_csm.xz
                let n = self.tn_noise(np * 0.45) * 0.6 + self.tn_noise(np * 1.9) * 0.4
                // 0.36, not 0.5: three or four layers often share a vertex,
                // so the base under an overlay may itself hold under half.
                let cover = w + (tex.w - 0.5) * 0.45 + (n - 0.5) * 0.5
                alpha = mix(0.0, 1.0, step(0.36, cover) * step(0.02, w)) * self.material_alpha
            }
            if self.surface_on<0.5 && tex.w<0.5 {self.clip()}
            if self.alpha_mode>0.5 && self.alpha_mode<1.5 && alpha<self.alpha_cutoff {self.clip()}
            let base = self.to_scene(vec3(tex.x, tex.y, tex.z))
            // `surface` is the game.material hook (DrawSceneCustom overrides
            // it; stock PBR keeps the identity). A custom surface that cuts
            // alpha under 0.5 discards; on the stock lane that case was
            // already discarded above (tex.w < 0.5), so output is unchanged.
            let surf = self.surface(vec4(self.color_adjust(
                base * self.to_lin(self.v_tint.xyz),
                self.tint,
                self.color_adjust_ctl
            ), alpha))
            if self.surface_on < 0.5 && surf.w < 0.5 {self.clip()}
            // A Mask material cuts on the alpha its surface hook gives (the
            // stock hook returns the alpha tested above).
            if self.alpha_mode>0.5 && self.alpha_mode<1.5 && surf.w<self.alpha_cutoff {self.clip()}
            let albedo = surf.xyz
            return self.shade(albedo, surf.w)
        }

        // The lit surface: occlusion, sun, lamps, GI and the material's
        // composition of them, then emission, the finish, fog and the
        // debug views. Its own function so an unlit program's variant can
        // replace it with shade_unlit (renderer/variants.rs).
        shade: fn(albedo: vec3, surf_w: float) -> vec4 {
            // Occlusion, sun visibility and lamps: verbatim from
            // DrawSceneSkinned, so a PBR prop sits in the same light as the
            // wall behind it.
            var baked = 0.0
            if self.ao_enabled > 0.5 { baked = self.ao_map.sample(self.v_ao_uv).x }
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
            // Realtime cascades with clustered lamps read none of the baked
            // atlas (DrawSceneSkinned explains): its three reads are skipped.
            var lm = vec4(0.0, 0.0, 0.0, 0.0)
            var sun_vis_g = 1.0
            if self.csm_p.x < 0.5 || self.cluster_on < 0.5 {
                lm = self.light_map.sample(self.v_lm_uv)
                let lmg = self.light_map.sample(self.v_lmg.xy)
                let top_g = self.lm_top_decode.x
                    + self.top_map.sample(self.v_lmg.xy).x * self.lm_top_decode.y
                let occ_g = 1.0 - smoothstep(top_g - 0.15, top_g + 0.15, self.v_lmg.w)
                sun_vis_g = mix(1.0, smoothstep(0.2, 0.8, lmg.w), self.v_lmg.z * occ_g)
            }
            let has_lm = step(0.000001, self.lm_rect.z)
            let sun_vis = mix(1.0, smoothstep(0.2, 0.8, lm.w), has_lm)
            // Swaying foliage (the wind flag, morph_ctl.w = -1) reads the
            // cascades with one tap: leaf cards overdraw several deep.
            var csm = 1.0
            if self.morph_ctl.w < -0.5 {
                csm = self.csm_vis_fast(self.v_csm.xyz, self.v_csm_n, self.v_csm.w)
            } else {
                csm = self.csm_vis(self.v_csm.xyz, self.v_csm_n, self.v_csm.w)
            }
            let sun_all = mix(sun_vis * sun_vis_g, csm, self.csm_p.x)
            // 0.9 = lightmap::LM_LAMP_CEIL — the atlas RGB decode.
            let lamps = lm.xyz * (0.9 * has_lm) * (1.0 - self.cluster_on)
            // Legacy local diffuse; clustered PBR evaluates the same light
            // list once below, including each lamp's GGX specular response.
            var local = lamps + self.v_dl
            if self.cluster_on > 0.5 { local = vec3(0.0,0.0,0.0) }
            let sun_lit = self.sun_filled(sun_all, local)

            // The material. Factor x map, per the glTF spec; orm_on is 0 for
            // a factors-only material so the sample folds out to 1.
            // Factors only (orm_on 0): the map would fold out, so it is not read.
            var orm = vec4(1.0, 1.0, 1.0, 1.0)
            if self.orm_on > 0.0 { orm = self.orm_map.sample_repeat(self.v_uv) }
            // Roughness floored at 0.045: a2 goes to zero below that and the
            // GGX denominator collapses to a single blown-out pixel that
            // aliases into a crawling white dot as the camera moves.
            let mr = self.mat_metal_rough(vec2(
                clamp(self.metallic * mix(1.0, orm.z, self.orm_on), 0.0, 1.0),
                clamp(self.roughness * mix(1.0, orm.y, self.orm_on), 0.045, 1.0)
            ))
            let rough = mr.y
            let metal = mr.x

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
                    // Tangent-space X and Y; Z is rebuilt (BC5 normal maps store only XY).
                    let xy=self.normal_map.sample_repeat(self.v_uv).xy*2.0-vec2(1.0,1.0)
                    let mapped=vec3(xy.x,xy.y,sqrt(max(1.0-dot(xy,xy),0.0)))
                    n=normalize(tangent*(mapped.x*self.normal_scale)+bitangent*(mapped.y*self.normal_scale)+n*mapped.z)
                }
            }
            n = self.mat_normal(n)
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
            let surface_ambient=self.mat_ambient(n,self.gi_ambient(self.v_csm.xyz,n,mix(self.v_ambient,mix(self.sun_ground,self.sun_sky,clamp(n.y*0.5+0.5,0.0,1.0)),self.surface_on)))
            // Light arrives as irradiance/pi units (the diffuse lobe carries
            // no 1/pi), so the HDR lane lifts the lobe by pi to match.
            let sun_spec = surface_direct * (dist * geo / max(4.0 * ndv * ndl, 0.0001)) * mix(1.0, 3.14159265, self.lin_ctl.x)
                * (sun_lit * ao_direct)

            // Ambient specular from the analytic sky environment (sky_env)
            // along the reflected view ray, gated by the roughness-aware
            // Fresnel (Karis' EnvBRDF fallback). A polished surface picks up
            // the bright horizon, the sky above and the ground below, and
            // the reflection slides as the camera orbits; a rough one
            // collapses back to the flat ambient.
            let refl = n * (2.0 * ndv) - v
            let env = self.sky_env(normalize(refl), rough)
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
            // Per-draw factors of zero fold these maps out: skip the reads.
            var occlusion=1.0
            if self.occlusion_strength*self.surface_on>0.0 {occlusion=mix(1.0,self.occlusion_map.sample_repeat(self.v_uv).x,self.occlusion_strength*self.surface_on)}
            var emission=vec3(0.0,0.0,0.0)
            if max(self.emissive.x,max(self.emissive.y,self.emissive.z))>0.0 {emission=self.to_scene(self.emissive_map.sample_repeat(self.v_uv).xyz)*self.emissive}
            emission = self.mat_emission(emission)
            // The sun as the light hook sees it: its radiance (colour and
            // shadow, before N.L) and its specular lobe per unit N.L.
            let sun_rad = self.sun_color * (ao_direct * sun_lit)
            let lobe = (dist * geo / max(4.0 * ndv * ndl, 0.0001)) * mix(1.0, 3.14159265, self.lin_ctl.x)
            var lit = self.fur_shade(self.mat_compose(albedo, metal, surface_ambient*(ao*sao*occlusion), surface_direct*(ao_direct*sun_lit), local*ao_direct, sun_spec*f, amb_spec*occlusion, local_pbr*ao_direct, n, l, v, sun_rad, f, lobe), n, self.eye.xyz-self.v_csm.xyz) + emission
            if self.tex_mag.y > 0.5 {
                lit = self.clear_coat(lit, n, v, l, albedo, surface_direct * (sun_lit * ao_direct), ao * sao)
            }
            let fin=self.mat_finish(vec4(self.to_display(lit),mix(1.0,surf_w,step(1.5,self.alpha_mode))))
            let coverage=fin.w
            return self.csm_debug_view(self.gi_display(vec4(mix(fin.xyz, self.fog_color, self.scene_fog(self.v_fog, self.v_csm.xyz, self.fog_density))*coverage,coverage),self.v_csm.xyz,n),self.v_csm.xyz,n)
        }

        // shade for a program whose composition reads no light (Unlit,
        // Flat, the error material): the same normal, emission, finish, fog
        // and debug views, without computing the light it would ignore (nor
        // a clear coat: an unlit surface reflects nothing).
        shade_unlit: fn(albedo: vec3, surf_w: float) -> vec4 {
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
                    let xy=self.normal_map.sample_repeat(self.v_uv).xy*2.0-vec2(1.0,1.0)
                    let mapped=vec3(xy.x,xy.y,sqrt(max(1.0-dot(xy,xy),0.0)))
                    n=normalize(tangent*(mapped.x*self.normal_scale)+bitangent*(mapped.y*self.normal_scale)+n*mapped.z)
                }
            }
            n = self.mat_normal(n)
            let l = normalize(self.light_dir)
            let v = normalize(self.eye.xyz - self.v_csm.xyz)
            var emission=vec3(0.0,0.0,0.0)
            if max(self.emissive.x,max(self.emissive.y,self.emissive.z))>0.0 {emission=self.to_scene(self.emissive_map.sample_repeat(self.v_uv).xyz)*self.emissive}
            emission = self.mat_emission(emission)
            let none = vec3(0.0,0.0,0.0)
            let lit = self.fur_shade(self.mat_compose(albedo, 0.0, none, none, none, none, none, none, n, l, v, none, none, 0.0), n, self.eye.xyz-self.v_csm.xyz) + emission
            let fin=self.mat_finish(vec4(self.to_display(lit),mix(1.0,surf_w,step(1.5,self.alpha_mode))))
            let coverage=fin.w
            return self.csm_debug_view(self.gi_display(vec4(mix(fin.xyz, self.fog_color, self.scene_fog(self.v_fog, self.v_csm.xyz, self.fog_density))*coverage,coverage),self.v_csm.xyz,n),self.v_csm.xyz,n)
        }

        // Re-declared rather than inherited so the depth-clip wrapper is
        // provably calling THIS pixel fn.
        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // Camera-space FPS held mesh. A small sibling of DrawScenePbr, not a
    // mode inside the world shader: no lightmap, top-map, CSM, fog or
    // dynamic-light instructions. It is still a real PBR surface, because a
    // held weapon fills a third of the screen at arm's length: the layer's
    // normal map, its metallic-roughness (factor x map), a GGX sun lobe and
    // the same analytic sky reflection the world props get (sky_env), so
    // blued steel, polymer, wood and fabric read by their material rather
    // than as flat paint.
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
        orm_map: texture_2d(float)
        normal_map: texture_2d(float)
        emissive_map: texture_2d(float)
        v_uv: varying(vec2f)
        v_world: varying(vec3f)
        v_n: varying(vec3f)
        v_eye: varying(vec3f)
        v_vc: varying(vec4f)

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
            let world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * world
            let clip = self.draw_pass.camera_projection * view_pos
            self.v_n = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            self.v_world = world.xyz
            self.v_eye = (self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)).xyz
            self.v_vc = unpack4u8(self.geom.color)
            self.v_uv = unpack2f16(self.geom.uv)
            // Portable late overlay: 0..w clip depth is valid on Metal/D3D/
            // Vulkan and also inside GL's -w..w range. Retaining a tiny slice
            // of original depth preserves the held model's triangle order.
            let original_01 = clamp(clip.z / clip.w * 0.5 + 0.5, 0.0, 1.0)
            self.vertex_pos = vec4(
                clip.x,
                clip.y,
                clip.w * (0.0001 + original_01 * 0.0008),
                clip.w
            )
        }

        // sRGB to linear (the same curve as `to_lin`) in the HDR lane.
        lin3: fn(c: vec3) -> vec3 {
            if self.lin < 0.5 { return c }
            return c * (c * (c * 0.305306011 + vec3(0.682171111, 0.682171111, 0.682171111)) + vec3(0.012522878, 0.012522878, 0.012522878))
        }

        pow5: fn(x: float) -> float {
            let x2 = x * x
            return x2 * x2 * x
        }

        // DrawScenePbr's analytic sky environment, verbatim (fog colour as
        // the horizon, sky fill overhead, a sun aureole, the ground below).
        sky_env: fn(r: vec3, rough: float) -> vec3 {
            let band = 0.06 + rough * 0.5
            let up = smoothstep(0.0 - band * 0.25, band + 0.3, r.y)
            var sky = mix(self.fog_color, self.sun_sky, up)
            let sd = max(dot(r, normalize(self.light_dir)), 0.0)
            let sd4 = sd * sd * sd * sd
            sky = sky + self.sun_color * (sd4 * sd4 * (0.12 - 0.08 * rough))
            let below = smoothstep(0.0, 0.0 - band - 0.04, r.y)
            let env = mix(sky, self.sun_ground * 0.7, below)
            let avg = mix(self.sun_ground, self.sun_sky, clamp(r.y * 0.5 + 0.5, 0.0, 1.0))
            return mix(env, avg, rough)
        }

        pixel: fn() {
            // Repeat, like the world shaders: a material `tile` pattern
            // (wood grain, knurling, camo) runs its UVs past 1.
            let tex = self.tex.sample_repeat(self.v_uv)
            // Masked decals (stencilled markings, vents, serrations) cut out.
            if self.alpha_mode > 0.5 && tex.w < self.alpha_cutoff { discard() }
            let albedo = self.lin3(tex.xyz) * self.lin3(self.v_vc.xyz)
            let ao = self.v_vc.w
            var n = normalize(self.v_n)
            if self.normal_scale > 0.0 {
                let dp1 = dFdx(self.v_world)
                let dp2 = dFdy(self.v_world)
                let du1 = dFdx(self.v_uv)
                let du2 = dFdy(self.v_uv)
                let det = du1.x * du2.y - du1.y * du2.x
                if abs(det) > 0.00000001 {
                    let o = sign(det)
                    let t = normalize(dp1 * du2.y - dp2 * du1.y) * o
                    let b = normalize(dp2 * du1.x - dp1 * du2.x) * o
                    let xy = self.normal_map.sample_repeat(self.v_uv).xy * 2.0 - vec2(1.0, 1.0)
                    let m = vec3(xy.x, xy.y, sqrt(max(1.0 - dot(xy, xy), 0.0)))
                    n = normalize(t * (m.x * self.normal_scale) + b * (m.y * self.normal_scale) + n * m.z)
                }
            }
            let orm = self.orm_map.sample_repeat(self.v_uv)
            let rough = clamp(self.roughness * mix(1.0, orm.y, self.orm_on), 0.04, 1.0)
            let metal = clamp(self.metallic * mix(1.0, orm.z, self.orm_on), 0.0, 1.0)
            let l = normalize(self.light_dir)
            let v = normalize(self.v_eye - self.v_world)
            let h = normalize(l + v)
            let ndv = max(dot(n, v), 0.0001)
            let ndl = max(dot(n, l), 0.0)
            let ndh = max(dot(n, h), 0.0001)
            let vdh = max(dot(v, h), 0.0)
            let f0 = mix(vec3(0.04, 0.04, 0.04), albedo, metal)
            let f = f0 + (vec3(1.0, 1.0, 1.0) - f0) * self.pow5(1.0 - vdh)
            let a2 = rough * rough * rough * rough
            let den = ndh * ndh * (a2 - 1.0) + 1.0
            let dist = a2 / max(3.14159265 * den * den, 0.0001)
            let k = (rough + 1.0) * (rough + 1.0) * 0.125
            let geo = (ndv / max(ndv * (1.0 - k) + k, 0.0001)) * (max(ndl, 0.0001) / max(ndl * (1.0 - k) + k, 0.0001))
            let direct = self.sun_color * (ndl * self.sun_vis)
            let sun_spec = direct * (dist * geo / max(4.0 * ndv * max(ndl, 0.0001), 0.0001)) * mix(1.0, 3.14159265, step(0.5, self.lin))
            let ambient = mix(self.sun_ground, self.sun_sky, clamp(n.y * 0.5 + 0.5, 0.0, 1.0))
            let refl = n * (2.0 * ndv) - v
            let env = self.sky_env(normalize(refl), rough)
            let smooth3 = 1.0 - rough
            let fr = max(vec3(smooth3, smooth3, smooth3), f0)
            let f_env = f0 + (fr - f0) * self.pow5(1.0 - ndv)
            let emission = self.lin3(self.emissive_map.sample_repeat(self.v_uv).xyz) * self.emissive
            let lit = albedo * ((1.0 - metal) * (ambient * ao + direct * mix(1.0, ao, 0.35)))
                + sun_spec * f + env * f_env * ao + emission
            return vec4(lit, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}
