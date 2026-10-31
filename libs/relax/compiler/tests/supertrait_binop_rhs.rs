// Binary operators on a type parameter whose operator traits come from a supertrait list
// with a non-Self RHS (`Mul<f32, Output = Self>`): render_kernels functions/curve.rs.
use std::ops::{Add, Mul, Sub};

pub trait Point: Copy + Add<Output = Self> + Sub<Output = Self> + Mul<f32, Output = Self> {
    fn length(self) -> f32;
    fn unit(self) -> Self {
        let n = self.length();
        self * if n > 0.0 { 1.0 / n } else { 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct V2 {
    x: f32,
    y: f32,
}
impl Add for V2 {
    type Output = V2;
    fn add(self, o: V2) -> V2 {
        V2 { x: self.x + o.x, y: self.y + o.y }
    }
}
impl Sub for V2 {
    type Output = V2;
    fn sub(self, o: V2) -> V2 {
        V2 { x: self.x - o.x, y: self.y - o.y }
    }
}
impl Mul<f32> for V2 {
    type Output = V2;
    fn mul(self, s: f32) -> V2 {
        V2 { x: self.x * s, y: self.y * s }
    }
}
impl Point for V2 {
    fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

fn mix<P: Point>(a: P, b: P, t: f32) -> P {
    a + (b - a) * t
}

fn bezier2<P: Point>(a: P, b: P, c: P, t: f32) -> P {
    let u = 1.0 - t;
    a * (u * u) + b * (2.0 * u * t) + c * (t * t)
}

fn catmull_rom<P: Point>(p0: P, p1: P, p2: P, p3: P, t: f32) -> P {
    let t2 = t * t;
    (p1 * 2.0 + (p2 - p0) * t + (p0 * 2.0 - p1 * 5.0 + p2 * 4.0 - p3) * t2) * 0.5
}

#[test]
fn supertrait_binop_rhs() {
    let a = V2 { x: 0.0, y: 0.0 };
    let b = V2 { x: 2.0, y: 4.0 };
    assert_eq!(mix(a, b, 0.5), V2 { x: 1.0, y: 2.0 });
    assert_eq!(bezier2(a, b, b, 1.0), b);
    assert_eq!(catmull_rom(a, a, b, b, 0.0), a);
    assert_eq!(V2 { x: 3.0, y: 4.0 }.unit(), V2 { x: 0.6, y: 0.8 });
}
