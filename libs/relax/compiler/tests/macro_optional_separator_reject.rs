//! `?` takes no separator (rustc mbe/quoted.rs; relax issue 21).
// rejects: the `?` macro repetition operator does not take a separator
macro_rules! sep_opt {
    ($(a),?) => { 0 };
}

#[test]
fn defined() {
    assert_eq!(sep_opt!(a), 0);
}
