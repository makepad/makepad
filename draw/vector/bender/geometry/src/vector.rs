use std::ops::{Add, Div, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug)]
pub struct Vector {
    x: f32,
    y: f32,
}

impl Vector {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn x(self) -> f32 {
        self.x
    }

    pub fn y(self) -> f32 {
        self.y
    }

    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }

    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y
    }

    pub fn cross(self, other: Self) -> f32 {
        self.x * other.y - other.x * self.y
    }

    pub fn normalize(self) -> Option<Self> {
        let length = self.length();
        if length == 0.0 {
            None
        } else {
            Some(self / length)
        }
    }

    pub fn perpendicular(self) -> Self {
        Self {
            x: -self.y,
            y: self.x,
        }
    }
}

impl Add for Vector {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
        }
    }
}

impl Sub for Vector {
    type Output = Self;

    fn sub(self, other: Self) -> Self::Output {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
        }
    }
}

impl Mul<f32> for Vector {
    type Output = Self;

    fn mul(self, k: f32) -> Self::Output {
        Self {
            x: self.x * k,
            y: self.y * k,
        }
    }
}

impl Div<f32> for Vector {
    type Output = Self;

    fn div(self, k: f32) -> Self::Output {
        Self {
            x: self.x / k,
            y: self.y / k,
        }
    }
}

impl Neg for Vector {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self {
            x: -self.x,
            y: -self.y,
        }
    }
}
