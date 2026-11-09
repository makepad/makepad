//! OpenCV cameras: x right, y down, z forward; pixel (x, y) has centre (x, y).
//! Intrinsics are [fx, fy, cx, cy]. Camera-to-world matrices are row-major rigid
//! transforms in metres. Depth is camera z. Unit normals are in the reference
//! camera frame and face it: n dot K^-1 [x, y, 1] < 0. Images are row-major gray
//! values in [0, 1].

/// A calibrated grayscale image and its rigid camera-to-world pose.
pub struct GrayView {
    /// Number of pixel columns.
    pub width: usize,
    /// Number of pixel rows.
    pub height: usize,
    /// Row-major grayscale values in [0, 1].
    pub pixels: Vec<f32>,
    /// Intrinsics [fx, fy, cx, cy], in pixels.
    pub k: [f64; 4],
    /// Row-major rigid camera-to-world transform, with translation in metres.
    pub cam_to_world: [[f64; 4]; 4],
}

impl GrayView {
    /// Validate positive dimensions and focal lengths, pixel count and range,
    /// and finite values. The rotation must be orthonormal with determinant +1
    /// within 1e-6; the last transform row must be exactly [0, 0, 0, 1].
    pub fn check(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0
            || self.width.checked_mul(self.height) != Some(self.pixels.len())
        {
            return Err("image dimensions do not match the pixel buffer".into());
        }
        if self.pixels.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) {
            return Err("pixels must be finite and in [0, 1]".into());
        }
        if self.k.iter().any(|v| !v.is_finite()) || self.k[0] <= 0.0 || self.k[1] <= 0.0 {
            return Err("intrinsics must be finite with positive focal lengths".into());
        }
        let m = &self.cam_to_world;
        if m.iter().flatten().any(|v| !v.is_finite()) || m[3] != [0.0, 0.0, 0.0, 1.0] {
            return Err("pose must be finite with last row [0, 0, 0, 1]".into());
        }
        for i in 0..3 {
            for j in 0..3 {
                let dot: f64 = (0..3).map(|k| m[k][i] * m[k][j]).sum();
                let expected = if i == j { 1.0 } else { 0.0 };
                if (dot - expected).abs() > 1e-6 {
                    return Err("pose rotation must be orthonormal".into());
                }
            }
        }
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if (det - 1.0).abs() > 1e-6 {
            return Err("pose rotation must have determinant +1".into());
        }
        Ok(())
    }

    /// Bilinear sampling at a real pixel position. Returns None outside the
    /// closed image rectangle, for nonfinite positions, or missing pixels.
    pub fn sample(&self, x: f64, y: f64) -> Option<f32> {
        if self.width == 0 || self.height == 0 || !x.is_finite() || !y.is_finite()
            || x < 0.0 || y < 0.0 || x > (self.width - 1) as f64 || y > (self.height - 1) as f64
        {
            return None;
        }
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = x0.saturating_add(1).min(self.width - 1);
        let y1 = y0.saturating_add(1).min(self.height - 1);
        let pixel = |px: usize, py: usize| {
            self.pixels.get(py.checked_mul(self.width)?.checked_add(px)?).copied().map(f64::from)
        };
        let tx = x - x0 as f64;
        let ty = y - y0 as f64;
        let a = pixel(x0, y0)? * (1.0 - tx) + pixel(x1, y0)? * tx;
        let b = pixel(x0, y1)? * (1.0 - tx) + pixel(x1, y1)? * tx;
        Some((a * (1.0 - ty) + b * ty) as f32)
    }
}

/// Transform reference-camera points to source-camera points: X_s = R X_r + t,
/// where R = R_s^T R_r and t = R_s^T (c_r - c_s), all in f64.
pub fn relative_pose(reference: &GrayView, source: &GrayView) -> ([[f64; 3]; 3], [f64; 3]) {
    let r = &reference.cam_to_world;
    let s = &source.cam_to_world;
    let mut rotation = [[0.0; 3]; 3];
    let mut translation = [0.0; 3];
    for i in 0..3 {
        for j in 0..3 {
            rotation[i][j] = (0..3).map(|k| s[k][i] * r[k][j]).sum();
        }
        translation[i] = (0..3).map(|k| s[k][i] * (r[k][3] - s[k][3])).sum();
    }
    (rotation, translation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_view() -> GrayView {
        GrayView {
            width: 2, height: 2, pixels: vec![0.0, 0.25, 0.5, 0.75],
            k: [120.0, 120.0, 0.5, 0.5],
            cam_to_world: [[1.0, 0.0, 0.0, 0.2], [0.0, 1.0, 0.0, -0.1],
                [0.0, 0.0, 1.0, 0.3], [0.0, 0.0, 0.0, 1.0]],
        }
    }

    #[test]
    fn relative_pose_maps_the_same_world_point_between_cameras() {
        let reference = reference_view();
        let angle = 0.3_f64;
        let source = GrayView {
            width: 1, height: 1, pixels: vec![0.5], k: reference.k,
            cam_to_world: [[angle.cos(), 0.0, angle.sin(), -0.2], [0.0, 1.0, 0.0, 0.4],
                [-angle.sin(), 0.0, angle.cos(), -0.1], [0.0, 0.0, 0.0, 1.0]],
        };
        assert!(reference.check().is_ok());
        assert!(source.check().is_ok());
        let world = [0.1, 0.2, 1.3];
        let camera_point = |view: &GrayView| -> [f64; 3] {
            std::array::from_fn(|i| (0..3).map(|j|
                view.cam_to_world[j][i] * (world[j] - view.cam_to_world[j][3])).sum())
        };
        let a = camera_point(&reference);
        let b = camera_point(&source);
        let (r, t) = relative_pose(&reference, &source);
        for i in 0..3 {
            let mapped = t[i] + (0..3).map(|j| r[i][j] * a[j]).sum::<f64>();
            assert!((mapped - b[i]).abs() < 1e-12, "relative pose component {i}");
        }
    }

    #[test]
    fn sample_is_exact_at_pixels_and_bilinear_between_them() {
        let reference = reference_view();
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(reference.sample(x as f64, y as f64), Some(reference.pixels[y * 2 + x]));
            }
        }
        assert_eq!(reference.sample(0.5, 0.5), Some(0.375));
        assert_eq!(reference.sample(0.25, 0.75), Some(0.4375));
        for (x, y) in [(-0.1, 0.0), (0.0, -0.1), (1.1, 0.0), (0.0, 1.1), (f64::NAN, 0.0)] {
            assert_eq!(reference.sample(x, y), None);
        }
    }

    #[test]
    fn check_rejects_reflections_short_buffers_and_invalid_camera_data() {
        let mut reference = reference_view();
        assert!(reference.check().is_ok());
        reference.cam_to_world[0][0] = -1.0;
        assert!(reference.check().is_err(), "reflection must fail");
        reference.cam_to_world[0][0] = 1.0;
        reference.pixels.pop();
        assert!(reference.check().is_err(), "short buffer must fail");
        reference.pixels.push(0.75);
        reference.k[0] = 0.0;
        assert!(reference.check().is_err(), "zero focal length must fail");
        reference.k[0] = f64::NAN;
        assert!(reference.check().is_err(), "nonfinite intrinsics must fail");
        reference.k[0] = 120.0;
        reference.cam_to_world[0][0] = 1.01;
        assert!(reference.check().is_err(), "nonrigid rotation must fail");
        reference.cam_to_world[0][0] = 1.0;
        reference.cam_to_world[3][0] = 0.01;
        assert!(reference.check().is_err(), "nonaffine last row must fail");
        reference.cam_to_world[3][0] = 0.0;
        reference.cam_to_world[0][3] = f64::INFINITY;
        assert!(reference.check().is_err(), "nonfinite pose must fail");
        reference.cam_to_world[0][3] = 0.0;
        reference.pixels[0] = f32::NAN;
        assert!(reference.check().is_err(), "nonfinite pixel must fail");
        reference.pixels[0] = 1.1;
        assert!(reference.check().is_err(), "pixel range must fail");
        reference.width = 0;
        assert!(reference.check().is_err(), "empty image must fail");
        reference.width = usize::MAX;
        assert!(reference.check().is_err(), "dimension overflow must fail");
    }
}
