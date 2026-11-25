//! File promises of a drag out of the app (`input::drag_item::PROMISE`).
//!
//! The app starts making the file the moment the drag starts, on its own
//! thread, at a path it names in the drag item (`<path>\n<mime>`): it writes
//! [`part_path`], then renames it to the final path, or writes
//! [`failed_path`] with a reason, holding [`hold`] from before the promise
//! is handed out until one of them is in place. A backend that has to hand
//! over the file (the drop landed, or the receiver asked for its URL) waits
//! for it with [`wait`]. The file system is the only channel, so this works
//! the same when the app and the window system are in different processes,
//! and the backend never needs the app's event loop to answer while it waits.

use std::path::{Path, PathBuf};

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

/// Locked by the producer while it makes the file ([`hold`]).
pub fn producing_path(path: &Path) -> PathBuf {
    sibling(path, ".producing")
}

/// The producer's hold on `path`'s promise (its folder must exist): an exclusive lock on
/// [`producing_path`], taken before the promise is handed out and kept until the file or its
/// failure is in place. The system lets it go when the producer drops it or its process ends,
/// however it ends, so [`wait`] can tell a producer at work from one that is gone.
pub fn hold(path: &Path) -> std::io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(producing_path(path))?;
    file.lock()?;
    Ok(file)
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

/// Block until the promised file is complete or failed, for as long as its producer is at work
/// on it (it keeps its [`hold`]): no clock gives up on a producer that is still making the file,
/// a long export or a loaded box alike. A producer that let go with neither in place (it
/// panicked, its process ended) has failed, and so has a promise no producer ever held.
pub fn wait(path: &Path) -> Result<(), String> {
    if let Some(done) = poll(path) {
        return done;
    }
    // (a shared lock: it is ours once the producer lets go, beside other waiters)
    if let Ok(held) = std::fs::File::open(producing_path(path)) {
        held.lock_shared().map_err(|e| format!("waiting for {}: {e}", path.display()))?;
    }
    poll(path).unwrap_or_else(|| Err(format!("nothing made {}: its producer ended without it", path.display())))
}

/// Hand a finished promised file to a receiver's `dest` (a file promise's destination): copy it
/// to a hidden file in `dest`'s folder, put its bytes on the disk, and rename it to `dest`. The
/// destination name appears only with the whole file under it, so a receiver that reads it the
/// moment it is told the file is there (or watches the folder) never sees a partial or empty file.
pub fn deliver(src: &Path, dest: &Path) -> Result<(), String> {
    let part = part_path(dest);
    let result = (|| {
        let copied = std::fs::copy(src, &part).map_err(|e| format!("copying to {}: {e}", dest.display()))?;
        // write access: Windows flushes a file only through a handle that may write it
        let file = std::fs::OpenOptions::new().write(true).open(&part).map_err(|e| format!("opening {}: {e}", part.display()))?;
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

/// Is the producer of `path` at work on it (it keeps its [`hold`])? A look, never a wait.
fn producing(path: &Path) -> bool {
    std::fs::File::open(producing_path(path)).is_ok_and(|held| matches!(held.try_lock_shared(), Err(std::fs::TryLockError::WouldBlock)))
}

/// Is any promised file among `items` (file items under [`root`]) still being made: neither made
/// nor failed, its producer still at work? A look, never a wait: an event thread asks it before it
/// hands an answer to [`when_made`], and again when woken.
pub fn pending(items: &[(u32, String)]) -> bool {
    items.iter().any(|(kind, s)| *kind == crate::types::input::drag_item::FILE && is_promised(s) && poll(Path::new(s)).is_none() && producing(Path::new(s)))
}

type Waited = (Vec<(u32, String)>, Box<dyn FnOnce(bool) + Send>);

/// [`when_made`]'s waiter thread and its channel, started on first use. Each side of the JIT
/// boundary keeps its own: the loader's HAL (a window system's wait) and an app Relax compiles
/// (its UI thread's) each feed a thread of their own std, running their own side's closures.
#[relax_api::relax_api(per_side)]
static WAITER: std::sync::OnceLock<Option<std::sync::mpsc::Sender<Waited>>> = std::sync::OnceLock::new();

/// Runs `then` with [`wait_promised`]'s answer for `items` on the process's one promise waiter
/// thread, fed by a channel: an event thread (a window system's, an app's UI thread) hands its
/// wait there and goes on serving its events while the producers work. The waits run one after
/// another, in the order asked. Without that thread (it could not be started), `then` runs here
/// after the wait, as before, and says so.
pub fn when_made(items: Vec<(u32, String)>, then: impl FnOnce(bool) + Send + 'static) {
    let waiter = WAITER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Waited>();
        let spawned = std::thread::Builder::new().name("drag-promise".into()).spawn(move || {
            for (items, then) in rx {
                then(wait_promised(&items));
            }
        });
        match spawned {
            Ok(_) => Some(tx),
            Err(e) => {
                eprintln!("[hal] drag out: no promise waiter thread ({e}): drops wait on their event thread");
                None
            }
        }
    });
    match waiter {
        Some(tx) => {
            if let Err(std::sync::mpsc::SendError((items, then))) = tx.send((items, Box::new(then))) {
                then(wait_promised(&items));
            }
        }
        None => then(wait_promised(&items)),
    }
}

/// Wait for every promised file among `items` (file items under [`root`]); false when one of
/// them failed or its producer ended without it.
pub fn wait_promised(items: &[(u32, String)]) -> bool {
    let mut ok = true;
    for (kind, s) in items {
        if *kind == crate::types::input::drag_item::FILE && is_promised(s) {
            if let Err(e) = wait(Path::new(s)) {
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
    use std::time::Duration;

    #[test]
    fn promised_files_are_waited_for_and_failures_reported() {
        let dir = root().join(format!("test-{}", std::process::id()));
        // (what a failed run with this pid left)
        let _ = std::fs::remove_dir_all(&dir);
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
        let held = hold(&g).unwrap();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            std::fs::write(part_path(&g), b"x").unwrap();
            std::fs::rename(part_path(&g), &g).unwrap();
            drop(held);
        });
        assert!(wait_promised(&items));
        t.join().unwrap();
        std::fs::write(failed_path(&bad), "no decoder").unwrap();
        assert_eq!(wait(&bad), Err("no decoder".into()));
        // neither held by a producer nor made: failed at once, no clock
        assert!(wait(&dir.join("never.mp3")).is_err());
        // a producer that lets go with neither in place has failed, however long it took
        let gone = dir.join("gone.mp3");
        let held = hold(&gone).unwrap();
        let t = std::thread::spawn(move || drop(held));
        assert!(wait(&gone).unwrap_err().contains("ended without it"));
        t.join().unwrap();
        assert!(!pending(&[(drag_item::FILE, gone.to_string_lossy().into_owned())]), "a producer gone is nothing to wait for");
        // An event thread hands its wait over and goes on: the answer comes once the producer
        // lets go, however long it holds, and none before.
        let later = dir.join("later.mp3");
        let held = hold(&later).unwrap();
        let items = vec![(drag_item::FILE, later.to_string_lossy().into_owned())];
        assert!(pending(&items));
        let (answer, answered) = std::sync::mpsc::channel();
        when_made(items.clone(), move |made| answer.send(made).unwrap());
        assert_eq!(answered.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty), "no answer while the producer holds");
        std::fs::write(&later, b"y").unwrap();
        drop(held);
        assert_eq!(answered.recv(), Ok(true));
        assert!(!pending(&items));
        // Win32: a receiver's GetData, which must answer in its call on the window thread, waits
        // while that thread's messages go on being dispatched
        #[cfg(windows)]
        crate::backend::windows::win32::check_get_data_pumps(&dir);
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
