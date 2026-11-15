//! fmt::NumBuffer: port of library/core/src/fmt/num_buffer.rs @1.100.0 (int_format_into, stable
//! since 1.98). The memory `format_into` writes an integer's decimals into (fmt/num.rs).

use crate::mem::MaybeUninit;

/// Trait used to describe the maximum number of digits in decimal base of the implemented integer.
#[unstable(feature = "fmt_internals", issue = "none")]
pub trait NumBufferTrait {
    /// Used for initializing the `NumberBuffer` value.
    const DEFAULT: Self::Buf;
    /// The actual underlying type.
    type Buf: AsRef<[MaybeUninit<u8>]> + AsMut<[MaybeUninit<u8>]>;
}

macro_rules! impl_NumBufferTrait {
    ($($signed:ident, $unsigned:ident,)*) => {
        $(
            impl NumBufferTrait for $signed {
                // `+ 2` and not `+ 1` to include the `-` character.
                const DEFAULT: Self::Buf = [MaybeUninit::<u8>::uninit(); $signed::MAX.ilog10() as usize + 2];
                type Buf = [MaybeUninit<u8>; $signed::MAX.ilog10() as usize + 2];
            }
            impl NumBufferTrait for $unsigned {
                const DEFAULT: Self::Buf = [MaybeUninit::<u8>::uninit(); $unsigned::MAX.ilog10() as usize + 1];
                type Buf = [MaybeUninit<u8>; $unsigned::MAX.ilog10() as usize + 1];
            }
        )*
    }
}

impl_NumBufferTrait! {
    i8, u8,
    i16, u16,
    i32, u32,
    i64, u64,
    isize, usize,
    i128, u128,
}

/// Memory for formatting numbers using `T::format_into()`: enough to hold the longest decimal
/// string representation a number of type `T` could have. It is used only by calling
/// `format_into()`; there is no other way to access its contents.
pub struct NumBuffer<T: NumBufferTrait> {
    pub(crate) buf: T::Buf,
    phantom: crate::marker::PhantomData<T>,
}

impl<T: NumBufferTrait> crate::fmt::Debug for NumBuffer<T> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        f.debug_struct("NumBuffer").finish()
    }
}

impl<T: NumBufferTrait> NumBuffer<T> {
    /// Initializes internal buffer.
    pub const fn new() -> Self {
        NumBuffer { buf: T::DEFAULT, phantom: crate::marker::PhantomData }
    }
}
