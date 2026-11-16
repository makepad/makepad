// An #[allow(long_running_const_eval)] constant past the step limit (3M loop iterations, short of
// the progress warnings past 4M steps) is evaluated: no diagnostic, and its value.
#[allow(long_running_const_eval)]
pub const ODD: u64 = {
    let mut i = 0u64;
    let mut n = 0u64;
    while i < 3_000_000 {
        n += i & 1;
        i += 1;
    }
    n
};

#[test]
fn the_value() {
    assert_eq!(ODD, 1_500_000);
}
