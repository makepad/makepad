//! The threaded face of the client: commands in, events out, nothing on
//! the caller's thread but a channel drain.
//!
//! Three lanes, because the three kinds of work have three kinds of cost
//! and must not queue behind each other:
//!
//! * **control** (one thread) — searches and item lookups. Sub-second each;
//!   a job whose generation the host has already superseded is skipped
//!   unstarted, so typing fast never runs a backlog of dead searches.
//! * **thumbs** (a small pool) — tile pictures, a page of them at a time,
//!   each cached on disk after its first fetch. Jobs from an earlier
//!   search page are dropped unstarted and reported as such, so the host
//!   can clear its in-flight mark and ask again if it still wants them.
//! * **bulk** (one thread) — media downloads. A new download cancels the
//!   one in flight: on a picture wall there is only ever one clip being
//!   auditioned, and the operator has moved on.
//!
//! Every event carries the generation of the command it answers; the host
//! discards anything older than what it last asked for.

use crate::cache::{cache_file_for, head_file_for, part_file_for, thumb_file_for};
use crate::http::{download_head_to_file, download_to_file, fetch_bytes, Error, Progress};
use crate::item::{parse_item, Item, ItemFile};
use crate::search::{parse_search, SearchPage, SearchQuery};
use crate::url::{is_valid_identifier, metadata_url, thumb_url};
use crate::{MAX_IMPORT_BYTES, MAX_JSON_BYTES, MAX_THUMB_BYTES, PREVIEW_HEAD_BYTES};
use makepad_network::blocking_http::CancelToken;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;

/// Why a file is being downloaded — it sets the size ceiling and tells
/// the host which of its two flows the finished path belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Preview,
    Import,
}

impl Purpose {
    /// The refusal threshold for a WHOLE-file download. A preview is
    /// never refused: past this it is fetched as a head instead.
    pub fn max_bytes(self) -> u64 {
        match self {
            Purpose::Preview => PREVIEW_HEAD_BYTES,
            Purpose::Import => MAX_IMPORT_BYTES,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Cmd {
    Search { gen: u64, query: SearchQuery },
    Item { gen: u64, identifier: String },
    /// Fetch (or read from cache) the item tile; `epoch` is the search
    /// page it belongs to.
    Thumb { epoch: u64, identifier: String },
    Download { gen: u64, identifier: String, file: ItemFile, purpose: Purpose },
}

#[derive(Clone, Debug)]
pub enum Ev {
    Search { gen: u64, result: Result<SearchPage, Error> },
    Item { gen: u64, result: Result<Item, Error> },
    /// Encoded JPEG/PNG bytes of the tile, or why not. `Err(Cancelled)`
    /// means the job was dropped unstarted because a newer page replaced
    /// it — not a failure, and not a picture.
    Thumb { identifier: String, result: Result<Vec<u8>, Error> },
    /// The body has started streaming into `part` (a growing file with
    /// the media's own extension). A host may open it now and play what
    /// has landed; `Download` follows with the final path when the last
    /// byte is in. Not sent for a cache hit — that goes straight to
    /// `Download`.
    DownloadStarted {
        gen: u64,
        purpose: Purpose,
        identifier: String,
        file: ItemFile,
        part: PathBuf,
        /// What the progress counts to: the file, or the head cap.
        total: Option<u64>,
        /// `Some(cap)` when only the first `cap` bytes are being fetched
        /// (a preview of something bigger than the cap).
        head: Option<u64>,
    },
    Progress { gen: u64, purpose: Purpose, progress: Progress },
    Download {
        gen: u64,
        purpose: Purpose,
        identifier: String,
        file: ItemFile,
        result: Result<PathBuf, Error>,
    },
}

type Job = Box<dyn FnOnce() + Send + 'static>;

/// N threads draining one queue.
struct Lane {
    tx: Sender<Job>,
}

impl Lane {
    fn spawn(name: &'static str, threads: usize) -> Lane {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..threads.max(1) {
            let rx = rx.clone();
            thread::Builder::new()
                .name(format!("archive-{name}-{i}"))
                .spawn(move || loop {
                    // Hold the lock only while waiting for a job; run it
                    // with the lock released so the rest of the pool keeps
                    // draining.
                    let job = match rx.lock() {
                        Ok(guard) => guard.recv(),
                        Err(_) => return,
                    };
                    match job {
                        Ok(job) => job(),
                        Err(_) => return,
                    }
                })
                .expect("spawn archive lane thread");
        }
        Lane { tx }
    }

    fn submit(&self, job: Job) {
        let _ = self.tx.send(job);
    }
}

pub struct ArchiveWorker {
    cache_dir: PathBuf,
    control: Lane,
    thumbs: Lane,
    bulk: Lane,
    ev_tx: Sender<Ev>,
    ev_rx: Receiver<Ev>,
    /// Newest search / item generations handed in, so stale jobs can skip.
    latest_search: Arc<AtomicU64>,
    latest_item: Arc<AtomicU64>,
    latest_thumb_epoch: Arc<AtomicU64>,
    /// The download in flight (or queued); replaced — and cancelled — by
    /// the next one.
    download_cancel: Arc<Mutex<Option<CancelToken>>>,
}

impl ArchiveWorker {
    /// Start the lanes. `cache_dir` receives `thumbs/` and `media/`.
    pub fn spawn(cache_dir: PathBuf) -> ArchiveWorker {
        let (ev_tx, ev_rx) = mpsc::channel();
        ArchiveWorker {
            cache_dir,
            control: Lane::spawn("control", 1),
            thumbs: Lane::spawn("thumb", 4),
            bulk: Lane::spawn("bulk", 1),
            ev_tx,
            ev_rx,
            latest_search: Arc::new(AtomicU64::new(0)),
            latest_item: Arc::new(AtomicU64::new(0)),
            latest_thumb_epoch: Arc::new(AtomicU64::new(0)),
            download_cancel: Arc::new(Mutex::new(None)),
        }
    }

    pub fn cache_dir(&self) -> &PathBuf {
        &self.cache_dir
    }

    /// Drain everything the lanes have reported since the last poll.
    pub fn poll(&self) -> Vec<Ev> {
        let mut out = Vec::new();
        loop {
            match self.ev_rx.try_recv() {
                Ok(ev) => out.push(ev),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }

    /// Stop the download in flight (and any queued behind it). The bulk
    /// lane reports it as `Err(Cancelled)`.
    pub fn cancel_download(&self) {
        if let Ok(mut slot) = self.download_cancel.lock() {
            if let Some(token) = slot.take() {
                token.cancel();
            }
        }
    }

    pub fn send(&self, cmd: Cmd) {
        match cmd {
            Cmd::Search { gen, query } => {
                self.latest_search.fetch_max(gen, Ordering::AcqRel);
                let latest = self.latest_search.clone();
                let tx = self.ev_tx.clone();
                self.control.submit(Box::new(move || {
                    if latest.load(Ordering::Acquire) != gen {
                        return;
                    }
                    let result = fetch_bytes(&query.url(), MAX_JSON_BYTES, &CancelToken::new())
                        .and_then(|bytes| {
                            let text = String::from_utf8(bytes)
                                .map_err(|_| Error::Json("search page is not utf-8".into()))?;
                            parse_search(&text, &query)
                        });
                    let _ = tx.send(Ev::Search { gen, result });
                }));
            }
            Cmd::Item { gen, identifier } => {
                self.latest_item.fetch_max(gen, Ordering::AcqRel);
                let latest = self.latest_item.clone();
                let tx = self.ev_tx.clone();
                self.control.submit(Box::new(move || {
                    if latest.load(Ordering::Acquire) != gen {
                        return;
                    }
                    let result = if !is_valid_identifier(&identifier) {
                        Err(Error::InvalidUrl)
                    } else {
                        fetch_bytes(&metadata_url(&identifier), MAX_JSON_BYTES, &CancelToken::new())
                            .and_then(|bytes| {
                                let text = String::from_utf8(bytes)
                                    .map_err(|_| Error::Json("metadata is not utf-8".into()))?;
                                parse_item(&text)
                            })
                    };
                    let _ = tx.send(Ev::Item { gen, result });
                }));
            }
            Cmd::Thumb { epoch, identifier } => {
                self.latest_thumb_epoch.fetch_max(epoch, Ordering::AcqRel);
                let latest = self.latest_thumb_epoch.clone();
                let tx = self.ev_tx.clone();
                let path = thumb_file_for(&self.cache_dir, &identifier);
                self.thumbs.submit(Box::new(move || {
                    if latest.load(Ordering::Acquire) != epoch {
                        let _ = tx.send(Ev::Thumb { identifier, result: Err(Error::Cancelled) });
                        return;
                    }
                    if !is_valid_identifier(&identifier) {
                        let _ = tx.send(Ev::Thumb { identifier, result: Err(Error::InvalidUrl) });
                        return;
                    }
                    if let Ok(bytes) = std::fs::read(&path) {
                        if !bytes.is_empty() {
                            let _ = tx.send(Ev::Thumb { identifier, result: Ok(bytes) });
                            return;
                        }
                    }
                    let result =
                        fetch_bytes(&thumb_url(&identifier), MAX_THUMB_BYTES, &CancelToken::new());
                    if let Ok(bytes) = &result {
                        if let Some(dir) = path.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        // Best effort: a cache miss next time is only a refetch.
                        let tmp = path.with_extension("jpg.part");
                        if std::fs::write(&tmp, bytes).is_ok() {
                            let _ = std::fs::rename(&tmp, &path);
                        }
                    }
                    let _ = tx.send(Ev::Thumb { identifier, result });
                }));
            }
            Cmd::Download { gen, identifier, file, purpose } => {
                let cancel = CancelToken::new();
                if let Ok(mut slot) = self.download_cancel.lock() {
                    if let Some(previous) = slot.replace(cancel.clone()) {
                        previous.cancel();
                    }
                }
                let tx = self.ev_tx.clone();
                let whole = cache_file_for(&self.cache_dir, &identifier, &file.name);
                // A preview of something bigger than the head cap fetches
                // (and caches) only its head, under the head's own name.
                let head = (purpose == Purpose::Preview && file.size > PREVIEW_HEAD_BYTES)
                    .then_some(PREVIEW_HEAD_BYTES);
                let dest = if head.is_some() { head_file_for(&whole) } else { whole };
                self.bulk.submit(Box::new(move || {
                    let result =
                        download_job(&identifier, &file, purpose, head, &dest, &cancel, gen, &tx);
                    let _ = tx.send(Ev::Download { gen, purpose, identifier, file, result });
                }));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn download_job(
    identifier: &str,
    file: &ItemFile,
    purpose: Purpose,
    head: Option<u64>,
    dest: &PathBuf,
    cancel: &CancelToken,
    gen: u64,
    tx: &Sender<Ev>,
) -> Result<PathBuf, Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if !is_valid_identifier(identifier) {
        return Err(Error::InvalidUrl);
    }
    let max = purpose.max_bytes();
    if head.is_none() && file.size > max {
        return Err(Error::TooLarge);
    }
    // Cache hit: the archive's own size for the file is the check — a
    // partial from a killed run never keeps the final name, so a file that
    // exists at the right size IS the download. A head is whole at the cap.
    let want = match head {
        Some(cap) => cap.min(file.size),
        None => file.size,
    };
    if let Ok(meta) = std::fs::metadata(dest) {
        if meta.is_file() && meta.len() > 0 && (want == 0 || meta.len() == want) {
            let _ = tx.send(Ev::Progress {
                gen,
                purpose,
                progress: Progress { loaded: meta.len(), total: Some(meta.len()) },
            });
            return Ok(dest.clone());
        }
    }
    let url = file.download_url(identifier);
    let part = part_file_for(dest);
    let mut started = false;
    let mut report = |progress: Progress| {
        // The first report comes right after a 200 head, with the part
        // file created: that is the moment a host may start reading it.
        if !started {
            started = true;
            let _ = tx.send(Ev::DownloadStarted {
                gen,
                purpose,
                identifier: identifier.to_string(),
                file: file.clone(),
                part: part.clone(),
                total: progress.total,
                head,
            });
        }
        let _ = tx.send(Ev::Progress { gen, purpose, progress });
    };
    match head {
        Some(cap) => {
            download_head_to_file(&url, dest, cap, cancel, &mut report)?;
        }
        None => {
            download_to_file(&url, dest, max, cancel, &mut report)?;
        }
    }
    Ok(dest.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn drain(worker: &ArchiveWorker, want: usize) -> Vec<Ev> {
        let start = Instant::now();
        let mut out = Vec::new();
        while out.len() < want && start.elapsed() < Duration::from_secs(5) {
            out.extend(worker.poll());
            thread::sleep(Duration::from_millis(5));
        }
        out
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("makepad-archive-org-test-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stale_search_is_skipped_and_invalid_identifier_refused() {
        let worker = ArchiveWorker::spawn(scratch("stale"));
        // Two searches in a row: only the newer one may run. Neither
        // touches the network here — the lane checks staleness before it
        // fetches, and the newer one fails fast on an invalid identifier
        // path instead (Item with a bad id).
        worker.send(Cmd::Item { gen: 1, identifier: "bad id".into() });
        worker.send(Cmd::Item { gen: 2, identifier: "also bad".into() });
        let evs = drain(&worker, 1);
        assert_eq!(evs.len(), 1, "the superseded lookup never ran");
        match &evs[0] {
            Ev::Item { gen: 2, result: Err(Error::InvalidUrl) } => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn thumb_cache_hit_and_stale_epoch() {
        let dir = scratch("thumb");
        let worker = ArchiveWorker::spawn(dir.clone());
        let path = thumb_file_for(&dir, "cached_item");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"\xff\xd8jpeg").unwrap();
        worker.send(Cmd::Thumb { epoch: 7, identifier: "cached_item".into() });
        let evs = drain(&worker, 1);
        match &evs[0] {
            Ev::Thumb { identifier, result: Ok(bytes) } => {
                assert_eq!(identifier, "cached_item");
                assert_eq!(bytes, b"\xff\xd8jpeg");
            }
            other => panic!("unexpected {other:?}"),
        }
        // An older epoch is dropped unstarted and says so.
        worker.send(Cmd::Thumb { epoch: 9, identifier: "bad id".into() });
        worker.send(Cmd::Thumb { epoch: 8, identifier: "cached_item".into() });
        let evs = drain(&worker, 2);
        assert!(evs.iter().any(|e| matches!(e, Ev::Thumb { result: Err(Error::Cancelled), .. })));
        assert!(evs.iter().any(|e| matches!(e, Ev::Thumb { result: Err(Error::InvalidUrl), .. })));
    }

    #[test]
    fn download_cache_hit_and_size_gate() {
        let dir = scratch("dl");
        let worker = ArchiveWorker::spawn(dir.clone());
        let file = ItemFile {
            name: "clip.mp4".into(),
            source: crate::item::FileSource::Original,
            format: "MPEG4".into(),
            size: 4,
            width: 0,
            height: 0,
            length_secs: 0.0,
            md5: String::new(),
        };
        let dest = cache_file_for(&dir, "item_x", "clip.mp4");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"mp4!").unwrap();
        worker.send(Cmd::Download {
            gen: 1,
            identifier: "item_x".into(),
            file: file.clone(),
            purpose: Purpose::Preview,
        });
        let evs = drain(&worker, 2);
        assert!(evs.iter().any(|e| matches!(e, Ev::Download { gen: 1, result: Ok(p), .. } if *p == dest)));
        let huge = ItemFile { size: crate::MAX_IMPORT_BYTES + 1, ..file.clone() };
        worker.send(Cmd::Download {
            gen: 2,
            identifier: "item_x".into(),
            file: huge,
            purpose: Purpose::Import,
        });
        let evs = drain(&worker, 1);
        assert!(evs.iter().any(|e| matches!(e, Ev::Download { gen: 2, result: Err(Error::TooLarge), .. })));
        // A preview of something over the head cap is served from its
        // cached HEAD (whole at the cap), never refused.
        let big = ItemFile { size: PREVIEW_HEAD_BYTES * 4, ..file };
        let head = head_file_for(&dest);
        std::fs::write(&head, vec![0u8; PREVIEW_HEAD_BYTES as usize]).unwrap();
        worker.send(Cmd::Download {
            gen: 3,
            identifier: "item_x".into(),
            file: big,
            purpose: Purpose::Preview,
        });
        let evs = drain(&worker, 2);
        assert!(evs.iter().any(|e| matches!(e, Ev::Download { gen: 3, result: Ok(p), .. } if *p == head)));
    }
}
