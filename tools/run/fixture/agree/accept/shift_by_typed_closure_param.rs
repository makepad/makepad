// A shift by a closure parameter whose type is known: built in (rustc op.rs is_builtin_binop), and
// `& 1` takes the left operand's type. A later call that names the parameter's type resolves the
// operator trait call's output before fallback.
pub fn by_call(a: u8) -> bool {
    let g = |n| (a >> n) & 1 != 0;
    g(3u32)
}
pub fn by_signature(a: u8) -> impl Fn(u32) -> bool {
    move |n| (a >> n) & 1 != 0
}
pub fn by_iterator(a: u8) -> usize {
    (0..8).filter(|&n| (a >> n) & 1 != 0).count()
}
#[test] fn runs() { assert!(by_call(0b1000)); assert!(!by_signature(0b1000)(2)); assert_eq!(by_iterator(0b1011), 3); }
