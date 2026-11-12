// relax issue 64: std::arch::x86_64's ADX carry chain (`_addcarry_u32`/`_u64`,
// `_addcarryx_u32`/`_u64`, `_subborrow_u32`/`_u64`) was missing, so big-integer code that chains
// adc/sbb through them did not resolve. (x86_64 only: the file is empty elsewhere.)
#[cfg(target_arch = "x86_64")]
#[test]
fn carry_and_borrow_chains() {
    use std::arch::x86_64::*;
    let mut out = 0u64;
    assert_eq!((_addcarry_u64(0, u64::MAX, 1, &mut out), out), (1, 0));
    assert_eq!((_addcarry_u64(1, u64::MAX, 0, &mut out), out), (1, 0));
    assert_eq!((_addcarry_u64(1, 6, 7, &mut out), out), (0, 14));
    assert_eq!((_addcarry_u64(2, 6, 7, &mut out), out), (0, 14), "any nonzero carry-in is one");
    assert_eq!((_subborrow_u64(0, 0, 1, &mut out), out), (1, u64::MAX));
    assert_eq!((_subborrow_u64(1, 9, 4, &mut out), out), (0, 4));
    assert_eq!((_subborrow_u64(1, 4, 4, &mut out), out), (1, u64::MAX));
    let mut o32 = 0u32;
    assert_eq!((_addcarry_u32(1, u32::MAX, u32::MAX, &mut o32), o32), (1, u32::MAX));
    assert_eq!((_subborrow_u32(0, 3, 5, &mut o32), o32), (1, u32::MAX - 1));
    // two limbs added and subtracted again, as a bignum's loop chains them
    let (a, b) = ([u64::MAX, 1], [1u64, 2]);
    let mut sum = [0u64; 2];
    let c = _addcarry_u64(0, a[0], b[0], &mut sum[0]);
    let c = _addcarry_u64(c, a[1], b[1], &mut sum[1]);
    assert_eq!((sum, c), ([0, 4], 0));
    let mut diff = [0u64; 2];
    let w = _subborrow_u64(0, sum[0], b[0], &mut diff[0]);
    let w = _subborrow_u64(w, sum[1], b[1], &mut diff[1]);
    assert_eq!((diff, w), (a, 0));
    if is_x86_feature_detected!("adx") {
        assert_eq!(unsafe { adx_forms(u64::MAX, 1) }, (1, 0, 1, 0));
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "adx")]
unsafe fn adx_forms(a: u64, b: u64) -> (u8, u64, u8, u32) {
    let (mut o64, mut o32) = (0u64, 0u32);
    let c64 = std::arch::x86_64::_addcarryx_u64(0, a, b, &mut o64);
    let c32 = std::arch::x86_64::_addcarryx_u32(0, a as u32, b as u32, &mut o32);
    (c64, o64, c32, o32)
}
