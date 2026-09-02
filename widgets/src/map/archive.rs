use super::geometry::TileKey;
use crate::makepad_draw::*;
use makepad_mbtile_reader::{mkmap_tile_id, BlobRef, MkmapLeaf, MkmapRoot};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const LEAF_CACHE_CAPACITY: usize = 32;
const LEAF_CACHE_BYTE_CAPACITY: usize = 64 * 1024 * 1024;
const FILE_CACHE_CAPACITY: usize = 8;
const MAX_ARCHIVE_WAITERS: usize = 64;
const MAX_ARCHIVE_IN_FLIGHT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ROOT_BYTES: usize = 64 * 1024 * 1024;
const MAX_RANGE_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_COALESCE_MAX_GAP: u64 = 64 * 1024;
const DEFAULT_COALESCE_MAX_LEN: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ReadToken(pub u64);

pub type ArchiveWorkerPool = Arc<TagThreadPool<ReadToken>>;

pub fn new_archive_worker_pool(cx: &mut Cx) -> ArchiveWorkerPool {
    Arc::new(TagThreadPool::new(cx, 2))
}

pub fn next_archive_task_token() -> ReadToken {
    static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
    ReadToken(NEXT_TOKEN.fetch_add(1, Ordering::Relaxed).max(1))
}

#[derive(Debug)]
pub struct ReadCompletion {
    pub token: ReadToken,
    pub result: Result<Arc<[u8]>, String>,
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
    fn cancel(&mut self, cx: &mut Cx, token: ReadToken);
    fn poll(&mut self, cx: &mut Cx, event: &Event) -> Vec<ReadCompletion>;
}

#[derive(Default)]
struct ShardFileCache {
    files: HashMap<u32, std::fs::File>,
    lru: VecDeque<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileReadState {
    Queued,
    Running,
    Completed,
    Cancelled,
}

/// Local `.mkmap` reads performed by Makepad workers, never by the UI thread.
pub struct FileByteSource {
    dir: PathBuf,
    completions: ToUIReceiver<ReadCompletion>,
    workers: ArchiveWorkerPool,
    shard_files: Arc<Mutex<ShardFileCache>>,
    token_states: Arc<Mutex<HashMap<ReadToken, FileReadState>>>,
    #[cfg(test)]
    completion_barriers: Option<(
        Arc<std::sync::Barrier>,
        Arc<std::sync::Barrier>,
    )>,
}

impl FileByteSource {
    pub fn new(path: impl AsRef<Path>, workers: ArchiveWorkerPool) -> Self {
        let path = path.as_ref();
        let dir = if path.file_name().is_some_and(|name| name == "root.mkidx") {
            path.parent().unwrap_or(Path::new(".")).to_path_buf()
        } else {
            path.to_path_buf()
        };
        Self {
            dir,
            completions: Default::default(),
            workers,
            shard_files: Default::default(),
            token_states: Default::default(),
            #[cfg(test)]
            completion_barriers: None,
        }
    }
}

fn begin_file_read(
    states: &Mutex<HashMap<ReadToken, FileReadState>>,
    token: ReadToken,
) -> bool {
    let Ok(mut states) = states.lock() else {
        return false;
    };
    match states.get_mut(&token) {
        Some(state @ FileReadState::Queued) => {
            *state = FileReadState::Running;
            true
        }
        Some(FileReadState::Cancelled) => {
            states.remove(&token);
            false
        }
        _ => false,
    }
}

fn complete_file_read(
    states: &Mutex<HashMap<ReadToken, FileReadState>>,
    token: ReadToken,
) -> bool {
    let Ok(mut states) = states.lock() else {
        return false;
    };
    match states.get_mut(&token) {
        Some(state @ FileReadState::Running) => {
            *state = FileReadState::Completed;
            states.remove(&token);
            true
        }
        Some(FileReadState::Cancelled) => {
            states.remove(&token);
            false
        }
        _ => false,
    }
}

impl ByteSource for FileByteSource {
    fn request_root(&mut self, _cx: &mut Cx, token: ReadToken) {
        let path = self.dir.join("root.mkidx");
        let sender = self.completions.sender();
        let token_states = self.token_states.clone();
        let _ = self
            .token_states
            .lock()
            .map(|mut states| states.insert(token, FileReadState::Queued));
        #[cfg(test)]
        let completion_barriers = self.completion_barriers.clone();
        self.workers.execute_rev(token, move |_| {
            if !begin_file_read(&token_states, token) {
                return;
            }
            let result = std::fs::read(&path)
                .map_err(|err| format!("read {}: {err}", path.display()))
                .and_then(|bytes| {
                    (bytes.len() <= MAX_ROOT_BYTES)
                        .then(|| Arc::from(bytes))
                        .ok_or_else(|| "mkmap root exceeds byte limit".to_string())
                });
            #[cfg(test)]
            if let Some((reached, release)) = completion_barriers {
                reached.wait();
                release.wait();
            }
            if complete_file_read(&token_states, token) {
                let _ = sender.send(ReadCompletion { token, result });
            }
        });
    }

    fn request_range(
        &mut self,
        _cx: &mut Cx,
        shard: u32,
        offset: u64,
        len: u64,
        token: ReadToken,
    ) {
        if len == 0 || len > MAX_RANGE_BYTES || offset.checked_add(len).is_none() {
            let _ = self.completions.sender().send(ReadCompletion {
                token,
                result: Err("invalid mkmap file range".to_string()),
            });
            return;
        }
        let path = self.dir.join(format!("tiles-{shard:03}.mkshard"));
        let sender = self.completions.sender();
        let shard_files = self.shard_files.clone();
        let token_states = self.token_states.clone();
        let _ = self
            .token_states
            .lock()
            .map(|mut states| states.insert(token, FileReadState::Queued));
        #[cfg(test)]
        let completion_barriers = self.completion_barriers.clone();
        self.workers.execute_rev(token, move |_| {
            if !begin_file_read(&token_states, token) {
                return;
            }
            let result = read_file_range(&path, shard, offset, len, &shard_files);
            #[cfg(test)]
            if let Some((reached, release)) = completion_barriers {
                reached.wait();
                release.wait();
            }
            if complete_file_read(&token_states, token) {
                let _ = sender.send(ReadCompletion { token, result });
            }
        });
    }

    fn cancel(&mut self, _cx: &mut Cx, token: ReadToken) {
        let _ = self.token_states.lock().map(|mut states| {
            if let Some(state @ (FileReadState::Queued | FileReadState::Running)) =
                states.get_mut(&token)
            {
                *state = FileReadState::Cancelled;
            }
        });
        let dropped = self.workers.retain_queued(|queued| *queued != token);
        if !dropped.is_empty() {
            let _ = self.token_states.lock().map(|mut states| {
                if matches!(
                    states.get(&token),
                    Some(FileReadState::Queued | FileReadState::Cancelled)
                ) {
                    states.remove(&token);
                }
            });
        }
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
    shard_files: &Mutex<ShardFileCache>,
) -> Result<Arc<[u8]>, String> {
    use std::io::{Read, Seek, SeekFrom};

    let len = usize::try_from(len).map_err(|_| "mkmap range is too large".to_string())?;
    let mut files = shard_files
        .lock()
        .map_err(|_| "mkmap shard file cache lock poisoned".to_string())?;
    if !files.files.contains_key(&shard) {
        while files.files.len() >= FILE_CACHE_CAPACITY {
            if let Some(oldest) = files.lru.pop_front() {
                files.files.remove(&oldest);
            }
        }
        files.files.insert(
            shard,
            std::fs::File::open(path)
                .map_err(|err| format!("open {}: {err}", path.display()))?,
        );
    }
    files.lru.retain(|cached| *cached != shard);
    files.lru.push_back(shard);
    let file = files.files.get_mut(&shard).unwrap();
    file.seek(SeekFrom::Start(offset))
        .map_err(|err| format!("seek {}: {err}", path.display()))?;
    let mut bytes = vec![0; len];
    file.read_exact(&mut bytes)
        .map_err(|err| format!("read {}: {err}", path.display()))?;
    Ok(Arc::from(bytes))
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
                if len > MAX_RANGE_BYTES {
                    self.ready.push_back(ReadCompletion {
                        token: pending.token,
                        result: Err("mkmap HTTP range exceeds byte limit".to_string()),
                    });
                    cx.redraw_all();
                    return;
                }
                let Some(end) = offset
                    .checked_add(len)
                    .filter(|_| len != 0)
                    .and_then(|end| end.checked_sub(1))
                else {
                    self.ready.push_back(ReadCompletion {
                        token: pending.token,
                        result: Err("invalid mkmap HTTP range".to_string()),
                    });
                    cx.redraw_all();
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

    fn retry_or_complete(
        &mut self,
        cx: &mut Cx,
        mut pending: PendingHttpRead,
        result: Result<Arc<[u8]>, String>,
    ) -> Option<ReadCompletion> {
        match result {
            Ok(bytes) => Some(ReadCompletion {
                token: pending.token,
                result: Ok(bytes),
            }),
            Err(_error) if pending.retry_count == 0 => {
                pending.retry_count = 1;
                self.issue(cx, pending);
                None
            }
            Err(error) => Some(ReadCompletion {
                token: pending.token,
                result: Err(error),
            }),
        }
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
                    if let Some(completion) = self.retry_or_complete(
                        cx,
                        pending.clone(),
                        validate_http_response(&pending.kind, response),
                    ) {
                        out.push(completion);
                    }
                }
                NetworkResponse::HttpError { request_id, error } => {
                    let Some(pending) = self.requests.remove(request_id) else {
                        continue;
                    };
                    if let Some(completion) = self.retry_or_complete(
                        cx,
                        pending,
                        Err(format!("mkmap HTTP transport: {}", error.message)),
                    ) {
                        out.push(completion);
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn cancel(&mut self, cx: &mut Cx, token: ReadToken) {
        let request_ids: Vec<LiveId> = self
            .requests
            .iter()
            .filter(|(_, pending)| pending.token == token)
            .map(|(request_id, _)| *request_id)
            .collect();
        for request_id in request_ids {
            self.requests.remove(&request_id);
            cx.cancel_http_request(request_id);
        }
        self.ready.retain(|completion| completion.token != token);
    }
}

fn validate_http_response(
    kind: &HttpReadKind,
    response: &HttpResponse,
) -> Result<Arc<[u8]>, String> {
    match *kind {
        HttpReadKind::Root => {
            if response.status_code != 200 {
                return Err(format!(
                    "mkmap root requires HTTP 200, got {}",
                    response.status_code
                ));
            }
            let body_len = response.body.as_ref().map_or(0, |body| body.len());
            if !(112..=MAX_ROOT_BYTES).contains(&body_len) {
                return Err(format!(
                    "mkmap root response length is invalid: {body_len}"
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
            let body_len = response.body.as_ref().map_or(0, |body| body.len());
            if body_len as u64 != len {
                return Err(format!(
                    "mkmap shard response length mismatch: expected {len}, got {}",
                    body_len
                ));
            }
        }
    }
    Ok(response
        .body
        .as_ref()
        .map(Arc::clone)
        .unwrap_or_else(|| Arc::from([])))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileBytesResult {
    Bytes(Arc<[u8]>),
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
    Blob(BlobRef),
}

#[derive(Clone, Copy, Debug)]
struct TileWaiter {
    generation: u64,
    stage: WaitStage,
}

#[derive(Clone, Copy, Debug)]
enum PendingRead {
    Root { generation: u64 },
    Leaf {
        generation: u64,
        index: usize,
        shard_count: u32,
        start_tile_id: u64,
        end_tile_id: u64,
        dir_len: u64,
    },
    Blob { generation: u64, blob: BlobRef },
}

impl PendingRead {
    fn generation(self) -> u64 {
        match self {
            Self::Root { generation }
            | Self::Leaf { generation, .. }
            | Self::Blob { generation, .. } => generation,
        }
    }

    fn byte_len(self) -> u64 {
        match self {
            Self::Root { .. } => 0,
            Self::Leaf { dir_len, .. } => dir_len,
            Self::Blob { blob, .. } => blob.len,
        }
    }
}

struct BlobInFlight {
    token: ReadToken,
    generation: u64,
    waiters: HashSet<TileKey>,
}

enum ProcessedRead {
    Root(Result<MkmapRoot, String>),
    Leaf(Result<MkmapLeaf, String>),
    Blob(Result<Arc<[u8]>, String>),
}

struct ProcessedCompletion {
    token: ReadToken,
    result: ProcessedRead,
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
    workers: ArchiveWorkerPool,
    processed: ToUIReceiver<ProcessedCompletion>,
    processing: HashSet<ReadToken>,
    root: Option<Arc<MkmapRoot>>,
    root_error: Option<String>,
    root_in_flight: Option<ReadToken>,
    leaves: HashMap<usize, CachedLeaf>,
    leaf_lru_clock: u64,
    leaf_in_flight: HashMap<usize, ReadToken>,
    blob_in_flight: HashMap<BlobRef, BlobInFlight>,
    waiters: HashMap<TileKey, TileWaiter>,
    pending_reads: HashMap<ReadToken, PendingRead>,
    ready: VecDeque<TileBytes>,
    generation: Option<u64>,
    in_flight_bytes: u64,
}

impl<S: ByteSource> TileArchive<S> {
    pub fn new(source: S, workers: ArchiveWorkerPool) -> Self {
        Self {
            source,
            workers,
            processed: Default::default(),
            processing: HashSet::new(),
            root: None,
            root_error: None,
            root_in_flight: None,
            leaves: HashMap::new(),
            leaf_lru_clock: 0,
            leaf_in_flight: HashMap::new(),
            blob_in_flight: HashMap::new(),
            waiters: HashMap::new(),
            pending_reads: HashMap::new(),
            ready: VecDeque::new(),
            generation: None,
            in_flight_bytes: 0,
        }
    }

    pub fn zoom_range(&self) -> Option<(u32, u32)> {
        self.root.as_ref().map(|root| root.zoom_range())
    }

    pub fn metadata(&self) -> Option<&HashMap<String, String>> {
        self.root.as_ref().map(|root| root.metadata())
    }

    pub fn request_tile(&mut self, cx: &mut Cx, key: TileKey, generation: u64) {
        if self.generation.is_some_and(|current| generation < current) {
            return;
        }
        if self.generation != Some(generation) {
            self.reset_generation(cx, generation);
        }
        if self.waiters.contains_key(&key) {
            return;
        }
        if self.root_error.is_some() {
            self.reload(cx, generation);
        }
        if self.waiters.len() >= MAX_ARCHIVE_WAITERS {
            self.finish(
                key,
                generation,
                TileBytesResult::Error("mkmap waiter limit reached".to_string()),
            );
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
            let Some(pending) = self.pending_reads.get(&completion.token).copied() else {
                continue;
            };
            if self.generation != Some(pending.generation())
                || !self.processing.insert(completion.token)
            {
                continue;
            }
            let bytes = match completion.result {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.processing.remove(&completion.token);
                    self.remove_pending_read(completion.token);
                    self.complete_error(pending, error);
                    self.advance_waiters(cx);
                    continue;
                }
            };
            let root = self.root.clone();
            let sender = self.processed.sender();
            let token = completion.token;
            self.workers.execute_rev(token, move |_| {
                let result = match pending {
                    PendingRead::Root { .. } => ProcessedRead::Root(MkmapRoot::parse(&bytes)),
                    PendingRead::Leaf {
                        shard_count,
                        start_tile_id,
                        end_tile_id,
                        ..
                    } => ProcessedRead::Leaf(MkmapLeaf::parse_for_root(
                        &bytes,
                        shard_count,
                        start_tile_id,
                        end_tile_id,
                    )),
                    PendingRead::Blob { .. } => ProcessedRead::Blob(
                        root.ok_or_else(|| "mkmap root disappeared".to_string())
                            .and_then(|root| root.decode_blob(&bytes))
                            .map(Arc::from),
                    ),
                };
                let _ = sender.send(ProcessedCompletion { token, result });
            });
        }
        while let Ok(completion) = self.processed.try_recv() {
            self.processing.remove(&completion.token);
            let Some(pending) = self.remove_pending_read(completion.token) else {
                continue;
            };
            if self.generation != Some(pending.generation()) {
                continue;
            }
            match (pending, completion.result) {
                (PendingRead::Root { generation }, ProcessedRead::Root(result)) => {
                    self.root_in_flight = None;
                    match result {
                        Ok(root) => self.root = Some(Arc::new(root)),
                        Err(error) => {
                            self.root_error = Some(error.clone());
                            self.fail_waiters(generation, |_| true, error);
                        }
                    }
                }
                (PendingRead::Leaf { generation, index, .. }, ProcessedRead::Leaf(result)) => {
                    self.leaf_in_flight.remove(&index);
                    match result {
                        Ok(leaf) => self.insert_leaf(index, leaf),
                        Err(error) => self.fail_waiters(
                            generation,
                            |waiter| waiter.stage == WaitStage::Leaf(index),
                            error,
                        ),
                    }
                }
                (PendingRead::Blob { generation, blob }, ProcessedRead::Blob(result)) => {
                    self.complete_blob(generation, blob, result);
                }
                (pending, _) => self.complete_error(
                    pending,
                    "mkmap worker returned mismatched completion".to_string(),
                ),
            }
            self.advance_waiters(cx);
            self.enforce_leaf_cache_bounds();
        }
        self.ready.drain(..).collect()
    }

    pub fn reset_generation(&mut self, cx: &mut Cx, generation: u64) {
        for token in self.pending_reads.keys().copied().collect::<Vec<_>>() {
            self.source.cancel(cx, token);
            self.workers.retain_queued(|queued| *queued != token);
        }
        self.root = None;
        self.root_error = None;
        self.root_in_flight = None;
        self.leaves.clear();
        self.leaf_in_flight.clear();
        self.blob_in_flight.clear();
        self.waiters.clear();
        self.pending_reads.clear();
        self.processing.clear();
        self.ready.clear();
        self.in_flight_bytes = 0;
        self.generation = Some(generation);
    }

    pub fn reload(&mut self, cx: &mut Cx, generation: u64) {
        self.reset_generation(cx, generation);
    }

    fn ensure_root(&mut self, cx: &mut Cx, generation: u64) {
        if self.root_in_flight.is_some() {
            return;
        }
        let token = next_archive_task_token();
        self.root_in_flight = Some(token);
        self.pending_reads
            .insert(token, PendingRead::Root { generation });
        self.source.request_root(cx, token);
    }

    fn can_start_range(&self, len: u64) -> bool {
        len <= MAX_ARCHIVE_IN_FLIGHT_BYTES
            && self
                .in_flight_bytes
                .checked_add(len)
                .is_some_and(|total| total <= MAX_ARCHIVE_IN_FLIGHT_BYTES)
    }

    fn insert_pending_read(&mut self, token: ReadToken, pending: PendingRead, byte_len: u64) {
        self.in_flight_bytes = self.in_flight_bytes.saturating_add(byte_len);
        self.pending_reads.insert(token, pending);
    }

    fn remove_pending_read(&mut self, token: ReadToken) -> Option<PendingRead> {
        let pending = self.pending_reads.remove(&token)?;
        self.in_flight_bytes = self.in_flight_bytes.saturating_sub(pending.byte_len());
        Some(pending)
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
            if matches!(waiter.stage, WaitStage::Blob(_)) {
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
                    if let Some(in_flight) = self.blob_in_flight.get_mut(&blob) {
                        self.waiters.get_mut(&key).unwrap().stage = WaitStage::Blob(blob);
                        in_flight.waiters.insert(key);
                    } else if self.can_start_range(blob.len) {
                        self.waiters.get_mut(&key).unwrap().stage = WaitStage::Blob(blob);
                        let token = next_archive_task_token();
                        self.insert_pending_read(
                            token,
                            PendingRead::Blob {
                                generation: waiter.generation,
                                blob,
                            },
                            blob.len,
                        );
                        self.blob_in_flight.insert(
                            blob,
                            BlobInFlight {
                                token,
                                generation: waiter.generation,
                                waiters: HashSet::from([key]),
                            },
                        );
                        self.source.request_range(
                            cx,
                            blob.shard,
                            blob.offset,
                            blob.len,
                            token,
                        );
                    }
                }
                Some(None) => self.finish(key, waiter.generation, TileBytesResult::Missing),
                None => {
                    if self.leaf_in_flight.contains_key(&record.index) {
                        self.waiters.get_mut(&key).unwrap().stage = WaitStage::Leaf(record.index);
                    } else if self.can_start_range(record.dir_len) {
                        self.waiters.get_mut(&key).unwrap().stage = WaitStage::Leaf(record.index);
                        let token = next_archive_task_token();
                        self.leaf_in_flight.insert(record.index, token);
                        self.insert_pending_read(
                            token,
                            PendingRead::Leaf {
                                generation: waiter.generation,
                                index: record.index,
                                shard_count: self.root.as_ref().unwrap().shard_count(),
                                start_tile_id: record.start_tile_id,
                                end_tile_id: record.end_tile_id,
                                dir_len: record.dir_len,
                            },
                            record.dir_len,
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
    }

    fn enforce_leaf_cache_bounds(&mut self) {
        while self.leaves.len() > LEAF_CACHE_CAPACITY
            || self
                .leaves
                .values()
                .map(|cached| cached.leaf.retained_bytes())
                .sum::<usize>()
                > LEAF_CACHE_BYTE_CAPACITY
        {
            if let Some(oldest) = self
                .leaves
                .iter()
                .min_by_key(|(_, cached)| cached.used)
                .map(|(index, _)| *index)
            {
                self.leaves.remove(&oldest);
            } else {
                break;
            }
        }
    }

    fn complete_error(&mut self, pending: PendingRead, error: String) {
        match pending {
            PendingRead::Root { generation } => {
                self.root_in_flight = None;
                self.root_error = Some(error.clone());
                self.fail_waiters(generation, |_| true, error);
            }
            PendingRead::Leaf { generation, index, .. } => {
                self.leaf_in_flight.remove(&index);
                self.fail_waiters(
                    generation,
                    |waiter| waiter.stage == WaitStage::Leaf(index),
                    error,
                );
            }
            PendingRead::Blob { generation, blob } => {
                self.complete_blob(generation, blob, Err(error));
            }
        }
    }

    fn complete_blob(
        &mut self,
        generation: u64,
        blob: BlobRef,
        result: Result<Arc<[u8]>, String>,
    ) {
        let Some(in_flight) = self.blob_in_flight.remove(&blob) else {
            return;
        };
        if in_flight.generation != generation {
            return;
        }
        for key in in_flight.waiters {
            if self
                .waiters
                .get(&key)
                .is_some_and(|waiter| waiter.stage == WaitStage::Blob(blob))
            {
                let result = match &result {
                    Ok(bytes) => TileBytesResult::Bytes(bytes.clone()),
                    Err(error) => TileBytesResult::Error(error.clone()),
                };
                self.finish(key, generation, result);
            }
        }
    }

    pub fn cancel_tile(&mut self, cx: &mut Cx, key: TileKey) {
        let Some(waiter) = self.waiters.remove(&key) else {
            return;
        };
        match waiter.stage {
            WaitStage::Root => {
                if !self.waiters.values().any(|waiter| waiter.stage == WaitStage::Root) {
                    if let Some(token) = self.root_in_flight.take() {
                        self.cancel_token(cx, token);
                    }
                }
            }
            WaitStage::Leaf(index) => {
                if !self
                    .waiters
                    .values()
                    .any(|waiter| waiter.stage == WaitStage::Leaf(index))
                {
                    if let Some(token) = self.leaf_in_flight.remove(&index) {
                        self.cancel_token(cx, token);
                    }
                }
            }
            WaitStage::Blob(blob) => {
                let cancel = self.blob_in_flight.get_mut(&blob).is_some_and(|in_flight| {
                    in_flight.waiters.remove(&key);
                    in_flight.waiters.is_empty()
                });
                if cancel {
                    if let Some(in_flight) = self.blob_in_flight.remove(&blob) {
                        self.cancel_token(cx, in_flight.token);
                    }
                }
            }
        }
    }

    fn cancel_token(&mut self, cx: &mut Cx, token: ReadToken) {
        self.source.cancel(cx, token);
        self.workers.retain_queued(|queued| *queued != token);
        self.remove_pending_read(token);
        self.processing.remove(&token);
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
    pub fn file(path: impl AsRef<Path>, workers: ArchiveWorkerPool) -> Self {
        Self::File(TileArchive::new(
            FileByteSource::new(path, workers.clone()),
            workers,
        ))
    }

    pub fn http(root_url: impl Into<String>, workers: ArchiveWorkerPool) -> Self {
        Self::Http(TileArchive::new(
            HttpRangeByteSource::new(root_url),
            workers,
        ))
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

    pub fn cancel_tile(&mut self, cx: &mut Cx, key: TileKey) {
        match self {
            Self::File(archive) => archive.cancel_tile(cx, key),
            Self::Http(archive) => archive.cancel_tile(cx, key),
        }
    }

    pub fn reset_generation(&mut self, cx: &mut Cx, generation: u64) {
        match self {
            Self::File(archive) => archive.reset_generation(cx, generation),
            Self::Http(archive) => archive.reset_generation(cx, generation),
        }
    }

    pub fn zoom_range(&self) -> Option<(u32, u32)> {
        match self {
            Self::File(archive) => archive.zoom_range(),
            Self::Http(archive) => archive.zoom_range(),
        }
    }

    #[cfg(test)]
    pub(crate) fn source_request_count(&self) -> usize {
        match self {
            Self::File(_) => 0,
            Self::Http(archive) => archive.source.requests.len(),
        }
    }

    #[cfg(test)]
    pub(crate) fn waiter_count(&self) -> usize {
        match self {
            Self::File(archive) => archive.waiters.len(),
            Self::Http(archive) => archive.waiters.len(),
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
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
        cancelled: Vec<ReadToken>,
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

        fn cancel(&mut self, _cx: &mut Cx, token: ReadToken) {
            self.cancelled.push(token);
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

    fn tiny_parts(keys: [TileKey; 2], shared_blob: bool) -> (Vec<u8>, Vec<u8>) {
        tiny_parts_with_blob_len(keys, shared_blob, 3)
    }

    fn tiny_parts_with_blob_len(
        keys: [TileKey; 2],
        shared_blob: bool,
        blob_len: u64,
    ) -> (Vec<u8>, Vec<u8>) {
        let ids = keys.map(|key| mkmap_tile_id(key.z as u8, key.x as u32, key.y as u32));
        assert!(ids[0] < ids[1]);
        let mut leaf_raw = Vec::new();
        write_varint(2, &mut leaf_raw);
        write_varint(ids[0], &mut leaf_raw);
        write_varint(0, &mut leaf_raw);
        write_varint(200, &mut leaf_raw);
        write_varint(blob_len, &mut leaf_raw);
        write_varint(ids[1] - ids[0], &mut leaf_raw);
        write_varint(0, &mut leaf_raw);
        write_varint(if shared_blob { 200 } else { 300 }, &mut leaf_raw);
        write_varint(blob_len, &mut leaf_raw);
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

    fn response(status: u16, range: Option<&str>, body: &[u8]) -> HttpResponse {
        HttpResponse {
            metadata_id: LiveId::empty(),
            status_code: status,
            headers: range
                .map(|range| {
                    BTreeMap::from([("Content-Range".to_string(), vec![range.to_string()])])
                })
                .unwrap_or_default(),
            body: Some(Arc::from(body)),
        }
    }

    fn poll_until(
        archive: &mut TileArchive<MockByteSource>,
        cx: &mut Cx,
        mut done: impl FnMut(&TileArchive<MockByteSource>) -> bool,
    ) -> Vec<TileBytes> {
        let mut completed = Vec::new();
        for _ in 0..2_000 {
            completed.extend(archive.drain(cx, &Event::Startup));
            if done(archive) {
                return completed;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("archive worker did not complete");
    }

    fn retry_http_range(first: HttpResponse, second: HttpResponse) -> ReadCompletion {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut source = HttpRangeByteSource::new("https://tiles.invalid/world.mkmap");
        let token = next_archive_task_token();
        source.request_range(&mut cx, 0, 10, 4, token);
        let first_id = *source.requests.keys().next().unwrap();
        assert!(source
            .poll(
                &mut cx,
                &Event::NetworkResponses(vec![NetworkResponse::HttpResponse {
                    request_id: first_id,
                    response: first,
                }]),
            )
            .is_empty());
        assert_eq!(source.requests.len(), 1, "first failure must retry once");
        let retry_id = *source.requests.keys().next().unwrap();
        assert_ne!(retry_id, first_id);
        let completed = source.poll(
            &mut cx,
            &Event::NetworkResponses(vec![NetworkResponse::HttpResponse {
                request_id: retry_id,
                response: second,
            }]),
        );
        assert_eq!(completed.len(), 1);
        completed.into_iter().next().unwrap()
    }

    #[test]
    fn http_correlates_ids_and_retries_every_validation_failure_once() {
        let good = response(206, Some("bytes 10-13/100"), b"abcd");
        for bad in [
            response(200, None, b"abcd"),
            response(404, None, b"nope"),
            response(500, None, b"nope"),
            response(206, None, b"abcd"),
            response(206, Some("bytes 11-14/100"), b"abcd"),
            response(206, Some("bytes 10-13/100"), b"abc"),
        ] {
            assert_eq!(
                retry_http_range(bad, good.clone()).result.unwrap().as_ref(),
                b"abcd"
            );
        }

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut source = HttpRangeByteSource::new("https://tiles.invalid/world.mkmap");
        let first = next_archive_task_token();
        let second = next_archive_task_token();
        source.request_range(&mut cx, 0, 10, 4, first);
        source.request_range(&mut cx, 0, 20, 4, second);
        let request_id = source
            .requests
            .iter()
            .find(|(_, pending)| pending.token == second)
            .map(|(id, _)| *id)
            .unwrap();
        let done = source.poll(
            &mut cx,
            &Event::NetworkResponses(vec![NetworkResponse::HttpResponse {
                request_id,
                response: response(206, Some("bytes 20-23/100"), b"wxyz"),
            }]),
        );
        assert_eq!(done[0].token, second);
        assert!(source.requests.values().any(|pending| pending.token == first));

        let request_id = source
            .requests
            .iter()
            .find(|(_, pending)| pending.token == first)
            .map(|(id, _)| *id)
            .unwrap();
        assert!(source
            .poll(
                &mut cx,
                &Event::NetworkResponses(vec![NetworkResponse::HttpError {
                    request_id,
                    error: HttpError {
                        message: "offline".to_string(),
                        metadata_id: LiveId::empty(),
                    },
                }]),
            )
            .is_empty());
        let retry_id = source
            .requests
            .iter()
            .find(|(_, pending)| pending.token == first)
            .map(|(id, _)| *id)
            .unwrap();
        let done = source.poll(
            &mut cx,
            &Event::NetworkResponses(vec![NetworkResponse::HttpError {
                request_id: retry_id,
                error: HttpError {
                    message: "still offline".to_string(),
                    metadata_id: LiveId::empty(),
                },
            }]),
        );
        assert!(done[0].result.as_ref().unwrap_err().contains("still offline"));
    }

    #[test]
    fn root_truncation_retries_then_reports_second_attempt() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut source = HttpRangeByteSource::new("https://tiles.invalid/world.mkmap");
        let token = next_archive_task_token();
        source.request_root(&mut cx, token);
        let first_id = *source.requests.keys().next().unwrap();
        assert!(source
            .poll(
                &mut cx,
                &Event::NetworkResponses(vec![NetworkResponse::HttpResponse {
                    request_id: first_id,
                    response: response(200, None, b"short"),
                }]),
            )
            .is_empty());
        let retry_id = *source.requests.keys().next().unwrap();
        let done = source.poll(
            &mut cx,
            &Event::NetworkResponses(vec![NetworkResponse::HttpResponse {
                request_id: retry_id,
                response: response(404, None, b"missing"),
            }]),
        );
        assert!(done[0].result.as_ref().unwrap_err().contains("404"));
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
    fn archive_deduplicates_root_leaf_and_writer_shared_blob() {
        let keys = [
            TileKey { z: 1, x: 0, y: 0 },
            TileKey { z: 1, x: 0, y: 1 },
        ];
        let (root, leaf) = tiny_parts(keys, true);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let workers = new_archive_worker_pool(&mut cx);
        let mut archive = TileArchive::new(MockByteSource::default(), workers);
        archive.request_tile(&mut cx, keys[0], 7);
        archive.request_tile(&mut cx, keys[1], 7);
        assert_eq!(archive.source.requests.len(), 1);
        let MockRequest::Root(root_token) = archive.source.requests[0] else {
            panic!("first request was not root")
        };
        archive.source.completions.push_back(ReadCompletion {
            token: root_token,
            result: Ok(root.into()),
        });
        let _ = poll_until(&mut archive, &mut cx, |archive| archive.source.requests.len() >= 2);
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
            result: Ok(leaf.into()),
        });
        let _ = poll_until(&mut archive, &mut cx, |archive| archive.source.requests.len() >= 3);
        assert_eq!(archive.source.requests.len(), 3, "shared blob needs one range");
        let MockRequest::Range { token, offset, .. } = archive.source.requests[2] else {
            panic!("third request was not a blob")
        };
        assert_eq!(offset, 200);
        archive.source.completions.push_back(ReadCompletion {
            token,
            result: Ok(Arc::from(&b"one"[..])),
        });
        let done = poll_until(&mut archive, &mut cx, |archive| archive.waiters.is_empty());
        assert_eq!(done.len(), 2);
        assert!(done.iter().any(|tile| tile.key == keys[0]));
        assert!(done.iter().any(|tile| tile.key == keys[1]));
        let blobs = done
            .iter()
            .map(|tile| match &tile.result {
                TileBytesResult::Bytes(bytes) => bytes,
                result => panic!("unexpected tile result: {result:?}"),
            })
            .collect::<Vec<_>>();
        assert!(Arc::ptr_eq(blobs[0], blobs[1]));
    }

    #[test]
    fn aggregate_in_flight_blob_bytes_are_bounded() {
        let keys = [
            TileKey { z: 1, x: 0, y: 0 },
            TileKey { z: 1, x: 0, y: 1 },
        ];
        let blob_len = 40 * 1024 * 1024;
        let (root, leaf) = tiny_parts_with_blob_len(keys, false, blob_len);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let workers = new_archive_worker_pool(&mut cx);
        let mut archive = TileArchive::new(MockByteSource::default(), workers);
        for key in keys {
            archive.request_tile(&mut cx, key, 1);
        }
        let MockRequest::Root(root_token) = archive.source.requests[0] else { unreachable!() };
        archive.source.completions.push_back(ReadCompletion {
            token: root_token,
            result: Ok(root.into()),
        });
        let _ = poll_until(&mut archive, &mut cx, |archive| archive.source.requests.len() >= 2);
        let MockRequest::Range { token: leaf_token, .. } = archive.source.requests[1] else {
            unreachable!()
        };
        archive.source.completions.push_back(ReadCompletion {
            token: leaf_token,
            result: Ok(leaf.into()),
        });
        let _ = poll_until(&mut archive, &mut cx, |archive| archive.source.requests.len() >= 3);
        let blob_requests = archive.source.requests[2..]
            .iter()
            .filter_map(|request| match request {
                MockRequest::Range { len, .. } if *len == blob_len => Some(*len),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(blob_requests, [blob_len]);
        assert_eq!(archive.in_flight_bytes, blob_len);
        assert!(archive.in_flight_bytes <= MAX_ARCHIVE_IN_FLIGHT_BYTES);
    }

    #[test]
    fn shared_blob_read_cancels_only_after_its_last_waiter() {
        let keys = [
            TileKey { z: 1, x: 0, y: 0 },
            TileKey { z: 1, x: 0, y: 1 },
        ];
        let (root, leaf) = tiny_parts(keys, true);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let workers = new_archive_worker_pool(&mut cx);
        let mut archive = TileArchive::new(MockByteSource::default(), workers);
        archive.request_tile(&mut cx, keys[0], 1);
        archive.request_tile(&mut cx, keys[1], 1);
        let MockRequest::Root(root_token) = archive.source.requests[0] else { unreachable!() };
        archive.source.completions.push_back(ReadCompletion {
            token: root_token,
            result: Ok(root.into()),
        });
        let _ = poll_until(&mut archive, &mut cx, |archive| archive.source.requests.len() >= 2);
        let MockRequest::Range { token: leaf_token, .. } = archive.source.requests[1] else {
            unreachable!()
        };
        archive.source.completions.push_back(ReadCompletion {
            token: leaf_token,
            result: Ok(leaf.into()),
        });
        let _ = poll_until(&mut archive, &mut cx, |archive| archive.source.requests.len() >= 3);
        let MockRequest::Range { token: blob_token, .. } = archive.source.requests[2] else {
            unreachable!()
        };
        archive.cancel_tile(&mut cx, keys[0]);
        assert!(!archive.source.cancelled.contains(&blob_token));
        archive.cancel_tile(&mut cx, keys[1]);
        assert!(archive.source.cancelled.contains(&blob_token));
    }

    #[test]
    fn archive_drops_stale_generation_completions() {
        let keys = [
            TileKey { z: 1, x: 0, y: 0 },
            TileKey { z: 1, x: 0, y: 1 },
        ];
        let (root, _) = tiny_parts(keys, false);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let workers = new_archive_worker_pool(&mut cx);
        let mut archive = TileArchive::new(MockByteSource::default(), workers);
        archive.request_tile(&mut cx, keys[0], 1);
        let MockRequest::Root(stale_token) = archive.source.requests[0] else {
            panic!("first request was not root")
        };
        archive.request_tile(&mut cx, keys[1], 2);
        assert!(archive.source.cancelled.contains(&stale_token));
        archive.source.completions.push_back(ReadCompletion {
            token: stale_token,
            result: Ok(root.into()),
        });
        assert!(archive.drain(&mut cx, &Event::Startup).is_empty());
        assert_eq!(archive.source.requests.len(), 2);
    }

    #[test]
    fn root_error_can_reload_without_poisoning_the_generation() {
        let key = TileKey { z: 1, x: 0, y: 0 };
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let workers = new_archive_worker_pool(&mut cx);
        let mut archive = TileArchive::new(MockByteSource::default(), workers);
        archive.request_tile(&mut cx, key, 9);
        let MockRequest::Root(token) = archive.source.requests[0] else { unreachable!() };
        archive.source.completions.push_back(ReadCompletion {
            token,
            result: Err("root unavailable".to_string()),
        });
        assert!(matches!(
            archive.drain(&mut cx, &Event::Startup)[0].result,
            TileBytesResult::Error(_)
        ));
        archive.request_tile(&mut cx, key, 9);
        assert_eq!(archive.source.requests.len(), 2);
    }

    #[test]
    fn waiter_and_leaf_lru_are_bounded() {
        let keys = [
            TileKey { z: 1, x: 0, y: 0 },
            TileKey { z: 1, x: 0, y: 1 },
        ];
        let (_, leaf_bytes) = tiny_parts(keys, false);
        let leaf = MkmapLeaf::parse(&leaf_bytes).unwrap();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let workers = new_archive_worker_pool(&mut cx);
        let mut archive = TileArchive::new(MockByteSource::default(), workers);
        for x in 0..=MAX_ARCHIVE_WAITERS {
            archive.request_tile(
                &mut cx,
                TileKey {
                    z: 7,
                    x: x as i32,
                    y: 0,
                },
                1,
            );
        }
        assert_eq!(archive.waiters.len(), MAX_ARCHIVE_WAITERS);
        assert_eq!(archive.ready.len(), 1);
        for index in 0..=LEAF_CACHE_CAPACITY {
            archive.insert_leaf(index, leaf.clone());
        }
        archive.enforce_leaf_cache_bounds();
        assert_eq!(archive.leaves.len(), LEAF_CACHE_CAPACITY);
        assert!(!archive.leaves.contains_key(&0));
    }

    #[test]
    fn file_source_is_async_and_bounded() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let workers = new_archive_worker_pool(&mut cx);
        let token = next_archive_task_token();
        let dir = PathBuf::from(format!("target/map-archive-test-{}", token.0));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("root.mkidx"), vec![7_u8; 112]).unwrap();
        for shard in 0..10_u32 {
            std::fs::write(dir.join(format!("tiles-{shard:03}.mkshard")), [shard as u8]).unwrap();
        }

        let mut first = FileByteSource::new(&dir, workers.clone());
        first.request_root(&mut cx, token);
        for shard in 0..10_u32 {
            first.request_range(&mut cx, shard, 0, 1, next_archive_task_token());
        }
        let mut completions = Vec::new();
        for _ in 0..2_000 {
            completions.extend(first.poll(&mut cx, &Event::Startup));
            if completions.len() == 11 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(completions.len(), 11);
        assert!(first.shard_files.lock().unwrap().files.len() <= FILE_CACHE_CAPACITY);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn file_cancel_wins_completion_race_without_leaking_token_state() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let workers = new_archive_worker_pool(&mut cx);
        let token = next_archive_task_token();
        let dir = PathBuf::from(format!("target/map-archive-cancel-race-{}", token.0));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tiles-000.mkshard"), b"tile").unwrap();

        let reached = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let mut source = FileByteSource::new(&dir, workers);
        source.completion_barriers = Some((reached.clone(), release.clone()));
        source.request_range(&mut cx, 0, 0, 4, token);
        reached.wait();
        source.cancel(&mut cx, token);
        assert_eq!(
            source.token_states.lock().unwrap().get(&token),
            Some(&FileReadState::Cancelled)
        );
        release.wait();
        for _ in 0..2_000 {
            if source.token_states.lock().unwrap().is_empty() {
                break;
            }
            std::thread::yield_now();
        }
        assert!(source.poll(&mut cx, &Event::Startup).is_empty());
        assert!(source.token_states.lock().unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn http_reset_cancels_and_ignores_stale_error_without_retry() {
        let key = TileKey { z: 1, x: 0, y: 0 };
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let workers = new_archive_worker_pool(&mut cx);
        let mut archive = TileArchive::new(
            HttpRangeByteSource::new("https://tiles.invalid/world.mkmap"),
            workers,
        );
        archive.request_tile(&mut cx, key, 1);
        let stale_id = *archive.source.requests.keys().next().unwrap();
        archive.reset_generation(&mut cx, 2);
        assert!(archive.source.requests.is_empty());
        let done = archive.drain(
            &mut cx,
            &Event::NetworkResponses(vec![NetworkResponse::HttpError {
                request_id: stale_id,
                error: HttpError {
                    message: "late".to_string(),
                    metadata_id: LiveId::empty(),
                },
            }]),
        );
        assert!(done.is_empty());
        assert!(archive.source.requests.is_empty());
    }
}
