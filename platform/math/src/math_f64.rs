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
pub struct RectF64 {
    pub pos: Vec2F64,
    pub size: Vec2F64,
}

impl RectF64 {
    
    pub fn translate(self, pos: Vec2F64) -> RectF64 {
        RectF64 {pos: self.pos + pos, size: self.size}
    }
    
    pub fn contains(&self, pos: Vec2F64) -> bool {
        return pos.x >= self.pos.x && pos.x <= self.pos.x + self.size.x &&
        pos.y >= self.pos.y && pos.y <= self.pos.y + self.size.y;
    }
    pub fn intersects(&self, r: RectF64) -> bool {
        !(
            r.pos.x > self.pos.x + self.size.x ||
            r.pos.x + r.size.x < self.pos.x ||
            r.pos.y > self.pos.y + self.size.y ||
            r.pos.y + r.size.y < self.pos.y
        )
    }

    pub fn scroll_and_clip(&self, scroll:Vec2F64, clip:(Vec2F64,Vec2F64)) -> RectF64 {
        let mut x1 = self.pos.x - scroll.x;
        let mut y1 = self.pos.y - scroll.y;
        let mut x2 = x1 + self.size.x;
        let mut y2 = y1 + self.size.y;
        x1 = x1.max(clip.0.x).min(clip.1.x);
        y1 = y1.max(clip.0.y).min(clip.1.y);
        x2 = x2.max(clip.0.x).min(clip.1.x);
        y2 = y2.max(clip.0.y).min(clip.1.y);
        return RectF64 {pos: vec2f64(x1, y1), size: vec2f64(x2 - x1, y2 - y1)};
    }
    /*
    pub fn contains_with_margin(&self, pos: Vec2, margin: &Option<Margin>) -> bool {
        if let Some(margin) = margin {
            return
            pos.x >= self.pos.x - margin.l
                && pos.x <= self.pos.x + self.size.x + margin.r
                && pos.y >= self.pos.y - margin.t
                && pos.y <= self.pos.y + self.size.y + margin.b;
        }
        else {
            return self.contains(pos);
        }
    }
    */
    pub fn from_lerp(a: RectF64, b: RectF64, f: f64) -> RectF64 {
        RectF64 {
            pos: (b.pos - a.pos) * f + a.pos,
            size: (b.size - a.size) * f + a.size
        }
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Vec2F64 {
    pub x: f64,
    pub y: f64,
}


impl Vec2F64 {
    pub fn new() -> Vec2F64 {
        Vec2F64::default()
    }
    
    pub fn all(x: f64) -> Vec2F64 {
        Vec2F64 {x: x, y: x}
    }
    
    pub fn into_vec2(self)->Vec2{
        Vec2{x:self.x as f32, y:self.y as f32}
    }
    
    pub fn from_lerp(a: Vec2F64, b: Vec2F64, f: f64) -> Vec2F64 {
        let nf = 1.0 - f;
        return Vec2F64{
            x: nf * a.x + f * b.x,
            y: nf * a.y + f * b.y,
        };
    }
    
    pub fn distance(&self, other: &Vec2F64) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
}

impl fmt::Display for Vec2F64 {
    // This trait requires `fmt` with this exact signature.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,"vec2f64({},{})",self.x, self.y)
    }
}

pub fn vec2f64(x: f64, y: f64) -> Vec2F64 {Vec2F64 {x, y}}

//------ Vec2 operators

impl ops::Add<Vec2F64> for Vec2F64 {
    type Output = Vec2F64;
    fn add(self, rhs: Vec2F64) -> Vec2F64 {
        Vec2F64 {x: self.x + rhs.x, y: self.y + rhs.y}
    }
}

impl ops::Sub<Vec2F64> for Vec2F64 {
    type Output = Vec2F64;
    fn sub(self, rhs: Vec2F64) -> Vec2F64 {
        Vec2F64 {x: self.x - rhs.x, y: self.y - rhs.y}
    }
}

impl ops::Mul<Vec2F64> for Vec2F64 {
    type Output = Vec2F64;
    fn mul(self, rhs: Vec2F64) -> Vec2F64 {
        Vec2F64 {x: self.x * rhs.x, y: self.y * rhs.y}
    }
}

impl ops::Div<Vec2F64> for Vec2F64 {
    type Output = Vec2F64;
    fn div(self, rhs: Vec2F64) -> Vec2F64 {
        Vec2F64 {x: self.x / rhs.x, y: self.y / rhs.y}
    }
}


impl ops::Add<Vec2F64> for f64 {
    type Output = Vec2F64;
    fn add(self, rhs: Vec2F64) -> Vec2F64 {
        Vec2F64 {x: self + rhs.x, y: self + rhs.y}
    }
}

impl ops::Sub<Vec2F64> for f64 {
    type Output = Vec2F64;
    fn sub(self, rhs: Vec2F64) -> Vec2F64 {
        Vec2F64 {x: self -rhs.x, y: self -rhs.y}
    }
}

impl ops::Mul<Vec2F64> for f64 {
    type Output = Vec2F64;
    fn mul(self, rhs: Vec2F64) -> Vec2F64 {
        Vec2F64 {x: self *rhs.x, y: self *rhs.y}
    }
}

impl ops::Div<Vec2F64> for f64 {
    type Output = Vec2F64;
    fn div(self, rhs: Vec2F64) -> Vec2F64 {
        Vec2F64 {x: self / rhs.x, y: self / rhs.y}
    }
}


impl ops::Add<f64> for Vec2F64 {
    type Output = Vec2F64;
    fn add(self, rhs: f64) -> Vec2F64 {
        Vec2F64 {x: self.x + rhs, y: self.y + rhs}
    }
}

impl ops::Sub<f64> for Vec2F64 {
    type Output = Vec2F64;
    fn sub(self, rhs: f64) -> Vec2F64 {
        Vec2F64 {x: self.x - rhs, y: self.y - rhs}
    }
}

impl ops::Mul<f64> for Vec2F64 {
    type Output = Vec2F64;
    fn mul(self, rhs: f64) -> Vec2F64 {
        Vec2F64 {x: self.x * rhs, y: self.y * rhs}
    }
}

impl ops::Div<f64> for Vec2F64 {
    type Output = Vec2F64;
    fn div(self, rhs: f64) -> Vec2F64 {
        Vec2F64 {x: self.x / rhs, y: self.y / rhs}
    }
}

impl ops::AddAssign<Vec2F64> for Vec2F64 {
    fn add_assign(&mut self, rhs: Vec2F64) {
        self.x = self.x + rhs.x;
        self.y = self.y + rhs.y;
    }
}

impl ops::SubAssign<Vec2F64> for Vec2F64 {
    fn sub_assign(&mut self, rhs: Vec2F64) {
        self.x = self.x - rhs.x;
        self.y = self.y - rhs.y;
    }
}

impl ops::MulAssign<Vec2F64> for Vec2F64 {
    fn mul_assign(&mut self, rhs: Vec2F64) {
        self.x = self.x * rhs.x;
        self.y = self.y * rhs.y;
    }
}

impl ops::DivAssign<Vec2F64> for Vec2F64 {
    fn div_assign(&mut self, rhs: Vec2F64) {
        self.x = self.x / rhs.x;
        self.y = self.y / rhs.y;
    }
}


impl ops::AddAssign<f64> for Vec2F64 {
    fn add_assign(&mut self, rhs: f64) {
        self.x = self.x + rhs;
        self.y = self.y + rhs;
    }
}

impl ops::SubAssign<f64> for Vec2F64 {
    fn sub_assign(&mut self, rhs: f64) {
        self.x = self.x - rhs;
        self.y = self.y - rhs;
    }
}

impl ops::MulAssign<f64> for Vec2F64 {
    fn mul_assign(&mut self, rhs: f64) {
        self.x = self.x * rhs;
        self.y = self.y * rhs;
    }
}

impl ops::DivAssign<f64> for Vec2F64 {
    fn div_assign(&mut self, rhs: f64) {
        self.x = self.x / rhs;
        self.y = self.y / rhs;
    }
}

impl ops::Neg for Vec2F64 {
    type Output = Vec2F64;
    fn neg(self) -> Self { Vec2F64{x:-self.x, y:-self.y}}
}
