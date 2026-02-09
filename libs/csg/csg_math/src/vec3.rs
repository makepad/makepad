// Vec3d - f64 3D vector for CSG operations
// Cloned from makepad math library, extended with full API for CSG use.
// Will merge back into libs/math when complete.

use std::{fmt, ops};

#[derive(Clone, Copy, Default, Debug, PartialEq)]
#[repr(C)]
pub struct Vec3d {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

pub const fn dvec3(x: f64, y: f64, z: f64) -> Vec3d {
    Vec3d { x, y, z }
}

impl Vec3d {
    pub const ZERO: Vec3d = Vec3d {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const ONE: Vec3d = Vec3d {
        x: 1.0,
        y: 1.0,
        z: 1.0,
    };
    pub const X: Vec3d = Vec3d {
        x: 1.0,
        y: 0.0,
        z: 0.0,
    };
    pub const Y: Vec3d = Vec3d {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };
    pub const Z: Vec3d = Vec3d {
        x: 0.0,
        y: 0.0,
        z: 1.0,
    };

    pub const fn new(x: f64, y: f64, z: f64) -> Vec3d {
        Vec3d { x, y, z }
    }

    pub const fn all(v: f64) -> Vec3d {
        Vec3d { x: v, y: v, z: v }
    }

    pub fn dot(self, other: Vec3d) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(self, other: Vec3d) -> Vec3d {
        Vec3d {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    pub fn length_sq(self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    pub fn length(self) -> f64 {
        self.length_sq().sqrt()
    }

    pub fn normalize(self) -> Vec3d {
        let len_sq = self.length_sq();
        if len_sq > 0.0 {
            let inv = 1.0 / len_sq.sqrt();
            Vec3d {
                x: self.x * inv,
                y: self.y * inv,
                z: self.z * inv,
            }
        } else {
            Vec3d::ZERO
        }
    }

    pub fn distance(self, other: Vec3d) -> f64 {
        (self - other).length()
    }

    pub fn distance_sq(self, other: Vec3d) -> f64 {
        (self - other).length_sq()
    }

    pub fn lerp(self, other: Vec3d, t: f64) -> Vec3d {
        Vec3d {
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
            z: self.z + (other.z - self.z) * t,
        }
    }

    pub fn min(self, other: Vec3d) -> Vec3d {
        Vec3d {
            x: self.x.min(other.x),
            y: self.y.min(other.y),
            z: self.z.min(other.z),
        }
    }

    pub fn max(self, other: Vec3d) -> Vec3d {
        Vec3d {
            x: self.x.max(other.x),
            y: self.y.max(other.y),
            z: self.z.max(other.z),
        }
    }

    pub fn abs(self) -> Vec3d {
        Vec3d {
            x: self.x.abs(),
            y: self.y.abs(),
            z: self.z.abs(),
        }
    }

    /// Index of the component with the largest absolute value
    pub fn dominant_axis(self) -> usize {
        let a = self.abs();
        if a.x >= a.y && a.x >= a.z {
            0
        } else if a.y >= a.z {
            1
        } else {
            2
        }
    }

    pub fn component(self, i: usize) -> f64 {
        match i {
            0 => self.x,
            1 => self.y,
            2 => self.z,
            _ => panic!("Vec3d::component index out of range"),
        }
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

impl fmt::Display for Vec3d {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "dvec3({}, {}, {})", self.x, self.y, self.z)
    }
}

// Vec3d op Vec3d
impl ops::Add<Vec3d> for Vec3d {
    type Output = Vec3d;
    fn add(self, rhs: Vec3d) -> Vec3d {
        Vec3d {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl ops::Sub<Vec3d> for Vec3d {
    type Output = Vec3d;
    fn sub(self, rhs: Vec3d) -> Vec3d {
        Vec3d {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

impl ops::Mul<Vec3d> for Vec3d {
    type Output = Vec3d;
    fn mul(self, rhs: Vec3d) -> Vec3d {
        Vec3d {
            x: self.x * rhs.x,
            y: self.y * rhs.y,
            z: self.z * rhs.z,
        }
    }
}

impl ops::Div<Vec3d> for Vec3d {
    type Output = Vec3d;
    fn div(self, rhs: Vec3d) -> Vec3d {
        Vec3d {
            x: self.x / rhs.x,
            y: self.y / rhs.y,
            z: self.z / rhs.z,
        }
    }
}

// Vec3d op f64
impl ops::Add<f64> for Vec3d {
    type Output = Vec3d;
    fn add(self, rhs: f64) -> Vec3d {
        Vec3d {
            x: self.x + rhs,
            y: self.y + rhs,
            z: self.z + rhs,
        }
    }
}

impl ops::Sub<f64> for Vec3d {
    type Output = Vec3d;
    fn sub(self, rhs: f64) -> Vec3d {
        Vec3d {
            x: self.x - rhs,
            y: self.y - rhs,
            z: self.z - rhs,
        }
    }
}

impl ops::Mul<f64> for Vec3d {
    type Output = Vec3d;
    fn mul(self, rhs: f64) -> Vec3d {
        Vec3d {
            x: self.x * rhs,
            y: self.y * rhs,
            z: self.z * rhs,
        }
    }
}

impl ops::Div<f64> for Vec3d {
    type Output = Vec3d;
    fn div(self, rhs: f64) -> Vec3d {
        Vec3d {
            x: self.x / rhs,
            y: self.y / rhs,
            z: self.z / rhs,
        }
    }
}

// f64 op Vec3d
impl ops::Add<Vec3d> for f64 {
    type Output = Vec3d;
    fn add(self, rhs: Vec3d) -> Vec3d {
        Vec3d {
            x: self + rhs.x,
            y: self + rhs.y,
            z: self + rhs.z,
        }
    }
}

impl ops::Sub<Vec3d> for f64 {
    type Output = Vec3d;
    fn sub(self, rhs: Vec3d) -> Vec3d {
        Vec3d {
            x: self - rhs.x,
            y: self - rhs.y,
            z: self - rhs.z,
        }
    }
}

impl ops::Mul<Vec3d> for f64 {
    type Output = Vec3d;
    fn mul(self, rhs: Vec3d) -> Vec3d {
        Vec3d {
            x: self * rhs.x,
            y: self * rhs.y,
            z: self * rhs.z,
        }
    }
}

impl ops::Div<Vec3d> for f64 {
    type Output = Vec3d;
    fn div(self, rhs: Vec3d) -> Vec3d {
        Vec3d {
            x: self / rhs.x,
            y: self / rhs.y,
            z: self / rhs.z,
        }
    }
}

// Assign operators
impl ops::AddAssign<Vec3d> for Vec3d {
    fn add_assign(&mut self, rhs: Vec3d) {
        self.x += rhs.x;
        self.y += rhs.y;
        self.z += rhs.z;
    }
}
impl ops::SubAssign<Vec3d> for Vec3d {
    fn sub_assign(&mut self, rhs: Vec3d) {
        self.x -= rhs.x;
        self.y -= rhs.y;
        self.z -= rhs.z;
    }
}
impl ops::MulAssign<Vec3d> for Vec3d {
    fn mul_assign(&mut self, rhs: Vec3d) {
        self.x *= rhs.x;
        self.y *= rhs.y;
        self.z *= rhs.z;
    }
}
impl ops::DivAssign<Vec3d> for Vec3d {
    fn div_assign(&mut self, rhs: Vec3d) {
        self.x /= rhs.x;
        self.y /= rhs.y;
        self.z /= rhs.z;
    }
}
impl ops::AddAssign<f64> for Vec3d {
    fn add_assign(&mut self, rhs: f64) {
        self.x += rhs;
        self.y += rhs;
        self.z += rhs;
    }
}
impl ops::SubAssign<f64> for Vec3d {
    fn sub_assign(&mut self, rhs: f64) {
        self.x -= rhs;
        self.y -= rhs;
        self.z -= rhs;
    }
}
impl ops::MulAssign<f64> for Vec3d {
    fn mul_assign(&mut self, rhs: f64) {
        self.x *= rhs;
        self.y *= rhs;
        self.z *= rhs;
    }
}
impl ops::DivAssign<f64> for Vec3d {
    fn div_assign(&mut self, rhs: f64) {
        self.x /= rhs;
        self.y /= rhs;
        self.z /= rhs;
    }
}

impl ops::Neg for Vec3d {
    type Output = Vec3d;
    fn neg(self) -> Vec3d {
        Vec3d {
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dot() {
        let a = dvec3(1.0, 2.0, 3.0);
        let b = dvec3(4.0, 5.0, 6.0);
        assert_eq!(a.dot(b), 32.0);
    }

    #[test]
    fn test_cross() {
        let a = Vec3d::X;
        let b = Vec3d::Y;
        let c = a.cross(b);
        assert_eq!(c.x, 0.0);
        assert_eq!(c.y, 0.0);
        assert_eq!(c.z, 1.0);

        // anti-commutativity
        let d = b.cross(a);
        assert_eq!(d.x, 0.0);
        assert_eq!(d.y, 0.0);
        assert_eq!(d.z, -1.0);
    }

    #[test]
    fn test_length_normalize() {
        let v = dvec3(3.0, 4.0, 0.0);
        assert!((v.length() - 5.0).abs() < 1e-12);

        let n = v.normalize();
        assert!((n.length() - 1.0).abs() < 1e-12);
        assert!((n.x - 0.6).abs() < 1e-12);
        assert!((n.y - 0.8).abs() < 1e-12);
    }

    #[test]
    fn test_normalize_zero() {
        let v = Vec3d::ZERO;
        let n = v.normalize();
        assert_eq!(n, Vec3d::ZERO);
    }

    #[test]
    fn test_lerp() {
        let a = dvec3(0.0, 0.0, 0.0);
        let b = dvec3(10.0, 20.0, 30.0);
        let mid = a.lerp(b, 0.5);
        assert_eq!(mid, dvec3(5.0, 10.0, 15.0));
    }

    #[test]
    fn test_min_max() {
        let a = dvec3(1.0, 5.0, 3.0);
        let b = dvec3(4.0, 2.0, 6.0);
        assert_eq!(a.min(b), dvec3(1.0, 2.0, 3.0));
        assert_eq!(a.max(b), dvec3(4.0, 5.0, 6.0));
    }

    #[test]
    fn test_arithmetic() {
        let a = dvec3(1.0, 2.0, 3.0);
        let b = dvec3(4.0, 5.0, 6.0);
        assert_eq!(a + b, dvec3(5.0, 7.0, 9.0));
        assert_eq!(a - b, dvec3(-3.0, -3.0, -3.0));
        assert_eq!(a * 2.0, dvec3(2.0, 4.0, 6.0));
        assert_eq!(2.0 * a, dvec3(2.0, 4.0, 6.0));
        assert_eq!(-a, dvec3(-1.0, -2.0, -3.0));
    }

    #[test]
    fn test_dominant_axis() {
        assert_eq!(dvec3(10.0, 1.0, 2.0).dominant_axis(), 0);
        assert_eq!(dvec3(1.0, 10.0, 2.0).dominant_axis(), 1);
        assert_eq!(dvec3(1.0, 2.0, 10.0).dominant_axis(), 2);
        assert_eq!(dvec3(-10.0, 1.0, 2.0).dominant_axis(), 0);
    }

    #[test]
    fn test_distance() {
        let a = dvec3(1.0, 0.0, 0.0);
        let b = dvec3(4.0, 0.0, 0.0);
        assert!((a.distance(b) - 3.0).abs() < 1e-12);
    }
}
