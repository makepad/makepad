//~ msg: suffixes on string literals are invalid
// A literal's suffix in live code stays an error (rustc report_lit_error) where a suffix in a
// cfg-stripped item before it is none (relax issue 125): the stripped `1foo` would say "invalid
// suffix `foo`".
#[cfg(any())]
const N: u32 = 1foo;
fn main() {
    let _s = "a"x;
}
