//~ E0271
//~ help: type mismatch resolving
// A `try` block's tail is `Try::from_output`'s argument, of the projection type
// `<Option<&str> as Try>::Output`: it is not coerced, so a `&String` tail is no `&str` (relax
// issue 34).
#![feature(try_blocks)]
fn main() {
    let s = String::from("ab");
    let r: Option<&str> = try { &s };
    let _ = r;
}
