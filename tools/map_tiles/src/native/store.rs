use super::FastHashMap;
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const NODE_DATA_MAGIC: &[u8; 8] = b"MPNODED2";
const NODE_INDEX_MAGIC: &[u8; 8] = b"MPNODEI2";
const WAY_DATA_MAGIC: &[u8; 8] = b"MPWAYDT1";
const WAY_INDEX_MAGIC: &[u8; 8] = b"MPWAYIX1";
const NODE_GROUP_SHIFT: u32 = 16;
const NODE_CHUNK_SHIFT: u32 = 8;
const NODE_GROUP_MASK: u64 = (1 << NODE_GROUP_SHIFT) - 1;
const NODE_CHUNK_MASK: u64 = (1 << NODE_CHUNK_SHIFT) - 1;
const WAY_GROUP_SHIFT: u32 = 12;
const WAY_GROUP_MASK: u64 = (1 << WAY_GROUP_SHIFT) - 1;
const BITSET_PAGE_BYTES: usize = 4096;
const BITSET_PAGE_BITS: u64 = (BITSET_PAGE_BYTES * 8) as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeCoord {
    pub id: i64,
    pub x: i64,
    pub y: i64,
}

#[derive(Clone, Copy, Debug)]
struct GroupIndexEntry {
    group_id: u64,
    offset: u64,
    length: u32,
}

#[derive(Debug)]
struct CacheEntry<T> {
    value: T,
    used: u64,
}

struct StoreIndex {
    sorted: Vec<GroupIndexEntry>,
    direct: Option<Vec<Option<GroupIndexEntry>>>,
}

impl StoreIndex {
    fn new(sorted: Vec<GroupIndexEntry>) -> Self {
        const MAX_DIRECT_GROUPS: u64 = 4_000_000;
        let direct = sorted
            .last()
            .map(|entry| entry.group_id)
            .filter(|group_id| *group_id <= MAX_DIRECT_GROUPS)
            .map(|max_group_id| {
                let mut direct = vec![None; max_group_id as usize + 1];
                for entry in &sorted {
                    direct[entry.group_id as usize] = Some(*entry);
                }
                direct
            });
        Self { sorted, direct }
    }

    fn get(&self, group_id: u64) -> Option<GroupIndexEntry> {
        if let Some(direct) = &self.direct {
            return direct.get(group_id as usize).copied().flatten();
        }
        find_entry(&self.sorted, group_id)
    }
}

fn write_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

/// Node-group cache size: MAKEPAD_NODE_CACHE_MIB (default 8192 MiB).
pub fn node_cache_groups_public() -> usize {
    node_cache_groups()
}

/// The whole node store decoded into RAM: lock-free shared lookups for
/// parallel way resolution. Europe's ways visit node groups in creation
/// order (i.e. randomly in space), so any cache small enough to fit loses;
/// the full decode is ~5x the compressed store and loads in about a
/// minute with all cores inflating.
pub struct FlatNodeStore {
    groups: Vec<Option<DecodedNodeGroup>>,
}

impl FlatNodeStore {
    pub fn load(data_path: &Path, index_path: &Path) -> Result<Self, String> {
        let mut file = File::open(data_path)
            .map_err(|err| format!("open {}: {err}", data_path.display()))?;
        verify_data_magic(&mut file, data_path, NODE_DATA_MAGIC)?;
        drop(file);
        let entries = read_index(index_path, NODE_INDEX_MAGIC)?;
        let max_group = entries
            .iter()
            .map(|entry| entry.group_id)
            .max()
            .unwrap_or(0) as usize;
        let mut groups: Vec<Option<DecodedNodeGroup>> = Vec::new();
        groups.resize_with(max_group + 1, || None);

        let workers = std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 16))
            .unwrap_or(4);
        let chunk_size = entries.len().div_ceil(workers).max(1);
        let decoded: Vec<Vec<(u64, DecodedNodeGroup)>> =
            std::thread::scope(|scope| -> Result<_, String> {
                let mut handles = Vec::new();
                for chunk in entries.chunks(chunk_size) {
                    let data_path = data_path.to_path_buf();
                    handles.push(scope.spawn(move || -> Result<_, String> {
                        let mut file = File::open(&data_path)
                            .map_err(|err| format!("open {}: {err}", data_path.display()))?;
                        let mut out = Vec::with_capacity(chunk.len());
                        for entry in chunk {
                            file.seek(SeekFrom::Start(entry.offset)).map_err(|err| {
                                format!("seek {}: {err}", data_path.display())
                            })?;
                            let mut bytes = vec![0_u8; entry.length as usize];
                            file.read_exact(&mut bytes).map_err(|err| {
                                format!("read {}: {err}", data_path.display())
                            })?;
                            out.push((entry.group_id, decode_node_group(&bytes, entry.group_id)?));
                        }
                        Ok(out)
                    }));
                }
                let mut decoded = Vec::new();
                for handle in handles {
                    decoded.push(
                        handle
                            .join()
                            .map_err(|_| "node store load thread panicked".to_string())??,
                    );
                }
                Ok(decoded)
            })?;
        for part in decoded {
            for (group_id, group) in part {
                groups[group_id as usize] = Some(group);
            }
        }
        Ok(Self { groups })
    }

    pub fn get(&self, id: i64) -> Result<Option<NodeCoord>, String> {
        if id < 0 {
            return Ok(None);
        }
        let group_id = (id as u64 >> NODE_GROUP_SHIFT) as usize;
        match self.groups.get(group_id).and_then(|group| group.as_ref()) {
            Some(group) => group.get(id),
            None => Ok(None),
        }
    }
}

fn node_cache_groups() -> usize {
    let mib = std::env::var("MAKEPAD_NODE_CACHE_MIB")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(8192);
    (mib * 4 / 3).max(256)
}

fn write_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn write_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn read_u16(input: &[u8], offset: &mut usize) -> Result<u16, String> {
    let bytes = take(input, offset, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(input: &[u8], offset: &mut usize) -> Result<u32, String> {
    let bytes = take(input, offset, 4)?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_u64(input: &[u8], offset: &mut usize) -> Result<u64, String> {
    let bytes = take(input, offset, 8)?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_i64(input: &[u8], offset: &mut usize) -> Result<i64, String> {
    let bytes = take(input, offset, 8)?;
    Ok(i64::from_le_bytes(bytes.try_into().unwrap()))
}

fn take<'a>(input: &'a [u8], offset: &mut usize, length: usize) -> Result<&'a [u8], String> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| "scratch record offset overflow".to_string())?;
    let bytes = input
        .get(*offset..end)
        .ok_or_else(|| "truncated scratch record".to_string())?;
    *offset = end;
    Ok(bytes)
}

fn write_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn read_varint(input: &[u8], offset: &mut usize) -> Result<u64, String> {
    let mut value = 0_u64;
    for shift in (0..=63).step_by(7) {
        let byte = *input
            .get(*offset)
            .ok_or_else(|| "truncated scratch varint".to_string())?;
        *offset += 1;
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

fn zigzag_i64(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn unzigzag_i64(value: u64) -> i64 {
    ((value >> 1) as i64) ^ (-((value & 1) as i64))
}

fn set_mask_bit(mask: &mut [u8], bit: usize) {
    mask[bit >> 3] |= 1 << (bit & 7);
}

fn mask_has_bit(mask: &[u8], bit: usize) -> bool {
    mask[bit >> 3] & (1 << (bit & 7)) != 0
}

fn checked_u32(value: usize, what: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{what} exceeds 4 GiB"))
}

fn create_store_file(path: &Path, magic: &[u8; 8]) -> Result<File, String> {
    if path.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite scratch data",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|err| format!("create {}: {err}", path.display()))?;
    file.write_all(magic)
        .map_err(|err| format!("write {}: {err}", path.display()))?;
    Ok(file)
}

fn write_index(
    path: &Path,
    magic: &[u8; 8],
    entries: &[GroupIndexEntry],
) -> Result<(), String> {
    if path.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite scratch index",
            path.display()
        ));
    }
    let mut bytes = Vec::with_capacity(16 + entries.len() * 24);
    bytes.extend_from_slice(magic);
    write_u64(&mut bytes, entries.len() as u64);
    for entry in entries {
        write_u64(&mut bytes, entry.group_id);
        write_u64(&mut bytes, entry.offset);
        write_u32(&mut bytes, entry.length);
        write_u32(&mut bytes, 0);
    }
    fs::write(path, bytes).map_err(|err| format!("write {}: {err}", path.display()))
}

fn read_index(path: &Path, magic: &[u8; 8]) -> Result<Vec<GroupIndexEntry>, String> {
    let bytes = fs::read(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    if bytes.get(..8) != Some(magic) {
        return Err(format!("{} has the wrong scratch index magic", path.display()));
    }
    let mut offset = 8;
    let count = usize::try_from(read_u64(&bytes, &mut offset)?)
        .map_err(|_| "scratch index entry count exceeds usize".to_string())?;
    let expected = 16_usize
        .checked_add(
            count
                .checked_mul(24)
                .ok_or_else(|| "scratch index size overflow".to_string())?,
        )
        .ok_or_else(|| "scratch index size overflow".to_string())?;
    if bytes.len() != expected {
        return Err(format!(
            "{} has length {}, expected {expected}",
            path.display(),
            bytes.len()
        ));
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        entries.push(GroupIndexEntry {
            group_id: read_u64(&bytes, &mut offset)?,
            offset: read_u64(&bytes, &mut offset)?,
            length: read_u32(&bytes, &mut offset)?,
        });
        let _reserved = read_u32(&bytes, &mut offset)?;
    }
    if entries
        .windows(2)
        .any(|pair| pair[0].group_id >= pair[1].group_id)
    {
        return Err(format!("{} is not strictly sorted", path.display()));
    }
    Ok(entries)
}

fn verify_data_magic(file: &mut File, path: &Path, magic: &[u8; 8]) -> Result<(), String> {
    let mut actual = [0_u8; 8];
    file.read_exact(&mut actual)
        .map_err(|err| format!("read {}: {err}", path.display()))?;
    if &actual != magic {
        return Err(format!("{} has the wrong scratch data magic", path.display()));
    }
    Ok(())
}

pub struct NodeStoreBuilder {
    data_path: PathBuf,
    index_path: PathBuf,
    file: File,
    entries: Vec<GroupIndexEntry>,
    group_id: Option<u64>,
    nodes: Vec<NodeCoord>,
    last_id: Option<i64>,
    count: u64,
}

impl NodeStoreBuilder {
    pub fn create(data_path: &Path, index_path: &Path) -> Result<Self, String> {
        Ok(Self {
            data_path: data_path.to_path_buf(),
            index_path: index_path.to_path_buf(),
            file: create_store_file(data_path, NODE_DATA_MAGIC)?,
            entries: Vec::new(),
            group_id: None,
            nodes: Vec::new(),
            last_id: None,
            count: 0,
        })
    }

    pub fn push(&mut self, node: NodeCoord) -> Result<(), String> {
        if node.id < 0 {
            return Err(format!("negative OSM node id {} is unsupported", node.id));
        }
        if self.last_id.is_some_and(|last| node.id <= last) {
            return Err(format!(
                "OSM nodes are not strictly sorted by id: {} follows {}",
                node.id,
                self.last_id.unwrap()
            ));
        }
        let group_id = node.id as u64 >> NODE_GROUP_SHIFT;
        if self.group_id.is_some_and(|current| current != group_id) {
            self.flush_group()?;
        }
        self.group_id = Some(group_id);
        self.nodes.push(node);
        self.last_id = Some(node.id);
        self.count += 1;
        Ok(())
    }

    fn flush_group(&mut self) -> Result<(), String> {
        let Some(group_id) = self.group_id else {
            return Ok(());
        };
        if self.nodes.is_empty() {
            return Err("node scratch group is unexpectedly empty".to_string());
        }

        let mut chunk_mask = [0_u8; 32];
        let mut chunks = Vec::<Vec<u8>>::new();
        let mut start = 0;
        while start < self.nodes.len() {
            let chunk_id = ((self.nodes[start].id as u64 & NODE_GROUP_MASK)
                >> NODE_CHUNK_SHIFT) as usize;
            let mut end = start + 1;
            while end < self.nodes.len()
                && ((self.nodes[end].id as u64 & NODE_GROUP_MASK) >> NODE_CHUNK_SHIFT)
                    as usize
                    == chunk_id
            {
                end += 1;
            }
            set_mask_bit(&mut chunk_mask, chunk_id);
            chunks.push(encode_node_chunk(&self.nodes[start..end])?);
            start = end;
        }

        let mut payload = Vec::new();
        write_u64(&mut payload, group_id);
        payload.extend_from_slice(&chunk_mask);
        write_u16(
            &mut payload,
            u16::try_from(chunks.len()).map_err(|_| "too many node chunks".to_string())?,
        );
        write_u16(&mut payload, 0);
        let mut running = 0_usize;
        for chunk in &chunks {
            write_u32(&mut payload, checked_u32(running, "node chunk offset")?);
            running = running
                .checked_add(chunk.len())
                .ok_or_else(|| "node group size overflow".to_string())?;
        }
        write_u32(&mut payload, checked_u32(running, "node group length")?);
        for chunk in chunks {
            payload.extend_from_slice(&chunk);
        }

        let offset = self
            .file
            .stream_position()
            .map_err(|err| format!("position {}: {err}", self.data_path.display()))?;
        self.file
            .write_all(&payload)
            .map_err(|err| format!("write {}: {err}", self.data_path.display()))?;
        self.entries.push(GroupIndexEntry {
            group_id,
            offset,
            length: checked_u32(payload.len(), "node group")?,
        });
        self.nodes.clear();
        self.group_id = None;
        Ok(())
    }

    pub fn finish(mut self) -> Result<u64, String> {
        self.flush_group()?;
        self.file
            .sync_all()
            .map_err(|err| format!("sync {}: {err}", self.data_path.display()))?;
        write_index(&self.index_path, NODE_INDEX_MAGIC, &self.entries)?;
        Ok(self.count)
    }
}

fn encode_node_chunk(nodes: &[NodeCoord]) -> Result<Vec<u8>, String> {
    let first = nodes
        .first()
        .ok_or_else(|| "cannot encode an empty node chunk".to_string())?;
    let chunk_id = (first.id as u64 & NODE_GROUP_MASK) >> NODE_CHUNK_SHIFT;
    let mut mask = [0_u8; 32];
    for node in nodes {
        let node_chunk = (node.id as u64 & NODE_GROUP_MASK) >> NODE_CHUNK_SHIFT;
        if node_chunk != chunk_id {
            return Err("node chunk crosses a chunk boundary".to_string());
        }
        set_mask_bit(&mut mask, (node.id as u64 & NODE_CHUNK_MASK) as usize);
    }

    let mut output = Vec::new();
    output.extend_from_slice(&mask);
    write_u16(
        &mut output,
        u16::try_from(nodes.len()).map_err(|_| "node chunk count overflow".to_string())?,
    );
    write_u16(&mut output, 0);
    output.extend_from_slice(&first.x.to_le_bytes());
    output.extend_from_slice(&first.y.to_le_bytes());
    let mut x = first.x;
    let mut y = first.y;
    for node in &nodes[1..] {
        write_varint(
            zigzag_i64(
                node.x
                    .checked_sub(x)
                    .ok_or_else(|| "projected node x delta overflow".to_string())?,
            ),
            &mut output,
        );
        write_varint(
            zigzag_i64(
                node.y
                    .checked_sub(y)
                    .ok_or_else(|| "projected node y delta overflow".to_string())?,
            ),
            &mut output,
        );
        x = node.x;
        y = node.y;
    }
    Ok(output)
}

pub struct NodeStore {
    path: PathBuf,
    file: File,
    entries: StoreIndex,
    cache: FastHashMap<u64, CacheEntry<DecodedNodeGroup>>,
    cache_capacity: usize,
    cache_clock: u64,
    last_group: Option<u64>,
    // FIFO eviction: O(1) amortized. The old min_by_key full scan was
    // O(cache) per miss — with a Europe-sized cache that scan WAS the
    // pass-3 bottleneck.
    eviction_queue: std::collections::VecDeque<u64>,
}

impl NodeStore {
    pub fn open(data_path: &Path, index_path: &Path) -> Result<Self, String> {
        let mut file =
            File::open(data_path).map_err(|err| format!("open {}: {err}", data_path.display()))?;
        verify_data_magic(&mut file, data_path, NODE_DATA_MAGIC)?;
        Ok(Self {
            path: data_path.to_path_buf(),
            file,
            entries: StoreIndex::new(read_index(index_path, NODE_INDEX_MAGIC)?),
            cache: FastHashMap::default(),
            // ~0.75MB per decoded group; 16384 groups ~= 12GB. The old 256
            // (128MB) thrashed on Europe-scale way resolution — cache-miss
            // zlib re-decode was ~115us per way, the whole pass-3 cost.
            cache_capacity: node_cache_groups(),
            cache_clock: 0,
            last_group: None,
            eviction_queue: std::collections::VecDeque::new(),
        })
    }

    /// Same store with an explicit group-cache budget (parallel way
    /// resolution gives each worker its own slice of the total).
    pub fn open_with_cache(
        data_path: &Path,
        index_path: &Path,
        cache_groups: usize,
    ) -> Result<Self, String> {
        let mut store = Self::open(data_path, index_path)?;
        store.cache_capacity = cache_groups.max(64);
        Ok(store)
    }

    pub fn get(&mut self, id: i64) -> Result<Option<NodeCoord>, String> {
        if id < 0 {
            return Ok(None);
        }
        let group_id = id as u64 >> NODE_GROUP_SHIFT;
        let Some(entry) = self.entries.get(group_id) else {
            return Ok(None);
        };
        let group = self.group(entry)?;
        group.get(id)
    }

    fn group(&mut self, entry: GroupIndexEntry) -> Result<&DecodedNodeGroup, String> {
        self.cache_clock += 1;
        if self.last_group == Some(entry.group_id) {
            let cached = self.cache.get_mut(&entry.group_id).unwrap();
            cached.used = self.cache_clock;
            return Ok(&cached.value);
        }
        if self.cache.contains_key(&entry.group_id) {
            let cached = self.cache.get_mut(&entry.group_id).unwrap();
            cached.used = self.cache_clock;
            self.last_group = Some(entry.group_id);
            return Ok(&cached.value);
        }
        while self.cache.len() >= self.cache_capacity {
            let Some(candidate) = self.eviction_queue.pop_front() else {
                break;
            };
            self.cache.remove(&candidate);
        }
        self.file
            .seek(SeekFrom::Start(entry.offset))
            .map_err(|err| format!("seek {}: {err}", self.path.display()))?;
        let mut bytes = vec![0_u8; entry.length as usize];
        self.file
            .read_exact(&mut bytes)
            .map_err(|err| format!("read {}: {err}", self.path.display()))?;
        let group = decode_node_group(&bytes, entry.group_id)?;
        self.eviction_queue.push_back(entry.group_id);
        self.cache.insert(
            entry.group_id,
            CacheEntry {
                value: group,
                used: self.cache_clock,
            },
        );
        self.last_group = Some(entry.group_id);
        Ok(&self.cache.get(&entry.group_id).unwrap().value)
    }
}

#[derive(Debug)]
struct DecodedNodeChunk {
    positions: Box<[u16; 256]>,
    // Projected grid coords are <= 2^26 at zoom 14 (2^30 at zoom 18):
    // i32 is lossless and halves the biggest allocation. Decode errors
    // out-of-range instead of ever truncating.
    coordinates: Vec<(i32, i32)>,
}

#[derive(Debug)]
struct DecodedNodeGroup {
    group_id: u64,
    chunks: Vec<Option<DecodedNodeChunk>>,
}

impl DecodedNodeGroup {
    fn get(&self, id: i64) -> Result<Option<NodeCoord>, String> {
        if self.group_id != id as u64 >> NODE_GROUP_SHIFT {
            return Err("decoded node group id mismatch".to_string());
        }
        let chunk_id = ((id as u64 & NODE_GROUP_MASK) >> NODE_CHUNK_SHIFT) as usize;
        let Some(chunk) = &self.chunks[chunk_id] else {
            return Ok(None);
        };
        let local_id = (id as u64 & NODE_CHUNK_MASK) as usize;
        let rank = chunk.positions[local_id];
        if rank == u16::MAX {
            return Ok(None);
        }
        let &(x, y) = chunk
            .coordinates
            .get(rank as usize)
            .ok_or_else(|| "decoded node rank exceeds chunk length".to_string())?;
        Ok(Some(NodeCoord {
            id,
            x: i64::from(x),
            y: i64::from(y),
        }))
    }
}

fn decode_node_group(bytes: &[u8], expected_group_id: u64) -> Result<DecodedNodeGroup, String> {
    let mut offset = 0;
    let group_id = read_u64(bytes, &mut offset)?;
    if group_id != expected_group_id {
        return Err("node scratch group id mismatch".to_string());
    }
    let chunk_mask = take(bytes, &mut offset, 32)?;
    let chunk_count = read_u16(bytes, &mut offset)? as usize;
    let _reserved = read_u16(bytes, &mut offset)?;
    let offsets_base = offset;
    let data_base = offsets_base
        .checked_add((chunk_count + 1) * 4)
        .ok_or_else(|| "node scratch group offset overflow".to_string())?;
    let mut chunks = std::iter::repeat_with(|| None)
        .take(1 << (NODE_GROUP_SHIFT - NODE_CHUNK_SHIFT))
        .collect::<Vec<_>>();
    let mut chunk_rank = 0;
    for (chunk_id, slot) in chunks.iter_mut().enumerate() {
        if !mask_has_bit(chunk_mask, chunk_id) {
            continue;
        }
        if chunk_rank >= chunk_count {
            return Err("node scratch chunk rank exceeds chunk count".to_string());
        }
        let mut start_pos = offsets_base + chunk_rank * 4;
        let start = read_u32(bytes, &mut start_pos)? as usize;
        let end = read_u32(bytes, &mut start_pos)? as usize;
        if start > end {
            return Err("node scratch chunk offsets are reversed".to_string());
        }
        let chunk = bytes
            .get(data_base + start..data_base + end)
            .ok_or_else(|| "node scratch chunk lies outside its group".to_string())?;
        *slot = Some(decode_node_chunk(chunk)?);
        chunk_rank += 1;
    }
    if chunk_rank != chunk_count {
        return Err("node scratch chunk count does not match its mask".to_string());
    }
    Ok(DecodedNodeGroup { group_id, chunks })
}

fn decode_node_chunk(bytes: &[u8]) -> Result<DecodedNodeChunk, String> {
    let mut offset = 0;
    let node_mask: [u8; 32] = take(bytes, &mut offset, 32)?.try_into().unwrap();
    let count = read_u16(bytes, &mut offset)? as usize;
    let _reserved = read_u16(bytes, &mut offset)?;
    if count == 0 {
        return Err("node scratch chunk is empty".to_string());
    }
    if node_mask.iter().map(|byte| byte.count_ones() as usize).sum::<usize>() != count {
        return Err("node scratch chunk count does not match its mask".to_string());
    }
    let mut positions = Box::new([u16::MAX; 256]);
    let mut rank = 0_u16;
    for (local_id, position) in positions.iter_mut().enumerate() {
        if mask_has_bit(&node_mask, local_id) {
            *position = rank;
            rank += 1;
        }
    }
    let mut x = read_i64(bytes, &mut offset)?;
    let mut y = read_i64(bytes, &mut offset)?;
    let compact = |x: i64, y: i64| -> Result<(i32, i32), String> {
        Ok((
            i32::try_from(x).map_err(|_| "projected node x exceeds i32".to_string())?,
            i32::try_from(y).map_err(|_| "projected node y exceeds i32".to_string())?,
        ))
    };
    let mut coordinates = Vec::with_capacity(count);
    coordinates.push(compact(x, y)?);
    for _ in 1..count {
        x = x
            .checked_add(unzigzag_i64(read_varint(bytes, &mut offset)?))
            .ok_or_else(|| "decoded projected node x overflow".to_string())?;
        y = y
            .checked_add(unzigzag_i64(read_varint(bytes, &mut offset)?))
            .ok_or_else(|| "decoded projected node y overflow".to_string())?;
        coordinates.push(compact(x, y)?);
    }
    if offset != bytes.len() {
        return Err("node scratch chunk has trailing bytes".to_string());
    }
    Ok(DecodedNodeChunk {
        positions,
        coordinates,
    })
}

fn find_entry(entries: &[GroupIndexEntry], group_id: u64) -> Option<GroupIndexEntry> {
    entries
        .binary_search_by_key(&group_id, |entry| entry.group_id)
        .ok()
        .map(|index| entries[index])
}

pub struct WayStoreBuilder {
    data_path: PathBuf,
    index_path: PathBuf,
    file: File,
    entries: Vec<GroupIndexEntry>,
    group_id: Option<u64>,
    ways: Vec<(i64, Vec<i64>)>,
    last_id: Option<i64>,
    count: u64,
}

impl WayStoreBuilder {
    pub fn create(data_path: &Path, index_path: &Path) -> Result<Self, String> {
        Ok(Self {
            data_path: data_path.to_path_buf(),
            index_path: index_path.to_path_buf(),
            file: create_store_file(data_path, WAY_DATA_MAGIC)?,
            entries: Vec::new(),
            group_id: None,
            ways: Vec::new(),
            last_id: None,
            count: 0,
        })
    }

    pub fn push(&mut self, id: i64, refs: Vec<i64>) -> Result<(), String> {
        if id < 0 {
            return Err(format!("negative OSM way id {id} is unsupported"));
        }
        if self.last_id.is_some_and(|last| id <= last) {
            return Err(format!(
                "OSM ways are not strictly sorted by id: {id} follows {}",
                self.last_id.unwrap()
            ));
        }
        let group_id = id as u64 >> WAY_GROUP_SHIFT;
        if self.group_id.is_some_and(|current| current != group_id) {
            self.flush_group()?;
        }
        self.group_id = Some(group_id);
        self.ways.push((id, refs));
        self.last_id = Some(id);
        self.count += 1;
        Ok(())
    }

    fn flush_group(&mut self) -> Result<(), String> {
        let Some(group_id) = self.group_id else {
            return Ok(());
        };
        if self.ways.is_empty() {
            return Err("way scratch group is unexpectedly empty".to_string());
        }
        let mut mask = vec![0_u8; 1 << (WAY_GROUP_SHIFT - 3)];
        let mut records = Vec::<Vec<u8>>::with_capacity(self.ways.len());
        for (id, refs) in &self.ways {
            let local = (*id as u64 & WAY_GROUP_MASK) as usize;
            set_mask_bit(&mut mask, local);
            records.push(encode_way_refs(refs)?);
        }
        let mut payload = Vec::new();
        write_u64(&mut payload, group_id);
        payload.extend_from_slice(&mask);
        write_u32(
            &mut payload,
            u32::try_from(records.len()).map_err(|_| "too many ways in group".to_string())?,
        );
        let mut running = 0_usize;
        for record in &records {
            write_u32(&mut payload, checked_u32(running, "way record offset")?);
            running = running
                .checked_add(record.len())
                .ok_or_else(|| "way group size overflow".to_string())?;
        }
        write_u32(&mut payload, checked_u32(running, "way group length")?);
        for record in records {
            payload.extend_from_slice(&record);
        }

        let offset = self
            .file
            .stream_position()
            .map_err(|err| format!("position {}: {err}", self.data_path.display()))?;
        self.file
            .write_all(&payload)
            .map_err(|err| format!("write {}: {err}", self.data_path.display()))?;
        self.entries.push(GroupIndexEntry {
            group_id,
            offset,
            length: checked_u32(payload.len(), "way group")?,
        });
        self.ways.clear();
        self.group_id = None;
        Ok(())
    }

    pub fn finish(mut self) -> Result<u64, String> {
        self.flush_group()?;
        self.file
            .sync_all()
            .map_err(|err| format!("sync {}: {err}", self.data_path.display()))?;
        write_index(&self.index_path, WAY_INDEX_MAGIC, &self.entries)?;
        Ok(self.count)
    }
}

fn encode_way_refs(refs: &[i64]) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    write_varint(refs.len() as u64, &mut output);
    let mut previous = 0_i64;
    for &node_id in refs {
        let delta = node_id
            .checked_sub(previous)
            .ok_or_else(|| "way node id delta overflow".to_string())?;
        write_varint(zigzag_i64(delta), &mut output);
        previous = node_id;
    }
    Ok(output)
}

/// The relation-member way store fully decoded into RAM (mirrors
/// FlatNodeStore): relations reference ways randomly across the whole
/// extract, so the 128-group LRU thrashed pass 4 the same way pass 3 did.
pub struct FlatWayStore {
    groups: Vec<Option<DecodedWayGroup>>,
}

impl FlatWayStore {
    pub fn load(data_path: &Path, index_path: &Path) -> Result<Self, String> {
        let mut file = File::open(data_path)
            .map_err(|err| format!("open {}: {err}", data_path.display()))?;
        verify_data_magic(&mut file, data_path, WAY_DATA_MAGIC)?;
        drop(file);
        let entries = read_index(index_path, WAY_INDEX_MAGIC)?;
        let max_group = entries
            .iter()
            .map(|entry| entry.group_id)
            .max()
            .unwrap_or(0) as usize;
        let mut groups: Vec<Option<DecodedWayGroup>> = Vec::new();
        groups.resize_with(max_group + 1, || None);
        let workers = std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 16))
            .unwrap_or(4);
        let chunk_size = entries.len().div_ceil(workers).max(1);
        let decoded: Vec<Vec<(u64, DecodedWayGroup)>> =
            std::thread::scope(|scope| -> Result<_, String> {
                let mut handles = Vec::new();
                for chunk in entries.chunks(chunk_size) {
                    let data_path = data_path.to_path_buf();
                    handles.push(scope.spawn(move || -> Result<_, String> {
                        let mut file = File::open(&data_path)
                            .map_err(|err| format!("open {}: {err}", data_path.display()))?;
                        let mut out = Vec::with_capacity(chunk.len());
                        for entry in chunk {
                            file.seek(SeekFrom::Start(entry.offset)).map_err(|err| {
                                format!("seek {}: {err}", data_path.display())
                            })?;
                            let mut bytes = vec![0_u8; entry.length as usize];
                            file.read_exact(&mut bytes).map_err(|err| {
                                format!("read {}: {err}", data_path.display())
                            })?;
                            out.push((entry.group_id, decode_way_group(&bytes, entry.group_id)?));
                        }
                        Ok(out)
                    }));
                }
                let mut decoded = Vec::new();
                for handle in handles {
                    decoded.push(
                        handle
                            .join()
                            .map_err(|_| "way store load thread panicked".to_string())??,
                    );
                }
                Ok(decoded)
            })?;
        for part in decoded {
            for (group_id, group) in part {
                groups[group_id as usize] = Some(group);
            }
        }
        Ok(Self { groups })
    }

    pub fn get(&self, id: i64) -> Result<Option<&[i64]>, String> {
        if id < 0 {
            return Ok(None);
        }
        let group_id = (id as u64 >> WAY_GROUP_SHIFT) as usize;
        match self.groups.get(group_id).and_then(|group| group.as_ref()) {
            Some(group) => group.get(id),
            None => Ok(None),
        }
    }
}

pub struct WayStore {
    path: PathBuf,
    file: File,
    entries: StoreIndex,
    cache: FastHashMap<u64, CacheEntry<DecodedWayGroup>>,
    cache_capacity: usize,
    cache_clock: u64,
    last_group: Option<u64>,
}

impl WayStore {
    pub fn open(data_path: &Path, index_path: &Path) -> Result<Self, String> {
        let mut file =
            File::open(data_path).map_err(|err| format!("open {}: {err}", data_path.display()))?;
        verify_data_magic(&mut file, data_path, WAY_DATA_MAGIC)?;
        Ok(Self {
            path: data_path.to_path_buf(),
            file,
            entries: StoreIndex::new(read_index(index_path, WAY_INDEX_MAGIC)?),
            cache: FastHashMap::default(),
            cache_capacity: 128,
            cache_clock: 0,
            last_group: None,
        })
    }

    pub fn get(&mut self, id: i64) -> Result<Option<&[i64]>, String> {
        if id < 0 {
            return Ok(None);
        }
        let group_id = id as u64 >> WAY_GROUP_SHIFT;
        let Some(entry) = self.entries.get(group_id) else {
            return Ok(None);
        };
        let group = self.group(entry)?;
        group.get(id)
    }

    fn group(&mut self, entry: GroupIndexEntry) -> Result<&DecodedWayGroup, String> {
        self.cache_clock += 1;
        if self.last_group == Some(entry.group_id) {
            let cached = self.cache.get_mut(&entry.group_id).unwrap();
            cached.used = self.cache_clock;
            return Ok(&cached.value);
        }
        if self.cache.contains_key(&entry.group_id) {
            let cached = self.cache.get_mut(&entry.group_id).unwrap();
            cached.used = self.cache_clock;
            self.last_group = Some(entry.group_id);
            return Ok(&cached.value);
        }
        if self.cache.len() >= self.cache_capacity {
            let oldest = *self
                .cache
                .iter()
                .min_by_key(|(_, cached)| cached.used)
                .map(|(group_id, _)| group_id)
                .unwrap();
            self.cache.remove(&oldest);
        }
        self.file
            .seek(SeekFrom::Start(entry.offset))
            .map_err(|err| format!("seek {}: {err}", self.path.display()))?;
        let mut bytes = vec![0_u8; entry.length as usize];
        self.file
            .read_exact(&mut bytes)
            .map_err(|err| format!("read {}: {err}", self.path.display()))?;
        let group = decode_way_group(&bytes, entry.group_id)?;
        self.cache.insert(
            entry.group_id,
            CacheEntry {
                value: group,
                used: self.cache_clock,
            },
        );
        self.last_group = Some(entry.group_id);
        Ok(&self.cache.get(&entry.group_id).unwrap().value)
    }
}

#[derive(Debug)]
struct DecodedWayGroup {
    group_id: u64,
    ways: Vec<Option<Vec<i64>>>,
}

impl DecodedWayGroup {
    fn get(&self, id: i64) -> Result<Option<&[i64]>, String> {
        if self.group_id != id as u64 >> WAY_GROUP_SHIFT {
            return Err("decoded way group id mismatch".to_string());
        }
        Ok(self.ways[(id as u64 & WAY_GROUP_MASK) as usize].as_deref())
    }
}

fn decode_way_group(bytes: &[u8], expected_group_id: u64) -> Result<DecodedWayGroup, String> {
    let mut offset = 0;
    let group_id = read_u64(bytes, &mut offset)?;
    if group_id != expected_group_id {
        return Err("way scratch group id mismatch".to_string());
    }
    let mask_len = 1 << (WAY_GROUP_SHIFT - 3);
    let mask = take(bytes, &mut offset, mask_len)?;
    let count = read_u32(bytes, &mut offset)? as usize;
    let offsets_base = offset;
    let data_base = offsets_base
        .checked_add((count + 1) * 4)
        .ok_or_else(|| "way scratch group offset overflow".to_string())?;
    let mut ways = std::iter::repeat_with(|| None)
        .take(1 << WAY_GROUP_SHIFT)
        .collect::<Vec<_>>();
    let mut rank = 0;
    for (local_id, slot) in ways.iter_mut().enumerate() {
        if !mask_has_bit(mask, local_id) {
            continue;
        }
        if rank >= count {
            return Err("way scratch rank exceeds way count".to_string());
        }
        let mut start_pos = offsets_base + rank * 4;
        let start = read_u32(bytes, &mut start_pos)? as usize;
        let end = read_u32(bytes, &mut start_pos)? as usize;
        let record = bytes
            .get(data_base + start..data_base + end)
            .ok_or_else(|| "way scratch record lies outside its group".to_string())?;
        *slot = Some(decode_way_refs(record)?);
        rank += 1;
    }
    if rank != count {
        return Err("way scratch count does not match its mask".to_string());
    }
    Ok(DecodedWayGroup { group_id, ways })
}

fn decode_way_refs(bytes: &[u8]) -> Result<Vec<i64>, String> {
    let mut offset = 0;
    let count = usize::try_from(read_varint(bytes, &mut offset)?)
        .map_err(|_| "way node count exceeds usize".to_string())?;
    let mut refs = Vec::with_capacity(count);
    let mut previous = 0_i64;
    for _ in 0..count {
        previous = previous
            .checked_add(unzigzag_i64(read_varint(bytes, &mut offset)?))
            .ok_or_else(|| "decoded way node id overflow".to_string())?;
        refs.push(previous);
    }
    if offset != bytes.len() {
        return Err("way scratch record has trailing bytes".to_string());
    }
    Ok(refs)
}

pub struct PagedBitsetWriter {
    path: PathBuf,
    file: File,
    bytes: Vec<u8>,
}

impl PagedBitsetWriter {
    pub fn create(path: &Path) -> Result<Self, String> {
        if path.exists() {
            return Err(format!(
                "{} already exists; refusing to overwrite relation bitset",
                path.display()
            ));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("create {}: {err}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|err| format!("create {}: {err}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            bytes: Vec::new(),
        })
    }

    pub fn set(&mut self, id: i64) -> Result<(), String> {
        if id < 0 {
            return Err(format!("negative relation way id {id} is unsupported"));
        }
        let id = id as u64;
        let byte = usize::try_from(id >> 3)
            .map_err(|_| format!("relation way id {id} exceeds addressable memory"))?;
        if byte >= self.bytes.len() {
            let required = byte
                .checked_add(1)
                .ok_or_else(|| "relation way bitset length overflow".to_string())?;
            self.bytes.resize(required, 0);
        }
        self.bytes[byte] |= 1 << (id as usize & 7);
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), String> {
        self.file
            .write_all(&self.bytes)
            .map_err(|err| format!("write {}: {err}", self.path.display()))?;
        self.file
            .sync_all()
            .map_err(|err| format!("sync {}: {err}", self.path.display()))
    }
}

pub struct PagedBitset {
    path: PathBuf,
    file: File,
    len: u64,
    pages: VecDeque<CachedBitsetPage>,
    capacity: usize,
}

struct CachedBitsetPage {
    number: u64,
    bytes: Vec<u8>,
}

impl PagedBitset {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
        let len = file
            .metadata()
            .map_err(|err| format!("stat {}: {err}", path.display()))?
            .len();
        Ok(Self {
            path: path.to_path_buf(),
            file,
            len,
            pages: VecDeque::new(),
            capacity: 2,
        })
    }

    pub fn contains(&mut self, id: i64) -> Result<bool, String> {
        if id < 0 {
            return Ok(false);
        }
        let id = id as u64;
        let number = id / BITSET_PAGE_BITS;
        let offset = number * BITSET_PAGE_BYTES as u64;
        if offset >= self.len {
            return Ok(false);
        }
        let bit = (id % BITSET_PAGE_BITS) as usize;
        let bytes = self.page(number)?;
        Ok(bytes[bit >> 3] & (1 << (bit & 7)) != 0)
    }

    fn page(&mut self, number: u64) -> Result<&[u8], String> {
        if let Some(index) = self
            .pages
            .iter()
            .position(|cached| cached.number == number)
        {
            let cached = self.pages.remove(index).unwrap();
            self.pages.push_front(cached);
            return Ok(&self.pages.front().unwrap().bytes);
        }
        let offset = number * BITSET_PAGE_BYTES as u64;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|err| format!("seek {}: {err}", self.path.display()))?;
        let available = usize::try_from((self.len - offset).min(BITSET_PAGE_BYTES as u64)).unwrap();
        let mut bytes = vec![0_u8; BITSET_PAGE_BYTES];
        self.file
            .read_exact(&mut bytes[..available])
            .map_err(|err| format!("read {}: {err}", self.path.display()))?;
        self.pages.push_front(CachedBitsetPage {
            number,
            bytes,
        });
        while self.pages.len() > self.capacity {
            self.pages.pop_back();
        }
        Ok(&self.pages.front().unwrap().bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn node_store_round_trips_sparse_groups_and_deltas() {
        let dir = temp_dir("makepad-node-store");
        let data = dir.join("nodes.dat");
        let index = dir.join("nodes.idx");
        let nodes = [
            NodeCoord {
                id: 1,
                x: 10,
                y: 20,
            },
            NodeCoord {
                id: 255,
                x: -30,
                y: 41,
            },
            NodeCoord {
                id: 256,
                x: 1_000_000,
                y: -2_000_000,
            },
            NodeCoord {
                id: 65_535,
                x: 1_000_001,
                y: -2_000_005,
            },
            NodeCoord {
                id: 65_536,
                x: -180_000_000,
                y: 850_000_000,
            },
            NodeCoord {
                id: 131_100,
                x: 45,
                y: 90,
            },
        ];
        let mut builder = NodeStoreBuilder::create(&data, &index).unwrap();
        for node in nodes {
            builder.push(node).unwrap();
        }
        assert_eq!(builder.finish().unwrap(), nodes.len() as u64);

        let mut store = NodeStore::open(&data, &index).unwrap();
        for node in nodes {
            assert_eq!(store.get(node.id).unwrap(), Some(node));
        }
        for missing in [0, 2, 254, 257, 65_534, 65_537, 999_999] {
            assert_eq!(store.get(missing).unwrap(), None);
        }

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn way_store_round_trips_sparse_ids_and_reverse_refs() {
        let dir = temp_dir("makepad-way-store");
        let data = dir.join("ways.dat");
        let index = dir.join("ways.idx");
        let ways = [
            (7, vec![100, 104, 103, 9_000_000_000]),
            (4095, vec![2, 1, 2]),
            (4097, vec![45]),
            (20_000, Vec::new()),
        ];
        let mut builder = WayStoreBuilder::create(&data, &index).unwrap();
        for (id, refs) in &ways {
            builder.push(*id, refs.clone()).unwrap();
        }
        assert_eq!(builder.finish().unwrap(), ways.len() as u64);

        let mut store = WayStore::open(&data, &index).unwrap();
        for (id, refs) in &ways {
            assert_eq!(store.get(*id).unwrap(), Some(refs.as_slice()));
        }
        assert_eq!(store.get(8).unwrap(), None);
        assert_eq!(store.get(4096).unwrap(), None);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn paged_bitset_handles_sparse_large_ids() {
        let dir = temp_dir("makepad-paged-bitset");
        let path = dir.join("relations.bits");
        let ids = [0, 7, 32_767, 32_768, 4_000_000, 1_500_000_000];
        let mut writer = PagedBitsetWriter::create(&path).unwrap();
        for id in ids {
            writer.set(id).unwrap();
        }
        writer.finish().unwrap();

        let mut bitset = PagedBitset::open(&path).unwrap();
        for id in ids {
            assert!(bitset.contains(id).unwrap());
        }
        for id in [1, 8, 32_766, 32_769, 4_000_001, 1_499_999_999] {
            assert!(!bitset.contains(id).unwrap());
        }

        fs::remove_dir_all(dir).unwrap();
    }
}
