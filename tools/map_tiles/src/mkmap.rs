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
//!
//! The weave is reversible: `extract` walks the leaf directories back out and
//! rebuilds an MBTiles archive (optionally clipped to a bbox — one bake cell),
//! copying blobs verbatim and carrying the metadata table, so the sources a
//! weave consumed need not be kept. `write_weave_manifest` records what each
//! source held so a single one can be picked out again, and `compare` proves
//! an extraction really carries a source, tile for tile.

use makepad_map_build::versatiles::{GeoBounds, TileBounds};
use makepad_mbtile_reader::{
    compression_metadata_rows, tile_rowid_xyz, MbtilesReader, MbtilesWriter, MkmapReader,
    MkmapTileRef, TileCodec, TileCompression, COMPRESSION_DICT_METADATA_KEY,
};
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
    /// Additional source archives woven AFTER `source` (first-wins on
    /// duplicate tile ids — order the world low-zoom slab first, then the
    /// spiral cells).
    pub extra_sources: Vec<PathBuf>,
    pub source: PathBuf,
    pub output: PathBuf,
    pub shard_cap: u64,
    pub sample_stride: u64,
}

pub fn parse_transmux_options(args: &[String]) -> Result<TransmuxOptions, String> {
    if args.len() < 3 {
        return Err(
            "transmux needs <source.mbtiles> [more.mbtiles ...] <output.mkmap>".to_string(),
        );
    }
    // All-but-last positional args are sources; the last is the output.
    let mut positional_end = args.len();
    for (i, arg) in args.iter().enumerate().skip(1) {
        if arg.starts_with("--") {
            positional_end = i;
            break;
        }
    }
    if positional_end < 3 {
        return Err("transmux needs at least one source and an output".to_string());
    }
    let mut options = TransmuxOptions {
        source: PathBuf::from(&args[1]),
        extra_sources: args[2..positional_end - 1]
            .iter()
            .map(PathBuf::from)
            .collect(),
        output: PathBuf::from(&args[positional_end - 1]),
        shard_cap: SHARD_HARD_CAP,
        sample_stride: 37,
    };
    let mut index = positional_end;
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

#[cfg(test)]
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

#[cfg(test)]
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
    let all_paths: Vec<PathBuf> = std::iter::once(options.source.clone())
        .chain(options.extra_sources.iter().cloned())
        .collect();

    // Pass 1: enumerate all sources' tiles, map to Hilbert ids. Duplicate
    // ids resolve FIRST-SOURCE-WINS (weave: world low-zoom slab first,
    // then spiral cells — each cell's clipped world tiles lose).
    // A source that fails to open or scan (e.g. a corrupt cell mbtiles) is
    // SKIPPED with a loud warning instead of aborting the whole weave —
    // the operator deletes the corrupt ledger file so a later fleet pass
    // rebakes that cell.
    println!("mkmap: pass 1/3 enumerating tiles ({} sources)", all_paths.len());
    let mut source_paths: Vec<PathBuf> = Vec::with_capacity(all_paths.len());
    let mut readers: Vec<MbtilesReader> = Vec::with_capacity(all_paths.len());
    let mut sources_bytes = 0_u64;
    let mut tiles: Vec<(u64, u8, u32, u32, u32)> = Vec::new();
    let mut min_zoom = u8::MAX;
    let mut max_zoom = 0_u8;
    for path in &all_paths {
        let mut reader = match MbtilesReader::open(path) {
            Ok(reader) => reader,
            Err(err) => {
                eprintln!(
                    "mkmap: WARNING skipping unreadable source {}: {err}",
                    path.display()
                );
                continue;
            }
        };
        let src = readers.len() as u32;
        let mut local: Vec<(u64, u8, u32, u32, u32)> = Vec::new();
        let mut local_min = u8::MAX;
        let mut local_max = 0_u8;
        let scanned = reader.for_each_tile(|tile| {
            let zoom = tile.zoom_level as u8;
            let x = tile.tile_column as u32;
            let axis = 1_u32 << zoom;
            let y = axis - 1 - tile.tile_row as u32; // TMS -> XYZ
            local.push((tile_id(zoom, x, y), zoom, x, y, src));
            local_min = local_min.min(zoom);
            local_max = local_max.max(zoom);
        });
        if let Err(err) = scanned {
            eprintln!(
                "mkmap: WARNING skipping corrupt source {}: {err}",
                path.display()
            );
            continue;
        }
        tiles.extend(local);
        min_zoom = min_zoom.min(local_min);
        max_zoom = max_zoom.max(local_max);
        sources_bytes += fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        source_paths.push(path.clone());
        readers.push(reader);
    }
    if source_paths.len() < all_paths.len() {
        eprintln!(
            "mkmap: WARNING {} of {} sources skipped — output will have holes until they are rebaked",
            all_paths.len() - source_paths.len(),
            all_paths.len()
        );
    }
    if tiles.is_empty() {
        return Err("source archives contain no tiles".to_string());
    }
    let metadata = readers[0]
        .get_metadata()
        .map_err(|err| format!("read metadata: {err}"))?;
    let dict = readers[0].tile_codec().dict().map(<[u8]>::to_vec);
    for (path, reader) in source_paths.iter().zip(&readers).skip(1) {
        if reader.tile_codec().dict().map(<[u8]>::to_vec) != dict {
            return Err(format!("{}: dictionary differs from first source", path.display()));
        }
    }
    // Disk preflight: refuse before writing gigabytes that cannot fit.
    // The woven output is bounded by the summed source sizes (dedup only
    // shrinks it); require half that plus slack, which comfortably covers
    // the real ratio observed on cell weaves.
    if let Some(free) = free_disk_bytes(&options.output) {
        // Full summed source size, not an estimate: mid-run weaves stay
        // blocked while the store still occupies the disk, and resume
        // automatically the moment the endgame frees it.
        let needed = sources_bytes + 5_000_000_000;
        if free < needed {
            return Err(format!(
                "insufficient disk for weave: {free} bytes free, ~{needed} needed — free space and retry"
            ));
        }
    }
    // Stable resolution: sort by (id, src). z14+ duplicates resolve
    // first-source-wins (full per-tile spool copies — identical content).
    // BELOW z14 the copies are per-cell clipped pyramid halves; keep all
    // sources per id so pass 2 can MERGE them (first-wins there produced
    // blank stripes along every cell boundary).
    tiles.sort_unstable_by_key(|&(id, _, _, _, src)| (id, src));
    let before = tiles.len();
    let mut merge_sources: HashMap<u64, Vec<u32>> = HashMap::new();
    {
        let mut read = 0usize;
        let mut write = 0usize;
        while read < tiles.len() {
            let (id, zoom, ..) = tiles[read];
            let mut end = read + 1;
            while end < tiles.len() && tiles[end].0 == id {
                end += 1;
            }
            if end - read > 1 && zoom < 14 {
                merge_sources
                    .insert(id, tiles[read..end].iter().map(|t| t.4).collect());
            }
            tiles[write] = tiles[read];
            write += 1;
            read = end;
        }
        tiles.truncate(write);
    }
    let woven_out = before - tiles.len();
    if woven_out > 0 {
        println!(
            "  {} duplicate tiles: {} merged below z14, rest first-source-wins",
            woven_out,
            merge_sources.len()
        );
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

    let finalize_shard = |shard_index: &mut u32,
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

    let output_compression = TileCompression::Brotli { quality: 11 };
    for (index, &(id, zoom, x, y, src)) in tiles.iter().enumerate() {
        let axis = 1_i64 << zoom;
        let tms_row = axis - 1 - i64::from(y);
        let blob = match merge_sources.get(&id) {
            Some(sources) => {
                let mut decoded: Vec<Vec<u8>> = Vec::with_capacity(sources.len());
                for &merge_src in sources {
                    let copy = readers[merge_src as usize]
                        .get_tile(i64::from(zoom), i64::from(x), tms_row)
                        .map_err(|err| format!("read z{zoom}/{x}/{y}: {err}"))?
                        .ok_or_else(|| {
                            format!("tile z{zoom}/{x}/{y} vanished during transmux")
                        })?;
                    let raw = readers[merge_src as usize]
                        .decode_tile(&copy)
                        .map_err(|err| format!("decode z{zoom}/{x}/{y}: {err}"))?;
                    let raw = strip_baked_field(raw);
                    if !decoded.contains(&raw) {
                        decoded.push(raw);
                    }
                }
                let mut merged =
                    Vec::with_capacity(decoded.iter().map(Vec::len).sum());
                for part in &decoded {
                    merged.extend_from_slice(part);
                }
                makepad_mbtile_reader::compress_tile(
                    &output_compression,
                    dict.as_deref(),
                    &merged,
                )
                .map_err(|err| format!("compress merged z{zoom}/{x}/{y}: {err}"))?
            }
            None => readers[src as usize]
                .get_tile(i64::from(zoom), i64::from(x), tms_row)
                .map_err(|err| format!("read z{zoom}/{x}/{y}: {err}"))?
                .ok_or_else(|| format!("tile z{zoom}/{x}/{y} vanished during transmux"))?,
        };
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

    // Pass 3: verification (mandatory) — against the READABLE sources.
    println!("mkmap: pass 3/3 verification");
    verify(&source_paths, &options.output, options.sample_stride)
}

/// Remove any baked-faces field (field 101, LEN) from a decoded tile
/// payload: a per-cell bake covers clipped content and is invalid for a
/// merged border tile.
fn strip_baked_field(pbf: Vec<u8>) -> Vec<u8> {
    const BAKED_FACES_FIELD: u64 = 101;
    let read_varint = |bytes: &[u8], i: &mut usize| -> Option<u64> {
        let mut value = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = *bytes.get(*i)?;
            *i += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    };
    let mut out: Option<Vec<u8>> = None;
    let mut i = 0usize;
    while i < pbf.len() {
        let start = i;
        let Some(key) = read_varint(&pbf, &mut i) else { break };
        if key & 0x7 != 2 {
            break;
        }
        let Some(len) = read_varint(&pbf, &mut i) else { break };
        let end = i + len as usize;
        if end > pbf.len() {
            break;
        }
        if key >> 3 == BAKED_FACES_FIELD {
            if out.is_none() {
                let mut fresh = Vec::with_capacity(pbf.len());
                fresh.extend_from_slice(&pbf[..start]);
                out = Some(fresh);
            }
        } else if let Some(out) = out.as_mut() {
            out.extend_from_slice(&pbf[start..end]);
        }
        i = end;
    }
    out.unwrap_or(pbf)
}

/// Free bytes on the filesystem holding `path` (via df; None if that
/// fails — preflight then simply doesn't gate).
fn free_disk_bytes(path: &Path) -> Option<u64> {
    let probe = path.parent().filter(|p| p.exists()).unwrap_or(Path::new("."));
    let output = std::process::Command::new("df")
        .arg("-k")
        .arg(probe)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().nth(1)?;
    let avail_kb: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    Some(avail_kb * 1024)
}

// ---------------------------------------------------------------------------
// Verification reader
// ---------------------------------------------------------------------------

struct VerifyReader {
    dir: PathBuf,
    root: Vec<RootRecord>,
    shard_cap: u64,
    shard_count: u32,
    tile_count: u64,
    /// Cache of decoded leaf directories, keyed by root record index.
    leaf_cache: HashMap<usize, Vec<LeafEntry>>,
    shard_files: HashMap<u32, File>,
}

impl VerifyReader {
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
pub fn verify(sources: &[PathBuf], mkmap: &Path, sample_stride: u64) -> Result<(), String> {
    let mut container = VerifyReader::open(mkmap)?;
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

    // Expected tile set: union over ALL sources with the same
    // first-source-wins ownership as the weave, so each tile is byte-
    // compared against the source that actually supplied it.
    let mut readers: Vec<MbtilesReader> = Vec::with_capacity(sources.len());
    for path in sources {
        readers.push(
            MbtilesReader::open(path)
                .map_err(|err| format!("open {}: {err}", path.display()))?,
        );
    }
    let mut owner: HashMap<u64, (usize, u8, u32, u32, u32)> = HashMap::new();
    for (src, reader) in readers.iter_mut().enumerate() {
        reader
            .for_each_tile(|tile| {
                let zoom = tile.zoom_level as u8;
                let axis = 1_u32 << zoom;
                let x = tile.tile_column as u32;
                let y = axis - 1 - tile.tile_row as u32;
                owner
                    .entry(tile_id(zoom, x, y))
                    .and_modify(|entry| entry.4 += 1)
                    .or_insert((src, zoom, x, y, 1));
            })
            .map_err(|err| format!("scan {}: {err}", sources[src].display()))?;
    }
    if owner.len() as u64 != container.tile_count {
        return Err(format!(
            "VERIFICATION FAILED: index declares {} tiles, sources have {}",
            container.tile_count,
            owner.len()
        ));
    }
    // Resolve in Hilbert order so leaf loads are sequential.
    let mut listed: Vec<(u64, usize, u8, u32, u32, u32)> = owner
        .into_iter()
        .map(|(id, (src, zoom, x, y, copies))| (id, src, zoom, x, y, copies))
        .collect();
    listed.sort_unstable_by_key(|&(id, ..)| id);
    let mut resolved = 0_u64;
    let mut compared = 0_u64;
    for (index, &(_, src, zoom, x, y, copies)) in listed.iter().enumerate() {
        let blob_ref = container
            .resolve(zoom, x, y)?
            .ok_or_else(|| {
                format!("VERIFICATION FAILED: z{zoom}/{x}/{y} does not resolve")
            })?;
        resolved += 1;
        // Merged tiles (multi-copy below z14) are a layer-union of their
        // sources: resolvable, but byte-compare against one source would
        // rightly fail — skip the sample there.
        let merged = copies > 1 && zoom < 14;
        if !merged && index as u64 % sample_stride == 0 {
            let from_shard =
                container.read_range(blob_ref.shard, blob_ref.offset, blob_ref.len)?;
            let axis = 1_i64 << zoom;
            let from_source = readers[src]
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
        "mkmap: verification OK — {} shards under cap, {resolved} tiles resolved, {compared} sampled byte-identical against {} sources",
        container.shard_count,
        readers.len()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Extraction — the inverse of the weave
// ---------------------------------------------------------------------------

/// Beyond this many tiles in a zoom's bbox rectangle it is cheaper to walk
/// the zoom's whole id band than to enumerate the rectangle for its extent.
const RECT_SCAN_LIMIT: u64 = 4_000_000;

pub struct ExtractOptions {
    pub source: PathBuf,
    pub output: PathBuf,
    pub bounds: Option<GeoBounds>,
    pub min_zoom: Option<u8>,
    pub max_zoom: Option<u8>,
    /// Rings of extra tiles around the bbox rectangle, per zoom. A bake
    /// buffers its geometry by a fraction of a tile, so a cell holds tiles
    /// just outside its own declared bounds; reconstructing one wants
    /// `--pad-tiles 1` to catch them.
    pub pad_tiles: u32,
}

pub fn parse_extract_options(args: &[String]) -> Result<ExtractOptions, String> {
    if args.len() < 3 {
        return Err("mkmap-extract needs <dir.mkmap> <output.mbtiles>".to_string());
    }
    let mut options = ExtractOptions {
        source: PathBuf::from(&args[1]),
        output: PathBuf::from(&args[2]),
        bounds: None,
        min_zoom: None,
        max_zoom: None,
        pad_tiles: 0,
    };
    let mut index = 3;
    while index < args.len() {
        match args[index].as_str() {
            "--pad-tiles" => {
                let value = args.get(index + 1).ok_or("--pad-tiles requires a number")?;
                options.pad_tiles = value
                    .parse::<u32>()
                    .map_err(|err| format!("invalid --pad-tiles '{value}': {err}"))?;
                index += 2;
            }
            "--bbox" => {
                let value = args.get(index + 1).ok_or("--bbox requires w,s,e,n")?;
                options.bounds = Some(GeoBounds::parse(value)?);
                index += 2;
            }
            "--min-zoom" => {
                let value = args.get(index + 1).ok_or("--min-zoom requires a number")?;
                options.min_zoom = Some(parse_zoom(value)?);
                index += 2;
            }
            "--max-zoom" => {
                let value = args.get(index + 1).ok_or("--max-zoom requires a number")?;
                options.max_zoom = Some(parse_zoom(value)?);
                index += 2;
            }
            value => return Err(format!("unknown mkmap-extract argument '{value}'")),
        }
    }
    Ok(options)
}

fn parse_zoom(value: &str) -> Result<u8, String> {
    let zoom: u8 = value
        .parse()
        .map_err(|err| format!("invalid zoom '{value}': {err}"))?;
    if zoom > 30 {
        return Err(format!("zoom {zoom} is out of range"));
    }
    Ok(zoom)
}

/// One tile picked for extraction, keyed by the rowid the MBTiles writer
/// demands input to be sorted by. ~48 bytes each, so even a whole-world
/// extraction's selection list stays well inside memory.
struct Selection {
    rowid: i64,
    tile: MkmapTileRef,
}

/// The bbox rectangle at one zoom, grown by `pad` rings of tiles and clamped
/// to the pyramid — the same clamp (no antimeridian wrap) the bake used when
/// it decided which tiles a feature touches.
fn padded_rect(bounds: GeoBounds, zoom: u8, pad: u32) -> TileBounds {
    let last = (1_u32 << zoom) - 1;
    let rect = bounds.tile_bounds(zoom);
    TileBounds {
        x_min: rect.x_min.saturating_sub(pad),
        y_min: rect.y_min.saturating_sub(pad),
        x_max: rect.x_max.saturating_add(pad).min(last),
        y_max: rect.y_max.saturating_add(pad).min(last),
    }
}

/// Per-zoom tile-id windows covering the requested area: the bbox rectangle
/// where enumerating it is cheap, the zoom's whole band otherwise.
fn selection_windows(
    min_zoom: u8,
    max_zoom: u8,
    bounds: Option<GeoBounds>,
    pad_tiles: u32,
) -> Vec<(u8, u64, u64)> {
    let mut windows = Vec::new();
    for zoom in min_zoom..=max_zoom {
        let band = (zoom_base_id(zoom), zoom_base_id(zoom + 1) - 1);
        let window = match bounds {
            Some(bounds) => {
                let rect = padded_rect(bounds, zoom, pad_tiles);
                let width = u64::from(rect.x_max - rect.x_min) + 1;
                let height = u64::from(rect.y_max - rect.y_min) + 1;
                if width * height <= RECT_SCAN_LIMIT {
                    let mut lowest = u64::MAX;
                    let mut highest = 0_u64;
                    for y in rect.y_min..=rect.y_max {
                        for x in rect.x_min..=rect.x_max {
                            let id = tile_id(zoom, x, y);
                            lowest = lowest.min(id);
                            highest = highest.max(id);
                        }
                    }
                    (lowest, highest)
                } else {
                    band
                }
            }
            None => band,
        };
        windows.push((zoom, window.0, window.1));
    }
    windows
}

/// Rebuild an MBTiles archive out of a woven `.mkmap`: tile blobs are copied
/// byte-verbatim out of the shards and the metadata table (compression and
/// shared dictionary included) is carried over, so the result feeds straight
/// back into `transmux`. With `--bbox` this reconstructs one bake cell; the
/// extraction may pick up neighbouring tiles that share the boundary, which
/// is harmless for both re-weaving and serving. A cell holds a few tiles just
/// outside its declared bounds (the bake buffers geometry by a fraction of a
/// tile before deciding which tiles it touches), so reconstructing one takes
/// `--pad-tiles 1`.
pub fn extract(options: ExtractOptions) -> Result<(), String> {
    let started = Instant::now();
    if options.output.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite it",
            options.output.display()
        ));
    }
    let mut reader = MkmapReader::open(&options.source)
        .map_err(|err| format!("open {}: {err}", options.source.display()))?;
    let metadata = reader
        .get_metadata()
        .map_err(|err| format!("read mkmap metadata: {err}"))?;
    let (archive_min, archive_max) = reader.zoom_range();
    let min_zoom = options
        .min_zoom
        .map_or(archive_min as u8, |zoom| zoom.max(archive_min as u8));
    let max_zoom = options
        .max_zoom
        .map_or(archive_max as u8, |zoom| zoom.min(archive_max as u8));
    if min_zoom > max_zoom {
        return Err(format!(
            "requested zoom range is empty (archive holds z{archive_min}..z{archive_max})"
        ));
    }
    println!(
        "mkmap-extract: {} ({} shards, {} tiles, z{archive_min}..z{archive_max})",
        options.source.display(),
        reader.shard_count(),
        reader.tile_count()
    );

    // Pass 1: pick the tiles, in whatever order the container yields them.
    let mut selected: Vec<Selection> = Vec::new();
    for (zoom, start_id, end_id) in
        selection_windows(min_zoom, max_zoom, options.bounds, options.pad_tiles)
    {
        let rect = options
            .bounds
            .map(|bounds| padded_rect(bounds, zoom, options.pad_tiles));
        let mut rejected = Ok(());
        reader
            .for_each_tile_ref_in_range(start_id, end_id, |tile| {
                if tile.zoom != zoom {
                    return;
                }
                if let Some(rect) = rect {
                    if !rect.contains(tile.x, tile.y) {
                        return;
                    }
                }
                match tile_rowid_xyz(tile.zoom, tile.x, tile.y) {
                    Some(rowid) => selected.push(Selection { rowid, tile }),
                    None => {
                        rejected = Err(format!(
                            "tile z{}/{}/{} has no MBTiles rowid",
                            tile.zoom, tile.x, tile.y
                        ))
                    }
                }
            })
            .map_err(|err| format!("walk z{zoom} directory: {err}"))?;
        rejected?;
    }
    if selected.is_empty() {
        return Err("selection is empty — nothing to extract".to_string());
    }
    // MBTiles rows are written in ascending rowid order; the container hands
    // tiles out in Hilbert order, so sort once before streaming.
    selected.sort_unstable_by_key(|entry| entry.rowid);
    let observed_min = selected.iter().map(|entry| entry.tile.zoom).min().unwrap();
    let observed_max = selected.iter().map(|entry| entry.tile.zoom).max().unwrap();
    println!(
        "  selected {} tiles, z{observed_min}..z{observed_max}",
        selected.len()
    );

    // Pass 2: metadata table first (the codec has to be declared before the
    // verbatim blobs mean anything), then the blobs themselves.
    let mut writer = MbtilesWriter::create(&options.output)
        .map_err(|err| format!("create {}: {err}", options.output.display()))?;
    for (key, value) in &metadata {
        writer.set_metadata(key.clone(), value.clone());
    }
    match (reader.dict(), reader.shared_dict()) {
        (Some(declared), Some(raw)) if declared != raw => {
            return Err(
                "mkmap dictionary section disagrees with its compression_dict metadata row"
                    .to_string(),
            )
        }
        // A container whose metadata row went missing still carries the raw
        // dictionary; re-declare it so the extraction decodes.
        (None, Some(raw)) => {
            for (key, value) in
                compression_metadata_rows(&TileCompression::Brotli { quality: 11 }, Some(raw))
            {
                writer.set_metadata(key, value);
            }
            eprintln!(
                "mkmap-extract: NOTE metadata carried no {COMPRESSION_DICT_METADATA_KEY}; \
                 restored it from the index dictionary section"
            );
        }
        _ => {}
    }
    writer.set_metadata("minzoom", observed_min.to_string());
    writer.set_metadata("maxzoom", observed_max.to_string());
    if let Some(bounds) = options.bounds {
        writer.set_metadata("bounds", bounds.as_csv());
        let (longitude, latitude) = bounds.center();
        let center_zoom = metadata
            .get("center")
            .and_then(|value| value.split(',').nth(2).map(str::to_string))
            .unwrap_or_else(|| observed_max.to_string());
        writer.set_metadata(
            "center",
            format!("{longitude:.7},{latitude:.7},{center_zoom}"),
        );
    }

    let mut written_bytes = 0_u64;
    let mut last_progress = Instant::now();
    for (index, entry) in selected.iter().enumerate() {
        let tile = &entry.tile;
        let blob = reader
            .read_tile_ref(tile)
            .map_err(|err| format!("read z{}/{}/{}: {err}", tile.zoom, tile.x, tile.y))?;
        written_bytes += blob.len() as u64;
        writer
            .write_tile_xyz(tile.zoom, tile.x, tile.y, &blob)
            .map_err(|err| format!("write z{}/{}/{}: {err}", tile.zoom, tile.x, tile.y))?;
        if last_progress.elapsed().as_secs() >= 2 {
            println!(
                "  {}/{} tiles | {:.2} GiB",
                index + 1,
                selected.len(),
                written_bytes as f64 / 1_073_741_824.0
            );
            last_progress = Instant::now();
        }
    }
    let stats = writer
        .finish()
        .map_err(|err| format!("finalize {}: {err}", options.output.display()))?;
    println!(
        "mkmap-extract: wrote {} ({} tiles, {:.2} GiB tile bytes, {:.2} GiB file) in {:.1}s",
        options.output.display(),
        stats.tile_count,
        stats.tile_bytes as f64 / 1_073_741_824.0,
        stats.file_bytes as f64 / 1_073_741_824.0,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Weave manifest — what each source contributed, so a single cell can be
// reconstructed on demand once the sources themselves are gone
// ---------------------------------------------------------------------------

/// Read a weave source list (one path per line, `#` comments allowed) and
/// write a manifest row per source: bounds, zoom range and tile count taken
/// from the archive itself, in weave order.
pub fn write_weave_manifest(sources: &Path, output: &Path) -> Result<(), String> {
    let started = Instant::now();
    let listing = fs::read_to_string(sources)
        .map_err(|err| format!("read {}: {err}", sources.display()))?;
    let paths: Vec<&str> = listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    if paths.is_empty() {
        return Err(format!("{} lists no sources", sources.display()));
    }
    println!("mkmap: manifest over {} sources", paths.len());
    let mut rows = String::new();
    rows.push_str("# makepad weave manifest v1 — one row per source, in weave order\n");
    rows.push_str(&format!("# sources: {}\n", sources.display()));
    rows.push_str("# index\tsource\tbounds(w,s,e,n)\tminzoom\tmaxzoom\ttiles\tbytes\n");
    let mut total_tiles = 0_u64;
    let mut total_bytes = 0_u64;
    let mut unreadable = 0_usize;
    let mut last_progress = Instant::now();
    for (index, path) in paths.iter().enumerate() {
        let path = Path::new(path);
        let bytes = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
        let mut reader = match MbtilesReader::open(path) {
            Ok(reader) => reader,
            Err(err) => {
                eprintln!("mkmap: WARNING unreadable source {}: {err}", path.display());
                unreadable += 1;
                rows.push_str(&format!(
                    "{}\t{}\tUNREADABLE\t-\t-\t-\t{bytes}\n",
                    index + 1,
                    path.display()
                ));
                continue;
            }
        };
        let bounds = reader
            .get_metadata()
            .ok()
            .and_then(|metadata| metadata.get("bounds").cloned())
            .unwrap_or_else(|| "-".to_string());
        // Zoom range and tile count come from the tiles table, not the
        // metadata rows: the bakes declare a nominal minzoom the archive
        // does not actually hold.
        let summary = match reader.tile_summary() {
            Ok(summary) => summary,
            Err(err) => {
                eprintln!("mkmap: WARNING corrupt source {}: {err}", path.display());
                unreadable += 1;
                rows.push_str(&format!(
                    "{}\t{}\t{bounds}\tCORRUPT\t-\t-\t{bytes}\n",
                    index + 1,
                    path.display()
                ));
                continue;
            }
        };
        let tiles: u64 = summary.iter().map(|&(_, count)| count as u64).sum();
        let min_zoom = summary.first().map_or(-1, |&(zoom, _)| zoom);
        let max_zoom = summary.last().map_or(-1, |&(zoom, _)| zoom);
        total_tiles += tiles;
        total_bytes += bytes;
        rows.push_str(&format!(
            "{}\t{}\t{bounds}\t{min_zoom}\t{max_zoom}\t{tiles}\t{bytes}\n",
            index + 1,
            path.display()
        ));
        if last_progress.elapsed().as_secs() >= 5 {
            println!(
                "  {}/{} sources | {total_tiles} tiles | {:.1} GiB scanned",
                index + 1,
                paths.len(),
                total_bytes as f64 / 1_073_741_824.0
            );
            last_progress = Instant::now();
        }
    }
    rows.push_str(&format!(
        "# totals: {} sources ({unreadable} unreadable), {total_tiles} tiles, {total_bytes} bytes\n",
        paths.len()
    ));
    fs::write(output, rows).map_err(|err| format!("write {}: {err}", output.display()))?;
    println!(
        "mkmap: wrote {} — {} sources, {total_tiles} tiles, {:.1} GiB in {:.1}s",
        output.display(),
        paths.len(),
        total_bytes as f64 / 1_073_741_824.0,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Containment check — does an extraction really carry a source archive?
// ---------------------------------------------------------------------------

fn contains_slice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    let first = needle[0];
    (0..=haystack.len() - needle.len())
        .any(|start| haystack[start] == first && &haystack[start..start + needle.len()] == needle)
}

/// Every tile of `original` must be present in `extracted`. Bytes usually
/// match verbatim; where they do not, the weave itself explains it and the
/// check demands that explanation rather than waving the tile through:
///
/// - FIRST-WINS (z14 and above): a duplicate id was supplied by an earlier
///   weave source, so the extracted blob must equal that peer's blob.
/// - MERGED (below z14): the weave unions the layer payloads of every source
///   holding the id, so the extracted payload must *contain* this source's
///   payload (with the per-cell baked-faces field stripped, as the weave
///   strips it).
///
/// Anything else is unexplained and fails the check. Extra tiles in
/// `extracted` are fine — a bbox extraction pulls in boundary neighbours.
pub fn compare(original: &Path, extracted: &Path, peers: &[PathBuf]) -> Result<(), String> {
    let started = Instant::now();
    let mut source = MbtilesReader::open(original)
        .map_err(|err| format!("open {}: {err}", original.display()))?;
    let source_codec = TileCodec::from_metadata(
        &source
            .get_metadata()
            .map_err(|err| format!("read {} metadata: {err}", original.display()))?,
    )
    .map_err(|err| format!("{}: {err}", original.display()))?;
    let mut restored = MbtilesReader::open(extracted)
        .map_err(|err| format!("open {}: {err}", extracted.display()))?;
    let restored_codec = TileCodec::from_metadata(
        &restored
            .get_metadata()
            .map_err(|err| format!("read {} metadata: {err}", extracted.display()))?,
    )
    .map_err(|err| format!("{}: {err}", extracted.display()))?;
    let restored_tiles: u64 = restored
        .tile_summary()
        .map_err(|err| format!("scan {}: {err}", extracted.display()))?
        .iter()
        .map(|&(_, count)| count as u64)
        .sum();
    let mut peer_readers: Vec<(PathBuf, MbtilesReader, u64)> = Vec::with_capacity(peers.len());
    for path in peers {
        peer_readers.push((
            path.clone(),
            MbtilesReader::open(path).map_err(|err| format!("open {}: {err}", path.display()))?,
            0,
        ));
    }

    let mut total = 0_u64;
    let mut identical = 0_u64;
    let mut first_wins = 0_u64;
    let mut merged = 0_u64;
    let mut missing = 0_u64;
    let mut unexplained = 0_u64;
    let mut complaints: Vec<String> = Vec::new();
    let note = |complaints: &mut Vec<String>, text: String| {
        if complaints.len() < 10 {
            complaints.push(text);
        }
    };
    source
        .for_each_tile(|tile| {
            total += 1;
            let (zoom, column, row) = (tile.zoom_level, tile.tile_column, tile.tile_row);
            let restored_blob = match restored.get_tile(zoom, column, row) {
                Ok(Some(blob)) => blob,
                Ok(None) => {
                    missing += 1;
                    unexplained += 1;
                    note(
                        &mut complaints,
                        format!("z{zoom}/{column}/{row} (tms) missing from the extraction"),
                    );
                    return;
                }
                Err(err) => {
                    unexplained += 1;
                    note(
                        &mut complaints,
                        format!("z{zoom}/{column}/{row} (tms) read failed: {err}"),
                    );
                    return;
                }
            };
            if restored_blob == tile.tile_data {
                identical += 1;
                return;
            }
            // First-wins: an earlier source in the weave order supplied this
            // id, and its bytes are what the container stored.
            for (_, peer, wins) in peer_readers.iter_mut() {
                if let Ok(Some(peer_blob)) = peer.get_tile(zoom, column, row) {
                    if peer_blob == restored_blob {
                        first_wins += 1;
                        *wins += 1;
                        return;
                    }
                }
            }
            // Merged below z14: the union payload must still contain ours.
            if zoom < 14 {
                let ours = source_codec
                    .decode(&tile.tile_data)
                    .map(strip_baked_field)
                    .unwrap_or_default();
                let theirs = restored_codec.decode(&restored_blob).unwrap_or_default();
                if !ours.is_empty() && contains_slice(&theirs, &ours) {
                    merged += 1;
                    return;
                }
            }
            unexplained += 1;
            note(
                &mut complaints,
                format!(
                    "z{zoom}/{column}/{row} (tms) differs: {} source bytes vs {} extracted bytes",
                    tile.tile_data.len(),
                    restored_blob.len()
                ),
            );
        })
        .map_err(|err| format!("scan {}: {err}", original.display()))?;

    println!(
        "mbtiles-compare: {} -> {}",
        original.display(),
        extracted.display()
    );
    println!("  original tiles:    {total}");
    println!("  extraction tiles:  {restored_tiles}");
    println!("  identical:         {identical}");
    println!("  first-wins:        {first_wins} (an earlier weave source owned the id)");
    for (path, _, wins) in &peer_readers {
        if *wins > 0 {
            println!("    {wins} won by {}", path.display());
        }
    }
    println!("  merged:            {merged} (below z14; extracted payload contains ours)");
    println!("  missing:           {missing}");
    println!("  unexplained:       {unexplained}");
    for complaint in &complaints {
        println!("    {complaint}");
    }
    println!("  {:.1}s", started.elapsed().as_secs_f64());
    if unexplained > 0 {
        return Err(format!(
            "COMPARE FAILED: {unexplained} of {total} tiles unexplained"
        ));
    }
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

    /// A protobuf LEN field, the shape `strip_baked_field` walks.
    fn pbf_field(field: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        write_varint((field << 3) | 2, &mut out);
        write_varint(payload.len() as u64, &mut out);
        out.extend_from_slice(payload);
        out
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "makepad-mkmap-{tag}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn stored_tile(path: &Path, zoom: u8, x: u32, y: u32) -> Vec<u8> {
        let axis = 1_i64 << zoom;
        MbtilesReader::open(path)
            .unwrap()
            .get_tile(i64::from(zoom), i64::from(x), axis - 1 - i64::from(y))
            .unwrap()
            .unwrap_or_else(|| panic!("{} has no z{zoom}/{x}/{y}", path.display()))
    }

    /// Weave two tiny archives, extract the result, and hold the extraction
    /// to the three promises the weave makes: blobs come back byte-verbatim,
    /// a duplicate id at z14 keeps the FIRST source's bytes, and the codec
    /// (brotli + shared dictionary) survives the round trip.
    #[test]
    fn extract_restores_sources_from_a_weave() {
        let scratch = scratch_dir("extract");
        let dict = b"streets water_polygons buildings landuse".to_vec();
        let compression = TileCompression::Brotli { quality: 9 };

        // Below z14 the weave unions payloads, so give the shared z10 tile a
        // per-cell baked-faces field (101) that the union has to strip.
        let mut alpha_z10 = pbf_field(1, b"alpha-streets");
        alpha_z10.extend_from_slice(&pbf_field(101, b"per-cell-bake"));
        let beta_z10 = pbf_field(1, b"beta-streets");

        let first = scratch.join("first.mbtiles");
        let mut writer = MbtilesWriter::create(&first).unwrap();
        writer.set_tile_compression(compression, Some(dict.clone()));
        writer.set_metadata("name", "first");
        writer.set_metadata("bounds", "-180.0000000,-85.0511,180.0000000,85.0511");
        writer.set_metadata("center", "0.0000000,0.0000000,7");
        writer.write_tile_encoded(10, 6, 12, &alpha_z10).unwrap();
        writer.write_tile_encoded(14, 100, 200, b"first-only-tile").unwrap();
        writer.write_tile_encoded(14, 101, 200, b"first-copy-of-shared").unwrap();
        writer.finish().unwrap();

        let second = scratch.join("second.mbtiles");
        let mut writer = MbtilesWriter::create(&second).unwrap();
        writer.set_tile_compression(compression, Some(dict.clone()));
        writer.set_metadata("name", "second");
        writer.write_tile_encoded(10, 6, 12, &beta_z10).unwrap();
        writer.write_tile_encoded(14, 101, 200, b"second-copy-of-shared").unwrap();
        writer.write_tile_encoded(14, 102, 200, b"second-only-tile").unwrap();
        writer.finish().unwrap();

        let woven = scratch.join("woven.mkmap");
        transmux(TransmuxOptions {
            source: first.clone(),
            extra_sources: vec![second.clone()],
            output: woven.clone(),
            shard_cap: SHARD_HARD_CAP,
            sample_stride: 1,
        })
        .unwrap();

        let restored = scratch.join("restored.mbtiles");
        extract(ExtractOptions {
            source: woven.clone(),
            output: restored.clone(),
            bounds: None,
            min_zoom: None,
            max_zoom: None,
            pad_tiles: 0,
        })
        .unwrap();

        // The codec and its dictionary survive, so the extraction decodes and
        // can be woven again without re-encoding a single tile.
        let mut reader = MbtilesReader::open(&restored).unwrap();
        let metadata = reader.get_metadata().unwrap();
        assert_eq!(metadata.get("compression").map(String::as_str), Some("br:dict-v1"));
        let codec = TileCodec::from_metadata(&metadata).unwrap();
        assert_eq!(codec.dict(), Some(dict.as_slice()));
        assert_eq!(metadata.get("name").map(String::as_str), Some("first"));
        assert_eq!(metadata.get("minzoom").map(String::as_str), Some("10"));
        assert_eq!(metadata.get("maxzoom").map(String::as_str), Some("14"));
        assert_eq!(
            reader
                .tile_summary()
                .unwrap()
                .iter()
                .map(|&(_, count)| count)
                .sum::<usize>(),
            4
        );

        // Verbatim: tiles only one source held come back bit for bit.
        assert_eq!(
            stored_tile(&restored, 14, 100, 200),
            stored_tile(&first, 14, 100, 200)
        );
        assert_eq!(
            stored_tile(&restored, 14, 102, 200),
            stored_tile(&second, 14, 102, 200)
        );
        // First-wins: the shared z14 id keeps the FIRST source's bytes.
        assert_eq!(
            stored_tile(&restored, 14, 101, 200),
            stored_tile(&first, 14, 101, 200)
        );
        assert_ne!(
            stored_tile(&restored, 14, 101, 200),
            stored_tile(&second, 14, 101, 200)
        );
        // Merged: below z14 the shared id is the union of both payloads, with
        // the per-cell baked-faces field stripped out.
        let mut expected = pbf_field(1, b"alpha-streets");
        expected.extend_from_slice(&pbf_field(1, b"beta-streets"));
        let merged = codec.decode(&stored_tile(&restored, 10, 6, 12)).unwrap();
        assert_eq!(merged, expected);
        assert!(contains_slice(&merged, &strip_baked_field(alpha_z10)));
        assert!(contains_slice(&merged, &beta_z10));

        // A bbox narrows the extraction to the tiles it covers.
        let clipped = scratch.join("clipped.mbtiles");
        // A longitude slice one z14 column wide: only x=100 survives it.
        let bounds = GeoBounds::parse("-177.80,84.0,-177.79,85.0").unwrap();
        extract(ExtractOptions {
            source: woven,
            output: clipped.clone(),
            bounds: Some(bounds),
            min_zoom: Some(14),
            max_zoom: Some(14),
            pad_tiles: 0,
        })
        .unwrap();
        let clipped_tiles: usize = MbtilesReader::open(&clipped)
            .unwrap()
            .tile_summary()
            .unwrap()
            .iter()
            .map(|&(_, count)| count)
            .sum();
        assert_eq!(clipped_tiles, 1);

        // Containment holds against the originals, with the weave's own rules
        // as the only permitted explanation for a byte difference.
        compare(&first, &restored, &[]).unwrap();
        compare(&second, &restored, &[first.clone()]).unwrap();

        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn contains_slice_finds_merged_payloads() {
        assert!(contains_slice(b"abcdef", b"cde"));
        assert!(contains_slice(b"abcdef", b"abcdef"));
        assert!(contains_slice(b"abcdef", b""));
        assert!(!contains_slice(b"abcdef", b"cdf"));
        assert!(!contains_slice(b"abc", b"abcd"));
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
