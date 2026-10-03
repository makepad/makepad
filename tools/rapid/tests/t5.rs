// traits: user traits with default methods, trait impls on structs and primitives,
// generic fns with bounds, Trait::method paths, operator traits with Output.
use std::ops::{Add, Mul};

#[derive(Clone, Copy)]
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

impl Mul<f32> for V2 {
    type Output = V2;
    fn mul(self, s: f32) -> V2 {
        V2 { x: self.x * s, y: self.y * s }
    }
}

trait Shape {
    fn area(&self) -> f32;
    fn name(&self) -> u32 {
        7
    }
    fn double_area(&self) -> f32 {
        self.area() * 2.0
    }
}

struct Sq {
    s: f32,
}
struct Circle {
    r: f32,
}

impl Shape for Sq {
    fn area(&self) -> f32 {
        self.s * self.s
    }
    fn name(&self) -> u32 {
        1
    }
}

impl Shape for Circle {
    fn area(&self) -> f32 {
        3.0 * self.r * self.r
    }
}

trait Tag {
    fn tag() -> u64;
}
impl Tag for u8 {
    fn tag() -> u64 {
        8
    }
}
impl Tag for u64 {
    fn tag() -> u64 {
        64
    }
}

fn total<T: Shape>(a: &T, b: &T) -> f32 {
    a.double_area() + b.area()
}

fn tag_of<T: Tag>() -> u64 {
    T::tag()
}

#[test]
fn traits() {
    let a = V2 { x: 1.0, y: 2.0 };
    let b = V2 { x: 3.0, y: 4.0 };
    let c = (a + b) * 2.0;
    assert!(c.x == 8.0 && c.y == 12.0);
    let s = Sq { s: 3.0 };
    let k = Circle { r: 1.0 };
    assert_eq!(s.name(), 1);
    assert_eq!(k.name(), 7);
    assert!(s.double_area() == 18.0);
    assert!(total(&s, &Sq { s: 1.0 }) == 19.0);
    assert!(Shape::area(&k) == 3.0);
    assert_eq!(tag_of::<u8>() + tag_of::<u64>(), 72);
    assert_eq!(<u8 as Tag>::tag(), 8);
}
