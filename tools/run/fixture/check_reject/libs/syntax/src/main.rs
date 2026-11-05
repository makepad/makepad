// rustc refuses this macro where it is defined (missing fragment specifier), whatever calls it.
macro_rules! m {
    ($x) => {};
}

fn main() {}

#[test]
fn runs_only_if_the_macro_is_taken() {}
