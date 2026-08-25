//! `SceneInput` → the GPU's data: float texels for triangles, per-vertex
//! attributes, BVH4 nodes, materials and emissive lights; an 8-bit texture
//! atlas; and the unindexed vertex stream the G-buffer rasterizer draws.
//!
//! Every data texture is `DATA_W` texels wide (a power of two, so the shader
//! addresses texel `i` with `i & (DATA_W-1)` / `i >> log2`) and as tall as it
//! needs to be. Layouts:
//!
//! ```text
//! tri   (3/tri):  (v0.xyz, material) (v1.xyz, original id)
//!                 (v2.xyz, emissive-selection pdf)
//! attr  (4/tri):  (n0.xyz, u0) (n1.xyz, v0) (n2.xyz, u1)
//!                 (v1, u2, v2, coplanar-group/priority code)
//! bvh   (2/node): see bvh.rs (threaded skip-link layout)
//! mat   (4/mat):  (albedo.rgb, roughness) (emission.rgb, metal)
//!                 (ior, transmission, texture, 0) (atlas rect x, y, w, h)
//! light (1/emissive tri, then 6 normal-conditioned sky CDFs/cell):
//!                 (direction bin, solid angle, selection pdf, cdf)
//! ```
//!
//! Triangles are in BVH order; `tri_material`/attributes follow.

use crate::bvh::{Bvh, Tri};
use crate::scene::{Image, SceneInput};
use makepad_draw::*;

pub const DATA_W: usize = 2048;
pub const DATA_SHIFT: u32 = 11;
pub const ATLAS_MAX: usize = 4096;
const ATLAS_GUTTER: usize = 1;
pub const ENV_THETA_BINS: usize = 8;
pub const ENV_PHI_BINS: usize = 16;
pub const ENV_DIR_BINS: usize = ENV_THETA_BINS * ENV_PHI_BINS;
pub(crate) const MAX_SHADOW_GLASS_HITS: usize = 8;

#[derive(Clone, Debug, Default)]
pub struct DataTex {
    pub data: Vec<f32>,
    pub width: usize,
    pub height: usize,
}

impl DataTex {
    fn from_texels(mut data: Vec<f32>) -> Self {
        let texels = (data.len() + 3) / 4;
        let height = ((texels + DATA_W - 1) / DATA_W).max(1);
        data.resize(height * DATA_W * 4, 0.0);
        Self { data, width: DATA_W, height }
    }
    pub fn inv(&self) -> Vec2f {
        vec2f(1.0 / self.width as f32, 1.0 / self.height as f32)
    }
}

#[derive(Clone, Debug, Default)]
pub struct PackedScene {
    pub tri: DataTex,
    pub attr: DataTex,
    pub bvh: DataTex,
    pub mat: DataTex,
    pub light: DataTex,
    pub n_lights: usize,
    /// Number of decoded transmissive triangles (environment-guide evidence,
    /// not a light-record count).
    pub n_portals: usize,
    /// Spatial/normal-conditioned upper-hemisphere guide appended to `light`.
    pub env_grid_dim: u32,
    pub env_min: Vec3f,
    pub env_inv_extent: Vec3f,
    pub atlas: Option<Image>,
    /// `DepthMeshVertex` stream: (x,y,z,priority) (b0,b1,b2,tri) per vertex,
    /// 3 per triangle.
    pub raster_verts: Vec<f32>,
    pub raster_indices: Vec<u32>,
    pub accel: Bvh,
    pub tri_material: Vec<u32>,
    pub tri_priority: Vec<u16>,
    pub tri_coplanar_group: Vec<u32>,
    pub tri_count: usize,
    pub bounds: (Vec3f, Vec3f),
    /// World-space origin subtracted from all ray-traced geometry. Keeping
    /// traversal near zero preserves f32 precision in georeferenced models.
    pub origin: Vec3f,
    pub light_power: f32,
}

impl PackedScene {
    /// Shadow-ray self-occlusion skin, scaled to the scene: a next-event
    /// (shadow) ray ignores blockers closer than this to the surface it
    /// leaves. Building models stack coincident construction layers a few
    /// millimetres apart; those layers black out each other's sun/sky rays
    /// (measured on the woodside roof: 99% of all sun blockers on roof
    /// pixels sat within 5 cm of the surface) which converges to a dirty,
    /// half-shadowed surface where a rasterizer shows a clean one. A real
    /// shadow caster (dormer, eave, furniture) is centimetres-to-metres
    /// away and is unaffected. Deliberate, documented bias; the CPU twin
    /// applies the identical value.
    pub fn auto_shadow_skin(&self) -> f32 {
        let d = self.bounds.1 - self.bounds.0;
        let diag = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
        (diag * 3.0e-4).clamp(0.002, 0.08)
    }

    pub fn pack(scene: &SceneInput) -> PackedScene {
        let tri_count = scene.tri_count();
        assert!(tri_count < (1 << 24), "path tracer supports fewer than 2^24 triangles (f32 exact-index limit)");
        let bounds = scene.bounds();
        let origin = bounds.0 + (bounds.1 - bounds.0) * 0.5;
        let p = |i: u32| {
            let v = scene.positions[i as usize];
            vec3f(v[0], v[1], v[2]) - origin
        };
        let tris: Vec<Tri> = scene
            .indices
            .chunks_exact(3)
            .map(|t| Tri { v0: p(t[0]), v1: p(t[1]), v2: p(t[2]) })
            .collect();
        let accel = Bvh::build_with_coplanar(
            &tris,
            &scene.tri_priority,
            &scene.tri_coplanar_group,
        );
        let has_n = scene.normals.len() == scene.positions.len();
        let has_uv = scene.uvs.len() == scene.positions.len();

        let light_weights: Vec<f32> = accel
            .tri_order
            .iter()
            .enumerate()
            .map(|(new_i, &old_i)| {
                let mat_id = scene.tri_material.get(old_i as usize).copied().unwrap_or(0) as usize;
                let e = scene.materials.get(mat_id).map(|m| m.emission).unwrap_or([0.0; 3]);
                let lum = 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
                let t = &accel.tris[new_i];
                let area = Vec3f::cross(t.v1 - t.v0, t.v2 - t.v0).length() * 0.5;
                (lum * area).max(0.0)
            })
            .collect();
        let light_power: f32 = light_weights.iter().sum();
        let portal_weights: Vec<f32> = accel
            .tri_order
            .iter()
            .enumerate()
            .map(|(new_i, &old_i)| {
                let mat_id = scene.tri_material.get(old_i as usize).copied().unwrap_or(0) as usize;
                let transmission = scene.materials.get(mat_id).map(|m| m.transmission).unwrap_or(0.0);
                if transmission <= 0.0 {
                    return 0.0;
                }
                let t = &accel.tris[new_i];
                Vec3f::cross(t.v1 - t.v0, t.v2 - t.v0).length() * 0.5
            })
            .collect();
        let mut light_cdf = 0.0f32;
        let mut tri = Vec::with_capacity(tri_count * 12);
        let mut attr = Vec::with_capacity(tri_count * 16);
        let mut light = Vec::new();
        let mut tri_material = Vec::with_capacity(tri_count);
        let mut tri_priority = Vec::with_capacity(tri_count);
        let mut tri_coplanar_group = Vec::with_capacity(tri_count);
        let mut raster_verts = Vec::with_capacity(tri_count * 24);
        let mut raster_indices = Vec::with_capacity(tri_count * 3);
        for (new_i, &old_i) in accel.tri_order.iter().enumerate() {
            let idx = &scene.indices[old_i as usize * 3..old_i as usize * 3 + 3];
            let m = scene.tri_material.get(old_i as usize).copied().unwrap_or(0);
            let priority = scene.tri_priority.get(old_i as usize).copied().unwrap_or(0);
            let coplanar_group = scene
                .tri_coplanar_group
                .get(old_i as usize)
                .copied()
                .unwrap_or(0);
            tri_material.push(m);
            tri_priority.push(priority);
            tri_coplanar_group.push(coplanar_group);
            let t = &accel.tris[new_i];
            let select_pdf = if light_power > 0.0 { light_weights[new_i] / light_power } else { 0.0 };
            tri.extend([t.v0.x, t.v0.y, t.v0.z, m as f32]);
            tri.extend([t.v1.x, t.v1.y, t.v1.z, old_i as f32]);
            tri.extend([t.v2.x, t.v2.y, t.v2.z, select_pdf]);
            let ng = Vec3f::cross(t.v1 - t.v0, t.v2 - t.v0);
            let ngl = ng.length();
            let ng = if ngl > 0.0 { ng / ngl } else { vec3f(0.0, 1.0, 0.0) };
            let n = |k: usize| -> [f32; 3] {
                if has_n {
                    let v = scene.normals[idx[k] as usize];
                    if v[0] != 0.0 || v[1] != 0.0 || v[2] != 0.0 {
                        return v;
                    }
                }
                [ng.x, ng.y, ng.z]
            };
            let uv = |k: usize| -> [f32; 2] { if has_uv { scene.uvs[idx[k] as usize] } else { [0.0, 0.0] } };
            let (n0, n1, n2) = (n(0), n(1), n(2));
            let (u0, u1, u2) = (uv(0), uv(1), uv(2));
            attr.extend([n0[0], n0[1], n0[2], u0[0]]);
            attr.extend([n1[0], n1[1], n1[2], u0[1]]);
            attr.extend([n2[0], n2[1], n2[2], u1[0]]);
            // Last lane is an exact 24-bit render-only group/priority code;
            // UV consumers use xyz. The measured component gates priority so
            // unrelated nearby geometry remains ordinary nearest-hit.
            let coplanar_code = ((coplanar_group.min(4095) << 12)
                | u32::from(priority.min(4095))) as f32;
            attr.extend([u1[1], u2[0], u2[1], coplanar_code]);
            if select_pdf > 0.0 && scene.materials.get(m as usize).map_or(false, |mm| mm.is_emissive()) {
                light_cdf += select_pdf;
                light.extend([new_i as f32, ngl * 0.5, select_pdf, light_cdf]);
            }
            let base = raster_verts.len() / 8;
            let world = |k: usize| {
                let q = scene.positions[idx[k] as usize];
                vec3f(q[0], q[1], q[2])
            };
            let (r0, r1, r2) = (world(0), world(1), world(2));
            // `pos.w` is ignored as a homogeneous coordinate by PtGbuf and
            // carries the same render-only priority into its depth bias.
            raster_verts.extend([r0.x, r0.y, r0.z, priority as f32, 1.0, 0.0, 0.0, new_i as f32]);
            raster_verts.extend([r1.x, r1.y, r1.z, priority as f32, 0.0, 1.0, 0.0, new_i as f32]);
            raster_verts.extend([r2.x, r2.y, r2.z, priority as f32, 0.0, 0.0, 1.0, new_i as f32]);
            raster_indices.extend([base as u32, base as u32 + 1, base as u32 + 2]);
        }
        let n_lights = light.len() / 4;
        if let Some(cdf) = light.last_mut() {
            *cdf = cdf.min(1.0);
        }
        let portal_indices: Vec<usize> = portal_weights
            .iter()
            .enumerate()
            .filter_map(|(i, &area)| (area > 0.0).then_some(i))
            .collect();
        let n_portals = portal_indices.len();
        // 216 cells × 6 normal bins × 128 directions = 165,888 texels
        // (2.53 MiB). The guide is built once with the scene and never adds a
        // ray or bounce to a rendered path.
        let env_grid_dim = if tri_count > 0 { 6 } else { 0 };
        // Grid the occupied glazing bounds, not the full model bounds. Site
        // meshes often extend tens of metres below the building and would put
        // every inhabited point in one high, poorly represented cell.
        let (env_min, env_max) = if n_portals > 0 {
            let mut lo = vec3f(f32::INFINITY, f32::INFINITY, f32::INFINITY);
            let mut hi = vec3f(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
            for &ti in &portal_indices {
                let tri = &accel.tris[ti];
                for p in [tri.v0, tri.v1, tri.v2] {
                    lo.x = lo.x.min(p.x);
                    lo.y = lo.y.min(p.y);
                    lo.z = lo.z.min(p.z);
                    hi.x = hi.x.max(p.x);
                    hi.y = hi.y.max(p.y);
                    hi.z = hi.z.max(p.z);
                }
            }
            (lo, hi)
        } else {
            (bounds.0 - origin, bounds.1 - origin)
        };
        let env_extent = env_max - env_min;
        let env_inv_extent = vec3f(
            1.0 / env_extent.x.max(1.0e-6),
            1.0 / env_extent.y.max(1.0e-6),
            1.0 / env_extent.z.max(1.0e-6),
        );
        if env_grid_dim > 0 {
            let d = env_grid_dim as usize;
            let solid_angle = std::f32::consts::TAU / ENV_DIR_BINS as f32;
            for z in 0..d {
                for y in 0..d {
                    for x in 0..d {
                        let cell = env_min
                            + vec3f(
                                (x as f32 + 0.5) * env_extent.x / d as f32,
                                (y as f32 + 0.5) * env_extent.y / d as f32,
                                (z as f32 + 0.5) * env_extent.z / d as f32,
                            );
                        let directions: Vec<(Vec3f, f32)> = (0..ENV_DIR_BINS)
                            .map(|bin| {
                                let direction = environment_bin_direction(scene.up, bin, 0.5, 0.5);
                                let visible = ray_reaches_environment(&accel, scene, cell, direction, -1);
                                (direction, if visible { 1.0 } else { 0.002 })
                            })
                            .collect();
                        for axis in [
                            vec3f(1.0, 0.0, 0.0),
                            vec3f(-1.0, 0.0, 0.0),
                            vec3f(0.0, 1.0, 0.0),
                            vec3f(0.0, -1.0, 0.0),
                            vec3f(0.0, 0.0, 1.0),
                            vec3f(0.0, 0.0, -1.0),
                        ] {
                            let weights: Vec<f32> = directions
                                .iter()
                                .map(|&(direction, visibility)| {
                                    visibility * (0.002 + axis.dot(direction).max(0.0))
                                })
                                .collect();
                            let total = weights.iter().sum::<f32>();
                            // Defensive mixture: (1-a) guided + a uniform,
                            // folded into the stored per-bin probability so
                            // the sampled CDF and the reported pdf stay one
                            // object and no shader changes. The cell guide is
                            // built at cell CENTRES: where a shading point's
                            // visibility disagrees with its cell (a reveal, a
                            // balcony underside) a purely guided pdf can be
                            // four orders of magnitude below uniform and one
                            // lucky sample becomes a firefly that takes
                            // thousands of frames to fade — the measured
                            // non-converging speckle in shadowed areas. The
                            // uniform component floors the solid-angle pdf
                            // at a/(2*PI) (about 0.008 at a = 0.05 — a 100x
                            // tighter bound on the worst sample weight) for
                            // the cost of ~5% of env samples spent probing
                            // officially-blocked bins. 0.1 measurably slowed
                            // the sunlit-quad 1/N variance decay; 0.05 keeps
                            // it (ratio 0.70 against the 1.1 gate).
                            const DEFENSIVE_UNIFORM: f32 = 0.05;
                            let uniform_bin = 1.0 / ENV_DIR_BINS as f32;
                            let mut cdf = 0.0f32;
                            for (bin, weight) in weights.into_iter().enumerate() {
                                let guided = if total > 0.0 {
                                    weight / total
                                } else {
                                    1.0 / ENV_DIR_BINS as f32
                                };
                                let select_pdf = (1.0 - DEFENSIVE_UNIFORM) * guided
                                    + DEFENSIVE_UNIFORM * uniform_bin;
                                cdf += select_pdf;
                                light.extend([bin as f32, solid_angle, select_pdf, cdf.min(1.0)]);
                            }
                            if let Some(last) = light.last_mut() {
                                *last = 1.0;
                            }
                        }
                    }
                }
            }
        }

        // Atlas + materials.
        let (atlas, rects) = build_atlas(&scene.images);
        let mut mat = Vec::with_capacity(scene.materials.len() * 16);
        for m in &scene.materials {
            mat.extend([m.albedo[0], m.albedo[1], m.albedo[2], m.roughness.clamp(0.0, 1.0)]);
            mat.extend([m.emission[0], m.emission[1], m.emission[2], m.metal.clamp(0.0, 1.0)]);
            let (tex, rect) = match m.texture.and_then(|t| rects.get(t).copied().flatten()) {
                Some(r) => (1.0, r),
                None => (-1.0, [0.0; 4]),
            };
            mat.extend([m.ior.max(1.0), m.transmission.clamp(0.0, 1.0), tex, if m.two_sided { 1.0 } else { 0.0 }]);
            mat.extend(rect);
        }
        if mat.is_empty() {
            mat.extend([0.8, 0.8, 0.8, 1.0, 0.0, 0.0, 0.0, 0.0, 1.5, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        }

        PackedScene {
            tri: DataTex::from_texels(tri),
            attr: DataTex::from_texels(attr),
            bvh: DataTex::from_texels(accel.texels()),
            mat: DataTex::from_texels(mat),
            light: DataTex::from_texels(if light.is_empty() { vec![0.0; 4] } else { light }),
            n_lights,
            n_portals,
            env_grid_dim,
            env_min,
            env_inv_extent,
            atlas,
            raster_verts,
            raster_indices,
            accel,
            tri_material,
            tri_priority,
            tri_coplanar_group,
            tri_count,
            bounds,
            origin,
            light_power,
        }
    }
}

fn environment_bin_direction(up: Vec3f, bin: usize, theta_u: f32, phi_u: f32) -> Vec3f {
    let theta_bin = bin / ENV_PHI_BINS;
    let phi_bin = bin % ENV_PHI_BINS;
    let cos_theta = (theta_bin as f32 + theta_u) / ENV_THETA_BINS as f32;
    let sin_theta = (1.0 - cos_theta * cos_theta).sqrt();
    let phi = std::f32::consts::TAU * (phi_bin as f32 + phi_u) / ENV_PHI_BINS as f32;
    let local = vec3f(sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta);
    let n = up.normalize();
    let sign = if n.z >= 0.0 { 1.0 } else { -1.0 };
    let a = -1.0 / (sign + n.z);
    let b = n.x * n.y * a;
    let tangent = vec3f(1.0 + sign * n.x * n.x * a, sign * b, -sign * n.x);
    let bitangent = vec3f(b, sign + n.y * n.y * a, -n.y);
    tangent * local.x + bitangent * local.y + n * local.z
}

fn ray_reaches_environment(
    accel: &Bvh,
    scene: &SceneInput,
    mut origin: Vec3f,
    direction: Vec3f,
    mut skip: i32,
) -> bool {
    // The extra trace proves that the ray escaped after the last allowed hit.
    for _ in 0..=MAX_SHADOW_GLASS_HITS {
        let hit = accel.trace_skip(origin, direction, 0.0, 1.0e9, false, skip);
        if hit.truncated {
            return false;
        }
        if !hit.is_hit() {
            return true;
        }
        let hit_tri = hit.tri as usize;
        let old_tri = accel.tri_order[hit_tri] as usize;
        let material = scene.tri_material.get(old_tri).copied().unwrap_or(0) as usize;
        if scene.materials.get(material).map_or(0.0, |m| m.transmission) <= 0.0 {
            return false;
        }
        origin = origin + direction * (hit.t + 1.0e-4);
        skip = hit.tri;
    }
    false
}

/// Shelf-pack the images into one atlas (downscaling by 2 until it fits
/// `ATLAS_MAX`²). Every image gets a duplicated one-texel gutter; returned
/// rects address texel centres from the first through the last source texel.
fn build_atlas(images: &[Image]) -> (Option<Image>, Vec<Option<[f32; 4]>>) {
    if images.is_empty() {
        return (None, Vec::new());
    }
    let mut imgs: Vec<Image> = images.to_vec();
    loop {
        // Sort by height for shelves, remembering original indices.
        let mut order: Vec<usize> = (0..imgs.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(imgs[i].height + 2 * ATLAS_GUTTER));
        let mut placed: Vec<Option<(usize, usize)>> = vec![None; imgs.len()];
        let (mut x, mut y, mut shelf_h) = (0usize, 0usize, 0usize);
        let mut fits = true;
        for &i in &order {
            let im = &imgs[i];
            let outer_w = im.width + 2 * ATLAS_GUTTER;
            let outer_h = im.height + 2 * ATLAS_GUTTER;
            if im.width == 0 || im.height == 0 || outer_w > ATLAS_MAX || outer_h > ATLAS_MAX {
                fits = false;
                break;
            }
            if x + outer_w > ATLAS_MAX {
                x = 0;
                y += shelf_h;
                shelf_h = 0;
            }
            if y + outer_h > ATLAS_MAX {
                fits = false;
                break;
            }
            placed[i] = Some((x, y));
            x += outer_w;
            shelf_h = shelf_h.max(outer_h);
        }
        if fits {
            let total_h = (y + shelf_h).max(1);
            let w = ATLAS_MAX.min(next_pow2(
                order
                    .iter()
                    .map(|&i| placed[i].unwrap().0 + imgs[i].width + 2 * ATLAS_GUTTER)
                    .max()
                    .unwrap_or(1),
            ));
            let h = next_pow2(total_h).min(ATLAS_MAX);
            let mut data = vec![0u32; w * h];
            let mut rects = vec![None; imgs.len()];
            for (i, im) in imgs.iter().enumerate() {
                let (px, py) = placed[i].unwrap();
                for oy in 0..im.height + 2 * ATLAS_GUTTER {
                    let sy = oy.saturating_sub(ATLAS_GUTTER).min(im.height - 1);
                    for ox in 0..im.width + 2 * ATLAS_GUTTER {
                        let sx = ox.saturating_sub(ATLAS_GUTTER).min(im.width - 1);
                        data[(py + oy) * w + px + ox] = im.data[sy * im.width + sx];
                    }
                }
                // The outer top-left gutter corner (rect.xy minus one texel)
                // carries the image's LINEAR-space mean, sRGB-encoded so the
                // shader's decode recovers it exactly. Under heavy
                // minification a nearest fetch is a per-sample texel lottery
                // (a tiled roof at 760 repeats reads a random texel every
                // sample and never converges pixel-to-pixel); the shader
                // blends toward this mean as the pixel footprint grows —
                // the same value the accumulation would converge to anyway,
                // reached with zero variance.
                let mut sum = [0.0f64; 3];
                for texel in &im.data {
                    sum[0] += (((texel >> 16) & 255) as f64 / 255.0).powf(2.2);
                    sum[1] += (((texel >> 8) & 255) as f64 / 255.0).powf(2.2);
                    sum[2] += ((texel & 255) as f64 / 255.0).powf(2.2);
                }
                let n = im.data.len().max(1) as f64;
                let enc = |v: f64| ((v / n).powf(1.0 / 2.2) * 255.0 + 0.5) as u32;
                data[py * w + px] =
                    0xff00_0000 | (enc(sum[0]) << 16) | (enc(sum[1]) << 8) | enc(sum[2]);
                rects[i] = Some([
                    (px + ATLAS_GUTTER) as f32 / w as f32 + 0.5 / w as f32,
                    (py + ATLAS_GUTTER) as f32 / h as f32 + 0.5 / h as f32,
                    im.width.saturating_sub(1) as f32 / w as f32,
                    im.height.saturating_sub(1) as f32 / h as f32,
                ]);
            }
            return (Some(Image { width: w, height: h, data }), rects);
        }
        // Halve everything and retry.
        for im in imgs.iter_mut() {
            *im = downscale2(im);
        }
        if imgs.iter().all(|im| im.width <= 1 && im.height <= 1) {
            return (None, vec![None; images.len()]);
        }
    }
}

fn next_pow2(v: usize) -> usize {
    let mut p = 1;
    while p < v {
        p <<= 1;
    }
    p
}

fn downscale2(im: &Image) -> Image {
    let w = (im.width / 2).max(1);
    let h = (im.height / 2).max(1);
    let mut data = vec![0u32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0u32; 4];
            let mut n = 0;
            for dy in 0..2 {
                for dx in 0..2 {
                    let sx = (x * 2 + dx).min(im.width - 1);
                    let sy = (y * 2 + dy).min(im.height - 1);
                    let p = im.data[sy * im.width + sx];
                    acc[0] += p & 0xff;
                    acc[1] += (p >> 8) & 0xff;
                    acc[2] += (p >> 16) & 0xff;
                    acc[3] += p >> 24;
                    n += 1;
                }
            }
            data[y * w + x] = (acc[0] / n) | ((acc[1] / n) << 8) | ((acc[2] / n) << 16) | ((acc[3] / n) << 24);
        }
    }
    Image { width: w, height: h, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cornell_packs_with_one_light_and_contiguous_leaves() {
        let s = SceneInput::cornell_box(false);
        let p = PackedScene::pack(&s);
        assert_eq!(p.tri_count, s.tri_count());
        assert_eq!(p.n_lights, 2, "the panel is two emissive triangles");
        assert_eq!(p.tri.width, DATA_W);
        assert_eq!(p.raster_verts.len(), p.tri_count * 24);
        // Every triangle texel's material id round-trips.
        for i in 0..p.tri_count {
            assert_eq!(p.tri.data[i * 12 + 3] as u32, p.tri_material[i]);
        }
        let pdf_sum: f32 = p.light.data[..p.n_lights * 4].chunks_exact(4).map(|l| l[2]).sum();
        assert!((pdf_sum - 1.0).abs() < 1.0e-6, "power-selection pdf sum {pdf_sum}");
    }

    #[test]
    fn atlas_has_duplicated_gutters_and_centre_rects() {
        let red = 0xffff_0000;
        let blue = 0xff00_00ff;
        let images = vec![
            Image { width: 1, height: 1, data: vec![red] },
            Image { width: 1, height: 1, data: vec![blue] },
        ];
        let (atlas, rects) = build_atlas(&images);
        let atlas = atlas.unwrap();
        for (i, expected) in [red, blue].into_iter().enumerate() {
            let r = rects[i].unwrap();
            let x = (r[0] * atlas.width as f32).floor() as usize;
            let y = (r[1] * atlas.height as f32).floor() as usize;
            for yy in y - 1..=y + 1 {
                for xx in x - 1..=x + 1 {
                    if (xx, yy) == (x - 1, y - 1) {
                        // The outer corner is the image's mean texel; for a
                        // one-colour image that IS the colour.
                        assert_eq!(atlas.data[yy * atlas.width + xx] & 0x00ff_ffff, expected & 0x00ff_ffff);
                        continue;
                    }
                    assert_eq!(atlas.data[yy * atlas.width + xx], expected);
                }
            }
        }
    }

    #[test]
    fn atlas_mean_corner_is_the_linear_mean() {
        // Half black, half white: the linear mean is 0.5, whose sRGB
        // encoding is (0.5)^(1/2.2) = 186.
        let images = vec![Image {
            width: 2,
            height: 1,
            data: vec![0xff00_0000, 0xffff_ffff],
        }];
        let (atlas, rects) = build_atlas(&images);
        let atlas = atlas.unwrap();
        let r = rects[0].unwrap();
        let x = (r[0] * atlas.width as f32).floor() as usize;
        let y = (r[1] * atlas.height as f32).floor() as usize;
        let corner = atlas.data[(y - 1) * atlas.width + x - 1];
        let g = (corner >> 8) & 255;
        assert!((g as i32 - 186).abs() <= 1, "mean corner green {g}");
    }

    #[test]
    fn georeferenced_scene_is_rebased_before_bvh_build() {
        let mut s = SceneInput::default();
        s.materials.push(crate::scene::Material::default());
        s.push_mesh(
            &[[1_000_000.0, 2_000_000.0, 10.0], [1_000_010.0, 2_000_000.0, 10.0], [1_000_000.0, 2_000_010.0, 10.0]],
            None,
            None,
            &[0, 1, 2],
            0,
        );
        let p = PackedScene::pack(&s);
        assert!(p.accel.tris[0].v0.x.abs() < 6.0 && p.accel.tris[0].v0.y.abs() < 6.0);
        let ro = vec3f(1_000_001.0, 2_000_001.0, 20.0) - p.origin;
        assert!(p.accel.trace(ro, vec3f(0.0, 0.0, -1.0), 100.0, false).is_hit());
    }

    #[test]
    fn coplanar_priority_reorders_with_the_bvh_and_reaches_both_gpu_paths() {
        let mut s = SceneInput::default();
        s.materials.push(crate::scene::Material::default());
        s.push_mesh(
            &[[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]],
            None,
            None,
            &[0, 1, 2],
            0,
        );
        s.tri_priority[0] = 37;
        s.tri_coplanar_group[0] = 23;
        let p = PackedScene::pack(&s);
        assert_eq!(p.tri_priority, vec![37]);
        assert_eq!(p.tri_coplanar_group, vec![23]);
        assert_eq!(p.accel.priorities, vec![37]);
        assert_eq!(p.accel.coplanar_groups, vec![23]);
        assert_eq!(p.attr.data[15], (23 * 4096 + 37) as f32, "ray shader group/priority lane");
        assert_eq!(p.raster_verts[3], 37.0, "hybrid primary priority lane");
    }

    #[test]
    fn emissive_selection_is_power_weighted() {
        let mut s = SceneInput::default();
        s.materials = vec![
            crate::scene::Material::emissive([1.0; 3], 1.0),
            crate::scene::Material::emissive([1.0; 3], 9.0),
        ];
        s.push_mesh(&[[-1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [-1.0, 1.0, 0.0]], None, None, &[0, 1, 2], 0);
        s.push_mesh(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], None, None, &[0, 1, 2], 1);
        let p = PackedScene::pack(&s);
        let mut pdfs: Vec<f32> = p.light.data[..8].chunks_exact(4).map(|l| l[2]).collect();
        pdfs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!((pdfs[0] - 0.1).abs() < 1.0e-6 && (pdfs[1] - 0.9).abs() < 1.0e-6, "{pdfs:?}");
    }
}
