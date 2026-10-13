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

// Null, as real core (there `T: ?Sized + Thin`; Relax has no Thin, so sized T).
impl<T> Default for *const T {
    fn default() -> *const T {
        crate::ptr::null()
    }
}

impl<T> Default for *mut T {
    fn default() -> *mut T {
        crate::ptr::null_mut()
    }
}
