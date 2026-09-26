//! Downloads for installs run with `jobs::run`: every payload of every
//! component goes through one queue served by a fixed set of connection
//! threads (they do not count against the unpack pool), largest file first,
//! with a cap per host.
//!
//! The download hosts throttle each connection, so a large file is fetched
//! as byte ranges over several connections at once, after a HEAD request
//! has confirmed its size and range support (else it is one request).
//! Segments land in the file's `.part` at their offset and are listed in
//! its `.segs`, so an interrupted download resumes with the missing ones;
//! the file is hashed in order as its segments complete and published
//! exactly as before (`.ok` stamp, then rename). Payloads come back in
//! memory, so unpacking does not read them again.
//!
//! Manifests (catalogs) go first: until every component has asked for its
//! first payload, only manifest requests are served, so all queues fill
//! before the largest downloads take the connections.
use crate::{http, jobs, progress, sha256, timing};
use std::{
    collections::{HashMap, HashSet},
    fs,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{atomic::{AtomicU64, Ordering}, mpsc, Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

type Task = Box<dyn FnOnce() + Send>;
struct Queued {
    priority: u64,
    urgent: bool,
    seq: u64,
    host: String,
    task: Task,
}
#[derive(Default)]
struct HostStats {
    bytes: u64,
    requests: u64,
    first: Option<Instant>,
    last: Option<Instant>,
    active: usize,
    peak: usize,
}
struct Queue {
    tasks: Vec<Queued>,
    hosts: HashMap<String, HostStats>,
    seq: u64,
    closed: bool,
    /// Components still reading their manifests.
    expected: HashSet<String>,
    gate_until: Instant,
}
pub struct Fetcher {
    queue: Mutex<Queue>,
    ready: Condvar,
    threads: usize,
    segment: u64,
}

fn setting(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|s| s.parse().ok()).filter(|n| *n > 0)
}
/// Connections per host: `MAKEPAD_BUILDER_HOST_CONNECTIONS` for all, else
/// what was measured to help. Rust's and Microsoft's CDNs give each
/// connection a few MB/s; NVIDIA's gives one connection most of the line.
fn host_cap(host: &str) -> usize {
    if let Some(n) = setting("MAKEPAD_BUILDER_HOST_CONNECTIONS") {
        return n as usize;
    }
    match host {
        "developer.download.nvidia.com" => 4,
        _ => 12,
    }
}

impl Default for Fetcher {
    fn default() -> Self {
        Self::new()
    }
}
impl Fetcher {
    /// `MAKEPAD_BUILDER_CONNECTIONS` in all (24) and segments of
    /// `MAKEPAD_BUILDER_SEGMENT_MB` (4: measured best on the Windows test box).
    pub fn new() -> Fetcher {
        Fetcher {
            queue: Mutex::new(Queue {
                tasks: Vec::new(),
                hosts: HashMap::new(),
                seq: 0,
                closed: false,
                expected: HashSet::new(),
                gate_until: Instant::now(),
            }),
            ready: Condvar::new(),
            threads: setting("MAKEPAD_BUILDER_CONNECTIONS").unwrap_or(24) as usize,
            segment: setting("MAKEPAD_BUILDER_SEGMENT_MB").unwrap_or(4) << 20,
        }
    }
    pub fn threads(&self) -> usize {
        self.threads
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Hold payload downloads until each of `labels` asked for its first
    /// payload (or finished), at most a few seconds.
    pub fn expect(&self, labels: &[&str]) {
        let mut queue = self.lock();
        queue.expected = labels.iter().map(|l| l.to_string()).collect();
        queue.gate_until = Instant::now() + Duration::from_secs(4);
    }
    /// `label` has read its manifests: its payloads may start.
    pub fn ready(&self, label: &str) {
        let mut queue = self.lock();
        if queue.expected.remove(label) && queue.expected.is_empty() {
            drop(queue);
            self.ready.notify_all();
        }
    }
    fn submit(&self, host: &str, priority: u64, urgent: bool, task: Task) {
        let mut queue = self.lock();
        queue.seq += 1;
        let seq = queue.seq;
        queue.tasks.push(Queued { priority, urgent, seq, host: host.into(), task });
        drop(queue);
        self.ready.notify_all();
    }
    pub fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }
    /// A connection thread: the most urgent, then largest waiting download
    /// whose host has a free connection, until closed.
    pub fn work(&self) {
        loop {
            let (host, task) = {
                let mut queue = self.lock();
                loop {
                    let gated = !queue.expected.is_empty() && Instant::now() < queue.gate_until;
                    let best = queue
                        .tasks
                        .iter()
                        .enumerate()
                        .filter(|(_, t)| !gated || t.urgent)
                        .filter(|(_, t)| queue.hosts.get(&t.host).map_or(0, |h| h.active) < host_cap(&t.host))
                        .max_by(|(_, a), (_, b)| (a.urgent, a.priority).cmp(&(b.urgent, b.priority)).then(b.seq.cmp(&a.seq)))
                        .map(|(i, _)| i);
                    if let Some(index) = best {
                        let queued = queue.tasks.swap_remove(index);
                        let stats = queue.hosts.entry(queued.host.clone()).or_default();
                        stats.active += 1;
                        stats.peak = stats.peak.max(stats.active);
                        break (queued.host, queued.task);
                    }
                    if queue.closed && queue.tasks.is_empty() {
                        return;
                    }
                    let wait = if gated { Duration::from_millis(50) } else { Duration::from_secs(1) };
                    queue = self.ready.wait_timeout(queue, wait).unwrap_or_else(|e| e.into_inner()).0;
                }
            };
            let _ = catch_unwind(AssertUnwindSafe(task));
            if let Some(stats) = self.lock().hosts.get_mut(&host) {
                stats.active -= 1;
            }
            self.ready.notify_all();
        }
    }
    fn record(&self, host: &str, bytes: u64, start: Instant) {
        let mut queue = self.lock();
        let stats = queue.hosts.entry(host.into()).or_default();
        stats.bytes += bytes;
        stats.requests += 1;
        stats.first = Some(stats.first.map_or(start, |f| f.min(start)));
        stats.last = Some(Instant::now());
    }
    /// Per host: bytes, the time from its first request to its last byte,
    /// and the connections it used at most.
    pub fn report(&self) -> Vec<String> {
        let queue = self.lock();
        let mut hosts: Vec<_> = queue.hosts.iter().filter(|(_, s)| s.requests > 0).collect();
        hosts.sort_by(|a, b| a.0.cmp(b.0));
        hosts
            .into_iter()
            .map(|(host, s)| {
                let secs = match (s.first, s.last) {
                    (Some(a), Some(b)) => b.duration_since(a).as_secs_f64(),
                    _ => 0.0,
                };
                let mb = s.bytes as f64 / 1_048_576.0;
                format!(
                    "timing: net {host}: {mb:.0} MB in {secs:.1} s = {:.1} MB/s, {} requests, up to {} of {} connections",
                    mb / secs.max(0.001),
                    s.requests,
                    s.peak,
                    host_cap(host)
                )
            })
            .collect()
    }
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?']).next().unwrap_or(rest).to_ascii_lowercase()
}

/// The payload, from the cache when a verified copy is there, else
/// downloaded into it; then `then` runs on the unpack pool with its bytes.
/// Counts as `size` units of the component's bar when it arrives (the
/// download half; `then` steps the unpack half).
pub fn file_then<R: Send + 'static>(
    cache: &Path,
    url: &str,
    file_name: &str,
    sha256_hex: Option<&str>,
    size: u64,
    then: impl FnOnce(Arc<Vec<u8>>) -> Result<R, String> + Send + 'static,
) -> jobs::Pending<R> {
    file_with(false, cache, url, file_name, sha256_hex, size, then)
}
/// [`file_then`] for a small payload that says what else to download (the
/// SDK's MSIs name its cabinets): served ahead of the large ones, as
/// manifests are.
pub fn listing_then<R: Send + 'static>(
    cache: &Path,
    url: &str,
    file_name: &str,
    sha256_hex: Option<&str>,
    size: u64,
    then: impl FnOnce(Arc<Vec<u8>>) -> Result<R, String> + Send + 'static,
) -> jobs::Pending<R> {
    file_with(true, cache, url, file_name, sha256_hex, size, then)
}
fn file_with<R: Send + 'static>(
    urgent: bool,
    cache: &Path,
    url: &str,
    file_name: &str,
    sha256_hex: Option<&str>,
    size: u64,
    then: impl FnOnce(Arc<Vec<u8>>) -> Result<R, String> + Send + 'static,
) -> jobs::Pending<R> {
    let Some(env) = jobs::env() else {
        // Outside an install run: the plain sequential download.
        let result = http::cached_file(cache, url, file_name, sha256_hex)
            .and_then(|path| crate::extract::read(&path))
            .and_then(|bytes| then(Arc::new(bytes)));
        return jobs::Pending::ready(result);
    };
    if !urgent {
        env.fetch.ready(&timing::current());
    }
    let (tx, pending) = jobs::Pending::channel();
    let request = Arc::new(FileRequest {
        cache: cache.to_path_buf(),
        url: url.into(),
        host: host_of(url),
        file_name: file_name.into(),
        sha: sha256_hex.map(str::to_owned),
        size,
        persist: true,
        urgent,
    });
    let then: Box<dyn FnOnce(Arc<Vec<u8>>) -> Result<R, String> + Send> = Box::new(then);
    let fetch = env.fetch.clone();
    // Checking the cache hashes the file: CPU work, on the pool.
    let _ = jobs::spawn(move || {
        let done = move |result: Result<Arc<Vec<u8>>, String>| {
            let _ = tx.send(result.and_then(then));
        };
        match request.cached() {
            Ok(Some(bytes)) => {
                progress::advance(request.size);
                done(Ok(Arc::new(bytes)));
            }
            Ok(None) => start(&fetch, request, Box::new(move |result| {
                // The last segment arrives on a connection thread; unpack on the pool.
                let _ = jobs::spawn(move || {
                    done(result);
                    Ok(())
                });
            })),
            Err(e) => done(Err(e)),
        }
        Ok(())
    });
    pending
}
/// A manifest or catalog: not cached, ahead of every payload, and in
/// segments when large.
pub fn bytes(url: &str) -> Result<Vec<u8>, String> {
    let Some(env) = jobs::env() else {
        return http::fetch_bytes(url);
    };
    let request = Arc::new(FileRequest {
        cache: PathBuf::new(),
        url: url.into(),
        host: host_of(url),
        file_name: url.rsplit('/').next().unwrap_or(url).into(),
        sha: None,
        size: 0,
        persist: false,
        urgent: true,
    });
    let (tx, rx) = mpsc::channel();
    start(&env.fetch, request, Box::new(move |result| {
        let _ = tx.send(result);
    }));
    let bytes = rx.recv().map_err(|_| "a download stopped without a result".to_string())??;
    Ok(Arc::try_unwrap(bytes).unwrap_or_else(|shared| (*shared).clone()))
}

struct FileRequest {
    cache: PathBuf,
    url: String,
    host: String,
    file_name: String,
    sha: Option<String>,
    /// From the manifest: what the file counts on the bar (0: unknown).
    size: u64,
    /// Kept in the cache (payloads) or only returned (manifests).
    persist: bool,
    urgent: bool,
}
impl FileRequest {
    fn dest(&self) -> PathBuf {
        self.cache.join(http::safe_name(&self.file_name))
    }
    /// A complete, verified cached copy, as `http::cached_file` accepts it.
    fn cached(&self) -> Result<Option<Vec<u8>>, String> {
        fs::create_dir_all(&self.cache).map_err(|e| e.to_string())?;
        let dest = self.dest();
        let ok = http::sidecar(&dest, ".ok");
        progress::stage("Verify cache", &self.file_name, 0.0);
        if !dest.is_file() {
            return Ok(None);
        }
        let stamp = fs::read_to_string(&ok).unwrap_or_default();
        let field = |key: &str| stamp.lines().find_map(|l| l.strip_prefix(key)).map(|v| v.trim().to_owned());
        let size = field("size=").and_then(|v| v.parse::<u64>().ok());
        let recorded = field("sha256=");
        let expect = self.sha.clone().or(recorded);
        let len = dest.metadata().map_err(|e| e.to_string())?.len();
        if len > 0 && size == Some(len) {
            if let Some(expect) = expect {
                let bytes = crate::extract::read(&dest)?;
                let mut span = timing::span(timing::Phase::Verify);
                span.bytes(bytes.len() as u64);
                if sha256::sha256_hex(&bytes).eq_ignore_ascii_case(&expect) {
                    return Ok(Some(bytes));
                }
            }
        }
        crate::setup_note!("  incomplete or corrupt {}, redownloading", self.file_name);
        let _ = fs::remove_file(&dest);
        let _ = fs::remove_file(&ok);
        Ok(None)
    }
}

type Finish = Box<dyn FnOnce(Result<Arc<Vec<u8>>, String>) + Send>;

/// Queue the download: a first task asks the server for the size and
/// range support (for files big enough to split), then queues the ranges.
fn start(fetch: &Arc<Fetcher>, request: Arc<FileRequest>, finish: Finish) {
    let segment = fetch.segment;
    let split = request.size >= 2 * segment || (request.size == 0 && request.urgent);
    let ctx = jobs::Ctx::current();
    let fetch2 = fetch.clone();
    let (host, priority, urgent) = (request.host.clone(), request.size, request.urgent);
    let task: Task = Box::new(move || {
        ctx.enter(|| {
            let (url, size) = if split { probe(&request) } else { (request.url.clone(), None) };
            let (size, segment) = match size {
                Some(size) if size >= 2 * segment => (size, segment),
                Some(size) if !request.urgent => (size, size.max(1)),
                _ => (0, 1),
            };
            let count = if size == 0 { 1 } else { size.div_ceil(segment) as usize };
            let dest = request.dest();
            let download = Arc::new(Download {
                part: http::sidecar(&dest, ".part"),
                segs: http::sidecar(&dest, ".segs"),
                request: request.clone(),
                url,
                size,
                segment,
                count,
                state: Mutex::new(State {
                    buf: Vec::new(),
                    done: vec![false; count],
                    remaining: count,
                    hashed: 0,
                    hasher: sha256::Sha256::new(),
                    over: false,
                }),
                received: AtomicU64::new(0),
                reported: AtomicU64::new(0),
                finish: Mutex::new(Some(finish)),
            });
            if request.persist {
            }
            let todo = download.resume();
            if todo.is_empty() {
                download.complete_if_done();
                return;
            }
            // The first range runs right here, on this connection.
            let first = todo[0];
            for &index in &todo[1..] {
                let download = download.clone();
                let ctx = jobs::Ctx::current();
                fetch2.submit(&request.host, request.size.max(size), request.urgent, Box::new(move || ctx.enter(|| download.segment_task(index))));
            }
            download.segment_task(first);
        })
    });
    fetch.submit(&host, priority, urgent, task);
}

/// The file's address after redirects and its length, when the server
/// can send it in ranges (HEAD; `None` if it cannot, or on failure).
fn probe(request: &FileRequest) -> (String, Option<u64>) {
    let start = Instant::now();
    let result = http::fetch_to("HEAD", &request.url, &[], &[], None, &[200]);
    if let Some(env) = jobs::env() {
        env.fetch.record(&request.host, 0, start);
    }
    match result {
        Ok((resp, url)) => {
            let ranges = resp.header("accept-ranges").is_some_and(|v| v.eq_ignore_ascii_case("bytes"));
            let len = http::content_length(&resp).filter(|n| *n > 0);
            match (ranges, len) {
                (true, Some(len)) => (url, Some(len)),
                _ => (url, None),
            }
        }
        Err(_) => (request.url.clone(), None),
    }
}

/// A file being downloaded, in ranges when `count` > 1.
struct Download {
    request: Arc<FileRequest>,
    /// Where the redirects led.
    url: String,
    /// The server's length (0: one request of unknown length).
    size: u64,
    part: PathBuf,
    segs: PathBuf,
    segment: u64,
    count: usize,
    state: Mutex<State>,
    received: AtomicU64,
    reported: AtomicU64,
    finish: Mutex<Option<Finish>>,
}
struct State {
    buf: Vec<u8>,
    done: Vec<bool>,
    remaining: usize,
    hashed: usize,
    hasher: sha256::Sha256,
    over: bool,
}

impl Download {
    fn range(&self, index: usize) -> (u64, u64) {
        let a = index as u64 * self.segment;
        (a, (a + self.segment).min(self.size))
    }
    fn split(&self) -> bool {
        self.count > 1
    }
    /// Segments an earlier run completed stay; the rest are to download.
    fn resume(&self) -> Vec<usize> {
        let size = self.size;
        let mut state = self.lock();
        if size > 0 {
            state.buf = vec![0; size as usize];
        }
        if !self.request.persist {
            return (0..self.count).collect();
        }
        let header = format!("size={size} segment={}", self.segment);
        let listed: Vec<usize> = fs::read_to_string(&self.segs)
            .ok()
            .filter(|text| text.lines().next() == Some(header.as_str()))
            .map(|text| text.lines().skip(1).filter_map(|l| l.trim().parse().ok()).filter(|i| *i < self.count).collect())
            .unwrap_or_default();
        let part_ok = self.split() && self.part.metadata().is_ok_and(|m| m.len() == size);
        if !listed.is_empty() && part_ok {
            if let Ok(bytes) = crate::extract::read(&self.part) {
                let mut resumed = 0;
                for &index in &listed {
                    if state.done[index] {
                        continue;
                    }
                    let (a, b) = self.range(index);
                    state.buf[a as usize..b as usize].copy_from_slice(&bytes[a as usize..b as usize]);
                    state.done[index] = true;
                    state.remaining -= 1;
                    resumed += b - a;
                }
                crate::setup_note!("  resume {}: {} of {} parts on disk", self.request.file_name, self.count - state.remaining, self.count);
                progress::advance(resumed);
                self.received.fetch_add(resumed, Ordering::Relaxed);
                self.hash_ready(&mut state);
                return (0..self.count).filter(|i| !state.done[*i]).collect();
            }
        }
        let _ = fs::remove_file(&self.part);
        if self.split() {
            let _ = fs::write(&self.segs, format!("{header}\n"));
        } else {
            let _ = fs::remove_file(&self.segs);
        }
        (0..self.count).collect()
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn segment_task(self: Arc<Self>, index: usize) {
        if self.lock().over {
            return;
        }
        if jobs::cancelled() {
            return self.fail("stopped: another component failed".into());
        }
        let _downloading = progress::activity(progress::Activity::Download);
        let (a, b) = if self.split() { self.range(index) } else { (0, 0) };
        let mut counted = 0u64;
        let mut attempt = 0;
        let body = loop {
            attempt += 1;
            match self.request_range(a, b, &mut counted) {
                Ok(body) => break body,
                Err(e) if attempt < 4 && !jobs::cancelled() => {
                    crate::setup_note!("  retry {} bytes {a}-{b}: {e}", self.request.file_name);
                    std::thread::sleep(Duration::from_millis(500 * attempt));
                }
                Err(e) => return self.fail(e),
            }
        };
        self.store(index, a, body);
    }
    /// One range (or the whole file) over one connection; its bytes move
    /// the bar as they arrive, a retry only once past what was counted.
    fn request_range(self: &Arc<Self>, a: u64, b: u64, counted: &mut u64) -> Result<Vec<u8>, String> {
        let request = &self.request;
        let headers = if self.split() { vec![("Range".to_owned(), format!("bytes={a}-{}", b - 1))] } else { vec![] };
        let seen = Arc::new(AtomicU64::new(0));
        let base = *counted;
        let on_body: http::BodyProgress = {
            let seen = seen.clone();
            let this = self.clone();
            Arc::new(move |loaded: u64, _| {
                let before = seen.swap(loaded, Ordering::Relaxed);
                if loaded <= before {
                    return;
                }
                progress::downloaded(loaded - before);
                // Only bytes past what an earlier attempt counted move the bar.
                let from = before.max(base);
                if loaded > from {
                    progress::advance(loaded - from);
                    let got = this.received.fetch_add(loaded - from, Ordering::Relaxed) + (loaded - from);
                    let last = this.reported.load(Ordering::Relaxed);
                    let total = this.size.max(this.request.size);
                    if got >= last + (1 << 20) || got >= total {
                        this.reported.store(got, Ordering::Relaxed);
                        progress::measured("Download", &this.request.file_name, got, total, progress::Unit::Bytes);
                    }
                }
            })
        };
        let start = Instant::now();
        let ok: &[u16] = if self.split() { &[206] } else { &[200] };
        let result = http::fetch_with("GET", &self.url, &headers, &[], Some(on_body), ok);
        let got = seen.load(Ordering::Relaxed);
        *counted = (*counted).max(got);
        if let Some(env) = jobs::env() {
            env.fetch.record(&request.host, got, start);
        }
        let resp = result?;
        if self.split() && resp.body.len() as u64 != b - a {
            return Err(format!("truncated download {}: {} != {}", request.file_name, resp.body.len(), b - a));
        }
        Ok(resp.body)
    }
    fn store(&self, index: usize, a: u64, body: Vec<u8>) {
        // Write the segment to the part file first, so a crash leaves only
        // listed segments that are really on disk.
        if self.split() && self.request.persist {
            let _write = timing::span(timing::Phase::Write);
            if let Err(e) = write_at(&self.part, self.size, a, &body) {
                return self.fail(format!("write {}: {e}", self.part.display()));
            }
        }
        let mut state = self.lock();
        if state.over || state.done[index] {
            return;
        }
        if self.split() {
            state.buf[a as usize..a as usize + body.len()].copy_from_slice(&body);
            if self.request.persist {
                use std::io::Write;
                if let Ok(mut f) = fs::OpenOptions::new().append(true).open(&self.segs) {
                    let _ = writeln!(f, "{index}");
                }
            }
        } else {
            state.buf = body;
        }
        state.done[index] = true;
        state.remaining -= 1;
        self.hash_ready(&mut state);
        let finished = state.remaining == 0;
        drop(state);
        if finished {
            self.complete_if_done();
        }
    }
    /// Hash the segments that now follow the hashed ones without a gap.
    fn hash_ready(&self, state: &mut State) {
        let mut span = timing::span(timing::Phase::Verify);
        while state.hashed < self.count && state.done[state.hashed] {
            let (a, b) = if self.split() { self.range(state.hashed) } else { (0, state.buf.len() as u64) };
            let bytes = &state.buf[a as usize..b as usize];
            span.bytes(bytes.len() as u64);
            state.hasher.update(bytes);
            state.hashed += 1;
        }
    }
    fn complete_if_done(&self) {
        let mut state = self.lock();
        if state.over || state.remaining > 0 {
            return;
        }
        state.over = true;
        let buf = std::mem::take(&mut state.buf);
        let hasher = std::mem::replace(&mut state.hasher, sha256::Sha256::new());
        drop(state);
        let result = self.publish(buf, hasher);
        if let Some(finish) = self.finish.lock().unwrap_or_else(|e| e.into_inner()).take() {
            finish(result);
        }
    }
    /// Verified, stamped and renamed into the cache, as `http::cached_file` does.
    fn publish(&self, buf: Vec<u8>, hasher: sha256::Sha256) -> Result<Arc<Vec<u8>>, String> {
        let request = &self.request;
        if buf.is_empty() {
            return Err(format!("empty download {}", request.file_name));
        }
        let got = sha256::hex(&hasher.finish());
        if let Some(expect) = &request.sha {
            if !got.eq_ignore_ascii_case(expect) {
                let _ = fs::remove_file(&self.part);
                let _ = fs::remove_file(&self.segs);
                return Err(format!("sha256 mismatch for {}: got {got} want {expect}", request.file_name));
            }
        }
        if !request.persist {
            return Ok(Arc::new(buf));
        }
        let dest = request.dest();
        let ok = http::sidecar(&dest, ".ok");
        let _write = timing::span(timing::Phase::Write);
        {
            let mut file = if self.split() {
                fs::OpenOptions::new().write(true).open(&self.part)
            } else {
                fs::File::create(&self.part).and_then(|mut f| std::io::Write::write_all(&mut f, &buf).map(|_| f))
            }
            .map_err(|e| format!("write {}: {e}", self.part.display()))?;
            file.sync_all().map_err(|e| format!("write {}: {e}", self.part.display()))?;
            let _ = &mut file;
        }
        fs::write(&ok, format!("size={}\nsha256={got}\n", buf.len())).map_err(|e| e.to_string())?;
        if let Err(e) = fs::rename(&self.part, &dest) {
            let _ = fs::remove_file(&self.part);
            return Err(format!("save {}: {e}", request.file_name));
        }
        let _ = fs::remove_file(&self.segs);
        Ok(Arc::new(buf))
    }
    fn fail(&self, error: String) {
        let mut state = self.lock();
        if state.over {
            return;
        }
        state.over = true;
        drop(state);
        if let Some(finish) = self.finish.lock().unwrap_or_else(|e| e.into_inner()).take() {
            finish(Err(error));
        }
    }
}

/// Write `bytes` at `offset` of a file that is `size` long.
fn write_at(path: &Path, size: u64, offset: u64, bytes: &[u8]) -> std::io::Result<()> {
    let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
    if file.metadata()?.len() < size {
        // Grows zero-filled; concurrent writers only ever extend it.
        file.set_len(size)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(bytes, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < bytes.len() {
            let n = file.seek_write(&bytes[done..], offset + done as u64)?;
            if n == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            done += n;
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = file;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(bytes)
    }
}

/// Sizes of files not yet in the cache, asked of their servers in parallel
/// (HEAD); cached files count their size on disk.
pub fn sizes(cache: &Path, urls: &[(String, String)]) -> Vec<u64> {
    let pending: Vec<_> = urls
        .iter()
        .map(|(url, name)| {
            let (cache, url, name) = (cache.to_path_buf(), url.clone(), name.clone());
            jobs::spawn(move || Ok(http::download_size(&cache, &url, &name)))
        })
        .collect();
    pending.into_iter().map(|p| p.wait().unwrap_or(0)).collect()
}
