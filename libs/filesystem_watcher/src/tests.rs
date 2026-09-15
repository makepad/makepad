//! Headless watcher tests against a temporary directory on the host OS.
use super::*;
use std::sync::mpsc;
use std::time::Duration;

fn scratch(tag: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "fswatch-{tag}-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    std::fs::canonicalize(path).unwrap()
}

/// Wait for an event whose path ends with `name` (or the root itself when
/// `name` is empty) and satisfies `accept`.
fn wait_for(
    rx: &mpsc::Receiver<FileSystemEvent>,
    seen: &mut Vec<FileSystemEvent>,
    accept: impl Fn(&FileSystemEvent) -> bool,
    timeout: Duration,
) -> Option<FileSystemEvent> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match rx.recv_timeout(remaining) {
            Ok(event) => {
                let hit = accept(&event);
                seen.push(event.clone());
                if hit {
                    return Some(event);
                }
            }
            Err(_) => return None,
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[test]
fn kinds_sequence_and_epochs_on_the_host_backend() {
    let dir = scratch("kinds");
    let second = scratch("second");
    let (tx, rx) = mpsc::channel();
    let watcher = FileSystemWatcher::start(
        vec![WatchRoot {
            mount: "one".into(),
            path: dir.clone(),
        }],
        move |event| {
            let _ = tx.send(event);
        },
    )
    .unwrap();
    let mut seen = Vec::new();
    let wait = Duration::from_secs(6);

    // Create: a fresh file at a fresh path.
    let a = dir.join("a.rs");
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(&a, "fn a() {}\n").unwrap();
    let created = wait_for(&rx, &mut seen, |e| e.path == a, wait).expect("create event");
    assert!(
        matches!(created.kind, FileSystemEventKind::Created | FileSystemEventKind::Changed),
        "{:?}",
        created.kind
    );
    assert_eq!(created.epoch, 0);
    assert_eq!(created.mount, "one");
    assert_ne!(created.is_dir, Some(true));

    // Modify well outside the creation window.
    std::thread::sleep(Duration::from_millis(900));
    std::fs::write(&a, "fn a() { 1 }\n").unwrap();
    let changed = wait_for(
        &rx,
        &mut seen,
        |e| e.path == a && e.kind == FileSystemEventKind::Changed,
        wait,
    )
    .expect("change event");
    assert!(changed.seq > created.seq);

    // Rename: either a paired Renamed{from} or Removed + Created.
    std::thread::sleep(Duration::from_millis(400));
    let b = dir.join("b.rs");
    std::fs::rename(&a, &b).unwrap();
    let arrived = wait_for(
        &rx,
        &mut seen,
        |e| {
            e.path == b
                && matches!(
                    e.kind,
                    FileSystemEventKind::Created | FileSystemEventKind::Renamed { .. }
                )
        },
        wait,
    )
    .expect("rename arrival");
    if let FileSystemEventKind::Renamed { from } = &arrived.kind {
        assert_eq!(from, &a);
    } else {
        assert!(
            seen.iter()
                .any(|e| e.path == a && e.kind == FileSystemEventKind::Removed),
            "unpaired rename must report the old path as removed"
        );
    }

    // Remove.
    std::thread::sleep(Duration::from_millis(400));
    std::fs::remove_file(&b).unwrap();
    let removed = wait_for(
        &rx,
        &mut seen,
        |e| e.path == b && e.kind == FileSystemEventKind::Removed,
        wait,
    )
    .expect("remove event");
    assert_eq!(removed.epoch, 0);

    // Sequence numbers are strictly increasing in callback order.
    for pair in seen.windows(2) {
        assert!(pair[1].seq > pair[0].seq, "seq not increasing: {:?}", pair);
        assert!(pair[1].received_at >= pair[0].received_at);
    }

    // Root change: the new epoch starts with RescanRequired for the new root
    // and every later event carries it.
    let epoch = watcher.set_roots(vec![WatchRoot {
        mount: "two".into(),
        path: second.clone(),
    }])
    .unwrap();
    assert_eq!(epoch, 1);
    let rescan = wait_for(
        &rx,
        &mut seen,
        |e| matches!(&e.kind, FileSystemEventKind::RescanRequired { root, .. } if root == &second),
        wait,
    )
    .expect("rescan marker for the new root");
    assert_eq!(rescan.epoch, 1);
    assert_eq!(rescan.mount, "two");
    let deadline = Instant::now() + Duration::from_secs(5);
    while watcher.epoch() != 1 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(watcher.last_error().is_none(), "{:?}", watcher.last_error());
    std::thread::sleep(Duration::from_millis(300));
    let c = second.join("c.rs");
    std::fs::write(&c, "fn c() {}\n").unwrap();
    let in_new = wait_for(&rx, &mut seen, |e| e.path == c, wait).expect("event in the new root");
    assert_eq!(in_new.epoch, 1);
    assert_eq!(in_new.mount, "two");
    // The old root is silent now.
    std::fs::write(dir.join("late.rs"), "late").unwrap();
    assert!(
        wait_for(&rx, &mut seen, |e| e.path.starts_with(&dir), Duration::from_secs(1)).is_none(),
        "old root still delivers after set_roots"
    );
    drop(watcher);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(second);
}
