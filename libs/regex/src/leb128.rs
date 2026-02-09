//! Functions for encoding/decoding integers as LEB128.
//!
//! For more information, see: https://en.wikipedia.org/wiki/LEB128.

/// Decodes an `isize` as LEB128 from `bytes`.
///
/// # Panics
///
/// Panics if `bytes` is not valid LEB128.
pub fn decode_isize(bytes: &mut &[u8]) -> isize {
    let un = decode_usize(bytes);

    // Decodes `n` as a `usize`, shifted one bit to the left, with the sign in bit 0.
    let mut n = (un >> 1) as isize;
    if un & 1 != 0 {
        n = !n;
    }
    n
}

/// Decodes an `usize` as LEB128 from `bytes`.
///
/// # Panics
///
/// Panics if `bytes` is not valid LEB128.
pub fn decode_usize(bytes: &mut &[u8]) -> usize {
    let mut n = 0;
    let mut shift = 0;
    while !bytes.is_empty() {
        let b = bytes[0];
        *bytes = &bytes[1..];
        n |= ((b & 0x7F) as usize) << shift;
        if b < 0x80 {
            return n;
        }
        shift += 7;
    }
    panic!("invalid LEB128")
}

/// Encodes an `isize` as LEB128 into `bytes`.
pub fn encode_isize(bytes: &mut Vec<u8>, n: isize) {
    // Encodes `n` as a `usize`, shifted one bit to the left, with the sign in bit 0.
    let mut un = (n as usize) << 1;
    if n < 0 {
        un = !un;
    }

    encode_usize(bytes, un)
}

/// Encodes an `usize` as LEB128 into `bytes`.
pub fn encode_usize(bytes: &mut Vec<u8>, n: usize) {
    let mut n = n;
    while n >= 0x80 {
        bytes.push((n as u8) | 0x80);
        n >>= 7;
    }
    bytes.push(n as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isize() {
        let expected = -2_147_483_647isize;
        let mut bytes = Vec::new();
        encode_isize(&mut bytes, expected);
        let actual = decode_isize(&mut bytes.as_slice());
        assert_eq!(actual, expected);
    }

    #[test]
    fn usize() {
        let expected = 2_147_483_647usize;
        let mut bytes = Vec::new();
        encode_usize(&mut bytes, expected);
        let actual = decode_usize(&mut bytes.as_slice());
        assert_eq!(actual, expected);
    }
}
