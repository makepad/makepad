use std::arch::x86_64::{_mm_slli_epi32 as shift, _mm_set1_epi32};

#[test]
fn nested_definition_arguments() {
    unsafe {
        let a = _mm_set1_epi32(3);
        let _ = shift(a, (|| 2)());
        let _ = shift(a, const { 2 });
        let _ = shift(a, { const N: i32 = 2; N });
        let _ = shift(a, [2; 1][0]);
    }
}
