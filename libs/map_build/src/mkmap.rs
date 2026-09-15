//! Shared deterministic writer primitives for the sharded `.mkmap` format.

use makepad_mbtile_reader::{compress_tile, TileCompression};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

pub const SHARD_HARD_CAP: u64 = 510_000_000;
pub const MAGIC: &[u8; 8] = b"MKMAPIX1";
pub const VERSION: u32 = 2;
pub const HEADER_LEN: usize = 112;
pub const ROOT_RECORD_LEN: usize = 36;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobRef {
    pub shard: u32,
    pub offset: u64,
    pub len: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeafEntry {
    pub tile_id: u64,
    pub blob: BlobRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootRecord {
    pub start_tile_id: u64,
    pub end_tile_id: u64,
    pub shard: u32,
    pub dir_offset: u64,
    pub dir_len: u64,
}

pub fn write_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

pub fn content_hash(bytes: &[u8]) -> u128 {
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
        hash ^ bytes.len() as u64
    }
    (u128::from(mix(0x5851_f42d_4c95_7f2d, bytes)) << 64)
        | u128::from(mix(0x1405_7b7e_f767_814f, bytes))
}

fn brotli_pack(bytes: &[u8]) -> Result<Vec<u8>, String> {
    compress_tile(&TileCompression::Brotli { quality: 9 }, None, bytes)
        .map_err(|err| format!("brotli pack: {err}"))
}

pub fn encode_leaf_directory(entries: &[LeafEntry]) -> Result<Vec<u8>, String> {
    let mut raw = Vec::with_capacity(entries.len() * 8);
    write_varint(entries.len() as u64, &mut raw);
    let mut previous_id = 0_u64;
    for entry in entries {
        if entry.tile_id < previous_id || entry.blob.len == 0 {
            return Err("mkmap leaf entries are not ordered or contain an empty blob".to_string());
        }
        write_varint(entry.tile_id - previous_id, &mut raw);
        previous_id = entry.tile_id;
        write_varint(u64::from(entry.blob.shard), &mut raw);
        write_varint(entry.blob.offset, &mut raw);
        write_varint(entry.blob.len, &mut raw);
    }
    brotli_pack(&raw)
}

pub struct RootIndex<'a> {
    pub metadata: &'a HashMap<String, String>,
    pub dict: Option<&'a [u8]>,
    pub shard_cap: u64,
    pub tile_count: u64,
    pub unique_blobs: u64,
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub records: &'a [RootRecord],
}

pub fn write_root_index(output: &Path, index: &RootIndex<'_>) -> Result<u64, String> {
    if index.records.len() > u32::MAX as usize || index.min_zoom > index.max_zoom {
        return Err("invalid mkmap root index counts".to_string());
    }
    let mut metadata: Vec<_> = index.metadata.iter().collect();
    metadata.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let mut metadata_raw = Vec::new();
    write_varint(metadata.len() as u64, &mut metadata_raw);
    for (key, value) in metadata {
        write_varint(key.len() as u64, &mut metadata_raw);
        metadata_raw.extend_from_slice(key.as_bytes());
        write_varint(value.len() as u64, &mut metadata_raw);
        metadata_raw.extend_from_slice(value.as_bytes());
    }
    let metadata_br = brotli_pack(&metadata_raw)?;
    let mut root_raw = Vec::with_capacity(index.records.len() * ROOT_RECORD_LEN);
    for (expected_shard, record) in index.records.iter().enumerate() {
        if record.shard != expected_shard as u32 || record.start_tile_id > record.end_tile_id {
            return Err("mkmap root records are not contiguous and ordered".to_string());
        }
        root_raw.extend_from_slice(&record.start_tile_id.to_le_bytes());
        root_raw.extend_from_slice(&record.end_tile_id.to_le_bytes());
        root_raw.extend_from_slice(&record.shard.to_le_bytes());
        root_raw.extend_from_slice(&record.dir_offset.to_le_bytes());
        root_raw.extend_from_slice(&record.dir_len.to_le_bytes());
    }
    let root_br = brotli_pack(&root_raw)?;
    let dict = index.dict.unwrap_or(&[]);
    let mut header = vec![0_u8; HEADER_LEN];
    header[0..8].copy_from_slice(MAGIC);
    header[8..12].copy_from_slice(&VERSION.to_le_bytes());
    header[12..16].copy_from_slice(&(index.records.len() as u32).to_le_bytes());
    header[16..24].copy_from_slice(&index.shard_cap.to_le_bytes());
    header[24..32].copy_from_slice(&index.tile_count.to_le_bytes());
    header[32..40].copy_from_slice(&index.unique_blobs.to_le_bytes());
    header[40] = index.min_zoom;
    header[41] = index.max_zoom;
    let mut cursor = HEADER_LEN as u64;
    for (slot, len) in [
        (48_usize, metadata_br.len() as u64),
        (64, dict.len() as u64),
        (80, root_raw.len() as u64),
        (96, root_br.len() as u64),
    ] {
        header[slot..slot + 8].copy_from_slice(&cursor.to_le_bytes());
        header[slot + 8..slot + 16].copy_from_slice(&len.to_le_bytes());
        cursor += len;
    }
    let mut file = BufWriter::new(
        File::create(output).map_err(|err| format!("create {}: {err}", output.display()))?,
    );
    file.write_all(&header)
        .and_then(|_| file.write_all(&metadata_br))
        .and_then(|_| file.write_all(dict))
        .and_then(|_| file.write_all(&root_raw))
        .and_then(|_| file.write_all(&root_br))
        .and_then(|_| file.flush())
        .map_err(|err| format!("write {}: {err}", output.display()))?;
    Ok(cursor)
}

