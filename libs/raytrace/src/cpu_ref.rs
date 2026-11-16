//! The CPU reference integrator: a line-for-line twin of the trace shader's
//! `radiance()` (same sampler, same BVH walk, same BSDF, same NEE + MIS,
//! same Russian roulette and clamp). It exists to gate the GPU — the
//! selftest renders the Cornell box on both and compares — and it counts
//! rays, which the GPU cannot, so rays/s can be reported honestly.

use crate::bvh::Hit;
use crate::pack::{PackedScene, ENV_DIR_BINS, ENV_PHI_BINS, ENV_THETA_BINS, MAX_SHADOW_GLASS_HITS};
use crate::rng::{bounce_pairs, hash2, pixel_seed, sobol_2d, u32_to_unit};
use crate::scene::{Camera, Material, SceneInput};
use crate::sky::SkyUniforms;
use makepad_draw::*;

/// CPU-only transport counters. They do not participate in sampling, so the
/// reference estimator stays a line-for-line twin of the GPU shader.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuStats {
    pub paths: u64,
    pub primary_hits: u64,
    pub surface_hits: u64,
    pub escaped_to_sky: u64,
    pub sun_nee_successes: u64,
    pub sky_nee_successes: u64,
    pub emissive_nee_successes: u64,
    pub shadow_rays: u64,
    pub glass_shadow_hits: u64,
    pub opaque_backface_flips: u64,
    pub primary_backface_skips: u64,
}

impl CpuStats {
    pub fn average_bounces(self) -> f64 {
        self.surface_hits as f64 / self.paths.max(1) as f64
    }
}

pub struct CpuTracer<'a> {
    pub packed: &'a PackedScene,
    pub materials: &'a [Material],
    pub sky: SkyUniforms,
    pub camera: Camera,
    pub max_bounces: u32,
    pub max_diffuse: u32,
    /// Explicit biased preview mode. `None` is the unbiased reference path.
    pub preview_clamp: Option<f32>,
    /// BSDF-sampling-only transport (no NEE, no MIS) — diagnostics.
    pub brute: bool,
    /// Return (hit tri, t, tp.x) entering this bounce (diagnostics).
    pub probe_bounce: i32,
    /// CPU-only comparison sampler used by the RMSE regression.
    pub white_noise: bool,
    /// Shadow-ray self-occlusion skin (scene units), the GPU's `shadow_skin`
    /// uniform: next-event rays ignore blockers closer than this to the
    /// surface they leave. 0 disables (unbiased reference).
    pub shadow_skin: f32,
    pub rays: std::cell::Cell<u64>,
    pub invalid_samples: std::cell::Cell<u64>,
    stats: std::cell::Cell<CpuStats>,
}

fn v3(a: [f32; 3]) -> Vec3f {
    vec3f(a[0], a[1], a[2])
}

/// The GPU's precise `normalize`: each component divided by the correctly rounded length.
fn unit(v: Vec3f) -> Vec3f {
    let l = v.length();
    if l > 0.0 { vec3f(v.x / l, v.y / l, v.z / l) } else { v }
}

fn to_world(n: Vec3f, l: Vec3f) -> Vec3f {
    let sgn = if n.z >= 0.0 { 1.0 } else { -1.0 };
    let a = -1.0 / (sgn + n.z);
    let b = n.x * n.y * a;
    let t = vec3f(1.0 + sgn * n.x * n.x * a, sgn * b, -sgn * n.x);
    let bt = vec3f(b, sgn + n.y * n.y * a, -n.y);
    t * l.x + bt * l.y + n * l.z
}

fn ggx_d(nh: f32, a2: f32) -> f32 {
    let d = nh * nh * (a2 - 1.0) + 1.0;
    a2 / (std::f32::consts::PI * d * d)
}

fn bsdf_eval(n: Vec3f, v: Vec3f, l: Vec3f, kd: Vec3f, f0: Vec3f, alpha: f32) -> Vec3f {
    let h = unit(v + l);
    let nl = n.dot(l).max(0.0001);
    let nv = n.dot(v).max(0.0001);
    let nh = n.dot(h).max(0.0);
    let vh = v.dot(h).max(0.0);
    let a2 = alpha * alpha;
    let one = vec3f(1.0, 1.0, 1.0);
    let fr = f0 + (one - f0) * (1.0 - vh).clamp(0.0, 1.0).powi(5);
    let gv = nl * (nv * nv * (1.0 - a2) + a2).sqrt();
    let gl = nv * (nl * nl * (1.0 - a2) + a2).sqrt();
    let vis = 0.5 / (gv + gl).max(1.0e-6);
    kd * (one - fr) * std::f32::consts::FRAC_1_PI + fr * (ggx_d(nh, a2) * vis)
}

fn bsdf_pdf(n: Vec3f, v: Vec3f, l: Vec3f, alpha: f32, ps: f32) -> f32 {
    let h = unit(v + l);
    let nh = n.dot(h).max(0.0);
    let vh = v.dot(h).max(0.0001);
    let nl = n.dot(l).max(0.0);
    let a2 = alpha * alpha;
    ps * ggx_d(nh, a2) * nh / (4.0 * vh) + (1.0 - ps) * nl * std::f32::consts::FRAC_1_PI
}

fn spec_prob(f0: Vec3f, kd: Vec3f, nv: f32) -> f32 {
    let one = vec3f(1.0, 1.0, 1.0);
    let fr = f0 + (one - f0) * (1.0 - nv).clamp(0.0, 1.0).powi(5);
    let lf = fr.x.max(fr.y).max(fr.z);
    let lk = kd.x.max(kd.y).max(kd.z);
    (lf / (lf + lk + 0.0001)).clamp(0.05, 0.95)
}

fn fresnel_dielectric(cosi: f32, ior: f32) -> f32 {
    let cosi = cosi.clamp(0.0, 1.0);
    let sint2 = (1.0 - cosi * cosi) / (ior * ior);
    if sint2 >= 1.0 {
        return 1.0;
    }
    let cost = (1.0 - sint2).sqrt();
    let rs = (cosi - ior * cost) / (cosi + ior * cost);
    let rp = (ior * cosi - cost) / (ior * cosi + cost);
    0.5 * (rs * rs + rp * rp)
}

/// Exact unpolarised Fresnel, eta = incident IOR / transmitted IOR.
/// Keep the legacy thin-sheet arithmetic above unchanged.
fn solid_fresnel(cosi: f32, eta: f32) -> f32 {
    if eta == 1.0 {
        return 0.0;
    }
    let cosi = cosi.clamp(0.0, 1.0);
    let sint2 = eta * eta * (1.0 - cosi * cosi);
    if sint2 >= 1.0 {
        return 1.0;
    }
    let cost = (1.0 - sint2).sqrt();
    let rs = (eta * cosi - cost) / (eta * cosi + cost);
    let rp = (cosi - eta * cost) / (cosi + eta * cost);
    0.5 * (rs * rs + rp * rp)
}

/// Smooth solid boundary; `ng` is outward, including for an inside camera.
/// The caller supplies the existing lobe sample; TIR consumes no extra RNG.
fn solid_interface(rd: Vec3f, ng: Vec3f, ior: f32, sample: f32) -> (Vec3f, bool) {
    let front = ng.dot(rd) < 0.0;
    let eta = if front { 1.0 / ior } else { ior };
    let n = if front { ng } else { -ng };
    let cosi = (-rd.dot(n)).clamp(0.0, 1.0);
    let fr = solid_fresnel(cosi, eta);
    if fr >= 1.0 || sample < fr {
        return (rd - n * (2.0 * rd.dot(n)), true);
    }
    let cost = (1.0 - eta * eta * (1.0 - cosi * cosi)).max(0.0).sqrt();
    (unit(rd * eta + n * (eta * cosi - cost)), false)
}

fn medium_transmittance(absorption: [f32; 3], distance: f32) -> Vec3f {
    // Define zero distance explicitly, including fully absorbing channels.
    if distance <= 0.0 {
        return vec3f(1.0, 1.0, 1.0);
    }
    vec3f(
        absorption[0].clamp(0.0, 1.0).powf(distance),
        absorption[1].clamp(0.0, 1.0).powf(distance),
        absorption[2].clamp(0.0, 1.0).powf(distance),
    )
}

fn mix3(a: Vec3f, b: Vec3f, t: f32) -> Vec3f {
    a + (b - a) * t
}

fn ray_error(p: Vec3f, scene_scale: f32) -> f32 {
    // Relative reconstruction bound (16 f32 ulps at the largest component)
    // plus a scene-relative floor for geometry near the rebased origin.
    scene_scale * 5.0e-9 + p.x.abs().max(p.y.abs()).max(p.z.abs()) * 1.907_348_6e-6
}

fn offset_ray(p: Vec3f, ng: Vec3f, dir: Vec3f, scene_scale: f32) -> Vec3f {
    let n = if ng.dot(dir) >= 0.0 { ng } else { -ng };
    let eps = ray_error(p, scene_scale);
    // Geometric-normal offset leaves the source plane; the along-ray nudge
    // leaves a concave neighbour that is perpendicular to ng (a single-axis
    // offset stays on that neighbour's plane at a shared edge).
    p + n * eps + dir * eps
}

fn geom_ng(tri: &crate::bvh::Tri) -> Vec3f {
    unit(Vec3f::cross(tri.v1 - tri.v0, tri.v2 - tri.v0))
}

/// Offset origin sits behind an adjacent face within the scene-relative band.
fn spurious_neighbour(hit: &Hit, rd: Vec3f, tris: &[crate::bvh::Tri], scene_scale: f32) -> bool {
    if !hit.is_hit() || hit.t >= scene_scale * 5.0e-5 {
        return false;
    }
    geom_ng(&tris[hit.tri as usize]).dot(rd) > 0.0
}

fn shading_normal_correction(ng: Vec3f, ns: Vec3f, v: Vec3f, l: Vec3f) -> f32 {
    // Bounded: at grazing geometry the raw ratio is unbounded and turns a
    // modest NEE sample into a firefly. 4x covers every legitimate
    // smooth-normal correction on building geometry.
    ((v.dot(ns) * l.dot(ng)) / (v.dot(ng) * l.dot(ns)).max(1.0e-6)).abs().min(4.0)
}

#[derive(Clone, Copy, Debug, Default)]
struct ShadowVisibility {
    transmittance: Vec3f,
}

#[derive(Clone, Copy, Debug, Default)]
struct EnvironmentSample {
    direction: Vec3f,
    radiance: Vec3f,
    pdf: f32,
}

impl<'a> CpuTracer<'a> {
    fn bump_stats(&self, update: impl FnOnce(&mut CpuStats)) {
        let mut stats = self.stats.get();
        update(&mut stats);
        self.stats.set(stats);
    }

    pub fn stats(&self) -> CpuStats {
        self.stats.get()
    }

    pub fn reset_stats(&self) {
        self.stats.set(CpuStats::default());
    }

    fn trace(&self, ro: Vec3f, rd: Vec3f, tmax: f32, any: bool, tmin: f32, skip: i32) -> Hit {
        self.rays.set(self.rays.get() + 1);
        self.packed.accel.trace_skip(ro, rd, tmin, tmax, any, skip)
    }

    /// Secondary/shadow spawn: offset along ng and the ray, skip the source
    /// triangle, and if the offset origin is inside a concave neighbour
    /// (possibly several overlapping BIM faces) raise tmin past each
    /// back-facing hit within 5e-5 of the scene diagonal.
    fn secondary_trace(&self, p: Vec3f, ng: Vec3f, rd: Vec3f, tmax: f32, skip: i32) -> Hit {
        let ro = offset_ray(p, ng, rd, self.packed.scene_scale);
        let mut tmin = 0.0;
        let mut hit = Hit::miss(tmax);
        for _ in 0..4 {
            hit = self.trace(ro, rd, tmax, false, tmin, skip);
            if hit.truncated {
                // A truncated traversal must surface as truncated (GPU twin
                // returns immediately on its -2 code); the ladder must not
                // retry it into a fake clean result.
                return hit;
            }
            if !spurious_neighbour(&hit, rd, &self.packed.accel.tris, self.packed.scene_scale) {
                return hit;
            }
            // A nearby solid exit is a real medium boundary, not a BIM
            // reconstruction artefact. Never skip it, however thin the wall.
            let m = &self.materials[self.packed.tri_material[hit.tri as usize] as usize];
            if m.solid && m.transmission > 0.0 {
                return hit;
            }
            let tri = &self.packed.accel.tris[hit.tri as usize];
            let n = geom_ng(tri);
            let plane = n.dot(p - tri.v0).abs() / n.dot(rd).abs().max(1.0e-6);
            tmin = plane.max(hit.t) + ray_error(p, self.packed.scene_scale);
        }
        hit
    }

    /// BIM panes are often exported as a thin closed box although their
    /// material semantics are thin. Cross the paired back face without a
    /// second Fresnel/transmittance event or an extra path bounce.
    fn thin_transmission_trace(&self, p: Vec3f, ng: Vec3f, rd: Vec3f, tri: usize) -> Hit {
        let mut hit = self.secondary_trace(p, ng, rd, self.packed.ray_tmax(), tri as i32);
        if !hit.is_hit() || hit.t >= self.packed.scene_scale * 1.0e-4 {
            return hit;
        }
        let back_tri = hit.tri as usize;
        if self.packed.tri_material[back_tri] != self.packed.tri_material[tri]
            || geom_ng(&self.packed.accel.tris[back_tri]).dot(geom_ng(&self.packed.accel.tris[tri])) > -0.9
        {
            return hit;
        }
        let back = &self.packed.accel.tris[back_tri];
        let back_p = back.v0 * (1.0 - hit.u - hit.v) + back.v1 * hit.u + back.v2 * hit.v;
        hit = self.secondary_trace(back_p, geom_ng(back), rd, self.packed.ray_tmax(), back_tri as i32);
        hit
    }

    /// Deterministic straight-through guide, with eight continuation steps.
    /// Legacy thin panes may pair their back face in one step; solids never do.
    /// Select transmission whenever it exists (no RNG or Fresnel reflection);
    /// TIR, exhausted traversal/budget, and escape have no surface guide.
    fn guide_hit(&self, mut hit: Hit, rd: Vec3f) -> Hit {
        for layer in 0..=MAX_SHADOW_GLASS_HITS {
            if !hit.is_hit() || hit.truncated {
                return hit;
            }
            let ti = hit.tri as usize;
            let m = &self.materials[self.packed.tri_material[ti] as usize];
            if m.transmission <= 0.0 {
                return hit;
            }
            if layer == MAX_SHADOW_GLASS_HITS {
                break;
            }
            let tri = &self.packed.accel.tris[ti];
            let ng = geom_ng(tri);
            let pt = tri.v0 * (1.0 - hit.u - hit.v) + tri.v1 * hit.u + tri.v2 * hit.v;
            if m.solid {
                let ior = m.ior.max(1.0);
                let eta = if ng.dot(rd) < 0.0 { 1.0 / ior } else { ior };
                if solid_fresnel(ng.dot(-rd).abs(), eta) >= 1.0 {
                    break;
                }
                hit = self.secondary_trace(pt, ng, rd, self.packed.ray_tmax(), ti as i32);
            } else {
                hit = self.thin_transmission_trace(pt, ng, rd, ti);
            }
        }
        Hit::miss(self.packed.ray_tmax())
    }

    /// CPU mirror of PtGuide: world-space pinhole/orthographic ray with a unit
    /// direction. Returns interpolated normal and camera-forward depth, or -1 without a
    /// guide. Existing first opaque hits retain the raster's ortho depth 1.
    pub fn denoise_guide(&self, origin: Vec3f, direction: Vec3f) -> Vec4f {
        let ro = origin - self.packed.origin;
        let first = self.trace(ro, direction, self.packed.ray_tmax(), false, 0.0, -1);
        let hit = self.guide_hit(first, direction);
        if !hit.is_hit() || hit.truncated {
            return vec4f(0.0, 0.0, 0.0, -1.0);
        }
        let ti = hit.tri as usize;
        let w0 = 1.0 - hit.u - hit.v;
        let a = &self.packed.attr.data[ti * 16..ti * 16 + 16];
        let n = unit(vec3f(a[0], a[1], a[2]) * w0
            + vec3f(a[4], a[5], a[6]) * hit.u
            + vec3f(a[8], a[9], a[10]) * hit.v);
        let tri = &self.packed.accel.tris[ti];
        let pt = tri.v0 * w0 + tri.v1 * hit.u + tri.v2 * hit.v;
        let depth = if hit.tri == first.tri && self.camera.ortho_height.is_some() {
            1.0
        } else {
            (pt - (self.camera.pos - self.packed.origin)).dot(self.camera.basis().2)
        };
        vec4f(n.x, n.y, n.z, depth)
    }

    fn sample2(&self, pseed: u32, index: u32, pair: u32) -> (f32, f32) {
        if !self.white_noise {
            return sobol_2d(index, pseed, pair);
        }
        let base = hash2(pseed, index);
        (
            u32_to_unit(hash2(base, pair.wrapping_mul(2).wrapping_add(7777))),
            u32_to_unit(hash2(base, pair.wrapping_mul(2).wrapping_add(7778))),
        )
    }

    /// `footprint_m` is the pixel's world footprint at the hit (0 = skip
    /// the minification blend, e.g. shadow-ray glass tints). GPU twin: the
    /// blend in `PtTrace`'s albedo fetch.
    fn atlas_albedo(&self, ti: usize, w0: f32, u: f32, v: f32, base: Vec3f, footprint_m: f32) -> Vec3f {
        let mi = self.packed.tri_material[ti] as usize;
        let m2 = &self.packed.mat.data[mi * 20 + 8..mi * 20 + 12];
        if m2[2] <= 0.0 {
            return base;
        }
        let Some(atlas) = &self.packed.atlas else { return base };
        let a = &self.packed.attr.data[ti * 16..ti * 16 + 16];
        let uv = vec2f(
            a[3] * w0 + a[11] * u + a[13] * v,
            a[7] * w0 + a[12] * u + a[14] * v,
        );
        let r = &self.packed.mat.data[mi * 20 + 12..mi * 20 + 16];
        let fu = uv.x - uv.x.floor();
        let fv = uv.y - uv.y.floor();
        let x = ((r[0] + fu * r[2]) * atlas.width as f32).floor() as usize;
        let y = ((r[1] + fv * r[3]) * atlas.height as f32).floor() as usize;
        let px = atlas.data[y.min(atlas.height - 1) * atlas.width + x.min(atlas.width - 1)];
        let mut c = vec3f(((px >> 16) & 255) as f32, ((px >> 8) & 255) as f32, (px & 255) as f32) * (1.0 / 255.0);
        if footprint_m > 0.0 {
            // Minification blend toward the image mean (the atlas corner
            // texel one texel above-left of the rect): a nearest fetch on a
            // heavily tiled texture is a per-sample texel lottery.
            let tri = &self.packed.accel.tris[ti];
            let e1 = tri.v1 - tri.v0;
            let e2 = tri.v2 - tri.v0;
            let gn = Vec3f::cross(e1, e2);
            let duv1 = vec2f(a[11] - a[3], a[12] - a[7]);
            let duv2 = vec2f(a[13] - a[3], a[14] - a[7]);
            let uv_area = (duv1.x * duv2.y - duv1.y * duv2.x).abs();
            // Worst-axis density plus grazing stretch (GPU twin comments).
            let d_area = (uv_area / gn.length().max(self.packed.scene_area * 2.5e-17)).sqrt();
            let d1 = duv1.length() / e1.length().max(self.packed.scene_scale * 5.0e-9);
            let d2 = duv2.length() / e2.length().max(self.packed.scene_scale * 5.0e-9);
            let density = d_area.max(d1).max(d2);
            let texels = density * footprint_m * (r[2] * atlas.width as f32);
            let blend = ((texels - 2.0) * 0.071428575).clamp(0.0, 1.0);
            if blend > 0.0 {
                let mx = ((r[0] * atlas.width as f32).floor() as usize).saturating_sub(1);
                let my = ((r[1] * atlas.height as f32).floor() as usize).saturating_sub(1);
                let mp = atlas.data[my * atlas.width + mx];
                let mean = vec3f(
                    ((mp >> 16) & 255) as f32,
                    ((mp >> 8) & 255) as f32,
                    (mp & 255) as f32,
                ) * (1.0 / 255.0);
                c = c + (mean - c) * blend;
            }
        }
        base * vec3f(c.x.powf(2.2), c.y.powf(2.2), c.z.powf(2.2))
    }

    fn sun_is_on(&self) -> bool {
        self.sky.up.w > 0.5 && self.sky.sun_radiance.x > 0.0
    }

    fn sun_dir(&self) -> Vec3f {
        vec3f(self.sky.sun_dir.x, self.sky.sun_dir.y, self.sky.sun_dir.z)
    }

    fn sample_sun(&self, random: (f32, f32)) -> Vec3f {
        let cos_theta = 1.0 - random.0 * (1.0 - self.sky.sun_dir.w);
        let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
        let phi = std::f32::consts::TAU * random.1;
        to_world(
            self.sun_dir(),
            vec3f(
                sin_theta * phi.cos(),
                sin_theta * phi.sin(),
                cos_theta,
            ),
        )
    }

    /// Disc solid angle × disc radiance = unoccluded irradiance.
    fn sun_irradiance(&self) -> Vec3f {
        let omega = std::f32::consts::TAU * (1.0 - self.sky.sun_dir.w);
        vec3f(self.sky.sun_radiance.x, self.sky.sun_radiance.y, self.sky.sun_radiance.z) * omega
    }

    fn environment_radiance(&self, rd: Vec3f, include_sun: bool) -> Vec3f {
        self.sky.environment_radiance(rd, include_sun)
    }

    fn sky_pdf(&self, rd: Vec3f) -> f32 {
        let up = vec3f(self.sky.up.x, self.sky.up.y, self.sky.up.z);
        let z = rd.dot(up);
        if z <= 0.0 {
            0.0
        } else {
            // Equal mixture of a uniform upper dome and the analytic
            // horizon lobe z=u^2. Perez daylight is horizon-heavy.
            0.25 * std::f32::consts::FRAC_1_PI
                + 0.125 * std::f32::consts::FRAC_1_PI / z.max(1.0e-6).sqrt()
        }
    }

    fn environment_distribution(&self, p: Vec3f, normal: Vec3f) -> usize {
        let d = self.packed.env_grid_dim as usize;
        let rel = vec3f(
            (p.x - self.packed.env_min.x) * self.packed.env_inv_extent.x,
            (p.y - self.packed.env_min.y) * self.packed.env_inv_extent.y,
            (p.z - self.packed.env_min.z) * self.packed.env_inv_extent.z,
        );
        let x = (rel.x.clamp(0.0, 0.999_999_94) * d as f32) as usize;
        let y = (rel.y.clamp(0.0, 0.999_999_94) * d as f32) as usize;
        let z = (rel.z.clamp(0.0, 0.999_999_94) * d as f32) as usize;
        let axis = if normal.x.abs() >= normal.y.abs() && normal.x.abs() >= normal.z.abs() {
            if normal.x >= 0.0 { 0 } else { 1 }
        } else if normal.y.abs() >= normal.z.abs() {
            if normal.y >= 0.0 { 2 } else { 3 }
        } else if normal.z >= 0.0 {
            4
        } else {
            5
        };
        ((z * d + y) * d + x) * 6 + axis
    }

    fn sample_guided_environment(&self, p: Vec3f, normal: Vec3f, r: (f32, f32)) -> EnvironmentSample {
        let records = &self.packed.light.data;
        let offset = self.packed.n_lights + self.environment_distribution(p, normal) * ENV_DIR_BINS;
        let mut lo = 0usize;
        let mut hi = ENV_DIR_BINS;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if r.0 < records[(offset + mid) * 4 + 3] {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        let direction_bin = lo.min(ENV_DIR_BINS - 1);
        let rec = &records[(offset + direction_bin) * 4..(offset + direction_bin + 1) * 4];
        let previous_cdf = if direction_bin == 0 {
            0.0
        } else {
            records[(offset + direction_bin - 1) * 4 + 3]
        };
        let local_u = ((r.0 - previous_cdf) / rec[2].max(1.0e-8)).clamp(0.0, 0.999_999_94);
        let bin = rec[0] as usize;
        let theta_bin = bin / ENV_PHI_BINS;
        let phi_bin = bin % ENV_PHI_BINS;
        let cos_theta = (theta_bin as f32 + local_u) / ENV_THETA_BINS as f32;
        let sin_theta = (1.0 - cos_theta * cos_theta).sqrt();
        let phi = std::f32::consts::TAU * (phi_bin as f32 + r.1) / ENV_PHI_BINS as f32;
        let up = vec3f(self.sky.up.x, self.sky.up.y, self.sky.up.z);
        let direction = to_world(up, vec3f(sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta));
        EnvironmentSample {
            direction,
            radiance: self.environment_radiance(direction, false),
            pdf: rec[2] / rec[1].max(1.0e-12),
        }
    }

    fn sample_environment(&self, p: Vec3f, normal: Vec3f, r: (f32, f32)) -> EnvironmentSample {
        if self.packed.env_grid_dim > 0 {
            return self.sample_guided_environment(p, normal, r);
        }
        let up = vec3f(self.sky.up.x, self.sky.up.y, self.sky.up.z);
        let x = r.0.clamp(0.0, 0.999_999_94);
        let z = if x < 0.5 {
            (x * 2.0).powi(2)
        } else {
            (x - 0.5) * 2.0
        };
        let rr = (1.0 - z * z).sqrt();
        let phi = std::f32::consts::TAU * r.1;
        let ld = to_world(up, vec3f(rr * phi.cos(), rr * phi.sin(), z));
        EnvironmentSample {
            direction: ld,
            radiance: self.environment_radiance(ld, false),
            pdf: self.sky_pdf(ld),
        }
    }

    fn escape(&self, rd: Vec3f, prev_pdf: f32, delta: bool) -> Vec3f {
        // Brute mode is the BSDF-only oracle: no NEE runs, so nothing may be
        // suppressed here — return the complete environment, sun included.
        if self.brute {
            return self.environment_radiance(rd, true);
        }
        // Camera rays see the disc; indirect BSDF bounces leave its tiny
        // solid angle to the explicit uniform-disc NEE sampler.
        let l = self.environment_radiance(rd, delta);
        if delta {
            return l;
        }
        // The visibility guide owns upper-hemisphere environment paths.
        if self.packed.env_grid_dim > 0 {
            let up = vec3f(self.sky.up.x, self.sky.up.y, self.sky.up.z);
            // Preserve BSDF escape below it, where the analytic environment
            // models ground haze and the guide has no support.
            return if rd.dot(up) < 0.0 { l } else { Vec3f::default() };
        }
        let pe = self.sky_pdf(rd);
        let w = prev_pdf * prev_pdf / (prev_pdf * prev_pdf + pe * pe);
        l * w
    }

    /// Straight NEE through thin/solid glass: ignore bending, multiply each
    /// interface's (1-F) and transmission, and attenuate back-side solid
    /// arrivals by their segment length. Exceeding the budget is opaque.
    fn shadow(&self, mut p: Vec3f, mut ng: Vec3f, rd: Vec3f, mut tmax: f32, mut skip: i32) -> Option<ShadowVisibility> {
        self.bump_stats(|stats| stats.shadow_rays += 1);
        let mut tr = vec3f(1.0, 1.0, 1.0);
        let mut travelled = 0.0f32;
        let mut previous_glass: Option<(u32, Vec3f)> = None;
        // The extra trace proves that the ray escaped after the last allowed hit.
        for layer in 0..=MAX_SHADOW_GLASS_HITS {
            let h = self.secondary_trace(p, ng, rd, tmax, skip);
            if h.truncated {
                return None;
            }
            if !h.is_hit() {
                return Some(ShadowVisibility { transmittance: tr });
            }
            if layer == MAX_SHADOW_GLASS_HITS {
                return Some(ShadowVisibility::default());
            }
            let ti = h.tri as usize;
            let material_id = self.packed.tri_material[ti];
            let m = &self.materials[material_id as usize];
            let solid = m.solid && m.transmission > 0.0;
            // Self-occlusion skin (GPU twin): a blocker within `shadow_skin`
            // of the ORIGINAL surface is a stacked coplanar layer of the
            // same construction assembly, not a shadow caster — step past
            // it. `travelled` bounds the total skip to one skin depth.
            if !solid && travelled + h.t < self.shadow_skin {
                travelled += h.t;
                let tri = &self.packed.accel.tris[ti];
                ng = geom_ng(tri);
                let w0 = 1.0 - h.u - h.v;
                p = tri.v0 * w0 + tri.v1 * h.u + tri.v2 * h.v;
                skip = h.tri;
                if tmax < self.packed.scene_scale * 2.5e6 {
                    tmax -= h.t + ray_error(p, self.packed.scene_scale);
                    if tmax <= 0.0 {
                        return Some(ShadowVisibility { transmittance: tr });
                    }
                }
                continue;
            }
            let trans = m.transmission.clamp(0.0, 1.0);
            if trans <= 0.0 {
                return Some(ShadowVisibility::default());
            }
            self.bump_stats(|stats| stats.glass_shadow_hits += 1);
            let tri = &self.packed.accel.tris[ti];
            let w0 = 1.0 - h.u - h.v;
            let segment_start = p;
            p = tri.v0 * w0 + tri.v1 * h.u + tri.v2 * h.v;
            ng = geom_ng(tri);
            let paired_back_face = previous_glass
                .is_some_and(|(previous_material, previous_normal)| {
                    previous_material == material_id && h.t < self.packed.scene_scale * 1.0e-4 && previous_normal.dot(ng) < -0.9
                });
            if paired_back_face {
                previous_glass = None;
                skip = h.tri;
                if tmax < self.packed.scene_scale * 2.5e6 {
                    tmax -= h.t + ray_error(p, self.packed.scene_scale);
                    if tmax <= 0.0 {
                        return Some(ShadowVisibility { transmittance: tr });
                    }
                }
                continue;
            }
            if solid {
                let front = ng.dot(rd) < 0.0;
                let ior = m.ior.max(1.0);
                let eta = if front { 1.0 / ior } else { ior };
                if !front {
                    tr = tr * medium_transmittance(m.absorption, (p - segment_start).length());
                }
                tr = tr * (trans * (1.0 - solid_fresnel(ng.dot(-rd).abs(), eta)));
                // Solids never use the thin exported-pane pairing shortcut.
                previous_glass = None;
            } else {
                let tint = self.atlas_albedo(ti, w0, h.u, h.v, v3(m.albedo), 0.0);
                let fr = fresnel_dielectric(ng.dot(-rd).abs(), m.ior.max(1.0));
                tr = tr * tint * (trans * (1.0 - fr));
                previous_glass = Some((material_id, ng));
            }
            skip = h.tri;
            if tmax < self.packed.scene_scale * 2.5e6 {
                tmax -= h.t + ray_error(p, self.packed.scene_scale);
                if tmax <= 0.0 {
                    return Some(ShadowVisibility { transmittance: tr });
                }
            }
        }
        Some(ShadowVisibility::default())
    }

    /// One path for pixel (px, py) with sample index `sidx` (the shader's
    /// non-G-buffer branch: jittered pinhole / thin lens).
    fn radiance_checked(&self, px: u32, py: u32, w: u32, h: u32, seed: u32, sidx: u32) -> Option<Vec3f> {
        let pseed = pixel_seed(px, py, seed);
        let p = self.packed;
        let (right, up, fwd) = self.camera.basis();
        let aspect = w as f32 / h as f32;
        let tan_y = (self.camera.fov_y * 0.5).tan();
        let inv = vec2f(1.0 / w as f32, 1.0 / h as f32);
        let (jx, jy) = self.sample2(pseed, sidx, 0);
        let mut ro = self.camera.pos - p.origin;
        let ndc = vec2f((px as f32 + jx) * inv.x * 2.0 - 1.0, 1.0 - (py as f32 + jy) * inv.y * 2.0);
        let mut rd = unit(fwd + right * (ndc.x * (tan_y * aspect)) + up * (ndc.y * tan_y));
        if let Some(hh) = self.camera.ortho_height {
            ro = ro + right * (ndc.x * (hh * 0.5 * aspect)) + up * (ndc.y * (hh * 0.5));
            rd = fwd;
        }
        let lens_r = self.camera.lens_radius();
        if lens_r > 0.0 && self.camera.ortho_height.is_none() {
            let (lu, lv) = self.sample2(pseed, sidx, 1);
            let rr = lu.sqrt();
            let phi = std::f32::consts::TAU * lv;
            let ap = vec2f(rr * phi.cos(), rr * phi.sin()) * lens_r;
            let fp = ro + rd * (self.camera.focus_dist / rd.dot(fwd).max(0.001));
            ro = ro + right * ap.x + up * ap.y;
            rd = unit(fp - ro);
        }
        let mut hit = self.trace(ro, rd, p.ray_tmax(), false, 0.0, -1);
        self.bump_stats(|stats| {
            stats.paths += 1;
            if hit.is_hit() {
                stats.primary_hits += 1;
            }
        });
        // One pixel's world footprint (the GPU's `pixel_world` uniform):
        // scene units per unit ray length, or scene units for ortho.
        let pixel_world = match self.camera.ortho_height {
            Some(hh) => hh / h as f32,
            None => 2.0 * tan_y / h as f32,
        };
        // Primary parity with the raster pane: a camera ray passes through
        // one-sided backfaces exactly as fixed-function backface culling
        // does, so the two panes agree about WHICH surface exists at every
        // pixel (a wrong-wound sign plate or deck otherwise shows in one
        // pane only). Bounce rays keep flip-shading below — culling them
        // would leak sky through every single-sided wall into the GI.
        // Single-hop continuations only: the spurious-neighbour ladder in
        // `secondary_trace` is material-blind and could step past a paired
        // two-sided glazing backface the raster would keep. Every returned
        // surface comes back to this loop for its own facing/material test.
        // A stack deeper than eight falls through to the bounce loop's
        // flip-shading — a visible ninth layer, never a rejected sample
        // (rejection is what freezes a pixel's display).
        for _ in 0..8 {
            if !hit.is_hit() || hit.truncated {
                break;
            }
            let ti = hit.tri as usize;
            let tri = &p.accel.tris[ti];
            let png = geom_ng(tri);
            if png.dot(rd) < 0.0 {
                break;
            }
            let material = &self.materials[p.tri_material[ti] as usize];
            if material.two_sided || (material.solid && material.transmission > 0.0) {
                break;
            }
            self.bump_stats(|stats| stats.primary_backface_skips += 1);
            let w0 = 1.0 - hit.u - hit.v;
            let pt = tri.v0 * w0 + tri.v1 * hit.u + tri.v2 * hit.v;
            hit = self.trace(offset_ray(pt, png, rd, p.scene_scale), rd, p.ray_tmax(), false, 0.0, ti as i32);
        }
        let mut tp = vec3f(1.0, 1.0, 1.0);
        let mut lsum = vec3f(0.0, 0.0, 0.0);
        let mut prev_pdf = 0.0f32;
        let mut delta = true;
        let mut ndiff = 0.0f32;
        for b in 0..self.max_bounces.min(16) {
            if hit.truncated {
                return None;
            }
            if self.probe_bounce >= 0 && b == self.probe_bounce as u32 {
                return Some(vec3f(hit.tri as f32 + 1000.0, hit.t, tp.x));
            }
            if !hit.is_hit() {
                self.bump_stats(|stats| stats.escaped_to_sky += 1);
                lsum = lsum + tp * self.escape(rd, prev_pdf, delta);
                break;
            }
            self.bump_stats(|stats| stats.surface_hits += 1);
            let ti = hit.tri as usize;
            let tri = &p.accel.tris[ti];
            let w0 = 1.0 - hit.u - hit.v;
            let pt = tri.v0 * w0 + tri.v1 * hit.u + tri.v2 * hit.v;
            let e1 = tri.v1 - tri.v0;
            let e2 = tri.v2 - tri.v0;
            let gn = Vec3f::cross(e1, e2);
            let mut ng = unit(gn);
            let a = &p.attr.data[ti * 16..ti * 16 + 16];
            let n0 = vec3f(a[0], a[1], a[2]);
            let n1 = vec3f(a[4], a[5], a[6]);
            let n2 = vec3f(a[8], a[9], a[10]);
            let mut ns = unit(n0 * w0 + n1 * hit.u + n2 * hit.v);
            let m = &self.materials[p.tri_material[ti] as usize];
            let front = ng.dot(rd) < 0.0;
            if m.solid && m.transmission > 0.0 && !front {
                // ro is the physical previous vertex/camera, not its offset.
                tp = tp * medium_transmittance(m.absorption, (pt - ro).length());
            }
            if !front {
                // A one-sided face seen from behind shades with the flipped
                // normal instead of terminating the path black. The realtime
                // rasterizer culls such faces; painting them black made every
                // exposed underside (shingle overlaps, framing bays, a
                // wrong-wound roof plane) a hard black stripe the raster pane
                // never shows. Emission stays front-gated below, so the
                // sidedness test for lights is unchanged.
                if !m.two_sided && !(m.solid && m.transmission > 0.0) {
                    self.bump_stats(|stats| stats.opaque_backface_flips += 1);
                }
                ng = -ng;
            }
            if ns.dot(ng) < 0.0 {
                ns = -ns;
            }
            if ns.dot(ng) < 0.05 {
                ns = ng;
            }
            let footprint_m = if self.camera.ortho_height.is_some() {
                pixel_world
            } else {
                pixel_world * hit.t
            } / ng.dot(rd).abs().max(0.1);
            let albedo = self.atlas_albedo(ti, w0, hit.u, hit.v, v3(m.albedo), footprint_m);
            let rough = m.roughness.clamp(0.0, 1.0).max(0.03);
            let emission = v3(m.emission);
            let metal = m.metal.clamp(0.0, 1.0);
            let ior = m.ior.max(1.0);
            let trans = m.transmission.clamp(0.0, 1.0);
            if emission.x + emission.y + emission.z > 0.0 && (front || m.two_sided) {
                let mut wgt = 1.0;
                if !delta && !self.brute {
                    let area = gn.length() * 0.5;
                    let raw_n = gn / gn.length().max(p.scene_area * 2.5e-25);
                    let cosl = if m.two_sided { raw_n.dot(-rd).abs() } else { raw_n.dot(-rd).max(0.0) };
                    let select_pdf = p.tri.data[ti * 12 + 11];
                    let pl = hit.t * hit.t * select_pdf / (cosl * area).max(p.scene_area * 2.5e-11);
                    wgt = prev_pdf * prev_pdf / (prev_pdf * prev_pdf + pl * pl);
                }
                lsum = lsum + tp * emission * wgt;
            }
            let dims = bounce_pairs(b);
            let r_lobe = self.sample2(pseed, sidx, dims[0]);
            let r_bsdf = self.sample2(pseed, sidx, dims[1]);
            let r_env = self.sample2(pseed, sidx, dims[2]);
            let r_light = self.sample2(pseed, sidx, dims[3]);
            let r_misc = self.sample2(pseed, sidx, dims[4]);
            let v = -rd;
            if m.solid && trans > 0.0 {
                let (direction, reflected) = solid_interface(rd, unit(gn), ior, r_lobe.0);
                rd = direction;
                if reflected {
                    delta = true;
                } else {
                    tp = tp * trans;
                    // As for thin sheets, preserve diffuse NEE ownership
                    // under the straight-shadow approximation.
                }
                ro = pt;
                hit = self.secondary_trace(pt, ng, rd, p.ray_tmax(), ti as i32);
                continue;
            }
            // Classified glazing is a thin dielectric, not an alpha blend
            // between glass and an opaque diffuse sheet. Alpha supplies
            // absorption on transmission; Fresnel is the only lobe choice.
            if trans > 0.0 {
                ro = pt;
                let fr = fresnel_dielectric(ng.dot(v).abs(), ior);
                if r_lobe.0 < fr {
                    rd = rd - ng * (2.0 * rd.dot(ng));
                    delta = true;
                    hit = self.secondary_trace(pt, ng, rd, p.ray_tmax(), ti as i32);
                } else {
                    tp = tp * albedo * trans;
                    // Keep the previous event's delta state: after a diffuse
                    // arrival, environment NEE already owns this light path.
                    hit = self.thin_transmission_trace(pt, ng, rd, ti);
                }
                continue;
            }
            let rx = r_lobe.0;
            let f0 = mix3(vec3f(0.04, 0.04, 0.04), albedo, metal);
            let kd = albedo * (1.0 - metal);
            let alpha = rough * rough;
            let nv = ns.dot(v).max(0.0001);
            let mut ps = spec_prob(f0, kd, nv);
            if !self.brute {
                if self.sun_is_on() {
                    // One uniform solar-disc shadow ray. The tiny solid angle
                    // is handled explicitly instead of relying on a BSDF hit.
                    let ld = self.sample_sun(r_light);
                    let n_nee = if ng.dot(ld) > 0.0 && ns.dot(ld) <= 0.0 { ng } else { ns };
                    let nl = n_nee.dot(ld);
                    if nl > 0.0 && ng.dot(ld) > 0.0 {
                        let Some(vis) = self.shadow(pt, ng, ld, p.ray_tmax(), ti as i32) else { return None };
                        let vis = vis.transmittance;
                        if vis.x + vis.y + vis.z > 0.0 {
                            self.bump_stats(|stats| stats.sun_nee_successes += 1);
                            let e = self.sun_irradiance();
                            let f = bsdf_eval(n_nee, v, ld, kd, f0, alpha);
                            let corr = shading_normal_correction(ng, n_nee, v, ld);
                            lsum = lsum + tp * f * e * vis * (nl * corr);
                        }
                    }
                }
                let env = self.sample_environment(pt, ng, r_env);
                let ld = env.direction;
                let n_nee = if ng.dot(ld) > 0.0 && ns.dot(ld) <= 0.0 { ng } else { ns };
                let nl = n_nee.dot(ld);
                if nl > 0.0 && ng.dot(ld) > 0.0 {
                    let Some(vis) = self.shadow(pt, ng, ld, p.ray_tmax(), ti as i32) else { return None };
                    let vis = vis.transmittance;
                    if env.pdf > 0.0 && vis.x + vis.y + vis.z > 0.0 {
                        self.bump_stats(|stats| stats.sky_nee_successes += 1);
                        let ps_nee = spec_prob(f0, kd, n_nee.dot(v).max(0.0001));
                        let f = bsdf_eval(n_nee, v, ld, kd, f0, alpha);
                        let pb = bsdf_pdf(n_nee, v, ld, alpha, ps_nee);
                        let wgt = if p.env_grid_dim > 0 {
                            1.0
                        } else {
                            env.pdf * env.pdf / (env.pdf * env.pdf + pb * pb)
                        };
                        let corr = shading_normal_correction(ng, n_nee, v, ld);
                        lsum = lsum + tp * f * env.radiance * vis * (nl * corr / env.pdf * wgt);
                    }
                }
            }
            if p.n_lights > 0 && !self.brute {
                let mut lo = 0usize;
                let mut hi = p.n_lights;
                for _ in 0..24 {
                    if lo >= hi {
                        break;
                    }
                    let mid = (lo + hi) / 2;
                    if r_misc.0 < p.light.data[mid * 4 + 3] {
                        hi = mid;
                    } else {
                        lo = mid + 1;
                    }
                }
                let li = lo.min(p.n_lights - 1);
                let lrec = &p.light.data[li * 4..li * 4 + 4];
                let lt = &p.accel.tris[lrec[0] as usize];
                let su = r_light.0.sqrt();
                let bw1 = su * (1.0 - r_light.1);
                let bw2 = su * r_light.1;
                let lp = lt.v0 * (1.0 - su) + lt.v1 * bw1 + lt.v2 * bw2;
                let lnrm = unit(Vec3f::cross(lt.v1 - lt.v0, lt.v2 - lt.v0));
                let nominal = unit(lp - pt);
                let origin = offset_ray(pt, ng, nominal, p.scene_scale);
                let mut ld = lp - origin;
                let dist = ld.length();
                ld = ld / dist;
                let n_nee = if ng.dot(ld) > 0.0 && ns.dot(ld) <= 0.0 { ng } else { ns };
                let nl = n_nee.dot(ld);
                let lm_ref = &self.materials[p.tri_material[lrec[0] as usize] as usize];
                let cosl = if lm_ref.two_sided { lnrm.dot(-ld).abs() } else { lnrm.dot(-ld).max(0.0) };
                if nl > 0.0 && ng.dot(ld) > 0.0 && cosl > 0.0001 && dist > ray_error(pt, p.scene_scale) + ray_error(lp, p.scene_scale) {
                    let Some(vis) = self.shadow(pt, ng, ld, dist - ray_error(lp, p.scene_scale), ti as i32) else { return None };
                    let vis = vis.transmittance;
                    if vis.x + vis.y + vis.z > 0.0 {
                        self.bump_stats(|stats| stats.emissive_nee_successes += 1);
                        let lm = v3(lm_ref.emission);
                        let pl = dist * dist * lrec[2] / (cosl * lrec[1]).max(p.scene_area * 2.5e-13);
                        let ps_nee = spec_prob(f0, kd, n_nee.dot(v).max(0.0001));
                        let f = bsdf_eval(n_nee, v, ld, kd, f0, alpha);
                        let pb = bsdf_pdf(n_nee, v, ld, alpha, ps_nee);
                        let wgt = pl * pl / (pl * pl + pb * pb);
                        let corr = shading_normal_correction(ng, n_nee, v, ld);
                        lsum = lsum + tp * f * lm * vis * (nl * corr / pl * wgt);
                    }
                }
            }
            let mut sample_n = ns;
            let mut sampled_diffuse = rx >= ps;
            let mut ld = if !sampled_diffuse {
                let a2 = alpha * alpha;
                let ct = ((1.0 - r_bsdf.0) / (1.0 + (a2 - 1.0) * r_bsdf.0)).sqrt();
                let st = (1.0 - ct * ct).max(0.0).sqrt();
                let phi = std::f32::consts::TAU * r_bsdf.1;
                let hv = to_world(sample_n, vec3f(st * phi.cos(), st * phi.sin(), ct));
                hv * (2.0 * v.dot(hv)) - v
            } else {
                let rr = r_bsdf.0.sqrt();
                let phi = std::f32::consts::TAU * r_bsdf.1;
                to_world(sample_n, vec3f(rr * phi.cos(), rr * phi.sin(), (1.0 - r_bsdf.0).max(0.0).sqrt()))
            };
            // A bump/smooth normal must never reflect below the geometric
            // surface. Fall back to the geometric frame with the same sample.
            if ng.dot(ld) <= 0.0 {
                sample_n = ng;
                ps = spec_prob(f0, kd, sample_n.dot(v).max(0.0001));
                sampled_diffuse = rx >= ps;
                ld = if !sampled_diffuse {
                    let a2 = alpha * alpha;
                    let ct = ((1.0 - r_bsdf.0) / (1.0 + (a2 - 1.0) * r_bsdf.0)).sqrt();
                    let st = (1.0 - ct * ct).max(0.0).sqrt();
                    let phi = std::f32::consts::TAU * r_bsdf.1;
                    let hv = to_world(sample_n, vec3f(st * phi.cos(), st * phi.sin(), ct));
                    hv * (2.0 * v.dot(hv)) - v
                } else {
                    let rr = r_bsdf.0.sqrt();
                    let phi = std::f32::consts::TAU * r_bsdf.1;
                    to_world(sample_n, vec3f(rr * phi.cos(), rr * phi.sin(), (1.0 - r_bsdf.0).max(0.0).sqrt()))
                };
            }
            if sampled_diffuse {
                ndiff += 1.0;
            }
            let nl = sample_n.dot(ld);
            if nl <= 0.0 || ng.dot(ld) <= 0.0 {
                break;
            }
            let f = bsdf_eval(sample_n, v, ld, kd, f0, alpha);
            let pb = bsdf_pdf(sample_n, v, ld, alpha, ps);
            if pb <= 0.0 {
                break;
            }
            let corr = shading_normal_correction(ng, sample_n, v, ld);
            tp = tp * f * (nl * corr / pb);
            prev_pdf = pb;
            delta = false;
            if ndiff > self.max_diffuse as f32 {
                break;
            }
            if b >= 2 {
                let q = tp.x.max(tp.y).max(tp.z).min(0.95);
                if r_misc.1 >= q {
                    break;
                }
                tp = tp / q;
            }
            rd = ld;
            ro = pt;
            hit = self.secondary_trace(pt, ng, rd, p.ray_tmax(), ti as i32);
        }
        if let Some(limit) = self.preview_clamp {
            let peak = lsum.x.max(lsum.y).max(lsum.z);
            if peak > limit {
                lsum = lsum * (limit / peak);
            }
        }
        if !lsum.x.is_finite()
            || !lsum.y.is_finite()
            || !lsum.z.is_finite()
            || lsum.x.abs() >= 1.0e30
            || lsum.y.abs() >= 1.0e30
            || lsum.z.abs() >= 1.0e30
        {
            return None;
        }
        Some(lsum)
    }

    pub fn radiance(&self, px: u32, py: u32, w: u32, h: u32, seed: u32, sidx: u32) -> Vec3f {
        self.radiance_checked(px, py, w, h, seed, sidx).unwrap_or_else(|| {
            self.invalid_samples.set(self.invalid_samples.get() + 1);
            Vec3f::default()
        })
    }

    /// The primary hit the hybrid G-buffer path uses: a pixel-CENTRE ray
    /// offset by the frame's jitter (`gpu::frame_jitter`). Returns
    /// (tri, u, v) or tri = -1.
    pub fn primary_hit(&self, px: u32, py: u32, w: u32, h: u32, jitter: (f32, f32)) -> Vec3f {
        let (right, up, fwd) = self.camera.basis();
        let aspect = w as f32 / h as f32;
        let tan_y = (self.camera.fov_y * 0.5).tan();
        let inv = vec2f(1.0 / w as f32, 1.0 / h as f32);
        let ndc = vec2f((px as f32 + 0.5 + jitter.0) * inv.x * 2.0 - 1.0, 1.0 - (py as f32 + 0.5 + jitter.1) * inv.y * 2.0);
        let mut ro = self.camera.pos - self.packed.origin;
        let mut rd = unit(fwd + right * (ndc.x * (tan_y * aspect)) + up * (ndc.y * tan_y));
        if let Some(hh) = self.camera.ortho_height {
            ro = ro + right * (ndc.x * (hh * 0.5 * aspect)) + up * (ndc.y * (hh * 0.5));
            rd = fwd;
        }
        let hit = self.packed.accel.trace(ro, rd, self.packed.ray_tmax(), false);
        vec3f(if hit.truncated { -2.0 } else { hit.tri as f32 }, hit.u, hit.v)
    }

    /// Diagnostics: the per-sample jittered primary hit (exactly the ray
    /// `radiance` would shoot for `sidx`) plus the sun-shadow visibility from
    /// that hit — `(tri, t, mean_transmittance)`, tri = -1 on a miss and
    /// visibility -2 when the surface faces away from `sun_dir`. Lets a
    /// harness histogram which surface each sample of one pixel lands on and
    /// whether its next-event sun ray alternates between lit and blocked.
    pub fn probe_primary_and_sun(
        &self,
        px: u32,
        py: u32,
        w: u32,
        h: u32,
        seed: u32,
        sidx: u32,
        sun_dir: Vec3f,
    ) -> (i32, f32, f32) {
        let pseed = pixel_seed(px, py, seed);
        let (right, up, fwd) = self.camera.basis();
        let aspect = w as f32 / h as f32;
        let tan_y = (self.camera.fov_y * 0.5).tan();
        let inv = vec2f(1.0 / w as f32, 1.0 / h as f32);
        let (jx, jy) = self.sample2(pseed, sidx, 0);
        let mut ro = self.camera.pos - self.packed.origin;
        let ndc = vec2f((px as f32 + jx) * inv.x * 2.0 - 1.0, 1.0 - (py as f32 + jy) * inv.y * 2.0);
        let mut rd = unit(fwd + right * (ndc.x * (tan_y * aspect)) + up * (ndc.y * tan_y));
        if let Some(hh) = self.camera.ortho_height {
            ro = ro + right * (ndc.x * (hh * 0.5 * aspect)) + up * (ndc.y * (hh * 0.5));
            rd = fwd;
        }
        let hit = self.trace(ro, rd, self.packed.ray_tmax(), false, 0.0, -1);
        if !hit.is_hit() {
            return (-1, 0.0, 0.0);
        }
        let tri = &self.packed.accel.tris[hit.tri as usize];
        let w0 = 1.0 - hit.u - hit.v;
        let p = tri.v0 * w0 + tri.v1 * hit.u + tri.v2 * hit.v;
        let mut ng = geom_ng(tri);
        if ng.dot(rd) >= 0.0 {
            ng = -ng;
        }
        let vis = if ng.dot(sun_dir) <= 0.0 {
            -2.0
        } else {
            self.shadow(p, ng, sun_dir, self.packed.ray_tmax(), hit.tri)
                .map(|sh| (sh.transmittance.x + sh.transmittance.y + sh.transmittance.z) / 3.0)
                .unwrap_or(0.0)
        };
        (hit.tri, hit.t, vis)
    }

    /// Diagnostics: distance to the first blocker along `dir` from the
    /// per-sample primary hit of pixel (px, py) — `f32::INFINITY` when the
    /// ray escapes, negative when the primary ray misses.
    pub fn probe_blocker_distance(
        &self,
        px: u32,
        py: u32,
        w: u32,
        h: u32,
        seed: u32,
        sidx: u32,
        dir: Vec3f,
    ) -> f32 {
        let pseed = pixel_seed(px, py, seed);
        let (right, up, fwd) = self.camera.basis();
        let aspect = w as f32 / h as f32;
        let tan_y = (self.camera.fov_y * 0.5).tan();
        let inv = vec2f(1.0 / w as f32, 1.0 / h as f32);
        let (jx, jy) = self.sample2(pseed, sidx, 0);
        let ro = self.camera.pos - self.packed.origin;
        let ndc = vec2f((px as f32 + jx) * inv.x * 2.0 - 1.0, 1.0 - (py as f32 + jy) * inv.y * 2.0);
        let rd = unit(fwd + right * (ndc.x * (tan_y * aspect)) + up * (ndc.y * tan_y));
        let hit = self.trace(ro, rd, self.packed.ray_tmax(), false, 0.0, -1);
        if !hit.is_hit() {
            return -1.0;
        }
        let tri = &self.packed.accel.tris[hit.tri as usize];
        let w0 = 1.0 - hit.u - hit.v;
        let p = tri.v0 * w0 + tri.v1 * hit.u + tri.v2 * hit.v;
        let mut ng = geom_ng(tri);
        if ng.dot(rd) >= 0.0 {
            ng = -ng;
        }
        let b = self.secondary_trace(p, ng, dir, self.packed.ray_tmax(), hit.tri);
        if b.is_hit() {
            b.t
        } else {
            f32::INFINITY
        }
    }

    /// Mean radiance per pixel after `spp` samples. Row-major, RGB.
    pub fn render(&self, w: u32, h: u32, spp: u32, seed: u32) -> Vec<[f32; 3]> {
        let mut out = vec![[0.0f32; 3]; (w * h) as usize];
        for py in 0..h {
            for px in 0..w {
                let mut sum = vec3f(0.0, 0.0, 0.0);
                let mut valid = 0u32;
                for s in 0..spp {
                    if let Some(mut l) = self.radiance_checked(px, py, w, h, seed, s) {
                        // Preview-only relative clamp (GPU twin agrees):
                        // once a pixel has a mean, no single sample may
                        // exceed five times it. Off (with preview_clamp)
                        // for final/parity renders.
                        if self.preview_clamp.is_some() && valid >= 8 {
                            let mean_peak = (sum.x.max(sum.y).max(sum.z)) / valid as f32;
                            let limit = (mean_peak * 5.0).max(0.05);
                            let pk = l.x.max(l.y).max(l.z);
                            if pk > limit {
                                l = l * (limit / pk);
                            }
                        }
                        sum = sum + l;
                        valid += 1;
                    } else {
                        self.invalid_samples.set(self.invalid_samples.get() + 1);
                    }
                }
                let mean = if valid > 0 { sum / valid as f32 } else { Vec3f::default() };
                out[(py * w + px) as usize] = [mean.x, mean.y, mean.z];
            }
        }
        out
    }
}

/// Convenience: a CPU tracer over a scene with the scene's own sky.
pub fn cpu_tracer<'a>(scene: &'a SceneInput, packed: &'a PackedScene) -> CpuTracer<'a> {
    CpuTracer {
        packed,
        materials: &scene.materials,
        sky: crate::sky::sky_uniforms(&scene.sun, scene.up),
        camera: scene.camera.clone(),
        max_bounces: 8,
        max_diffuse: 4,
        preview_clamp: None,
        brute: false,
        probe_bounce: -1,
        white_noise: false,
        shadow_skin: packed.auto_shadow_skin(),
        rays: std::cell::Cell::new(0),
        invalid_samples: std::cell::Cell::new(0),
        stats: std::cell::Cell::new(CpuStats::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaled_scene(scene: &SceneInput, scale: f32) -> SceneInput {
        let mut scene = scene.clone();
        for position in &mut scene.positions {
            for component in position {
                *component *= scale;
            }
        }
        scene.camera.pos = scene.camera.pos * scale;
        scene.camera.target = scene.camera.target * scale;
        scene.camera.focus_dist *= scale;
        scene.camera.ortho_height = scene.camera.ortho_height.map(|height| height * scale);
        // The aperture is a camera length (scene units): it scales with the camera.
        scene.camera.bokeh_scale *= scale;
        scene
    }

    #[test]
    fn transport_is_invariant_under_scene_scaling() {
        let mut scene = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        scene.materials = vec![
            Material::diffuse([0.7, 0.7, 0.7]),
            Material::diffuse([0.7, 0.1, 0.1]),
            Material::diffuse([0.1, 0.7, 0.1]),
            Material::emissive([1.0, 0.9, 0.8], 2.0),
        ];
        // Sunlit floor, shadow-casting box, and two inward-facing walls
        // meeting the floor in a concave corner.
        scene.push_quad([[-50.0, 0.0, -50.0], [-50.0, 0.0, 50.0], [50.0, 0.0, 50.0], [50.0, 0.0, -50.0]], 0);
        scene.tri_coplanar_group.fill(1);
        scene.tri_priority.fill(1);
        crate::scene::push_box(&mut scene, vec3f(-15.0, 0.0, 0.0), vec3f(16.0, 20.0, 20.0), 0.0, 0);
        scene.push_quad([[-40.0, 0.0, -40.0], [40.0, 0.0, -40.0], [40.0, 30.0, -40.0], [-40.0, 30.0, -40.0]], 0);
        scene.push_quad([[-40.0, 0.0, 40.0], [-40.0, 0.0, -40.0], [-40.0, 30.0, -40.0], [-40.0, 30.0, 40.0]], 0);
        // Priority must select the farther green decal inside the tie band,
        // even at scale 100 where the old fixed window missed this pair.
        for (y, material, priority) in [(0.0, 1, 2), (-0.0005, 2, 9)] {
            let first = scene.tri_count();
            scene.push_quad([[10.0, y, -5.0], [10.0, y, 20.0], [35.0, y, 20.0], [35.0, y, -5.0]], material);
            scene.tri_coplanar_group[first..].fill(1);
            scene.tri_priority[first..].fill(priority);
        }
        // Also exercise area-valued emissive MIS guards. One light triangle
        // keeps its random sample independent of BVH triangle reordering.
        scene.push_mesh(
            &[[15.0, 25.0, -25.0], [25.0, 25.0, -25.0], [25.0, 25.0, -15.0]],
            None, None, &[0, 1, 2], 3,
        );
        scene.ensure_normals();
        scene.camera = Camera {
            pos: vec3f(85.0, 75.0, 110.0),
            target: vec3f(0.0, 3.0, 0.0),
            focus_dist: vec3f(85.0, 72.0, 110.0).length(),
            f_stop: 2.0,
            bokeh_scale: 8.0,
            ..Default::default()
        };
        scene.sun.dir = vec3f(0.6, 0.8, 0.2).normalize();
        let mut reference: Option<Vec<[f32; 3]>> = None;
        let mut reference_scale = 0.0;
        for scale in [1.0, 0.01, 100.0] {
            let scene = scaled_scene(&scene, scale);
            let packed = PackedScene::pack(&scene);
            assert_eq!(packed.scene_scale, packed.accel.scene_scale, "BVH must use the packed scale");
            if scale == 1.0 {
                reference_scale = packed.scene_scale;
            }
            assert!((packed.scene_scale / scale / reference_scale - 1.0).abs() < 2.0e-6);
            let mut tracer = cpu_tracer(&scene, &packed);
            tracer.max_bounces = 4;
            tracer.max_diffuse = 3;
            let decal = packed.accel.trace(
                vec3f(20.0, 10.0, 5.0) * scale - packed.origin,
                vec3f(0.0, -1.0, 0.0), packed.ray_tmax(), false,
            );
            assert!(decal.is_hit() && !decal.truncated);
            assert_eq!(packed.tri_material[decal.tri as usize], 2, "farther decal priority at scale {scale}");
            let up = vec3f(0.0, 1.0, 0.0);
            for (point, expected) in [(vec3f(-25.0, 0.0, -9.0), 0.0), (vec3f(40.0, 0.0, 30.0), 1.0)] {
                let visibility = tracer.shadow(point * scale - packed.origin, up, scene.sun.dir, packed.ray_tmax(), -1).unwrap();
                assert_eq!(visibility.transmittance.x, expected, "floor shadow at scale {scale}");
            }
            let image = tracer.render(16, 12, 32, 73);
            assert_eq!(tracer.invalid_samples.get(), 0, "no rejected samples at scale {scale}");
            assert!(tracer.stats().sun_nee_successes > 0);
            if let Some(reference) = &reference {
                let mut error2 = 0.0f64;
                let mut signal2 = 0.0f64;
                let mut worst = 0.0f32;
                let mut peak = 0.0f32;
                for (actual, expected) in image.iter().zip(reference) {
                    for channel in 0..3 {
                        let error = (actual[channel] - expected[channel]).abs();
                        error2 += (error as f64).powi(2);
                        signal2 += (expected[channel] as f64).powi(2);
                        worst = worst.max(error);
                        peak = peak.max(expected[channel]);
                    }
                }
                assert!(signal2 > 0.0 && peak > 0.0);
                // Common random numbers cancel Monte Carlo noise. Allow
                // 0.2% relative RMS / 2% peak for f32 path-edge decisions,
                // much less than independent 32-spp sampling noise.
                assert!((error2 / signal2).sqrt() <= 0.002, "relative RMS at scale {scale}: {}", (error2 / signal2).sqrt());
                assert!(worst <= peak * 0.02, "worst channel at scale {scale}: {worst}, peak {peak}");
            } else {
                reference = Some(image);
            }
        }
    }

    #[test]
    fn micron_step_is_lit_without_acne_and_keeps_its_wall_shadow() {
        // A 100 mm diagonal plate and a 35 micron raised pad. Only direct
        // sun is enabled so visibility cannot hide behind indirect light.
        let mut scene = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        scene.materials = vec![Material::diffuse([0.7; 3])];
        let half = 0.05 / 2.0f32.sqrt();
        scene.push_quad([[-half, 0.0, -half], [-half, 0.0, half], [half, 0.0, half], [half, 0.0, -half]], 0);
        crate::scene::push_box(&mut scene, Vec3f::default(), vec3f(0.02, 35.0e-6, 0.02), 0.0, 0);
        scene.ensure_normals();
        scene.camera = Camera {
            pos: vec3f(0.0, 0.05, 0.0),
            target: Vec3f::default(),
            up: vec3f(0.0, 0.0, -1.0),
            f_stop: 0.0,
            focus_dist: 0.05,
            ortho_height: Some(0.06),
            ..Default::default()
        };
        scene.sun = crate::scene::Sun { dir: vec3f(-1.0, 0.5, 0.0).normalize(), sky_strength: 0.0, ..Default::default() };
        let packed = PackedScene::pack(&scene);
        assert!((packed.scene_scale - 0.1).abs() < 1.0e-6);
        let mut tracer = cpu_tracer(&scene, &packed);
        tracer.max_bounces = 1;
        let up = vec3f(0.0, 1.0, 0.0);
        let expected = bsdf_eval(up, up, scene.sun.dir, vec3f(0.7, 0.7, 0.7), vec3f(0.04, 0.04, 0.04), 1.0)
            * tracer.sun_irradiance() * up.dot(scene.sun.dir);
        assert!(expected.x > 0.0 && expected.y > 0.0 && expected.z > 0.0);
        let image = tracer.render(12, 12, 32, 91);
        for y in 0..12 {
            for x in 0..12 {
                // Exclude only the four cells straddling the physical shadow
                // immediately to the pad's right; inspect that band below.
                if x == 8 && (4..8).contains(&y) { continue; }
                let pixel = image[y * 12 + x];
                for (actual, expected) in pixel.into_iter().zip([expected.x, expected.y, expected.z]) {
                    // The full solar disc changes cos(theta) by <1% at this
                    // incidence. 2% also covers the smooth GGX variation;
                    // this bounds every sample, not just the image average.
                    assert!((actual / expected - 1.0).abs() <= 0.02, "acne at ({x},{y}): {pixel:?}, reference {expected}");
                }
            }
        }
        // Resolve the umbra 36..44 microns beside the wall with a 4x4 crop.
        // Rays reach the wall below its top and beyond the 30 micron skin;
        // the former fixed 2 mm minimum incorrectly skipped this blocker.
        tracer.camera.pos = vec3f(0.01004, 0.05, 0.0);
        tracer.camera.target = vec3f(0.01004, 0.0, 0.0);
        tracer.camera.ortho_height = Some(8.0e-6);
        for (pixel, value) in tracer.render(4, 4, 32, 91).iter().enumerate() {
            let hit = tracer.primary_hit((pixel % 4) as u32, (pixel / 4) as u32, 4, 4, (0.0, 0.0));
            assert!(hit.x >= 0.0, "shadow crop must hit the plate, not the black sky");
            let tri = &packed.accel.tris[hit.x as usize];
            let point = tri.v0 * (1.0 - hit.y - hit.z) + tri.v1 * hit.y + tri.v2 * hit.z + packed.origin;
            assert!(point.y.abs() <= packed.scene_scale * 1.0e-7 && geom_ng(tri).dot(up) > 0.99, "shadow crop must see the plate top");
            assert!(value.iter().all(|v| v.abs() <= expected.x * 1.0e-6), "light leak beside the step at pixel {pixel}: {value:?}");
        }
        assert_eq!(tracer.invalid_samples.get(), 0, "dark pixels must not be rejected samples");
    }

    #[test]
    fn environment_importance_pdf_normalizes_and_sun_samples_the_disc() {
        let scene = SceneInput {
            up: vec3f(0.0, 1.0, 0.0),
            ..Default::default()
        };
        let packed = PackedScene::pack(&scene);
        let tracer = cpu_tracer(&scene, &packed);
        let mut integral = 0.0f32;
        const SAMPLES: usize = 20_000;
        for sample in 0..SAMPLES {
            let z = (sample as f32 + 0.5) / SAMPLES as f32;
            let direction = vec3f((1.0 - z * z).sqrt(), z, 0.0);
            integral += tracer.sky_pdf(direction) * std::f32::consts::TAU / SAMPLES as f32;
        }
        assert!(
            (integral - 1.0).abs() < 0.01,
            "environment PDF integrates to {integral}"
        );
        for sample in 0..64 {
            let direction = tracer.sample_sun((
                (sample as f32 + 0.5) / 64.0,
                (sample as f32 * 0.618_034).fract(),
            ));
            assert!(direction.dot(tracer.sun_dir()) >= tracer.sky.sun_dir.w - 1.0e-6);
        }
    }

    #[test]
    fn heavy_minification_blends_the_albedo_to_the_texture_mean() {
        // A ground quad tiled 400x with a black/white texture: up close the
        // fetch keeps texel detail; once the pixel footprint spans texels
        // the albedo converges to the image's linear mean instead of being
        // a per-sample texel lottery (the roof moire / lawn shimmer).
        let mut s = SceneInput { up: vec3f(0.0, 0.0, 1.0), ..Default::default() };
        s.materials = vec![Material {
            albedo: [1.0, 1.0, 1.0],
            texture: Some(0),
            ..Default::default()
        }];
        s.images = vec![crate::scene::Image {
            width: 2,
            height: 1,
            data: vec![0xff00_0000, 0xffff_ffff],
        }];
        s.push_mesh(
            &[[0.0, 0.0, 0.0], [200.0, 0.0, 0.0], [0.0, 200.0, 0.0]],
            None,
            Some(&[[0.0, 0.0], [400.0, 0.0], [0.0, 400.0]]),
            &[0, 1, 2],
            0,
        );
        s.ensure_normals();
        let packed = PackedScene::pack(&s);
        let tracer = cpu_tracer(&s, &packed);
        // Near: texel detail survives (footprint far below a texel).
        let dark = tracer.atlas_albedo(0, 0.9, 0.0004, 0.05, vec3f(1.0, 1.0, 1.0), 0.0001);
        let light = tracer.atlas_albedo(0, 0.9, 0.0016, 0.05, vec3f(1.0, 1.0, 1.0), 0.0001);
        assert!(dark.x < 0.05 && light.x > 0.9, "near: {dark:?} vs {light:?}");
        // Far: both sample points give the linear mean (0.5).
        let a = tracer.atlas_albedo(0, 0.9, 0.0004, 0.05, vec3f(1.0, 1.0, 1.0), 10.0);
        let b = tracer.atlas_albedo(0, 0.9, 0.0016, 0.05, vec3f(1.0, 1.0, 1.0), 10.0);
        assert!((a.x - b.x).abs() < 1.0e-3, "far mismatch: {a:?} vs {b:?}");
        assert!((a.x - 0.5).abs() < 0.02, "far mean off: {a:?}");
    }

    fn emissive_quad(two_sided: bool, value: f32, camera_z: f32) -> SceneInput {
        let mut s = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        s.materials = vec![Material { emission: [value; 3], two_sided, ..Default::default() }];
        s.push_quad([[-2.0, -2.0, 0.0], [2.0, -2.0, 0.0], [2.0, 2.0, 0.0], [-2.0, 2.0, 0.0]], 0);
        s.ensure_normals();
        s.camera = Camera { pos: vec3f(0.0, 0.0, camera_z), target: Vec3f::default(), f_stop: 0.0, ..Default::default() };
        s.sun = crate::scene::Sun { sky_strength: 0.0, sun_strength: 0.0, ..Default::default() };
        s
    }

    #[test]
    fn one_sided_backface_is_invisible_to_the_camera_like_the_raster() {
        // The raster pane culls one-sided backfaces; the tracer used to paint
        // them black, which drew stripes and holes the raster never shows.
        // Now the camera ray passes through them (raster parity): the back
        // view of a one-sided quad sees the sky behind it, the front view
        // shades normally.
        let quad = |camera_z: f32| {
            let mut s = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
            s.materials = vec![Material::diffuse([0.5, 0.5, 0.5])];
            s.push_quad(
                [[-2.0, -2.0, 0.0], [2.0, -2.0, 0.0], [2.0, 2.0, 0.0], [-2.0, 2.0, 0.0]],
                0,
            );
            s.ensure_normals();
            s.camera = Camera {
                pos: vec3f(0.0, 0.0, camera_z),
                target: Vec3f::default(),
                f_stop: 0.0,
                ..Default::default()
            };
            s
        };
        let mean = |scene: &SceneInput| {
            let packed = PackedScene::pack(scene);
            let mut t = cpu_tracer(scene, &packed);
            t.sky = SkyUniforms::uniform_white(1.0);
            let mut sum = 0.0f32;
            for sidx in 0..64 {
                sum += t.radiance(0, 0, 1, 1, 1, sidx).x;
            }
            sum / 64.0
        };
        let front = mean(&quad(3.0));
        let back = mean(&quad(-3.0));
        assert!(front > 0.2 && front < 0.9, "front view must be lit surface: {front}");
        assert!(
            (back - 1.0).abs() < 0.02,
            "the camera must see through a one-sided backface to the sky: back {back}"
        );
    }

    #[test]
    fn bounce_rays_flip_shade_one_sided_backfaces_instead_of_culling() {
        // A one-sided canopy ABOVE a lit floor, wound so the floor's bounce
        // rays strike its BACK. If bounces culled it like primary rays do,
        // the floor would see the full sky through it (no darkening); the
        // opaque flip-shade keeps it a light blocker for GI. Direct camera
        // rays reach the floor from the side, under the canopy's edge.
        let floor_only = || {
            let mut s = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
            s.materials = vec![Material::diffuse([0.7, 0.7, 0.7])];
            s.push_quad(
                [[-4.0, 0.0, -4.0], [-4.0, 0.0, 4.0], [4.0, 0.0, 4.0], [4.0, 0.0, -4.0]],
                0,
            );
            s.ensure_normals();
            // Looking steeply down from under the canopy edge: every jittered
            // ray of the 1x1 frame lands on the floor beneath the canopy.
            s.camera = Camera {
                pos: vec3f(0.0, 0.5, 5.0),
                target: vec3f(0.0, 0.0, 1.5),
                f_stop: 0.0,
                ..Default::default()
            };
            s
        };
        let mean = |scene: &SceneInput| {
            let packed = PackedScene::pack(scene);
            let mut t = cpu_tracer(scene, &packed);
            t.sky = SkyUniforms::uniform_white(1.0);
            let mut sum = 0.0f32;
            for sidx in 0..96 {
                sum += t.radiance(0, 0, 1, 1, 1, sidx).x;
            }
            sum / 96.0
        };
        let open_sky = mean(&floor_only());
        let mut covered = floor_only();
        // Canopy at y=1 with its normal UP: the floor's upward bounce rays
        // hit its underside (a backface of a one-sided material).
        covered.push_quad(
            [[-4.0, 1.0, -4.0], [-4.0, 1.0, 4.0], [4.0, 1.0, 4.0], [4.0, 1.0, -4.0]],
            0,
        );
        covered.ensure_normals();
        let covered = mean(&covered);
        assert!(open_sky > 0.4, "open floor must be sky-lit: {open_sky}");
        assert!(
            covered < open_sky * 0.75,
            "a one-sided canopy's backface must still block GI at bounce time: open {open_sky} covered {covered}"
        );
        assert!(covered > 0.01, "the covered floor must not be black: {covered}");
    }

    #[test]
    fn shadow_skin_ignores_stacked_layers_but_keeps_real_casters() {
        // A construction sandwich: a 100-unit roof deck at z = 1.0 with a
        // covering layer 1.06e-4 diagonals up — the measured woodside regime
        // (blocker distances p50 = 1.2e-4, p99 = 3.25e-4 diagonals), past the
        // spurious-neighbour band that already absorbs closer back-faces.
        // Without the skin every NEE ray from the deck is blocked by the
        // covering (the roof noise); with the scene-scaled skin the stacked
        // layer is stepped past while a real caster farther up still
        // shadows.
        let quad = |z: f32| {
            [[-50.0, -50.0, z], [50.0, -50.0, z], [50.0, 50.0, z], [-50.0, 50.0, z]]
        };
        let mut s = SceneInput { up: vec3f(0.0, 0.0, 1.0), ..Default::default() };
        s.materials = vec![Material::default()];
        s.push_quad(quad(1.0), 0);
        s.push_quad(quad(1.015), 0);
        s.ensure_normals();
        let packed = PackedScene::pack(&s);
        let skin = packed.auto_shadow_skin();
        assert!(skin > 0.015, "skin must cover the stacked layer: {skin}");
        assert!((skin / packed.scene_scale - 3.0e-4).abs() < 1.0e-10, "skin must remain bounded by the scene-relative band");
        let tracer = cpu_tracer(&s, &packed);
        assert_eq!(tracer.shadow_skin, skin);
        // From the deck surface straight up: covering inside the skin.
        let p = vec3f(0.2, 0.3, 1.0) - packed.origin;
        let up = vec3f(0.0, 0.0, 1.0);
        let lit = tracer.shadow(p, up, up, packed.ray_tmax(), 0).expect("finite");
        assert_eq!(lit.transmittance, vec3f(1.0, 1.0, 1.0), "stacked layer must not shadow its own deck");
        // The unbiased reference (skin off) still sees the blocker.
        let mut reference = cpu_tracer(&s, &packed);
        reference.shadow_skin = 0.0;
        let dark = reference.shadow(p, up, up, packed.ray_tmax(), 0).expect("finite");
        assert_eq!(dark.transmittance, Vec3f::default());

        // A real caster beyond the skin shadows with the skin active.
        let mut with_caster = s.clone();
        with_caster.push_quad(quad(2.0), 0);
        with_caster.ensure_normals();
        let packed2 = PackedScene::pack(&with_caster);
        let tracer2 = cpu_tracer(&with_caster, &packed2);
        let p2 = vec3f(0.2, 0.3, 1.0) - packed2.origin;
        let shadowed = tracer2.shadow(p2, up, up, packed2.ray_tmax(), 0).expect("finite");
        assert_eq!(shadowed.transmittance, Vec3f::default(), "a separated caster is a real shadow");
    }

    #[test]
    fn relative_origin_offset_avoids_self_hit_without_skipping_neighbour() {
        use crate::bvh::{Bvh, Tri};
        let face = |z| Tri { v0: vec3f(-1.0, -1.0, z), v1: vec3f(1.0, -1.0, z), v2: vec3f(0.0, 1.0, z) };
        let bvh = Bvh::build(&[face(0.0), face(0.01)]);
        let p = vec3f(0.0, 0.0, 0.0);
        let ro = offset_ray(p, vec3f(0.0, 0.0, 1.0), vec3f(0.0, 0.0, 1.0), bvh.scene_scale);
        let h = bvh.trace_from(ro, vec3f(0.0, 0.0, 1.0), 0.0, 1.0, false);
        assert!(h.is_hit());
        assert_eq!(bvh.tri_order[h.tri as usize], 1, "self face must be behind the offset");
        assert!((h.t - 0.01).abs() < 2.0e-5, "nearby face was over-offset: {h:?}");
    }

    #[test]
    fn reference_is_unclamped_and_invalid_samples_are_counted() {
        let scene = emissive_quad(false, 100.0, 3.0);
        let packed = PackedScene::pack(&scene);
        let mut t = cpu_tracer(&scene, &packed);
        t.sky = SkyUniforms::uniform_white(0.0);
        let reference = t.radiance(0, 0, 1, 1, 3, 0);
        assert!(reference.x > 90.0, "default estimator clipped {reference:?}");
        t.preview_clamp = Some(12.0);
        let preview = t.radiance(0, 0, 1, 1, 3, 0);
        assert!((preview.x - 12.0).abs() < 1.0e-5, "preview clamp {preview:?}");

        let bad = emissive_quad(false, f32::INFINITY, 3.0);
        let packed_bad = PackedScene::pack(&bad);
        let bad_t = cpu_tracer(&bad, &packed_bad);
        let image = bad_t.render(1, 1, 1, 9);
        assert_eq!(image[0], [0.0; 3]);
        assert_eq!(bad_t.invalid_samples.get(), 1);
    }

    #[test]
    fn material_sidedness_prevents_opaque_backface_double_lighting() {
        let front = emissive_quad(false, 5.0, 3.0);
        let pf = PackedScene::pack(&front);
        assert!(cpu_tracer(&front, &pf).radiance(0, 0, 1, 1, 1, 0).x > 4.9);

        let back = emissive_quad(false, 5.0, -3.0);
        let pb = PackedScene::pack(&back);
        assert_eq!(cpu_tracer(&back, &pb).radiance(0, 0, 1, 1, 1, 0).x, 0.0);

        let sheet = emissive_quad(true, 5.0, -3.0);
        let ps = PackedScene::pack(&sheet);
        assert!(cpu_tracer(&sheet, &ps).radiance(0, 0, 1, 1, 1, 0).x > 4.9);
    }

    #[test]
    fn cpu_transport_samples_atlas_texels() {
        let mut s = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        s.images.push(crate::scene::Image { width: 1, height: 1, data: vec![0xffff_0000] });
        s.materials.push(Material { texture: Some(0), roughness: 1.0, ..Default::default() });
        let pos = [[-2.0, -2.0, 0.0], [2.0, -2.0, 0.0], [2.0, 2.0, 0.0], [-2.0, 2.0, 0.0]];
        let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        s.push_mesh(&pos, None, Some(&uv), &[0, 1, 2, 0, 2, 3], 0);
        s.ensure_normals();
        s.camera = Camera { pos: vec3f(0.0, 0.0, 3.0), target: Vec3f::default(), f_stop: 0.0, ..Default::default() };
        let packed = PackedScene::pack(&s);
        let mut t = cpu_tracer(&s, &packed);
        t.sky = SkyUniforms::uniform_white(1.0);
        let c = t.render(1, 1, 512, 5)[0];
        assert!(c[0] > 0.7 && c[1] < 0.15 && c[2] < 0.15, "textured transport {c:?}");
    }

    #[test]
    fn furnace_converges_to_the_environment() {
        // A white Lambertian sphere under a uniform unit sky: every sphere
        // pixel's expected radiance is exactly 1 (energy conservation of
        // the diffuse lobe + cosine sampling + MIS-free sky).
        let scene = SceneInput::furnace();
        let packed = PackedScene::pack(&scene);
        let mut t = cpu_tracer(&scene, &packed);
        t.sky = SkyUniforms::uniform_white(1.0);
        t.max_bounces = 16;
        t.max_diffuse = 16;
        t.preview_clamp = None;
        let (w, h) = (16, 16);
        let img = t.render(w, h, 256, 3);
        let (mut sum, mut n) = (0.0, 0);
        for py in 4..12 {
            for px in 4..12 {
                let p = img[(py * w + px) as usize];
                sum += (p[0] + p[1] + p[2]) / 3.0;
                n += 1;
            }
        }
        let mean = sum / n as f32;
        assert!((mean - 1.0).abs() < 0.03, "furnace mean {mean}");
    }

    /// Bokeh sanity on the CPU: an emissive dot at the focus distance is a
    /// few pixels; with the focus elsewhere it is a disc many times larger,
    /// carrying the same energy.
    #[test]
    fn thin_lens_blurs_off_focus_points_into_a_disc() {
        fn scene(focus: f32) -> SceneInput {
            let mut s = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
            s.materials = vec![Material::emissive([1.0, 0.9, 0.8], 200.0)];
            crate::scene::push_sphere(&mut s, vec3f(0.0, 0.0, 0.0), 0.02, 12, 0);
            s.ensure_normals();
            s.camera = Camera {
                pos: vec3f(0.0, 0.0, 5.0),
                target: vec3f(0.0, 0.0, 0.0),
                fov_y: 20.0f32.to_radians(),
                focal_mm: 50.0,
                f_stop: 2.0,
                bokeh_scale: 8.0,
                focus_dist: focus,
                ..Default::default()
            };
            s.sun = crate::scene::Sun { sky_strength: 0.0, sun_strength: 0.0, ..Default::default() };
            s
        }
        let count = |focus: f32| -> (usize, f64) {
            let sc = scene(focus);
            let packed = PackedScene::pack(&sc);
            let mut t = cpu_tracer(&sc, &packed);
            t.sky = SkyUniforms::uniform_white(0.0);
            t.preview_clamp = None;
            let (w, h) = (96u32, 96u32);
            let img = t.render(w, h, 16, 11);
            let bright = img.iter().filter(|p| p[0] + p[1] + p[2] > 0.3).count();
            let energy: f64 = img.iter().map(|p| (p[0] + p[1] + p[2]) as f64).sum();
            (bright, energy)
        };
        let (sharp, e_sharp) = count(5.0);
        let (blur, e_blur) = count(2.5);
        assert!(sharp > 0 && sharp < 40, "sharp dot covers {sharp} px");
        assert!(blur > sharp * 4, "blur disc {blur} px vs sharp {sharp}");
        assert!((e_blur - e_sharp).abs() / e_sharp < 0.3, "energy {e_blur} vs {e_sharp}");
    }

    /// Closed-form lateral displacement measured on a receiver plane.
    #[test]
    fn solid_slab_lateral_shift_matches_snell() {
        let thickness = 0.003f32;
        let mut scene = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        scene.materials = vec![
            Material { solid: true, ..Material::glass([1.0; 3]) },
            Material::diffuse([1.0; 3]),
        ];
        crate::scene::push_box(&mut scene, vec3f(0.0, -0.01, 0.0), vec3f(0.02, 0.02, thickness), 0.0, 0);
        let plane_z = -thickness * 0.5 - 0.001;
        scene.push_quad([[-0.012, -0.012, plane_z], [0.012, -0.012, plane_z], [0.012, 0.012, plane_z], [-0.012, 0.012, plane_z]], 1);
        let packed = PackedScene::pack(&scene);
        let tracer = cpu_tracer(&scene, &packed);
        for degrees in [20.0f32, 45.0, 70.0] {
            let theta = degrees.to_radians();
            let initial = vec3f(theta.sin(), 0.0, -theta.cos());
            let origin = vec3f(-0.002 * theta.tan(), 0.001, thickness * 0.5 + 0.002);
            let mut direction = initial;
            let mut hit = tracer.trace(origin - packed.origin, direction, packed.ray_tmax(), false, 0.0, -1);
            for interface in 0..2 {
                assert!(hit.is_hit() && !hit.truncated, "missing interface {interface} at {degrees}");
                assert_eq!(packed.tri_material[hit.tri as usize], 0);
                let tri = &packed.accel.tris[hit.tri as usize];
                let ng = geom_ng(tri);
                let point = tri.v0 * (1.0 - hit.u - hit.v) + tri.v1 * hit.u + tri.v2 * hit.v;
                // Deterministic transmitted primary branch, using the same
                // interface and spawn functions as radiance_checked.
                let (next, reflected) = solid_interface(direction, ng, 1.5, 0.99999994);
                assert!(!reflected, "unexpected reflection at interface {interface}, {degrees}");
                direction = next;
                hit = tracer.secondary_trace(point, ng, direction, packed.ray_tmax(), hit.tri);
            }
            assert!(hit.is_hit() && !hit.truncated);
            assert_eq!(packed.tri_material[hit.tri as usize], 1, "ray must land on the rear plane");
            assert!((direction - initial).length() < 1.0e-6, "parallel slab restores direction");
            let tri = &packed.accel.tris[hit.tri as usize];
            let point = tri.v0 * (1.0 - hit.u - hit.v) + tri.v1 * hit.u + tri.v2 * hit.v + packed.origin;
            let unobstructed_x = origin.x + (origin.z - plane_z) * theta.tan();
            // Lateral shift is perpendicular to the incident ray, not the
            // larger x displacement measured on the z-constant receiver.
            let shift = (unobstructed_x - point.x) * theta.cos();
            let theta_t = (theta.sin() / 1.5).asin();
            let expected = thickness * (theta - theta_t).sin() / theta_t.cos();
            assert!((shift / expected - 1.0).abs() < 1.0e-4, "{degrees} degrees: {shift} vs {expected}");
        }
    }

    #[test]
    fn solid_total_internal_reflection_and_inside_camera() {
        let mut scene = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        // Deliberately not two_sided: inside-solid primary rays still see exits.
        scene.materials = vec![Material { solid: true, transmission: 1.0, roughness: 0.9, ..Default::default() }];
        crate::scene::push_box(&mut scene, vec3f(0.0, -2.0, 0.0), vec3f(8.0, 4.0, 2.0), 0.0, 0);
        let packed = PackedScene::pack(&scene);
        let mut tracer = cpu_tracer(&scene, &packed);
        let theta = 60.0f32.to_radians();
        assert!(theta > (1.0f32 / 1.5).asin());
        let direction = vec3f(theta.sin(), 0.0, theta.cos());
        let hit = tracer.trace(-packed.origin, direction, packed.ray_tmax(), false, 0.0, -1);
        assert!(hit.is_hit() && !hit.truncated);
        let ng = geom_ng(&packed.accel.tris[hit.tri as usize]);
        assert!(ng.dot(direction) > 0.0, "primary arrives from inside");
        assert_eq!(solid_fresnel(ng.dot(direction), 1.5), 1.0);
        for sample in [0.0, 0.5, 0.99999994, 1.0] {
            let (next, reflected) = solid_interface(direction, ng, 1.5, sample);
            assert!(reflected && next.dot(ng) < 0.0, "TIR must reflect every sample");
            assert!((next.length() - 1.0).abs() < 1.0e-6);
        }
        tracer.camera = Camera {
            pos: Vec3f::default(), target: direction, f_stop: 0.0,
            ortho_height: Some(1.0e-5), ..Default::default()
        };
        tracer.sky = SkyUniforms::uniform_white(1.0);
        tracer.probe_bounce = 1;
        let probe = tracer.radiance(0, 0, 1, 1, 17, 0);
        assert!(probe.x >= 1000.0, "reflected primary must still hit the closed box: {probe:?}");
        assert_eq!(tracer.stats().primary_backface_skips, 0, "solid exits are never primary-culled");
        let shadow = tracer.shadow(-packed.origin, ng, direction, packed.ray_tmax(), -1).unwrap();
        assert_eq!(shadow.transmittance, Vec3f::default(), "straight NEE also respects TIR");
    }

    #[test]
    fn solid_absorption_matches_thickness_and_inside_segments() {
        let absorption = [0.25f32, 0.5, 0.8];
        for thickness in [1.0f32, 2.0, 4.0] {
            let mut scene = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
            scene.materials = vec![Material {
                solid: true, absorption, transmission: 1.0, albedo: [0.01; 3],
                ..Default::default()
            }];
            crate::scene::push_box(&mut scene, vec3f(0.0, -0.2, 0.0), vec3f(0.4, 0.4, thickness), 0.0, 0);
            let packed = PackedScene::pack(&scene);
            // Also guard the five-texel material layout/default-independent tint.
            assert_eq!(&packed.mat.data[16..20], &[0.25, 0.5, 0.8, 1.0]);
            let mut tracer = cpu_tracer(&scene, &packed);
            let direction = vec3f(0.0, 0.0, -1.0);
            for (origin, distance, interfaces) in [
                (vec3f(0.01, 0.02, thickness * 0.5 + 1.0), thickness, 2),
                (vec3f(0.01, 0.02, 0.0), thickness * 0.5, 1),
            ] {
                let actual = tracer.shadow(origin - packed.origin, -direction, direction, packed.ray_tmax(), -1).unwrap().transmittance;
                for (value, a) in [actual.x, actual.y, actual.z].into_iter().zip(absorption) {
                    let expected = a.powf(distance) * 0.96f32.powi(interfaces);
                    assert!((value / expected - 1.0).abs() < 1.0e-4, "thickness {thickness}, distance {distance}: {value} vs {expected}");
                }
            }
            // The path integrator must attenuate a camera-inside segment too.
            // Select one actual Sobol sample that transmits the first interface.
            tracer.camera = Camera {
                pos: vec3f(0.01, 0.02, 0.0), target: vec3f(0.01, 0.02, -1.0),
                f_stop: 0.0, ortho_height: Some(0.001), ..Default::default()
            };
            tracer.sky = SkyUniforms::uniform_white(1.0);
            tracer.max_bounces = 2;
            let sidx = (0..32).find(|&i| tracer.sample2(pixel_seed(0, 0, 7), i, bounce_pairs(0)[0]).0 > 0.04).unwrap();
            let actual = tracer.radiance(0, 0, 1, 1, 7, sidx);
            for (value, a) in [actual.x, actual.y, actual.z].into_iter().zip(absorption) {
                let expected = a.powf(thickness * 0.5);
                assert!((value / expected - 1.0).abs() < 1.0e-4, "inside-camera path: {value} vs {expected}");
            }
            assert_eq!(tracer.invalid_samples.get(), 0);
        }
    }

    #[test]
    fn solid_clear_sphere_furnace() {
        let mut scene = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        scene.materials = vec![Material { solid: true, ..Material::glass([1.0; 3]) }];
        // Test Fresnel branch weights, not trapping in a coarse polyhedron.
        // At distance 3, a square 4-degree view has impact parameter at most
        // 3*sin(atan(sqrt(2)*tan(2 degrees))) < 0.148 radii, including jitter.
        // 64 segments give 5.625-degree cells: facet/radial tilt is bounded
        // by acos(cos(2.8125 degrees)^2) < 4 degrees, radius >= 0.99759.
        // Entry incidence < asin(0.148/0.99759)+4 = 12.54 degrees, so the
        // refracted angle is < 8.33 degrees. Each chord adds at most 8.2
        // degrees (two facet tilts plus the radius variation here). Even
        // after three internal reflections, incidence < 8.33+4*8.2 = 41.13
        // degrees < asin(1/1.5) = 41.81 degrees: no forced TIR on those paths.
        // Entry + three reflections + exit + sky take 6 of 16 iterations.
        // Fresnel can still choose longer reflection chains; require every
        // seeded path to escape below the cap with escaped_to_sky below.
        crate::scene::push_sphere(&mut scene, Vec3f::default(), 1.0, 64, 0);
        scene.camera = Camera {
            pos: vec3f(0.0, 0.0, 3.0), target: Vec3f::default(),
            fov_y: 4.0f32.to_radians(), f_stop: 0.0, ..Default::default()
        };
        let packed = PackedScene::pack(&scene);
        let mut tracer = cpu_tracer(&scene, &packed);
        tracer.sky = SkyUniforms::uniform_white(1.0);
        tracer.max_bounces = 16;
        for (i, pixel) in tracer.render(4, 4, 256, 43).iter().enumerate() {
            assert!(pixel.iter().all(|&v| (v - 1.0).abs() < 0.02), "furnace pixel {i}: {pixel:?}");
        }
        assert_eq!(tracer.stats().primary_hits, 4 * 4 * 256, "test must cover the sphere, not empty sky");
        assert_eq!(tracer.stats().escaped_to_sky, 4 * 4 * 256, "every furnace path must escape before the bounce cap");
        assert_eq!(tracer.invalid_samples.get(), 0);
    }

    #[test]
    fn solid_closed_box_sun_crosses_four_interfaces() {
        let mut scene = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        scene.materials = vec![
            Material { solid: true, ..Material::glass([1.0; 3]) },
            Material::diffuse([0.7; 3]),
        ];
        // Two separate closed shells: cavity -> glass -> air -> glass -> air.
        // A single shell from an inside point would cross only TWO interfaces.
        // Reversing each inner box makes its normals point into the cavity.
        for (base, size, inward) in [
            (0.0, 2.0, false), (0.1, 1.8, true),
            (0.2, 1.6, false), (0.3, 1.4, true),
        ] {
            let first = scene.indices.len();
            crate::scene::push_box(&mut scene, vec3f(0.0, base, 0.0), vec3f(size, size, size), 0.0, 0);
            if inward {
                for tri in scene.indices[first..].chunks_exact_mut(3) { tri.swap(1, 2); }
            }
        }
        scene.push_quad([[-0.5, 0.4, -0.5], [-0.5, 0.4, 0.5], [0.5, 0.4, 0.5], [0.5, 0.4, -0.5]], 1);
        scene.camera = Camera {
            pos: vec3f(0.03, 0.5, 0.02), target: vec3f(0.03, 0.4, 0.02),
            up: vec3f(0.0, 0.0, -1.0), ortho_height: Some(0.01), f_stop: 0.0,
            ..Default::default()
        };
        scene.sun = crate::scene::Sun { dir: vec3f(0.0, 1.0, 0.0), sky_strength: 0.0, ..Default::default() };
        let packed = PackedScene::pack(&scene);
        let mut tracer = cpu_tracer(&scene, &packed);
        tracer.max_bounces = 1;
        let up = scene.sun.dir;
        let point = vec3f(0.03, 0.4, 0.02) - packed.origin;
        let vis = tracer.shadow(point, up, up, packed.ray_tmax(), -1).unwrap().transmittance;
        let expected_visibility = 0.96f32.powi(4);
        assert!((vis.x - expected_visibility).abs() < 1.0e-3, "closed case: {vis:?}");
        assert_eq!(tracer.stats().glass_shadow_hits, 4, "every real boundary counts");
        let unoccluded = bsdf_eval(up, up, up, vec3f(0.7, 0.7, 0.7), vec3f(0.04, 0.04, 0.04), 1.0)
            * tracer.sun_irradiance();
        assert!(unoccluded.x > 0.0);
        let pixel = tracer.render(1, 1, 16, 29)[0];
        for (actual, open) in pixel.into_iter().zip([unoccluded.x, unoccluded.y, unoccluded.z]) {
            assert!((actual / open - expected_visibility).abs() < 1.0e-3, "direct-light ratio {actual}/{open}");
        }
        assert_eq!(tracer.invalid_samples.get(), 0);
        scene.materials[0].transmission = 0.0;
        let opaque_packed = PackedScene::pack(&scene);
        let mut opaque = cpu_tracer(&scene, &opaque_packed);
        opaque.max_bounces = 1;
        assert_eq!(opaque.shadow(point, up, up, opaque_packed.ray_tmax(), -1).unwrap().transmittance, Vec3f::default());
        assert_eq!(opaque.render(1, 1, 16, 29)[0], [0.0; 3], "opaque case must be black");
        assert_eq!(opaque.invalid_samples.get(), 0);
    }

    #[test]
    fn dielectric_guides_reach_the_board_and_solid_shadow_budget_is_bounded() {
        let mut scene = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        scene.materials = vec![
            Material { solid: true, ..Material::glass([1.0; 3]) },
            Material::diffuse([0.7; 3]),
        ];
        // Exit faces lie inside BOTH the neighbour band and shadow skin.
        // They must still contribute Fresnel and count against the budget.
        for z in [0.3, 0.2, 0.1, 0.0] {
            crate::scene::push_box(&mut scene, vec3f(0.0, -1.0, z), vec3f(2.0, 2.0, 1.0e-5), 0.0, 0);
        }
        scene.push_quad([[-1.0, -1.0, -0.5], [1.0, -1.0, -0.3], [1.0, 1.0, -0.3], [-1.0, 1.0, -0.5]], 1);
        scene.camera = Camera { pos: vec3f(0.0, 0.0, 0.6), target: Vec3f::default(), f_stop: 0.0, ..Default::default() };
        let direction = vec3f(0.0, 0.0, -1.0);
        let packed = PackedScene::pack(&scene);
        let tracer = cpu_tracer(&scene, &packed);
        let guide = tracer.denoise_guide(scene.camera.pos, direction);
        let normal = vec3f(-0.1, 0.0, 1.0).normalize();
        assert!((vec3f(guide.x, guide.y, guide.z) - normal).length() < 1.0e-6, "guide must be the board's normal: {guide:?}");
        assert!((guide.w - 1.0).abs() < 1.0e-5, "guide must be the board's depth: {guide:?}");
        let vis = tracer.shadow(scene.camera.pos - packed.origin, -direction, direction, 0.85, -1).unwrap().transmittance;
        assert!((vis.x - 0.96f32.powi(8)).abs() < 1.0e-4, "eight solid boundaries: {vis:?}");
        assert_eq!(tracer.stats().glass_shadow_hits, 8);
        // The same deterministic guide continuation also supports legacy thin panes.
        scene.materials[0].solid = false;
        let thin_packed = PackedScene::pack(&scene);
        let thin_guide = cpu_tracer(&scene, &thin_packed).denoise_guide(scene.camera.pos, direction);
        assert!((vec3f(thin_guide.x, thin_guide.y, thin_guide.z) - normal).length() < 1.0e-6);
        assert!((thin_guide.w - 1.0).abs() < 1.0e-5);
        scene.materials[0].solid = true;
        crate::scene::push_box(&mut scene, vec3f(0.0, -1.0, -0.1), vec3f(2.0, 2.0, 1.0e-5), 0.0, 0);
        let packed = PackedScene::pack(&scene);
        let tracer = cpu_tracer(&scene, &packed);
        assert_eq!(tracer.shadow(scene.camera.pos - packed.origin, -direction, direction, 0.85, -1).unwrap().transmittance, Vec3f::default(), "ninth interface must occlude");
        assert_eq!(tracer.denoise_guide(scene.camera.pos, direction).w, -1.0, "exhaustion must not invent a board guide");
    }

    /// Thin glass passes most light straight through (a window, not a wall).
    #[test]
    fn thin_glass_transmits() {
        let mut s = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        s.materials = vec![Material::glass([1.0, 1.0, 1.0])];
        s.push_quad([[-2.0, -2.0, 0.0], [2.0, -2.0, 0.0], [2.0, 2.0, 0.0], [-2.0, 2.0, 0.0]], 0);
        s.ensure_normals();
        s.camera = Camera { pos: vec3f(0.0, 0.0, 3.0), target: vec3f(0.0, 0.0, 0.0), f_stop: 0.0, ..Default::default() };
        let packed = PackedScene::pack(&s);
        let mut t = cpu_tracer(&s, &packed);
        t.sky = SkyUniforms::uniform_white(1.0);
        let img = t.render(8, 8, 64, 3);
        let p = img[8 * 4 + 4];
        assert!(p[0] > 0.9 && p[0] < 1.05, "through the pane: {p:?}");
    }

    #[test]
    fn thin_glass_transmits_shadow_rays() {
        let mut s = SceneInput::default();
        s.materials = vec![Material::glass([0.8, 0.9, 1.0])];
        s.push_quad([[-2.0, -2.0, 0.0], [2.0, -2.0, 0.0], [2.0, 2.0, 0.0], [-2.0, 2.0, 0.0]], 0);
        // A boxed pane inside the pairing band but beyond the spurious-
        // neighbour band, so this exercises the material-aware pairing:
        // its opposite face must not apply transmittance a second time.
        let gap = (s.bounds().1 - s.bounds().0).length() * 7.5e-5;
        s.push_quad([[-2.0, 2.0, -gap], [2.0, 2.0, -gap], [2.0, -2.0, -gap], [-2.0, -2.0, -gap]], 0);
        let packed = PackedScene::pack(&s);
        let t = cpu_tracer(&s, &packed);
        let tr = t
            .shadow(
                vec3f(0.0, 0.0, 1.0),
                vec3f(0.0, 0.0, 1.0),
                vec3f(0.0, 0.0, -1.0),
                10.0,
                -1,
            )
            .unwrap()
            .transmittance;
        // Normal-incidence dielectric Fresnel is 4%; tint is applied once.
        assert!((tr.x - 0.768).abs() < 0.002 && (tr.y - 0.864).abs() < 0.002 && (tr.z - 0.96).abs() < 0.002, "{tr:?}");
    }

    #[test]
    fn textured_glass_tints_shadow_rays() {
        let mut s = SceneInput::default();
        s.images.push(crate::scene::Image { width: 1, height: 1, data: vec![0xffff_0000] });
        s.materials = vec![Material { albedo: [1.0; 3], transmission: 1.0, texture: Some(0), ..Default::default() }];
        let positions = [[-2.0, -2.0, 0.0], [2.0, -2.0, 0.0], [2.0, 2.0, 0.0], [-2.0, 2.0, 0.0]];
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        s.push_mesh(&positions, None, Some(&uvs), &[0, 1, 2, 0, 2, 3], 0);
        let packed = PackedScene::pack(&s);
        let tr = cpu_tracer(&s, &packed)
            .shadow(
                vec3f(0.2, 0.1, 1.0),
                vec3f(0.0, 0.0, 1.0),
                vec3f(0.0, 0.0, -1.0),
                10.0,
                -1,
            )
            .unwrap()
            .transmittance;
        assert!(tr.x > 0.95 && tr.y < 0.001 && tr.z < 0.001, "{tr:?}");
    }

    #[test]
    fn shadow_budget_crosses_four_boxed_panes() {
        let mut s = SceneInput::default();
        s.materials = vec![Material::glass([1.0; 3])];
        let gap = vec3f(4.0, 4.0, 0.3).length() * 7.5e-5;
        for z in [0.0f32, -0.1, -0.2, -0.3] {
            s.push_quad(
                [[-2.0, -2.0, z], [2.0, -2.0, z], [2.0, 2.0, z], [-2.0, 2.0, z]],
                0,
            );
            s.push_quad(
                [
                    [-2.0, 2.0, z - gap],
                    [2.0, 2.0, z - gap],
                    [2.0, -2.0, z - gap],
                    [-2.0, -2.0, z - gap],
                ],
                0,
            );
        }
        let packed = PackedScene::pack(&s);
        let tr = cpu_tracer(&s, &packed)
            .shadow(
                vec3f(0.2, 0.1, 1.0),
                vec3f(0.0, 0.0, 1.0),
                vec3f(0.0, 0.0, -1.0),
                10.0,
                -1,
            )
            .unwrap()
            .transmittance;
        let expected = 0.96f32.powi(4);
        assert!((tr.x - expected).abs() < 0.002, "{tr:?} expected {expected}");
    }

    /// Closed room whose only connection to the sun and sky is one thin pane.
    fn glazed_room() -> SceneInput {
        let mut s = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        s.materials = vec![
            Material::diffuse([0.75, 0.75, 0.75]),
            Material {
                albedo: [0.92, 0.97, 1.0],
                roughness: 0.02,
                ior: 1.52,
                transmission: 0.69,
                two_sided: true,
                ..Default::default()
            },
        ];
        // Inward-facing floor, ceiling, rear and side walls.
        s.push_quad([[-2.0, 0.0, -2.0], [-2.0, 0.0, 2.0], [2.0, 0.0, 2.0], [2.0, 0.0, -2.0]], 0);
        s.push_quad([[-2.0, 2.5, -2.0], [2.0, 2.5, -2.0], [2.0, 2.5, 2.0], [-2.0, 2.5, 2.0]], 0);
        s.push_quad([[-2.0, 0.0, -2.0], [2.0, 0.0, -2.0], [2.0, 2.5, -2.0], [-2.0, 2.5, -2.0]], 0);
        s.push_quad([[-2.0, 0.0, 2.0], [-2.0, 0.0, -2.0], [-2.0, 2.5, -2.0], [-2.0, 2.5, 2.0]], 0);
        s.push_quad([[2.0, 0.0, -2.0], [2.0, 0.0, 2.0], [2.0, 2.5, 2.0], [2.0, 2.5, -2.0]], 0);
        // Front wall strips leave a 2.4 m × 1.5 m window opening.
        s.push_quad([[-2.0, 0.0, 2.0], [-2.0, 2.5, 2.0], [-1.2, 2.5, 2.0], [-1.2, 0.0, 2.0]], 0);
        s.push_quad([[1.2, 0.0, 2.0], [1.2, 2.5, 2.0], [2.0, 2.5, 2.0], [2.0, 0.0, 2.0]], 0);
        s.push_quad([[-1.2, 0.0, 2.0], [-1.2, 0.6, 2.0], [1.2, 0.6, 2.0], [1.2, 0.0, 2.0]], 0);
        s.push_quad([[-1.2, 2.1, 2.0], [-1.2, 2.5, 2.0], [1.2, 2.5, 2.0], [1.2, 2.1, 2.0]], 0);
        s.push_quad([[-1.2, 0.6, 2.0], [-1.2, 2.1, 2.0], [1.2, 2.1, 2.0], [1.2, 0.6, 2.0]], 1);
        s.ensure_normals();
        s.camera = Camera {
            pos: vec3f(0.0, 1.25, 1.0),
            target: vec3f(0.0, 1.25, -2.0),
            up: vec3f(0.0, 1.0, 0.0),
            f_stop: 0.0,
            ..Default::default()
        };
        s.sun = crate::scene::Sun { dir: vec3f(0.1, 0.3, 1.0).normalize(), ..Default::default() };
        s
    }

    #[test]
    fn glazed_room_interior_wall_is_lit_by_sun_and_sky() {
        let scene = glazed_room();
        let packed = PackedScene::pack(&scene);
        assert_eq!(packed.n_portals, 2);
        let tracer = cpu_tracer(&scene, &packed);
        let image = tracer.render(24, 24, 64, 23);
        let image_mean = |image: &[[f32; 3]]| {
            image.iter().map(|p| (p[0] + p[1] + p[2]) / 3.0).sum::<f32>() / image.len() as f32
        };
        let mean = image_mean(&image);

        let mut opaque = glazed_room();
        opaque.materials[1].transmission = 0.0;
        let opaque_packed = PackedScene::pack(&opaque);
        let opaque_mean = image_mean(&cpu_tracer(&opaque, &opaque_packed).render(24, 24, 64, 23));
        let gain = mean / opaque_mean.max(1.0e-6);
        println!(
            "glazed room 64 spp: mean={mean:.6} opaque={opaque_mean:.6} gain={gain:.1}x stats={:?}",
            tracer.stats()
        );
        assert!(mean > 0.05, "closed-room wall mean {mean}");
        assert!(gain > 50.0, "transparent-shadow gain {gain}x ({mean} vs {opaque_mean})");

        // The GPU uses a fixed 4x4 log meter; compare its CPU mirror with a
        // full-image reference on the same one-window closed room. Displayed
        // means, not merely exposure factors, are the picture-level contract.
        let gpu_exposure = crate::gpu::metered_exposure_from_rgb(&image, 24, 24, 1.0);
        let reference_log_mean = (image
            .iter()
            .map(|pixel| {
                (0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2])
                    .max(0.0001)
                    .ln()
            })
            .sum::<f32>()
            / image.len() as f32)
            .exp();
        let cpu_exposure = (0.08 / reference_log_mean).clamp(1.0, 16.0);
        let display_mean = |exposure| {
            image
                .iter()
                .map(|&pixel| {
                    let display = crate::gpu::tonemap_rgb(pixel, exposure);
                    0.2126 * display[0] + 0.7152 * display[1] + 0.0722 * display[2]
                })
                .sum::<f32>()
                / image.len() as f32
        };
        let gpu_mean = display_mean(gpu_exposure);
        let cpu_mean = display_mean(cpu_exposure);
        let parity_error = (gpu_mean - cpu_mean).abs() / cpu_mean.max(1.0e-6);
        println!(
            "closed-room metered parity: gpu-grid={gpu_mean:.6} cpu-full={cpu_mean:.6} error={:.2}%",
            parity_error * 100.0
        );
        assert!(
            parity_error < 0.10,
            "closed-room GPU/CPU displayed means differ by {:.2}% ({gpu_mean} vs {cpu_mean})",
            parity_error * 100.0
        );
    }

    #[test]
    fn cornell_box_has_energy_where_it_should() {
        let scene = SceneInput::cornell_box(false);
        let packed = PackedScene::pack(&scene);
        let t = cpu_tracer(&scene, &packed);
        let (w, h) = (24, 24);
        let img = t.render(w, h, 32, 5);
        let px = |x: u32, y: u32| img[(y * w + x) as usize];
        // Left wall red, right wall green (in linear radiance).
        let l = px(1, 12);
        let r = px(22, 12);
        assert!(l[0] > l[1] * 2.0 && l[0] > l[2] * 2.0, "left wall not red: {l:?}");
        assert!(r[1] > r[0] * 1.5 && r[1] > r[2] * 1.5, "right wall not green: {r:?}");
        // The ceiling panel is the brightest thing in the frame (top rows).
        let mut top = 0.0f32;
        for y in 0..8 {
            for x in 6..18 {
                top = top.max(px(x, y)[0]);
            }
        }
        assert!(top > 5.0, "light panel dim: {top}");
        // Rays per path is a sane number and gets reported.
        let paths = (w * h * 32) as f64;
        let rpp = t.rays.get() as f64 / paths;
        assert!(rpp > 2.0 && rpp < 20.0, "rays per path {rpp}");
    }

    fn sunlit_quad() -> SceneInput {
        let mut s = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        s.materials = vec![Material::diffuse([0.8, 0.8, 0.8])];
        s.push_quad([[-2.0, -2.0, 0.0], [2.0, -2.0, 0.0], [2.0, 2.0, 0.0], [-2.0, 2.0, 0.0]], 0);
        s.ensure_normals();
        // Ortho so every pixel sees the same ray direction; remaining spatial
        // variance is estimator noise, not a perspective shading gradient.
        s.camera = Camera {
            pos: vec3f(0.0, 0.0, 3.0),
            target: Vec3f::default(),
            f_stop: 0.0,
            ortho_height: Some(2.0),
            ..Default::default()
        };
        s.sun = crate::scene::Sun {
            dir: vec3f(0.2, 0.5, 0.8).normalize(),
            ..Default::default()
        };
        s
    }

    fn spatial_luma_var(img: &[[f32; 3]]) -> (f64, f64, f64, f64) {
        let lum: Vec<f64> = img
            .iter()
            .map(|p| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64)
            .collect();
        let n = lum.len() as f64;
        let mean = lum.iter().sum::<f64>() / n;
        let var = lum.iter().map(|l| (l - mean) * (l - mean)).sum::<f64>() / n.max(1.0);
        let min = lum.iter().copied().fold(f64::INFINITY, f64::min);
        let max = lum.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (mean, var, min, max)
    }

    #[test]
    fn sunlit_quad_variance_decays_as_one_over_n() {
        let scene = sunlit_quad();
        let packed = PackedScene::pack(&scene);
        assert_eq!(cpu_tracer(&scene, &packed).sky.sun_sample_probability(), 1.0);
        let luma = |c: Vec3f| 0.2126 * c.x as f64 + 0.7152 * c.y as f64 + 0.0722 * c.z as f64;
        let (w, h) = (8u32, 8u32);

        // Spatial picture at 64/256/1024 (Sobol) — printed for the report.
        for spp in [64u32, 256, 1024] {
            let img = cpu_tracer(&scene, &packed).render(w, h, spp, 7);
            let (mean, var, min, max) = spatial_luma_var(&img);
            println!(
                "sunlit quad {spp:>4} spp: mean={mean:.6} var={var:.8e} min={min:.6} max={max:.6} rel_rms={:.6}",
                var.sqrt() / mean.max(1e-12)
            );
        }

        let tr = cpu_tracer(&scene, &packed);
        let mut samples = Vec::new();
        for s in 0..256u32 {
            samples.push(luma(tr.radiance(4, 4, w, h, 7, s)) as f32);
        }
        let sm = samples.iter().map(|x| *x as f64).sum::<f64>() / samples.len() as f64;
        let sv = samples.iter().map(|x| {
            let d = *x as f64 - sm;
            d * d
        }).sum::<f64>() / samples.len() as f64;
        let smin = samples.iter().copied().fold(f32::INFINITY, f32::min);
        let smax = samples.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        println!(
            "one-pixel 256 samples: mean={sm:.6} var={sv:.8e} min={smin:.6} max={smax:.6} max/mean={:.3}",
            smax as f64 / sm.max(1e-12)
        );

        let cornell = SceneInput::cornell_box(false);
        let packed_c = PackedScene::pack(&cornell);
        for spp in [64u32, 256, 1024] {
            let img = cpu_tracer(&cornell, &packed_c).render(8, 8, spp, 5);
            let (mean, var, min, max) = spatial_luma_var(&img);
            println!("cornell     {spp:>4} spp: mean={mean:.6} var={var:.8e} min={min:.6} max={max:.6}");
        }

        // Per-pixel variance of the mean across independent seeds. White
        // noise must decay as 1/N; a floor here is a correlated or biased
        // estimator, not Sobol spatial structure.
        let mut white = cpu_tracer(&scene, &packed);
        white.white_noise = true;
        let n_trials = 48u32;
        let mut means64 = Vec::with_capacity(n_trials as usize);
        let mut means1024 = Vec::with_capacity(n_trials as usize);
        for trial in 0..n_trials {
            let mut s64 = 0.0;
            let mut s1024 = 0.0;
            for s in 0..1024u32 {
                let y = luma(white.radiance(4, 4, w, h, 1000 + trial, s));
                s1024 += y;
                if s < 64 {
                    s64 += y;
                }
            }
            means64.push(s64 / 64.0);
            means1024.push(s1024 / 1024.0);
        }
        let var_of = |xs: &[f64]| {
            let m = xs.iter().sum::<f64>() / xs.len() as f64;
            xs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / xs.len() as f64
        };
        let v64 = var_of(&means64);
        let v1024 = var_of(&means1024);
        let expected = v64 * (64.0 / 1024.0);
        let ratio = v1024 / expected.max(1e-30);
        println!(
            "white-noise per-pixel var of mean: 64 spp {v64:.8e}  1024 spp {v1024:.8e}  1/N extra {expected:.8e}  ratio {ratio:.4}"
        );
        assert!(
            ratio <= 1.1,
            "1024 spp variance {v1024:.4e} exceeds 1.1× the 1/N extrapolation {expected:.4e} from 64 spp (ratio {ratio:.4})"
        );
        assert!(smin > 0.3, "sun-miss samples still in the estimator: min={smin}");
        assert!(smax as f64 / sm < 3.0, "firefly max/mean {}", smax as f64 / sm);
    }

    #[test]
    fn nee_uses_geometric_frame_when_shading_normal_hides_the_sun() {
        let mut s = sunlit_quad();
        // Keep ns in the same hemisphere as ng (so it is not flipped/snapped)
        // but facing away from the sun. Geometric +Z still sees the sun.
        let ns = vec3f(0.0, -0.9, 0.2).normalize();
        s.normals = vec![[ns.x, ns.y, ns.z]; s.positions.len()];
        let packed = PackedScene::pack(&s);
        let c = cpu_tracer(&s, &packed).radiance(4, 4, 8, 8, 7, 0);
        assert!(
            c.x + c.y + c.z > 0.3,
            "NEE skipped the sun because ns faced away: {c:?}"
        );
    }

    /// Two perpendicular quads meeting at a concave edge, plus a duplicated
    /// overlapping floor from a second element — the BIM junction that turns
    /// a geometric-normal origin offset into an immediate shadow-ray hit.
    ///
    /// The duplicate sits 5e-7 diagonals above the floor so hits in a 2-pixel
    /// band along the edge reconstruct *behind* it (unwelded BIM vertices).
    /// The camera is a tight ortho crop of that junction.
    fn concave_junction_scene() -> SceneInput {
        let mut s = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        s.materials = vec![Material::diffuse([0.8, 0.8, 0.8])];
        // Floor y=0, z≥0, facing +Y.
        s.push_quad([[-2.0, 0.0, 0.0], [-2.0, 0.0, 4.0], [2.0, 0.0, 4.0], [2.0, 0.0, 0.0]], 0);
        // Wall z=0, y≥0, facing +Z.
        s.push_quad([[-2.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 4.0, 0.0], [-2.0, 4.0, 0.0]], 0);
        // Second element's floor slab, overlapping the junction. Scale the
        // reconstruction-error fixture and its crop with the tolerance.
        let relative = vec3f(4.0, 4.0, 4.3).length() * 0.005;
        let layer = 1.0e-4 * relative;
        s.push_quad([[-2.0, layer, -0.3], [-2.0, layer, 0.3], [2.0, layer, 0.3], [2.0, layer, -0.3]], 0);
        s.ensure_normals();
        s.camera = Camera {
            pos: vec3f(0.0, 5.0e-4 * relative, 2.0),
            target: vec3f(0.0, 5.0e-4 * relative, 0.0),
            f_stop: 0.0,
            ortho_height: Some(0.002 * relative),
            ..Default::default()
        };
        s.sun = crate::scene::Sun {
            dir: vec3f(0.08, 0.06, 1.0).normalize(),
            sky_strength: 0.0,
            sun_strength: 4.0,
            ..Default::default()
        };
        s
    }

    #[test]
    fn offset_from_concave_edge_must_not_hit_the_neighbour() {
        let scene = concave_junction_scene();
        let packed = PackedScene::pack(&scene);
        let t = cpu_tracer(&scene, &packed);
        let ld = scene.sun.dir.normalize();
        let ng_wall = vec3f(0.0, 0.0, 1.0);
        let ng_floor = vec3f(0.0, 1.0, 0.0);
        let skip_of = |ng: Vec3f| {
            packed
                .accel
                .tris
                .iter()
                .position(|tri| geom_ng(tri).dot(ng) > 0.9)
                .map(|i| i as i32)
                .unwrap_or(-1)
        };
        let mut immediate = 0u32;
        let mut legacy = 0u32;
        let mut tested = 0u32;
        // Hits reconstructed on the shared edge and a band behind the
        // concave neighbour — the pixels that show up as isolated black dots.
        for (y, z, ng) in [
            (0.0, 0.0, ng_wall),
            (0.0, 0.0, ng_floor),
            (1.0e-7, 0.0, ng_wall),
            (1.0e-6, 0.0, ng_wall),
            (1.0e-5, 0.0, ng_wall),
            (1.0e-4, 0.0, ng_wall),
            (0.0, 1.0e-7, ng_floor),
            (0.0, 1.0e-6, ng_floor),
            (0.0, 1.0e-5, ng_floor),
            (0.0, 1.0e-4, ng_floor),
            (-1.0e-7, 0.0, ng_wall),
            (-1.0e-6, 0.0, ng_wall),
            (-1.0e-5, 0.0, ng_wall),
            (-1.0e-4, 0.0, ng_wall),
            (0.0, -1.0e-7, ng_floor),
            (0.0, -1.0e-6, ng_floor),
            (0.0, -1.0e-5, ng_floor),
            (0.0, -1.0e-4, ng_floor),
            (-1.0e-6, -1.0e-6, ng_wall),
            (-1.0e-6, -1.0e-6, ng_floor),
        ] {
            let p = vec3f(0.0, y, z) * (packed.scene_scale * 0.005) - packed.origin;
            if ng.dot(ld) <= 0.0 {
                continue;
            }
            tested += 1;
            let n = if ng.dot(ld) >= 0.0 { ng } else { -ng };
            let old = packed.accel.trace(p + n * ray_error(p, packed.scene_scale), ld, packed.ray_tmax(), false);
            if old.is_hit() && old.t < packed.scene_scale * 5.0e-5 {
                legacy += 1;
            }
            let h = t.secondary_trace(p, ng, ld, packed.ray_tmax(), skip_of(ng));
            if h.is_hit() && h.t < packed.scene_scale * 5.0e-5 {
                immediate += 1;
            }
        }
        println!("concave-edge probes: legacy {legacy}/{tested} leaked within the neighbour band; fixed {immediate}/{tested}");
        assert!(tested > 0);
        assert_eq!(immediate, 0, "{immediate}/{tested} concave-edge shadow rays leaked at t≈0");
    }

    #[test]
    fn concave_junction_has_no_black_pixels_at_64_spp() {
        let scene = concave_junction_scene();
        let packed = PackedScene::pack(&scene);
        let t = cpu_tracer(&scene, &packed);
        let (w, h) = (40u32, 40u32);
        let img = t.render(w, h, 64, 11);
        let luma = |p: [f32; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
        let pixel_h = scene.camera.ortho_height.unwrap() / h as f32;
        let band = 2.0 * pixel_h;

        let mut near = Vec::new();
        let mut wall_open = Vec::new();
        for py in 0..h {
            for px in 0..w {
                let ph = t.primary_hit(px, py, w, h, (0.0, 0.0));
                if ph.x < 0.0 {
                    continue;
                }
                let tri = &packed.accel.tris[ph.x as usize];
                let w0 = 1.0 - ph.y - ph.z;
                let pt = tri.v0 * w0 + tri.v1 * ph.y + tri.v2 * ph.z;
                let world = pt + packed.origin;
                let ng = geom_ng(tri);
                let i = (py * w + px) as usize;
                let on_wall = ng.z.abs() > ng.y.abs();
                if !on_wall {
                    continue;
                }
                if world.y.abs() <= band {
                    near.push(i);
                } else {
                    wall_open.push(luma(img[i]));
                }
            }
        }
        wall_open.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let wall_med = wall_open[wall_open.len() / 2];
        let thresh = wall_med * 0.08;
        let black = near.iter().filter(|&&i| luma(img[i]) < thresh).count();
        println!(
            "concave junction: near={n} wall_med={wall_med:.4} thresh={thresh:.4} black={black} open_wall={ow}",
            n = near.len(),
            ow = wall_open.len()
        );
        assert!(wall_med > 0.2, "sunlit wall too dim to test against: {wall_med}");
        assert!(!near.is_empty(), "camera missed the concave edge");
        assert_eq!(black, 0, "junction black pixels: {black}/{} near-junction (wall median {wall_med})", near.len());
    }

    #[test]
    fn scrambled_sobol_beats_white_noise_at_16_64_256_spp() {
        let scene = SceneInput::cornell_box(false);
        let packed = PackedScene::pack(&scene);
        let reference = cpu_tracer(&scene, &packed).render(12, 12, 4096, 23);
        let aces = |x: f32| (x * (2.51 * x + 0.03) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0);
        let rmse = |a: &[[f32; 3]]| {
            let e: f64 = a
                .iter()
                .zip(&reference)
                .flat_map(|(a, b)| (0..3).map(move |k| (aces(a[k]) - aces(b[k])) as f64))
                .map(|d| d * d)
                .sum();
            (e / (a.len() * 3) as f64).sqrt()
        };
        let mut wins = 0;
        let mut table = Vec::new();
        for spp in [16, 64, 256] {
            let qmc = cpu_tracer(&scene, &packed).render(12, 12, spp, 23);
            let mut white = cpu_tracer(&scene, &packed);
            white.white_noise = true;
            let white = white.render(12, 12, spp, 23);
            let (rq, rw) = (rmse(&qmc), rmse(&white));
            wins += (rq < rw) as usize;
            table.push((spp, rq, rw));
        }
        println!("Cornell ACES RMSE vs 4096 spp: {table:?}");
        assert_eq!(wins, 3, "scrambled Sobol must win every checkpoint: {table:?}");
    }
}
