//! Standard-alphabet base64 for script strings and byte arrays. Decoding follows
//! WHATWG forgiving-base64 (JS `atob`): ASCII whitespace is skipped, padding is optional.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

const INVALID: u8 = 0xFF;
const SKIP: u8 = 0xFE;
const PAD: u8 = 0xFD;

/// Sextet value of each input byte, or one of the INVALID / SKIP / PAD markers.
static DECODE: [u8; 256] = {
    let mut table = [INVALID; 256];
    let mut i = 0;
    while i < ALPHABET.len() {
        table[ALPHABET[i] as usize] = i as u8;
        i += 1;
    }
    // ASCII whitespace per WHATWG: tab, line feed, form feed, carriage return, space.
    table[b'\t' as usize] = SKIP;
    table[b'\n' as usize] = SKIP;
    table[0x0C] = SKIP;
    table[b'\r' as usize] = SKIP;
    table[b' ' as usize] = SKIP;
    table[b'=' as usize] = PAD;
    table
};

/// Exact decoded length, or `None` if `input` is not valid forgiving-base64.
pub(crate) fn decoded_len(input: &str) -> Option<usize> {
    let mut sextets = 0usize;
    let mut pad = 0usize;
    for &byte in input.as_bytes() {
        match DECODE[byte as usize] {
            SKIP => {}
            PAD => pad += 1,
            INVALID => return None,
            _ if pad > 0 => return None,
            _ => sextets += 1,
        }
    }
    // Padding is only legal when it completes the final 4-character group.
    let is_valid = match pad {
        0 => sextets % 4 != 1,
        1 | 2 => (sextets + pad).is_multiple_of(4),
        _ => false,
    };
    // A trailing group of 2 or 3 sextets carries 1 or 2 bytes.
    is_valid.then(|| sextets / 4 * 3 + (sextets % 4).saturating_sub(1))
}

/// Appends the bytes encoded by `input`, which must have passed `decoded_len`.
pub(crate) fn decode_into(input: &str, out: &mut Vec<u8>) {
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &byte in input.as_bytes() {
        let sextet = DECODE[byte as usize];
        if sextet < 64 {
            acc = acc << 6 | sextet as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
    }
}

/// Length of the padded encoding of `len` bytes.
pub(crate) fn encoded_len(len: usize) -> usize {
    len.div_ceil(3).saturating_mul(4)
}

/// Appends the padded standard-alphabet encoding of `bytes`.
pub(crate) fn encode_into(bytes: &[u8], out: &mut String) {
    for chunk in bytes.chunks(3) {
        let at = |i: usize| chunk.get(i).copied().unwrap_or(0) as u32;
        let group = at(0) << 16 | at(1) << 8 | at(2);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(group >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(input: &str) -> Option<Vec<u8>> {
        let len = decoded_len(input)?;
        let mut out = Vec::with_capacity(len);
        decode_into(input, &mut out);
        assert_eq!(out.len(), len, "decoded_len disagrees with decode_into for {input:?}");
        Some(out)
    }

    fn encode(bytes: &[u8]) -> String {
        let mut out = String::new();
        encode_into(bytes, &mut out);
        assert_eq!(out.len(), encoded_len(bytes.len()));
        out
    }

    #[test]
    fn rfc4648_vectors() {
        let vectors = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in vectors {
            assert_eq!(encode(plain.as_bytes()), encoded);
            assert_eq!(decode(encoded).as_deref(), Some(plain.as_bytes()));
            let unpadded = encoded.trim_end_matches('=');
            assert_eq!(decode(unpadded).as_deref(), Some(plain.as_bytes()), "{unpadded:?}");
        }
    }

    #[test]
    fn round_trips_every_length_and_byte_value() {
        for len in 0..=300usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 167 + len) as u8).collect();
            let encoded = encode(&bytes);
            assert!(encoded.len().is_multiple_of(4), "encoding must be padded: {encoded:?}");
            assert_eq!(decode(&encoded), Some(bytes.clone()), "padded, len {len}");
            assert_eq!(decode(encoded.trim_end_matches('=')), Some(bytes), "unpadded, len {len}");
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&encode(&all)), Some(all));
    }

    #[test]
    fn skips_ascii_whitespace_anywhere() {
        assert_eq!(decode(" Zm 9v\nYmFy\r\n").as_deref(), Some(&b"foobar"[..]));
        assert_eq!(decode("Zm9v\tYg\x0C=\n=").as_deref(), Some(&b"foob"[..]));
        assert_eq!(decode("Zg==\n").as_deref(), Some(&b"f"[..]));
        assert_eq!(decode(" \t\r\n").as_deref(), Some(&b""[..]));
        // Vertical tab and non-ASCII spaces are not WHATWG ASCII whitespace.
        assert_eq!(decode("Zm9v\x0BYg"), None);
        assert_eq!(decode("Zm9v\u{a0}Yg"), None);
    }

    #[test]
    fn rejects_invalid_input() {
        let invalid = [
            "Z", "Zm9vY", "=", "==", "===", "====", "Zg=", "Zg===", "Zm8==", "Zm9v=",
            "Zm9v====", "Zg==Zg==", "Zm=9v", "Zm-v", "Zm_v", "Zm9v!", "Zm9v\0", "Zm9vé",
        ];
        for input in invalid {
            assert_eq!(decoded_len(input), None, "{input:?} should be rejected");
        }
    }

    #[test]
    fn accepts_non_zero_trailing_bits_like_atob() {
        assert_eq!(decode("Zh==").as_deref(), Some(&b"f"[..]));
        assert_eq!(decode("TWF=").as_deref(), Some(&b"Ma"[..]));
        assert_eq!(decode("TR").as_deref(), Some(&b"M"[..]));
    }

    #[test]
    fn encoded_len_saturates_instead_of_wrapping() {
        assert_eq!(encoded_len(0), 0);
        assert_eq!(encoded_len(1), 4);
        assert_eq!(encoded_len(3), 4);
        assert_eq!(encoded_len(4), 8);
        assert_eq!(encoded_len(usize::MAX), usize::MAX);
    }
}
