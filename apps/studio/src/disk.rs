//! Bounded, read-only disk inventory. One lifetime worker owns filesystem I/O;
//! the UI exchanges commands and immutable snapshots without blocking.
use makepad_widgets::makepad_platform::thread::{SignalToUI, TaskHandle, ThreadOptions, ThreadSpawner};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, SyncSender, TrySendError}, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct Volume {
    pub total: u64,
    pub used: u64,
    pub available: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub apparent: u64,
    pub allocated: Option<u64>,
    pub files: u64,
    /// A partial scan is a lower bound, never a complete zero-size directory.
    pub incomplete: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: &'static str,
    pub associations: Vec<PathBuf>,
    pub usage: Usage,
    pub cleanup_blocked: String,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub root: PathBuf,
    pub volume: Option<Volume>,
    pub volume_at: u64,
    pub scanned_at: u64,
    pub scanning: bool,
    pub entries: Vec<Entry>,
    pub errors: Vec<String>,
    pub history: Vec<(u64, u64)>,
}

pub fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
pub fn size(bytes: u64) -> String { format!("{:.1} GiB", bytes as f64 / 1_073_741_824.0) }

impl Snapshot {
    pub fn summary(&self) -> String {
        match &self.volume {
            Some(v) => format!("Disk {:.0}% · {} free", 100.0 * v.used as f64 / v.total.max(1) as f64, size(v.available)),
            None => "Disk usage unavailable".into(),
        }
    }

    pub fn report(&self) -> String {
        let mut s = format!("{}\nRepository: {}\n", self.summary(), self.root.display());
        if let Some(v) = &self.volume {
            s.push_str(&format!("Volume: {} used / {} total; sampled {}s ago\n", size(v.used), size(v.total), now().saturating_sub(self.volume_at)));
        }
        s.push_str(if self.scanning { "Inspecting workspaces and build directories…\n" } else { "" });
        if self.scanned_at != 0 { s.push_str(&format!("Inventory: {}s ago\n", now().saturating_sub(self.scanned_at))); }
        if self.history.len() > 1 {
            s.push_str(&format!("Recent change: {:+.2} GiB over {}s ({} samples)\n", (self.history.last().unwrap().1 as f64-self.history[0].1 as f64)/1_073_741_824.0, self.history.last().unwrap().0.saturating_sub(self.history[0].0), self.history.len()));
        }
        s.push_str("\nBuild folders are listed separately. Shared paths and hard links count once. Sizes are estimates; partial scans are lower bounds. Actual space reclaimed may differ. Use Preview cleanup to inspect a path before acting.\n");
        for e in &self.entries {
            s.push_str(&format!("\n{} · {} apparent · {} allocated · {} files\n{}\n", e.kind,
                size(e.usage.apparent), e.usage.allocated.map(size).unwrap_or_else(|| "unknown".into()), e.usage.files, e.path.display()));
            if e.associations.len() > 1 { s.push_str(&format!("Shared by {} workspaces\n", e.associations.len())); }
            if let Some(reason) = &e.usage.incomplete { s.push_str(&format!("Partial measurement: {reason}\n")); }

        }
        for error in &self.errors { s.push_str(&format!("\nUnavailable: {error}\n")); }
        s
    }

    pub fn cleanup_preview(&self, path: &Path) -> Result<String, String> {
        let entry = self.entries.iter().find(|e| e.path == path).ok_or("path is not in the current inventory")?;
        Ok(format!("Path: {}\nKind: {}\nObserved: {} apparent; {} allocated\nEstimated reclaimable: unknown\nRemoval blocked: {}\nNo files changed.", entry.path.display(), entry.kind,
            size(entry.usage.apparent), entry.usage.allocated.map(size).unwrap_or_else(|| "unknown".into()), entry.cleanup_blocked))
    }
}

pub struct DiskWorker {
    commands: SyncSender<()>,
    snapshots: Receiver<Arc<Snapshot>>,
    stop: Arc<AtomicBool>,
    _task: TaskHandle<()>,
    retry: bool,
}

impl DiskWorker {
    pub fn start(spawner: &ThreadSpawner, cwd: PathBuf) -> Result<Self, String> {
        let (commands, rx) = mpsc::sync_channel(1);
        let (tx, snapshots) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = stop.clone();
        let task = spawner.spawn_worker(ThreadOptions { name: Some("studio-disk".into()), ..Default::default() }, move || {
            #[cfg(not(target_arch = "wasm32"))]
            native::run(cwd, rx, tx, cancel);
            #[cfg(target_arch = "wasm32")]
            {
                let _ = (cwd, rx, cancel);
                let s = Snapshot { errors: vec!["Native filesystem inspection is unavailable in this browser".into()], ..Default::default() };
                let _ = tx.try_send(Arc::new(s));
                SignalToUI::set_ui_signal();
            }
        }).map_err(|e| e.to_string())?;
        Ok(Self { commands, snapshots, stop, _task: task, retry: false })
    }
    pub fn request_stop(&self) { self.stop.store(true, Ordering::Relaxed); }
    pub fn is_finished(&self) -> bool { self._task.is_finished() }
    pub fn refresh(&mut self) { self.retry = true; self.retry_command(); }
    fn retry_command(&mut self) {
        if self.retry {
            match self.commands.try_send(()) {
                Ok(()) => self.retry = false,
                Err(TrySendError::Full(_)) => {},
                Err(TrySendError::Disconnected(_)) => self.retry = false,
            }
        }
    }
    pub fn poll(&mut self) -> Option<Arc<Snapshot>> {
        self.retry_command();
        let mut latest = None;
        while let Ok(snapshot) = self.snapshots.try_recv() { latest = Some(snapshot); }
        latest
    }
}

impl Drop for DiskWorker { fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); } }

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::{fs, io::Read, process::{Command, Stdio}};

    // Local tools run with no shell, bounded time and bounded output. A file
    // avoids blocking on a full stdout pipe while polling cancellation.
    fn command(program: &str, args: &[&str], cwd: &Path, stop: &AtomicBool) -> Result<String, String> {
        if stop.load(Ordering::Relaxed) { return Err("inspection cancelled".into()); }
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("studio-disk-{}-{stamp}", std::process::id()));
        struct Temp(PathBuf);
        impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_file(&self.0); } }
        let file = fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
        let _temp = Temp(path.clone());
        let mut child = Command::new(program).args(args).current_dir(cwd)
            .env("LC_ALL", "C").env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null()).stdout(file).stderr(Stdio::null()).spawn().map_err(|e| format!("{program}: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(status)) => return Err(format!("{program} exited {status}")),
                Err(e) => { let _ = child.kill(); let _ = child.wait(); return Err(e.to_string()); },
                _ => {},
            }
            if stop.load(Ordering::Relaxed) || Instant::now() > deadline || fs::metadata(&path).map(|m| m.len() > 8*1024*1024).unwrap_or(false) {
                let _ = child.kill(); let _ = child.wait();
                return Err(format!("{program} cancelled or exceeded its time/output budget"));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let mut bytes = Vec::new();
        fs::File::open(&path).map_err(|e| e.to_string())?.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() > 8 * 1024 * 1024 { return Err(format!("{program} output exceeded 8 MiB")); }
        String::from_utf8(bytes).map_err(|_| format!("{program} returned a non-UTF-8 path or response"))
    }

    fn sample_volume(s: &mut Snapshot, stop: &AtomicBool) {
                match command("df", &["-Pk", "."], &s.root, &stop).and_then(|s| parse_volume(&s)) {
                    Ok(v) => {
                        s.volume_at = now();
                        s.history.push((s.volume_at, v.used));
                        if s.history.len() > 120 { s.history.remove(0); }
                        s.volume = Some(v);
                    },
                    Err(e) => { s.volume = None; if !s.errors.contains(&e) { s.errors.push(e); } },
                }
    }

    pub(super) fn run(cwd: PathBuf, rx: Receiver<()>, tx: SyncSender<Arc<Snapshot>>, stop: Arc<AtomicBool>) {
        let mut s = Snapshot { root: cwd.clone(), ..Default::default() };
        match command("git", &["rev-parse", "--show-toplevel"], &cwd, &stop) {
            Ok(root) => s.root = PathBuf::from(root.trim_end()),
            Err(e) => s.errors.push(format!("repository discovery: {e}")),
        }
        let mut next_sample = Instant::now();
        let mut next_inventory = Instant::now();
        let mut pending: Option<Arc<Snapshot>> = None;
        while !stop.load(Ordering::Relaxed) {
            if Instant::now() >= next_inventory { s.scanning = true; }
            if Instant::now() >= next_sample {
                sample_volume(&mut s, &stop);
                next_sample = Instant::now() + Duration::from_secs(15);
                pending = Some(Arc::new(s.clone()));
            }
            if let Some(snapshot) = pending.take() {
                match tx.try_send(snapshot) {
                    Ok(()) => SignalToUI::set_ui_signal(),
                    Err(TrySendError::Full(snapshot)) => pending = Some(snapshot),
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
            if Instant::now() >= next_inventory {
                s.scanning = true;
                if tx.try_send(Arc::new(s.clone())).is_ok() { SignalToUI::set_ui_signal(); }
                let root = s.root.clone();
                let (entries, errors) = inventory(&root, &stop, &mut || {
                    if Instant::now() >= next_sample && !stop.load(Ordering::Relaxed) {
                        sample_volume(&mut s, &stop);
                        next_sample = Instant::now() + Duration::from_secs(15);
                        if tx.try_send(Arc::new(s.clone())).is_ok() { SignalToUI::set_ui_signal(); }
                    }
                });
                s.entries = entries;
                s.errors = errors;
                s.scanned_at = now();
                s.scanning = false;
                pending = Some(Arc::new(s.clone()));
                next_inventory = Instant::now() + Duration::from_secs(120);
                continue;
            }
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(()) => { next_inventory = Instant::now(); next_sample = Instant::now(); },
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                _ => {},
            }
        }
    }

    pub(super) fn parse_volume(text: &str) -> Result<Volume, String> {
        let fields: Vec<&str> = text.lines().nth(1).ok_or("df returned no volume")?.split_whitespace().collect();
        // Parse from the capacity field so device names containing spaces do
        // not shift the numeric columns. Mount names may also contain spaces.
        let cap = fields.iter().position(|s| s.ends_with('%')).ok_or("df has no capacity")?;
        if cap < 3 { return Err("df has incomplete sizes".into()); }
        let bytes = |i: usize| fields[i].parse::<u64>().ok().and_then(|v| v.checked_mul(1024)).ok_or_else(|| "df has invalid sizes".to_string());
        let v = Volume { total: bytes(cap-3)?, used: bytes(cap-2)?, available: bytes(cap-1)? };
        if v.total == 0 { return Err("df reported a zero capacity".into()); }
        Ok(v)
    }

    fn add_target(entries: &mut Vec<Entry>, target: &Path, workspace: &Path, errors: &mut Vec<String>) {
        match fs::canonicalize(target) {
            Ok(target) => {
                if let Some(e) = entries.iter_mut().find(|e| e.path == target) {
                    if !e.associations.iter().any(|p| p == workspace) { e.associations.push(workspace.to_owned()); }
                } else {
                    entries.push(Entry { path: target, kind: "Build directory", associations: vec![workspace.to_owned()], usage: Usage::default(), cleanup_blocked: "Live build/process ownership is unknown. Inspect this path before removing artifacts; automatic removal is unavailable.".into() });
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => errors.push(format!("{}: {e}", target.display())),
        }
    }

    // Discover conventional build directories without descending into them.
    // Nested workspace manifests and local Cargo config directories are also
    // metadata candidates, so their configured external/shared targets count.
    fn discover(root: &Path, deadline: Instant, stop: &AtomicBool, tick: &mut dyn FnMut()) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<String>) {
        let mut dirs = vec![root.to_owned()];
        let mut targets = Vec::new();
        let mut configs = vec![root.to_owned()];
        let mut errors = Vec::new();
        while let Some(dir) = dirs.pop() {
            tick();
            if stop.load(Ordering::Relaxed) || Instant::now() >= deadline { errors.push(format!("{}: build-directory discovery is partial (time budget)", root.display())); break; }
            let children = match fs::read_dir(&dir) { Ok(c) => c, Err(e) => { errors.push(format!("{} discovery: {e}", dir.display())); continue; } };
            for child in children {
                if stop.load(Ordering::Relaxed) || Instant::now() >= deadline { errors.push(format!("{}: build-directory discovery is partial (time budget)", root.display())); return (targets, configs, errors); }
                let child = match child { Ok(c) => c, Err(e) => { errors.push(e.to_string()); continue; } };
                let path = child.path();
                let kind = match child.file_type() { Ok(t) => t, Err(e) => { errors.push(format!("{}: {e}", path.display())); continue; } };
                if kind.is_symlink() { continue; }
                if kind.is_dir() {
                    if child.file_name() == ".git" { continue; }
                    if child.file_name() == "target" { targets.push(path); continue; }
                    if child.file_name() == ".cargo" && !configs.contains(&dir) { configs.push(dir.clone()); }
                    dirs.push(path);
                } else if child.file_name() == "Cargo.toml" && dir != root {
                    // Candidate discovery only: Cargo resolves the real workspace
                    // and target. A false positive merely adds a bounded read.
                    if let Ok(file) = fs::File::open(&path) {
                        let mut text = String::new();
                        if file.take(128 * 1024).read_to_string(&mut text).is_ok()
                            && text.lines().any(|l| l.split('#').next().unwrap_or("").trim() == "[workspace]")
                            && !configs.contains(&dir) { configs.push(dir.clone()); }
                    }
                }
            }
        }
        (targets, configs, errors)
    }

    fn inventory(root: &Path, stop: &AtomicBool, tick: &mut dyn FnMut()) -> (Vec<Entry>, Vec<String>) {
        let mut errors = Vec::new();
        let mut worktrees = Vec::new();
        let discovery_deadline = Instant::now() + Duration::from_secs(15);
        match command("git", &["worktree", "list", "--porcelain", "-z"], root, stop) {
            Ok(text) => for field in text.split('\0') {
                if let Some(path) = field.strip_prefix("worktree ") { worktrees.push(PathBuf::from(path)); }
            },
            Err(e) => errors.push(format!("worktree list: {e}")),
        }
        if worktrees.is_empty() { worktrees.push(root.to_owned()); }
        if worktrees.len() > 64 { errors.push(format!("Inventory covers 64 of {} workspaces", worktrees.len())); worktrees.truncate(64); }
        let mut entries: Vec<Entry> = Vec::new();
        let mut roots = Vec::new();
        let mut configs = Vec::new();
        for worktree in &worktrees {
            if stop.load(Ordering::Relaxed) { break; }
            let canonical = match fs::canonicalize(worktree) {
                Ok(p) => p,
                Err(e) => { errors.push(format!("{}: {e}", worktree.display())); continue; },
            };
            if roots.contains(&canonical) { continue; }
            roots.push(canonical.clone());
            let (targets, candidates, mut discovery_errors) = discover(&canonical, discovery_deadline, stop, tick);
            errors.append(&mut discovery_errors);
            for target in targets { add_target(&mut entries, &target, &canonical, &mut errors); }
            for candidate in candidates {
                if !configs.iter().any(|(p, _)| p == &candidate) { configs.push((candidate, canonical.clone())); }
            }
        }
        // Metadata cannot update Cargo.lock or download dependencies. A missing
        // lock/config is reported, with conventional target discovery retained.
        let metadata_deadline = Instant::now() + Duration::from_secs(20);
        for (index, (config, workspace)) in configs.iter().enumerate() {
            tick();
            if stop.load(Ordering::Relaxed) || Instant::now() >= metadata_deadline || index >= 24 {
                errors.push(format!("Configured targets checked for {index}/{} Cargo locations; remaining custom paths unknown", configs.len()));
                break;
            }
            let target = command("cargo", &["metadata", "--no-deps", "--format-version=1", "--frozen"], config, stop)
                .and_then(|text| makepad_strict_json::parse(text.as_bytes()).map_err(str::to_owned))
                .and_then(|json| json.get("target_directory").and_then(|v| v.as_str()).map(PathBuf::from).ok_or("Cargo metadata has no target directory".into()));
            match target {
                Ok(target) => add_target(&mut entries, &target, workspace, &mut errors),
                Err(e) => errors.push(format!("{} build configuration: {e}; custom target coverage unknown", config.display())),
            }
        }
        let targets: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
        for root in &roots {
            entries.push(Entry { path: root.clone(), kind: "Workspace", associations: vec![root.clone()], usage: Usage::default(), cleanup_blocked: "Agent/process ownership is not yet tracked. Current, dirty, locked and active worktrees must be retained; automatic worktree removal is unavailable.".into() });
        }
        let mut seen = HashSet::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        for entry in &mut entries {
            let exclusions: Vec<_> = targets.iter().chain(roots.iter()).filter(|p| **p != entry.path).cloned().collect();
            tick();
            entry.usage = walk(&entry.path, &exclusions, &mut seen, deadline, stop, tick);
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.usage.allocated.unwrap_or(e.usage.apparent)));
        (entries, errors)
    }

    pub(super) fn walk(root: &Path, exclusions: &[PathBuf], seen: &mut HashSet<(u64, u64)>, deadline: Instant, stop: &AtomicBool, tick: &mut dyn FnMut()) -> Usage {
        let mut usage = Usage { allocated: if cfg!(unix) { Some(0) } else { None }, ..Default::default() };
        let mut todo = vec![root.to_owned()];
        while let Some(path) = todo.pop() {
            if usage.files % 1024 == 0 { tick(); }
            if stop.load(Ordering::Relaxed) || Instant::now() >= deadline || seen.len() >= 1_000_000 { usage.incomplete = Some("scan time budget reached or cancelled".into()); break; }
            if exclusions.contains(&path) || path.file_name().is_some_and(|n| n == ".git") { continue; }
            let meta = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) => { usage.incomplete = Some(format!("{}: {e}", path.display())); continue; },
            };
            if meta.file_type().is_symlink() { continue; }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if !seen.insert((meta.dev(), meta.ino())) { continue; }
                usage.allocated = usage.allocated.map(|v| v.saturating_add(meta.blocks().saturating_mul(512)));
            }
            if meta.is_dir() {
                match fs::read_dir(&path) {
                    Ok(children) => for child in children {
                        if stop.load(Ordering::Relaxed) || Instant::now() >= deadline || todo.len() >= 250_000 {
                            usage.incomplete = Some("scan budget reached while reading directory".into());
                            return usage;
                        }
                        match child { Ok(child) => todo.push(child.path()), Err(e) => usage.incomplete = Some(e.to_string()) }
                    },
                    Err(e) => usage.incomplete = Some(format!("{}: {e}", path.display())),
                }
            } else if meta.is_file() { usage.apparent = usage.apparent.saturating_add(meta.len()); usage.files += 1; }
        }
        usage
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn df_has_checked_units_and_spaces() {
            let v = parse_volume("Filesystem 1024-blocks Used Available Capacity Mounted on\nmy disk 1000 600 300 67% /my mount\n").unwrap();
            assert_eq!((v.total, v.used, v.available), (1024000, 614400, 307200));
            assert!(parse_volume("bad").is_err());
        }
        fn scratch(tag: &str) -> PathBuf {
            let path = std::env::temp_dir().join(format!("studio-{tag}-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
            fs::create_dir(&path).unwrap(); path
        }
        #[test]
        fn inventory_metadata_does_not_create_a_lockfile() {
            let root = scratch("frozen");
            fs::write(root.join("Cargo.toml"), "[package]\nname = \"studio-scan-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n[workspace]\n").unwrap();
            fs::write(root.join("lib.rs"), "").unwrap();
            let _ = inventory(&root, &AtomicBool::new(false), &mut || {});
            assert!(!root.join("Cargo.lock").exists(), "inspection must not create Cargo.lock");
            assert!(!root.join("target").exists(), "inspection must not build");
            fs::remove_dir_all(root).unwrap();
        }
        #[test]
        fn discovers_nested_targets_without_entering_their_build_trees() {
            let root = scratch("discover");
            fs::create_dir_all(root.join("libs/nested/target/generated/target")).unwrap();
            fs::create_dir_all(root.join("apps/other/target")).unwrap();
            let (targets, _, errors) = discover(&root, Instant::now()+Duration::from_secs(2), &AtomicBool::new(false), &mut || {});
            assert!(errors.is_empty());
            assert_eq!(targets.len(), 2);
            assert!(targets.contains(&root.join("libs/nested/target")));
            assert!(targets.contains(&root.join("apps/other/target")));
            fs::remove_dir_all(root).unwrap();
        }
        #[cfg(unix)]
        #[test]
        fn scan_excludes_targets_deduplicates_links_and_ignores_symlink_cycles() {
            let root = std::env::temp_dir().join(format!("studio-scan-test-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
            fs::create_dir(&root).unwrap();
            fs::write(root.join("file"), b"12345").unwrap();
            fs::hard_link(root.join("file"), root.join("link")).unwrap();
            std::os::unix::fs::symlink(&root, root.join("cycle")).unwrap();
            fs::create_dir(root.join("target")).unwrap();
            fs::write(root.join("target/build"), b"123456789").unwrap();
            let usage = walk(&root, &[root.join("target")], &mut HashSet::new(), Instant::now()+Duration::from_secs(1), &AtomicBool::new(false), &mut || {});
            assert_eq!(usage.apparent, 5);
            assert_eq!(usage.files, 1);
            assert!(usage.incomplete.is_none());
            let partial = walk(&root, &[], &mut HashSet::new(), Instant::now(), &AtomicBool::new(false), &mut || {});
            assert!(partial.incomplete.is_some());
            fs::remove_dir_all(root).unwrap();
        }
    }
}
