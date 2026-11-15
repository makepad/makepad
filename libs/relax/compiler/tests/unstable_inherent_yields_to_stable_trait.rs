//! A stable crate's method call picks a stable trait method over an unstable inherent one, as
//! rustc's probe takes stable candidates first.

/// shellexpand's extension trait gives `str` an `as_str() -> Option<&str>`; core's inherent
/// `str::as_str` is unstable (`str_as_str`), so a stable crate's method call picks the trait's
/// (rustc's probe takes stable candidates first).
#[test]
fn an_unstable_inherent_method_yields_to_a_stable_trait_method() {
    trait AsStr {
        fn as_str(&self) -> Option<&str>;
    }
    impl AsStr for str {
        fn as_str(&self) -> Option<&str> {
            Some(self)
        }
    }
    let name: &str = "HOME";
    let picked: Option<&str> = name.as_str();
    assert_eq!(picked, Some("HOME"));
    // String's own `as_str` is stable: still the inherent one
    let owned = String::from("x");
    let s: &str = owned.as_str();
    assert_eq!(s, "x");
}
