#[test]
fn legacy_const_argument() {
    unsafe {
        let a = std::arch::x86_64::_mm_set1_epi32(3);
        let b = std::arch::x86_64::_mm_slli_epi32(a, 1 + 1);
        assert_eq!(std::arch::x86_64::_mm_cvtsi128_si32(b), 12);
    }
}
