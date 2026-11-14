//~ E0599
// A method of an associated type's bound needs its trait in scope: rustc assembles no method
// candidates from an alias's bounds (method/probe.rs assemble_probe has no alias arm).
mod m {
    pub trait Tr {
        fn go(&self) -> u8 {
            7
        }
    }
    impl Tr for u8 {}
    pub trait Has {
        type A: Tr;
        fn a(&self) -> Self::A;
    }
    impl Has for () {
        type A = u8;
        fn a(&self) -> u8 {
            1
        }
    }
}
fn f<H: m::Has>(h: &H) -> u8 {
    h.a().go()
}
fn main() {
    assert_eq!(f(&()), 7);
}
