use makepad_filesystem_watcher::FileSystemWatcher;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub use makepad_filesystem_watcher::WatchRoot;

#[derive(Clone, Debug)]
pub struct LiveReloadWatchPlan {
    pub roots: Vec<WatchRoot>,
    pub files_by_root: HashMap<String, Vec<String>>,
    pub initial_contents: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct LiveReloadFileChange {
    pub file_name: String,
    pub content: String,
}

pub struct LiveReloadWatcherHandle {
    _watcher: FileSystemWatcher,
}

#[derive(Clone)]
pub struct LiveReloadLogger {
    info: Arc<dyn Fn(String) + Send + Sync + 'static>,
    error: Arc<dyn Fn(String) + Send + Sync + 'static>,
}

impl LiveReloadLogger {
    pub fn new<FInfo, FError>(info: FInfo, error: FError) -> Self
    where
        FInfo: Fn(String) + Send + Sync + 'static,
        FError: Fn(String) + Send + Sync + 'static,
    {
        Self {
            info: Arc::new(info),
            error: Arc::new(error),
        }
    }

    fn info(&self, message: String) {
        (self.info)(message);
    }

    fn error(&self, message: String) {
        (self.error)(message);
    }
}

pub fn start_live_reload_watcher<F>(
    plan: LiveReloadWatchPlan,
    sink: F,
    logger: LiveReloadLogger,
) -> Result<LiveReloadWatcherHandle, String>
where
    F: Fn(LiveReloadFileChange) -> Result<(), String> + Send + Sync + 'static,
{
    let watched_file_count = plan.initial_contents.len();
    let root_count = plan.roots.len();
    let file_map = Arc::new(plan.files_by_root);
    let file_cache = Arc::new(Mutex::new(plan.initial_contents));
    let sink: Arc<dyn Fn(LiveReloadFileChange) -> Result<(), String> + Send + Sync + 'static> =
        Arc::new(sink);

    let watcher = FileSystemWatcher::start(plan.roots, {
        let file_map = Arc::clone(&file_map);
        let file_cache = Arc::clone(&file_cache);
        let sink = Arc::clone(&sink);
        let logger = logger.clone();
        move |event| {
            forward_hot_reload_fs_event(
                event.mount,
                event.path,
                &file_map,
                &file_cache,
                &sink,
                &logger,
            );
        }
    })?;

    logger.info(format!(
        "hot reload watching {} script_mod source files across {} crate roots",
        watched_file_count, root_count
    ));

    Ok(LiveReloadWatcherHandle { _watcher: watcher })
}

fn forward_hot_reload_fs_event(
    mount: String,
    path: PathBuf,
    files_by_root: &HashMap<String, Vec<String>>,
    file_cache: &Mutex<HashMap<String, String>>,
    sink: &Arc<dyn Fn(LiveReloadFileChange) -> Result<(), String> + Send + Sync + 'static>,
    logger: &LiveReloadLogger,
) {
    let changed_path = normalize_path_string(&path);
    let candidates = resolve_candidates_for_change(&mount, &changed_path, files_by_root);

    if candidates.is_empty() {
        return;
    }

    let Ok(mut cache) = file_cache.lock() else {
        return;
    };

    for file_name in candidates {
        let Ok(content) = fs::read_to_string(&file_name) else {
            continue;
        };
        if cache
            .get(&file_name)
            .is_some_and(|previous| previous == &content)
        {
            continue;
        }
        cache.insert(file_name.clone(), content.clone());

        logger.info(format!(
            "hot reload detected: {}",
            hot_reload_display_name(&file_name)
        ));

        if let Err(err) = sink(LiveReloadFileChange {
            file_name: file_name.clone(),
            content,
        }) {
            logger.error(format!(
                "hot reload watcher channel closed before dispatch: {}",
                err
            ));
        }
    }
}

pub fn resolve_candidates_for_change(
    mount: &str,
    changed_path: &str,
    files_by_root: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    if files_by_root
        .get(mount)
        .is_some_and(|files| files.iter().any(|file| file == changed_path))
    {
        vec![changed_path.to_string()]
    } else {
        files_by_root.get(mount).cloned().unwrap_or_default()
    }
}

pub fn hot_reload_display_name(file_name: &str) -> String {
    Path::new(file_name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(file_name)
        .to_string()
}

pub fn normalize_relative_path_string(path: &Path) -> String {
    normalize_path(path).to_string_lossy().replace('\\', "/")
}

pub fn normalize_path_string(path: &Path) -> String {
    let path = if path.exists() {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    normalize_path(&path).to_string_lossy().replace('\\', "/")
}

pub fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            std::path::Component::RootDir => out.push(comp.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            std::path::Component::Normal(part) => out.push(part),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    };

    fn unique_temp_dir(label: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("makepad-live-reload-core-{label}-{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn candidate_selection_prefers_direct_path_hit() {
        let files_by_root = HashMap::from([(
            "root".to_string(),
            vec!["/tmp/a.rs".to_string(), "/tmp/b.rs".to_string()],
        )]);

        let candidates = resolve_candidates_for_change("root", "/tmp/a.rs", &files_by_root);
        assert_eq!(candidates, vec!["/tmp/a.rs".to_string()]);
    }

    #[test]
    fn candidate_selection_falls_back_to_mount_files() {
        let files_by_root = HashMap::from([(
            "root".to_string(),
            vec!["/tmp/a.rs".to_string(), "/tmp/b.rs".to_string()],
        )]);

        let candidates = resolve_candidates_for_change("root", "/tmp/c.rs", &files_by_root);
        assert_eq!(
            candidates,
            vec!["/tmp/a.rs".to_string(), "/tmp/b.rs".to_string()]
        );
    }

    #[test]
    fn dedupes_unchanged_content() {
        let dir = unique_temp_dir("dedupe");
        let file = dir.join("file.rs");
        fs::write(&file, "new").unwrap();

        let mount = normalize_path_string(&dir);
        let file_name = normalize_path_string(&file);

        let files_by_root = HashMap::from([(mount.clone(), vec![file_name.clone()])]);
        let file_cache = Mutex::new(HashMap::from([(file_name.clone(), "old".to_string())]));
        let delivered = Arc::new(Mutex::new(Vec::<String>::new()));

        let delivered_sink = Arc::clone(&delivered);
        let sink: Arc<dyn Fn(LiveReloadFileChange) -> Result<(), String> + Send + Sync> =
            Arc::new(move |change: LiveReloadFileChange| {
                let mut out = delivered_sink.lock().unwrap();
                out.push(change.content);
                Ok(())
            });
        let logger = LiveReloadLogger::new(|_| {}, |_| {});

        forward_hot_reload_fs_event(
            mount.clone(),
            PathBuf::from(&file_name),
            &files_by_root,
            &file_cache,
            &sink,
            &logger,
        );
        forward_hot_reload_fs_event(
            mount,
            PathBuf::from(&file_name),
            &files_by_root,
            &file_cache,
            &sink,
            &logger,
        );

        let out = delivered.lock().unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], "new");
    }

    #[test]
    fn sink_error_is_logged_and_next_file_continues() {
        let dir = unique_temp_dir("sink-error");
        let file_a = dir.join("a.rs");
        let file_b = dir.join("b.rs");
        fs::write(&file_a, "aa").unwrap();
        fs::write(&file_b, "bb").unwrap();

        let mount = normalize_path_string(&dir);
        let a = normalize_path_string(&file_a);
        let b = normalize_path_string(&file_b);

        let files_by_root = HashMap::from([(mount.clone(), vec![a.clone(), b.clone()])]);
        let file_cache = Mutex::new(HashMap::new());
        let delivered = Arc::new(Mutex::new(Vec::<String>::new()));
        let errors = Arc::new(Mutex::new(Vec::<String>::new()));

        let delivered_sink = Arc::clone(&delivered);
        let sink: Arc<dyn Fn(LiveReloadFileChange) -> Result<(), String> + Send + Sync> =
            Arc::new(move |change: LiveReloadFileChange| {
                let mut out = delivered_sink.lock().unwrap();
                out.push(change.file_name);
                if out.len() == 1 {
                    return Err("channel closed".to_string());
                }
                Ok(())
            });

        let errors_clone = Arc::clone(&errors);
        let logger = LiveReloadLogger::new(
            |_| {},
            move |message| {
                errors_clone.lock().unwrap().push(message);
            },
        );

        forward_hot_reload_fs_event(
            mount,
            dir,
            &files_by_root,
            &file_cache,
            &sink,
            &logger,
        );

        let out = delivered.lock().unwrap();
        assert_eq!(out.len(), 2);
        let errs = errors.lock().unwrap();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("hot reload watcher channel closed before dispatch"));
    }

    #[test]
    fn normalize_path_string_resolves_dot_segments() {
        let normalized = normalize_path_string(Path::new("alpha/./beta/../gamma.rs"));
        assert!(normalized.ends_with("alpha/gamma.rs"));
        assert!(!normalized.contains("/./"));
        assert!(!normalized.contains("beta/.."));
    }
}
