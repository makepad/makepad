//! C/Unicode escape decoder for udev labels (serialport's `unescaper` 0.1.8).
//! Public API is `unescape`.

use std::fmt;

/// Invalid escape (or resulting invalid UTF-8) at a byte offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EscapeError {
    pub offset: usize,
}

impl fmt::Display for EscapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid escape at byte offset {}", self.offset)
    }
}

impl std::error::Error for EscapeError {}

/// Decode C and Unicode escapes in `input`.
pub fn unescape(input: &str) -> Result<String, EscapeError> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'\\' {
            out.push(b);
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        let Some(&esc) = bytes.get(i) else {
            return Err(EscapeError { offset: start });
        };
        i += 1;
        match esc {
            b'\\' | b'\'' | b'"' | b'/' => out.push(esc),
            b'a' => out.push(7),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'n' => out.push(10),
            b'r' => out.push(13),
            b't' => out.push(9),
            b'v' => out.push(11),
            b'e' => out.push(27),
            b'x' => {
                // Exactly two hex digits to one byte: udev encodes UTF-8 bytes,
                // not individual Unicode characters.
                let Some(val) = parse_hex(bytes, i, 2) else {
                    return Err(EscapeError { offset: start });
                };
                out.push(val as u8);
                i += 2;
            }
            b'u' => {
                if bytes.get(i) == Some(&b'{') {
                    i += 1;
                    let hex_start = i;
                    while i < bytes.len() && hex_value(bytes[i]).is_some() {
                        i += 1;
                    }
                    let n = i - hex_start;
                    if n == 0 || n > 6 || bytes.get(i) != Some(&b'}') {
                        return Err(EscapeError { offset: start });
                    }
                    i += 1;
                    let Some(code) = parse_hex(bytes, hex_start, n) else {
                        return Err(EscapeError { offset: start });
                    };
                    push_codepoint(&mut out, code, start)?;
                } else {
                    let Some(code) = parse_hex(bytes, i, 4) else {
                        return Err(EscapeError { offset: start });
                    };
                    push_codepoint(&mut out, code, start)?;
                    i += 4;
                }
            }
            b'U' => {
                let Some(code) = parse_hex(bytes, i, 8) else {
                    return Err(EscapeError { offset: start });
                };
                push_codepoint(&mut out, code, start)?;
                i += 8;
            }
            b'0'..=b'7' => {
                let mut val = (esc - b'0') as u32;
                let mut n = 1;
                while n < 3 {
                    let Some(&d) = bytes.get(i) else {
                        break;
                    };
                    if !(b'0'..=b'7').contains(&d) {
                        break;
                    }
                    val = val * 8 + (d - b'0') as u32;
                    i += 1;
                    n += 1;
                }
                if val > 255 {
                    return Err(EscapeError { offset: start });
                }
                out.push(val as u8);
            }
            _ => return Err(EscapeError { offset: start }),
        }
    }
    match String::from_utf8(out) {
        Ok(s) => Ok(s),
        Err(err) => Err(EscapeError {
            offset: err.utf8_error().valid_up_to(),
        }),
    }
}

fn hex_value(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some((b - b'0') as u32),
        b'a'..=b'f' => Some((b - b'a' + 10) as u32),
        b'A'..=b'F' => Some((b - b'A' + 10) as u32),
        _ => None,
    }
}

/// Consume exactly `width` hex digits starting at `start`. None on short input or bad hex.
fn parse_hex(bytes: &[u8], start: usize, width: usize) -> Option<u32> {
    let end = start.checked_add(width)?;
    if end > bytes.len() {
        return None;
    }
    let mut val = 0u32;
    for i in start..end {
        val = (val << 4) | hex_value(bytes[i])?;
    }
    Some(val)
}

fn push_codepoint(out: &mut Vec<u8>, code: u32, start: usize) -> Result<(), EscapeError> {
    let Some(ch) = char::from_u32(code) else {
        return Err(EscapeError { offset: start });
    };
    let mut buf = [0u8; 4];
    let encoded = ch.encode_utf8(&mut buf);
    out.extend_from_slice(encoded.as_bytes());
    Ok(())
}
