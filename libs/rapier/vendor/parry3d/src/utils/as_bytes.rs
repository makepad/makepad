use core::mem;
use core::slice;

use crate::math::{Real, Vector2, Vector3};

/// Trait that transforms thing to a slice of u8.
pub trait AsBytes {
    /// Converts `self` to a slice of bytes.
    fn as_bytes(&self) -> &[u8];
}

macro_rules! as_bytes_impl(
    ($T: ty, $dimension: expr) => (
        impl AsBytes for $T {
            #[inline(always)]
            fn as_bytes(&self) -> &[u8] {
                unsafe {
                    slice::from_raw_parts(self as *const $T as *const u8, mem::size_of::<Real>() * $dimension)
                }
            }
        }
    )
);

as_bytes_impl!(Vector2, 2);
as_bytes_impl!(Vector3, 3);

// TODO: implement for all `T: Copy` instead?
