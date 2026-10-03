//! The water surface (local/plans/water.md): one radial mesh around the
//! eye, displaced by the swell, shaded in one pass.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // The water surface. One mesh for every water: rings of vertices around
    // a centre (the eye for a sea, the point of a box nearest the eye for a
    // bounded volume), so detail falls off with distance by construction and
    // the last ring reaches the horizon. Box volumes clamp the vertices into
    // their rectangle.
    //
    // The SWELL displaces it in the vertex stage: the same Gerstner-free
    // sine sum the sim evaluates CPU-side (`sim::water::wave_terms`, pinned
    // by renderer/water_tests.rs), with the sim's raw coefficients as
    // uniforms, so a floating crate rides exactly the surface you see. Each
    // wave fades out where the rings sample it too sparsely to show it.
    //
    // The pixel stage adds ripples from a tiling detail normal map (made on
    // the CPU from the same wind spectrum, SeaLook::detail_map) scrolled in
    // two layers, and shades: Fresnel against a small baked sky texture,
    // absorption through the water column down to the seabed (a terrain
    // height texture), in-scatter, sun glint, crest and shore foam and
    // seabed caustics. Output is premultiplied over the opaque scene.
    //
    // Not culled: a trough seen flat-on, and the whole surface seen from
    // under water, show the mesh's underside.
    mod.draw.DrawSceneWater = mod.std.set_type_default() do #(DrawSceneWater::script_shader(vm)){
        alpha_blend: true
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)
        world: varying(vec4f)
        v_wp: varying(vec3f)
        // Swell slope (dh/dx, dh/dz) and height above level / amplitude sum.
        v_swell: varying(vec4f)
        v_fog: varying(float)
        // Per-volume swell: wave_aN = (dir_x, dir_z, k, omega), wave_bN =
        // (amp, phase, group, 0) — the sim's WaterWave fields unmodified.
        wave_a0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // (centre x, level, centre z, 1 / amplitude sum)
        water_center: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        // (min x, min z, max x, max z): the box; a sea is unbounded.
        water_rect: uniform(vec4(0.0 - 1000000.0, 0.0 - 1000000.0, 1000000.0, 1000000.0))
        // (eye under water 0/1, t = the sim's f32 tick-time, 1 = the scene
        // under this surface absorbs through it itself (uw_*), 1 = drawn
        // from a mesh of world positions instead of rings)
        water_params: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // Look: shallow rgb + clarity (m); deep rgb + caustics.
        sea_shallow: uniform(vec4(0.06, 0.32, 0.30, 7.0))
        sea_deep: uniform(vec4(0.006, 0.05, 0.09, 0.7))
        // (crest foam, shore foam, ripple strength, ripple tile metres)
        sea_foam: uniform(vec4(0.5, 0.5, 1.0, 12.0))
        // Light the liquid gives off (lava), linear rgb.
        sea_glow: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // (wind dir x, wind dir z, ripple drift m/s, 0)
        sea_wind: uniform(vec4(0.0, 1.0, 0.6, 0.0))
        // Seabed: (x0, z0, 1/width, 1/depth) of the height texture, and
        // (lowest height, height range, has seabed, 0).
        bed_rect: uniform(vec4(0.0, 0.0, 1.0, 1.0))
        bed_decode: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        lin_ctl: uniform(vec4(0.0, 1.0, 1.0, 1.0))
        water_eye: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        water_fog: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // Upper-hemisphere sky radiance, equirect, RGBM (rgb · a · 8).
        sky_tex: texture_2d(float)
        // Ripple slopes (x in R, z in G, 0.5 = flat).
        detail_tex: texture_2d(float)
        // Seabed height, 16 bits: high byte in G, low byte in A.
        bed_tex: texture_2d(float)

        // THE swell term, mirrored from sim::water::wave_terms: returns
        // (height, dh/dx, dh/dz). The envelope's derivative is omitted from
        // the slope exactly as it is CPU-side.
        wave_term: fn(p: vec2, wa: vec4, wb: vec4, t: float) -> vec3 {
            let phase = wa.z * (wa.x * p.x + wa.y * p.y) - wa.w * t + wb.y
            var env = 1.0
            if wb.z > 0.0 {
                let e = 0.5 + 0.5 * cos(phase / wb.z)
                env = e * e
            }
            let slope = wb.x * env * cos(phase) * wa.z
            return vec3(wb.x * env * sin(phase), slope * wa.x, slope * wa.y)
        }

        // 1 while a wave's wavelength spans 8+ vertex spacings, 0 at 4.
        wave_fade: fn(k: float, f: float) -> float {
            return clamp(2.0 - k * f / 0.7853982, 0.0, 1.0)
        }

        swell: fn(p: vec2, t: float, f: float) -> vec3 {
            var acc = self.wave_term(p, self.wave_a0, self.wave_b0, t) * self.wave_fade(self.wave_a0.z, f)
            acc = acc + self.wave_term(p, self.wave_a1, self.wave_b1, t) * self.wave_fade(self.wave_a1.z, f)
            acc = acc + self.wave_term(p, self.wave_a2, self.wave_b2, t) * self.wave_fade(self.wave_a2.z, f)
            acc = acc + self.wave_term(p, self.wave_a3, self.wave_b3, t) * self.wave_fade(self.wave_a3.z, f)
            acc = acc + self.wave_term(p, self.wave_a4, self.wave_b4, t) * self.wave_fade(self.wave_a4.z, f)
            acc = acc + self.wave_term(p, self.wave_a5, self.wave_b5, t) * self.wave_fade(self.wave_a5.z, f)
            acc = acc + self.wave_term(p, self.wave_a6, self.wave_b6, t) * self.wave_fade(self.wave_a6.z, f)
            acc = acc + self.wave_term(p, self.wave_a7, self.wave_b7, t) * self.wave_fade(self.wave_a7.z, f)
            return acc
        }

        // PIXEL-stage twin of the slope half of wave_term (a helper is
        // emitted for ONE stage on Metal): the swell's slope per pixel, each
        // wave faded by the pixel's footprint, so far water keeps the long
        // waves its sparse rings cannot displace, and never stripes.
        px_slope: fn(p: vec2, wa: vec4, wb: vec4, t: float, f: float) -> vec2 {
            let phase = wa.z * (wa.x * p.x + wa.y * p.y) - wa.w * t + wb.y
            var env = 1.0
            if wb.z > 0.0 {
                let e = 0.5 + 0.5 * cos(phase / wb.z)
                env = e * e
            }
            let slope = wb.x * env * cos(phase) * wa.z * clamp(2.0 - wa.z * f / 0.7853982, 0.0, 1.0)
            return vec2(slope * wa.x, slope * wa.y)
        }

        swell_slope: fn(p: vec2, t: float, f: float) -> vec2 {
            var s = self.px_slope(p, self.wave_a0, self.wave_b0, t, f)
            s = s + self.px_slope(p, self.wave_a1, self.wave_b1, t, f)
            s = s + self.px_slope(p, self.wave_a2, self.wave_b2, t, f)
            s = s + self.px_slope(p, self.wave_a3, self.wave_b3, t, f)
            s = s + self.px_slope(p, self.wave_a4, self.wave_b4, t, f)
            s = s + self.px_slope(p, self.wave_a5, self.wave_b5, t, f)
            s = s + self.px_slope(p, self.wave_a6, self.wave_b6, t, f)
            s = s + self.px_slope(p, self.wave_a7, self.wave_b7, t, f)
            return s
        }

        vertex: fn() {
            let t = self.water_params.y
            let c = self.water_center
            let r = self.water_rect
            // Ring vertex around the centre, clamped into the box; or, for a
            // mesh-drawn surface (water_params.w = 1), the mesh's own world
            // position and height.
            let g = self.geom.pos_nx.xyz
            let mesh = self.water_params.w
            let xz = mix(clamp(c.xz + g.xz, r.xy, r.zw), g.xz, mesh)
            let base = mix(c.y, g.y, mesh)
            // Ring spacing at this vertex (metres), from the mesh.
            let spacing = max(self.geom.pos_nx.w, 0.05)
            let acc = self.swell(xz, t, spacing)
            let pos = vec3(xz.x, base + acc.x, xz.y)
            self.v_swell = vec4(acc.y, acc.z, acc.x * c.w, spacing)
            self.v_wp = pos
            self.world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        // The sky the water reflects, along a direction (upper hemisphere).
        sky_along: fn(d: vec3) -> vec3 {
            let u = atan2(d.z, d.x) * 0.15915494 + 0.5
            let v = 1.0 - clamp(d.y, 0.0, 1.0)
            let s = self.sky_tex.sample_lod(vec2(u, v), 0.0)
            return s.xyz * (s.w * 8.0)
        }

        // Seabed height under p and whether it is known there (1 / 0).
        bed_at: fn(p: vec2) -> vec2 {
            let uv = (p - self.bed_rect.xy) * self.bed_rect.zw
            let s = self.bed_tex.sample_lod(uv, 0.0)
            let h = self.bed_decode.x + (s.y * 255.0 * 256.0 + s.w * 255.0) / 65535.0 * self.bed_decode.y
            let inside = step(0.0, uv.x) * step(uv.x, 1.0) * step(0.0, uv.y) * step(uv.y, 1.0) * self.bed_decode.z
            return vec2(mix(self.water_center.y - 1000.0, h, inside), inside)
        }

        pixel: fn() {
            let p = self.v_wp
            let t = self.water_params.y
            let hdr = step(0.5, self.lin_ctl.x)
            // Pixel footprint on the water: ripples fade before they alias.
            let foot = max(length(dFdx(p)), length(dFdy(p)))
            let tile = self.sea_foam.w
            let wind = self.sea_wind.xy
            let drift = wind * (t * self.sea_wind.z)
            // Three scales of the ripple map, rotated against each other so
            // no tile repeat lines up; each fades out before it aliases, the
            // largest last, so the far sea keeps a texture that never reads
            // as tiles.
            let s0 = self.detail_tex.sample_repeat(vec2(p.z, 0.0 - p.x) / (tile * 3.1) + drift / (tile * 6.0)).xy - vec2(0.5, 0.5)
            let s1 = self.detail_tex.sample_repeat(vec2(p.x, p.z) / tile + drift / tile).xy - vec2(0.5, 0.5)
            let q = vec2(p.x * 0.8 - p.z * 0.6, p.x * 0.6 + p.z * 0.8)
            let s2 = self.detail_tex.sample_repeat(q / (tile * 0.37) - vec2(drift.y, drift.x) / (tile * 0.3)).xy - vec2(0.5, 0.5)
            let near = clamp(1.5 - foot / (tile * 0.04), 0.0, 1.0)
            let rip = (s0 * clamp(1.5 - foot / (tile * 0.15), 0.0, 1.0) * 0.8 + (s1 * 0.6 + s2 * 0.35) * near) * self.sea_foam.z * 0.3
            let sw = self.swell_slope(p.xz, t, foot)
            let n = normalize(vec3(0.0 - sw.x - rip.x, 1.0, 0.0 - sw.y - rip.y))
            let eye = self.water_eye.xyz
            let v = normalize(eye - p)
            let l = normalize(self.light_dir)
            let under = self.water_params.x
            // Water column below this pixel, and the light path through it.
            let bed = self.bed_at(p.xz)
            let depth = max(self.water_center.y - bed.x, 0.0)
            let clarity = max(self.sea_shallow.w, 0.1)
            let ambient = self.sun_sky + self.sun_ground * 0.5
            let lit = ambient + self.sun_color * max(l.y, 0.0)
            if under > 0.5 {
                // Seen from below: inside Snell's window the world above
                // shows through (already drawn, fogged by the water), a
                // bright rim at its edge; outside it total internal
                // reflection mirrors the deep water.
                let ndv = abs(dot(n, v))
                let win = smoothstep(0.62, 0.7, ndv)
                let rim = (1.0 - abs(win * 2.0 - 1.0)) * 0.6
                let a = 1.0 - win * 0.8
                let rgb = self.sea_deep.xyz * (lit * (1.0 - win)) + self.fog_color * (win * 0.25 + rim)
                return vec4(mix(rgb, self.fog_color * a, self.v_fog), a)
            }
            let ndv = max(dot(n, v), 0.0)
            let fr = 0.02 + 0.98 * pow(1.0 - ndv, 5.0)
            let r = n * (2.0 * dot(n, v)) - v
            var sky = self.sky_along(vec3(r.x, max(r.y, 0.0), r.z))
            sky = mix(self.sun_sky * 2.0 + self.fog_color * 0.5, sky, hdr)
            // Refracted path down to the seabed (cos of the refracted angle).
            let ct = sqrt(1.0 - (1.0 - ndv * ndv) * 0.5625)
            let path = depth / max(ct, 0.2)
            let trans = exp(0.0 - path / clarity)
            // In-scatter: shallow tint near the bed, deep colour beyond, lit
            // by the sky and sun; wave backs glow with the sun behind them.
            let deepness = 1.0 - exp(0.0 - depth / (clarity * 1.5))
            let body = mix(self.sea_shallow.xyz, self.sea_deep.xyz, deepness) * lit
            let back = pow(max(dot(v, vec3(0.0 - l.x, 0.0, 0.0 - l.z)), 0.0), 4.0)
            let sss = self.sea_shallow.xyz * self.sun_color * (back * clamp(self.v_swell.z + 0.3, 0.0, 1.0) * 0.5)
            // Caustics on the seabed seen through the water: where the two
            // ripple layers agree, light focuses.
            let caus = pow(clamp(1.0 - length(s1 - s2) * 2.4, 0.0, 1.0), 5.0)
                * self.sea_deep.w * clamp(1.0 - depth / (clarity * 2.0), 0.0, 1.0) * step(0.01, depth)
            let seabed_light = self.sun_color * (caus * max(l.y, 0.0) * 1.5 * trans)
            // Glint: a tight GGX lobe the bloom picks up.
            let h = normalize(l + v)
            let ndh = max(dot(n, h), 0.0)
            let a2 = 0.0016
            let den = ndh * ndh * (a2 - 1.0) + 1.0
            let glint = self.sun_color * (a2 / max(den * den, 0.000001) * 0.25 * fr * max(dot(n, l), 0.0))
            // Foam: whitecaps on the highest, steepest crests, broken up by
            // the ripples; surf bands running up the shallows.
            let fn1 = s1.x + s2.y
            let crest = smoothstep(0.3, 0.8, self.v_swell.z * 0.8 + length(sw) * 3.0 + fn1 * 0.8) * self.sea_foam.x
            let shore = (1.0 - smoothstep(0.0, 1.4, depth)) * self.sea_foam.y
            let band = 0.5 + 0.5 * sin(depth * 5.0 - t * 1.3 + fn1 * 4.0)
            let foam = clamp(max(crest, shore * smoothstep(0.35, 0.85, band) + shore * 0.25 * step(depth, 0.25)), 0.0, 1.0)
            // Premultiplied. Where the scene behind absorbs through this
            // water itself (water_params.z: the clustered mixin's uw_*, over
            // a known seabed) the surface only reflects; elsewhere the
            // column hides what is behind it by 1 - transmittance.
            let opacity = (1.0 - trans) * (1.0 - self.water_params.z * bed.y)
            var rgb = body * (opacity * (1.0 - fr)) + sky * fr + glint + (seabed_light + sss) * (1.0 - fr)
            var a = opacity * (1.0 - fr) + fr
            rgb = mix(rgb, lit * 0.9, foam)
            a = mix(a, 1.0, foam)
            // A glowing liquid: brightest in the ripple troughs (cooling
            // crust on the crests).
            rgb = rgb + self.sea_glow.xyz * (a * (0.35 + 1.3 * (1.0 - smoothstep(0.0 - 0.25, 0.1, fn1))))
            var fog = self.v_fog
            if self.water_fog.w > 0.5 {
                let d = p - eye
                let k = self.water_fog.y
                let fa = min((self.water_fog.x - eye.y) * k, 30.0)
                let dy = d.y * k
                var slope = 1.0 - dy * 0.5 + dy * dy * 0.1666667
                if abs(dy) > 0.05 { slope = (1.0 - exp(0.0 - dy)) / dy }
                fog = 1.0 - exp(0.0 - length(d) * self.fog_density * exp(fa) * slope)
            }
            return vec4(mix(rgb, self.fog_color * a, fog), a)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}
