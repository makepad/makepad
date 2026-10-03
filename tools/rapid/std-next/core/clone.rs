//! Clone. Primitive impls are generated (num/prim_traits.rs); derive(Clone) is native.

pub trait Clone: Sized {
    fn clone(&self) -> Self;
    fn clone_from(&mut self, source: &Self) {
        *self = source.clone();
    }
}

impl<'a, T: ?Sized> Clone for &'a T {
    fn clone(&self) -> &'a T {
        *self
    }
}
