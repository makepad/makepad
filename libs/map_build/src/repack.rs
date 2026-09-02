//! Lossless-in-the-renderer-sense rewrite of decoded Makepad vector tiles.

use crate::mkmap::{
    content_hash, encode_leaf_directory, write_root_index, BlobRef, LeafEntry, RootIndex,
    RootRecord, SHARD_HARD_CAP,
};
use makepad_mbtile_reader::{
    compress_tile, read_pb_len_slice, read_pb_varint, skip_pb_field, MkmapReader, MkmapTileRef,
    TileCodec, TileCompression, DETAIL_POINT_EXTRA_KEYS, DETAIL_WAY_KEYS,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc::sync_channel, Arc, Mutex};
use std::time::{Duration, Instant};

pub const DETAIL_LAYERS: &[&str] = &[
    "osm_points",
    "osm_lines",
    "osm_polygons",
    "osm_relation_lines",
    "osm_relation_polygons",
    "osm_relation_points",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyAction {
    Keep,
    Drop,
}

#[derive(Clone, Copy, Debug)]
pub struct PolicyRow {
    pub action: PolicyAction,
    pub item: &'static str,
    pub reader_or_reason: &'static str,
}

pub const POLICY_BASE: usize = 0;
pub const POLICY_DETAIL: usize = 1;
pub const POLICY_REGIONS: usize = 2;
pub const POLICY_BUILDINGS: usize = 3;
pub const POLICY_FILLS: usize = 4;
pub const POLICY_TAGS: usize = 5;
pub const POLICY_SYNTHETIC: usize = 6;
pub const POLICY_SHADOWS: usize = 7;

/// The archive data contract. Keep this table in lock-step with renderer
/// reads; reports use the same rows, so every removed byte has a stated case.
pub const DATA_POLICY: [PolicyRow; 8] = [
    PolicyRow {
        action: PolicyAction::Keep,
        item: "shortbread base layers (non-osm_*)",
        reader_or_reason: "widgets/src/map/tile.rs: LayerParseFilter::BaseNoDetailLayers",
    },
    PolicyRow {
        action: PolicyAction::Keep,
        item: "six osm_* detail layers: geometry + whitelisted tags",
        reader_or_reason: "widgets/src/map/tile.rs: LayerParseFilter::DetailLayers/tag_key_whitelist",
    },
    PolicyRow {
        action: PolicyAction::Keep,
        item: "field 101 painter-cascade REGIONS",
        reader_or_reason: "widgets/src/map/tile.rs: parse_baked_faces/bake.regions cascade hit",
    },
    PolicyRow {
        action: PolicyAction::Keep,
        item: "field 101 v4 building groups",
        reader_or_reason: "widgets/src/map/tile.rs: bake.building_signature/bake.buildings substitution",
    },
    PolicyRow {
        action: PolicyAction::Keep,
        item: "field 100 baked fill triangulations",
        reader_or_reason: "widgets/src/map/tile.rs: parse_baked_fills",
    },
    PolicyRow {
        action: PolicyAction::Drop,
        item: "osm_* tags outside DETAIL_WAY_KEYS + DETAIL_POINT_EXTRA_KEYS",
        reader_or_reason: "discarded by widgets/src/map/tile.rs: tag_key_whitelist",
    },
    PolicyRow {
        action: PolicyAction::Drop,
        item: "__makepad_osm_id/type/closed",
        reader_or_reason: "renderer-tree matches are cfg(test) probes; production only writes them",
    },
    PolicyRow {
        action: PolicyAction::Drop,
        item: "field 101 shadow shapes + grounded footprints",
        reader_or_reason: "widgets/src/map/view.rs: draw_shadow_mask_pass derives live shadows",
    },
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TileRewriteStats {
    pub decoded_before: u64,
    pub decoded_after: u64,
    pub savings: [u64; DATA_POLICY.len()],
}

impl TileRewriteStats {
    pub fn add_assign(&mut self, other: &Self) {
        self.decoded_before += other.decoded_before;
        self.decoded_after += other.decoded_after;
        for (left, right) in self.savings.iter_mut().zip(other.savings) {
            *left += right;
        }
    }
}

#[derive(Clone, Copy)]
struct PbField {
    start: usize,
    end: usize,
    field: u32,
    wire: u8,
    payload_start: usize,
    payload_end: usize,
}

fn next_field(bytes: &[u8], pos: &mut usize) -> Result<PbField, String> {
    let start = *pos;
    let key = read_pb_varint(bytes, pos)?;
    let field = u32::try_from(key >> 3).map_err(|_| "protobuf field number overflow".to_string())?;
    let wire = (key & 7) as u8;
    if field == 0 {
        return Err("protobuf field zero".to_string());
    }
    let payload_start;
    let payload_end;
    if wire == 2 {
        let payload = read_pb_len_slice(bytes, pos)?;
        payload_start = payload.as_ptr() as usize - bytes.as_ptr() as usize;
        payload_end = payload_start + payload.len();
    } else {
        payload_start = *pos;
        skip_pb_field(bytes, pos, wire)?;
        payload_end = *pos;
    }
    Ok(PbField {
        start,
        end: *pos,
        field,
        wire,
        payload_start,
        payload_end,
    })
}

fn write_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn write_len_field(field: u32, payload: &[u8], out: &mut Vec<u8>) {
    write_varint(u64::from(field) << 3 | 2, out);
    write_varint(payload.len() as u64, out);
    out.extend_from_slice(payload);
}

fn is_detail_layer(name: &str) -> bool {
    DETAIL_LAYERS.contains(&name)
}

fn is_synthetic_key(key: &str) -> bool {
    matches!(
        key,
        "__makepad_osm_id" | "__makepad_osm_type" | "__makepad_osm_closed"
    )
}

pub fn detail_key_allowed(key: &str) -> bool {
    DETAIL_WAY_KEYS.contains(&key) || DETAIL_POINT_EXTRA_KEYS.contains(&key)
}

fn layer_name(layer: &[u8]) -> Result<&str, String> {
    let mut pos = 0;
    while pos < layer.len() {
        let field = next_field(layer, &mut pos)?;
        if field.field == 1 && field.wire == 2 {
            return std::str::from_utf8(&layer[field.payload_start..field.payload_end])
                .map_err(|_| "MVT layer name is not UTF-8".to_string());
        }
    }
    Err("MVT layer has no name".to_string())
}

fn feature_tags(feature: &[u8]) -> Result<Vec<(usize, usize)>, String> {
    let mut tags = Vec::new();
    let mut pos = 0;
    while pos < feature.len() {
        let field = next_field(feature, &mut pos)?;
        if field.field == 2 {
            if field.wire != 2 {
                return Err("MVT feature tags are not packed".to_string());
            }
            let mut packed_pos = field.payload_start;
            while packed_pos < field.payload_end {
                let key = usize::try_from(read_pb_varint(feature, &mut packed_pos)?)
                    .map_err(|_| "MVT key index overflow".to_string())?;
                let value = usize::try_from(read_pb_varint(feature, &mut packed_pos)?)
                    .map_err(|_| "MVT value index overflow".to_string())?;
                tags.push((key, value));
            }
            if packed_pos != field.payload_end {
                return Err("MVT packed tags overrun".to_string());
            }
        }
    }
    Ok(tags)
}

struct LayerTables<'a> {
    fields: Vec<PbField>,
    keys: Vec<&'a str>,
    values: Vec<&'a [u8]>,
    features: Vec<&'a [u8]>,
}

fn parse_layer_tables(layer: &[u8]) -> Result<LayerTables<'_>, String> {
    let mut fields = Vec::new();
    let mut keys = Vec::new();
    let mut values = Vec::new();
    let mut features = Vec::new();
    let mut pos = 0;
    while pos < layer.len() {
        let field = next_field(layer, &mut pos)?;
        match (field.field, field.wire) {
            (2, 2) => features.push(&layer[field.payload_start..field.payload_end]),
            (3, 2) => keys.push(
                std::str::from_utf8(&layer[field.payload_start..field.payload_end])
                    .map_err(|_| "MVT key is not UTF-8".to_string())?,
            ),
            (4, 2) => values.push(&layer[field.payload_start..field.payload_end]),
            _ => {}
        }
        fields.push(field);
    }
    Ok(LayerTables {
        fields,
        keys,
        values,
        features,
    })
}

fn rewrite_detail_layer_with(
    layer: &[u8],
    allowed: impl Fn(&str) -> bool,
) -> Result<Vec<u8>, String> {
    let tables = parse_layer_tables(layer)?;
    let mut kept_tags = Vec::with_capacity(tables.features.len());
    let mut used_keys = vec![false; tables.keys.len()];
    let mut used_values = vec![false; tables.values.len()];
    for feature in &tables.features {
        let mut tags = Vec::new();
        for (key, value) in feature_tags(feature)? {
            let key_name = tables
                .keys
                .get(key)
                .ok_or_else(|| "MVT feature key index is out of range".to_string())?;
            if value >= tables.values.len() {
                return Err("MVT feature value index is out of range".to_string());
            }
            if allowed(key_name) {
                used_keys[key] = true;
                used_values[value] = true;
                tags.push((key, value));
            }
        }
        kept_tags.push(tags);
    }
    let mut key_map = vec![None; tables.keys.len()];
    let mut retained_keys = Vec::new();
    let mut key_by_name: HashMap<&str, u32> = HashMap::new();
    for (old, key) in tables.keys.iter().copied().enumerate() {
        if !used_keys[old] {
            continue;
        }
        let mapped = if let Some(mapped) = key_by_name.get(key) {
            *mapped
        } else {
            let mapped = retained_keys.len() as u32;
            retained_keys.push(key);
            key_by_name.insert(key, mapped);
            mapped
        };
        key_map[old] = Some(mapped);
    }
    let mut value_map = vec![None; tables.values.len()];
    let mut retained_values = Vec::new();
    let mut value_by_payload: HashMap<&[u8], u32> = HashMap::new();
    for (old, value) in tables.values.iter().copied().enumerate() {
        if !used_values[old] {
            continue;
        }
        let mapped = if let Some(mapped) = value_by_payload.get(value) {
            *mapped
        } else {
            let mapped = retained_values.len() as u32;
            retained_values.push(value);
            value_by_payload.insert(value, mapped);
            mapped
        };
        value_map[old] = Some(mapped);
    }
    let mut rewritten_features = Vec::with_capacity(tables.features.len());
    for (feature, tags) in tables.features.iter().zip(kept_tags) {
        let mut packed = Vec::new();
        for (key, value) in tags {
            write_varint(u64::from(key_map[key].unwrap()), &mut packed);
            write_varint(u64::from(value_map[value].unwrap()), &mut packed);
        }
        let rewritten = rewrite_feature_filtered(feature, &packed)?;
        rewritten_features.push(rewritten);
    }
    let mut out = Vec::with_capacity(layer.len());
    let mut feature_index = 0;
    let mut wrote_keys = false;
    let mut wrote_values = false;
    for field in tables.fields {
        match (field.field, field.wire) {
            (2, 2) => {
                write_len_field(2, &rewritten_features[feature_index], &mut out);
                feature_index += 1;
            }
            (3, 2) => {
                if !wrote_keys {
                    for key in &retained_keys {
                        write_len_field(3, key.as_bytes(), &mut out);
                    }
                    wrote_keys = true;
                }
            }
            (4, 2) => {
                if !wrote_values {
                    for value in &retained_values {
                        write_len_field(4, value, &mut out);
                    }
                    wrote_values = true;
                }
            }
            _ => out.extend_from_slice(&layer[field.start..field.end]),
        }
    }
    Ok(out)
}

fn rewrite_feature_filtered(
    feature: &[u8],
    packed: &[u8],
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(feature.len());
    let mut pos = 0;
    let mut wrote_tags = false;
    while pos < feature.len() {
        let field = next_field(feature, &mut pos)?;
        if field.field == 2 {
            if !wrote_tags && !packed.is_empty() {
                write_len_field(2, packed, &mut out);
            }
            wrote_tags = true;
        } else {
            out.extend_from_slice(&feature[field.start..field.end]);
        }
    }
    Ok(out)
}

fn fnv_step(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100_0000_01b3);
    }
}

fn scan_shapes(
    bytes: &[u8],
    pos: &mut usize,
    first: &mut u64,
    mut second: Option<&mut u64>,
) -> Result<(), String> {
    let shapes = usize::try_from(read_pb_varint(bytes, pos)?)
        .map_err(|_| "field 101 shape count overflow".to_string())?;
    if shapes > 1_000_000 {
        return Err("field 101 shape count exceeds limit".to_string());
    }
    for _ in 0..shapes {
        let rings = usize::try_from(read_pb_varint(bytes, pos)?)
            .map_err(|_| "field 101 ring count overflow".to_string())?;
        if rings > 1_000_000 {
            return Err("field 101 ring count exceeds limit".to_string());
        }
        for _ in 0..rings {
            let points = usize::try_from(read_pb_varint(bytes, pos)?)
                .map_err(|_| "field 101 point count overflow".to_string())?;
            if points > 4_000_000 {
                return Err("field 101 point count exceeds limit".to_string());
            }
            let (mut x, mut y) = (0_i64, 0_i64);
            for _ in 0..points {
                x = x.wrapping_add(zigzag_decode(read_pb_varint(bytes, pos)?));
                y = y.wrapping_add(zigzag_decode(read_pb_varint(bytes, pos)?));
                fnv_step(first, &x.to_le_bytes());
                fnv_step(first, &y.to_le_bytes());
                if let Some(hash) = second.as_deref_mut() {
                    fnv_step(hash, &x.to_le_bytes());
                    fnv_step(hash, &y.to_le_bytes());
                }
            }
        }
    }
    Ok(())
}

fn zigzag_decode(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

fn rewrite_baked_faces(blob: &[u8]) -> Result<Vec<u8>, String> {
    let version = *blob.first().ok_or_else(|| "empty field 101".to_string())?;
    if version != 3 && version != 4 {
        return Err(format!("unsupported field 101 version {version}"));
    }
    let mut pos = 1;
    let bucket_count = usize::try_from(read_pb_varint(blob, &mut pos)?)
        .map_err(|_| "field 101 bucket count overflow".to_string())?;
    if bucket_count > 1024 {
        return Err("field 101 bucket count exceeds limit".to_string());
    }
    let prefix_end = pos;
    let mut out = Vec::with_capacity(blob.len());
    out.extend_from_slice(&blob[..prefix_end]);
    for _ in 0..bucket_count {
        let bucket_start = pos;
        let _ = read_pb_varint(blob, &mut pos)?;
        let signature_end = pos
            .checked_add(8)
            .filter(|end| *end <= blob.len())
            .ok_or_else(|| "truncated field 101 signature".to_string())?;
        pos = signature_end;
        let checksum_pos = pos;
        let checksum_end = pos
            .checked_add(8)
            .filter(|end| *end <= blob.len())
            .ok_or_else(|| "truncated field 101 checksum".to_string())?;
        let stored_checksum = u64::from_le_bytes(blob[pos..checksum_end].try_into().unwrap());
        pos = checksum_end;
        let body_len = usize::try_from(read_pb_varint(blob, &mut pos)?)
            .map_err(|_| "field 101 body length overflow".to_string())?;
        let body_end = pos
            .checked_add(body_len)
            .filter(|end| *end <= blob.len())
            .ok_or_else(|| "truncated field 101 body".to_string())?;
        let body = &blob[pos..body_end];
        pos = body_end;

        let mut bpos = 0;
        let mut old_checksum = 0xcbf2_9ce4_8422_2325;
        let mut new_checksum = old_checksum;
        let regions = usize::try_from(read_pb_varint(body, &mut bpos)?)
            .map_err(|_| "field 101 region count overflow".to_string())?;
        if regions > 100_000 {
            return Err("field 101 region count exceeds limit".to_string());
        }
        for _ in 0..regions {
            let _ = read_pb_varint(body, &mut bpos)?;
            for _ in 0..3 {
                scan_shapes(
                    body,
                    &mut bpos,
                    &mut old_checksum,
                    Some(&mut new_checksum),
                )?;
            }
        }
        let regions_end = bpos;
        bpos = bpos
            .checked_add(8)
            .filter(|end| *end <= body.len())
            .ok_or_else(|| "truncated field 101 shadow signature".to_string())?;
        scan_shapes(body, &mut bpos, &mut old_checksum, None)?;
        scan_shapes(body, &mut bpos, &mut old_checksum, None)?;
        let buildings_start = bpos;
        if version == 4 {
            bpos = bpos
                .checked_add(8)
                .filter(|end| *end <= body.len())
                .ok_or_else(|| "truncated field 101 building signature".to_string())?;
            let groups = usize::try_from(read_pb_varint(body, &mut bpos)?)
                .map_err(|_| "field 101 building count overflow".to_string())?;
            if groups > 100_000 {
                return Err("field 101 building count exceeds limit".to_string());
            }
            for _ in 0..groups {
                let _ = read_pb_varint(body, &mut bpos)?;
                let _ = read_pb_varint(body, &mut bpos)?;
                scan_shapes(
                    body,
                    &mut bpos,
                    &mut old_checksum,
                    Some(&mut new_checksum),
                )?;
            }
        }
        if bpos != body.len() {
            return Err("field 101 body has trailing bytes".to_string());
        }
        if old_checksum != stored_checksum {
            return Err(format!(
                "field 101 checksum mismatch: stored {stored_checksum:016x}, got {old_checksum:016x}"
            ));
        }
        let mut new_body = Vec::with_capacity(body.len());
        new_body.extend_from_slice(&body[..regions_end]);
        new_body.extend_from_slice(&0_u64.to_le_bytes());
        write_varint(0, &mut new_body);
        write_varint(0, &mut new_body);
        new_body.extend_from_slice(&body[buildings_start..]);
        out.extend_from_slice(&blob[bucket_start..checksum_pos]);
        out.extend_from_slice(&new_checksum.to_le_bytes());
        write_varint(new_body.len() as u64, &mut out);
        out.extend_from_slice(&new_body);
    }
    if pos != blob.len() {
        return Err("field 101 has trailing bytes".to_string());
    }
    Ok(out)
}

fn varint_len(mut value: u64) -> usize {
    let mut len = 1;
    while value >= 0x80 {
        len += 1;
        value >>= 7;
    }
    len
}

fn len_field_size(field: u32, payload_len: usize) -> usize {
    varint_len(u64::from(field) << 3 | 2) + varint_len(payload_len as u64) + payload_len
}

/// Rewrite one decoded tile protobuf according to [`DATA_POLICY`].
pub fn rewrite_tile(input: &[u8]) -> Result<(Vec<u8>, TileRewriteStats), String> {
    let mut out = Vec::with_capacity(input.len());
    let mut stats = TileRewriteStats {
        decoded_before: input.len() as u64,
        ..Default::default()
    };
    let mut pos = 0;
    while pos < input.len() {
        let field = next_field(input, &mut pos)?;
        match (field.field, field.wire) {
            (3, 2) => {
                let layer = &input[field.payload_start..field.payload_end];
                if is_detail_layer(layer_name(layer)?) {
                    let final_layer = rewrite_detail_layer_with(layer, detail_key_allowed)?;
                    let no_synthetic =
                        rewrite_detail_layer_with(layer, |key| !is_synthetic_key(key))?;
                    let old_size = field.end - field.start;
                    let total = old_size.saturating_sub(len_field_size(3, final_layer.len())) as u64;
                    let synthetic = old_size
                        .saturating_sub(len_field_size(3, no_synthetic.len()))
                        as u64;
                    stats.savings[POLICY_SYNTHETIC] += synthetic.min(total);
                    stats.savings[POLICY_TAGS] += total.saturating_sub(synthetic);
                    write_len_field(3, &final_layer, &mut out);
                } else {
                    out.extend_from_slice(&input[field.start..field.end]);
                }
            }
            (101, 2) => {
                let rewritten = rewrite_baked_faces(&input[field.payload_start..field.payload_end])?;
                stats.savings[POLICY_SHADOWS] += (field.end - field.start)
                    .saturating_sub(len_field_size(101, rewritten.len())) as u64;
                write_len_field(101, &rewritten, &mut out);
            }
            _ => out.extend_from_slice(&input[field.start..field.end]),
        }
    }
    stats.decoded_after = out.len() as u64;
    Ok((out, stats))
}

/// Strict verifier used by both unit tests and `makepad-map-repack --verify`.
pub fn verify_rewritten_tile(before: &[u8], after: &[u8]) -> Result<(), String> {
    let expected = rewrite_tile(before)?.0;
    if expected != after {
        return Err("repacked tile differs from the deterministic policy rewrite".to_string());
    }
    let again = rewrite_tile(after)?.0;
    if again != after {
        return Err("repacked tile is not idempotent".to_string());
    }
    verify_kept_sections(before, after)
}

#[derive(Default)]
struct TileSections {
    base_layers: Vec<(String, Vec<u8>)>,
    detail_geometry: Vec<(String, Vec<Vec<u8>>)>,
    detail_tags: Vec<(String, Vec<Vec<(String, Vec<u8>)>>)>,
    fills: Vec<Vec<u8>>,
    faces: Vec<(u64, Vec<u8>, Vec<u8>, bool)>,
}

fn feature_without_tags(feature: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < feature.len() {
        let field = next_field(feature, &mut pos)?;
        if field.field != 2 {
            out.extend_from_slice(&feature[field.start..field.end]);
        }
    }
    Ok(out)
}

fn face_sections(blob: &[u8]) -> Result<Vec<(u64, Vec<u8>, Vec<u8>, bool)>, String> {
    let version = *blob.first().ok_or_else(|| "empty field 101".to_string())?;
    if version != 3 && version != 4 {
        return Err(format!("unsupported field 101 version {version}"));
    }
    let mut pos = 1;
    let count = usize::try_from(read_pb_varint(blob, &mut pos)?)
        .map_err(|_| "field 101 count overflow".to_string())?;
    let mut sections = Vec::with_capacity(count);
    for _ in 0..count {
        let bucket = read_pb_varint(blob, &mut pos)?;
        pos = pos
            .checked_add(16)
            .filter(|end| *end <= blob.len())
            .ok_or_else(|| "truncated field 101 bucket header".to_string())?;
        let len = usize::try_from(read_pb_varint(blob, &mut pos)?)
            .map_err(|_| "field 101 body overflow".to_string())?;
        let body = blob
            .get(pos..pos + len)
            .ok_or_else(|| "truncated field 101 body".to_string())?;
        pos += len;
        let mut bpos = 0;
        let regions = read_pb_varint(body, &mut bpos)? as usize;
        let mut discard = 0xcbf2_9ce4_8422_2325;
        for _ in 0..regions {
            let _ = read_pb_varint(body, &mut bpos)?;
            for _ in 0..3 {
                scan_shapes(body, &mut bpos, &mut discard, None)?;
            }
        }
        let region_bytes = body[..bpos].to_vec();
        bpos += 8;
        let shadow_shapes_start = bpos;
        scan_shapes(body, &mut bpos, &mut discard, None)?;
        let shapes_empty = body[shadow_shapes_start..bpos] == [0];
        let footprint_start = bpos;
        scan_shapes(body, &mut bpos, &mut discard, None)?;
        let shadows_empty = shapes_empty && body[footprint_start..bpos] == [0];
        let buildings = body[bpos..].to_vec();
        sections.push((bucket, region_bytes, buildings, shadows_empty));
    }
    Ok(sections)
}

fn tile_sections(tile: &[u8]) -> Result<TileSections, String> {
    let mut sections = TileSections::default();
    let mut pos = 0;
    while pos < tile.len() {
        let field = next_field(tile, &mut pos)?;
        match (field.field, field.wire) {
            (3, 2) => {
                let layer = &tile[field.payload_start..field.payload_end];
                let name = layer_name(layer)?.to_string();
                if is_detail_layer(&name) {
                    let tables = parse_layer_tables(layer)?;
                    let geometry = tables
                        .features
                        .iter()
                        .map(|feature| feature_without_tags(feature))
                        .collect::<Result<Vec<_>, _>>()?;
                    let tags = tables
                        .features
                        .iter()
                        .map(|feature| {
                            feature_tags(feature)?
                                .into_iter()
                                .map(|(key, value)| {
                                    let key = tables.keys.get(key).ok_or_else(|| {
                                        "MVT feature key index is out of range".to_string()
                                    })?;
                                    let value = tables.values.get(value).ok_or_else(|| {
                                        "MVT feature value index is out of range".to_string()
                                    })?;
                                    Ok(((*key).to_string(), (*value).to_vec()))
                                })
                                .collect::<Result<Vec<_>, String>>()
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    sections.detail_geometry.push((name.clone(), geometry));
                    sections.detail_tags.push((name, tags));
                } else {
                    sections.base_layers.push((name, layer.to_vec()));
                }
            }
            (100, 2) => sections.fills.push(tile[field.start..field.end].to_vec()),
            (101, 2) => sections.faces.extend(face_sections(
                &tile[field.payload_start..field.payload_end],
            )?),
            _ => {}
        }
    }
    Ok(sections)
}

fn verify_detail_tables_compact(tile: &[u8]) -> Result<(), String> {
    let mut pos = 0;
    while pos < tile.len() {
        let field = next_field(tile, &mut pos)?;
        if (field.field, field.wire) != (3, 2) {
            continue;
        }
        let layer = &tile[field.payload_start..field.payload_end];
        if !is_detail_layer(layer_name(layer)?) {
            continue;
        }
        let tables = parse_layer_tables(layer)?;
        if tables.keys.iter().any(|key| !detail_key_allowed(key)) {
            return Err("dropped detail tag key remains".to_string());
        }
        let mut used_keys = vec![false; tables.keys.len()];
        let mut used_values = vec![false; tables.values.len()];
        for feature in &tables.features {
            for (key, value) in feature_tags(feature)? {
                let key = used_keys
                    .get_mut(key)
                    .ok_or_else(|| "MVT feature key index is out of range".to_string())?;
                let value = used_values
                    .get_mut(value)
                    .ok_or_else(|| "MVT feature value index is out of range".to_string())?;
                *key = true;
                *value = true;
            }
        }
        if used_keys.iter().any(|used| !used) || used_values.iter().any(|used| !used) {
            return Err("detail tag table contains an unreferenced entry".to_string());
        }
        if tables.keys.iter().copied().collect::<BTreeSet<_>>().len() != tables.keys.len()
            || tables
                .values
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len()
                != tables.values.len()
        {
            return Err("detail tag table contains a duplicate entry".to_string());
        }
    }
    Ok(())
}

fn verify_kept_sections(before: &[u8], after: &[u8]) -> Result<(), String> {
    verify_detail_tables_compact(after)?;
    let before = tile_sections(before)?;
    let after = tile_sections(after)?;
    if before.base_layers != after.base_layers {
        return Err("non-osm base layer bytes changed".to_string());
    }
    if before.detail_geometry != after.detail_geometry {
        return Err("detail feature geometry/non-tag fields changed".to_string());
    }
    let retained_before_tags: Vec<_> = before
        .detail_tags
        .iter()
        .map(|(name, features)| {
            (
                name,
                features
                    .iter()
                    .map(|tags| {
                        tags.iter()
                            .filter(|(key, _)| detail_key_allowed(key))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let after_tags: Vec<_> = after
        .detail_tags
        .iter()
        .map(|(name, features)| {
            (
                name,
                features
                    .iter()
                    .map(|tags| tags.iter().collect::<Vec<_>>())
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    if retained_before_tags != after_tags {
        return Err("retained detail tag key/value pairs changed".to_string());
    }
    if before.fills != after.fills {
        return Err("field 100 bytes changed".to_string());
    }
    if before.faces.len() != after.faces.len() {
        return Err("field 101 bucket count changed".to_string());
    }
    for (left, right) in before.faces.iter().zip(&after.faces) {
        if left.0 != right.0 || left.1 != right.1 {
            return Err("field 101 REGIONS bytes changed".to_string());
        }
        if left.2 != right.2 {
            return Err("field 101 building-group bytes changed".to_string());
        }
        if !right.3 {
            return Err("field 101 shadow stub is not empty".to_string());
        }
    }
    Ok(())
}

pub const TILE_BROTLI_QUALITY: u32 = 11;
const MANIFEST_MAGIC: &[u8; 8] = b"MKRPMF02";

#[derive(Clone, Debug)]
pub enum TileSelection {
    All,
    HilbertRange { start: u64, end: u64 },
    Explicit(BTreeSet<u64>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShardRange {
    pub start: usize,
    pub end: usize,
}

impl ShardRange {
    fn contains(self, shard: usize) -> bool {
        (self.start..self.end).contains(&shard)
    }
}

impl TileSelection {
    fn contains(&self, id: u64) -> bool {
        match self {
            Self::All => true,
            Self::HilbertRange { start, end } => (*start..=*end).contains(&id),
            Self::Explicit(ids) => ids.contains(&id),
        }
    }

    fn fingerprint(&self) -> u128 {
        let mut bytes = Vec::new();
        match self {
            Self::All => bytes.push(0),
            Self::HilbertRange { start, end } => {
                bytes.push(1);
                bytes.extend_from_slice(&start.to_le_bytes());
                bytes.extend_from_slice(&end.to_le_bytes());
            }
            Self::Explicit(ids) => {
                bytes.push(2);
                for id in ids {
                    bytes.extend_from_slice(&id.to_le_bytes());
                }
            }
        }
        content_hash(&bytes)
    }
}

#[derive(Clone, Debug)]
pub struct RepackOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub selection: TileSelection,
    pub dry_run: bool,
    pub verify: bool,
    pub resume: bool,
    pub jobs: usize,
    pub brotli_quality: u32,
    pub verify_shards: Option<ShardRange>,
    pub log: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct RepackReport {
    pub shards: u32,
    pub tiles: u64,
    pub unique_blobs: u64,
    pub decoded_before: u64,
    pub decoded_after: u64,
    pub compressed_before: u64,
    pub compressed_after: u64,
    pub savings: [u64; DATA_POLICY.len()],
}

impl RepackReport {
    fn add_manifest(&mut self, manifest: &ShardManifest) {
        self.shards += 1;
        self.tiles += manifest.tile_count;
        self.unique_blobs += manifest.unique_blobs;
        self.decoded_before += manifest.decoded_before;
        self.decoded_after += manifest.decoded_after;
        self.compressed_before += manifest.compressed_before;
        self.compressed_after += manifest.compressed_after;
        for (total, shard) in self.savings.iter_mut().zip(manifest.savings) {
            *total += shard;
        }
    }
}

#[derive(Clone, Debug)]
struct ShardManifest {
    input_root_hash: u128,
    selection_hash: u128,
    input_record: u32,
    output_shard: u32,
    brotli_quality: u32,
    file_len: u64,
    tile_count: u64,
    unique_blobs: u64,
    start_tile_id: u64,
    end_tile_id: u64,
    dir_offset: u64,
    dir_len: u64,
    decoded_before: u64,
    decoded_after: u64,
    compressed_before: u64,
    compressed_after: u64,
    savings: [u64; DATA_POLICY.len()],
    min_zoom: u8,
    max_zoom: u8,
}

fn shard_path(dir: &Path, shard: u32) -> PathBuf {
    dir.join(format!("tiles-{shard:03}.mkshard"))
}

fn manifest_path(dir: &Path, shard: u32) -> PathBuf {
    dir.join(format!("tiles-{shard:03}.mkrepack"))
}

fn encode_manifest(manifest: &ShardManifest) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(MANIFEST_MAGIC);
    out.extend_from_slice(&manifest.input_root_hash.to_le_bytes());
    out.extend_from_slice(&manifest.selection_hash.to_le_bytes());
    out.extend_from_slice(&manifest.input_record.to_le_bytes());
    out.extend_from_slice(&manifest.output_shard.to_le_bytes());
    out.extend_from_slice(&manifest.brotli_quality.to_le_bytes());
    for value in [
        manifest.file_len,
        manifest.tile_count,
        manifest.unique_blobs,
        manifest.start_tile_id,
        manifest.end_tile_id,
        manifest.dir_offset,
        manifest.dir_len,
        manifest.decoded_before,
        manifest.decoded_after,
        manifest.compressed_before,
        manifest.compressed_after,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in manifest.savings {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.push(manifest.min_zoom);
    out.push(manifest.max_zoom);
    out
}

fn decode_manifest(bytes: &[u8]) -> Result<ShardManifest, String> {
    let expected_len = 8 + 16 + 16 + 4 + 4 + 4 + 11 * 8 + DATA_POLICY.len() * 8 + 2;
    if bytes.len() != expected_len || bytes.get(..8) != Some(MANIFEST_MAGIC) {
        return Err("invalid repack shard manifest".to_string());
    }
    let mut pos = 8;
    let take_u128 = |pos: &mut usize| {
        let value = u128::from_le_bytes(bytes[*pos..*pos + 16].try_into().unwrap());
        *pos += 16;
        value
    };
    let take_u32 = |pos: &mut usize| {
        let value = u32::from_le_bytes(bytes[*pos..*pos + 4].try_into().unwrap());
        *pos += 4;
        value
    };
    let take_u64 = |pos: &mut usize| {
        let value = u64::from_le_bytes(bytes[*pos..*pos + 8].try_into().unwrap());
        *pos += 8;
        value
    };
    let input_root_hash = take_u128(&mut pos);
    let selection_hash = take_u128(&mut pos);
    let input_record = take_u32(&mut pos);
    let output_shard = take_u32(&mut pos);
    let brotli_quality = take_u32(&mut pos);
    let file_len = take_u64(&mut pos);
    let tile_count = take_u64(&mut pos);
    let unique_blobs = take_u64(&mut pos);
    let start_tile_id = take_u64(&mut pos);
    let end_tile_id = take_u64(&mut pos);
    let dir_offset = take_u64(&mut pos);
    let dir_len = take_u64(&mut pos);
    let decoded_before = take_u64(&mut pos);
    let decoded_after = take_u64(&mut pos);
    let compressed_before = take_u64(&mut pos);
    let compressed_after = take_u64(&mut pos);
    let mut savings = [0; DATA_POLICY.len()];
    for value in &mut savings {
        *value = take_u64(&mut pos);
    }
    let min_zoom = bytes[pos];
    let max_zoom = bytes[pos + 1];
    Ok(ShardManifest {
        input_root_hash,
        selection_hash,
        input_record,
        output_shard,
        brotli_quality,
        file_len,
        tile_count,
        unique_blobs,
        start_tile_id,
        end_tile_id,
        dir_offset,
        dir_len,
        decoded_before,
        decoded_after,
        compressed_before,
        compressed_after,
        savings,
        min_zoom,
        max_zoom,
    })
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let partial = path.with_extension("partial");
    match fs::remove_file(&partial) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("remove {}: {err}", partial.display())),
    }
    let mut file = File::create(&partial)
        .map_err(|err| format!("create {}: {err}", partial.display()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|err| format!("write {}: {err}", partial.display()))?;
    fs::rename(&partial, path)
        .map_err(|err| format!("rename {} to {}: {err}", partial.display(), path.display()))
}

fn selected_refs(
    reader: &mut MkmapReader,
    record: usize,
    selection: &TileSelection,
) -> Result<Vec<MkmapTileRef>, String> {
    let mut refs = Vec::new();
    reader
        .for_each_root_record_tile_ref(record, |tile| {
            if selection.contains(tile.tile_id) {
                refs.push(tile);
            }
        })
        .map_err(|err| format!("read input root record {record}: {err}"))?;
    Ok(refs)
}

fn selection_records(
    reader: &mut MkmapReader,
    selection: &TileSelection,
) -> Result<Vec<usize>, String> {
    if matches!(selection, TileSelection::All) {
        return Ok((0..reader.root_record_count()).collect());
    }
    let mut records = Vec::new();
    for record in 0..reader.root_record_count() {
        if !selected_refs(reader, record, selection)?.is_empty() {
            records.push(record);
        }
    }
    Ok(records)
}

fn load_completed_manifest(
    output: &Path,
    input_root_hash: u128,
    selection_hash: u128,
    input_record: u32,
    output_shard: u32,
    brotli_quality: u32,
) -> Result<Option<ShardManifest>, String> {
    let path = manifest_path(output, output_shard);
    let shard = shard_path(output, output_shard);
    if !path.exists() || !shard.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let Ok(manifest) = decode_manifest(&bytes) else {
        return Ok(None);
    };
    let actual_len = fs::metadata(&shard)
        .map_err(|err| format!("stat {}: {err}", shard.display()))?
        .len();
    if manifest.input_root_hash != input_root_hash
        || manifest.selection_hash != selection_hash
        || manifest.input_record != input_record
        || manifest.output_shard != output_shard
        || manifest.brotli_quality != brotli_quality
        || manifest.file_len != actual_len
        || manifest.dir_offset.checked_add(manifest.dir_len) != Some(actual_len)
    {
        return Ok(None);
    }
    Ok(Some(manifest))
}

struct ProgressLog {
    file: Option<BufWriter<File>>,
}

impl ProgressLog {
    fn open(path: Option<&Path>) -> Result<Self, String> {
        let file = path
            .map(|path| {
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map(BufWriter::new)
                    .map_err(|err| format!("open progress log {}: {err}", path.display()))
            })
            .transpose()?;
        Ok(Self { file })
    }

    fn line(&mut self, line: &str) -> Result<(), String> {
        println!("{line}");
        if let Some(file) = &mut self.file {
            writeln!(file, "{line}")
                .and_then(|_| file.flush())
                .map_err(|err| format!("write progress log: {err}"))?;
        }
        Ok(())
    }
}

fn display_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{:02}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
}

fn process_shard(
    reader: &mut MkmapReader,
    input_record: usize,
    output_shard: u32,
    refs: &[MkmapTileRef],
    codec: &TileCodec,
    dict: Option<&[u8]>,
    output: Option<&Path>,
    input_root_hash: u128,
    selection_hash: u128,
    jobs: usize,
    brotli_quality: u32,
) -> Result<ShardManifest, String> {
    if refs.is_empty() {
        return Err("cannot write an empty output shard".to_string());
    }
    let final_path = output.map(|dir| shard_path(dir, output_shard));
    let partial_path = final_path.as_ref().map(|path| path.with_extension("partial"));
    if let Some(path) = partial_path.as_ref() {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("remove {}: {err}", path.display())),
        }
    }
    let file = match partial_path.as_ref() {
        Some(path) => Some(
            File::create(path).map_err(|err| format!("create {}: {err}", path.display()))?,
        ),
        None => None,
    };
    let mut writer = file.map(|file| BufWriter::with_capacity(4 * 1024 * 1024, file));
    let mut offset = 0_u64;
    let mut entries = Vec::with_capacity(refs.len());
    let mut dedup: HashMap<u128, BlobRef> = HashMap::new();
    let mut tile_stats = TileRewriteStats::default();
    let mut compressed_before = 0_u64;
    let mut compressed_after = 0_u64;
    let mut min_zoom = u8::MAX;
    let mut max_zoom = 0_u8;

    struct TileOutput {
        tile: MkmapTileRef,
        source_len: u64,
        compressed: Vec<u8>,
        stats: TileRewriteStats,
    }

    let jobs = jobs.max(1);
    let window = jobs.saturating_mul(2).max(1);
    let pipeline_result = std::thread::scope(|scope| -> Result<(), String> {
        let (job_tx, job_rx) = sync_channel::<(usize, MkmapTileRef, Vec<u8>)>(jobs);
        let job_rx = Arc::new(Mutex::new(job_rx));
        let (result_tx, result_rx) =
            sync_channel::<(usize, Result<TileOutput, String>)>(jobs);
        let (permit_tx, permit_rx) = sync_channel::<()>(window);
        for _ in 0..window {
            permit_tx.send(()).unwrap();
        }

        let producer = scope.spawn(move || -> Result<(), String> {
            for (seq, tile) in refs.iter().copied().enumerate() {
                permit_rx
                    .recv()
                    .map_err(|_| "tile pipeline stopped before input was read".to_string())?;
                let source = reader.read_tile_ref(&tile).map_err(|err| {
                    format!("read z{}/{}/{}: {err}", tile.zoom, tile.x, tile.y)
                })?;
                job_tx
                    .send((seq, tile, source))
                    .map_err(|_| "tile pipeline stopped before input was queued".to_string())?;
            }
            Ok(())
        });

        let mut workers = Vec::with_capacity(jobs);
        for _ in 0..jobs {
            let job_rx = Arc::clone(&job_rx);
            let result_tx = result_tx.clone();
            workers.push(scope.spawn(move || -> Result<(), String> {
                loop {
                    let job = job_rx
                        .lock()
                        .map_err(|_| "tile job queue mutex poisoned".to_string())?
                        .recv();
                    let Ok((seq, tile, source)) = job else {
                        return Ok(());
                    };
                    let result = (|| {
                        let decoded = codec.decode(&source).map_err(|err| {
                            format!("decode z{}/{}/{}: {err}", tile.zoom, tile.x, tile.y)
                        })?;
                        let (rewritten, stats) = rewrite_tile(&decoded).map_err(|err| {
                            format!("rewrite z{}/{}/{}: {err}", tile.zoom, tile.x, tile.y)
                        })?;
                        let compressed = compress_tile(
                            &TileCompression::Brotli {
                                quality: brotli_quality,
                            },
                            dict,
                            &rewritten,
                        )
                        .map_err(|err| {
                            format!("compress z{}/{}/{}: {err}", tile.zoom, tile.x, tile.y)
                        })?;
                        Ok(TileOutput {
                            tile,
                            source_len: source.len() as u64,
                            compressed,
                            stats,
                        })
                    })();
                    result_tx
                        .send((seq, result))
                        .map_err(|_| "tile result writer stopped".to_string())?;
                }
            }));
        }
        drop(result_tx);

        let mut next = 0_usize;
        let mut pending = BTreeMap::new();
        let mut first_error = None;
        for (seq, result) in result_rx {
            pending.insert(seq, result);
            while let Some(result) = pending.remove(&next) {
                match result {
                    Ok(result) if first_error.is_none() => {
                        let hash = content_hash(&result.compressed);
                        let blob = if let Some(blob) = dedup.get(&hash) {
                            *blob
                        } else {
                            let blob = BlobRef {
                                shard: output_shard,
                                offset,
                                len: result.compressed.len() as u64,
                            };
                            if let Some(writer) = writer.as_mut() {
                                writer.write_all(&result.compressed).map_err(|err| {
                                    format!("write output shard {output_shard}: {err}")
                                })?;
                            }
                            offset += result.compressed.len() as u64;
                            dedup.insert(hash, blob);
                            blob
                        };
                        entries.push(LeafEntry {
                            tile_id: result.tile.tile_id,
                            blob,
                        });
                        tile_stats.add_assign(&result.stats);
                        compressed_before += result.source_len;
                        compressed_after += result.compressed.len() as u64;
                        min_zoom = min_zoom.min(result.tile.zoom);
                        max_zoom = max_zoom.max(result.tile.zoom);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
                next += 1;
                let _ = permit_tx.send(());
            }
        }

        let producer_result = producer
            .join()
            .map_err(|_| "tile input thread panicked".to_string())?;
        for worker in workers {
            worker
                .join()
                .map_err(|_| "tile worker thread panicked".to_string())??;
        }
        producer_result?;
        if next != refs.len() {
            return Err(format!(
                "tile pipeline returned {next} of {} results",
                refs.len()
            ));
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    });
    pipeline_result?;
    let directory = encode_leaf_directory(&entries)?;
    let dir_offset = offset;
    let dir_len = directory.len() as u64;
    let file_len = dir_offset + dir_len;
    if file_len >= SHARD_HARD_CAP {
        return Err(format!(
            "output shard {output_shard} is {file_len} bytes, cap {SHARD_HARD_CAP}"
        ));
    }
    if let Some(mut writer) = writer {
        writer
            .write_all(&directory)
            .and_then(|_| writer.flush())
            .map_err(|err| format!("finish output shard {output_shard}: {err}"))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|err| format!("sync output shard {output_shard}: {err}"))?;
        drop(writer);
        let partial = partial_path.as_ref().unwrap();
        let final_path = final_path.as_ref().unwrap();
        fs::rename(partial, final_path).map_err(|err| {
            format!("rename {} to {}: {err}", partial.display(), final_path.display())
        })?;
    }
    let manifest = ShardManifest {
        input_root_hash,
        selection_hash,
        input_record: input_record as u32,
        output_shard,
        brotli_quality,
        file_len,
        tile_count: entries.len() as u64,
        unique_blobs: dedup.len() as u64,
        start_tile_id: entries.first().unwrap().tile_id,
        end_tile_id: entries.last().unwrap().tile_id,
        dir_offset,
        dir_len,
        decoded_before: tile_stats.decoded_before,
        decoded_after: tile_stats.decoded_after,
        compressed_before,
        compressed_after,
        savings: tile_stats.savings,
        min_zoom,
        max_zoom,
    };
    if let Some(output) = output {
        write_atomic(&manifest_path(output, output_shard), &encode_manifest(&manifest))?;
    }
    Ok(manifest)
}

fn verify_archives(
    input: &Path,
    output: &Path,
    selection: &TileSelection,
    shard_range: Option<ShardRange>,
) -> Result<u64, String> {
    let mut source = MkmapReader::open(input)
        .map_err(|err| format!("open input {}: {err}", input.display()))?;
    let mut repacked = MkmapReader::open(output)
        .map_err(|err| format!("open output {}: {err}", output.display()))?;
    let source_codec = TileCodec::from_metadata(
        &source
            .get_metadata()
            .map_err(|err| format!("input metadata: {err}"))?,
    )
    .map_err(|err| format!("input codec: {err}"))?;
    let output_codec = TileCodec::from_metadata(
        &repacked
            .get_metadata()
            .map_err(|err| format!("output metadata: {err}"))?,
    )
    .map_err(|err| format!("output codec: {err}"))?;
    let records = selection_records(&mut source, selection)?;
    if repacked.root_record_count() != records.len() {
        return Err(format!(
            "output root record count {} differs from selected input count {}",
            repacked.root_record_count(),
            records.len()
        ));
    }
    if let Some(range) = shard_range {
        if range.start >= range.end || range.end > records.len() {
            return Err(format!(
                "--shards range {}..{} is outside 0..{}",
                range.start,
                range.end,
                records.len()
            ));
        }
    }
    let mut verified_count = 0_u64;
    for (output_record, input_record) in records.into_iter().enumerate() {
        if shard_range.is_some_and(|range| !range.contains(output_record)) {
            continue;
        }
        let input_refs = selected_refs(&mut source, input_record, selection)?;
        let output_refs = selected_refs(&mut repacked, output_record, &TileSelection::All)?;
        if input_refs.len() != output_refs.len() {
            return Err(format!(
                "shard {output_record} tile count differs: input {} output {}",
                input_refs.len(),
                output_refs.len()
            ));
        }
        for (tile, output_ref) in input_refs.iter().zip(&output_refs) {
            if tile.tile_id != output_ref.tile_id {
                return Err(format!(
                    "shard {output_record} tile order differs: input {} output {}",
                    tile.tile_id, output_ref.tile_id
                ));
            }
            let before_blob = source
                .read_tile_ref(tile)
                .map_err(|err| format!("read input tile {}: {err}", tile.tile_id))?;
            let after_blob = repacked
                .read_tile_ref(&output_ref)
                .map_err(|err| format!("read output tile {}: {err}", tile.tile_id))?;
            let before = source_codec
                .decode(&before_blob)
                .map_err(|err| format!("decode input tile {}: {err}", tile.tile_id))?;
            let after = output_codec
                .decode(&after_blob)
                .map_err(|err| format!("decode output tile {}: {err}", tile.tile_id))?;
            let expected = rewrite_tile(&before)
                .map_err(|err| format!("verify rewrite tile {}: {err}", tile.tile_id))?
                .0;
            if expected != after {
                return Err(format!(
                    "verify tile {}: repacked tile differs from the deterministic policy rewrite",
                    tile.tile_id
                ));
            }
            verify_kept_sections(&before, &after)
                .map_err(|err| format!("verify tile {}: {err}", tile.tile_id))?;
            verified_count += 1;
        }
    }
    Ok(verified_count)
}

fn write_root_index_atomic(output: &Path, index: &RootIndex<'_>) -> Result<u64, String> {
    let final_path = output.join("root.mkidx");
    let partial_path = output.join("root.partial");
    match fs::remove_file(&partial_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("remove {}: {err}", partial_path.display())),
    }
    let len = write_root_index(&partial_path, index)?;
    File::open(&partial_path)
        .and_then(|file| file.sync_all())
        .map_err(|err| format!("sync {}: {err}", partial_path.display()))?;
    fs::rename(&partial_path, &final_path).map_err(|err| {
        format!(
            "rename {} to {}: {err}",
            partial_path.display(),
            final_path.display()
        )
    })?;
    Ok(len)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepackStatus {
    pub completed_shards: usize,
    pub total_shards: usize,
    pub compressed_before: u64,
    pub compressed_after: u64,
}

pub fn repack_status(options: &RepackOptions) -> Result<RepackStatus, String> {
    if options.brotli_quality > 11 {
        return Err("--brotli-quality must be in 0..=11".to_string());
    }
    let root_path = options.input.join("root.mkidx");
    let root_bytes = fs::read(&root_path)
        .map_err(|err| format!("read {}: {err}", root_path.display()))?;
    let input_root_hash = content_hash(&root_bytes);
    let selection_hash = options.selection.fingerprint();
    let mut reader = MkmapReader::open(&options.input)
        .map_err(|err| format!("open {}: {err}", options.input.display()))?;
    let records = selection_records(&mut reader, &options.selection)?;
    let mut status = RepackStatus {
        total_shards: records.len(),
        ..Default::default()
    };
    if !options.output.exists() {
        return Ok(status);
    }
    for (output_shard, input_record) in records.into_iter().enumerate() {
        if let Some(manifest) = load_completed_manifest(
            &options.output,
            input_root_hash,
            selection_hash,
            input_record as u32,
            output_shard as u32,
            options.brotli_quality,
        )? {
            status.completed_shards += 1;
            status.compressed_before += manifest.compressed_before;
            status.compressed_after += manifest.file_len;
        }
    }
    Ok(status)
}

pub fn repack_archive(options: &RepackOptions) -> Result<RepackReport, String> {
    if options.jobs == 0 {
        return Err("--jobs must be at least 1".to_string());
    }
    if options.brotli_quality > 11 {
        return Err("--brotli-quality must be in 0..=11".to_string());
    }
    if options.verify_shards.is_some() && !options.verify {
        return Err("--shards requires --verify".to_string());
    }
    if options.dry_run && options.verify {
        return Err("--verify requires an output archive and cannot be combined with --dry-run"
            .to_string());
    }
    let same_path = options.input == options.output
        || (options.output.exists()
            && fs::canonicalize(&options.input).ok() == fs::canonicalize(&options.output).ok());
    if same_path {
        return Err("input and output archive directories must differ".to_string());
    }
    let root_path = options.input.join("root.mkidx");
    let root_bytes = fs::read(&root_path)
        .map_err(|err| format!("read {}: {err}", root_path.display()))?;
    let input_root_hash = content_hash(&root_bytes);
    let selection_hash = options.selection.fingerprint();
    let mut reader = MkmapReader::open(&options.input)
        .map_err(|err| format!("open {}: {err}", options.input.display()))?;
    let metadata = reader
        .get_metadata()
        .map_err(|err| format!("read input metadata: {err}"))?;
    let codec = TileCodec::from_metadata(&metadata)
        .map_err(|err| format!("read input codec: {err}"))?;
    if !codec.is_brotli() {
        return Err(format!(
            "input codec '{}' is unsupported; repack accepts br and br:dict-v1",
            codec.metadata_value()
        ));
    }
    let dict = codec.dict().map(<[u8]>::to_vec);
    if reader.shared_dict().unwrap_or(&[]) != dict.as_deref().unwrap_or(&[]) {
        return Err("root dictionary section differs from codec metadata".to_string());
    }
    let records = selection_records(&mut reader, &options.selection)?;
    if records.is_empty() {
        return Err("--tiles selected no input tiles".to_string());
    }
    if !options.dry_run {
        if options.output.exists() && !options.resume {
            return Err(format!(
                "{} already exists; use --resume or choose a new output",
                options.output.display()
            ));
        }
        fs::create_dir_all(&options.output)
            .map_err(|err| format!("create {}: {err}", options.output.display()))?;
    }
    if let Some(range) = options.verify_shards {
        if range.start >= range.end || range.end > records.len() {
            return Err(format!(
                "--shards range {}..{} is outside 0..{}",
                range.start,
                range.end,
                records.len()
            ));
        }
    }
    let mut progress = ProgressLog::open(options.log.as_deref())?;
    let total_shards = records.len();
    let run_start = Instant::now();
    let mut run_shards = 0_u32;
    let mut run_tiles = 0_u64;
    let mut run_output_bytes = 0_u64;
    let mut manifests = Vec::with_capacity(records.len());
    let mut report = RepackReport::default();
    for (output_index, input_record) in records.into_iter().enumerate() {
        let output_shard = output_index as u32;
        if options.resume && !options.dry_run {
            if let Some(manifest) = load_completed_manifest(
                &options.output,
                input_root_hash,
                selection_hash,
                input_record as u32,
                output_shard,
                options.brotli_quality,
            )? {
                progress.line(&format!(
                    "{}/{total_shards} shard {output_shard:03} {} bytes → {} bytes, {} tiles, resumed, running 0.00 MB/s, ETA --:--:--",
                    output_index + 1,
                    manifest.compressed_before,
                    manifest.file_len,
                    manifest.tile_count,
                ))?;
                report.add_manifest(&manifest);
                manifests.push(manifest);
                continue;
            }
        }
        let refs = selected_refs(&mut reader, input_record, &options.selection)?;
        let shard_start = Instant::now();
        let manifest = process_shard(
            &mut reader,
            input_record,
            output_shard,
            &refs,
            &codec,
            dict.as_deref(),
            (!options.dry_run).then_some(options.output.as_path()),
            input_root_hash,
            selection_hash,
            options.jobs,
            options.brotli_quality,
        )?;
        let shard_elapsed = shard_start.elapsed();
        run_shards += 1;
        run_tiles += manifest.tile_count;
        run_output_bytes += manifest.file_len;
        report.add_manifest(&manifest);
        let remaining = total_shards - output_index - 1;
        let eta = if run_shards == 0 {
            None
        } else {
            Some(run_start.elapsed().mul_f64(remaining as f64 / f64::from(run_shards)))
        };
        let mib_per_second = run_output_bytes as f64
            / 1_048_576.0
            / run_start.elapsed().as_secs_f64().max(f64::EPSILON);
        progress.line(&format!(
            "{}/{total_shards} shard {output_shard:03} {} bytes → {} bytes, {} tiles, {}, running {:.2} MB/s, ETA {}",
            output_index + 1,
            manifest.compressed_before,
            manifest.file_len,
            manifest.tile_count,
            display_duration(shard_elapsed),
            mib_per_second,
            eta.map(display_duration).unwrap_or_else(|| "--:--:--".to_string()),
        ))?;
        manifests.push(manifest);
    }
    if !options.dry_run {
        let roots: Vec<_> = manifests
            .iter()
            .map(|manifest| RootRecord {
                start_tile_id: manifest.start_tile_id,
                end_tile_id: manifest.end_tile_id,
                shard: manifest.output_shard,
                dir_offset: manifest.dir_offset,
                dir_len: manifest.dir_len,
            })
            .collect();
        let min_zoom = manifests.iter().map(|manifest| manifest.min_zoom).min().unwrap();
        let max_zoom = manifests.iter().map(|manifest| manifest.max_zoom).max().unwrap();
        write_root_index_atomic(
            &options.output,
            &RootIndex {
                metadata: &metadata,
                dict: dict.as_deref(),
                shard_cap: SHARD_HARD_CAP,
                tile_count: report.tiles,
                unique_blobs: report.unique_blobs,
                min_zoom,
                max_zoom,
                records: &roots,
            },
        )?;
        if options.verify {
            let verified = verify_archives(
                &options.input,
                &options.output,
                &options.selection,
                options.verify_shards,
            )?;
            progress.line(&format!(
                "verify: OK — {verified} tiles decoded once per archive and policy-checked"
            ))?;
        }
    }
    progress.line(&format!(
        "total: {} shards, {} tiles, {} bytes in → {} bytes out, elapsed {}, running {:.2} tiles/s, {:.2} MB/s; {}",
        report.shards,
        report.tiles,
        report.compressed_before,
        manifests.iter().map(|manifest| manifest.file_len).sum::<u64>(),
        display_duration(run_start.elapsed()),
        run_tiles as f64 / run_start.elapsed().as_secs_f64().max(f64::EPSILON),
        run_output_bytes as f64 / 1_048_576.0 / run_start.elapsed().as_secs_f64().max(f64::EPSILON),
        if options.dry_run { "root.mkidx not written (dry run)" } else { "root.mkidx written at end" },
    ))?;
    println!("action | data | decoded bytes saved | reader / reason");
    for (row, saved) in DATA_POLICY.iter().zip(report.savings) {
        println!(
            "{} | {} | {} | {}",
            match row.action {
                PolicyAction::Keep => "KEEP",
                PolicyAction::Drop => "DROP",
            },
            row.item,
            saved,
            row.reader_or_reason
        );
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../seed-files/amsterdam-tiles")
    }

    #[test]
    fn detail_tables_are_compacted_without_touching_geometry() {
        let mut feature = Vec::new();
        let mut tags = Vec::new();
        for value in [0_u64, 0, 1, 1, 2, 2] {
            write_varint(value, &mut tags);
        }
        write_len_field(2, &tags, &mut feature);
        write_len_field(4, &[9, 2, 2, 0], &mut feature);
        let mut layer = Vec::new();
        write_len_field(1, b"osm_points", &mut layer);
        write_len_field(2, &feature, &mut layer);
        for key in ["name", "addr:housenumber", "__makepad_osm_id"] {
            write_len_field(3, key.as_bytes(), &mut layer);
        }
        for value in [b"kept".as_slice(), b"gone", b"42"] {
            let mut message = Vec::new();
            write_len_field(1, value, &mut message);
            write_len_field(4, &message, &mut layer);
        }
        let mut tile = Vec::new();
        write_len_field(3, &layer, &mut tile);
        let (rewritten, stats) = rewrite_tile(&tile).unwrap();
        verify_rewritten_tile(&tile, &rewritten).unwrap();
        let sections = tile_sections(&rewritten).unwrap();
        let retained_keys: Vec<_> = sections
            .detail_tags
            .iter()
            .flat_map(|(_, features)| features.iter())
            .flat_map(|tags| tags.iter().map(|(key, _)| key.as_str()))
            .collect();
        assert_eq!(retained_keys, ["name"]);
        assert!(stats.savings[POLICY_TAGS] > 0);
        assert!(stats.savings[POLICY_SYNTHETIC] > 0);
    }

    fn test_write_shapes(points: &[(i64, i64)], out: &mut Vec<u8>, checksum: &mut u64) {
        write_varint(usize::from(!points.is_empty()) as u64, out);
        if points.is_empty() {
            return;
        }
        write_varint(1, out);
        write_varint(points.len() as u64, out);
        let (mut previous_x, mut previous_y) = (0_i64, 0_i64);
        for &(x, y) in points {
            let delta_x = x - previous_x;
            let delta_y = y - previous_y;
            write_varint(((delta_x << 1) ^ (delta_x >> 63)) as u64, out);
            write_varint(((delta_y << 1) ^ (delta_y >> 63)) as u64, out);
            fnv_step(checksum, &x.to_le_bytes());
            fnv_step(checksum, &y.to_le_bytes());
            previous_x = x;
            previous_y = y;
        }
    }

    fn test_faces_tile(version: u8) -> Vec<u8> {
        let mut body = Vec::new();
        let mut checksum = 0xcbf2_9ce4_8422_2325;
        write_varint(1, &mut body);
        write_varint(7, &mut body);
        test_write_shapes(&[(2, 3), (5, 7)], &mut body, &mut checksum);
        test_write_shapes(&[], &mut body, &mut checksum);
        test_write_shapes(&[], &mut body, &mut checksum);
        body.extend_from_slice(&123_u64.to_le_bytes());
        test_write_shapes(&[(9, 11)], &mut body, &mut checksum);
        test_write_shapes(&[(1, 2)], &mut body, &mut checksum);
        if version == 4 {
            body.extend_from_slice(&456_u64.to_le_bytes());
            write_varint(1, &mut body);
            write_varint(32, &mut body);
            write_varint(0xff00ff, &mut body);
            test_write_shapes(&[(13, 17), (19, 23)], &mut body, &mut checksum);
        }
        let mut blob = vec![version];
        write_varint(1, &mut blob);
        write_varint(14, &mut blob);
        blob.extend_from_slice(&99_u64.to_le_bytes());
        blob.extend_from_slice(&checksum.to_le_bytes());
        write_varint(body.len() as u64, &mut blob);
        blob.extend_from_slice(&body);
        let mut tile = Vec::new();
        write_len_field(101, &blob, &mut tile);
        tile
    }

    #[test]
    fn v3_and_v4_faces_keep_regions_and_buildings_but_stub_shadows() {
        for version in [3, 4] {
            let input = test_faces_tile(version);
            let output = rewrite_tile(&input).unwrap().0;
            verify_rewritten_tile(&input, &output).unwrap();
            let before = tile_sections(&input).unwrap();
            let after = tile_sections(&output).unwrap();
            assert_eq!(before.faces[0].1, after.faces[0].1);
            assert_eq!(before.faces[0].2, after.faces[0].2);
            assert!(after.faces[0].3);
            assert!(output.len() < input.len());
        }
    }

    #[test]
    fn dictionary_archive_roundtrip_preserves_codec() {
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../target/map-repack-dict-test-{}", std::process::id()));
        if scratch.exists() {
            fs::remove_dir_all(&scratch).unwrap();
        }
        let input_dir = scratch.join("input.mkmap");
        let output_dir = scratch.join("output.mkmap");
        fs::create_dir_all(&input_dir).unwrap();
        let mut layer = Vec::new();
        write_len_field(1, b"water", &mut layer);
        let mut tile = Vec::new();
        write_len_field(3, &layer, &mut tile);
        let dict = b"water layer polygons streets";
        let compression = TileCompression::Brotli { quality: 5 };
        let compressed = compress_tile(&compression, Some(dict), &tile).unwrap();
        let entry = LeafEntry {
            tile_id: makepad_mbtile_reader::mkmap_tile_id(0, 0, 0),
            blob: BlobRef {
                shard: 0,
                offset: 0,
                len: compressed.len() as u64,
            },
        };
        let directory = encode_leaf_directory(&[entry]).unwrap();
        let mut shard = compressed;
        let dir_offset = shard.len() as u64;
        shard.extend_from_slice(&directory);
        fs::write(input_dir.join("tiles-000.mkshard"), shard).unwrap();
        let metadata = makepad_mbtile_reader::compression_metadata_rows(
            &compression,
            Some(dict),
        )
        .into_iter()
        .collect();
        write_root_index(
            &input_dir.join("root.mkidx"),
            &RootIndex {
                metadata: &metadata,
                dict: Some(dict),
                shard_cap: SHARD_HARD_CAP,
                tile_count: 1,
                unique_blobs: 1,
                min_zoom: 0,
                max_zoom: 0,
                records: &[RootRecord {
                    start_tile_id: entry.tile_id,
                    end_tile_id: entry.tile_id,
                    shard: 0,
                    dir_offset,
                    dir_len: directory.len() as u64,
                }],
            },
        )
        .unwrap();
        repack_archive(&RepackOptions {
            input: input_dir,
            output: output_dir.clone(),
            selection: TileSelection::All,
            dry_run: false,
            verify: true,
            resume: false,
            jobs: 2,
            brotli_quality: TILE_BROTLI_QUALITY,
            verify_shards: None,
            log: None,
        })
        .unwrap();
        let mut reader = MkmapReader::open(&output_dir).unwrap();
        assert_eq!(reader.shared_dict(), Some(dict.as_slice()));
        let tile_ref = reader.resolve_tile(0, 0, 0).unwrap().unwrap();
        let stored = reader.read_tile_ref(&tile_ref).unwrap();
        let codec = TileCodec::from_metadata(&reader.get_metadata().unwrap()).unwrap();
        assert_eq!(codec.metadata_value(), "br:dict-v1");
        assert_eq!(codec.decode(&stored).unwrap(), tile);
        fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn two_shard_partial_resume_finishes_a_readable_archive() {
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/map-repack-resume-test-{}",
            std::process::id()
        ));
        if scratch.exists() {
            fs::remove_dir_all(&scratch).unwrap();
        }
        let input_dir = scratch.join("input.mkmap");
        let output_dir = scratch.join("output.mkmap");
        fs::create_dir_all(&input_dir).unwrap();
        let mut tiles = Vec::new();
        for name in [b"water".as_slice(), b"landuse"] {
            let mut layer = Vec::new();
            write_len_field(1, name, &mut layer);
            let mut tile = Vec::new();
            write_len_field(3, &layer, &mut tile);
            tiles.push(tile);
        }
        let compression = TileCompression::Brotli { quality: 1 };
        let mut records = Vec::new();
        for (shard_index, tile) in tiles.iter().enumerate() {
            let tile_id = shard_index as u64;
            let compressed = compress_tile(&compression, None, tile).unwrap();
            let entry = LeafEntry {
                tile_id,
                blob: BlobRef {
                    shard: shard_index as u32,
                    offset: 0,
                    len: compressed.len() as u64,
                },
            };
            let directory = encode_leaf_directory(&[entry]).unwrap();
            let dir_offset = compressed.len() as u64;
            let mut shard = compressed;
            shard.extend_from_slice(&directory);
            fs::write(
                input_dir.join(format!("tiles-{shard_index:03}.mkshard")),
                shard,
            )
            .unwrap();
            records.push(RootRecord {
                start_tile_id: tile_id,
                end_tile_id: tile_id,
                shard: shard_index as u32,
                dir_offset,
                dir_len: directory.len() as u64,
            });
        }
        let metadata = [("compression".to_string(), "br".to_string())]
            .into_iter()
            .collect();
        write_root_index(
            &input_dir.join("root.mkidx"),
            &RootIndex {
                metadata: &metadata,
                dict: None,
                shard_cap: SHARD_HARD_CAP,
                tile_count: 2,
                unique_blobs: 2,
                min_zoom: 0,
                max_zoom: 1,
                records: &records,
            },
        )
        .unwrap();
        let options = RepackOptions {
            input: input_dir,
            output: output_dir.clone(),
            selection: TileSelection::All,
            dry_run: false,
            verify: true,
            resume: false,
            jobs: 2,
            brotli_quality: 1,
            verify_shards: None,
            log: Some(output_dir.join("repack.log")),
        };
        repack_archive(&options).unwrap();
        assert_eq!(repack_status(&options).unwrap().completed_shards, 2);
        let first_shard = fs::read(output_dir.join("tiles-000.mkshard")).unwrap();

        fs::remove_file(output_dir.join("tiles-001.mkrepack")).unwrap();
        fs::remove_file(output_dir.join("root.mkidx")).unwrap();
        fs::write(output_dir.join("tiles-001.partial"), b"interrupted").unwrap();
        let mut resumed = options.clone();
        resumed.resume = true;
        assert_eq!(repack_status(&resumed).unwrap().completed_shards, 1);
        let mut changed_quality = resumed.clone();
        changed_quality.brotli_quality = 2;
        assert_eq!(
            repack_status(&changed_quality).unwrap().completed_shards,
            0
        );
        repack_archive(&resumed).unwrap();
        let status = repack_status(&resumed).unwrap();
        assert_eq!((status.completed_shards, status.total_shards), (2, 2));
        assert_eq!(
            verify_archives(
                &resumed.input,
                &resumed.output,
                &resumed.selection,
                Some(ShardRange { start: 1, end: 2 }),
            )
            .unwrap(),
            1
        );
        assert_eq!(
            fs::read(output_dir.join("tiles-000.mkshard")).unwrap(),
            first_shard
        );
        let mut reader = MkmapReader::open(&output_dir).unwrap();
        let codec = TileCodec::from_metadata(&reader.get_metadata().unwrap()).unwrap();
        let mut decoded = Vec::new();
        let mut refs = Vec::new();
        reader.for_each_tile_ref(|tile| refs.push(tile)).unwrap();
        for tile in refs {
            decoded.push(codec.decode(&reader.read_tile_ref(&tile).unwrap()).unwrap());
        }
        assert_eq!(decoded, tiles);
        let log = fs::read_to_string(output_dir.join("repack.log")).unwrap();
        assert!(log.contains("1/2 shard 000"));
        assert!(log.contains("2/2 shard 001"));
        assert!(log.contains("root.mkidx written at end"));
        fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    #[ignore = "acceptance fixture: deterministic Brotli q11 over 25 large tiles"]
    fn amsterdam_tiles_rewrite_and_verify() {
        let fixture_dir = fixture_dir();
        let mut paths: Vec<_> = fs::read_dir(fixture_dir)
            .expect("Amsterdam fixture directory")
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "decoded"))
            .collect();
        paths.sort();
        assert_eq!(paths.len(), 25);
        let compression = TileCompression::Brotli {
            quality: TILE_BROTLI_QUALITY,
        };
        let mut before = 0_u64;
        let mut after = 0_u64;
        let mut compressed = 0_u64;
        let mut shadows = 0_u64;
        let mut tags = 0_u64;
        for path in paths {
            let input = fs::read(&path).unwrap();
            let (output, stats) = rewrite_tile(&input)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            verify_rewritten_tile(&input, &output)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            before += input.len() as u64;
            after += output.len() as u64;
            compressed += compress_tile(&compression, None, &output).unwrap().len() as u64;
            shadows += stats.savings[POLICY_SHADOWS];
            tags += stats.savings[POLICY_TAGS] + stats.savings[POLICY_SYNTHETIC];
        }
        assert!(after < before);
        assert!(shadows > 0);
        assert!(tags > 0);
        println!(
            "Amsterdam 25: decoded {before} -> {after} bytes; q{} brotli {} bytes ({:.2} MiB); shadows saved {shadows}; tags saved {tags}",
            TILE_BROTLI_QUALITY,
            compressed,
            compressed as f64 / 1_048_576.0
        );
    }

    fn write_repeated_amsterdam_archive(output: &Path, tile_count: usize) {
        fs::create_dir_all(output).unwrap();
        let mut paths: Vec<_> = fs::read_dir(fixture_dir())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "decoded"))
            .collect();
        paths.sort();
        let compression = TileCompression::Brotli { quality: 1 };
        let mut shard = Vec::new();
        let mut blobs = Vec::new();
        for path in paths {
            let decoded = fs::read(path).unwrap();
            let compressed = compress_tile(&compression, None, &decoded).unwrap();
            let blob = BlobRef {
                shard: 0,
                offset: shard.len() as u64,
                len: compressed.len() as u64,
            };
            shard.extend_from_slice(&compressed);
            blobs.push(blob);
        }
        let first_id = makepad_mbtile_reader::mkmap_tile_id(14, 0, 0);
        let entries: Vec<_> = (0..tile_count)
            .map(|index| LeafEntry {
                tile_id: first_id + index as u64,
                blob: blobs[index % blobs.len()],
            })
            .collect();
        let directory = encode_leaf_directory(&entries).unwrap();
        let dir_offset = shard.len() as u64;
        shard.extend_from_slice(&directory);
        fs::write(output.join("tiles-000.mkshard"), shard).unwrap();
        let metadata = [("compression".to_string(), "br".to_string())]
            .into_iter()
            .collect();
        write_root_index(
            &output.join("root.mkidx"),
            &RootIndex {
                metadata: &metadata,
                dict: None,
                shard_cap: SHARD_HARD_CAP,
                tile_count: tile_count as u64,
                unique_blobs: blobs.len() as u64,
                min_zoom: 14,
                max_zoom: 14,
                records: &[RootRecord {
                    start_tile_id: entries.first().unwrap().tile_id,
                    end_tile_id: entries.last().unwrap().tile_id,
                    shard: 0,
                    dir_offset,
                    dir_len: directory.len() as u64,
                }],
            },
        )
        .unwrap();
    }

    #[test]
    #[ignore = "acceptance benchmark: six q11 archive repacks"]
    fn benchmark_parallel_amsterdam_repack() {
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/map-repack-benchmark-{}",
            std::process::id()
        ));
        if scratch.exists() {
            fs::remove_dir_all(&scratch).unwrap();
        }
        fs::create_dir_all(&scratch).unwrap();
        for tile_count in [25_usize, 500] {
            let input = scratch.join(format!("input-{tile_count}.mkmap"));
            write_repeated_amsterdam_archive(&input, tile_count);
            for jobs in [1_usize, 4, 16] {
                let output = scratch.join(format!("output-{tile_count}-{jobs}.mkmap"));
                let start = Instant::now();
                let report = repack_archive(&RepackOptions {
                    input: input.clone(),
                    output,
                    selection: TileSelection::All,
                    dry_run: false,
                    verify: false,
                    resume: false,
                    jobs,
                    brotli_quality: TILE_BROTLI_QUALITY,
                    verify_shards: None,
                    log: None,
                })
                .unwrap();
                let elapsed = start.elapsed().as_secs_f64();
                println!(
                    "BENCH tiles={tile_count} jobs={jobs} elapsed={elapsed:.3}s tiles/s={:.3} output-MiB/s={:.3}",
                    tile_count as f64 / elapsed,
                    report.compressed_after as f64 / 1_048_576.0 / elapsed,
                );
            }
        }
        fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn manifest_roundtrip_is_fixed_and_deterministic() {
        let manifest = ShardManifest {
            input_root_hash: 1,
            selection_hash: 2,
            input_record: 3,
            output_shard: 4,
            brotli_quality: 11,
            file_len: 5,
            tile_count: 6,
            unique_blobs: 7,
            start_tile_id: 8,
            end_tile_id: 9,
            dir_offset: 10,
            dir_len: 11,
            decoded_before: 12,
            decoded_after: 13,
            compressed_before: 14,
            compressed_after: 15,
            savings: [16; DATA_POLICY.len()],
            min_zoom: 14,
            max_zoom: 18,
        };
        let bytes = encode_manifest(&manifest);
        let decoded = decode_manifest(&bytes).unwrap();
        assert_eq!(encode_manifest(&decoded), bytes);
    }

    #[test]
    #[ignore = "acceptance fixture: builds and verifies a 25-tile mkmap twice"]
    fn amsterdam_archive_roundtrip() {
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../target/map-repack-test-{}", std::process::id()));
        if scratch.exists() {
            fs::remove_dir_all(&scratch).unwrap();
        }
        let input_dir = scratch.join("input.mkmap");
        let output_dir = scratch.join("output.mkmap");
        fs::create_dir_all(&input_dir).unwrap();
        let fixture_dir = fixture_dir();
        let mut paths: Vec<_> = fs::read_dir(fixture_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "decoded"))
            .collect();
        paths.sort();
        let compression = TileCompression::Brotli { quality: 5 };
        let mut tiles = Vec::new();
        for path in paths {
            let stem = path.file_stem().unwrap().to_string_lossy();
            let mut parts = stem.split('-');
            let zoom = parts.next().unwrap()[1..].parse::<u8>().unwrap();
            let x = parts.next().unwrap()[1..].parse::<u32>().unwrap();
            let y = parts.next().unwrap()[1..].parse::<u32>().unwrap();
            let id = makepad_mbtile_reader::mkmap_tile_id(zoom, x, y);
            let decoded = fs::read(path).unwrap();
            let compressed = compress_tile(&compression, None, &decoded).unwrap();
            tiles.push((id, compressed));
        }
        tiles.sort_by_key(|tile| tile.0);
        let mut shard = Vec::new();
        let mut entries = Vec::new();
        for (tile_id, compressed) in tiles {
            let offset = shard.len() as u64;
            shard.extend_from_slice(&compressed);
            entries.push(LeafEntry {
                tile_id,
                blob: BlobRef {
                    shard: 0,
                    offset,
                    len: compressed.len() as u64,
                },
            });
        }
        let directory = encode_leaf_directory(&entries).unwrap();
        let dir_offset = shard.len() as u64;
        shard.extend_from_slice(&directory);
        fs::write(input_dir.join("tiles-000.mkshard"), shard).unwrap();
        let metadata = [("compression".to_string(), "br".to_string())]
            .into_iter()
            .collect();
        write_root_index(
            &input_dir.join("root.mkidx"),
            &RootIndex {
                metadata: &metadata,
                dict: None,
                shard_cap: SHARD_HARD_CAP,
                tile_count: entries.len() as u64,
                unique_blobs: entries.len() as u64,
                min_zoom: 14,
                max_zoom: 14,
                records: &[RootRecord {
                    start_tile_id: entries.first().unwrap().tile_id,
                    end_tile_id: entries.last().unwrap().tile_id,
                    shard: 0,
                    dir_offset,
                    dir_len: directory.len() as u64,
                }],
            },
        )
        .unwrap();
        let options = RepackOptions {
            input: input_dir,
            output: output_dir.clone(),
            selection: TileSelection::All,
            dry_run: false,
            verify: true,
            resume: false,
            jobs: 4,
            brotli_quality: TILE_BROTLI_QUALITY,
            verify_shards: None,
            log: None,
        };
        let report = repack_archive(&options).unwrap();
        assert_eq!(report.tiles, 25);
        let first_root = fs::read(output_dir.join("root.mkidx")).unwrap();
        let first_shard = fs::read(output_dir.join("tiles-000.mkshard")).unwrap();
        let mut resumed = options.clone();
        resumed.resume = true;
        repack_archive(&resumed).unwrap();
        assert_eq!(fs::read(output_dir.join("root.mkidx")).unwrap(), first_root);
        assert_eq!(
            fs::read(output_dir.join("tiles-000.mkshard")).unwrap(),
            first_shard
        );
        // Model a kill after a partial shard write and before its manifest.
        fs::remove_file(output_dir.join("tiles-000.mkrepack")).unwrap();
        fs::write(output_dir.join("tiles-000.partial"), b"truncated").unwrap();
        repack_archive(&resumed).unwrap();
        assert_eq!(fs::read(output_dir.join("root.mkidx")).unwrap(), first_root);
        assert_eq!(
            fs::read(output_dir.join("tiles-000.mkshard")).unwrap(),
            first_shard
        );
        let mut reader = MkmapReader::open(&output_dir).unwrap();
        assert_eq!(reader.tile_count(), 25);
        assert_eq!(reader.for_each_tile_ref(|_| {}).map(|_| 25).unwrap(), 25);
        fs::remove_dir_all(scratch).unwrap();
    }

    #[cfg(feature = "faces")]
    fn assert_float_stream_equal(left: &[f32], right: &[f32], context: &str, stream: &str) {
        assert_eq!(left.len(), right.len(), "{context} {stream} length");
        for (index, (left, right)) in left.iter().zip(right).enumerate() {
            assert_eq!(
                left.to_bits(),
                right.to_bits(),
                "{context} {stream}[{index}]"
            );
        }
    }

    #[cfg(feature = "faces")]
    fn assert_index_stream_equal(left: &[u32], right: &[u32], context: &str, stream: &str) {
        assert_eq!(left.len(), right.len(), "{context} {stream} length");
        for (index, (left, right)) in left.iter().zip(right).enumerate() {
            assert_eq!(left, right, "{context} {stream}[{index}]");
        }
    }

    #[cfg(feature = "faces")]
    fn assert_tile_buffers_byte_equal(
        left: &makepad_widgets::map::tile::TileBuffers,
        right: &makepad_widgets::map::tile::TileBuffers,
        context: &str,
    ) {
        macro_rules! same_indices {
            ($($field:ident),+ $(,)?) => {$ (
                assert_index_stream_equal(
                    &left.$field,
                    &right.$field,
                    context,
                    stringify!($field),
                );
            )+ };
        }
        macro_rules! same_floats {
            ($($field:ident),+ $(,)?) => {$ (
                assert_float_stream_equal(
                    &left.$field,
                    &right.$field,
                    context,
                    stringify!($field),
                );
            )+ };
        }
        macro_rules! same_bytes {
            ($($field:ident),+ $(,)?) => {$ (
                assert_eq!(&left.$field, &right.$field, "{context} {}", stringify!($field));
            )+ };
        }
        same_indices!(
            fill_indices,
            fill_misc_indices,
            casing_indices,
            stroke_indices,
            icon_indices,
            icon_high_indices,
            fringe_indices,
            fill_3d_indices,
            fill_3d_misc_indices,
            wall_indices,
            tree_indices,
            tree_cross_indices,
            tree_template_indices,
            tree_cross_template_indices,
            road_icon_indices,
        );
        same_floats!(
            fill_misc_vertices,
            icon_vertices,
            icon_high_vertices,
            shadow_disc_instances,
            fill_3d_misc_vertices,
            wall_vertices,
            wall_instances,
            tree_vertices,
            tree_cross_vertices,
            tree_template_vertices,
            tree_cross_template_vertices,
            tree_instances,
            road_icon_vertices,
        );
        same_bytes!(
            fill_vertices,
            casing_vertices,
            stroke_vertices,
            fringe_vertices,
            fill_3d_vertices,
        );
        for (name, left, right) in [
            ("icon_instances", &left.icon_instances, &right.icon_instances),
            (
                "icon_high_instances",
                &left.icon_high_instances,
                &right.icon_high_instances,
            ),
        ] {
            assert_eq!(left.len(), right.len(), "{context} {name} length");
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                assert_eq!(left.mesh_slot, right.mesh_slot, "{context} {name} mesh slot");
                assert_float_stream_equal(
                    &left.data,
                    &right.data,
                    context,
                    &format!("{name}[{index}].data"),
                );
            }
        }
        assert_eq!(left.labels.len(), right.labels.len(), "{context} labels length");
        for (index, (left, right)) in left.labels.iter().zip(&right.labels).enumerate() {
            assert_eq!(left, right, "{context} labels[{index}]");
        }
        assert_eq!(
            left.pin_hits.len(),
            right.pin_hits.len(),
            "{context} pin_hits length"
        );
        for (index, (left, right)) in left.pin_hits.iter().zip(&right.pin_hits).enumerate() {
            assert_eq!(left, right, "{context} pin_hits[{index}]");
        }
        assert_eq!(left.mode_overlay_only, right.mode_overlay_only, "{context}");
        assert_eq!(left.feature_count, right.feature_count, "{context}");
        assert_eq!(left.render_zoom, right.render_zoom, "{context}");
    }

    #[cfg(feature = "faces")]
    #[test]
    #[ignore = "acceptance test: 25 tiles x render zooms 14-18 x 2D/3D"]
    fn amsterdam_bake_parity() {
        use makepad_widgets::map::geometry::TileKey;
        use makepad_widgets::map::style::probe_compiled_theme;
        use makepad_widgets::map::tile::build_tile_buffers_from_mvt;

        let fixture_dir = fixture_dir();
        let mut paths: Vec<_> = fs::read_dir(fixture_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "decoded"))
            .collect();
        paths.sort();
        assert_eq!(paths.len(), 25);
        let theme = probe_compiled_theme();
        let mut builds = 0;
        for path in paths {
            let stem = path.file_stem().unwrap().to_string_lossy();
            let mut parts = stem.split('-');
            let zoom = parts.next().unwrap()[1..].parse::<u32>().unwrap();
            let x = parts.next().unwrap()[1..].parse::<i32>().unwrap();
            let y = parts.next().unwrap()[1..].parse::<i32>().unwrap();
            let key = TileKey { z: zoom, x, y };
            let original = fs::read(&path).unwrap();
            let repacked = rewrite_tile(&original).unwrap().0;
            for render_zoom in 14..=18 {
                for buildings_3d in [false, true] {
                    let left = build_tile_buffers_from_mvt(
                        key,
                        &original,
                        Some(&original),
                        None,
                        false,
                        &[],
                        &theme,
                        render_zoom,
                        buildings_3d,
                        true,
                        false,
                    )
                    .unwrap();
                    let right = build_tile_buffers_from_mvt(
                        key,
                        &repacked,
                        Some(&repacked),
                        None,
                        false,
                        &[],
                        &theme,
                        render_zoom,
                        buildings_3d,
                        true,
                        false,
                    )
                    .unwrap();
                    assert_tile_buffers_byte_equal(
                        &left,
                        &right,
                        &format!("{stem} rz{render_zoom} 3d={buildings_3d}"),
                    );
                    builds += 2;
                }
            }
        }
        println!("Amsterdam bake parity: {builds} builds renderer-equivalent");
    }
}
