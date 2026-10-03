//! core::time::Duration. Behaviour (rounding of float conversions, Debug output, panic
//! messages) follows Rust's core/src/time.rs (MIT/Apache-2.0), rewritten without macros.

use crate::fmt;
use crate::fmt::Write as _;
use crate::iter::Sum;
use crate::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign};

const NANOS_PER_SEC: u32 = 1_000_000_000;
const NANOS_PER_MILLI: u32 = 1_000_000;
const NANOS_PER_MICRO: u32 = 1_000;
const MILLIS_PER_SEC: u64 = 1_000;
const MICROS_PER_SEC: u64 = 1_000_000;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Duration {
    secs: u64,
    nanos: u32, // always < NANOS_PER_SEC
}

impl Duration {
    pub const SECOND: Duration = Duration { secs: 1, nanos: 0 };
    pub const MILLISECOND: Duration = Duration { secs: 0, nanos: NANOS_PER_MILLI };
    pub const MICROSECOND: Duration = Duration { secs: 0, nanos: NANOS_PER_MICRO };
    pub const NANOSECOND: Duration = Duration { secs: 0, nanos: 1 };
    pub const ZERO: Duration = Duration { secs: 0, nanos: 0 };
    pub const MAX: Duration = Duration { secs: u64::MAX, nanos: NANOS_PER_SEC - 1 };

    pub const fn new(secs: u64, nanos: u32) -> Duration {
        if nanos < NANOS_PER_SEC {
            Duration { secs, nanos }
        } else {
            let extra = (nanos / NANOS_PER_SEC) as u64;
            let secs = match secs.checked_add(extra) {
                Some(s) => s,
                None => panic!("overflow in Duration::new"),
            };
            Duration { secs, nanos: nanos % NANOS_PER_SEC }
        }
    }
    pub const fn from_secs(secs: u64) -> Duration {
        Duration { secs, nanos: 0 }
    }
    pub const fn from_millis(millis: u64) -> Duration {
        Duration { secs: millis / MILLIS_PER_SEC, nanos: ((millis % MILLIS_PER_SEC) as u32) * NANOS_PER_MILLI }
    }
    pub const fn from_micros(micros: u64) -> Duration {
        Duration { secs: micros / MICROS_PER_SEC, nanos: ((micros % MICROS_PER_SEC) as u32) * NANOS_PER_MICRO }
    }
    pub const fn from_nanos(nanos: u64) -> Duration {
        Duration { secs: nanos / (NANOS_PER_SEC as u64), nanos: (nanos % (NANOS_PER_SEC as u64)) as u32 }
    }
    pub const fn from_mins(mins: u64) -> Duration {
        if mins > u64::MAX / 60 {
            panic!("overflow in Duration::from_mins");
        }
        Duration::from_secs(mins * 60)
    }
    pub const fn from_hours(hours: u64) -> Duration {
        if hours > u64::MAX / 3600 {
            panic!("overflow in Duration::from_hours");
        }
        Duration::from_secs(hours * 3600)
    }
    pub const fn is_zero(&self) -> bool {
        self.secs == 0 && self.nanos == 0
    }
    pub const fn as_secs(&self) -> u64 {
        self.secs
    }
    pub const fn subsec_millis(&self) -> u32 {
        self.nanos / NANOS_PER_MILLI
    }
    pub const fn subsec_micros(&self) -> u32 {
        self.nanos / NANOS_PER_MICRO
    }
    pub const fn subsec_nanos(&self) -> u32 {
        self.nanos
    }
    pub const fn as_millis(&self) -> u128 {
        self.secs as u128 * MILLIS_PER_SEC as u128 + (self.nanos / NANOS_PER_MILLI) as u128
    }
    pub const fn as_micros(&self) -> u128 {
        self.secs as u128 * MICROS_PER_SEC as u128 + (self.nanos / NANOS_PER_MICRO) as u128
    }
    pub const fn as_nanos(&self) -> u128 {
        self.secs as u128 * NANOS_PER_SEC as u128 + self.nanos as u128
    }
    pub const fn abs_diff(self, other: Duration) -> Duration {
        if let Some(res) = self.checked_sub(other) {
            res
        } else {
            match other.checked_sub(self) {
                Some(r) => r,
                None => panic!("called `Option::unwrap()` on a `None` value"),
            }
        }
    }
    pub const fn checked_add(self, rhs: Duration) -> Option<Duration> {
        let Some(mut secs) = self.secs.checked_add(rhs.secs) else {
            return None;
        };
        let mut nanos = self.nanos + rhs.nanos;
        if nanos >= NANOS_PER_SEC {
            nanos -= NANOS_PER_SEC;
            let Some(s) = secs.checked_add(1) else {
                return None;
            };
            secs = s;
        }
        Some(Duration { secs, nanos })
    }
    pub const fn saturating_add(self, rhs: Duration) -> Duration {
        match self.checked_add(rhs) {
            Some(res) => res,
            None => Duration::MAX,
        }
    }
    pub const fn checked_sub(self, rhs: Duration) -> Option<Duration> {
        let Some(mut secs) = self.secs.checked_sub(rhs.secs) else {
            return None;
        };
        let nanos = if self.nanos >= rhs.nanos {
            self.nanos - rhs.nanos
        } else if let Some(s) = secs.checked_sub(1) {
            secs = s;
            self.nanos + NANOS_PER_SEC - rhs.nanos
        } else {
            return None;
        };
        Some(Duration { secs, nanos })
    }
    pub const fn saturating_sub(self, rhs: Duration) -> Duration {
        match self.checked_sub(rhs) {
            Some(res) => res,
            None => Duration::ZERO,
        }
    }
    pub const fn checked_mul(self, rhs: u32) -> Option<Duration> {
        let total_nanos = self.nanos as u64 * rhs as u64;
        let extra_secs = total_nanos / (NANOS_PER_SEC as u64);
        let nanos = (total_nanos % (NANOS_PER_SEC as u64)) as u32;
        if let Some(s) = self.secs.checked_mul(rhs as u64) {
            if let Some(secs) = s.checked_add(extra_secs) {
                return Some(Duration { secs, nanos });
            }
        }
        None
    }
    pub const fn saturating_mul(self, rhs: u32) -> Duration {
        match self.checked_mul(rhs) {
            Some(res) => res,
            None => Duration::MAX,
        }
    }
    pub const fn checked_div(self, rhs: u32) -> Option<Duration> {
        if rhs == 0 {
            return None;
        }
        let secs = self.secs / (rhs as u64);
        let extra_secs = self.secs % (rhs as u64);
        let mut nanos = self.nanos / rhs;
        let extra_nanos = self.nanos % rhs;
        nanos += ((extra_secs * (NANOS_PER_SEC as u64) + extra_nanos as u64) / (rhs as u64)) as u32;
        Some(Duration { secs, nanos })
    }
    pub const fn as_secs_f64(&self) -> f64 {
        (self.secs as f64) + (self.nanos as f64) / (NANOS_PER_SEC as f64)
    }
    pub const fn as_secs_f32(&self) -> f32 {
        (self.secs as f32) + (self.nanos as f32) / (NANOS_PER_SEC as f32)
    }
    pub const fn as_millis_f64(&self) -> f64 {
        (self.secs as f64) * (MILLIS_PER_SEC as f64) + (self.nanos as f64) / (NANOS_PER_MILLI as f64)
    }
    pub const fn as_millis_f32(&self) -> f32 {
        (self.secs as f32) * (MILLIS_PER_SEC as f32) + (self.nanos as f32) / (NANOS_PER_MILLI as f32)
    }
    pub fn from_secs_f64(secs: f64) -> Duration {
        match Duration::try_from_secs_f64(secs) {
            Ok(v) => v,
            Err(e) => panic!("{}", e),
        }
    }
    pub fn from_secs_f32(secs: f32) -> Duration {
        match Duration::try_from_secs_f32(secs) {
            Ok(v) => v,
            Err(e) => panic!("{}", e),
        }
    }
    pub fn mul_f64(self, rhs: f64) -> Duration {
        Duration::from_secs_f64(rhs * self.as_secs_f64())
    }
    pub fn mul_f32(self, rhs: f32) -> Duration {
        Duration::from_secs_f64(rhs as f64 * self.as_secs_f64())
    }
    pub fn div_f64(self, rhs: f64) -> Duration {
        Duration::from_secs_f64(self.as_secs_f64() / rhs)
    }
    pub fn div_f32(self, rhs: f32) -> Duration {
        Duration::from_secs_f64(self.as_secs_f64() / rhs as f64)
    }
    pub const fn div_duration_f64(self, rhs: Duration) -> f64 {
        let self_nanos = (self.secs as f64) * (NANOS_PER_SEC as f64) + (self.nanos as f64);
        let rhs_nanos = (rhs.secs as f64) * (NANOS_PER_SEC as f64) + (rhs.nanos as f64);
        self_nanos / rhs_nanos
    }
    pub const fn div_duration_f32(self, rhs: Duration) -> f32 {
        let self_nanos = (self.secs as f32) * (NANOS_PER_SEC as f32) + (self.nanos as f32);
        let rhs_nanos = (rhs.secs as f32) * (NANOS_PER_SEC as f32) + (rhs.nanos as f32);
        self_nanos / rhs_nanos
    }

    /// Exact conversion with round-half-to-even on the nanosecond (Rust's try_from_secs!
    /// with f64 parameters: 52 mantissa bits, 11 exponent bits, offset 44, u128 products).
    pub fn try_from_secs_f64(secs: f64) -> Result<Duration, TryFromFloatSecsError> {
        const MANT_BITS: i16 = 52;
        const MIN_EXP: i16 = 1 - (1i16 << 11) / 2;
        const MANT_MASK: u64 = (1u64 << 52) - 1;
        const EXP_MASK: u64 = (1u64 << 11) - 1;
        if secs < 0.0 {
            return Err(TryFromFloatSecsError { kind: TryFromFloatSecsErrorKind::Negative });
        }
        let bits = secs.to_bits();
        let mant = (bits & MANT_MASK) | (MANT_MASK + 1);
        let exp = ((bits >> 52) & EXP_MASK) as i16 + MIN_EXP;
        let (s, n) = if exp < -31 {
            (0u64, 0u32)
        } else if exp < 0 {
            let t = (mant as u128) << ((44 + exp) as u32);
            let nanos_offset = (MANT_BITS + 44) as u32;
            let nanos_tmp = (NANOS_PER_SEC as u128) * t;
            let nanos = (nanos_tmp >> nanos_offset) as u32;
            let add = round_up_u128(nanos_tmp, nanos_offset, nanos);
            let nanos = nanos + add as u32;
            if nanos != NANOS_PER_SEC {
                (0, nanos)
            } else {
                (1, 0)
            }
        } else if exp < MANT_BITS {
            let secs = mant >> ((MANT_BITS - exp) as u32);
            let t = ((mant << (exp as u32)) & MANT_MASK) as u128;
            let nanos_offset = MANT_BITS as u32;
            let nanos_tmp = (NANOS_PER_SEC as u128) * t;
            let nanos = (nanos_tmp >> nanos_offset) as u32;
            let add = round_up_u128(nanos_tmp, nanos_offset, nanos);
            let nanos = nanos + add as u32;
            if nanos != NANOS_PER_SEC {
                (secs, nanos)
            } else {
                (secs + 1, 0)
            }
        } else if exp < 64 {
            (mant << ((exp - MANT_BITS) as u32), 0)
        } else {
            return Err(TryFromFloatSecsError { kind: TryFromFloatSecsErrorKind::OverflowOrNan });
        };
        Ok(Duration::new(s, n))
    }

    /// Same as try_from_secs_f64 with f32 parameters (23, 8, offset 41, u64 products).
    pub fn try_from_secs_f32(secs: f32) -> Result<Duration, TryFromFloatSecsError> {
        const MANT_BITS: i16 = 23;
        const MIN_EXP: i16 = 1 - (1i16 << 8) / 2;
        const MANT_MASK: u32 = (1u32 << 23) - 1;
        const EXP_MASK: u32 = (1u32 << 8) - 1;
        if secs < 0.0 {
            return Err(TryFromFloatSecsError { kind: TryFromFloatSecsErrorKind::Negative });
        }
        let bits = secs.to_bits();
        let mant = (bits & MANT_MASK) | (MANT_MASK + 1);
        let exp = ((bits >> 23) & EXP_MASK) as i16 + MIN_EXP;
        let (s, n) = if exp < -31 {
            (0u64, 0u32)
        } else if exp < 0 {
            let t = (mant as u64) << ((41 + exp) as u32);
            let nanos_offset = (MANT_BITS + 41) as u32;
            let nanos_tmp = (NANOS_PER_SEC as u128) * (t as u128);
            let nanos = (nanos_tmp >> nanos_offset) as u32;
            let add = round_up_u128(nanos_tmp, nanos_offset, nanos);
            (0, nanos + add as u32)
        } else if exp < MANT_BITS {
            let secs = (mant >> ((MANT_BITS - exp) as u32)) as u64;
            let t = ((mant << (exp as u32)) & MANT_MASK) as u64;
            let nanos_offset = MANT_BITS as u32;
            let nanos_tmp = (NANOS_PER_SEC as u64) * t;
            let nanos = (nanos_tmp >> nanos_offset) as u32;
            let add = round_up_u128(nanos_tmp as u128, nanos_offset, nanos);
            (secs, nanos + add as u32)
        } else if exp < 64 {
            ((mant as u64) << ((exp - MANT_BITS) as u32), 0)
        } else {
            return Err(TryFromFloatSecsError { kind: TryFromFloatSecsErrorKind::OverflowOrNan });
        };
        Ok(Duration::new(s, n))
    }
}

/// Round-half-to-even decision for the bits below `offset` of `tmp`.
fn round_up_u128(tmp: u128, offset: u32, nanos: u32) -> bool {
    let rem_mask = (1u128 << offset) - 1;
    let rem_msb_mask = 1u128 << (offset - 1);
    let rem = tmp & rem_mask;
    let is_tie = rem == rem_msb_mask;
    let is_even = (nanos & 1) == 0;
    let rem_msb = tmp & rem_msb_mask == 0;
    !(rem_msb || (is_even && is_tie))
}

impl Add for Duration {
    type Output = Duration;
    fn add(self, rhs: Duration) -> Duration {
        match self.checked_add(rhs) {
            Some(d) => d,
            None => panic!("overflow when adding durations"),
        }
    }
}
impl AddAssign for Duration {
    fn add_assign(&mut self, rhs: Duration) {
        *self = *self + rhs;
    }
}
impl Sub for Duration {
    type Output = Duration;
    fn sub(self, rhs: Duration) -> Duration {
        match self.checked_sub(rhs) {
            Some(d) => d,
            None => panic!("overflow when subtracting durations"),
        }
    }
}
impl SubAssign for Duration {
    fn sub_assign(&mut self, rhs: Duration) {
        *self = *self - rhs;
    }
}
impl Mul<u32> for Duration {
    type Output = Duration;
    fn mul(self, rhs: u32) -> Duration {
        match self.checked_mul(rhs) {
            Some(d) => d,
            None => panic!("overflow when multiplying duration by scalar"),
        }
    }
}
impl Mul<Duration> for u32 {
    type Output = Duration;
    fn mul(self, rhs: Duration) -> Duration {
        rhs * self
    }
}
impl MulAssign<u32> for Duration {
    fn mul_assign(&mut self, rhs: u32) {
        *self = *self * rhs;
    }
}
impl Div<u32> for Duration {
    type Output = Duration;
    fn div(self, rhs: u32) -> Duration {
        match self.checked_div(rhs) {
            Some(d) => d,
            None => panic!("divide by zero error when dividing duration by scalar"),
        }
    }
}
impl DivAssign<u32> for Duration {
    fn div_assign(&mut self, rhs: u32) {
        *self = *self / rhs;
    }
}

fn sum_step(total_secs: &mut u64, total_nanos: &mut u64, d: &Duration) {
    *total_secs = match total_secs.checked_add(d.secs) {
        Some(s) => s,
        None => panic!("overflow in iter::sum over durations"),
    };
    *total_nanos = match total_nanos.checked_add(d.nanos as u64) {
        Some(n) => n,
        None => {
            *total_secs = match total_secs.checked_add(*total_nanos / NANOS_PER_SEC as u64) {
                Some(s) => s,
                None => panic!("overflow in iter::sum over durations"),
            };
            (*total_nanos % NANOS_PER_SEC as u64) + d.nanos as u64
        }
    };
}

fn sum_finish(total_secs: u64, total_nanos: u64) -> Duration {
    let secs = match total_secs.checked_add(total_nanos / NANOS_PER_SEC as u64) {
        Some(s) => s,
        None => panic!("overflow in iter::sum over durations"),
    };
    Duration::new(secs, (total_nanos % NANOS_PER_SEC as u64) as u32)
}

impl Sum for Duration {
    fn sum<I: Iterator<Item = Duration>>(iter: I) -> Duration {
        let mut s = 0u64;
        let mut n = 0u64;
        for d in iter {
            sum_step(&mut s, &mut n, &d);
        }
        sum_finish(s, n)
    }
}

impl<'a> Sum<&'a Duration> for Duration {
    fn sum<I: Iterator<Item = &'a Duration>>(iter: I) -> Duration {
        let mut s = 0u64;
        let mut n = 0u64;
        for d in iter {
            sum_step(&mut s, &mut n, d);
        }
        sum_finish(s, n)
    }
}

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let prefix = if f.sign_plus() { "+" } else { "" };
        if self.secs > 0 {
            fmt_decimal(f, self.secs, self.nanos, NANOS_PER_SEC / 10, prefix, "s")
        } else if self.nanos >= NANOS_PER_MILLI {
            fmt_decimal(f, (self.nanos / NANOS_PER_MILLI) as u64, self.nanos % NANOS_PER_MILLI, NANOS_PER_MILLI / 10, prefix, "ms")
        } else if self.nanos >= NANOS_PER_MICRO {
            fmt_decimal(f, (self.nanos / NANOS_PER_MICRO) as u64, self.nanos % NANOS_PER_MICRO, NANOS_PER_MICRO / 10, prefix, "µs")
        } else {
            fmt_decimal(f, self.nanos as u64, 0, 1, prefix, "ns")
        }
    }
}

/// Decimal digits of `fractional_part / (divisor * 10)` honouring precision (rounded half up,
/// carrying into the integer part) and width (fill/alignment, default left).
fn fmt_decimal(f: &mut fmt::Formatter, integer_part: u64, fractional_part: u32, divisor: u32, prefix: &str, postfix: &str) -> fmt::Result {
    let mut frac = fractional_part;
    let mut div = divisor;
    let mut buf = [b'0'; 9];
    let mut pos = 0usize;
    let max_end = match f.precision() {
        Some(p) => if p < 9 { p } else { 9 },
        None => 9,
    };
    while frac > 0 && pos < max_end {
        buf[pos] = b'0' + (frac / div) as u8;
        frac %= div;
        div /= 10;
        pos += 1;
    }
    // None = integer part overflowed u64 (it is then u64::MAX + 1)
    let mut int_part = Some(integer_part);
    if frac > 0 && frac >= div * 5 {
        let mut rev_pos = pos;
        let mut carry = true;
        while carry && rev_pos > 0 {
            rev_pos -= 1;
            if buf[rev_pos] < b'9' {
                buf[rev_pos] += 1;
                carry = false;
            } else {
                buf[rev_pos] = b'0';
            }
        }
        if carry {
            int_part = integer_part.checked_add(1);
        }
    }
    let end = match f.precision() {
        Some(p) => if p < 9 { p } else { 9 },
        None => pos,
    };
    let frac_w = match f.precision() {
        Some(p) => p,
        None => pos,
    };
    let padding = match f.width() {
        None => 0,
        Some(requested_w) => {
            let mut actual_w = prefix.len() + postfix.chars().count();
            match int_part {
                Some(i) => actual_w += decimal_digits(i),
                None => actual_w += 20,
            }
            if end > 0 {
                actual_w += 1 + frac_w;
            }
            if requested_w <= actual_w {
                0
            } else {
                requested_w - actual_w
            }
        }
    };
    let (pre, post) = match f.align() {
        Some(fmt::Alignment::Right) => (padding, 0),
        Some(fmt::Alignment::Center) => (padding / 2, (padding + 1) / 2),
        _ => (0, padding),
    };
    let fill = f.fill();
    for _ in 0..pre {
        f.write_char(fill)?;
    }
    f.write_str(prefix)?;
    match int_part {
        Some(i) => write!(f, "{}", i)?,
        None => f.write_str("18446744073709551616")?,
    }
    if end > 0 {
        f.write_str(".")?;
        let digits = unsafe { crate::str::from_utf8_unchecked(&buf[..end]) };
        f.write_str(digits)?;
        let mut k = end;
        while k < frac_w {
            f.write_str("0")?;
            k += 1;
        }
    }
    f.write_str(postfix)?;
    for _ in 0..post {
        f.write_char(fill)?;
    }
    Ok(())
}

fn decimal_digits(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 10 {
        v /= 10;
        n += 1;
    }
    n
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TryFromFloatSecsError {
    kind: TryFromFloatSecsErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TryFromFloatSecsErrorKind {
    Negative,
    OverflowOrNan,
}

impl fmt::Display for TryFromFloatSecsError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self.kind {
            TryFromFloatSecsErrorKind::Negative => "cannot convert float seconds to Duration: value is negative",
            TryFromFloatSecsErrorKind::OverflowOrNan => "cannot convert float seconds to Duration: value is either too big or NaN",
        };
        fmt::Display::fmt(s, f)
    }
}

impl crate::error::Error for TryFromFloatSecsError {}
