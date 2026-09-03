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
//!
//! Tile ids are self-describing (zoom band + Hilbert position), so the leaf
//! directories are also a complete listing of the container: `for_each_tile_ref`
//! walks them one at a time and recovers every tile's z/x/y without a side
//! table, which is what lets `mkmap-extract` reverse a weave.

use crate::{Error, Result, TileCodec};
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::VecDeque;
#[cfg(not(target_arch = "wasm32"))]
use std::fs::File;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

const MAGIC: &[u8; 8] = b"MKMAPIX1";
#[cfg(not(target_arch = "wasm32"))]
const SHARD_FILE_CACHE_CAPACITY: usize = 8;
// v2: metadata section is varint KV (was JSON).
const VERSION: u32 = 2;
const HEADER_LEN: usize = 112;
const ROOT_RECORD_LEN: usize = 36;
const MAX_METADATA_BYTES: usize = 16 * 1024 * 1024;
// A root record lists every tile of one shard; the densest world shards carry over five
// million tiles, whose decoded refs pass 64 MiB (the repack died on record 185 at that cap).
const MAX_LEAF_BYTES: usize = 512 * 1024 * 1024;
const MAX_TILE_BYTES: usize = 64 * 1024 * 1024;

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

fn hilbert_d_to_xy(zoom: u8, mut d: u64) -> (u32, u32) {
    let side = 1_u32 << zoom;
    let (mut x, mut y) = (0_u32, 0_u32);
    let mut s = 1_u32;
    while s < side {
        let rx = 1 & (d / 2) as u32;
        let ry = 1 & ((d as u32) ^ rx);
        hilbert_rotate(s, &mut x, &mut y, rx, ry);
        x += s * rx;
        y += s * ry;
        d /= 4;
        s <<= 1;
    }
    (x, y)
}

fn zoom_base_id(zoom: u8) -> u64 {
    // Sum of 4^z for z < zoom: each level owns a contiguous id band.
    ((1_u64 << (2 * zoom)) - 1) / 3
}

/// Global tile id: zoom band base + Hilbert distance within the zoom.
pub fn mkmap_tile_id(zoom: u8, x: u32, y: u32) -> u64 {
    zoom_base_id(zoom) + hilbert_xy_to_d(zoom, x, y)
}

/// Inverse of [`mkmap_tile_id`]: the zoom band the id falls in, then the
/// Hilbert position inside it. Ids are self-describing, so a container walk
/// recovers every tile's address without a side table.
pub fn mkmap_zxy_from_tile_id(id: u64) -> (u8, u32, u32) {
    let mut zoom = 0_u8;
    while zoom < 31 && zoom_base_id(zoom + 1) <= id {
        zoom += 1;
    }
    let (x, y) = hilbert_d_to_xy(zoom, id - zoom_base_id(zoom));
    (zoom, x, y)
}

/// One tile's address plus where its blob lives, as handed out by
/// [`MkmapReader::for_each_tile_ref`] / [`MkmapReader::resolve_tile`].
#[derive(Clone, Copy, Debug)]
pub struct MkmapTileRef {
    pub tile_id: u64,
    pub zoom: u8,
    pub x: u32,
    pub y: u32,
    pub shard: u32,
    pub offset: u64,
    pub len: u64,
}

fn read_varint(input: &[u8], offset: &mut usize) -> Result<u64> {
    let mut value = 0_u64;
    for byte_index in 0..10_u32 {
        let byte = *input.get(*offset).ok_or(Error::CorruptVarint)?;
        *offset += 1;
        if byte_index == 9 && byte & 0xfe != 0 {
            return Err(Error::CorruptVarint);
        }
        value |= u64::from(byte & 0x7f) << (byte_index * 7);
        if byte & 0x80 == 0 {
            if byte_index != 0 && byte & 0x7f == 0 {
                return Err(Error::CorruptVarint);
            }
            return Ok(value);
        }
    }
    Err(Error::CorruptVarint)
}

/// Metadata section: varint KV pairs (count, then length-prefixed key and
/// value bytes per pair) — the same primitive the leaf directories use.
fn parse_metadata_kv(bytes: &[u8]) -> Result<HashMap<String, String>> {
    let corrupt = || Error::CorruptRecord("mkmap metadata kv");
    let mut cursor = 0_usize;
    let count = usize::try_from(read_varint(bytes, &mut cursor)?).map_err(|_| corrupt())?;
    if count > bytes.len().saturating_sub(cursor) / 2 {
        return Err(corrupt());
    }
    let mut out = HashMap::with_capacity(count);
    let read_string = |cursor: &mut usize| -> Result<String> {
        let len = usize::try_from(read_varint(bytes, cursor)?).map_err(|_| corrupt())?;
        let end = (*cursor).checked_add(len).ok_or_else(corrupt)?;
        let slice = bytes.get(*cursor..end).ok_or_else(corrupt)?;
        *cursor = end;
        String::from_utf8(slice.to_vec()).map_err(|_| corrupt())
    };
    for _ in 0..count {
        let key = read_string(&mut cursor)?;
        let value = read_string(&mut cursor)?;
        out.insert(key, value);
    }
    if cursor != bytes.len() {
        return Err(corrupt());
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct BlobRef {
    pub shard: u32,
    pub offset: u64,
    pub len: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LeafEntry {
    tile_id: u64,
    blob: BlobRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RootRecord {
    start_tile_id: u64,
    end_tile_id: u64,
    shard: u32,
    dir_offset: u64,
    dir_len: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootRecordRef {
    pub index: usize,
    pub start_tile_id: u64,
    pub end_tile_id: u64,
    pub shard: u32,
    pub dir_offset: u64,
    pub dir_len: u64,
}

/// Parsed, I/O-free `root.mkidx` state shared by local and ranged readers.
#[derive(Clone, Debug)]
pub struct MkmapRoot {
    metadata: HashMap<String, String>,
    codec: TileCodec,
    shared_dict: Vec<u8>,
    records: Vec<RootRecord>,
    min_zoom: u8,
    max_zoom: u8,
    shard_count: u32,
    tile_count: u64,
}

impl MkmapRoot {
    pub fn parse(bytes: &[u8]) -> std::result::Result<MkmapRoot, String> {
        Self::parse_inner(bytes).map_err(|err| err.to_string())
    }

    fn parse_inner(bytes: &[u8]) -> Result<MkmapRoot> {
        if bytes.len() < HEADER_LEN || &bytes[0..8] != MAGIC {
            return Err(Error::InvalidMagic);
        }
        let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if version != VERSION {
            return Err(Error::CorruptRecord("mkmap index version"));
        }
        let read_u64 = |slot: usize| u64::from_le_bytes(bytes[slot..slot + 8].try_into().unwrap());
        let shard_count = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let tile_count = read_u64(24);
        let min_zoom = bytes[40];
        let max_zoom = bytes[41];
        if min_zoom > max_zoom || max_zoom > 30 {
            return Err(Error::CorruptRecord("mkmap zoom range"));
        }
        let section = |slot: usize| -> Result<&[u8]> {
            let offset = usize::try_from(read_u64(slot))
                .map_err(|_| Error::CorruptRecord("mkmap section bounds"))?;
            let len = usize::try_from(read_u64(slot + 8))
                .map_err(|_| Error::CorruptRecord("mkmap section bounds"))?;
            let end = offset
                .checked_add(len)
                .ok_or(Error::CorruptRecord("mkmap section bounds"))?;
            bytes
                .get(offset..end)
                .ok_or(Error::CorruptRecord("mkmap section bounds"))
        };
        let brotli = TileCodec::from_metadata(
            &[("compression".to_string(), "br".to_string())]
                .into_iter()
                .collect(),
        )?;
        let metadata_bytes = brotli.decode_limited(section(48)?, MAX_METADATA_BYTES)?;
        let metadata = parse_metadata_kv(&metadata_bytes)?;
        let codec = TileCodec::from_metadata(&metadata)?;
        let shared_dict = section(64)?.to_vec();
        let root_raw = section(80)?;
        if root_raw.len() % ROOT_RECORD_LEN != 0 {
            return Err(Error::CorruptRecord("mkmap root alignment"));
        }
        if root_raw.len() / ROOT_RECORD_LEN != shard_count as usize
            || (tile_count != 0 && shard_count == 0)
        {
            return Err(Error::CorruptRecord("mkmap header counts"));
        }
        let root_packed = section(96)?;
        if !root_packed.is_empty()
            && brotli.decode_limited(root_packed, root_raw.len())? != root_raw
        {
            return Err(Error::CorruptRecord("mkmap root copies"));
        }
        let mut records = Vec::with_capacity(root_raw.len() / ROOT_RECORD_LEN);
        for record in root_raw.chunks_exact(ROOT_RECORD_LEN) {
            let record = RootRecord {
                start_tile_id: u64::from_le_bytes(record[0..8].try_into().unwrap()),
                end_tile_id: u64::from_le_bytes(record[8..16].try_into().unwrap()),
                shard: u32::from_le_bytes(record[16..20].try_into().unwrap()),
                dir_offset: u64::from_le_bytes(record[20..28].try_into().unwrap()),
                dir_len: u64::from_le_bytes(record[28..36].try_into().unwrap()),
            };
            if record.start_tile_id > record.end_tile_id
                || record.shard >= shard_count
                || record.dir_len == 0
                || record.dir_len > MAX_LEAF_BYTES as u64
                || record.dir_offset.checked_add(record.dir_len).is_none()
                || records
                    .last()
                    .is_some_and(|previous: &RootRecord| {
                        previous.end_tile_id >= record.start_tile_id
                    })
            {
                return Err(Error::CorruptRecord("mkmap root record"));
            }
            records.push(record);
        }
        Ok(MkmapRoot {
            metadata,
            codec,
            shared_dict,
            records,
            min_zoom,
            max_zoom,
            shard_count,
            tile_count,
        })
    }

    pub fn locate(&self, tile_id: u64) -> Option<RootRecordRef> {
        self.records
            .binary_search_by(|record| {
                if tile_id < record.start_tile_id {
                    std::cmp::Ordering::Greater
                } else if tile_id > record.end_tile_id {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()
            .map(|index| {
                let record = self.records[index];
                RootRecordRef {
                    index,
                    start_tile_id: record.start_tile_id,
                    end_tile_id: record.end_tile_id,
                    shard: record.shard,
                    dir_offset: record.dir_offset,
                    dir_len: record.dir_len,
                }
            })
    }

    pub fn decode_blob(&self, bytes: &[u8]) -> std::result::Result<Vec<u8>, String> {
        self.codec
            .decode_limited(bytes, MAX_TILE_BYTES)
            .map_err(|err| err.to_string())
    }

    pub fn metadata(&self) -> &HashMap<String, String> {
        &self.metadata
    }

    pub fn zoom_range(&self) -> (u32, u32) {
        (u32::from(self.min_zoom), u32::from(self.max_zoom))
    }

    pub fn tile_count(&self) -> u64 {
        self.tile_count
    }

    pub fn shard_count(&self) -> u32 {
        self.shard_count
    }

    pub fn dict(&self) -> Option<&[u8]> {
        self.codec.dict()
    }

    pub fn shared_dict(&self) -> Option<&[u8]> {
        (!self.shared_dict.is_empty()).then_some(self.shared_dict.as_slice())
    }
}

/// Parsed, I/O-free leaf directory.
#[derive(Clone, Debug)]
pub struct MkmapLeaf {
    entries: Vec<LeafEntry>,
}

impl MkmapLeaf {
    pub fn parse(packed: &[u8]) -> std::result::Result<MkmapLeaf, String> {
        Self::parse_inner(packed, None).map_err(|err| err.to_string())
    }

    pub fn parse_for_root(
        packed: &[u8],
        shard_count: u32,
        start_tile_id: u64,
        end_tile_id: u64,
    ) -> std::result::Result<MkmapLeaf, String> {
        Self::parse_inner(
            packed,
            Some((shard_count, start_tile_id, end_tile_id)),
        )
        .map_err(|err| err.to_string())
    }

    fn parse_inner(
        packed: &[u8],
        bounds: Option<(u32, u64, u64)>,
    ) -> Result<MkmapLeaf> {
        let raw = TileCodec::from_metadata(
            &[("compression".to_string(), "br".to_string())]
                .into_iter()
                .collect(),
        )?
        .decode_limited(packed, MAX_LEAF_BYTES)?;
        let mut cursor = 0_usize;
        let count = usize::try_from(read_varint(&raw, &mut cursor)?)
            .map_err(|_| Error::CorruptRecord("mkmap leaf count"))?;
        if count > raw.len().saturating_sub(cursor) / 4 {
            return Err(Error::CorruptRecord("mkmap leaf count"));
        }
        let mut entries = Vec::with_capacity(count);
        let mut tile_id = 0_u64;
        for index in 0..count {
            let delta = read_varint(&raw, &mut cursor)?;
            if index != 0 && delta == 0 {
                return Err(Error::CorruptRecord("mkmap leaf tile order"));
            }
            tile_id = tile_id
                .checked_add(delta)
                .ok_or(Error::CorruptRecord("mkmap leaf tile id"))?;
            let shard = u32::try_from(read_varint(&raw, &mut cursor)?)
                .map_err(|_| Error::CorruptRecord("mkmap leaf shard"))?;
            let blob_offset = read_varint(&raw, &mut cursor)?;
            let len = read_varint(&raw, &mut cursor)?;
            if len == 0
                || len > MAX_TILE_BYTES as u64
                || blob_offset.checked_add(len).is_none()
                || bounds.is_some_and(|(shard_count, start, end)| {
                    shard >= shard_count || tile_id < start || tile_id > end
                })
            {
                return Err(Error::CorruptRecord("mkmap leaf entry"));
            }
            entries.push(LeafEntry {
                tile_id,
                blob: BlobRef {
                    shard,
                    offset: blob_offset,
                    len,
                },
            });
        }
        if cursor != raw.len() {
            return Err(Error::CorruptRecord("mkmap leaf trailing bytes"));
        }
        Ok(MkmapLeaf { entries })
    }

    pub fn find(&self, tile_id: u64) -> Option<BlobRef> {
        self.entries
            .binary_search_by_key(&tile_id, |entry| entry.tile_id)
            .ok()
            .map(|index| self.entries[index].blob)
    }

    pub fn retained_bytes(&self) -> usize {
        self.entries.len().saturating_mul(std::mem::size_of::<LeafEntry>())
    }
}

/// Positioned-read `.mkmap` consumer with the same surface the tile loader
/// uses on `MbtilesReader`: metadata + per-tile decoded bytes.
#[cfg(not(target_arch = "wasm32"))]
mod file_reader {
use super::*;

pub struct MkmapReader {
    dir: PathBuf,
    root: MkmapRoot,
    /// Decoded leaf directories, keyed by root record index. A viewport's
    /// tiles are Hilbert-adjacent, so a handful of leaves covers a session.
    leaf_cache: HashMap<usize, MkmapLeaf>,
    pub(super) shard_files: HashMap<u32, File>,
    shard_file_lru: VecDeque<u32>,
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
        let root = MkmapRoot::parse_inner(&bytes)?;
        Ok(MkmapReader {
            dir,
            root,
            leaf_cache: HashMap::new(),
            shard_files: HashMap::new(),
            shard_file_lru: VecDeque::new(),
        })
    }

    pub fn get_metadata(&mut self) -> Result<HashMap<String, String>> {
        Ok(self.root.metadata.clone())
    }

    pub fn zoom_range(&self) -> (u32, u32) {
        self.root.zoom_range()
    }

    /// Tile count declared by the index header (leaf entries, before dedup).
    pub fn tile_count(&self) -> u64 {
        self.root.tile_count()
    }

    pub fn shard_count(&self) -> u32 {
        self.root.shard_count()
    }

    /// Number of root records. Writers currently emit one record per shard;
    /// keeping this separate from `shard_count` makes record-wise streaming
    /// explicit for archive transformation tools.
    pub fn root_record_count(&self) -> usize {
        self.root.records.len()
    }

    /// The shared dictionary the carried metadata declares, if any — the
    /// bytes an extracted archive has to re-declare to stay decodable.
    pub fn dict(&self) -> Option<&[u8]> {
        self.root.dict()
    }

    /// The raw shared dictionary stored in `root.mkidx` itself.
    pub fn shared_dict(&self) -> Option<&[u8]> {
        self.root.shared_dict()
    }

    pub(super) fn read_range(&mut self, shard: u32, offset: u64, len: u64) -> Result<Vec<u8>> {
        if !self.shard_files.contains_key(&shard) {
            let path = self.dir.join(format!("tiles-{shard:03}.mkshard"));
            while self.shard_files.len() >= SHARD_FILE_CACHE_CAPACITY {
                if let Some(oldest) = self.shard_file_lru.pop_front() {
                    self.shard_files.remove(&oldest);
                }
            }
            self.shard_files
                .insert(shard, File::open(path).map_err(Error::Io)?);
        }
        self.shard_file_lru.retain(|cached| *cached != shard);
        self.shard_file_lru.push_back(shard);
        let file = self.shard_files.get_mut(&shard).unwrap();
        file.seek(SeekFrom::Start(offset)).map_err(Error::Io)?;
        let mut bytes = vec![0_u8; len as usize];
        file.read_exact(&mut bytes).map_err(Error::Io)?;
        Ok(bytes)
    }

    /// Read and decode one root record's leaf directory (no caching — the
    /// caller decides whether the entries are worth keeping).
    fn read_leaf(&mut self, record_index: usize) -> Result<MkmapLeaf> {
        let record = self.root.records[record_index];
        let (shard, offset, len) = (record.shard, record.dir_offset, record.dir_len);
        let packed = self.read_range(shard, offset, len)?;
        MkmapLeaf::parse_inner(
            &packed,
            Some((
                self.root.shard_count,
                record.start_tile_id,
                record.end_tile_id,
            )),
        )
    }

    fn resolve(&mut self, zoom: u8, x: u32, y: u32) -> Result<Option<BlobRef>> {
        let id = mkmap_tile_id(zoom, x, y);
        let record_index = match self.root.locate(id) {
            Some(record) => record.index,
            None => return Ok(None),
        };
        if !self.leaf_cache.contains_key(&record_index) {
            let entries = self.read_leaf(record_index)?;
            if self.leaf_cache.len() > 16 {
                self.leaf_cache.clear();
            }
            self.leaf_cache.insert(record_index, entries);
        }
        Ok(self.leaf_cache[&record_index].find(id))
    }

    /// Address + blob location of one tile, in XYZ orientation.
    pub fn resolve_tile(&mut self, zoom: u8, x: u32, y: u32) -> Result<Option<MkmapTileRef>> {
        Ok(self.resolve(zoom, x, y)?.map(|blob| MkmapTileRef {
            tile_id: mkmap_tile_id(zoom, x, y),
            zoom,
            x,
            y,
            shard: blob.shard,
            offset: blob.offset,
            len: blob.len,
        }))
    }

    /// Walk every tile in the container, in tile-id (Hilbert, zoom-banded)
    /// order — the inverse of the weave: one leaf directory in memory at a
    /// time, so the whole archive enumerates in bounded memory.
    pub fn for_each_tile_ref(&mut self, callback: impl FnMut(MkmapTileRef)) -> Result<()> {
        self.for_each_tile_ref_in_range(0, u64::MAX, callback)
    }

    /// Walk one root record's leaf, retaining only that leaf directory.
    /// This is the shard-at-a-time path used by `.mkmap` rewriters.
    pub fn for_each_root_record_tile_ref(
        &mut self,
        record_index: usize,
        mut callback: impl FnMut(MkmapTileRef),
    ) -> Result<()> {
        if record_index >= self.root.records.len() {
            return Err(Error::InvalidInput(
                "mkmap root record is out of range".to_string(),
            ));
        }
        for entry in self.read_leaf(record_index)?.entries {
            let (zoom, x, y) = mkmap_zxy_from_tile_id(entry.tile_id);
            callback(MkmapTileRef {
                tile_id: entry.tile_id,
                zoom,
                x,
                y,
                shard: entry.blob.shard,
                offset: entry.blob.offset,
                len: entry.blob.len,
            });
        }
        Ok(())
    }

    /// [`MkmapReader::for_each_tile_ref`] restricted to a tile-id window.
    /// Ids are zoom-banded and spatially local, so a region of interest is a
    /// short list of windows and only the leaf directories that overlap them
    /// are ever read.
    pub fn for_each_tile_ref_in_range(
        &mut self,
        start_id: u64,
        end_id: u64,
        mut callback: impl FnMut(MkmapTileRef),
    ) -> Result<()> {
        if start_id > end_id {
            return Ok(());
        }
        for record_index in 0..self.root.records.len() {
            let record = &self.root.records[record_index];
            if record.end_tile_id < start_id || record.start_tile_id > end_id {
                continue;
            }
            for entry in self.read_leaf(record_index)?.entries {
                if entry.tile_id < start_id || entry.tile_id > end_id {
                    continue;
                }
                let (zoom, x, y) = mkmap_zxy_from_tile_id(entry.tile_id);
                callback(MkmapTileRef {
                    tile_id: entry.tile_id,
                    zoom,
                    x,
                    y,
                    shard: entry.blob.shard,
                    offset: entry.blob.offset,
                    len: entry.blob.len,
                });
            }
        }
        Ok(())
    }

    /// The stored (still compressed) bytes a tile ref points at.
    pub fn read_tile_ref(&mut self, tile: &MkmapTileRef) -> Result<Vec<u8>> {
        self.read_range(tile.shard, tile.offset, tile.len)
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
        self.root.codec.decode(bytes)
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
        Ok(Some(self.root.codec.decode(&bytes)?))
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

    /// Validated request zoom range. Mkmap uses its checked binary header;
    /// legacy MBTiles metadata is accepted only when ordered and in bounds.
    pub fn validated_zoom_range(&mut self) -> Option<(u32, u32)> {
        match self {
            TileArchiveReader::Mkmap(reader) => Some(reader.zoom_range()),
            TileArchiveReader::Mbtiles(reader) => {
                let metadata = reader.get_metadata().ok()?;
                let min = metadata.get("minzoom")?.trim().parse().ok()?;
                let max = metadata.get("maxzoom")?.trim().parse().ok()?;
                (min <= max && max <= 30).then_some((min, max))
            }
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

}

#[cfg(not(target_arch = "wasm32"))]
pub use file_reader::{MkmapReader, TileArchiveReader};

#[cfg(target_arch = "wasm32")]
mod file_reader {
    use super::*;

    const UNSUPPORTED: &str = "local tile archive file access is unavailable on wasm32";

    /// Type-preserving placeholder for the native local-file archive reader.
    pub enum TileArchiveReader {
        Unsupported,
    }

    impl TileArchiveReader {
        pub fn is_mkmap_path(path: &Path) -> bool {
            path.file_name().is_some_and(|name| name == "root.mkidx")
                || path.extension().is_some_and(|extension| extension == "mkmap")
        }

        pub fn open(_path: &Path) -> Result<TileArchiveReader> {
            Err(Error::Unsupported(UNSUPPORTED))
        }

        pub fn get_metadata(&mut self) -> Result<HashMap<String, String>> {
            match self {
                TileArchiveReader::Unsupported => Err(Error::Unsupported(UNSUPPORTED)),
            }
        }

        pub fn validated_zoom_range(&mut self) -> Option<(u32, u32)> {
            match self {
                TileArchiveReader::Unsupported => None,
            }
        }

        pub fn get_tile_decoded(
            &mut self,
            _zoom: i64,
            _column: i64,
            _row: i64,
        ) -> Result<Option<Vec<u8>>> {
            match self {
                TileArchiveReader::Unsupported => Err(Error::Unsupported(UNSUPPORTED)),
            }
        }

        pub fn supports_direct_tile_lookup(&self) -> bool {
            match self {
                TileArchiveReader::Unsupported => false,
            }
        }

        pub fn get_tile(
            &mut self,
            _zoom: i64,
            _column: i64,
            _row: i64,
        ) -> Result<Option<Vec<u8>>> {
            match self {
                TileArchiveReader::Unsupported => Err(Error::Unsupported(UNSUPPORTED)),
            }
        }

        pub fn decode_tile(&self, _bytes: &[u8]) -> Result<Vec<u8>> {
            match self {
                TileArchiveReader::Unsupported => Err(Error::Unsupported(UNSUPPORTED)),
            }
        }

        pub fn get_tiles_at_zoom(&mut self, _zoom: i64) -> Result<Vec<crate::Tile>> {
            match self {
                TileArchiveReader::Unsupported => Err(Error::Unsupported(UNSUPPORTED)),
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use file_reader::TileArchiveReader;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn write_varint(mut value: u64, out: &mut Vec<u8>) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    fn brotli(raw: &[u8]) -> Vec<u8> {
        crate::compress_tile(&crate::TileCompression::Brotli { quality: 5 }, None, raw).unwrap()
    }

    fn tiny_archive_parts() -> (Vec<u8>, Vec<u8>, u64, u64) {
        tiny_archive_parts_with_metadata("1", "1")
    }

    fn tiny_archive_parts_with_metadata(
        metadata_min: &str,
        metadata_max: &str,
    ) -> (Vec<u8>, Vec<u8>, u64, u64) {
        let first = mkmap_tile_id(1, 0, 0);
        let second = mkmap_tile_id(1, 0, 1);
        let mut leaf_raw = Vec::new();
        write_varint(2, &mut leaf_raw);
        write_varint(first, &mut leaf_raw);
        write_varint(0, &mut leaf_raw);
        write_varint(7, &mut leaf_raw);
        write_varint(3, &mut leaf_raw);
        write_varint(second - first, &mut leaf_raw);
        write_varint(0, &mut leaf_raw);
        write_varint(11, &mut leaf_raw);
        write_varint(5, &mut leaf_raw);
        let leaf = brotli(&leaf_raw);

        let metadata = [
            ("compression", "gzip"),
            ("minzoom", metadata_min),
            ("maxzoom", metadata_max),
        ];
        let mut metadata_raw = Vec::new();
        write_varint(metadata.len() as u64, &mut metadata_raw);
        for (key, value) in metadata {
            write_varint(key.len() as u64, &mut metadata_raw);
            metadata_raw.extend_from_slice(key.as_bytes());
            write_varint(value.len() as u64, &mut metadata_raw);
            metadata_raw.extend_from_slice(value.as_bytes());
        }
        let metadata = brotli(&metadata_raw);
        let mut root_raw = Vec::new();
        root_raw.extend_from_slice(&first.to_le_bytes());
        root_raw.extend_from_slice(&second.to_le_bytes());
        root_raw.extend_from_slice(&0_u32.to_le_bytes());
        root_raw.extend_from_slice(&100_u64.to_le_bytes());
        root_raw.extend_from_slice(&(leaf.len() as u64).to_le_bytes());
        let root_br = brotli(&root_raw);

        let mut header = vec![0_u8; HEADER_LEN];
        header[0..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&VERSION.to_le_bytes());
        header[12..16].copy_from_slice(&1_u32.to_le_bytes());
        header[24..32].copy_from_slice(&2_u64.to_le_bytes());
        header[40] = 1;
        header[41] = 1;
        let mut cursor = HEADER_LEN as u64;
        for (slot, len) in [
            (48, metadata.len() as u64),
            (64, 0),
            (80, root_raw.len() as u64),
            (96, root_br.len() as u64),
        ] {
            header[slot..slot + 8].copy_from_slice(&cursor.to_le_bytes());
            header[slot + 8..slot + 16].copy_from_slice(&len.to_le_bytes());
            cursor += len;
        }
        header.extend_from_slice(&metadata);
        header.extend_from_slice(&root_raw);
        header.extend_from_slice(&root_br);
        (header, leaf, first, second)
    }

    fn root_with_records(template: &[u8], shard_count: u32, root_raw: &[u8]) -> Vec<u8> {
        let section = |slot: usize| {
            let offset = u64::from_le_bytes(template[slot..slot + 8].try_into().unwrap()) as usize;
            let len =
                u64::from_le_bytes(template[slot + 8..slot + 16].try_into().unwrap()) as usize;
            template[offset..offset + len].to_vec()
        };
        let metadata = section(48);
        let dict = section(64);
        let packed = brotli(root_raw);
        let mut root = template[..HEADER_LEN].to_vec();
        root[12..16].copy_from_slice(&shard_count.to_le_bytes());
        let mut cursor = HEADER_LEN as u64;
        for (slot, bytes) in [(48, &metadata), (64, &dict), (80, &root_raw.to_vec()), (96, &packed)] {
            root[slot..slot + 8].copy_from_slice(&cursor.to_le_bytes());
            root[slot + 8..slot + 16].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
            cursor += bytes.len() as u64;
        }
        root.extend_from_slice(&metadata);
        root.extend_from_slice(&dict);
        root.extend_from_slice(root_raw);
        root.extend_from_slice(&packed);
        root
    }

    fn record(start: u64, end: u64, shard: u32, offset: u64, len: u64) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&end.to_le_bytes());
        out.extend_from_slice(&shard.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out
    }

    #[test]
    fn pure_root_and_leaf_parsers_locate_tiles() {
        let (root_bytes, leaf_bytes, first, second) = tiny_archive_parts();
        let root = MkmapRoot::parse(&root_bytes).unwrap();
        assert_eq!(root.zoom_range(), (1, 1));
        assert_eq!(root.tile_count(), 2);
        assert_eq!(root.shard_count(), 1);
        assert_eq!(root.metadata().get("compression").map(String::as_str), Some("gzip"));
        assert_eq!(
            root.locate(first),
            Some(RootRecordRef {
                index: 0,
                start_tile_id: first,
                end_tile_id: second,
                shard: 0,
                dir_offset: 100,
                dir_len: leaf_bytes.len() as u64,
            })
        );
        assert!(root.locate(mkmap_tile_id(2, 0, 0)).is_none());
        assert_eq!(root.decode_blob(b"raw tile").unwrap(), b"raw tile");

        let leaf = MkmapLeaf::parse(&leaf_bytes).unwrap();
        assert_eq!(
            leaf.find(first),
            Some(BlobRef {
                shard: 0,
                offset: 7,
                len: 3,
            })
        );
        assert_eq!(
            leaf.find(second),
            Some(BlobRef {
                shard: 0,
                offset: 11,
                len: 5,
            })
        );
    }

    #[test]
    fn malformed_roots_and_varints_are_rejected() {
        let (root, _, first, second) = tiny_archive_parts();

        let mut bad_zoom = root.clone();
        bad_zoom[40] = 2;
        bad_zoom[41] = 1;
        assert!(MkmapRoot::parse(&bad_zoom).is_err());

        assert!(MkmapRoot::parse(&root_with_records(
            &root,
            1,
            &record(second, first, 0, 10, 1),
        ))
        .is_err());
        assert!(MkmapRoot::parse(&root_with_records(
            &root,
            1,
            &record(first, second, 1, 10, 1),
        ))
        .is_err());
        assert!(MkmapRoot::parse(&root_with_records(
            &root,
            1,
            &record(first, second, 0, 10, 0),
        ))
        .is_err());
        assert!(MkmapRoot::parse(&root_with_records(
            &root,
            1,
            &record(first, second, 0, 0, MAX_LEAF_BYTES as u64 + 1),
        ))
        .is_err());

        let mut overlapping = record(first, second, 0, 10, 1);
        overlapping.extend_from_slice(&record(second, second + 1, 1, 20, 1));
        assert!(MkmapRoot::parse(&root_with_records(&root, 2, &overlapping)).is_err());

        for bytes in [
            vec![0x80, 0x00],
            vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02],
            vec![0x80; 10],
        ] {
            assert!(read_varint(&bytes, &mut 0).is_err());
        }
    }

    #[test]
    fn malformed_leaf_entries_are_rejected_before_lookup() {
        let (_, _, first, second) = tiny_archive_parts();
        let leaf = |delta: u64, shard: u64, len: u64| {
            let mut raw = Vec::new();
            write_varint(1, &mut raw);
            write_varint(delta, &mut raw);
            write_varint(shard, &mut raw);
            write_varint(0, &mut raw);
            write_varint(len, &mut raw);
            brotli(&raw)
        };
        assert!(MkmapLeaf::parse_for_root(&leaf(first, 0, 0), 1, first, second).is_err());
        assert!(MkmapLeaf::parse_for_root(
            &leaf(first, 0, MAX_TILE_BYTES as u64 + 1),
            1,
            first,
            second,
        )
        .is_err());
        assert!(MkmapLeaf::parse_for_root(&leaf(first, 1, 1), 1, first, second).is_err());
        assert!(MkmapLeaf::parse_for_root(&leaf(second + 1, 0, 1), 1, first, second).is_err());

        let mut duplicate = Vec::new();
        write_varint(2, &mut duplicate);
        for delta in [first, 0] {
            write_varint(delta, &mut duplicate);
            write_varint(0, &mut duplicate);
            write_varint(0, &mut duplicate);
            write_varint(1, &mut duplicate);
        }
        assert!(MkmapLeaf::parse_for_root(&brotli(&duplicate), 1, first, second).is_err());
    }

    fn temp_path(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::fs::create_dir_all("target").unwrap();
        PathBuf::from(format!("target/{name}-{nonce}"))
    }

    #[test]
    fn validated_zoom_range_rejects_malformed_mbtiles_metadata() {
        let path = temp_path("mkmap-invalid-zoom").with_extension("mbtiles");
        let mut writer = crate::MbtilesWriter::create(&path).unwrap();
        writer.set_metadata("minzoom", "20");
        writer.set_metadata("maxzoom", "3");
        writer.finish().unwrap();
        let mut reader = TileArchiveReader::open(&path).unwrap();
        assert_eq!(reader.validated_zoom_range(), None);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mkmap_zoom_range_uses_validated_header_not_disagreeing_metadata() {
        let dir = temp_path("mkmap-header-zoom");
        std::fs::create_dir_all(&dir).unwrap();
        let (root, _, _, _) = tiny_archive_parts_with_metadata("7", "9");
        std::fs::write(dir.join("root.mkidx"), root).unwrap();
        let mut reader = TileArchiveReader::open(&dir).unwrap();
        assert_eq!(reader.validated_zoom_range(), Some((1, 1)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn synchronous_shard_file_cache_is_lru_bounded_and_reopens() {
        let dir = temp_path("mkmap-shard-cache");
        std::fs::create_dir_all(&dir).unwrap();
        let (root, _, _, _) = tiny_archive_parts();
        std::fs::write(dir.join("root.mkidx"), root).unwrap();
        for shard in 0..=SHARD_FILE_CACHE_CAPACITY as u32 {
            std::fs::write(dir.join(format!("tiles-{shard:03}.mkshard")), [shard as u8])
                .unwrap();
        }
        let mut reader = MkmapReader::open(&dir).unwrap();
        for shard in 0..SHARD_FILE_CACHE_CAPACITY as u32 {
            assert_eq!(&*reader.read_range(shard, 0, 1).unwrap(), [shard as u8]);
        }
        assert_eq!(&*reader.read_range(0, 0, 1).unwrap(), [0]);
        assert_eq!(
            &*reader
                .read_range(SHARD_FILE_CACHE_CAPACITY as u32, 0, 1)
                .unwrap(),
            [SHARD_FILE_CACHE_CAPACITY as u8]
        );
        assert_eq!(reader.shard_files.len(), SHARD_FILE_CACHE_CAPACITY);
        assert!(reader.shard_files.contains_key(&0));
        assert!(!reader.shard_files.contains_key(&1));
        assert_eq!(&*reader.read_range(1, 0, 1).unwrap(), [1]);
        assert!(reader.shard_files.contains_key(&1));
        assert!(!reader.shard_files.contains_key(&2));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A container walk recovers each tile's address from its id alone, so
    /// the inverse has to agree with the id function at every zoom.
    #[test]
    fn tile_ids_round_trip_to_addresses() {
        for zoom in 0..=8_u8 {
            let side = 1_u32 << zoom;
            for y in 0..side {
                for x in 0..side {
                    let id = mkmap_tile_id(zoom, x, y);
                    assert_eq!(mkmap_zxy_from_tile_id(id), (zoom, x, y), "z{zoom} {x},{y}");
                }
            }
        }
        for (zoom, x, y) in [(10_u8, 6_u32, 12_u32), (14, 8412, 5384), (14, 16383, 16383)] {
            let id = mkmap_tile_id(zoom, x, y);
            assert_eq!(mkmap_zxy_from_tile_id(id), (zoom, x, y));
        }
    }

    /// Each zoom owns a contiguous id band, which is what lets a bbox
    /// extraction prune whole leaf directories by id range.
    #[test]
    fn zoom_bands_are_contiguous_and_ordered() {
        assert_eq!(zoom_base_id(0), 0);
        assert_eq!(zoom_base_id(1), 1);
        assert_eq!(zoom_base_id(2), 5);
        for zoom in 0..14_u8 {
            let side = 1_u64 << zoom;
            assert_eq!(zoom_base_id(zoom) + side * side, zoom_base_id(zoom + 1));
        }
    }
}
