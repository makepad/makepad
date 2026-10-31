// typeck-audit 1.17: a suffix that does not match the parameter type is the anon const's
// type mismatch (E0308) in rustc; checked as a package with `makepad check`.
#![allow(dead_code)]
struct U8<const N: u8>;
fn suffix() -> U8<3u16> { U8 }
fn negative_unsigned() -> U8<{ -1 }> { U8 }
