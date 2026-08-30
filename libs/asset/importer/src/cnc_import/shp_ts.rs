use std::fmt;

const FILE_HEADER_SIZE: usize = 8;
const FRAME_HEADER_SIZE: usize = 24;
const MAX_DECODED_PIXELS: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShpTsError {
    Truncated,
    InvalidHeader,
    InvalidDimensions,
    InvalidFrameBounds,
    InvalidDataOffset,
    InvalidRle,
    ResourceLimit,
}

impl fmt::Display for ShpTsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated TS SHP sprite"),
            Self::InvalidHeader => f.write_str("invalid TS SHP header"),
            Self::InvalidDimensions => f.write_str("invalid TS SHP dimensions"),
            Self::InvalidFrameBounds => f.write_str("TS SHP frame is outside its canvas"),
            Self::InvalidDataOffset => f.write_str("TS SHP frame data is outside the file"),
            Self::InvalidRle => f.write_str("invalid TS SHP scanline RLE"),
            Self::ResourceLimit => f.write_str("TS SHP decoded size exceeds the safety limit"),
        }
    }
}

impl std::error::Error for ShpTsError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub radar_rgb: [u8; 3],
    /// Palette indices for the complete sprite canvas. Index zero is transparent.
    pub pixels: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShpTs {
    width: u16,
    height: u16,
    frames: Vec<Frame>,
}

#[derive(Clone, Copy, Debug)]
struct FrameHeader {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    flags: u32,
    radar_rgb: [u8; 3],
    data_offset: u32,
}

impl ShpTs {
    pub fn parse(bytes: &[u8]) -> Result<Self, ShpTsError> {
        if bytes.len() < FILE_HEADER_SIZE {
            return Err(ShpTsError::Truncated);
        }
        if read_u16(bytes, 0)? != 0 {
            return Err(ShpTsError::InvalidHeader);
        }
        let width = read_u16(bytes, 2)?;
        let height = read_u16(bytes, 4)?;
        let frame_count = usize::from(read_u16(bytes, 6)?);
        if (width == 0) != (height == 0) {
            return Err(ShpTsError::InvalidDimensions);
        }

        let canvas_size = usize::from(width)
            .checked_mul(usize::from(height))
            .ok_or(ShpTsError::ResourceLimit)?;
        let decoded_size = canvas_size
            .checked_mul(frame_count)
            .ok_or(ShpTsError::ResourceLimit)?;
        if canvas_size > MAX_DECODED_PIXELS || decoded_size > MAX_DECODED_PIXELS {
            return Err(ShpTsError::ResourceLimit);
        }
        let table_end = frame_count
            .checked_mul(FRAME_HEADER_SIZE)
            .and_then(|size| FILE_HEADER_SIZE.checked_add(size))
            .ok_or(ShpTsError::ResourceLimit)?;
        if table_end > bytes.len() {
            return Err(ShpTsError::Truncated);
        }

        let mut headers = Vec::with_capacity(frame_count);
        for index in 0..frame_count {
            let at = FILE_HEADER_SIZE + index * FRAME_HEADER_SIZE;
            headers.push(FrameHeader {
                x: read_u16(bytes, at)?,
                y: read_u16(bytes, at + 2)?,
                w: read_u16(bytes, at + 4)?,
                h: read_u16(bytes, at + 6)?,
                flags: read_u32(bytes, at + 8)?,
                radar_rgb: [bytes[at + 12], bytes[at + 13], bytes[at + 14]],
                data_offset: read_u32(bytes, at + 20)?,
            });
        }

        let mut frames = Vec::with_capacity(frame_count);
        for header in headers {
            let right = u32::from(header.x)
                .checked_add(u32::from(header.w))
                .ok_or(ShpTsError::InvalidFrameBounds)?;
            let bottom = u32::from(header.y)
                .checked_add(u32::from(header.h))
                .ok_or(ShpTsError::InvalidFrameBounds)?;
            if right > u32::from(width) || bottom > u32::from(height) {
                return Err(ShpTsError::InvalidFrameBounds);
            }

            let mut pixels = vec![0; canvas_size];
            if header.data_offset != 0 && header.w != 0 && header.h != 0 {
                let frame_pixels = if header.flags & 2 != 0 {
                    decode_rle(bytes, header.data_offset as usize, header.w, header.h)?
                } else {
                    decode_raw(bytes, header.data_offset as usize, header.w, header.h)?
                };
                blit(
                    &mut pixels,
                    usize::from(width),
                    &frame_pixels,
                    header,
                );
            }
            frames.push(Frame {
                x: header.x,
                y: header.y,
                w: header.w,
                h: header.h,
                radar_rgb: header.radar_rgb,
                pixels,
            });
        }

        Ok(Self {
            width,
            height,
            frames,
        })
    }

    pub fn canvas(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }
}

fn decode_raw(bytes: &[u8], offset: usize, w: u16, h: u16) -> Result<Vec<u8>, ShpTsError> {
    let size = usize::from(w)
        .checked_mul(usize::from(h))
        .ok_or(ShpTsError::ResourceLimit)?;
    let end = offset
        .checked_add(size)
        .ok_or(ShpTsError::InvalidDataOffset)?;
    Ok(bytes
        .get(offset..end)
        .ok_or(ShpTsError::InvalidDataOffset)?
        .to_vec())
}

fn decode_rle(bytes: &[u8], mut offset: usize, w: u16, h: u16) -> Result<Vec<u8>, ShpTsError> {
    let width = usize::from(w);
    let size = width
        .checked_mul(usize::from(h))
        .ok_or(ShpTsError::ResourceLimit)?;
    let mut output = Vec::with_capacity(size);
    for _ in 0..h {
        let row_bytes = usize::from(read_u16_at(bytes, offset, ShpTsError::InvalidDataOffset)?);
        if row_bytes < 2 {
            return Err(ShpTsError::InvalidRle);
        }
        let row_end = offset
            .checked_add(row_bytes)
            .ok_or(ShpTsError::InvalidDataOffset)?;
        if row_end > bytes.len() {
            return Err(ShpTsError::InvalidDataOffset);
        }
        let row_start = output.len();
        offset += 2;
        while offset < row_end {
            let value = bytes[offset];
            offset += 1;
            if value != 0 {
                if output.len() - row_start >= width {
                    return Err(ShpTsError::InvalidRle);
                }
                output.push(value);
                continue;
            }
            let count = *bytes.get(offset).ok_or(ShpTsError::InvalidRle)? as usize;
            offset += 1;
            let row_len = output.len() - row_start;
            let encoded_width = width.checked_add(1).ok_or(ShpTsError::InvalidRle)?;
            if row_len
                .checked_add(count)
                .filter(|&len| len <= encoded_width)
                .is_none()
            {
                return Err(ShpTsError::InvalidRle);
            }
            output.resize(output.len() + count, 0);
        }
        // Westwood's encoder commonly emits a trailing transparent sentinel
        // that makes a scanline one pixel wider than the frame header. It is
        // safe to discard only that transparent final pixel; any other
        // overrun remains malformed.
        if output.len() - row_start == width + 1 && output.last() == Some(&0) {
            output.pop();
        }
        if output.len() - row_start != width || offset != row_end {
            return Err(ShpTsError::InvalidRle);
        }
    }
    Ok(output)
}

fn blit(canvas: &mut [u8], canvas_width: usize, source: &[u8], header: FrameHeader) {
    let frame_width = usize::from(header.w);
    for row in 0..usize::from(header.h) {
        let source_start = row * frame_width;
        let destination_start = (usize::from(header.y) + row) * canvas_width
            + usize::from(header.x);
        canvas[destination_start..destination_start + frame_width]
            .copy_from_slice(&source[source_start..source_start + frame_width]);
    }
}

fn read_u16(bytes: &[u8], at: usize) -> Result<u16, ShpTsError> {
    read_u16_at(bytes, at, ShpTsError::Truncated)
}

fn read_u16_at(bytes: &[u8], at: usize, error: ShpTsError) -> Result<u16, ShpTsError> {
    let end = at.checked_add(2).ok_or_else(|| error.clone())?;
    let value = bytes.get(at..end).ok_or(error)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, ShpTsError> {
    let end = at.checked_add(4).ok_or(ShpTsError::Truncated)?;
    let value = bytes.get(at..end).ok_or(ShpTsError::Truncated)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_header(
        x: u16,
        y: u16,
        w: u16,
        h: u16,
        flags: u32,
        radar: [u8; 3],
        offset: u32,
    ) -> Vec<u8> {
        let mut header = Vec::new();
        header.extend_from_slice(&x.to_le_bytes());
        header.extend_from_slice(&y.to_le_bytes());
        header.extend_from_slice(&w.to_le_bytes());
        header.extend_from_slice(&h.to_le_bytes());
        header.extend_from_slice(&flags.to_le_bytes());
        header.extend_from_slice(&radar);
        header.push(0);
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(&offset.to_le_bytes());
        header
    }

    #[test]
    fn cnc_import_shp_ts_raw_rle_and_empty_frames() {
        let data_offset = FILE_HEADER_SIZE + 3 * FRAME_HEADER_SIZE;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes());
        bytes.extend_from_slice(&frame_header(1, 0, 2, 2, 0, [1, 2, 3], data_offset as u32));
        bytes.extend_from_slice(&frame_header(
            0,
            2,
            4,
            1,
            2,
            [4, 5, 6],
            (data_offset + 4) as u32,
        ));
        bytes.extend_from_slice(&frame_header(0, 0, 0, 0, 0, [7, 8, 9], 0));
        bytes.extend_from_slice(&[10, 11, 12, 13]);
        bytes.extend_from_slice(&6u16.to_le_bytes());
        bytes.extend_from_slice(&[20, 0, 2, 21]);

        let shp = ShpTs::parse(&bytes).unwrap();
        assert_eq!(shp.canvas(), (4, 3));
        assert_eq!(shp.frames().len(), 3);
        assert_eq!(
            shp.frames()[0].pixels,
            [0, 10, 11, 0, 0, 12, 13, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            shp.frames()[1].pixels,
            [0, 0, 0, 0, 0, 0, 0, 0, 20, 0, 0, 21]
        );
        assert_eq!(shp.frames()[1].radar_rgb, [4, 5, 6]);
        assert_eq!(shp.frames()[2].pixels, [0; 12]);
    }

    #[test]
    fn cnc_import_shp_ts_rejects_hostile_lengths() {
        assert!(matches!(ShpTs::parse(&[]), Err(ShpTsError::Truncated)));
        let bytes = [0, 0, 0xff, 0xff, 0xff, 0xff, 1, 0];
        assert!(matches!(
            ShpTs::parse(&bytes),
            Err(ShpTsError::ResourceLimit)
        ));
    }
}
