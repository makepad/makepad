// Expansion arithmetic for exact floating-point computations.
//
// An "expansion" is an exact real number represented as a sum of
// non-overlapping IEEE 754 f64 values, stored in increasing order
// of magnitude. This allows exact representation of sums and products
// without rounding error.
//
// Based on Shewchuk's "Adaptive Precision Floating-Point Arithmetic" (1997).

/// An exact real number represented as a sum of non-overlapping f64 components.
/// Components are stored in increasing order of magnitude.
/// The most significant component (last element) is the best f64 approximation.
#[derive(Clone, Debug)]
pub struct Expansion {
    pub components: Vec<f64>,
}

impl Expansion {
    /// Create from a single f64 value.
    pub fn from_f64(v: f64) -> Expansion {
        if v == 0.0 {
            Expansion {
                components: vec![0.0],
            }
        } else {
            Expansion {
                components: vec![v],
            }
        }
    }

    /// Create zero.
    pub fn zero() -> Expansion {
        Expansion {
            components: vec![0.0],
        }
    }

    /// Get the best f64 approximation (most significant component).
    pub fn estimate(&self) -> f64 {
        if self.components.is_empty() {
            0.0
        } else {
            self.components[self.components.len() - 1]
        }
    }

    /// Get the exact sign: 1, -1, or 0.
    pub fn sign(&self) -> i32 {
        let e = self.estimate();
        if e > 0.0 {
            1
        } else if e < 0.0 {
            -1
        } else {
            0
        }
    }

    /// Is this exactly zero?
    pub fn is_zero(&self) -> bool {
        self.sign() == 0
    }

    /// Negate.
    pub fn negate(&self) -> Expansion {
        Expansion {
            components: self.components.iter().map(|&x| -x).collect(),
        }
    }

    /// Add a scalar.
    pub fn add_f64(&self, b: f64) -> Expansion {
        Expansion {
            components: grow_expansion(&self.components, b),
        }
    }

    /// Subtract a scalar.
    pub fn sub_f64(&self, b: f64) -> Expansion {
        self.add_f64(-b)
    }

    /// Add another expansion.
    pub fn add(&self, other: &Expansion) -> Expansion {
        Expansion {
            components: expansion_sum(&self.components, &other.components),
        }
    }

    /// Subtract another expansion.
    pub fn sub(&self, other: &Expansion) -> Expansion {
        self.add(&other.negate())
    }

    /// Multiply by a scalar.
    pub fn mul_f64(&self, b: f64) -> Expansion {
        Expansion {
            components: scale_expansion(&self.components, b),
        }
    }

    /// Multiply by another expansion.
    pub fn mul(&self, other: &Expansion) -> Expansion {
        if self.components.is_empty() || other.components.is_empty() {
            return Expansion::zero();
        }
        // Multiply each component of self by the entire other expansion,
        // then sum the results.
        let mut result = Expansion::zero();
        for &c in &self.components {
            let scaled = scale_expansion(&other.components, c);
            result = Expansion {
                components: expansion_sum(&result.components, &scaled),
            };
        }
        result
    }
}

// ============================================================================
// Low-level expansion arithmetic primitives
// ============================================================================

/// Exact sum: returns (s, e) where s + e = a + b exactly.
#[inline(always)]
pub fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let bv = x - a;
    let av = x - bv;
    let br = b - bv;
    let ar = a - av;
    (x, ar + br)
}

/// Exact difference: returns (s, e) where s + e = a - b exactly.
#[inline(always)]
pub fn two_diff(a: f64, b: f64) -> (f64, f64) {
    let x = a - b;
    let bv = a - x;
    let av = x + bv;
    let br = bv - b;
    let ar = a - av;
    (x, ar + br)
}

/// Split a f64 into high and low parts for exact multiplication.
#[inline(always)]
pub fn split(a: f64) -> (f64, f64) {
    let c = 134217729.0 * a; // (2^27 + 1) * a
    let abig = c - a;
    let ahi = c - abig;
    let alo = a - ahi;
    (ahi, alo)
}

/// Exact product: returns (p, e) where p + e = a * b exactly.
#[inline(always)]
pub fn two_product(a: f64, b: f64) -> (f64, f64) {
    let x = a * b;
    let (ahi, alo) = split(a);
    let (bhi, blo) = split(b);
    let err = ((ahi * bhi - x) + ahi * blo + alo * bhi) + alo * blo;
    (x, err)
}

/// Add a scalar to an expansion (sorted by magnitude).
pub fn grow_expansion(e: &[f64], b: f64) -> Vec<f64> {
    let mut result = Vec::with_capacity(e.len() + 1);
    let mut q = b;
    for &ei in e {
        let (nq, ne) = two_sum(q, ei);
        if ne != 0.0 {
            result.push(ne);
        }
        q = nq;
    }
    if q != 0.0 || result.is_empty() {
        result.push(q);
    }
    result
}

/// Multiply an expansion by a scalar.
pub fn scale_expansion(e: &[f64], b: f64) -> Vec<f64> {
    if e.is_empty() || b == 0.0 {
        return vec![0.0];
    }
    let mut result = Vec::with_capacity(e.len() * 2);
    let (mut q, hh) = two_product(e[0], b);
    if hh != 0.0 {
        result.push(hh);
    }
    for &ei in &e[1..] {
        let (ti1, ti0) = two_product(ei, b);
        let (nq, nqe) = two_sum(q, ti0);
        if nqe != 0.0 {
            result.push(nqe);
        }
        let (nq2, nqe2) = two_sum(ti1, nq);
        if nqe2 != 0.0 {
            result.push(nqe2);
        }
        q = nq2;
    }
    if q != 0.0 || result.is_empty() {
        result.push(q);
    }
    result
}

/// Sum two expansions.
pub fn expansion_sum(e: &[f64], f: &[f64]) -> Vec<f64> {
    let mut result = e.to_vec();
    for &fi in f {
        result = grow_expansion(&result, fi);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expansion_from_f64() {
        let e = Expansion::from_f64(42.0);
        assert_eq!(e.estimate(), 42.0);
        assert_eq!(e.sign(), 1);
    }

    #[test]
    fn test_expansion_zero() {
        let e = Expansion::zero();
        assert!(e.is_zero());
        assert_eq!(e.sign(), 0);
    }

    #[test]
    fn test_expansion_add() {
        let a = Expansion::from_f64(1.0);
        let b = Expansion::from_f64(2.0);
        let c = a.add(&b);
        assert_eq!(c.estimate(), 3.0);
    }

    #[test]
    fn test_expansion_sub() {
        let a = Expansion::from_f64(5.0);
        let b = Expansion::from_f64(3.0);
        let c = a.sub(&b);
        assert_eq!(c.estimate(), 2.0);
    }

    #[test]
    fn test_expansion_mul() {
        let a = Expansion::from_f64(7.0);
        let b = Expansion::from_f64(6.0);
        let c = a.mul(&b);
        assert_eq!(c.estimate(), 42.0);
    }

    #[test]
    fn test_expansion_negate() {
        let a = Expansion::from_f64(3.14);
        let b = a.negate();
        assert_eq!(b.estimate(), -3.14);
    }

    #[test]
    fn test_expansion_exact_sum_captures_roundoff() {
        // 1.0 + 1e-16 loses precision in f64, but expansion captures it
        let a = Expansion::from_f64(1.0);
        let b = Expansion::from_f64(1e-16);
        let c = a.add(&b);
        // The expansion should have two components capturing both values
        assert!(c.components.len() >= 2, "should have at least 2 components");
        // The estimate is the largest component
        assert_eq!(c.estimate(), 1.0);
    }

    #[test]
    fn test_expansion_add_sub_roundtrip() {
        // Adding and subtracting should preserve value
        let a = Expansion::from_f64(3.0);
        let b = Expansion::from_f64(7.0);
        let c = a.add(&b).sub(&b);
        assert_eq!(c.estimate(), 3.0);
    }

    #[test]
    fn test_expansion_determinant_clear_sign() {
        // det(|3 2; 1 4|) = 12 - 2 = 10
        let ea = Expansion::from_f64(3.0);
        let eb = Expansion::from_f64(2.0);
        let ec = Expansion::from_f64(1.0);
        let ed = Expansion::from_f64(4.0);

        let ad = ea.mul(&ed);
        let bc = eb.mul(&ec);
        let det = ad.sub(&bc);
        assert_eq!(det.estimate(), 10.0);
        assert_eq!(det.sign(), 1);
    }

    #[test]
    fn test_two_sum_basic() {
        let (s, e) = two_sum(1.0, 2.0);
        assert_eq!(s, 3.0);
        assert_eq!(e, 0.0);
    }

    #[test]
    fn test_two_product_basic() {
        let (p, e) = two_product(3.0, 7.0);
        assert_eq!(p, 21.0);
        assert_eq!(e, 0.0);
    }
}
