// A trait object's principal trait and its supertraits are always method candidates on it,
// imported or not (rustc method/probe.rs assemble_inherent_candidates_from_object): here the
// traits live in `shapes` and the call sites never import them.

mod shapes {
    pub trait Named {
        fn name(&self) -> String;
    }
    pub trait Shape: Named {
        fn area(&self) -> u32;
        fn grow(&mut self, by: u32);
    }
    pub struct Square(pub u32);
    impl Named for Square {
        fn name(&self) -> String { format!("square{}", self.0) }
    }
    impl Shape for Square {
        fn area(&self) -> u32 { self.0 * self.0 }
        fn grow(&mut self, by: u32) { self.0 += by; }
    }
    pub fn make(side: u32) -> Box<dyn Shape> { Box::new(Square(side)) }
}

fn area_of(s: &dyn shapes::Shape) -> u32 { s.area() }

fn grow_twice(s: &mut dyn shapes::Shape) { s.grow(1); s.grow(1); }

#[test]
fn ref_dyn_methods_without_import() {
    let sq = shapes::Square(3);
    let r: &dyn shapes::Shape = &sq;
    assert_eq!(r.area(), 9);
    assert_eq!(r.name(), "square3");
    assert_eq!(area_of(&sq), 9);
}

#[test]
fn box_dyn_methods_without_import() {
    let mut b = shapes::make(4);
    assert_eq!(b.area(), 16);
    assert_eq!(b.name(), "square4");
    b.grow(1);
    assert_eq!(b.area(), 25);
    grow_twice(&mut *b);
    let m: &mut dyn shapes::Shape = &mut *b;
    m.grow(1);
    assert_eq!(m.area(), 64);
    assert_eq!(m.name(), "square8");
}
