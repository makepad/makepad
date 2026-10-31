// rustc_borrowck/src/lib.rs:127: borrowck consumes identity MIR with analysis
// predicates. MAKEPAD_RUN_BORROWCK_ALL must not erase this generic environment.
trait Source { type Item: Copy; }
impl Source for u32 { type Item = u32; }
fn projected<T: Source>(value: T::Item) -> (T::Item, T::Item) {
    (value, value)
}
fn const_parameter<const N: usize>(value: [u8; N]) -> ([u8; N], [u8; N]) {
    (value, value)
}
#[test]
fn projection_copy() {
    assert_eq!(projected::<u32>(7), (7, 7));
}
#[test]
fn const_copy() {
    assert_eq!(const_parameter([1, 2]), ([1, 2], [1, 2]));
}
