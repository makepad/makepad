//! Reader for the `.mkmap` sharded tile container (the transmux output in
//! tools/map_tiles/src/mkmap.rs — that file owns the writer and the format
//! constants below must match it byte for byte).
//!
//! Layout: `root.mkidx` = 112-byte header (magic, version, shard count/cap,
//! tile count, zoom range, then (offset, len) slot pairs for the metadata
//! varint-KV section (brotli), shared dict, raw root directory and its
//! brotli copy),
//! followed by those sections. The root directory is fixed 36-byte records
//! mapping Hilbert tile-id ranges to a leaf directory span inside a shard;
//! leaves are brotli-packed delta varints of (tile_id, shard, offset, len).
//! Shards (`tiles-NNN.mkshard`) hold the source archive's compressed tile
//! blobs verbatim, so tile decode reuses the same `TileCodec` the mbtiles
//! path uses. All reads are positioned (`pread`-style via seek+read on a
//! per-shard `File`), which serves both the local mmap-equivalent case and,
//! later, an HTTP range client with the same access pattern.

use crate::{Error, Result, TileCodec};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"MKMAPIX1";
// v2: metadata section is varint KV (was JSON).
const VERSION: u32 = 2;
const HEADER_LEN: usize = 112;
const ROOT_RECORD_LEN: usize = 36;

// --- Hilbert tile ids (identical to the writer) ---

fn hilbert_rotate(side: u32, x: &mut u32, y: &mut u32, rx: u32, ry: u32) {
    if ry == 0 {
        if rx == 1 {
            *x = side - 1 - *x;
            *y = side - 1 - *y;
        }
        std::mem::swap(x, y);
    }
}

fn hilbert_xy_to_d(zoom: u8, mut x: u32, mut y: u32) -> u64 {
    let side = 1_u32 << zoom;
    let mut rx;
    let mut ry;
    let mut d = 0_u64;
    let mut s = side >> 1;
    while s > 0 {
        rx = u32::from(x & s > 0);
        ry = u32::from(y & s > 0);
        d += u64::from(s) * u64::from(s) * u64::from((3 * rx) ^ ry);
        hilbert_rotate(side, &mut x, &mut y, rx, ry);
        s >>= 1;
    }
    d
}

fn zoom_base_id(zoom: u8) -> u64 {
    // Sum of 4^z for z < zoom: each level owns a contiguous id band.
    ((1_u64 << (2 * zoom)) - 1) / 3
}

/// Global tile id: zoom band base + Hilbert distance within the zoom.
pub fn mkmap_tile_id(zoom: u8, x: u32, y: u32) -> u64 {
    zoom_base_id(zoom) + hilbert_xy_to_d(zoom, x, y)
}

fn read_varint(input: &[u8], offset: &mut usize) -> Result<u64> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    loop {
        let byte = *input.get(*offset).ok_or(Error::CorruptVarint)?;
        *offset += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift > 63 {
            return Err(Error::CorruptVarint);
        }
    }
}

/// Metadata section: varint KV pairs (count, then length-prefixed key and
/// value bytes per pair) — the same primitive the leaf directories use.
fn parse_metadata_kv(bytes: &[u8]) -> Result<HashMap<String, String>> {
    let corrupt = || Error::CorruptRecord("mkmap metadata kv");
    let mut cursor = 0_usize;
    let count = read_varint(bytes, &mut cursor)? as usize;
    let mut out = HashMap::with_capacity(count);
    let mut read_string = |cursor: &mut usize| -> Result<String> {
        let len = read_varint(bytes, cursor)? as usize;
        let slice = bytes.get(*cursor..*cursor + len).ok_or_else(corrupt)?;
        *cursor += len;
        String::from_utf8(slice.to_vec()).map_err(|_| corrupt())
    };
    for _ in 0..count {
        let key = read_string(&mut cursor)?;
        let value = read_string(&mut cursor)?;
        out.insert(key, value);
    }
    Ok(out)
}

#[derive(Clone, Copy)]
struct BlobRef {
    shard: u32,
    offset: u64,
    len: u64,
}

struct LeafEntry {
    tile_id: u64,
    blob: BlobRef,
}

struct RootRecord {
    start_tile_id: u64,
    end_tile_id: u64,
    shard: u32,
    dir_offset: u64,
    dir_len: u64,
}

/// Positioned-read `.mkmap` consumer with the same surface the tile loader
/// uses on `MbtilesReader`: metadata + per-tile decoded bytes.
pub struct MkmapReader {
    dir: PathBuf,
    metadata: HashMap<String, String>,
    codec: TileCodec,
    root: Vec<RootRecord>,
    min_zoom: u8,
    max_zoom: u8,
    /// Decoded leaf directories, keyed by root record index. A viewport's
    /// tiles are Hilbert-adjacent, so a handful of leaves covers a session.
    leaf_cache: HashMap<usize, Vec<LeafEntry>>,
    shard_files: HashMap<u32, File>,
}

impl MkmapReader {
    /// `path` may be the container directory or its `root.mkidx`.
    pub fn open(path: &Path) -> Result<MkmapReader> {
        let dir = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(Path::new(".")).to_path_buf()
        };
        let bytes = std::fs::read(dir.join("root.mkidx")).map_err(Error::Io)?;
        if bytes.len() < HEADER_LEN || &bytes[0..8] != MAGIC {
            return Err(Error::InvalidMagic);
        }
        let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if version != VERSION {
            return Err(Error::CorruptRecord("mkmap index version"));
        }
        let read_u64 = |slot: usize| u64::from_le_bytes(bytes[slot..slot + 8].try_into().unwrap());
        let min_zoom = bytes[40];
        let max_zoom = bytes[41];
        let section = |slot: usize| -> Result<&[u8]> {
            let offset = read_u64(slot) as usize;
            let len = read_u64(slot + 8) as usize;
            bytes
                .get(offset..offset + len)
                .ok_or(Error::CorruptRecord("mkmap section bounds"))
        };
        let brotli = TileCodec::from_metadata(
            &[("compression".to_string(), "br".to_string())]
                .into_iter()
                .collect(),
        )?;
        let metadata_bytes = brotli.decode(section(48)?)?;
        let metadata = parse_metadata_kv(&metadata_bytes)?;
        // Tile blobs are the source archive's bytes verbatim: build the
        // tile codec from the carried metadata (compression + dict).
        let codec = TileCodec::from_metadata(&metadata)?;
        let root_raw = section(80)?;
        if root_raw.len() % ROOT_RECORD_LEN != 0 {
            return Err(Error::CorruptRecord("mkmap root alignment"));
        }
        let mut root = Vec::with_capacity(root_raw.len() / ROOT_RECORD_LEN);
        for record in root_raw.chunks_exact(ROOT_RECORD_LEN) {
            root.push(RootRecord {
                start_tile_id: u64::from_le_bytes(record[0..8].try_into().unwrap()),
                end_tile_id: u64::from_le_bytes(record[8..16].try_into().unwrap()),
                shard: u32::from_le_bytes(record[16..20].try_into().unwrap()),
                dir_offset: u64::from_le_bytes(record[20..28].try_into().unwrap()),
                dir_len: u64::from_le_bytes(record[28..36].try_into().unwrap()),
            });
        }
        Ok(MkmapReader {
            dir,
            metadata,
            codec,
            root,
            min_zoom,
            max_zoom,
            leaf_cache: HashMap::new(),
            shard_files: HashMap::new(),
        })
    }

    pub fn get_metadata(&mut self) -> Result<HashMap<String, String>> {
        Ok(self.metadata.clone())
    }

    pub fn zoom_range(&self) -> (u32, u32) {
        (u32::from(self.min_zoom), u32::from(self.max_zoom))
    }

    fn read_range(&mut self, shard: u32, offset: u64, len: u64) -> Result<Vec<u8>> {
        if !self.shard_files.contains_key(&shard) {
            let path = self.dir.join(format!("tiles-{shard:03}.mkshard"));
            self.shard_files
                .insert(shard, File::open(path).map_err(Error::Io)?);
        }
        let file = self.shard_files.get_mut(&shard).unwrap();
        file.seek(SeekFrom::Start(offset)).map_err(Error::Io)?;
        let mut bytes = vec![0_u8; len as usize];
        file.read_exact(&mut bytes).map_err(Error::Io)?;
        Ok(bytes)
    }

    fn resolve(&mut self, zoom: u8, x: u32, y: u32) -> Result<Option<BlobRef>> {
        let id = mkmap_tile_id(zoom, x, y);
        let record_index = match self.root.binary_search_by(|record| {
            if id < record.start_tile_id {
                std::cmp::Ordering::Greater
            } else if id > record.end_tile_id {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        }) {
            Ok(index) => index,
            Err(_) => return Ok(None),
        };
        if !self.leaf_cache.contains_key(&record_index) {
            let record = &self.root[record_index];
            let (shard, offset, len) = (record.shard, record.dir_offset, record.dir_len);
            let packed = self.read_range(shard, offset, len)?;
            let raw = TileCodec::from_metadata(
                &[("compression".to_string(), "br".to_string())]
                    .into_iter()
                    .collect(),
            )?
            .decode(&packed)?;
            let mut cursor = 0_usize;
            let count = read_varint(&raw, &mut cursor)? as usize;
            let mut entries = Vec::with_capacity(count);
            let mut tile_id = 0_u64;
            for _ in 0..count {
                tile_id += read_varint(&raw, &mut cursor)?;
                let shard = u32::try_from(read_varint(&raw, &mut cursor)?)
                    .map_err(|_| Error::CorruptRecord("mkmap leaf shard"))?;
                let blob_offset = read_varint(&raw, &mut cursor)?;
                let len = read_varint(&raw, &mut cursor)?;
                entries.push(LeafEntry {
                    tile_id,
                    blob: BlobRef {
                        shard,
                        offset: blob_offset,
                        len,
                    },
                });
            }
            if self.leaf_cache.len() > 16 {
                self.leaf_cache.clear();
            }
            self.leaf_cache.insert(record_index, entries);
        }
        let entries = &self.leaf_cache[&record_index];
        Ok(entries
            .binary_search_by_key(&id, |entry| entry.tile_id)
            .ok()
            .map(|found| entries[found].blob))
    }

    /// Raw compressed tile bytes at (zoom, column, TMS row) — the same
    /// addressing `MbtilesReader::get_tile` uses.
    pub fn get_tile(&mut self, zoom: i64, column: i64, row: i64) -> Result<Option<Vec<u8>>> {
        let axis = 1_i64 << zoom.clamp(0, 30);
        let y = axis - 1 - row; // TMS -> XYZ (the id space the writer used)
        if zoom < 0 || column < 0 || !(0..axis).contains(&y) || !(0..axis).contains(&column) {
            return Ok(None);
        }
        let Some(blob) = self.resolve(zoom as u8, column as u32, y as u32)? else {
            return Ok(None);
        };
        Ok(Some(self.read_range(blob.shard, blob.offset, blob.len)?))
    }

    pub fn decode_tile(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        self.codec.decode(bytes)
    }

    /// Same (zoom, column, TMS row) addressing and decoded output as
    /// `MbtilesReader::get_tile_decoded`.
    pub fn get_tile_decoded(
        &mut self,
        zoom: i64,
        column: i64,
        row: i64,
    ) -> Result<Option<Vec<u8>>> {
        let Some(bytes) = self.get_tile(zoom, column, row)? else {
            return Ok(None);
        };
        Ok(Some(self.codec.decode(&bytes)?))
    }
}

/// A local tile archive behind one surface: classic mbtiles or the sharded
/// `.mkmap` directory. Sniffed from the path so DSL-configured archive
/// paths can point at either.
pub enum TileArchiveReader {
    Mbtiles(crate::MbtilesReader),
    Mkmap(MkmapReader),
}

impl TileArchiveReader {
    pub fn is_mkmap_path(path: &Path) -> bool {
        path.join("root.mkidx").is_file()
            || path.file_name().is_some_and(|n| n == "root.mkidx")
            || path.extension().is_some_and(|e| e == "mkmap")
    }

    pub fn open(path: &Path) -> Result<TileArchiveReader> {
        if Self::is_mkmap_path(path) {
            Ok(TileArchiveReader::Mkmap(MkmapReader::open(path)?))
        } else {
            Ok(TileArchiveReader::Mbtiles(crate::MbtilesReader::open(path)?))
        }
    }

    pub fn get_metadata(&mut self) -> Result<HashMap<String, String>> {
        match self {
            TileArchiveReader::Mbtiles(reader) => reader.get_metadata(),
            TileArchiveReader::Mkmap(reader) => reader.get_metadata(),
        }
    }

    pub fn get_tile_decoded(
        &mut self,
        zoom: i64,
        column: i64,
        row: i64,
    ) -> Result<Option<Vec<u8>>> {
        match self {
            TileArchiveReader::Mbtiles(reader) => reader.get_tile_decoded(zoom, column, row),
            TileArchiveReader::Mkmap(reader) => reader.get_tile_decoded(zoom, column, row),
        }
    }

    /// The mkmap index is always a direct lookup; mbtiles depends on its
    /// rowid scheme.
    pub fn supports_direct_tile_lookup(&self) -> bool {
        match self {
            TileArchiveReader::Mbtiles(reader) => reader.supports_direct_tile_lookup(),
            TileArchiveReader::Mkmap(_) => true,
        }
    }

    pub fn get_tile(&mut self, zoom: i64, column: i64, row: i64) -> Result<Option<Vec<u8>>> {
        match self {
            TileArchiveReader::Mbtiles(reader) => reader.get_tile(zoom, column, row),
            TileArchiveReader::Mkmap(reader) => reader.get_tile(zoom, column, row),
        }
    }

    pub fn decode_tile(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        match self {
            TileArchiveReader::Mbtiles(reader) => reader.decode_tile(bytes),
            TileArchiveReader::Mkmap(reader) => reader.decode_tile(bytes),
        }
    }

    /// Bulk zoom scan — the fallback for mbtiles without direct rowids.
    /// mkmap always supports direct lookup, so this path is unreachable
    /// there by construction.
    pub fn get_tiles_at_zoom(&mut self, zoom: i64) -> Result<Vec<crate::Tile>> {
        match self {
            TileArchiveReader::Mbtiles(reader) => reader.get_tiles_at_zoom(zoom),
            TileArchiveReader::Mkmap(_) => Err(Error::InvalidInput(
                "mkmap archives use direct tile lookup".to_string(),
            )),
        }
    }
}
