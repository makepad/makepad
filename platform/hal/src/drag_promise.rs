//! File promises of a drag out of the app (`input::drag_item::PROMISE`).
//!
//! The app starts making the file the moment the drag starts, on its own
//! thread, at a path it names in the drag item (`<path>\n<mime>`): it writes
//! [`part_path`], then renames it to the final path, or writes
//! [`failed_path`] with a reason. A backend that has to hand over the file
//! (the drop landed, or the receiver asked for its URL) waits for it with
//! [`wait`]. The file system is the only channel, so this works the same
//! when the app and the window system are in different processes, and the
//! backend never needs the app's event loop to answer while it waits.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a backend waits for a promised file before giving up the drop.
pub const WAIT: Duration = Duration::from_secs(120);

/// A promise item's string: the final path, then the MIME type.
pub fn encode(path: &str, mime: &str) -> String {
    format!("{path}\n{mime}")
}

/// (final path, MIME type) of a promise item's string.
pub fn decode(item: &str) -> (&str, &str) {
    item.split_once('\n').unwrap_or((item, ""))
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}{suffix}"))
}

/// Where the producer writes before the file is complete.
pub fn part_path(path: &Path) -> PathBuf {
    sibling(path, ".part")
}

/// Written (with the reason) when the producer failed.
pub fn failed_path(path: &Path) -> PathBuf {
    sibling(path, ".failed")
}

/// Is the promised file there yet? `Some(Ok)` ready, `Some(Err)` failed,
/// `None` still being made.
pub fn poll(path: &Path) -> Option<Result<(), String>> {
    if path.is_file() {
        return Some(Ok(()));
    }
    let failed = failed_path(path);
    if failed.exists() {
        return Some(Err(std::fs::read_to_string(failed).unwrap_or_else(|_| "the file could not be made".into())));
    }
    None
}

/// Block until the promised file is complete, failed, or `timeout` passed.
pub fn wait(path: &Path, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    loop {
        if let Some(done) = poll(path) {
            return done;
        }
        if start.elapsed() > timeout {
            return Err(format!("timed out making {}", path.display()));
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// Hand a finished promised file to a receiver's `dest` (a file promise's destination): copy it
/// to a hidden file in `dest`'s folder, put its bytes on the disk, and rename it to `dest`. The
/// destination name appears only with the whole file under it, so a receiver that reads it the
/// moment it is told the file is there (or watches the folder) never sees a partial or empty file.
pub fn deliver(src: &Path, dest: &Path) -> Result<(), String> {
    let part = part_path(dest);
    let result = (|| {
        let copied = std::fs::copy(src, &part).map_err(|e| format!("copying to {}: {e}", dest.display()))?;
        let file = std::fs::File::open(&part).map_err(|e| format!("opening {}: {e}", part.display()))?;
        file.sync_all().map_err(|e| format!("syncing {}: {e}", part.display()))?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        if len == 0 || len != copied {
            return Err(format!("{} came out {len} bytes", dest.display()));
        }
        drop(file);
        std::fs::rename(&part, dest).map_err(|e| format!("renaming to {}: {e}", dest.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// The folder promised files are made in (one subfolder per drag).
pub fn root() -> PathBuf {
    std::env::temp_dir().join("makepad-drag-out")
}

/// Is `path` a promised file (made under [`root`])?
pub fn is_promised(path: &str) -> bool {
    Path::new(path).starts_with(root())
}

/// Promise items as plain file items of their final paths, for a window system that hands files
/// over by path (a URI list, CF_HDROP): it calls [`wait_promised`] before it hands them over.
pub fn to_files(items: Vec<(u32, String)>) -> Vec<(u32, String)> {
    items
        .into_iter()
        .map(|(kind, s)| {
            if kind == crate::types::input::drag_item::PROMISE {
                (crate::types::input::drag_item::FILE, decode(&s).0.to_string())
            } else {
                (kind, s)
            }
        })
        .collect()
}

/// Wait for every promised file among `items` (file items under [`root`]); false when one of
/// them failed or timed out.
pub fn wait_promised(items: &[(u32, String)]) -> bool {
    let mut ok = true;
    for (kind, s) in items {
        if *kind == crate::types::input::drag_item::FILE && is_promised(s) {
            if let Err(e) = wait(Path::new(s), WAIT) {
                eprintln!("[hal] drag out: {e}");
                ok = false;
            }
        }
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::input::drag_item;

    #[test]
    fn promised_files_are_waited_for_and_failures_reported() {
        let dir = root().join(format!("test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("a.mp3");
        let bad = dir.join("b.mp3");
        let items = to_files(vec![
            (drag_item::PROMISE, encode(&good.to_string_lossy(), "audio/mpeg")),
            (drag_item::FILE, "/etc/hosts".into()),
        ]);
        assert_eq!(items[0], (drag_item::FILE, good.to_string_lossy().into_owned()));
        assert!(is_promised(&items[0].1) && !is_promised(&items[1].1));
        assert_eq!(poll(&good), None);
        let g = good.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            std::fs::write(part_path(&g), b"x").unwrap();
            std::fs::rename(part_path(&g), &g).unwrap();
        });
        assert!(wait_promised(&items));
        t.join().unwrap();
        std::fs::write(failed_path(&bad), "no decoder").unwrap();
        assert_eq!(wait(&bad, Duration::from_secs(1)), Err("no decoder".into()));
        assert!(wait(&dir.join("never.mp3"), Duration::from_millis(50)).is_err());
        // delivery: the destination name only ever holds the whole file, no part file stays
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        deliver(&good, &out.join("a.mp3")).unwrap();
        assert_eq!(std::fs::read(out.join("a.mp3")).unwrap(), b"x");
        assert!(!part_path(&out.join("a.mp3")).exists());
        assert!(deliver(&dir.join("missing.mp3"), &out.join("m.mp3")).is_err());
        assert!(!out.join("m.mp3").exists() && !part_path(&out.join("m.mp3")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
