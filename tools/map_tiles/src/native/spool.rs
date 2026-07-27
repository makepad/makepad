use super::mvt::{
    decode_scratch_feature, encode_scratch_parts_into, GeometryType, Layer, OsmType, TagPair,
    TileFeature, TilePoint,
};
use super::FastHashMap;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeSet, BinaryHeap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

const BLOCK_SHIFT: u32 = 8;
const BLOCK_MASK: u32 = (1 << BLOCK_SHIFT) - 1;
const RECORD_HEADER_LEN: usize = 2 + 1 + 1 + 8 + 1 + 4;
const DEFAULT_SORT_MEMORY: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockKey {
    pub y: u32,
    pub x: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RecordKey {
    pub tile: u16,
    pub layer: u8,
    pub osm_type: u8,
    pub id: u64,
    pub geometry_type: u8,
}

#[derive(Debug)]
pub struct SpoolRecord {
    pub key: RecordKey,
    pub payload: Vec<u8>,
}

struct OpenBlock {
    writer: BufWriter<File>,
    used: u64,
}

pub struct BlockSpoolWriter {
    dir: PathBuf,
    open: FastHashMap<BlockKey, OpenBlock>,
    open_capacity: usize,
    clock: u64,
    blocks: BTreeSet<BlockKey>,
    records: u64,
    bytes: u64,
    scratch: Vec<u8>,
}

impl BlockSpoolWriter {
    pub fn create(dir: &Path) -> Result<Self, String> {
        if dir.exists() {
            return Err(format!(
                "{} already exists; refusing to mix native tile spool data",
                dir.display()
            ));
        }
        fs::create_dir_all(dir).map_err(|err| format!("create {}: {err}", dir.display()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            open: FastHashMap::default(),
            open_capacity: 384,
            clock: 0,
            blocks: BTreeSet::new(),
            records: 0,
            bytes: 0,
            scratch: Vec::new(),
        })
    }

    #[cfg(test)]
    pub fn push(&mut self, tile_x: u32, tile_y: u32, feature: &TileFeature) -> Result<(), String> {
        self.push_parts(
            tile_x,
            tile_y,
            feature.layer,
            feature.geometry_type,
            feature.osm_type,
            feature.id,
            feature.closed,
            &feature.tags,
            feature.paths.iter().map(Vec::as_slice),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_parts<'a, T, P>(
        &mut self,
        tile_x: u32,
        tile_y: u32,
        layer: Layer,
        geometry_type: GeometryType,
        osm_type: OsmType,
        id: i64,
        closed: bool,
        tags: &[T],
        paths: P,
    ) -> Result<(), String>
    where
        T: TagPair,
        P: ExactSizeIterator<Item = &'a [TilePoint]>,
    {
        let block = BlockKey {
            y: tile_y >> BLOCK_SHIFT,
            x: tile_x >> BLOCK_SHIFT,
        };
        let local_x = tile_x & BLOCK_MASK;
        let local_y = tile_y & BLOCK_MASK;
        let tile = u16::try_from((local_y << BLOCK_SHIFT) | local_x).unwrap();
        let mut payload = std::mem::take(&mut self.scratch);
        encode_scratch_parts_into(
            layer,
            geometry_type,
            osm_type,
            id,
            closed,
            tags,
            paths,
            &mut payload,
        )?;
        let key = RecordKey {
            tile,
            layer: layer as u8,
            osm_type: osm_type as u8,
            id: id as u64,
            geometry_type: geometry_type as u8,
        };
        let record_bytes = RECORD_HEADER_LEN
            .checked_add(payload.len())
            .ok_or_else(|| "spool record size overflow".to_string())?;
        let writer = self.writer(block)?;
        write_record(writer, key, &payload)?;
        payload.clear();
        self.scratch = payload;
        self.blocks.insert(block);
        self.records += 1;
        self.bytes = self
            .bytes
            .checked_add(record_bytes as u64)
            .ok_or_else(|| "spool byte count overflow".to_string())?;
        Ok(())
    }

    fn writer(&mut self, key: BlockKey) -> Result<&mut BufWriter<File>, String> {
        self.clock += 1;
        if self.open.contains_key(&key) {
            let entry = self.open.get_mut(&key).unwrap();
            entry.used = self.clock;
            return Ok(&mut entry.writer);
        }
        if self.open.len() >= self.open_capacity {
            let oldest = *self
                .open
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key)
                .unwrap();
            let mut old = self.open.remove(&oldest).unwrap();
            old.writer
                .flush()
                .map_err(|err| format!("flush block spool: {err}"))?;
        }
        let path = block_path(&self.dir, key);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|err| format!("open {}: {err}", path.display()))?;
        self.open.insert(
            key,
            OpenBlock {
                writer: BufWriter::with_capacity(256 * 1024, file),
                used: self.clock,
            },
        );
        Ok(&mut self.open.get_mut(&key).unwrap().writer)
    }

    pub fn finish(mut self) -> Result<SpoolSummary, String> {
        for entry in self.open.values_mut() {
            entry
                .writer
                .flush()
                .map_err(|err| format!("flush block spool: {err}"))?;
        }
        Ok(SpoolSummary {
            dir: self.dir,
            blocks: self.blocks.into_iter().collect(),
            records: self.records,
            bytes: self.bytes,
        })
    }
}

pub struct SpoolSummary {
    pub dir: PathBuf,
    pub blocks: Vec<BlockKey>,
    pub records: u64,
    pub bytes: u64,
}

impl SpoolSummary {
    pub fn from_dir(dir: &Path) -> Result<Self, String> {
        let mut blocks = Vec::new();
        for entry in fs::read_dir(dir).map_err(|err| format!("read {}: {err}", dir.display()))? {
            let entry = entry.map_err(|err| format!("read {} entry: {err}", dir.display()))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if let Some(key) = parse_block_name(name) {
                blocks.push(key);
            }
        }
        blocks.sort();
        Ok(Self {
            dir: dir.to_path_buf(),
            blocks,
            records: 0,
            bytes: 0,
        })
    }
}

fn block_path(dir: &Path, key: BlockKey) -> PathBuf {
    dir.join(format!("block-{}-{}.spool", key.y, key.x))
}

fn parse_block_name(name: &str) -> Option<BlockKey> {
    let body = name.strip_prefix("block-")?.strip_suffix(".spool")?;
    let (y, x) = body.split_once('-')?;
    Some(BlockKey {
        y: y.parse().ok()?,
        x: x.parse().ok()?,
    })
}

fn write_record(writer: &mut impl Write, key: RecordKey, payload: &[u8]) -> Result<(), String> {
    let mut header = [0_u8; RECORD_HEADER_LEN];
    header[0..2].copy_from_slice(&key.tile.to_le_bytes());
    header[2] = key.layer;
    header[3] = key.osm_type;
    header[4..12].copy_from_slice(&key.id.to_le_bytes());
    header[12] = key.geometry_type;
    header[13..17].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    writer
        .write_all(&header)
        .and_then(|_| writer.write_all(payload))
        .map_err(|err| format!("write native tile spool: {err}"))
}

fn read_record(reader: &mut impl Read) -> Result<Option<SpoolRecord>, String> {
    let mut header = [0_u8; RECORD_HEADER_LEN];
    let mut filled = 0;
    while filled < header.len() {
        match reader.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err("truncated native tile spool header".to_string()),
            Ok(count) => filled += count,
            Err(err) => return Err(format!("read native tile spool: {err}")),
        }
    }
    let key = RecordKey {
        tile: u16::from_le_bytes(header[0..2].try_into().unwrap()),
        layer: header[2],
        osm_type: header[3],
        id: u64::from_le_bytes(header[4..12].try_into().unwrap()),
        geometry_type: header[12],
    };
    let length = u32::from_le_bytes(header[13..17].try_into().unwrap()) as usize;
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .map_err(|err| format!("read native tile spool payload: {err}"))?;
    Ok(Some(SpoolRecord { key, payload }))
}

pub struct SortedBlock {
    #[cfg(test)]
    source_path: PathBuf,
    chunk_paths: Vec<PathBuf>,
    memory: Option<Vec<SpoolRecord>>,
}

impl SortedBlock {
    pub fn prepare(
        dir: &Path,
        key: BlockKey,
        memory_limit: Option<usize>,
    ) -> Result<Self, String> {
        let source_path = block_path(dir, key);
        let memory_limit = memory_limit.unwrap_or(DEFAULT_SORT_MEMORY).max(1024 * 1024);
        let mut reader = BufReader::with_capacity(
            4 * 1024 * 1024,
            File::open(&source_path)
                .map_err(|err| format!("open {}: {err}", source_path.display()))?,
        );
        let mut chunk_paths = Vec::new();
        let mut records = Vec::new();
        let mut bytes = 0_usize;
        while let Some(record) = read_record(&mut reader)? {
            bytes = bytes
                .checked_add(RECORD_HEADER_LEN + record.payload.len())
                .ok_or_else(|| "spool sort memory count overflow".to_string())?;
            records.push(record);
            if bytes >= memory_limit {
                let chunk = write_sorted_chunk(dir, key, chunk_paths.len(), &mut records)?;
                chunk_paths.push(chunk);
                bytes = 0;
            }
        }
        if chunk_paths.is_empty() {
            records.sort_unstable_by_key(|record| record.key);
            Ok(Self {
                #[cfg(test)]
                source_path,
                chunk_paths,
                memory: Some(records),
            })
        } else {
            if !records.is_empty() {
                let chunk = write_sorted_chunk(dir, key, chunk_paths.len(), &mut records)?;
                chunk_paths.push(chunk);
            }
            Ok(Self {
                #[cfg(test)]
                source_path,
                chunk_paths,
                memory: None,
            })
        }
    }

    pub fn for_each<F>(&mut self, mut callback: F) -> Result<(), String>
    where
        F: FnMut(SpoolRecord) -> Result<(), String>,
    {
        if let Some(records) = &mut self.memory {
            for record in records.drain(..) {
                callback(record)?;
            }
        } else {
            merge_chunks(&self.chunk_paths, &mut callback)?;
        }
        Ok(())
    }

    pub fn cleanup_chunks(&mut self) -> Result<(), String> {
        for path in self.chunk_paths.drain(..) {
            fs::remove_file(&path).map_err(|err| format!("remove {}: {err}", path.display()))?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn cleanup_all(mut self) -> Result<(), String> {
        self.cleanup_chunks()?;
        fs::remove_file(&self.source_path)
            .map_err(|err| format!("remove {}: {err}", self.source_path.display()))
    }
}

fn write_sorted_chunk(
    dir: &Path,
    key: BlockKey,
    index: usize,
    records: &mut Vec<SpoolRecord>,
) -> Result<PathBuf, String> {
    records.sort_unstable_by_key(|record| record.key);
    let path = dir.join(format!("block-{}-{}.sort-{index}", key.y, key.x));
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|err| format!("create {}: {err}", path.display()))?;
    let mut writer = BufWriter::with_capacity(4 * 1024 * 1024, file);
    for record in records.drain(..) {
        write_record(&mut writer, record.key, &record.payload)?;
    }
    writer
        .flush()
        .map_err(|err| format!("flush {}: {err}", path.display()))?;
    Ok(path)
}

struct MergeInput {
    reader: BufReader<File>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeapItem {
    key: RecordKey,
    input: usize,
}

impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key
            .cmp(&other.key)
            .then_with(|| self.input.cmp(&other.input))
    }
}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn merge_chunks<F>(paths: &[PathBuf], callback: &mut F) -> Result<(), String>
where
    F: FnMut(SpoolRecord) -> Result<(), String>,
{
    let mut inputs = Vec::with_capacity(paths.len());
    let mut pending = Vec::<Option<SpoolRecord>>::with_capacity(paths.len());
    let mut heap = BinaryHeap::<Reverse<HeapItem>>::new();
    for (index, path) in paths.iter().enumerate() {
        let file = File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
        let mut input = MergeInput {
            reader: BufReader::with_capacity(1024 * 1024, file),
        };
        let record = read_record(&mut input.reader)?;
        if let Some(record) = &record {
            heap.push(Reverse(HeapItem {
                key: record.key,
                input: index,
            }));
        }
        inputs.push(input);
        pending.push(record);
    }
    while let Some(Reverse(item)) = heap.pop() {
        let record = pending[item.input]
            .take()
            .ok_or_else(|| "spool merge heap referenced no record".to_string())?;
        callback(record)?;
        let next = read_record(&mut inputs[item.input].reader)?;
        if let Some(record) = &next {
            heap.push(Reverse(HeapItem {
                key: record.key,
                input: item.input,
            }));
        }
        pending[item.input] = next;
    }
    Ok(())
}

pub fn records_to_tiles<F>(
    mut sorted: SortedBlock,
    key: BlockKey,
    mut callback: F,
) -> Result<SortedBlock, String>
where
    F: FnMut(u32, u32, Vec<TileFeature>) -> Result<(), String>,
{
    let mut current_tile = None;
    let mut features = Vec::<TileFeature>::new();
    let mut pending_feature: Option<TileFeature> = None;

    let flush_feature = |pending: &mut Option<TileFeature>, features: &mut Vec<TileFeature>| {
        if let Some(feature) = pending.take() {
            features.push(feature);
        }
    };
    let flush_tile = |tile: Option<u16>,
                      pending: &mut Option<TileFeature>,
                      features: &mut Vec<TileFeature>,
                      callback: &mut F|
     -> Result<(), String> {
        let Some(tile) = tile else {
            return Ok(());
        };
        flush_feature(pending, features);
        let local_x = u32::from(tile & 255);
        let local_y = u32::from(tile >> 8);
        callback(
            (key.x << BLOCK_SHIFT) | local_x,
            (key.y << BLOCK_SHIFT) | local_y,
            std::mem::take(features),
        )
    };

    sorted.for_each(|record| {
        if current_tile != Some(record.key.tile) {
            flush_tile(
                current_tile,
                &mut pending_feature,
                &mut features,
                &mut callback,
            )?;
            current_tile = Some(record.key.tile);
        }
        let feature = decode_scratch_feature(&record.payload)?;
        if pending_feature
            .as_ref()
            .is_some_and(|pending| pending.can_merge(&feature))
        {
            pending_feature.as_mut().unwrap().merge_paths(feature);
        } else {
            flush_feature(&mut pending_feature, &mut features);
            pending_feature = Some(feature);
        }
        Ok(())
    })?;
    flush_tile(
        current_tile,
        &mut pending_feature,
        &mut features,
        &mut callback,
    )?;
    Ok(sorted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::mvt::{GeometryType, Layer, OsmType, TilePoint};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("{name}-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        path
    }

    fn feature(id: i64, path_x: i32) -> TileFeature {
        TileFeature {
            layer: Layer::OsmLines,
            geometry_type: GeometryType::LineString,
            osm_type: OsmType::Way,
            id,
            closed: false,
            tags: vec![("name".to_string(), format!("way-{id}"))],
            paths: vec![vec![
                TilePoint { x: path_x, y: 0 },
                TilePoint {
                    x: path_x + 1,
                    y: 1,
                },
            ]],
        }
    }

    #[test]
    fn external_sort_groups_tiles_and_merges_feature_paths() {
        let dir = temp_dir("makepad-native-spool");
        let spool_dir = dir.join("spool");
        let mut writer = BlockSpoolWriter::create(&spool_dir).unwrap();
        writer.push(257, 259, &feature(9, 30)).unwrap();
        writer.push(256, 258, &feature(4, 10)).unwrap();
        writer.push(257, 259, &feature(9, 40)).unwrap();
        writer.push(256, 258, &feature(3, 20)).unwrap();
        let summary = writer.finish().unwrap();
        assert_eq!(summary.blocks, vec![BlockKey { x: 1, y: 1 }]);

        let sorted = SortedBlock::prepare(&spool_dir, summary.blocks[0], Some(1024 * 1024))
            .unwrap();
        let mut tiles = Vec::new();
        let sorted = records_to_tiles(sorted, summary.blocks[0], |x, y, features| {
            tiles.push((x, y, features));
            Ok(())
        })
        .unwrap();
        assert_eq!(tiles.len(), 2);
        assert_eq!((tiles[0].0, tiles[0].1), (256, 258));
        assert_eq!(tiles[0].2[0].id, 3);
        assert_eq!(tiles[0].2[1].id, 4);
        assert_eq!((tiles[1].0, tiles[1].1), (257, 259));
        assert_eq!(tiles[1].2.len(), 1);
        assert_eq!(tiles[1].2[0].paths.len(), 2);
        sorted.cleanup_all().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
}
