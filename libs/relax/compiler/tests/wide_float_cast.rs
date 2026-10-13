// Integer-to-float casts round the complete value once, including negative
// values whose two's-complement halves would cancel if converted separately.
#[inline(never)]
fn signed64(value: i128) -> f64 { value as f64 }
#[inline(never)]
fn unsigned64(value: u128) -> f64 { value as f64 }
#[inline(never)]
fn signed32(value: i128) -> f32 { value as f32 }
#[inline(never)]
fn unsigned32(value: u128) -> f32 { value as f32 }

#[test]
fn small_signed_values() {
    for value in [-97500i64, -65000, -2500, -25, -1, 0, 1, 2500, 97500] {
        assert_eq!(signed64(value as i128).to_bits(), (value as f64).to_bits());
        assert_eq!(signed32(value as i128).to_bits(), (value as f32).to_bits());
    }
}

#[test]
fn unsigned_double_precision_rounding() {
    let base = 1u128 << 127;
    let half = 1u128 << 74;
    let bits = 1150u64 << 52;
    assert_eq!(unsigned64(base + half - 1).to_bits(), bits);
    assert_eq!(unsigned64(base + half).to_bits(), bits);
    assert_eq!(unsigned64(base + half + 1).to_bits(), bits + 1);
    assert_eq!(unsigned64(u128::MAX).to_bits(), 1151u64 << 52);
}

#[test]
fn signed_double_precision_rounding() {
    let base = 1i128 << 126;
    let half = 1i128 << 73;
    let bits = 1149u64 << 52;
    assert_eq!(signed64(base + half + 1).to_bits(), bits + 1);
    assert_eq!(signed64(-base - half).to_bits(), (1u64 << 63) | bits);
    assert_eq!(signed64(-base - half - 1).to_bits(), (1u64 << 63) | (bits + 1));
    assert_eq!(signed64(i128::MIN).to_bits(), (1u64 << 63) | (1150u64 << 52));
    assert_eq!(signed64(i128::MAX).to_bits(), 1150u64 << 52);
}

#[test]
fn unsigned_single_precision_rounding() {
    let base = 1u128 << 64;
    let half = 1u128 << 40;
    let bits = 191u32 << 23;
    assert_eq!(unsigned32(base + half - 1).to_bits(), bits);
    assert_eq!(unsigned32(base + half).to_bits(), bits);
    assert_eq!(unsigned32(base + half + 1).to_bits(), bits + 1);
    assert_eq!(unsigned32(u128::MAX), f32::INFINITY);
}

#[test]
fn signed_single_precision_rounding() {
    let base = 1i128 << 64;
    let half = 1i128 << 40;
    let bits = 191u32 << 23;
    assert_eq!(signed32(base + half + 1).to_bits(), bits + 1);
    assert_eq!(signed32(-base - half).to_bits(), (1u32 << 31) | bits);
    assert_eq!(signed32(-base - half - 1).to_bits(), (1u32 << 31) | (bits + 1));
    assert_eq!(signed32(i128::MIN).to_bits(), (1u32 << 31) | (254u32 << 23));
    assert_eq!(signed32(i128::MAX).to_bits(), 254u32 << 23);
}
