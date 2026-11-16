// rejects: constant evaluation is taking a long time
// A constant whose evaluation never ends: rustc's deny-by-default long_running_const_eval at its
// step limit, in a check (the interpreter's count) and when the test runs (the count of the
// constant's code).
pub const FOREVER: u32 = {
    let mut i = 0u32;
    loop {
        i = i.wrapping_add(1);
    }
};

#[test]
fn reads_it() {
    assert_eq!(FOREVER, 0);
}
