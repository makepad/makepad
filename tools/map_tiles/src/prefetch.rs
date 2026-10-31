//! `mkmap-prefetch`: fill the app's own archive range cache
//! (`makepad_home()/maps/cache/<url hash>/`) with every tile of a bbox, so
//! the map renders there with no network. The entries are what the hosted
//! reader writes: the root index, each needed leaf directory, and the tile
//! blobs (coalesced into larger ranges, which the cache serves by
//! containment), so the app finds them exactly as if it had browsed there.
//!
//! Resumable: a range already on disk is skipped. Run it again after an
//! interruption, or with a larger bbox / zoom range, and only the rest is
//! fetched.

use makepad_map_build::versatiles::GeoBounds;
use makepad_mbtile_reader::{mkmap_tile_id, BlobRef, MkmapLeaf, MkmapRoot};
use makepad_network::blocking_http::{self, Limits, Request};
use makepad_platform::archive_cache::ArchiveCacheStore;
use makepad_platform::home::makepad_home;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_GAP: u64 = 64 * 1024;
const MAX_RANGE: u64 = 4 * 1024 * 1024;
const THREADS: usize = 4;
/// The app's own budget; a fill larger than this would be swept by the app.
const CACHE_BUDGET: u64 = 8 * 1024 * 1024 * 1024;

pub struct PrefetchOptions {
    pub url: String,
    pub bounds: GeoBounds,
    pub min_zoom: Option<u8>,
    pub max_zoom: Option<u8>,
    /// Rings of extra tiles around the bbox at each zoom, so the viewport
    /// edge and label placement find their neighbours.
    pub pad_tiles: u32,
    pub cache_root: Option<PathBuf>,
    /// Count and report; fetch only the root index and leaf directories.
    pub plan_only: bool,
    /// Offline check: no network at all; read every tile of the region
    /// back through the cache the way the app does and decode it.
    pub verify: bool,
}

pub fn parse_options(args: &[String]) -> Result<PrefetchOptions, String> {
    let mut url = None;
    let mut bounds = None;
    let mut options = PrefetchOptions {
        url: String::new(),
        bounds: GeoBounds { west: 0.0, south: 0.0, east: 0.0, north: 0.0 },
        min_zoom: None,
        max_zoom: None,
        pad_tiles: 1,
        cache_root: None,
        plan_only: false,
        verify: false,
    };
    let mut index = 1;
    while index < args.len() {
        let value = |index: usize| -> Result<&String, String> {
            args.get(index + 1)
                .ok_or_else(|| format!("{} needs a value", args[index]))
        };
        match args[index].as_str() {
            "--bbox" => {
                let parts = value(index)?
                    .split(',')
                    .map(|part| part.trim().parse::<f64>().map_err(|e| format!("bbox: {e}")))
                    .collect::<Result<Vec<_>, _>>()?;
                if parts.len() != 4 || parts[0] >= parts[2] || parts[1] >= parts[3] {
                    return Err("--bbox wants west,south,east,north".to_string());
                }
                bounds = Some(GeoBounds {
                    west: parts[0],
                    south: parts[1],
                    east: parts[2],
                    north: parts[3],
                });
                index += 2;
            }
            "--min-zoom" => {
                options.min_zoom = Some(value(index)?.parse().map_err(|e| format!("{e}"))?);
                index += 2;
            }
            "--max-zoom" => {
                options.max_zoom = Some(value(index)?.parse().map_err(|e| format!("{e}"))?);
                index += 2;
            }
            "--pad-tiles" => {
                options.pad_tiles = value(index)?.parse().map_err(|e| format!("{e}"))?;
                index += 2;
            }
            "--cache-root" => {
                options.cache_root = Some(PathBuf::from(value(index)?));
                index += 2;
            }
            "--verify" => {
                options.verify = true;
                index += 1;
            }
            "--plan-only" => {
                options.plan_only = true;
                index += 1;
            }
            other if !other.starts_with("--") && url.is_none() => {
                url = Some(other.to_string());
                index += 1;
            }
            other => return Err(format!("unknown mkmap-prefetch argument '{other}'")),
        }
    }
    options.url = url.ok_or("mkmap-prefetch needs <archive url>")?;
    options.bounds = bounds.ok_or("mkmap-prefetch needs --bbox w,s,e,n")?;
    Ok(options)
}

fn get(url: &str, range: Option<(u64, u64)>, max_body: usize) -> Result<Vec<u8>, String> {
    let limits = Limits {
        max_body_bytes: max_body,
        idle_timeout: Duration::from_secs(120),
        ..Limits::default()
    };
    let mut last = String::new();
    for _ in 0..4 {
        let mut request = Request::get(url.to_string()).limits(limits);
        if let Some((offset, len)) = range {
            request = request
                .header("Range", &format!("bytes={}-{}", offset, offset + len - 1))
                .map_err(|e| format!("{e}"))?;
        }
        match blocking_http::request_no_redirect(request) {
            Ok(response) if response.status == 200 || response.status == 206 => {
                if range.is_some_and(|(_, len)| response.body.len() as u64 != len) {
                    last = format!("{url}: got {} bytes", response.body.len());
                    continue;
                }
                return Ok(response.body);
            }
            Ok(response) => last = format!("{url}: HTTP {}", response.status),
            Err(err) => last = format!("{url}: {err}"),
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err(last)
}

struct LeafWork {
    start: u64,
    end: u64,
    shard: u32,
    dir_offset: u64,
    dir_len: u64,
    ids: Vec<u64>,
}

pub fn prefetch(options: PrefetchOptions) -> Result<(), String> {
    let started = Instant::now();
    // The reader trims the trailing slash before keying its cache.
    let url = options.url.trim_end_matches('/');
    let cache_root = options
        .cache_root
        .clone()
        .unwrap_or_else(|| makepad_home().join("maps/cache"));
    let mut store = ArchiveCacheStore::open_at(&cache_root, url, CACHE_BUDGET)
        .map_err(|e| format!("open cache {}: {e}", cache_root.display()))?;
    let archive_dir = store.archive_dir().to_path_buf();
    let root_bytes = match store.read_root() {
        Some(bytes) => bytes,
        None if options.verify => return Err("verify: root index is not cached".to_string()),
        None => {
            let bytes = get(&format!("{url}/root.mkidx"), None, 64 * 1024 * 1024)?;
            store.write_root(&bytes).map_err(|e| format!("write root: {e}"))?;
            bytes
        }
    };
    let root = MkmapRoot::parse(&root_bytes)?;
    let (archive_min, archive_max) = root.zoom_range();
    let min_zoom = options.min_zoom.map_or(archive_min, |z| u32::from(z).max(archive_min));
    let max_zoom = options.max_zoom.map_or(archive_max, |z| u32::from(z).min(archive_max));
    println!(
        "{url}: zooms {archive_min}..{archive_max}, fetching {min_zoom}..{max_zoom}, cache {}",
        archive_dir.display()
    );

    // Tile ids of the bbox at every wanted zoom, grouped by the root record
    // (leaf directory) that holds them.
    let pad = options.pad_tiles;
    let mut wanted: BTreeMap<usize, LeafWork> = BTreeMap::new();
    let mut tile_total = 0_u64;
    for zoom in min_zoom..=max_zoom {
        let zoom = zoom as u8;
        let last = (1_u32 << zoom) - 1;
        let rect = options.bounds.tile_bounds(zoom);
        for y in rect.y_min.saturating_sub(pad)..=rect.y_max.saturating_add(pad).min(last) {
            for x in rect.x_min.saturating_sub(pad)..=rect.x_max.saturating_add(pad).min(last) {
                let id = mkmap_tile_id(zoom, x, y);
                if let Some(record) = root.locate(id) {
                    tile_total += 1;
                    wanted
                        .entry(record.index)
                        .or_insert_with(|| LeafWork {
                            start: record.start_tile_id,
                            end: record.end_tile_id,
                            shard: record.shard,
                            dir_offset: record.dir_offset,
                            dir_len: record.dir_len,
                            ids: Vec::new(),
                        })
                        .ids
                        .push(id);
                }
            }
        }
    }
    println!("{tile_total} tile positions in {} leaf directories", wanted.len());

    // Leaf directories: cache each exactly as the reader requests it, then
    // read the blob locations out of it.
    let shard_count = root.shard_count();
    let mut blobs = BTreeSet::<(u32, u64, u64)>::new();
    for work in wanted.values() {
        let packed = match store.read_range(work.shard, work.dir_offset, work.dir_len) {
            Some(bytes) => bytes,
            None if options.verify => {
                return Err(format!(
                    "verify: leaf directory {}:{} is not cached",
                    work.shard, work.dir_offset
                ));
            }
            None => {
                let bytes = get(
                    &format!("{url}/tiles-{:03}.mkshard", work.shard),
                    Some((work.dir_offset, work.dir_len)),
                    work.dir_len as usize,
                )?;
                store
                    .write_range(work.shard, work.dir_offset, &bytes)
                    .map_err(|e| format!("write leaf: {e}"))?;
                bytes
            }
        };
        let leaf = MkmapLeaf::parse_for_root(&packed, shard_count, work.start, work.end)?;
        for id in &work.ids {
            if let Some(BlobRef { shard, offset, len }) = leaf.find(*id) {
                blobs.insert((shard, offset, len));
            }
        }
    }

    if options.verify {
        let mut missing = 0_usize;
        for (shard, offset, len) in &blobs {
            let readable = store
                .read_range(*shard, *offset, *len)
                .is_some_and(|bytes| root.decode_blob(&bytes).is_ok());
            missing += usize::from(!readable);
        }
        println!(
            "verify: {} tile blobs read back from the cache and decoded, {} missing",
            blobs.len(),
            missing
        );
        return if missing == 0 {
            Ok(())
        } else {
            Err(format!("{missing} tiles are not cached"))
        };
    }

    // Blobs -> coalesced ranges.
    let mut ranges = Vec::<(u32, u64, u64)>::new();
    for (shard, offset, len) in blobs.iter().copied() {
        if let Some(last) = ranges.last_mut() {
            let end = last.1 + last.2;
            if last.0 == shard && offset <= end + MAX_GAP && offset + len - last.1 <= MAX_RANGE {
                last.2 = last.2.max(offset + len - last.1);
                continue;
            }
        }
        ranges.push((shard, offset, len));
    }
    let todo = ranges
        .iter()
        .filter(|(shard, offset, len)| {
            !archive_dir
                .join(format!("{shard:03}/{offset}-{len}.bin"))
                .is_file()
        })
        .copied()
        .collect::<Vec<_>>();
    let total_bytes: u64 = ranges.iter().map(|r| r.2).sum();
    let todo_bytes: u64 = todo.iter().map(|r| r.2).sum();
    println!(
        "{} blobs, {} ranges, {:.1} MB total; {} ranges / {:.1} MB still to fetch",
        blobs.len(),
        ranges.len(),
        total_bytes as f64 / 1e6,
        todo.len(),
        todo_bytes as f64 / 1e6
    );
    if options.plan_only || todo.is_empty() {
        return Ok(());
    }

    let store = Mutex::new(store);
    let next = AtomicUsize::new(0);
    let done_bytes = AtomicU64::new(0);
    let failures = Mutex::new(Vec::<String>::new());
    std::thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(shard, offset, len)) = todo.get(index) else {
                    break;
                };
                let result = get(
                    &format!("{url}/tiles-{shard:03}.mkshard"),
                    Some((offset, len)),
                    len as usize,
                )
                .and_then(|bytes| {
                    store
                        .lock()
                        .unwrap()
                        .write_range(shard, offset, &bytes)
                        .map_err(|e| format!("write range: {e}"))
                });
                match result {
                    Ok(()) => {
                        let done = done_bytes.fetch_add(len, Ordering::Relaxed) + len;
                        if index % 100 == 0 {
                            println!(
                                "  {:.1}% ({:.1} / {:.1} MB, {:.0}s)",
                                done as f64 * 100.0 / todo_bytes as f64,
                                done as f64 / 1e6,
                                todo_bytes as f64 / 1e6,
                                started.elapsed().as_secs_f64()
                            );
                        }
                    }
                    Err(err) => failures.lock().unwrap().push(err),
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    if !failures.is_empty() {
        return Err(format!(
            "{} ranges failed (rerun to resume); first: {}",
            failures.len(),
            failures[0]
        ));
    }
    println!("done in {:.0}s", started.elapsed().as_secs_f64());
    Ok(())
}
