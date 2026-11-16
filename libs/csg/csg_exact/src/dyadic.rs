use crate::expansion::Expansion;
use std::cmp::Ordering;

const INLINE_LIMBS: usize = 6;
const SCRATCH_LIMBS: usize = 16;

#[derive(Clone, Debug)]
enum Limbs {
    Inline { len: u8, data: [u64; INLINE_LIMBS] },
    Heap(Vec<u64>),
}

impl Limbs {
    fn from_slice(limbs: &[u64]) -> Self {
        if limbs.len() <= INLINE_LIMBS {
            let mut data = [0; INLINE_LIMBS];
            data[..limbs.len()].copy_from_slice(limbs);
            Self::Inline { len: limbs.len() as u8, data }
        } else {
            Self::Heap(limbs.to_vec())
        }
    }

    fn as_slice(&self) -> &[u64] {
        match self {
            Self::Inline { len, data } => &data[..usize::from(*len)],
            Self::Heap(limbs) => limbs,
        }
    }

    fn len(&self) -> usize {
        self.as_slice().len()
    }

    fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }
}

enum LimbBuffer<'a> {
    Stack(&'a mut [u64]),
    Heap(Vec<u64>),
}

impl<'a> LimbBuffer<'a> {
    fn new(len: usize, stack: &'a mut [u64; SCRATCH_LIMBS]) -> Self {
        if len <= stack.len() {
            let limbs = &mut stack[..len];
            limbs.fill(0);
            Self::Stack(limbs)
        } else {
            Self::Heap(vec![0; len])
        }
    }

    fn as_slice(&self) -> &[u64] {
        match self {
            Self::Stack(limbs) => limbs,
            Self::Heap(limbs) => limbs,
        }
    }

    fn as_mut_slice(&mut self) -> &mut [u64] {
        match self {
            Self::Stack(limbs) => limbs,
            Self::Heap(limbs) => limbs,
        }
    }

    fn into_limbs(self, len: usize) -> Limbs {
        match self {
            Self::Heap(mut limbs) if len > INLINE_LIMBS => {
                limbs.truncate(len);
                Limbs::Heap(limbs)
            }
            buffer => Limbs::from_slice(&buffer.as_slice()[..len]),
        }
    }
}

/// An exact dyadic rational, (-1)^negative * magnitude * 2^exponent, with the magnitude in
/// little-endian u64 limbs. It has no exponent range, so no product underflows or overflows.
#[derive(Clone, Debug)]
pub struct Dyadic {
    negative: bool,
    limbs: Limbs,
    exponent: i64,
}

impl PartialEq for Dyadic {
    fn eq(&self, other: &Self) -> bool {
        self.negative == other.negative
            && self.exponent == other.exponent
            && self.limbs.as_slice() == other.limbs.as_slice()
    }
}

impl Eq for Dyadic {}

impl Dyadic {
    pub fn zero() -> Dyadic {
        Dyadic {
            negative: false,
            limbs: Limbs::from_slice(&[]),
            exponent: 0,
        }
    }

    pub fn from_f64(v: f64) -> Dyadic {
        assert!(v.is_finite(), "a dyadic input must be finite");
        let bits = v.to_bits();
        let encoded = ((bits >> 52) & 0x7ff) as i64;
        let fraction = bits & ((1_u64 << 52) - 1);
        let (magnitude, exponent) = if encoded == 0 {
            (fraction, -1074)
        } else {
            (fraction | (1_u64 << 52), encoded - 1023 - 52)
        };
        Self::normalized(bits >> 63 != 0, LimbBuffer::Stack(&mut [magnitude]), exponent)
    }

    pub fn from_expansion(e: &Expansion) -> Dyadic {
        e.components.iter().fold(Self::zero(), |sum, &component| {
            sum.add(&Self::from_f64(component))
        })
    }

    pub fn add(&self, other: &Dyadic) -> Dyadic {
        self.add_signed(other, other.negative)
    }

    pub fn sub(&self, other: &Dyadic) -> Dyadic {
        self.add_signed(other, !other.negative)
    }

    pub fn mul(&self, other: &Dyadic) -> Dyadic {
        if self.is_zero() || other.is_zero() {
            return Self::zero();
        }
        let mut stack = [0; SCRATCH_LIMBS];
        let mut buffer = LimbBuffer::new(self.limbs.len() + other.limbs.len(), &mut stack);
        let limbs = buffer.as_mut_slice();
        for (i, &a) in self.limbs.as_slice().iter().enumerate() {
            let mut carry = 0_u128;
            for (j, &b) in other.limbs.as_slice().iter().enumerate() {
                let product = u128::from(a) * u128::from(b) + u128::from(limbs[i + j]) + carry;
                limbs[i + j] = product as u64;
                carry = product >> 64;
            }
            limbs[i + other.limbs.len()] = carry as u64;
        }
        Self::normalized(self.negative != other.negative, buffer, self.exponent + other.exponent)
    }

    pub fn negate(&self) -> Dyadic {
        let mut result = self.clone();
        if !result.is_zero() {
            result.negative = !result.negative;
        }
        result
    }

    pub fn sign(&self) -> i32 {
        if self.is_zero() {
            0
        } else if self.negative {
            -1
        } else {
            1
        }
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    /// Round toward zero to at most 53 bits, preserving representable subnormals.
    pub fn estimate(&self) -> f64 {
        if self.is_zero() {
            return 0.0;
        }
        let (top, exponent) = self.leading();
        let significand = top >> 11;
        let bits = if exponent > 1023
            || (exponent == 1023 && significand == (1_u64 << 53) - 1
                && i128::from(self.exponent) < exponent - 52)
        {
            0x7ff_u64 << 52
        } else if exponent >= -1022 {
            ((exponent + 1023) as u64) << 52 | (significand & ((1_u64 << 52) - 1))
        } else if exponent >= -1074 {
            top >> (-exponent - 1011) as u32
        } else {
            0
        };
        f64::from_bits(bits | (u64::from(self.negative) << 63))
    }

    /// Divide with relative error below 2^-50 for normal quotients.
    pub fn ratio(&self, den: &Dyadic) -> f64 {
        assert!(!den.is_zero(), "a dyadic denominator must be nonzero");
        if self.is_zero() {
            return 0.0;
        }
        let (a, ae) = self.leading();
        let (b, be) = den.leading();
        // Each discarded tail is below 2^-63 relative; two casts and division add three roundings.
        let mut fraction = a as f64 / b as f64;
        let mut exponent = ae - be;
        if fraction < 1.0 {
            fraction *= 2.0;
            exponent -= 1;
        } else if fraction >= 2.0 {
            fraction *= 0.5;
            exponent += 1;
        }
        let magnitude = if exponent > 1023 {
            // Rounding at the upper boundary must not turn a normal quotient into infinity.
            if exponent == 1024 && fraction == 1.0 {
                f64::MAX
            } else {
                f64::INFINITY
            }
        } else if exponent >= -1022 {
            fraction * f64::from_bits(((exponent + 1023) as u64) << 52)
        } else if exponent >= -1075 {
            // Scale through a normal power of two so only the final result can underflow.
            fraction * f64::from_bits(((exponent + 2045) as u64) << 52) * f64::MIN_POSITIVE
        } else {
            0.0
        };
        if self.negative != den.negative { -magnitude } else { magnitude }
    }

    fn add_signed(&self, other: &Dyadic, other_negative: bool) -> Dyadic {
        if self.is_zero() {
            let mut result = other.clone();
            if !result.is_zero() {
                result.negative = other_negative;
            }
            return result;
        }
        if other.is_zero() {
            return self.clone();
        }
        let exponent = self.exponent.min(other.exponent);
        let mut a_stack = [0; SCRATCH_LIMBS];
        let mut b_stack = [0; SCRATCH_LIMBS];
        let a_buffer = self.shifted(exponent, &mut a_stack);
        let b_buffer = other.shifted(exponent, &mut b_stack);
        let a = a_buffer.as_slice();
        let b = b_buffer.as_slice();
        let mut stack = [0; SCRATCH_LIMBS];
        if self.negative == other_negative {
            let len = a.len().max(b.len());
            let mut buffer = LimbBuffer::new(len + 1, &mut stack);
            let limbs = buffer.as_mut_slice();
            let mut carry = 0_u128;
            for index in 0..len {
                let sum = u128::from(a.get(index).copied().unwrap_or(0))
                    + u128::from(b.get(index).copied().unwrap_or(0))
                    + carry;
                limbs[index] = sum as u64;
                carry = sum >> 64;
            }
            limbs[len] = carry as u64;
            return Self::normalized(self.negative, buffer, exponent);
        }

        let order = a.len().cmp(&b.len()).then_with(|| a.iter().rev().cmp(b.iter().rev()));
        let (large, small, negative) = match order {
            Ordering::Greater => (a, b, self.negative),
            Ordering::Less => (b, a, other_negative),
            Ordering::Equal => return Self::zero(),
        };
        let mut buffer = LimbBuffer::new(large.len(), &mut stack);
        let limbs = buffer.as_mut_slice();
        let mut borrow = false;
        for (index, &limb) in large.iter().enumerate() {
            let (difference, first) = limb.overflowing_sub(small.get(index).copied().unwrap_or(0));
            let (difference, second) = difference.overflowing_sub(u64::from(borrow));
            limbs[index] = difference;
            borrow = first || second;
        }
        Self::normalized(negative, buffer, exponent)
    }

    fn normalized(negative: bool, mut buffer: LimbBuffer<'_>, mut exponent: i64) -> Dyadic {
        let limbs = buffer.as_mut_slice();
        let mut len = limbs.len();
        while len != 0 && limbs[len - 1] == 0 {
            len -= 1;
        }
        if len == 0 {
            return Self::zero();
        }
        // Removing every low zero bit gives each value a unique representation.
        let words = limbs[..len].iter().position(|&limb| limb != 0).unwrap();
        if words != 0 {
            limbs.copy_within(words..len, 0);
        }
        len -= words;
        exponent += words as i64 * 64;
        let bits = limbs[0].trailing_zeros();
        if bits != 0 {
            for index in 0..len {
                limbs[index] = (limbs[index] >> bits)
                    | if index + 1 < len { limbs[index + 1] << (64 - bits) } else { 0 };
            }
            if limbs[len - 1] == 0 {
                len -= 1;
            }
            exponent += i64::from(bits);
        }
        Dyadic { negative, limbs: buffer.into_limbs(len), exponent }
    }

    fn shifted<'a>(&self, exponent: i64, stack: &'a mut [u64; SCRATCH_LIMBS]) -> LimbBuffer<'a> {
        let shift = self.exponent.abs_diff(exponent);
        let words = usize::try_from(shift / 64).expect("dyadic magnitude exceeds address space");
        let bits = (shift % 64) as u32;
        let source = self.limbs.as_slice();
        let extra = bits != 0 && source[source.len() - 1] >> (64 - bits) != 0;
        let len = words + source.len() + usize::from(extra);
        let mut buffer = LimbBuffer::new(len, stack);
        let limbs = buffer.as_mut_slice();
        let mut carry = 0;
        for (index, &limb) in source.iter().enumerate() {
            limbs[words + index] = (limb << bits) | carry;
            carry = if bits != 0 { limb >> (64 - bits) } else { 0 };
        }
        if extra {
            limbs[len - 1] = carry;
        }
        buffer
    }

    fn leading(&self) -> (u64, i128) {
        let limbs = self.limbs.as_slice();
        let index = limbs.len() - 1;
        let shift = limbs[index].leading_zeros();
        let mut top = limbs[index] << shift;
        if shift != 0 && index != 0 {
            top |= limbs[index - 1] >> (64 - shift);
        }
        let exponent = i128::from(self.exponent) + index as i128 * 64 + i128::from(63 - shift);
        (top, exponent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_and_conversion() {
        for value in [
            0.0, -0.0, 5e-324, -5e-324, f64::from_bits(2),
            f64::from_bits((1_u64 << 52) - 1), f64::MIN_POSITIVE,
            -f64::MIN_POSITIVE, 0.1, -0.1, 1.0, -3.5, 1e300, f64::MAX, -f64::MAX,
        ] {
            assert_eq!(Dyadic::from_f64(value).estimate(), value, "exact conversion for {value}");
        }
        let zero = Dyadic::zero();
        assert_eq!(Dyadic::from_f64(0.0), zero, "positive zero is canonical");
        assert_eq!(Dyadic::from_f64(-0.0), zero, "negative zero is canonical");
        assert_eq!(zero.negate(), zero, "negated zero is canonical");
        let one = Dyadic::from_f64(1.0);
        assert_eq!(one.add(&one), Dyadic::from_f64(2.0), "addition removes low zero bits");
        assert_eq!(one.sub(&one), zero, "cancellation is canonical zero");
        let x = Dyadic::from_f64(1e300);
        let y = Dyadic::from_f64(5e-324);
        assert_eq!(x.add(&y).sub(&x).sub(&y), zero, "widely separated bits survive addition");
        let y2 = y.mul(&y);
        let y4 = y2.mul(&y).mul(&y);
        assert_eq!(y4.sign(), 1, "products below the f64 range remain positive");
        assert_eq!(y4.sub(&y2.mul(&y2)), zero, "tiny products agree exactly");
        let x2 = x.mul(&x);
        let numerator = x2.mul(&x).mul(&y);
        let denominator = x2.mul(&y);
        let tolerance = 2.0_f64.powi(-50);
        assert!((numerator.ratio(&denominator) / 1e300 - 1.0).abs() < tolerance,
            "ratio remains accurate for products outside the f64 range");
        assert!((numerator.mul(&x2).ratio(&denominator.mul(&x2)) / 1e300 - 1.0).abs() < tolerance,
            "ratio remains accurate when both operands exceed the f64 range");
        assert!((numerator.mul(&y4).ratio(&denominator.mul(&y4)) / 1e300 - 1.0).abs() < tolerance,
            "ratio remains accurate when both operands are below the f64 range");
        assert!((numerator.negate().ratio(&denominator) / -1e300 - 1.0).abs() < tolerance,
            "ratio preserves the numerator sign");
        assert!((numerator.ratio(&denominator.negate()) / -1e300 - 1.0).abs() < tolerance,
            "ratio preserves the denominator sign");
        assert_eq!(x2.estimate(), f64::INFINITY, "large estimates overflow positively");
        assert_eq!(x2.negate().estimate(), f64::NEG_INFINITY, "large estimates overflow negatively");
        assert_eq!(y4.estimate(), 0.0, "estimates below the subnormal range are zero");
        assert_eq!(y.ratio(&one), 5e-324, "ratio retains a representable subnormal");
        let maximum = Dyadic::from_f64(f64::MAX);
        let three = Dyadic::from_f64(3.0);
        let boundary = maximum.mul(&three).ratio(&three);
        assert!(boundary.is_finite() && (boundary / f64::MAX - 1.0).abs() < tolerance,
            "a normal quotient at the upper boundary stays finite and accurate");

        let factor = Dyadic::from_f64((1_u64 << 53) as f64 - 1.0);
        let seven = (0..7).fold(one.clone(), |product, _| product.mul(&factor));
        assert!(matches!(&seven.limbs, Limbs::Inline { len: 6, .. }),
            "a six-limb product stays inline");
        let eight = seven.mul(&factor);
        assert!(matches!(&eight.limbs, Limbs::Heap(_)),
            "a product of eight full mantissas spills to the heap");
        let factor2 = factor.mul(&factor);
        let factor4 = factor2.mul(&factor2);
        assert_eq!(eight, factor4.mul(&factor4), "heap products agree across multiplication orders");
        let twenty_four = (0..24).fold(one.clone(), |product, _| product.mul(&factor));
        assert_eq!(twenty_four, eight.mul(&eight).mul(&eight),
            "products beyond the stack scratch capacity agree exactly");
        let recovered = twenty_four.sub(&twenty_four.sub(&one));
        assert_eq!(recovered, one, "cancellation of heap values recovers an inline value");
        assert!(matches!(&recovered.limbs, Limbs::Inline { len: 1, .. }),
            "a magnitude reduced to one limb returns to inline storage");

        let distant = Dyadic::from_f64(2.0_f64.powi(400));
        for high in [distant.clone(), distant.mul(&distant).mul(&distant)] {
            let spread = high.add(&one);
            assert_eq!(spread.sub(&high), one, "wide exponent alignment preserves the low term");
            assert_eq!(spread.sub(&one), high, "wide exponent alignment preserves the high term");
            let difference = high.sub(&one);
            assert_eq!(difference.add(&one), high, "a carry across many limbs normalizes exactly");
            assert_eq!(one.sub(&high), difference.negate(), "wide subtraction preserves the sign");
        }
    }

    #[test]
    fn determinants_agree() {
        let mut state = 7_u64;
        for case in 0..200 {
            let mut entries = [[0.0; 3]; 3];
            for row in &mut entries {
                for entry in row {
                    state = state.wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    let exponent = ((state >> 53) % 60) as i32 - 30;
                    let fraction = 1.0 + (state & ((1_u64 << 52) - 1)) as f64 / (1_u64 << 52) as f64;
                    let sign = if state >> 63 == 0 { 1.0 } else { -1.0 };
                    *entry = sign * fraction * 2.0_f64.powi(exponent);
                }
            }
            if case % 17 == 0 {
                entries[2] = entries[0];
            }
            let [a, b, c] = entries.map(|row| row.map(Dyadic::from_f64));
            let exact = a[0].mul(&b[1].mul(&c[2]).sub(&b[2].mul(&c[1])))
                .sub(&a[1].mul(&b[0].mul(&c[2]).sub(&b[2].mul(&c[0]))))
                .add(&a[2].mul(&b[0].mul(&c[1]).sub(&b[1].mul(&c[0]))));
            let [a, b, c] = entries.map(|row| row.map(Expansion::from_f64));
            let expansion = a[0].mul(&b[1].mul(&c[2]).sub(&b[2].mul(&c[1])))
                .sub(&a[1].mul(&b[0].mul(&c[2]).sub(&b[2].mul(&c[0]))))
                .add(&a[2].mul(&b[0].mul(&c[1]).sub(&b[1].mul(&c[0]))));
            assert_eq!(exact, Dyadic::from_expansion(&expansion), "determinant value for case {case}");
            assert_eq!(exact.sign(), expansion.sign(), "determinant sign for case {case}");
            if case % 17 == 0 {
                assert!(exact.is_zero(), "repeated rows are exactly singular for case {case}");
            }
        }
    }
}
