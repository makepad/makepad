//! Bounds-checked decoder for Westwood's nibble-packed bitmap fonts.

use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FntError {
    Truncated,
    InvalidFileSize { declared: usize, actual: usize },
    InvalidHeader,
    InvalidTables,
    InvalidGlyph { index: usize },
}

impl fmt::Display for FntError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated FNT font"),
            Self::InvalidFileSize { declared, actual } => {
                write!(f, "FNT size field is {declared}, file is {actual} bytes")
            }
            Self::InvalidHeader => f.write_str("invalid FNT header"),
            Self::InvalidTables => f.write_str("invalid FNT table layout"),
            Self::InvalidGlyph { index } => write!(f, "invalid FNT glyph {index}"),
        }
    }
}

impl std::error::Error for FntError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FntGlyph {
    width: u8,
    y_offset: u8,
    height: u8,
    pixels: Vec<u8>,
}

impl FntGlyph {
    pub fn width(&self) -> u8 {
        self.width
    }

    pub fn y_offset(&self) -> u8 {
        self.y_offset
    }

    pub fn height(&self) -> u8 {
        self.height
    }

    /// Row-major four-bit shade values. Zero is transparent.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fnt {
    line_height: u8,
    max_width: u8,
    glyphs: Vec<FntGlyph>,
}

impl Fnt {
    pub fn parse(bytes: &[u8]) -> Result<Self, FntError> {
        if bytes.len() < 20 {
            return Err(FntError::Truncated);
        }
        let declared_size = read_u16(bytes, 0)? as usize;
        if declared_size != bytes.len() {
            return Err(FntError::InvalidFileSize {
                declared: declared_size,
                actual: bytes.len(),
            });
        }
        if bytes[2] != 0 || bytes[3] != 5 {
            return Err(FntError::InvalidHeader);
        }

        // Empirical TD layout. The first offset names a six-byte metadata
        // record; the remaining four name the absolute-offset, width, pixel,
        // and two-byte height tables respectively.
        let metadata_at = read_u16(bytes, 4)? as usize;
        let glyph_offsets_at = read_u16(bytes, 6)? as usize;
        let widths_at = read_u16(bytes, 8)? as usize;
        let data_at = read_u16(bytes, 10)? as usize;
        let heights_at = read_u16(bytes, 12)? as usize;
        let metadata_end = metadata_at.checked_add(6).ok_or(FntError::InvalidTables)?;
        let metadata = bytes
            .get(metadata_at..metadata_end)
            .ok_or(FntError::InvalidTables)?;
        if metadata[2] != 0 {
            return Err(FntError::InvalidHeader);
        }
        let glyph_count = metadata[3] as usize + 1;
        let line_height = metadata[4];
        let max_width = metadata[5];
        if glyph_count > 256 || line_height == 0 || max_width == 0 {
            return Err(FntError::InvalidHeader);
        }

        let offsets_bytes = glyph_count.checked_mul(2).ok_or(FntError::InvalidTables)?;
        let offsets_end = glyph_offsets_at
            .checked_add(offsets_bytes)
            .ok_or(FntError::InvalidTables)?;
        let widths_end = widths_at
            .checked_add(glyph_count)
            .ok_or(FntError::InvalidTables)?;
        let heights_bytes = glyph_count.checked_mul(2).ok_or(FntError::InvalidTables)?;
        let heights_end = heights_at
            .checked_add(heights_bytes)
            .ok_or(FntError::InvalidTables)?;
        if metadata_end > glyph_offsets_at
            || offsets_end > widths_at
            || widths_end > data_at
            || data_at > heights_at
            || heights_end != bytes.len()
        {
            return Err(FntError::InvalidTables);
        }

        let mut glyphs = Vec::with_capacity(glyph_count);
        for index in 0..glyph_count {
            let offset_at = glyph_offsets_at
                .checked_add(index.checked_mul(2).ok_or(FntError::InvalidTables)?)
                .ok_or(FntError::InvalidTables)?;
            let glyph_at = read_u16(bytes, offset_at)? as usize;
            let width = *bytes.get(widths_at + index).ok_or(FntError::InvalidTables)?;
            let height_at = heights_at
                .checked_add(index.checked_mul(2).ok_or(FntError::InvalidTables)?)
                .ok_or(FntError::InvalidTables)?;
            let y_offset = *bytes.get(height_at).ok_or(FntError::InvalidTables)?;
            let height = *bytes.get(height_at + 1).ok_or(FntError::InvalidTables)?;
            if width > max_width
                || y_offset > line_height
                || height > line_height
                || y_offset
                    .checked_add(height)
                    .is_none_or(|bottom| bottom > line_height)
            {
                return Err(FntError::InvalidGlyph { index });
            }
            let pixel_count = (width as usize)
                .checked_mul(height as usize)
                .ok_or(FntError::InvalidGlyph { index })?;
            // Rows are nibble-packed LOW nibble first and each row starts on a
            // byte boundary: an odd width carries one padding nibble per row.
            // (Packing the whole glyph as one nibble stream sheared every
            // odd-width glyph by a pixel per row.)
            let row_bytes = (width as usize + 1) / 2;
            let packed_len = row_bytes
                .checked_mul(height as usize)
                .ok_or(FntError::InvalidGlyph { index })?;
            // Unassigned slots use the explicit empty sentinel
            // `(offset,width,yoff,height) = (0,0,line_height,0)`. A space,
            // by contrast, is empty but retains a real data offset and
            // advance width.
            if pixel_count == 0 && glyph_at == 0 {
                glyphs.push(FntGlyph {
                    width,
                    y_offset,
                    height,
                    pixels: Vec::new(),
                });
                continue;
            }
            if glyph_at < data_at || glyph_at > heights_at {
                return Err(FntError::InvalidGlyph { index });
            }
            let glyph_end = glyph_at
                .checked_add(packed_len)
                .ok_or(FntError::InvalidGlyph { index })?;
            let packed = bytes
                .get(glyph_at..glyph_end)
                .filter(|_| glyph_end <= heights_at)
                .ok_or(FntError::InvalidGlyph { index })?;
            let mut pixels = Vec::with_capacity(pixel_count);
            for row in packed.chunks(row_bytes.max(1)) {
                let mut emitted = 0usize;
                for &byte in row {
                    if emitted < width as usize {
                        pixels.push(byte & 0x0f);
                        emitted += 1;
                    }
                    if emitted < width as usize {
                        pixels.push(byte >> 4);
                        emitted += 1;
                    }
                }
            }
            glyphs.push(FntGlyph {
                width,
                y_offset,
                height,
                pixels,
            });
        }
        Ok(Self {
            line_height,
            max_width,
            glyphs,
        })
    }

    pub fn line_height(&self) -> u8 {
        self.line_height
    }

    pub fn max_width(&self) -> u8 {
        self.max_width
    }

    pub fn glyphs(&self) -> &[FntGlyph] {
        &self.glyphs
    }
}

fn read_u16(bytes: &[u8], at: usize) -> Result<u16, FntError> {
    let pair = bytes.get(at..at + 2).ok_or(FntError::Truncated)?;
    Ok(u16::from_le_bytes([pair[0], pair[1]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_fnt_decodes_synthetic_two_glyph_font() {
        let mut bytes = Vec::new();
        // Glyph rows are byte-aligned: the 3-wide glyph packs each row as
        // two bytes (three nibbles + one padding nibble).
        bytes.extend_from_slice(&35u16.to_le_bytes());
        bytes.extend_from_slice(&[0, 5]);
        for offset in [14u16, 20, 24, 26, 31] {
            bytes.extend_from_slice(&offset.to_le_bytes());
        }
        bytes.extend_from_slice(&[0x12, 0x10, 0, 1, 2, 3]);
        bytes.extend_from_slice(&26u16.to_le_bytes());
        bytes.extend_from_slice(&27u16.to_le_bytes());
        bytes.extend_from_slice(&[2, 3]);
        bytes.extend_from_slice(&[0x21, 0x01, 0x03, 0x02, 0x01]);
        bytes.extend_from_slice(&[1, 1, 0, 2]);

        let font = Fnt::parse(&bytes).expect("synthetic FNT");
        assert_eq!((font.line_height(), font.max_width()), (2, 3));
        assert_eq!(font.glyphs().len(), 2);
        assert_eq!(font.glyphs()[0].pixels(), [1, 2]);
        assert_eq!(font.glyphs()[1].pixels(), [1, 0, 3, 2, 0, 1]);
        assert_eq!((font.glyphs()[0].y_offset(), font.glyphs()[0].height()), (1, 1));
    }

    #[test]
    fn cnc_import_fnt_rejects_truncation_and_bad_glyph_bounds() {
        assert_eq!(Fnt::parse(&[0; 19]), Err(FntError::Truncated));

        let mut bytes = vec![0u8; 24];
        bytes[0..2].copy_from_slice(&24u16.to_le_bytes());
        bytes[3] = 5;
        bytes[4..6].copy_from_slice(&14u16.to_le_bytes());
        bytes[6..8].copy_from_slice(&20u16.to_le_bytes());
        bytes[8..10].copy_from_slice(&22u16.to_le_bytes());
        bytes[10..12].copy_from_slice(&23u16.to_le_bytes());
        bytes[12..14].copy_from_slice(&23u16.to_le_bytes());
        bytes[14..20].copy_from_slice(&[0x12, 0x10, 0, 0, 1, 1]);
        assert!(Fnt::parse(&bytes).is_err());
    }
}
