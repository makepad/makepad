// Small numerics shared by the instrument: a deterministic noise source,
// Bessel functions of the first kind (membrane mode shapes and their zeros),
// and a one-pole smoother. Nothing here allocates; the Bessel code runs at
// construction (zeros) and at trigger time (strike-point weights, ~60
// evaluations of ~150 recurrence steps each: a few microseconds).

/// xorshift32. Every voice owns one, seeded from (voice kind, trigger
/// serial), so a given trigger stream renders bit-identically while
/// consecutive hits still differ (round-robin by physics, not by samples).
#[derive(Clone, Copy)]
pub struct Rng(pub u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x9e37_79b9 } else { seed })
    }
    #[inline(always)]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    /// Uniform in [-1, 1).
    #[inline(always)]
    pub fn bipolar(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (2.0 / 16_777_216.0) - 1.0
    }
    /// Uniform in [0, 1).
    #[inline(always)]
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }
    pub fn unit_f64(&mut self) -> f64 {
        (self.next_u32() >> 8) as f64 * (1.0 / 16_777_216.0)
    }
}

/// J_m(x) for integer order by Miller's backward recurrence, normalised with
/// J_0 + 2 (J_2 + J_4 + ...) = 1. Accurate to double precision for every
/// order and argument this crate uses (x <= ~80), unlike the power series,
/// which cancels catastrophically past x ~ 25.
pub fn bessel_j(m: usize, x: f64) -> f64 {
    let ax = x.abs();
    if ax < 1e-12 {
        return if m == 0 { 1.0 } else { 0.0 };
    }
    let mut n_start = (ax as usize).max(m) + 34 + 4 * (ax.sqrt() as usize);
    n_start += n_start & 1; // even start so the normalisation sum is well defined
    let mut jp1 = 0.0f64; // J_{k+1}
    let mut j = 1e-30f64; // J_k
    let mut sum = 0.0f64;
    let mut result = 0.0f64;
    let mut k = n_start;
    while k >= 1 {
        let jm1 = (2.0 * k as f64 / ax) * j - jp1;
        jp1 = j;
        j = jm1;
        let order = k - 1;
        if order == m {
            result = j;
        }
        if order % 2 == 0 {
            sum += if order == 0 { j } else { 2.0 * j };
        }
        if j.abs() > 1e150 {
            j *= 1e-150;
            jp1 *= 1e-150;
            sum *= 1e-150;
            result *= 1e-150;
        }
        k -= 1;
    }
    let v = result / sum;
    // J_m(-x) = (-1)^m J_m(x)
    if x < 0.0 && m % 2 == 1 { -v } else { v }
}

/// n-th positive zero of J_m (n >= 1): bracket by scanning from the first
/// possible location (j_{m,1} > m), then bisect.
pub fn bessel_zero(m: usize, n: usize) -> f64 {
    let mut x = m as f64 + 1.0e-3;
    let mut prev = bessel_j(m, x);
    let step = 0.2;
    let mut found = 0;
    loop {
        let xn = x + step;
        let cur = bessel_j(m, xn);
        if prev == 0.0 || (prev > 0.0) != (cur > 0.0) {
            found += 1;
            if found == n {
                // bisect in [x, xn]
                let (mut lo, mut hi) = (x, xn);
                let mut flo = prev;
                for _ in 0..60 {
                    let mid = 0.5 * (lo + hi);
                    let fm = bessel_j(m, mid);
                    if (fm > 0.0) == (flo > 0.0) {
                        lo = mid;
                        flo = fm;
                    } else {
                        hi = mid;
                    }
                }
                return 0.5 * (lo + hi);
            }
        }
        prev = cur;
        x = xn;
        if x > 400.0 {
            return x; // unreachable for the orders used; keeps the loop finite
        }
    }
}

/// Coefficient for a one-pole with time constant `tau` seconds.
pub fn one_pole_coeff(tau: f32, fs: f32) -> f32 {
    if tau <= 0.0 {
        1.0
    } else {
        1.0 - (-1.0 / (tau * fs)).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bessel_values() {
        // Abramowitz & Stegun table values.
        assert!((bessel_j(0, 1.0) - 0.7651976866).abs() < 1e-9);
        assert!((bessel_j(1, 1.0) - 0.4400505857).abs() < 1e-9);
        assert!((bessel_j(0, 5.0) + 0.1775967713).abs() < 1e-9);
        assert!((bessel_j(2, 10.0) - 0.2546303137).abs() < 1e-9);
        assert!((bessel_j(0, 30.0) + 0.0863679836).abs() < 1e-9);
        assert!((bessel_j(1, 30.0) + 0.1187510626).abs() < 1e-9);
    }

    #[test]
    fn bessel_zeros() {
        assert!((bessel_zero(0, 1) - 2.404825557).abs() < 1e-8);
        assert!((bessel_zero(0, 2) - 5.520078110).abs() < 1e-8);
        assert!((bessel_zero(1, 1) - 3.831705970).abs() < 1e-8);
        assert!((bessel_zero(2, 1) - 5.135622302).abs() < 1e-8);
        assert!((bessel_zero(3, 2) - 9.761023130).abs() < 1e-8);
        assert!((bessel_zero(10, 1) - 14.475500686).abs() < 1e-7);
    }
}
