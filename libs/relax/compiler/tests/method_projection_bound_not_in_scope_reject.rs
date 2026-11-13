// rejects: E0599
// The trait an associated type is bounded by gives its methods only when it is in scope (rustc's probe;
// relax issue 101): `item.pick()` finds no method.
mod picks {
    pub trait Bound {
        fn pick(&self) -> u32 {
            1
        }
    }
    impl Bound for u8 {}
}
pub trait Holder {
    type Item: picks::Bound;
}
pub fn pick<T: Holder>(item: T::Item) -> u32 {
    item.pick()
}
