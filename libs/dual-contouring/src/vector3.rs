use crate::lerp::Lerp;
use std::ops::{Add, Div, Sub};

#[derive(Clone, Copy, Debug)]
pub struct Vector3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vector3 {
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    pub fn normalize(self) -> Vector3 {
        self / self.length()
    }
}

impl Add for Vector3 {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        Vector3 {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
}

impl Sub for Vector3 {
    type Output = Self;

    fn sub(self, other: Self) -> Self::Output {
        Vector3 {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }
}

impl Div<f32> for Vector3 {
    type Output = Self;

    fn div(self, other: f32) -> Self::Output {
        Vector3 {
            x: self.x / other,
            y: self.y / other,
            z: self.z / other,
        }
    }
}

impl Lerp for Vector3 {
    fn lerp(self, other: Self, t: f32) -> Self {
        Vector3 {
            x: self.x.lerp(other.x, t),
            y: self.y.lerp(other.y, t),
            z: self.z.lerp(other.z, t),
        }
    }
}
