//! Continuous AI-content library publication.
//!
//! The AI-content library commits payload bytes before atomically replacing
//! `index.json`. We still treat every observation as potentially foreign or
//! incomplete: a row must have identical payload metadata for a full poll
//! interval before import, the importer rechecks metadata around its read and
//! probe, and failures retry with bounded exponential backoff. Successfully
//! published/already-present/skipped rows are remembered by a metadata +
//! index-row fingerprint, so steady-state polls do no network or media work.

use crate::import::{self, IndexItem};
use makepad_asset_client::AssetClient;
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

const POLL_MS: u64 = 1_000;
const STABLE_MS: u64 = 1_000;
const RETRY_MAX_MS: u64 = 30_000;
const STOP_SLICE_MS: u64 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified_ns: u128,
    row_hash: u64,
}

impl FileStamp {
    fn read(dir: &Path, item: &IndexItem) -> Result<Self, String> {
        let path = dir.join(&item.file);
        let meta = std::fs::metadata(&path)
            .map_err(|error| format!("{}: {error}", item.file))?;
        if !meta.is_file() {
            return Err(format!("{}: payload is not a file", item.file));
        }
        let modified_ns = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let mut row = DefaultHasher::new();
        item.hash(&mut row);
        Ok(Self {
            len: meta.len(),
            modified_ns,
            row_hash: row.finish(),
        })
    }
}

#[derive(Clone, Debug)]
struct Observed {
    stamp: FileStamp,
    stable_since_ms: u64,
    complete: bool,
    failures: u32,
    retry_at_ms: u64,
}

#[derive(Clone, Debug)]
struct ReadyItem {
    item: IndexItem,
    stamp: FileStamp,
}

#[derive(Default)]
struct WatchState {
    files: HashMap<String, Observed>,
}

impl WatchState {
    /// Observe one committed index snapshot and return only rows whose exact
    /// payload+metadata fingerprint is both new/changed and stable.
    fn observe(&mut self, dir: &Path, items: &[IndexItem], now_ms: u64) -> Vec<ReadyItem> {
        let indexed: HashSet<&str> = items.iter().map(|item| item.file.as_str()).collect();
        self.files.retain(|file, _| indexed.contains(file.as_str()));

        let mut ready = Vec::new();
        for item in items {
            let Ok(stamp) = FileStamp::read(dir, item) else {
                // Missing/being-renamed payloads are normal transient states.
                // Keep any previous observation; a changed stamp resets it
                // as soon as the committed file appears.
                continue;
            };
            let observed = self.files.entry(item.file.clone()).or_insert_with(|| Observed {
                stamp,
                stable_since_ms: now_ms,
                complete: false,
                failures: 0,
                retry_at_ms: 0,
            });
            if observed.stamp != stamp {
                *observed = Observed {
                    stamp,
                    stable_since_ms: now_ms,
                    complete: false,
                    failures: 0,
                    retry_at_ms: 0,
                };
                continue;
            }
            if observed.complete
                || now_ms.saturating_sub(observed.stable_since_ms) < STABLE_MS
                || now_ms < observed.retry_at_ms
            {
                continue;
            }
            ready.push(ReadyItem { item: item.clone(), stamp });
        }
        ready
    }

    fn complete(&mut self, file: &str, stamp: FileStamp) {
        if let Some(observed) = self.files.get_mut(file) {
            if observed.stamp == stamp {
                observed.complete = true;
                observed.failures = 0;
                observed.retry_at_ms = 0;
            }
        }
    }

    fn failed(&mut self, file: &str, stamp: FileStamp, now_ms: u64) -> u64 {
        let Some(observed) = self.files.get_mut(file) else {
            return POLL_MS;
        };
        if observed.stamp != stamp {
            return POLL_MS;
        }
        observed.failures = observed.failures.saturating_add(1);
        let shift = observed.failures.saturating_sub(1).min(5);
        let delay = POLL_MS.saturating_mul(1u64 << shift).min(RETRY_MAX_MS);
        observed.retry_at_ms = now_ms.saturating_add(delay);
        delay
    }
}

fn sleep_interruptible(stop: &AtomicBool, millis: u64) {
    let slices = millis.div_ceil(STOP_SLICE_MS);
    for _ in 0..slices {
        if stop.load(Ordering::Acquire) {
            return;
        }
        std::thread::sleep(Duration::from_millis(STOP_SLICE_MS));
    }
}

/// Watch until SIGINT/SIGTERM flips `stop`. Work is sequential and bounded:
/// at most the changed stable rows from one ≤64-item library snapshot are
/// processed per pass, and shutdown waits only for the current publication.
pub fn run(
    client: &mut AssetClient,
    dir: &Path,
    namespace: &str,
    rights: &makepad_asset_client::PublishRights,
    log: bool,
    stop: &AtomicBool,
) {
    let mut state = WatchState::default();
    let started = std::time::Instant::now();
    let mut last_index_error: Option<String> = None;
    while !stop.load(Ordering::Acquire) {
        let now_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        match import::read_index(dir) {
            Err(error) => {
                if log && last_index_error.as_deref() != Some(&error) {
                    eprintln!("[asset-worker] watch waiting for library index: {error}");
                }
                last_index_error = Some(error);
            }
            Ok(items) => {
                if log && last_index_error.take().is_some() {
                    eprintln!("[asset-worker] watch library index recovered");
                }
                for ready in state.observe(dir, &items, now_ms) {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let report = import::import_items(
                        client,
                        dir,
                        namespace,
                        rights,
                        std::slice::from_ref(&ready.item),
                        log,
                    );
                    if report.failed.is_empty() {
                        state.complete(&ready.item.file, ready.stamp);
                    } else {
                        let delay = state.failed(&ready.item.file, ready.stamp, now_ms);
                        if log {
                            eprintln!(
                                "[asset-worker] watch will retry {} in {}ms",
                                ready.item.file, delay
                            );
                        }
                    }
                }
            }
        }
        sleep_interruptible(stop, POLL_MS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn root(name: &str) -> std::path::PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "asset-worker-watch-{}-{id}-{name}",
            std::process::id()
        ))
    }

    fn item(file: &str, prompt: &str) -> IndexItem {
        IndexItem {
            file: file.to_string(),
            label: "clip".to_string(),
            domain: "video".to_string(),
            content_type: "video/mp4".to_string(),
            prompt: prompt.to_string(),
        }
    }

    #[test]
    fn watcher_waits_for_stability_then_processes_new_and_changed_only() {
        let dir = root("stable");
        std::fs::create_dir_all(&dir).unwrap();
        let row = item("lib-1.mp4", "first");
        std::fs::write(dir.join(&row.file), b"first payload").unwrap();
        let mut state = WatchState::default();

        assert!(state.observe(&dir, std::slice::from_ref(&row), 0).is_empty());
        assert!(state.observe(&dir, std::slice::from_ref(&row), 999).is_empty());
        let first = state.observe(&dir, std::slice::from_ref(&row), 1_000);
        assert_eq!(first.len(), 1);
        state.complete(&row.file, first[0].stamp);
        assert!(state.observe(&dir, std::slice::from_ref(&row), 2_000).is_empty());

        // Payload replacement resets stability even under the same file id.
        std::fs::write(dir.join(&row.file), b"second, longer payload").unwrap();
        assert!(state.observe(&dir, std::slice::from_ref(&row), 2_001).is_empty());
        let changed = state.observe(&dir, std::slice::from_ref(&row), 3_001);
        assert_eq!(changed.len(), 1);

        // Index metadata is part of identity too; prompt/title edits do not
        // accidentally inherit the completed marker of an older row.
        state.complete(&row.file, changed[0].stamp);
        let edited = item("lib-1.mp4", "edited prompt");
        assert!(state.observe(&dir, std::slice::from_ref(&edited), 4_000).is_empty());
        assert_eq!(state.observe(&dir, std::slice::from_ref(&edited), 5_000).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_rows_retry_with_bounded_backoff_and_changes_reset_it() {
        let dir = root("retry");
        std::fs::create_dir_all(&dir).unwrap();
        let row = item("lib-2.mp4", "retry");
        std::fs::write(dir.join(&row.file), b"incomplete").unwrap();
        let mut state = WatchState::default();
        state.observe(&dir, std::slice::from_ref(&row), 0);
        let ready = state.observe(&dir, std::slice::from_ref(&row), 1_000);
        assert_eq!(ready.len(), 1);
        assert_eq!(state.failed(&row.file, ready[0].stamp, 1_000), 1_000);
        assert!(state.observe(&dir, std::slice::from_ref(&row), 1_999).is_empty());
        let retry = state.observe(&dir, std::slice::from_ref(&row), 2_000);
        assert_eq!(retry.len(), 1);
        assert_eq!(state.failed(&row.file, retry[0].stamp, 2_000), 2_000);
        assert!(state.observe(&dir, std::slice::from_ref(&row), 3_999).is_empty());

        std::fs::write(dir.join(&row.file), b"now complete and changed").unwrap();
        assert!(state.observe(&dir, std::slice::from_ref(&row), 4_000).is_empty());
        assert_eq!(state.observe(&dir, std::slice::from_ref(&row), 5_000).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stop_sleep_is_prompt() {
        let stop = AtomicBool::new(true);
        let start = std::time::Instant::now();
        sleep_interruptible(&stop, POLL_MS);
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn continuous_watch_publishes_to_real_server_and_stops_cleanly() {
        use makepad_asset_store::{AssetServer, ServerConfig};
        use makepad_asset_client::{ApiEndpoints, ClientConfig, ClientError};
        use std::sync::Arc;

        let server_root = root("server");
        let mut server_config = ServerConfig::new(server_root.clone());
        server_config.control_addr = "127.0.0.1:0".parse().unwrap();
        server_config.data_addr = "127.0.0.1:0".parse().unwrap();
        server_config.bootstrap_admin = true;
        server_config.log = false;
        let server = AssetServer::start(server_config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(server_root.join("admin-token"))
            .unwrap()
            .trim()
            .to_string();
        let endpoints = ApiEndpoints {
            control: server.control_addr(),
            data: server.data_addr(),
        };
        let connect = |cache: std::path::PathBuf| {
            let mut config = ClientConfig::new(cache);
            config.token = Some(token.clone());
            AssetClient::connect(config, endpoints, Some(server.server_id())).unwrap()
        };
        let mut watch_client = connect(root("watch-cache"));
        let probe_client = connect(root("probe-cache"));

        let library = root("library");
        std::fs::create_dir_all(&library).unwrap();
        let mut png = vec![0u8; 24];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[12..16].copy_from_slice(b"IHDR");
        png[16..20].copy_from_slice(&512u32.to_be_bytes());
        png[20..24].copy_from_slice(&512u32.to_be_bytes());
        std::fs::write(library.join("lib-1.png"), &png).unwrap();
        std::fs::write(
            library.join("index.json"),
            br#"{"items":[{"file":"lib-1.png","label":"Watched PNG","domain":"image","content_type":"image/png","prompt":"watch test"}],"next_id":2}"#,
        )
        .unwrap();
        let (_, alias) = crate::import::derived_identity(&png, "gen").unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread_library = library.clone();
        let thread = std::thread::spawn(move || {
            run(
                &mut watch_client,
                &thread_library,
                "gen",
                &makepad_asset_client::PublishRights::declared(
                    "CC0-1.0",
                    "",
                    "",
                    makepad_asset_data::Redistribution::Allowed,
                    makepad_asset_data::DerivativePolicy::Allowed,
                ),
                false,
                &thread_stop,
            );
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match probe_client.resolve_alias(&alias) {
                Ok(_) => break,
                Err(ClientError::NotFound { .. }) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                other => panic!("watched publication did not arrive: {other:?}"),
            }
        }
        stop.store(true, Ordering::Release);
        let join_started = std::time::Instant::now();
        thread.join().expect("watch thread");
        assert!(join_started.elapsed() < Duration::from_millis(250));
    }
}
