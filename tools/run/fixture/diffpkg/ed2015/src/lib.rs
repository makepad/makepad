// These deprecated spellings are the subject of the edition 2015 regression.
#![allow(bare_trait_objects)]

pub unsafe trait Marker {}
unsafe impl Marker for Send + Sync {}

pub unsafe trait StaticMarker {}
unsafe impl StaticMarker for Shape + 'static {}

pub trait Shape {
    fn area(&self) -> u32;
}

struct Square(u32);

impl Shape for Square {
    fn area(&self) -> u32 {
        self.0 * self.0
    }
}

fn marker<T: Marker + ?Sized>(_: &T) -> u32 { 17 }
fn static_marker<T: StaticMarker + ?Sized>(_: &T) -> u32 { 23 }

fn borrowed_area(shape: &(Shape + Sync)) -> u32 {
    shape.area()
}

pub fn lines() -> Vec<String> {
    let square = Square(7);
    let marked: &(Send + Sync) = &square;
    let static_shape: &(Shape + 'static) = &square;
    let boxed: Box<Shape + Send> = Box::new(Square(3));
    vec![
        format!("bare impl {} {}", marker(marked), static_marker(static_shape)),
        format!("bare box {}", boxed.area()),
        format!("bare ref {}", borrowed_area(&square)),
    ]
}
