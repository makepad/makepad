//~ E0658
//~ help: `try` expression is experimental
// A `try` block without `#![feature(try_blocks)]` (relax issue 34).
fn main() {
    let x: Option<u8> = try { Some(1u8)? + 1 };
    assert_eq!(x, Some(2));
}
