// ported from library/core/src/marker.rs @8d1a764
//! Marker traits and PhantomData. Copy/Send/Sync/Sized/Unpin are known to the compiler.

#[cfg_attr(not(relax_check), lang = "sized")]
#[fundamental]
pub trait Sized: MetaSized {}
#[cfg_attr(not(relax_check), lang = "meta_sized")]
#[fundamental]
pub trait MetaSized: PointeeSized {}
#[cfg_attr(not(relax_check), lang = "pointee_sized")]
#[fundamental]
pub trait PointeeSized {}
#[cfg_attr(not(relax_check), lang = "unsize")]
#[rustc_deny_explicit_impl]
pub trait Unsize<T: PointeeSized>: PointeeSized {}
#[cfg_attr(not(relax_check), lang = "copy")]
pub trait Copy: crate::clone::Clone {}
// library/core/src/marker.rs:484: the never type is trivially Copy.
impl Copy for ! {}
// library/core/src/marker.rs:488: copying a shared reference does not copy its pointee.
impl<T: PointeeSized> Copy for &T {}
pub unsafe auto trait Send {}
#[cfg_attr(not(relax_check), lang = "sync")]
pub unsafe auto trait Sync {}
// library/core/src/marker.rs:105,537-538: a shared borrow transfers only
// thread-safe access; a mutable borrow transfers exclusive access.
unsafe impl<T: Sync + PointeeSized> Send for &T {}
unsafe impl<T: Send + PointeeSized> Send for &mut T {}
#[cfg_attr(not(relax_check), lang = "unpin")]
pub auto trait Unpin {}
/// Zero-sized marker owning a `T` for variance/drop-check purposes.
#[cfg_attr(not(relax_check), lang = "phantom_data")]
pub struct PhantomData<T: PointeeSized>;

impl<T: PointeeSized> crate::clone::Clone for PhantomData<T> {
    fn clone(&self) -> PhantomData<T> {
        PhantomData
    }
}
impl<T: PointeeSized> Copy for PhantomData<T> {}
unsafe impl<T: PointeeSized> crate::clone::TrivialClone for PhantomData<T> {}
impl<T: PointeeSized> crate::default::Default for PhantomData<T> {
    fn default() -> PhantomData<T> {
        PhantomData
    }
}
impl<T: PointeeSized> crate::cmp::PartialEq for PhantomData<T> {
    fn eq(&self, _o: &PhantomData<T>) -> bool {
        true
    }
}
impl<T: PointeeSized> crate::cmp::Eq for PhantomData<T> {}
impl<T: PointeeSized> crate::hash::Hash for PhantomData<T> {
    fn hash<H: crate::hash::Hasher>(&self, _state: &mut H) {}
}

pub struct PhantomPinned;
// core's #[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)] (the ones bytemuck's
// `unsafe impl Pod for PhantomPinned` needs: Pod: Copy)
impl crate::clone::Clone for PhantomPinned {
    fn clone(&self) -> PhantomPinned {
        PhantomPinned
    }
}
impl Copy for PhantomPinned {}
impl crate::default::Default for PhantomPinned {
    fn default() -> PhantomPinned {
        PhantomPinned
    }
}

#[cfg_attr(not(relax_check), lang = "tuple_trait")]
pub trait Tuple {}

// library/core/src/marker.rs:1039-1051: a pointer moves independently of its pointee.
impl<T: PointeeSized> Unpin for &T {}
impl<T: PointeeSized> Unpin for &mut T {}
impl<T: PointeeSized> Unpin for *const T {}
impl<T: PointeeSized> Unpin for *mut T {}
impl !Unpin for PhantomPinned {}

// :257: implemented by the PartialEq derive with its exact generic bounds.
pub trait StructuralPartialEq {}

// library/core/src/marker.rs:880-887,904-906,1059-1063.
#[cfg_attr(not(relax_check), lang = "discriminant_kind")]
pub trait DiscriminantKind {
    #[cfg_attr(not(relax_check), lang = "discriminant_type")]
    type Discriminant: crate::clone::Clone + Copy + crate::fmt::Debug + crate::cmp::Eq + crate::cmp::PartialEq + crate::hash::Hash + Send + Sync + Unpin;
}
#[cfg_attr(not(relax_check), lang = "destruct")]
pub trait Destruct: PointeeSized {}
#[cfg_attr(not(relax_check), lang = "freeze")]
pub unsafe auto trait Freeze {}
impl<T: PointeeSized> !Freeze for crate::cell::UnsafeCell<T> {}

// library/core/src/marker.rs:910-917: indirection does not inherit interior mutability.
unsafe impl<T: PointeeSized> Freeze for PhantomData<T> {}
unsafe impl<T: PointeeSized> Freeze for *const T {}
unsafe impl<T: PointeeSized> Freeze for *mut T {}
unsafe impl<T: PointeeSized> Freeze for &T {}
unsafe impl<T: PointeeSized> Freeze for &mut T {}
