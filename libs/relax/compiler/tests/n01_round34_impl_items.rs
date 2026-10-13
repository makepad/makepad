#![allow(dead_code)]
trait Tr {
    type T;
    const N: usize = 2;
    fn f(x: Self::T) -> Self::T;
    fn defaulted() -> usize { Self::N }
}
struct S;
impl Tr for S { type T = i32; fn f(x: i32) -> i32 { x } }
trait Unsized { fn impossible() where Self: Sized; fn len(&self) -> usize; }
impl Unsized for str { fn len(&self) -> usize { str::len(self) } }
#[test] fn trait_items() { assert_eq!(S::f(3), 3); assert_eq!(S::defaulted(), 2); assert_eq!(Unsized::len("ok"), 2); }
