//! The scene's own draw shaders: the city glow, the mountain ranges (and the front treeline) and the
//! ground band, plus the sky backdrop the app shows behind a view.
//!
//! Every scene shader writes the scene pass's RGBA16F targets, all blending premultiplied-over:
//! `fb0` the premultiplied colour and coverage, `fb1` the light and the same coverage, and for a band
//! `fb2` the distance (m) of what it draws premultiplied by that coverage, and its light plane: (a D,
//! L Dl / 1000, L, a), L the luminance of its light, Dl the distance (m) that light burns at. So a
//! nearer silhouette hides the lights behind it, and the final pass adds the light back on top; and
//! the third target holds the coverage-weighted distance of what each pixel shows and the
//! light-weighted distance of the lights it shows (plan hdri-6 R4: the host lets the lights shine
//! through its weather). Shaders of other stages (the city, the parts, the final pass) join the
//! scene with `..mod.draw.SkyScene`, which also gives them the off-centre view, the city camera and
//! the page's integer hashes (checked against `rng.rs` on the GPU by `tests/checks.rs`).
use makepad_widgets::makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // What every scene shader shares: the second and third outputs, the view, the column data and helpers.
    // The view uniforms are set by `SceneCtx::set_uniforms`; a parameter's uniform has its key's name.
    mod.draw.SkyScene = {
        fb1: fragment_output(1, vec4f)
        // the distance and the light plane (`SceneTarget::dist`, a band's alone; another pass drops it): written by every
        // shader of the scene, `at_dist` / `at_lit`; the passes after the scene hand it on in their second output
        fb2: fragment_output(2, vec4f)
        // the scene pass's size in pixels
        view_size: uniform(vec2(1.0, 1.0))
        // the first azimuth and the span the view shows (0..1 = 0..360 degrees)
        view_span: uniform(vec2(0.0, 1.0))
        // the horizon line, a fraction of the height from the top
        view_horizon: uniform(0.8)
        // the pass's margin past the view on each side (px; `ViewSpec::pad`): the view is view_in() px, the page's arithmetic
        view_pad: uniform(0.0)
        // column texture width and height, margin columns, column count
        col_geom: uniform(vec4(1.0, 7.0, 0.0, 1.0))
        // 1 in the reflection run: the scene seen mirrored about the horizon
        mirror: uniform(0.0)
        // the upright run's horizon row less this run's (px): 0, and under a pixel in the reflection run (`ViewSpec::mirrored`)
        up_dy: uniform(0.0)
        cols: texture_2d(float)
        tree_cov: texture_2d(float)
        haze: uniform(0.5)
        glow: uniform(0.5)
        sil: uniform(vec3(0.004, 0.006, 0.012))
        // the sky's light (`light.rs`): the sun's azimuth (a strip u) and elevation (rad), the sun's and the sky's ambient
        // light (radiance off a white Lambertian surface), the share of the lights on, the scale from the skyline's designed
        // colours to the host's units (`SkyLight::design_gain`; `sil` comes scaled)
        sun_u: uniform(0.0)
        sun_el: uniform(-1.5707964)
        sun_rgb: uniform(vec3(0.0, 0.0, 0.0))
        ambient_rgb: uniform(vec3(0.0, 0.0, 0.0))
        lit: uniform(1.0)
        design_gain: uniform(1.0)
        // 1: the near ground is the host's own (`SkyLight::host_ground`, the ring texture's third row)
        host_ground: uniform(0.0)
        // the sea's near edge, rad below the horizon (`crate::sea_edge_rad`: the sea distance seen from the eye); nearer than
        // it the shore, land. 10 (past every view): no shore, the sea reaches the viewer's feet (the example's)
        sea_edge: uniform(10.0)
        // the lakes (`lakes::texels`: LAKES_MAX x 1, each u, distance m, half-width u, half-depth m), how many, and the eye's
        // height over the ground (m: EYE x Size)
        lake_tex: texture_2d(float)
        lake_n: uniform(0.0)
        lake_eye: uniform(6.3)
        // the near rise's distance (m: `NEAR_RISE_M`)
        near_rise: uniform(500.0)
        // the share of the host's ground under standing water (`SkyLight::wet`): the land and the shore below the horizon are
        // water by it. 0: dry (the example's)
        wet: uniform(0.0)
        // over a host's wet ground, the share of its flat ground below the horizon (`under_floor`) the strip leaves to
        // the host's floor, which lays its puddles there (`SkyLight::floor_share`: the puddles' mean share); 0 elsewhere
        host_wet: uniform(0.0)
        // a host's beacons as points (`SkyLight::point_core`): a core's sigma (rad) and its peak over the designed one; 0:
        // the example's core (`beacon_core`)
        point_core: uniform(vec2(0.0, 0.0))
        // the horizon rings, RING x 4 texels: row 0 the sky at +1 degree, row 1 at +15 degrees, row 2 the host's ground,
        // row 3 the coast (x: 1 on land, 0 from SHORE_DEG out to sea; `SkyLight::ring_texels`, `coast_ring`)
        ring_tex: texture_2d(float)
        seed: uniform(7.0)
        ct_c: uniform(0.55)
        ct_r: uniform(1.0)
        off: uniform(0.0)
        off_dir: uniform(0.55)

        // the fragment's centre in pass pixels, exactly (the page's gl_FragCoord): the interpolated pos rounds by the quad's size
        frag: fn() -> vec2 { return floor(self.rect_pos + self.pos * self.rect_size) + 0.5 }
        // a host's beacon's core (`point_core` set) at d pixels from its centre (across, up), pixels of px rad, at
        // point_core.y times the designed peak: a core whose sigma is under half a pixel lights only the pixel its centre
        // falls in, its edges soft over 0.05 px (a centre on an edge shares it between the two, their sum the peak, so it
        // never drops out), so in a map it is one texel, not a few; a wider one is that Gaussian (the example's core,
        // exp(-d2 / 3), is written at its callers, unchanged)
        beacon_core: fn(d: vec2, px: float) -> float {
            let s = self.point_core.x / px
            if s < 0.5 { let b = clamp((0.5 - abs(d)) / 0.05 + 0.5, vec2(0.0), vec2(1.0)); return self.point_core.y * b.x * b.y }
            return self.point_core.y * exp(0.0 - 0.5 * dot(d, d) / (s * s))
        }
        // radians per pixel of the raw view
        px0: fn() -> float { return self.view_span.y * 6.2831853 / self.view_in().x }
        view_in: fn() -> vec2 { return self.view_size - 2.0 * self.view_pad }
        // raw elevation (rad) of pass row y (mirrored in the reflection run)
        ang_raw: fn(y: float) -> float {
            let a = (self.view_horizon * self.view_in().y - (y - self.view_pad)) * self.px0()
            return mix(a, 0.0 - a, self.mirror)
        }
        // the fragment's place in the upright run's targets (the trees, the city): there, the row of its own raw elevation
        up_uv: fn() -> vec2 {
            let pp = self.frag()
            return vec2(pp.x, self.view_horizon * self.view_in().y + self.up_dy - self.ang_raw(pp.y) / self.px0() + self.view_pad) / self.view_size
        }
        // row r of the column under pass x: rows 0-4 the ranges (ridge, tree foot, canopy top,
        // density), 5 the front treeline, 6 (land, ground rise, off-centre factor t, city glow)
        col_row: fn(x: float, r: float) -> vec4 {
            let c = floor(x) + self.col_geom.z
            let q = floor(c / self.col_geom.x)
            let xx = c - q * self.col_geom.x
            return self.cols.sample_nearest(vec2((xx + 0.5) / self.col_geom.x, (r + 7.0 * q + 0.5) / self.col_geom.y))
        }
        // coverage of a signed distance d over a pixel of size px
        cover: fn(d: float, px: float) -> float { return clamp(d / px + 0.5, 0.0, 1.0) }
        // ring row v (1/8 the low ring, 3/8 the high one, 5/8 the host's ground, 7/8 the coast) at azimuth u: linear between its two nearest samples, wrapping
        // round the turn, as a + (b - a) f, so a ring of one colour gives that colour exactly (the example's night)
        ring_at: fn(u: float, v: float) -> vec3 {
            let n = self.ring_tex.size().x; let x = fract(u) * n - 0.5; let i0 = floor(x); let i1 = i0 + 1.0
            let a = self.ring_tex.sample_nearest(vec2((i0 - n * floor(i0 / n) + 0.5) / n, v)).xyz
            let b = self.ring_tex.sample_nearest(vec2((i1 - n * floor(i1 / n) + 0.5) / n, v)).xyz
            return a + (b - a) * (x - i0)
        }
        // the haze at azimuth u: the sky just over the horizon there
        haze_at: fn(u: float) -> vec3 { return self.ring_at(u, 0.125) }
        // a host's coast at azimuth u (`host_ground`): 1 on land, fading to 0 out to sea
        coast_at: fn(u: float) -> float { return clamp(self.ring_at(u, 0.875).x, 0.0, 1.0) }
        // the shore's share of a pixel al rad below the horizon, px rad a side: the land nearer than the sea distance
        shore_at: fn(al: float, px: float) -> float { if self.sea_edge > 1.5 { return 0.0 } return self.cover(al - self.sea_edge, px) }
        // the lakes' share of a pixel al rad down at world azimuth u, px rad a side: each lake's depth at the pixel's place
        // across it as depressions (its far and near shore there), boxed over the pixel's al -+ px / 2, times its share
        // across; the lakes never overlap, so their shares add, and one whose sides the pixel is past adds none
        // (`lakes::cover`, the same on the CPU)
        lake_at: fn(u: float, al: float, px: float) -> float {
            if self.lake_n < 0.5 || al <= 0.0 { return 0.0 }
            var m = 0.0
            for i in 0..u32(self.lake_n) {
                let l = self.lake_tex.sample_nearest(vec2((float(i) + 0.5) / self.lake_tex.size().x, 0.5))
                let x = (fract(u - l.x + 0.5) - 0.5) / l.z
                if abs(x) < 1.0 {
                    let hy = sqrt(1.0 - x * x)
                    let a0 = atan(self.lake_eye / (l.y + hy * l.w)); let a1 = atan(self.lake_eye / (l.y - hy * l.w))
                    let cy = (clamp(al + 0.5 * px, a0, a1) - clamp(al - 0.5 * px, a0, a1)) / px
                    m = m + clamp((1.0 - abs(x)) / (px / 6.2831853 / l.z) + 0.5, 0.0, 1.0) * cy
                }
            }
            return min(m, 1.0)
        }
        // what is water at a pixel al rad below the horizon (world azimuth u, px rad a side) in column g: x the sea from its
        // near edge out, y a lake on the land, z standing water on the land and the shore (the host's wetness there)
        water_at: fn(g: vec4, u: float, al: float, px: float) -> vec3 {
            let sea = (1.0 - smoothstep(0.0, 0.35, g.x)) * (1.0 - self.shore_at(al, px))
            // a lake stands on land columns only
            return vec3(sea, self.lake_at(u, al, px) * smoothstep(0.35, 0.6, g.x), self.wet * (1.0 - sea))
        }
        // the share of the fragment under the horizon, for a fragment whose centre is under it (as the ground's own
        // `al > 0`), in the upright run, under a host's wet floor (`host_wet`); elsewhere 0. There the strip's flat ground
        // gives way to the host's floor at every depression (DrawSkyGround; the owner's D1 applied literally), only what
        // stands keeping its own pixels, above the horizon: so what is drawn before the ground and reaches under the
        // horizon (the ranges, the front treeline, the glow, the city's blit, the props in the scene list) leaves that
        // share out, or it would show through
        under_floor: fn() -> float {
            let b = 0.0 - self.ang_raw(self.frag().y)
            if self.host_wet > 0.0 && self.mirror < 0.5 && b > 0.0 { return self.cover(b, self.px0()) }
            return 0.0
        }
        // the sky at azimuth u, elevation el (rad): the low ring, toward the high one by +15 degrees
        sky_at: fn(u: float, el: float) -> vec3 { let a = self.ring_at(u, 0.125); return a + (self.ring_at(u, 0.375) - a) * clamp(el / 0.2617994, 0.0, 1.0) }
        // the sun's direction in the frame of a pixel at azimuth u: x to its right (growing u), y up, z along the view
        sun_dir: fn(u: float) -> vec3 {
            let az = (self.sun_u - u) * 6.2831853
            return vec3(cos(self.sun_el) * sin(az), sin(self.sun_el), cos(self.sun_el) * cos(az))
        }
        // daylight off a surface of albedo al: the sky's ambient (times ao) plus the sun times its cosine term ks; 0 at night
        day_lit: fn(al: vec3, ao: float, ks: float) -> vec3 { return al * (self.ambient_rgb * ao + self.sun_rgb * ks) }
        // the sun's share of the day: 0 below -6 degrees, 1 above +2 (`lights_factor` the other way round, without lights_by_day)
        day: fn() -> float { let t = clamp((0.0349066 - self.sun_el) / 0.1396263, 0.0, 1.0); return 1.0 - t * t * (3.0 - 2.0 * t) }
        // the daylight's aerial perspective: the share of the haze in front of a surface d m away, by the city march's
        // extinction (per 70 m block); 0 at night, where the night's own haze terms stand in
        aerial: fn(d: float) -> float { return self.day() * (1.0 - exp(0.0 - d * (0.0015 + 0.0075 * self.haze) / 70.0)) }
        // the distance output (fb2) of a surface d m away drawn at coverage a: premultiplied, so the target holds the
        // coverage-weighted distance of the visible surfaces, the nearest winning where it is opaque
        at_dist: fn(a: float, d: float) -> vec4 { return vec4(a * d, 0.0, 0.0, a) }
        // `at_dist` with the light plane of light e (fb1's, premultiplied) burning dl m off: its luminance L (Rec. 709) in
        // z and L dl in y (in km, so a bright far light stays inside a half's range), blended as the light is, so the target
        // holds the light-weighted distance of the lights the pixel shows, a nearer surface hiding those behind it
        at_lit: fn(a: float, d: float, e: vec3, dl: float) -> vec4 { let l = dot(e, vec3(0.2126, 0.7152, 0.0722)); return vec4(a * d, l * dl * 0.001, l, a) }
        // the fragment's raw view: azimuth u (0..1), raw elevation (rad), radians per pixel
        view_raw: fn() -> vec3 {
            let pp = self.frag()
            return vec3(fract(self.view_span.x + (pp.x - self.view_pad) / self.view_in().x * self.view_span.y), self.ang_raw(pp.y), self.px0())
        }
        // the page's `pixView`: raw view vr re-aimed for the off-centre camera (the scenery is a
        // ring of radius 1, the camera `off` from its centre toward `off_dir`): the world azimuth
        // the pixel sees, elevation and pixel size scaled by the distance t to that point
        pix_view: fn(vr: vec3) -> vec3 {
            let d = vec2(cos(vr.x * 6.2831853), sin(vr.x * 6.2831853))
            let c = self.off * vec2(cos(self.off_dir * 6.2831853), sin(self.off_dir * 6.2831853))
            let cd = dot(c, d)
            let t = 0.0 - cd + sqrt(max(cd * cd - dot(c, c) + 1.0, 0.0001))
            let q = c + t * d
            return vec3(fract(atan2(q.y, q.x) / 6.2831853), vr.y * t, vr.z * t)
        }
        // the city camera: downtown's distance (blocks) and direction, the real camera offset (blocks)
        city_d: fn() -> float { return 28.0 * self.ct_r }
        city_dir: fn() -> vec2 { return vec2(cos(self.ct_c * 6.2831853), sin(self.ct_c * 6.2831853)) }
        city_cam: fn() -> vec2 { return (0.8 * self.off * self.city_d()) * vec2(cos(self.off_dir * 6.2831853), sin(self.off_dir * 6.2831853)) }
        // the page's `prCityView`: world point w (m; 70 m a block) from the city camera: raw azimuth, distance (m)
        city_view: fn(w: vec2) -> vec2 {
            let q = w - self.city_cam() * 70.0
            return vec2(fract(atan2(q.y, q.x) / 6.2831853), length(q))
        }
        // the page's integer hashes, bit-exact with `rng.rs` (u32 arithmetic wraps on every backend;
        // a negative i32 keeps its bits as u32, so `+ 4096u` is the page's `int(id) + 4096`)
        hu: fn(x_in: u32) -> u32 {
            var x = x_in
            x = x ^ (x >> 16u)
            x = x * 2146121005u
            x = x ^ (x >> 15u)
            x = x * 2221713035u
            return x ^ (x >> 16u)
        }
        // `rnd`'s and `hcell`'s common form: hu(a K1 ^ (b K2 + c K3 + seed K4))
        hash3: fn(a: u32, b: u32, c: u32) -> u32 {
            return self.hu((a * 2654435761u) ^ ((b * 2246822507u) + (c * 3266489909u) + (u32(i32(self.seed)) * 668265263u)))
        }
        // `rnd(id, st, ly)`: 0..1 in steps of 2^-24
        rnd: fn(id: float, st: float, ly: float) -> float {
            return f32(self.hash3(u32(i32(id)) + 4096u, u32(i32(st)), u32(i32(ly))) >> 8u) / 16777216.0
        }
        // `hcell`: the hash of integer cell c, layer k (the city's lots, the trees' and the town's cells)
        hcell: fn(c: vec2, k: float) -> u32 { return self.hash3(u32(i32(c.x)) + 8192u, u32(i32(c.y)) + 8192u, u32(i32(k))) }
        // a hash's four bytes as 0..1
        hb4: fn(h: u32) -> vec4 { return vec4(f32(h & 255u), f32((h >> 8u) & 255u), f32((h >> 16u) & 255u), f32(h >> 24u)) * 0.003921569 }
        // the page's `kelvin`: a black-body-like light colour at k K, brightest channel 1 (the city's windows, the props' lights)
        kelvin: fn(k: float) -> vec3 {
            let t = clamp(k, 1000.0, 12000.0) / 100.0
            if t > 66.0 { return vec3(clamp(1.292936 * pow(t - 60.0, -0.1332047592), 0.0, 1.0), clamp(1.129891 * pow(t - 60.0, -0.0755148492), 0.0, 1.0), 1.0) }
            var b = 1.0; if t < 66.0 { b = if t <= 19.0 { 0.0 } else { clamp(0.54320679 * log(t - 10.0) - 1.19625409, 0.0, 1.0) } }
            return vec3(1.0, clamp(0.39008158 * log(t) - 0.63184144, 0.0, 1.0), b)
        }
    }

    // The glow dome over downtown: light only, behind everything (the ranges hide it).
    mod.draw.DrawSkyGlow = mod.std.set_type_default() do #(DrawSkyGlow::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        // its e-folding rate in elevation: the camera's distance to downtown over the tower height
        dome_k: uniform(1.0)
        pixel: fn(){
            let pp = self.frag()
            let g = self.col_row(pp.x, 6.0)
            let a = self.ang_raw(pp.y)
            let dome = exp(0.0 - max(a, 0.0) * self.dome_k) * g.w * self.glow * smoothstep(0.0 - 0.012, 0.0, a)
            // the city's own light: fades with the lights by day
            return vec4(vec3(1.0, 0.55, 0.25) * (dome * 0.10) * (self.lit * self.design_gain), 0.0)
        }
        // no coverage, so no distance; its light burns over downtown, at downtown's distance from the camera (m); under a
        // host's wet floor only its share over the horizon (`under_floor`)
        fragment: fn(){
            var e = self.pixel()
            if self.host_wet > 0.0 { e = e * (1.0 - self.under_floor()) }
            self.fb0 = vec4(0.0, 0.0, 0.0, 0.0)
            self.fb1 = e
            self.fb2 = self.at_lit(0.0, 0.0, e.xyz, length(self.city_d() * self.city_dir() - self.city_cam()) * 70.0)
        }
    }

    // A mountain range k (0 farthest .. 4 nearest) over its height band, with its forest: rock in the
    // night's tonal order (farther and lower is hazier), then the trees over it (the darkest thing
    // on a flank): solid below the forest's foot line, the canopy, the single trees. k = 5 is the
    // front treeline, trees only.
    mod.draw.DrawSkyRange = mod.std.set_type_default() do #(DrawSkyRange::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        k: 0.0
        hk: 0.2
        dk: 5000.0
        pixel: fn(){
            let pp = self.frag(); let g = self.col_row(pp.x, 6.0); let k = self.k; let sil = self.sil
            let ang = self.ang_raw(pp.y) * g.z; let px = self.px0() * g.z; let r = self.col_row(pp.x, min(k, 5.0))
            var tm = 0.0
            if k > 1.5 {
                // the trees are drawn upright once a picture: the reflection run reads them at the mirrored row
                let tr = self.tree_cov.sample_nearest(self.up_uv())
                let ch = vec4(step(abs(k - 2.0), 0.5), step(abs(k - 3.0), 0.5), step(abs(k - 4.0), 0.5), step(4.5, k))
                tm = r.w * max(self.cover(r.z - ang, 1.5 * px), dot(tr, ch))
                if ang < r.y - px { tm = r.w }
            }
            // by day: the trees (no normal: lit half from above), the rock by the ridge's slope (the change of its height
            // per column over 0.01 rad either side, so the ridge's pixel-scale roughness does not stripe the flank below; within
            // the columns' margin, which is over 0.045 rad; facing the viewer, -z in the pixel's frame), warmer on the nearer ranges
            let u = self.view_raw().x; let sun = self.sun_dir(u)
            let tday = self.day_lit(vec3(0.07, 0.07, 0.07), 1.0, 0.5 + 0.5 * sun.y)
            if k > 4.5 { return vec4(sil * (0.6 * tm) + tday * tm, tm) }
            let dn = max(1.0, floor(0.01 / self.px0() + 0.5))
            let dh = (self.col_row(pp.x + dn, k).x - self.col_row(pp.x - dn, k).x) / (2.0 * dn * px)
            let rday = self.day_lit(vec3(0.14, 0.14, 0.14) * mix(vec3(1.0, 1.0, 1.0), vec3(1.15, 1.0, 0.82), k / 4.0), 1.0, max(dot(normalize(vec3(0.0 - dh, 1.0, -0.6)), sun), 0.0))
            let t = (4.0 - k) / 4.0; let hz = self.haze_at(u); let hz2 = self.haze * self.haze * 0.35; let m = self.cover(r.x - ang, px)
            let ftop = clamp(mix(0.012 + 0.01 * t, 0.30 * t * t + 0.06 * t + 0.02, hz2), 0.0, 1.0)
            let fog = pow(1.0 - smoothstep(0.0, 0.55 * self.hk + 0.004, ang), 1.4)
            let fogc = mix(hz * (0.7 + 0.25 * t), hz * (1.6 + 1.6 * t) + vec3(0.06, 0.07, 0.09) * (t * self.design_gain), hz2)
            let rock = mix(mix(sil + rday, fogc, ftop), fogc * (1.1 + 0.2 * t), fog * mix(0.1, 0.9, hz2) * (0.25 + 0.75 * t))
            let c = vec4(sil * (0.6 * tm) + tday * tm + rock * (m * (1.0 - tm)), tm + m * (1.0 - tm))
            // by day the air between: toward the haze by the range's distance (none at night, the night's haze is above)
            return vec4(c.xyz + (hz * c.w - c.xyz) * self.aerial(5000.0 * pow(6.0, (4.0 - k) * 0.25)), c.w)
        }
        // at the range's distance, its trees' too (`dk`)
        // (under a host's wet floor, only its share over the horizon: `under_floor`)
        fragment: fn(){
            var c = self.pixel()
            if self.host_wet > 0.0 { c = c * (1.0 - self.under_floor()) }
            self.fb0 = c; self.fb1 = vec4(0.0, 0.0, 0.0, c.w); self.fb2 = self.at_dist(c.w, self.dk)
        }
    }

    // The ground band: the near ground rise over land, flat sea below the horizon.
    mod.draw.DrawSkyGround = mod.std.set_type_default() do #(DrawSkyGround::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        pixel: fn(){
            let pp = self.frag(); let g = self.col_row(pp.x, 6.0)
            let gm = self.cover(g.y - self.ang_raw(pp.y) * g.z, self.px0() * g.z)
            // by day: lit from above (normal up)
            let day = self.day_lit(vec3(0.10, 0.10, 0.10), 1.0, max(sin(self.sun_el), 0.0))
            // the land: the column's, and the shore in front of the sea (nearer than the sea distance)
            let al = 0.0 - self.ang_raw(pp.y) * g.z
            var land = g.x
            if al > 0.0 { land = max(land, self.shore_at(al, self.px0() * g.z)) }
            var c = mix(self.sil * 0.5, self.sil * 1.6, land) + day
            // a host's own ground over the land and on out to sea from its coast, so the band's ground is the map's
            // and the land meets the sea without an edge (`SkyLight::host_ground`, `coast_ring`)
            if self.host_ground > 0.5 { let u = self.view_raw().x; c = mix(c, self.ring_at(u, 0.625), max(land, self.coast_at(u))) }
            // the land's own colour (the host's ground on its land; what a host's wet floor takes the place of)
            let cl = c
            // over a host's wet floor (`host_wet`) the near rise's face, which stands, is a bank under the trees, not the
            // floor: the host's ground (the damp ground, its sky sheen included) gives way to the front treeline's own
            // colour (DrawSkyRange's where its trees are solid) by the floor's share, so at Wet ground 1.00 the rise and
            // its reflection in the floor read as the treeline's foot (plan hdri-6 fix wave 1, ruling 1's acceptance)
            var rc = cl
            if self.host_wet > 0.0 {
                let bank = self.sil * 0.6 + self.day_lit(vec3(0.07, 0.07, 0.07), 1.0, 0.5 + 0.5 * sin(self.sun_el))
                rc = cl + (bank - cl) * self.host_wet
                if al <= 0.0 { c = rc }
            }
            // water standing on the land (a lake, the host's wetness on the land and the shore): (1 - Fresnel) of the ground
            // shows through it, darkened as through a puddle, and by Fresnel the dark water the night sea shows (its 0.5 sil);
            // the water shader adds what it mirrors
            if al > 0.0 && (self.lake_n > 0.5 || self.wet > 0.0) {
                let v = self.pix_view(self.view_raw()); let ws = self.water_at(g, v.x, al, self.px0() * g.z)
                let fr = 0.02 + 0.98 * pow(clamp(1.0 - al, 0.0, 1.0), 5.0)
                c = c + (c * (0.35 * (1.0 - fr)) + self.sil * (0.5 * fr) - c) * max(ws.y, ws.z)
            }
            // over a host's wet floor (`host_wet`): by day the sea is the floor's water, by the floor's share (one
            // reflection runs from the sea onto the floor, the owner's D2): no base of its own, the host's ground
            // through it darkened as through a puddle (x 0.35 (1 - Fresnel)), the water shader mirroring at full
            // strength. And below the horizon (`under_floor`) the strip's flat ground gives way to the host's floor:
            // the share host_wet of the land (its colour `cl`; in the sea's columns the coast's, the host's ground
            // `gr`) is left clear at every depression, and its open water (the sea, faded from the host's coast by day,
            // and a lake) is kept whole, so a lake's edge keeps its water and the cover is applied once; the host lays
            // its puddles there (plan hdri-6 fix wave 1, the owner's D1 applied literally)
            if al > 0.0 && self.host_wet > 0.0 {
                let u = self.view_raw().x; let gr = self.ring_at(u, 0.625); let coast = self.coast_at(u)
                let fr = 0.02 + 0.98 * pow(clamp(1.0 - al, 0.0, 1.0), 5.0)
                c = c + (gr * (0.35 * (1.0 - fr)) - c) * ((1.0 - max(land, coast)) * self.host_wet * self.day())
                let ws = self.water_at(g, self.pix_view(self.view_raw()).x, al, self.px0() * g.z)
                let gw = self.host_wet * (1.0 - max(ws.x * (1.0 - coast * self.day()), ws.y)) * self.under_floor()
                var lc = cl
                if ws.x > 0.0 { lc = gr }
                // (the share of a texel across the horizon over it is the rise's face: the bank)
                let up = max(gm - self.under_floor(), 0.0)
                return vec4(max(c * gm - lc * gw + (rc - c) * up, vec3(0.0, 0.0, 0.0)), gm - gw)
            }
            return vec4(c * gm, gm)
        }
        // the ground's distance (m): below the horizon the ground point the pixel sees, EYE x Size / tan(al) (al its
        // depression as `pixel` takes it, the column's off-centre factor applied), at most `FAR_M`; at and above it the
        // near rise (`NEAR_RISE_M`). Over the land the rise stands at its distance and hides the ground beyond, so the
        // rows down to its foot (the depression of the ground at NEAR_RISE_M) show the rise: no farther than it, by the
        // column's land share (the sea's columns keep their ground point, the shore's and the water's)
        ground_d: fn() -> float {
            let pp = self.frag(); let g = self.col_row(pp.x, 6.0); let al = 0.0 - self.ang_raw(pp.y) * g.z
            if al <= 0.0 { return self.near_rise }
            let d = min(self.lake_eye / tan(al), 60000.0)
            return d + (min(d, self.near_rise) - d) * g.x
        }
        fragment: fn(){ let c = self.pixel(); self.fb0 = c; self.fb1 = vec4(0.0, 0.0, 0.0, c.w); self.fb2 = self.at_dist(c.w, self.ground_d()) }
    }

    // The night sky behind a view (the page's CSS gradient), with a hard step to the sea at the horizon.
    mod.draw.DrawSkyBackdrop = mod.std.set_type_default() do #(DrawSkyBackdrop::script_shader(vm)){
        ..mod.draw.DrawQuad,
        horizon: uniform(0.8)
        pixel: fn(){
            let y = self.pos.y; let h = self.horizon; let f = y / h
            if y >= h { return #x0a0d16 }
            if f < 0.45 { return mix(#x050813, #x0a1230, f / 0.45) }
            if f < 0.8 { return mix(#x0a1230, #x1b2547, (f - 0.45) / 0.35) }
            if f < 0.95 { return mix(#x1b2547, #x4a4258, (f - 0.8) / 0.15) }
            return mix(#x4a4258, #x8a5a55, (f - 0.95) / 0.05)
        }
    }
}

#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyGlow { #[deref] pub draw_super: DrawQuad }

/// `k`: the range (0 farthest .. 4 nearest; 5 the front treeline); `hk`: its peak height (rad); `dk`: its distance (m), the
/// one its air is taken over (`zones::mt_dist`; the treeline's `NEAR_RISE_M`). Handed in rather than worked out in the
/// shader, where sharing the air's own arithmetic moved a texel of the colour by an ulp (fxc).
#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyRange { #[deref] pub draw_super: DrawQuad, #[live] pub k: f32, #[live] pub hk: f32, #[live] pub dk: f32 }
#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyGround { #[deref] pub draw_super: DrawQuad }
#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyBackdrop { #[deref] pub draw_super: DrawQuad }
