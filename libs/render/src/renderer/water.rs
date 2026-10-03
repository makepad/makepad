//! The water surface (local/plans/water.md): one radial mesh drawn per
//! visible water volume, its swell displaced in the vertex shader from the
//! sim's own coefficients, shaded in one pass from three small textures:
//! a baked sky for reflections, a ripple map made from the sea's spectrum,
//! and the seabed's height for depth colour, shore foam and caustics.

use super::*;
use makepad_scene::{SeaLook, DETAIL_N};
use std::sync::Arc;

/// Segments around each ring of the surface mesh.
const RING_SEGMENTS: usize = 128;
/// Radius of the first ring, metres.
const RING_START: f32 = 0.6;
/// Growth from ring to ring: a vertex's radial spacing matches its angular
/// spacing (2π/128 ≈ 4.9%), so the cells stay square at every distance.
const RING_GROWTH: f32 = 1.05;
/// Where the fine rings stop; beyond, a few long steps reach the horizon.
const RING_FINE_END: f32 = 1500.0;
const HORIZON: [f32; 3] = [4000.0, 12000.0, 40000.0];
/// Baked sky: azimuth × elevation texels.
const SKY_W: usize = 64;
const SKY_H: usize = 16;
/// Largest side of the seabed height texture.
const BED_MAX: usize = 1024;

#[derive(Default)]
pub(super) struct WaterGpu {
    mesh: Option<Geometry>,
    /// One ripple map per distinct look (seed, wind, spread): (key, map,
    /// tile metres).
    ripples: Vec<(u64, Texture, f32)>,
    sky: Option<Texture>,
    sky_key: [u32; 8],
    bed: Option<(Texture, [f32; 4], [f32; 2])>,
    pub(super) bed_key: Option<(usize, u64)>,
    blank: Option<Texture>,
    /// Mesh-drawn surfaces (rivers, imported liquids), by the mesh's Arc.
    meshes: Vec<(usize, Geometry, Vec3f, Vec3f)>,
}

impl WaterGpu {
    /// A new realm: its seabed and liquid meshes are another world's (the
    /// mesh, sky and ripple textures are shared by every realm and kept).
    pub(super) fn enter_realm(&mut self) {
        self.meshes.clear();
        self.bed = None;
        self.bed_key = None;
    }
}

/// A mesh-drawn surface as PbrVertex geometry: world positions, the mesh's
/// spacing in w (the shader's wave fade).
fn mesh_geometry_data(mesh: &makepad_scene::WaterMesh) -> (Vec<f32>, Vec<u32>) {
    let mut vertices = Vec::with_capacity(mesh.positions.len() * 16);
    for p in &mesh.positions {
        vertices.extend_from_slice(&[p[0], p[1], p[2], mesh.spacing.max(0.05), 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0]);
    }
    let n = mesh.positions.len() as u32;
    let indices = mesh.indices.iter().copied().filter(|i| *i < n).collect::<Vec<_>>();
    let indices = indices[..indices.len() / 3 * 3].to_vec();
    (vertices, indices)
}

/// Pack a volume's swell into the shader's uniform slots — the RAW
/// WaterWave fields, bit-for-bit, no unit conversion: this function being
/// trivial IS the CPU/GPU agreement story (renderer/water_tests.rs holds
/// the shader to the same expression over these same numbers). Unused
/// slots are zero; a zero amplitude contributes nothing in the shader.
pub fn pack_wave_uniforms(
    volume: &WaterSurface,
) -> ([[f32; 4]; MAX_WAVES], [[f32; 4]; MAX_WAVES]) {
    let mut a = [[0.0f32; 4]; MAX_WAVES];
    let mut b = [[0.0f32; 4]; MAX_WAVES];
    for (i, w) in volume.waves.iter().take(MAX_WAVES).enumerate() {
        a[i] = [w.dir_x, w.dir_z, w.k, w.omega];
        b[i] = [w.amp, w.phase, w.group, 0.0];
    }
    (a, b)
}

/// The surface mesh: rings around the origin (PbrVertex layout). A vertex
/// carries its offset from the centre in xyz and its ring spacing in w (the
/// shader fades waves that spacing cannot show).
pub(super) fn water_ring_mesh_data() -> (Vec<f32>, Vec<u32>) {
    let mut radii = vec![0.0f32];
    let mut r = RING_START;
    while r < RING_FINE_END {
        radii.push(r);
        r *= RING_GROWTH;
    }
    radii.extend_from_slice(&HORIZON);
    let seg = RING_SEGMENTS;
    let mut vertices: Vec<f32> = Vec::with_capacity((radii.len() * seg + 1) * 16);
    let angular = std::f32::consts::TAU / seg as f32;
    // The centre: one vertex.
    vertices.extend_from_slice(&[0.0, 0.0, 0.0, RING_START, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0]);
    for (j, r) in radii.iter().enumerate().skip(1) {
        // The horizon rings are far beyond every wave: their spacing only
        // has to fade them all out.
        let spacing = if j + HORIZON.len() >= radii.len() { r * angular * 4.0 } else { r * angular };
        for i in 0..seg {
            let a = i as f32 * angular;
            vertices.extend_from_slice(&[
                a.cos() * r, 0.0, a.sin() * r, spacing, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0,
                1.0, 0.0, 0.0, 1.0,
            ]);
        }
    }
    let seg32 = seg as u32;
    let ring = |j: usize, i: usize| 1 + (j as u32 - 1) * seg32 + (i % seg) as u32;
    let mut indices = Vec::new();
    for i in 0..seg {
        // Counter-clockwise seen from +y (angles grow from +x toward +z).
        indices.extend_from_slice(&[0, ring(1, i + 1), ring(1, i)]);
    }
    for j in 1..radii.len() - 1 {
        for i in 0..seg {
            let (a, b, c, d) = (ring(j, i), ring(j, i + 1), ring(j + 1, i), ring(j + 1, i + 1));
            indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    (vertices, indices)
}

/// RGBM (rgb / m, m / 8 in alpha) as a BGRA8 texel.
fn rgbm(c: Vec3f) -> u32 {
    let m = c.x.max(c.y).max(c.z).clamp(1.0e-6, 8.0);
    let m = (m / 8.0 * 255.0).ceil().max(1.0) / 255.0 * 8.0;
    let q = |v: f32| ((v / m).clamp(0.0, 1.0) * 255.0).round() as u32;
    (q(m / 8.0) << 24) | (q(c.x) << 16) | (q(c.y) << 8) | q(c.z)
}

/// The analytic sky's linear radiance along `v` — the HDR lane of
/// DrawSceneSkyAnalytic's day dome (Perez × zenith, palette tint, night
/// floor), without the sun disc (the water draws its own glint) or stars.
fn sky_linear(frame: &crate::sky::SkyFrame, v: Vec3f, gain: f32) -> Vec3f {
    let ct = v.y.max(0.01);
    let sun = vec3f(frame.sun.x, frame.sun.y, frame.sun.z);
    let cg = v.dot(sun).clamp(-1.0, 1.0);
    let g = cg.acos();
    let perez = |c: Vec4f, e: f32| (1.0 + c.x * (c.y / ct).exp()) * (1.0 + c.z * (c.w * g).exp() + e * cg * cg);
    let yl = frame.zenith.x * perez(frame.pz_y, frame.pz_e.x) * frame.pz_f0.x;
    let xc = frame.zenith.y * perez(frame.pz_x, frame.pz_e.y) * frame.pz_f0.y;
    let yc = (frame.zenith.z * perez(frame.pz_yc, frame.pz_e.z) * frame.pz_f0.z).max(0.0001);
    let yt = (yl * frame.sun.w).max(0.0) * gain;
    let bx = xc * (yt / yc);
    let bz = (1.0 - xc - yc) * (yt / yc);
    let day = vec3f(
        (3.2406 * bx - 1.5372 * yt - 0.4986 * bz).max(0.0),
        (-0.9689 * bx + 1.8758 * yt + 0.0415 * bz).max(0.0),
        (0.0557 * bx - 0.204 * yt + 1.057 * bz).max(0.0),
    );
    let day = vec3f(day.x * frame.dome_tint.x, day.y * frame.dome_tint.y, day.z * frame.dome_tint.z);
    let night = vec3f(0.010, 0.012, 0.020) * 0.2;
    day + (night - day) * frame.zenith.w
}

/// The upper hemisphere the water reflects, SKY_W × SKY_H RGBM texels
/// (row 0 = zenith, the last row = the horizon), its horizon faded into the
/// fog colour the way the dome's height fog does.
fn sky_texels(frame: Option<&crate::sky::SkyFrame>, gain: f32, fog: Vec3f, zenith_fill: Vec3f) -> Vec<u32> {
    let mut out = Vec::with_capacity(SKY_W * SKY_H);
    for y in 0..SKY_H {
        let el = (1.0 - (y as f32 + 0.5) / SKY_H as f32).clamp(0.0, 1.0);
        for x in 0..SKY_W {
            let az = ((x as f32 + 0.5) / SKY_W as f32 - 0.5) * std::f32::consts::TAU;
            let h = (1.0 - el * el).max(0.0).sqrt();
            let v = vec3f(az.cos() * h, el, az.sin() * h);
            let c = match frame {
                Some(f) => sky_linear(f, v, gain),
                None => zenith_fill,
            };
            let haze = (-el * 9.0).exp();
            out.push(rgbm(c + (fog - c) * haze));
        }
    }
    out
}

/// The seabed as a 16-bit height texture (high byte in RGB, low byte in A:
/// the encoding every backend samples in every stage, and one bilinear
/// filters exactly, the decode being linear), at most BED_MAX per side.
/// Returns (texels, w, h, rect (x0, z0, 1/width, 1/depth), (lo, range)).
fn bed_texels(t: &Terrain) -> Option<(Vec<u32>, usize, usize, [f32; 4], [f32; 2])> {
    if t.cells < 2 || t.heights.len() < t.cells * t.cells {
        return None;
    }
    let step = t.cells.div_ceil(BED_MAX).max(1);
    let n = (t.cells - 1) / step + 1;
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for h in &t.heights {
        lo = lo.min(*h);
        hi = hi.max(*h);
    }
    let range = (hi - lo).max(0.01);
    let mut texels = Vec::with_capacity(n * n);
    for z in 0..n {
        for x in 0..n {
            let h = t.heights[(z * step).min(t.cells - 1) * t.cells + (x * step).min(t.cells - 1)];
            let q = (((h - lo) / range) * 65535.0).round().clamp(0.0, 65535.0) as u32;
            texels.push(((q & 0xFF) << 24) | ((q >> 8) << 16) | ((q >> 8) << 8) | (q >> 8));
        }
    }
    // Texel centres sit on the sampled vertices: the rect spans half a
    // texel beyond the first and last.
    let cell = t.cell_size * step as f32;
    let x0 = t.origin - cell * 0.5;
    let span = cell * n as f32;
    Some((texels, n, n, [x0, x0, 1.0 / span, 1.0 / span], [lo, range]))
}

/// The look's ripple-map identity: what `SeaLook::detail_map` depends on.
fn ripple_key(look: &SeaLook) -> u64 {
    let mut h = look.seed as u64;
    for v in [look.wind_dir[0], look.wind_dir[1], look.spread, look.wind, look.fetch, look.gravity] {
        h = h.wrapping_mul(0x100_0000_01b3) ^ v.to_bits() as u64;
    }
    h
}

/// The volume whose swell surface is above `eye`, when the eye is inside
/// its box: the water the camera is under.
pub fn eye_under_water<'a>(water: &'a WaterView, eye: Vec3f, t: f32) -> Option<&'a WaterSurface> {
    water.volumes.iter().find(|v| {
        v.draw_sheet
            && v.covers_xz(eye.x, eye.z)
            && (v.unbounded || eye.y >= v.min.y)
            && eye.y < v.swell_height(eye.x, eye.z, t)
    })
}

/// The water lit surfaces are seen through this frame (the clustered
/// mixin's `uw_*`): the drawn volume under the eye, else the first sea,
/// else the nearest drawn box. Off (zeros) when the eye is under water —
/// the fog is the water then — or there is no drawn water.
pub(super) fn water_column(water: Option<&WaterView>, eye: Vec3f, t: f32, sun: &SunLight) -> [[f32; 4]; 3] {
    let Some(water) = water else { return [[0.0; 4]; 3] };
    if eye_under_water(water, eye, t).is_some() {
        return [[0.0; 4]; 3];
    }
    // Mesh-drawn waters (a level's pools, a river's ribbon) are not a level
    // the whole scene below sits under: their own surface hides what is
    // beneath it instead.
    let drawn = || water.volumes.iter().filter(|v| v.draw_sheet && v.mesh.is_none());
    let dist = |v: &WaterSurface| {
        if v.unbounded { return 0.0 }
        let dx = (v.min.x - eye.x).max(eye.x - v.max.x).max(0.0);
        let dz = (v.min.z - eye.z).max(eye.z - v.max.z).max(0.0);
        dx * dx + dz * dz
    };
    let Some(v) = drawn()
        .find(|v| !v.unbounded && v.covers_xz(eye.x, eye.z))
        .or_else(|| drawn().find(|v| v.unbounded))
        .or_else(|| drawn().min_by(|a, b| dist(a).total_cmp(&dist(b))))
    else {
        return [[0.0; 4]; 3];
    };
    let rect = if v.unbounded { [-1.0e6, -1.0e6, 1.0e6, 1.0e6] } else { [v.min.x, v.min.z, v.max.x, v.max.z] };
    let c = in_scatter(&v.look, sun);
    [[v.level(), 1.0, v.look.clarity.max(0.1), v.amp_sum()], rect, [c.x, c.y, c.z, 0.0]]
}

/// The colour a water column scatters toward the eye, lit by the sun and
/// sky: what deep water, and the far end of an underwater view, look like.
pub(super) fn in_scatter(look: &SeaLook, sun: &SunLight) -> Vec3f {
    let lit = sun.sky + sun.ground * 0.5 + sun.color * (sun.dir.y.max(0.0) * 0.6);
    let (d, s) = (look.deep, look.shallow);
    vec3f((d[0] * 0.7 + s[0] * 0.3) * lit.x, (d[1] * 0.7 + s[1] * 0.3) * lit.y, (d[2] * 0.7 + s[2] * 0.3) * lit.z)
}

impl Renderer {
    fn water_textures(&mut self, cx: &mut Cx, water: &WaterView, sky_frame: Option<&crate::sky::SkyFrame>, fog: Vec3f, sun: &SunLight) {
        let gpu = &mut self.water;
        if gpu.mesh.is_none() {
            let (vertices, indices) = water_ring_mesh_data();
            let geometry = Geometry::new(cx);
            geometry.update(cx, indices, vertices);
            gpu.mesh = Some(geometry);
        }
        if gpu.blank.is_none() {
            gpu.blank = Some(Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
                width: 1, height: 1, data: Some(vec![0x8080_8080]), updated: TextureUpdated::Full,
            }));
        }
        // Ripple maps: one per look, made once (a few ms).
        for v in water.volumes.iter().filter(|v| v.draw_sheet) {
            let key = ripple_key(&v.look);
            if !gpu.ripples.iter().any(|r| r.0 == key) {
                let (texels, tile, _) = v.look.detail_map();
                let tex = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
                    width: DETAIL_N, height: DETAIL_N, data: Some(texels), updated: TextureUpdated::Full,
                });
                gpu.ripples.push((key, tex, tile));
            }
        }
        // The sky the water reflects: re-baked when the sun or fog moves.
        let gain = if self.hdr_output { super::frame::HDR_SKY_GAIN } else { 1.0 };
        let fill = sun.sky * 2.5;
        // Quantised: a clock-driven sun re-bakes every ~0.1 degree, not
        // every frame.
        let q = |v: f32| (v * 512.0).round() as i32 as u32;
        let key = [
            q(sun.dir.x), q(sun.dir.y), q(sun.dir.z),
            q(fog.x * 64.0), q(fog.y * 64.0), q(fog.z * 64.0),
            sky_frame.map_or(0, |f| q(f.zenith.w) ^ q(f.dome_tint.x).rotate_left(16)),
            q(fill.y * 64.0),
        ];
        if gpu.sky.is_none() || gpu.sky_key != key {
            let texels = sky_texels(sky_frame.filter(|_| self.hdr_output), gain, fog, fill);
            match &gpu.sky {
                Some(t) => t.put_back_vec_u32(cx, texels, None),
                None => gpu.sky = Some(Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
                    width: SKY_W, height: SKY_H, data: Some(texels), updated: TextureUpdated::Full,
                })),
            }
            gpu.sky_key = key;
        }
        // Mesh-drawn surfaces: uploaded once per mesh, dropped with it.
        let live: Vec<usize> = water.volumes.iter().filter_map(|v| v.mesh.as_ref().map(|m| Arc::as_ptr(m) as usize)).collect();
        gpu.meshes.retain(|(k, ..)| live.contains(k));
        for v in &water.volumes {
            if let Some(m) = v.mesh.as_ref().filter(|_| v.draw_sheet) {
                let key = Arc::as_ptr(m) as usize;
                if !gpu.meshes.iter().any(|(k, ..)| *k == key) {
                    let (vertices, indices) = mesh_geometry_data(m);
                    let geometry = Geometry::new(cx);
                    geometry.update(cx, indices, vertices);
                    let mut min = vec3f(f32::MAX, f32::MAX, f32::MAX);
                    let mut max = vec3f(f32::MIN, f32::MIN, f32::MIN);
                    for p in &m.positions {
                        min = vec3f(min.x.min(p[0]), min.y.min(p[1]), min.z.min(p[2]));
                        max = vec3f(max.x.max(p[0]), max.y.max(p[1]), max.z.max(p[2]));
                    }
                    gpu.meshes.push((key, geometry, min, max));
                }
            }
        }
        // The seabed, when the world published one.
        let bed_key = water.bed.as_ref().map(|t| (Arc::as_ptr(t) as usize, t.revision));
        if bed_key != gpu.bed_key {
            gpu.bed = water.bed.as_deref().and_then(bed_texels).map(|(texels, w, h, rect, decode)| {
                (Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 { width: w, height: h, data: Some(texels), updated: TextureUpdated::Full }), rect, decode)
            });
            gpu.bed_key = bed_key;
            match (&gpu.bed, water.bed.as_deref()) {
                (Some((_, rect, decode)), Some(t)) => log!(
                    "water: seabed {}x{} cells of {:.1} m from {:.0}, heights {:.1}..{:.1} (rect {:?})",
                    t.cells, t.cells, t.cell_size, t.origin, decode[0], decode[0] + decode[1], rect
                ),
                _ => log!("water: no seabed (depth colour and shore foam off)"),
            }
        }
    }

    /// 3w. The water surfaces: drawn after every opaque pass (blending sees
    /// depth: a hull below the surface tints, one above does not) and before
    /// the alpha batches, so sensor ghosts and particles composite over it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_water(
        &mut self,
        cx: &mut Cx3d,
        draws: &mut SceneDraws,
        world: &World,
        sun: &SunLight,
        sky_frame: Option<&crate::sky::SkyFrame>,
        (fog_color, fog_density): (Vec3f, f32),
        frustum: Option<&Frustum>,
        shows_environment: bool,
        camera_pos: Vec3f,
    ) {
        let Some(water) = world.water.as_deref() else { return };
        if !shows_environment || !water.volumes.iter().any(|v| v.draw_sheet) {
            return;
        }
        let Some(water_draw) = draws.water.as_deref_mut() else { return };
        self.water_textures(cx.cx, water, sky_frame, fog_color, sun);
        let t = world.water_time;
        let under = eye_under_water(water, camera_pos, t).is_some();
        water_draw.transform = Mat4f::identity();
        water_draw.depth_clip = 1.0;
        water_draw.fog_color = fog_color;
        water_draw.fog_density = fog_density;
        sun.write_into(&mut water_draw.light_dir, &mut water_draw.sun_color, &mut water_draw.sun_sky, &mut water_draw.sun_ground);
        let dv = &mut water_draw.draw_vars;
        let lin = self.lin_ctl();
        dv.set_uniform(cx.cx, live_id!(lin_ctl), &lin);
        dv.set_uniform(cx.cx, live_id!(water_eye), &[camera_pos.x, camera_pos.y, camera_pos.z, 0.0]);
        let fog_ctl = self.clustered.fog_ctl;
        dv.set_uniform(cx.cx, live_id!(water_fog), &fog_ctl);
        let gpu = &self.water;
        let blank = gpu.blank.clone().unwrap();
        crate::fast_gi::bind_texture(cx.cx, dv, live_id!(sky_tex), gpu.sky.as_ref().unwrap_or(&blank));
        match &gpu.bed {
            Some((tex, rect, decode)) => {
                crate::fast_gi::bind_texture(cx.cx, dv, live_id!(bed_tex), tex);
                dv.set_uniform(cx.cx, live_id!(bed_rect), rect);
                dv.set_uniform(cx.cx, live_id!(bed_decode), &[decode[0], decode[1], 1.0, 0.0]);
            }
            None => {
                crate::fast_gi::bind_texture(cx.cx, dv, live_id!(bed_tex), &blank);
                dv.set_uniform(cx.cx, live_id!(bed_decode), &[0.0, 1.0, 0.0, 0.0]);
            }
        }
        const WAVE_A: [LiveId; 8] = [
            live_id!(wave_a0), live_id!(wave_a1), live_id!(wave_a2), live_id!(wave_a3),
            live_id!(wave_a4), live_id!(wave_a5), live_id!(wave_a6), live_id!(wave_a7),
        ];
        const WAVE_B: [LiveId; 8] = [
            live_id!(wave_b0), live_id!(wave_b1), live_id!(wave_b2), live_id!(wave_b3),
            live_id!(wave_b4), live_id!(wave_b5), live_id!(wave_b6), live_id!(wave_b7),
        ];
        let Some(rings) = gpu.mesh.as_ref() else { return };
        for volume in water.volumes.iter().filter(|v| v.draw_sheet) {
            let headroom = volume.amp_sum() + 0.5;
            let mesh_entry = match &volume.mesh {
                Some(m) => match gpu.meshes.iter().find(|(k, ..)| *k == Arc::as_ptr(m) as usize) {
                    Some(e) => Some(e),
                    None => continue,
                },
                None => None,
            };
            if !volume.unbounded {
                if let Some(frustum) = frustum {
                    let (min, max) = match mesh_entry {
                        Some((_, _, min, max)) => (*min - vec3f(0.0, headroom, 0.0), *max + vec3f(0.0, headroom, 0.0)),
                        None => (
                            vec3f(volume.min.x, volume.level() - headroom, volume.min.z),
                            vec3f(volume.max.x, volume.level() + headroom, volume.max.z),
                        ),
                    };
                    if !frustum.intersects_aabb(min, max) {
                        continue;
                    }
                }
            }
            // Rings around the eye, or the box's point nearest the eye: the
            // densest rings land where the eye looks closest.
            let (rect, cx_, cz_) = if volume.unbounded {
                ([-1.0e6, -1.0e6, 1.0e6, 1.0e6], camera_pos.x, camera_pos.z)
            } else {
                (
                    [volume.min.x, volume.min.z, volume.max.x, volume.max.z],
                    camera_pos.x.clamp(volume.min.x, volume.max.x),
                    camera_pos.z.clamp(volume.min.z, volume.max.z),
                )
            };
            let (a, b) = pack_wave_uniforms(volume);
            for i in 0..MAX_WAVES {
                dv.set_uniform(cx.cx, WAVE_A[i], &a[i]);
                dv.set_uniform(cx.cx, WAVE_B[i], &b[i]);
            }
            let amp = volume.amp_sum().max(0.01);
            dv.set_uniform(cx.cx, live_id!(water_center), &[cx_, volume.level(), cz_, 1.0 / amp]);
            // Is this the water the lit scene absorbs through (uw_*)?
            let uw = self.clustered.uw;
            let primary = uw[0][1] > 0.5 && uw[0][0] == volume.level()
                && uw[1] == if volume.unbounded { [-1.0e6, -1.0e6, 1.0e6, 1.0e6] } else { [volume.min.x, volume.min.z, volume.max.x, volume.max.z] };
            let geometry = match mesh_entry {
                Some((_, g, ..)) => g,
                None => rings,
            };
            let mesh_mode = if volume.mesh.is_some() { 1.0 } else { 0.0 };
            dv.set_uniform(cx.cx, live_id!(water_params), &[if under { 1.0 } else { 0.0 }, t, if primary { 1.0 } else { 0.0 }, mesh_mode]);
            dv.set_uniform(cx.cx, live_id!(water_rect), &rect);
            let look = &volume.look;
            dv.set_uniform(cx.cx, live_id!(sea_glow), &[look.glow[0], look.glow[1], look.glow[2], 0.0]);
            dv.set_uniform(cx.cx, live_id!(sea_shallow), &[look.shallow[0], look.shallow[1], look.shallow[2], look.clarity]);
            dv.set_uniform(cx.cx, live_id!(sea_deep), &[look.deep[0], look.deep[1], look.deep[2], look.caustics]);
            let key = ripple_key(look);
            let (ripple, tile) = gpu.ripples.iter().find(|r| r.0 == key).map(|r| (&r.1, r.2)).unwrap_or((&blank, 8.0));
            crate::fast_gi::bind_texture(cx.cx, dv, live_id!(detail_tex), ripple);
            dv.set_uniform(cx.cx, live_id!(sea_foam), &[look.foam, look.shore_foam, look.detail, tile]);
            let drift = 0.25 + look.wind * 0.06;
            dv.set_uniform(cx.cx, live_id!(sea_wind), &[look.wind_dir[0], look.wind_dir[1], drift, 0.0]);
            dv.geometry_id = Some(geometry.geometry_id());
            if dv.can_instance() {
                let new_area = cx.add_instance(dv);
                dv.area = cx.update_area_refs(dv.area, new_area);
            }
        }
    }
}
