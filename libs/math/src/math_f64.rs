//use std::f64::consts::PI;

use {
    crate::math_f32::*,
    //    makepad_microserde::*,
    //    crate::colorhex::*
    std::{fmt, ops},
};

pub struct PrettyPrintedF64(pub f64);

impl fmt::Display for PrettyPrintedF64 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.0.abs().fract() < 0.00000001 {
            write!(f, "{}.0", self.0)
        } else {
            write!(f, "{}", self.0)
        }
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Rect {
    pub pos: Vec2d,
    pub size: Vec2d,
}

impl Rect {
    pub fn translate(self, pos: Vec2d) -> Rect {
        Rect {
            pos: self.pos + pos,
            size: self.size,
        }
    }

    pub fn contains(&self, pos: Vec2d) -> bool {
        pos.x >= self.pos.x
            && pos.x <= self.pos.x + self.size.x
            && pos.y >= self.pos.y
            && pos.y <= self.pos.y + self.size.y
    }

    pub fn center(&self) -> Vec2d {
        Vec2d {
            x: self.pos.x + self.size.x * 0.5,
            y: self.pos.y + self.size.y * 0.5,
        }
    }

    pub fn scale_and_shift(&self, center: Vec2d, scale: f64, shift: Vec2d) -> Rect {
        Rect {
            pos: (self.pos - center) * scale + center + shift,
            size: self.size * scale,
        }
    }

    pub fn is_inside_of(&self, r: Rect) -> bool {
        self.pos.x >= r.pos.x
            && self.pos.y >= r.pos.y
            && self.pos.x + self.size.x <= r.pos.x + r.size.x
            && self.pos.y + self.size.y <= r.pos.y + r.size.y
    }

    pub fn intersects(&self, r: Rect) -> bool {
        r.pos.x < self.pos.x + self.size.x
            && r.pos.x + r.size.x > self.pos.x
            && r.pos.y < self.pos.y + self.size.y
            && r.pos.y + r.size.y > self.pos.y
    }

    pub fn add_margin(self, size: Vec2d) -> Rect {
        Rect {
            pos: self.pos - size,
            size: self.size + 2.0 * size,
        }
    }

    pub fn contain(&self, other: Rect) -> Rect {
        let mut pos = other.pos;
        if pos.x < self.pos.x {
            pos.x = self.pos.x
        };
        if pos.y < self.pos.y {
            pos.y = self.pos.y
        };
        if pos.x + other.size.x > self.pos.x + self.size.x {
            pos.x = self.pos.x + self.size.x - other.size.x
        }
        if pos.y + other.size.y > self.pos.y + self.size.y {
            pos.y = self.pos.y + self.size.y - other.size.y
        }
        Rect {
            pos,
            size: other.size,
        }
    }

    pub fn hull(&self, other: Rect) -> Rect {
        let otherpos = other.pos;
        let otherfarside = other.pos + other.size;
        let farside = self.pos + self.size;
        let mut finalpos = self.pos;
        let mut finalfarside = farside;
        if otherpos.x < finalpos.x {
            finalpos.x = otherpos.x
        };
        if otherpos.y < finalpos.y {
            finalpos.y = otherpos.y
        };

        if otherfarside.x > finalfarside.x {
            finalfarside.x = otherfarside.x
        };
        if otherfarside.y > finalfarside.y {
            finalfarside.y = otherfarside.y
        };
        let finalsize = finalfarside - finalpos;
        Rect {
            pos: finalpos,
            size: finalsize,
        }
    }

    pub fn clip(&self, clip: (Vec2d, Vec2d)) -> Rect {
        let mut x1 = self.pos.x;
        let mut y1 = self.pos.y;
        let mut x2 = x1 + self.size.x;
        let mut y2 = y1 + self.size.y;
        x1 = x1.max(clip.0.x).min(clip.1.x);
        y1 = y1.max(clip.0.y).min(clip.1.y);
        x2 = x2.max(clip.0.x).min(clip.1.x);
        y2 = y2.max(clip.0.y).min(clip.1.y);
        Rect {
            pos: dvec2(x1, y1),
            size: dvec2(x2 - x1, y2 - y1),
        }
    }

    pub fn from_lerp(a: Rect, b: Rect, f: f64) -> Rect {
        Rect {
            pos: (b.pos - a.pos) * f + a.pos,
            size: (b.size - a.size) * f + a.size,
        }
    }

    pub fn dpi_snap(&self, f: f64) -> Rect {
        Rect {
            pos: dvec2((self.pos.x / f).floor() * f, (self.pos.y / f).floor() * f),
            size: dvec2((self.size.x / f).floor() * f, (self.size.y / f).floor() * f),
        }
    }

    pub fn grow(&mut self, amt: f64) {
        self.pos.x = self.pos.x - amt;
        self.pos.y = self.pos.y - amt;
        self.size.x = self.size.x + amt * 2.;
        self.size.y = self.size.y + amt * 2.;
    }

    pub fn clip_y_between(&mut self, y1: f64, y2: f64) {
        if self.pos.y < y1 {
            let diff = y1 - self.pos.y;
            self.pos.y = y1;
            self.size.y = self.size.y - diff;
        }

        if (self.pos.y + self.size.y) > y2 {
            let diff = y2 - (self.pos.y + self.size.y);
            self.size.y = self.size.y + diff;
        }
    }

    pub fn clip_x_between(&mut self, x1: f64, x2: f64) {
        if self.pos.x < x1 {
            let diff = x1 - self.pos.x;
            self.pos.x = x1;
            self.size.x = self.size.x - diff;
        }

        if (self.pos.x + self.size.x) > x2 {
            let diff = x2 - (self.pos.x + self.size.x);
            self.size.x = self.size.x + diff;
        }
    }

    pub fn is_nan(&self) -> bool {
        self.pos.is_nan() || self.size.is_nan()
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Vec4d {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}
pub type DVec4 = Vec4d;

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Vec3d {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}
pub type DVec3 = Vec3d;

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Vec2d {
    pub x: f64,
    pub y: f64,
}
pub type DVec2 = Vec2d;

impl std::convert::From<Vec2f> for Vec2d {
    fn from(other: Vec2f) -> Vec2d {
        Vec2d {
            x: other.x as f64,
            y: other.y as f64,
        }
    }
}

impl std::convert::From<Vec2d> for Vec2f {
    fn from(other: Vec2d) -> Vec2f {
        Vec2f {
            x: other.x as f32,
            y: other.y as f32,
        }
    }
}

impl std::convert::From<(Vec2d, Vec2d)> for Rect {
    fn from(o: (Vec2d, Vec2d)) -> Rect {
        Rect {
            pos: dvec2(o.0.x, o.0.y),
            size: dvec2(o.1.x - o.0.x, o.1.y - o.0.y),
        }
    }
}

impl Vec2d {
    pub const fn new() -> Vec2d {
        Vec2d { x: 0.0, y: 0.0 }
    }

    pub fn zero(&mut self) {
        self.x = 0.;
        self.y = 0.;
    }

    pub fn dpi_snap(&self, f: f64) -> Vec2d {
        Vec2d {
            x: (self.x * f).round() / f,
            y: (self.y * f).round() / f,
        }
    }

    pub const fn all(x: f64) -> Vec2d {
        Vec2d { x, y: x }
    }

    pub const fn index(&self, index: Vec2Index) -> f64 {
        match index {
            Vec2Index::X => self.x,
            Vec2Index::Y => self.y,
        }
    }

    pub fn set_index(&mut self, index: Vec2Index, v: f64) {
        match index {
            Vec2Index::X => self.x = v,
            Vec2Index::Y => self.y = v,
        }
    }

    pub const fn from_index_pair(index: Vec2Index, a: f64, b: f64) -> Self {
        match index {
            Vec2Index::X => Self { x: a, y: b },
            Vec2Index::Y => Self { x: b, y: a },
        }
    }

    pub const fn into_vec2(self) -> Vec2f {
        Vec2f {
            x: self.x as f32,
            y: self.y as f32,
        }
    }

    pub fn from_lerp(a: Vec2d, b: Vec2d, f: f64) -> Vec2d {
        let nf = 1.0 - f;
        Vec2d {
            x: nf * a.x + f * b.x,
            y: nf * a.y + f * b.y,
        }
    }

    pub fn floor(self) -> Vec2d {
        Vec2d {
            x: self.x.floor(),
            y: self.y.floor(),
        }
    }

    pub fn ceil(self) -> Vec2d {
        Vec2d {
            x: self.x.ceil(),
            y: self.y.ceil(),
        }
    }

    pub fn distance(&self, other: &Vec2d) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }

    pub fn angle_in_radians(&self) -> f64 {
        self.y.atan2(self.x)
    }
    pub fn swapxy(&self) -> Vec2d {
        dvec2(self.y, self.x)
    }
    pub fn angle_in_degrees(&self) -> f64 {
        self.y.atan2(self.x) * (360.0 / (2. * std::f64::consts::PI))
    }

    pub fn length(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    pub fn normalize(&self) -> Vec2d {
        let l = self.length();
        if l == 0.0 {
            return dvec2(0., 0.);
        }
        return dvec2(self.x / l, self.y / l);
    }

    pub fn clockwise_tangent(&self) -> Vec2d {
        return dvec2(-self.y, self.x);
    }

    pub fn counterclockwise_tangent(&self) -> Vec2d {
        return dvec2(self.y, -self.x);
    }

    pub fn normalize_to_x(&self) -> Vec2d {
        let l = self.x;
        if l == 0.0 {
            return dvec2(1., 0.);
        }
        return dvec2(1., self.y / l);
    }
    pub fn normalize_to_y(&self) -> Vec2d {
        let l = self.y;
        if l == 0.0 {
            return dvec2(1., 0.);
        }
        return dvec2(self.x / l, 1.);
    }

    pub fn lengthsquared(&self) -> f64 {
        self.x * self.x + self.y * self.y
    }

    pub fn is_nan(&self) -> bool {
        self.x.is_nan() || self.y.is_nan()
    }
}

impl fmt::Display for Vec2d {
    // This trait requires `fmt` with this exact signature.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "vec2f64({},{})", self.x, self.y)
    }
}

pub const fn dvec2(x: f64, y: f64) -> Vec2d {
    Vec2d { x, y }
}

pub const fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        pos: Vec2d { x, y },
        size: Vec2d { x: w, y: h },
    }
}

//------ Vec2f operators

impl ops::Add<Vec2d> for Vec2d {
    type Output = Vec2d;
    fn add(self, rhs: Vec2d) -> Vec2d {
        Vec2d {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl ops::Sub<Vec2d> for Vec2d {
    type Output = Vec2d;
    fn sub(self, rhs: Vec2d) -> Vec2d {
        Vec2d {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl ops::Mul<Vec2d> for Vec2d {
    type Output = Vec2d;
    fn mul(self, rhs: Vec2d) -> Vec2d {
        Vec2d {
            x: self.x * rhs.x,
            y: self.y * rhs.y,
        }
    }
}

impl ops::Div<Vec2d> for Vec2d {
    type Output = Vec2d;
    fn div(self, rhs: Vec2d) -> Vec2d {
        Vec2d {
            x: self.x / rhs.x,
            y: self.y / rhs.y,
        }
    }
}

impl ops::Add<Vec2d> for f64 {
    type Output = Vec2d;
    fn add(self, rhs: Vec2d) -> Vec2d {
        Vec2d {
            x: self + rhs.x,
            y: self + rhs.y,
        }
    }
}

impl ops::Sub<Vec2d> for f64 {
    type Output = Vec2d;
    fn sub(self, rhs: Vec2d) -> Vec2d {
        Vec2d {
            x: self - rhs.x,
            y: self - rhs.y,
        }
    }
}

impl ops::Mul<Vec2d> for f64 {
    type Output = Vec2d;
    fn mul(self, rhs: Vec2d) -> Vec2d {
        Vec2d {
            x: self * rhs.x,
            y: self * rhs.y,
        }
    }
}

impl ops::Div<Vec2d> for f64 {
    type Output = Vec2d;
    fn div(self, rhs: Vec2d) -> Vec2d {
        Vec2d {
            x: self / rhs.x,
            y: self / rhs.y,
        }
    }
}

impl ops::Add<f64> for Vec2d {
    type Output = Vec2d;
    fn add(self, rhs: f64) -> Vec2d {
        Vec2d {
            x: self.x + rhs,
            y: self.y + rhs,
        }
    }
}

impl ops::Sub<f64> for Vec2d {
    type Output = Vec2d;
    fn sub(self, rhs: f64) -> Vec2d {
        Vec2d {
            x: self.x - rhs,
            y: self.y - rhs,
        }
    }
}

impl ops::Mul<f64> for Vec2d {
    type Output = Vec2d;
    fn mul(self, rhs: f64) -> Vec2d {
        Vec2d {
            x: self.x * rhs,
            y: self.y * rhs,
        }
    }
}

impl ops::Div<f64> for Vec2d {
    type Output = Vec2d;
    fn div(self, rhs: f64) -> Vec2d {
        Vec2d {
            x: self.x / rhs,
            y: self.y / rhs,
        }
    }
}

impl ops::AddAssign<Vec2d> for Vec2d {
    fn add_assign(&mut self, rhs: Vec2d) {
        self.x = self.x + rhs.x;
        self.y = self.y + rhs.y;
    }
}

impl ops::SubAssign<Vec2d> for Vec2d {
    fn sub_assign(&mut self, rhs: Vec2d) {
        self.x = self.x - rhs.x;
        self.y = self.y - rhs.y;
    }
}

impl ops::MulAssign<Vec2d> for Vec2d {
    fn mul_assign(&mut self, rhs: Vec2d) {
        self.x = self.x * rhs.x;
        self.y = self.y * rhs.y;
    }
}

impl ops::DivAssign<Vec2d> for Vec2d {
    fn div_assign(&mut self, rhs: Vec2d) {
        self.x = self.x / rhs.x;
        self.y = self.y / rhs.y;
    }
}

impl ops::AddAssign<f64> for Vec2d {
    fn add_assign(&mut self, rhs: f64) {
        self.x = self.x + rhs;
        self.y = self.y + rhs;
    }
}

impl ops::SubAssign<f64> for Vec2d {
    fn sub_assign(&mut self, rhs: f64) {
        self.x = self.x - rhs;
        self.y = self.y - rhs;
    }
}

impl ops::MulAssign<f64> for Vec2d {
    fn mul_assign(&mut self, rhs: f64) {
        self.x = self.x * rhs;
        self.y = self.y * rhs;
    }
}

impl ops::DivAssign<f64> for Vec2d {
    fn div_assign(&mut self, rhs: f64) {
        self.x = self.x / rhs;
        self.y = self.y / rhs;
    }
}

impl ops::Neg for Vec2d {
    type Output = Vec2d;
    fn neg(self) -> Self {
        Vec2d {
            x: -self.x,
            y: -self.y,
        }
    }
}
