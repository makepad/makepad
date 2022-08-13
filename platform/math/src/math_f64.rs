use{
    std::{fmt,ops},
    crate::math_f32::*,
//    makepad_microserde::*,
//    crate::colorhex::*
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
pub struct DVec2 {
    pub x: f64,
    pub y: f64,
}

impl std::convert::From<Vec2> for DVec2{
    fn from(other:Vec2)->DVec2{DVec2{x:other.x as f64, y:other.y as f64}}
}

impl std::convert::From<DVec2> for Vec2{
    fn from(other:DVec2)->Vec2{Vec2{x:other.x as f32, y:other.y as f32}}
}

impl DVec2 {
    pub fn new() -> DVec2 {
        DVec2::default()
    }
    
    pub fn all(x: f64) -> DVec2 {
        DVec2 {x: x, y: x}
    }
    
    pub fn into_vec2(self)->Vec2{
        Vec2{x:self.x as f32, y:self.y as f32}
    }
    
    pub fn from_lerp(a: DVec2, b: DVec2, f: f64) -> DVec2 {
        let nf = 1.0 - f;
        return DVec2{
            x: nf * a.x + f * b.x,
            y: nf * a.y + f * b.y,
        };
    }
    
    pub fn distance(&self, other: &DVec2) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
    
    pub fn length(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }    
}

impl fmt::Display for DVec2 {
    // This trait requires `fmt` with this exact signature.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,"vec2f64({},{})",self.x, self.y)
    }
}

pub fn dvec2(x: f64, y: f64) -> DVec2 {DVec2 {x, y}}

//------ Vec2 operators

impl ops::Add<DVec2> for DVec2 {
    type Output = DVec2;
    fn add(self, rhs: DVec2) -> DVec2 {
        DVec2 {x: self.x + rhs.x, y: self.y + rhs.y}
    }
}

impl ops::Sub<DVec2> for DVec2 {
    type Output = DVec2;
    fn sub(self, rhs: DVec2) -> DVec2 {
        DVec2 {x: self.x - rhs.x, y: self.y - rhs.y}
    }
}

impl ops::Mul<DVec2> for DVec2 {
    type Output = DVec2;
    fn mul(self, rhs: DVec2) -> DVec2 {
        DVec2 {x: self.x * rhs.x, y: self.y * rhs.y}
    }
}

impl ops::Div<DVec2> for DVec2 {
    type Output = DVec2;
    fn div(self, rhs: DVec2) -> DVec2 {
        DVec2 {x: self.x / rhs.x, y: self.y / rhs.y}
    }
}


impl ops::Add<DVec2> for f64 {
    type Output = DVec2;
    fn add(self, rhs: DVec2) -> DVec2 {
        DVec2 {x: self + rhs.x, y: self + rhs.y}
    }
}

impl ops::Sub<DVec2> for f64 {
    type Output = DVec2;
    fn sub(self, rhs: DVec2) -> DVec2 {
        DVec2 {x: self -rhs.x, y: self -rhs.y}
    }
}

impl ops::Mul<DVec2> for f64 {
    type Output = DVec2;
    fn mul(self, rhs: DVec2) -> DVec2 {
        DVec2 {x: self *rhs.x, y: self *rhs.y}
    }
}

impl ops::Div<DVec2> for f64 {
    type Output = DVec2;
    fn div(self, rhs: DVec2) -> DVec2 {
        DVec2 {x: self / rhs.x, y: self / rhs.y}
    }
}


impl ops::Add<f64> for DVec2 {
    type Output = DVec2;
    fn add(self, rhs: f64) -> DVec2 {
        DVec2 {x: self.x + rhs, y: self.y + rhs}
    }
}

impl ops::Sub<f64> for DVec2 {
    type Output = DVec2;
    fn sub(self, rhs: f64) -> DVec2 {
        DVec2 {x: self.x - rhs, y: self.y - rhs}
    }
}

impl ops::Mul<f64> for DVec2 {
    type Output = DVec2;
    fn mul(self, rhs: f64) -> DVec2 {
        DVec2 {x: self.x * rhs, y: self.y * rhs}
    }
}

impl ops::Div<f64> for DVec2 {
    type Output = DVec2;
    fn div(self, rhs: f64) -> DVec2 {
        DVec2 {x: self.x / rhs, y: self.y / rhs}
    }
}

impl ops::AddAssign<DVec2> for DVec2 {
    fn add_assign(&mut self, rhs: DVec2) {
        self.x = self.x + rhs.x;
        self.y = self.y + rhs.y;
    }
}

impl ops::SubAssign<DVec2> for DVec2 {
    fn sub_assign(&mut self, rhs: DVec2) {
        self.x = self.x - rhs.x;
        self.y = self.y - rhs.y;
    }
}

impl ops::MulAssign<DVec2> for DVec2 {
    fn mul_assign(&mut self, rhs: DVec2) {
        self.x = self.x * rhs.x;
        self.y = self.y * rhs.y;
    }
}

impl ops::DivAssign<DVec2> for DVec2 {
    fn div_assign(&mut self, rhs: DVec2) {
        self.x = self.x / rhs.x;
        self.y = self.y / rhs.y;
    }
}


impl ops::AddAssign<f64> for DVec2 {
    fn add_assign(&mut self, rhs: f64) {
        self.x = self.x + rhs;
        self.y = self.y + rhs;
    }
}

impl ops::SubAssign<f64> for DVec2 {
    fn sub_assign(&mut self, rhs: f64) {
        self.x = self.x - rhs;
        self.y = self.y - rhs;
    }
}

impl ops::MulAssign<f64> for DVec2 {
    fn mul_assign(&mut self, rhs: f64) {
        self.x = self.x * rhs;
        self.y = self.y * rhs;
    }
}

impl ops::DivAssign<f64> for DVec2 {
    fn div_assign(&mut self, rhs: f64) {
        self.x = self.x / rhs;
        self.y = self.y / rhs;
    }
}

impl ops::Neg for DVec2 {
    type Output = DVec2;
    fn neg(self) -> Self { DVec2{x:-self.x, y:-self.y}}
}
