//! NonZero<T> (NonZeroU32 = NonZero<u32> etc., as current core).

use crate::cmp::Ordering;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::num::{IntErrorKind, ParseIntError, TryFromIntError};
use crate::str::FromStr;

/// The integer types that have a NonZero variant.
pub trait ZeroablePrimitive: Copy + PartialEq + PartialOrd + Ord + Hash + fmt::Debug + fmt::Display + FromStr<Err = ParseIntError> {
    const ZERO: Self;
}

impl ZeroablePrimitive for u8 {
    const ZERO: u8 = 0;
}
impl ZeroablePrimitive for u16 {
    const ZERO: u16 = 0;
}
impl ZeroablePrimitive for u32 {
    const ZERO: u32 = 0;
}
impl ZeroablePrimitive for u64 {
    const ZERO: u64 = 0;
}
impl ZeroablePrimitive for u128 {
    const ZERO: u128 = 0;
}
impl ZeroablePrimitive for usize {
    const ZERO: usize = 0;
}
impl ZeroablePrimitive for i8 {
    const ZERO: i8 = 0;
}
impl ZeroablePrimitive for i16 {
    const ZERO: i16 = 0;
}
impl ZeroablePrimitive for i32 {
    const ZERO: i32 = 0;
}
impl ZeroablePrimitive for i64 {
    const ZERO: i64 = 0;
}
impl ZeroablePrimitive for i128 {
    const ZERO: i128 = 0;
}
impl ZeroablePrimitive for isize {
    const ZERO: isize = 0;
}

#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct NonZero<T: ZeroablePrimitive>(T);

pub type NonZeroU8 = NonZero<u8>;
pub type NonZeroU16 = NonZero<u16>;
pub type NonZeroU32 = NonZero<u32>;
pub type NonZeroU64 = NonZero<u64>;
pub type NonZeroU128 = NonZero<u128>;
pub type NonZeroUsize = NonZero<usize>;
pub type NonZeroI8 = NonZero<i8>;
pub type NonZeroI16 = NonZero<i16>;
pub type NonZeroI32 = NonZero<i32>;
pub type NonZeroI64 = NonZero<i64>;
pub type NonZeroI128 = NonZero<i128>;
pub type NonZeroIsize = NonZero<isize>;

impl<T: ZeroablePrimitive> NonZero<T> {
    pub fn new(n: T) -> Option<NonZero<T>> {
        if n == T::ZERO {
            None
        } else {
            Some(NonZero(n))
        }
    }
    pub const unsafe fn new_unchecked(n: T) -> NonZero<T> {
        NonZero(n)
    }
    pub const fn get(self) -> T {
        self.0
    }
}

impl<T: ZeroablePrimitive> PartialEq for NonZero<T> {
    fn eq(&self, other: &NonZero<T>) -> bool {
        self.0 == other.0
    }
}
impl<T: ZeroablePrimitive> Eq for NonZero<T> {}
impl<T: ZeroablePrimitive> PartialOrd for NonZero<T> {
    fn partial_cmp(&self, other: &NonZero<T>) -> Option<Ordering> {
        Some(self.0.cmp(&other.0))
    }
}
impl<T: ZeroablePrimitive> Ord for NonZero<T> {
    fn cmp(&self, other: &NonZero<T>) -> Ordering {
        self.0.cmp(&other.0)
    }
}
impl<T: ZeroablePrimitive> Hash for NonZero<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state)
    }
}
impl<T: ZeroablePrimitive> fmt::Debug for NonZero<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}
impl<T: ZeroablePrimitive> fmt::Display for NonZero<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}
impl<T: ZeroablePrimitive> FromStr for NonZero<T> {
    type Err = ParseIntError;
    fn from_str(src: &str) -> Result<NonZero<T>, ParseIntError> {
        match NonZero::new(T::from_str(src)?) {
            Some(v) => Ok(v),
            None => Err(ParseIntError { kind: IntErrorKind::Zero }),
        }
    }
}
impl<T: ZeroablePrimitive> TryFrom<T> for NonZero<T> {
    type Error = TryFromIntError;
    fn try_from(value: T) -> Result<NonZero<T>, TryFromIntError> {
        match NonZero::new(value) {
            Some(v) => Ok(v),
            None => Err(TryFromIntError(IntErrorKind::Zero)),
        }
    }
}

/// `u32::from(nz)` for each primitive (not a blanket impl: one per type).
impl From<NonZero<u8>> for u8 {
    fn from(n: NonZero<u8>) -> u8 {
        n.0
    }
}
impl From<NonZero<u16>> for u16 {
    fn from(n: NonZero<u16>) -> u16 {
        n.0
    }
}
impl From<NonZero<u32>> for u32 {
    fn from(n: NonZero<u32>) -> u32 {
        n.0
    }
}
impl From<NonZero<u64>> for u64 {
    fn from(n: NonZero<u64>) -> u64 {
        n.0
    }
}
impl From<NonZero<usize>> for usize {
    fn from(n: NonZero<usize>) -> usize {
        n.0
    }
}
impl From<NonZero<i32>> for i32 {
    fn from(n: NonZero<i32>) -> i32 {
        n.0
    }
}
impl From<NonZero<i64>> for i64 {
    fn from(n: NonZero<i64>) -> i64 {
        n.0
    }
}
