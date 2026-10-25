//! Wrapping<T> and Saturating<T>. Integer `+ - * << >>` and unary `-` already wrap in Relax
//! (release semantics, as rustc --release), so Wrapping's operators forward to them.

use crate::fmt;
use crate::ops::*;

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Default, Hash)]
#[repr(transparent)]
pub struct Wrapping<T>(pub T);

impl<T: fmt::Debug> fmt::Debug for Wrapping<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl<T: fmt::Display> fmt::Display for Wrapping<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl<T: fmt::LowerHex> fmt::LowerHex for Wrapping<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl<T: fmt::UpperHex> fmt::UpperHex for Wrapping<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl<T: fmt::Binary> fmt::Binary for Wrapping<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl<T: fmt::Octal> fmt::Octal for Wrapping<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: Add<Output = T>> Add for Wrapping<T> {
    type Output = Wrapping<T>;
    fn add(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 + rhs.0)
    }
}
impl<T: Sub<Output = T>> Sub for Wrapping<T> {
    type Output = Wrapping<T>;
    fn sub(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 - rhs.0)
    }
}
impl<T: Mul<Output = T>> Mul for Wrapping<T> {
    type Output = Wrapping<T>;
    fn mul(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 * rhs.0)
    }
}
impl<T: Div<Output = T>> Div for Wrapping<T> {
    type Output = Wrapping<T>;
    fn div(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 / rhs.0)
    }
}
impl<T: Rem<Output = T>> Rem for Wrapping<T> {
    type Output = Wrapping<T>;
    fn rem(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 % rhs.0)
    }
}
impl<T: BitXor<Output = T>> BitXor for Wrapping<T> {
    type Output = Wrapping<T>;
    fn bitxor(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 ^ rhs.0)
    }
}
impl<T: BitOr<Output = T>> BitOr for Wrapping<T> {
    type Output = Wrapping<T>;
    fn bitor(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 | rhs.0)
    }
}
impl<T: BitAnd<Output = T>> BitAnd for Wrapping<T> {
    type Output = Wrapping<T>;
    fn bitand(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 & rhs.0)
    }
}
impl<T: Shl<usize, Output = T>> Shl<usize> for Wrapping<T> {
    type Output = Wrapping<T>;
    fn shl(self, rhs: usize) -> Wrapping<T> {
        Wrapping(self.0 << rhs)
    }
}
impl<T: Shr<usize, Output = T>> Shr<usize> for Wrapping<T> {
    type Output = Wrapping<T>;
    fn shr(self, rhs: usize) -> Wrapping<T> {
        Wrapping(self.0 >> rhs)
    }
}
impl<T: Neg<Output = T>> Neg for Wrapping<T> {
    type Output = Wrapping<T>;
    fn neg(self) -> Wrapping<T> {
        Wrapping(-self.0)
    }
}
impl<T: Not<Output = T>> Not for Wrapping<T> {
    type Output = Wrapping<T>;
    fn not(self) -> Wrapping<T> {
        Wrapping(!self.0)
    }
}
impl<T: Add<Output = T> + Copy> AddAssign for Wrapping<T> {
    fn add_assign(&mut self, rhs: Wrapping<T>) {
        self.0 = self.0 + rhs.0;
    }
}
impl<T: Sub<Output = T> + Copy> SubAssign for Wrapping<T> {
    fn sub_assign(&mut self, rhs: Wrapping<T>) {
        self.0 = self.0 - rhs.0;
    }
}
impl<T: Mul<Output = T> + Copy> MulAssign for Wrapping<T> {
    fn mul_assign(&mut self, rhs: Wrapping<T>) {
        self.0 = self.0 * rhs.0;
    }
}
impl<T: BitXor<Output = T> + Copy> BitXorAssign for Wrapping<T> {
    fn bitxor_assign(&mut self, rhs: Wrapping<T>) {
        self.0 = self.0 ^ rhs.0;
    }
}
impl<T: BitOr<Output = T> + Copy> BitOrAssign for Wrapping<T> {
    fn bitor_assign(&mut self, rhs: Wrapping<T>) {
        self.0 = self.0 | rhs.0;
    }
}
impl<T: BitAnd<Output = T> + Copy> BitAndAssign for Wrapping<T> {
    fn bitand_assign(&mut self, rhs: Wrapping<T>) {
        self.0 = self.0 & rhs.0;
    }
}

/// Saturating<T> (rare in client programs): operations through the integer's saturating methods are
/// written per use site; only the container and formatting are provided.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Default, Hash)]
#[repr(transparent)]
pub struct Saturating<T>(pub T);

impl<T: fmt::Debug> fmt::Debug for Saturating<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl<T: fmt::Display> fmt::Display for Saturating<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

macro_rules! saturating_impl {
    ($($t:ty)*) => ($(
        impl Add for Saturating<$t> {
            type Output = Saturating<$t>;
            fn add(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0.saturating_add(rhs.0))
            }
        }
        impl Add<$t> for Saturating<$t> {
            type Output = Saturating<$t>;
            fn add(self, rhs: $t) -> Saturating<$t> {
                Saturating(self.0.saturating_add(rhs))
            }
        }
        impl AddAssign for Saturating<$t> {
            fn add_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0.saturating_add(rhs.0);
            }
        }
        impl AddAssign<$t> for Saturating<$t> {
            fn add_assign(&mut self, rhs: $t) {
                self.0 = self.0.saturating_add(rhs);
            }
        }
        impl Sub for Saturating<$t> {
            type Output = Saturating<$t>;
            fn sub(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0.saturating_sub(rhs.0))
            }
        }
        impl Sub<$t> for Saturating<$t> {
            type Output = Saturating<$t>;
            fn sub(self, rhs: $t) -> Saturating<$t> {
                Saturating(self.0.saturating_sub(rhs))
            }
        }
        impl SubAssign for Saturating<$t> {
            fn sub_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0.saturating_sub(rhs.0);
            }
        }
        impl SubAssign<$t> for Saturating<$t> {
            fn sub_assign(&mut self, rhs: $t) {
                self.0 = self.0.saturating_sub(rhs);
            }
        }
        impl Mul for Saturating<$t> {
            type Output = Saturating<$t>;
            fn mul(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0.saturating_mul(rhs.0))
            }
        }
        impl Mul<$t> for Saturating<$t> {
            type Output = Saturating<$t>;
            fn mul(self, rhs: $t) -> Saturating<$t> {
                Saturating(self.0.saturating_mul(rhs))
            }
        }
        impl MulAssign for Saturating<$t> {
            fn mul_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0.saturating_mul(rhs.0);
            }
        }
        impl MulAssign<$t> for Saturating<$t> {
            fn mul_assign(&mut self, rhs: $t) {
                self.0 = self.0.saturating_mul(rhs);
            }
        }
        impl Div for Saturating<$t> {
            type Output = Saturating<$t>;
            fn div(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0.saturating_div(rhs.0))
            }
        }
        impl Div<$t> for Saturating<$t> {
            type Output = Saturating<$t>;
            fn div(self, rhs: $t) -> Saturating<$t> {
                Saturating(self.0.saturating_div(rhs))
            }
        }
        impl DivAssign for Saturating<$t> {
            fn div_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0.saturating_div(rhs.0);
            }
        }
        impl DivAssign<$t> for Saturating<$t> {
            fn div_assign(&mut self, rhs: $t) {
                self.0 = self.0.saturating_div(rhs);
            }
        }
        impl Rem for Saturating<$t> {
            type Output = Saturating<$t>;
            fn rem(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0 % rhs.0)
            }
        }
        impl Rem<$t> for Saturating<$t> {
            type Output = Saturating<$t>;
            fn rem(self, rhs: $t) -> Saturating<$t> {
                Saturating(self.0 % rhs)
            }
        }
        impl RemAssign for Saturating<$t> {
            fn rem_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0 % rhs.0;
            }
        }
        impl RemAssign<$t> for Saturating<$t> {
            fn rem_assign(&mut self, rhs: $t) {
                self.0 = self.0 % rhs;
            }
        }
        impl BitXor for Saturating<$t> {
            type Output = Saturating<$t>;
            fn bitxor(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0 ^ rhs.0)
            }
        }
        impl BitXor<$t> for Saturating<$t> {
            type Output = Saturating<$t>;
            fn bitxor(self, rhs: $t) -> Saturating<$t> {
                Saturating(self.0 ^ rhs)
            }
        }
        impl BitXorAssign for Saturating<$t> {
            fn bitxor_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0 ^ rhs.0;
            }
        }
        impl BitXorAssign<$t> for Saturating<$t> {
            fn bitxor_assign(&mut self, rhs: $t) {
                self.0 = self.0 ^ rhs;
            }
        }
        impl BitOr for Saturating<$t> {
            type Output = Saturating<$t>;
            fn bitor(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0 | rhs.0)
            }
        }
        impl BitOr<$t> for Saturating<$t> {
            type Output = Saturating<$t>;
            fn bitor(self, rhs: $t) -> Saturating<$t> {
                Saturating(self.0 | rhs)
            }
        }
        impl BitOrAssign for Saturating<$t> {
            fn bitor_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0 | rhs.0;
            }
        }
        impl BitOrAssign<$t> for Saturating<$t> {
            fn bitor_assign(&mut self, rhs: $t) {
                self.0 = self.0 | rhs;
            }
        }
        impl BitAnd for Saturating<$t> {
            type Output = Saturating<$t>;
            fn bitand(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0 & rhs.0)
            }
        }
        impl BitAnd<$t> for Saturating<$t> {
            type Output = Saturating<$t>;
            fn bitand(self, rhs: $t) -> Saturating<$t> {
                Saturating(self.0 & rhs)
            }
        }
        impl BitAndAssign for Saturating<$t> {
            fn bitand_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0 & rhs.0;
            }
        }
        impl BitAndAssign<$t> for Saturating<$t> {
            fn bitand_assign(&mut self, rhs: $t) {
                self.0 = self.0 & rhs;
            }
        }
        impl Not for Saturating<$t> {
            type Output = Saturating<$t>;
            fn not(self) -> Saturating<$t> {
                Saturating(!self.0)
            }
        }
    )*)
}

saturating_impl! { usize u8 u16 u32 u64 u128 isize i8 i16 i32 i64 i128 }
