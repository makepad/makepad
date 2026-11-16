// rejects: constant evaluation is taking a long time
// 2^26 calls of a const fn: far past the step limit, in a check and when the test runs.
const fn calls(n: u32) -> u64 {
    if n == 0 { 1 } else { calls(n - 1) + calls(n - 1) }
}

pub const CALLS: u64 = calls(25);

#[test]
fn reads_it() {
    assert_eq!(CALLS, 1 << 25);
}
