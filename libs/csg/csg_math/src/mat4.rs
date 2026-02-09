// Mat4d - f64 4x4 matrix for CSG transforms
// Cloned from makepad Mat4f, ported to f64 and extended.
// Column-major layout matching the existing convention.

use crate::vec3::Vec3d;

#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(C)]
pub struct Mat4d {
    pub v: [f64; 16],
}

impl Default for Mat4d {
    fn default() -> Self {
        Self::identity()
    }
}

impl Mat4d {
    pub const fn identity() -> Mat4d {
        Mat4d {
            v: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    pub const fn translation(v: Vec3d) -> Mat4d {
        Mat4d {
            v: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, v.x, v.y, v.z, 1.0,
            ],
        }
    }

    pub fn scale_xyz(s: Vec3d) -> Mat4d {
        Mat4d {
            v: [
                s.x, 0.0, 0.0, 0.0, 0.0, s.y, 0.0, 0.0, 0.0, 0.0, s.z, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    pub fn scale_uniform(s: f64) -> Mat4d {
        Mat4d {
            v: [
                s, 0.0, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    /// Rotation around an arbitrary axis by angle_rad radians (Rodrigues' formula).
    pub fn rotation(axis: Vec3d, angle_rad: f64) -> Mat4d {
        let a = axis.normalize();
        let c = angle_rad.cos();
        let s = angle_rad.sin();
        let t = 1.0 - c;

        Mat4d {
            v: [
                t * a.x * a.x + c,
                t * a.x * a.y + s * a.z,
                t * a.x * a.z - s * a.y,
                0.0,
                t * a.x * a.y - s * a.z,
                t * a.y * a.y + c,
                t * a.y * a.z + s * a.x,
                0.0,
                t * a.x * a.z + s * a.y,
                t * a.y * a.z - s * a.x,
                t * a.z * a.z + c,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
            ],
        }
    }

    pub fn rotate_x(angle_rad: f64) -> Mat4d {
        let c = angle_rad.cos();
        let s = angle_rad.sin();
        Mat4d {
            v: [
                1.0, 0.0, 0.0, 0.0, 0.0, c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    pub fn rotate_y(angle_rad: f64) -> Mat4d {
        let c = angle_rad.cos();
        let s = angle_rad.sin();
        Mat4d {
            v: [
                c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    pub fn rotate_z(angle_rad: f64) -> Mat4d {
        let c = angle_rad.cos();
        let s = angle_rad.sin();
        Mat4d {
            v: [
                c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    /// Transform a point (applies translation).
    pub fn transform_point(self, p: Vec3d) -> Vec3d {
        let m = &self.v;
        Vec3d {
            x: m[0] * p.x + m[4] * p.y + m[8] * p.z + m[12],
            y: m[1] * p.x + m[5] * p.y + m[9] * p.z + m[13],
            z: m[2] * p.x + m[6] * p.y + m[10] * p.z + m[14],
        }
    }

    /// Transform a direction vector (no translation).
    pub fn transform_vector(self, v: Vec3d) -> Vec3d {
        let m = &self.v;
        Vec3d {
            x: m[0] * v.x + m[4] * v.y + m[8] * v.z,
            y: m[1] * v.x + m[5] * v.y + m[9] * v.z,
            z: m[2] * v.x + m[6] * v.y + m[10] * v.z,
        }
    }

    pub fn transpose(self) -> Mat4d {
        let v = &self.v;
        Mat4d {
            v: [
                v[0], v[4], v[8], v[12], v[1], v[5], v[9], v[13], v[2], v[6], v[10], v[14], v[3],
                v[7], v[11], v[15],
            ],
        }
    }

    pub fn mul(a: &Mat4d, b: &Mat4d) -> Mat4d {
        let a = &a.v;
        let b = &b.v;
        // column-major: element (row, col) = a[row + 4*col]
        #[inline]
        fn e(m: &[f64; 16], row: usize, col: usize) -> f64 {
            m[row + 4 * col]
        }
        let mut out = [0.0f64; 16];
        for col in 0..4 {
            for row in 0..4 {
                out[row + 4 * col] = e(a, row, 0) * e(b, 0, col)
                    + e(a, row, 1) * e(b, 1, col)
                    + e(a, row, 2) * e(b, 2, col)
                    + e(a, row, 3) * e(b, 3, col);
            }
        }
        Mat4d { v: out }
    }

    pub fn invert(self) -> Option<Mat4d> {
        let a = &self.v;
        let a00 = a[0];
        let a01 = a[1];
        let a02 = a[2];
        let a03 = a[3];
        let a10 = a[4];
        let a11 = a[5];
        let a12 = a[6];
        let a13 = a[7];
        let a20 = a[8];
        let a21 = a[9];
        let a22 = a[10];
        let a23 = a[11];
        let a30 = a[12];
        let a31 = a[13];
        let a32 = a[14];
        let a33 = a[15];

        let b00 = a00 * a11 - a01 * a10;
        let b01 = a00 * a12 - a02 * a10;
        let b02 = a00 * a13 - a03 * a10;
        let b03 = a01 * a12 - a02 * a11;
        let b04 = a01 * a13 - a03 * a11;
        let b05 = a02 * a13 - a03 * a12;
        let b06 = a20 * a31 - a21 * a30;
        let b07 = a20 * a32 - a22 * a30;
        let b08 = a20 * a33 - a23 * a30;
        let b09 = a21 * a32 - a22 * a31;
        let b10 = a21 * a33 - a23 * a31;
        let b11 = a22 * a33 - a23 * a32;

        let det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
        if det.abs() < 1e-15 {
            return None;
        }

        let idet = 1.0 / det;
        Some(Mat4d {
            v: [
                (a11 * b11 - a12 * b10 + a13 * b09) * idet,
                (a02 * b10 - a01 * b11 - a03 * b09) * idet,
                (a31 * b05 - a32 * b04 + a33 * b03) * idet,
                (a22 * b04 - a21 * b05 - a23 * b03) * idet,
                (a12 * b08 - a10 * b11 - a13 * b07) * idet,
                (a00 * b11 - a02 * b08 + a03 * b07) * idet,
                (a32 * b02 - a30 * b05 - a33 * b01) * idet,
                (a20 * b05 - a22 * b02 + a23 * b01) * idet,
                (a10 * b10 - a11 * b08 + a13 * b06) * idet,
                (a01 * b08 - a00 * b10 - a03 * b06) * idet,
                (a30 * b04 - a31 * b02 + a33 * b00) * idet,
                (a21 * b02 - a20 * b04 - a23 * b00) * idet,
                (a11 * b07 - a10 * b09 - a12 * b06) * idet,
                (a00 * b09 - a01 * b07 + a02 * b06) * idet,
                (a31 * b01 - a30 * b03 - a32 * b00) * idet,
                (a20 * b03 - a21 * b01 + a22 * b00) * idet,
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vec3::dvec3;

    #[test]
    fn test_identity_transform() {
        let p = dvec3(1.0, 2.0, 3.0);
        let m = Mat4d::identity();
        let r = m.transform_point(p);
        assert_eq!(r, p);
    }

    #[test]
    fn test_translation() {
        let p = dvec3(1.0, 2.0, 3.0);
        let m = Mat4d::translation(dvec3(10.0, 20.0, 30.0));
        let r = m.transform_point(p);
        assert_eq!(r, dvec3(11.0, 22.0, 33.0));
    }

    #[test]
    fn test_translation_does_not_affect_vectors() {
        let v = dvec3(1.0, 0.0, 0.0);
        let m = Mat4d::translation(dvec3(100.0, 200.0, 300.0));
        let r = m.transform_vector(v);
        assert_eq!(r, v);
    }

    #[test]
    fn test_scale() {
        let p = dvec3(1.0, 2.0, 3.0);
        let m = Mat4d::scale_xyz(dvec3(2.0, 3.0, 4.0));
        let r = m.transform_point(p);
        assert_eq!(r, dvec3(2.0, 6.0, 12.0));
    }

    #[test]
    fn test_rotate_z_90() {
        let p = dvec3(1.0, 0.0, 0.0);
        let m = Mat4d::rotate_z(std::f64::consts::FRAC_PI_2);
        let r = m.transform_point(p);
        assert!((r.x - 0.0).abs() < 1e-12);
        assert!((r.y - 1.0).abs() < 1e-12);
        assert!((r.z - 0.0).abs() < 1e-12);
    }

    #[test]
    fn test_invert() {
        let m = Mat4d::translation(dvec3(5.0, 10.0, 15.0));
        let inv = m.invert().unwrap();
        let p = dvec3(1.0, 2.0, 3.0);
        let r = inv.transform_point(m.transform_point(p));
        assert!((r.x - p.x).abs() < 1e-12);
        assert!((r.y - p.y).abs() < 1e-12);
        assert!((r.z - p.z).abs() < 1e-12);
    }

    #[test]
    fn test_mul_identity() {
        let m = Mat4d::translation(dvec3(1.0, 2.0, 3.0));
        let r = Mat4d::mul(&m, &Mat4d::identity());
        assert_eq!(r, m);
    }

    #[test]
    fn test_rotation_axis_angle() {
        // Rotate (1,0,0) around Z axis by 90 degrees
        let m = Mat4d::rotation(Vec3d::Z, std::f64::consts::FRAC_PI_2);
        let r = m.transform_point(dvec3(1.0, 0.0, 0.0));
        assert!((r.x - 0.0).abs() < 1e-12);
        assert!((r.y - 1.0).abs() < 1e-12);
        assert!((r.z - 0.0).abs() < 1e-12);
    }

    #[test]
    fn test_singular_matrix_no_inverse() {
        let m = Mat4d { v: [0.0; 16] };
        assert!(m.invert().is_none());
    }
}
