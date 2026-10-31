//! Program implementations of lowering helpers for the web target. No host ABI.

// Construct IEEE bits with a single round-to-nearest, ties-to-even step. Converting
// the two limbs separately (or going through f64 for f32) can double round. Do not
// use a u128-to-float cast here: lowering that cast calls these very helpers.
fn wide_float_bits(signed: u64, lo: u64, hi: u64, precision: u32, bias: u32) -> u64 {
    let negative = signed != 0 && hi >> 63 != 0;
    let mut magnitude = ((hi as u128) << 64) | lo as u128;
    if negative { magnitude = (!magnitude).wrapping_add(1); }
    if magnitude == 0 { return 0; }
    let bits = 128 - magnitude.leading_zeros();
    let mut exponent = bits - 1 + bias;
    let mut significand;
    if bits <= precision {
        significand = (magnitude as u64) << (precision - bits);
    } else {
        let shift = bits - precision;
        significand = (magnitude >> shift) as u64;
        let half = 1u128 << (shift - 1);
        let remainder = magnitude & ((half << 1) - 1);
        if remainder > half || (remainder == half && significand & 1 != 0) {
            significand += 1;
            if significand == 1u64 << precision {
                significand >>= 1;
                exponent += 1;
            }
        }
    }
    let fraction = significand & ((1u64 << (precision - 1)) - 1);
    let sign_shift = if precision == 24 { 31 } else { 63 };
    ((negative as u64) << sign_shift) | ((exponent as u64) << (precision - 1)) | fraction
}

pub fn wide_f32(signed: u64, lo: u64, hi: u64) -> f32 {
    f32::from_bits(wide_float_bits(signed, lo, hi, 24, 127) as u32)
}

pub fn wide_f64(signed: u64, lo: u64, hi: u64) -> f64 {
    f64::from_bits(wide_float_bits(signed, lo, hi, 53, 1023))
}

pub unsafe fn str_eq(a: *const u8, an: usize, b: *const u8, bn: usize) -> bool {
    if an != bn { return false; }
    let mut i = 0;
    while i < an {
        if *a.add(i) != *b.add(i) { return false; }
        i += 1;
    }
    true
}

/// Binary long division without recursively using the u128 division operator.
/// Lowering checks division by zero and signed MIN / -1 before calling this.
pub fn divrem128(mode: u64, alo: u64, ahi: u64, blo: u64, bhi: u64) -> u128 {
    let mut a = ((ahi as u128) << 64) | alo as u128;
    let mut b = ((bhi as u128) << 64) | blo as u128;
    let aneg = mode & 2 != 0 && ahi >> 63 != 0;
    let bneg = mode & 2 != 0 && bhi >> 63 != 0;
    if aneg { a = (!a).wrapping_add(1); }
    if bneg { b = (!b).wrapping_add(1); }
    let mut q = 0u128;
    let mut r = 0u128;
    let mut i = 128u32;
    while i != 0 {
        i -= 1;
        let carry = r >> 127 != 0;
        r = (r << 1) | ((a >> i) & 1);
        if carry || r >= b {
            r = r.wrapping_sub(b);
            q |= 1u128 << i;
        }
    }
    let rem = mode & 1 != 0;
    let result = if rem { r } else { q };
    if if rem { aneg } else { aneg != bneg } { (!result).wrapping_add(1) } else { result }
}

#[relax(keep_reachable)]
pub unsafe fn panic_site(msg: *const u8, len: usize, file: *const u8, file_len: usize, line: u32, col: u32) -> ! {
    let msg = crate::str::from_utf8_unchecked(crate::slice::from_raw_parts(msg, len));
    let loc = crate::panic::Location::internal_constructor(crate::slice::from_raw_parts(file, file_len), line, col);
    crate::panicking::panic_at(format_args!("{}", msg), &loc)
}

#[relax(keep_reachable)]
pub unsafe fn panic_bounds(index: usize, len: usize, file: *const u8, file_len: usize, line: u32, col: u32) -> ! {
    let loc = crate::panic::Location::internal_constructor(crate::slice::from_raw_parts(file, file_len), line, col);
    crate::panicking::panic_at(format_args!("index out of bounds: the len is {} but the index is {}", len, index), &loc)
}
