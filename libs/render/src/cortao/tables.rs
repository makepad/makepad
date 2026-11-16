// Source: utils/generators/OccluderLookupTableGenerator.py, CORTAO v1.0.2, commit f1b18d7772.
// MIT License
//
// Copyright (c) 2026 Daniel Isheden
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//
// Modified:
// - Translated to Rust with f64 math, checked dimensions, and 8..=32 rays.
// - Generate vectors at runtime; omit file/DDS output and console messages.
// - Encode masks as exact floats or pairs of 16-bit float halves for textures.
// - Add property tests. The upstream generator's header still says v1.0.1.

//! Runtime occluder lookup tables with the reference generator's texel order.

use std::f64::consts::PI;

/// CORTAO's golden-angle spiral ray directions, cosine distributed over +z.
/// Panics unless `ray_count` is in 8..=32 and `index < ray_count`.
pub fn ray_direction(index: u32, ray_count: u32) -> [f64; 3] {
    assert!((8..=32).contains(&ray_count), "ray count must be in 8..=32");
    assert!(index < ray_count, "ray index must be below ray count");
    let golden_angle = PI * (3.0 - 5.0_f64.sqrt());
    let angle = index as f64 * golden_angle;
    let radius = ((index as f64 + 0.5) / ray_count as f64).sqrt();
    let x = angle.cos() * radius;
    let y = angle.sin() * radius;
    // Keep the generator's x*x + y*y evaluation, rather than radius*radius.
    let z = (1.0 - x * x - y * y).sqrt();
    [x, y, z]
}

fn decode_sphere_map(x: f64, y: f64) -> [f64; 3] {
    let ex = x * 2.0 - 1.0;
    let ey = y * 2.0 - 1.0;
    let z = (1.0 - ex * ex - ey * ey).max(0.0);
    let mul = 2.0 * z.sqrt();
    [ex * mul, ey * mul, z * 2.0 - 1.0]
}

/// The occluder lookup table: radius slice z, row y, then column x (x fastest,
/// matching Python's output order). A bit marks a ray covered by that texel's
/// tangent-space direction and angular radius, including the closest-texel fix-up.
///
/// Panics for zero dimensions, a size that cannot fit a vector, or a ray count
/// outside 8..=32.
pub fn occluder_table(direction_res: u32, radius_res: u32, ray_count: u32) -> Vec<u32> {
    assert!(direction_res > 0, "direction resolution must be positive");
    assert!(radius_res > 0, "radius resolution must be positive");
    assert!((8..=32).contains(&ray_count), "ray count must be in 8..=32");
    let direction_size = direction_res as usize;
    let len = direction_size
        .checked_mul(direction_size)
        .and_then(|size| size.checked_mul(radius_res as usize))
        .filter(|&size| size <= isize::MAX as usize / std::mem::size_of::<u32>())
        .expect("occluder table dimensions overflow vector capacity");
    let max_occluder_radius = 2.0_f64.sqrt();
    let rays: Vec<_> = (0..ray_count)
        .map(|index| ray_direction(index, ray_count))
        .collect();
    let mut output = Vec::with_capacity(len);
    for z in 0..radius_res {
        let occluder_radius = max_occluder_radius * (z as f64 + 0.5) / radius_res as f64;
        let occluder_cos_angle = 1.0 - occluder_radius * occluder_radius * 0.5;
        let mut closest_pixel = [(-1_i64, -1_i64, -1.0_f64); 32];
        let offset = output.len();
        for y in 0..direction_res {
            for x in 0..direction_res {
                let direction = decode_sphere_map(
                    (x as f64 + 0.5) / direction_res as f64,
                    (y as f64 + 0.5) / direction_res as f64,
                );
                let mut ray_bits = 0_u32;
                if direction[2] != -1.0 {
                    for i in 0..ray_count as usize {
                        let ray = rays[i];
                        let cos_angle =
                            direction[0] * ray[0] + direction[1] * ray[1] + direction[2] * ray[2];
                        if cos_angle > closest_pixel[i].2 {
                            closest_pixel[i] = (x as i64, y as i64, cos_angle);
                        }
                        if cos_angle > occluder_cos_angle {
                            ray_bits |= 1_u32 << i;
                        }
                    }
                }
                output.push(ray_bits);
            }
        }
        for i in 0..ray_count as usize {
            let (x, y, _) = closest_pixel[i];
            // Every positive resolution includes a texel inside the disc.
            debug_assert!(x >= 0 && y >= 0);
            let index = y as usize * direction_size + x as usize;
            output[offset + index] |= 1_u32 << i;
        }
    }
    output
}

/// A `direction_res` by `direction_res * radius_res` R32F texture: slice z
/// occupies rows starting at `z * direction_res`; texels are exact numeric masks.
/// Panics above 16 rays; use [`occluder_table_halves`] for larger masks.
pub fn occluder_table_rf32(direction_res: u32, radius_res: u32, ray_count: u32) -> Vec<f32> {
    assert!(
        ray_count <= 16,
        "R32F numeric masks support at most 16 rays"
    );
    occluder_table(direction_res, radius_res, ray_count)
        .into_iter()
        .map(|mask| mask as f32)
        .collect()
}

/// The same texel order as [`occluder_table`], with each mask split into exact
/// numeric `(lo16, hi16)` floats. Supports all ray counts in 8..=32.
pub fn occluder_table_halves(direction_res: u32, radius_res: u32, ray_count: u32) -> Vec<[f32; 2]> {
    occluder_table(direction_res, radius_res, ray_count)
        .into_iter()
        .map(|mask| [(mask & 0xffff) as f32, (mask >> 16) as f32])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_grows_with_radius_and_stays_inside_the_disc() {
        let direction_res = 24;
        let radius_res = 16;
        let slice_len = (direction_res * direction_res) as usize;
        for ray_count in [8, 16, 32] {
            let table = occluder_table(direction_res, radius_res, ray_count);
            let all_rays = u32::MAX >> (32 - ray_count);
            for (z, slice) in table.chunks_exact(slice_len).enumerate() {
                assert_eq!(
                    slice.iter().fold(0, |bits, &mask| bits | mask),
                    all_rays,
                    "every ray must occur in slice {z} with {ray_count} rays"
                );
                for y in 0..direction_res {
                    for x in 0..direction_res {
                        let index = (y * direction_res + x) as usize;
                        let direction = decode_sphere_map(
                            (x as f64 + 0.5) / direction_res as f64,
                            (y as f64 + 0.5) / direction_res as f64,
                        );
                        if direction[2] == -1.0 {
                            assert_eq!(slice[index], 0, "outside-disc texel at {x}, {y}, {z}");
                        }
                        if z + 1 < radius_res as usize {
                            let next = table[(z + 1) * slice_len + index];
                            assert_eq!(slice[index] & !next, 0, "radius must only add rays");
                        }
                    }
                }
            }
            let last = &table[table.len() - slice_len..];
            // Their union covers the hemisphere; individual centre texels can
            // miss grazing rays because the last radius is sampled below sqrt(2).
            let mut centre_coverage = 0;
            for y in [11, 12] {
                for x in [11, 12] {
                    centre_coverage |= last[y * 24 + x];
                }
            }
            assert_eq!(centre_coverage, all_rays, "last slice centre coverage");
        }
    }

    #[test]
    fn texture_encodings_preserve_masks_and_rays_are_unit_vectors() {
        for ray_count in 8..=32 {
            for i in 0..ray_count {
                let [x, y, z] = ray_direction(i, ray_count);
                assert!((x * x + y * y + z * z - 1.0).abs() < 1e-14);
                assert!(z > 0.0, "rays must stay in the positive hemisphere");
            }
            // Small tables also exercise the closest-texel fix-up.
            let masks = occluder_table(5, 3, ray_count);
            let halves = occluder_table_halves(5, 3, ray_count);
            for (&mask, &[lo, hi]) in masks.iter().zip(&halves) {
                assert_eq!(lo.fract(), 0.0);
                assert_eq!(hi.fract(), 0.0);
                assert_eq!(lo as u32 | ((hi as u32) << 16), mask);
            }
            if ray_count <= 16 {
                let floats = occluder_table_rf32(5, 3, ray_count);
                for (&mask, value) in masks.iter().zip(floats) {
                    assert_eq!(value.fract(), 0.0);
                    assert_eq!(value as u32, mask);
                }
            }
        }
        assert_eq!(occluder_table(1, 1, 32), vec![u32::MAX]);
    }

    #[test]
    #[should_panic(expected = "at most 16 rays")]
    fn single_float_texture_rejects_large_masks() {
        occluder_table_rf32(1, 1, 32);
    }
}
