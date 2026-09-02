use super::geometry::TileKey;
use crate::makepad_draw::*;
use makepad_mbtile_reader::{mkmap_tile_id, BlobRef, MkmapLeaf, MkmapRoot};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const LEAF_CACHE_CAPACITY: usize = 32;
const DEFAULT_COALESCE_MAX_GAP: u64 = 64 * 1024;
const DEFAULT_COALESCE_MAX_LEN: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ReadToken(pub u64);

#[derive(Debug)]
pub struct ReadCompletion {
    pub token: ReadToken,
    pub result: Result<Vec<u8>, String>,
}

pub trait ByteSource {
    fn request_root(&mut self, cx: &mut Cx, token: ReadToken);
    fn request_range(
        &mut self,
        cx: &mut Cx,
        shard: u32,
        offset: u64,
        len: u64,
        token: ReadToken,
    );
    fn poll(&mut self, cx: &mut Cx, event: &Event) -> Vec<ReadCompletion>;
}

/// Local `.mkmap` reads performed by Makepad workers, never by the UI thread.
pub struct FileByteSource {
    dir: PathBuf,
    completions: ToUIReceiver<ReadCompletion>,
    workers: Option<TagThreadPool<ReadToken>>,
    shard_files: std::sync::Arc<std::sync::Mutex<HashMap<u32, std::fs::File>>>,
}

impl FileByteSource {
    pub fn new(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let dir = if path.file_name().is_some_and(|name| name == "root.mkidx") {
            path.parent().unwrap_or(Path::new(".")).to_path_buf()
        } else {
            path.to_path_buf()
        };
        Self {
            dir,
            completions: Default::default(),
            workers: None,
            shard_files: Default::default(),
        }
    }

    fn workers(&mut self, cx: &mut Cx) -> &TagThreadPool<ReadToken> {
        self.workers
            .get_or_insert_with(|| TagThreadPool::new(cx, 1))
    }
}

impl ByteSource for FileByteSource {
    fn request_root(&mut self, cx: &mut Cx, token: ReadToken) {
        let path = self.dir.join("root.mkidx");
        let sender = self.completions.sender();
        self.workers(cx).execute_rev(token, move |_| {
            let result = std::fs::read(&path)
                .map_err(|err| format!("read {}: {err}", path.display()));
            let _ = sender.send(ReadCompletion { token, result });
        });
    }

    fn request_range(
        &mut self,
        cx: &mut Cx,
        shard: u32,
        offset: u64,
        len: u64,
        token: ReadToken,
    ) {
        let path = self.dir.join(format!("tiles-{shard:03}.mkshard"));
        let sender = self.completions.sender();
        let shard_files = self.shard_files.clone();
        self.workers(cx).execute_rev(token, move |_| {
            let result = read_file_range(&path, shard, offset, len, &shard_files);
            let _ = sender.send(ReadCompletion { token, result });
        });
    }

    fn poll(&mut self, _cx: &mut Cx, _event: &Event) -> Vec<ReadCompletion> {
        let mut out = Vec::new();
        while let Ok(completion) = self.completions.try_recv() {
            out.push(completion);
        }
        out
    }
}

fn read_file_range(
    path: &Path,
    shard: u32,
    offset: u64,
    len: u64,
    shard_files: &std::sync::Mutex<HashMap<u32, std::fs::File>>,
) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek, SeekFrom};

    let len = usize::try_from(len).map_err(|_| "mkmap range is too large".to_string())?;
    let mut files = shard_files
        .lock()
        .map_err(|_| "mkmap shard file cache lock poisoned".to_string())?;
    if !files.contains_key(&shard) {
        files.insert(
            shard,
            std::fs::File::open(path)
                .map_err(|err| format!("open {}: {err}", path.display()))?,
        );
    }
    let file = files.get_mut(&shard).unwrap();
    file.seek(SeekFrom::Start(offset))
        .map_err(|err| format!("seek {}: {err}", path.display()))?;
    let mut bytes = vec![0; len];
    file.read_exact(&mut bytes)
        .map_err(|err| format!("read {}: {err}", path.display()))?;
    Ok(bytes)
}

#[derive(Clone, Copy, Debug)]
enum HttpReadKind {
    Root,
    Range {
        shard: u32,
        offset: u64,
        len: u64,
    },
}

#[derive(Clone, Debug)]
struct PendingHttpRead {
    token: ReadToken,
    kind: HttpReadKind,
    retry_count: u8,
}

/// HTTP `.mkmap` source using one whole-root GET and strict shard ranges.
pub struct HttpRangeByteSource {
    root_url: String,
    requests: HashMap<LiveId, PendingHttpRead>,
    ready: VecDeque<ReadCompletion>,
}

impl HttpRangeByteSource {
    pub fn new(root_url: impl Into<String>) -> Self {
        Self {
            root_url: root_url.into().trim_end_matches('/').to_string(),
            requests: HashMap::new(),
            ready: VecDeque::new(),
        }
    }

    fn issue(&mut self, cx: &mut Cx, pending: PendingHttpRead) {
        static NEXT_HTTP_ARCHIVE_ID: AtomicU64 = AtomicU64::new(0x4d4b_0000_0000_0001);
        let request_id = LiveId(NEXT_HTTP_ARCHIVE_ID.fetch_add(1, Ordering::Relaxed));
        let (url, range) = match pending.kind {
            HttpReadKind::Root => (format!("{}/root.mkidx", self.root_url), None),
            HttpReadKind::Range {
                shard,
                offset,
                len,
            } => {
                let Some(end) = offset
                    .checked_add(len)
                    .filter(|_| len != 0)
                    .and_then(|end| end.checked_sub(1))
                else {
                    self.ready.push_back(ReadCompletion {
                        token: pending.token,
                        result: Err("invalid mkmap HTTP range".to_string()),
                    });
                    return;
                };
                (
                    format!("{}/tiles-{shard:03}.mkshard", self.root_url),
                    Some(format!("bytes={offset}-{end}")),
                )
            }
        };
        let mut request = HttpRequest::new(url, HttpMethod::GET);
        request.set_header("Accept".to_string(), "application/octet-stream".to_string());
        if let Some(range) = range {
            request.set_header("Range".to_string(), range);
        }
        self.requests.insert(request_id, pending);
        cx.http_request(request_id, request);
    }

    fn content_range(response: &HttpResponse) -> Option<&str> {
        response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-range"))
            .and_then(|(_, values)| values.first())
            .map(String::as_str)
    }
}

impl ByteSource for HttpRangeByteSource {
    fn request_root(&mut self, cx: &mut Cx, token: ReadToken) {
        self.issue(
            cx,
            PendingHttpRead {
                token,
                kind: HttpReadKind::Root,
                retry_count: 0,
            },
        );
    }

    fn request_range(
        &mut self,
        cx: &mut Cx,
        shard: u32,
        offset: u64,
        len: u64,
        token: ReadToken,
    ) {
        self.issue(
            cx,
            PendingHttpRead {
                token,
                kind: HttpReadKind::Range {
                    shard,
                    offset,
                    len,
                },
                retry_count: 0,
            },
        );
    }

    fn poll(&mut self, cx: &mut Cx, event: &Event) -> Vec<ReadCompletion> {
        let mut out: Vec<ReadCompletion> = self.ready.drain(..).collect();
        let Event::NetworkResponses(responses) = event else {
            return out;
        };
        for network_response in responses {
            match network_response {
                NetworkResponse::HttpResponse {
                    request_id,
                    response,
                } => {
                    let Some(pending) = self.requests.remove(request_id) else {
                        continue;
                    };
                    out.push(ReadCompletion {
                        token: pending.token,
                        result: validate_http_response(&pending.kind, response),
                    });
                }
                NetworkResponse::HttpError { request_id, error } => {
                    let Some(mut pending) = self.requests.remove(request_id) else {
                        continue;
                    };
                    if pending.retry_count == 0 {
                        pending.retry_count = 1;
                        self.issue(cx, pending);
                    } else {
                        out.push(ReadCompletion {
                            token: pending.token,
                            result: Err(format!("mkmap HTTP transport: {}", error.message)),
                        });
                    }
                }
                _ => {}
            }
        }
        out
    }
}

fn validate_http_response(kind: &HttpReadKind, response: &HttpResponse) -> Result<Vec<u8>, String> {
    match *kind {
        HttpReadKind::Root => {
            if response.status_code != 200 {
                return Err(format!(
                    "mkmap root requires HTTP 200, got {}",
                    response.status_code
                ));
            }
        }
        HttpReadKind::Range { offset, len, .. } => {
            if response.status_code != 206 {
                return Err(format!(
                    "mkmap shard range requires HTTP 206, got {}",
                    response.status_code
                ));
            }
            let end = offset
                .checked_add(len)
                .and_then(|end| end.checked_sub(1))
                .ok_or_else(|| "invalid mkmap HTTP range".to_string())?;
            let content_range = HttpRangeByteSource::content_range(response)
                .ok_or_else(|| "mkmap shard response has no Content-Range".to_string())?;
            let matched = content_range
                .trim()
                .strip_prefix("bytes ")
                .and_then(|value| value.split_once('/'))
                .and_then(|(range, total)| {
                    let (start, finish) = range.split_once('-')?;
                    let start = start.parse::<u64>().ok()?;
                    let finish = finish.parse::<u64>().ok()?;
                    let total_matches = total == "*"
                        || total
                            .parse::<u64>()
                            .ok()
                            .is_some_and(|total| total > finish);
                    Some(start == offset && finish == end && total_matches)
                })
                .unwrap_or(false);
            if !matched {
                return Err(format!(
                    "mkmap Content-Range mismatch: expected bytes {offset}-{end}/TOTAL, got {content_range}"
                ));
            }
            let body_len = response.body.as_ref().map_or(0, Vec::len);
            if body_len as u64 != len {
                return Err(format!(
                    "mkmap shard response length mismatch: expected {len}, got {}",
                    body_len
                ));
            }
        }
    }
    Ok(response.body.clone().unwrap_or_default())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileBytesResult {
    Bytes(Vec<u8>),
    Missing,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileBytes {
    pub key: TileKey,
    pub generation: u64,
    pub result: TileBytesResult,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitStage {
    Root,
    Leaf(usize),
    Blob,
}

#[derive(Clone, Copy, Debug)]
struct TileWaiter {
    generation: u64,
    stage: WaitStage,
}

#[derive(Clone, Copy, Debug)]
enum PendingRead {
    Root { generation: u64 },
    Leaf { generation: u64, index: usize },
    Blob { generation: u64, key: TileKey },
}

#[derive(Clone, Debug)]
struct CachedLeaf {
    leaf: MkmapLeaf,
    used: u64,
}

/// Persistent archive lookup state: root, LRU leaves, in-flight reads and
/// per-tile waiters are all shared across requests.
pub struct TileArchive<S: ByteSource> {
    source: S,
    root: Option<MkmapRoot>,
    root_error: Option<String>,
    root_in_flight: Option<ReadToken>,
    leaves: HashMap<usize, CachedLeaf>,
    leaf_lru_clock: u64,
    leaf_in_flight: HashMap<usize, ReadToken>,
    waiters: HashMap<TileKey, TileWaiter>,
    pending_reads: HashMap<ReadToken, PendingRead>,
    ready: VecDeque<TileBytes>,
    generation: Option<u64>,
    next_token: u64,
}

impl<S: ByteSource> TileArchive<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            root: None,
            root_error: None,
            root_in_flight: None,
            leaves: HashMap::new(),
            leaf_lru_clock: 0,
            leaf_in_flight: HashMap::new(),
            waiters: HashMap::new(),
            pending_reads: HashMap::new(),
            ready: VecDeque::new(),
            generation: None,
            next_token: 1,
        }
    }

    pub fn zoom_range(&self) -> Option<(u32, u32)> {
        self.root.as_ref().map(MkmapRoot::zoom_range)
    }

    pub fn metadata(&self) -> Option<&HashMap<String, String>> {
        self.root.as_ref().map(MkmapRoot::metadata)
    }

    pub fn request_tile(&mut self, cx: &mut Cx, key: TileKey, generation: u64) {
        if self.generation.is_some_and(|current| generation < current) {
            return;
        }
        if self.generation != Some(generation) {
            self.reset_generation(generation);
        }
        if self.waiters.contains_key(&key) {
            return;
        }
        if let Some(error) = self.root_error.clone() {
            self.finish(key, generation, TileBytesResult::Error(error));
            cx.redraw_all();
            return;
        }
        self.waiters.insert(
            key,
            TileWaiter {
                generation,
                stage: WaitStage::Root,
            },
        );
        if self.root.is_none() {
            self.ensure_root(cx, generation);
        } else {
            let ready_len = self.ready.len();
            self.advance_waiters(cx);
            if self.ready.len() != ready_len {
                cx.redraw_all();
            }
        }
    }

    pub fn drain(&mut self, cx: &mut Cx, event: &Event) -> Vec<TileBytes> {
        for completion in self.source.poll(cx, event) {
            let Some(pending) = self.pending_reads.remove(&completion.token) else {
                continue;
            };
            let pending_generation = match pending {
                PendingRead::Root { generation }
                | PendingRead::Leaf { generation, .. }
                | PendingRead::Blob { generation, .. } => generation,
            };
            if self.generation != Some(pending_generation) {
                continue;
            }
            match pending {
                PendingRead::Root { generation } => {
                    self.root_in_flight = None;
                    match completion.result.and_then(|bytes| MkmapRoot::parse(&bytes)) {
                        Ok(root) => self.root = Some(root),
                        Err(error) => {
                            self.root_error = Some(error.clone());
                            self.fail_waiters(generation, |_| true, error);
                        }
                    }
                }
                PendingRead::Leaf {
                    generation,
                    index,
                } => {
                    self.leaf_in_flight.remove(&index);
                    match completion.result.and_then(|bytes| MkmapLeaf::parse(&bytes)) {
                        Ok(leaf) => self.insert_leaf(index, leaf),
                        Err(error) => self.fail_waiters(
                            generation,
                            |waiter| waiter.stage == WaitStage::Leaf(index),
                            error,
                        ),
                    }
                }
                PendingRead::Blob { generation, key } => match completion.result {
                    Ok(bytes) => {
                        let result = self
                            .root
                            .as_ref()
                            .ok_or_else(|| "mkmap root disappeared".to_string())
                            .and_then(|root| root.decode_blob(&bytes));
                        match result {
                            Ok(bytes) => self.finish(key, generation, TileBytesResult::Bytes(bytes)),
                            Err(error) => {
                                self.finish(key, generation, TileBytesResult::Error(error))
                            }
                        }
                    }
                    Err(error) => self.finish(key, generation, TileBytesResult::Error(error)),
                },
            }
            self.advance_waiters(cx);
        }
        self.ready.drain(..).collect()
    }

    fn reset_generation(&mut self, generation: u64) {
        self.root = None;
        self.root_error = None;
        self.root_in_flight = None;
        self.leaves.clear();
        self.leaf_in_flight.clear();
        self.waiters.clear();
        self.pending_reads.clear();
        self.ready.clear();
        self.generation = Some(generation);
    }

    fn token(&mut self) -> ReadToken {
        let token = ReadToken(self.next_token);
        self.next_token = self.next_token.wrapping_add(1).max(1);
        token
    }

    fn ensure_root(&mut self, cx: &mut Cx, generation: u64) {
        if self.root_in_flight.is_some() {
            return;
        }
        let token = self.token();
        self.root_in_flight = Some(token);
        self.pending_reads
            .insert(token, PendingRead::Root { generation });
        self.source.request_root(cx, token);
    }

    fn advance_waiters(&mut self, cx: &mut Cx) {
        if self.root.is_none() {
            return;
        }
        let keys: Vec<TileKey> = self.waiters.keys().copied().collect();
        for key in keys {
            let Some(waiter) = self.waiters.get(&key).copied() else {
                continue;
            };
            if waiter.stage == WaitStage::Blob {
                continue;
            }
            let Some(tile_id) = tile_id_for_key(key) else {
                self.finish(
                    key,
                    waiter.generation,
                    TileBytesResult::Error("tile coordinate is outside mkmap bounds".to_string()),
                );
                continue;
            };
            let Some(record) = self.root.as_ref().and_then(|root| root.locate(tile_id)) else {
                self.finish(key, waiter.generation, TileBytesResult::Missing);
                continue;
            };
            let cached = if let Some(cached) = self.leaves.get_mut(&record.index) {
                self.leaf_lru_clock = self.leaf_lru_clock.wrapping_add(1);
                cached.used = self.leaf_lru_clock;
                Some(cached.leaf.find(tile_id))
            } else {
                None
            };
            match cached {
                Some(Some(blob)) => {
                    let token = self.token();
                    self.pending_reads.insert(
                        token,
                        PendingRead::Blob {
                            generation: waiter.generation,
                            key,
                        },
                    );
                    self.waiters.get_mut(&key).unwrap().stage = WaitStage::Blob;
                    self.source.request_range(
                        cx,
                        blob.shard,
                        blob.offset,
                        blob.len,
                        token,
                    );
                }
                Some(None) => self.finish(key, waiter.generation, TileBytesResult::Missing),
                None => {
                    self.waiters.get_mut(&key).unwrap().stage = WaitStage::Leaf(record.index);
                    if !self.leaf_in_flight.contains_key(&record.index) {
                        let token = self.token();
                        self.leaf_in_flight.insert(record.index, token);
                        self.pending_reads.insert(
                            token,
                            PendingRead::Leaf {
                                generation: waiter.generation,
                                index: record.index,
                            },
                        );
                        self.source.request_range(
                            cx,
                            record.shard,
                            record.dir_offset,
                            record.dir_len,
                            token,
                        );
                    }
                }
            }
        }
    }

    fn insert_leaf(&mut self, index: usize, leaf: MkmapLeaf) {
        self.leaf_lru_clock = self.leaf_lru_clock.wrapping_add(1);
        self.leaves.insert(
            index,
            CachedLeaf {
                leaf,
                used: self.leaf_lru_clock,
            },
        );
        if self.leaves.len() > LEAF_CACHE_CAPACITY {
            if let Some(oldest) = self
                .leaves
                .iter()
                .min_by_key(|(_, cached)| cached.used)
                .map(|(index, _)| *index)
            {
                self.leaves.remove(&oldest);
            }
        }
    }

    fn fail_waiters(
        &mut self,
        generation: u64,
        mut matches: impl FnMut(&TileWaiter) -> bool,
        error: String,
    ) {
        let keys: Vec<TileKey> = self
            .waiters
            .iter()
            .filter(|(_, waiter)| waiter.generation == generation && matches(waiter))
            .map(|(key, _)| *key)
            .collect();
        for key in keys {
            self.finish(key, generation, TileBytesResult::Error(error.clone()));
        }
    }

    fn finish(&mut self, key: TileKey, generation: u64, result: TileBytesResult) {
        self.waiters.remove(&key);
        self.ready.push_back(TileBytes {
            key,
            generation,
            result,
        });
    }
}

fn tile_id_for_key(key: TileKey) -> Option<u64> {
    if key.z > 30 || key.x < 0 || key.y < 0 {
        return None;
    }
    let axis = 1_u64 << key.z;
    if key.x as u64 >= axis || key.y as u64 >= axis {
        return None;
    }
    Some(mkmap_tile_id(key.z as u8, key.x as u32, key.y as u32))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoalescedRange {
    pub shard: u32,
    pub offset: u64,
    pub len: u64,
}

/// Bounded range-coalescing hook. It is deliberately not wired into archive
/// dispatch until real hosted-tile measurements justify a gap policy.
pub fn coalesce_refs(refs: &[BlobRef]) -> Vec<CoalescedRange> {
    coalesce_refs_bounded(
        refs,
        DEFAULT_COALESCE_MAX_GAP,
        DEFAULT_COALESCE_MAX_LEN,
    )
}

fn coalesce_refs_bounded(refs: &[BlobRef], max_gap: u64, max_len: u64) -> Vec<CoalescedRange> {
    let mut refs = refs.to_vec();
    refs.sort_unstable_by_key(|blob| (blob.shard, blob.offset));
    let mut out = Vec::<CoalescedRange>::new();
    for blob in refs {
        let Some(blob_end) = blob.offset.checked_add(blob.len) else {
            continue;
        };
        if let Some(last) = out.last_mut() {
            let last_end = last.offset.saturating_add(last.len);
            let merged_len = blob_end.saturating_sub(last.offset);
            if last.shard == blob.shard
                && blob.offset <= last_end.saturating_add(max_gap)
                && merged_len <= max_len
            {
                last.len = last.len.max(merged_len);
                continue;
            }
        }
        if blob.len <= max_len {
            out.push(CoalescedRange {
                shard: blob.shard,
                offset: blob.offset,
                len: blob.len,
            });
        }
    }
    out
}

pub enum MapTileArchive {
    File(TileArchive<FileByteSource>),
    Http(TileArchive<HttpRangeByteSource>),
}

impl MapTileArchive {
    pub fn file(path: impl AsRef<Path>) -> Self {
        Self::File(TileArchive::new(FileByteSource::new(path)))
    }

    pub fn http(root_url: impl Into<String>) -> Self {
        Self::Http(TileArchive::new(HttpRangeByteSource::new(root_url)))
    }

    pub fn request_tile(&mut self, cx: &mut Cx, key: TileKey, generation: u64) {
        match self {
            Self::File(archive) => archive.request_tile(cx, key, generation),
            Self::Http(archive) => archive.request_tile(cx, key, generation),
        }
    }

    pub fn drain(&mut self, cx: &mut Cx, event: &Event) -> Vec<TileBytes> {
        match self {
            Self::File(archive) => archive.drain(cx, event),
            Self::Http(archive) => archive.drain(cx, event),
        }
    }

    pub fn zoom_range(&self) -> Option<(u32, u32)> {
        match self {
            Self::File(archive) => archive.zoom_range(),
            Self::Http(archive) => archive.zoom_range(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Clone, Copy, Debug)]
    enum MockRequest {
        Root(ReadToken),
        Range {
            shard: u32,
            offset: u64,
            len: u64,
            token: ReadToken,
        },
    }

    #[derive(Default)]
    struct MockByteSource {
        requests: Vec<MockRequest>,
        completions: VecDeque<ReadCompletion>,
    }

    impl ByteSource for MockByteSource {
        fn request_root(&mut self, _cx: &mut Cx, token: ReadToken) {
            self.requests.push(MockRequest::Root(token));
        }

        fn request_range(
            &mut self,
            _cx: &mut Cx,
            shard: u32,
            offset: u64,
            len: u64,
            token: ReadToken,
        ) {
            self.requests.push(MockRequest::Range {
                shard,
                offset,
                len,
                token,
            });
        }

        fn poll(&mut self, _cx: &mut Cx, _event: &Event) -> Vec<ReadCompletion> {
            self.completions.drain(..).collect()
        }
    }

    fn write_varint(mut value: u64, out: &mut Vec<u8>) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                return;
            }
        }
    }

    fn brotli(raw: &[u8]) -> Vec<u8> {
        makepad_mbtile_reader::compress_tile(
            &makepad_mbtile_reader::TileCompression::Brotli { quality: 5 },
            None,
            raw,
        )
        .unwrap()
    }

    fn tiny_parts(keys: [TileKey; 2]) -> (Vec<u8>, Vec<u8>) {
        let ids = keys.map(|key| mkmap_tile_id(key.z as u8, key.x as u32, key.y as u32));
        assert!(ids[0] < ids[1]);
        let mut leaf_raw = Vec::new();
        write_varint(2, &mut leaf_raw);
        write_varint(ids[0], &mut leaf_raw);
        write_varint(0, &mut leaf_raw);
        write_varint(200, &mut leaf_raw);
        write_varint(3, &mut leaf_raw);
        write_varint(ids[1] - ids[0], &mut leaf_raw);
        write_varint(0, &mut leaf_raw);
        write_varint(300, &mut leaf_raw);
        write_varint(3, &mut leaf_raw);
        let leaf = brotli(&leaf_raw);

        let metadata = [("compression", "gzip"), ("minzoom", "1"), ("maxzoom", "1")];
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
        root_raw.extend_from_slice(&ids[0].to_le_bytes());
        root_raw.extend_from_slice(&ids[1].to_le_bytes());
        root_raw.extend_from_slice(&0_u32.to_le_bytes());
        root_raw.extend_from_slice(&100_u64.to_le_bytes());
        root_raw.extend_from_slice(&(leaf.len() as u64).to_le_bytes());
        let root_copy = brotli(&root_raw);
        let mut root = vec![0_u8; 112];
        root[0..8].copy_from_slice(b"MKMAPIX1");
        root[8..12].copy_from_slice(&2_u32.to_le_bytes());
        root[12..16].copy_from_slice(&1_u32.to_le_bytes());
        root[24..32].copy_from_slice(&2_u64.to_le_bytes());
        root[40] = 1;
        root[41] = 1;
        let mut cursor = 112_u64;
        for (slot, len) in [
            (48, metadata.len() as u64),
            (64, 0),
            (80, root_raw.len() as u64),
            (96, root_copy.len() as u64),
        ] {
            root[slot..slot + 8].copy_from_slice(&cursor.to_le_bytes());
            root[slot + 8..slot + 16].copy_from_slice(&len.to_le_bytes());
            cursor += len;
        }
        root.extend_from_slice(&metadata);
        root.extend_from_slice(&root_raw);
        root.extend_from_slice(&root_copy);
        (root, leaf)
    }

    #[test]
    fn range_response_requires_exact_206() {
        let kind = HttpReadKind::Range {
            shard: 0,
            offset: 10,
            len: 4,
        };
        let response = |status, body: &[u8]| HttpResponse {
            metadata_id: LiveId::empty(),
            status_code: status,
            headers: BTreeMap::from([(
                "Content-Range".to_string(),
                vec!["bytes 10-13/100".to_string()],
            )]),
            body: Some(body.to_vec()),
        };
        assert!(validate_http_response(&kind, &response(200, b"abcd")).is_err());
        assert!(validate_http_response(&kind, &response(206, b"abc")).is_err());
        assert_eq!(
            validate_http_response(&kind, &response(206, b"abcd")).unwrap(),
            b"abcd"
        );
    }

    #[test]
    fn coalescing_is_bounded() {
        let refs = [
            BlobRef {
                shard: 0,
                offset: 0,
                len: 10,
            },
            BlobRef {
                shard: 0,
                offset: 12,
                len: 10,
            },
            BlobRef {
                shard: 1,
                offset: 0,
                len: 10,
            },
        ];
        assert_eq!(coalesce_refs_bounded(&refs, 2, 32).len(), 2);
        assert_eq!(coalesce_refs_bounded(&refs, 1, 32).len(), 3);
    }

    #[test]
    fn archive_deduplicates_root_and_leaf_and_tolerates_duplicate_out_of_order_reads() {
        let keys = [
            TileKey { z: 1, x: 0, y: 0 },
            TileKey { z: 1, x: 0, y: 1 },
        ];
        let (root, leaf) = tiny_parts(keys);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut archive = TileArchive::new(MockByteSource::default());
        archive.request_tile(&mut cx, keys[0], 7);
        archive.request_tile(&mut cx, keys[1], 7);
        assert_eq!(archive.source.requests.len(), 1);
        let MockRequest::Root(root_token) = archive.source.requests[0] else {
            panic!("first request was not root")
        };
        archive.source.completions.push_back(ReadCompletion {
            token: root_token,
            result: Ok(root.clone()),
        });
        archive.source.completions.push_back(ReadCompletion {
            token: root_token,
            result: Ok(root),
        });
        assert!(archive.drain(&mut cx, &Event::Startup).is_empty());
        assert_eq!(archive.source.requests.len(), 2);
        let MockRequest::Range {
            shard,
            offset,
            len,
            token: leaf_token,
        } = archive.source.requests[1]
        else {
            panic!("second request was not a leaf")
        };
        assert_eq!((shard, offset, len), (0, 100, leaf.len() as u64));
        archive.source.completions.push_back(ReadCompletion {
            token: leaf_token,
            result: Ok(leaf),
        });
        assert!(archive.drain(&mut cx, &Event::Startup).is_empty());
        assert_eq!(archive.source.requests.len(), 4);

        let blob_requests: Vec<(ReadToken, u64)> = archive.source.requests[2..]
            .iter()
            .map(|request| match *request {
                MockRequest::Range { token, offset, .. } => (token, offset),
                MockRequest::Root(_) => panic!("blob request was root"),
            })
            .collect();
        for (token, offset) in blob_requests.iter().rev() {
            archive.source.completions.push_back(ReadCompletion {
                token: *token,
                result: Ok(if *offset == 200 { b"one" } else { b"two" }.to_vec()),
            });
        }
        archive.source.completions.push_back(ReadCompletion {
            token: blob_requests[0].0,
            result: Ok(b"duplicate".to_vec()),
        });
        let done = archive.drain(&mut cx, &Event::Startup);
        assert_eq!(done.len(), 2);
        assert!(done.iter().any(|tile| tile.key == keys[0]));
        assert!(done.iter().any(|tile| tile.key == keys[1]));
    }

    #[test]
    fn archive_drops_stale_generation_completions() {
        let keys = [
            TileKey { z: 1, x: 0, y: 0 },
            TileKey { z: 1, x: 0, y: 1 },
        ];
        let (root, _) = tiny_parts(keys);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut archive = TileArchive::new(MockByteSource::default());
        archive.request_tile(&mut cx, keys[0], 1);
        let MockRequest::Root(stale_token) = archive.source.requests[0] else {
            panic!("first request was not root")
        };
        archive.request_tile(&mut cx, keys[1], 2);
        archive.source.completions.push_back(ReadCompletion {
            token: stale_token,
            result: Ok(root),
        });
        assert!(archive.drain(&mut cx, &Event::Startup).is_empty());
        assert_eq!(archive.source.requests.len(), 2);
    }
}
