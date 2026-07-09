use super::*;
use makepad_filesystem_watcher::WatchRoot;
use makepad_studio_protocol::hub_protocol::{
    ClientId, HubToClient, LogEntry, QueryId, SearchResult,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

impl HubCore {
    pub(super) fn reset_fs_watcher(&mut self) {
        self.fs_watcher.take();
        self.fs_watch_events.clear();
        self.fs_event_last_by_path.clear();
        self.fs_recent_change_at_by_path.clear();
        self.fs_pending_diffs.clear();
        self.fs_pending_reload_mounts.clear();
        self.fs_diff_flush_scheduled = false;
        self.fs_event_last_prune = Instant::now();
        self.mount_suppress_fs_until.clear();
        self.self_save_suppress_until_by_path.clear();

        let roots: Vec<WatchRoot> = self
            .vfs
            .mounts()
            .into_iter()
            .map(|mount| WatchRoot {
                mount: mount.name,
                path: mount.path,
            })
            .collect();
        if roots.is_empty() {
            return;
        }

        let event_tx = self.event_tx.clone();
        let fs_watch_events = Arc::clone(&self.fs_watch_events);
        match FileSystemWatcher::start(roots, move |event| {
            if fs_watch_events.push(event.mount, event.path) {
                schedule_fs_event_flush(event_tx.clone());
            }
        }) {
            Ok(watcher) => {
                self.fs_watcher = Some(watcher);
            }
            Err(err) => {
                eprintln!("[studio2-backend] filesystem watcher unavailable: {}", err);
            }
        }
    }

    pub(super) fn queue_mount_fs_changed(&mut self, mount: String, path: PathBuf) {
        if self.fs_watch_events.push(mount, path) {
            schedule_fs_event_flush(self.event_tx.clone());
        }
    }

    pub(super) fn flush_pending_mount_fs_events(&mut self) {
        let pending = self.fs_watch_events.take_ready();
        if pending.is_empty() {
            return;
        }

        let mut by_mount: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for (mount, path) in pending {
            by_mount.entry(mount).or_default().push(path);
        }
        for (mount, paths) in by_mount {
            self.flush_pending_mount_fs_events_for_mount(mount, paths);
        }
    }

    pub(super) fn flush_pending_mount_fs_events_for_mount(
        &mut self,
        mount: String,
        paths: Vec<PathBuf>,
    ) {
        let mut paths_to_process = Vec::with_capacity(paths.len());
        let mut saw_git_status_change = false;
        for path in paths {
            let Some(virtual_path) = self.mount_path_to_virtual(&mount, &path) else {
                paths_to_process.push(path);
                continue;
            };
            if self.is_git_status_watch_virtual_path(&mount, &virtual_path) {
                saw_git_status_change = true;
                continue;
            }
            if self.should_ignore_fs_watch_virtual_path(&mount, &virtual_path) {
                continue;
            }
            paths_to_process.push(path);
        }

        if saw_git_status_change {
            self.invalidate_git_status_cache_for_mount(&mount);
            self.reload_mount_file_tree_broadcast(&mount);
        }

        if paths_to_process.is_empty() {
            return;
        }
        paths_to_process.sort();
        paths_to_process.dedup();

        if paths_to_process.len() > FS_EVENT_BATCH_RELOAD_THRESHOLD {
            self.process_mount_fs_storm(&mount);
            return;
        }

        for path in paths_to_process {
            self.process_mount_fs_changed(mount.clone(), path);
        }
    }

    pub(super) fn enqueue_file_tree_load_for_client(&mut self, mount: String, client_id: ClientId) {
        let mut waiters = HashSet::new();
        waiters.insert(client_id);
        self.enqueue_file_tree_load(mount, waiters);
    }

    pub(super) fn enqueue_file_tree_load_for_all_clients(&mut self, mount: &str) {
        let waiters: HashSet<ClientId> = self.ui_clients.keys().copied().collect();
        self.enqueue_file_tree_load(mount.to_string(), waiters);
    }

    pub(super) fn enqueue_file_tree_load(&mut self, mount: String, new_waiters: HashSet<ClientId>) {
        if new_waiters.is_empty() {
            return;
        }
        let waiters = self
            .file_tree_load_waiters
            .entry(mount.clone())
            .or_default();
        let first_request = waiters.is_empty();
        waiters.extend(new_waiters);
        if !first_request {
            return;
        }

        let mount_name = mount.clone();
        let vfs = self.vfs.clone_for_search();
        let event_tx = self.event_tx.clone();
        self.worker_pool.execute(move || {
            let result = vfs
                .load_file_tree(&mount_name)
                .map_err(|err| err.to_string());
            let _ = event_tx.send(HubEvent::WorkerLoadFileTreeDone {
                mount: mount_name,
                result,
            });
        });
    }

    pub(super) fn process_mount_fs_changed(&mut self, mount: String, path: PathBuf) {
        let now = Instant::now();
        let path_is_file = path.is_file();
        let path_is_dir = path.is_dir();
        if self
            .mount_suppress_fs_until
            .get(&mount)
            .is_some_and(|until| now >= *until)
        {
            self.mount_suppress_fs_until.remove(&mount);
        }
        let Some(virtual_path) = self.mount_path_to_virtual(&mount, &path) else {
            self.reload_mount_file_tree_broadcast(&mount);
            return;
        };
        if self.is_git_status_watch_virtual_path(&mount, &virtual_path) {
            self.invalidate_git_status_cache_for_mount(&mount);
            self.reload_mount_file_tree_broadcast(&mount);
            return;
        }
        if self.should_ignore_fs_watch_virtual_path(&mount, &virtual_path) {
            return;
        }
        if virtual_path == mount {
            if self
                .mount_suppress_fs_until
                .get(&mount)
                .is_some_and(|until| now < *until)
            {
                return;
            }
            if self.should_suppress_self_save_mount_root_event(&mount, now) {
                return;
            }
            self.record_recent_fs_change(mount.clone(), now);
            self.process_ai_path_change(&mount, &mount);
            // Some watcher implementations only report "mount root changed".
            // Broadcast a mount-level FileChanged so UI can refresh open tabs.
            self.broadcast_ui_message(HubToClient::FileChanged {
                path: mount.clone(),
            });
            self.maybe_revive_mount_root_splash_from_fs_fallback(&mount);
            self.reload_mount_file_tree_broadcast(&mount);
            return;
        }
        if self.should_suppress_self_save_event(&virtual_path, now) {
            return;
        }
        self.record_recent_fs_change(virtual_path.clone(), now);
        self.process_ai_path_change(&mount, &virtual_path);
        if Self::is_mount_root_splash_virtual_path(&mount, &virtual_path) {
            self.request_mount_root_splash_reload(&mount);
        }
        if path_is_file && !self.should_ignore_virtual_path(&mount, &virtual_path) {
            self.broadcast_ui_message(HubToClient::FileChanged {
                path: virtual_path.clone(),
            });
            if virtual_path.ends_with(".rs") {
                if let Ok(content) = fs::read_to_string(&path) {
                    let file_name = path.canonicalize().unwrap_or_else(|_| path.clone());
                    self.forward_live_change_to_builds(
                        "watch",
                        &virtual_path,
                        file_name.to_string_lossy().replace('\\', "/"),
                        content,
                    );
                }
            }
        }
        if path_is_dir {
            self.maybe_revive_mount_root_splash_from_fs_fallback(&mount);
            self.reload_mount_file_tree_broadcast(&mount);
            return;
        }
        let (path, virtual_path) =
            self.collapse_removed_path_to_missing_ancestor(&mount, path, virtual_path);
        self.enqueue_file_tree_delta(&mount, &virtual_path, path, now);
    }

    pub(super) fn process_mount_fs_storm(&mut self, mount: &str) {
        let now = Instant::now();
        self.record_recent_fs_change(mount.to_string(), now);
        self.process_ai_path_change(mount, mount);
        self.broadcast_ui_message(HubToClient::FileChanged {
            path: mount.to_string(),
        });
        self.maybe_revive_mount_root_splash_from_fs_fallback(mount);
        self.reload_mount_file_tree_broadcast(mount);
    }

    pub(super) fn suppress_mount_root_fs_events(&mut self, mount: &str, duration: Duration) {
        let until = Instant::now() + duration;
        self.mount_suppress_fs_until
            .entry(mount.to_string())
            .and_modify(|existing| {
                if *existing < until {
                    *existing = until;
                }
            })
            .or_insert(until);
    }

    pub(super) fn collapse_removed_path_to_missing_ancestor(
        &self,
        mount: &str,
        path: PathBuf,
        virtual_path: String,
    ) -> (PathBuf, String) {
        if path.exists() {
            return (path, virtual_path);
        }
        let mount_root = match self.vfs.resolve_mount(mount) {
            Ok(root) => root,
            Err(_) => return (path, virtual_path),
        };
        let mut probe = path.clone();
        let mut collapsed = None;
        loop {
            if !probe.starts_with(&mount_root) || probe.exists() {
                break;
            }
            collapsed = Some(probe.clone());
            if probe == mount_root || !probe.pop() {
                break;
            }
        }
        let Some(collapsed_path) = collapsed else {
            return (path, virtual_path);
        };
        let Some(collapsed_virtual) = self.mount_path_to_virtual(mount, &collapsed_path) else {
            return (path, virtual_path);
        };
        if collapsed_virtual == mount {
            return (path, virtual_path);
        }
        (collapsed_path, collapsed_virtual)
    }

    pub(super) fn mount_path_to_virtual(&self, mount: &str, path: &Path) -> Option<String> {
        let mount_root = self.vfs.resolve_mount(mount).ok()?;
        let path = path
            .strip_prefix(&mount_root)
            .ok()
            .map(Path::to_path_buf)
            .or_else(|| {
                #[cfg(target_os = "macos")]
                {
                    let normalized_mount_root = normalize_macos_private_alias(&mount_root);
                    let normalized_path = normalize_macos_private_alias(path);
                    normalized_path
                        .strip_prefix(&normalized_mount_root)
                        .ok()
                        .map(Path::to_path_buf)
                }
                #[cfg(not(target_os = "macos"))]
                {
                    None
                }
            })?;
        if path.as_os_str().is_empty() {
            return Some(mount.to_string());
        }
        let path_string = path.to_string_lossy().replace('\\', "/");
        if let Some(rest) = path_string.strip_prefix("branch/") {
            if let Some((branch, tail)) = rest.split_once('/') {
                let encoded = percent_encode_local(branch);
                return Some(format!("{}/@{}/{}", mount, encoded, tail));
            }
            let encoded = percent_encode_local(rest);
            return Some(format!("{}/@{}", mount, encoded));
        }
        Some(format!("{}/{}", mount, path_string))
    }

    pub(super) fn enqueue_file_tree_delta_for_virtual_path(&mut self, virtual_path: &str) {
        let Some((_mount, _)) = virtual_path.split_once('/') else {
            return;
        };
        let disk_path = match self.vfs.resolve_path(virtual_path) {
            Ok(path) => path,
            Err(_) => return,
        };
        self.enqueue_file_tree_delta_for_known_path(virtual_path, disk_path);
    }

    pub(super) fn enqueue_file_tree_delta_for_known_path(
        &mut self,
        virtual_path: &str,
        disk_path: PathBuf,
    ) {
        let Some((mount, _)) = virtual_path.split_once('/') else {
            return;
        };
        self.enqueue_file_tree_delta(mount, virtual_path, disk_path, Instant::now());
    }

    pub(super) fn enqueue_file_tree_delta(
        &mut self,
        mount: &str,
        virtual_path: &str,
        disk_path: PathBuf,
        now: Instant,
    ) {
        if self.should_ignore_virtual_path(mount, virtual_path) {
            return;
        }
        self.prune_fs_event_history(now);
        if let Some(last) = self.fs_event_last_by_path.get(virtual_path).copied() {
            if now.saturating_duration_since(last) < FS_EVENT_PATH_DEBOUNCE {
                return;
            }
        }
        self.fs_event_last_by_path
            .insert(virtual_path.to_string(), now);

        if let Ok(mut cache_guard) = self.git_status_cache.lock() {
            cache_guard.entries.remove(&disk_path);
        }

        let mount = mount.to_string();
        let virtual_path = virtual_path.to_string();
        let event_tx = self.event_tx.clone();
        let git_status_cache = Arc::clone(&self.git_status_cache);
        self.worker_pool.execute(move || {
            let change =
                compute_filetree_change_for_path(&git_status_cache, &disk_path, virtual_path);
            let _ = event_tx.send(HubEvent::WorkerFileTreeDeltaDone { mount, change });
        });
    }

    pub(super) fn invalidate_git_status_cache_for_mount(&mut self, mount: &str) {
        let Ok(root) = self.vfs.resolve_mount(mount) else {
            return;
        };
        if let Ok(mut cache_guard) = self.git_status_cache.lock() {
            cache_guard
                .entries
                .retain(|path, _| !path.starts_with(&root));
        }
    }

    pub(super) fn is_git_status_watch_virtual_path(&self, mount: &str, virtual_path: &str) -> bool {
        let prefix = format!("{}/", mount);
        let Some(rest) = virtual_path.strip_prefix(&prefix) else {
            return false;
        };
        rest == ".git" || rest.starts_with(".git/")
    }

    pub(super) fn should_ignore_fs_watch_virtual_path(
        &self,
        mount: &str,
        virtual_path: &str,
    ) -> bool {
        let prefix = format!("{}/", mount);
        let Some(rest) = virtual_path.strip_prefix(&prefix) else {
            return false;
        };
        rest == ".git"
            || rest.starts_with(".git/")
            || rest == ".makepad"
            || rest.starts_with(".makepad/")
    }

    pub(super) fn should_ignore_virtual_path(&self, mount: &str, virtual_path: &str) -> bool {
        if virtual_path == mount {
            return true;
        }
        let prefix = format!("{}/", mount);
        let Some(rest) = virtual_path.strip_prefix(&prefix) else {
            return true;
        };
        rest == "target"
            || rest.starts_with("target/")
            || rest == ".git"
            || rest.starts_with(".git/")
            || rest == ".makepad"
            || rest.starts_with(".makepad/")
    }

    pub(super) fn reload_mount_file_tree_broadcast(&mut self, mount: &str) {
        let now = Instant::now();
        self.prune_fs_event_history(now);
        let reload_key = format!("__mount_reload__/{}", mount);
        if let Some(last) = self.fs_event_last_by_path.get(&reload_key).copied() {
            if now.saturating_duration_since(last) < FS_EVENT_RELOAD_DEBOUNCE {
                // Don't drop the reload: re-queue it so bursty fs events still
                // produce one eventual tree refresh after debounce.
                self.fs_pending_reload_mounts.insert(mount.to_string());
                self.schedule_fs_diff_flush();
                return;
            }
        }
        self.fs_event_last_by_path.insert(reload_key, now);
        self.enqueue_file_tree_load_for_all_clients(mount);
    }

    pub(super) fn queue_file_tree_delta_change(
        &mut self,
        mount: String,
        change: backend_proto::FileTreeChange,
    ) {
        if self.fs_pending_reload_mounts.contains(&mount) {
            self.schedule_fs_diff_flush();
            return;
        }
        let pending = self.fs_pending_diffs.entry(mount.clone()).or_default();
        coalesce_file_tree_change(pending, change);
        if pending.len() >= FS_DELTA_RELOAD_THRESHOLD {
            self.fs_pending_diffs.remove(&mount);
            self.fs_pending_reload_mounts.insert(mount);
        }
        self.schedule_fs_diff_flush();
    }

    pub(super) fn schedule_fs_diff_flush(&mut self) {
        if self.fs_diff_flush_scheduled {
            return;
        }
        self.fs_diff_flush_scheduled = true;
        let event_tx = self.event_tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(FS_DELTA_FLUSH_DELAY);
            let _ = event_tx.send(HubEvent::FlushPendingFileTreeDiffs);
        });
    }

    pub(super) fn flush_pending_file_tree_diffs(&mut self) {
        self.fs_diff_flush_scheduled = false;

        let reload_mounts: Vec<String> = self.fs_pending_reload_mounts.drain().collect();
        for mount in reload_mounts {
            self.reload_mount_file_tree_broadcast(&mount);
        }

        let pending = std::mem::take(&mut self.fs_pending_diffs);
        for (mount, mut changes) in pending {
            if changes.is_empty() {
                continue;
            }
            changes.sort_by(|a, b| file_tree_change_path(a).cmp(file_tree_change_path(b)));
            self.broadcast_ui_message(HubToClient::FileTreeDiff { mount, changes });
        }
    }

    pub(super) fn prune_fs_event_history(&mut self, now: Instant) {
        if now.saturating_duration_since(self.fs_event_last_prune) < FS_EVENT_HISTORY_PRUNE_INTERVAL
        {
            return;
        }
        self.fs_event_last_prune = now;
        self.fs_event_last_by_path
            .retain(|_, ts| now.saturating_duration_since(*ts) < FS_EVENT_HISTORY_RETENTION);
        self.fs_recent_change_at_by_path
            .retain(|_, ts| now.saturating_duration_since(*ts) < FS_RECENT_CHANGE_RETENTION);
        self.self_save_suppress_until_by_path
            .retain(|_, until| *until > now);
    }

    pub(super) fn record_recent_fs_change(&mut self, path: String, now: Instant) {
        self.fs_recent_change_at_by_path.insert(path, now);
    }

    pub(super) fn should_suppress_self_save_event(
        &mut self,
        virtual_path: &str,
        now: Instant,
    ) -> bool {
        self.self_save_suppress_until_by_path
            .retain(|_, until| *until > now);
        self.self_save_suppress_until_by_path
            .get(virtual_path)
            .is_some_and(|until| now < *until)
    }

    pub(super) fn should_suppress_self_save_mount_root_event(
        &mut self,
        mount: &str,
        now: Instant,
    ) -> bool {
        self.self_save_suppress_until_by_path
            .retain(|_, until| *until > now);
        let mount_prefix = format!("{}/", mount);
        self.self_save_suppress_until_by_path
            .iter()
            .any(|(path, until)| now < *until && path.starts_with(&mount_prefix))
    }

    pub(super) fn on_worker_find_files_done(
        &mut self,
        client_id: ClientId,
        query_id: QueryId,
        result: Result<Vec<String>, String>,
    ) {
        if self.cancelled_queries.remove(&query_id) {
            return;
        }

        match result {
            Ok(paths) => self.send_ui_reply(
                client_id,
                HubToClient::FindFileResults {
                    query_id,
                    paths,
                    done: true,
                },
            ),
            Err(err) => self.send_ui_error(client_id, err),
        }
    }

    pub(super) fn on_worker_find_in_files_done(
        &mut self,
        client_id: ClientId,
        query_id: QueryId,
        result: Result<Vec<SearchResult>, String>,
    ) {
        if self.cancelled_queries.remove(&query_id) {
            return;
        }

        match result {
            Ok(results) => self.send_ui_reply(
                client_id,
                HubToClient::SearchFileResults {
                    query_id,
                    results,
                    done: true,
                },
            ),
            Err(err) => self.send_ui_error(client_id, err),
        }
    }

    pub(super) fn on_worker_query_logs_done(
        &mut self,
        client_id: ClientId,
        query_id: QueryId,
        query: LogQuery,
        live: bool,
        entries: Vec<(usize, LogEntry)>,
    ) {
        if self.cancelled_queries.remove(&query_id) {
            return;
        }

        self.send_ui_reply(
            client_id,
            HubToClient::QueryLogResults {
                query_id,
                entries,
                done: !live,
            },
        );

        if live && self.ui_clients.contains_key(&client_id) {
            self.live_log_queries
                .insert(query_id, LiveLogSubscription { client_id, query });
        }
    }

    pub(super) fn on_worker_load_file_tree_done(
        &mut self,
        mount: String,
        result: Result<backend_proto::FileTreeData, String>,
    ) {
        let waiters = self
            .file_tree_load_waiters
            .remove(&mount)
            .unwrap_or_default();
        if waiters.is_empty() {
            return;
        }
        match result {
            Ok(data) => {
                for client_id in waiters {
                    self.send_ui_reply(
                        client_id,
                        HubToClient::FileTree {
                            mount: mount.clone(),
                            data: data.clone(),
                        },
                    );
                }
            }
            Err(err) => {
                for client_id in waiters {
                    self.send_ui_error(client_id, err.clone());
                }
            }
        }
    }

    pub(super) fn send_branch_op_result(
        &self,
        client_id: ClientId,
        mount: String,
        before: Option<backend_proto::FileTreeData>,
        result: Result<(), impl std::fmt::Display>,
    ) {
        if let Err(err) = result {
            self.send_ui_error(client_id, err.to_string());
            return;
        }
        match self.vfs.load_file_tree(&mount) {
            Ok(data) => self.send_ui_reply(
                client_id,
                HubToClient::FileTree {
                    mount: mount.clone(),
                    data: data.clone(),
                },
            ),
            Err(err) => self.send_ui_error(client_id, err.to_string()),
        }
        if let Some(before) = before {
            if let Ok(after) = self.vfs.load_file_tree(&mount) {
                self.send_ui_reply(
                    client_id,
                    HubToClient::FileTreeDiff {
                        mount,
                        changes: file_tree_diff(&before, &after),
                    },
                );
            }
        }
    }
}
