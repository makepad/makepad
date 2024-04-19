use {crate::trap::Trap, std::mem};

pub(crate) trait RelOps {
    fn eq(self, other: Self) -> Result<i32, Trap>;
    fn ne(self, other: Self) -> Result<i32, Trap>;
    fn lt(self, other: Self) -> Result<i32, Trap>;
    fn le(self, other: Self) -> Result<i32, Trap>;
    fn gt(self, other: Self) -> Result<i32, Trap>;
    fn ge(self, other: Self) -> Result<i32, Trap>;
}

macro_rules! impl_rel_ops {
    ($T:ty) => {
        impl RelOps for $T {
            fn eq(self, other: Self) -> Result<i32, Trap> {
                Ok((self == other).into())
            }

            fn ne(self, other: Self) -> Result<i32, Trap> {
                Ok((self != other).into())
            }

            fn lt(self, other: Self) -> Result<i32, Trap> {
                Ok((self < other).into())
            }

            fn gt(self, other: Self) -> Result<i32, Trap> {
                Ok((self > other).into())
            }

            fn le(self, other: Self) -> Result<i32, Trap> {
                Ok((self <= other).into())
            }

            fn ge(self, other: Self) -> Result<i32, Trap> {
                Ok((self >= other).into())
            }
        }
    };
}

impl_rel_ops!(i32);
impl_rel_ops!(u32);
impl_rel_ops!(i64);
impl_rel_ops!(u64);
impl_rel_ops!(f32);
impl_rel_ops!(f64);

pub(crate) trait IntOps: Sized {
    fn eqz(self) -> Result<i32, Trap>;
    fn clz(self) -> Result<Self, Trap>;
    fn ctz(self) -> Result<Self, Trap>;
    fn popcnt(self) -> Result<Self, Trap>;
    fn add(self, other: Self) -> Result<Self, Trap>;
    fn sub(self, other: Self) -> Result<Self, Trap>;
    fn mul(self, other: Self) -> Result<Self, Trap>;
    fn div(self, other: Self) -> Result<Self, Trap>;
    fn rem(self, other: Self) -> Result<Self, Trap>;
    fn and(self, other: Self) -> Result<Self, Trap>;
    fn or(self, other: Self) -> Result<Self, Trap>;
    fn xor(self, other: Self) -> Result<Self, Trap>;
    fn shl(self, other: Self) -> Result<Self, Trap>;
    fn shr(self, other: Self) -> Result<Self, Trap>;
    fn rotl(self, other: Self) -> Result<Self, Trap>;
    fn rotr(self, other: Self) -> Result<Self, Trap>;
}

macro_rules! impl_int_ops {
    ($T:ty) => {
        impl IntOps for $T {
            fn eqz(self) -> Result<i32, Trap> {
                Ok((self == 0).into())
            }

            fn clz(self) -> Result<Self, Trap> {
                Ok(self.leading_zeros() as Self)
            }

            fn ctz(self) -> Result<Self, Trap> {
                Ok(self.trailing_zeros() as Self)
            }

            fn popcnt(self) -> Result<Self, Trap> {
                Ok(self.count_ones() as Self)
            }

            fn add(self, other: Self) -> Result<Self, Trap> {
                Ok(self.wrapping_add(other))
            }

            fn sub(self, other: Self) -> Result<Self, Trap> {
                Ok(self.wrapping_sub(other))
            }

            fn mul(self, other: Self) -> Result<Self, Trap> {
                Ok(self.wrapping_mul(other))
            }

            fn div(self, other: Self) -> Result<Self, Trap> {
                if other == 0 {
                    return Err(Trap::IntDivByZero);
                }
                match self.overflowing_div(other) {
                    (result, false) => Ok(result),
                    (_, true) => Err(Trap::IntOverflow),
                }
            }

            fn rem(self, other: Self) -> Result<Self, Trap> {
                if other == 0 {
                    return Err(Trap::IntDivByZero);
                }
                Ok(self.wrapping_rem(other))
            }

            fn and(self, other: Self) -> Result<Self, Trap> {
                Ok(self & other)
            }

            fn or(self, other: Self) -> Result<Self, Trap> {
                Ok(self | other)
            }

            fn xor(self, other: Self) -> Result<Self, Trap> {
                Ok(self ^ other)
            }

            fn shl(self, other: Self) -> Result<Self, Trap> {
                Ok(self.wrapping_shl(other as u32))
            }

            fn shr(self, other: Self) -> Result<Self, Trap> {
                Ok(self.wrapping_shr(other as u32))
            }

            fn rotl(self, other: Self) -> Result<Self, Trap> {
                Ok(self.rotate_left(other as u32))
            }

            fn rotr(self, other: Self) -> Result<Self, Trap> {
                Ok(self.rotate_right(other as u32))
            }
        }
    };
}

impl_int_ops!(i32);
impl_int_ops!(u32);
impl_int_ops!(i64);
impl_int_ops!(u64);

pub(crate) trait FloatOps: Sized {
    fn abs(self) -> Result<Self, Trap>;
    fn neg(self) -> Result<Self, Trap>;
    fn ceil(self) -> Result<Self, Trap>;
    fn floor(self) -> Result<Self, Trap>;
    fn trunc(self) -> Result<Self, Trap>;
    fn nearest(self) -> Result<Self, Trap>;
    fn sqrt(self) -> Result<Self, Trap>;
    fn add(self, other: Self) -> Result<Self, Trap>;
    fn sub(self, other: Self) -> Result<Self, Trap>;
    fn mul(self, other: Self) -> Result<Self, Trap>;
    fn div(self, other: Self) -> Result<Self, Trap>;
    fn min(self, other: Self) -> Result<Self, Trap>;
    fn max(self, other: Self) -> Result<Self, Trap>;
    fn copysign(self, other: Self) -> Result<Self, Trap>;
}

macro_rules! impl_float_ops {
    ($T:ty) => {
        impl FloatOps for $T {
            fn abs(self) -> Result<Self, Trap> {
                Ok(self.abs())
            }

            fn neg(self) -> Result<Self, Trap> {
                Ok(-self)
            }

            fn ceil(self) -> Result<Self, Trap> {
                Ok(self.ceil())
            }

            fn floor(self) -> Result<Self, Trap> {
                Ok(self.floor())
            }

            fn trunc(self) -> Result<Self, Trap> {
                Ok(self.trunc())
            }

            fn nearest(self) -> Result<Self, Trap> {
                let round = self.round();
                if self.fract().abs() != 0.5 {
                    Ok(round)
                } else {
                    let rem = round % 2.0;
                    if rem == 1.0 {
                        Ok(self.floor())
                    } else if rem == -1.0 {
                        Ok(self.ceil())
                    } else {
                        Ok(round)
                    }
                }
            }

            fn sqrt(self) -> Result<Self, Trap> {
                Ok(self.sqrt())
            }

            fn add(self, other: Self) -> Result<Self, Trap> {
                Ok(self + other)
            }

            fn sub(self, other: Self) -> Result<Self, Trap> {
                Ok(self - other)
            }

            fn mul(self, other: Self) -> Result<Self, Trap> {
                Ok(self * other)
            }

            fn div(self, other: Self) -> Result<Self, Trap> {
                Ok(self / other)
            }

            fn min(self, other: Self) -> Result<Self, Trap> {
                if self < other {
                    Ok(self)
                } else if other < self {
                    Ok(other)
                } else if self == other {
                    if self.is_sign_negative() && other.is_sign_positive() {
                        Ok(self)
                    } else {
                        Ok(other)
                    }
                } else {
                    Ok(self + other)
                }
            }

            fn max(self, other: Self) -> Result<Self, Trap> {
                if self > other {
                    Ok(self)
                } else if other > self {
                    Ok(other)
                } else if self == other {
                    if self.is_sign_positive() && other.is_sign_negative() {
                        Ok(self)
                    } else {
                        Ok(other)
                    }
                } else {
                    Ok(self + other)
                }
            }

            fn copysign(self, other: Self) -> Result<Self, Trap> {
                let sign_mask = 1 << (mem::size_of::<Self>() * 8) - 1;
                let self_bits = self.to_bits();
                let other_bits = other.to_bits();
                let is_self_sign_set = self_bits & sign_mask != 0;
                let is_other_sign_set = other_bits & sign_mask != 0;
                if is_self_sign_set == is_other_sign_set {
                    Ok(self)
                } else if is_other_sign_set {
                    Ok(Self::from_bits(self_bits | sign_mask))
                } else {
                    Ok(Self::from_bits(self_bits & !sign_mask))
                }
            }
        }
    };
}

impl_float_ops!(f32);
impl_float_ops!(f64);

pub(crate) trait Extend<T>: Sized {
    fn extend(val: T) -> Result<Self, Trap>;
}

macro_rules! impl_extend {
    ($T:ty, $U:ty) => {
        impl Extend<$T> for $U {
            fn extend(val: $T) -> Result<Self, Trap> {
                Ok(val as $U)
            }
        }
    };
}

impl_extend!(i32, i64);
impl_extend!(u32, u64);

pub(crate) trait Wrap<T>: Sized {
    fn wrap(val: T) -> Result<Self, Trap>;
}

macro_rules! impl_wrap {
    ($T:ty, $U:ty) => {
        impl Wrap<$T> for $U {
            fn wrap(val: $T) -> Result<Self, Trap> {
                Ok(val as Self)
            }
        }
    };
}

impl_wrap!(u64, u32);

pub(crate) trait Trunc<T>: Sized {
    fn trunc(val: T) -> Result<Self, Trap>;

    fn trunc_sat(val: T) -> Result<Self, Trap>;
}

macro_rules! impl_trunc {
    ($T:ty, $U:ty, $MIN:literal, $MAX:literal) => {
        impl Trunc<$T> for $U {
            fn trunc(val: $T) -> Result<Self, Trap> {
                if val.is_nan() {
                    return Err(Trap::InvalidConversionToInt);
                }
                if val <= $MIN || val >= $MAX {
                    return Err(Trap::IntOverflow);
                }
                Ok(val as Self)
            }

            fn trunc_sat(val: $T) -> Result<Self, Trap> {
                if val.is_nan() {
                    Ok(0)
                } else if val <= $MIN {
                    Ok(Self::MIN)
                } else if val >= $MAX {
                    Ok(Self::MAX)
                } else {
                    Ok(val as Self)
                }
            }
        }
    };
}

impl_trunc!(f32, i32, -2147483904f32, 2147483648f32);
impl_trunc!(f32, u32, -1f32, 4294967296f32);
impl_trunc!(f64, i32, -2147483649f64, 2147483648f64);
impl_trunc!(f64, u32, -1f64, 4294967296f64);
impl_trunc!(f32, i64, -9223373136366403584f32, 9223372036854775808f32);
impl_trunc!(f32, u64, -1f32, 18446744073709551616f32);
impl_trunc!(f64, i64, -9223372036854777856f64, 9223372036854775808f64);
impl_trunc!(f64, u64, -1f64, 18446744073709551616f64);

pub(crate) trait Promote<T>: Sized {
    fn promote(val: T) -> Result<Self, Trap>;
}

macro_rules! impl_promote {
    ($T:ty, $U:ty) => {
        impl Promote<$T> for $U {
            fn promote(val: $T) -> Result<Self, Trap> {
                Ok(val as Self)
            }
        }
    };
}

impl_promote!(f32, f64);

pub(crate) trait Demote<T>: Sized {
    fn demote(val: T) -> Result<Self, Trap>;
}

macro_rules! impl_demote {
    ($T:ty, $U:ty) => {
        impl Demote<$T> for $U {
            fn demote(val: $T) -> Result<Self, Trap> {
                Ok(val as Self)
            }
        }
    };
}

impl_demote!(f64, f32);

pub(crate) trait Convert<T>: Sized {
    fn convert(val: T) -> Result<Self, Trap>;
}

macro_rules! impl_convert {
    ($T:ty, $U:ty) => {
        impl Convert<$T> for $U {
            fn convert(val: $T) -> Result<Self, Trap> {
                Ok(val as Self)
            }
        }
    };
}

impl_convert!(i32, f32);
impl_convert!(u32, f32);
impl_convert!(i64, f32);
impl_convert!(u64, f32);
impl_convert!(i32, f64);
impl_convert!(u32, f64);
impl_convert!(i64, f64);
impl_convert!(u64, f64);
impl_convert!(f32, i32);
impl_convert!(f32, u32);

pub(crate) trait Reinterpret<T>: Sized {
    fn reinterpret(val: T) -> Result<Self, Trap>;
}

macro_rules! impl_reinterpret {
    ($T:ty, $U:ty) => {
        impl Reinterpret<$T> for $U {
            fn reinterpret(val: $T) -> Result<Self, Trap> {
                Ok(unsafe { mem::transmute(val) })
            }
        }
    };
}

impl_reinterpret!(f32, u32);
impl_reinterpret!(f64, u64);
impl_reinterpret!(u32, f32);
impl_reinterpret!(u64, f64);

pub(crate) trait ExtendN<T>: Sized {
    fn extend_n(self) -> Result<Self, Trap>;
}

macro_rules! impl_extend_n {
    ($T:ty, $U:ty) => {
        impl ExtendN<$T> for $U {
            fn extend_n(self) -> Result<Self, Trap> {
                Ok(self as $T as Self)
            }
        }
    };
}

impl_extend_n!(i8, i32);
impl_extend_n!(i8, i64);
impl_extend_n!(i16, i32);
impl_extend_n!(i16, i64);
impl_extend_n!(i32, i64);
