use core::cmp::Ordering;
use num_traits::float::FloatCore;

#[allow(dead_code)] // We don't use this currently, but might in the future.
pub fn z_cmp_ints(lhs: &[usize], rhs: &[usize]) -> Ordering {
    assert_eq!(
        lhs.len(),
        rhs.len(),
        "Cannot compare array with different lengths."
    );

    let mut msd = 0;

    for dim in 1..rhs.len() {
        if less_msb(lhs[msd] ^ rhs[msd], lhs[dim] ^ rhs[dim]) {
            msd = dim;
        }
    }

    lhs[msd].cmp(&rhs[msd])
}

fn less_msb(x: usize, y: usize) -> bool {
    x < y && x < (x ^ y)
}

// Fast construction of k-Nearest Neighbor Graphs for Point Clouds
// Michael Connor, Piyush Kumar
// Algorithm 1
//
// http://compgeom.com/~piyush/papers/tvcg_stann.pdf
pub fn z_cmp_floats(p1: &[Real], p2: &[Real]) -> Option<Ordering> {
    assert_eq!(
        p1.len(),
        p2.len(),
        "Cannot compare array with different lengths."
    );
    let mut x = 0;
    let mut dim = 0;

    for j in 0..p1.len() {
        let y = xor_msb_float(p1[j], p2[j]);
        if x < y {
            x = y;
            dim = j;
        }
    }

    p1[dim].partial_cmp(&p2[dim])
}

fn xor_msb_float(fa: Real, fb: Real) -> i16 {
    let (mantissa1, exponent1, _sign1) = fa.integer_decode();
    let (mantissa2, exponent2, _sign2) = fb.integer_decode();
    let x = exponent1; // To keep the same notation as the paper.
    let y = exponent2; // To keep the same notation as the paper.

    if x == y {
        let z = msdb(mantissa1, mantissa2);
        x - z
    } else if y < x {
        x
    } else {
        y
    }
}

fn msdb(x: u64, y: u64) -> i16 {
    64i16 - (x ^ y).leading_zeros() as i16
}
