//! Observed process and source-file activity for Studio's shared workspace.
//!
//! A single lifetime worker owns all OS and filesystem inspection. Snapshots
//! are bounded and immutable; callers never wait for the worker on the UI.
//! Process exit status and file-change authorship are deliberately unknown.
use makepad_widgets::makepad_platform::thread::{SignalToUI, TaskHandle, ThreadOptions, ThreadSpawner};
use std::path::PathBuf;
use std::sync::{atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, SyncSender, TrySendError}, Arc};

const MAX_WATCHED_FILES: usize = 512;
const MAX_RECENT_FILES: usize = 64;
const MAX_RUNNING_PROCESSES: usize = 128;
const MAX_EXITED_PROCESSES: usize = 32;
const MAX_COMMAND_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState { Running, Exited }

#[derive(Clone, Debug)]
pub struct ProcessActivity {
    pub pid: u32,
    pub parent_pid: u32,
    pub command: String,
    pub state: ProcessState,
    /// Observation times, not the process's OS creation or termination time.
    pub first_seen: u64,
    pub last_seen: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileState { Changed, Removed }

#[derive(Clone, Debug)]
pub struct FileActivity {
    /// Absolute path inside the observed Git worktree.
    pub path: PathBuf,
    /// Monotonic within this worker. A newer revision of a path advances it.
    pub sequence: u64,
    pub changed_at: u64,
    pub state: FileState,
}

#[derive(Clone, Debug, Default)]
pub struct ActivitySnapshot {
    pub project: PathBuf,
    pub sampled_at: u64,
    pub processes: Vec<ProcessActivity>,
    /// Most recently observed changes first; startup dirty files are silent.
    pub changed_files: Vec<FileActivity>,
    pub watched_files: usize,
    pub coverage: String,
    pub errors: Vec<String>,
}

impl ActivitySnapshot {
    /// Includes only live terminal roots and their observed descendants. Studio
    /// also owns housekeeping processes; those are diagnostics, not agent work.
    pub fn terminal_descendants(
        &self,
        terminals: impl IntoIterator<Item = u32>,
    ) -> std::collections::HashSet<u32> {
        let mut work: std::collections::HashSet<_> = terminals.into_iter().collect();
        loop {
            let previous = work.len();
            for process in &self.processes {
                if work.contains(&process.parent_pid) {
                    work.insert(process.pid);
                }
            }
            if work.len() == previous {
                return work;
            }
        }
    }
}

#[cfg(test)]
mod graph_tests {
    use super::*;

    #[test]
    fn graph_keeps_terminal_jobs_and_excludes_housekeeping_trees() {
        // Deliberately reverse ancestry order and retain an exited job. A
        // closed terminal's orphaned history must not become an active lane.
        let process = |pid, parent_pid, state| ProcessActivity {
            pid, parent_pid, state, command: String::new(), first_seen: 0, last_seen: 0,
        };
        let snapshot = ActivitySnapshot {
            processes: vec![
                process(31, 30, ProcessState::Running),
                process(21, 20, ProcessState::Exited),
                process(20, 10, ProcessState::Running),
                process(10, 1, ProcessState::Running),
                process(30, 1, ProcessState::Running), // quota CLI
                process(40, 1, ProcessState::Running), // disk helper
                process(51, 50, ProcessState::Exited), // closed terminal
                process(60, 61, ProcessState::Running), // malformed cycle
                process(61, 60, ProcessState::Running),
            ],
            ..Default::default()
        };
        assert_eq!(snapshot.terminal_descendants([10]), [10, 20, 21].into_iter().collect());
        assert!(snapshot.terminal_descendants([]).is_empty());
    }
}

pub struct ActivityWorker {
    snapshots: Receiver<Arc<ActivitySnapshot>>,
    stop: Arc<AtomicBool>,
    task: TaskHandle<()>,
}

impl ActivityWorker {
    pub fn start(spawner: &ThreadSpawner, project: PathBuf, root_pid: u32) -> Result<Self, String> {
        let (tx, snapshots) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = stop.clone();
        let task = spawner.spawn_worker(ThreadOptions {
            name: Some("studio-activity".into()), ..Default::default()
        }, move || {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            native::run(project, root_pid, tx, cancel);
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                let _ = (root_pid, cancel);
                let snapshot = ActivitySnapshot {
                    project,
                    errors: vec!["Process and Git activity observation requires macOS or Linux".into()],
                    ..Default::default()
                };
                let _ = tx.try_send(Arc::new(snapshot));
                SignalToUI::set_ui_signal();
            }
        }).map_err(|error| error.to_string())?;
        Ok(Self { snapshots, stop, task })
    }

    pub fn poll(&mut self) -> Option<Arc<ActivitySnapshot>> {
        let mut latest = None;
        while let Ok(snapshot) = self.snapshots.try_recv() { latest = Some(snapshot); }
        latest
    }

    pub fn request_stop(&self) { self.stop.store(true, Ordering::Relaxed); }
    pub fn is_finished(&self) -> bool { self.task.is_finished() }
}

impl Drop for ActivityWorker {
    fn drop(&mut self) { self.request_stop(); }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod native {
    use super::*;
    use std::{collections::{BTreeMap, BTreeSet, HashSet}, fs, io::{self, Read},
        os::unix::{ffi::OsStringExt, fs::MetadataExt, io::AsRawFd},
        path::{Component, Path}, process::{Child, Command, Stdio},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

    const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
    const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
    // Both supported platforms expose these POSIX fcntl command values.
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    #[cfg(target_os = "macos")]
    const O_NONBLOCK: i32 = 0x4;
    #[cfg(target_os = "linux")]
    const O_NONBLOCK: i32 = 0o4000;
    unsafe extern "C" { fn fcntl(fd: i32, cmd: i32, ...) -> i32; }

    fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }

    // This guard owns only a helper command spawned by this worker. Killing
    // it cannot affect a user terminal, app, or another Studio instance.
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    /// Drain a nonblocking pipe while polling cancellation and a deadline.
    /// No temporary output files, helper reader threads, or unbounded output.
    fn command(program: &str, args: &[&str], cwd: &Path, stop: &AtomicBool) -> Result<(Vec<u8>, u32), String> {
        if stop.load(Ordering::Relaxed) { return Err("activity inspection stopped".into()); }
        let mut child = OwnedChild(Command::new(program).args(args).current_dir(cwd)
            .env("LC_ALL", "C").env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn().map_err(|error| format!("{program}: {error}"))?);
        let pid = child.0.id();
        let mut stdout = child.0.stdout.take().ok_or("helper stdout unavailable")?;
        let fd = stdout.as_raw_fd();
        // SAFETY: stdout owns this valid descriptor for this entire call. Only
        // its descriptor flags change; the worker is the sole reader.
        let flags = unsafe { fcntl(fd, F_GETFL) };
        if flags < 0 || unsafe { fcntl(fd, F_SETFL, flags | O_NONBLOCK) } < 0 {
            return Err(format!("{program}: nonblocking pipe: {}", io::Error::last_os_error()));
        }
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let mut output = Vec::new();
        let mut buffer = [0u8; 16 * 1024];
        let mut eof = false;
        loop {
            if stop.load(Ordering::Relaxed) { return Err("activity inspection stopped".into()); }
            if Instant::now() >= deadline { return Err(format!("{program} exceeded its 5 second activity budget")); }
            // Bound each drain too, so continuously writing helpers cannot
            // prevent deadline or cancellation checks.
            for _ in 0..16 {
                match stdout.read(&mut buffer) {
                    Ok(0) => { eof = true; break; },
                    Ok(count) => {
                        if output.len() + count > MAX_COMMAND_BYTES {
                            return Err(format!("{program} exceeded its 2 MiB activity output budget"));
                        }
                        output.extend_from_slice(&buffer[..count]);
                    },
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(format!("{program}: {error}")),
                }
            }
            if let Some(status) = child.0.try_wait().map_err(|error| error.to_string())? {
                if !status.success() { return Err(format!("{program} activity inspection exited {status}")); }
                if eof { return Ok((output, pid)); }
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    #[derive(Clone, Debug)]
    struct ProcessRow { pid: u32, parent_pid: u32, command: String }

    fn parse_processes(bytes: &[u8]) -> Vec<ProcessRow> {
        String::from_utf8_lossy(bytes).lines().filter_map(|line| {
            let line = line.trim_start();
            let split = line.find(char::is_whitespace)?;
            let pid = line[..split].parse().ok()?;
            let line = line[split..].trim_start();
            let split = line.find(char::is_whitespace)?;
            let parent_pid = line[..split].parse().ok()?;
            let line = line[split..].trim_start();
            let split = line.find(char::is_whitespace)?;
            let state = &line[..split];
            // Zombies have already exited. They disappear from running and
            // become retained Exited observations, without inventing a code.
            if state.starts_with('Z') { return None; }
            let command = line[split..].trim().chars().take(2048).collect();
            Some(ProcessRow { pid, parent_pid, command })
        }).collect()
    }

    fn inspection_helper(row: &ProcessRow, root_pid: u32, helper_pid: u32) -> bool {
        if row.pid == helper_pid { return true; }
        if row.parent_pid != root_pid { return false; }
        let executable = row.command.split_whitespace().next().unwrap_or("");
        // The shell's descendants (including user git/ps/df commands) remain
        // visible. These direct Studio children are inventory helpers.
        matches!(Path::new(executable).file_name().and_then(|s| s.to_str()), Some("git" | "ps" | "df"))
    }

    fn descendants(rows: &[ProcessRow], root_pid: u32, helper_pid: u32) -> Vec<ProcessRow> {
        let mut children: BTreeMap<u32, Vec<&ProcessRow>> = BTreeMap::new();
        for row in rows {
            if row.pid != root_pid && !inspection_helper(row, root_pid, helper_pid) {
                children.entry(row.parent_pid).or_default().push(row);
            }
        }
        let mut parents = vec![root_pid];
        let mut seen = HashSet::from([root_pid]);
        let mut result = Vec::new();
        while let Some(parent) = parents.pop() {
            if let Some(children) = children.get(&parent) {
                for child in children {
                    if seen.insert(child.pid) {
                        result.push((*child).clone());
                        parents.push(child.pid);
                    }
                }
            }
        }
        result.sort_by_key(|row| row.pid);
        result
    }

    fn update_processes(snapshot: &mut ActivitySnapshot, rows: Vec<ProcessRow>, at: u64) -> bool {
        let truncated = rows.len() > MAX_RUNNING_PROCESSES;
        let mut old: BTreeMap<_, _> = snapshot.processes.drain(..).map(|row| (row.pid, row)).collect();
        let observed: HashSet<_> = rows.iter().map(|row| row.pid).collect();
        let mut running = Vec::new();
        for row in rows.into_iter().take(MAX_RUNNING_PROCESSES) {
            let first_seen = old.remove(&row.pid).filter(|previous| previous.state == ProcessState::Running)
                .map(|previous| previous.first_seen).unwrap_or(at);
            running.push(ProcessActivity { pid: row.pid, parent_pid: row.parent_pid, command: row.command,
                state: ProcessState::Running, first_seen, last_seen: at });
        }
        let mut exited = Vec::new();
        for (_, mut row) in old {
            // Capacity overflow is not a process exit.
            if observed.contains(&row.pid) { continue; }
            if row.state == ProcessState::Running { row.state = ProcessState::Exited; row.last_seen = at; }
            exited.push(row);
        }
        exited.sort_by_key(|row| std::cmp::Reverse((row.last_seen, row.pid)));
        running.extend(exited.into_iter().take(MAX_EXITED_PROCESSES));
        snapshot.processes = running;
        truncated
    }

    fn source_path(path: &Path) -> bool {
        if path.is_absolute() { return false; }
        for component in path.components() {
            match component {
                Component::Normal(name) => {
                    let name = name.to_string_lossy();
                    if matches!(name.as_ref(), ".git" | "target" | "vendor" | "node_modules" | "local")
                        || name.starts_with("target-") || name.starts_with("target_") { return false; }
                },
                _ => return false,
            }
        }
        matches!(path.extension().and_then(|s| s.to_str()), Some(
            "rs" | "toml" | "md" | "splash" | "js" | "jsx" | "ts" | "tsx" | "json" | "html" | "css"
            | "scss" | "py" | "sh" | "zsh" | "bash" | "c" | "h" | "cpp" | "hpp" | "cc" | "m" | "mm"
            | "swift" | "go" | "java" | "kt" | "ron" | "yaml" | "yml" | "xml" | "wgsl" | "glsl" | "metal"
        )) || matches!(path.file_name().and_then(|s| s.to_str()), Some("Makefile" | "Dockerfile" | "CMakeLists.txt"))
    }

    fn status_paths(bytes: &[u8]) -> (Vec<PathBuf>, bool) {
        let mut paths = BTreeSet::new();
        let mut records = bytes.split(|byte| *byte == 0);
        let mut truncated = false;
        while let Some(record) = records.next() {
            if record.len() < 4 || record[2] != b' ' { continue; }
            let path = PathBuf::from(std::ffi::OsString::from_vec(record[3..].to_vec()));
            if source_path(&path) {
                if paths.len() < MAX_WATCHED_FILES { paths.insert(path); }
                else if !paths.contains(&path) { truncated = true; }
            }
            // Porcelain v1 -z emits destination then source for rename/copy.
            if record[..2].iter().any(|byte| matches!(byte, b'R' | b'C')) { records.next(); }
        }
        (paths.into_iter().collect(), truncated)
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct FileStamp { modified: Option<SystemTime>, len: u64, device: u64, inode: u64 }

    fn stamp(path: &Path, project: &Path) -> Result<Option<FileStamp>, String> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound || error.kind() == io::ErrorKind::NotADirectory => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if !metadata.is_file() { return Err("not a regular source file".into()); }
        // Never follow a directory symlink out of the selected worktree.
        let actual = fs::canonicalize(path).map_err(|error| error.to_string())?;
        if !actual.starts_with(project) { return Err("source path leaves the observed worktree".into()); }
        Ok(Some(FileStamp { modified: metadata.modified().ok(), len: metadata.len(), device: metadata.dev(), inode: metadata.ino() }))
    }

    fn source_error(snapshot: &mut ActivitySnapshot, path: &Path, error: String) {
        if snapshot.errors.len() < 8 {
            snapshot.errors.push(format!("Source {}: {error}", path.display()));
        }
    }

    #[derive(Default)]
    struct FileWatch {
        baseline: bool,
        watched: BTreeMap<PathBuf, Option<FileStamp>>,
        sequence: u64,
        truncated: bool,
    }

    impl FileWatch {
        fn sample(&mut self, snapshot: &mut ActivitySnapshot, candidates: Vec<PathBuf>, at: u64) {
            let mut newly_seen = HashSet::new();
            for relative in candidates {
                let path = snapshot.project.join(relative);
                if self.watched.contains_key(&path) { continue; }
                if self.watched.len() >= MAX_WATCHED_FILES { self.truncated = true; continue; }
                let initial = match stamp(&path, &snapshot.project) {
                    Ok(initial) => initial,
                    Err(error) => { source_error(snapshot, &path, error); continue; },
                };
                newly_seen.insert(path.clone());
                self.watched.insert(path, initial);
            }
            for (path, previous) in &mut self.watched {
                let current = match stamp(path, &snapshot.project) {
                    Ok(current) => current,
                    Err(error) => { source_error(snapshot, path, error); continue; },
                };
                if self.baseline && (current != *previous || newly_seen.contains(path)) {
                    self.sequence = self.sequence.saturating_add(1);
                    snapshot.changed_files.retain(|entry| entry.path != *path);
                    snapshot.changed_files.insert(0, FileActivity { path: path.clone(), sequence: self.sequence,
                        changed_at: at, state: if current.is_some() { FileState::Changed } else { FileState::Removed } });
                    snapshot.changed_files.truncate(MAX_RECENT_FILES);
                }
                *previous = current;
            }
            self.baseline = true;
            snapshot.watched_files = self.watched.len();
        }
    }

    pub(super) fn run(project: PathBuf, root_pid: u32, tx: SyncSender<Arc<ActivitySnapshot>>, stop: Arc<AtomicBool>) {
        let mut snapshot = ActivitySnapshot { project, ..Default::default() };
        let discovery = command("git", &["rev-parse", "--show-toplevel"], &snapshot.project, &stop);
        let discovery_error = match discovery {
            Ok((mut bytes, _)) => {
                if bytes.last() == Some(&b'\n') { bytes.pop(); }
                let root = PathBuf::from(std::ffi::OsString::from_vec(bytes));
                snapshot.project = fs::canonicalize(&root).unwrap_or(root);
                None
            },
            Err(error) => Some(format!("Source observation unavailable: {error}")),
        };
        let mut files = FileWatch::default();
        let mut next_sample = Instant::now();
        let mut pending: Option<Arc<ActivitySnapshot>> = None;
        while !stop.load(Ordering::Relaxed) {
            if Instant::now() >= next_sample {
                snapshot.sampled_at = now();
                snapshot.errors.clear();
                let mut process_truncated = false;
                match command("/bin/ps", &["-axo", "pid=,ppid=,stat=,command="], &snapshot.project, &stop) {
                    Ok((output, helper_pid)) => {
                        let rows = descendants(&parse_processes(&output), root_pid, helper_pid);
                        process_truncated = update_processes(&mut snapshot, rows, now());
                    },
                    Err(error) => snapshot.errors.push(format!("Processes: {error}; previous observations retained")),
                }
                if let Some(error) = &discovery_error { snapshot.errors.push(error.clone()); }
                else {
                    let args = ["-c", "core.fsmonitor=false", "-c", "core.untrackedCache=false", "status",
                        "--porcelain=v1", "-z", "--untracked-files=all", "--", ".",
                        ":(exclude,glob)**/target/**", ":(exclude,glob)**/target-*/**", ":(exclude,glob)**/target_*/**",
                        ":(exclude,glob)**/node_modules/**", ":(exclude,glob)**/vendor/**", ":(exclude,glob)**/local/**"];
                    match command("git", &args, &snapshot.project, &stop) {
                        Ok((output, _)) => {
                            let (paths, truncated) = status_paths(&output);
                            files.truncated |= truncated;
                            files.sample(&mut snapshot, paths, now());
                        },
                        Err(error) => snapshot.errors.push(format!("Source discovery: {error}")),
                    }
                }
                snapshot.coverage = format!(
                    "Observed Studio descendants every 2s (up to {MAX_RUNNING_PROCESSES} running + {MAX_EXITED_PROCESSES} recent exits). Exits have unknown result; brief processes can be missed. Git source metadata: {} / {MAX_WATCHED_FILES} paths, {} recent changes; startup changes, ignored files, unsaved buffers and actor attribution are not observed.{}{}",
                    snapshot.watched_files, snapshot.changed_files.len(),
                    if process_truncated { " Process capacity reached." } else { "" },
                    if files.truncated { " Source capacity reached; coverage is partial." } else { "" });
                pending = Some(Arc::new(snapshot.clone()));
                next_sample = Instant::now() + SAMPLE_INTERVAL;
            }
            if let Some(snapshot) = pending.take() {
                match tx.try_send(snapshot) {
                    Ok(()) => SignalToUI::set_ui_signal(),
                    Err(TrySendError::Full(snapshot)) => pending = Some(snapshot),
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn process_tree_excludes_inventory_and_other_instances_but_keeps_terminal_jobs() {
            let rows = parse_processes(b" 10 1 S Studio\n 11 10 S /bin/zsh\n 12 11 R git status\n 13 12 R test app\n 14 10 R /bin/ps -axo\n 15 10 R git status\n 16 99 R unrelated\n 17 10 Z exited\n 18 15 R git helper\n");
            let children = descendants(&rows, 10, 14);
            assert_eq!(children.iter().map(|row| row.pid).collect::<Vec<_>>(), vec![11, 12, 13]);
            assert_eq!(children[2].command, "test app");
        }

        #[test]
        fn process_removal_is_unknown_exit_and_transitions_only_once() {
            let mut snapshot = ActivitySnapshot::default();
            update_processes(&mut snapshot, vec![ProcessRow { pid: 2, parent_pid: 1, command: "cargo test".into() }], 10);
            update_processes(&mut snapshot, vec![], 20);
            assert_eq!(snapshot.processes[0].state, ProcessState::Exited);
            assert_eq!(snapshot.processes[0].last_seen, 20);
            update_processes(&mut snapshot, vec![], 30);
            assert_eq!(snapshot.processes[0].last_seen, 20);
        }

        #[test]
        fn porcelain_paths_handle_spaces_renames_and_skip_generated_trees() {
            let (paths, truncated) = status_paths(b" M apps/a file.rs\0R  apps/new.rs\0apps/old.rs\0?? local/plans/foo.md\0?? apps/x/target/build.rs\0?? apps/x/node_modules/a.js\0?? libs/p/src/lib.rs\0?? ../escape.rs\0?? /absolute.rs\0");
            assert!(!truncated);
            assert_eq!(paths, vec![PathBuf::from("apps/a file.rs"), PathBuf::from("apps/new.rs"), PathBuf::from("libs/p/src/lib.rs")]);
        }

        #[test]
        fn watched_paths_are_bounded_and_baseline_does_not_emit_changes() {
            let (paths, truncated) = status_paths((0..600).map(|i| format!(" M src/file_{i:04}.rs\0")).collect::<String>().as_bytes());
            assert!(truncated);
            assert_eq!(paths.len(), MAX_WATCHED_FILES);
            let mut snapshot = ActivitySnapshot { project: PathBuf::from("/nonexistent-studio-activity-test"), ..Default::default() };
            let mut files = FileWatch::default();
            files.sample(&mut snapshot, paths, 1);
            assert!(snapshot.changed_files.is_empty());
            files.sample(&mut snapshot, vec![PathBuf::from("another.rs")], 2);
            assert_eq!(files.watched.len(), MAX_WATCHED_FILES);
            assert!(files.truncated);
        }

        #[test]
        fn file_edits_atomic_replacement_and_removal_advance_actual_revisions() {
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let dir = std::env::temp_dir().join(format!("studio-activity-test-{}-{stamp}", std::process::id()));
            fs::create_dir(&dir).unwrap();
            struct Temp(PathBuf);
            impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
            let _temp = Temp(dir.clone());
            let root = fs::canonicalize(dir).unwrap();
            let path = root.join("source.rs");
            fs::write(&path, "initial dirty source").unwrap();
            let mut snapshot = ActivitySnapshot { project: root.clone(), ..Default::default() };
            let mut files = FileWatch::default();
            files.sample(&mut snapshot, vec![PathBuf::from("source.rs")], 1);
            assert!(snapshot.changed_files.is_empty());
            fs::write(&path, "a real external edit with a new length").unwrap();
            files.sample(&mut snapshot, vec![PathBuf::from("source.rs")], 2);
            assert_eq!(snapshot.changed_files.len(), 1);
            assert_eq!(snapshot.changed_files[0].state, FileState::Changed);
            let first = snapshot.changed_files[0].sequence;
            files.sample(&mut snapshot, vec![], 3);
            assert_eq!(snapshot.changed_files[0].sequence, first);
            fs::write(root.join("swap.rs"), "a real external edit with a new length").unwrap();
            fs::rename(root.join("swap.rs"), &path).unwrap();
            files.sample(&mut snapshot, vec![], 4);
            assert_eq!(snapshot.changed_files[0].sequence, first + 1);
            fs::remove_file(&path).unwrap();
            files.sample(&mut snapshot, vec![], 5);
            assert_eq!(snapshot.changed_files.len(), 1);
            assert_eq!(snapshot.changed_files[0].state, FileState::Removed);
            assert_eq!(snapshot.changed_files[0].sequence, first + 2);
        }
    }
}
