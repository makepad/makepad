//! The baker: a manifest of image URLs in, a baked tile library out.
//!
//! Deliberately small and linear so it is easy to customise — swap
//! [`parse_manifest`] for your own catalogue walker, or call [`bake`] from
//! your own tool with sources you built any other way. The pipeline is:
//! fetch threads pull bytes off the wire; encode workers decode in RAM,
//! cut the slot pyramid and write the HEVC full/pyramid frames; the packer
//! (this thread) blits slots into the open shard, records the index row and
//! seals full shards to tape. Re-running is cheap: URLs already baked are
//! skipped, and a crash mid-shard resets only that shard's items.

use crate::db::{ShardRow, TileDb};
use crate::library::{mark_no_pyramid, Library};
use crate::tape::{
    box_downscale, build_pyramid, decode_image, fit_dims, image_to_rgba, page_size, write_frame, Planes, TilePyramid,
    FULL_BPP, FULL_MAX_PX, LEVELS, PAGE_BPP, PYRAMID_LEVELS, SHARD_CAP,
};
use makepad_network::blocking_http::{self, Limits, Request};
use std::collections::VecDeque;
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The largest picture download accepted.
pub const PICTURE_MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

#[derive(Clone, Debug)]
pub struct Source {
    pub url: String,
    pub title: String,
    pub link: String,
}

#[derive(Clone, Copy, Debug)]
pub struct BakeOptions {
    /// Network threads: sockets, not cores; raise freely.
    pub fetch_threads: usize,
    /// Decode + HEVC encode workers. Every worker holds a VideoToolbox
    /// compression session per frame it writes; dozens of concurrent
    /// session create/teardowns per second have kernel-panicked an M3 Max
    /// (the AVE encoder's IOMMU falls over), so this stays bounded and
    /// modest — the clamp in [`bake`] is a hard one, not a suggestion.
    pub encode_threads: usize,
    /// Put previously-failed items back in the queue first.
    pub retry_failed: bool,
}

impl Default for BakeOptions {
    fn default() -> BakeOptions {
        BakeOptions { fetch_threads: 6, encode_threads: 4, retry_failed: false }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BakeSummary {
    pub baked: usize,
    pub failed: usize,
    pub skipped: usize,
    pub shards_sealed: usize,
}

/// One manifest line per picture: a bare URL, or `url<TAB>title<TAB>link`.
/// Blank lines and `#` comments are skipped.
pub fn parse_manifest(text: &str) -> Vec<Source> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split('\t');
        let url = parts.next().unwrap_or("").trim().to_string();
        if url.is_empty() {
            continue;
        }
        let title = parts.next().map(str::trim).filter(|t| !t.is_empty()).map(String::from).unwrap_or_else(|| {
            url.rsplit('/').find(|s| !s.is_empty()).unwrap_or("untitled").to_string()
        });
        let link = parts.next().map(str::trim).unwrap_or("").to_string();
        out.push(Source { url, title, link });
    }
    out
}

/// GET with a bounded body and a short redirect chain; the platform client
/// does one hop at a time and never follows a redirect on its own.
pub fn fetch_bytes(url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let limits = Limits { max_body_bytes: max_bytes, total_timeout: Duration::from_secs(120), ..Limits::default() };
        let request = Request::get(current.clone()).limits(limits);
        let response = blocking_http::request_no_redirect(request).map_err(|e| format!("GET {current}: {e:?}"))?;
        match response.status {
            200 => return Ok(response.body),
            301 | 302 | 303 | 307 | 308 => {
                let location = response.header("location").ok_or_else(|| format!("GET {current}: redirect without location"))?;
                current = if location.starts_with("http://") || location.starts_with("https://") {
                    location.to_string()
                } else if let Some(rest) = location.strip_prefix('/') {
                    let origin_end = current.find("://").map(|i| i + 3).unwrap_or(0);
                    let origin = match current[origin_end..].find('/') {
                        Some(i) => &current[..origin_end + i],
                        None => &current,
                    };
                    format!("{origin}/{rest}")
                } else {
                    return Err(format!("GET {current}: unsupported relative redirect {location}"));
                };
            }
            status => return Err(format!("GET {current}: HTTP {status}")),
        }
    }
    Err(format!("GET {url}: too many redirects"))
}

struct Baked {
    width: u32,
    height: u32,
    pyramid: TilePyramid,
}

/// The CPU half of one picture: decode, the slot pyramid, the capped full
/// frame and its pre-cut zoom levels — everything except the shard, which
/// belongs to the packer.
fn process_picture(library: &Library, id: i64, bytes: &[u8]) -> Result<Baked, String> {
    let img = decode_image(bytes)?;
    let (w, h) = (img.width as u32, img.height as u32);
    let rgba = image_to_rgba(&img);
    let pyramid = build_pyramid(&rgba, w, h);
    let (fw, fh) = fit_dims(w, h, FULL_MAX_PX);
    // The fitted buffer is kept: it is also the zoom pyramid's source. The
    // levels are each made from the one above, so the whole pyramid costs
    // about a third more than its top level instead of a multiple of it.
    let fitted: Option<Vec<u8>> = if (fw, fh) == (w, h) { None } else { Some(box_downscale(&rgba, w, h, 4, fw, fh)) };
    let full = Planes::from_rgba(fitted.as_deref().unwrap_or(&rgba), fw, fh);
    write_frame(&library.full_path(id), &full, FULL_BPP).map_err(|e| format!("full frame: {e}"))?;
    let (mut lw, mut lh) = (fw, fh);
    let mut level_rgba: Option<Vec<u8>> = None;
    let mut any = false;
    for px in PYRAMID_LEVELS {
        let (nw, nh) = fit_dims(lw, lh, px);
        if (nw, nh) == (lw, lh) {
            continue;
        }
        let src = level_rgba.as_deref().or(fitted.as_deref()).unwrap_or(&rgba);
        let next = box_downscale(src, lw, lh, 4, nw, nh);
        let frame = Planes::from_rgba(&next, nw, nh);
        write_frame(&library.pyramid_path(id, px), &frame, FULL_BPP).map_err(|e| format!("pyramid {px}: {e}"))?;
        level_rgba = Some(next);
        (lw, lh) = (nw, nh);
        any = true;
    }
    if !any {
        mark_no_pyramid(library, id);
    }
    Ok(Baked { width: w, height: h, pyramid })
}

struct OpenShard {
    id: i64,
    count: u32,
    pages: Vec<Planes>,
}

fn open_shard(id: i64) -> OpenShard {
    OpenShard { id, count: 0, pages: (0..LEVELS).map(|l| Planes::black(page_size(l), page_size(l))).collect() }
}

/// Write a filled (or final partial) shard's five tape frames and mark it
/// sealed. Runs on the packer thread: the writes are serial, which also
/// keeps the encoder-session count honest.
fn seal(library: &Library, db: &mut TileDb, shard: OpenShard) -> Result<(), String> {
    for (level, page) in shard.pages.iter().enumerate() {
        write_frame(&library.tape_path(shard.id, level), page, PAGE_BPP).map_err(|e| format!("tape {} L{level}: {e}", shard.id))?;
    }
    db.upsert_shard(ShardRow { id: shard.id, count: shard.count as i64, sealed: true })
}

enum PackMsg {
    Baked { id: i64, baked: Baked },
    Failed { id: i64, error: String },
}

/// Bake `sources` into the library at `root`. Safe to re-run: known URLs
/// keep their pixels, only pending (and, with `retry_failed`, failed) items
/// are fetched. `log` gets one line per notable event.
pub fn bake(
    root: &std::path::Path,
    sources: &[Source],
    options: &BakeOptions,
    log: &mut dyn FnMut(String),
) -> Result<BakeSummary, String> {
    let library = Library::new(root);
    library.ensure_dirs()?;
    let mut db = TileDb::open(&library.db_path())?;
    let reset = db.reset_unsealed_shards()?;
    if reset > 0 {
        log(format!("{reset} unsealed shard(s) from an interrupted run reset"));
    }
    for s in sources {
        db.add_source(&s.url, &s.title, &s.link)?;
    }
    if options.retry_failed {
        let retried = db.retry_failed()?;
        if retried > 0 {
            log(format!("{retried} failed item(s) back in the queue"));
        }
    }
    let pending = db.pending()?;
    let (already_pending, ready, failed_before) = db.counts()?;
    let _ = already_pending;
    let mut summary = BakeSummary { skipped: ready as usize, ..Default::default() };
    if pending.is_empty() {
        log(format!("nothing to bake: {ready} ready, {failed_before} failed"));
        return Ok(summary);
    }
    log(format!("baking {} picture(s) ({ready} already in the library)", pending.len()));

    let fetch_threads = options.fetch_threads.clamp(1, 32);
    // The hard encoder cap — see BakeOptions::encode_threads.
    let encode_threads = options.encode_threads.clamp(1, 8);

    let (job_tx, job_rx) = mpsc::channel::<(i64, String)>();
    for job in &pending {
        let _ = job_tx.send(job.clone());
    }
    drop(job_tx);
    let job_rx = Arc::new(Mutex::new(job_rx));
    // Bounded: fetched bytes wait here for a core, and a fast line must not
    // move the whole manifest into RAM.
    let (fetched_tx, fetched_rx) = mpsc::sync_channel::<(i64, Vec<u8>)>(encode_threads);
    let fetched_rx = Arc::new(Mutex::new(fetched_rx));
    let (done_tx, done_rx) = mpsc::channel::<PackMsg>();

    let total = pending.len();
    let result = std::thread::scope(|scope| -> Result<(), String> {
        for _ in 0..fetch_threads {
            let job_rx = job_rx.clone();
            let fetched_tx: SyncSender<(i64, Vec<u8>)> = fetched_tx.clone();
            let done_tx = done_tx.clone();
            scope.spawn(move || loop {
                let job = { job_rx.lock().unwrap().recv() };
                let Ok((id, url)) = job else { break };
                match fetch_bytes(&url, PICTURE_MAX_BYTES) {
                    Ok(bytes) => {
                        if fetched_tx.send((id, bytes)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        if done_tx.send(PackMsg::Failed { id, error }).is_err() {
                            break;
                        }
                    }
                }
            });
        }
        drop(fetched_tx);
        for _ in 0..encode_threads {
            let fetched_rx = fetched_rx.clone();
            let done_tx = done_tx.clone();
            let library = library.clone();
            scope.spawn(move || loop {
                let job = { fetched_rx.lock().unwrap().recv() };
                let Ok((id, bytes)) = job else { break };
                let msg = match process_picture(&library, id, &bytes) {
                    Ok(baked) => PackMsg::Baked { id, baked },
                    Err(error) => PackMsg::Failed { id, error },
                };
                if done_tx.send(msg).is_err() {
                    break;
                }
            });
        }
        drop(done_tx);

        // The packer: this thread. One open shard, slots filled in arrival
        // order, sealed when full.
        let mut open: Option<OpenShard> = None;
        let mut next_shard = db.next_shard()?;
        let mut taken = 0usize;
        let mut last_report = Instant::now();
        let mut recent_errors: VecDeque<String> = VecDeque::new();
        while let Ok(msg) = done_rx.recv() {
            taken += 1;
            match msg {
                PackMsg::Baked { id, baked } => {
                    let shard = open.get_or_insert_with(|| {
                        let s = open_shard(next_shard);
                        next_shard += 1;
                        s
                    });
                    let slot = shard.count;
                    for (level, planes) in baked.pyramid.levels.iter().enumerate() {
                        let (x, y) = crate::tape::slot_origin(slot, level);
                        shard.pages[level].blit(planes, x, y);
                    }
                    shard.count += 1;
                    db.upsert_shard(ShardRow { id: shard.id, count: shard.count as i64, sealed: false })?;
                    db.set_ready(id, baked.width, baked.height, shard.id, slot)?;
                    summary.baked += 1;
                    if shard.count >= SHARD_CAP {
                        let full = open.take().unwrap();
                        seal(&library, &mut db, full)?;
                        summary.shards_sealed += 1;
                    }
                }
                PackMsg::Failed { id, error } => {
                    db.set_failed(id, &error)?;
                    summary.failed += 1;
                    recent_errors.push_back(error);
                    if recent_errors.len() > 3 {
                        recent_errors.pop_front();
                    }
                }
            }
            if last_report.elapsed().as_secs_f64() > 2.0 || taken == total {
                last_report = Instant::now();
                let mut line = format!("baked {}/{total} ({} failed)", summary.baked, summary.failed);
                for e in recent_errors.drain(..) {
                    line.push_str(&format!("\n  {e}"));
                }
                log(line);
            }
        }
        if let Some(shard) = open.take() {
            seal(&library, &mut db, shard)?;
            summary.shards_sealed += 1;
        }
        Ok(())
    });
    result?;
    let (pending_after, ready_after, failed_after) = db.counts()?;
    log(format!(
        "done: {ready_after} ready, {failed_after} failed, {pending_after} pending, {} shard(s) sealed this run",
        summary.shards_sealed
    ));
    Ok(summary)
}
