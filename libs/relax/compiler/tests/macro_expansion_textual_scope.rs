//! An expansion must not see a later block-local macro with the same name.
use std::fmt;

macro_rules! display {
    () => {
        struct Value;
        impl fmt::Display for Value {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "value={}", 7)
            }
        }
    };
}
display!();

fn unrelated() -> u32 {
    macro_rules! write {
        ($value:expr) => { $value + 1 };
    }
    write!(2)
}

#[test]
fn expanded_write_uses_the_scope_of_its_invocation() {
    assert_eq!(Value.to_string(), "value=7");
    assert_eq!(unrelated(), 3);
}

#[test]
fn nested_expansion_sees_an_in_scope_local_macro() {
    macro_rules! local { () => { 19u32 }; }
    macro_rules! inner { () => { local!() }; }
    macro_rules! outer { () => { inner!() }; }
    assert_eq!(outer!(), 19);
    {
        macro_rules! local { () => { 23u32 }; }
        assert_eq!(outer!(), 23);
    }
    assert_eq!(outer!(), 19);
}
