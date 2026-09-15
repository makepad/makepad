//! Pixel-aligned conditioning for Pixal3D. Coordinates are in the model's
//! Y-up grid; camera projection follows Tencent's ProjGrid. Sampling uses
//! border padding and align_corners=false, including the half-pixel offset.
//!
//! See THIRD_PARTY_NOTICES.md for source provenance. This module does not
//! execute Python or depend on ComfyUI.

use crate::{DiffusionError, Result};

#[derive(Clone, Copy, Debug)]
pub struct PixalCamera {
    pub fov_degrees: f32,
    pub distance: f32,
    pub mesh_scale: f32,
}

impl PixalCamera {
    /// ComfyUI's front camera, framing the unit reconstruction volume.
    pub fn from_fov(fov_degrees: f32) -> Result<Self> {
        let camera = Self {
            fov_degrees,
            distance: 0.5 / (fov_degrees.to_radians() * 0.5).tan(),
            mesh_scale: 1.0,
        };
        camera.validate()?;
        Ok(camera)
    }

    pub fn validate(self) -> Result<()> {
        if !self.fov_degrees.is_finite()
            || !(1.0..=170.0).contains(&self.fov_degrees)
            || !self.distance.is_finite()
            || self.distance <= 0.0
            || !self.mesh_scale.is_finite()
            || self.mesh_scale <= 0.0
        {
            return Err(DiffusionError::workflow("invalid Pixal3D camera"));
        }
        Ok(())
    }

    /// Normalized image coordinates before border clamping. Do not mask
    /// out-of-frame or behind-camera samples: the reference clamps them.
    pub fn project(
        self,
        coords: &[[i32; 3]],
        grid_resolution: usize,
        image_resolution: usize,
    ) -> Result<Vec<[f32; 2]>> {
        self.validate()?;
        if grid_resolution == 0 || image_resolution == 0 {
            return Err(DiffusionError::workflow("empty Pixal3D projection grid"));
        }
        let focal = 0.5 / (self.fov_degrees.to_radians() * 0.5).tan();
        let mut uv = Vec::with_capacity(coords.len());
        for coord in coords {
            if coord
                .iter()
                .any(|&x| x < 0 || x as usize >= grid_resolution)
            {
                return Err(DiffusionError::workflow("Pixal3D coordinate outside grid"));
            }
            let p = coord.map(|x| {
                if grid_resolution == 1 {
                    0.0
                } else {
                    (x as f32 / (grid_resolution - 1) as f32 * 2.0 - 1.0) / self.mesh_scale / 2.0
                }
            });
            // Grid rotation (x,-z,y) then inverse front camera: (x,y,z-d).
            let depth = self.distance - p[2] + 1e-8;
            let pixel_center = 0.5 + 0.5 / image_resolution as f32;
            let point = [
                focal * p[0] / depth + pixel_center,
                -focal * p[1] / depth + pixel_center,
            ];
            if point.iter().any(|v| !v.is_finite()) {
                return Err(DiffusionError::workflow("non-finite Pixal3D projection"));
            }
            uv.push(point);
        }
        Ok(uv)
    }
}

/// Pixal3D quantizes to grid endpoints, using ties-to-even rounding.
/// TRELLIS.2 uses cell centers and floor; these must remain separate.
pub fn pixal_quantize_unique_coords(
    coords: &[[i32; 3]],
    source_resolution: usize,
    target_resolution: usize,
) -> Result<Vec<[i32; 3]>> {
    if source_resolution == 0 || target_resolution < 16 || target_resolution % 16 != 0 {
        return Err(DiffusionError::workflow(
            "invalid Pixal3D cascade resolution",
        ));
    }
    let grid = target_resolution / 16;
    let factor = (grid - 1) as f32 / source_resolution as f32;
    let mut out = Vec::with_capacity(coords.len());
    for coord in coords {
        if coord
            .iter()
            .any(|&x| x < 0 || x as usize >= source_resolution)
        {
            return Err(DiffusionError::workflow(
                "Pixal3D cascade coordinate outside grid",
            ));
        }
        out.push(coord.map(|x| ((x as f32 + 0.5) * factor).round_ties_even() as i32));
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Four bilinear taps in a row-major image. Weights include border clamping.
pub fn bilinear_taps(uv: [f32; 2], width: usize, height: usize) -> Result<[(usize, f32); 4]> {
    if width == 0 || height == 0 || uv.iter().any(|v| !v.is_finite()) {
        return Err(DiffusionError::workflow("invalid Pixal3D feature sample"));
    }
    let x = (uv[0] * width as f32 - 0.5).clamp(0.0, (width - 1) as f32);
    let y = (uv[1] * height as f32 - 0.5).clamp(0.0, (height - 1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let dx = x - x0 as f32;
    let dy = y - y0 as f32;
    Ok([
        (y0 * width + x0, (1.0 - dx) * (1.0 - dy)),
        (y0 * width + x1, dx * (1.0 - dy)),
        (y1 * width + x0, (1.0 - dx) * dy),
        (y1 * width + x1, dx * dy),
    ])
}

/// Sample a token-major image feature map directly at sparse coordinates.
/// Memory is proportional to active voxels, with no dense 3D feature grid.
pub fn sample_features(
    features: &[f32],
    width: usize,
    height: usize,
    channels: usize,
    uv: &[[f32; 2]],
) -> Result<Vec<f32>> {
    let expected = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(channels));
    if channels == 0 || expected != Some(features.len()) {
        return Err(DiffusionError::workflow("Pixal3D feature shape mismatch"));
    }
    let len = uv
        .len()
        .checked_mul(channels)
        .ok_or_else(|| DiffusionError::workflow("Pixal3D sample size overflow"))?;
    let mut out = vec![0.0; len];
    for (point, row) in uv.iter().zip(out.chunks_exact_mut(channels)) {
        for (index, weight) in bilinear_taps(*point, width, height)? {
            let source = &features[index * channels..(index + 1) * channels];
            for (dst, src) in row.iter_mut().zip(source) {
                *dst += weight * src;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_preserves_half_pixel_and_y_up_frame() {
        let camera = PixalCamera::from_fov(49.13).unwrap();
        let uv = camera
            .project(&[[1, 1, 1], [0, 1, 1], [2, 1, 1], [1, 2, 1]], 3, 512)
            .unwrap();
        let center = 0.5 + 0.5 / 512.0;
        assert_eq!(uv[0], [center, center]);
        assert!((uv[1][0] - (center - 0.5)).abs() < 1e-6);
        assert!((uv[2][0] - (center + 0.5)).abs() < 1e-6);
        assert!(uv[3][1] < center);
        assert!(PixalCamera::from_fov(f32::NAN).is_err());
        assert!(camera.project(&[[3, 0, 0]], 3, 512).is_err());
    }

    #[test]
    fn bilinear_matches_border_and_align_corners_false() {
        let values = [0.0, 10.0, 20.0, 30.0];
        let out = sample_features(
            &values,
            2,
            2,
            1,
            &[[0.5, 0.5], [-3.0, 0.25], [3.0, 0.75], [0.5, 0.25]],
        )
        .unwrap();
        assert_eq!(out, [15.0, 0.0, 30.0, 5.0]);
        assert!(sample_features(&values, 2, 2, 1, &[[f32::NAN, 0.0]]).is_err());
    }

    #[test]
    fn cascade_rounds_ties_even_and_deduplicates() {
        let out = pixal_quantize_unique_coords(&[[0, 1, 2], [1, 1, 2], [2, 1, 0]], 3, 64).unwrap();
        assert_eq!(out, [[0, 2, 2], [2, 2, 0], [2, 2, 2]]);
        assert!(pixal_quantize_unique_coords(&[[3, 0, 0]], 3, 64).is_err());
    }
}
