use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum XorDeltaError {
    Truncated,
    OutputOverflow,
    MissingEnd,
}

impl fmt::Display for XorDeltaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated XOR delta stream"),
            Self::OutputOverflow => f.write_str("XOR delta runs past the output frame"),
            Self::MissingEnd => f.write_str("XOR delta stream has no end command"),
        }
    }
}

impl std::error::Error for XorDeltaError {}

pub fn decode(src: &[u8], dst: &mut [u8]) -> Result<(), XorDeltaError> {
    let mut src_at = 0usize;
    let mut dst_at = 0usize;
    while let Some(&command) = src.get(src_at) {
        src_at += 1;
        if command & 0x80 == 0 {
            if command == 0 {
                let count = take_u8(src, &mut src_at)? as usize;
                let value = take_u8(src, &mut src_at)?;
                xor_fill(dst, &mut dst_at, count, value)?;
            } else {
                xor_literal(src, &mut src_at, dst, &mut dst_at, command as usize)?;
            }
        } else if command & 0x7f != 0 {
            advance_dst(dst, &mut dst_at, (command & 0x7f) as usize)?;
        } else {
            let mut count = take_u16(src, &mut src_at)?;
            if count == 0 {
                return Ok(());
            }
            if count & 0x8000 == 0 {
                advance_dst(dst, &mut dst_at, count as usize)?;
            } else if count & 0x4000 == 0 {
                count &= 0x3fff;
                xor_literal(src, &mut src_at, dst, &mut dst_at, count as usize)?;
            } else {
                count &= 0x3fff;
                let value = take_u8(src, &mut src_at)?;
                xor_fill(dst, &mut dst_at, count as usize, value)?;
            }
        }
    }
    Err(XorDeltaError::MissingEnd)
}

fn take_u8(src: &[u8], at: &mut usize) -> Result<u8, XorDeltaError> {
    let value = *src.get(*at).ok_or(XorDeltaError::Truncated)?;
    *at += 1;
    Ok(value)
}

fn take_u16(src: &[u8], at: &mut usize) -> Result<u16, XorDeltaError> {
    let lo = take_u8(src, at)?;
    let hi = take_u8(src, at)?;
    Ok(u16::from_le_bytes([lo, hi]))
}

fn advance_dst(dst: &[u8], at: &mut usize, count: usize) -> Result<(), XorDeltaError> {
    let end = at.checked_add(count).ok_or(XorDeltaError::OutputOverflow)?;
    if end > dst.len() {
        return Err(XorDeltaError::OutputOverflow);
    }
    *at = end;
    Ok(())
}

fn xor_literal(
    src: &[u8],
    src_at: &mut usize,
    dst: &mut [u8],
    dst_at: &mut usize,
    count: usize,
) -> Result<(), XorDeltaError> {
    let src_end = src_at.checked_add(count).ok_or(XorDeltaError::Truncated)?;
    let values = src.get(*src_at..src_end).ok_or(XorDeltaError::Truncated)?;
    let dst_end = dst_at.checked_add(count).ok_or(XorDeltaError::OutputOverflow)?;
    let output = dst.get_mut(*dst_at..dst_end).ok_or(XorDeltaError::OutputOverflow)?;
    for (byte, delta) in output.iter_mut().zip(values) {
        *byte ^= delta;
    }
    *src_at = src_end;
    *dst_at = dst_end;
    Ok(())
}

fn xor_fill(
    dst: &mut [u8],
    dst_at: &mut usize,
    count: usize,
    value: u8,
) -> Result<(), XorDeltaError> {
    let end = dst_at.checked_add(count).ok_or(XorDeltaError::OutputOverflow)?;
    let output = dst.get_mut(*dst_at..end).ok_or(XorDeltaError::OutputOverflow)?;
    for byte in output {
        *byte ^= value;
    }
    *dst_at = end;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_xor_short_and_long_commands() {
        let mut frame = [1, 2, 3, 4, 5, 6, 7, 8];
        let delta = [
            2, 0x10, 0x20, // short literal
            0x82, // short skip
            0, 2, 0xff, // short fill
            0x80, 1, 0, // long skip
            1, 0x08, // final literal
            0x80, 0, 0, // end
        ];
        decode(&delta, &mut frame).unwrap();
        assert_eq!(frame, [0x11, 0x22, 3, 4, 0xfa, 0xf9, 7, 0]);
    }

    #[test]
    fn cnc_import_xor_rejects_overflow() {
        assert_eq!(decode(&[0x82], &mut [0]), Err(XorDeltaError::OutputOverflow));
    }

    #[test]
    fn cnc_import_xor_long_literal_and_fill() {
        let mut frame = [0u8; 4];
        let delta = [
            0x80, 2, 0x80, 1, 2, // long literal
            0x80, 2, 0xc0, 3, // long fill
            0x80, 0, 0,
        ];
        decode(&delta, &mut frame).unwrap();
        assert_eq!(frame, [1, 2, 3, 3]);
    }
}
