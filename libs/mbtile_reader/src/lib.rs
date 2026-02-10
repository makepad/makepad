//! Minimal read-only SQLite file format parser for MBTiles tile extraction.
//!
//! Implements just enough of the SQLite on-disk format to:
//! - Parse the 100-byte database header
//! - Walk B-tree pages (table interior + leaf)
//! - Decode varint-encoded records
//! - Follow overflow page chains for large blobs
//! - Scan the sqlite_master table to find table root pages
//! - Scan the tiles table and look up tiles by (zoom, column, row)
//!
//! Reference: <https://www.sqlite.org/fileformat.html>
//! Reference implementation studied: turso/libsql core/storage/sqlite3_ondisk.rs

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

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
    /// Root page number of the `tile_index` index (1-based), if present
    tile_index_root_page: Option<u32>,
}

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
        };

        // Scan sqlite_master (always rooted at page 1) to find our tables
        reader.scan_schema()?;

        if reader.tiles_root_page == 0 {
            return Err(Error::TableNotFound("tiles"));
        }

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
                let root_page = record[3].as_integer().unwrap_or(0) as u32;

                let _ = rowid;
                match (obj_type, name) {
                    ("table", "tiles") => reader.tiles_root_page = root_page,
                    ("table", "metadata") => reader.metadata_root_page = root_page,
                    ("index", "tile_index") => reader.tile_index_root_page = Some(root_page),
                    _ => {}
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

    /// Get a single tile by (zoom_level, tile_column, tile_row).
    /// Returns the raw tile_data blob (typically gzip-compressed PBF).
    pub fn get_tile(&mut self, zoom: i64, column: i64, row: i64) -> Result<Option<Vec<u8>>> {
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
}
