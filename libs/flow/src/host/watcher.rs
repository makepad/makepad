use super::backend::Backend;
use super::config::SharedConfig;
use super::util::log;
use super::ServerError;
use std::collections::BTreeMap;
use std::fs::DirEntry;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

/// What the flows folder holds now: the baseline a watcher started later
/// compares against. Taken before the backend loads the folder, so a file
/// that appears or changes after the load is never already in the baseline
/// and silently skipped.
pub(crate) fn watch_baseline(root: &Path) -> Result<BTreeMap<String, Stamp>, ServerError> {
    scan(&root.join("flows")).map_err(|error| ServerError::io("scan flows directory", error))
}

pub(crate) fn spawn_watcher(
    config: SharedConfig,
    backend: Arc<Backend>,
    mut previous: BTreeMap<String, Stamp>,
) -> Result<(mpsc::Sender<()>, JoinHandle<()>), ServerError> {
    let flows = config.root.join("flows");
    let (stop_tx, stop_rx) = mpsc::channel();
    let join = std::thread::Builder::new()
        .name("flow-server-watcher".to_string())
        .spawn(move || {
            let interval = Duration::from_millis(config.watch_interval_ms);
            while matches!(stop_rx.recv_timeout(interval), Err(mpsc::RecvTimeoutError::Timeout)) {
                let current = match scan(&flows) {
                    Ok(current) => current,
                    Err(error) => {
                        log(&config, &format!("watch scan failed: {error}"));
                        continue;
                    }
                };
                let mut next_previous = previous.clone();
                for (name, stamp) in &current {
                    if previous.get(name) == Some(stamp) {
                        continue;
                    }
                    // The backend reads the file again under the lock that
                    // orders it with every PUT, so what it loads is what is
                    // on disk at that moment, never this scan's older view.
                    if backend.watched_changed(name) {
                        next_previous.insert(name.clone(), stamp.clone());
                    }
                }
                for name in previous.keys().filter(|name| !current.contains_key(*name)) {
                    if backend.watched_removed(name) {
                        next_previous.remove(name);
                    }
                }
                previous = next_previous;
            }
        })
        .map_err(|error| ServerError::io("spawn watcher thread", error))?;
    Ok((stop_tx, join))
}

fn scan(root: &Path) -> std::io::Result<BTreeMap<String, Stamp>> {
    scan_entries(std::fs::read_dir(root)?)
}

fn scan_entries(
    entries: impl Iterator<Item = std::io::Result<DirEntry>>,
) -> std::io::Result<BTreeMap<String, Stamp>> {
    let mut out = BTreeMap::new();
    for entry in entries {
        let entry = entry?;
        let path: PathBuf = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("splash") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !super::routes::valid_name(name) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            // Removed since the folder was listed: it is simply not there.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        out.insert(
            name.to_string(),
            Stamp { modified: metadata.modified().ok(), len: metadata.len() },
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_removed_during_a_scan_is_absent_not_an_error() {
        let dir = std::env::temp_dir().join(format!("makepad-flow-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a", "b", "c"] {
            std::fs::write(dir.join(format!("{name}.splash")), name).unwrap();
        }
        // The listing is read in one go on the first entry: the others are
        // still handed out after their files are gone.
        let mut entries = std::fs::read_dir(&dir).unwrap();
        let first = entries.next().unwrap();
        let kept = first.as_ref().unwrap().path();
        for name in ["a", "b", "c"] {
            let path = dir.join(format!("{name}.splash"));
            if path != kept {
                std::fs::remove_file(path).unwrap();
            }
        }
        let scanned = scan_entries(std::iter::once(first).chain(entries));
        let stem = kept.file_stem().unwrap().to_str().unwrap().to_string();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(scanned.unwrap().keys().collect::<Vec<_>>(), [&stem]);
    }
}
