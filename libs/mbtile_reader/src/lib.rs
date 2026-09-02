//! MBTiles reading and streaming writing on top of the SQLite file format.
//!
//! The SQLite format work — header parsing, b-tree pages, the record codec,
//! overflow chains, `sqlite_master` and WAL-aware page reads — lives in
//! [`makepad_sqlite`]; this crate is the tile-shaped layer on top of it:
//!
//! - find the `tiles`, `metadata` and tile-index roots in `sqlite_master`
//! - look a tile up by (zoom, column, row), through the deterministic rowid
//!   scheme of [`MbtilesWriter`] or through a conventional composite index
//! - stream a sorted set of tiles into a new MBTiles database
//! - generic table access for GeoPackages and overlay databases
//!
//! Reference: <https://www.sqlite.org/fileformat.html>

use std::collections::HashMap;
use std::path::Path;

mod codec;
mod mkmap;
#[cfg(not(target_arch = "wasm32"))]
mod writer;
pub use codec::{
    compress_tile, compression_metadata_rows, TileCodec, TileCompression,
    COMPRESSION_DICT_METADATA_KEY, COMPRESSION_METADATA_KEY,
};
pub use mkmap::{
    mkmap_tile_id, mkmap_zxy_from_tile_id, BlobRef, MkmapLeaf, MkmapReader, MkmapRoot,
    MkmapTileRef, RootRecordRef, TileArchiveReader,
};
#[cfg(not(target_arch = "wasm32"))]
pub use writer::{MbtilesWriter, MbtilesWriterStats, WriterValue};

use makepad_sqlite::btree::{IndexCursor, TableCursor};
use makepad_sqlite::schema::{read_objects, SchemaObject};
use makepad_sqlite::value::TextMode;
use makepad_sqlite::{Collation, Pager, Value as DbValue};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use makepad_sqlite::btree::{local_payload_size, PageType};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use makepad_sqlite::pager::MAGIC as SQLITE_MAGIC;
pub use makepad_sqlite::pager::DbHeader;
pub use makepad_sqlite::value::TextEncoding;

/// Text is decoded leniently here: historical mbtiles/GeoPackage archives in
/// the wild are not always clean UTF-8 and readers must keep working.
const TEXT_MODE: TextMode = TextMode::Lossy;

/// Compute the deterministic rowid used for Makepad-authored MBTiles files.
///
/// Coordinates are ordered by zoom, then 256×256 block row and column, then
/// local row and column. This matches the order in a VersaTiles v02 archive.
pub fn tile_rowid_xyz(zoom: u8, x: u32, y: u32) -> Option<i64> {
    if zoom > 31 {
        return None;
    }
    let axis = 1_u64 << zoom;
    if u64::from(x) >= axis || u64::from(y) >= axis {
        return None;
    }

    let zoom_capacity = 1_u128 << (u32::from(zoom) * 2);
    let prefix = (zoom_capacity - 1) / 3;
    let within_zoom = if zoom <= 8 {
        u128::from(y) * u128::from(axis) + u128::from(x)
    } else {
        let blocks_per_axis = 1_u128 << (zoom - 8);
        let block_x = u128::from(x >> 8);
        let block_y = u128::from(y >> 8);
        let local_x = u128::from(x & 255);
        let local_y = u128::from(y & 255);
        ((block_y * blocks_per_axis + block_x) << 16) + (local_y << 8) + local_x
    };
    i64::try_from(prefix + within_zoom + 1).ok()
}

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
    /// Anything the SQLite-format engine reported: corrupt pages, unsupported
    /// format features, IO below the page cache.
    Db(makepad_sqlite::Error),
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
            Error::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<makepad_sqlite::Error> for Error {
    fn from(e: makepad_sqlite::Error) -> Self {
        match e {
            makepad_sqlite::Error::Io(io) => Error::Io(io),
            makepad_sqlite::Error::NotADatabase => Error::InvalidMagic,
            other => Error::Db(other),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// One column value from a row.
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

impl From<DbValue> for Value {
    fn from(v: DbValue) -> Value {
        match v {
            DbValue::Null => Value::Null,
            DbValue::Integer(i) => Value::Integer(i),
            DbValue::Real(f) => Value::Float(f),
            DbValue::Text(s) => Value::Text(s),
            DbValue::Blob(b) => Value::Blob(b),
        }
    }
}

fn convert(values: Vec<DbValue>) -> Vec<Value> {
    values.into_iter().map(Value::from).collect()
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

impl From<SchemaObject> for SchemaEntry {
    fn from(o: SchemaObject) -> SchemaEntry {
        SchemaEntry {
            obj_type: o.obj_type,
            name: o.name,
            tbl_name: o.tbl_name,
            root_page: o.root_page,
            sql: o.sql,
        }
    }
}

/// A single tile from the mbtiles database.
#[derive(Debug, Clone)]
pub struct Tile {
    pub zoom_level: i64,
    pub tile_column: i64,
    pub tile_row: i64,
    pub tile_data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

pub struct MbtilesReader {
    pager: Pager,
    /// Root page of the `tiles` table (1-based).
    tiles_root_page: u32,
    /// Root page of the `metadata` table (1-based).
    metadata_root_page: u32,
    /// Root page of the standard `(zoom_level, tile_column, tile_row)` index.
    tile_index_root_page: Option<u32>,
    /// Makepad-authored files use deterministic rowids for direct lookup.
    makepad_block_rowids: bool,
    /// Tile payload codec, parsed once from metadata (absent = gzip).
    tile_codec: TileCodec,
}

impl MbtilesReader {
    /// Open an MBTiles file and locate its tables.
    pub fn open(path: &Path) -> Result<Self> {
        let mut reader = MbtilesReader::open_sqlite(path)?;
        for entry in reader.schema_entries()? {
            match (entry.obj_type.as_str(), entry.name.as_str()) {
                ("table", "tiles") => reader.tiles_root_page = entry.root_page,
                ("table", "metadata") => reader.metadata_root_page = entry.root_page,
                _ => {}
            }
            // `tile_index` is the conventional MBTiles spelling. Also accept
            // SQLite's automatic index for a UNIQUE constraint on the tiles
            // table; that one has no SQL of its own, hence the name checks.
            if entry.obj_type == "index"
                && entry.tbl_name == "tiles"
                && (entry.name == "tile_index"
                    || entry.name.starts_with("sqlite_autoindex_tiles_")
                    || (entry.sql.contains("zoom_level")
                        && entry.sql.contains("tile_column")
                        && entry.sql.contains("tile_row")))
            {
                reader.tile_index_root_page = Some(entry.root_page);
            }
        }
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

    /// Open any SQLite database (e.g. a GeoPackage) for generic table access.
    /// The tile-specific methods will not find their tables on such a file, but
    /// [`MbtilesReader::schema_entries`] and [`MbtilesReader::for_each_row`]
    /// work on any table.
    pub fn open_sqlite(path: &Path) -> Result<Self> {
        Ok(MbtilesReader {
            pager: Pager::open(path)?,
            tiles_root_page: 0,
            metadata_root_page: 0,
            tile_index_root_page: None,
            makepad_block_rowids: false,
            tile_codec: TileCodec::gzip(),
        })
    }

    /// Access the database header info.
    pub fn header(&self) -> &DbHeader {
        self.pager.header()
    }

    /// All objects recorded in sqlite_master: tables, indexes, views, triggers.
    pub fn schema_entries(&mut self) -> Result<Vec<SchemaEntry>> {
        Ok(read_objects(&mut self.pager)?
            .into_iter()
            .map(SchemaEntry::from)
            .collect())
    }

    fn table_root(&mut self, table: &str) -> Result<u32> {
        let root = read_objects(&mut self.pager)?
            .into_iter()
            .find(|e| e.obj_type == "table" && e.name == table)
            .map(|e| e.root_page)
            .ok_or(Error::TableNotFound("requested table"))?;
        if root == 0 {
            return Err(Error::TableNotFound("requested table"));
        }
        Ok(root)
    }

    /// Get all metadata key-value pairs.
    pub fn get_metadata(&mut self) -> Result<HashMap<String, String>> {
        if self.metadata_root_page == 0 {
            return Ok(HashMap::new());
        }
        let mut metadata = HashMap::new();
        let mut cursor = TableCursor::new(self.metadata_root_page);
        cursor.rewind(&mut self.pager)?;
        while let Some(row) = cursor.next(&mut self.pager)? {
            let record = row.payload.values(&mut self.pager, TEXT_MODE)?;
            if record.len() >= 2 {
                if let (Some(key), Some(val)) = (record[0].as_text(), record[1].as_text()) {
                    metadata.insert(key.to_string(), val.to_string());
                }
            }
        }
        Ok(metadata)
    }

    /// Whether `get_tile` can seek directly to one deterministic tile row.
    ///
    /// Files emitted by [`MbtilesWriter`] use deterministic table rowids.
    /// Conventional MBTiles files use their composite tile index. Only
    /// malformed or unusually index-free third-party files need a scan.
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
            let Some(rowid) = tile_rowid_xyz(zoom_u8, column_u32, xyz_row) else {
                return Ok(None);
            };
            let Some(record) = self.tile_row(rowid)? else {
                return Ok(None);
            };
            return self.tile_blob(record, zoom, column, row, "deterministic rowid");
        }

        if let Some(index_root) = self.tile_index_root_page {
            let Some(table_rowid) = self.find_tile_rowid_in_index(index_root, [zoom, column, row])?
            else {
                return Ok(None);
            };
            let Some(record) = self.tile_row(table_rowid)? else {
                return Err(Error::CorruptRecord(
                    "tile index points at a missing table row",
                ));
            };
            return self.tile_blob(record, zoom, column, row, "tile index");
        }

        // Index-free archive: scan, filtering on the locally stored columns so
        // overflow pages are only touched for the tile we want.
        let root = self.tiles_root_page;
        let mut cursor = TableCursor::new(root);
        cursor.rewind(&mut self.pager)?;
        while let Some(row_data) = cursor.next(&mut self.pager)? {
            let head = row_data.payload.prefix(&mut self.pager, 3, TEXT_MODE)?;
            if head.len() < 3 {
                continue;
            }
            if head[0].as_integer() == Some(zoom)
                && head[1].as_integer() == Some(column)
                && head[2].as_integer() == Some(row)
            {
                let record = row_data.payload.values(&mut self.pager, TEXT_MODE)?;
                return Ok(record.into_iter().nth(3).and_then(|v| match v {
                    DbValue::Blob(b) => Some(b),
                    _ => None,
                }));
            }
        }
        Ok(None)
    }

    fn tile_row(&mut self, rowid: i64) -> Result<Option<Vec<DbValue>>> {
        let root = self.tiles_root_page;
        let mut cursor = TableCursor::new(root);
        let Some(row) = cursor.seek_exact(&mut self.pager, rowid)? else {
            return Ok(None);
        };
        Ok(Some(row.payload.values(&mut self.pager, TEXT_MODE)?))
    }

    fn tile_blob(
        &self,
        record: Vec<DbValue>,
        zoom: i64,
        column: i64,
        row: i64,
        via: &'static str,
    ) -> Result<Option<Vec<u8>>> {
        if record.len() < 4
            || record[0].as_integer() != Some(zoom)
            || record[1].as_integer() != Some(column)
            || record[2].as_integer() != Some(row)
        {
            return Err(Error::CorruptRecord(match via {
                "tile index" => "tile index points at the wrong tile",
                _ => "deterministic rowid points at the wrong tile",
            }));
        }
        Ok(record.into_iter().nth(3).and_then(|v| match v {
            DbValue::Blob(b) => Some(b),
            _ => None,
        }))
    }

    /// Find the tiles-table rowid through a conventional MBTiles composite
    /// index, so third-party archives stay streamable without a full scan.
    fn find_tile_rowid_in_index(&mut self, root: u32, target: [i64; 3]) -> Result<Option<i64>> {
        let key = [
            DbValue::Integer(target[0]),
            DbValue::Integer(target[1]),
            DbValue::Integer(target[2]),
        ];
        let colls = [Collation::Binary; 3];
        let mut cursor = IndexCursor::new(root);
        cursor.seek_ge(&mut self.pager, &key, &colls)?;
        let Some(entry) = cursor.next(&mut self.pager)? else {
            return Ok(None);
        };
        let record = entry.values(&mut self.pager, TEXT_MODE)?;
        if record.len() < 4 {
            return Err(Error::CorruptRecord(
                "tile index entry has fewer than four columns",
            ));
        }
        if record[0].as_integer() != Some(target[0])
            || record[1].as_integer() != Some(target[1])
            || record[2].as_integer() != Some(target[2])
        {
            return Ok(None);
        }
        record
            .last()
            .and_then(DbValue::as_integer)
            .map(Some)
            .ok_or(Error::CorruptRecord(
                "tile index table rowid is not an integer",
            ))
    }

    /// The archive's tile payload codec (parsed once from metadata at open;
    /// archives without a `compression` metadata row are gzip).
    pub fn tile_codec(&self) -> &TileCodec {
        &self.tile_codec
    }

    /// Decode a stored tile payload to raw bytes using the archive's codec.
    pub fn decode_tile(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        self.tile_codec.decode(bytes)
    }

    /// [`MbtilesReader::get_tile`] followed by [`MbtilesReader::decode_tile`].
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
        let mut cursor = TableCursor::new(root);
        cursor.rewind(&mut self.pager)?;
        while let Some(row) = cursor.next(&mut self.pager)? {
            let head = row.payload.prefix(&mut self.pager, 3, TEXT_MODE)?;
            if head.len() < 3 || head[0].as_integer() != Some(zoom) {
                continue;
            }
            let record = row.payload.values(&mut self.pager, TEXT_MODE)?;
            if record.len() < 4 {
                continue;
            }
            let (z, c, r) = (
                record[0].as_integer().unwrap_or(0),
                record[1].as_integer().unwrap_or(0),
                record[2].as_integer().unwrap_or(0),
            );
            if let Some(DbValue::Blob(data)) = record.into_iter().nth(3) {
                tiles.push(Tile {
                    zoom_level: z,
                    tile_column: c,
                    tile_row: r,
                    tile_data: data,
                });
            }
        }
        Ok(tiles)
    }

    /// Iterate over all tiles in the database, calling `callback` for each.
    pub fn for_each_tile(&mut self, mut callback: impl FnMut(Tile)) -> Result<()> {
        let root = self.tiles_root_page;
        let mut cursor = TableCursor::new(root);
        cursor.rewind(&mut self.pager)?;
        while let Some(row) = cursor.next(&mut self.pager)? {
            let record = row.payload.values(&mut self.pager, TEXT_MODE)?;
            if record.len() < 4 {
                continue;
            }
            let zoom = record[0].as_integer().unwrap_or(0);
            let col = record[1].as_integer().unwrap_or(0);
            let tile_row = record[2].as_integer().unwrap_or(0);
            if let Some(DbValue::Blob(data)) = record.into_iter().nth(3) {
                callback(Tile {
                    zoom_level: zoom,
                    tile_column: col,
                    tile_row,
                    tile_data: data,
                });
            }
        }
        Ok(())
    }

    /// Get a summary of tiles per zoom level: Vec<(zoom_level, count)>.
    pub fn tile_summary(&mut self) -> Result<Vec<(i64, usize)>> {
        let mut counts: HashMap<i64, usize> = HashMap::new();
        let root = self.tiles_root_page;
        let mut cursor = TableCursor::new(root);
        cursor.rewind(&mut self.pager)?;
        while let Some(row) = cursor.next(&mut self.pager)? {
            let head = row.payload.prefix(&mut self.pager, 1, TEXT_MODE)?;
            if let Some(z) = head.first().and_then(DbValue::as_integer) {
                *counts.entry(z).or_insert(0) += 1;
            }
        }
        let mut summary: Vec<(i64, usize)> = counts.into_iter().collect();
        summary.sort_by_key(|&(z, _)| z);
        Ok(summary)
    }

    /// Walk every row of the named table, decoding each record's values.
    /// A column declared INTEGER PRIMARY KEY is the rowid alias and appears as
    /// [`Value::Null`] in the record; use the callback's rowid for it.
    pub fn for_each_row(
        &mut self,
        table: &str,
        mut callback: impl FnMut(i64, Vec<Value>),
    ) -> Result<()> {
        let root = self.table_root(table)?;
        let mut cursor = TableCursor::new(root);
        cursor.rewind(&mut self.pager)?;
        while let Some(row) = cursor.next(&mut self.pager)? {
            let record = row.payload.values(&mut self.pager, TEXT_MODE)?;
            callback(row.rowid, convert(record));
        }
        Ok(())
    }

    /// Walk rows of the named table whose rowid lies in `lo..=hi`, seeking
    /// straight to `lo` through the b-tree — an indexed range query without SQL.
    pub fn for_each_row_in_range(
        &mut self,
        table: &str,
        lo: i64,
        hi: i64,
        mut callback: impl FnMut(i64, Vec<Value>),
    ) -> Result<()> {
        let root = self.table_root(table)?;
        if lo > hi {
            return Ok(());
        }
        let mut cursor = TableCursor::new(root);
        cursor.seek_ge(&mut self.pager, lo)?;
        while let Some(row) = cursor.next(&mut self.pager)? {
            if row.rowid > hi {
                break;
            }
            let record = row.payload.values(&mut self.pager, TEXT_MODE)?;
            callback(row.rowid, convert(record));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sqlite_fixture(tag: &str, sql: &[u8]) -> Option<std::path::PathBuf> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "makepad-mbtiles-{tag}-{}-{nonce}.mbtiles",
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
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
            Err(err) => panic!("start sqlite3: {err}"),
        };
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
        Some(path)
    }

    #[test]
    fn direct_lookup_through_standard_tile_index() {
        // Build this interoperability fixture with the system SQLite when it
        // is available. The library itself has no native SQLite dependency.
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
        let Some(path) = sqlite_fixture("index", sql) else {
            return;
        };
        let mut reader = MbtilesReader::open(&path).unwrap();
        assert!(reader.supports_direct_tile_lookup());
        assert_eq!(
            reader.get_tile(14, 4321, 95679).unwrap().unwrap(),
            b"tile-4321"
        );
        assert_eq!(reader.get_tile(14, 6000, 94000).unwrap(), None);
        assert_eq!(reader.tile_summary().unwrap(), vec![(14, 5000)]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn scan_fallback_without_an_index() {
        let sql = br#"
CREATE TABLE tiles (
    zoom_level INTEGER,
    tile_column INTEGER,
    tile_row INTEGER,
    tile_data BLOB
);
CREATE TABLE metadata (name TEXT, value TEXT);
INSERT INTO tiles VALUES (3, 1, 2, CAST('hello' AS BLOB));
INSERT INTO tiles VALUES (3, 1, 3, randomblob(20000));
"#;
        let Some(path) = sqlite_fixture("scan", sql) else {
            return;
        };
        let mut reader = MbtilesReader::open(&path).unwrap();
        assert!(!reader.supports_direct_tile_lookup());
        assert_eq!(reader.get_tile(3, 1, 2).unwrap().unwrap(), b"hello");
        // The overflowing row still assembles correctly.
        assert_eq!(reader.get_tile(3, 1, 3).unwrap().unwrap().len(), 20000);
        assert_eq!(reader.get_tile(3, 9, 9).unwrap(), None);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn generic_table_access() {
        let sql = br#"
CREATE TABLE features (id INTEGER PRIMARY KEY, name TEXT, geom BLOB);
INSERT INTO features VALUES (10, 'a', x'01');
INSERT INTO features VALUES (20, 'b', x'02');
INSERT INTO features VALUES (30, 'c', x'03');
"#;
        let Some(path) = sqlite_fixture("generic", sql) else {
            return;
        };
        let mut db = MbtilesReader::open_sqlite(&path).unwrap();
        let entries = db.schema_entries().unwrap();
        assert!(entries.iter().any(|e| e.name == "features"));
        let mut ids = Vec::new();
        db.for_each_row("features", |rowid, _values| ids.push(rowid))
            .unwrap();
        assert_eq!(ids, vec![10, 20, 30]);
        let mut ranged = Vec::new();
        db.for_each_row_in_range("features", 15, 25, |rowid, values| {
            ranged.push((rowid, values[1].as_text().unwrap_or("").to_string()))
        })
        .unwrap();
        assert_eq!(ranged, vec![(20, "b".to_string())]);
        assert!(db.for_each_row("nope", |_, _| {}).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
