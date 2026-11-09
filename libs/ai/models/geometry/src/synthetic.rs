//! Deterministic textured geometry for comparisons and examples.

use crate::{patchmatch::DepthNormalMap, views::GrayView};

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn ray(view: &GrayView, x: usize, y: usize) -> [f64; 3] {
    [(x as f64 - view.k[2]) / view.k[0], (y as f64 - view.k[3]) / view.k[1], 1.0]
}

fn normalise(v: [f64; 3]) -> [f64; 3] {
    let length = dot(v, v).sqrt();
    v.map(|x| x / length)
}

fn noise(x: i64, y: i64) -> f64 {
    let mut h = (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ (y as u64).wrapping_mul(0xbf58476d1ce4e5b9);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
    h ^= h >> 31;
    0.1 + 0.8 * (h >> 11) as f64 / 9007199254740992.0
}

fn texture(u: f64, v: f64) -> f32 {
    let x = u.floor() as i64;
    let y = v.floor() as i64;
    let tx = u - x as f64;
    let ty = v - y as f64;
    ((noise(x, y) * (1.0 - tx) + noise(x + 1, y) * tx) * (1.0 - ty)
        + (noise(x, y + 1) * (1.0 - tx) + noise(x + 1, y + 1) * tx) * ty) as f32
}

/// Camera-z depths, in row-major order, of the plane rendered by [`textured_plane`].
/// The plane passes through world (0, 0, 0.5) with normal (sin(angle), 0, cos(angle)).
/// Intersects each unnormalised camera ray, transformed by the view's rigid pose,
/// with that plane. The view and angle must be valid; parallel rays yield nonfinite
/// depths and intersections behind the camera yield negative depths.
pub fn plane_depth(view: &GrayView, angle: f64) -> Vec<f64> {
    let normal = [angle.sin(), 0.0, angle.cos()];
    let pose = &view.cam_to_world;
    let origin = [pose[0][3], pose[1][3], pose[2][3]];
    let numerator = 0.5 * normal[2] - dot(normal, origin);
    let mut depths = Vec::with_capacity(view.width * view.height);
    for y in 0..view.height {
        for x in 0..view.width {
            let local = ray(view, x, y);
            let direction = std::array::from_fn(|i| {
                pose[i][0] * local[0] + pose[i][1] * local[1] + pose[i][2]
            });
            depths.push(numerator / dot(normal, direction));
        }
    }
    depths
}

/// Render a textured plane through (0, 0, 0.5), with cameras looking at its centre.
/// The reference is at x=0; source cameras use the supplied x offsets.
pub fn textured_plane(size: usize, focal: f64, angle: f64, source_offsets: &[f64]) -> (Vec<GrayView>, DepthNormalMap) {
    let plane_normal = [angle.sin(), 0.0, angle.cos()];
    let tangent = [angle.cos(), 0.0, -angle.sin()];
    let mut views = Vec::new();
    let mut truth = DepthNormalMap { width: size, height: size,
        depth: Vec::new(), normal: vec![plane_normal.map(|v| -v); size * size],
        cost: vec![2.0; size * size] };
    for (index, cx) in std::iter::once(0.0).chain(source_offsets.iter().copied()).enumerate() {
        let forward = normalise([-cx, 0.0, 0.5]);
        let right = [forward[2], 0.0, -forward[0]];
        let mut view = GrayView { width: size, height: size, pixels: Vec::new(),
            k: [focal, focal, (size as f64 - 1.0) * 0.5, (size as f64 - 1.0) * 0.5],
            cam_to_world: [[right[0], 0.0, forward[0], cx], [0.0, 1.0, 0.0, 0.0],
                [right[2], 0.0, forward[2], 0.0], [0.0, 0.0, 0.0, 1.0]] };
        for y in 0..size {
            for x in 0..size {
                let local = ray(&view, x, y);
                let direction = [right[0] * local[0] + forward[0], local[1],
                    right[2] * local[0] + forward[2]];
                let distance = (0.5 * plane_normal[2] - cx * plane_normal[0])
                    / dot(plane_normal, direction);
                let point = [cx + distance * direction[0], distance * direction[1],
                    distance * direction[2] - 0.5];
                view.pixels.push(texture(dot(point, tangent) / 0.008, point[1] / 0.008));
                if index == 0 { truth.depth.push(distance); }
            }
        }
        views.push(view);
    }
    (views, truth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_depth_matches_rendered_reference_truth() {
        for degrees in [0.0_f64, 30.0] {
            let angle = degrees.to_radians();
            let (views, truth) = textured_plane(96, 120.0, angle, &[-0.10, 0.10, 0.05]);
            let depths = plane_depth(&views[0], angle);
            assert_eq!(depths.len(), truth.depth.len(), "depth count");
            for (i, (&depth, &expected)) in depths.iter().zip(&truth.depth).enumerate() {
                assert!((depth / expected - 1.0).abs() <= 1e-12,
                    "{degrees} degree reference depth at {i}: {depth} vs {expected}");
            }
        }
    }
}
