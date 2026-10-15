#[test]
fn primitive_module_paths() {
    let s = "ab\0c";
    assert!(::core::iter::Iterator::any(&mut ::core::primitive::str::bytes(s), |b| b == b'\0'));
    assert_eq!(::core::primitive::str::len("abc"), 3);
    assert_eq!(core::primitive::u32::max(3, 5), 5);
    assert_eq!(<core::primitive::u8>::MAX, 255);
    let x: core::primitive::usize = 4;
    assert_eq!(x, 4);
}

#[test]
fn std_primitive_module_paths() {
    assert_eq!(::std::primitive::str::len("abc"), 3);
    assert_eq!(std::primitive::u32::max(3, 5), 5);
    assert_eq!(<std::primitive::u8>::MAX, 255);
    let x: std::primitive::usize = 4;
    assert_eq!(x, 4);
}

mod reexports {
    pub use str as Text;
    pub use u32 as Number;
    pub use u8 as Byte;

    pub mod nested {
        pub use super::{Byte, Number, Text};
    }
}

#[test]
fn renamed_primitive_paths() {
    assert_eq!(reexports::nested::Text::bytes("abc").next(), Some(b'a'));
    assert_eq!(reexports::nested::Number::max(3, 5), 5);
    assert_eq!(reexports::nested::Byte::MAX, 255);
    let x: reexports::nested::Number = 4;
    assert_eq!(x, 4);
}
