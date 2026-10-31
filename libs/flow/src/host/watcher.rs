use super::backend::Backend;
use super::config::SharedConfig;
use super::util::log;
use super::ServerError;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

pub(crate) fn spawn_watcher(
    config: SharedConfig,
    backend: Arc<Backend>,
) -> Result<(mpsc::Sender<()>, JoinHandle<()>), ServerError> {
    let (stop_tx, stop_rx) = mpsc::channel();
    let join = std::thread::Builder::new()
        .name("flow-server-watcher".to_string())
        .spawn(move || {
            let flows = config.root.join("flows");
            let mut previous = scan(&flows).unwrap_or_default();
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
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(root)? {
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
        let metadata = entry.metadata()?;
        out.insert(
            name.to_string(),
            Stamp { modified: metadata.modified().ok(), len: metadata.len() },
        );
    }
    Ok(out)
}
