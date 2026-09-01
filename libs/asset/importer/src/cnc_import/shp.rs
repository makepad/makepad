use super::{
    lcw,
    shp_ts::{ShpTs, ShpTsError},
    xor_delta,
};
use std::fmt;

#[derive(Clone, Debug)]
pub enum Sprite {
    Classic(Shp),
    TiberianSun(ShpTs),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpriteError {
    Classic(ShpError),
    TiberianSun(ShpTsError),
}

impl fmt::Display for SpriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Classic(error) => write!(f, "classic SHP error: {error}"),
            Self::TiberianSun(error) => write!(f, "TS SHP error: {error}"),
        }
    }
}

impl std::error::Error for SpriteError {}

impl Sprite {
    /// Parses the TS flavor when the leading word is zero, otherwise the
    /// original 1995 TD/RA flavor.
    pub fn parse(bytes: &[u8]) -> Result<Self, SpriteError> {
        match bytes.get(..2) {
            Some([0, 0]) => ShpTs::parse(bytes)
                .map(Self::TiberianSun)
                .map_err(SpriteError::TiberianSun),
            _ => Shp::parse(bytes)
                .map(Self::Classic)
                .map_err(SpriteError::Classic),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShpError {
    Unsupported,
    Truncated,
    InvalidFormat(u8),
    InvalidReference,
    InvalidFrameSize,
    Lcw(lcw::LcwError),
    XorDelta(xor_delta::XorDeltaError),
}

impl fmt::Display for ShpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("unsupported SHP variant"),
            Self::Truncated => f.write_str("truncated SHP sprite"),
            Self::InvalidFormat(format) => write!(f, "invalid SHP frame format {format:#04x}"),
            Self::InvalidReference => f.write_str("invalid SHP delta reference"),
            Self::InvalidFrameSize => f.write_str("decoded SHP frame has the wrong size"),
            Self::Lcw(error) => write!(f, "SHP LCW error: {error}"),
            Self::XorDelta(error) => write!(f, "SHP XOR delta error: {error}"),
        }
    }
}

impl std::error::Error for ShpError {}

#[derive(Clone, Copy, Debug)]
struct FrameEntry {
    offset: usize,
    format: u8,
    ref_offset: usize,
}

#[derive(Clone, Debug)]
pub struct Shp {
    width: u16,
    height: u16,
    frames: Vec<Vec<u8>>,
}

impl Shp {
    pub fn parse(bytes: &[u8]) -> Result<Self, ShpError> {
        if bytes.len() < 14 {
            return Err(ShpError::Unsupported);
        }
        let frame_count = read_u16(bytes, 0)? as usize;
        let width = read_u16(bytes, 6)?;
        let height = read_u16(bytes, 8)?;
        if frame_count == 0 || width == 0 || height == 0 || width > 1024 || height > 1024 {
            return Err(ShpError::Unsupported);
        }
        let table_bytes = frame_count
            .checked_add(2)
            .and_then(|count| count.checked_mul(8))
            .ok_or(ShpError::Unsupported)?;
        // The actual TD files use a 14-byte header: largest_frame_size is a
        // u16 followed by u16 flags. The first data offset aligns exactly to
        // the end of this table in every supported sprite.
        let table_end = 14usize.checked_add(table_bytes).ok_or(ShpError::Unsupported)?;
        if table_end > bytes.len() {
            return Err(ShpError::Unsupported);
        }
        let mut entries = Vec::with_capacity(frame_count + 2);
        for index in 0..frame_count + 2 {
            let at = 14 + index * 8;
            let a = read_u32(bytes, at)?;
            let b = read_u32(bytes, at + 4)?;
            entries.push(FrameEntry {
                offset: (a & 0x00ff_ffff) as usize,
                format: (a >> 24) as u8,
                ref_offset: (b & 0x00ff_ffff) as usize,
            });
        }
        if entries[0].offset != table_end
            || entries[..=frame_count]
                .iter()
                .any(|entry| entry.offset < table_end || entry.offset > bytes.len())
        {
            return Err(ShpError::Unsupported);
        }
        let frame_size = (width as usize)
            .checked_mul(height as usize)
            .ok_or(ShpError::Unsupported)?;
        let mut frames: Vec<Vec<u8>> = Vec::with_capacity(frame_count);
        for index in 0..frame_count {
            let entry = entries[index];
            let end = entries[index + 1].offset;
            if end < entry.offset || end > bytes.len() {
                return Err(ShpError::Truncated);
            }
            let encoded = &bytes[entry.offset..end];
            let frame = match entry.format {
                0x80 => {
                    let mut frame = Vec::with_capacity(frame_size);
                    lcw::decode(encoded, &mut frame).map_err(ShpError::Lcw)?;
                    frame
                }
                0x40 => {
                    let reference_index = entries[..frame_count]
                        .iter()
                        .position(|candidate| candidate.offset == entry.ref_offset)
                        .filter(|&reference| reference < index)
                        .ok_or(ShpError::InvalidReference)?;
                    let mut frame = frames[reference_index].clone();
                    xor_delta::decode(encoded, &mut frame).map_err(ShpError::XorDelta)?;
                    frame
                }
                0x20 => {
                    let mut frame = frames.last().cloned().ok_or(ShpError::InvalidReference)?;
                    xor_delta::decode(encoded, &mut frame).map_err(ShpError::XorDelta)?;
                    frame
                }
                format => return Err(ShpError::InvalidFormat(format)),
            };
            if frame.len() != frame_size {
                return Err(ShpError::InvalidFrameSize);
            }
            frames.push(frame);
        }
        Ok(Self {
            width,
            height,
            frames,
        })
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn frames(&self) -> &[Vec<u8>] {
        &self.frames
    }
}

fn read_u16(bytes: &[u8], at: usize) -> Result<u16, ShpError> {
    let value = bytes.get(at..at + 2).ok_or(ShpError::Truncated)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, ShpError> {
    let value = bytes.get(at..at + 4).ok_or(ShpError::Truncated)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_sprite_routes_by_leading_word() {
        let empty_ts = [0u8; 8];
        assert!(matches!(
            Sprite::parse(&empty_ts),
            Ok(Sprite::TiberianSun(_))
        ));
        assert!(matches!(
            Sprite::parse(&[1, 0]),
            Err(SpriteError::Classic(_))
        ));
    }
}
