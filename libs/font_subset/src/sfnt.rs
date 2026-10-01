//! The sfnt container: reading the table directory and writing a font back
//! with its directory, padding and checksums.

use crate::SubsetError;

pub(crate) type Tag = [u8; 4];

/// Big-endian reads that fail as a malformed font instead of panicking.
pub(crate) fn u16_at(data: &[u8], at: usize) -> Result<u16, SubsetError> {
    data.get(at..at + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
        .ok_or(SubsetError::Malformed("read past the end of a table"))
}

pub(crate) fn u32_at(data: &[u8], at: usize) -> Result<u32, SubsetError> {
    data.get(at..at + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(SubsetError::Malformed("read past the end of a table"))
}

pub(crate) fn put_u16(data: &mut [u8], at: usize, value: u16) {
    data[at..at + 2].copy_from_slice(&value.to_be_bytes());
}

pub(crate) fn put_u32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_be_bytes());
}

/// A font's tables in directory order, borrowed from the font.
pub(crate) struct Tables<'a> {
    pub version: u32,
    pub tables: Vec<(Tag, &'a [u8])>,
}

impl<'a> Tables<'a> {
    pub fn read(font: &'a [u8]) -> Result<Self, SubsetError> {
        let version = u32_at(font, 0)?;
        match &version.to_be_bytes() {
            b"ttcf" => return Err(SubsetError::Unsupported("font collections")),
            b"OTTO" => return Err(SubsetError::Unsupported("CFF outlines")),
            b"true" | [0, 1, 0, 0] => {}
            _ => return Err(SubsetError::Malformed("not an sfnt font")),
        }
        let count = u16_at(font, 4)? as usize;
        let mut tables = Vec::with_capacity(count);
        for i in 0..count {
            let record = 12 + 16 * i;
            let tag = font
                .get(record..record + 4)
                .ok_or(SubsetError::Malformed("table directory truncated"))?;
            let offset = u32_at(font, record + 8)? as usize;
            let length = u32_at(font, record + 12)? as usize;
            let data = font
                .get(offset..offset.saturating_add(length))
                .ok_or(SubsetError::Malformed("table outside the font"))?;
            tables.push(([tag[0], tag[1], tag[2], tag[3]], data));
        }
        Ok(Self { version, tables })
    }

    pub fn get(&self, tag: &Tag) -> Option<&'a [u8]> {
        self.tables.iter().find(|(t, _)| t == tag).map(|(_, data)| *data)
    }
}

fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in data.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    sum
}

/// Writes a font from its tables: the directory sorted by tag, every table
/// 4-byte aligned, the table checksums and `head.checkSumAdjustment`.
pub(crate) fn write(version: u32, mut tables: Vec<(Tag, Vec<u8>)>) -> Vec<u8> {
    tables.sort_by(|a, b| a.0.cmp(&b.0));
    for (tag, data) in &mut tables {
        if tag == b"head" && data.len() >= 12 {
            put_u32(data, 8, 0);
        }
    }
    let count = tables.len() as u16;
    let mut entry_selector = 0u16;
    while (2u32 << entry_selector) <= count as u32 {
        entry_selector += 1;
    }
    let search_range = 16u16 << entry_selector;
    let range_shift = count * 16 - search_range;

    let mut font = Vec::new();
    font.extend_from_slice(&version.to_be_bytes());
    font.extend_from_slice(&count.to_be_bytes());
    font.extend_from_slice(&search_range.to_be_bytes());
    font.extend_from_slice(&entry_selector.to_be_bytes());
    font.extend_from_slice(&range_shift.to_be_bytes());
    let mut offset = 12 + 16 * tables.len();
    for (tag, data) in &tables {
        font.extend_from_slice(tag);
        font.extend_from_slice(&checksum(data).to_be_bytes());
        font.extend_from_slice(&(offset as u32).to_be_bytes());
        font.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += (data.len() + 3) & !3;
    }
    let mut head_at = None;
    for (tag, data) in &tables {
        if tag == b"head" {
            head_at = Some(font.len());
        }
        font.extend_from_slice(data);
        font.resize((font.len() + 3) & !3, 0);
    }
    if let Some(head_at) = head_at {
        let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&font));
        put_u32(&mut font, head_at + 8, adjustment);
    }
    font
}
