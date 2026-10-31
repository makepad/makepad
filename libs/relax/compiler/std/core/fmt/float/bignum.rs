//! Fixed-size unsigned big integer (40 x 32-bit limbs = 1280 bits), enough for exact
//! decimal conversion of any f64 (as real core's Big32x40).

use crate::cmp::Ordering;

pub(crate) const LIMBS: usize = 40;

#[derive(Clone, Copy)]
pub(crate) struct Big {
    /// limbs in use; base[size..] are zero
    size: usize,
    /// little-endian limbs
    base: [u32; LIMBS],
}

impl Big {
    pub(crate) fn from_small(v: u32) -> Big {
        let mut base = [0u32; LIMBS];
        base[0] = v;
        Big { size: 1, base }
    }

    pub(crate) fn from_u64(v: u64) -> Big {
        let mut base = [0u32; LIMBS];
        base[0] = v as u32;
        base[1] = (v >> 32) as u32;
        Big { size: if base[1] != 0 { 2 } else { 1 }, base }
    }

    pub(crate) fn is_zero(&self) -> bool {
        let mut i = 0;
        while i < self.size {
            if self.base[i] != 0 {
                return false;
            }
            i += 1;
        }
        true
    }

    pub(crate) fn add(&mut self, other: &Big) -> &mut Big {
        let sz = if self.size > other.size { self.size } else { other.size };
        let mut carry = 0u64;
        let mut i = 0;
        while i < sz {
            let v = self.base[i] as u64 + other.base[i] as u64 + carry;
            self.base[i] = v as u32;
            carry = v >> 32;
            i += 1;
        }
        let mut sz = sz;
        if carry > 0 {
            self.base[sz] = carry as u32;
            sz += 1;
        }
        self.size = sz;
        self
    }

    /// self -= other; requires self >= other.
    pub(crate) fn sub(&mut self, other: &Big) -> &mut Big {
        let sz = if self.size > other.size { self.size } else { other.size };
        let mut borrow = 0u64;
        let mut i = 0;
        while i < sz {
            let a = self.base[i] as u64;
            let b = other.base[i] as u64 + borrow;
            if a >= b {
                self.base[i] = (a - b) as u32;
                borrow = 0;
            } else {
                self.base[i] = ((a + (1u64 << 32)) - b) as u32;
                borrow = 1;
            }
            i += 1;
        }
        self.size = sz;
        self
    }

    pub(crate) fn mul_small(&mut self, other: u32) -> &mut Big {
        let mut carry = 0u64;
        let mut i = 0;
        while i < self.size {
            let v = self.base[i] as u64 * other as u64 + carry;
            self.base[i] = v as u32;
            carry = v >> 32;
            i += 1;
        }
        if carry > 0 {
            self.base[self.size] = carry as u32;
            self.size += 1;
        }
        self
    }

    pub(crate) fn mul_pow2(&mut self, bits: usize) -> &mut Big {
        let digits = bits / 32;
        let bits = bits % 32;
        // shift by whole limbs
        let mut i = self.size;
        while i > 0 {
            i -= 1;
            self.base[i + digits] = self.base[i];
        }
        let mut j = 0;
        while j < digits {
            self.base[j] = 0;
            j += 1;
        }
        let mut sz = self.size + digits;
        if bits > 0 {
            let last = sz;
            let overflow = self.base[last - 1] >> (32 - bits);
            if overflow > 0 {
                self.base[last] = overflow;
                sz += 1;
            }
            let mut k = last - 1;
            while k > digits {
                self.base[k] = (self.base[k] << bits) | (self.base[k - 1] >> (32 - bits));
                k -= 1;
            }
            self.base[digits] <<= bits;
        }
        self.size = sz;
        self
    }

    pub(crate) fn mul_pow10(&mut self, n: usize) -> &mut Big {
        let mut n = n;
        while n >= 9 {
            self.mul_small(1_000_000_000);
            n -= 9;
        }
        let mut p = 1u32;
        while n > 0 {
            p *= 10;
            n -= 1;
        }
        if p != 1 {
            self.mul_small(p);
        }
        self
    }

    /// self /= other, returns the remainder.
    pub(crate) fn div_rem_small(&mut self, other: u32) -> u32 {
        let mut rem = 0u64;
        let mut i = self.size;
        while i > 0 {
            i -= 1;
            let v = (rem << 32) | self.base[i] as u64;
            self.base[i] = (v / other as u64) as u32;
            rem = v % other as u64;
        }
        rem as u32
    }

    pub(crate) fn cmp(&self, other: &Big) -> Ordering {
        let sz = if self.size > other.size { self.size } else { other.size };
        let mut i = sz;
        while i > 0 {
            i -= 1;
            if self.base[i] != other.base[i] {
                return if self.base[i] < other.base[i] { Ordering::Less } else { Ordering::Greater };
            }
        }
        Ordering::Equal
    }
}
