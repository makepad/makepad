// closures are Clone/Copy when their captures are (Reference types/closure.md "Other traits"):
// `callback.clone()` with `F: Fn(..) + Clone` (makepad apple_game_input.rs:103)
use std::rc::Rc;
use std::cell::Cell;
fn twice<F: Fn(u32) -> u32 + Clone>(f: F) -> (u32, u32) {
    let g = f.clone();
    (f(1), g(2))
}
#[test]
fn closure_clone() {
    let k = 10;
    assert_eq!(twice(|x| x + k), (11, 12));
    let s = String::from("ab");
    let r = twice(move |x| x + s.len() as u32);
    assert_eq!(r, (3, 4));
    let c = Rc::new(Cell::new(0u32));
    let c2 = c.clone();
    let f = move |x: u32| { c2.set(c2.get() + x); c2.get() };
    let g = f.clone();
    assert_eq!(Rc::strong_count(&c), 3);
    f(1);
    g(5);
    assert_eq!(c.get(), 6);
    drop(f);
    drop(g);
    assert_eq!(Rc::strong_count(&c), 1);
}
// `boxed_slice.as_mut_ptr()`: Box's associated `as_mut_ptr(b: &mut Box<T>)` is no method
// candidate, the probe autoderefs to the slice's
#[test]
fn box_slice_as_mut_ptr() {
    let mut b = vec![1u8, 2, 3].into_boxed_slice();
    let p = b.as_mut_ptr();
    unsafe { *p.add(1) = 9 };
    assert_eq!(&*b, &[1, 9, 3]);
}
// literal and range patterns match through references (match ergonomics)
#[test]
fn literal_patterns_through_refs() {
    let s = b"ab+c";
    assert!(s.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-')));
    assert_eq!(s.iter().filter(|b| matches!(b, b'a'..=b'z')).count(), 3);
    let r = &&5u32;
    assert!(matches!(r, 5));
    let names = ["x", "y"];
    assert!(names.iter().any(|n| matches!(*n, "y")));
}
