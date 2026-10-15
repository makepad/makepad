// #[track_caller] locations as rustc: `Location::caller().file()` is the path `file!()` names
// (Relax used the absolute path); a trait method declared #[track_caller] makes its impls
// tracked and is tracked through a trait object (its vtable entry takes the location); a
// method call's location is the method name (`a.\n    f()` is at `f`); a tracked fn as a fn
// pointer reports its definition. Line/column numbers below are this file's.
use std::panic::Location;

#[track_caller]
fn here() -> &'static Location<'static> { Location::caller() }

trait Tr {
    #[track_caller]
    fn decl_tracked(&self) -> &'static Location<'static>;
    #[track_caller]
    fn default_tracked(&self) -> &'static Location<'static> { Location::caller() }
}
struct S;
impl Tr for S {
    fn decl_tracked(&self) -> &'static Location<'static> { Location::caller() }
}
impl S {
    #[track_caller]
    fn chain(&self, v: &mut Vec<(u32, u32)>) -> &Self {
        let l = Location::caller();
        v.push((l.line(), l.column()));
        self
    }
}
fn via_ptr(f: fn() -> &'static Location<'static>) -> &'static Location<'static> { f() }

#[test]
fn locations() {
    let (l, n) = (here(), line!());
    assert_eq!(l.file(), file!());
    assert_eq!((l.line(), l.column()), (n, 19));
    let s = S;
    let (l, n) = (s.decl_tracked(), line!());
    assert_eq!((l.line(), l.column()), (n, 21));
    let d: &dyn Tr = &s;
    let (l, n) = (d.decl_tracked(), line!());
    assert_eq!((l.line(), l.column()), (n, 21));
    let (l, n) = (d.default_tracked(), line!());
    assert_eq!((l.line(), l.column()), (n, 21));
    let mut v = Vec::new();
    let n = line!();
    s.
        chain(&mut v).chain(&mut v)
        .chain(&mut v);
    assert_eq!(v, [(n + 2, 9), (n + 2, 23), (n + 3, 10)]);
    // a tracked fn as a value: its definition's location
    let l = via_ptr(here);
    assert_eq!((l.line(), l.column()), (9, 1));
}

// Index::index is #[track_caller] (core): an impl sees the indexing site, the brackets
// (rustc issue 114388: `base\n[0]` is at `[0]`)
struct Ix(u32);
impl std::ops::Index<usize> for Ix {
    type Output = u32;
    fn index(&self, _i: usize) -> &u32 {
        assert_eq!(Location::caller().line(), self.0);
        &self.0
    }
}
#[test]
fn index_location() {
    let n = line!();
    let _ = (Ix(n + 2))
    [0];
}
