//! Names of Windows host frames from PE images and their PDBs, read after capture. PE section
//! headers map CodeView segments to image RVAs; the debug directory names the PDB through RSDS.
//! MSF 7.0 stores the PDB's streams in blocks: DBI selects the stream of CodeView symbol records.

/// Reads a bounded byte range without overflowing its end offset.
fn bytes(data: &[u8], offset: usize, len: usize) -> Option<&[u8]> {
    data.get(offset..offset.checked_add(len)?)
}

/// Reads a little-endian 16-bit field.
fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes(data, offset, 2)?.try_into().ok()?))
}

/// Reads a little-endian 32-bit field.
fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes(data, offset, 4)?.try_into().ok()?))
}

/// Reads a 32-bit size or offset representable on this host.
fn usize_at(data: &[u8], offset: usize) -> Option<usize> {
    usize::try_from(u32_at(data, offset)?).ok()
}

/// Reads a UTF-8 name terminated within the supplied record.
fn cstr(data: &[u8], offset: usize) -> Option<&str> {
    let tail = data.get(offset..)?;
    let len = tail.iter().position(|&b| b == 0)?;
    std::str::from_utf8(tail.get(..len)?).ok()
}

/// The optional header and complete section table of a PE file.
fn pe_headers(image: &[u8]) -> Option<(&[u8], &[u8])> {
    let pe = usize_at(image, 0x3c)?;
    let header = bytes(image, pe, 24)?;
    if !header.starts_with(b"PE\0\0") {
        return None;
    }
    let count = usize::from(u16_at(header, 6)?);
    let optional_size = usize::from(u16_at(header, 20)?);
    let optional_offset = pe.checked_add(24)?;
    let optional = bytes(image, optional_offset, optional_size)?;
    let sections = bytes(image, optional_offset.checked_add(optional_size)?, count.checked_mul(40)?)?;
    Some((optional, sections))
}

/// Section (VirtualAddress, VirtualSize) pairs in PE header order.
pub(crate) fn pe_sections(image: &[u8]) -> Vec<(u32, u32)> {
    let Some((_, sections)) = pe_headers(image) else { return Vec::new() };
    sections.chunks_exact(40).filter_map(|s| Some((u32_at(s, 12)?, u32_at(s, 8)?))).collect()
}

/// Maps an RVA range to bytes backed by a section's raw file data.
fn pe_rva<'a>(image: &'a [u8], sections: &[u8], rva: u32, size: usize) -> Option<&'a [u8]> {
    for section in sections.chunks_exact(40) {
        let address = u32_at(section, 12)?;
        let virtual_size = u32_at(section, 8)?;
        let raw_size = u32_at(section, 16)?;
        let Some(delta) = rva.checked_sub(address) else { continue };
        if delta >= virtual_size.max(raw_size) {
            continue;
        }
        let delta = usize::try_from(delta).ok()?;
        if delta.checked_add(size)? > usize::try_from(raw_size).ok()? {
            continue;
        }
        let offset = usize_at(section, 20)?.checked_add(delta)?;
        return bytes(image, offset, size);
    }
    None
}

/// The UTF-8 PDB path in a PE32+ image's RSDS debug record.
pub(crate) fn pdb_path(image: &[u8]) -> Option<String> {
    let (optional, sections) = pe_headers(image)?;
    if u16_at(optional, 0)? != 0x20b || u32_at(optional, 108)? <= 6 {
        return None;
    }
    let rva = u32_at(optional, 112 + 6 * 8)?;
    let size = usize_at(optional, 112 + 6 * 8 + 4)?;
    if rva == 0 || size == 0 {
        return None;
    }
    let directory = pe_rva(image, sections, rva, size)?;
    for entry in directory.chunks_exact(28) {
        if u32_at(entry, 12)? != 2 {
            continue;
        }
        let size = usize_at(entry, 16)?;
        let offset = usize_at(entry, 24)?;
        let Some(record) = bytes(image, offset, size) else { continue };
        if record.starts_with(b"RSDS") {
            if let Some(path) = cstr(record, 24) {
                return Some(path.to_owned());
            }
        }
    }
    None
}

const MSF_MAGIC: &[u8; 32] = b"Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0";

/// Validates the MSF superblock and returns its block size and count.
fn msf_layout(pdb: &[u8]) -> Option<(usize, usize)> {
    if !pdb.starts_with(MSF_MAGIC) {
        return None;
    }
    let block_size = usize_at(pdb, 32)?;
    if !matches!(block_size, 512 | 1024 | 2048 | 4096) {
        return None;
    }
    let count = usize_at(pdb, 40)?;
    if count == 0 || count.checked_mul(block_size)? > pdb.len() {
        return None;
    }
    Some((block_size, count))
}

/// The number of blocks for a byte size; block_size is validated by msf_layout.
fn blocks_for(size: usize, block_size: usize) -> usize {
    size / block_size + usize::from(size % block_size != 0)
}

/// Reads one complete block within the MSF's declared block count.
fn block(pdb: &[u8], block_size: usize, count: usize, index: usize) -> Option<&[u8]> {
    if index >= count {
        return None;
    }
    bytes(pdb, index.checked_mul(block_size)?, block_size)
}

/// Concatenates a list of block indices, cutting the result to its declared size.
fn read_blocks(pdb: &[u8], block_size: usize, count: usize, indices: &[u8], size: usize) -> Option<Vec<u8>> {
    let needed = blocks_for(size, block_size);
    if needed > count || size > pdb.len() || indices.len() != needed.checked_mul(4)? {
        return None;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(size).ok()?;
    for index in indices.chunks_exact(4) {
        let data = block(pdb, block_size, count, usize_at(index, 0)?)?;
        let len = size.checked_sub(out.len())?.min(block_size);
        out.extend_from_slice(data.get(..len)?);
    }
    Some(out)
}

/// Reads the stream directory; its block indices must fit in one block-map block.
fn directory(pdb: &[u8]) -> Option<Vec<u8>> {
    let (block_size, count) = msf_layout(pdb)?;
    let size = usize_at(pdb, 44)?;
    let map = block(pdb, block_size, count, usize_at(pdb, 52)?)?;
    let indices = bytes(map, 0, blocks_for(size, block_size).checked_mul(4)?)?;
    read_blocks(pdb, block_size, count, indices, size)
}

/// Reads stream k using the sizes and block lists in the MSF directory.
fn stream(pdb: &[u8], dir: &[u8], k: usize) -> Option<Vec<u8>> {
    let (block_size, count) = msf_layout(pdb)?;
    let streams = usize_at(dir, 0)?;
    if k >= streams {
        return None;
    }
    let sizes = bytes(dir, 4, streams.checked_mul(4)?)?;
    let mut offset = 4usize.checked_add(sizes.len())?;
    for (index, entry) in sizes.chunks_exact(4).enumerate() {
        let raw_size = u32_at(entry, 0)?;
        let size = if raw_size == u32::MAX { 0 } else { usize::try_from(raw_size).ok()? };
        let len = blocks_for(size, block_size).checked_mul(4)?;
        let indices = bytes(dir, offset, len)?;
        if index == k {
            return read_blocks(pdb, block_size, count, indices, size);
        }
        offset = offset.checked_add(len)?;
    }
    None
}

/// Decodes a public code or function symbol, preserving its raw name.
fn public_symbol(data: &[u8], sections: &[(u32, u32)]) -> Option<(u32, String)> {
    if u32_at(data, 0)? & 3 == 0 {
        return None;
    }
    let offset = u32_at(data, 4)?;
    let segment = usize::from(u16_at(data, 8)?).checked_sub(1)?;
    let rva = sections.get(segment)?.0.checked_add(offset)?;
    Some((rva, cstr(data, 10)?.to_owned()))
}

/// Raw public code and function names from the PDB, sorted by image-relative RVA.
pub(crate) fn public_symbols(pdb: &[u8], sections: &[(u32, u32)]) -> Vec<(u32, String)> {
    let Some(dir) = directory(pdb) else { return Vec::new() };
    let Some(dbi) = stream(pdb, &dir, 3) else { return Vec::new() };
    let Some(index) = u16_at(&dbi, 20) else { return Vec::new() };
    if index == u16::MAX {
        return Vec::new();
    }
    let Some(records) = stream(pdb, &dir, usize::from(index)) else { return Vec::new() };
    let mut remaining = records.as_slice();
    let mut out = Vec::new();
    while let Some(len) = u16_at(remaining, 0) {
        if len < 2 {
            break;
        }
        let len = usize::from(len);
        let Some(record) = bytes(remaining, 2, len) else { break };
        if u16_at(record, 0) == Some(0x110e) {
            if let Some(symbol) = record.get(2..).and_then(|data| public_symbol(data, sections)) {
                out.push(symbol);
            }
        }
        let Some(next) = len.checked_add(2) else { break };
        let Some(tail) = remaining.get(next..) else { break };
        remaining = tail;
    }
    out.sort_unstable_by_key(|symbol| symbol.0);
    out
}

#[cfg(test)]
mod tests {
    use super::{pdb_path, pe_sections, public_symbols, MSF_MAGIC};

    /// Writes a fixture's 16-bit field.
    fn put_u16(data: &mut [u8], offset: usize, value: u16) {
        data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes a fixture's 32-bit field.
    fn put_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Appends a CodeView record with its length and kind.
    fn record(out: &mut Vec<u8>, kind: u16, data: &[u8]) {
        out.extend_from_slice(&((data.len() + 2) as u16).to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(data);
    }

    /// Makes the payload of an S_PUB32 record.
    fn public(flags: u32, offset: u32, segment: u16, name: &[u8]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&flags.to_le_bytes());
        data.extend_from_slice(&offset.to_le_bytes());
        data.extend_from_slice(&segment.to_le_bytes());
        data.extend_from_slice(name);
        data
    }

    /// Packs small streams into an MSF with 512-byte blocks and one directory block.
    fn make_pdb(symbols: &[u8]) -> Vec<u8> {
        let mut dbi = [0u8; 64];
        put_u32(&mut dbi, 0, u32::MAX);
        put_u16(&mut dbi, 16, 2);
        put_u16(&mut dbi, 20, 5);
        let filler = [0u8; 600];
        let streams: [Option<&[u8]>; 6] = [None, Some(&filler), Some(&[]), Some(&dbi), Some(&[]), Some(symbols)];
        let mut dir = Vec::new();
        dir.extend_from_slice(&(streams.len() as u32).to_le_bytes());
        for data in &streams {
            dir.extend_from_slice(&data.map_or(u32::MAX, |s| s.len() as u32).to_le_bytes());
        }
        // Blocks 0..4: superblock, free block map, directory block map, directory.
        let mut pdb = vec![0u8; 4 * 512];
        pdb[..32].copy_from_slice(MSF_MAGIC);
        put_u32(&mut pdb, 32, 512);
        put_u32(&mut pdb, 36, 1);
        put_u32(&mut pdb, 52, 2);
        put_u32(&mut pdb, 2 * 512, 3);
        for data in streams.into_iter().flatten() {
            for chunk in data.chunks(512) {
                let offset = pdb.len();
                dir.extend_from_slice(&((offset / 512) as u32).to_le_bytes());
                pdb.resize(offset + 512, 0);
                pdb[offset..offset + chunk.len()].copy_from_slice(chunk);
            }
        }
        let count = (pdb.len() / 512) as u32;
        put_u32(&mut pdb, 40, count);
        put_u32(&mut pdb, 44, dir.len() as u32);
        pdb[3 * 512..3 * 512 + dir.len()].copy_from_slice(&dir);
        pdb
    }

    /// Public flags, segment mapping, ordering and truncated records across stream blocks.
    #[test]
    fn reads_public_symbols() {
        let sections = [(0x1000, 0x200), (0x3000, 0x100)];
        let mut records = Vec::new();
        record(&mut records, 0x110e, &public(2, 0x24, 2, b"?entry@@YAXXZ\0"));
        record(&mut records, 0x1125, &[0; 500]);
        record(&mut records, 0x110e, &public(0, 0x10, 1, b"data\0"));
        let function = (0x3024, "?entry@@YAXXZ".to_owned());
        assert_eq!(public_symbols(&make_pdb(&records), &sections), vec![function.clone()]);

        record(&mut records, 0x110e, &public(1, 8, 1, b"code\0"));
        record(&mut records, 0x110e, &public(2, 0, 0, b"no_segment\0"));
        record(&mut records, 0x110e, &public(2, 0, 3, b"bad_segment\0"));
        record(&mut records, 0x110e, &public(2, u32::MAX, 1, b"overflow\0"));
        record(&mut records, 0x110e, &public(2, 0, 1, b"unterminated"));
        record(&mut records, 0x110e, &public(2, 0, 1, b"\xff\0"));
        // A truncated final record preserves the complete records preceding it.
        records.extend_from_slice(&[20, 0, 0x0e, 0x11, 0]);
        let expected = vec![(0x1008, "code".to_owned()), function];
        let pdb = make_pdb(&records);
        assert_eq!(public_symbols(&pdb, &sections), expected);
        assert!(public_symbols(&pdb, &[]).is_empty());

        let mut invalid = pdb.clone();
        let map = invalid.len();
        invalid.resize(133 * 512, 0);
        put_u32(&mut invalid, 40, 133);
        put_u32(&mut invalid, 52, (map / 512) as u32);
        for index in 0..129 {
            put_u32(&mut invalid, map + index * 4, 3);
        }
        put_u32(&mut invalid, 44, 129 * 512);
        assert!(public_symbols(&invalid, &sections).is_empty(), "directory indices exceed one map block");
        let mut invalid = pdb.clone();
        put_u32(&mut invalid, 2 * 512, u32::MAX);
        assert!(public_symbols(&invalid, &sections).is_empty(), "directory block is out of range");
        let mut invalid = pdb.clone();
        put_u32(&mut invalid, 3 * 512, u32::MAX);
        assert!(public_symbols(&invalid, &sections).is_empty(), "stream sizes are truncated");
        let mut invalid = pdb.clone();
        put_u32(&mut invalid, 32, 256);
        assert!(public_symbols(&invalid, &sections).is_empty(), "unsupported block size");
        assert!(public_symbols(&pdb[..pdb.len() - 1], &sections).is_empty(), "truncated block");
        assert!(public_symbols(&[], &sections).is_empty());
    }

    /// Builds a PE32+ image with a section, two debug entries and an RSDS path.
    fn make_pe() -> Vec<u8> {
        let mut image = vec![0u8; 0x400];
        image[..2].copy_from_slice(b"MZ");
        put_u32(&mut image, 0x3c, 0x80);
        image[0x80..0x84].copy_from_slice(b"PE\0\0");
        put_u16(&mut image, 0x84, 0x8664);
        put_u16(&mut image, 0x86, 1);
        put_u16(&mut image, 0x94, 0xf0);
        let optional = 0x98;
        put_u16(&mut image, optional, 0x20b);
        put_u32(&mut image, optional + 108, 16);
        put_u32(&mut image, optional + 112 + 6 * 8, 0x1020);
        put_u32(&mut image, optional + 112 + 6 * 8 + 4, 56);
        let section = optional + 0xf0;
        image[section..section + 8].copy_from_slice(b".rdata\0\0");
        put_u32(&mut image, section + 8, 0x180);
        put_u32(&mut image, section + 12, 0x1000);
        put_u32(&mut image, section + 16, 0x200);
        put_u32(&mut image, section + 20, 0x200);
        let entry = 0x220 + 28;
        let path = b"symbols\\sample.pdb\0";
        put_u32(&mut image, entry + 12, 2);
        put_u32(&mut image, entry + 16, (24 + path.len()) as u32);
        put_u32(&mut image, entry + 20, 0x1080);
        put_u32(&mut image, entry + 24, 0x280);
        image[0x280..0x284].copy_from_slice(b"RSDS");
        image[0x284..0x294].fill(1);
        put_u32(&mut image, 0x294, 1);
        image[0x298..0x298 + path.len()].copy_from_slice(path);
        image
    }

    /// PE section metadata and debug paths stay within their declared file ranges.
    #[test]
    fn reads_pe_metadata() {
        let image = make_pe();
        assert_eq!(pe_sections(&image), vec![(0x1000, 0x180)]);
        assert_eq!(pdb_path(&image).as_deref(), Some("symbols\\sample.pdb"));
        let mut invalid = image.clone();
        put_u32(&mut invalid, 0x98 + 108, 6);
        assert_eq!(pdb_path(&invalid), None, "debug directory is not declared");
        let mut invalid = image.clone();
        put_u16(&mut invalid, 0x94, 112);
        assert_eq!(pdb_path(&invalid), None, "directory lies outside the optional header");
        let mut invalid = image.clone();
        put_u32(&mut invalid, 0x220 + 28 + 16, (24 + b"symbols\\sample.pdb".len()) as u32);
        assert_eq!(pdb_path(&invalid), None, "path has no terminator within SizeOfData");
        let mut invalid = image.clone();
        invalid[0x298] = 0xff;
        assert_eq!(pdb_path(&invalid), None, "path is not UTF-8");
        let mut invalid = image.clone();
        put_u32(&mut invalid, 0x188 + 8, 0x400);
        put_u32(&mut invalid, 0x98 + 112 + 6 * 8, 0x1210);
        assert_eq!(pdb_path(&invalid), None, "RVA has no raw file backing");
        let mut invalid = image.clone();
        put_u32(&mut invalid, 0x3c, u32::MAX);
        assert_eq!(pdb_path(&invalid), None);
        assert!(pe_sections(&invalid).is_empty());
        assert!(pe_sections(&image[..0x188 + 39]).is_empty());
        assert!(pe_sections(&[]).is_empty());
        assert_eq!(pdb_path(&[]), None);
    }
}
