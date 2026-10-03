//! Default. Primitive impls are generated (num/prim_traits.rs); derive(Default) is native.

pub trait Default: Sized {
    fn default() -> Self;
}

impl Default for () {
    fn default() {}
}

impl<'a> Default for &'a str {
    fn default() -> &'a str {
        ""
    }
}

impl<'a, T> Default for &'a [T] {
    fn default() -> &'a [T] {
        &[]
    }
}
