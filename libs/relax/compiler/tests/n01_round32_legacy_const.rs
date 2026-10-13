use std::arch::x86_64::{_mm_slli_epi32 as shift, _mm_set1_epi32, _mm_cvtsi128_si32};

#[test]
fn legacy_paths_and_explicit_args() {
    unsafe {
        let a = _mm_set1_epi32(3);
        assert_eq!(_mm_cvtsi128_si32(shift(a, 1 + 1)), 12);
        assert_eq!(_mm_cvtsi128_si32(std::arch::x86_64::_mm_slli_epi32(a, 2)), 12);
        assert_eq!(_mm_cvtsi128_si32(shift::<2>(a)), 12);
        let nested = || shift(a, 2);
        assert_eq!(_mm_cvtsi128_si32(nested()), 12);
    }
}

#[test]
fn local_bindings_and_macro_hygiene() {
    let shift = |a: i32, b: i32| a + b;
    assert_eq!(shift(3, 2), 5);
    {
        use std::arch::x86_64::_mm_slli_epi32 as shift;
        unsafe { assert_eq!(_mm_cvtsi128_si32(shift(_mm_set1_epi32(3), 2)), 12); }
    }
    macro_rules! call_local { () => { shift(3, 2) }; }
    let shift = |a: i32, b: i32| a * b;
    assert_eq!(call_local!(), 5);
    assert_eq!(shift(3, 2), 6);
}
