//! Small client-side primitives: real clock, hex, and bounded text hygiene.
//! Only the top-level client supplies real time; every lower layer takes an
//! explicit `now_ms`/deadline so tests are deterministic.

/// Wall-clock milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Strict lowercase-hex decode of exactly `N` bytes; anything else is None.
pub fn from_hex_exact<const N: usize>(s: &str) -> Option<[u8; N]> {
    let b = s.as_bytes();
    if b.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for i in 0..N {
        let hi = hex_val(b[i * 2])?;
        let lo = hex_val(b[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// Bound and sanitize server-supplied display text: control characters are
/// dropped, the result is cut at a char boundary within `max` bytes. Used for
/// error details and other strings that reach logs or UI labels; identifiers
/// and DTO fields have their own strict validators and never pass through
/// here.
pub fn sanitize_text(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max));
    for c in s.chars() {
        if c.is_control() {
            continue;
        }
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip_and_strictness() {
        let bytes = [0u8, 15, 255, 16];
        let hex = to_hex(&bytes);
        assert_eq!(hex, "000fff10");
        assert_eq!(from_hex_exact::<4>(&hex), Some(bytes));
        assert_eq!(from_hex_exact::<4>("000FFF10"), None); // uppercase refused
        assert_eq!(from_hex_exact::<4>("000fff1"), None); // short
        assert_eq!(from_hex_exact::<3>(&hex), None); // wrong width
        assert_eq!(from_hex_exact::<4>("000fff1g"), None); // charset
    }

    #[test]
    fn sanitize_bounds_and_strips_control() {
        assert_eq!(sanitize_text("ab\x00c\nd", 16), "abcd");
        assert_eq!(sanitize_text("aaaa", 3), "aaa");
        // Never cuts inside a multi-byte char.
        assert_eq!(sanitize_text("aé", 2), "a");
    }
}
