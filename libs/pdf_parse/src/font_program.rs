//! Glyph-name recovery from embedded CFF and sfnt font programs.
//!
//! This is intentionally a metadata parser: it reads only the tables needed
//! to connect an encoded character code to a glyph ID and then to a PostScript
//! glyph name. It does not parse outlines or change PDF text decoding.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;

/// The kind of embedded font container that was parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontProgramKind {
    Cff,
    TrueType,
    OpenType,
}

/// Whether an embedded program can provide genuine glyph names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontNameStatus {
    /// GIDs have PostScript glyph names.
    GlyphNames,
    /// The CFF Top DICT contains ROS; charset values are CIDs, not names.
    Cids,
    /// No supported glyph-name table was present.
    Unavailable,
}

/// A bounded, typed failure from parsing embedded font metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontProgramError {
    pub kind: FontProgramErrorKind,
    pub context: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontProgramErrorKind {
    Truncated,
    Invalid,
    Unsupported,
}

impl FontProgramError {
    fn truncated(context: &'static str) -> Self {
        Self {
            kind: FontProgramErrorKind::Truncated,
            context,
        }
    }

    fn invalid(context: &'static str) -> Self {
        Self {
            kind: FontProgramErrorKind::Invalid,
            context,
        }
    }

    fn unsupported(context: &'static str) -> Self {
        Self {
            kind: FontProgramErrorKind::Unsupported,
            context,
        }
    }
}

impl fmt::Display for FontProgramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} embedded font data: {}", self.kind, self.context)
    }
}

impl Error for FontProgramError {}

/// Metadata recovered from an embedded font program.
#[derive(Clone, Debug)]
pub struct FontProgram {
    pub kind: FontProgramKind,
    pub name_status: FontNameStatus,
    glyph_names: Vec<Option<String>>,
    cff_encoding: HashMap<u8, u16>,
    cmap: Option<Cmap>,
}

impl FontProgram {
    /// Resolve a font-program character code to its glyph ID.
    ///
    /// Bare CFF uses its Encoding; sfnt TrueType/OpenType uses `cmap`.
    pub fn glyph_id(&self, code: u32) -> Option<u16> {
        if let Ok(code) = u8::try_from(code) {
            if let Some(gid) = self.cff_encoding.get(&code) {
                return Some(*gid);
            }
        }
        self.cmap_gid(code)
    }

    /// Look up a Unicode/character code in an sfnt `cmap` table.
    pub fn cmap_gid(&self, code: u32) -> Option<u16> {
        self.cmap.as_ref()?.lookup(code)
    }

    /// Resolve a font-program character code to a genuine glyph name.
    pub fn glyph_name(&self, code: u32) -> Option<&str> {
        let gid = usize::from(self.glyph_id(code)?);
        self.glyph_names.get(gid)?.as_deref()
    }

    /// Resolve a GID directly to a genuine glyph name.
    pub fn glyph_name_for_gid(&self, gid: u16) -> Option<&str> {
        self.glyph_names.get(usize::from(gid))?.as_deref()
    }
}

/// Parse a bare CFF (PDF `FontFile3` subtype `Type1C` or
/// `CIDFontType0C`) program.
pub fn parse_cff(data: &[u8]) -> Result<FontProgram, FontProgramError> {
    let header = data
        .get(..4)
        .ok_or_else(|| FontProgramError::truncated("CFF header"))?;
    if header[0] != 1 {
        return Err(FontProgramError::unsupported("CFF major version"));
    }
    let header_size = usize::from(header[2]);
    if header_size < 4 || header_size > data.len() || !(1..=4).contains(&header[3]) {
        return Err(FontProgramError::invalid("CFF header size/offSize"));
    }

    let mut cursor = header_size;
    let names = parse_cff_index(data, &mut cursor, "CFF Name INDEX")?;
    if names.is_empty() {
        return Err(FontProgramError::invalid("empty CFF Name INDEX"));
    }
    let top_dicts = parse_cff_index(data, &mut cursor, "CFF Top DICT INDEX")?;
    let top = top_dicts
        .first()
        .ok_or_else(|| FontProgramError::invalid("empty CFF Top DICT INDEX"))?;
    let top = parse_top_dict(top)?;
    let strings = parse_cff_index(data, &mut cursor, "CFF String INDEX")?;
    // Parse even though names do not consume subroutines: doing so validates
    // the required CFF layout and catches truncation cleanly.
    let _global_subrs = parse_cff_index(data, &mut cursor, "CFF Global Subr INDEX")?;

    let charstrings_offset = top
        .charstrings_offset
        .ok_or_else(|| FontProgramError::invalid("missing CFF CharStrings offset"))?;
    let mut charstrings_cursor = charstrings_offset;
    let charstrings = parse_cff_index(data, &mut charstrings_cursor, "CFF CharStrings INDEX")?;
    if charstrings.is_empty() || charstrings.len() > usize::from(u16::MAX) {
        return Err(FontProgramError::invalid("invalid CFF glyph count"));
    }
    let glyph_count = charstrings.len();
    let sids = parse_cff_charset(data, top.charset_offset.unwrap_or(0), glyph_count)?;

    if top.has_ros {
        return Ok(FontProgram {
            kind: FontProgramKind::Cff,
            name_status: FontNameStatus::Cids,
            glyph_names: vec![None; glyph_count],
            cff_encoding: HashMap::new(),
            cmap: None,
        });
    }

    let glyph_names = sids
        .iter()
        .map(|sid| resolve_cff_sid(*sid, &strings).map(str::to_owned))
        .collect::<Vec<_>>();
    let sid_to_gid = sids
        .iter()
        .enumerate()
        .filter_map(|(gid, sid)| u16::try_from(gid).ok().map(|gid| (*sid, gid)))
        .collect::<HashMap<_, _>>();
    let cff_encoding = parse_cff_encoding(
        data,
        top.encoding_offset.unwrap_or(0),
        glyph_count,
        &sid_to_gid,
    )?;

    Ok(FontProgram {
        kind: FontProgramKind::Cff,
        name_status: FontNameStatus::GlyphNames,
        glyph_names,
        cff_encoding,
        cmap: None,
    })
}

/// Parse a TrueType/OpenType sfnt program. `OpenType` containers may use
/// either TrueType outlines or a `CFF ` table.
pub fn parse_sfnt(
    data: &[u8],
    kind: FontProgramKind,
) -> Result<FontProgram, FontProgramError> {
    if !matches!(kind, FontProgramKind::TrueType | FontProgramKind::OpenType) {
        return Err(FontProgramError::invalid("sfnt kind"));
    }
    let header = data
        .get(..12)
        .ok_or_else(|| FontProgramError::truncated("sfnt header"))?;
    if !matches!(&header[..4], b"\0\x01\0\0" | b"true" | b"typ1" | b"OTTO") {
        return Err(FontProgramError::invalid("sfnt scaler type"));
    }
    let table_count = usize::from(be_u16(header, 4)?);
    let directory_len = table_count
        .checked_mul(16)
        .and_then(|n| n.checked_add(12))
        .ok_or_else(|| FontProgramError::invalid("sfnt table count"))?;
    if directory_len > data.len() {
        return Err(FontProgramError::truncated("sfnt table directory"));
    }

    let mut tables = HashMap::<[u8; 4], &[u8]>::new();
    for index in 0..table_count {
        let record_offset = 12 + index * 16;
        let record = &data[record_offset..record_offset + 16];
        let tag: [u8; 4] = record[..4].try_into().unwrap();
        let offset = usize::try_from(be_u32(record, 8)?)
            .map_err(|_| FontProgramError::invalid("sfnt table offset"))?;
        let len = usize::try_from(be_u32(record, 12)?)
            .map_err(|_| FontProgramError::invalid("sfnt table length"))?;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| FontProgramError::invalid("sfnt table range"))?;
        let table = data
            .get(offset..end)
            .ok_or_else(|| FontProgramError::truncated("sfnt table"))?;
        tables.entry(tag).or_insert(table);
    }

    // A bad optional table degrades to "unavailable" rather than making a
    // usable font resource disappear.
    let post_names = tables
        .get(b"post")
        .and_then(|table| parse_post_names(table).ok().flatten());
    let cmap = tables
        .get(b"cmap")
        .and_then(|table| Cmap::parse(table).ok());

    let mut cff_status = FontNameStatus::Unavailable;
    let cff_names = tables.get(b"CFF ").and_then(|table| {
        let cff = parse_cff(table).ok()?;
        cff_status = cff.name_status;
        Some(cff.glyph_names)
    });
    let glyph_names = post_names.or(cff_names).unwrap_or_default();
    let name_status = if glyph_names.iter().any(Option::is_some) {
        FontNameStatus::GlyphNames
    } else {
        cff_status
    };

    Ok(FontProgram {
        kind,
        name_status,
        glyph_names,
        cff_encoding: HashMap::new(),
        cmap,
    })
}

#[derive(Clone, Copy, Debug, Default)]
struct CffTopDict {
    charset_offset: Option<usize>,
    encoding_offset: Option<usize>,
    charstrings_offset: Option<usize>,
    has_ros: bool,
}

fn parse_top_dict(data: &[u8]) -> Result<CffTopDict, FontProgramError> {
    let mut top = CffTopDict::default();
    let mut cursor = 0usize;
    let mut operands = Vec::<i32>::new();
    while cursor < data.len() {
        let byte = data[cursor];
        cursor += 1;
        if byte <= 21 {
            let operator = if byte == 12 {
                let escaped = *data
                    .get(cursor)
                    .ok_or_else(|| FontProgramError::truncated("CFF DICT operator"))?;
                cursor += 1;
                1200 + u16::from(escaped)
            } else {
                u16::from(byte)
            };
            let offset = || {
                if operands.len() == 1 && operands[0] >= 0 {
                    usize::try_from(operands[0]).ok()
                } else {
                    None
                }
            };
            match operator {
                15 => {
                    top.charset_offset = Some(
                        offset()
                            .ok_or_else(|| FontProgramError::invalid("CFF charset offset"))?,
                    )
                }
                16 => {
                    top.encoding_offset = Some(
                        offset()
                            .ok_or_else(|| FontProgramError::invalid("CFF Encoding offset"))?,
                    )
                }
                17 => {
                    top.charstrings_offset = Some(
                        offset()
                            .ok_or_else(|| FontProgramError::invalid("CFF CharStrings offset"))?,
                    )
                }
                1230 => {
                    if operands.len() != 3 || operands.iter().any(|operand| *operand < 0) {
                        return Err(FontProgramError::invalid("CFF ROS operands"));
                    }
                    top.has_ros = true;
                }
                _ => {}
            }
            operands.clear();
        } else {
            operands.push(parse_cff_dict_integer(byte, data, &mut cursor)?);
            if operands.len() > 48 {
                return Err(FontProgramError::invalid("CFF DICT operand limit"));
            }
        }
    }
    Ok(top)
}

fn parse_cff_dict_integer(
    first: u8,
    data: &[u8],
    cursor: &mut usize,
) -> Result<i32, FontProgramError> {
    let take = |cursor: &mut usize| -> Result<u8, FontProgramError> {
        let byte = *data
            .get(*cursor)
            .ok_or_else(|| FontProgramError::truncated("CFF DICT number"))?;
        *cursor += 1;
        Ok(byte)
    };
    match first {
        28 => {
            let high = take(cursor)?;
            let low = take(cursor)?;
            Ok(i16::from_be_bytes([high, low]).into())
        }
        29 => {
            let bytes = [take(cursor)?, take(cursor)?, take(cursor)?, take(cursor)?];
            Ok(i32::from_be_bytes(bytes))
        }
        30 => {
            // Real operands are legal in DICTs but cannot be table offsets.
            // Consume the bounded nibble stream and retain a sentinel value.
            loop {
                let byte = take(cursor)?;
                if byte >> 4 == 0x0f || byte & 0x0f == 0x0f {
                    break;
                }
            }
            Ok(i32::MIN)
        }
        32..=246 => Ok(i32::from(first) - 139),
        247..=250 => Ok((i32::from(first) - 247) * 256 + i32::from(take(cursor)?) + 108),
        251..=254 => Ok(-(i32::from(first) - 251) * 256 - i32::from(take(cursor)?) - 108),
        _ => Err(FontProgramError::invalid("CFF DICT number encoding")),
    }
}

fn parse_cff_index<'a>(
    data: &'a [u8],
    cursor: &mut usize,
    context: &'static str,
) -> Result<Vec<&'a [u8]>, FontProgramError> {
    let count = usize::from(read_u16(data, cursor, context)?);
    if count == 0 {
        return Ok(Vec::new());
    }
    let off_size = usize::from(read_u8(data, cursor, context)?);
    if !(1..=4).contains(&off_size) {
        return Err(FontProgramError::invalid("CFF INDEX offSize"));
    }
    let offsets_len = count
        .checked_add(1)
        .and_then(|n| n.checked_mul(off_size))
        .ok_or_else(|| FontProgramError::invalid("CFF INDEX count"))?;
    let offsets_data = data
        .get(*cursor..cursor.saturating_add(offsets_len))
        .ok_or_else(|| FontProgramError::truncated(context))?;
    *cursor += offsets_len;
    let mut offsets = Vec::with_capacity(count + 1);
    for raw in offsets_data.chunks_exact(off_size) {
        let mut value = 0usize;
        for byte in raw {
            value = value
                .checked_mul(256)
                .and_then(|n| n.checked_add(usize::from(*byte)))
                .ok_or_else(|| FontProgramError::invalid("CFF INDEX offset"))?;
        }
        let value = value
            .checked_sub(1)
            .ok_or_else(|| FontProgramError::invalid("zero CFF INDEX offset"))?;
        if offsets.last().is_some_and(|previous| value < *previous) {
            return Err(FontProgramError::invalid("descending CFF INDEX offsets"));
        }
        offsets.push(value);
    }
    if offsets.first() != Some(&0) {
        return Err(FontProgramError::invalid("CFF INDEX first offset"));
    }
    let data_len = *offsets
        .last()
        .ok_or_else(|| FontProgramError::invalid("CFF INDEX offsets"))?;
    let object_data = data
        .get(*cursor..cursor.saturating_add(data_len))
        .ok_or_else(|| FontProgramError::truncated(context))?;
    *cursor += data_len;
    let mut items = Vec::with_capacity(count);
    for pair in offsets.windows(2) {
        items.push(
            object_data
                .get(pair[0]..pair[1])
                .ok_or_else(|| FontProgramError::invalid("CFF INDEX item range"))?,
        );
    }
    Ok(items)
}

fn parse_cff_charset(
    data: &[u8],
    offset: usize,
    glyph_count: usize,
) -> Result<Vec<u16>, FontProgramError> {
    let mut sids = Vec::with_capacity(glyph_count);
    sids.push(0);
    match offset {
        0 => {
            if glyph_count > 229 {
                return Err(FontProgramError::invalid("ISOAdobe charset glyph count"));
            }
            for gid in 1..glyph_count {
                sids.push(
                    u16::try_from(gid)
                        .map_err(|_| FontProgramError::invalid("ISOAdobe charset GID"))?,
                );
            }
        }
        1 => {
            for gid in 1..glyph_count {
                let sid = EXPERT_CHARSET
                    .get(gid)
                    .ok_or_else(|| FontProgramError::invalid("Expert charset GID"))?;
                sids.push(*sid);
            }
        }
        2 => {
            for gid in 1..glyph_count {
                let sid = EXPERT_SUBSET_CHARSET
                    .get(gid)
                    .ok_or_else(|| FontProgramError::invalid("ExpertSubset charset GID"))?;
                sids.push(*sid);
            }
        }
        _ => {
            let mut cursor = offset;
            let format = read_u8(data, &mut cursor, "CFF charset format")?;
            match format {
                0 => {
                    while sids.len() < glyph_count {
                        sids.push(read_u16(data, &mut cursor, "CFF charset format 0")?);
                    }
                }
                1 | 2 => {
                    while sids.len() < glyph_count {
                        let first = read_u16(data, &mut cursor, "CFF charset range")?;
                        let left = if format == 1 {
                            u16::from(read_u8(data, &mut cursor, "CFF charset format 1")?)
                        } else {
                            read_u16(data, &mut cursor, "CFF charset format 2")?
                        };
                        let range_len = usize::from(left) + 1;
                        if range_len > glyph_count - sids.len() {
                            return Err(FontProgramError::invalid("CFF charset range length"));
                        }
                        for delta in 0..=left {
                            sids.push(
                                first
                                    .checked_add(delta)
                                    .ok_or_else(|| FontProgramError::invalid("CFF charset SID"))?,
                            );
                        }
                    }
                }
                _ => return Err(FontProgramError::unsupported("CFF charset format")),
            }
        }
    }
    Ok(sids)
}

fn parse_cff_encoding(
    data: &[u8],
    offset: usize,
    glyph_count: usize,
    sid_to_gid: &HashMap<u16, u16>,
) -> Result<HashMap<u8, u16>, FontProgramError> {
    if offset == 0 {
        return Ok(STANDARD_ENCODING
            .iter()
            .enumerate()
            .filter_map(|(code, sid)| {
                if *sid == 0 {
                    None
                } else {
                    sid_to_gid
                        .get(sid)
                        .copied()
                        .map(|gid| (code as u8, gid))
                }
            })
            .collect());
    }
    if offset == 1 {
        return Err(FontProgramError::unsupported(
            "predefined CFF ExpertEncoding",
        ));
    }

    let mut cursor = offset;
    let raw_format = read_u8(data, &mut cursor, "CFF Encoding format")?;
    let has_supplements = raw_format & 0x80 != 0;
    let mut map = HashMap::new();
    let mut next_gid = 1usize;
    match raw_format & 0x7f {
        0 => {
            let count = usize::from(read_u8(data, &mut cursor, "CFF Encoding format 0")?);
            if count > glyph_count.saturating_sub(1) {
                return Err(FontProgramError::invalid("CFF Encoding glyph count"));
            }
            for _ in 0..count {
                let code = read_u8(data, &mut cursor, "CFF Encoding format 0 code")?;
                let gid = u16::try_from(next_gid)
                    .map_err(|_| FontProgramError::invalid("CFF Encoding GID"))?;
                map.insert(code, gid);
                next_gid += 1;
            }
        }
        1 => {
            let range_count = usize::from(read_u8(data, &mut cursor, "CFF Encoding format 1")?);
            for _ in 0..range_count {
                let first = read_u8(data, &mut cursor, "CFF Encoding range")?;
                let left = read_u8(data, &mut cursor, "CFF Encoding range")?;
                for delta in 0..=left {
                    if next_gid >= glyph_count {
                        return Err(FontProgramError::invalid("CFF Encoding range length"));
                    }
                    let code = first
                        .checked_add(delta)
                        .ok_or_else(|| FontProgramError::invalid("CFF Encoding code range"))?;
                    let gid = u16::try_from(next_gid)
                        .map_err(|_| FontProgramError::invalid("CFF Encoding GID"))?;
                    map.insert(code, gid);
                    next_gid += 1;
                }
            }
        }
        _ => return Err(FontProgramError::unsupported("CFF Encoding format")),
    }

    if has_supplements {
        let count = usize::from(read_u8(data, &mut cursor, "CFF Encoding supplements")?);
        for _ in 0..count {
            let code = read_u8(data, &mut cursor, "CFF Encoding supplement code")?;
            let sid = read_u16(data, &mut cursor, "CFF Encoding supplement SID")?;
            if let Some(gid) = sid_to_gid.get(&sid) {
                map.insert(code, *gid);
            }
        }
    }
    Ok(map)
}

fn resolve_cff_sid<'a>(sid: u16, strings: &'a [&'a [u8]]) -> Option<&'a str> {
    let sid = usize::from(sid);
    if let Some(name) = CFF_STANDARD_STRINGS.get(sid) {
        return Some(name);
    }
    let custom = strings.get(sid.checked_sub(CFF_STANDARD_STRINGS.len())?)?;
    std::str::from_utf8(custom).ok()
}

fn parse_post_names(data: &[u8]) -> Result<Option<Vec<Option<String>>>, FontProgramError> {
    let header = data
        .get(..32)
        .ok_or_else(|| FontProgramError::truncated("post header"))?;
    if be_u32(header, 0)? != 0x0002_0000 {
        return Ok(None);
    }
    let glyph_count = usize::from(be_u16(data, 32)?);
    let indexes_end = 34usize
        .checked_add(
            glyph_count
                .checked_mul(2)
                .ok_or_else(|| FontProgramError::invalid("post glyph count"))?,
        )
        .ok_or_else(|| FontProgramError::invalid("post glyph indexes"))?;
    let indexes = data
        .get(34..indexes_end)
        .ok_or_else(|| FontProgramError::truncated("post glyph indexes"))?;
    let max_custom = indexes
        .chunks_exact(2)
        .filter_map(|raw| {
            let index = u16::from_be_bytes([raw[0], raw[1]]);
            index.checked_sub(257)
        })
        .max()
        .unwrap_or(0);
    let mut custom_names = Vec::with_capacity(usize::from(max_custom));
    let mut cursor = indexes_end;
    for _ in 0..max_custom {
        let len = usize::from(read_u8(data, &mut cursor, "post custom name length")?);
        if len == 0 {
            return Err(FontProgramError::invalid("empty post custom name"));
        }
        let bytes = data
            .get(cursor..cursor.saturating_add(len))
            .ok_or_else(|| FontProgramError::truncated("post custom name"))?;
        cursor += len;
        custom_names.push(std::str::from_utf8(bytes).ok().map(str::to_owned));
    }

    let names = indexes
        .chunks_exact(2)
        .map(|raw| {
            let index = u16::from_be_bytes([raw[0], raw[1]]);
            if let Some(name) = MACINTOSH_GLYPH_NAMES.get(usize::from(index)) {
                Some((*name).to_owned())
            } else {
                let custom = usize::from(index.checked_sub(258)?);
                custom_names.get(custom)?.clone()
            }
        })
        .collect();
    Ok(Some(names))
}

#[derive(Clone, Debug)]
struct Cmap {
    data: Vec<u8>,
    records: Vec<CmapRecord>,
}

#[derive(Clone, Copy, Debug)]
struct CmapRecord {
    offset: usize,
    format: u16,
    symbol: bool,
    rank: u8,
}

impl Cmap {
    fn parse(data: &[u8]) -> Result<Self, FontProgramError> {
        let header = data
            .get(..4)
            .ok_or_else(|| FontProgramError::truncated("cmap header"))?;
        if be_u16(header, 0)? != 0 {
            return Err(FontProgramError::invalid("cmap version"));
        }
        let count = usize::from(be_u16(header, 2)?);
        let records_end = count
            .checked_mul(8)
            .and_then(|n| n.checked_add(4))
            .ok_or_else(|| FontProgramError::invalid("cmap record count"))?;
        let records_data = data
            .get(4..records_end)
            .ok_or_else(|| FontProgramError::truncated("cmap records"))?;
        let mut records = Vec::new();
        for raw in records_data.chunks_exact(8) {
            let platform = be_u16(raw, 0)?;
            let encoding = be_u16(raw, 2)?;
            let offset = usize::try_from(be_u32(raw, 4)?)
                .map_err(|_| FontProgramError::invalid("cmap subtable offset"))?;
            let Some(format_data) = data.get(offset..) else {
                continue;
            };
            let Ok(format) = be_u16(format_data, 0) else {
                continue;
            };
            if !matches!(format, 0 | 4 | 6 | 10 | 12 | 13) {
                continue;
            }
            let rank = match (platform, encoding) {
                (0, _) => 0,
                (3, 10) => 1,
                (3, 1) => 2,
                (3, 0) => 3,
                (1, 0) => 4,
                _ => 5,
            };
            records.push(CmapRecord {
                offset,
                format,
                symbol: platform == 3 && encoding == 0,
                rank,
            });
        }
        records.sort_by_key(|record| record.rank);
        Ok(Self {
            data: data.to_vec(),
            records,
        })
    }

    fn lookup(&self, code: u32) -> Option<u16> {
        for record in &self.records {
            let data = self.data.get(record.offset..)?;
            if let Some(gid) = cmap_subtable_lookup(data, record.format, code) {
                if gid != 0 {
                    return Some(gid);
                }
            }
            if record.symbol && code <= 0xff {
                if let Some(gid) = cmap_subtable_lookup(data, record.format, code + 0xf000) {
                    if gid != 0 {
                        return Some(gid);
                    }
                }
            }
        }
        None
    }
}

fn cmap_subtable_lookup(data: &[u8], format: u16, code: u32) -> Option<u16> {
    match format {
        0 => {
            let length = usize::from(be_u16(data, 2).ok()?);
            let table = data.get(..length)?;
            let code = usize::try_from(code).ok()?;
            Some(u16::from(*table.get(6usize.checked_add(code)?)?))
        }
        4 => cmap_format_4(data, code),
        6 => {
            let length = usize::from(be_u16(data, 2).ok()?);
            let table = data.get(..length)?;
            let first = u32::from(be_u16(table, 6).ok()?);
            let count = u32::from(be_u16(table, 8).ok()?);
            let index = code.checked_sub(first)?;
            if index >= count {
                return None;
            }
            let offset = usize::try_from(index).ok()?.checked_mul(2)?.checked_add(10)?;
            be_u16(table, offset).ok()
        }
        10 => {
            let length = usize::try_from(be_u32(data, 4).ok()?).ok()?;
            let table = data.get(..length)?;
            let first = be_u32(table, 12).ok()?;
            let count = be_u32(table, 16).ok()?;
            let index = code.checked_sub(first)?;
            if index >= count {
                return None;
            }
            let offset = usize::try_from(index).ok()?.checked_mul(2)?.checked_add(20)?;
            be_u16(table, offset).ok()
        }
        12 | 13 => {
            let length = usize::try_from(be_u32(data, 4).ok()?).ok()?;
            let table = data.get(..length)?;
            let groups = usize::try_from(be_u32(table, 12).ok()?).ok()?;
            let groups_end = groups.checked_mul(12)?.checked_add(16)?;
            table.get(..groups_end)?;
            let mut low = 0usize;
            let mut high = groups;
            while low < high {
                let mid = low + (high - low) / 2;
                let offset = 16 + mid * 12;
                let start = be_u32(table, offset).ok()?;
                let end = be_u32(table, offset + 4).ok()?;
                if code < start {
                    high = mid;
                } else if code > end {
                    low = mid + 1;
                } else {
                    let first_gid = be_u32(table, offset + 8).ok()?;
                    let gid = if format == 12 {
                        first_gid.checked_add(code - start)?
                    } else {
                        first_gid
                    };
                    return u16::try_from(gid).ok();
                }
            }
            None
        }
        _ => None,
    }
}

fn cmap_format_4(data: &[u8], code: u32) -> Option<u16> {
    let code = u16::try_from(code).ok()?;
    let length = usize::from(be_u16(data, 2).ok()?);
    let table = data.get(..length)?;
    let seg_count_x2 = usize::from(be_u16(table, 6).ok()?);
    if seg_count_x2 == 0 || seg_count_x2 % 2 != 0 {
        return None;
    }
    let seg_count = seg_count_x2 / 2;
    let end_codes = 14usize;
    let start_codes = end_codes.checked_add(seg_count * 2)?.checked_add(2)?;
    let deltas = start_codes.checked_add(seg_count * 2)?;
    let range_offsets = deltas.checked_add(seg_count * 2)?;
    table.get(..range_offsets.checked_add(seg_count * 2)?)?;
    for segment in 0..seg_count {
        let end = be_u16(table, end_codes + segment * 2).ok()?;
        if code > end {
            continue;
        }
        let start = be_u16(table, start_codes + segment * 2).ok()?;
        if code < start {
            return None;
        }
        let delta = be_u16(table, deltas + segment * 2).ok()?;
        let range_offset_pos = range_offsets + segment * 2;
        let range_offset = usize::from(be_u16(table, range_offset_pos).ok()?);
        if range_offset == 0 {
            return Some(code.wrapping_add(delta));
        }
        let glyph_pos = range_offset_pos
            .checked_add(range_offset)?
            .checked_add(usize::from(code - start) * 2)?;
        let glyph = be_u16(table, glyph_pos).ok()?;
        return Some(if glyph == 0 {
            0
        } else {
            glyph.wrapping_add(delta)
        });
    }
    None
}

fn read_u8(
    data: &[u8],
    cursor: &mut usize,
    context: &'static str,
) -> Result<u8, FontProgramError> {
    let byte = *data
        .get(*cursor)
        .ok_or_else(|| FontProgramError::truncated(context))?;
    *cursor += 1;
    Ok(byte)
}

fn read_u16(
    data: &[u8],
    cursor: &mut usize,
    context: &'static str,
) -> Result<u16, FontProgramError> {
    let bytes = data
        .get(*cursor..cursor.saturating_add(2))
        .ok_or_else(|| FontProgramError::truncated(context))?;
    *cursor += 2;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn be_u16(data: &[u8], offset: usize) -> Result<u16, FontProgramError> {
    let bytes = data
        .get(offset..offset.saturating_add(2))
        .ok_or_else(|| FontProgramError::truncated("big-endian u16"))?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn be_u32(data: &[u8], offset: usize) -> Result<u32, FontProgramError> {
    let bytes = data
        .get(offset..offset.saturating_add(4))
        .ok_or_else(|| FontProgramError::truncated("big-endian u32"))?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

// Constants are the tables defined by the CFF and TrueType specifications.
// Keeping them local avoids dependencies and makes SID/name recovery exact.
include!("font_program_tables.rs");

#[cfg(test)]
mod tests {
    use super::*;

    fn push_index(out: &mut Vec<u8>, objects: &[&[u8]]) {
        out.extend_from_slice(&(objects.len() as u16).to_be_bytes());
        if objects.is_empty() {
            return;
        }
        out.push(1); // fixtures are deliberately small enough for offSize 1
        let mut offset = 1usize;
        out.push(offset as u8);
        for object in objects {
            offset += object.len();
            out.push(offset as u8);
        }
        for object in objects {
            out.extend_from_slice(object);
        }
    }

    fn dict_offset(out: &mut Vec<u8>, offset: usize, operator: u8) {
        out.push(29);
        out.extend_from_slice(&(offset as i32).to_be_bytes());
        out.push(operator);
    }

    fn cff_fixture(charset: Vec<u8>, encoding: Vec<u8>, cid: bool) -> Vec<u8> {
        let mut name_index = Vec::new();
        push_index(&mut name_index, &[b"Test"]);
        let custom = [
            b"noteheads.s2".as_slice(),
            b"clefs.G".as_slice(),
            b"accidentals.sharp".as_slice(),
        ];
        let mut string_index = Vec::new();
        push_index(&mut string_index, &custom);
        let global_subrs = [0u8, 0];
        let top_len = 18 + if cid { 5 } else { 0 };
        let top_index_len = 2 + 1 + 2 + top_len;
        let charset_offset = 4 + name_index.len() + top_index_len + string_index.len() + 2;
        let encoding_offset = charset_offset + charset.len();
        let charstrings_offset = encoding_offset + encoding.len();

        let mut top = Vec::new();
        if cid {
            // Registry SID, Ordering SID, supplement, ROS operator.
            top.extend_from_slice(&[139, 139, 139, 12, 30]);
        }
        dict_offset(&mut top, charset_offset, 15);
        dict_offset(&mut top, encoding_offset, 16);
        dict_offset(&mut top, charstrings_offset, 17);

        let mut out = vec![1, 0, 4, 4];
        out.extend_from_slice(&name_index);
        push_index(&mut out, &[&top]);
        out.extend_from_slice(&string_index);
        out.extend_from_slice(&global_subrs);
        out.extend_from_slice(&charset);
        out.extend_from_slice(&encoding);
        push_index(&mut out, &[&[14], &[14], &[14], &[14]]);
        out
    }

    fn charset_format_0() -> Vec<u8> {
        let mut out = vec![0];
        for sid in 391u16..=393 {
            out.extend_from_slice(&sid.to_be_bytes());
        }
        out
    }

    #[test]
    fn cff_format_0_names_and_encoding() {
        let cff = cff_fixture(charset_format_0(), vec![0, 3, 10, 11, 12], false);
        let font = parse_cff(&cff).unwrap();
        assert_eq!(font.kind, FontProgramKind::Cff);
        assert_eq!(font.name_status, FontNameStatus::GlyphNames);
        assert_eq!(font.glyph_name(10), Some("noteheads.s2"));
        assert_eq!(font.glyph_name(11), Some("clefs.G"));
        assert_eq!(font.glyph_name(12), Some("accidentals.sharp"));
        assert_eq!(font.glyph_name(13), None);
    }

    #[test]
    fn predefined_name_tables_are_complete() {
        assert_eq!(CFF_STANDARD_STRINGS.len(), 391);
        assert_eq!(MACINTOSH_GLYPH_NAMES.len(), 258);
        assert_eq!(EXPERT_CHARSET.len(), 166);
        assert_eq!(EXPERT_SUBSET_CHARSET.len(), 87);
        assert_eq!(resolve_cff_sid(34, &[]), Some("A"));
    }

    #[test]
    fn cff_format_1_charset_encoding_and_supplement() {
        let mut charset = vec![1];
        charset.extend_from_slice(&391u16.to_be_bytes());
        charset.push(2);
        let mut encoding = vec![0x81, 1, 40, 1, 1, 99];
        encoding.extend_from_slice(&393u16.to_be_bytes());
        let font = parse_cff(&cff_fixture(charset, encoding, false)).unwrap();
        assert_eq!(font.glyph_name(40), Some("noteheads.s2"));
        assert_eq!(font.glyph_name(41), Some("clefs.G"));
        assert_eq!(font.glyph_name(99), Some("accidentals.sharp"));
    }

    #[test]
    fn cff_format_2_charset() {
        let mut charset = vec![2];
        charset.extend_from_slice(&391u16.to_be_bytes());
        charset.extend_from_slice(&2u16.to_be_bytes());
        let font = parse_cff(&cff_fixture(charset, vec![0, 3, 20, 21, 22], false)).unwrap();
        assert_eq!(font.glyph_name(20), Some("noteheads.s2"));
        assert_eq!(font.glyph_name(22), Some("accidentals.sharp"));
    }

    #[test]
    fn cid_cff_reports_cids_not_names() {
        let font = parse_cff(&cff_fixture(
            charset_format_0(),
            vec![0, 3, 10, 11, 12],
            true,
        ))
        .unwrap();
        assert_eq!(font.name_status, FontNameStatus::Cids);
        assert_eq!(font.glyph_name_for_gid(1), None);
        assert_eq!(font.glyph_name(10), None);
    }

    fn cmap_format_0(entries: &[(u8, u8)]) -> Vec<u8> {
        let mut subtable = Vec::with_capacity(262);
        subtable.extend_from_slice(&0u16.to_be_bytes());
        subtable.extend_from_slice(&262u16.to_be_bytes());
        subtable.extend_from_slice(&0u16.to_be_bytes());
        let mut glyphs = [0u8; 256];
        for (code, gid) in entries {
            glyphs[usize::from(*code)] = *gid;
        }
        subtable.extend_from_slice(&glyphs);
        cmap_with_subtable(3, 1, subtable)
    }

    fn cmap_with_subtable(platform: u16, encoding: u16, subtable: Vec<u8>) -> Vec<u8> {
        let mut cmap = Vec::new();
        cmap.extend_from_slice(&0u16.to_be_bytes());
        cmap.extend_from_slice(&1u16.to_be_bytes());
        cmap.extend_from_slice(&platform.to_be_bytes());
        cmap.extend_from_slice(&encoding.to_be_bytes());
        cmap.extend_from_slice(&12u32.to_be_bytes());
        cmap.extend_from_slice(&subtable);
        cmap
    }

    fn post_format_2() -> Vec<u8> {
        let mut post = vec![0u8; 32];
        post[..4].copy_from_slice(&0x0002_0000u32.to_be_bytes());
        post.extend_from_slice(&3u16.to_be_bytes());
        post.extend_from_slice(&0u16.to_be_bytes());
        post.extend_from_slice(&258u16.to_be_bytes());
        post.extend_from_slice(&36u16.to_be_bytes()); // Macintosh standard "A"
        post.push(12);
        post.extend_from_slice(b"noteheads.s2");
        post
    }

    fn sfnt(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"\0\x01\0\0");
        out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        let mut offset = 12 + tables.len() * 16;
        for (tag, table) in tables {
            out.extend_from_slice(tag);
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(&(offset as u32).to_be_bytes());
            out.extend_from_slice(&(table.len() as u32).to_be_bytes());
            offset += table.len();
        }
        for (_, table) in tables {
            out.extend_from_slice(table);
        }
        out
    }

    #[test]
    fn sfnt_post_2_and_cmap_recover_names() {
        let bytes = sfnt(&[
            (*b"post", post_format_2()),
            (*b"cmap", cmap_format_0(&[(7, 1), (65, 2)])),
        ]);
        let font = parse_sfnt(&bytes, FontProgramKind::TrueType).unwrap();
        assert_eq!(font.name_status, FontNameStatus::GlyphNames);
        assert_eq!(font.cmap_gid(7), Some(1));
        assert_eq!(font.glyph_name(7), Some("noteheads.s2"));
        assert_eq!(font.glyph_name(65), Some("A"));
    }

    #[test]
    fn differences_win_without_changing_text_decode() {
        use crate::page::{BaseEncoding, FontEncoding, FontResource};

        let bytes = sfnt(&[
            (*b"post", post_format_2()),
            (*b"cmap", cmap_format_0(&[(7, 1), (65, 2)])),
        ]);
        let program = parse_sfnt(&bytes, FontProgramKind::TrueType).unwrap();
        let mut differences = HashMap::new();
        differences.insert(7, "explicit.override".to_owned());
        let font = FontResource {
            subtype: "TrueType".to_owned(),
            base_font: "Fixture".to_owned(),
            encoding: FontEncoding::Custom(BaseEncoding::WinAnsi, differences),
            widths: Vec::new(),
            first_char: 0,
            last_char: 255,
            to_unicode: None,
            default_width: 500.0,
            cid_widths: None,
            font_program: Some(program),
        };
        assert_eq!(font.glyph_name(7), Some("explicit.override"));
        assert_eq!(font.glyph_name(65), Some("A"));
        assert_eq!(crate::font::decode_text(&font, b"A"), "A");
    }

    #[test]
    fn cmap_without_post_reports_names_unavailable() {
        let bytes = sfnt(&[(*b"cmap", cmap_format_0(&[(65, 2)]))]);
        let font = parse_sfnt(&bytes, FontProgramKind::TrueType).unwrap();
        assert_eq!(font.name_status, FontNameStatus::Unavailable);
        assert_eq!(font.cmap_gid(65), Some(2));
        assert_eq!(font.glyph_name(65), None);
    }

    #[test]
    fn cmap_formats_4_and_12_lookup() {
        let mut format4 = Vec::new();
        format4.extend_from_slice(&4u16.to_be_bytes());
        format4.extend_from_slice(&32u16.to_be_bytes());
        format4.extend_from_slice(&0u16.to_be_bytes());
        format4.extend_from_slice(&4u16.to_be_bytes()); // two segments
        format4.extend_from_slice(&[0; 6]);
        format4.extend_from_slice(&65u16.to_be_bytes());
        format4.extend_from_slice(&0xffffu16.to_be_bytes());
        format4.extend_from_slice(&0u16.to_be_bytes());
        format4.extend_from_slice(&65u16.to_be_bytes());
        format4.extend_from_slice(&0xffffu16.to_be_bytes());
        format4.extend_from_slice(&0xffc1u16.to_be_bytes()); // 65 + delta = GID 2
        format4.extend_from_slice(&1u16.to_be_bytes());
        format4.extend_from_slice(&[0; 4]);
        let cmap4 = Cmap::parse(&cmap_with_subtable(3, 1, format4)).unwrap();
        assert_eq!(cmap4.lookup(65), Some(2));
        assert_eq!(cmap4.lookup(66), None);

        let mut format12 = Vec::new();
        format12.extend_from_slice(&12u16.to_be_bytes());
        format12.extend_from_slice(&0u16.to_be_bytes());
        format12.extend_from_slice(&28u32.to_be_bytes());
        format12.extend_from_slice(&0u32.to_be_bytes());
        format12.extend_from_slice(&1u32.to_be_bytes());
        format12.extend_from_slice(&0x1d11eu32.to_be_bytes());
        format12.extend_from_slice(&0x1d11fu32.to_be_bytes());
        format12.extend_from_slice(&7u32.to_be_bytes());
        let cmap12 = Cmap::parse(&cmap_with_subtable(3, 10, format12)).unwrap();
        assert_eq!(cmap12.lookup(0x1d11e), Some(7));
        assert_eq!(cmap12.lookup(0x1d11f), Some(8));
    }

    #[test]
    fn malformed_font_programs_never_panic() {
        assert_eq!(
            parse_cff(&[1, 0]).unwrap_err().kind,
            FontProgramErrorKind::Truncated
        );
        let valid_cff = cff_fixture(charset_format_0(), vec![0, 3, 10, 11, 12], false);
        for end in 0..valid_cff.len() {
            let _ = parse_cff(&valid_cff[..end]);
        }
        for index in 0..valid_cff.len() {
            for mask in [0x01, 0x80, 0xff] {
                let mut corrupted = valid_cff.clone();
                corrupted[index] ^= mask;
                let _ = parse_cff(&corrupted);
            }
        }

        let valid_sfnt = sfnt(&[
            (*b"post", post_format_2()),
            (*b"cmap", cmap_format_0(&[(7, 1), (65, 2)])),
        ]);
        for end in 0..valid_sfnt.len() {
            let _ = parse_sfnt(&valid_sfnt[..end], FontProgramKind::TrueType);
        }
        for index in (0..valid_sfnt.len()).step_by(7) {
            let mut corrupted = valid_sfnt.clone();
            corrupted[index] ^= 0xff;
            let _ = parse_sfnt(&corrupted, FontProgramKind::TrueType);
        }
    }
}
