// ported from library/core/src/clone.rs @8d1a764
//! Clone. Primitive impls are generated (num/prim_traits.rs); derive(Clone) is native.

#[cfg_attr(not(relax_check), lang = "clone")]
pub trait Clone: Sized {
    #[cfg_attr(not(relax_check), lang = "clone_fn")]
    fn clone(&self) -> Self;
    fn clone_from(&mut self, source: &Self) {
        *self = source.clone();
    }
}

// library/core/src/clone.rs:737: the never type is trivially Clone.
impl Clone for ! {
    fn clone(&self) -> ! { *self }
}

impl<'a, T: crate::marker::PointeeSized> Clone for &'a T {
    fn clone(&self) -> &'a T {
        *self
    }
}

pub use crate::clone_uninit::CloneToUninit;

// :283,458,468. Derive expansion emits these declarations' ordinary bounds.
// Const effects are outside this port.
#[cfg_attr(not(relax_check), lang = "trivial_clone")]
pub unsafe trait TrivialClone: Clone {}
pub struct AssertParamIsClone<T: Clone + crate::marker::PointeeSized> { _field: crate::marker::PhantomData<T> }
pub struct AssertParamIsCopy<T: crate::marker::Copy + crate::marker::PointeeSized> { _field: crate::marker::PhantomData<T> }

// library/core/src/clone.rs:721-794: primitive and pointer trivial cloning.
unsafe impl TrivialClone for usize {}
unsafe impl TrivialClone for u8 {}
unsafe impl TrivialClone for u16 {}
unsafe impl TrivialClone for u32 {}
unsafe impl TrivialClone for u64 {}
unsafe impl TrivialClone for u128 {}
unsafe impl TrivialClone for isize {}
unsafe impl TrivialClone for i8 {}
unsafe impl TrivialClone for i16 {}
unsafe impl TrivialClone for i32 {}
unsafe impl TrivialClone for i64 {}
unsafe impl TrivialClone for i128 {}
unsafe impl TrivialClone for f32 {}
unsafe impl TrivialClone for f64 {}
unsafe impl TrivialClone for bool {}
unsafe impl TrivialClone for char {}
unsafe impl TrivialClone for ! {}
unsafe impl<T: crate::marker::PointeeSized> TrivialClone for &T {}
unsafe impl<T: crate::marker::PointeeSized> TrivialClone for *const T {}
unsafe impl<T: crate::marker::PointeeSized> TrivialClone for *mut T {}
