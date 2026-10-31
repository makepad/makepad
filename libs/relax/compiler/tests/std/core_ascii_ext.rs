#![allow(deprecated)]

use std::ascii::AsciiExt;

fn upper(byte: u8) -> u8 {
    if byte >= b'a' && byte <= b'z' { byte - 32 } else { byte }
}

fn lower(byte: u8) -> u8 {
    if byte >= b'A' && byte <= b'Z' { byte + 32 } else { byte }
}

#[test]
fn ascii_ext_bytes_and_chars() {
    // UFCS exercises the trait, even when identically named inherent methods exist.
    for n in 0..=255u16 {
        let b = n as u8;
        assert_eq!(AsciiExt::is_ascii(&b), b < 128);
        assert_eq!(AsciiExt::to_ascii_uppercase(&b), upper(b));
        assert_eq!(AsciiExt::to_ascii_lowercase(&b), lower(b));
        let mut changed = b;
        AsciiExt::make_ascii_uppercase(&mut changed);
        assert_eq!(changed, upper(b));
        AsciiExt::make_ascii_lowercase(&mut changed);
        assert_eq!(changed, lower(b));

        let c = b as char;
        assert_eq!(AsciiExt::is_ascii(&c), b < 128);
        assert_eq!(AsciiExt::to_ascii_uppercase(&c), upper(b) as char);
        assert_eq!(AsciiExt::to_ascii_lowercase(&c), lower(b) as char);
        let mut changed = c;
        AsciiExt::make_ascii_uppercase(&mut changed);
        assert_eq!(changed, upper(b) as char);
        AsciiExt::make_ascii_lowercase(&mut changed);
        assert_eq!(changed, lower(b) as char);

        for m in 0..=255u16 {
            let other = m as u8;
            let equal = lower(b) == lower(other);
            assert_eq!(AsciiExt::eq_ignore_ascii_case(&b, &other), equal);
            assert_eq!(AsciiExt::eq_ignore_ascii_case(&c, &(other as char)), equal);
        }
    }
    for c in ['\u{301}', '中', '🦀', '\u{10ffff}'] {
        assert!(!AsciiExt::is_ascii(&c));
        assert_eq!(AsciiExt::to_ascii_uppercase(&c), c);
        assert_eq!(AsciiExt::to_ascii_lowercase(&c), c);
        let mut changed = c;
        AsciiExt::make_ascii_uppercase(&mut changed);
        AsciiExt::make_ascii_lowercase(&mut changed);
        assert_eq!(changed, c);
    }
    println!("| AsciiExt bytes and chars");
}

#[test]
fn ascii_ext_slices_and_strings() {
    let bytes: Vec<u8> = (0..=255u16).map(|n| n as u8).collect();
    let uppercase: Vec<u8> = bytes.iter().map(|b| upper(*b)).collect();
    let lowercase: Vec<u8> = bytes.iter().map(|b| lower(*b)).collect();
    assert!(!AsciiExt::is_ascii(bytes.as_slice()));
    assert!(AsciiExt::is_ascii(&bytes[..128]));
    assert!(AsciiExt::is_ascii(&bytes[..0]));
    let owned_upper: Vec<u8> = AsciiExt::to_ascii_uppercase(bytes.as_slice());
    let owned_lower: Vec<u8> = AsciiExt::to_ascii_lowercase(bytes.as_slice());
    assert_eq!(owned_upper, uppercase);
    assert_eq!(owned_lower, lowercase);
    assert!(AsciiExt::eq_ignore_ascii_case(uppercase.as_slice(), lowercase.as_slice()));
    assert!(!AsciiExt::eq_ignore_ascii_case(&bytes[..1], &bytes[..0]));
    let mut changed = bytes.clone();
    AsciiExt::make_ascii_uppercase(changed.as_mut_slice());
    assert_eq!(changed, uppercase);
    AsciiExt::make_ascii_lowercase(changed.as_mut_slice());
    assert_eq!(changed, lowercase);

    for s in ["", "aZ09\0\n", "café CAFÉ", "Cafe\u{301} 中🦀"] {
        let uppercase = String::from_utf8(s.bytes().map(upper).collect()).unwrap();
        let lowercase = String::from_utf8(s.bytes().map(lower).collect()).unwrap();
        assert_eq!(AsciiExt::is_ascii(s), s.bytes().all(|b| b < 128));
        let owned_upper: String = AsciiExt::to_ascii_uppercase(s);
        let owned_lower: String = AsciiExt::to_ascii_lowercase(s);
        assert_eq!(owned_upper, uppercase);
        assert_eq!(owned_lower, lowercase);
        assert!(AsciiExt::eq_ignore_ascii_case(uppercase.as_str(), lowercase.as_str()));
        let mut changed = s.to_owned();
        AsciiExt::make_ascii_uppercase(changed.as_mut_str());
        assert_eq!(changed, uppercase);
        AsciiExt::make_ascii_lowercase(changed.as_mut_str());
        assert_eq!(changed, lowercase);
    }
    assert!(!AsciiExt::eq_ignore_ascii_case("é", "É"));
    assert!(!AsciiExt::eq_ignore_ascii_case("abc", "ABCD"));
    assert_eq!(std::ascii::escape_default(b'\n').to_string(), "\\n");
    println!("| AsciiExt slices and strings");
}
