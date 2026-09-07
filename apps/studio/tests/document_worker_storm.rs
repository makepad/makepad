//! Headless storm tests for DocumentWorker's event-driven notify path.

use makepad_studio::document::DocumentRegistry;
use makepad_studio::document_worker::{DocumentWorker, FileSnapshot, MAX_DOCUMENTS};
use makepad_studio::makepad_widgets::makepad_platform::makepad_network::install_ui_waker;
use makepad_studio::makepad_widgets::Cx;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// A headless test has no event loop to wake. `Cx::new` installs the
/// platform waker, whose first use from a worker thread creates the
/// shared NSApplication off the main thread and never returns; the
/// real app installs its own waker, so tests run without one.
fn headless_cx() -> Cx {
    let cx = Cx::new(Box::new(|_, _| {}));
    install_ui_waker(None);
    cx
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "studio-doc-storm-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self {
            path: std::fs::canonicalize(path).unwrap(),
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn p95_ms(mut samples: Vec<f64>) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((samples.len() as f64 - 1.0) * 0.95).round() as usize;
    samples[idx.min(samples.len() - 1)]
}

fn collect(worker: &mut DocumentWorker, window: Duration) -> Vec<Arc<FileSnapshot>> {
    let deadline = Instant::now() + window;
    let mut out = Vec::new();
    while Instant::now() < deadline {
        out.extend(worker.poll());
        std::thread::sleep(Duration::from_millis(2));
    }
    out
}

fn collect_until(
    worker: &mut DocumentWorker,
    timeout: Duration,
    mut done: impl FnMut(&[Arc<FileSnapshot>]) -> bool,
) -> Vec<Arc<FileSnapshot>> {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    while Instant::now() < deadline {
        out.extend(worker.poll());
        if done(&out) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    out
}

#[test]
fn notify_changed_storm_delivers_snapshots() {
    let dir = TempDir::new();
    const NOTIFIES: usize = 200;
    let watched_n = MAX_DOCUMENTS;
    let mut paths = Vec::with_capacity(NOTIFIES);
    for i in 0..NOTIFIES {
        let path = dir.path.join(format!("f{i:03}.rs"));
        std::fs::write(&path, format!("v0-{i}\n")).unwrap();
        paths.push(path);
    }

    let cx = headless_cx();
    let mut worker =
        DocumentWorker::start_with_fallback(&cx.thread_spawner(), Duration::from_secs(120))
            .unwrap();
    for path in paths.iter().take(watched_n) {
        worker.watch(path.clone()).unwrap();
    }

    let initial = collect_until(&mut worker, Duration::from_secs(5), |snaps| {
        let mut latest = BTreeMap::new();
        for snap in snaps {
            latest.insert(snap.requested_path.clone(), snap.clone());
        }
        latest.len() == watched_n
    });
    let mut latest: BTreeMap<PathBuf, Arc<FileSnapshot>> = BTreeMap::new();
    for snap in &initial {
        latest.insert(snap.requested_path.clone(), snap.clone());
    }
    assert_eq!(latest.len(), watched_n, "initial snapshots missing");
    for snap in latest.values() {
        assert_eq!(snap.observed, None);
        assert!(snap.text.is_some());
    }

    for (i, path) in paths.iter().take(watched_n).enumerate() {
        std::fs::write(path, format!("v1-{i}\n")).unwrap();
    }

    let epoch = 7u64;
    let mut first_notify: BTreeMap<PathBuf, Instant> = BTreeMap::new();
    let storm_start = Instant::now();
    for i in 0..NOTIFIES {
        let path = &paths[i];
        let seq = 1000 + i as u64;
        worker.notify_changed(path.clone(), epoch, seq).unwrap();
        first_notify.entry(path.clone()).or_insert(storm_start);
        if i + 1 < NOTIFIES {
            let target = storm_start + Duration::from_millis(100) * (i as u32 + 1) / NOTIFIES as u32;
            let now = Instant::now();
            if target > now {
                std::thread::sleep(target - now);
            }
        }
    }
    let storm_ms = storm_start.elapsed().as_secs_f64() * 1000.0;

    let mut first_seen_at: BTreeMap<PathBuf, Instant> = BTreeMap::new();
    let after = collect_until(&mut worker, Duration::from_secs(4), |snaps| {
        for snap in snaps {
            first_seen_at
                .entry(snap.requested_path.clone())
                .or_insert_with(Instant::now);
        }
        snaps
            .iter()
            .filter(|s| s.observed.is_some())
            .map(|s| s.requested_path.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == watched_n
    });

    let mut observed_snaps: BTreeMap<PathBuf, Vec<Arc<FileSnapshot>>> = BTreeMap::new();
    for snap in &after {
        assert!(
            snap.observed.is_some(),
            "storm snapshot missing observed (epoch, seq): {snap:?}"
        );
        observed_snaps
            .entry(snap.requested_path.clone())
            .or_default()
            .push(snap.clone());
    }
    assert_eq!(
        observed_snaps.len(),
        watched_n,
        "expected a snapshot for every watched file, got {}",
        observed_snaps.len()
    );

    let mut latencies = Vec::new();
    for (path, snaps) in &observed_snaps {
        let mut hashes = std::collections::BTreeSet::new();
        for snap in snaps {
            assert_eq!(snap.observed.map(|(e, _)| e), Some(epoch));
            let hash = snap.text.as_deref().map(String::as_str).unwrap_or("");
            assert!(
                hashes.insert(hash),
                "duplicate snapshot for unchanged hash on {}",
                path.display()
            );
        }
        if let (Some(t0), Some(seen)) = (first_notify.get(path), first_seen_at.get(path)) {
            latencies.push(seen.saturating_duration_since(*t0).as_secs_f64() * 1000.0);
        }
    }
    let p95 = p95_ms(latencies);

    let mut all_snaps: Vec<Arc<FileSnapshot>> = latest.values().cloned().collect();
    for snaps in observed_snaps.values() {
        all_snaps.extend(snaps.iter().cloned());
    }
    all_snaps.sort_by_key(|s| s.revision);
    for pair in all_snaps.windows(2) {
        assert!(
            pair[1].revision >= pair[0].revision,
            "revisions not monotonic: {} then {}",
            pair[0].revision,
            pair[1].revision
        );
    }
    for snaps in observed_snaps.values() {
        let initial_rev = latest
            .get(&snaps[0].requested_path)
            .map(|s| s.revision)
            .unwrap_or(0);
        let mut prev = initial_rev;
        for snap in snaps {
            assert!(
                snap.revision > prev,
                "per-path revision not increasing for {}",
                snap.requested_path.display()
            );
            prev = snap.revision;
        }
    }

    worker.notify_changed(paths[0].clone(), epoch, 3000).unwrap();
    let dupes = collect(&mut worker, Duration::from_millis(150));
    assert!(
        dupes.is_empty(),
        "unchanged hash produced a duplicate snapshot: {dupes:?}"
    );

    eprintln!(
        "record=document_notify_storm watched={watched_n} notified={NOTIFIES} snapshots={} storm_ms={storm_ms:.2} p95_ms={p95:.2}",
        after.len()
    );

    worker.request_stop();
}

#[test]
fn dirty_buffer_exposes_one_conflict_after_notified_disk_storm() {
    let dir = TempDir::new();
    let path = dir.path.join("conflict.rs");
    std::fs::write(&path, "base\n").unwrap();

    let cx = headless_cx();
    let mut worker =
        DocumentWorker::start_with_fallback(&cx.thread_spawner(), Duration::from_secs(120))
            .unwrap();
    worker.watch(path.clone()).unwrap();
    let first = collect_until(&mut worker, Duration::from_secs(5), |snaps| !snaps.is_empty())
        .pop()
        .expect("initial snapshot");
    assert_eq!(first.text.as_deref().map(String::as_str), Some("base\n"));

    let mut registry = DocumentRegistry::default();
    let doc = registry.apply_snapshot(&first).unwrap();
    let human = doc.current_text();
    // Public-API stand-in for a local edit: a save acknowledgement of a
    // different payload advances the baseline while the in-memory buffer
    // stays put, which is the dirty state `external_update` conflicts on.
    doc.save_succeeded(
        Arc::new("human base\n".to_owned()),
        first.revision,
    );
    assert!(doc.is_dirty());
    assert!(!doc.has_conflict());
    assert_eq!(doc.current_text(), human);

    for i in 0..10 {
        std::fs::write(&path, format!("disk-{i}\n")).unwrap();
        worker.notify_changed(path.clone(), 9, 1 + i as u64).unwrap();
    }

    let snaps = collect_until(&mut worker, Duration::from_secs(3), |snaps| {
        snaps.iter().any(|s| s.observed.is_some() && s.revision > first.revision)
    });
    assert!(
        !snaps.is_empty(),
        "notified disk changes produced no snapshot"
    );

    let mut conflict_edges = 0u32;
    let mut was = doc.has_conflict();
    for snap in &snaps {
        registry.apply_snapshot(snap).unwrap();
        let now = doc.has_conflict();
        if now && !was {
            conflict_edges += 1;
        }
        was = now;
    }
    eprintln!(
        "record=document_conflict snapshots={} conflict_edges={conflict_edges} conflict={} disk={:?}",
        snaps.len(),
        doc.has_conflict(),
        doc.disk_text()
    );
    assert_eq!(doc.current_text(), human, "local buffer must be retained");
    assert!(doc.has_conflict());
    assert_eq!(
        conflict_edges, 1,
        "expected exactly one conflict state, saw {conflict_edges} edges over {} snapshots",
        snaps.len()
    );

    worker.request_stop();
}
