//! Equirectangular environment radiance and shared CPU/GPU sampling tables.

use crate::pack::DataTex;
use makepad_draw::*;
use std::f32::consts::{PI, TAU};

/// Linear radiance over the sphere. In the map's frame +Y is up,
/// u = 0.5 + atan2(d.x, -d.z) / 2pi and v = acos(d.y) / pi.
/// -Z is at the image centre and row 0 is at the zenith. Texel (i, j)
/// covers [i/W, (i+1)/W) in u and [j/H, (j+1)/H) in v.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentMap {
    pub width: usize,
    pub height: usize,
    /// Row-major, `width * height` texels.
    pub texels: Vec<[f32; 3]>,
}

/// Where a map hangs and how bright it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentFrame {
    /// Rows of the rotation that takes a world direction into the map's frame.
    pub world_to_map: [Vec3f; 3],
    /// Radiance multiplier.
    pub strength: f32,
}

impl Default for EnvironmentFrame {
    fn default() -> Self {
        Self {
            world_to_map: [vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0)],
            strength: 1.0,
        }
    }
}

impl EnvironmentFrame {
    pub fn to_map(&self, d: Vec3f) -> Vec3f {
        let rows = self.world_to_map;
        vec3f(rows[0].dot(d), rows[1].dot(d), rows[2].dot(d))
    }

    pub fn to_world(&self, m: Vec3f) -> Vec3f {
        let rows = self.world_to_map;
        rows[0] * m.x + rows[1] * m.y + rows[2] * m.z
    }
}

/// A map with its importance-sampling tables, built once per map.
pub struct EnvironmentLight {
    map: EnvironmentMap,
    marginal_cdf: Vec<f32>,
    conditional_cdf: Vec<f32>,
}

impl EnvironmentLight {
    /// None for an empty map or a texel count that is not width * height.
    pub fn new(map: EnvironmentMap) -> Option<Self> {
        let count = map.width.checked_mul(map.height)?;
        if count == 0 || map.texels.len() != count {
            return None;
        }
        let mut conditional_cdf = Vec::with_capacity(count);
        let mut row_sums = Vec::with_capacity(map.height);
        for (j, row) in map.texels.chunks_exact(map.width).enumerate() {
            let sin_theta = (PI * (j as f32 + 0.5) / map.height as f32).sin();
            let mut sum = 0.0f64;
            for &[r, g, b] in row {
                let weight = (0.2126 * r + 0.7152 * g + 0.0722 * b).max(0.0) * sin_theta;
                conditional_cdf.push(weight);
                sum += weight as f64;
            }
            row_sums.push(sum);
        }
        let mut total: f64 = row_sums.iter().sum();
        if total == 0.0 || !total.is_finite() {
            for (j, row) in conditional_cdf.chunks_exact_mut(map.width).enumerate() {
                let weight = (PI * (j as f32 + 0.5) / map.height as f32).sin();
                row.fill(weight);
                row_sums[j] = weight as f64 * map.width as f64;
            }
            total = row_sums.iter().sum();
        }
        let mut marginal_cdf = Vec::with_capacity(map.height);
        let mut running = 0.0f64;
        for (j, row) in conditional_cdf.chunks_exact_mut(map.width).enumerate() {
            let mut row_running = 0.0f64;
            for (i, entry) in row.iter_mut().enumerate() {
                row_running += *entry as f64;
                *entry = if row_sums[j] > 0.0 {
                    (row_running / row_sums[j]) as f32
                } else {
                    (i + 1) as f32 / map.width as f32
                };
            }
            row[map.width - 1] = 1.0;
            running += row_sums[j];
            marginal_cdf.push((running / total) as f32);
        }
        marginal_cdf[map.height - 1] = 1.0;
        Some(Self { map, marginal_cdf, conditional_cdf })
    }

    pub fn map(&self) -> &EnvironmentMap {
        &self.map
    }

    fn texel(&self, d: Vec3f) -> (usize, usize) {
        let u = 0.5 + d.x.atan2(-d.z) / TAU;
        let v = d.y.clamp(-1.0, 1.0).acos() / PI;
        let i = ((u * self.map.width as f32).floor() as usize).min(self.map.width - 1);
        let j = ((v * self.map.height as f32).floor() as usize).min(self.map.height - 1);
        (i, j)
    }

    /// Nearest texel at a map-frame direction.
    pub fn radiance(&self, d: Vec3f) -> Vec3f {
        let (i, j) = self.texel(d);
        let [r, g, b] = self.map.texels[j * self.map.width + i];
        vec3f(r, g, b)
    }

    /// A map-frame direction and its solid-angle pdf, from two uniform numbers.
    pub fn sample(&self, r: (f32, f32)) -> (Vec3f, f32) {
        let (j, v_off, row_pdf) = sample_cdf(&self.marginal_cdf, r.0);
        let row = &self.conditional_cdf[j * self.map.width..(j + 1) * self.map.width];
        let (i, u_off, col_pdf) = sample_cdf(row, r.1);
        let u = (i as f32 + u_off) / self.map.width as f32;
        let v = (j as f32 + v_off) / self.map.height as f32;
        let theta = PI * v;
        let phi = TAU * (u - 0.5);
        let sin_theta = theta.sin();
        let d = vec3f(sin_theta * phi.sin(), theta.cos(), -sin_theta * phi.cos());
        (d, self.solid_angle_pdf(row_pdf * col_pdf, sin_theta))
    }

    fn solid_angle_pdf(&self, probability: f32, sin_theta: f32) -> f32 {
        if sin_theta <= 0.0 {
            return 0.0;
        }
        probability * self.map.width as f32 * self.map.height as f32 / (2.0 * PI * PI * sin_theta)
    }

    /// The solid-angle pdf `sample` draws `d` with.
    pub fn pdf(&self, d: Vec3f) -> f32 {
        let (i, j) = self.texel(d);
        let row_prev = if j == 0 { 0.0 } else { self.marginal_cdf[j - 1] };
        let index = j * self.map.width + i;
        let col_prev = if i == 0 { 0.0 } else { self.conditional_cdf[index - 1] };
        let probability = (self.marginal_cdf[j] - row_prev) * (self.conditional_cdf[index] - col_prev);
        self.solid_angle_pdf(probability, (1.0 - d.y * d.y).max(0.0).sqrt())
    }

    /// Radiance and conditional CDFs, followed by the marginal CDF.
    pub fn data_texture(&self) -> DataTex {
        let mut data = Vec::with_capacity((self.map.texels.len() + self.map.height) * 4);
        for (rgb, &cdf) in self.map.texels.iter().zip(&self.conditional_cdf) {
            data.extend([rgb[0], rgb[1], rgb[2], cdf]);
        }
        for &cdf in &self.marginal_cdf {
            data.extend([cdf, 0.0, 0.0, 0.0]);
        }
        DataTex::from_texels(data)
    }
}

fn sample_cdf(cdf: &[f32], r: f32) -> (usize, f32, f32) {
    let mut lo = 0;
    let mut hi = cdf.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if r < cdf[mid] { hi = mid } else { lo = mid + 1 }
    }
    let index = lo.min(cdf.len() - 1);
    let prev = if index == 0 { 0.0 } else { cdf[index - 1] };
    let probability = cdf[index] - prev;
    let offset = if probability > 0.0 { ((r - prev) / probability).clamp(0.0, 0.999_999_94) } else { 0.0 };
    (index, offset, probability)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu_ref::cpu_tracer;
    use crate::pack::PackedScene;
    use crate::rng::{hash2, sobol_2d, u32_to_unit};
    use crate::scene::SceneInput;
    use std::sync::Arc;

    /// The furnace (`furnace_converges_to_the_environment`) under a uniform map instead of the
    /// uniform sky: the same settings, the same exact expectation, the sky's radiance 1.
    #[test]
    fn uniform_map_matches_constant_sky() {
        let scene = SceneInput::furnace();
        let packed = PackedScene::pack(&scene);
        let light = Arc::new(EnvironmentLight::new(EnvironmentMap {
            width: 8,
            height: 4,
            texels: vec![[1.0; 3]; 8 * 4],
        }).unwrap());
        let mut t = cpu_tracer(&scene, &packed);
        t.environment = Some((light, EnvironmentFrame::default()));
        t.max_bounces = 16;
        t.max_diffuse = 16;
        t.preview_clamp = None;
        let (w, h) = (16, 16);
        let image = t.render(w, h, 256, 3);
        let mut sum = 0.0;
        for py in 4..12 {
            for px in 4..12 {
                let p = image[(py * w + px) as usize];
                sum += (p[0] + p[1] + p[2]) / 3.0;
            }
        }
        let mean = sum / 64.0;
        assert!((mean - 1.0).abs() < 0.03, "sphere mean under a uniform unit map (a constant unit sky gives 1): {mean}");
    }

    #[test]
    fn importance_sampling_resolves_a_bright_patch_with_fewer_samples() {
        let mut map = EnvironmentMap {
            width: 64,
            height: 32,
            texels: vec![[0.05; 3]; 64 * 32],
        };
        for j in 8..10 {
            for i in 30..32 {
                map.texels[j * map.width + i] = [2000.0; 3];
            }
        }
        let light = EnvironmentLight::new(map).unwrap();
        let estimate = |count: u32, importance: bool| {
            let seed = if importance { 17 } else { 29 };
            let mut mean = 0.0f64;
            let mut m2 = 0.0f64;
            for index in 0..count {
                let r = (u32_to_unit(hash2(index, seed)), u32_to_unit(hash2(index, seed + 1)));
                let (d, pdf) = if importance {
                    light.sample(r)
                } else {
                    let y = 1.0 - 2.0 * r.0;
                    let radius = (1.0 - y * y).max(0.0).sqrt();
                    let phi = TAU * (r.1 - 0.5);
                    (vec3f(radius * phi.sin(), y, -radius * phi.cos()), 1.0 / (4.0 * PI))
                };
                let value = light.radiance(d).x as f64 * d.y.max(0.0) as f64 / pdf as f64;
                let delta = value - mean;
                mean += delta / (index + 1) as f64;
                m2 += delta * (value - mean);
            }
            (mean, (m2 / (count - 1) as f64 / count as f64).sqrt())
        };
        let (importance_mean, importance_se) = estimate(1024, true);
        let (uniform_mean, uniform_se) = estimate(262144, false);
        let combined_se = importance_se.hypot(uniform_se);
        assert!(
            (importance_mean - uniform_mean).abs() <= 4.0 * combined_se,
            "irradiance estimates: importance {importance_mean} ± {importance_se}, uniform {uniform_mean} ± {uniform_se}"
        );
        assert!(
            importance_se < uniform_se,
            "standard error with 256x fewer samples: importance {importance_se}, uniform {uniform_se}"
        );
        for index in 0..64 {
            let (d, sampled_pdf) = light.sample(sobol_2d(index, 37, 0));
            let evaluated_pdf = light.pdf(d);
            assert!(
                (sampled_pdf - evaluated_pdf).abs() <= 1.0e-3 * sampled_pdf,
                "sample {index} pdf agreement: sampled {sampled_pdf}, evaluated {evaluated_pdf}"
            );
        }
    }
}
