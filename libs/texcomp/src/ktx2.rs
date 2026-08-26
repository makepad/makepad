//! KTX2 texture container (Khronos KTX File Format Specification v2).
//!
//! Implemented:
//! - 80-byte little-endian file header
//! - Level index (`max(1, levelCount)` entries of three `u64`s)
//! - Data Format Descriptor as an opaque byte blob
//! - Key/value data (KVD), including 4-byte `valuePadding`
//! - Supercompression schemes 0 (none, fully implemented), 1 (BasisLZ),
//!   2 (Zstandard) and 3 (ZLIB). Schemes 1–3 return the compressed level
//!   bytes unchanged; decompression is the caller's job.
//!
//! Anything else (vendor supercompression, SGD contents, DFD internals) is
//! rejected or ignored rather than guessed at.
//!
//! Level ordering (spec §3.7 / §3.9.7 / Appendix F):
//! - The **level index** is largest-first: `levels[0]` is the base mip.
//! - The **mip level array** in the file is stored smallest-first (streaming).
//! - [`Writer::level`] is called largest-first, matching the index order.
//!   On write we emit index entries largest-first and payload smallest-first.
//!
//! Alignment (spec §3.9.7 / §3.13.2):
//! - Scheme 0: each mip level is aligned to 16 bytes. 16 is a multiple of
//!   `lcm(texel_block_size, 4)` for every `vkFormat` this crate names
//!   (4 / 8 / 16-byte blocks), so the spec's required alignment is honoured.
//! - Schemes 1–3: tightly packed (alignment 1); no `mipPadding`.
//!
//! `levelCount == 0` means "generate mipmaps at load" and the file contains
//! only the base level. The level index still has exactly one entry
//! (`max(1, levelCount)`). [`Header::levels`] keeps the raw `0`; [`Reader::levels`]
//! exposes that single present level.

use std::convert::TryFrom;

/// KTX 2 identifier: `«KTX 20»\r\n\x1A\n`.
pub const IDENTIFIER: [u8; 12] = [
    0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];

const HEADER_SIZE: usize = 80;
const LEVEL_INDEX_ENTRY_SIZE: u64 = 24;

pub const SUPERCOMPRESSION_NONE: u32 = 0;
pub const SUPERCOMPRESSION_BASISLZ: u32 = 1;
pub const SUPERCOMPRESSION_ZSTANDARD: u32 = 2;
pub const SUPERCOMPRESSION_ZLIB: u32 = 3;

/// `VK_FORMAT_UNDEFINED`
pub const VK_FORMAT_UNDEFINED: u32 = 0;
/// `VK_FORMAT_R8G8B8A8_UNORM`
pub const VK_FORMAT_R8G8B8A8_UNORM: u32 = 37;
/// `VK_FORMAT_BC1_RGB_UNORM_BLOCK`
pub const VK_FORMAT_BC1_RGB_UNORM_BLOCK: u32 = 131;
/// `VK_FORMAT_BC1_RGBA_UNORM_BLOCK`
pub const VK_FORMAT_BC1_RGBA_UNORM_BLOCK: u32 = 133;
/// `VK_FORMAT_BC7_UNORM_BLOCK`
pub const VK_FORMAT_BC7_UNORM_BLOCK: u32 = 145;
/// `VK_FORMAT_ETC2_R8G8B8_UNORM_BLOCK`
pub const VK_FORMAT_ETC2_R8G8B8_UNORM_BLOCK: u32 = 147;
/// `VK_FORMAT_ASTC_4x4_UNORM_BLOCK`
#[allow(non_upper_case_globals)]
pub const VK_FORMAT_ASTC_4x4_UNORM_BLOCK: u32 = 157;

const LEVEL_ALIGN_UNCOMPRESSED: u64 = 16;

/// One mip level's location in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Level {
    pub offset: u64,
    pub length: u64,
    pub uncompressed_length: u64,
}

/// Container-level metadata from the 80-byte KTX2 header.
///
/// Dimension fields are the raw file values: `height`/`depth`/`layers` of `0`
/// mean "not present / treat as 1". `levels` of `0` means "generate mips at
/// load"; one level is still present in the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub vk_format: u32,
    pub type_size: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub layers: u32,
    pub faces: u32,
    pub levels: u32,
    pub supercompression: u32,
}

/// Human-readable name for a known `vkFormat`, or `"VK_FORMAT_UNKNOWN"`.
#[allow(non_upper_case_globals)]
pub fn format_name(vk_format: u32) -> &'static str {
    match vk_format {
        VK_FORMAT_UNDEFINED => "VK_FORMAT_UNDEFINED",
        VK_FORMAT_R8G8B8A8_UNORM => "VK_FORMAT_R8G8B8A8_UNORM",
        VK_FORMAT_BC1_RGB_UNORM_BLOCK => "VK_FORMAT_BC1_RGB_UNORM_BLOCK",
        VK_FORMAT_BC1_RGBA_UNORM_BLOCK => "VK_FORMAT_BC1_RGBA_UNORM_BLOCK",
        VK_FORMAT_BC7_UNORM_BLOCK => "VK_FORMAT_BC7_UNORM_BLOCK",
        VK_FORMAT_ETC2_R8G8B8_UNORM_BLOCK => "VK_FORMAT_ETC2_R8G8B8_UNORM_BLOCK",
        VK_FORMAT_ASTC_4x4_UNORM_BLOCK => "VK_FORMAT_ASTC_4x4_UNORM_BLOCK",
        _ => "VK_FORMAT_UNKNOWN",
    }
}

/// Number of level-index entries actually stored: `max(1, levelCount)`.
fn present_level_count(level_count: u32) -> u64 {
    if level_count == 0 {
        1
    } else {
        u64::from(level_count)
    }
}

fn supercompression_alignment(scheme: u32) -> u64 {
    if scheme == SUPERCOMPRESSION_NONE {
        LEVEL_ALIGN_UNCOMPRESSED
    } else {
        1
    }
}

fn recognised_supercompression(scheme: u32) -> bool {
    matches!(
        scheme,
        SUPERCOMPRESSION_NONE
            | SUPERCOMPRESSION_BASISLZ
            | SUPERCOMPRESSION_ZSTANDARD
            | SUPERCOMPRESSION_ZLIB
    )
}

fn scheme_has_sgd(scheme: u32) -> bool {
    scheme == SUPERCOMPRESSION_BASISLZ
}

fn align_up(value: u64, align: u64) -> Result<u64, String> {
    if align <= 1 {
        return Ok(value);
    }
    let add = align - 1;
    let padded = value
        .checked_add(add)
        .ok_or_else(|| format!("aligning {value} to {align} overflows u64"))?;
    Ok(padded - (padded % align))
}

fn u64_to_usize(value: u64, what: &str) -> Result<usize, String> {
    usize::try_from(value).map_err(|_| format!("{what} {value} exceeds usize"))
}

fn u64_to_u32(value: u64, what: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{what} {value} exceeds u32"))
}

/// Bounds-check `offset + length` against `file_len`. Uses checked
/// arithmetic so a wrapping add cannot sneak past EOF.
fn checked_span(offset: u64, length: u64, file_len: u64, what: &str) -> Result<u64, String> {
    let end = offset.checked_add(length).ok_or_else(|| {
        format!("{what}: offset {offset} + length {length} overflows u64")
    })?;
    if end > file_len {
        return Err(format!(
            "{what}: offset {offset} + length {length} exceeds file size {file_len}"
        ));
    }
    Ok(end)
}

fn file_slice<'a>(bytes: &'a [u8], offset: u64, length: u64, what: &str) -> Result<&'a [u8], String> {
    checked_span(offset, length, bytes.len() as u64, what)?;
    let start = u64_to_usize(offset, what)?;
    let end = start
        .checked_add(u64_to_usize(length, what)?)
        .ok_or_else(|| format!("{what}: offset + length overflows usize"))?;
    bytes
        .get(start..end)
        .ok_or_else(|| format!("{what}: slice [{start}, {end}) out of file"))
}

fn read_u32_at(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| format!("u32 offset {offset} overflows"))?;
    let slice = bytes
        .get(offset..end)
        .ok_or_else(|| format!("truncated u32 at {offset}"))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64_at(bytes: &[u8], offset: usize) -> Result<u64, String> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| format!("u64 offset {offset} overflows"))?;
    let slice = bytes
        .get(offset..end)
        .ok_or_else(|| format!("truncated u64 at {offset}"))?;
    Ok(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

fn parse_key_values(
    bytes: &[u8],
    offset: u64,
    length: u64,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    if length == 0 {
        return Ok(Vec::new());
    }
    let kvd = file_slice(bytes, offset, length, "KVD")?;
    let mut pos = 0usize;
    let mut file_pos = offset;
    let mut out = Vec::new();

    while pos < kvd.len() {
        let remaining = kvd.len() - pos;
        if remaining < 4 {
            return Err("KVD: truncated keyAndValueByteLength".into());
        }
        let kv_len = u64::from(read_u32_at(kvd, pos)?);
        pos = pos
            .checked_add(4)
            .ok_or_else(|| "KVD: position overflow".to_string())?;
        file_pos = file_pos
            .checked_add(4)
            .ok_or_else(|| "KVD: file position overflow".to_string())?;

        if kv_len == 0 {
            return Err("KVD: keyAndValueByteLength is 0".into());
        }
        let kv_len_usize = u64_to_usize(kv_len, "KVD keyAndValueByteLength")?;
        let kv_end = pos
            .checked_add(kv_len_usize)
            .ok_or_else(|| "KVD: keyAndValue length overflow".to_string())?;
        let kv = kvd
            .get(pos..kv_end)
            .ok_or_else(|| "KVD: truncated keyAndValue".to_string())?;
        pos = kv_end;
        file_pos = file_pos
            .checked_add(kv_len)
            .ok_or_else(|| "KVD: file position overflow".to_string())?;

        let nul = kv
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| "KVD: key is not NUL-terminated".to_string())?;
        let key = std::str::from_utf8(kv.get(..nul).ok_or_else(|| "KVD: key slice".to_string())?)
            .map_err(|_| "KVD: key is not valid UTF-8".to_string())?
            .to_string();
        let value_start = nul
            .checked_add(1)
            .ok_or_else(|| "KVD: value start overflow".to_string())?;
        let value = kv
            .get(value_start..)
            .ok_or_else(|| "KVD: value slice".to_string())?
            .to_vec();
        out.push((key, value));

        // valuePadding aligns the next pair to a 4-byte *file* offset.
        let misalign = (file_pos % 4) as usize;
        if misalign != 0 {
            let pad = 4 - misalign;
            let pad_end = pos
                .checked_add(pad)
                .ok_or_else(|| "KVD: padding overflow".to_string())?;
            let _ = kvd
                .get(pos..pad_end)
                .ok_or_else(|| "KVD: truncated valuePadding".to_string())?;
            pos = pad_end;
            file_pos = file_pos
                .checked_add(pad as u64)
                .ok_or_else(|| "KVD: file position overflow".to_string())?;
        }
    }

    if pos != kvd.len() {
        return Err("KVD: parsed length does not match kvdByteLength".into());
    }
    Ok(out)
}

fn encode_kvd(pairs: &[(String, Vec<u8>)], start: u64) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut file_pos = start;
    for (key, value) in pairs {
        if key.as_bytes().contains(&0) {
            return Err("KVD key contains a NUL byte".into());
        }
        let kv_len = (key.len() as u64)
            .checked_add(1)
            .and_then(|n| n.checked_add(value.len() as u64))
            .ok_or_else(|| "KVD key+value length overflows u64".to_string())?;
        let kv_len_u32 = u64_to_u32(kv_len, "KVD keyAndValueByteLength")?;
        buf.extend_from_slice(&kv_len_u32.to_le_bytes());
        buf.extend_from_slice(key.as_bytes());
        buf.push(0);
        buf.extend_from_slice(value);
        file_pos = file_pos
            .checked_add(4)
            .and_then(|n| n.checked_add(kv_len))
            .ok_or_else(|| "KVD write position overflow".to_string())?;
        let misalign = file_pos % 4;
        if misalign != 0 {
            let pad = 4 - misalign;
            let new_len = (buf.len() as u64)
                .checked_add(pad)
                .ok_or_else(|| "KVD padding overflow".to_string())?;
            buf.resize(u64_to_usize(new_len, "KVD buffer")?, 0);
            file_pos = file_pos
                .checked_add(pad)
                .ok_or_else(|| "KVD write position overflow".to_string())?;
        }
    }
    Ok(buf)
}

fn validate_header_fields(header: &Header) -> Result<(), String> {
    if header.width == 0 {
        return Err("pixelWidth must not be 0".into());
    }
    if header.faces != 1 && header.faces != 6 {
        return Err(format!("faceCount must be 1 or 6, got {}", header.faces));
    }
    if header.faces == 6 {
        if header.height != header.width {
            return Err("cubemap pixelHeight must equal pixelWidth".into());
        }
        if header.depth != 0 {
            return Err("cubemap pixelDepth must be 0".into());
        }
    }
    if !recognised_supercompression(header.supercompression) {
        return Err(format!(
            "unsupported supercompressionScheme {}",
            header.supercompression
        ));
    }
    Ok(())
}

/// Parses and validates a KTX2 file, borrowing the original bytes.
pub struct Reader<'a> {
    bytes: &'a [u8],
    header: Header,
    levels: Vec<Level>,
    dfd: Option<&'a [u8]>,
    key_values: Vec<(String, Vec<u8>)>,
}

impl<'a> Reader<'a> {
    /// Parse and validate. Every offset/length must lie inside the file and
    /// not overlap the 80-byte header; reject otherwise.
    pub fn new(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.len() < IDENTIFIER.len() {
            return Err("truncated identifier".into());
        }
        let ident = bytes
            .get(..IDENTIFIER.len())
            .ok_or_else(|| "truncated identifier".to_string())?;
        if ident != IDENTIFIER {
            return Err("invalid KTX2 identifier".into());
        }
        if bytes.len() < HEADER_SIZE {
            return Err("truncated header".into());
        }

        let vk_format = read_u32_at(bytes, 12)?;
        let type_size = read_u32_at(bytes, 16)?;
        let width = read_u32_at(bytes, 20)?;
        let height = read_u32_at(bytes, 24)?;
        let depth = read_u32_at(bytes, 28)?;
        let layers = read_u32_at(bytes, 32)?;
        let faces = read_u32_at(bytes, 36)?;
        let levels_field = read_u32_at(bytes, 40)?;
        let supercompression = read_u32_at(bytes, 44)?;
        let dfd_offset = u64::from(read_u32_at(bytes, 48)?);
        let dfd_length = u64::from(read_u32_at(bytes, 52)?);
        let kvd_offset = u64::from(read_u32_at(bytes, 56)?);
        let kvd_length = u64::from(read_u32_at(bytes, 60)?);
        let sgd_offset = read_u64_at(bytes, 64)?;
        let sgd_length = read_u64_at(bytes, 72)?;

        let header = Header {
            vk_format,
            type_size,
            width,
            height,
            depth,
            layers,
            faces,
            levels: levels_field,
            supercompression,
        };
        validate_header_fields(&header)?;

        let file_len = bytes.len() as u64;
        let nlevels = present_level_count(levels_field);
        let index_bytes = nlevels
            .checked_mul(LEVEL_INDEX_ENTRY_SIZE)
            .ok_or_else(|| "level index size overflows u64".to_string())?;
        let index_end = (HEADER_SIZE as u64)
            .checked_add(index_bytes)
            .ok_or_else(|| "level index end overflows u64".to_string())?;
        if index_end > file_len {
            return Err("truncated level index".into());
        }

        let nlevels_usize = u64_to_usize(nlevels, "levelCount")?;
        let mut levels = Vec::new();
        levels.try_reserve(nlevels_usize).map_err(|_| {
            format!("levelCount {nlevels} is too large to allocate")
        })?;
        for i in 0..nlevels_usize {
            let entry = (HEADER_SIZE as u64)
                .checked_add(
                    (i as u64)
                        .checked_mul(LEVEL_INDEX_ENTRY_SIZE)
                        .ok_or_else(|| "level index offset overflow".to_string())?,
                )
                .ok_or_else(|| "level index offset overflow".to_string())?;
            let entry_usize = u64_to_usize(entry, "level index entry")?;
            let offset = read_u64_at(bytes, entry_usize)?;
            let length = read_u64_at(bytes, entry_usize + 8)?;
            let uncompressed_length = read_u64_at(bytes, entry_usize + 16)?;

            if offset < HEADER_SIZE as u64 {
                return Err(format!(
                    "level {i}: offset {offset} points into the header"
                ));
            }
            checked_span(offset, length, file_len, &format!("level {i}"))?;
            if header.supercompression == SUPERCOMPRESSION_NONE && uncompressed_length != length {
                return Err(format!(
                    "level {i}: uncompressedByteLength {uncompressed_length} != byteLength {length} (scheme 0)"
                ));
            }
            levels.push(Level {
                offset,
                length,
                uncompressed_length,
            });
        }

        let dfd = if dfd_length == 0 {
            if dfd_offset != 0 {
                return Err("dfdByteLength is 0 but dfdByteOffset is not".into());
            }
            None
        } else {
            if dfd_offset < HEADER_SIZE as u64 {
                return Err("DFD overlaps the header".into());
            }
            Some(file_slice(bytes, dfd_offset, dfd_length, "DFD")?)
        };

        let key_values = if kvd_length == 0 {
            if kvd_offset != 0 {
                return Err("kvdByteLength is 0 but kvdByteOffset is not".into());
            }
            Vec::new()
        } else {
            if kvd_offset < HEADER_SIZE as u64 {
                return Err("KVD overlaps the header".into());
            }
            parse_key_values(bytes, kvd_offset, kvd_length)?
        };

        if sgd_length == 0 {
            if sgd_offset != 0 {
                return Err("sgdByteLength is 0 but sgdByteOffset is not".into());
            }
        } else {
            if !scheme_has_sgd(header.supercompression) {
                return Err(format!(
                    "supercompressionScheme {} must not have SGD",
                    header.supercompression
                ));
            }
            if sgd_offset < HEADER_SIZE as u64 {
                return Err("SGD overlaps the header".into());
            }
            // Bounds-check only; BasisLZ global data is not interpreted here.
            let _ = file_slice(bytes, sgd_offset, sgd_length, "SGD")?;
        }

        Ok(Reader {
            bytes,
            header,
            levels,
            dfd,
            key_values,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn levels(&self) -> &[Level] {
        &self.levels
    }

    /// Raw bytes of one mip level, still supercompressed if the scheme is
    /// non-zero. Err on out-of-range index.
    pub fn level_data(&self, index: usize) -> Result<&'a [u8], String> {
        let level = self
            .levels
            .get(index)
            .ok_or_else(|| format!("level index {index} out of range ({} levels)", self.levels.len()))?;
        file_slice(self.bytes, level.offset, level.length, "level data")
    }

    /// Key/value pairs from the KVD section, in file order.
    pub fn key_values(&self) -> Vec<(String, Vec<u8>)> {
        self.key_values.clone()
    }

    /// The raw Data Format Descriptor block, if present.
    pub fn dfd(&self) -> Option<&'a [u8]> {
        self.dfd
    }
}

/// Accumulates mip levels, key/value pairs and an optional DFD, then
/// serialises a KTX2 file.
pub struct Writer {
    header: Header,
    levels: Vec<(Vec<u8>, u64)>,
    key_values: Vec<(String, Vec<u8>)>,
    dfd: Option<Vec<u8>>,
}

impl Writer {
    pub fn new(header: Header) -> Self {
        Writer {
            header,
            levels: Vec::new(),
            key_values: Vec::new(),
            dfd: None,
        }
    }

    /// Push one mip level, **largest first**. `uncompressed_length` should
    /// equal the data length when supercompression is 0.
    pub fn level(&mut self, data: Vec<u8>, uncompressed_length: u64) -> &mut Self {
        self.levels.push((data, uncompressed_length));
        self
    }

    pub fn key_value(&mut self, key: &str, value: &[u8]) -> &mut Self {
        self.key_values.push((key.to_string(), value.to_vec()));
        self
    }

    pub fn dfd(&mut self, dfd: Vec<u8>) -> &mut Self {
        self.dfd = if dfd.is_empty() { None } else { Some(dfd) };
        self
    }

    /// Serialise. Level data is 16-byte aligned when `supercompression == 0`
    /// and tightly packed otherwise. Level-index offsets match the bytes
    /// actually written. Mip payload is stored smallest-first; the index is
    /// largest-first.
    pub fn write(&self) -> Result<Vec<u8>, String> {
        validate_header_fields(&self.header)?;

        let expected = present_level_count(self.header.levels);
        let n = self.levels.len() as u64;
        if n != expected {
            return Err(format!(
                "pushed {} levels but header.levels is {} (need {})",
                n, self.header.levels, expected
            ));
        }
        if n == 0 {
            return Err("no mip levels".into());
        }

        for (i, (data, uncompressed)) in self.levels.iter().enumerate() {
            if self.header.supercompression == SUPERCOMPRESSION_NONE
                && *uncompressed != data.len() as u64
            {
                return Err(format!(
                    "level {i}: uncompressed_length {uncompressed} != data length {} (scheme 0)",
                    data.len()
                ));
            }
        }

        let n_usize = u64_to_usize(n, "level count")?;
        let index_bytes = n
            .checked_mul(LEVEL_INDEX_ENTRY_SIZE)
            .ok_or_else(|| "level index size overflows u64".to_string())?;
        let mut pos = (HEADER_SIZE as u64)
            .checked_add(index_bytes)
            .ok_or_else(|| "header + level index overflows u64".to_string())?;

        let (dfd_offset, dfd_length) = match &self.dfd {
            Some(dfd) => {
                let off = pos;
                let len = dfd.len() as u64;
                pos = pos
                    .checked_add(len)
                    .ok_or_else(|| "DFD length overflows u64".to_string())?;
                (off, len)
            }
            None => (0, 0),
        };

        let kvd = if self.key_values.is_empty() {
            Vec::new()
        } else {
            encode_kvd(&self.key_values, pos)?
        };
        let (kvd_offset, kvd_length) = if kvd.is_empty() {
            (0, 0)
        } else {
            let off = pos;
            let len = kvd.len() as u64;
            pos = pos
                .checked_add(len)
                .ok_or_else(|| "KVD length overflows u64".to_string())?;
            (off, len)
        };

        let align = supercompression_alignment(self.header.supercompression);
        let mut file_offsets = vec![0u64; n_usize];
        for i in (0..n_usize).rev() {
            pos = align_up(pos, align)?;
            file_offsets[i] = pos;
            let len = self.levels[i].0.len() as u64;
            pos = pos
                .checked_add(len)
                .ok_or_else(|| format!("level {i} data length overflows u64"))?;
        }
        let total = pos;

        let mut out = Vec::new();
        out.try_reserve(u64_to_usize(total, "file size")?)
            .map_err(|_| "file size is too large to allocate".to_string())?;

        out.extend_from_slice(&IDENTIFIER);
        out.extend_from_slice(&self.header.vk_format.to_le_bytes());
        out.extend_from_slice(&self.header.type_size.to_le_bytes());
        out.extend_from_slice(&self.header.width.to_le_bytes());
        out.extend_from_slice(&self.header.height.to_le_bytes());
        out.extend_from_slice(&self.header.depth.to_le_bytes());
        out.extend_from_slice(&self.header.layers.to_le_bytes());
        out.extend_from_slice(&self.header.faces.to_le_bytes());
        out.extend_from_slice(&self.header.levels.to_le_bytes());
        out.extend_from_slice(&self.header.supercompression.to_le_bytes());
        out.extend_from_slice(&u64_to_u32(dfd_offset, "dfdByteOffset")?.to_le_bytes());
        out.extend_from_slice(&u64_to_u32(dfd_length, "dfdByteLength")?.to_le_bytes());
        out.extend_from_slice(&u64_to_u32(kvd_offset, "kvdByteOffset")?.to_le_bytes());
        out.extend_from_slice(&u64_to_u32(kvd_length, "kvdByteLength")?.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes()); // sgdByteOffset
        out.extend_from_slice(&0u64.to_le_bytes()); // sgdByteLength

        for i in 0..n_usize {
            let (ref data, uncompressed) = self.levels[i];
            out.extend_from_slice(&file_offsets[i].to_le_bytes());
            out.extend_from_slice(&(data.len() as u64).to_le_bytes());
            out.extend_from_slice(&uncompressed.to_le_bytes());
        }

        if let Some(dfd) = &self.dfd {
            out.extend_from_slice(dfd);
        }
        if !kvd.is_empty() {
            out.extend_from_slice(&kvd);
        }

        for i in (0..n_usize).rev() {
            let target = u64_to_usize(file_offsets[i], "level offset")?;
            if out.len() > target {
                return Err(format!(
                    "level {i} offset {} sits before the previous section (at {})",
                    file_offsets[i],
                    out.len()
                ));
            }
            out.resize(target, 0);
            out.extend_from_slice(&self.levels[i].0);
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_u32(buf: &mut Vec<u8>, v: u32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    fn put_u64(buf: &mut Vec<u8>, v: u64) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    fn set_u64(buf: &mut [u8], at: usize, v: u64) {
        let bytes = v.to_le_bytes();
        buf[at..at + 8].copy_from_slice(&bytes);
    }

    /// A 2×2 R8G8B8A8 image, one mip, no DFD/KVD/SGD. Built by hand so the
    /// reader is not merely agreeing with the writer.
    fn hand_built_minimal() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&IDENTIFIER);
        put_u32(&mut b, VK_FORMAT_R8G8B8A8_UNORM); // vkFormat
        put_u32(&mut b, 1); // typeSize
        put_u32(&mut b, 2); // pixelWidth
        put_u32(&mut b, 2); // pixelHeight
        put_u32(&mut b, 0); // pixelDepth
        put_u32(&mut b, 0); // layerCount
        put_u32(&mut b, 1); // faceCount
        put_u32(&mut b, 1); // levelCount
        put_u32(&mut b, 0); // supercompressionScheme
        put_u32(&mut b, 0); // dfdByteOffset
        put_u32(&mut b, 0); // dfdByteLength
        put_u32(&mut b, 0); // kvdByteOffset
        put_u32(&mut b, 0); // kvdByteLength
        put_u64(&mut b, 0); // sgdByteOffset
        put_u64(&mut b, 0); // sgdByteLength
        assert_eq!(b.len(), HEADER_SIZE);
        put_u64(&mut b, 104); // levels[0].byteOffset
        put_u64(&mut b, 16); // byteLength
        put_u64(&mut b, 16); // uncompressedByteLength
        assert_eq!(b.len(), 104);
        b.extend_from_slice(&[0x11u8; 16]);
        assert_eq!(b.len(), 120);
        b
    }

    fn sample_header(levels: u32, faces: u32, layers: u32, scheme: u32) -> Header {
        Header {
            vk_format: VK_FORMAT_R8G8B8A8_UNORM,
            type_size: 1,
            width: 4,
            height: 4,
            depth: 0,
            layers,
            faces,
            levels,
            supercompression: scheme,
        }
    }

    fn assert_err<T>(r: Result<T, String>, ctx: &str) {
        assert!(r.is_err(), "{ctx}: expected Err, got Ok");
    }

    #[test]
    fn hand_built_header_parse() {
        let bytes = hand_built_minimal();
        let r = Reader::new(&bytes).expect("hand-built file must parse");
        let h = r.header();
        assert_eq!(h.vk_format, VK_FORMAT_R8G8B8A8_UNORM);
        assert_eq!(h.type_size, 1);
        assert_eq!(h.width, 2);
        assert_eq!(h.height, 2);
        assert_eq!(h.depth, 0);
        assert_eq!(h.layers, 0);
        assert_eq!(h.faces, 1);
        assert_eq!(h.levels, 1);
        assert_eq!(h.supercompression, SUPERCOMPRESSION_NONE);
        assert_eq!(r.levels().len(), 1);
        assert_eq!(
            r.levels()[0],
            Level {
                offset: 104,
                length: 16,
                uncompressed_length: 16,
            }
        );
        assert_eq!(r.level_data(0).unwrap(), &[0x11u8; 16]);
        assert!(r.dfd().is_none());
        assert!(r.key_values().is_empty());
        assert!(r.level_data(1).is_err());
    }

    #[test]
    fn round_trip_mips_kvd_dfd() {
        let dfd = vec![0xAAu8, 0xBB, 0xCC, 0xDD, 0x01, 0x02, 0x03, 0x04];
        let mip0 = (0u8..64).collect::<Vec<_>>();
        let mip1 = vec![0x55u8; 16];
        let mip2 = vec![0x66u8; 4];

        let bytes = Writer::new(sample_header(3, 1, 0, SUPERCOMPRESSION_NONE))
            .level(mip0.clone(), 64)
            .level(mip1.clone(), 16)
            .level(mip2.clone(), 4)
            .key_value("KTXorientation", b"rd")
            .key_value("hello", b"world")
            .dfd(dfd.clone())
            .write()
            .expect("write");

        let r = Reader::new(&bytes).expect("read back");
        let h = r.header();
        assert_eq!(h.vk_format, VK_FORMAT_R8G8B8A8_UNORM);
        assert_eq!(h.type_size, 1);
        assert_eq!(h.width, 4);
        assert_eq!(h.height, 4);
        assert_eq!(h.depth, 0);
        assert_eq!(h.layers, 0);
        assert_eq!(h.faces, 1);
        assert_eq!(h.levels, 3);
        assert_eq!(h.supercompression, SUPERCOMPRESSION_NONE);
        assert_eq!(r.levels().len(), 3);
        assert_eq!(r.level_data(0).unwrap(), mip0.as_slice());
        assert_eq!(r.level_data(1).unwrap(), mip1.as_slice());
        assert_eq!(r.level_data(2).unwrap(), mip2.as_slice());
        assert_eq!(r.levels()[0].uncompressed_length, 64);
        assert_eq!(r.levels()[1].uncompressed_length, 16);
        assert_eq!(r.levels()[2].uncompressed_length, 4);
        assert_eq!(
            r.key_values(),
            vec![
                ("KTXorientation".to_string(), b"rd".to_vec()),
                ("hello".to_string(), b"world".to_vec()),
            ]
        );
        assert_eq!(r.dfd(), Some(dfd.as_slice()));
        // Index 0 is the largest mip (first pushed).
        assert!(r.levels()[0].length >= r.levels()[1].length);
        assert!(r.levels()[1].length >= r.levels()[2].length);
        // Payload is stored smallest-first: last index entry has the lowest offset.
        assert!(r.levels()[2].offset < r.levels()[1].offset);
        assert!(r.levels()[1].offset < r.levels()[0].offset);
    }

    #[test]
    fn identifier_wrong_byte_is_err() {
        for i in 0..IDENTIFIER.len() {
            let mut bytes = hand_built_minimal();
            bytes[i] ^= 0x01;
            assert_err(Reader::new(&bytes), &format!("flipped identifier byte {i}"));
        }
    }

    #[test]
    fn truncated_files_err_never_panic() {
        let full = hand_built_minimal();
        let cuts = [
            0,
            1,
            11,
            12,  // after identifier
            20,  // mid-header
            48,
            79,  // last byte of header missing
            80,  // header only, no level index
            88,  // mid-level-index
            103, // one byte short of a complete index
            104, // index complete, no data
            110, // mid-data
            119, // one byte short of data
        ];
        for &n in &cuts {
            let slice = &full[..n];
            assert_err(Reader::new(slice), &format!("truncated to {n} bytes"));
        }
        // Complete file still works.
        Reader::new(&full).unwrap();
    }

    #[test]
    fn hostile_offsets_err() {
        let full = hand_built_minimal();

        // Level pointing past EOF.
        let mut past_eof = full.clone();
        set_u64(&mut past_eof, 80, 10_000);
        set_u64(&mut past_eof, 88, 16);
        assert_err(Reader::new(&past_eof), "level offset past EOF");

        // Length overflows when added to offset (wrapping add would succeed).
        let mut overflow = full.clone();
        set_u64(&mut overflow, 80, 104);
        set_u64(&mut overflow, 88, u64::MAX);
        assert_err(Reader::new(&overflow), "level length u64::MAX overflow");

        let mut overflow2 = full.clone();
        set_u64(&mut overflow2, 80, u64::MAX);
        set_u64(&mut overflow2, 88, 1);
        assert_err(Reader::new(&overflow2), "level offset u64::MAX overflow");

        // Pointing into the header.
        let mut into_header = full.clone();
        set_u64(&mut into_header, 80, 8);
        set_u64(&mut into_header, 88, 16);
        assert_err(Reader::new(&into_header), "level offset into header");

        let mut into_header0 = full.clone();
        set_u64(&mut into_header0, 80, 0);
        set_u64(&mut into_header0, 88, 16);
        assert_err(Reader::new(&into_header0), "level offset 0 into header");
    }

    #[test]
    fn level_count_zero_is_one_present_level() {
        let mut bytes = hand_built_minimal();
        // levelCount field at offset 40.
        bytes[40..44].copy_from_slice(&0u32.to_le_bytes());
        let r = Reader::new(&bytes).expect("levelCount 0 must parse");
        assert_eq!(r.header().levels, 0);
        assert_eq!(r.levels().len(), 1);
        assert_eq!(r.level_data(0).unwrap(), &[0x11u8; 16]);

        // Writer: header.levels = 0 requires exactly one pushed level.
        let out = Writer::new(Header {
            vk_format: VK_FORMAT_R8G8B8A8_UNORM,
            type_size: 1,
            width: 2,
            height: 2,
            depth: 0,
            layers: 0,
            faces: 1,
            levels: 0,
            supercompression: 0,
        })
        .level(vec![0x22; 16], 16)
        .write()
        .unwrap();
        let r2 = Reader::new(&out).unwrap();
        assert_eq!(r2.header().levels, 0);
        assert_eq!(r2.levels().len(), 1);
        assert_eq!(r2.level_data(0).unwrap(), &[0x22u8; 16]);
    }

    #[test]
    fn cubemap_face_count_six_round_trips() {
        let data = vec![0x77u8; 6 * 4 * 4 * 4]; // 6 faces of 4×4 RGBA
        let bytes = Writer::new(sample_header(1, 6, 0, SUPERCOMPRESSION_NONE))
            .level(data.clone(), data.len() as u64)
            .write()
            .unwrap();
        let r = Reader::new(&bytes).unwrap();
        assert_eq!(r.header().faces, 6);
        assert_eq!(r.header().width, 4);
        assert_eq!(r.header().height, 4);
        assert_eq!(r.header().depth, 0);
        assert_eq!(r.level_data(0).unwrap(), data.as_slice());
    }

    #[test]
    fn layer_count_gt_one_round_trips() {
        let data = vec![0x88u8; 3 * 4 * 4 * 4]; // 3 layers of 4×4 RGBA
        let bytes = Writer::new(sample_header(1, 1, 3, SUPERCOMPRESSION_NONE))
            .level(data.clone(), data.len() as u64)
            .write()
            .unwrap();
        let r = Reader::new(&bytes).unwrap();
        assert_eq!(r.header().layers, 3);
        assert_eq!(r.level_data(0).unwrap(), data.as_slice());
    }

    #[test]
    fn supercompression_round_trips_compressed_bytes() {
        let compressed = vec![0xC0, 0xDE, 0xFE, 0xED, 0x01, 0x02, 0x03];
        let bytes = Writer::new(sample_header(1, 1, 0, SUPERCOMPRESSION_ZSTANDARD))
            .level(compressed.clone(), 64)
            .write()
            .unwrap();
        let r = Reader::new(&bytes).unwrap();
        assert_eq!(r.header().supercompression, SUPERCOMPRESSION_ZSTANDARD);
        assert_eq!(r.level_data(0).unwrap(), compressed.as_slice());
        assert_eq!(r.levels()[0].length, compressed.len() as u64);
        assert_eq!(r.levels()[0].uncompressed_length, 64);
        // Supercompressed payload is tightly packed; offset need not be 16-aligned.
        // (It may happen to be, but the scheme is visible either way.)
        assert_ne!(r.header().supercompression, SUPERCOMPRESSION_NONE);

        let zlib = Writer::new(sample_header(1, 1, 0, SUPERCOMPRESSION_ZLIB))
            .level(vec![1, 2, 3, 4], 16)
            .write()
            .unwrap();
        let r = Reader::new(&zlib).unwrap();
        assert_eq!(r.header().supercompression, SUPERCOMPRESSION_ZLIB);
        assert_eq!(r.level_data(0).unwrap(), &[1, 2, 3, 4]);
    }

    #[test]
    fn written_level_offsets_are_16_aligned() {
        let bytes = Writer::new(sample_header(3, 1, 0, SUPERCOMPRESSION_NONE))
            .level(vec![1u8; 64], 64)
            .level(vec![2u8; 16], 16)
            .level(vec![3u8; 4], 4)
            .write()
            .unwrap();
        let r = Reader::new(&bytes).unwrap();
        for (i, level) in r.levels().iter().enumerate() {
            assert_eq!(
                level.offset % 16,
                0,
                "level {i} offset {} is not 16-byte aligned",
                level.offset
            );
        }
        // Smallest-first storage: padding sits between the 4-byte mip and the next.
        let small = &r.levels()[2];
        let mid = &r.levels()[1];
        assert_eq!(small.length, 4);
        assert!(
            mid.offset >= small.offset.checked_add(small.length).unwrap(),
            "mid level overlaps small level"
        );
        assert_eq!((mid.offset - (small.offset + small.length)) % 16, 12);
    }

    #[test]
    fn format_name_known_and_unknown() {
        assert_eq!(format_name(VK_FORMAT_UNDEFINED), "VK_FORMAT_UNDEFINED");
        assert_eq!(
            format_name(VK_FORMAT_R8G8B8A8_UNORM),
            "VK_FORMAT_R8G8B8A8_UNORM"
        );
        assert_eq!(
            format_name(VK_FORMAT_BC1_RGB_UNORM_BLOCK),
            "VK_FORMAT_BC1_RGB_UNORM_BLOCK"
        );
        assert_eq!(
            format_name(VK_FORMAT_BC1_RGBA_UNORM_BLOCK),
            "VK_FORMAT_BC1_RGBA_UNORM_BLOCK"
        );
        assert_eq!(
            format_name(VK_FORMAT_BC7_UNORM_BLOCK),
            "VK_FORMAT_BC7_UNORM_BLOCK"
        );
        assert_eq!(
            format_name(VK_FORMAT_ETC2_R8G8B8_UNORM_BLOCK),
            "VK_FORMAT_ETC2_R8G8B8_UNORM_BLOCK"
        );
        assert_eq!(
            format_name(VK_FORMAT_ASTC_4x4_UNORM_BLOCK),
            "VK_FORMAT_ASTC_4x4_UNORM_BLOCK"
        );
        assert_eq!(format_name(0xFFFF_FFFF), "VK_FORMAT_UNKNOWN");
        assert_eq!(format_name(99), "VK_FORMAT_UNKNOWN");
        assert!(!format_name(12345).is_empty());
    }

    #[test]
    fn unknown_supercompression_is_err() {
        let mut bytes = hand_built_minimal();
        bytes[44..48].copy_from_slice(&99u32.to_le_bytes());
        assert_err(Reader::new(&bytes), "scheme 99");
        bytes[44..48].copy_from_slice(&4u32.to_le_bytes());
        assert_err(Reader::new(&bytes), "scheme 4 reserved");
    }

    #[test]
    fn bad_face_count_is_err() {
        let mut bytes = hand_built_minimal();
        bytes[36..40].copy_from_slice(&2u32.to_le_bytes());
        assert_err(Reader::new(&bytes), "faceCount 2");
        bytes[36..40].copy_from_slice(&0u32.to_le_bytes());
        assert_err(Reader::new(&bytes), "faceCount 0");
    }

    #[test]
    fn dfd_and_kvd_out_of_range_err() {
        let mut bytes = hand_built_minimal();
        bytes[48..52].copy_from_slice(&10_000u32.to_le_bytes());
        bytes[52..56].copy_from_slice(&8u32.to_le_bytes());
        assert_err(Reader::new(&bytes), "DFD past EOF");

        let mut bytes = hand_built_minimal();
        bytes[56..60].copy_from_slice(&8u32.to_le_bytes()); // kvd offset into header
        bytes[60..64].copy_from_slice(&8u32.to_le_bytes());
        assert_err(Reader::new(&bytes), "KVD overlaps header");

        let mut bytes = hand_built_minimal();
        bytes[64..72].copy_from_slice(&1u64.to_le_bytes()); // sgd offset, length 0
        assert_err(Reader::new(&bytes), "SGD offset with zero length");
    }

    #[test]
    fn writer_rejects_level_count_mismatch() {
        let err = Writer::new(sample_header(2, 1, 0, 0))
            .level(vec![0; 16], 16)
            .write();
        assert_err(err, "one level for header.levels=2");
    }

    #[test]
    fn kvd_missing_nul_is_err() {
        let mut bytes = hand_built_minimal();
        // Append a 4-byte length + 4 bytes of non-NUL data after the image,
        // and point KVD at it. Image occupies 104..120; put KVD at 120.
        put_u32(&mut bytes, 4);
        bytes.extend_from_slice(&[1, 2, 3, 4]);
        let kvd_off = 120u32;
        let kvd_len = 8u32;
        bytes[56..60].copy_from_slice(&kvd_off.to_le_bytes());
        bytes[60..64].copy_from_slice(&kvd_len.to_le_bytes());
        assert_err(Reader::new(&bytes), "KVD key without NUL");
    }

    #[test]
    fn basislz_sgd_is_bounds_checked_not_interpreted() {
        // Hand-build scheme 1 with a 8-byte SGD blob sitting after the image.
        let mut bytes = hand_built_minimal();
        bytes[44..48].copy_from_slice(&SUPERCOMPRESSION_BASISLZ.to_le_bytes());
        // Scheme 1 does not require uncompressedByteLength == byteLength.
        set_u64(&mut bytes, 96, 0);
        let sgd_off = 120u64;
        let sgd_len = 8u64;
        bytes.extend_from_slice(&[9u8; 8]);
        bytes[64..72].copy_from_slice(&sgd_off.to_le_bytes());
        bytes[72..80].copy_from_slice(&sgd_len.to_le_bytes());
        let r = Reader::new(&bytes).expect("BasisLZ with SGD should parse");
        assert_eq!(r.header().supercompression, SUPERCOMPRESSION_BASISLZ);
        assert_eq!(r.level_data(0).unwrap(), &[0x11u8; 16]);

        // SGD past EOF must fail.
        bytes[64..72].copy_from_slice(&10_000u64.to_le_bytes());
        assert_err(Reader::new(&bytes), "SGD past EOF");
    }

    #[test]
    fn sgd_rejected_for_scheme_zero() {
        let mut bytes = hand_built_minimal();
        bytes.extend_from_slice(&[1u8; 8]);
        bytes[64..72].copy_from_slice(&120u64.to_le_bytes());
        bytes[72..80].copy_from_slice(&8u64.to_le_bytes());
        assert_err(Reader::new(&bytes), "SGD with scheme 0");
    }

    #[test]
    fn vk_format_constants_are_the_vulkan_values() {
        assert_eq!(VK_FORMAT_UNDEFINED, 0);
        assert_eq!(VK_FORMAT_R8G8B8A8_UNORM, 37);
        assert_eq!(VK_FORMAT_BC1_RGB_UNORM_BLOCK, 131);
        assert_eq!(VK_FORMAT_BC1_RGBA_UNORM_BLOCK, 133);
        assert_eq!(VK_FORMAT_BC7_UNORM_BLOCK, 145);
        assert_eq!(VK_FORMAT_ETC2_R8G8B8_UNORM_BLOCK, 147);
        assert_eq!(VK_FORMAT_ASTC_4x4_UNORM_BLOCK, 157);
    }
}
