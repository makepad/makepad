//! Numbers: error types, Wrapping/Saturating, NonZero, FpCategory, integer parsing.
//! Inherent integer/float methods and the primitive trait impls are generated
//! (int_impls.rs, float_impls.rs, prim_traits.rs by check/src/gen_num.rs).

pub mod dec2flt;
#[cfg(not(hotrust_check))]
mod float_impls;
#[cfg(not(hotrust_check))]
mod int_impls;
#[cfg(not(hotrust_check))]
mod nonzero;
pub mod parse;
#[cfg(not(hotrust_check))]
mod prim_traits;
mod wrapping;

pub use dec2flt::ParseFloatError;
#[cfg(not(hotrust_check))]
pub use nonzero::{
    NonZero, NonZeroI128, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI8, NonZeroIsize, NonZeroU128, NonZeroU16, NonZeroU32,
    NonZeroU64, NonZeroU8, NonZeroUsize, ZeroablePrimitive,
};
pub use wrapping::{Saturating, Wrapping};

use crate::fmt;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FpCategory {
    Nan,
    Infinite,
    Zero,
    Subnormal,
    Normal,
}

#[derive(Debug, Clone, PartialEq, Eq, Copy, Hash)]
#[non_exhaustive]
pub enum IntErrorKind {
    Empty,
    InvalidDigit,
    PosOverflow,
    NegOverflow,
    Zero,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseIntError {
    pub(crate) kind: IntErrorKind,
}

impl ParseIntError {
    pub const fn kind(&self) -> &IntErrorKind {
        &self.kind
    }
}

impl fmt::Display for ParseIntError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self.kind {
            IntErrorKind::Empty => "cannot parse integer from empty string",
            IntErrorKind::InvalidDigit => "invalid digit found in string",
            IntErrorKind::PosOverflow => "number too large to fit in target type",
            IntErrorKind::NegOverflow => "number too small to fit in target type",
            IntErrorKind::Zero => "number would be zero for non-zero type",
        };
        fmt::Display::fmt(s, f)
    }
}

#[cfg(not(hotrust_check))]
impl crate::error::Error for ParseIntError {}

/// Failed integer conversion (`u8::try_from(300u32)`); Debug shows the overflow side as in
/// real core: `TryFromIntError(PosOverflow)`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct TryFromIntError(pub(crate) IntErrorKind);

impl fmt::Display for TryFromIntError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("out of range integral type conversion attempted", f)
    }
}

#[cfg(not(hotrust_check))]
impl crate::error::Error for TryFromIntError {}

impl From<crate::convert::Infallible> for TryFromIntError {
    fn from(x: crate::convert::Infallible) -> TryFromIntError {
        match x {}
    }
}
