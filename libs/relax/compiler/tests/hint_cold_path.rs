// `core::hint::cold_path` (stable since 1.95), as a crate imports it once its build script sees a 1.95+ rustc,
// and `std::hint::cold_path` re-exported, as a crate's branch-hint module does.
use core::hint::cold_path;

mod branch_hints {
    pub use std::hint::cold_path;
}

const fn checked_half(n: u32) -> Option<u32> {
    if n % 2 != 0 {
        cold_path();
        return None;
    }
    Some(n / 2)
}

const HALF: Option<u32> = checked_half(8);

#[test]
fn cold_path_does_not_change_the_result() {
    assert_eq!(HALF, Some(4));
    assert_eq!(checked_half(7), None);
    assert_eq!(checked_half(10), Some(5));
    let mut cold = 0;
    for n in [3u32, 4, 5] {
        if n % 2 != 0 {
            branch_hints::cold_path();
            std::hint::cold_path();
            cold += 1;
        }
    }
    assert_eq!(cold, 2);
}
