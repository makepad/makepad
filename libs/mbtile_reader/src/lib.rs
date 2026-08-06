//! Minimal SQLite file format reader and writer for MBTiles tile extraction.
//!
//! Implements just enough of the SQLite on-disk format to:
//! - Parse the 100-byte database header
//! - Walk B-tree pages (table interior + leaf)
//! - Decode varint-encoded records
//! - Follow overflow page chains for large blobs
//! - Scan the sqlite_master table to find table root pages
//! - Scan the tiles table and look up tiles by (zoom, column, row)
//! - Stream a sorted set of tiles into a new MBTiles database
//!
//! Reference: <https://www.sqlite.org/fileformat.html>
//! Reference implementation studied: turso/libsql core/storage/sqlite3_ondisk.rs

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

mod codec;
mod mkmap;
mod writer;
pub use codec::{
    compress_tile, compression_metadata_rows, TileCodec, TileCompression,
    COMPRESSION_DICT_METADATA_KEY, COMPRESSION_METADATA_KEY,
};
pub use mkmap::{mkmap_tile_id, MkmapReader, TileArchiveReader};
pub use writer::{MbtilesWriter, MbtilesWriterStats, WriterValue};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    InvalidMagic,
    InvalidPageSize,
    InvalidPageType(u8),
    CorruptVarint,
    CorruptCell(&'static str),
    CorruptRecord(&'static str),
    TableNotFound(&'static str),
    Utf16Decode,
    InvalidInput(String),
    InvalidWriterState(&'static str),
    Codec(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::InvalidMagic => write!(f, "not a SQLite database"),
            Error::InvalidPageSize => write!(f, "invalid page size"),
            Error::InvalidPageType(t) => write!(f, "invalid page type: {t}"),
            Error::CorruptVarint => write!(f, "corrupt varint"),
            Error::CorruptCell(msg) => write!(f, "corrupt cell: {msg}"),
            Error::CorruptRecord(msg) => write!(f, "corrupt record: {msg}"),
            Error::TableNotFound(name) => write!(f, "table not found: {name}"),
            Error::Utf16Decode => write!(f, "invalid UTF-16 text"),
            Error::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            Error::InvalidWriterState(msg) => write!(f, "invalid writer state: {msg}"),
            Error::Codec(msg) => write!(f, "codec: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------------------
// SQLite varint (big-endian, 1-9 bytes, MSB continuation bit)
// ---------------------------------------------------------------------------

/// Read a SQLite varint from `buf`. Returns (value, bytes_consumed).
fn read_varint(buf: &[u8]) -> Result<(u64, usize)> {
    let mut v: u64 = 0;
    for i in 0..8 {
        let c = *buf.get(i).ok_or(Error::CorruptVarint)?;
        v = (v << 7) | (c & 0x7f) as u64;
        if c & 0x80 == 0 {
            return Ok((v, i + 1));
        }
    }
    // 9th byte: full 8 bits, no continuation
    let c = *buf.get(8).ok_or(Error::CorruptVarint)?;
    v = (v << 8) | c as u64;
    Ok((v, 9))
}

// ---------------------------------------------------------------------------
// Reading big-endian integers of various widths
// ---------------------------------------------------------------------------

fn read_be_u16(buf: &[u8]) -> u16 {
    u16::from_be_bytes([buf[0], buf[1]])
}

fn read_be_u32(buf: &[u8]) -> u32 {
    u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]])
}

fn read_be_i8(buf: &[u8]) -> i64 {
    buf[0] as i8 as i64
}

fn read_be_i16(buf: &[u8]) -> i64 {
    i16::from_be_bytes([buf[0], buf[1]]) as i64
}

fn read_be_i24(buf: &[u8]) -> i64 {
    let sign = if buf[0] & 0x80 != 0 { 0xFF } else { 0x00 };
    i32::from_be_bytes([sign, buf[0], buf[1], buf[2]]) as i64
}

fn read_be_i32(buf: &[u8]) -> i64 {
    i32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as i64
}

fn read_be_i48(buf: &[u8]) -> i64 {
    let sign = if buf[0] & 0x80 != 0 { 0xFF } else { 0x00 };
    i64::from_be_bytes([sign, sign, buf[0], buf[1], buf[2], buf[3], buf[4], buf[5]])
}

fn read_be_i64(buf: &[u8]) -> i64 {
    i64::from_be_bytes([
        buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
    ])
}

fn read_be_f64(buf: &[u8]) -> f64 {
    f64::from_be_bytes([
        buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
    ])
}

// ---------------------------------------------------------------------------
// Database header (first 100 bytes of page 1)
// ---------------------------------------------------------------------------

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

#[derive(Debug, Clone)]
pub struct DbHeader {
    pub page_size: u32,
    pub reserved_space: u8,
    pub database_size_pages: u32,
    pub text_encoding: TextEncoding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

fn parse_db_header(buf: &[u8; 100]) -> Result<DbHeader> {
    if &buf[0..16] != SQLITE_MAGIC {
        return Err(Error::InvalidMagic);
    }
    let raw_page_size = read_be_u16(&buf[16..18]);
    let page_size = match raw_page_size {
        1 => 65536u32,
        n if n >= 512 && n.is_power_of_two() => n as u32,
        _ => return Err(Error::InvalidPageSize),
    };
    let reserved_space = buf[20];
    let database_size_pages = read_be_u32(&buf[28..32]);
    let text_encoding = match read_be_u32(&buf[56..60]) {
        1 => TextEncoding::Utf8,
        2 => TextEncoding::Utf16Le,
        3 => TextEncoding::Utf16Be,
        _ => TextEncoding::Utf8,
    };
    Ok(DbHeader {
        page_size,
        reserved_space,
        database_size_pages,
        text_encoding,
    })
}

// ---------------------------------------------------------------------------
// Page types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageType {
    IndexInterior, // 2
    TableInterior, // 5
    IndexLeaf,     // 10
    TableLeaf,     // 13
}

impl PageType {
    fn from_byte(b: u8) -> Result<Self> {
        match b {
            2 => Ok(PageType::IndexInterior),
            5 => Ok(PageType::TableInterior),
            10 => Ok(PageType::IndexLeaf),
            13 => Ok(PageType::TableLeaf),
            _ => Err(Error::InvalidPageType(b)),
        }
    }

    fn is_interior(self) -> bool {
        matches!(self, PageType::IndexInterior | PageType::TableInterior)
    }

    fn header_size(self) -> usize {
        if self.is_interior() {
            12
        } else {
            8
        }
    }
}

// ---------------------------------------------------------------------------
// Overflow threshold calculation (matches SQLite spec exactly)
// ---------------------------------------------------------------------------

fn payload_overflow_threshold_max(page_type: PageType, usable_size: usize) -> usize {
    match page_type {
        PageType::IndexInterior | PageType::IndexLeaf => ((usable_size - 12) * 64 / 255) - 23,
        PageType::TableInterior | PageType::TableLeaf => usable_size - 35,
    }
}

fn payload_overflow_threshold_min(_page_type: PageType, usable_size: usize) -> usize {
    ((usable_size - 12) * 32 / 255) - 23
}

/// Returns (overflows, local_payload_size_including_overflow_ptr).
fn payload_overflows(
    payload_size: usize,
    max_local: usize,
    min_local: usize,
    usable_size: usize,
) -> (bool, usize) {
    if payload_size <= max_local {
        return (false, payload_size);
    }
    let mut space = min_local + (payload_size - min_local) % (usable_size - 4);
    if space > max_local {
        space = min_local;
    }
    // +4 for the overflow page pointer
    (true, space + 4)
}

// ---------------------------------------------------------------------------
// Record value
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Integer(i64),
    Float(f64),
    Blob(Vec<u8>),
    Text(String),
}

impl Value {
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Value::Integer(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_blob(&self) -> Option<&[u8]> {
        match self {
            Value::Blob(b) => Some(b),
            _ => None,
        }
    }

    pub fn into_blob(self) -> Option<Vec<u8>> {
        match self {
            Value::Blob(b) => Some(b),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Serial type decoding
// ---------------------------------------------------------------------------

/// Returns (type_description, content_size_bytes) for a serial type code.
fn serial_type_content_size(serial_type: u64) -> usize {
    match serial_type {
        0 => 0,                                                // NULL
        1 => 1,                                                // i8
        2 => 2,                                                // i16
        3 => 3,                                                // i24
        4 => 4,                                                // i32
        5 => 6,                                                // i48
        6 => 8,                                                // i64
        7 => 8,                                                // f64
        8 | 9 => 0,                                            // const 0 / const 1
        10 | 11 => 0,                                          // reserved
        n if n >= 12 && n % 2 == 0 => ((n - 12) / 2) as usize, // blob
        n if n >= 13 && n % 2 == 1 => ((n - 13) / 2) as usize, // text
        _ => 0,
    }
}

fn decode_value(serial_type: u64, buf: &[u8], encoding: TextEncoding) -> Result<Value> {
    match serial_type {
        0 => Ok(Value::Null),
        1 => Ok(Value::Integer(read_be_i8(buf))),
        2 => Ok(Value::Integer(read_be_i16(buf))),
        3 => Ok(Value::Integer(read_be_i24(buf))),
        4 => Ok(Value::Integer(read_be_i32(buf))),
        5 => Ok(Value::Integer(read_be_i48(buf))),
        6 => Ok(Value::Integer(read_be_i64(buf))),
        7 => Ok(Value::Float(read_be_f64(buf))),
        8 => Ok(Value::Integer(0)),
        9 => Ok(Value::Integer(1)),
        n if n >= 12 && n % 2 == 0 => {
            let size = ((n - 12) / 2) as usize;
            Ok(Value::Blob(buf[..size].to_vec()))
        }
        n if n >= 13 && n % 2 == 1 => {
            let size = ((n - 13) / 2) as usize;
            let text = decode_text(&buf[..size], encoding)?;
            Ok(Value::Text(text))
        }
        _ => Ok(Value::Null),
    }
}

fn decode_text(buf: &[u8], encoding: TextEncoding) -> Result<String> {
    match encoding {
        TextEncoding::Utf8 => Ok(String::from_utf8_lossy(buf).into_owned()),
        TextEncoding::Utf16Le => {
            if buf.len() % 2 != 0 {
                return Err(Error::Utf16Decode);
            }
            let u16s: Vec<u16> = buf
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16(&u16s).map_err(|_| Error::Utf16Decode)
        }
        TextEncoding::Utf16Be => {
            if buf.len() % 2 != 0 {
                return Err(Error::Utf16Decode);
            }
            let u16s: Vec<u16> = buf
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16(&u16s).map_err(|_| Error::Utf16Decode)
        }
    }
}

// ---------------------------------------------------------------------------
// Record parsing (from a fully-assembled payload buffer)
// ---------------------------------------------------------------------------

fn parse_record(payload: &[u8], encoding: TextEncoding) -> Result<Vec<Value>> {
    if payload.is_empty() {
        return Ok(Vec::new());
    }
    let (header_size, hdr_varint_len) = read_varint(payload)?;
    let header_size = header_size as usize;
    if header_size > payload.len() {
        return Err(Error::CorruptRecord("header size exceeds payload"));
    }

    // Parse serial types from the header
    let mut serial_types = Vec::new();
    let mut hpos = hdr_varint_len;
    while hpos < header_size {
        let (st, n) = read_varint(&payload[hpos..])?;
        serial_types.push(st);
        hpos += n;
    }

    // Decode values from the body
    let mut data_pos = header_size;
    let mut values = Vec::with_capacity(serial_types.len());
    for &st in &serial_types {
        let size = serial_type_content_size(st);
        if data_pos + size > payload.len() {
            return Err(Error::CorruptRecord("value extends past payload"));
        }
        let val = decode_value(st, &payload[data_pos..], encoding)?;
        values.push(val);
        data_pos += size;
    }
    Ok(values)
}

// ---------------------------------------------------------------------------
// MBTiles reader
// ---------------------------------------------------------------------------

pub struct MbtilesReader {
    file: File,
    header: DbHeader,
    usable_size: usize,
    /// Root page number of the `tiles` table (1-based)
    tiles_root_page: u32,
    /// Root page number of the `metadata` table (1-based)
    metadata_root_page: u32,
    /// Root page number of the standard `(zoom_level, tile_column, tile_row)`
    /// tile index (1-based), if present.
    tile_index_root_page: Option<u32>,
    /// Makepad-authored files use deterministic rowids for direct B-tree lookup.
    makepad_block_rowids: bool,
    /// Tile payload codec, parsed once from metadata (absent = gzip).
    tile_codec: TileCodec,
    /// Small, bounded cache for table B-tree pages reused by adjacent lookups.
    btree_page_cache: HashMap<u32, Vec<u8>>,
    btree_page_cache_order: VecDeque<u32>,
}

const BTREE_PAGE_CACHE_CAPACITY: usize = 32;

/// A single tile from the mbtiles database.
#[derive(Debug, Clone)]
pub struct Tile {
    pub zoom_level: i64,
    pub tile_column: i64,
    pub tile_row: i64,
    pub tile_data: Vec<u8>,
}

impl MbtilesReader {
    /// Open an MBTiles file and parse the schema to find table root pages.
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;

        // Read page 1 (contains 100-byte db header + sqlite_master btree)
        let mut header_buf = [0u8; 100];
        file.read_exact(&mut header_buf)?;
        let header = parse_db_header(&header_buf)?;
        let usable_size = header.page_size as usize - header.reserved_space as usize;

        let mut reader = MbtilesReader {
            file,
            header,
            usable_size,
            tiles_root_page: 0,
            metadata_root_page: 0,
            tile_index_root_page: None,
            makepad_block_rowids: false,
            tile_codec: TileCodec::gzip(),
            btree_page_cache: HashMap::new(),
            btree_page_cache_order: VecDeque::new(),
        };

        // Scan sqlite_master (always rooted at page 1) to find our tables
        reader.scan_schema()?;

        if reader.tiles_root_page == 0 {
            return Err(Error::TableNotFound("tiles"));
        }

        let metadata = reader.get_metadata()?;
        reader.makepad_block_rowids = metadata
            .get("makepad_rowid_scheme")
            .is_some_and(|value| value == "block-v1-xyz");
        reader.tile_codec = TileCodec::from_metadata(&metadata)?;

        Ok(reader)
    }

    /// Read a full page from the database file. Pages are 1-indexed.
    fn read_page(&mut self, page_num: u32) -> Result<Vec<u8>> {
        let page_size = self.header.page_size as u64;
        let offset = (page_num as u64 - 1) * page_size;
        self.file.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; page_size as usize];
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    fn read_btree_page(&mut self, page_num: u32) -> Result<Vec<u8>> {
        if let Some(page) = self.btree_page_cache.get(&page_num) {
            return Ok(page.clone());
        }
        let page = self.read_page(page_num)?;
        if self.btree_page_cache.len() == BTREE_PAGE_CACHE_CAPACITY {
            if let Some(evicted) = self.btree_page_cache_order.pop_front() {
                self.btree_page_cache.remove(&evicted);
            }
        }
        self.btree_page_cache.insert(page_num, page.clone());
        self.btree_page_cache_order.push_back(page_num);
        Ok(page)
    }

    /// Assemble full payload from a cell, following overflow pages if necessary.
    fn assemble_payload(
        &mut self,
        local_payload: &[u8],
        total_payload_size: usize,
        first_overflow_page: Option<u32>,
    ) -> Result<Vec<u8>> {
        if first_overflow_page.is_none() || local_payload.len() >= total_payload_size {
            // No overflow - the whole payload is local
            return Ok(local_payload[..total_payload_size.min(local_payload.len())].to_vec());
        }

        let mut payload = Vec::with_capacity(total_payload_size);
        // Local portion (excluding the 4-byte overflow pointer at the end)
        let local_data_len = local_payload.len() - 4;
        payload.extend_from_slice(&local_payload[..local_data_len]);

        let mut overflow_page = first_overflow_page.unwrap();
        let overflow_content_size = self.usable_size - 4; // 4 bytes for next-page pointer

        while payload.len() < total_payload_size && overflow_page != 0 {
            let page = self.read_page(overflow_page)?;
            let next_page = read_be_u32(&page[0..4]);
            let remaining = total_payload_size - payload.len();
            let to_copy = remaining.min(overflow_content_size);
            payload.extend_from_slice(&page[4..4 + to_copy]);
            overflow_page = next_page;
        }

        if payload.len() < total_payload_size {
            return Err(Error::CorruptCell("overflow chain too short"));
        }
        Ok(payload)
    }

    /// Parse a table leaf cell at position `pos` within a page buffer.
    /// Returns (rowid, payload_bytes, total_payload_size, first_overflow_page).
    fn parse_table_leaf_cell<'a>(
        &self,
        page: &'a [u8],
        pos: usize,
    ) -> Result<(i64, &'a [u8], usize, Option<u32>)> {
        let mut off = pos;
        let (payload_size, n) = read_varint(&page[off..])?;
        off += n;
        let (rowid, n) = read_varint(&page[off..])?;
        off += n;

        let payload_size = payload_size as usize;
        let page_type = PageType::TableLeaf;
        let max_local = payload_overflow_threshold_max(page_type, self.usable_size);
        let min_local = payload_overflow_threshold_min(page_type, self.usable_size);
        let (overflows, local_size) =
            payload_overflows(payload_size, max_local, min_local, self.usable_size);

        let end = (off + local_size).min(page.len());
        let cell_payload = &page[off..end];

        let first_overflow = if overflows && local_size >= 4 {
            // Last 4 bytes of local area are the overflow page pointer
            let ptr_start = off + local_size - 4;
            if ptr_start + 4 <= page.len() {
                Some(read_be_u32(&page[ptr_start..ptr_start + 4]))
            } else {
                None
            }
        } else {
            None
        };

        Ok((rowid as i64, cell_payload, payload_size, first_overflow))
    }

    /// Parse a table interior cell at position `pos`. Returns (left_child_page, rowid).
    fn parse_table_interior_cell(&self, page: &[u8], pos: usize) -> Result<(u32, i64)> {
        let left_child = read_be_u32(&page[pos..pos + 4]);
        let (rowid, _) = read_varint(&page[pos + 4..])?;
        Ok((left_child, rowid as i64))
    }

    /// Get cell pointer offsets from a page. The `page_header_offset` is 100 for
    /// page 1 (after the db header), 0 for all other pages.
    fn cell_pointers(
        &self,
        page: &[u8],
        page_header_offset: usize,
    ) -> Result<(PageType, Vec<usize>, Option<u32>)> {
        let page_type = PageType::from_byte(page[page_header_offset])?;
        let num_cells = read_be_u16(&page[page_header_offset + 3..]) as usize;

        let rightmost_ptr = if page_type.is_interior() {
            Some(read_be_u32(&page[page_header_offset + 8..]))
        } else {
            None
        };

        let ptr_array_start = page_header_offset + page_type.header_size();
        let mut pointers = Vec::with_capacity(num_cells);
        for i in 0..num_cells {
            let ptr = read_be_u16(&page[ptr_array_start + i * 2..]) as usize;
            pointers.push(ptr);
        }

        Ok((page_type, pointers, rightmost_ptr))
    }

    /// Scan the sqlite_master table (page 1) to find root pages for our tables.
    fn scan_schema(&mut self) -> Result<()> {
        // sqlite_master columns: type, name, tbl_name, rootpage, sql
        self.scan_table_pages(
            1,
            &mut |reader, rowid, local_payload, total_size, overflow_page| {
                let payload = reader.assemble_payload(local_payload, total_size, overflow_page)?;
                let record = parse_record(&payload, reader.header.text_encoding)?;
                if record.len() < 5 {
                    return Ok(());
                }
                let obj_type = record[0].as_text().unwrap_or("");
                let name = record[1].as_text().unwrap_or("");
                let table_name = record[2].as_text().unwrap_or("");
                let root_page = record[3].as_integer().unwrap_or(0) as u32;
                let sql = record[4].as_text().unwrap_or("");

                let _ = rowid;
                match (obj_type, name) {
                    ("table", "tiles") => reader.tiles_root_page = root_page,
                    ("table", "metadata") => reader.metadata_root_page = root_page,
                    _ => {}
                }
                // `tile_index` is the conventional MBTiles spelling. Also
                // accept SQLite's automatic index for a UNIQUE constraint on
                // the tiles table. The latter has a NULL SQL field, hence the
                // name/table checks rather than SQL parsing.
                if obj_type == "index"
                    && table_name == "tiles"
                    && (name == "tile_index"
                        || name.starts_with("sqlite_autoindex_tiles_")
                        || (sql.contains("zoom_level")
                            && sql.contains("tile_column")
                            && sql.contains("tile_row")))
                {
                    reader.tile_index_root_page = Some(root_page);
                }
                Ok(())
            },
        )?;
        Ok(())
    }

    /// Walk all rows of a table btree, calling `callback` for each leaf cell.
    /// The callback receives: (reader, rowid, local_payload, total_payload_size, first_overflow_page).
    fn scan_table_pages(
        &mut self,
        root_page_num: u32,
        callback: &mut dyn FnMut(&mut Self, i64, &[u8], usize, Option<u32>) -> Result<()>,
    ) -> Result<()> {
        // Use an explicit stack to avoid recursion
        let mut page_stack = vec![root_page_num];

        while let Some(page_num) = page_stack.pop() {
            let page = self.read_page(page_num)?;
            let header_offset = if page_num == 1 { 100 } else { 0 };
            let (page_type, cell_ptrs, rightmost_ptr) = self.cell_pointers(&page, header_offset)?;

            match page_type {
                PageType::TableLeaf => {
                    for &ptr in &cell_ptrs {
                        let (rowid, local_payload, total_size, overflow_page) =
                            self.parse_table_leaf_cell(&page, ptr)?;
                        callback(self, rowid, local_payload, total_size, overflow_page)?;
                    }
                }
                PageType::TableInterior => {
                    // Push rightmost child first (it'll be processed last = leftmost first)
                    if let Some(right) = rightmost_ptr {
                        page_stack.push(right);
                    }
                    // Push children in reverse order so leftmost is processed first
                    for &ptr in cell_ptrs.iter().rev() {
                        let (left_child, _rowid) = self.parse_table_interior_cell(&page, ptr)?;
                        page_stack.push(left_child);
                    }
                }
                _ => {
                    // Index pages - skip for table scan
                }
            }
        }
        Ok(())
    }

    /// Find a table row by integer rowid using the table B-tree separators.
    fn find_table_row(&mut self, root_page_num: u32, target: i64) -> Result<Option<Vec<u8>>> {
        let mut page_num = root_page_num;
        loop {
            let page = self.read_btree_page(page_num)?;
            let header_offset = if page_num == 1 { 100 } else { 0 };
            let (page_type, cell_ptrs, rightmost_ptr) =
                self.cell_pointers(&page, header_offset)?;

            match page_type {
                PageType::TableInterior => {
                    let mut child = rightmost_ptr.ok_or(Error::CorruptCell(
                        "table interior page has no rightmost child",
                    ))?;
                    for ptr in cell_ptrs {
                        let (left_child, separator) =
                            self.parse_table_interior_cell(&page, ptr)?;
                        if target <= separator {
                            child = left_child;
                            break;
                        }
                    }
                    page_num = child;
                }
                PageType::TableLeaf => {
                    for ptr in cell_ptrs {
                        let (rowid, local, total, overflow) =
                            self.parse_table_leaf_cell(&page, ptr)?;
                        if rowid == target {
                            return self
                                .assemble_payload(local, total, overflow)
                                .map(Some);
                        }
                        if rowid > target {
                            return Ok(None);
                        }
                    }
                    return Ok(None);
                }
                _ => {
                    return Err(Error::CorruptCell(
                        "table root points at an index B-tree page",
                    ));
                }
            }
        }
    }

    /// Parse an index B-tree cell and assemble its record payload.
    ///
    /// Index interior cells begin with a four-byte left-child page number;
    /// index leaf cells begin directly with the payload-size varint.
    fn parse_index_cell(
        &mut self,
        page: &[u8],
        page_type: PageType,
        pos: usize,
    ) -> Result<(Option<u32>, Vec<u8>)> {
        if !matches!(page_type, PageType::IndexInterior | PageType::IndexLeaf) {
            return Err(Error::CorruptCell("expected an index B-tree cell"));
        }

        let (left_child, mut offset) = if page_type == PageType::IndexInterior {
            let end = pos
                .checked_add(4)
                .ok_or(Error::CorruptCell("index cell offset overflow"))?;
            let bytes = page
                .get(pos..end)
                .ok_or(Error::CorruptCell("truncated index interior cell"))?;
            (Some(read_be_u32(bytes)), end)
        } else {
            (None, pos)
        };

        let (payload_size, varint_len) = read_varint(
            page.get(offset..)
                .ok_or(Error::CorruptCell("index cell starts past page"))?,
        )?;
        offset = offset
            .checked_add(varint_len)
            .ok_or(Error::CorruptCell("index payload offset overflow"))?;
        let payload_size = usize::try_from(payload_size)
            .map_err(|_| Error::CorruptCell("index payload exceeds address space"))?;
        let max_local = payload_overflow_threshold_max(page_type, self.usable_size);
        let min_local = payload_overflow_threshold_min(page_type, self.usable_size);
        let (overflows, local_size) =
            payload_overflows(payload_size, max_local, min_local, self.usable_size);
        let end = offset
            .checked_add(local_size)
            .ok_or(Error::CorruptCell("index local payload offset overflow"))?;
        let local = page
            .get(offset..end)
            .ok_or(Error::CorruptCell("truncated index payload"))?;
        let overflow_page = if overflows {
            if local.len() < 4 {
                return Err(Error::CorruptCell("truncated index overflow pointer"));
            }
            Some(read_be_u32(&local[local.len() - 4..]))
        } else {
            None
        };
        let payload = self.assemble_payload(local, payload_size, overflow_page)?;
        Ok((left_child, payload))
    }

    fn tile_index_key(record: &[Value]) -> Result<([i64; 3], i64)> {
        if record.len() < 4 {
            return Err(Error::CorruptRecord(
                "tile index entry has fewer than four columns",
            ));
        }
        let zoom = record[0]
            .as_integer()
            .ok_or(Error::CorruptRecord("tile index zoom is not an integer"))?;
        let column = record[1]
            .as_integer()
            .ok_or(Error::CorruptRecord("tile index column is not an integer"))?;
        let row = record[2]
            .as_integer()
            .ok_or(Error::CorruptRecord("tile index row is not an integer"))?;
        let table_rowid = record
            .last()
            .and_then(Value::as_integer)
            .ok_or(Error::CorruptRecord(
                "tile index table rowid is not an integer",
            ))?;
        Ok(([zoom, column, row], table_rowid))
    }

    /// Find the tiles-table rowid through a conventional MBTiles composite
    /// index. This keeps arbitrary third-party MBTiles archives streamable
    /// without scanning every row at the requested zoom.
    fn find_tile_rowid_in_index(
        &mut self,
        root_page_num: u32,
        target: [i64; 3],
    ) -> Result<Option<i64>> {
        let mut page_num = root_page_num;
        loop {
            let page = self.read_btree_page(page_num)?;
            let header_offset = if page_num == 1 { 100 } else { 0 };
            let (page_type, cell_ptrs, rightmost_ptr) =
                self.cell_pointers(&page, header_offset)?;

            match page_type {
                PageType::IndexInterior => {
                    let mut next_child = rightmost_ptr.ok_or(Error::CorruptCell(
                        "index interior page has no rightmost child",
                    ))?;
                    for ptr in cell_ptrs {
                        let (left_child, payload) =
                            self.parse_index_cell(&page, page_type, ptr)?;
                        let record = parse_record(&payload, self.header.text_encoding)?;
                        let (key, table_rowid) = Self::tile_index_key(&record)?;
                        match target.cmp(&key) {
                            Ordering::Less => {
                                next_child = left_child.ok_or(Error::CorruptCell(
                                    "index interior cell has no left child",
                                ))?;
                                break;
                            }
                            Ordering::Equal => return Ok(Some(table_rowid)),
                            Ordering::Greater => {}
                        }
                    }
                    page_num = next_child;
                }
                PageType::IndexLeaf => {
                    for ptr in cell_ptrs {
                        let (_, payload) = self.parse_index_cell(&page, page_type, ptr)?;
                        let record = parse_record(&payload, self.header.text_encoding)?;
                        let (key, table_rowid) = Self::tile_index_key(&record)?;
                        match target.cmp(&key) {
                            Ordering::Less => return Ok(None),
                            Ordering::Equal => return Ok(Some(table_rowid)),
                            Ordering::Greater => {}
                        }
                    }
                    return Ok(None);
                }
                _ => {
                    return Err(Error::CorruptCell(
                        "tile index root points at a table B-tree page",
                    ));
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Get all metadata key-value pairs.
    pub fn get_metadata(&mut self) -> Result<HashMap<String, String>> {
        if self.metadata_root_page == 0 {
            return Ok(HashMap::new());
        }
        let mut metadata = HashMap::new();
        let root = self.metadata_root_page;
        self.scan_table_pages(root, &mut |reader, _rowid, local, total, overflow| {
            let payload = reader.assemble_payload(local, total, overflow)?;
            let record = parse_record(&payload, reader.header.text_encoding)?;
            if record.len() >= 2 {
                if let (Some(key), Some(val)) = (record[0].as_text(), record[1].as_text()) {
                    metadata.insert(key.to_string(), val.to_string());
                }
            }
            Ok(())
        })?;
        Ok(metadata)
    }

    /// Whether `get_tile` can seek directly to one deterministic tile row.
    ///
    /// Files emitted by [`MbtilesWriter`] use deterministic table rowids.
    /// Conventional MBTiles files use their composite tile index. Only malformed
    /// or unusually index-free third-party files require a compatibility scan.
    pub fn supports_direct_tile_lookup(&self) -> bool {
        self.makepad_block_rowids || self.tile_index_root_page.is_some()
    }

    /// Get a single tile by (zoom_level, tile_column, tile_row).
    /// Returns the raw tile_data blob (typically gzip-compressed PBF).
    pub fn get_tile(&mut self, zoom: i64, column: i64, row: i64) -> Result<Option<Vec<u8>>> {
        if self.makepad_block_rowids {
            let Ok(zoom_u8) = u8::try_from(zoom) else {
                return Ok(None);
            };
            let Ok(column_u32) = u32::try_from(column) else {
                return Ok(None);
            };
            let Ok(tms_row_u32) = u32::try_from(row) else {
                return Ok(None);
            };
            let Some(axis) = 1_u32.checked_shl(u32::from(zoom_u8)) else {
                return Ok(None);
            };
            if column_u32 >= axis || tms_row_u32 >= axis {
                return Ok(None);
            }
            let xyz_row = axis - 1 - tms_row_u32;
            let Some(rowid) = writer::tile_rowid_xyz(zoom_u8, column_u32, xyz_row) else {
                return Ok(None);
            };
            let root = self.tiles_root_page;
            let Some(payload) = self.find_table_row(root, rowid)? else {
                return Ok(None);
            };
            let record = parse_record(&payload, self.header.text_encoding)?;
            if record.len() < 4
                || record[0].as_integer() != Some(zoom)
                || record[1].as_integer() != Some(column)
                || record[2].as_integer() != Some(row)
            {
                return Err(Error::CorruptRecord(
                    "deterministic rowid points at the wrong tile",
                ));
            }
            return Ok(record.into_iter().nth(3).and_then(Value::into_blob));
        }

        if let Some(index_root) = self.tile_index_root_page {
            let Some(table_rowid) =
                self.find_tile_rowid_in_index(index_root, [zoom, column, row])?
            else {
                return Ok(None);
            };
            let root = self.tiles_root_page;
            let Some(payload) = self.find_table_row(root, table_rowid)? else {
                return Err(Error::CorruptRecord(
                    "tile index points at a missing table row",
                ));
            };
            let record = parse_record(&payload, self.header.text_encoding)?;
            if record.len() < 4
                || record[0].as_integer() != Some(zoom)
                || record[1].as_integer() != Some(column)
                || record[2].as_integer() != Some(row)
            {
                return Err(Error::CorruptRecord(
                    "tile index points at the wrong tile",
                ));
            }
            return Ok(record.into_iter().nth(3).and_then(Value::into_blob));
        }

        let mut result: Option<Vec<u8>> = None;
        let root = self.tiles_root_page;
        self.scan_table_pages(root, &mut |reader, _rowid, local, total, overflow| {
            if result.is_some() {
                return Ok(());
            }
            // tiles columns: zoom_level, tile_column, tile_row, tile_data
            // We need to parse just enough of the record to check the first 3 columns.
            // For efficiency, first parse just the header to check integer columns
            // before assembling the full payload (which may involve overflow).
            let record_check = parse_record_header_and_ints(local, reader.header.text_encoding)?;
            if let Some((z, c, r)) = record_check {
                if z == zoom && c == column && r == row {
                    let payload = reader.assemble_payload(local, total, overflow)?;
                    let record = parse_record(&payload, reader.header.text_encoding)?;
                    if record.len() >= 4 {
                        result = record.into_iter().nth(3).and_then(|v| v.into_blob());
                    }
                }
            }
            Ok(())
        })?;
        Ok(result)
    }

    /// The archive's tile payload codec (parsed once from metadata at open;
    /// archives without a `compression` metadata row are gzip).
    pub fn tile_codec(&self) -> &TileCodec {
        &self.tile_codec
    }

    /// Decode a stored tile payload to raw bytes using the archive's codec.
    ///
    /// For gzip archives this sniffs gzip/zlib magic and passes unknown
    /// payloads through untouched (the historical behavior); for brotli
    /// archives it brotli-decodes, attaching the shared dictionary when the
    /// archive declares one.
    pub fn decode_tile(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        self.tile_codec.decode(bytes)
    }

    /// [`MbtilesReader::get_tile`] followed by [`MbtilesReader::decode_tile`]:
    /// returns the raw (decompressed) tile payload.
    pub fn get_tile_decoded(&mut self, zoom: i64, column: i64, row: i64) -> Result<Option<Vec<u8>>> {
        match self.get_tile(zoom, column, row)? {
            Some(bytes) => Ok(Some(self.decode_tile(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Get all tiles at a given zoom level.
    pub fn get_tiles_at_zoom(&mut self, zoom: i64) -> Result<Vec<Tile>> {
        let mut tiles = Vec::new();
        let root = self.tiles_root_page;
        self.scan_table_pages(root, &mut |reader, _rowid, local, total, overflow| {
            let record_check = parse_record_header_and_ints(local, reader.header.text_encoding)?;
            if let Some((z, c, r)) = record_check {
                if z == zoom {
                    let payload = reader.assemble_payload(local, total, overflow)?;
                    let record = parse_record(&payload, reader.header.text_encoding)?;
                    if record.len() >= 4 {
                        if let Some(data) = record.into_iter().nth(3).and_then(|v| v.into_blob()) {
                            tiles.push(Tile {
                                zoom_level: z,
                                tile_column: c,
                                tile_row: r,
                                tile_data: data,
                            });
                        }
                    }
                }
            }
            Ok(())
        })?;
        Ok(tiles)
    }

    /// Iterate over all tiles in the database, calling `callback` for each.
    pub fn for_each_tile(&mut self, mut callback: impl FnMut(Tile)) -> Result<()> {
        let root = self.tiles_root_page;
        self.scan_table_pages(root, &mut |reader, _rowid, local, total, overflow| {
            let payload = reader.assemble_payload(local, total, overflow)?;
            let record = parse_record(&payload, reader.header.text_encoding)?;
            if record.len() >= 4 {
                let zoom = record[0].as_integer().unwrap_or(0);
                let col = record[1].as_integer().unwrap_or(0);
                let row = record[2].as_integer().unwrap_or(0);
                if let Some(data) = record.into_iter().nth(3).and_then(|v| v.into_blob()) {
                    callback(Tile {
                        zoom_level: zoom,
                        tile_column: col,
                        tile_row: row,
                        tile_data: data,
                    });
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    /// Get a summary of tiles per zoom level: Vec<(zoom_level, count)>.
    pub fn tile_summary(&mut self) -> Result<Vec<(i64, usize)>> {
        let mut counts: HashMap<i64, usize> = HashMap::new();
        let root = self.tiles_root_page;
        self.scan_table_pages(root, &mut |reader, _rowid, local, _total, _overflow| {
            let check = parse_record_header_and_ints(local, reader.header.text_encoding)?;
            if let Some((z, _, _)) = check {
                *counts.entry(z).or_insert(0) += 1;
            }
            Ok(())
        })?;
        let mut summary: Vec<(i64, usize)> = counts.into_iter().collect();
        summary.sort_by_key(|&(z, _)| z);
        Ok(summary)
    }

    /// Access the database header info.
    pub fn header(&self) -> &DbHeader {
        &self.header
    }

    /// Open any SQLite database (e.g. a GeoPackage) for generic table access.
    /// Unlike [`MbtilesReader::open`] this does not require the mbtiles schema;
    /// the tile-specific methods will fail on such a database, but
    /// [`MbtilesReader::schema_entries`] and [`MbtilesReader::for_each_row`]
    /// work on any table.
    pub fn open_sqlite(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let mut header_buf = [0u8; 100];
        file.read_exact(&mut header_buf)?;
        let header = parse_db_header(&header_buf)?;
        let usable_size = header.page_size as usize - header.reserved_space as usize;
        Ok(MbtilesReader {
            file,
            header,
            usable_size,
            tiles_root_page: 0,
            metadata_root_page: 0,
            tile_index_root_page: None,
            makepad_block_rowids: false,
            tile_codec: TileCodec::gzip(),
            btree_page_cache: HashMap::new(),
            btree_page_cache_order: VecDeque::new(),
        })
    }

    /// All objects recorded in sqlite_master: tables, indexes, views, triggers.
    pub fn schema_entries(&mut self) -> Result<Vec<SchemaEntry>> {
        let mut entries = Vec::new();
        self.scan_table_pages(1, &mut |reader, _rowid, local, total, overflow| {
            let payload = reader.assemble_payload(local, total, overflow)?;
            let record = parse_record(&payload, reader.header.text_encoding)?;
            if record.len() >= 5 {
                entries.push(SchemaEntry {
                    obj_type: record[0].as_text().unwrap_or("").to_string(),
                    name: record[1].as_text().unwrap_or("").to_string(),
                    tbl_name: record[2].as_text().unwrap_or("").to_string(),
                    root_page: record[3].as_integer().unwrap_or(0) as u32,
                    sql: record[4].as_text().unwrap_or("").to_string(),
                });
            }
            Ok(())
        })?;
        Ok(entries)
    }

    /// Walk every row of the named table, decoding each record's values.
    /// A column declared INTEGER PRIMARY KEY is the rowid alias and appears as
    /// [`Value::Null`] in the record; use the callback's rowid for it.
    pub fn for_each_row(
        &mut self,
        table: &str,
        mut callback: impl FnMut(i64, Vec<Value>),
    ) -> Result<()> {
        let root = self
            .schema_entries()?
            .into_iter()
            .find(|e| e.obj_type == "table" && e.name == table)
            .map(|e| e.root_page)
            .ok_or(Error::TableNotFound("requested table"))?;
        if root == 0 {
            return Err(Error::TableNotFound("requested table"));
        }
        self.scan_table_pages(root, &mut |reader, rowid, local, total, overflow| {
            let payload = reader.assemble_payload(local, total, overflow)?;
            let record = parse_record(&payload, reader.header.text_encoding)?;
            callback(rowid, record);
            Ok(())
        })
    }

    /// Walk rows of the named table whose rowid lies in `lo..=hi`, pruning
    /// b-tree subtrees outside the range — an indexed range query without SQL.
    pub fn for_each_row_in_range(
        &mut self,
        table: &str,
        lo: i64,
        hi: i64,
        mut callback: impl FnMut(i64, Vec<Value>),
    ) -> Result<()> {
        let root = self
            .schema_entries()?
            .into_iter()
            .find(|e| e.obj_type == "table" && e.name == table)
            .map(|e| e.root_page)
            .ok_or(Error::TableNotFound("requested table"))?;
        if root == 0 {
            return Err(Error::TableNotFound("requested table"));
        }
        let mut page_stack = vec![root];
        while let Some(page_num) = page_stack.pop() {
            let page = self.read_btree_page(page_num)?;
            let header_offset = if page_num == 1 { 100 } else { 0 };
            let (page_type, cell_ptrs, rightmost_ptr) = self.cell_pointers(&page, header_offset)?;
            match page_type {
                PageType::TableLeaf => {
                    for &ptr in &cell_ptrs {
                        let (rowid, local, total, overflow) =
                            self.parse_table_leaf_cell(&page, ptr)?;
                        if rowid < lo || rowid > hi {
                            continue;
                        }
                        let payload = self.assemble_payload(local, total, overflow)?;
                        let record = parse_record(&payload, self.header.text_encoding)?;
                        callback(rowid, record);
                    }
                }
                PageType::TableInterior => {
                    // Interior cells hold (child, max_rowid_of_child) in
                    // ascending order; the rightmost pointer covers the rest.
                    let mut prev_max = i64::MIN;
                    let mut include_right = true;
                    for &ptr in &cell_ptrs {
                        let (child, key) = self.parse_table_interior_cell(&page, ptr)?;
                        if key >= lo && prev_max <= hi {
                            page_stack.push(child);
                        }
                        if key > hi {
                            include_right = false;
                            // later siblings are entirely above the range
                            break;
                        }
                        prev_max = key;
                    }
                    if include_right {
                        if let Some(right) = rightmost_ptr {
                            page_stack.push(right);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// One row of sqlite_master, describing a schema object.
#[derive(Debug, Clone)]
pub struct SchemaEntry {
    pub obj_type: String,
    pub name: String,
    pub tbl_name: String,
    pub root_page: u32,
    pub sql: String,
}

/// Quick parse of the first 3 integer columns from a record's local payload.
/// Used to avoid assembling overflow for non-matching tiles.
/// Returns Some((zoom, column, row)) if the first 3 columns are integers.
fn parse_record_header_and_ints(
    payload: &[u8],
    encoding: TextEncoding,
) -> Result<Option<(i64, i64, i64)>> {
    if payload.is_empty() {
        return Ok(None);
    }
    let (header_size, hdr_n) = read_varint(payload)?;
    let header_size = header_size as usize;
    if header_size > payload.len() {
        return Ok(None);
    }

    // We need at least 3 serial types
    let mut serial_types = Vec::new();
    let mut hpos = hdr_n;
    while hpos < header_size && serial_types.len() < 4 {
        let (st, n) = read_varint(&payload[hpos..])?;
        serial_types.push(st);
        hpos += n;
    }

    if serial_types.len() < 3 {
        return Ok(None);
    }

    let mut data_pos = header_size;
    let mut ints = [0i64; 3];
    for i in 0..3 {
        let st = serial_types[i];
        let size = serial_type_content_size(st);
        if data_pos + size > payload.len() {
            return Ok(None);
        }
        match decode_value(st, &payload[data_pos..], encoding)? {
            Value::Integer(v) => ints[i] = v,
            _ => return Ok(None),
        }
        data_pos += size;
    }

    Ok(Some((ints[0], ints[1], ints[2])))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn test_read_varint() {
        // Single byte
        assert_eq!(read_varint(&[0x00]).unwrap(), (0, 1));
        assert_eq!(read_varint(&[0x7f]).unwrap(), (127, 1));
        // Two bytes
        assert_eq!(read_varint(&[0x81, 0x00]).unwrap(), (128, 2));
        assert_eq!(read_varint(&[0x81, 0x01]).unwrap(), (129, 2));
    }

    #[test]
    fn test_serial_type_content_size() {
        assert_eq!(serial_type_content_size(0), 0); // NULL
        assert_eq!(serial_type_content_size(1), 1); // i8
        assert_eq!(serial_type_content_size(2), 2); // i16
        assert_eq!(serial_type_content_size(3), 3); // i24
        assert_eq!(serial_type_content_size(4), 4); // i32
        assert_eq!(serial_type_content_size(5), 6); // i48
        assert_eq!(serial_type_content_size(6), 8); // i64
        assert_eq!(serial_type_content_size(7), 8); // f64
        assert_eq!(serial_type_content_size(8), 0); // const 0
        assert_eq!(serial_type_content_size(9), 0); // const 1
        assert_eq!(serial_type_content_size(12), 0); // blob(0)
        assert_eq!(serial_type_content_size(14), 1); // blob(1)
        assert_eq!(serial_type_content_size(13), 0); // text(0)
        assert_eq!(serial_type_content_size(15), 1); // text(1)
    }

    #[test]
    fn test_decode_text_utf16le() {
        let bytes = b"h\x00e\x00l\x00l\x00o\x00";
        let s = decode_text(bytes, TextEncoding::Utf16Le).unwrap();
        assert_eq!(s, "hello");
    }

    #[test]
    fn test_payload_overflow_thresholds() {
        // page_size=4096, reserved=0 => usable=4096
        let usable = 4096;
        let max_leaf = payload_overflow_threshold_max(PageType::TableLeaf, usable);
        let min_leaf = payload_overflow_threshold_min(PageType::TableLeaf, usable);
        assert_eq!(max_leaf, 4096 - 35); // 4061
        assert_eq!(min_leaf, ((4096 - 12) * 32 / 255) - 23); // 489

        // Small payload - no overflow
        let (overflows, _) = payload_overflows(100, max_leaf, min_leaf, usable);
        assert!(!overflows);

        // Large payload - overflows
        let (overflows, local) = payload_overflows(50000, max_leaf, min_leaf, usable);
        assert!(overflows);
        assert!(local > 0);
        assert!(local <= max_leaf + 4);
    }

    #[test]
    fn direct_lookup_through_standard_tile_index() {
        // Build this interoperability fixture with the system SQLite when it
        // is available. The library itself has no native SQLite dependency.
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "makepad-mbtiles-index-{}-{nonce}.mbtiles",
            std::process::id()
        ));
        let mut child = match Command::new("sqlite3")
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
            Err(err) => panic!("start sqlite3: {err}"),
        };
        let sql = br#"
PRAGMA page_size=512;
VACUUM;
CREATE TABLE tiles (
    zoom_level INTEGER,
    tile_column INTEGER,
    tile_row INTEGER,
    tile_data BLOB
);
CREATE UNIQUE INDEX tile_index
    ON tiles (zoom_level, tile_column, tile_row);
CREATE TABLE metadata (name TEXT, value TEXT);
INSERT INTO metadata VALUES ('format', 'pbf');
WITH RECURSIVE sequence(i) AS (
    VALUES(0)
    UNION ALL
    SELECT i + 1 FROM sequence WHERE i < 4999
)
INSERT INTO tiles
SELECT 14, i, 100000 - i, CAST(printf('tile-%d', i) AS BLOB)
FROM sequence;
"#;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(sql)
            .expect("write sqlite fixture");
        let output = child.wait_with_output().expect("wait for sqlite3");
        assert!(
            output.status.success(),
            "sqlite3 fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let mut reader = MbtilesReader::open(&path).unwrap();
        assert!(reader.supports_direct_tile_lookup());
        assert_eq!(
            reader.get_tile(14, 4321, 95679).unwrap().unwrap(),
            b"tile-4321"
        );
        assert_eq!(reader.get_tile(14, 6000, 94000).unwrap(), None);

        std::fs::remove_file(path).unwrap();
    }
}
