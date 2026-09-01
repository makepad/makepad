//! Minimal unsigned little-limb bignum used only for MIX RSA key derivation.

use std::cmp::Ordering;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BigNum {
    limbs: Vec<u32>,
}

impl BigNum {
    pub(crate) fn from_bytes_le(bytes: &[u8]) -> Self {
        let mut limbs = Vec::with_capacity(bytes.len().div_ceil(4));
        for chunk in bytes.chunks(4) {
            let mut word = [0; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            limbs.push(u32::from_le_bytes(word));
        }
        Self::new(limbs)
    }

    pub(crate) fn from_bytes_be(bytes: &[u8]) -> Self {
        let reversed = bytes.iter().rev().copied().collect::<Vec<_>>();
        Self::from_bytes_le(&reversed)
    }

    pub(crate) fn to_bytes_le(&self, length: usize) -> Vec<u8> {
        let mut output = vec![0; length];
        for (index, byte) in output.iter_mut().enumerate() {
            *byte = ((self.limb(index / 4) >> ((index % 4) * 8)) & 0xff) as u8;
        }
        output
    }

    pub(crate) fn mul(&self, other: &Self) -> Self {
        if self.is_zero() || other.is_zero() {
            return Self::new(Vec::new());
        }
        let mut result = vec![0u32; self.limbs.len() + other.limbs.len()];
        for (left_index, &left) in self.limbs.iter().enumerate() {
            let mut carry = 0u64;
            for (right_index, &right) in other.limbs.iter().enumerate() {
                let at = left_index + right_index;
                let product = left as u64 * right as u64 + result[at] as u64 + carry;
                result[at] = product as u32;
                carry = product >> 32;
            }
            let mut at = left_index + other.limbs.len();
            while carry != 0 {
                if at == result.len() {
                    result.push(0);
                }
                let sum = result[at] as u64 + carry;
                result[at] = sum as u32;
                carry = sum >> 32;
                at += 1;
            }
        }
        Self::new(result)
    }

    pub(crate) fn remainder(&self, modulus: &Self) -> Option<Self> {
        if modulus.is_zero() {
            return None;
        }
        let mut remainder = Self::new(Vec::new());
        for bit in (0..self.bit_len()).rev() {
            remainder.shift_left_with_bit(self.bit(bit));
            if remainder.cmp(modulus) != Ordering::Less {
                remainder.sub_assign(modulus);
            }
        }
        Some(remainder)
    }

    pub(crate) fn powmod(&self, mut exponent: u32, modulus: &Self) -> Option<Self> {
        if modulus.is_zero() {
            return None;
        }
        let mut base = self.remainder(modulus)?;
        let mut result = Self::new(vec![1]).remainder(modulus)?;
        while exponent != 0 {
            if exponent & 1 != 0 {
                result = result.mul(&base).remainder(modulus)?;
            }
            exponent >>= 1;
            if exponent != 0 {
                base = base.mul(&base).remainder(modulus)?;
            }
        }
        Some(result)
    }

    fn new(mut limbs: Vec<u32>) -> Self {
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        Self { limbs }
    }

    fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    fn limb(&self, index: usize) -> u32 {
        self.limbs.get(index).copied().unwrap_or(0)
    }

    fn bit_len(&self) -> usize {
        self.limbs.last().map_or(0, |last| {
            (self.limbs.len() - 1) * 32 + (32 - last.leading_zeros() as usize)
        })
    }

    fn bit(&self, bit: usize) -> bool {
        self.limb(bit / 32) & (1 << (bit % 32)) != 0
    }

    fn shift_left_with_bit(&mut self, bit: bool) {
        let mut carry = u32::from(bit);
        for limb in &mut self.limbs {
            let next = *limb >> 31;
            *limb = (*limb << 1) | carry;
            carry = next;
        }
        if carry != 0 || (bit && self.limbs.is_empty()) {
            self.limbs.push(carry.max(u32::from(bit)));
        }
    }

    fn sub_assign(&mut self, other: &Self) {
        let mut borrow = 0u64;
        for index in 0..self.limbs.len() {
            let sub = other.limb(index) as u64 + borrow;
            let current = self.limbs[index] as u64;
            self.limbs[index] = current.wrapping_sub(sub) as u32;
            borrow = u64::from(current < sub);
        }
        while self.limbs.last() == Some(&0) {
            self.limbs.pop();
        }
    }
}

impl Ord for BigNum {
    fn cmp(&self, other: &Self) -> Ordering {
        self.limbs
            .len()
            .cmp(&other.limbs.len())
            .then_with(|| self.limbs.iter().rev().cmp(other.limbs.iter().rev()))
    }
}

impl PartialOrd for BigNum {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(value: u128) -> BigNum {
        BigNum::from_bytes_le(&value.to_le_bytes())
    }

    fn as_u128(value: &BigNum) -> u128 {
        let bytes = value.to_bytes_le(16);
        u128::from_le_bytes(bytes.try_into().unwrap())
    }

    #[test]
    fn cnc_import_bignum_mul_and_remainder_match_u128() {
        for (left, right, modulus) in [
            (0, 91, 17),
            (123_456, 654_321, 65_537),
            (u64::MAX as u128, 0x1234_5678, 0xffff_ffff_ffff_ffc5),
        ] {
            let product = number(left).mul(&number(right));
            assert_eq!(as_u128(&product), left * right);
            let remainder = product.remainder(&number(modulus)).unwrap();
            assert_eq!(as_u128(&remainder), (left * right) % modulus);
        }
    }

    #[test]
    fn cnc_import_bignum_powmod_matches_u128() {
        for (base, exponent, modulus) in [(2, 17, 257), (12345, 65537, 99991)] {
            let mut expected = 1u128;
            let mut power = base % modulus;
            let mut exponent_reference = exponent;
            while exponent_reference != 0 {
                if exponent_reference & 1 != 0 {
                    expected = expected * power % modulus;
                }
                exponent_reference >>= 1;
                power = power * power % modulus;
            }
            let actual = number(base)
                .powmod(exponent as u32, &number(modulus))
                .unwrap();
            assert_eq!(as_u128(&actual), expected);
        }
    }
}
