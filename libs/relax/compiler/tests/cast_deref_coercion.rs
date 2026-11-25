//! A cast between references is first a coercion (rustc try_coercion_cast): `&s as &str` is a deref
//! coercion of `&String`, through user Deref impls too. Its borrow has the cast type's mutability, so
//! `&mut c as &str` calls `Deref::deref` and only `as &mut str` calls `DerefMut::deref_mut`; and a cast
//! whose referent is still open (`&w as &str` with w: Wrapper<?T>) is checked after typeck, as rustc's
//! check_casts does, when later uses have decided it. Relax decided ?T = str from the cast, and
//! called deref_mut for a shared result. A block's or a function's tail coerced to `&str` is the same
//! shared borrow (`let x: &str = { &mut c };`): Relax called deref_mut there too.
use std::cell::Cell;
use std::ops::{Deref, DerefMut};
struct Wrapper<T>(T);
impl<T> Deref for Wrapper<T> { type Target = T; fn deref(&self) -> &T { &self.0 } }
fn wrap_from<T: From<&'static str>>(s: &'static str) -> Wrapper<T> { Wrapper(T::from(s)) }
struct Counted { s: String, shared: Cell<u32>, unique: u32 }
impl Deref for Counted { type Target = str; fn deref(&self) -> &str { self.shared.set(self.shared.get() + 1); &self.s } }
impl DerefMut for Counted { fn deref_mut(&mut self) -> &mut str { self.unique += 1; &mut self.s } }
fn view(c: &mut Counted) -> &str { c }

#[test]
fn shared_casts() {
    let s = String::from("abc");
    let a = &s as &str;
    assert_eq!(a, "abc");
    let r = &s;
    let b = &r as &str;
    assert_eq!(b.len(), 3);
    let v = vec![1u8, 2];
    let sl = &v as &[u8];
    assert_eq!(sl, [1, 2]);
}
#[test]
fn unique_casts() {
    let mut s = String::from("xy");
    { let m = &mut s as &mut str; m.make_ascii_uppercase(); }
    assert_eq!(s, "XY");
    let mut v = vec![0u8; 3];
    { let m = &mut v as &mut [u8]; m[2] = 4; }
    assert_eq!(v, [0, 0, 4]);
    let sh = &mut s as &str;
    assert_eq!(sh, "XY");
}
#[test]
fn open_inference() {
    let v = vec![3u16, 4];
    let sl = &v as &[_];
    assert_eq!(sl.len(), 2);
    let w = wrap_from("later");
    let d = &w as &str;
    assert_eq!(d, "later");
    let kept: Wrapper<String> = w;
    assert_eq!(kept.0.len(), 5);
}
#[test]
fn counted_derefs() {
    let mut c = Counted { s: "hi".into(), shared: Cell::new(0), unique: 0 };
    { let x = &c as &str; assert_eq!(x, "hi"); }
    { let x = &mut c as &str; assert_eq!(x, "hi"); }
    assert_eq!((c.shared.get(), c.unique), (2, 0));
    { let x = &mut c as &mut str; x.make_ascii_uppercase(); }
    assert_eq!((c.shared.get(), c.unique), (2, 1));
    assert_eq!(c.s, "HI");
    { let x: &str = { &mut c }; assert_eq!(x, "HI"); }
    assert_eq!(view(&mut c), "HI");
    assert_eq!((c.shared.get(), c.unique), (4, 1), "a tail coerced to &str borrows shared");
    { let x: &mut str = { &mut c }; x.make_ascii_lowercase(); }
    assert_eq!((c.shared.get(), c.unique), (4, 2), "a tail coerced to &mut str borrows unique");
    assert_eq!(c.s, "hi");
    // the arms of an if and of a match share the one coercion to &str
    let flag = c.unique > 0;
    { let x: &str = if flag { &mut c } else { &mut c }; assert_eq!(x, "hi"); }
    { let x: &str = match flag { true => &mut c, false => &mut c }; assert_eq!(x, "hi"); }
    assert_eq!((c.shared.get(), c.unique), (6, 2), "an if's and a match's arms coerced to &str borrow shared");
}
