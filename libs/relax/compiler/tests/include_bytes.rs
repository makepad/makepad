// include_bytes! passes the file as a payload behind a marker literal (no escape/lex/unescape round trip):
// the bytes, their length in the array type, const/static use and the same file twice must all hold.
// Quotes " and backslashes \ \0 \x00 in this file are part of what is checked.
const SELF: &[u8] = include_bytes!("include_bytes.rs");
static SELF2: &[u8; SELF.len()] = include_bytes!("include_bytes.rs");
#[test] fn a_equals_include_str() { assert_eq!(SELF, include_str!("include_bytes.rs").as_bytes()); }
#[test] fn b_static_and_len() { assert_eq!(SELF2.len(), include_str!("include_bytes.rs").len()); assert_eq!(&SELF2[..], SELF); }
#[test] fn c_index() { let b = include_bytes!("include_bytes.rs"); assert_eq!(b[0], b'/'); assert_eq!(b[b.len() - 1], b'\n'); }
#[test] fn d_plain_literal_with_nul() { let b = b"\0relax"; assert_eq!(b.len(), 6); assert_eq!(b[0], 0); assert_eq!("\0x".len(), 2); }
