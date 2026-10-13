use std::cell::Cell;

struct Counted<'a>(&'a Cell<u32>);
impl Clone for Counted<'_> {
    fn clone(&self) -> Self { self.0.set(self.0.get() + 1); Self(self.0) }
}
impl Copy for Counted<'_> {}

#[derive(Clone, Copy)]
struct Simple<'a>(Counted<'a>);
#[derive(Clone, Copy)]
struct Generic<T>(T);
#[derive(Clone, Copy)]
union Union<T: Copy> { value: T }
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(packed)]
struct Packed<T> { x: u8, value: T }
#[derive(Clone, PartialEq, Eq)]
enum Choice<T> { Empty, Tuple(T), Named { value: T } }

#[test]
fn clone_forms() {
    let n = Cell::new(0);
    let s = Simple(Counted(&n));
    let _ = s.clone();
    assert_eq!(n.get(), 0);
    let g = Generic(Counted(&n));
    let _ = g.clone();
    assert_eq!(n.get(), 1);
    let u = Union { value: Counted(&n) };
    let _ = u.clone();
    assert_eq!(n.get(), 1);
}
#[test]
fn derived_comparisons_and_packed_fields() {
    let p = Packed { x: 1, value: 23u32 };
    assert!(p == p.clone());
    assert!(p != Packed { x: 1, value: 24u32 });
    let a = Choice::Named { value: String::from("hello") };
    assert!(a == a.clone());
    assert!(a != Choice::Tuple(String::from("hello")));
    assert!(Choice::<String>::Empty == Choice::Empty);
}
