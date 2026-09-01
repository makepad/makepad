use makepad_micro_serde::{DeBin, DeBinErr, SerBin};
use std::fmt;
use std::num::NonZeroU64;

/// Construction or checked arithmetic failure for an exact rational.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RationalError {
    ZeroDenominator,
    Overflow,
    NonPositiveDuration,
}

impl fmt::Display for RationalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDenominator => f.write_str("rational denominator is zero"),
            Self::Overflow => f.write_str("rational arithmetic overflow"),
            Self::NonPositiveDuration => f.write_str("duration must be positive"),
        }
    }
}

impl std::error::Error for RationalError {}

/// A normalized checked rational. Its denominator is positive.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Rational {
    num: i64,
    den: NonZeroU64,
}

impl Rational {
    pub const ZERO: Self = Self {
        num: 0,
        den: NonZeroU64::MIN,
    };
    pub const ONE: Self = Self {
        num: 1,
        den: NonZeroU64::MIN,
    };
    pub const QUARTER: Self = Self {
        num: 1,
        den: match NonZeroU64::new(4) {
            Some(value) => value,
            None => NonZeroU64::MIN,
        },
    };

    pub fn new(num: i64, den: u64) -> Result<Self, RationalError> {
        let den = NonZeroU64::new(den).ok_or(RationalError::ZeroDenominator)?;
        let divisor = gcd(num.unsigned_abs(), den.get());
        let reduced_num = i128::from(num) / i128::from(divisor);
        let reduced_den = den.get() / divisor;
        Ok(Self {
            num: i64::try_from(reduced_num).map_err(|_| RationalError::Overflow)?,
            den: NonZeroU64::new(reduced_den).ok_or(RationalError::ZeroDenominator)?,
        })
    }

    pub const fn numerator(self) -> i64 {
        self.num
    }

    pub const fn denominator(self) -> u64 {
        self.den.get()
    }

    pub const fn is_positive(self) -> bool {
        self.num > 0
    }

    pub const fn is_zero(self) -> bool {
        self.num == 0
    }

    pub fn checked_neg(self) -> Result<Self, RationalError> {
        let num = self.num.checked_neg().ok_or(RationalError::Overflow)?;
        Self::new(num, self.den.get())
    }

    pub fn checked_add(self, rhs: Self) -> Result<Self, RationalError> {
        let divisor = gcd(self.den.get(), rhs.den.get());
        let left_scale = rhs.den.get() / divisor;
        let right_scale = self.den.get() / divisor;
        let num = i128::from(self.num) * i128::from(left_scale)
            + i128::from(rhs.num) * i128::from(right_scale);
        let den = u128::from(self.den.get()) * u128::from(left_scale);
        Self::from_wide(num, den)
    }

    pub fn checked_sub(self, rhs: Self) -> Result<Self, RationalError> {
        self.checked_add(rhs.checked_neg()?)
    }

    pub fn checked_mul(self, rhs: Self) -> Result<Self, RationalError> {
        let left_cancel = gcd(self.num.unsigned_abs(), rhs.den.get());
        let right_cancel = gcd(rhs.num.unsigned_abs(), self.den.get());
        let left_num = i128::from(self.num) / i128::from(left_cancel);
        let right_num = i128::from(rhs.num) / i128::from(right_cancel);
        let left_den = u128::from(self.den.get() / right_cancel);
        let right_den = u128::from(rhs.den.get() / left_cancel);
        Self::from_wide(left_num * right_num, left_den * right_den)
    }

    pub fn checked_div(self, rhs: Self) -> Result<Self, RationalError> {
        if rhs.num == 0 {
            return Err(RationalError::ZeroDenominator);
        }
        let sign = if rhs.num < 0 { -1_i64 } else { 1_i64 };
        let reciprocal_num = i64::try_from(rhs.den.get()).map_err(|_| RationalError::Overflow)?;
        let reciprocal_num = reciprocal_num
            .checked_mul(sign)
            .ok_or(RationalError::Overflow)?;
        let reciprocal = Self::new(reciprocal_num, rhs.num.unsigned_abs())?;
        self.checked_mul(reciprocal)
    }

    fn from_wide(num: i128, den: u128) -> Result<Self, RationalError> {
        if den == 0 {
            return Err(RationalError::ZeroDenominator);
        }
        let abs_num = num.unsigned_abs();
        let divisor = gcd_u128(abs_num, den);
        let num = num / i128::try_from(divisor).map_err(|_| RationalError::Overflow)?;
        let den = den / divisor;
        let num = i64::try_from(num).map_err(|_| RationalError::Overflow)?;
        let den = u64::try_from(den).map_err(|_| RationalError::Overflow)?;
        Self::new(num, den)
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Rational {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let left = i128::from(self.num) * i128::from(other.den.get());
        let right = i128::from(other.num) * i128::from(self.den.get());
        left.cmp(&right)
    }
}

impl SerBin for Rational {
    fn ser_bin(&self, output: &mut Vec<u8>) {
        self.num.ser_bin(output);
        self.den.get().ser_bin(output);
    }
}

impl DeBin for Rational {
    fn de_bin(offset: &mut usize, input: &[u8]) -> Result<Self, DeBinErr> {
        let num = i64::de_bin(offset, input)?;
        let den = u64::de_bin(offset, input)?;
        let value = Self::new(num, den).map_err(|error| DeBinErr {
            msg: error.to_string(),
            o: *offset,
            l: 0,
            s: input.len(),
        })?;
        if value.num != num || value.den.get() != den {
            return Err(DeBinErr {
                msg: "non-normalized Rational".to_string(),
                o: *offset,
                l: 0,
                s: input.len(),
            });
        }
        Ok(value)
    }
}

const fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    if left == 0 { 1 } else { left }
}

const fn gcd_u128(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    if left == 0 { 1 } else { left }
}

/// An absolute position or displacement measured in whole notes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ScoreTime(pub Rational);

impl ScoreTime {
    pub const ZERO: Self = Self(Rational::ZERO);

    pub fn new(num: i64, den: u64) -> Result<Self, RationalError> {
        Ok(Self(Rational::new(num, den)?))
    }

    pub fn checked_add(self, duration: Duration) -> Result<Self, RationalError> {
        Ok(Self(self.0.checked_add(duration.0)?))
    }

    pub fn checked_add_time(self, other: Self) -> Result<Self, RationalError> {
        Ok(Self(self.0.checked_add(other.0)?))
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, RationalError> {
        Ok(Self(self.0.checked_sub(other.0)?))
    }
}

impl SerBin for ScoreTime {
    fn ser_bin(&self, output: &mut Vec<u8>) {
        self.0.ser_bin(output);
    }
}

impl DeBin for ScoreTime {
    fn de_bin(offset: &mut usize, input: &[u8]) -> Result<Self, DeBinErr> {
        Ok(Self(Rational::de_bin(offset, input)?))
    }
}

/// A strictly positive duration; grace notes use separate timing metadata.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Duration(pub Rational);

impl Duration {
    pub fn new(num: i64, den: u64) -> Result<Self, RationalError> {
        Self::from_rational(Rational::new(num, den)?)
    }

    pub fn from_rational(value: Rational) -> Result<Self, RationalError> {
        if value.is_positive() {
            Ok(Self(value))
        } else {
            Err(RationalError::NonPositiveDuration)
        }
    }

    pub fn checked_add(self, rhs: Self) -> Result<Self, RationalError> {
        Self::from_rational(self.0.checked_add(rhs.0)?)
    }
}

impl SerBin for Duration {
    fn ser_bin(&self, output: &mut Vec<u8>) {
        self.0.ser_bin(output);
    }
}

impl DeBin for Duration {
    fn de_bin(offset: &mut usize, input: &[u8]) -> Result<Self, DeBinErr> {
        Self::from_rational(Rational::de_bin(offset, input)?).map_err(|error| DeBinErr {
            msg: error.to_string(),
            o: *offset,
            l: 0,
            s: input.len(),
        })
    }
}

/// A fractional semitone alteration.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Alter(pub Rational);

impl Alter {
    pub const NATURAL: Self = Self(Rational::ZERO);

    pub fn new(num: i64, den: u64) -> Result<Self, RationalError> {
        Ok(Self(Rational::new(num, den)?))
    }
}

impl SerBin for Alter {
    fn ser_bin(&self, output: &mut Vec<u8>) {
        self.0.ser_bin(output);
    }
}

impl DeBin for Alter {
    fn de_bin(offset: &mut usize, input: &[u8]) -> Result<Self, DeBinErr> {
        Ok(Self(Rational::de_bin(offset, input)?))
    }
}
