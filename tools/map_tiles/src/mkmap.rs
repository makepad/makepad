//! `.mkmap` sharded tile container: transmux a finished MBTiles archive
//! into a CDN-ready directory of files, none of which may reach the
//! Cloudflare cacheable-object limit.
//!
//! Layout of `<output>.mkmap/`:
//! - `root.mkidx` — tiny index: fixed header, brotli JSON metadata (the
//!   MBTiles metadata table, including `compression` / `compression_dict`),
//!   the raw shared dictionary when present, the root directory mapping
//!   Hilbert tile-ID ranges -> (shard, leaf-dir offset, len), and a brotli
//!   copy of that root directory.
//! - `tiles-NNN.mkshard` — Hilbert-ordered raw tile blobs (copied verbatim
//!   from the MBTiles archive, no re-encoding) with content-hash dedup,
//!   followed by the shard's brotli leaf directory (per-tile entries).
//!
//! Tile IDs follow the PMTiles convention: `(4^z - 1)/3 + hilbert(z, x, y)`
//! with XYZ row orientation, so consecutive IDs are spatially adjacent and
//! range->shard mapping stays compact.
//!
//! Every shard file (and the index) must stay under the hard cap of
//! 510_000_000 bytes: Cloudflare documents a "512 MB" cacheable limit with
//! ambiguous MB/MiB semantics, so the cap sits safely under both readings.
//! The writer asserts the cap per shard and the built-in verification pass
//! stats every produced file again and fails loudly on violation.

use makepad_mbtile_reader::{MbtilesReader, TileCompression};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const SHARD_HARD_CAP: u64 = 510_000_000;
const MAGIC: &[u8; 8] = b"MKMAPIX1";
// v2: metadata section is varint KV (was JSON).
const VERSION: u32 = 2;
const HEADER_LEN: usize = 112;
const ROOT_RECORD_LEN: usize = 36;

pub struct TransmuxOptions {
    pub source: PathBuf,
    pub output: PathBuf,
    pub shard_cap: u64,
    pub sample_stride: u64,
}

pub fn parse_transmux_options(args: &[String]) -> Result<TransmuxOptions, String> {
    if args.len() < 3 {
        return Err("transmux needs <source.mbtiles> <output.mkmap>".to_string());
    }
    let mut options = TransmuxOptions {
        source: PathBuf::from(&args[1]),
        output: PathBuf::from(&args[2]),
        shard_cap: SHARD_HARD_CAP,
        sample_stride: 37,
    };
    let mut index = 3;
    while index < args.len() {
        match args[index].as_str() {
            "--shard-cap-bytes" => {
                let value = args
                    .get(index + 1)
                    .ok_or("--shard-cap-bytes requires a number")?;
                options.shard_cap = value
                    .parse::<u64>()
                    .map_err(|err| format!("invalid --shard-cap-bytes '{value}': {err}"))?;
                index += 2;
            }
            "--sample-stride" => {
                let value = args
                    .get(index + 1)
                    .ok_or("--sample-stride requires a number")?;
                options.sample_stride = value
                    .parse::<u64>()
                    .map_err(|err| format!("invalid --sample-stride '{value}': {err}"))?
                    .max(1);
                index += 2;
            }
            value => return Err(format!("unknown transmux argument '{value}'")),
        }
    }
    if options.shard_cap > SHARD_HARD_CAP {
        return Err(format!(
            "--shard-cap-bytes {} exceeds the hard cap {SHARD_HARD_CAP}",
            options.shard_cap
        ));
    }
    Ok(options)
}

// ---------------------------------------------------------------------------
// Hilbert tile ids (PMTiles convention: XYZ rows, per-zoom Hilbert curve)
// ---------------------------------------------------------------------------

fn hilbert_rotate(side: u32, x: &mut u32, y: &mut u32, rx: u32, ry: u32) {
    if ry == 0 {
        if rx == 1 {
            *x = side.wrapping_sub(1).wrapping_sub(*x);
            *y = side.wrapping_sub(1).wrapping_sub(*y);
        }
        std::mem::swap(x, y);
    }
}

fn hilbert_xy_to_d(zoom: u8, mut x: u32, mut y: u32) -> u64 {
    let side = 1_u32 << zoom;
    let mut d = 0_u64;
    let mut s = side >> 1;
    while s > 0 {
        let rx = u32::from(x & s > 0);
        let ry = u32::from(y & s > 0);
        d += u64::from(s) * u64::from(s) * u64::from((3 * rx) ^ ry);
        hilbert_rotate(s.wrapping_mul(2), &mut x, &mut y, rx, ry);
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

/// Cumulative tile count of all zooms below `zoom`: (4^zoom - 1) / 3.
fn zoom_base_id(zoom: u8) -> u64 {
    ((1_u128 << (2 * u32::from(zoom))) as u64).wrapping_sub(1) / 3
}

pub fn tile_id(zoom: u8, x: u32, y: u32) -> u64 {
    zoom_base_id(zoom) + hilbert_xy_to_d(zoom, x, y)
}

pub fn tile_id_to_zxy(id: u64) -> (u8, u32, u32) {
    let mut zoom = 0_u8;
    while zoom < 31 && zoom_base_id(zoom + 1) <= id {
        zoom += 1;
    }
    let (x, y) = hilbert_d_to_xy(zoom, id - zoom_base_id(zoom));
    (zoom, x, y)
}

// ---------------------------------------------------------------------------
// varint + hashing helpers
// ---------------------------------------------------------------------------

fn write_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_varint(input: &[u8], offset: &mut usize) -> Result<u64, String> {
    let mut value = 0_u64;
    for shift in (0..=63).step_by(7) {
        let byte = *input
            .get(*offset)
            .ok_or_else(|| "truncated mkmap varint".to_string())?;
        *offset += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("mkmap varint overflow".to_string())
}

/// 128-bit content hash for dedup (two independently seeded 64-bit mixes;
/// collision odds for tens of millions of blobs are ~2^-75, far below disk
/// error rates).
fn content_hash(bytes: &[u8]) -> u128 {
    fn mix(seed: u64, bytes: &[u8]) -> u64 {
        let mut hash = seed ^ 0xcbf2_9ce4_8422_2325;
        for chunk in bytes.chunks(8) {
            let mut word = [0_u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            let mut value = u64::from_le_bytes(word) ^ hash;
            value = value.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            value ^= value >> 29;
            value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
            value ^= value >> 32;
            hash = hash.rotate_left(27) ^ value;
        }
        hash ^ (bytes.len() as u64)
    }
    (u128::from(mix(0x5851_f42d_4c95_7f2d, bytes)) << 64)
        | u128::from(mix(0x1405_7b7e_f767_814f, bytes))
}

fn brotli_pack(bytes: &[u8]) -> Result<Vec<u8>, String> {
    makepad_mbtile_reader::compress_tile(&TileCompression::Brotli { quality: 9 }, None, bytes)
        .map_err(|err| format!("brotli pack: {err}"))
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

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

fn encode_leaf_directory(entries: &[LeafEntry]) -> Result<Vec<u8>, String> {
    let mut raw = Vec::with_capacity(entries.len() * 8);
    write_varint(entries.len() as u64, &mut raw);
    let mut previous_id = 0_u64;
    for entry in entries {
        write_varint(entry.tile_id - previous_id, &mut raw);
        previous_id = entry.tile_id;
        write_varint(u64::from(entry.blob.shard), &mut raw);
        write_varint(entry.blob.offset, &mut raw);
        write_varint(entry.blob.len, &mut raw);
    }
    brotli_pack(&raw)
}

fn decode_leaf_directory(packed: &[u8]) -> Result<Vec<LeafEntry>, String> {
    let raw = makepad_mbtile_reader::TileCodec::from_metadata(
        &[("compression".to_string(), "br".to_string())]
            .into_iter()
            .collect(),
    )
    .map_err(|err| err.to_string())?
    .decode(packed)
    .map_err(|err| format!("leaf directory decode: {err}"))?;
    let mut offset = 0;
    let count = read_varint(&raw, &mut offset)? as usize;
    let mut entries = Vec::with_capacity(count);
    let mut tile_id = 0_u64;
    for _ in 0..count {
        tile_id += read_varint(&raw, &mut offset)?;
        let shard = u32::try_from(read_varint(&raw, &mut offset)?)
            .map_err(|_| "leaf shard exceeds u32".to_string())?;
        let blob_offset = read_varint(&raw, &mut offset)?;
        let len = read_varint(&raw, &mut offset)?;
        entries.push(LeafEntry {
            tile_id,
            blob: BlobRef {
                shard,
                offset: blob_offset,
                len,
            },
        });
    }
    Ok(entries)
}

struct RootRecord {
    start_tile_id: u64,
    end_tile_id: u64,
    shard: u32,
    dir_offset: u64,
    dir_len: u64,
}

fn shard_path(dir: &Path, shard: u32) -> PathBuf {
    dir.join(format!("tiles-{shard:03}.mkshard"))
}

pub fn transmux(options: TransmuxOptions) -> Result<(), String> {
    let started = Instant::now();
    if options.output.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite it",
            options.output.display()
        ));
    }
    let mut reader = MbtilesReader::open(&options.source)
        .map_err(|err| format!("open {}: {err}", options.source.display()))?;
    let metadata = reader
        .get_metadata()
        .map_err(|err| format!("read metadata: {err}"))?;
    let dict = reader.tile_codec().dict().map(<[u8]>::to_vec);

    // Pass 1: enumerate all tiles, map to Hilbert ids.
    println!("mkmap: pass 1/3 enumerating tiles");
    let mut tiles: Vec<(u64, u8, u32, u32)> = Vec::new();
    let mut min_zoom = u8::MAX;
    let mut max_zoom = 0_u8;
    reader
        .for_each_tile(|tile| {
            let zoom = tile.zoom_level as u8;
            let x = tile.tile_column as u32;
            let axis = 1_u32 << zoom;
            let y = axis - 1 - tile.tile_row as u32; // TMS -> XYZ
            tiles.push((tile_id(zoom, x, y), zoom, x, y));
            min_zoom = min_zoom.min(zoom);
            max_zoom = max_zoom.max(zoom);
        })
        .map_err(|err| format!("scan {}: {err}", options.source.display()))?;
    if tiles.is_empty() {
        return Err("source archive contains no tiles".to_string());
    }
    tiles.sort_unstable_by_key(|&(id, ..)| id);
    for pair in tiles.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(format!("duplicate tile id {} in source", pair[0].0));
        }
    }
    println!("  {} tiles, z{}..z{}", tiles.len(), min_zoom, max_zoom);

    fs::create_dir_all(&options.output)
        .map_err(|err| format!("create {}: {err}", options.output.display()))?;

    // Pass 2: write shards in Hilbert order with content dedup.
    println!("mkmap: pass 2/3 writing shards (cap {} bytes)", options.shard_cap);
    let mut dedup: HashMap<u128, BlobRef> = HashMap::new();
    let mut root: Vec<RootRecord> = Vec::new();
    let mut shard_index = 0_u32;
    let mut shard_buffer: Vec<u8> = Vec::with_capacity(options.shard_cap as usize / 2);
    let mut shard_entries: Vec<LeafEntry> = Vec::new();
    let mut total_blob_bytes = 0_u64;
    let mut unique_blobs = 0_u64;
    let mut last_progress = Instant::now();

    // Estimated leaf-directory overhead per entry (compressed); generous,
    // re-checked exactly at finalize time.
    const DIR_ENTRY_ESTIMATE: u64 = 12;
    const DIR_FIXED_ESTIMATE: u64 = 4096;

    let mut finalize_shard = |shard_index: &mut u32,
                              shard_buffer: &mut Vec<u8>,
                              shard_entries: &mut Vec<LeafEntry>,
                              root: &mut Vec<RootRecord>|
     -> Result<(), String> {
        if shard_entries.is_empty() {
            return Ok(());
        }
        let directory = encode_leaf_directory(shard_entries)?;
        let total = shard_buffer.len() as u64 + directory.len() as u64;
        if total >= options.shard_cap {
            return Err(format!(
                "internal error: shard {} would be {total} bytes (cap {})",
                *shard_index, options.shard_cap
            ));
        }
        let path = shard_path(&options.output, *shard_index);
        let file =
            File::create(&path).map_err(|err| format!("create {}: {err}", path.display()))?;
        let mut writer = BufWriter::with_capacity(4 * 1024 * 1024, file);
        writer
            .write_all(shard_buffer)
            .and_then(|_| writer.write_all(&directory))
            .and_then(|_| writer.flush())
            .map_err(|err| format!("write {}: {err}", path.display()))?;
        root.push(RootRecord {
            start_tile_id: shard_entries.first().unwrap().tile_id,
            end_tile_id: shard_entries.last().unwrap().tile_id,
            shard: *shard_index,
            dir_offset: shard_buffer.len() as u64,
            dir_len: directory.len() as u64,
        });
        *shard_index += 1;
        shard_buffer.clear();
        shard_entries.clear();
        Ok(())
    };

    for (index, &(id, zoom, x, y)) in tiles.iter().enumerate() {
        let axis = 1_i64 << zoom;
        let tms_row = axis - 1 - i64::from(y);
        let blob = reader
            .get_tile(i64::from(zoom), i64::from(x), tms_row)
            .map_err(|err| format!("read z{zoom}/{x}/{y}: {err}"))?
            .ok_or_else(|| format!("tile z{zoom}/{x}/{y} vanished during transmux"))?;
        let hash = content_hash(&blob);
        let blob_ref = if let Some(existing) = dedup.get(&hash) {
            *existing
        } else {
            let projected = shard_buffer.len() as u64
                + blob.len() as u64
                + (shard_entries.len() as u64 + 1) * DIR_ENTRY_ESTIMATE
                + DIR_FIXED_ESTIMATE;
            if projected >= options.shard_cap {
                finalize_shard(
                    &mut shard_index,
                    &mut shard_buffer,
                    &mut shard_entries,
                    &mut root,
                )?;
            }
            let blob_ref = BlobRef {
                shard: shard_index,
                offset: shard_buffer.len() as u64,
                len: blob.len() as u64,
            };
            shard_buffer.extend_from_slice(&blob);
            total_blob_bytes += blob.len() as u64;
            unique_blobs += 1;
            dedup.insert(hash, blob_ref);
            blob_ref
        };
        shard_entries.push(LeafEntry { tile_id: id, blob: blob_ref });
        if last_progress.elapsed().as_secs() >= 2 {
            println!(
                "  {}/{} tiles | shard {} | {:.2} GiB unique blobs",
                index + 1,
                tiles.len(),
                shard_index,
                total_blob_bytes as f64 / 1_073_741_824.0
            );
            last_progress = Instant::now();
        }
    }
    finalize_shard(
        &mut shard_index,
        &mut shard_buffer,
        &mut shard_entries,
        &mut root,
    )?;

    // Index file. Metadata is varint KV pairs (count, then per pair
    // length-prefixed key and value bytes) — same primitive the leaf
    // directories use; no JSON anywhere in the container.
    let metadata_map: std::collections::BTreeMap<&str, &str> = metadata
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let mut metadata_raw = Vec::new();
    write_varint(metadata_map.len() as u64, &mut metadata_raw);
    for (key, value) in &metadata_map {
        write_varint(key.len() as u64, &mut metadata_raw);
        metadata_raw.extend_from_slice(key.as_bytes());
        write_varint(value.len() as u64, &mut metadata_raw);
        metadata_raw.extend_from_slice(value.as_bytes());
    }
    let metadata_br = brotli_pack(&metadata_raw)?;
    let mut root_raw = Vec::with_capacity(root.len() * ROOT_RECORD_LEN);
    for record in &root {
        root_raw.extend_from_slice(&record.start_tile_id.to_le_bytes());
        root_raw.extend_from_slice(&record.end_tile_id.to_le_bytes());
        root_raw.extend_from_slice(&record.shard.to_le_bytes());
        root_raw.extend_from_slice(&record.dir_offset.to_le_bytes());
        root_raw.extend_from_slice(&record.dir_len.to_le_bytes());
    }
    let root_br = brotli_pack(&root_raw)?;

    let dict_bytes = dict.as_deref().unwrap_or(&[]);
    let mut header = vec![0_u8; HEADER_LEN];
    header[0..8].copy_from_slice(MAGIC);
    header[8..12].copy_from_slice(&VERSION.to_le_bytes());
    header[12..16].copy_from_slice(&shard_index.to_le_bytes());
    header[16..24].copy_from_slice(&options.shard_cap.to_le_bytes());
    header[24..32].copy_from_slice(&(tiles.len() as u64).to_le_bytes());
    header[32..40].copy_from_slice(&unique_blobs.to_le_bytes());
    header[40] = min_zoom;
    header[41] = max_zoom;
    let mut cursor = HEADER_LEN as u64;
    for (slot, len) in [
        (48_usize, metadata_br.len() as u64),
        (64, dict_bytes.len() as u64),
        (80, root_raw.len() as u64),
        (96, root_br.len() as u64),
    ] {
        header[slot..slot + 8].copy_from_slice(&cursor.to_le_bytes());
        header[slot + 8..slot + 16].copy_from_slice(&len.to_le_bytes());
        cursor += len;
    }
    let index_path = options.output.join("root.mkidx");
    let mut index_file = BufWriter::new(
        File::create(&index_path)
            .map_err(|err| format!("create {}: {err}", index_path.display()))?,
    );
    index_file
        .write_all(&header)
        .and_then(|_| index_file.write_all(&metadata_br))
        .and_then(|_| index_file.write_all(dict_bytes))
        .and_then(|_| index_file.write_all(&root_raw))
        .and_then(|_| index_file.write_all(&root_br))
        .and_then(|_| index_file.flush())
        .map_err(|err| format!("write {}: {err}", index_path.display()))?;
    drop(index_file);

    println!(
        "mkmap: wrote {} shards, {} tiles ({} unique blobs, {:.2} GiB), index {} bytes in {:.1}s",
        shard_index,
        tiles.len(),
        unique_blobs,
        total_blob_bytes as f64 / 1_073_741_824.0,
        fs::metadata(&index_path).map(|m| m.len()).unwrap_or(0),
        started.elapsed().as_secs_f64()
    );

    // Pass 3: verification (mandatory).
    println!("mkmap: pass 3/3 verification");
    verify(&options.source, &options.output, options.sample_stride)
}

// ---------------------------------------------------------------------------
// Verification reader
// ---------------------------------------------------------------------------

struct MkmapReader {
    dir: PathBuf,
    root: Vec<RootRecord>,
    shard_cap: u64,
    shard_count: u32,
    tile_count: u64,
    /// Cache of decoded leaf directories, keyed by root record index.
    leaf_cache: HashMap<usize, Vec<LeafEntry>>,
    shard_files: HashMap<u32, File>,
}

impl MkmapReader {
    fn open(dir: &Path) -> Result<Self, String> {
        let index_path = dir.join("root.mkidx");
        let bytes = fs::read(&index_path)
            .map_err(|err| format!("read {}: {err}", index_path.display()))?;
        if bytes.len() < HEADER_LEN || &bytes[0..8] != MAGIC {
            return Err(format!("{} is not an mkmap index", index_path.display()));
        }
        let read_u64 = |slot: usize| {
            u64::from_le_bytes(bytes[slot..slot + 8].try_into().unwrap())
        };
        let shard_count = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let shard_cap = read_u64(16);
        let tile_count = read_u64(24);
        let root_offset = read_u64(80) as usize;
        let root_len = read_u64(88) as usize;
        let root_raw = bytes
            .get(root_offset..root_offset + root_len)
            .ok_or_else(|| "root directory out of bounds".to_string())?;
        if root_len % ROOT_RECORD_LEN != 0 {
            return Err("root directory length is not record-aligned".to_string());
        }
        let mut root = Vec::with_capacity(root_len / ROOT_RECORD_LEN);
        for record in root_raw.chunks_exact(ROOT_RECORD_LEN) {
            root.push(RootRecord {
                start_tile_id: u64::from_le_bytes(record[0..8].try_into().unwrap()),
                end_tile_id: u64::from_le_bytes(record[8..16].try_into().unwrap()),
                shard: u32::from_le_bytes(record[16..20].try_into().unwrap()),
                dir_offset: u64::from_le_bytes(record[20..28].try_into().unwrap()),
                dir_len: u64::from_le_bytes(record[28..36].try_into().unwrap()),
            });
        }
        // The compressed root copy must decode to the same bytes.
        let root_br_offset = read_u64(96) as usize;
        let root_br_len = read_u64(104) as usize;
        let root_br = bytes
            .get(root_br_offset..root_br_offset + root_br_len)
            .ok_or_else(|| "compressed root copy out of bounds".to_string())?;
        let unpacked = decode_brotli_section(root_br)?;
        if unpacked != root_raw {
            return Err("compressed root copy does not match raw root".to_string());
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            root,
            shard_cap,
            shard_count,
            tile_count,
            leaf_cache: HashMap::new(),
            shard_files: HashMap::new(),
        })
    }

    fn shard_file(&mut self, shard: u32) -> Result<&mut File, String> {
        if !self.shard_files.contains_key(&shard) {
            let path = shard_path(&self.dir, shard);
            let file =
                File::open(&path).map_err(|err| format!("open {}: {err}", path.display()))?;
            self.shard_files.insert(shard, file);
        }
        Ok(self.shard_files.get_mut(&shard).unwrap())
    }

    fn read_range(&mut self, shard: u32, offset: u64, len: u64) -> Result<Vec<u8>, String> {
        let file = self.shard_file(shard)?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|err| format!("seek shard {shard}: {err}"))?;
        let mut bytes = vec![0_u8; len as usize];
        file.read_exact(&mut bytes)
            .map_err(|err| format!("read shard {shard}: {err}"))?;
        Ok(bytes)
    }

    fn resolve(&mut self, zoom: u8, x: u32, y: u32) -> Result<Option<BlobRef>, String> {
        let id = tile_id(zoom, x, y);
        let record_index = match self
            .root
            .binary_search_by(|record| {
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
            let entries = decode_leaf_directory(&packed)?;
            if self.leaf_cache.len() > 8 {
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
}

fn decode_brotli_section(bytes: &[u8]) -> Result<Vec<u8>, String> {
    makepad_mbtile_reader::TileCodec::from_metadata(
        &[("compression".to_string(), "br".to_string())]
            .into_iter()
            .collect(),
    )
    .map_err(|err| err.to_string())?
    .decode(bytes)
    .map_err(|err| format!("brotli section decode: {err}"))
}

/// Verify an mkmap directory against its source archive: every shard under
/// the cap, the index resolving every tile, and sampled tiles byte-identical.
pub fn verify(source: &Path, mkmap: &Path, sample_stride: u64) -> Result<(), String> {
    let mut container = MkmapReader::open(mkmap)?;
    // Shard cap re-check straight from the filesystem.
    for shard in 0..container.shard_count {
        let path = shard_path(mkmap, shard);
        let len = fs::metadata(&path)
            .map_err(|err| format!("stat {}: {err}", path.display()))?
            .len();
        if len >= container.shard_cap.min(SHARD_HARD_CAP) {
            return Err(format!(
                "VERIFICATION FAILED: {} is {len} bytes, cap {}",
                path.display(),
                container.shard_cap.min(SHARD_HARD_CAP)
            ));
        }
    }
    let index_len = fs::metadata(mkmap.join("root.mkidx"))
        .map_err(|err| format!("stat index: {err}"))?
        .len();
    if index_len >= SHARD_HARD_CAP {
        return Err(format!(
            "VERIFICATION FAILED: index is {index_len} bytes, cap {SHARD_HARD_CAP}"
        ));
    }

    let mut reader = MbtilesReader::open(source)
        .map_err(|err| format!("open {}: {err}", source.display()))?;
    let mut listed: Vec<(u8, u32, u32)> = Vec::new();
    reader
        .for_each_tile(|tile| {
            let zoom = tile.zoom_level as u8;
            let axis = 1_u32 << zoom;
            listed.push((
                zoom,
                tile.tile_column as u32,
                axis - 1 - tile.tile_row as u32,
            ));
        })
        .map_err(|err| format!("scan source: {err}"))?;
    if listed.len() as u64 != container.tile_count {
        return Err(format!(
            "VERIFICATION FAILED: index declares {} tiles, source has {}",
            container.tile_count,
            listed.len()
        ));
    }
    // Resolve in Hilbert order so leaf loads are sequential.
    listed.sort_unstable_by_key(|&(zoom, x, y)| tile_id(zoom, x, y));
    let mut resolved = 0_u64;
    let mut compared = 0_u64;
    for (index, &(zoom, x, y)) in listed.iter().enumerate() {
        let blob_ref = container
            .resolve(zoom, x, y)?
            .ok_or_else(|| {
                format!("VERIFICATION FAILED: z{zoom}/{x}/{y} does not resolve")
            })?;
        resolved += 1;
        if index as u64 % sample_stride == 0 {
            let from_shard =
                container.read_range(blob_ref.shard, blob_ref.offset, blob_ref.len)?;
            let axis = 1_i64 << zoom;
            let from_source = reader
                .get_tile(i64::from(zoom), i64::from(x), axis - 1 - i64::from(y))
                .map_err(|err| format!("read source z{zoom}/{x}/{y}: {err}"))?
                .ok_or_else(|| format!("source lost z{zoom}/{x}/{y}"))?;
            if from_shard != from_source {
                return Err(format!(
                    "VERIFICATION FAILED: z{zoom}/{x}/{y} bytes differ (shard {} offset {})",
                    blob_ref.shard, blob_ref.offset
                ));
            }
            compared += 1;
        }
    }
    println!(
        "mkmap: verification OK — {} shards under cap, {resolved} tiles resolved, {compared} sampled byte-identical",
        container.shard_count
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hilbert_round_trips_and_covers_each_zoom() {
        for zoom in 0..=6_u8 {
            let side = 1_u32 << zoom;
            let mut seen = vec![false; (side as usize) * (side as usize)];
            for y in 0..side {
                for x in 0..side {
                    let d = hilbert_xy_to_d(zoom, x, y);
                    assert!(d < u64::from(side) * u64::from(side));
                    assert!(!seen[d as usize], "duplicate d at z{zoom} {x},{y}");
                    seen[d as usize] = true;
                    assert_eq!(hilbert_d_to_xy(zoom, d), (x, y));
                }
            }
        }
    }

    #[test]
    fn hilbert_neighbors_are_adjacent() {
        // Consecutive d values must be 4-neighbors (the defining property).
        for zoom in 1..=5_u8 {
            let side = 1_u64 << zoom;
            let mut previous = hilbert_d_to_xy(zoom, 0);
            for d in 1..side * side {
                let current = hilbert_d_to_xy(zoom, d);
                let dx = i64::from(current.0) - i64::from(previous.0);
                let dy = i64::from(current.1) - i64::from(previous.1);
                assert_eq!(dx.abs() + dy.abs(), 1, "z{zoom} d{d}");
                previous = current;
            }
        }
    }

    #[test]
    fn tile_ids_are_zoom_prefixed_and_reversible() {
        assert_eq!(tile_id(0, 0, 0), 0);
        assert_eq!(zoom_base_id(1), 1);
        assert_eq!(zoom_base_id(2), 5);
        assert_eq!(zoom_base_id(14), (4_u64.pow(14) - 1) / 3);
        for (zoom, x, y) in [(1, 1, 0), (5, 17, 9), (14, 8412, 5384)] {
            let id = tile_id(zoom, x, y);
            assert_eq!(tile_id_to_zxy(id), (zoom, x, y));
        }
    }

    #[test]
    fn leaf_directory_round_trips() {
        let entries: Vec<LeafEntry> = (0..1000)
            .map(|index| LeafEntry {
                tile_id: 5 + index * 3,
                blob: BlobRef {
                    shard: (index % 4) as u32,
                    offset: index * 1717,
                    len: 100 + index,
                },
            })
            .collect();
        let packed = encode_leaf_directory(&entries).unwrap();
        let decoded = decode_leaf_directory(&packed).unwrap();
        assert_eq!(decoded.len(), entries.len());
        for (a, b) in entries.iter().zip(&decoded) {
            assert_eq!(a.tile_id, b.tile_id);
            assert_eq!(a.blob.shard, b.blob.shard);
            assert_eq!(a.blob.offset, b.blob.offset);
            assert_eq!(a.blob.len, b.blob.len);
        }
    }

    #[test]
    fn content_hash_distinguishes_blobs() {
        let a = content_hash(b"tile one");
        let b = content_hash(b"tile two");
        let c = content_hash(b"tile one");
        assert_ne!(a, b);
        assert_eq!(a, c);
        assert_ne!(content_hash(b""), content_hash(b"\0"));
    }
}
