//! Headless stress and semantics battery for the platform watcher and Settler.
//! Temp dirs live under `std::env::temp_dir()` and are removed on Drop.

use makepad_filesystem_watcher::settle::probe_file;
use makepad_filesystem_watcher::{
    FileProbe, FileSystemEvent, FileSystemEventKind, FileSystemWatcher, SettleConfig, Settlement,
    Settler, WatchRoot,
};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "fswatch-storm-{tag}-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self {
            path: std::fs::canonicalize(&path).unwrap(),
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

fn start_watcher(roots: Vec<WatchRoot>) -> (FileSystemWatcher, Receiver<FileSystemEvent>) {
    let (tx, rx) = mpsc::channel();
    let watcher = FileSystemWatcher::start(roots, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    (watcher, rx)
}

fn drain(rx: &Receiver<FileSystemEvent>, window: Duration) -> Vec<FileSystemEvent> {
    let deadline = Instant::now() + window;
    let mut out = Vec::new();
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining.min(Duration::from_millis(20))) {
            Ok(event) => out.push(event),
            Err(_) => {
                if Instant::now() >= deadline {
                    break;
                }
            }
        }
    }
    out
}

fn collect_until(
    rx: &Receiver<FileSystemEvent>,
    timeout: Duration,
    mut done: impl FnMut(&[FileSystemEvent]) -> bool,
) -> Vec<FileSystemEvent> {
    let deadline = Instant::now() + timeout;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        if done(&seen) {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining.min(Duration::from_millis(30))) {
            Ok(event) => seen.push(event),
            Err(_) => {
                if done(&seen) {
                    break;
                }
            }
        }
    }
    seen
}

fn is_rescan_for(event: &FileSystemEvent, root: &Path) -> bool {
    matches!(&event.kind, FileSystemEventKind::RescanRequired { root: r, .. } if r == root)
}

fn file_kind_created_or_changed(event: &FileSystemEvent) -> bool {
    event.is_dir != Some(true)
        && matches!(
            event.kind,
            FileSystemEventKind::Created | FileSystemEventKind::Changed
        )
}

fn synthetic_event(
    path: PathBuf,
    kind: FileSystemEventKind,
    seq: u64,
    at: Instant,
) -> FileSystemEvent {
    FileSystemEvent {
        mount: "storm".into(),
        path,
        kind,
        seq,
        epoch: 0,
        received_at: at,
        wall_time: SystemTime::now(),
        is_dir: Some(false),
    }
}

fn drain_settler(settler: &mut Settler, timeout: Duration) -> Vec<Settlement> {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    while Instant::now() < deadline {
        if settler.pending() == 0 && settler.next_deadline().is_none() {
            break;
        }
        let now = Instant::now();
        if let Some(due) = settler.next_deadline() {
            if now < due {
                std::thread::sleep((due - now).min(Duration::from_millis(15)));
            }
        } else {
            break;
        }
        out.extend(settler.poll(Instant::now(), &mut |path| probe_file(path)));
    }
    out
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
static WATCHER_GATE: Mutex<()> = Mutex::new(());

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn watcher_serial() -> std::sync::MutexGuard<'static, ()> {
    WATCHER_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[test]
fn thousand_file_write_storm_is_delivered_or_rescanned() {
    let _gate = watcher_serial();
    let dir = TempDir::new("thousand");
    let (watcher, rx) = start_watcher(vec![WatchRoot {
        mount: "one".into(),
        path: dir.path.clone(),
    }]);
    std::thread::sleep(Duration::from_millis(250));
    let _ = drain(&rx, Duration::from_millis(50));

    const N: usize = 1000;
    let names: Vec<String> = (0..N).map(|i| format!("f{i:04}.dat")).collect();
    let write_at = Arc::new(Mutex::new(HashMap::<String, Instant>::new()));
    let t_write = Instant::now();
    std::thread::scope(|scope| {
        for chunk in 0..4 {
            let names = &names;
            let root = &dir.path;
            let write_at = Arc::clone(&write_at);
            scope.spawn(move || {
                for i in (chunk..N).step_by(4) {
                    let path = root.join(&names[i]);
                    std::fs::write(&path, b"x").unwrap();
                    write_at
                        .lock()
                        .unwrap()
                        .insert(names[i].clone(), Instant::now());
                }
            });
        }
    });
    let write_ms = t_write.elapsed().as_secs_f64() * 1000.0;

    let events = collect_until(&rx, Duration::from_secs(5), |seen| {
        let rescan = seen.iter().any(|e| is_rescan_for(e, &dir.path));
        if rescan {
            return true;
        }
        let delivered: HashSet<&str> = seen
            .iter()
            .filter(|e| file_kind_created_or_changed(e))
            .filter_map(|e| e.path.file_name()?.to_str())
            .collect();
        names.iter().all(|n| delivered.contains(n.as_str()))
    });

    let rescan = events.iter().any(|e| is_rescan_for(e, &dir.path));
    let mut first_seen: HashMap<String, Instant> = HashMap::new();
    for event in &events {
        if !file_kind_created_or_changed(event) {
            continue;
        }
        if let Some(name) = event.path.file_name().and_then(|n| n.to_str()) {
            first_seen
                .entry(name.to_string())
                .or_insert(event.received_at);
        }
    }
    let delivered = names
        .iter()
        .filter(|n| first_seen.contains_key(n.as_str()))
        .count();
    let missing = N - delivered;
    let writes = write_at.lock().unwrap();
    let mut latencies = Vec::new();
    for name in &names {
        if let (Some(written), Some(seen)) = (writes.get(name), first_seen.get(name)) {
            latencies.push(seen.saturating_duration_since(*written).as_secs_f64() * 1000.0);
        } else if rescan {
            if let Some(rescan_at) = events
                .iter()
                .find(|e| is_rescan_for(e, &dir.path))
                .map(|e| e.received_at)
            {
                if let Some(written) = writes.get(name) {
                    latencies
                        .push(rescan_at.saturating_duration_since(*written).as_secs_f64() * 1000.0);
                }
            }
        }
    }
    let p95 = p95_ms(latencies);
    eprintln!(
        "record=thousand_file_write_storm delivered={delivered} missing={missing} rescan={rescan} events={} write_ms={write_ms:.2} p95_ms={p95:.2}",
        events.len()
    );
    assert!(
        rescan || missing == 0,
        "missing {missing} of {N} paths and no RescanRequired (events={})",
        events.len()
    );
    drop(watcher);
}

#[test]
fn atomic_replace_is_one_update_after_settle() {
    let dir = TempDir::new("atomic");
    let target = dir.path.join("d.rs");
    std::fs::write(&target, b"v-init\n").unwrap();
    let tmp = dir.path.join(".d.rs.tmp");
    let mut settler = Settler::new(SettleConfig::default());
    let t0 = Instant::now();
    let mut seq = 1u64;
    for i in 0..20u32 {
        let body = format!("v{i}\n");
        std::fs::write(&tmp, &body).unwrap();
        std::fs::rename(&tmp, &target).unwrap();
        let at = Instant::now();
        assert!(settler
            .observe(&synthetic_event(
                tmp.clone(),
                FileSystemEventKind::Created,
                seq,
                at,
            ))
            .is_empty());
        seq += 1;
        assert!(settler
            .observe(&synthetic_event(
                tmp.clone(),
                FileSystemEventKind::Changed,
                seq,
                at,
            ))
            .is_empty());
        seq += 1;
        assert!(settler
            .observe(&synthetic_event(
                target.clone(),
                FileSystemEventKind::Renamed { from: tmp.clone() },
                seq,
                at,
            ))
            .is_empty());
        seq += 1;
    }
    let storm_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let out = drain_settler(&mut settler, Duration::from_secs(2));
    let updated: Vec<&Settlement> = out
        .iter()
        .filter(|s| matches!(s, Settlement::Updated { path, .. } if path == &target))
        .collect();
    let removed: Vec<&Settlement> = out
        .iter()
        .filter(|s| matches!(s, Settlement::Removed { .. }))
        .collect();
    let final_probe = probe_file(&target).unwrap().expect("target exists");
    eprintln!(
        "record=atomic_replace settlements={} updated={} removed={} storm_ms={storm_ms:.2} hash={}",
        out.len(),
        updated.len(),
        removed.len(),
        final_probe.content_hash
    );
    assert!(removed.is_empty(), "temporary or target reported Removed: {removed:?}");
    assert_eq!(updated.len(), 1, "expected one Updated, got {out:?}");
    match updated[0] {
        Settlement::Updated { probe, path, .. } => {
            assert_eq!(path, &target);
            assert_eq!(probe.content_hash, final_probe.content_hash);
            assert_eq!(probe.len, final_probe.len);
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(settler.pending(), 0);
}

#[test]
fn half_written_file_stays_still_changing() {
    let dir = TempDir::new("half");
    let path = dir.path.join("growing.bin");
    std::fs::write(&path, b"").unwrap();
    let config = SettleConfig {
        quiescence: Duration::from_millis(30),
        confirm_gap: Duration::from_millis(80),
        give_up: Duration::from_millis(250),
        min_backoff: Duration::from_millis(80),
        max_backoff: Duration::from_millis(200),
    };
    let mut settler = Settler::new(config);
    let t0 = Instant::now();
    settler.observe(&synthetic_event(
        path.clone(),
        FileSystemEventKind::Changed,
        1,
        t0,
    ));

    let mut file = Some(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap(),
    );
    let chunk = [b'a'; 100];
    let mut appends = 0usize;
    let mut next_append = t0;
    let mut still = Vec::new();
    let mut updated: Option<FileProbe> = None;
    let mut polls = 0u32;
    while Instant::now() < t0 + Duration::from_secs(4) {
        let now = Instant::now();
        if appends < 20 && now >= next_append {
            if let Some(handle) = file.as_mut() {
                handle.write_all(&chunk).unwrap();
            }
            appends += 1;
            next_append += Duration::from_millis(50);
            if appends == 20 {
                file = None;
            }
        }
        for settlement in settler.poll(now, &mut |p| probe_file(p)) {
            match settlement {
                Settlement::StillChanging { retry_in, since, .. } => {
                    assert_eq!(since, t0);
                    still.push(retry_in);
                }
                Settlement::Updated { probe, path: p, .. } if p == path => {
                    updated = Some(probe);
                }
                Settlement::Removed { path: p, .. } => panic!("removed while growing: {p:?}"),
                other => panic!("unexpected {other:?}"),
            }
        }
        polls += 1;
        if updated.is_some() && appends >= 20 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // If the loop dropped the file at append 20 via `drop(file)` inside the
    // write branch, a second drop would not compile; reopen a probe instead.
    let final_probe = probe_file(&path).unwrap().expect("grown file");
    eprintln!(
        "record=half_written still_changing={} updated={} appends={appends} polls={polls} final_len={} elapsed_ms={:.2}",
        still.len(),
        updated.is_some(),
        final_probe.len,
        t0.elapsed().as_secs_f64() * 1000.0
    );
    assert!(appends >= 20, "did not finish the 1s append storm");
    assert!(
        !still.is_empty(),
        "Settler never reported StillChanging before quiescence"
    );
    let probe = updated.expect("file should settle to Updated after quiescence");
    assert_eq!(probe.content_hash, final_probe.content_hash);
    assert_eq!(probe.len, final_probe.len);
    assert_eq!(final_probe.len, 2000);
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[test]
fn rename_pairs_or_degrades_honestly() {
    let _gate = watcher_serial();
    let dir = TempDir::new("rename");
    const N: usize = 50;
    let srcs: Vec<PathBuf> = (0..N)
        .map(|i| dir.path.join(format!("r{i:02}.src")))
        .collect();
    let dsts: Vec<PathBuf> = (0..N)
        .map(|i| dir.path.join(format!("r{i:02}.dst")))
        .collect();
    for src in &srcs {
        std::fs::write(src, b"n").unwrap();
    }
    let (watcher, rx) = start_watcher(vec![WatchRoot {
        mount: "one".into(),
        path: dir.path.clone(),
    }]);
    std::thread::sleep(Duration::from_millis(400));
    let _warmup = drain(&rx, Duration::from_millis(200));

    for (src, dst) in srcs.iter().zip(dsts.iter()) {
        std::fs::rename(src, dst).unwrap();
    }

    let events = collect_until(&rx, Duration::from_secs(4), |seen| {
        if seen.iter().any(|e| is_rescan_for(e, &dir.path)) {
            return true;
        }
        srcs.iter().zip(dsts.iter()).all(|(src, dst)| {
            seen.iter().any(|e| {
                e.path == *dst
                    && matches!(&e.kind, FileSystemEventKind::Renamed { from } if from == src)
            }) || (seen
                .iter()
                .any(|e| e.path == *src && e.kind == FileSystemEventKind::Removed)
                && seen.iter().any(|e| {
                    e.path == *dst
                        && matches!(
                            e.kind,
                            FileSystemEventKind::Created | FileSystemEventKind::Changed
                        )
                }))
        })
    });

    let rescan = events.iter().any(|e| is_rescan_for(e, &dir.path));
    let mut paired_rename = 0usize;
    let mut removed_created = 0usize;
    let mut missing = 0usize;
    for (src, dst) in srcs.iter().zip(dsts.iter()) {
        let renamed = events.iter().any(|e| {
            e.path == *dst && matches!(&e.kind, FileSystemEventKind::Renamed { from } if from == src)
        });
        let removed = events
            .iter()
            .find(|e| e.path == *src && e.kind == FileSystemEventKind::Removed);
        let created = events
            .iter()
            .find(|e| e.path == *dst && e.kind == FileSystemEventKind::Created);
        let changed = events
            .iter()
            .find(|e| e.path == *dst && e.kind == FileSystemEventKind::Changed);
        if let Some(created) = created {
            assert!(
                removed.is_some_and(|r| r.seq < created.seq) || renamed,
                "lone Created without a prior Removed for {} -> {} (events={:?})",
                src.display(),
                dst.display(),
                events
                    .iter()
                    .map(|e| (e.seq, e.path.clone(), e.kind.clone()))
                    .collect::<Vec<_>>()
            );
        }
        if renamed {
            paired_rename += 1;
        } else if removed.is_some() && (created.is_some() || changed.is_some()) {
            removed_created += 1;
        } else {
            missing += 1;
        }
    }
    eprintln!(
        "record=rename_pairs renamed={paired_rename} removed_created={removed_created} missing={missing} rescan={rescan} events={}",
        events.len()
    );
    assert!(
        rescan || missing == 0,
        "unaccounted renames={missing} and no RescanRequired"
    );
    drop(watcher);
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[test]
fn set_roots_epoch_brackets() {
    let _gate = watcher_serial();
    let first = TempDir::new("epoch-a");
    let second = TempDir::new("epoch-b");
    let (watcher, rx) = start_watcher(vec![WatchRoot {
        mount: "one".into(),
        path: first.path.clone(),
    }]);
    std::thread::sleep(Duration::from_millis(250));
    let _ = drain(&rx, Duration::from_millis(50));

    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let root = first.path.clone();
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut i = 0u32;
            while !stop.load(Ordering::Relaxed) && i < 400 {
                let _ = std::fs::write(root.join(format!("s{i:04}.txt")), b"s");
                i += 1;
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };
    std::thread::sleep(Duration::from_millis(40));

    let epoch = watcher
        .set_roots(vec![
            WatchRoot {
                mount: "one".into(),
                path: first.path.clone(),
            },
            WatchRoot {
                mount: "two".into(),
                path: second.path.clone(),
            },
        ])
        .unwrap();
    assert_eq!(epoch, 1);

    let events = collect_until(&rx, Duration::from_secs(4), |seen| {
        let has_rescan = seen
            .iter()
            .any(|e| e.epoch == epoch && is_rescan_for(e, &second.path));
        let saw_new = seen.iter().any(|e| e.epoch == epoch);
        has_rescan && saw_new && seen.iter().any(|e| e.epoch == epoch && e.seq > 0)
    });
    stop.store(true, Ordering::Relaxed);
    let _ = writer.join();

    let deadline = Instant::now() + Duration::from_secs(2);
    while watcher.epoch() != epoch {
        assert!(Instant::now() < deadline, "epoch did not advance");
        std::thread::sleep(Duration::from_millis(10));
    }

    let rescans_new = events
        .iter()
        .filter(|e| e.epoch == epoch && is_rescan_for(e, &second.path))
        .count();
    let first_new = events.iter().position(|e| e.epoch == epoch);
    eprintln!(
        "record=set_roots_epoch epoch={epoch} stamped={} events={} rescans_new={rescans_new} first_new={first_new:?}",
        watcher.epoch(),
        events.len()
    );
    assert_eq!(epoch, 1);
    assert_eq!(
        rescans_new, 1,
        "expected one RescanRequired for the new root, events={:?}",
        events
            .iter()
            .map(|e| (e.epoch, e.seq, e.path.clone(), e.kind.clone()))
            .collect::<Vec<_>>()
    );
    let first_new = first_new.expect("new epoch should emit at least the rescan");
    assert!(
        events[first_new..]
            .iter()
            .all(|e| e.epoch == epoch),
        "old-epoch event after the new epoch's first event"
    );
    drop(watcher);
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[test]
fn symlink_dir_is_never_followed() {
    let _gate = watcher_serial();
    let root = TempDir::new("sym-root");
    let outside = TempDir::new("sym-out");
    let link = root.path.join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside.path, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&outside.path, &link).unwrap();

    let (watcher, rx) = start_watcher(vec![WatchRoot {
        mount: "one".into(),
        path: root.path.clone(),
    }]);
    std::thread::sleep(Duration::from_millis(300));
    let _ = drain(&rx, Duration::from_millis(80));

    std::fs::write(outside.path.join("secret.txt"), b"behind").unwrap();
    std::fs::write(outside.path.join("also.txt"), b"behind").unwrap();
    std::fs::write(root.path.join("inside.txt"), b"in").unwrap();

    let events = drain(&rx, Duration::from_secs(2));
    let outside_events: Vec<&FileSystemEvent> = events
        .iter()
        .filter(|e| {
            let path = &e.path;
            path.starts_with(&outside.path)
                || std::fs::canonicalize(path)
                    .map(|c| c.starts_with(&outside.path))
                    .unwrap_or(false)
        })
        .collect();
    eprintln!(
        "record=symlink_dir events={} outside={} inside_seen={}",
        events.len(),
        outside_events.len(),
        events.iter().any(|e| e
            .path
            .file_name()
            .is_some_and(|n| n == "inside.txt"))
    );
    assert!(
        outside_events.is_empty(),
        "watcher followed the symlink out of the root: {:?}",
        outside_events
            .iter()
            .map(|e| (e.path.clone(), e.kind.clone()))
            .collect::<Vec<_>>()
    );
    drop(watcher);
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[test]
fn seq_is_monotonic_under_two_roots() {
    let _gate = watcher_serial();
    let a = TempDir::new("seq-a");
    let b = TempDir::new("seq-b");
    let (watcher, rx) = start_watcher(vec![
        WatchRoot {
            mount: "a".into(),
            path: a.path.clone(),
        },
        WatchRoot {
            mount: "b".into(),
            path: b.path.clone(),
        },
    ]);
    std::thread::sleep(Duration::from_millis(300));
    let _ = drain(&rx, Duration::from_millis(50));

    for i in 0..8 {
        std::fs::write(a.path.join(format!("a{i}.rs")), b"a").unwrap();
        std::fs::write(b.path.join(format!("b{i}.rs")), b"b").unwrap();
    }
    let events = drain(&rx, Duration::from_secs(3));
    eprintln!(
        "record=seq_monotonic events={} first_seq={:?} last_seq={:?}",
        events.len(),
        events.first().map(|e| e.seq),
        events.last().map(|e| e.seq)
    );
    assert!(!events.is_empty(), "two-root watcher produced no events");
    for pair in events.windows(2) {
        assert!(
            pair[1].seq > pair[0].seq,
            "seq not strictly increasing in callback order: {:?}",
            pair
        );
        assert!(pair[1].received_at >= pair[0].received_at);
        assert_eq!(pair[0].epoch, 0);
        assert_eq!(pair[1].epoch, 0);
    }
    drop(watcher);
}
