use std::fmt;

const FILE_HEADER_SIZE: usize = 16;
const TILE_HEADER_SIZE: usize = 52;
const MAX_DECODED_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TmpTsError {
    Truncated,
    InvalidDimensions,
    InvalidOffset,
    InvalidTile,
    ResourceLimit,
}

impl fmt::Display for TmpTsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated TS TMP template"),
            Self::InvalidDimensions => f.write_str("invalid TS TMP dimensions"),
            Self::InvalidOffset => f.write_str("TS TMP data offset is outside the file"),
            Self::InvalidTile => f.write_str("invalid TS TMP tile"),
            Self::ResourceLimit => f.write_str("TS TMP decoded size exceeds the safety limit"),
        }
    }
}

impl std::error::Error for TmpTsError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtraImage {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub pixels: Vec<u8>,
    /// Some files provide an explicit offset for an extra-image height map.
    pub z_pixels: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsoTile {
    pub x: i32,
    pub y: i32,
    pub height: u8,
    pub terrain_type: u8,
    pub ramp_type: u8,
    pub low_rgb: [u8; 3],
    pub high_rgb: [u8; 3],
    pub has_z: bool,
    pub has_damaged: bool,
    /// Palette indices in a full `tile_width * tile_height` canvas.
    pub pixels: Vec<u8>,
    /// Height values in the same full-canvas layout as `pixels`.
    pub z_pixels: Option<Vec<u8>>,
    pub extra: Option<ExtraImage>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TmpTs {
    block_width: i32,
    block_height: i32,
    tile_width: i32,
    tile_height: i32,
    tiles: Vec<Option<IsoTile>>,
}

impl TmpTs {
    pub fn parse(bytes: &[u8]) -> Result<Self, TmpTsError> {
        if bytes.len() < FILE_HEADER_SIZE {
            return Err(TmpTsError::Truncated);
        }
        let block_width = read_i32(bytes, 0)?;
        let block_height = read_i32(bytes, 4)?;
        let tile_width = read_i32(bytes, 8)?;
        let tile_height = read_i32(bytes, 12)?;
        validate_dimensions(block_width, block_height, tile_width, tile_height)?;

        let block_count = positive_usize(block_width)?
            .checked_mul(positive_usize(block_height)?)
            .ok_or(TmpTsError::ResourceLimit)?;
        let table_size = block_count
            .checked_mul(4)
            .ok_or(TmpTsError::ResourceLimit)?;
        let table_end = FILE_HEADER_SIZE
            .checked_add(table_size)
            .ok_or(TmpTsError::ResourceLimit)?;
        if table_end > bytes.len() {
            return Err(TmpTsError::Truncated);
        }

        let canvas_size = positive_usize(tile_width)?
            .checked_mul(positive_usize(tile_height)?)
            .ok_or(TmpTsError::ResourceLimit)?;
        let mut decoded_bytes = 0usize;
        let mut tiles = Vec::with_capacity(block_count);
        for index in 0..block_count {
            let offset = read_i32(bytes, FILE_HEADER_SIZE + index * 4)?;
            if offset == 0 {
                tiles.push(None);
                continue;
            }
            let offset = nonnegative_usize(offset).map_err(|_| TmpTsError::InvalidOffset)?;
            let remaining = MAX_DECODED_BYTES
                .checked_sub(decoded_bytes)
                .ok_or(TmpTsError::ResourceLimit)?;
            let tile = parse_tile(bytes, offset, tile_width, tile_height, remaining)?;
            decoded_bytes = decoded_bytes
                .checked_add(canvas_size)
                .and_then(|size| size.checked_add(tile.z_pixels.as_ref().map_or(0, Vec::len)))
                .and_then(|size| {
                    tile.extra.as_ref().map_or(Some(size), |extra| {
                        size.checked_add(extra.pixels.len())?.checked_add(
                            extra.z_pixels.as_ref().map_or(0, Vec::len),
                        )
                    })
                })
                .ok_or(TmpTsError::ResourceLimit)?;
            if decoded_bytes > MAX_DECODED_BYTES {
                return Err(TmpTsError::ResourceLimit);
            }
            tiles.push(Some(tile));
        }

        Ok(Self {
            block_width,
            block_height,
            tile_width,
            tile_height,
            tiles,
        })
    }

    pub fn blocks(&self) -> (i32, i32) {
        (self.block_width, self.block_height)
    }

    pub fn tile_size(&self) -> (i32, i32) {
        (self.tile_width, self.tile_height)
    }

    pub fn tile(&self, bx: i32, by: i32) -> Option<&IsoTile> {
        if bx < 0 || by < 0 || bx >= self.block_width || by >= self.block_height {
            return None;
        }
        let index = by.checked_mul(self.block_width)?.checked_add(bx)?;
        self.tiles.get(usize::try_from(index).ok()?)?.as_ref()
    }
}

fn validate_dimensions(
    block_width: i32,
    block_height: i32,
    tile_width: i32,
    tile_height: i32,
) -> Result<(), TmpTsError> {
    if block_width <= 0
        || block_height <= 0
        || tile_width <= 0
        || tile_height <= 0
        || tile_width % 4 != 0
        || tile_height % 2 != 0
        || tile_height.checked_mul(2) != Some(tile_width)
    {
        return Err(TmpTsError::InvalidDimensions);
    }
    let block_count = positive_usize(block_width)?
        .checked_mul(positive_usize(block_height)?)
        .ok_or(TmpTsError::ResourceLimit)?;
    let canvas_size = positive_usize(tile_width)?
        .checked_mul(positive_usize(tile_height)?)
        .ok_or(TmpTsError::ResourceLimit)?;
    let slot_bytes = block_count
        .checked_mul(std::mem::size_of::<Option<IsoTile>>())
        .ok_or(TmpTsError::ResourceLimit)?;
    if slot_bytes > MAX_DECODED_BYTES || canvas_size > MAX_DECODED_BYTES {
        return Err(TmpTsError::ResourceLimit);
    }
    Ok(())
}

fn parse_tile(
    bytes: &[u8],
    offset: usize,
    tile_width: i32,
    tile_height: i32,
    remaining: usize,
) -> Result<IsoTile, TmpTsError> {
    let header_end = offset
        .checked_add(TILE_HEADER_SIZE)
        .ok_or(TmpTsError::InvalidOffset)?;
    let header = bytes
        .get(offset..header_end)
        .ok_or(TmpTsError::InvalidOffset)?;
    let x = read_i32(header, 0)?;
    let y = read_i32(header, 4)?;
    let extra_offset = read_i32(header, 8)?;
    let z_offset = read_i32(header, 12)?;
    let extra_z_offset = read_i32(header, 16)?;
    let extra_x = read_i32(header, 20)?;
    let extra_y = read_i32(header, 24)?;
    let extra_w = read_i32(header, 28)?;
    let extra_h = read_i32(header, 32)?;
    let flags = read_u32(header, 36)?;
    let has_extra = flags & 1 != 0;
    let has_z = flags & 2 != 0;
    let has_damaged = flags & 4 != 0;

    if has_extra && (extra_w < 0 || extra_h < 0) {
        return Err(TmpTsError::InvalidTile);
    }
    let extra_size = if has_extra {
        nonnegative_usize(extra_w)?
            .checked_mul(nonnegative_usize(extra_h)?)
            .ok_or(TmpTsError::ResourceLimit)?
    } else {
        0
    };
    let diamond_size = positive_usize(tile_width)?
        .checked_mul(positive_usize(tile_height)?)
        .and_then(|size| size.checked_div(2))
        .ok_or(TmpTsError::ResourceLimit)?;
    let canvas_size = positive_usize(tile_width)?
        .checked_mul(positive_usize(tile_height)?)
        .ok_or(TmpTsError::ResourceLimit)?;
    let decoded_size = canvas_size
        .checked_add(if has_z { canvas_size } else { 0 })
        .and_then(|size| size.checked_add(extra_size))
        .and_then(|size| {
            size.checked_add(if has_z && has_extra && extra_z_offset != 0 {
                extra_size
            } else {
                0
            })
        })
        .ok_or(TmpTsError::ResourceLimit)?;
    if decoded_size > remaining {
        return Err(TmpTsError::ResourceLimit);
    }
    let (pixels, main_end) = decode_diamond(
        bytes,
        header_end,
        tile_width,
        tile_height,
        diamond_size,
    )?;

    let (z_pixels, sequential_after_z) = if has_z {
        let z_start = data_offset(z_offset, main_end)?;
        let (pixels, end) = decode_diamond(
            bytes,
            z_start,
            tile_width,
            tile_height,
            diamond_size,
        )?;
        (Some(pixels), end)
    } else {
        (None, main_end)
    };

    let extra = if has_extra {
        let start = data_offset(extra_offset, sequential_after_z)?;
        let end = start
            .checked_add(extra_size)
            .ok_or(TmpTsError::InvalidOffset)?;
        let pixels = bytes
            .get(start..end)
            .ok_or(TmpTsError::InvalidOffset)?
            .to_vec();
        let z_pixels = if has_z && extra_z_offset != 0 {
            let z_start = data_offset(extra_z_offset, end)?;
            let z_end = z_start
                .checked_add(extra_size)
                .ok_or(TmpTsError::InvalidOffset)?;
            Some(
                bytes
                    .get(z_start..z_end)
                    .ok_or(TmpTsError::InvalidOffset)?
                    .to_vec(),
            )
        } else {
            None
        };
        Some(ExtraImage {
            x: extra_x,
            y: extra_y,
            w: extra_w,
            h: extra_h,
            pixels,
            z_pixels,
        })
    } else {
        None
    };

    Ok(IsoTile {
        x,
        y,
        height: header[40],
        terrain_type: header[41],
        ramp_type: header[42],
        low_rgb: [header[43], header[44], header[45]],
        high_rgb: [header[46], header[47], header[48]],
        has_z,
        has_damaged,
        pixels,
        z_pixels,
        extra,
    })
}

fn decode_diamond(
    bytes: &[u8],
    offset: usize,
    tile_width: i32,
    tile_height: i32,
    diamond_size: usize,
) -> Result<(Vec<u8>, usize), TmpTsError> {
    let end = offset
        .checked_add(diamond_size)
        .ok_or(TmpTsError::InvalidOffset)?;
    let source = bytes
        .get(offset..end)
        .ok_or(TmpTsError::InvalidOffset)?;
    let width = positive_usize(tile_width)?;
    let height = positive_usize(tile_height)?;
    let mut canvas = vec![0; width.checked_mul(height).ok_or(TmpTsError::ResourceLimit)?];
    let mut consumed = 0usize;
    for row in 0..height {
        // TS stores 4..tile_width, then tile_width-4..4. The nominal final
        // canvas scanline is empty, which makes the stored total exactly
        // tile_width * tile_height / 2 bytes.
        let units = if row < height / 2 {
            row + 1
        } else {
            height - row - 1
        };
        let row_width = units.checked_mul(4).ok_or(TmpTsError::ResourceLimit)?.min(width);
        let source_end = consumed
            .checked_add(row_width)
            .ok_or(TmpTsError::ResourceLimit)?;
        let source_row = source
            .get(consumed..source_end)
            .ok_or(TmpTsError::InvalidTile)?;
        let x = (width - row_width) / 2;
        let destination = row
            .checked_mul(width)
            .and_then(|start| start.checked_add(x))
            .ok_or(TmpTsError::ResourceLimit)?;
        canvas[destination..destination + row_width].copy_from_slice(source_row);
        consumed = source_end;
    }
    if consumed != diamond_size {
        return Err(TmpTsError::InvalidDimensions);
    }
    Ok((canvas, end))
}

fn data_offset(value: i32, sequential: usize) -> Result<usize, TmpTsError> {
    if value == 0 {
        Ok(sequential)
    } else {
        nonnegative_usize(value).map_err(|_| TmpTsError::InvalidOffset)
    }
}

fn positive_usize(value: i32) -> Result<usize, TmpTsError> {
    if value <= 0 {
        return Err(TmpTsError::InvalidDimensions);
    }
    usize::try_from(value).map_err(|_| TmpTsError::ResourceLimit)
}

fn nonnegative_usize(value: i32) -> Result<usize, TmpTsError> {
    usize::try_from(value).map_err(|_| TmpTsError::InvalidOffset)
}

fn read_i32(bytes: &[u8], at: usize) -> Result<i32, TmpTsError> {
    let value = bytes
        .get(at..at.checked_add(4).ok_or(TmpTsError::Truncated)?)
        .ok_or(TmpTsError::Truncated)?;
    Ok(i32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, TmpTsError> {
    let value = bytes
        .get(at..at.checked_add(4).ok_or(TmpTsError::Truncated)?)
        .ok_or(TmpTsError::Truncated)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_tmp_ts_one_tile() {
        let tile_offset = FILE_HEADER_SIZE + 4;
        let mut bytes = Vec::new();
        for value in [1i32, 1, 8, 4, tile_offset as i32] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [3i32, -2, 0, 0, 0, 0, 0, 0, 0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&[7, 9, 11, 1, 2, 3, 4, 5, 6, 0, 0, 0]);
        bytes.extend(1u8..=16);

        let template = TmpTs::parse(&bytes).unwrap();
        assert_eq!(template.blocks(), (1, 1));
        assert_eq!(template.tile_size(), (8, 4));
        let tile = template.tile(0, 0).unwrap();
        assert_eq!((tile.x, tile.y), (3, -2));
        assert_eq!((tile.height, tile.terrain_type, tile.ramp_type), (7, 9, 11));
        assert_eq!(tile.low_rgb, [1, 2, 3]);
        assert_eq!(tile.high_rgb, [4, 5, 6]);
        assert_eq!(
            tile.pixels,
            [
                0, 0, 1, 2, 3, 4, 0, 0, 5, 6, 7, 8, 9, 10, 11, 12, 0, 0, 13, 14,
                15, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ]
        );
        assert_eq!(template.tile(1, 0), None);
    }

    #[test]
    fn cnc_import_tmp_ts_rejects_hostile_headers() {
        assert_eq!(TmpTs::parse(&[]), Err(TmpTsError::Truncated));
        let mut bytes = Vec::new();
        for value in [i32::MAX, i32::MAX, 48, 24] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(TmpTs::parse(&bytes), Err(TmpTsError::ResourceLimit));
    }
}
