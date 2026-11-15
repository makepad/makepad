//! `use std::char;` brings the module in scope, and `char::is_ascii_hexdigit` is then the
//! primitive type's method: the module has no such item, so the path falls back to the primitive
//! (rustc_resolve late.rs resolve_qpath, the `use std::u8; u8::max_value()` case). A lexer can pass
//! `char::is_ascii_hexdigit` as a `fn(&char) -> bool`; Relax's core::char had free fns of those
//! names (what char's methods are made of), and the path found them instead.
use std::char;

fn count_while(s: &str, f: fn(&char) -> bool) -> usize {
    s.chars().take_while(f).count()
}

#[test]
fn a_char_method_through_the_char_module_path() {
    assert_eq!(count_while("ab1z", char::is_ascii_hexdigit), 3);
    assert_eq!(count_while("  x", char::is_ascii_whitespace), 2);
    let alphanumeric: fn(char) -> bool = char::is_alphanumeric;
    assert!(alphanumeric('a') && !alphanumeric('-'));
    assert_eq!(char::to_digit('7', 10), Some(7));
    assert_eq!(char::to_ascii_uppercase(&'q'), 'Q');
    // the module's own functions still resolve to the module
    assert_eq!(char::from_u32(65), Some('A'));
    assert_eq!(char::from_digit(7, 10), Some('7'));
    assert_eq!(char::decode_utf16([0x61u16]).map(|r| r.unwrap()).collect::<String>(), "a");
}
