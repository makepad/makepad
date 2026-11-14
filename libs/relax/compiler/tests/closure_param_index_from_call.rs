//! An unannotated closure parameter used as an index takes its type from the call: rustc indexes an
//! array or slice builtin only by a usize or an integer variable, and an open index type selects
//! `Index<?I>`, whose output the call's argument settles (`2..5usize`: a range, `2`: a usize).
#[test]
fn a_range_parameter_indexes_an_array() {
    let stamp = *b"xx123yy";
    let field = |range| std::str::from_utf8(&stamp[range]).unwrap();
    assert_eq!(field(2..5usize), "123");
}
#[test]
fn a_range_parameter_indexes_a_slice() {
    let stamp: &[u8] = b"xx123yy";
    let field = |range| &stamp[range];
    assert_eq!(field(..2), b"xx");
}
#[test]
fn an_integer_parameter_indexes_an_array() {
    let stamp = *b"xx123yy";
    let at = |i| stamp[i];
    assert_eq!(at(2), b'1');
}
