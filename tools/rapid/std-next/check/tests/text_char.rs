//! Rapid std char vs real std, over every code point.

mod text_common;

use rapid_std_check::char as hc;
use text_common::*;

fn all_chars() -> impl Iterator<Item = char> {
    (0..=0x10FFFFu32).filter_map(char::from_u32)
}

#[test]
fn properties_all_code_points() {
    let mut n = 0;
    for c in all_chars() {
        assert_eq!(hc::is_alphabetic(c), c.is_alphabetic(), "alphabetic {:?}", c);
        assert_eq!(hc::is_lowercase(c), c.is_lowercase(), "lowercase {:?}", c);
        assert_eq!(hc::is_uppercase(c), c.is_uppercase(), "uppercase {:?}", c);
        assert_eq!(hc::is_whitespace(c), c.is_whitespace(), "whitespace {:?}", c);
        assert_eq!(hc::is_numeric(c), c.is_numeric(), "numeric {:?}", c);
        assert_eq!(hc::is_alphanumeric(c), c.is_alphanumeric(), "alphanumeric {:?}", c);
        assert_eq!(hc::is_control(c), c.is_control(), "control {:?}", c);
        assert_eq!(hc::is_ascii(c), c.is_ascii());
        assert_eq!(hc::is_ascii_alphabetic(c), c.is_ascii_alphabetic());
        assert_eq!(hc::is_ascii_uppercase(c), c.is_ascii_uppercase());
        assert_eq!(hc::is_ascii_lowercase(c), c.is_ascii_lowercase());
        assert_eq!(hc::is_ascii_alphanumeric(c), c.is_ascii_alphanumeric());
        assert_eq!(hc::is_ascii_digit(c), c.is_ascii_digit());
        assert_eq!(hc::is_ascii_octdigit(c), matches!(c, '0'..='7'));
        assert_eq!(hc::is_ascii_hexdigit(c), c.is_ascii_hexdigit());
        assert_eq!(hc::is_ascii_punctuation(c), c.is_ascii_punctuation());
        assert_eq!(hc::is_ascii_graphic(c), c.is_ascii_graphic());
        assert_eq!(hc::is_ascii_whitespace(c), c.is_ascii_whitespace());
        assert_eq!(hc::is_ascii_control(c), c.is_ascii_control());
        assert_eq!(hc::to_ascii_uppercase(c), c.to_ascii_uppercase());
        assert_eq!(hc::to_ascii_lowercase(c), c.to_ascii_lowercase());
        assert_eq!(hc::len_utf8(c as u32), c.len_utf8());
        assert_eq!(hc::len_utf16(c as u32), c.len_utf16());
        n += 1;
    }
    assert_eq!(n, 0x110000 - 0x800);
}

#[test]
fn case_mapping_all_code_points() {
    for c in all_chars() {
        let mine: Vec<char> = hc::to_lowercase(c).collect();
        let real: Vec<char> = c.to_lowercase().collect();
        assert_eq!(mine, real, "to_lowercase {:?}", c);
        let mine: Vec<char> = hc::to_uppercase(c).collect();
        let real: Vec<char> = c.to_uppercase().collect();
        assert_eq!(mine, real, "to_uppercase {:?}", c);
        // reverse iteration, len, Display
        let mine: Vec<char> = hc::to_uppercase(c).rev().collect();
        let real: Vec<char> = c.to_uppercase().rev().collect();
        assert_eq!(mine, real);
        assert_eq!(hc::to_uppercase(c).len(), c.to_uppercase().len());
        assert_eq!(hdisplay(&hc::to_lowercase(c)), c.to_lowercase().to_string());
    }
}

#[test]
fn escapes_all_code_points() {
    for c in all_chars() {
        let mine: String = hc::escape_debug(c).collect();
        assert_eq!(mine, c.escape_debug().to_string(), "escape_debug {:?}", c);
        assert_eq!(hdisplay(&hc::escape_debug(c)), c.escape_debug().to_string());
        let mine: String = hc::escape_default(c).collect();
        assert_eq!(mine, c.escape_default().to_string(), "escape_default {:?}", c);
        let mine: String = hc::escape_unicode(c).collect();
        assert_eq!(mine, c.escape_unicode().to_string(), "escape_unicode {:?}", c);
        assert_eq!(hc::escape_unicode(c).len(), c.escape_unicode().len());
        // Debug through Rapid's fmt (char Debug body lives in char/)
        assert_eq!(hdebug(&c), format!("{:?}", c), "Debug {:?}", c);
    }
}

#[test]
fn digits() {
    for radix in 2..=36u32 {
        for c in all_chars().take(0x3000) {
            assert_eq!(hc::to_digit(c, radix), c.to_digit(radix), "{:?} radix {}", c, radix);
            assert_eq!(hc::is_digit(c, radix), c.is_digit(radix));
        }
        for num in 0..40u32 {
            assert_eq!(hc::from_digit(num, radix), char::from_digit(num, radix));
        }
    }
    assert_eq!(panic_msg(|| hc::to_digit('1', 1)), panic_msg(|| '1'.to_digit(1)));
    assert_eq!(panic_msg(|| hc::to_digit('1', 37)), panic_msg(|| '1'.to_digit(37)));
    assert_eq!(panic_msg(|| hc::from_digit(1, 37)), panic_msg(|| char::from_digit(1, 37)));
}

#[test]
fn from_u32_and_try_from() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut probes: Vec<u32> = vec![0, 0x7f, 0x80, 0xd7ff, 0xd800, 0xdbff, 0xdc00, 0xdfff, 0xe000, 0xffff, 0x10000, 0x10ffff, 0x110000, u32::MAX];
    for _ in 0..100_000 {
        probes.push(rng.next() as u32 % 0x120000);
    }
    for &u in &probes {
        assert_eq!(hc::from_u32(u), char::from_u32(u));
        assert_eq!(hc::try_from_u32(u).is_ok(), char::try_from(u).is_ok());
    }
    let e = hc::try_from_u32(0xd800).unwrap_err();
    assert_eq!(hdisplay(&e), char::try_from(0xd800u32).unwrap_err().to_string());
}

#[test]
fn encode_utf8_utf16() {
    for c in all_chars() {
        let mut a = [0u8; 4];
        let mut b = [0u8; 4];
        assert_eq!(hc::encode_utf8(c, &mut a).as_bytes(), c.encode_utf8(&mut b).as_bytes());
        let mut a = [0u16; 2];
        let mut b = [0u16; 2];
        assert_eq!(hc::encode_utf16(c, &mut a), c.encode_utf16(&mut b));
    }
    for c in ['a', 'é', '€', '𝄞'] {
        for n in 0..c.len_utf8() {
            let m = panic_msg(|| {
                let mut buf = vec![0u8; n];
                hc::encode_utf8(c, &mut buf);
            });
            let r = panic_msg(|| {
                let mut buf = vec![0u8; n];
                c.encode_utf8(&mut buf);
            });
            assert_eq!(m, r);
        }
        for n in 0..c.len_utf16() {
            let m = panic_msg(|| {
                let mut buf = vec![0u16; n];
                hc::encode_utf16(c, &mut buf);
            });
            let r = panic_msg(|| {
                let mut buf = vec![0u16; n];
                c.encode_utf16(&mut buf);
            });
            assert_eq!(m, r);
        }
    }
}

#[test]
fn decode_utf16_random() {
    let mut rng = Rng(77);
    let units = [0x41u16, 0xe9, 0x20ac, 0xd834, 0xdd1e, 0xd800, 0xdc00, 0xdfff, 0xffff, 0x0];
    for _ in 0..20_000 {
        let n = rng.below(8);
        let v: Vec<u16> = (0..n).map(|_| units[rng.below(units.len())]).collect();
        let mine: Vec<Result<char, u16>> =
            hc::decode_utf16(v.iter().cloned()).map(|r| r.map_err(|e| e.unpaired_surrogate())).collect();
        let real: Vec<Result<char, u16>> =
            char::decode_utf16(v.iter().cloned()).map(|r| r.map_err(|e| e.unpaired_surrogate())).collect();
        assert_eq!(mine, real, "{:x?}", v);
        let mut it_m = hc::decode_utf16(v.iter().cloned());
        let mut it_r = char::decode_utf16(v.iter().cloned());
        loop {
            assert_eq!(it_m.size_hint(), it_r.size_hint());
            match (it_m.next(), it_r.next()) {
                (None, None) => break,
                (Some(Err(a)), Some(Err(b))) => assert_eq!(hdisplay(&a), b.to_string()),
                _ => {}
            }
        }
    }
}

#[test]
fn parse_char() {
    for s in ["", "a", "ab", "é", "𝄞", "é\u{301}", " "] {
        let m = hc::parse_char(s);
        let r = s.parse::<char>();
        assert_eq!(m.is_ok(), r.is_ok());
        match (m, r) {
            (Ok(a), Ok(b)) => assert_eq!(a, b),
            (Err(a), Err(b)) => assert_eq!(hdisplay(&a), b.to_string()),
            _ => unreachable!(),
        }
    }
}
