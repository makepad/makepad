use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LcwError {
    Truncated,
    InvalidBackReference,
    MissingEnd,
}

impl fmt::Display for LcwError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated LCW stream"),
            Self::InvalidBackReference => f.write_str("invalid LCW back-reference"),
            Self::MissingEnd => f.write_str("LCW stream has no end command"),
        }
    }
}

impl std::error::Error for LcwError {}

pub fn decode(src: &[u8], dst: &mut Vec<u8>) -> Result<usize, LcwError> {
    let initial_len = dst.len();
    let mut at = 0usize;
    while let Some(&cmd) = src.get(at) {
        at += 1;
        if cmd & 0x80 == 0 {
            let count = ((cmd & 0x70) >> 4) as usize + 3;
            let low = *src.get(at).ok_or(LcwError::Truncated)? as usize;
            at += 1;
            let rel = ((cmd as usize & 0x0f) << 8) | low;
            if rel == 0 || rel > dst.len() {
                return Err(LcwError::InvalidBackReference);
            }
            let pos = dst.len() - rel;
            copy_from_output(dst, pos, count)?;
        } else if cmd & 0x40 == 0 {
            let count = (cmd & 0x3f) as usize;
            if count == 0 {
                return Ok(dst.len() - initial_len);
            }
            let end = at.checked_add(count).ok_or(LcwError::Truncated)?;
            dst.extend_from_slice(src.get(at..end).ok_or(LcwError::Truncated)?);
            at = end;
        } else {
            match cmd & 0x3f {
                0x3e => {
                    let count = take_u16(src, &mut at)? as usize;
                    let value = *src.get(at).ok_or(LcwError::Truncated)?;
                    at += 1;
                    let new_len = dst.len().checked_add(count).ok_or(LcwError::Truncated)?;
                    dst.resize(new_len, value);
                }
                0x3f => {
                    let count = take_u16(src, &mut at)? as usize;
                    let pos = take_u16(src, &mut at)? as usize;
                    copy_from_output(dst, pos, count)?;
                }
                count => {
                    let pos = take_u16(src, &mut at)? as usize;
                    copy_from_output(dst, pos, count as usize + 3)?;
                }
            }
        }
    }
    Err(LcwError::MissingEnd)
}

fn take_u16(src: &[u8], at: &mut usize) -> Result<u16, LcwError> {
    let end = at.checked_add(2).ok_or(LcwError::Truncated)?;
    let bytes = src.get(*at..end).ok_or(LcwError::Truncated)?;
    *at = end;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn copy_from_output(dst: &mut Vec<u8>, pos: usize, count: usize) -> Result<(), LcwError> {
    if count != 0 && pos >= dst.len() {
        return Err(LcwError::InvalidBackReference);
    }
    for index in 0..count {
        let source = pos.checked_add(index).ok_or(LcwError::InvalidBackReference)?;
        let value = *dst.get(source).ok_or(LcwError::InvalidBackReference)?;
        dst.push(value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_lcw_commands_and_overlap() {
        let mut decoded = Vec::new();
        // Literal "ABC", relative overlap to make "ABCABC", fill, then end.
        let stream = [0x83, b'A', b'B', b'C', 0x00, 0x03, 0xfe, 0x03, 0x00, b'!', 0x80];
        assert_eq!(decode(&stream, &mut decoded), Ok(9));
        assert_eq!(decoded, b"ABCABC!!!");
    }

    #[test]
    fn cnc_import_lcw_rejects_bad_input() {
        assert_eq!(decode(&[0, 1], &mut Vec::new()), Err(LcwError::InvalidBackReference));
        assert_eq!(decode(&[0x82, 1], &mut Vec::new()), Err(LcwError::Truncated));
    }

    #[test]
    fn cnc_import_lcw_absolute_commands_overlap() {
        let mut decoded = Vec::new();
        let stream = [
            0x83, b'A', b'B', b'C', // literal
            0xc0, 0, 0, // short absolute: 3 bytes from 0
            0xff, 3, 0, 3, 0, // long absolute: 3 bytes from 3
            0x80,
        ];
        assert_eq!(decode(&stream, &mut decoded), Ok(9));
        assert_eq!(decoded, b"ABCABCABC");
    }
}
