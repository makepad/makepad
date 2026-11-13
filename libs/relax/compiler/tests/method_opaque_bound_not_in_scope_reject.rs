// rejects: E0599
// The trait an `impl Trait` return type is bounded by gives its methods only when it is in scope (rustc's
// probe; relax issue 101, makepad PR 1499): `picks::make().pick()` finds no method.
mod picks {
    pub trait Bound {
        fn pick(&self) -> u32 {
            1
        }
    }
    impl Bound for u8 {}
    pub fn make() -> impl Bound {
        1u8
    }
}
pub fn pick() -> u32 {
    picks::make().pick()
}
