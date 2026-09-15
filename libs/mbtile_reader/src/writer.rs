use super::codec::{compress_tile, compression_metadata_rows};
use super::{
    local_payload_size, tile_rowid_xyz, Error, PageType, Result, TileCompression, SQLITE_MAGIC,
};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

const PAGE_SIZE: usize = 65_536;
/// Page containing file byte offset 1 GiB (1-based); reserved by SQLite for
/// file locks and never allocatable.
const LOCK_BYTE_PAGE: u32 = (1 << 30) / PAGE_SIZE as u32 + 1;
const SQLITE_APPLICATION_ID_MBTILES: u32 = 0x4d50_4258;
const MAKEPAD_ROWID_SCHEME_KEY: &str = "makepad_rowid_scheme";
const MAKEPAD_ROWID_SCHEME_VALUE: &str = "block-v1-xyz";

/// Summary returned after an MBTiles database has been finalized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MbtilesWriterStats {
    pub tile_count: u64,
    pub tile_bytes: u64,
    pub page_count: u32,
    pub file_bytes: u64,
}

/// Streaming MBTiles writer backed by a minimal pure-Rust SQLite file builder.
///
/// Tiles must be supplied in the order returned by [`tile_rowid_xyz`]. The
/// `makepad-map-tiles` utility naturally produces this order by traversing
/// zoom, 256×256 block row, block column, local row, and local column.
pub struct MbtilesWriter {
    db: RawDbWriter,
    metadata: BTreeMap<String, String>,
    tiles: TableStream,
    tile_count: u64,
    tile_bytes: u64,
    extra_tables: Vec<ExtraTable>,
    tile_compression: TileCompression,
    compression_dict: Option<Vec<u8>>,
}

struct ExtraTable {
    name: String,
    sql: String,
    stream: TableStream,
}

/// Value for rows in extra (non-tiles) tables.
pub enum WriterValue<'a> {
    Null,
    Integer(i64),
    Float(f64),
    Text(&'a str),
    Blob(&'a [u8]),
}

impl MbtilesWriter {
    /// Create or truncate an MBTiles file.
    pub fn create(path: &Path) -> Result<Self> {
        let mut db = RawDbWriter::create(path)?;
        let tiles = TableStream::new(&mut db)?;
        Ok(Self {
            db,
            metadata: BTreeMap::new(),
            tiles,
            tile_count: 0,
            tile_bytes: 0,
            extra_tables: Vec::new(),
            tile_compression: TileCompression::Gzip,
            compression_dict: None,
        })
    }

    /// Opt in to a per-archive tile codec (default: gzip, no metadata row).
    ///
    /// Records the `compression` (and, with a dictionary, `compression_dict`)
    /// metadata rows so readers decode correctly, and selects the codec used
    /// by [`MbtilesWriter::write_tile_encoded`]. Payloads passed to
    /// [`MbtilesWriter::write_tile_xyz`] are still stored verbatim, which
    /// lets parallel pipelines compress on worker threads via
    /// [`crate::compress_tile`] and hand finished bytes to the writer.
    pub fn set_tile_compression(
        &mut self,
        compression: TileCompression,
        dict: Option<Vec<u8>>,
    ) {
        let dict = dict.filter(|d| !d.is_empty());
        for (key, value) in compression_metadata_rows(&compression, dict.as_deref()) {
            self.set_metadata(key, value);
        }
        self.tile_compression = compression;
        self.compression_dict = dict;
    }

    /// Compress a raw tile payload with the archive codec, then store it.
    pub fn write_tile_encoded(&mut self, zoom: u8, x: u32, y: u32, raw: &[u8]) -> Result<()> {
        let compressed = compress_tile(
            &self.tile_compression,
            self.compression_dict.as_deref(),
            raw,
        )?;
        self.write_tile_xyz(zoom, x, y, &compressed)
    }

    /// Declare an additional table. Rows are supplied via
    /// [`MbtilesWriter::write_extra_row`] in strictly ascending rowid order
    /// (per table). The CREATE TABLE statement is recorded verbatim in
    /// sqlite_master, so standard SQLite tooling sees proper column names.
    pub fn begin_extra_table(&mut self, name: &str, create_sql: &str) -> Result<()> {
        if self.extra_tables.iter().any(|t| t.name == name) {
            return Err(Error::InvalidInput(format!(
                "extra table {name} already declared"
            )));
        }
        let stream = TableStream::new(&mut self.db)?;
        self.extra_tables.push(ExtraTable {
            name: name.to_string(),
            sql: create_sql.to_string(),
            stream,
        });
        Ok(())
    }

    /// Append one row to a declared extra table.
    pub fn write_extra_row(
        &mut self,
        table: &str,
        rowid: i64,
        values: &[WriterValue<'_>],
    ) -> Result<()> {
        let record: Vec<RecordValue<'_>> = values
            .iter()
            .map(|v| match v {
                WriterValue::Null => RecordValue::Null,
                WriterValue::Integer(i) => RecordValue::Integer(*i),
                WriterValue::Float(f) => RecordValue::Float(*f),
                WriterValue::Text(s) => RecordValue::Text(s),
                WriterValue::Blob(b) => RecordValue::Blob(b),
            })
            .collect();
        let payload = encode_record(&record)?;
        let index = self
            .extra_tables
            .iter()
            .position(|t| t.name == table)
            .ok_or_else(|| Error::InvalidInput(format!("extra table {table} not declared")))?;
        self.extra_tables[index]
            .stream
            .push(&mut self.db, rowid, &payload)
    }

    /// Set an MBTiles metadata value. Repeated keys replace the previous value.
    pub fn set_metadata(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.metadata.insert(name.into(), value.into());
    }

    /// Add a pre-compressed MVT/PBF tile addressed using XYZ coordinates.
    /// The payload bytes are stored verbatim; they must already match the
    /// archive codec (gzip by default, see
    /// [`MbtilesWriter::set_tile_compression`]).
    ///
    /// MBTiles stores rows in TMS orientation, so this method flips `y` before
    /// serializing the row. Input must be in deterministic block-major order.
    pub fn write_tile_xyz(&mut self, zoom: u8, x: u32, y: u32, data: &[u8]) -> Result<()> {
        let axis = 1_u32
            .checked_shl(u32::from(zoom))
            .ok_or_else(|| Error::InvalidInput(format!("zoom {zoom} is too large")))?;
        if x >= axis || y >= axis {
            return Err(Error::InvalidInput(format!(
                "tile z{zoom}/{x}/{y} lies outside the XYZ pyramid"
            )));
        }

        let rowid = tile_rowid_xyz(zoom, x, y).ok_or_else(|| {
            Error::InvalidInput(format!("tile z{zoom}/{x}/{y} cannot be represented as a rowid"))
        })?;
        let tms_y = axis - 1 - y;
        let payload = encode_record(&[
            RecordValue::Integer(i64::from(zoom)),
            RecordValue::Integer(i64::from(x)),
            RecordValue::Integer(i64::from(tms_y)),
            RecordValue::Blob(data),
        ])?;
        self.tiles.push(&mut self.db, rowid, &payload)?;
        self.tile_count += 1;
        self.tile_bytes = self
            .tile_bytes
            .checked_add(data.len() as u64)
            .ok_or(Error::InvalidWriterState("tile byte count overflow"))?;
        Ok(())
    }

    /// Finalize both table B-trees, write sqlite_master and the database header,
    /// flush the file, and return aggregate statistics.
    pub fn finish(mut self) -> Result<MbtilesWriterStats> {
        self.metadata.insert(
            MAKEPAD_ROWID_SCHEME_KEY.to_string(),
            MAKEPAD_ROWID_SCHEME_VALUE.to_string(),
        );

        let tiles_root = self.tiles.finish(&mut self.db)?;

        let mut metadata_table = TableStream::new(&mut self.db)?;
        for (index, (name, value)) in self.metadata.iter().enumerate() {
            let payload =
                encode_record(&[RecordValue::Text(name), RecordValue::Text(value)])?;
            metadata_table.push(&mut self.db, index as i64 + 1, &payload)?;
        }
        let metadata_root = metadata_table.finish(&mut self.db)?;

        let mut extra_roots = Vec::new();
        for table in std::mem::take(&mut self.extra_tables) {
            let root = table.stream.finish(&mut self.db)?;
            extra_roots.push((table.name, table.sql, root));
        }

        self.db
            .write_sqlite_master(metadata_root, tiles_root, &extra_roots)?;
        self.db.finish()?;

        Ok(MbtilesWriterStats {
            tile_count: self.tile_count,
            tile_bytes: self.tile_bytes,
            page_count: self.db.page_count,
            file_bytes: u64::from(self.db.page_count) * PAGE_SIZE as u64,
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct PageRef {
    page_num: u32,
    max_rowid: i64,
}

struct RawDbWriter {
    file: File,
    page_count: u32,
}

impl RawDbWriter {
    fn create(path: &Path) -> Result<Self> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path)?;
        file.write_all(&vec![0; PAGE_SIZE])?;
        Ok(Self {
            file,
            page_count: 1,
        })
    }

    fn allocate_page(&mut self) -> Result<u32> {
        self.page_count = self
            .page_count
            .checked_add(1)
            .ok_or(Error::InvalidWriterState("SQLite page number overflow"))?;
        // The page spanning byte offset 1 GiB is SQLite's lock-byte page: it
        // must exist in the file but can never be referenced by any b-tree.
        // Skipping it here leaves it zero-filled and unreferenced; without
        // this, every database larger than 1 GiB fails integrity checks with
        // "2nd reference to page 16385".
        if self.page_count == LOCK_BYTE_PAGE {
            self.page_count = self
                .page_count
                .checked_add(1)
                .ok_or(Error::InvalidWriterState("SQLite page number overflow"))?;
        }
        Ok(self.page_count)
    }

    fn write_page(&mut self, page_num: u32, page: &[u8]) -> Result<()> {
        if page.len() != PAGE_SIZE {
            return Err(Error::InvalidWriterState(
                "attempted to write a page with the wrong size",
            ));
        }
        if page_num == 0 || page_num > self.page_count {
            return Err(Error::InvalidWriterState(
                "attempted to write an unallocated page",
            ));
        }
        let offset = u64::from(page_num - 1) * PAGE_SIZE as u64;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(page)?;
        Ok(())
    }

    fn write_sqlite_master(
        &mut self,
        metadata_root: u32,
        tiles_root: u32,
        extra_tables: &[(String, String, u32)],
    ) -> Result<()> {
        let metadata_sql = "CREATE TABLE metadata (name TEXT, value TEXT)";
        let tiles_sql =
            "CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB)";

        let mut rows = vec![
            encode_record(&[
                RecordValue::Text("table"),
                RecordValue::Text("metadata"),
                RecordValue::Text("metadata"),
                RecordValue::Integer(i64::from(metadata_root)),
                RecordValue::Text(metadata_sql),
            ])?,
            encode_record(&[
                RecordValue::Text("table"),
                RecordValue::Text("tiles"),
                RecordValue::Text("tiles"),
                RecordValue::Integer(i64::from(tiles_root)),
                RecordValue::Text(tiles_sql),
            ])?,
        ];
        for (name, sql, root) in extra_tables {
            rows.push(encode_record(&[
                RecordValue::Text("table"),
                RecordValue::Text(name),
                RecordValue::Text(name),
                RecordValue::Integer(i64::from(*root)),
                RecordValue::Text(sql),
            ])?);
        }

        let indexed: Vec<(i64, &[u8])> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| (i as i64 + 1, r.as_slice()))
            .collect();
        let mut page = build_leaf_page(100, &indexed)?;
        write_database_header(&mut page, self.page_count);
        self.write_page(1, &page)
    }

    fn finish(&mut self) -> Result<()> {
        let file_len = u64::from(self.page_count) * PAGE_SIZE as u64;
        self.file.set_len(file_len)?;
        self.file.sync_all()?;
        Ok(())
    }
}

struct LeafPage {
    page_num: u32,
    bytes: Vec<u8>,
    cell_count: u16,
    content_start: usize,
    max_rowid: Option<i64>,
}

impl LeafPage {
    fn new(db: &mut RawDbWriter) -> Result<Self> {
        Ok(Self {
            page_num: db.allocate_page()?,
            bytes: vec![0; PAGE_SIZE],
            cell_count: 0,
            content_start: PAGE_SIZE,
            max_rowid: None,
        })
    }

    fn can_fit(&self, cell_len: usize) -> bool {
        let pointer_end = 8 + (usize::from(self.cell_count) + 1) * 2;
        self.content_start >= pointer_end.saturating_add(cell_len)
    }

    fn push_cell(&mut self, rowid: i64, cell: &[u8]) -> Result<()> {
        if !self.can_fit(cell.len()) {
            return Err(Error::InvalidWriterState("cell does not fit leaf page"));
        }
        self.content_start -= cell.len();
        self.bytes[self.content_start..self.content_start + cell.len()].copy_from_slice(cell);

        let pointer_offset = 8 + usize::from(self.cell_count) * 2;
        let cell_offset = u16::try_from(self.content_start)
            .map_err(|_| Error::InvalidWriterState("leaf cell offset exceeds u16"))?;
        self.bytes[pointer_offset..pointer_offset + 2]
            .copy_from_slice(&cell_offset.to_be_bytes());

        self.cell_count += 1;
        self.max_rowid = Some(rowid);
        Ok(())
    }

    fn flush(mut self, db: &mut RawDbWriter) -> Result<PageRef> {
        self.bytes[0] = 13;
        self.bytes[1..3].copy_from_slice(&0_u16.to_be_bytes());
        self.bytes[3..5].copy_from_slice(&self.cell_count.to_be_bytes());
        let content_offset = if self.content_start == PAGE_SIZE {
            0
        } else {
            u16::try_from(self.content_start)
                .map_err(|_| Error::InvalidWriterState("leaf content offset exceeds u16"))?
        };
        self.bytes[5..7].copy_from_slice(&content_offset.to_be_bytes());
        self.bytes[7] = 0;
        db.write_page(self.page_num, &self.bytes)?;
        Ok(PageRef {
            page_num: self.page_num,
            max_rowid: self.max_rowid.unwrap_or(0),
        })
    }
}

struct TableStream {
    current: LeafPage,
    leaves: Vec<PageRef>,
    last_rowid: Option<i64>,
}

impl TableStream {
    fn new(db: &mut RawDbWriter) -> Result<Self> {
        Ok(Self {
            current: LeafPage::new(db)?,
            leaves: Vec::new(),
            last_rowid: None,
        })
    }

    fn push(&mut self, db: &mut RawDbWriter, rowid: i64, payload: &[u8]) -> Result<()> {
        if rowid <= 0 {
            return Err(Error::InvalidInput(format!(
                "SQLite rowid must be positive, got {rowid}"
            )));
        }
        if self.last_rowid.is_some_and(|last| rowid <= last) {
            return Err(Error::InvalidInput(format!(
                "rows are not strictly ordered: {rowid} follows {}",
                self.last_rowid.unwrap()
            )));
        }

        let local_cell_len = table_leaf_cell_len(payload.len(), rowid)?;
        if !self.current.can_fit(local_cell_len) {
            let replacement = LeafPage::new(db)?;
            let old = std::mem::replace(&mut self.current, replacement);
            self.leaves.push(old.flush(db)?);
        }

        let cell = encode_table_leaf_cell(db, payload, rowid)?;
        self.current.push_cell(rowid, &cell)?;
        self.last_rowid = Some(rowid);
        Ok(())
    }

    fn finish(mut self, db: &mut RawDbWriter) -> Result<u32> {
        self.leaves.push(self.current.flush(db)?);
        build_table_root(db, self.leaves)
    }
}

fn table_leaf_cell_len(payload_len: usize, rowid: i64) -> Result<usize> {
    let usable_size = PAGE_SIZE;
    let (_, local_size) = local_payload_size(payload_len, PageType::TableLeaf, usable_size);
    let payload_len_u64 = u64::try_from(payload_len)
        .map_err(|_| Error::InvalidInput("payload length exceeds u64".to_string()))?;
    Ok(varint_len(payload_len_u64) + varint_len(rowid as u64) + local_size)
}

fn encode_table_leaf_cell(db: &mut RawDbWriter, payload: &[u8], rowid: i64) -> Result<Vec<u8>> {
    let (overflows, local_size) =
        local_payload_size(payload.len(), PageType::TableLeaf, PAGE_SIZE);
    let local_data_len = if overflows {
        local_size
            .checked_sub(4)
            .ok_or(Error::InvalidWriterState("invalid local payload size"))?
    } else {
        local_size
    };

    let mut cell = Vec::with_capacity(
        varint_len(payload.len() as u64) + varint_len(rowid as u64) + local_size,
    );
    write_varint(payload.len() as u64, &mut cell);
    write_varint(rowid as u64, &mut cell);
    cell.extend_from_slice(&payload[..local_data_len]);

    if overflows {
        let first_overflow = write_overflow_pages(db, &payload[local_data_len..])?;
        cell.extend_from_slice(&first_overflow.to_be_bytes());
    }
    Ok(cell)
}

fn write_overflow_pages(db: &mut RawDbWriter, payload: &[u8]) -> Result<u32> {
    if payload.is_empty() {
        return Err(Error::InvalidWriterState(
            "overflow chain requested for an empty payload",
        ));
    }
    let chunk_size = PAGE_SIZE - 4;
    let count = payload.len().div_ceil(chunk_size);
    let mut pages = Vec::with_capacity(count);
    for _ in 0..count {
        pages.push(db.allocate_page()?);
    }

    for (index, chunk) in payload.chunks(chunk_size).enumerate() {
        let mut page = vec![0; PAGE_SIZE];
        let next = pages.get(index + 1).copied().unwrap_or(0);
        page[0..4].copy_from_slice(&next.to_be_bytes());
        page[4..4 + chunk.len()].copy_from_slice(chunk);
        db.write_page(pages[index], &page)?;
    }
    Ok(pages[0])
}

fn build_table_root(db: &mut RawDbWriter, mut children: Vec<PageRef>) -> Result<u32> {
    if children.is_empty() {
        return Err(Error::InvalidWriterState("table has no leaf page"));
    }
    while children.len() > 1 {
        children = write_interior_level(db, &children)?;
    }
    Ok(children[0].page_num)
}

fn write_interior_level(db: &mut RawDbWriter, children: &[PageRef]) -> Result<Vec<PageRef>> {
    let mut parents = Vec::new();
    let mut start = 0;

    while start < children.len() {
        let mut end = start + 1;
        let mut pointer_bytes = 0_usize;
        let mut content_bytes = 0_usize;

        while end < children.len() {
            let separator_len = 4 + varint_len(children[end - 1].max_rowid as u64);
            let next_pointer_bytes = pointer_bytes + 2;
            let next_content_bytes = content_bytes + separator_len;
            if 12 + next_pointer_bytes + next_content_bytes > PAGE_SIZE {
                break;
            }
            pointer_bytes = next_pointer_bytes;
            content_bytes = next_content_bytes;
            end += 1;
        }

        let remaining = children.len() - end;
        if remaining == 1 && end - start > 2 {
            end -= 1;
        }
        if end - start < 2 {
            return Err(Error::InvalidWriterState(
                "could not fit two children in an interior page",
            ));
        }

        parents.push(write_interior_page(db, &children[start..end])?);
        start = end;
    }

    Ok(parents)
}

fn write_interior_page(db: &mut RawDbWriter, children: &[PageRef]) -> Result<PageRef> {
    let page_num = db.allocate_page()?;
    let mut page = vec![0; PAGE_SIZE];
    page[0] = 5;
    page[1..3].copy_from_slice(&0_u16.to_be_bytes());
    let cell_count = u16::try_from(children.len() - 1)
        .map_err(|_| Error::InvalidWriterState("too many interior cells"))?;
    page[3..5].copy_from_slice(&cell_count.to_be_bytes());
    page[8..12].copy_from_slice(&children.last().unwrap().page_num.to_be_bytes());

    let mut content_start = PAGE_SIZE;
    for (index, child) in children[..children.len() - 1].iter().enumerate() {
        let mut cell = Vec::with_capacity(13);
        cell.extend_from_slice(&child.page_num.to_be_bytes());
        write_varint(child.max_rowid as u64, &mut cell);
        content_start -= cell.len();
        page[content_start..content_start + cell.len()].copy_from_slice(&cell);
        let pointer_offset = 12 + index * 2;
        let cell_offset = u16::try_from(content_start)
            .map_err(|_| Error::InvalidWriterState("interior cell offset exceeds u16"))?;
        page[pointer_offset..pointer_offset + 2].copy_from_slice(&cell_offset.to_be_bytes());
    }
    let content_offset = u16::try_from(content_start)
        .map_err(|_| Error::InvalidWriterState("interior content offset exceeds u16"))?;
    page[5..7].copy_from_slice(&content_offset.to_be_bytes());
    page[7] = 0;
    db.write_page(page_num, &page)?;

    Ok(PageRef {
        page_num,
        max_rowid: children.last().unwrap().max_rowid,
    })
}

enum RecordValue<'a> {
    Null,
    Integer(i64),
    Float(f64),
    Blob(&'a [u8]),
    Text(&'a str),
}

fn encode_record(values: &[RecordValue<'_>]) -> Result<Vec<u8>> {
    let mut serials = Vec::with_capacity(values.len());
    let mut body = Vec::new();

    for value in values {
        let serial_type = match value {
            RecordValue::Null => 0,
            RecordValue::Float(value) => {
                body.extend_from_slice(&value.to_be_bytes());
                7
            }
            RecordValue::Integer(value) => encode_integer(*value, &mut body),
            RecordValue::Blob(value) => {
                body.extend_from_slice(value);
                12_u64
                    .checked_add(
                        u64::try_from(value.len())
                            .map_err(|_| {
                                Error::InvalidInput("blob length exceeds u64".to_string())
                            })?
                            .checked_mul(2)
                            .ok_or_else(|| {
                                Error::InvalidInput("blob serial type overflow".to_string())
                            })?,
                    )
                    .ok_or_else(|| Error::InvalidInput("blob serial type overflow".to_string()))?
            }
            RecordValue::Text(value) => {
                body.extend_from_slice(value.as_bytes());
                13_u64
                    .checked_add(
                        u64::try_from(value.len())
                            .map_err(|_| {
                                Error::InvalidInput("text length exceeds u64".to_string())
                            })?
                            .checked_mul(2)
                            .ok_or_else(|| {
                                Error::InvalidInput("text serial type overflow".to_string())
                            })?,
                    )
                    .ok_or_else(|| Error::InvalidInput("text serial type overflow".to_string()))?
            }
        };
        write_varint(serial_type, &mut serials);
    }

    let mut header_size = serials.len() + 1;
    loop {
        let next = serials.len() + varint_len(header_size as u64);
        if next == header_size {
            break;
        }
        header_size = next;
    }

    let mut payload = Vec::with_capacity(header_size + body.len());
    write_varint(header_size as u64, &mut payload);
    payload.extend_from_slice(&serials);
    payload.extend_from_slice(&body);
    Ok(payload)
}

fn encode_integer(value: i64, body: &mut Vec<u8>) -> u64 {
    match value {
        0 => 8,
        1 => 9,
        value if i8::try_from(value).is_ok() => {
            body.push(value as i8 as u8);
            1
        }
        value if i16::try_from(value).is_ok() => {
            body.extend_from_slice(&(value as i16).to_be_bytes());
            2
        }
        value if (-8_388_608..=8_388_607).contains(&value) => {
            body.extend_from_slice(&value.to_be_bytes()[5..]);
            3
        }
        value if i32::try_from(value).is_ok() => {
            body.extend_from_slice(&(value as i32).to_be_bytes());
            4
        }
        value if (-140_737_488_355_328..=140_737_488_355_327).contains(&value) => {
            body.extend_from_slice(&value.to_be_bytes()[2..]);
            5
        }
        value => {
            body.extend_from_slice(&value.to_be_bytes());
            6
        }
    }
}

fn build_leaf_page(header_offset: usize, rows: &[(i64, &[u8])]) -> Result<Vec<u8>> {
    let mut page = vec![0; PAGE_SIZE];
    let mut content_start = PAGE_SIZE;

    for (index, (rowid, payload)) in rows.iter().enumerate() {
        let mut cell = Vec::new();
        write_varint(payload.len() as u64, &mut cell);
        write_varint(*rowid as u64, &mut cell);
        cell.extend_from_slice(payload);

        let pointer_end = header_offset + 8 + (index + 1) * 2;
        if content_start < pointer_end + cell.len() {
            return Err(Error::InvalidWriterState(
                "sqlite_master does not fit on page 1",
            ));
        }
        content_start -= cell.len();
        page[content_start..content_start + cell.len()].copy_from_slice(&cell);
        let pointer_offset = header_offset + 8 + index * 2;
        let cell_offset = u16::try_from(content_start)
            .map_err(|_| Error::InvalidWriterState("master cell offset exceeds u16"))?;
        page[pointer_offset..pointer_offset + 2].copy_from_slice(&cell_offset.to_be_bytes());
    }

    page[header_offset] = 13;
    page[header_offset + 1..header_offset + 3].copy_from_slice(&0_u16.to_be_bytes());
    page[header_offset + 3..header_offset + 5]
        .copy_from_slice(&(rows.len() as u16).to_be_bytes());
    let content_offset = if rows.is_empty() {
        0
    } else {
        u16::try_from(content_start)
            .map_err(|_| Error::InvalidWriterState("master content offset exceeds u16"))?
    };
    page[header_offset + 5..header_offset + 7]
        .copy_from_slice(&content_offset.to_be_bytes());
    page[header_offset + 7] = 0;
    Ok(page)
}

fn write_database_header(page: &mut [u8], page_count: u32) {
    page[0..16].copy_from_slice(SQLITE_MAGIC);
    page[16..18].copy_from_slice(&1_u16.to_be_bytes());
    page[18] = 1;
    page[19] = 1;
    page[20] = 0;
    page[21] = 64;
    page[22] = 32;
    page[23] = 32;
    page[24..28].copy_from_slice(&1_u32.to_be_bytes());
    page[28..32].copy_from_slice(&page_count.to_be_bytes());
    page[32..40].fill(0);
    page[40..44].copy_from_slice(&2_u32.to_be_bytes());
    page[44..48].copy_from_slice(&4_u32.to_be_bytes());
    page[48..56].fill(0);
    page[56..60].copy_from_slice(&1_u32.to_be_bytes());
    page[60..68].fill(0);
    page[68..72].copy_from_slice(&SQLITE_APPLICATION_ID_MBTILES.to_be_bytes());
    page[72..92].fill(0);
    page[92..96].copy_from_slice(&1_u32.to_be_bytes());
    page[96..100].copy_from_slice(&3_045_000_u32.to_be_bytes());
}

fn varint_len(value: u64) -> usize {
    if value > 0x00ff_ffff_ffff_ffff {
        9
    } else {
        let bits = 64 - value.leading_zeros() as usize;
        (bits.max(1) + 6) / 7
    }
}

fn write_varint(value: u64, output: &mut Vec<u8>) {
    if value > 0x00ff_ffff_ffff_ffff {
        for shift in [57, 50, 43, 36, 29, 22, 15, 8] {
            output.push(((value >> shift) as u8 & 0x7f) | 0x80);
        }
        output.push(value as u8);
        return;
    }

    let len = varint_len(value);
    for index in (0..len).rev() {
        let mut byte = ((value >> (index * 7)) & 0x7f) as u8;
        if index != 0 {
            byte |= 0x80;
        }
        output.push(byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MbtilesReader;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{name}-{}-{nonce}.mbtiles", std::process::id()))
    }

    #[test]
    fn varints_round_trip_with_reader() {
        for value in [
            0,
            1,
            127,
            128,
            16_383,
            16_384,
            0x00ff_ffff_ffff_ffff,
            u64::MAX,
        ] {
            let mut bytes = Vec::new();
            write_varint(value, &mut bytes);
            assert_eq!(
                makepad_sqlite::value::read_varint(&bytes).unwrap(),
                (value, bytes.len())
            );
        }
    }

    #[test]
    fn rowids_follow_block_major_order() {
        let mut last = 0;
        for block_y in 0..2 {
            for block_x in 0..2 {
                for local_y in 0..256 {
                    for local_x in 0..256 {
                        let x = block_x * 256 + local_x;
                        let y = block_y * 256 + local_y;
                        let rowid = tile_rowid_xyz(9, x, y).unwrap();
                        assert!(rowid > last);
                        last = rowid;
                    }
                }
            }
        }
    }

    #[test]
    fn writer_round_trips_metadata_tiles_and_overflow() {
        let path = temp_path("makepad-mbtiles-writer");
        let mut writer = MbtilesWriter::create(&path).unwrap();
        writer.set_metadata("name", "writer test");
        writer.set_metadata("format", "pbf");

        let small = vec![1, 2, 3, 4];
        let large = (0..200_000).map(|n| (n % 251) as u8).collect::<Vec<_>>();
        writer.write_tile_xyz(0, 0, 0, &small).unwrap();
        writer.write_tile_xyz(9, 0, 0, &large).unwrap();
        writer.write_tile_xyz(9, 1, 0, &small).unwrap();
        let stats = writer.finish().unwrap();
        assert_eq!(stats.tile_count, 3);
        assert_eq!(stats.tile_bytes, (small.len() * 2 + large.len()) as u64);

        let mut reader = MbtilesReader::open(&path).unwrap();
        let metadata = reader.get_metadata().unwrap();
        assert_eq!(metadata.get("name").map(String::as_str), Some("writer test"));
        assert_eq!(
            metadata.get(MAKEPAD_ROWID_SCHEME_KEY).map(String::as_str),
            Some(MAKEPAD_ROWID_SCHEME_VALUE)
        );
        assert_eq!(reader.get_tile(0, 0, 0).unwrap().unwrap(), small);
        let tms_y = (1_i64 << 9) - 1;
        assert_eq!(reader.get_tile(9, 0, tms_y).unwrap().unwrap(), large);
        assert_eq!(
            reader.tile_summary().unwrap(),
            vec![(0, 1), (9, 2)]
        );

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn brotli_archive_round_trips_with_and_without_dict() {
        let raw_tile: Vec<u8> = b"streets water_polygons buildings place_labels "
            .iter()
            .copied()
            .cycle()
            .take(20_000)
            .collect();
        for dict in [None, Some(b"streets water_polygons buildings".to_vec())] {
            let path = temp_path("makepad-mbtiles-brotli");
            let mut writer = MbtilesWriter::create(&path).unwrap();
            writer.set_metadata("name", "brotli test");
            writer.set_tile_compression(TileCompression::Brotli { quality: 9 }, dict.clone());
            writer.write_tile_encoded(5, 3, 4, &raw_tile).unwrap();
            // Worker-thread style: pre-compress and store verbatim.
            let pre = crate::compress_tile(
                &TileCompression::Brotli { quality: 9 },
                dict.as_deref(),
                &raw_tile,
            )
            .unwrap();
            writer.write_tile_xyz(5, 4, 4, &pre).unwrap();
            writer.finish().unwrap();

            let mut reader = MbtilesReader::open(&path).unwrap();
            let metadata = reader.get_metadata().unwrap();
            let expected = if dict.is_some() { "br:dict-v1" } else { "br" };
            assert_eq!(
                metadata.get("compression").map(String::as_str),
                Some(expected)
            );
            assert!(reader.tile_codec().is_brotli());
            assert_eq!(reader.tile_codec().dict(), dict.as_deref());
            let tms_row = (1_i64 << 5) - 1 - 4;
            for x in [3_i64, 4] {
                let stored = reader.get_tile(5, x, tms_row).unwrap().unwrap();
                assert_ne!(stored, raw_tile);
                assert_eq!(reader.decode_tile(&stored).unwrap(), raw_tile);
                assert_eq!(
                    reader.get_tile_decoded(5, x, tms_row).unwrap().unwrap(),
                    raw_tile
                );
            }
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn gzip_archives_stay_readable_without_compression_metadata() {
        let path = temp_path("makepad-mbtiles-gzip-default");
        let raw_tile = b"legacy gzip archive tile payload".repeat(64);
        let mut writer = MbtilesWriter::create(&path).unwrap();
        writer.write_tile_encoded(3, 1, 2, &raw_tile).unwrap();
        writer.finish().unwrap();

        let mut reader = MbtilesReader::open(&path).unwrap();
        // Gzip is the default; no compression row is required for it.
        assert!(!reader.tile_codec().is_brotli());
        let tms_row = (1_i64 << 3) - 1 - 2;
        let stored = reader.get_tile(3, 1, tms_row).unwrap().unwrap();
        assert_eq!(&stored[..2], &[0x1f, 0x8b]);
        assert_eq!(reader.decode_tile(&stored).unwrap(), raw_tile);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn writer_builds_and_searches_interior_table_pages() {
        let path = temp_path("makepad-mbtiles-interior");
        let mut writer = MbtilesWriter::create(&path).unwrap();
        writer.set_metadata("name", "interior page test");
        let data = vec![0x5a; 1024];
        for y in 0..64 {
            for x in 0..64 {
                writer.write_tile_xyz(6, x, y, &data).unwrap();
            }
        }
        let stats = writer.finish().unwrap();
        assert_eq!(stats.tile_count, 4096);

        let mut reader = MbtilesReader::open(&path).unwrap();
        for (x, xyz_y) in [(0, 0), (31, 17), (63, 63)] {
            let tms_y = 63 - xyz_y;
            assert_eq!(reader.get_tile(6, x, tms_y).unwrap().unwrap(), data);
        }

        std::fs::remove_file(path).unwrap();
    }
}
