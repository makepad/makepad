use std::collections::BTreeMap;

use super::geom::MVT_EXTENT;
use super::FastHashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Layer {
    OsmPoints = 0,
    OsmLines = 1,
    OsmPolygons = 2,
    OsmRelationPoints = 3,
    OsmRelationLines = 4,
    OsmRelationPolygons = 5,
    /// bridge-bake output: solved per-vertex road/rail elevation.
    BridgeDz = 6,
    /// bridge-bake output keyed to BASE tile features: per-vertex dz for
    /// the exact geometry the renderer draws (L/F/P join, no matching).
    BaseDz = 7,
}

impl Layer {
    pub fn name(self) -> &'static str {
        match self {
            Self::OsmPoints => "osm_points",
            Self::OsmLines => "osm_lines",
            Self::OsmPolygons => "osm_polygons",
            Self::OsmRelationPoints => "osm_relation_points",
            Self::OsmRelationLines => "osm_relation_lines",
            Self::OsmRelationPolygons => "osm_relation_polygons",
            Self::BridgeDz => "bridge_dz",
            Self::BaseDz => "base_dz",
        }
    }

    pub fn from_u8(value: u8) -> Result<Self, String> {
        match value {
            0 => Ok(Self::OsmPoints),
            1 => Ok(Self::OsmLines),
            2 => Ok(Self::OsmPolygons),
            3 => Ok(Self::OsmRelationPoints),
            4 => Ok(Self::OsmRelationLines),
            5 => Ok(Self::OsmRelationPolygons),
            6 => Ok(Self::BridgeDz),
            7 => Ok(Self::BaseDz),
            _ => Err(format!("unknown native tile layer {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum OsmType {
    Node = 0,
    Way = 1,
    Relation = 2,
}

impl OsmType {
    pub fn name(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Way => "way",
            Self::Relation => "relation",
        }
    }

    pub fn from_u8(value: u8) -> Result<Self, String> {
        match value {
            0 => Ok(Self::Node),
            1 => Ok(Self::Way),
            2 => Ok(Self::Relation),
            _ => Err(format!("unknown OSM object type {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum GeometryType {
    Point = 1,
    LineString = 2,
    Polygon = 3,
}

impl GeometryType {
    pub fn from_u8(value: u8) -> Result<Self, String> {
        match value {
            1 => Ok(Self::Point),
            2 => Ok(Self::LineString),
            3 => Ok(Self::Polygon),
            _ => Err(format!("unknown MVT geometry type {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilePoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileFeature {
    pub layer: Layer,
    pub geometry_type: GeometryType,
    pub osm_type: OsmType,
    pub id: i64,
    pub closed: bool,
    pub tags: Vec<(String, String)>,
    pub paths: Vec<Vec<TilePoint>>,
}

pub trait TagPair {
    fn key(&self) -> &str;
    fn value(&self) -> &str;
}

impl<K: AsRef<str>, V: AsRef<str>> TagPair for (K, V) {
    fn key(&self) -> &str {
        self.0.as_ref()
    }

    fn value(&self) -> &str {
        self.1.as_ref()
    }
}

impl TileFeature {
    pub fn can_merge(&self, other: &Self) -> bool {
        self.layer == other.layer
            && self.geometry_type == other.geometry_type
            && self.osm_type == other.osm_type
            && self.id == other.id
            && self.closed == other.closed
            && self.tags == other.tags
    }

    pub fn merge_paths(&mut self, mut other: Self) {
        self.paths.append(&mut other.paths);
    }
}

#[cfg(test)]
pub fn encode_scratch_feature(feature: &TileFeature) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    encode_scratch_parts_into(
        feature.layer,
        feature.geometry_type,
        feature.osm_type,
        feature.id,
        feature.closed,
        &feature.tags,
        feature.paths.iter().map(Vec::as_slice),
        &mut output,
    )?;
    Ok(output)
}

pub fn encode_scratch_parts_into<'a, T, P>(
    layer: Layer,
    geometry_type: GeometryType,
    osm_type: OsmType,
    id: i64,
    closed: bool,
    tags: &[T],
    paths: P,
    output: &mut Vec<u8>,
) -> Result<(), String>
where
    T: TagPair,
    P: ExactSizeIterator<Item = &'a [TilePoint]>,
{
    if id < 0 {
        return Err(format!("negative OSM feature id {id} is unsupported"));
    }
    output.clear();
    output.push(1);
    output.push(layer as u8);
    output.push(geometry_type as u8);
    output.push(osm_type as u8);
    output.push(u8::from(closed));
    write_varint(id as u64, output);
    write_varint(tags.len() as u64, output);
    for tag in tags {
        write_bytes(tag.key().as_bytes(), output)?;
        write_bytes(tag.value().as_bytes(), output)?;
    }
    write_varint(paths.len() as u64, output);
    for path in paths {
        write_varint(path.len() as u64, output);
        let mut x = 0_i32;
        let mut y = 0_i32;
        for point in path {
            write_varint(zigzag_i64(i64::from(point.x) - i64::from(x)), output);
            write_varint(zigzag_i64(i64::from(point.y) - i64::from(y)), output);
            x = point.x;
            y = point.y;
        }
    }
    Ok(())
}

pub fn decode_scratch_feature(input: &[u8]) -> Result<TileFeature, String> {
    let mut offset = 0;
    let version = read_byte(input, &mut offset)?;
    if version != 1 {
        return Err(format!("unsupported native scratch feature version {version}"));
    }
    let layer = Layer::from_u8(read_byte(input, &mut offset)?)?;
    let geometry_type = GeometryType::from_u8(read_byte(input, &mut offset)?)?;
    let osm_type = OsmType::from_u8(read_byte(input, &mut offset)?)?;
    let closed = read_byte(input, &mut offset)? != 0;
    let id = i64::try_from(read_varint(input, &mut offset)?)
        .map_err(|_| "scratch feature id exceeds i64".to_string())?;
    let tag_count = to_usize(read_varint(input, &mut offset)?, "tag count")?;
    let mut tags = Vec::with_capacity(tag_count);
    for _ in 0..tag_count {
        tags.push((
            read_string(input, &mut offset)?,
            read_string(input, &mut offset)?,
        ));
    }
    let path_count = to_usize(read_varint(input, &mut offset)?, "path count")?;
    let mut paths = Vec::with_capacity(path_count);
    for _ in 0..path_count {
        let point_count = to_usize(read_varint(input, &mut offset)?, "point count")?;
        let mut path = Vec::with_capacity(point_count);
        let mut x = 0_i64;
        let mut y = 0_i64;
        for _ in 0..point_count {
            x = x
                .checked_add(unzigzag_i64(read_varint(input, &mut offset)?))
                .ok_or_else(|| "scratch point x overflow".to_string())?;
            y = y
                .checked_add(unzigzag_i64(read_varint(input, &mut offset)?))
                .ok_or_else(|| "scratch point y overflow".to_string())?;
            path.push(TilePoint {
                x: i32::try_from(x).map_err(|_| "scratch point x exceeds i32".to_string())?,
                y: i32::try_from(y).map_err(|_| "scratch point y exceeds i32".to_string())?,
            });
        }
        paths.push(path);
    }
    if offset != input.len() {
        return Err("scratch feature has trailing bytes".to_string());
    }
    Ok(TileFeature {
        layer,
        geometry_type,
        osm_type,
        id,
        closed,
        tags,
        paths,
    })
}

fn write_bytes(bytes: &[u8], output: &mut Vec<u8>) -> Result<(), String> {
    write_varint(
        u64::try_from(bytes.len()).map_err(|_| "string length exceeds u64".to_string())?,
        output,
    );
    output.extend_from_slice(bytes);
    Ok(())
}

fn read_string(input: &[u8], offset: &mut usize) -> Result<String, String> {
    let length = to_usize(read_varint(input, offset)?, "string length")?;
    let end = offset
        .checked_add(length)
        .ok_or_else(|| "scratch string offset overflow".to_string())?;
    let bytes = input
        .get(*offset..end)
        .ok_or_else(|| "truncated scratch string".to_string())?;
    *offset = end;
    String::from_utf8(bytes.to_vec()).map_err(|err| format!("scratch tag is not UTF-8: {err}"))
}

fn read_byte(input: &[u8], offset: &mut usize) -> Result<u8, String> {
    let value = *input
        .get(*offset)
        .ok_or_else(|| "truncated scratch feature".to_string())?;
    *offset += 1;
    Ok(value)
}

fn to_usize(value: u64, what: &str) -> Result<usize, String> {
    usize::try_from(value).map_err(|_| format!("{what} exceeds usize"))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum MvtValue {
    String(String),
    UInt(u64),
    Bool(bool),
}

struct LayerBuilder {
    name: &'static str,
    keys: Vec<String>,
    key_map: FastHashMap<String, u32>,
    values: Vec<MvtValue>,
    value_map: FastHashMap<MvtValue, u32>,
    features: Vec<Vec<u8>>,
}

impl LayerBuilder {
    fn new(layer: Layer) -> Self {
        Self {
            name: layer.name(),
            keys: Vec::new(),
            key_map: FastHashMap::default(),
            values: Vec::new(),
            value_map: FastHashMap::default(),
            features: Vec::new(),
        }
    }

    fn key_index(&mut self, key: &str) -> u32 {
        if let Some(&index) = self.key_map.get(key) {
            return index;
        }
        let index = self.keys.len() as u32;
        self.keys.push(key.to_string());
        self.key_map.insert(key.to_string(), index);
        index
    }

    fn value_index(&mut self, value: MvtValue) -> u32 {
        if let Some(&index) = self.value_map.get(&value) {
            return index;
        }
        let index = self.values.len() as u32;
        self.values.push(value.clone());
        self.value_map.insert(value, index);
        index
    }

    fn push(&mut self, feature: TileFeature) -> Result<(), String> {
        let mut tags = Vec::<u32>::new();
        for (key, value) in feature.tags {
            tags.push(self.key_index(&key));
            tags.push(self.value_index(MvtValue::String(value)));
        }
        for (key, value) in [
            (
                "__makepad_osm_id",
                MvtValue::UInt(
                    u64::try_from(feature.id)
                        .map_err(|_| "negative OSM feature id is unsupported".to_string())?,
                ),
            ),
            (
                "__makepad_osm_type",
                MvtValue::String(feature.osm_type.name().to_string()),
            ),
            (
                "__makepad_osm_closed",
                MvtValue::Bool(feature.closed),
            ),
        ] {
            tags.push(self.key_index(key));
            tags.push(self.value_index(value));
        }

        let mut message = Vec::new();
        protobuf_varint_field(1, feature.id as u64, &mut message);
        let mut packed_tags = Vec::new();
        for tag in tags {
            protobuf_varint(tag as u64, &mut packed_tags);
        }
        protobuf_bytes_field(2, &packed_tags, &mut message);
        protobuf_varint_field(3, feature.geometry_type as u64, &mut message);
        let geometry = encode_geometry(feature.geometry_type, &feature.paths)?;
        protobuf_bytes_field(4, &geometry, &mut message);
        self.features.push(message);
        Ok(())
    }

    fn finish(self) -> Vec<u8> {
        let mut message = Vec::new();
        protobuf_string_field(1, self.name, &mut message);
        for feature in self.features {
            protobuf_bytes_field(2, &feature, &mut message);
        }
        for key in self.keys {
            protobuf_string_field(3, &key, &mut message);
        }
        for value in self.values {
            let mut value_message = Vec::new();
            match value {
                MvtValue::String(value) => protobuf_string_field(1, &value, &mut value_message),
                MvtValue::UInt(value) => protobuf_varint_field(5, value, &mut value_message),
                MvtValue::Bool(value) => {
                    protobuf_varint_field(7, u64::from(value), &mut value_message)
                }
            }
            protobuf_bytes_field(4, &value_message, &mut message);
        }
        protobuf_varint_field(5, MVT_EXTENT as u64, &mut message);
        protobuf_varint_field(15, 2, &mut message);
        message
    }
}

pub fn encode_tile(features: Vec<TileFeature>) -> Result<Vec<u8>, String> {
    let mut layers = BTreeMap::<Layer, LayerBuilder>::new();
    for feature in features {
        layers
            .entry(feature.layer)
            .or_insert_with(|| LayerBuilder::new(feature.layer))
            .push(feature)?;
    }
    let mut tile = Vec::new();
    for (_, layer) in layers {
        protobuf_bytes_field(3, &layer.finish(), &mut tile);
    }
    Ok(tile)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LayerInspection {
    pub name: String,
    pub features: u64,
    pub tag_features: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TileInspection {
    pub layers: Vec<LayerInspection>,
}

pub fn inspect_tile(input: &[u8]) -> Result<TileInspection, String> {
    let mut offset = 0;
    let mut result = TileInspection::default();
    while offset < input.len() {
        let (field, wire) = read_protobuf_key(input, &mut offset)?;
        match (field, wire) {
            (3, 2) => result
                .layers
                .push(inspect_layer(read_protobuf_bytes(input, &mut offset)?)?),
            _ => skip_protobuf_value(input, &mut offset, wire)?,
        }
    }
    Ok(result)
}

fn inspect_layer(input: &[u8]) -> Result<LayerInspection, String> {
    let mut offset = 0;
    let mut name = None;
    let mut keys = Vec::<String>::new();
    let mut features = Vec::<&[u8]>::new();
    while offset < input.len() {
        let (field, wire) = read_protobuf_key(input, &mut offset)?;
        match (field, wire) {
            (1, 2) => {
                let bytes = read_protobuf_bytes(input, &mut offset)?;
                name = Some(
                    std::str::from_utf8(bytes)
                        .map_err(|err| format!("MVT layer name is not UTF-8: {err}"))?
                        .to_string(),
                );
            }
            (2, 2) => features.push(read_protobuf_bytes(input, &mut offset)?),
            (3, 2) => {
                let bytes = read_protobuf_bytes(input, &mut offset)?;
                keys.push(
                    std::str::from_utf8(bytes)
                        .map_err(|err| format!("MVT tag key is not UTF-8: {err}"))?
                        .to_string(),
                );
            }
            _ => skip_protobuf_value(input, &mut offset, wire)?,
        }
    }

    let mut tag_features = BTreeMap::<String, u64>::new();
    for feature in &features {
        let mut feature_offset = 0;
        while feature_offset < feature.len() {
            let (field, wire) = read_protobuf_key(feature, &mut feature_offset)?;
            if field == 2 && wire == 2 {
                let tags = read_protobuf_bytes(feature, &mut feature_offset)?;
                let mut tags_offset = 0;
                let mut expecting_key = true;
                while tags_offset < tags.len() {
                    let index = usize::try_from(read_varint(tags, &mut tags_offset)?)
                        .map_err(|_| "MVT tag index exceeds usize".to_string())?;
                    if expecting_key {
                        let key = keys.get(index).ok_or_else(|| {
                            format!("MVT feature references missing tag key {index}")
                        })?;
                        *tag_features.entry(key.clone()).or_default() += 1;
                    }
                    expecting_key = !expecting_key;
                }
                if !expecting_key {
                    return Err("MVT feature has an odd packed tag index count".to_string());
                }
            } else {
                skip_protobuf_value(feature, &mut feature_offset, wire)?;
            }
        }
    }
    Ok(LayerInspection {
        name: name.ok_or_else(|| "MVT layer has no name".to_string())?,
        features: features.len() as u64,
        tag_features,
    })
}

pub(crate) fn read_protobuf_key(input: &[u8], offset: &mut usize) -> Result<(u32, u8), String> {
    let key = read_varint(input, offset)?;
    let field = u32::try_from(key >> 3).map_err(|_| "protobuf field exceeds u32".to_string())?;
    let wire = (key & 7) as u8;
    if field == 0 {
        return Err("protobuf field number 0 is invalid".to_string());
    }
    Ok((field, wire))
}

pub(crate) fn read_protobuf_bytes<'a>(input: &'a [u8], offset: &mut usize) -> Result<&'a [u8], String> {
    let length = to_usize(read_varint(input, offset)?, "protobuf byte length")?;
    let end = offset
        .checked_add(length)
        .ok_or_else(|| "protobuf byte range overflow".to_string())?;
    let result = input
        .get(*offset..end)
        .ok_or_else(|| "truncated protobuf bytes".to_string())?;
    *offset = end;
    Ok(result)
}

pub(crate) fn skip_protobuf_value(input: &[u8], offset: &mut usize, wire: u8) -> Result<(), String> {
    match wire {
        0 => {
            read_varint(input, offset)?;
        }
        1 => {
            *offset = offset
                .checked_add(8)
                .ok_or_else(|| "protobuf offset overflow".to_string())?;
        }
        2 => {
            let _ = read_protobuf_bytes(input, offset)?;
        }
        5 => {
            *offset = offset
                .checked_add(4)
                .ok_or_else(|| "protobuf offset overflow".to_string())?;
        }
        _ => return Err(format!("unsupported protobuf wire type {wire}")),
    }
    if *offset > input.len() {
        return Err("truncated protobuf value".to_string());
    }
    Ok(())
}

fn encode_geometry(
    geometry_type: GeometryType,
    paths: &[Vec<TilePoint>],
) -> Result<Vec<u8>, String> {
    let mut commands = Vec::<u32>::new();
    let mut cursor = TilePoint { x: 0, y: 0 };
    match geometry_type {
        GeometryType::Point => {
            let points = paths
                .iter()
                .flat_map(|path| path.iter())
                .copied()
                .collect::<Vec<_>>();
            if points.is_empty() {
                return Err("point feature has no points".to_string());
            }
            commands.push(command(1, points.len())?);
            for point in points {
                push_delta(point, &mut cursor, &mut commands);
            }
        }
        GeometryType::LineString => {
            for path in paths {
                if path.len() < 2 {
                    continue;
                }
                commands.push(command(1, 1)?);
                push_delta(path[0], &mut cursor, &mut commands);
                commands.push(command(2, path.len() - 1)?);
                for &point in &path[1..] {
                    push_delta(point, &mut cursor, &mut commands);
                }
            }
        }
        GeometryType::Polygon => {
            for path in paths {
                if path.len() < 3 {
                    continue;
                }
                commands.push(command(1, 1)?);
                push_delta(path[0], &mut cursor, &mut commands);
                commands.push(command(2, path.len() - 1)?);
                for &point in &path[1..] {
                    push_delta(point, &mut cursor, &mut commands);
                }
                commands.push(command(7, 1)?);
            }
        }
    }
    if commands.is_empty() {
        return Err("feature has no encodable geometry".to_string());
    }
    let mut output = Vec::new();
    for value in commands {
        protobuf_varint(value as u64, &mut output);
    }
    Ok(output)
}

fn command(id: u32, count: usize) -> Result<u32, String> {
    let count = u32::try_from(count).map_err(|_| "MVT command count exceeds u32".to_string())?;
    count
        .checked_shl(3)
        .and_then(|value| value.checked_add(id))
        .ok_or_else(|| "MVT command integer overflow".to_string())
}

fn push_delta(point: TilePoint, cursor: &mut TilePoint, output: &mut Vec<u32>) {
    let dx = point.x.wrapping_sub(cursor.x);
    let dy = point.y.wrapping_sub(cursor.y);
    output.push(zigzag_i32(dx));
    output.push(zigzag_i32(dy));
    *cursor = point;
}

fn zigzag_i32(value: i32) -> u32 {
    ((value << 1) ^ (value >> 31)) as u32
}

fn zigzag_i64(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn unzigzag_i64(value: u64) -> i64 {
    ((value >> 1) as i64) ^ (-((value & 1) as i64))
}

fn write_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

pub(crate) fn read_varint(input: &[u8], offset: &mut usize) -> Result<u64, String> {
    let mut value = 0_u64;
    for shift in (0..=63).step_by(7) {
        let byte = read_byte(input, offset)?;
        if shift == 63 && byte > 1 {
            return Err("scratch varint overflow".to_string());
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("scratch varint overflow".to_string())
}

fn protobuf_key(field: u32, wire: u8, output: &mut Vec<u8>) {
    protobuf_varint((u64::from(field) << 3) | u64::from(wire), output);
}

fn protobuf_varint(value: u64, output: &mut Vec<u8>) {
    write_varint(value, output);
}

fn protobuf_varint_field(field: u32, value: u64, output: &mut Vec<u8>) {
    protobuf_key(field, 0, output);
    protobuf_varint(value, output);
}

fn protobuf_bytes_field(field: u32, value: &[u8], output: &mut Vec<u8>) {
    protobuf_key(field, 2, output);
    protobuf_varint(value.len() as u64, output);
    output.extend_from_slice(value);
}

fn protobuf_string_field(field: u32, value: &str, output: &mut Vec<u8>) {
    protobuf_bytes_field(field, value.as_bytes(), output);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_feature() -> TileFeature {
        TileFeature {
            layer: Layer::OsmLines,
            geometry_type: GeometryType::LineString,
            osm_type: OsmType::Way,
            id: 42,
            closed: false,
            tags: vec![
                ("highway".to_string(), "residential".to_string()),
                ("name".to_string(), "Test Street".to_string()),
                ("height".to_string(), "12.5".to_string()),
            ],
            paths: vec![vec![
                TilePoint { x: -64, y: 100 },
                TilePoint { x: 2048, y: 2100 },
                TilePoint { x: 4160, y: 4096 },
            ]],
        }
    }

    #[test]
    fn scratch_feature_round_trip_is_lossless() {
        let feature = sample_feature();
        let encoded = encode_scratch_feature(&feature).unwrap();
        assert_eq!(decode_scratch_feature(&encoded).unwrap(), feature);
    }

    #[test]
    fn encodes_nonempty_mvt_tile_with_all_tag_metadata() {
        let encoded = encode_tile(vec![sample_feature()]).unwrap();
        assert!(!encoded.is_empty());
        assert!(encoded
            .windows("osm_lines".len())
            .any(|window| window == b"osm_lines"));
        assert!(encoded
            .windows("__makepad_osm_type".len())
            .any(|window| window == b"__makepad_osm_type"));
        let inspected = inspect_tile(&encoded).unwrap();
        assert_eq!(inspected.layers.len(), 1);
        assert_eq!(inspected.layers[0].name, "osm_lines");
        assert_eq!(inspected.layers[0].features, 1);
        assert_eq!(inspected.layers[0].tag_features["height"], 1);
    }
}
