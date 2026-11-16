// rejects: constant evaluation is taking a long time
// The check-mode interpreter gives no verdict on this constant (it stops at `as_bytes`, outside
// its subset), so the count of the constant's code decides, in a check as when the test runs.
pub const LEN: usize = {
    let b = "ab".as_bytes();
    let mut n = 0usize;
    loop {
        n = n.wrapping_add(b.len());
    }
};

#[test]
fn reads_it() {
    assert_eq!(LEN, 0);
}
