use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TmpError {
    Truncated,
    InvalidDimensions,
    InvalidOffset,
    InvalidImage,
}

impl fmt::Display for TmpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated TMP template"),
            Self::InvalidDimensions => f.write_str("invalid TMP icon dimensions"),
            Self::InvalidOffset => f.write_str("TMP index is outside the file"),
            Self::InvalidImage => f.write_str("TMP icon references an image outside the file"),
        }
    }
}

impl std::error::Error for TmpError {}

#[derive(Clone, Debug)]
pub struct Tmp {
    icon_w: u16,
    icon_h: u16,
    icons: Vec<Option<Vec<u8>>>,
    index2: Vec<u8>,
    blocks: Option<(u16, u16)>,
}

#[derive(Clone, Debug)]
pub struct TmpRa {
    icon_w: u16,
    icon_h: u16,
    icons: Vec<Option<Vec<u8>>>,
    index2: Vec<u8>,
    blocks_x: u16,
    blocks_y: u16,
    id: u32,
    unknown: u32,
}

impl Tmp {
    /// Parses either the 32-byte TD header or the 40-byte RA header.
    pub fn parse(bytes: &[u8]) -> Result<Self, TmpError> {
        if looks_like_ra(bytes) {
            let ra = TmpRa::parse(bytes)?;
            return Ok(Self {
                icon_w: ra.icon_w,
                icon_h: ra.icon_h,
                icons: ra.icons,
                index2: ra.index2,
                blocks: Some((ra.blocks_x, ra.blocks_y)),
            });
        }
        parse_td(bytes)
    }

    pub fn icon_size(&self) -> (u16, u16) {
        (self.icon_w, self.icon_h)
    }

    pub fn icon_count(&self) -> usize {
        self.icons.len()
    }

    pub fn icon(&self, slot: usize) -> Option<&[u8]> {
        self.icons.get(slot)?.as_deref()
    }

    pub fn index2(&self) -> &[u8] {
        &self.index2
    }

    pub fn blocks(&self) -> Option<(u16, u16)> {
        self.blocks
    }
}

impl TmpRa {
    pub fn parse(bytes: &[u8]) -> Result<Self, TmpError> {
        if bytes.len() < 40 {
            return Err(TmpError::Truncated);
        }
        let icon_w = read_u16(bytes, 0)?;
        let icon_h = read_u16(bytes, 2)?;
        let icon_count = read_u16(bytes, 4)? as usize;
        let blocks_x = read_u16(bytes, 8)?;
        let blocks_y = read_u16(bytes, 10)?;
        let declared_size = read_u32(bytes, 12)? as usize;
        let image_offset = read_u32(bytes, 16)? as usize;
        let id = read_u32(bytes, 24)?;
        let index2_offset = read_u32(bytes, 28)? as usize;
        let unknown = read_u32(bytes, 32)?;
        let index1_offset = read_u32(bytes, 36)? as usize;
        validate_dimensions(icon_w, icon_h, icon_count)?;
        if declared_size < 40 || declared_size > bytes.len() {
            return Err(TmpError::Truncated);
        }
        if blocks_x == 0 || blocks_y == 0 {
            return Err(TmpError::InvalidDimensions);
        }
        // Some RA archives align the MIX entry past the header's logical
        // file_size; index2 can occupy those final alignment bytes.
        let index1 = take_index(bytes, index1_offset, icon_count)?;
        let index2_end = usize::try_from(unknown)
            .ok()
            .filter(|&offset| offset >= index2_offset && offset <= bytes.len())
            .unwrap_or(declared_size.min(bytes.len()));
        let index2 = bytes
            .get(index2_offset..index2_end)
            .ok_or(TmpError::InvalidOffset)?
            .to_vec();
        let image_limit = index1_offset.min(index2_offset);
        let icons = decode_icons(
            bytes,
            index1,
            image_offset,
            image_limit,
            icon_w,
            icon_h,
        )?;
        Ok(Self {
            icon_w,
            icon_h,
            icons,
            index2,
            blocks_x,
            blocks_y,
            id,
            unknown,
        })
    }

    pub fn icon_size(&self) -> (u16, u16) {
        (self.icon_w, self.icon_h)
    }

    pub fn icon_count(&self) -> usize {
        self.icons.len()
    }

    pub fn icon(&self, slot: usize) -> Option<&[u8]> {
        self.icons.get(slot)?.as_deref()
    }

    pub fn index2(&self) -> &[u8] {
        &self.index2
    }

    pub fn blocks(&self) -> (u16, u16) {
        (self.blocks_x, self.blocks_y)
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn unknown(&self) -> u32 {
        self.unknown
    }
}

fn parse_td(bytes: &[u8]) -> Result<Tmp, TmpError> {
    // The TD archives confirm a 32-byte header: image data begins at 32,
    // and index1_offset is the u32 at byte 28.
    if bytes.len() < 32 {
        return Err(TmpError::Truncated);
    }
    let icon_w = read_u16(bytes, 0)?;
    let icon_h = read_u16(bytes, 2)?;
    let icon_count = read_u16(bytes, 4)? as usize;
    let declared_size = read_u32(bytes, 8)? as usize;
    let image_offset = read_u32(bytes, 12)? as usize;
    let index2_offset = read_u32(bytes, 24)? as usize;
    let index1_offset = read_u32(bytes, 28)? as usize;
    if icon_w != 24 || icon_h != 24 || icon_count == 0 {
        return Err(TmpError::InvalidDimensions);
    }
    if declared_size > bytes.len() || declared_size < 32 {
        return Err(TmpError::Truncated);
    }
    let bytes = &bytes[..declared_size];
    if image_offset < 32 || index1_offset > index2_offset {
        return Err(TmpError::InvalidOffset);
    }
    let index1 = take_index(bytes, index1_offset, icon_count)?;
    // TD index2 is indexed by stored image number, so sparse templates can
    // have fewer bytes than icon_count.
    let index2 = bytes
        .get(index2_offset..)
        .ok_or(TmpError::InvalidOffset)?
        .to_vec();
    let icons = decode_icons(
        bytes,
        index1,
        image_offset,
        index1_offset,
        icon_w,
        icon_h,
    )?;
    Ok(Tmp {
        icon_w,
        icon_h,
        icons,
        index2,
        blocks: None,
    })
}

fn looks_like_ra(bytes: &[u8]) -> bool {
    if bytes.len() < 40 {
        return false;
    }
    let Some(declared_size) = read_u32_option(bytes, 12).map(|value| value as usize) else {
        return false;
    };
    let Some(image_offset) = read_u32_option(bytes, 16).map(|value| value as usize) else {
        return false;
    };
    let Some(index2_offset) = read_u32_option(bytes, 28).map(|value| value as usize) else {
        return false;
    };
    let Some(index1_offset) = read_u32_option(bytes, 36).map(|value| value as usize) else {
        return false;
    };
    declared_size >= 40
        && declared_size <= bytes.len()
        && image_offset >= 40
        && image_offset <= index1_offset.min(index2_offset)
        && index1_offset < declared_size
        && index2_offset < declared_size
}

fn validate_dimensions(icon_w: u16, icon_h: u16, icon_count: usize) -> Result<(), TmpError> {
    if icon_w == 0 || icon_h == 0 || icon_count == 0 {
        return Err(TmpError::InvalidDimensions);
    }
    usize::from(icon_w)
        .checked_mul(usize::from(icon_h))
        .ok_or(TmpError::InvalidDimensions)?;
    Ok(())
}

fn take_index(bytes: &[u8], offset: usize, count: usize) -> Result<&[u8], TmpError> {
    let end = offset.checked_add(count).ok_or(TmpError::InvalidOffset)?;
    bytes.get(offset..end).ok_or(TmpError::InvalidOffset)
}

fn decode_icons(
    bytes: &[u8],
    index1: &[u8],
    image_offset: usize,
    image_limit: usize,
    icon_w: u16,
    icon_h: u16,
) -> Result<Vec<Option<Vec<u8>>>, TmpError> {
    if image_offset > image_limit {
        return Err(TmpError::InvalidOffset);
    }
    let image_size = usize::from(icon_w)
        .checked_mul(usize::from(icon_h))
        .ok_or(TmpError::InvalidDimensions)?;
    let mut icons = Vec::with_capacity(index1.len());
    for &image_number in index1 {
        if image_number == 0xff {
            icons.push(None);
            continue;
        }
        let start = (image_number as usize)
            .checked_mul(image_size)
            .and_then(|offset| image_offset.checked_add(offset))
            .ok_or(TmpError::InvalidImage)?;
        let end = start.checked_add(image_size).ok_or(TmpError::InvalidImage)?;
        if end > image_limit {
            return Err(TmpError::InvalidImage);
        }
        icons.push(Some(
            bytes.get(start..end).ok_or(TmpError::InvalidImage)?.to_vec(),
        ));
    }
    Ok(icons)
}

fn read_u16(bytes: &[u8], at: usize) -> Result<u16, TmpError> {
    let value = bytes.get(at..at + 2).ok_or(TmpError::Truncated)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, TmpError> {
    read_u32_option(bytes, at).ok_or(TmpError::Truncated)
}

fn read_u32_option(bytes: &[u8], at: usize) -> Option<u32> {
    let value = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(value.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_tmp_ra_header_and_empty_slot() {
        let mut bytes = vec![0; 40 + 24 * 24 + 4];
        bytes[0..2].copy_from_slice(&24u16.to_le_bytes());
        bytes[2..4].copy_from_slice(&24u16.to_le_bytes());
        bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
        bytes[8..10].copy_from_slice(&2u16.to_le_bytes());
        bytes[10..12].copy_from_slice(&1u16.to_le_bytes());
        let length = bytes.len() as u32;
        bytes[12..16].copy_from_slice(&length.to_le_bytes());
        bytes[16..20].copy_from_slice(&40u32.to_le_bytes());
        let index1 = (40 + 24 * 24) as u32;
        let index2 = index1 + 2;
        bytes[28..32].copy_from_slice(&index2.to_le_bytes());
        bytes[32..36].copy_from_slice(&(index2 + 2).to_le_bytes());
        bytes[36..40].copy_from_slice(&index1.to_le_bytes());
        bytes[index1 as usize..index1 as usize + 2].copy_from_slice(&[0, 0xff]);
        bytes[index2 as usize..].copy_from_slice(&[3, 4]);
        let tmp = Tmp::parse(&bytes).unwrap();
        assert_eq!(tmp.blocks(), Some((2, 1)));
        assert_eq!(tmp.index2(), [3, 4]);
        assert_eq!(tmp.icon(0).unwrap().len(), 24 * 24);
        assert_eq!(tmp.icon(1), None);
    }
}
