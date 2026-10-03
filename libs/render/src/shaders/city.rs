//! DrawSceneCity: the streamed world's surfaces (glass, paint, lamps).

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // A sibling of DrawSceneCustom: DrawScenePbr's vertex stage, cascades,
    // clustered lights and sky environment, with its own pixel fn for the
    // materials a city is made of (shaders.rs DrawSceneCity for the kinds
    // and the ORM channels). Everything here is per pixel and texture-free
    // beyond the albedo, detail and ORM maps the layer already binds:
    //   * glass: a reflection of the sky and of a skyline of neighbouring
    //     towers (a hashed silhouette around the compass), Schlick Fresnel,
    //     per-pane flatness error so a curtain wall breaks into panes, and
    //     behind it a ROOM (interior mapping: back wall, side walls, floor,
    //     ceiling with a light, blinds) that lights up on its own hour;
    //   * paint: a base lobe under a clear coat with its own reflection;
    //   * emissive: lamps, lenses and posters, brighter after dark;
    //   * plain: roughness and metal per texel (wet asphalt, steel).
    mod.draw.DrawSceneCity = mod.std.set_type_default() do #(DrawSceneCity::script_shader(vm)){
        ..mod.draw.DrawScenePbr
        // x = night factor (0 day .. 1 night: window hours, lamp emission),
        // y = the stream clock. One per frame, so a uniform: as an instance
        // lane it took the lane one output register past what D3D11's
        // `vs_5_0` allows.
        city: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        c_hash: fn(p: vec2) -> float {
            return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453)
        }

        // GGX specular for one light direction (the sun), no Fresnel.
        c_ggx: fn(n: vec3, v: vec3, l: vec3, rough: float) -> float {
            let h = normalize(l + v)
            let ndv = max(dot(n, v), 0.0001)
            let ndl = max(dot(n, l), 0.0001)
            let ndh = max(dot(n, h), 0.0001)
            let a2 = rough * rough * rough * rough
            let den = ndh * ndh * (a2 - 1.0) + 1.0
            let dist = a2 / max(3.14159265 * den * den, 0.0001)
            let k = (rough + 1.0) * (rough + 1.0) * 0.125
            let geo = (ndv / max(ndv * (1.0 - k) + k, 0.0001)) * (ndl / max(ndl * (1.0 - k) + k, 0.0001))
            return dist * geo / max(4.0 * ndv * ndl, 0.0001)
        }

        // The environment a glossy city surface reflects: the sky, and
        // below a hashed skyline elevation the towers across the street
        // (lit windows after dark), then the street below the horizon.
        c_reflect: fn(r: vec3, rough: float, night: float, grid: float) -> vec3 {
            let sky = self.sky_env(r, rough)
            let az = atan2(r.z, r.x) * 9.0
            let col = floor(az)
            let hb = self.c_hash(vec2(col, 3.7))
            let top = 0.03 + hb * hb * hb * 0.8
            let elev = r.y / max(length(vec2(r.x, r.z)), 0.001)
            // Soft silhouette edges as the reflection blurs.
            let inb = 1.0 - smoothstep(top - 0.02 - rough, top + 0.02 + rough, elev)
            let lit_side = max(dot(normalize(vec3(r.x, 0.0, r.z)) * (0.0 - 1.0), normalize(self.light_dir)), 0.0)
            // Each reflected tower its own value and sunlit side.
            let hv = self.c_hash(vec2(col, 9.1))
            var bcol = mix(self.sun_ground * 0.4 + self.fog_color * 0.1, self.fog_color * 0.55 + self.sun_color * 0.04, lit_side * hv) * (0.6 + 0.8 * hv)
            // A window grid on the reflected tower, lit after dark.
            let wg = vec2(fract(az * 3.0), fract(elev * 18.0))
            let win = step(0.3, wg.x) * step(0.35, wg.y)
            let on = step(0.62, self.c_hash(vec2(floor(az * 3.0), floor(elev * 18.0) + col)))
            // `grid` 0 on paint: a flat panel mirroring a regular window
            // grid reads as a printed texture, not a reflection.
            bcol = bcol * (1.0 - 0.3 * win * grid) + vec3(1.0, 0.72, 0.42) * (win * on * night * 0.35 * grid)
            return mix(sky, bcol, inb * (1.0 - rough * 0.7))
        }

        pixel: fn() {
            // Streamed-LOD crossfade (stream.rs `Dither`), as DrawScenePbr.
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
            var tex = self.base_texel()
            if tex.w < 0.5 { self.clip() }
            let kind = self.metallic
            let night = self.city.x
            var orm = self.orm_map.sample_repeat(self.v_uv)
            let pos = self.v_csm.xyz
            let to_eye = self.eye.xyz - pos
            let dist = length(to_eye)
            let v = to_eye / max(dist, 0.0001)
            let ng = normalize(self.v_csm_n)
            // Recessed windows (facades, near): the glass sits 22 cm behind
            // the wall plane. Inside a window opening the pixel sees the
            // glass where the view ray meets it (a parallax shift of the
            // facade UVs, ~14.4 m a repeat), and where that ray meets the
            // opening's side instead, the reveal, in its own shadow.
            var reveal = 0.0
            if kind > 0.5 && kind < 1.5 && orm.x > 0.5 && dist < 140.0 {
                var tx = cross(vec3(0.0, 1.0, 0.0), ng)
                tx = tx / max(length(tx), 0.0001)
                let rd = v * (0.0 - 1.0)
                let into = max(dot(rd, ng * (0.0 - 1.0)), 0.08)
                let k = 0.22 / 14.4 / into * (1.0 - smoothstep(90.0, 140.0, dist))
                let uv2 = self.v_uv + vec2(dot(rd, tx) * k, (0.0 - rd.y) * k)
                let orm2 = self.orm_map.sample_repeat(uv2)
                let tex2 = self.tex.sample_repeat(uv2)
                reveal = 1.0 - orm2.x
                orm = vec4(orm2.x, mix(orm2.y, 0.8, reveal), mix(orm2.z, 0.0, reveal), orm2.w)
                tex = vec4(tex2.xyz, mix(tex2.w, 1.0, reveal))
            }

            // Albedo: texture x vertex colour x (weighted) instance tint.
            let is_facade = step(0.5, kind) * step(kind, 2.5)
            let tw = orm.x * (1.0 - is_facade)
            let tintc = mix(vec3(1.0, 1.0, 1.0), self.tint.xyz, tw)
            var albedo = self.to_scene(tex.xyz) * self.to_lin(self.v_tint.xyz) * tintc
            var n = ng
            if self.detail_st.x > 0.001 {
                let det = self.detail_map.sample_repeat(self.v_uv * self.detail_st)
                albedo = albedo * det.xyz * 2.0
                // Bump from the grain (surface-gradient bump mapping on the
                // screen derivatives): masonry reads as relief under a low
                // sun. Faded out before the grain is sub-pixel.
                // Walls only: on the ground the grain's bump glints like
                // frost under a high sun.
                let near = (1.0 - smoothstep(25.0, 70.0, dist)) * (1.0 - abs(ng.y))
                if near > 0.001 {
                    let hl = dot(det.xyz, vec3(0.333, 0.333, 0.333))
                    let dpx = dFdx(pos)
                    let dpy = dFdy(pos)
                    let r1 = cross(dpy, ng)
                    let r2 = cross(ng, dpx)
                    let dt = dot(dpx, r1)
                    let grad = (r1 * dFdx(hl) + r2 * dFdy(hl)) * sign(dt)
                    n = normalize(ng * abs(dt) - grad * (0.035 * near))
                }
            }

            var rough = clamp(orm.y, 0.045, 1.0)
            if kind < 0.5 && orm.w > 0.01 {
                // Puddles in world space (no tile repeats): standing water
                // is dark, mirror-smooth and flat.
                let wn = self.tn_noise(pos.xz * 0.03) * 0.75 + self.tn_noise(pos.xz * 0.13) * 0.25
                let wet = smoothstep(0.64, 0.72, wn) * orm.w
                rough = mix(rough, 0.05, wet)
                albedo = albedo * (1.0 - 0.45 * wet)
                // A slow, faint ripple (city.y is the stream clock): the
                // mirrored sky wavers the way a puddle's does.
                let t = self.city.y
                let rp = pos.xz * 1.7
                let rx = self.tn_noise(rp + vec2(t * 0.35, 0.0)) - self.tn_noise(rp + vec2(3.1, 1.7 + t * 0.29))
                let rz = self.tn_noise(rp * 1.3 + vec2(5.3 - t * 0.27, 0.0)) - self.tn_noise(rp * 1.3 + vec2(2.2, -t * 0.33))
                n = normalize(mix(n, ng, wet) + vec3(rx, 0.0, rz) * (0.07 * wet))
            }

            // Light, as DrawScenePbr: AO, SSAO, cascades, local lights.
            let hash = fract(sin(dot(self.world.xy + self.world.zz, vec2(12.9898, 78.233))) * 43758.5453)
            let ao = clamp(self.v_tint.w + (hash - 0.5) * 0.03, 0.0, 1.0)
            let ao_direct = mix(1.0, ao, mix(0.75, 0.35, self.lin_ctl.x))
            var sao = 1.0
            if self.ssao_ctl.x > 0.001 {
                let sp = self.v_spos.xy / max(self.v_spos.w, 0.000001)
                let suv = vec2(sp.x * 0.5 + 0.5, 0.5 - sp.y * 0.5)
                sao = 1.0 - (1.0 - self.ssao_map.sample_nearest(suv).x) * self.ssao_ctl.x
            }
            let sun_all = mix(1.0, self.csm_vis(pos, self.v_csm_n, self.v_csm.w), self.csm_p.x)
            var local = self.v_dl
            if self.cluster_on > 0.5 { local = vec3(0.0, 0.0, 0.0) }
            let sun_lit = self.sun_filled(sun_all, local)
            let l = normalize(self.light_dir)
            let ndl = max(dot(n, l), 0.0)
            let pi_hdr = mix(1.0, 3.14159265, self.lin_ctl.x)
            let direct = self.sun_color * (ndl * ao_direct * sun_lit)
            let ambient = self.gi_ambient(pos, n, self.v_ambient) * (ao * sao)
            let ndv = max(dot(n, v), 0.0001)
            let refl = n * (2.0 * ndv) - v

            var lit = vec3(0.0, 0.0, 0.0)
            var metal = orm.z
            if kind > 3.5 {
                // Emissive: lamps, lenses, lit posters.
                let e = orm.y * 3.0 + orm.z * night * 9.0
                lit = albedo * (ambient + direct) + albedo * e
            } else {
                let glass = orm.x * is_facade
                // Opaque part: frames, walls, paint base, plain surfaces.
                if kind > 2.5 {
                    // Paint: metallic flake under the base lobe.
                    metal = orm.z
                }
                var fm = metal
                if is_facade > 0.5 { fm = orm.z * (1.0 - glass) }
                let f0 = mix(vec3(0.04, 0.04, 0.04), albedo, fm)
                let fv = f0 + (vec3(1.0, 1.0, 1.0) - f0) * self.pow5(1.0 - ndv)
                let spec = self.c_ggx(n, v, l, rough) * pi_hdr
                let smooth3 = 1.0 - rough
                let fr = max(vec3(smooth3, smooth3, smooth3), f0)
                // Rough dielectrics keep little of the grazing sheen (the
                // split-sum's scale term falls with roughness).
                let f_env = (f0 + (fr - f0) * self.pow5(1.0 - ndv)) * (1.0 - 0.9 * rough * (1.0 - fm))
                var env = self.sky_env(normalize(refl), rough)
                // Streets and pavements (plain) mirror the real sky only: a
                // hashed skyline of lit windows pasted on the road read as a
                // printed picture. Glass and paint keep the skyline.
                if rough < 0.6 && kind > 0.5 { env = self.c_reflect(normalize(refl), rough, night, 1.0 - step(2.5, kind)) }
                let local_pbr = self.cluster_pbr(pos, n, self.eye.xyz, albedo, rough, fm)
                lit = albedo * ((1.0 - fm) * (ambient + direct + local * ao_direct))
                    + direct * spec * fv + env * f_env * (ao * sao) + local_pbr * ao_direct
                if kind > 2.5 {
                    // Clear coat: a second, near-mirror interface on top.
                    let cf = 0.04 + 0.96 * self.pow5(1.0 - ndv)
                    let cenv = self.c_reflect(normalize(refl), 0.12, night, 0.0)
                    let cspec = self.c_ggx(n, v, l, 0.06) * pi_hdr
                    lit = lit * (1.0 - cf) + (cenv * (ao * sao) + direct * cspec) * cf
                }
                // The reveal faces sideways, out of the sun: mostly sky fill.
                lit = lit * (1.0 - 0.5 * reveal)
                // Lit-window hours: the albedo alpha below 1 is a window's
                // lit mask; a window lights once it clears a threshold that
                // falls from 0.82 by day to 0.42 at night.
                let m = clamp((1.0 - tex.w) * 2.0, 0.0, 1.0)
                let on = smoothstep(mix(0.82, 0.42, night), mix(0.86, 0.5, night), m)
                let hue = fract(m * 7.13)
                let warm = mix(mix(vec3(1.0, 0.62, 0.3), vec3(1.0, 0.8, 0.55), hue), vec3(0.7, 0.8, 1.0), step(0.8, hue))
                if glass > 0.004 {
                    // Pane identity: the facade UVs tile 4 x 4 windows; the
                    // wall plane keeps neighbours apart.
                    let cell = floor(self.v_uv * vec2(4.0, 4.0))
                    let plane = floor(dot(pos, ng) * 0.5)
                    let pid = self.c_hash(cell + vec2(plane * 0.371, plane * 1.137))
                    var tax = cross(vec3(0.0, 1.0, 0.0), ng)
                    tax = tax / max(length(tax), 0.0001)
                    // Flatness error: every pane tilts a fraction of a degree.
                    let gn = normalize(ng + tax * ((pid - 0.5) * 0.035) + vec3(0.0, (fract(pid * 7.31) - 0.5) * 0.035, 0.0))
                    let gdv = max(dot(gn, v), 0.0001)
                    let grefl = gn * (2.0 * gdv) - v
                    let grough = clamp(orm.y, 0.03, 1.0)
                    let gf0 = mix(0.04, 0.5, orm.z)
                    let gf = gf0 + (1.0 - gf0) * self.pow5(1.0 - gdv)
                    let genv = self.c_reflect(normalize(grefl), grough, night, is_facade * (1.0 - step(1.5, kind)))
                    let gspec = self.c_ggx(gn, v, l, grough) * pi_hdr
                    // What is behind the glass.
                    var room = albedo * (ambient * 0.6)
                    var glow = on * mix(0.3, 0.8, night) * mix(0.45, 1.0, m)
                    var roomed = 0.0
                    if kind < 1.5 && dist < 220.0 {
                        // Interior mapping: a 3.6 x 3.6 x (3-6) m room behind
                        // each cell, entered where this pixel is.
                        let cu = fract(self.v_uv.x * 4.0)
                        let cv = 1.0 - fract(self.v_uv.y * 4.0)
                        let rd = v * (0.0 - 1.0)
                        let da = dot(rd, tax)
                        let db = rd.y
                        let dc = max(dot(rd, ng * (0.0 - 1.0)), 0.05)
                        let depth = 3.0 + pid * 3.0
                        let pa = cu * 3.6
                        let pb = cv * 3.6
                        let sa = step(0.0, da)
                        let sb = step(0.0, db)
                        let ta = (sa * 3.6 - pa) / (da + mix(0.0 - 0.0001, 0.0001, sa))
                        let tb = (sb * 3.6 - pb) / (db + mix(0.0 - 0.0001, 0.0001, sb))
                        let tc = depth / dc
                        let t = min(ta, min(tb, tc))
                        let hp = vec3(pa + da * t, pb + db * t, dc * t)
                        let wallc = mix(vec3(0.55, 0.52, 0.48), vec3(0.42, 0.46, 0.5), fract(pid * 3.7))
                        var rc = wallc * 0.8
                        // Back wall: a picture / shelf band.
                        if t >= tc - 0.001 {
                            let band = step(1.1, hp.y) * step(hp.y, 1.8) * step(0.6, hp.x) * step(hp.x, 3.0)
                            rc = mix(wallc, wallc * 0.45, band * step(0.4, fract(pid * 5.3)))
                        }
                        if t >= tb - 0.001 && t < tc - 0.001 {
                            // Floor dark, ceiling bright with a light panel.
                            let panel = step(1.2, hp.x) * step(hp.x, 2.4) * step(1.0, hp.z) * step(hp.z, depth - 1.0)
                            rc = mix(vec3(0.16, 0.13, 0.11), vec3(0.72, 0.72, 0.7) + vec3(1.2, 1.1, 0.95) * (panel * on), sb)
                        }
                        if t >= ta - 0.001 && t < tb - 0.001 && t < tc - 0.001 { rc = wallc * 0.62 }
                        // Depth falloff, then blinds across the pane's top.
                        rc = rc * (1.0 - 0.1 * min(hp.z, 5.0))
                        let blind = step(0.55, fract(pid * 11.3)) * fract(pid * 17.9) * 0.7
                        rc = mix(rc, vec3(0.78, 0.74, 0.66), step(1.0 - blind, cv))
                        room = rc * (ambient * 0.35 + self.sun_color * 0.02)
                        let fade = smoothstep(120.0, 220.0, dist)
                        roomed = 1.0 - fade
                        room = mix(room, albedo * (ambient * 0.6), fade)
                        if on > 0.001 { room = room + rc * warm * (glow * 2.0) * roomed }
                    }
                    let cap = max(max(lit.x, max(lit.y, lit.z)) * 3.0, 0.12)
                    let gl = warm * glow
                    room = room + gl * (min(1.0, cap / max(gl.x, 0.0001)) * (1.0 - roomed))
                    let glass_lit = room * (1.0 - gf) + genv * (gf * ao) + direct * (gspec * gf)
                    lit = mix(lit, glass_lit, glass)
                } else {
                    let glow = warm * (on * mix(0.3, 0.8, night) * mix(0.45, 1.0, m))
                    let cap = max(max(lit.x, max(lit.y, lit.z)) * 3.0, 0.12)
                    lit = lit + glow * min(1.0, cap / max(glow.x, 0.0001))
                }
            }
            return self.csm_debug_view(self.gi_display(vec4(self.scene_fogged(self.to_display(lit), self.v_fog, pos, self.fog_density), 1.0), pos, n), pos, n)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}
