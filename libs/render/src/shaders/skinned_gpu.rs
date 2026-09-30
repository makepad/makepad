//! DrawSceneSkinnedGpu: GPU-skinned characters.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // GPU-skinned character mesh: the REST mesh (geom.GameMeshVertexSkin,
    // uploaded once per rig) blended in the VERTEX stage against a joint
    // palette texture — 3 RGBA32F texels per joint, the top three rows of
    // each 3x4 matrix, all characters packed into one texture per frame with
    // a per-instance texel offset. What used to be a full posed vertex
    // stream per character per frame is now its palette.
    //
    // Deliberately a SIBLING of DrawSceneSkinned rather than a flag inside it
    // (the DrawSceneFoliage pattern): props draw with that shader and have no
    // joints, and this one costs up to 12 vertex texture fetches that the
    // static world must never pay. Lighting/fog match DrawSceneSkinned minus
    // the AO path — a deforming mesh cannot carry a baked occlusion atlas.
    mod.draw.DrawSceneSkinnedGpu = mod.std.set_type_default() do #(DrawSceneSkinnedGpu::script_shader(vm)){
        ..mod.draw.SceneFurSurface,
        ..mod.draw.SceneColorAdjust,
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexSkin, geom.GameMeshSkinGeom)
        tex: texture_2d(float)
        v_color: varying(vec4f)
        joint_tex: texture_2d(float)
        // Rest-pose AO chart atlas, one per rig, sampled per FRAGMENT: the
        // per-vertex bake it replaced interpolated an ear's darkness across
        // the whole low-poly skull dome (same failure that moved the props
        // to their atlas).
        ao_map: texture_2d(float)
        // The scene's baked-light atlas, addressed via the GROUND region by
        // world xz (the cube family's idiom): a character walking through a
        // house's shadow darkens. ONLY the A channel (sun visibility) is
        // read — lamp light arrives through the analytic dl_* array, and
        // adding the baked RGB too would double-light near every pole.
        // Zero lm_rect = no field, fully sunlit.
        light_map: texture_2d(float)
        // The ground field's shadow-top plane (R8, same uv): the ABSOLUTE
        // height each shadowed texel's sun ray was blocked at, decoded via
        // lm_top_decode. A vertex above the blocker rejects the ground's
        // shadow — a head clears a fence rail's shadow while the shins keep
        // it, and a jump rises out of a roof's shadow at roof height.
        top_map: texture_2d(float)
        lm_rect: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        lm_world: uniform(vec4(0.0, 0.0, 1.0, 1.0))
        // Decode for top_map: absolute blocked height = x + byte * y.
        lm_top_decode: uniform(vec4(0.0, 8.0, 0.0, 0.0))
        // xy = ground-region uv, z = in-field gate, w = TRUE world height
        // of the vertex (for the shadow-top comparison).
        v_lmg: varying(vec4f)
        // Realtime cascades (see DrawSceneCube's block — same contract).
        // v_csm = (true world position, N.L).
        csm_map: texture_depth(float)
        v_csm: varying(vec4f)
        v_csm_n: varying(vec3f)
        v_ambient: varying(vec3f)
        v_direct: varying(vec3f)
        v_uv: varying(vec2f)
        v_ao_uv: varying(vec2f)
        world: varying(vec4f)
        v_fog: varying(float)
        // Per-frame dynamic lights, up to 8, summed in the VERTEX stage.
        // Characters are always dynamic, so unlike DrawSceneSkinned there is
        // no static gate: every slot counts — street lamps, firework
        // flashes and host lights alike (renderer.rs write_light_uniforms).
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

        oct_decode: fn(e: vec2f) -> vec3f {
            let nz = 1.0 - abs(e.x) - abs(e.y)
            let t = max(0.0 - nz, 0.0)
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return normalize(vec3(e.x - t * sx, e.y - t * sy, nz))
        },

        ..mod.draw.SunCascades,

        // Same term as DrawSceneSkinned: (1 - d/r)^2 falloff, spot factor
        // mirroring lightmap.rs's lamp pass (SPILL = 0.35, emission axis
        // straight down), empty slots rejected on radius.
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
            dl = dl + self.dl_term(wp, n, self.dl_pos0, self.dl_col0)
            dl = dl + self.dl_term(wp, n, self.dl_pos1, self.dl_col1)
            dl = dl + self.dl_term(wp, n, self.dl_pos2, self.dl_col2)
            dl = dl + self.dl_term(wp, n, self.dl_pos3, self.dl_col3)
            dl = dl + self.dl_term(wp, n, self.dl_pos4, self.dl_col4)
            dl = dl + self.dl_term(wp, n, self.dl_pos5, self.dl_col5)
            dl = dl + self.dl_term(wp, n, self.dl_pos6, self.dl_col6)
            dl = dl + self.dl_term(wp, n, self.dl_pos7, self.dl_col7)
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

        // One palette row by flat texel index. sample_nearest with explicit
        // lod: the vertex stage cannot use implicit gradients, and RGBA32F is
        // not linearly filterable on every GLES/WebGPU device — nearest at
        // texel centres asks nothing of the filter.
        jrow: fn(t: float) -> vec4f {
            let dim = self.joint_tex.size()
            let y = floor(t / dim.x)
            let x = t - y * dim.x
            return self.joint_tex.sample_nearest(
                vec2((x + 0.5) / dim.x, (y + 0.5) / dim.y),
                0.0
            )
        }

        affine_normal: fn(r0: vec3, r1: vec3, r2: vec3, n: vec3) -> vec3 {
            let co0 = cross(r1, r2)
            let det = dot(r0, co0)
            if abs(det) < 0.00000001 { return vec3(dot(r0,n),dot(r1,n),dot(r2,n)) }
            return vec3(dot(co0,n),dot(cross(r2,r0),n),dot(cross(r0,r1),n)) / det
        }

        vertex: fn() {
            var rest = vec4(self.geom.px, self.geom.py, self.geom.pz, 1.0)
            var rn = self.oct_decode(unpack2f16(self.geom.nrm))
            if self.morph_ctl.w>0.5{rest=vec4(rest.xyz+self.morph_delta(self.geom.source_vertex,0.0),1.0);rn=normalize(rn+self.morph_delta(self.geom.source_vertex,1.0))}
            self.v_fur_root = vec3(self.geom.px, self.geom.py, self.geom.pz)
            self.v_fur_normal = self.oct_decode(unpack2f16(self.geom.nrm))
            rest = vec4(rest.xyz + rn * (self.fur.x * self.fur_layer.x), 1.0)
            let jj = unpack4u8(self.geom.joints)
            let jw = unpack4u8(self.geom.weights)
            var pos = vec3(0.0, 0.0, 0.0)
            var nrm = vec3(0.0, 0.0, 0.0)
            // Up to 4 influences; these rigs carry 1-2 on most vertices, so
            // the zero-weight branches skip their fetches.
            if jw.x > 0.0 {
                let b = self.joint_base + floor(jj.x * 255.0 + 0.5) * 3.0
                let r0 = self.jrow(b)
                let r1 = self.jrow(b + 1.0)
                let r2 = self.jrow(b + 2.0)
                pos = pos + vec3(dot(r0, rest), dot(r1, rest), dot(r2, rest)) * jw.x
                nrm = nrm + self.affine_normal(r0.xyz, r1.xyz, r2.xyz, rn) * jw.x
            }
            if jw.y > 0.0 {
                let b = self.joint_base + floor(jj.y * 255.0 + 0.5) * 3.0
                let r0 = self.jrow(b)
                let r1 = self.jrow(b + 1.0)
                let r2 = self.jrow(b + 2.0)
                pos = pos + vec3(dot(r0, rest), dot(r1, rest), dot(r2, rest)) * jw.y
                nrm = nrm + self.affine_normal(r0.xyz, r1.xyz, r2.xyz, rn) * jw.y
            }
            if jw.z > 0.0 {
                let b = self.joint_base + floor(jj.z * 255.0 + 0.5) * 3.0
                let r0 = self.jrow(b)
                let r1 = self.jrow(b + 1.0)
                let r2 = self.jrow(b + 2.0)
                pos = pos + vec3(dot(r0, rest), dot(r1, rest), dot(r2, rest)) * jw.z
                nrm = nrm + self.affine_normal(r0.xyz, r1.xyz, r2.xyz, rn) * jw.z
            }
            if jw.w > 0.0 {
                let b = self.joint_base + floor(jj.w * 255.0 + 0.5) * 3.0
                let r0 = self.jrow(b)
                let r1 = self.jrow(b + 1.0)
                let r2 = self.jrow(b + 2.0)
                pos = pos + vec3(dot(r0, rest), dot(r1, rest), dot(r2, rest)) * jw.w
                nrm = nrm + self.affine_normal(r0.xyz, r1.xyz, r2.xyz, rn) * jw.w
            }
            let model_view = self.draw_list.view_transform * self.transform
            let world_normal = normalize((model_view * vec4(nrm.x, nrm.y, nrm.z, 0.0)).xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(world_normal, normalize(self.light_dir)), 0.0)
            let hemi = clamp(world_normal.y * 0.5 + 0.5, 0.0, 1.0)
            self.v_ambient = mix(self.sun_ground, self.sun_sky, hemi)
            self.v_direct = self.sun_color * dp
            // Dynamic lights in TRUE world space (light positions are world
            // coordinates; the stage/view transform must not move them).
            let dl_wp = (self.transform * vec4(pos.x, pos.y, pos.z, 1.0)).xyz
            let dl_n = normalize((self.transform * vec4(nrm.x, nrm.y, nrm.z, 0.0)).xyz)
            self.v_dl = self.dl_sum(dl_wp, dl_n)
            // Ground-field uv for the baked sun shadow. The field stores
            // GROUND-level visibility, so the sample is projected ALONG THE
            // SUN RAY from this vertex down to the character's ground plane
            // (per-instance ground_y): a vertex at height h is shadowed iff
            // the sun ray through it lands on shadowed ground. The shadow
            // boundary slants across the body as they walk through it, and
            // a jumping character rises out of it.
            let dl_h = max(dl_wp.y - self.ground_y, 0.0)
            let dl_sun = normalize(self.light_dir)
            let dl_gxz = dl_wp.xz - dl_sun.xz * (dl_h / max(dl_sun.y, 0.2))
            let lgw = max(self.lm_world.zw, vec2(0.000001, 0.000001))
            let lgraw = (dl_gxz - self.lm_world.xy) / lgw
            let lgf = clamp(lgraw, vec2(0.0, 0.0), vec2(1.0, 1.0))
            let lg_in = step(0.000001, self.lm_rect.z)
                * step(0.0, lgraw.x) * step(lgraw.x, 1.0)
                * step(0.0, lgraw.y) * step(lgraw.y, 1.0)
            let lg_uv = self.lm_rect.xy + lgf * self.lm_rect.zw
            self.v_lmg = vec4(lg_uv.x, lg_uv.y, lg_in, dl_wp.y)
            self.v_csm = vec4(dl_wp.x, dl_wp.y, dl_wp.z, dp)
            self.v_csm_n = dl_n
            self.v_color=unpack4u8(self.geom.color)
            self.v_uv = unpack2f16(self.geom.uv)
            // ao_uv is unorm16x2 (model.rs pack_ao_uv), NOT an f16 pair — f16
            // spacing near 1.0 is a full texel of the atlas. Each axis is
            // (lo + 256*hi)/257 of the two unpacked bytes: 255*257 = 65535.
            // Rest-pose bake, valid in every pose: the crevice moves with
            // the surface because the topology never changes.
            let ao_uv_b = unpack4u8(self.geom.ao_uv)
            self.v_ao_uv = vec2(
                (ao_uv_b.x + ao_uv_b.y * 256.0) / 257.0,
                (ao_uv_b.z + ao_uv_b.w * 256.0) / 257.0
            )
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        orm_map: texture_2d(float)
        normal_map: texture_2d(float)
        occlusion_map: texture_2d(float)
        emissive_map: texture_2d(float)

        // Toy gloss on a character (fur_layer.y packs rim * 255 * 65536 +
        // clearcoat * 255 * 256 + flake * 255, the PBR lane's layout): a
        // clear lacquer that reflects the sky gradient with a tight sun
        // glint, and the soft studio rim that separates a silhouette.
        toy_coat: fn(base: vec3, n: vec3, v: vec3, l: vec3, sun: vec3, amb_occ: float) -> vec3 {
            let packed = floor(self.fur_layer.y + 0.5)
            let rim = floor(packed / 65536.0)
            let coat = floor((packed - rim * 65536.0) / 256.0) / 255.0
            let ndv = max(dot(n, v), 0.0001)
            let r = n * (2.0 * ndv) - v
            let fc = (0.04 + 0.96 * pow(1.0 - ndv, 5.0)) * coat
            let env = mix(self.sun_ground, self.sun_sky * 1.6, smoothstep(-0.2, 0.4, r.y))
            let h = normalize(l + v)
            let ndh = max(dot(n, h), 0.0)
            let den = ndh * ndh * (0.0016 - 1.0) + 1.0
            let glint = 0.0016 / max(3.14159265 * den * den, 0.0001) * 0.25
            let e = 1.0 - ndv
            let back = 0.35 + 0.65 * max(0.0 - dot(l, v), 0.0)
            let edge = (self.sun_color * (back * mix(1.0, 3.14159265, self.lin_ctl.x) * 0.45) + self.sun_sky * 0.8) * (e * e * e * amb_occ * rim / 255.0)
            return base * (1.0 - fc) + env * (fc * amb_occ) + sun * (glint * fc) + edge
        }
        surface_linear: fn(v:vec3)->vec3 {return mix(v/12.92,pow((v+vec3(0.055,0.055,0.055))/1.055,vec3(2.4,2.4,2.4)),step(vec3(0.04045,0.04045,0.04045),v))}
        surface_display: fn(v:vec3)->vec3 {return mix(v*12.92,1.055*pow(max(v,vec3(0.0,0.0,0.0)),vec3(0.4166667,0.4166667,0.4166667))-vec3(0.055,0.055,0.055),step(vec3(0.0031308,0.0031308,0.0031308),v))}
        pixel: fn() {
            if self.fur_mask() < 0.5 { discard() }
            let tex = self.tex.sample_as_bgra_repeat(self.v_uv)
            let alpha=tex.w*self.v_color.w*self.material_alpha
            if self.surface_on>0.5 && self.alpha_mode>0.5 && self.alpha_mode<1.5 && alpha<self.alpha_cutoff{discard()}
            let albedo = self.color_adjust(self.to_lin(tex.xyz)*self.to_lin(self.v_color.xyz), self.tint, self.color_adjust_ctl)
            // Same occlusion idiom as DrawSceneSkinned: per-fragment atlas
            // sample, world-anchored hash dither against 8-bit banding,
            // ambient scaled fully and direct partially — a crease should
            // read as a crease even in sunlight, but never darken a lit
            // wall twice.
            let baked = self.ao_map.sample(self.v_ao_uv).x
            let hash = fract(
                sin(dot(self.world.xy + self.world.zz, vec2(12.9898, 78.233))) * 43758.5453
            )
            let ao = clamp(baked + (hash - 0.5) * 0.03, 0.0, 1.0)
            // AO takes some of the DIRECT term too (a crease reads in sunlight);
            // the HDR lane keeps less of that stylisation — its real shadows
            // and fill already carry the contrast.
            let ao_direct = mix(1.0, ao, mix(0.75, 0.35, self.lin_ctl.x))
            // OnChange: baked sun shadow off the ground field gates the
            // DIRECT term only; A channel only — lamps come from v_dl,
            // never from the field's RGB (that would double-light under
            // every pole). The shadow-top plane rejects the ground's shadow
            // for vertices ABOVE the blocker along the sun ray: a fence
            // rail shades the shins, never the head over it.
            let lmg = self.light_map.sample_as_bgra(self.v_lmg.xy)
            let top_g = self.lm_top_decode.x
                + self.top_map.sample(self.v_lmg.xy).x * self.lm_top_decode.y
            let occ_g = 1.0 - smoothstep(top_g - 0.15, top_g + 0.15, self.v_lmg.w)
            // Realtime: the per-frame cascades replace the ground path
            // entirely — characters receive (and cast) through the same
            // maps as every other surface.
            let sun_vis = mix(
                mix(1.0, smoothstep(0.2, 0.8, lmg.w), self.v_lmg.z * occ_g),
                self.csm_vis(self.v_csm.xyz, self.v_csm_n, self.v_csm.w),
                self.csm_p.x
            )
            // A character standing in a lamp pool inside a building's shadow
            // gets the same fill the ground under its feet does, or it would
            // read as the one thing in the pool the lamp failed to light —
            // lightmap::lamp_shadow_fill.
            var local = self.v_dl
            if self.cluster_on > 0.5 { local = self.cluster_sum(self.v_csm.xyz, self.v_csm_n) }
            let lit = albedo * (
                self.gi_ambient(self.v_csm.xyz,self.v_csm_n,self.v_ambient) * ao
                    + (self.v_direct * self.sun_filled(sun_vis, local) + local)
                        * ao_direct
            )
            if self.surface_on>0.5 {
                let base=self.color_adjust(self.surface_linear(tex.xyz)*self.v_color.xyz,self.tint,self.color_adjust_ctl)
                var n=normalize(self.v_csm_n)
                let view=normalize(self.eye-self.v_csm.xyz)
                if self.double_sided>0.5 && dot(n,view)<0.0{n=n*(-1.0)}
                let dp1=dFdx(self.v_csm.xyz)
                let dp2=dFdy(self.v_csm.xyz)
                let du1=dFdx(self.v_uv)
                let du2=dFdy(self.v_uv)
                let determinant=du1.x*du2.y-du1.y*du2.x
                if abs(determinant)>0.00000001 && abs(self.normal_scale)>0.00001 {
                    let orientation=sign(determinant)
                    let tangent=normalize(dp1*du2.y-dp2*du1.y)*orientation
                    let bitangent=normalize(dp2*du1.x-dp1*du2.x)*orientation
                    // Tangent-space X and Y; Z is rebuilt (BC5 normal maps store only XY).
                    let xy=self.normal_map.sample_as_bgra_repeat(self.v_uv).xy*2.0-vec2(1.0,1.0)
                    let mapped=vec3(xy.x,xy.y,sqrt(max(1.0-dot(xy,xy),0.0)))
                    n=normalize(tangent*(mapped.x*self.normal_scale)+bitangent*(mapped.y*self.normal_scale)+n*mapped.z)
                }
                let orm=self.orm_map.sample_as_bgra_repeat(self.v_uv)
                let rough=clamp(self.roughness*orm.y,0.045,1.0)
                let metal=clamp(self.metallic*orm.z,0.0,1.0)
                let light=normalize(self.light_dir)
                let halfdir=normalize(light+view)
                let ndv=max(dot(n,view),0.0001)
                let ndl=max(dot(n,light),0.0)
                let ndh=max(dot(n,halfdir),0.0)
                let vdh=max(dot(view,halfdir),0.0)
                let f0=mix(vec3(0.04,0.04,0.04),base,metal)
                let f=f0+(vec3(1.0,1.0,1.0)-f0)*pow(1.0-vdh,5.0)
                let a2=rough*rough*rough*rough
                let denominator=ndh*ndh*(a2-1.0)+1.0
                let distribution=a2/max(3.14159265*denominator*denominator,0.000001)
                let k=(rough+1.0)*(rough+1.0)*0.125
                let geometry=(ndv/max(ndv*(1.0-k)+k,0.0001))*(ndl/max(ndl*(1.0-k)+k,0.0001))
                let spec=f*(distribution*geometry/max(4.0*ndv*ndl,0.0001))
                let diffuse=(vec3(1.0,1.0,1.0)-f)*base*((1.0-metal)/3.14159265)
                // Sun and fill arrive as irradiance/pi units: the HDR lane
                // restores the pi the 1/pi BRDF divides out.
                let direct=(diffuse+spec)*self.sun_color*(ndl*sun_vis*ao_direct*mix(1.0,3.14159265,self.lin_ctl.x))
                let reflection=n*(2.0*ndv)-view
                let environment=mix(self.sun_ground,self.sun_sky,clamp(reflection.y*0.5+0.5,0.0,1.0))
                let fresnel=f0+(max(vec3(1.0-rough,1.0-rough,1.0-rough),f0)-f0)*pow(1.0-ndv,5.0)
                let ambient=(base*(1.0-metal)*self.gi_ambient(self.v_csm.xyz,n,mix(self.sun_ground,self.sun_sky,clamp(n.y*0.5+0.5,0.0,1.0)))+environment*fresnel)*ao
                let occlusion=mix(1.0,self.occlusion_map.sample_as_bgra_repeat(self.v_uv).x,self.occlusion_strength)
                let emission=self.surface_linear(self.emissive_map.sample_as_bgra_repeat(self.v_uv).xyz)*self.emissive
                let punctual=self.cluster_pbr(self.v_csm.xyz,n,self.eye,base,rough,metal)
                var result=self.fur_shade(direct+ambient*occlusion+punctual*ao_direct,n,self.eye-self.v_csm.xyz)+emission
                if self.fur_layer.y>0.5{result=self.toy_coat(result,n,view,light,self.sun_color*(ndl*sun_vis*ao_direct*mix(1.0,3.14159265,self.lin_ctl.x)),ao)}
                let coverage=mix(1.0,alpha,step(1.5,self.alpha_mode))
                return self.csm_debug_view(self.gi_display(vec4(mix(mix(self.surface_display(result),result,self.lin_ctl.x),self.fog_color,self.scene_fog(self.v_fog,self.v_csm.xyz,self.fog_density))*coverage,coverage),self.v_csm.xyz,n),self.v_csm.xyz,n)
            }
            return self.csm_debug_view(self.gi_display(vec4(mix(lit, self.fog_color, self.scene_fog(self.v_fog, self.v_csm.xyz, self.fog_density)), 1.0),self.v_csm.xyz,self.v_csm_n),self.v_csm.xyz,self.v_csm_n)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
        morph_map: texture_2d(float)
        morph_delta: fn(vertex:float,lane:float)->vec3f {
            var delta=vec3(0.0,0.0,0.0)
            if self.morph_ctl.w > 0.5 {
                let index=(0.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.x
            }
            if self.morph_ctl.w > 1.5 {
                let index=(1.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.y
            }
            if self.morph_ctl.w > 2.5 {
                let index=(2.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.z
            }
            if self.morph_ctl.w > 3.5 {
                let index=(3.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.w
            }
            if self.morph_ctl.w > 4.5 {
                let index=(4.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.x
            }
            if self.morph_ctl.w > 5.5 {
                let index=(5.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.y
            }
            if self.morph_ctl.w > 6.5 {
                let index=(6.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.z
            }
            if self.morph_ctl.w > 7.5 {
                let index=(7.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.w
            }
            if self.morph_ctl.w > 8.5 {
                let index=(8.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.x
            }
            if self.morph_ctl.w > 9.5 {
                let index=(9.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.y
            }
            if self.morph_ctl.w > 10.5 {
                let index=(10.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.z
            }
            if self.morph_ctl.w > 11.5 {
                let index=(11.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.w
            }
            if self.morph_ctl.w > 12.5 {
                let index=(12.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.x
            }
            if self.morph_ctl.w > 13.5 {
                let index=(13.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.y
            }
            if self.morph_ctl.w > 14.5 {
                let index=(14.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.z
            }
            if self.morph_ctl.w > 15.5 {
                let index=(15.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.w
            }
            if self.morph_ctl.w > 16.5 {
                let index=(16.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.x
            }
            if self.morph_ctl.w > 17.5 {
                let index=(17.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.y
            }
            if self.morph_ctl.w > 18.5 {
                let index=(18.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.z
            }
            if self.morph_ctl.w > 19.5 {
                let index=(19.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.w
            }
            if self.morph_ctl.w > 20.5 {
                let index=(20.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.x
            }
            if self.morph_ctl.w > 21.5 {
                let index=(21.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.y
            }
            if self.morph_ctl.w > 22.5 {
                let index=(22.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.z
            }
            if self.morph_ctl.w > 23.5 {
                let index=(23.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.w
            }
            if self.morph_ctl.w > 24.5 {
                let index=(24.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.x
            }
            if self.morph_ctl.w > 25.5 {
                let index=(25.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.y
            }
            if self.morph_ctl.w > 26.5 {
                let index=(26.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.z
            }
            if self.morph_ctl.w > 27.5 {
                let index=(27.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.w
            }
            if self.morph_ctl.w > 28.5 {
                let index=(28.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.x
            }
            if self.morph_ctl.w > 29.5 {
                let index=(29.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.y
            }
            if self.morph_ctl.w > 30.5 {
                let index=(30.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.z
            }
            if self.morph_ctl.w > 31.5 {
                let index=(31.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.w
            }
            return delta
        }

    }
}
