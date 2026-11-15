//! A `$t:ty` fragment takes a whole type, `+` bounds of a bare trait object included (rustc parses
//! it with parse_ty): `Display + Send` and `FnOnce(u8) -> u8 + 'static` are one type each.
macro_rules! t {
    ($t:ty) => {
        stringify!($t)
    };
}

#[test]
fn bare_trait_objects_with_bounds_are_types() {
    assert_eq!(t!(std::fmt::Display + Send), "std::fmt::Display + Send");
    assert_eq!(t!(FnOnce(u8) -> u8 + 'static), "FnOnce(u8) -> u8 + 'static");
    assert_eq!(t!(dyn std::fmt::Display + Send), "dyn std::fmt::Display + Send");
}
