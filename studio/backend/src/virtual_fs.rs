use crate::worker_pool::WorkerPool;
use makepad_studio_protocol::backend_protocol::{
    FileError, FileNode, FileNodeType, FileTreeData, GitCommitInfo, GitLog, GitStatus, SearchResult,
};
use makepad_git::{FileStatus as GitFileStatus, GitError, Repository as GitRepository};
use makepad_rabin_karp::{search_with_limit as rabin_karp_search, RabinKarpResult};
use makepad_regex::{ParseOptions as RegexParseOptions, Regex};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

#[derive(Clone, Debug)]
pub struct MountPoint {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Default)]
pub struct VirtualFs {
    mounts: HashMap<String, MountPoint>,
    open_buffers: HashMap<String, String>,
}

#[derive(Debug)]
pub enum VirtualFsError {
    MissingMount(String),
    InvalidVirtualPath(String),
    Search(String),
    Io(std::io::Error),
    Git(String),
}

impl std::fmt::Display for VirtualFsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VirtualFsError::MissingMount(m) => write!(f, "missing mount: {}", m),
            VirtualFsError::InvalidVirtualPath(p) => write!(f, "invalid virtual path: {}", p),
            VirtualFsError::Search(err) => write!(f, "search error: {}", err),
            VirtualFsError::Io(err) => write!(f, "io error: {}", err),
            VirtualFsError::Git(err) => write!(f, "git error: {}", err),
        }
    }
}

impl std::error::Error for VirtualFsError {}

impl From<std::io::Error> for VirtualFsError {
    fn from(value: std::io::Error) -> Self {
        VirtualFsError::Io(value)
    }
}

impl From<VirtualFsError> for FileError {
    fn from(value: VirtualFsError) -> Self {
        match value {
            VirtualFsError::MissingMount(v) => FileError::InvalidPath(v),
            VirtualFsError::InvalidVirtualPath(v) => FileError::InvalidPath(v),
            VirtualFsError::Search(err) => FileError::Other(err),
            VirtualFsError::Io(err) => FileError::Io(err.to_string()),
            VirtualFsError::Git(err) => FileError::Git(err),
        }
    }
}

struct StatusContext {
    in_repo: bool,
    status_map: HashMap<String, GitStatus>,
}

struct SearchFileCandidate {
    index: usize,
    path: PathBuf,
    virtual_path: String,
}

impl VirtualFs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clone_for_search(&self) -> Self {
        Self {
            mounts: self.mounts.clone(),
            open_buffers: HashMap::new(),
        }
    }

    pub fn mount(&mut self, name: &str, path: impl Into<PathBuf>) -> Result<(), VirtualFsError> {
        if name.is_empty() {
            return Err(VirtualFsError::InvalidVirtualPath(
                "mount name cannot be empty".to_string(),
            ));
        }
        let mount_path = path.into().canonicalize()?;
        self.mounts.insert(
            name.to_string(),
            MountPoint {
                name: name.to_string(),
                path: mount_path,
            },
        );
        Ok(())
    }

    pub fn unmount(&mut self, name: &str) -> bool {
        self.mounts.remove(name).is_some()
    }

    pub fn mounts(&self) -> Vec<MountPoint> {
        let mut mounts: Vec<MountPoint> = self.mounts.values().cloned().collect();
        mounts.sort_by(|a, b| a.name.cmp(&b.name));
        mounts
    }

    pub fn resolve_mount(&self, mount: &str) -> Result<PathBuf, VirtualFsError> {
        let (mount_name, rest) = split_mount_and_rest(mount)?;
        let mount = self
            .mounts
            .get(mount_name)
            .ok_or_else(|| VirtualFsError::MissingMount(mount_name.to_string()))?;
        if rest.is_empty() {
            return Ok(mount.path.clone());
        }
        let branch = parse_branch_segment(rest)?;
        Ok(mount.path.join("branch").join(branch))
    }

    pub fn resolve_path(&self, path: &str) -> Result<PathBuf, VirtualFsError> {
        let (mount_name, rest) = split_mount_and_rest(path)?;
        if rest.is_empty() {
            return Err(VirtualFsError::InvalidVirtualPath(path.to_string()));
        }
        let mount = self
            .mounts
            .get(mount_name)
            .ok_or_else(|| VirtualFsError::MissingMount(mount_name.to_string()))?;

        if let Some((head, tail)) = split_head_tail(rest) {
            if head.starts_with('@') {
                let branch = parse_branch_segment(head)?;
                if tail.is_empty() {
                    return Ok(mount.path.join("branch").join(branch));
                }
                return Ok(mount.path.join("branch").join(branch).join(tail));
            }
        }
        Ok(mount.path.join(rest))
    }

    pub fn read_text_file(&mut self, path: &str) -> Result<String, VirtualFsError> {
        let disk_path = self.resolve_path(path)?;
        let data = fs::read_to_string(disk_path)?;
        self.open_buffers.insert(path.to_string(), data.clone());
        Ok(data)
    }

    pub fn read_text_range(
        &mut self,
        path: &str,
        start_line: usize,
        end_line: usize,
    ) -> Result<(String, usize), VirtualFsError> {
        if start_line == 0 || end_line == 0 || end_line < start_line {
            return Err(VirtualFsError::InvalidVirtualPath(format!(
                "invalid line range {}..{}",
                start_line, end_line
            )));
        }

        let content = self.read_text_file(path)?;
        let total_lines = content.lines().count();
        if total_lines == 0 {
            return Ok((String::new(), 0));
        }

        let mut out = String::new();
        for (index, line) in content.lines().enumerate() {
            let line_no = index + 1;
            if line_no < start_line {
                continue;
            }
            if line_no > end_line {
                break;
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
        Ok((out, total_lines))
    }

    pub fn open_text_file(&mut self, path: &str) -> Result<String, VirtualFsError> {
        let content = self.read_text_file(path)?;
        self.open_buffers.insert(path.to_string(), content.clone());
        Ok(content)
    }

    pub fn save_text_file(&mut self, path: &str, content: &str) -> Result<(), VirtualFsError> {
        let disk_path = self.resolve_path(path)?;
        if let Some(parent) = disk_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&disk_path, content.as_bytes())?;
        self.open_buffers
            .insert(path.to_string(), content.to_string());
        Ok(())
    }

    pub fn delete_path(&mut self, path: &str) -> Result<(), VirtualFsError> {
        let disk_path = self.resolve_path(path)?;
        if disk_path.is_dir() {
            fs::remove_dir_all(&disk_path)?;
        } else if disk_path.exists() {
            fs::remove_file(&disk_path)?;
        }
        self.open_buffers.remove(path);
        Ok(())
    }

    pub fn create_branch(
        &self,
        mount: &str,
        name: &str,
        from_ref: Option<&str>,
    ) -> Result<(), VirtualFsError> {
        if name.is_empty() {
            return Err(VirtualFsError::InvalidVirtualPath(
                "branch name cannot be empty".to_string(),
            ));
        }
        let (mount_name, rest) = split_mount_and_rest(mount)?;
        if !rest.is_empty() {
            return Err(VirtualFsError::InvalidVirtualPath(format!(
                "CreateBranch mount must be a mount name, got {}",
                mount
            )));
        }
        let mount = self
            .mounts
            .get(mount_name)
            .ok_or_else(|| VirtualFsError::MissingMount(mount_name.to_string()))?;
        let repo =
            GitRepository::open(&mount.path).map_err(|e| VirtualFsError::Git(e.to_string()))?;

        let base_oid = if let Some(from_ref) = from_ref {
            match repo.resolve_ref(from_ref) {
                Ok(oid) => oid,
                Err(_) => {
                    let as_branch = format!("refs/heads/{}", from_ref);
                    repo.resolve_ref(&as_branch)
                        .map_err(|e| VirtualFsError::Git(e.to_string()))?
                }
            }
        } else {
            repo.head_oid()
                .map_err(|e| VirtualFsError::Git(e.to_string()))?
        };

        repo.create_branch(name, &base_oid)
            .map_err(|e| VirtualFsError::Git(e.to_string()))?;

        let branch_dir = mount.path.join("branch").join(name);
        if branch_dir.exists() {
            return Ok(());
        }
        if let Some(parent) = branch_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        makepad_git::local_clone_depth1(&mount.path, &branch_dir, Some(name))
            .map_err(|e| VirtualFsError::Git(e.to_string()))?;
        Ok(())
    }

    pub fn delete_branch(&self, mount: &str, name: &str) -> Result<(), VirtualFsError> {
        let (mount_name, rest) = split_mount_and_rest(mount)?;
        if !rest.is_empty() {
            return Err(VirtualFsError::InvalidVirtualPath(format!(
                "DeleteBranch mount must be a mount name, got {}",
                mount
            )));
        }
        let mount = self
            .mounts
            .get(mount_name)
            .ok_or_else(|| VirtualFsError::MissingMount(mount_name.to_string()))?;
        let repo =
            GitRepository::open(&mount.path).map_err(|e| VirtualFsError::Git(e.to_string()))?;
        match repo.delete_branch(name) {
            Ok(()) => {}
            Err(GitError::RefNotFound(_)) => {}
            Err(err) => return Err(VirtualFsError::Git(err.to_string())),
        }
        let branch_dir = mount.path.join("branch").join(name);
        if branch_dir.exists() {
            fs::remove_dir_all(branch_dir)?;
        }
        Ok(())
    }

    pub fn git_log(&self, mount: &str, max_count: usize) -> Result<GitLog, VirtualFsError> {
        let real_mount = self.resolve_mount(mount)?;
        let mut repo =
            GitRepository::open(&real_mount).map_err(|e| VirtualFsError::Git(e.to_string()))?;
        let head = repo
            .head_oid()
            .map_err(|e| VirtualFsError::Git(e.to_string()))?;
        let log_entries = repo
            .log(&head, max_count)
            .map_err(|e| VirtualFsError::Git(e.to_string()))?;
        let commits = log_entries
            .into_iter()
            .map(|(oid, commit)| GitCommitInfo {
                hash: oid.to_hex(),
                message: commit.message.trim_end().to_string(),
                author: commit.author.name,
                timestamp: commit.author.timestamp,
            })
            .collect();
        Ok(GitLog { commits })
    }

    pub fn load_file_tree(&self, mount: &str) -> Result<FileTreeData, VirtualFsError> {
        let (mount_name, rest) = split_mount_and_rest(mount)?;
        let mount = self
            .mounts
            .get(mount_name)
            .ok_or_else(|| VirtualFsError::MissingMount(mount_name.to_string()))?;

        let mut nodes = Vec::new();
        if rest.is_empty() {
            let main_ctx = self.load_status_context(&mount.path);
            nodes.push(FileNode {
                path: mount_name.to_string(),
                name: mount_name.to_string(),
                node_type: FileNodeType::Dir,
                git_status: aggregate_root_git_status(&main_ctx),
            });
            self.walk_dir(
                &mount.path,
                &mount.path,
                mount_name,
                &main_ctx,
                &mut nodes,
                true,
            )?;

            for (branch_name, branch_root) in scan_branch_roots(&mount.path)? {
                let encoded = percent_encode(&branch_name);
                let branch_virtual_root = format!("{}/@{}", mount_name, encoded);
                let ctx = self.load_status_context(&branch_root);
                nodes.push(FileNode {
                    path: branch_virtual_root.clone(),
                    name: format!("@{}", encoded),
                    node_type: FileNodeType::Dir,
                    git_status: aggregate_root_git_status(&ctx),
                });
                self.walk_dir(
                    &branch_root,
                    &branch_root,
                    &branch_virtual_root,
                    &ctx,
                    &mut nodes,
                    false,
                )?;
            }
        } else {
            let branch = parse_branch_segment(rest)?;
            let branch_root = mount.path.join("branch").join(&branch);
            let virtual_root = format!("{}/@{}", mount_name, percent_encode(&branch));
            let ctx = self.load_status_context(&branch_root);
            nodes.push(FileNode {
                path: virtual_root.clone(),
                name: format!("@{}", percent_encode(&branch)),
                node_type: FileNodeType::Dir,
                git_status: aggregate_root_git_status(&ctx),
            });
            self.walk_dir(
                &branch_root,
                &branch_root,
                &virtual_root,
                &ctx,
                &mut nodes,
                false,
            )?;
        }
        Ok(FileTreeData { nodes })
    }

    pub fn find_files(
        &self,
        mount: Option<&str>,
        pattern: &str,
        max_results: Option<usize>,
    ) -> Result<Vec<String>, VirtualFsError> {
        let max_results = max_results.unwrap_or(10_000);
        let mut out = Vec::new();

        if let Some(mount) = mount {
            let real_mount = self.resolve_mount(mount)?;
            self.walk_files_for_search(&real_mount, mount, pattern, &mut out, max_results)?;
            return Ok(out);
        }

        let mut mounts: Vec<_> = self.mounts.values().collect();
        mounts.sort_by(|a, b| a.name.cmp(&b.name));
        for mount in mounts {
            self.walk_files_for_search(&mount.path, &mount.name, pattern, &mut out, max_results)?;
            for (branch_name, branch_root) in scan_branch_roots(&mount.path)? {
                let virtual_root = format!("{}/@{}", mount.name, percent_encode(&branch_name));
                self.walk_files_for_search(
                    &branch_root,
                    &virtual_root,
                    pattern,
                    &mut out,
                    max_results,
                )?;
                if out.len() >= max_results {
                    return Ok(out);
                }
            }
            if out.len() >= max_results {
                return Ok(out);
            }
        }
        Ok(out)
    }

    pub fn find_in_files(
        &self,
        mount: Option<&str>,
        pattern: &str,
        is_regex: bool,
        glob: Option<&str>,
        max_results: Option<usize>,
        regex_search_pool: Option<&WorkerPool>,
    ) -> Result<Vec<SearchResult>, VirtualFsError> {
        if pattern.is_empty() {
            return Ok(Vec::new());
        }

        let max_results = max_results.unwrap_or(10_000);
        if is_regex {
            if let Some(pool) = regex_search_pool {
                let options = RegexParseOptions {
                    multiline: true,
                    dot_all: true,
                    ..RegexParseOptions::default()
                };
                // Validate once up front so invalid regexes still return a query error.
                Regex::new_with_options(pattern, options)
                    .map_err(|err| VirtualFsError::Search(err.to_string()))?;
                let files = self.collect_files_for_find_in_files(mount, glob)?;
                return Ok(self.search_files_with_regex_pool(files, pattern, max_results, pool));
            }
        }

        let mut out = Vec::new();
        let matcher = if is_regex {
            let options = RegexParseOptions {
                multiline: true,
                dot_all: true,
                ..RegexParseOptions::default()
            };
            let regex = Regex::new_with_options(pattern, options)
                .map_err(|err| VirtualFsError::Search(err.to_string()))?;
            FindInFilesMatcher::Regex(regex)
        } else {
            FindInFilesMatcher::Literal(pattern.as_bytes().to_vec())
        };

        if let Some(mount) = mount {
            let real_mount = self.resolve_mount(mount)?;
            self.walk_files_for_content_search(
                &real_mount,
                mount,
                &matcher,
                glob,
                &mut out,
                max_results,
            )?;
            return Ok(out);
        }

        let mut mounts: Vec<_> = self.mounts.values().collect();
        mounts.sort_by(|a, b| a.name.cmp(&b.name));
        for mount in mounts {
            self.walk_files_for_content_search(
                &mount.path,
                &mount.name,
                &matcher,
                glob,
                &mut out,
                max_results,
            )?;
            for (branch_name, branch_root) in scan_branch_roots(&mount.path)? {
                let virtual_root = format!("{}/@{}", mount.name, percent_encode(&branch_name));
                self.walk_files_for_content_search(
                    &branch_root,
                    &virtual_root,
                    &matcher,
                    glob,
                    &mut out,
                    max_results,
                )?;
                if out.len() >= max_results {
                    return Ok(out);
                }
            }
            if out.len() >= max_results {
                return Ok(out);
            }
        }
        Ok(out)
    }

    fn collect_files_for_find_in_files(
        &self,
        mount: Option<&str>,
        glob: Option<&str>,
    ) -> Result<Vec<SearchFileCandidate>, VirtualFsError> {
        let mut out = Vec::new();
        let mut next_index = 0usize;

        if let Some(mount) = mount {
            let real_mount = self.resolve_mount(mount)?;
            self.walk_files_for_content_search_candidates(
                &real_mount,
                mount,
                glob,
                &mut out,
                &mut next_index,
            )?;
            return Ok(out);
        }

        let mut mounts: Vec<_> = self.mounts.values().collect();
        mounts.sort_by(|a, b| a.name.cmp(&b.name));
        for mount in mounts {
            self.walk_files_for_content_search_candidates(
                &mount.path,
                &mount.name,
                glob,
                &mut out,
                &mut next_index,
            )?;
            for (branch_name, branch_root) in scan_branch_roots(&mount.path)? {
                let virtual_root = format!("{}/@{}", mount.name, percent_encode(&branch_name));
                self.walk_files_for_content_search_candidates(
                    &branch_root,
                    &virtual_root,
                    glob,
                    &mut out,
                    &mut next_index,
                )?;
            }
        }
        Ok(out)
    }

    fn walk_files_for_search(
        &self,
        real_root: &Path,
        virtual_root: &str,
        pattern: &str,
        out: &mut Vec<String>,
        max_results: usize,
    ) -> Result<(), VirtualFsError> {
        if !real_root.exists() {
            return Ok(());
        }
        let mut stack = vec![real_root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name == ".git" || name == "target" {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !path.is_file() {
                    continue;
                }
                let rel = slash_rel(real_root, &path);
                let virtual_path = format!("{}/{}", virtual_root, rel);
                if virtual_path.contains(pattern) {
                    out.push(virtual_path.clone());
                    if out.len() >= max_results {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    fn walk_files_for_content_search(
        &self,
        real_root: &Path,
        virtual_root: &str,
        matcher: &FindInFilesMatcher,
        glob: Option<&str>,
        out: &mut Vec<SearchResult>,
        max_results: usize,
    ) -> Result<(), VirtualFsError> {
        if !real_root.exists() {
            return Ok(());
        }
        let mut stack = vec![real_root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name == ".git" || name == "target" {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !path.is_file() {
                    continue;
                }
                let rel = slash_rel(real_root, &path);
                let virtual_path = format!("{}/{}", virtual_root, rel);
                if !should_search_virtual_path(&path, &virtual_path, glob) {
                    continue;
                }
                let file_content = match fs::read_to_string(&path) {
                    Ok(content) => content,
                    Err(_) => continue,
                };
                if search_file_content(&virtual_path, &file_content, matcher, out, max_results) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn walk_files_for_content_search_candidates(
        &self,
        real_root: &Path,
        virtual_root: &str,
        glob: Option<&str>,
        out: &mut Vec<SearchFileCandidate>,
        next_index: &mut usize,
    ) -> Result<(), VirtualFsError> {
        if !real_root.exists() {
            return Ok(());
        }
        let mut stack = vec![real_root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name == ".git" || name == "target" {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !path.is_file() {
                    continue;
                }
                let rel = slash_rel(real_root, &path);
                let virtual_path = format!("{}/{}", virtual_root, rel);
                if !should_search_virtual_path(&path, &virtual_path, glob) {
                    continue;
                }
                out.push(SearchFileCandidate {
                    index: *next_index,
                    path,
                    virtual_path,
                });
                *next_index += 1;
            }
        }
        Ok(())
    }

    fn search_files_with_regex_pool(
        &self,
        files: Vec<SearchFileCandidate>,
        regex_pattern: &str,
        max_results: usize,
        pool: &WorkerPool,
    ) -> Vec<SearchResult> {
        if max_results == 0 || files.is_empty() {
            return Vec::new();
        }

        let total_files = files.len();
        let mut file_results: Vec<Option<Vec<SearchResult>>> = vec![None; total_files];
        let (result_tx, result_rx) = mpsc::channel::<(usize, Vec<SearchResult>)>();
        let remaining = Arc::new(AtomicUsize::new(max_results));
        let in_flight_cap = pool.worker_count().saturating_mul(2).max(1);

        let mut pending = files.into_iter();
        let mut in_flight = 0usize;

        while in_flight < in_flight_cap && remaining.load(Ordering::Acquire) > 0 {
            let Some(file) = pending.next() else {
                break;
            };
            dispatch_regex_search_job(pool, file, regex_pattern, &remaining, &result_tx);
            in_flight += 1;
        }

        while in_flight > 0 {
            let Ok((index, results)) = result_rx.recv() else {
                break;
            };
            in_flight = in_flight.saturating_sub(1);
            if index < file_results.len() {
                file_results[index] = Some(results);
            }

            while in_flight < in_flight_cap && remaining.load(Ordering::Acquire) > 0 {
                let Some(file) = pending.next() else {
                    break;
                };
                dispatch_regex_search_job(pool, file, regex_pattern, &remaining, &result_tx);
                in_flight += 1;
            }
        }

        let mut out = Vec::new();
        for results in file_results.into_iter().flatten() {
            out.extend(results);
            if out.len() >= max_results {
                out.truncate(max_results);
                break;
            }
        }
        out
    }

    fn load_status_context(&self, real_root: &Path) -> StatusContext {
        let mut ctx = StatusContext {
            in_repo: false,
            status_map: HashMap::new(),
        };
        let Ok(mut repo) = GitRepository::open(real_root) else {
            return ctx;
        };
        ctx.in_repo = true;
        let Ok(status) = repo.status_for_file_tree() else {
            return ctx;
        };
        for entry in status.entries {
            ctx.status_map
                .entry(entry.path)
                .or_insert_with(|| git_status_from_file_status(entry.status));
        }
        ctx
    }

    fn walk_dir(
        &self,
        real_root: &Path,
        status_root: &Path,
        virtual_prefix: &str,
        status_ctx: &StatusContext,
        out: &mut Vec<FileNode>,
        skip_branch_dir: bool,
    ) -> Result<(), VirtualFsError> {
        // Store plain paths, not DirEntry handles, so parent directory fds are
        // released before descending recursively.
        let read_dir = fs::read_dir(real_root)?;
        let mut entries: Vec<(String, PathBuf)> = read_dir
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == ".git" {
                    return None;
                }
                if skip_branch_dir && name == "branch" {
                    return None;
                }
                if name == "target" {
                    return None;
                }
                Some((name, entry.path()))
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        for (name, path) in entries {
            let virtual_path = format!("{}/{}", virtual_prefix, name);
            let file_type = match fs::symlink_metadata(&path) {
                Ok(meta) => meta.file_type(),
                Err(_) => continue,
            };
            if file_type.is_dir() {
                out.push(FileNode {
                    path: virtual_path.clone(),
                    name: name.clone(),
                    node_type: FileNodeType::Dir,
                    git_status: GitStatus::Unknown,
                });
                self.walk_dir(&path, status_root, &virtual_path, status_ctx, out, false)?;
            } else if file_type.is_file() || file_type.is_symlink() {
                let rel = slash_rel(status_root, &path);
                let git_status = if let Some(status) = status_ctx.status_map.get(&rel) {
                    *status
                } else if status_ctx.in_repo {
                    GitStatus::Clean
                } else {
                    GitStatus::Unknown
                };
                out.push(FileNode {
                    path: virtual_path,
                    name,
                    node_type: FileNodeType::File,
                    git_status,
                });
            }
        }
        Ok(())
    }
}

enum FindInFilesMatcher {
    Literal(Vec<u8>),
    Regex(Regex),
}

fn dispatch_regex_search_job(
    pool: &WorkerPool,
    file: SearchFileCandidate,
    regex_pattern: &str,
    remaining: &Arc<AtomicUsize>,
    result_tx: &mpsc::Sender<(usize, Vec<SearchResult>)>,
) {
    let regex_pattern = regex_pattern.to_string();
    let remaining = Arc::clone(remaining);
    let result_tx = result_tx.clone();
    pool.execute(move || {
        let results = search_regex_file_with_budget(
            &file.path,
            &file.virtual_path,
            &regex_pattern,
            &remaining,
        );
        let _ = result_tx.send((file.index, results));
    });
}

fn search_regex_file_with_budget(
    path: &Path,
    virtual_path: &str,
    regex_pattern: &str,
    remaining: &AtomicUsize,
) -> Vec<SearchResult> {
    if remaining.load(Ordering::Acquire) == 0 {
        return Vec::new();
    }
    let options = RegexParseOptions {
        multiline: true,
        dot_all: true,
        ..RegexParseOptions::default()
    };
    let Ok(regex) = Regex::new_with_options(regex_pattern, options) else {
        return Vec::new();
    };
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    search_regex_content_with_budget(virtual_path, &content, &regex, remaining)
}

fn search_regex_content_with_budget(
    virtual_path: &str,
    content: &str,
    regex: &Regex,
    remaining: &AtomicUsize,
) -> Vec<SearchResult> {
    let mut out = Vec::new();
    let line_starts = line_starts(content.as_bytes());
    let mut slots = vec![None; 2];
    let mut search_from = 0usize;
    while search_from <= content.len() {
        slots[0] = None;
        slots[1] = None;
        let haystack = &content[search_from..];
        if !regex.run(haystack, &mut slots) {
            break;
        }

        let start_rel = slots[0].unwrap_or(0);
        let end_rel = slots[1].unwrap_or(0);
        let match_start = search_from + start_rel;
        let match_end = search_from + end_rel;

        if !try_claim_search_result_slot(remaining) {
            break;
        }
        out.push(search_result_for_byte_offset(
            content,
            &line_starts,
            virtual_path,
            match_start,
        ));

        if match_end == search_from {
            search_from = next_char_boundary(content, match_end);
        } else {
            search_from = match_end;
        }
    }
    out
}

fn try_claim_search_result_slot(remaining: &AtomicUsize) -> bool {
    let mut current = remaining.load(Ordering::Acquire);
    while current > 0 {
        match remaining.compare_exchange_weak(
            current,
            current - 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(next) => current = next,
        }
    }
    false
}

fn should_search_virtual_path(path: &Path, virtual_path: &str, glob: Option<&str>) -> bool {
    if let Some(glob) = glob {
        let glob = glob.trim();
        if glob.is_empty() {
            return true;
        }
        return glob
            .split(',')
            .map(str::trim)
            .filter(|pat| !pat.is_empty())
            .any(|pat| wildcard_match(pat, virtual_path));
    }

    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    ext.eq_ignore_ascii_case("rs")
        || ext.eq_ignore_ascii_case("md")
        || ext.eq_ignore_ascii_case("toml")
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();

    let (mut pat_i, mut text_i) = (0usize, 0usize);
    let mut last_star_pat_i: Option<usize> = None;
    let mut last_star_text_i = 0usize;

    while text_i < text.len() {
        if pat_i < pattern.len() && (pattern[pat_i] == b'?' || pattern[pat_i] == text[text_i]) {
            pat_i += 1;
            text_i += 1;
            continue;
        }

        if pat_i < pattern.len() && pattern[pat_i] == b'*' {
            last_star_pat_i = Some(pat_i);
            pat_i += 1;
            last_star_text_i = text_i;
            continue;
        }

        if let Some(star_pat_i) = last_star_pat_i {
            pat_i = star_pat_i + 1;
            last_star_text_i += 1;
            text_i = last_star_text_i;
            continue;
        }

        return false;
    }

    while pat_i < pattern.len() && pattern[pat_i] == b'*' {
        pat_i += 1;
    }
    pat_i == pattern.len()
}

fn search_file_content(
    virtual_path: &str,
    content: &str,
    matcher: &FindInFilesMatcher,
    out: &mut Vec<SearchResult>,
    max_results: usize,
) -> bool {
    match matcher {
        FindInFilesMatcher::Literal(needle) => {
            let mut matches = Vec::<RabinKarpResult>::new();
            let remaining = max_results.saturating_sub(out.len());
            if remaining == 0 {
                return true;
            }
            rabin_karp_search(content.as_bytes(), needle.as_slice(), &mut matches, remaining);
            for matched in matches {
                let line = matched.line + 1;
                let column = matched.column_byte + 1;
                let raw_line = line_text_for_line_start(content, matched.new_line_byte);
                let line_text = compact_line_text(&raw_line, matched.column_byte);
                out.push(SearchResult {
                    path: virtual_path.to_string(),
                    line,
                    column,
                    line_text,
                });
                if out.len() >= max_results {
                    return true;
                }
            }
            false
        }
        FindInFilesMatcher::Regex(regex) => {
            let line_starts = line_starts(content.as_bytes());
            let mut slots = vec![None; 2];
            let mut search_from = 0usize;
            while search_from <= content.len() {
                slots[0] = None;
                slots[1] = None;
                let haystack = &content[search_from..];
                if !regex.run(haystack, &mut slots) {
                    break;
                }

                let start_rel = slots[0].unwrap_or(0);
                let end_rel = slots[1].unwrap_or(0);
                let match_start = search_from + start_rel;
                let match_end = search_from + end_rel;

                let result = search_result_for_byte_offset(content, &line_starts, virtual_path, match_start);
                out.push(result);
                if out.len() >= max_results {
                    return true;
                }

                if match_end == search_from {
                    search_from = next_char_boundary(content, match_end);
                } else {
                    search_from = match_end;
                }
            }
            false
        }
    }
}

fn line_starts(bytes: &[u8]) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' && index + 1 < bytes.len() {
            starts.push(index + 1);
        }
    }
    starts
}

fn search_result_for_byte_offset(
    content: &str,
    line_starts: &[usize],
    virtual_path: &str,
    byte_offset: usize,
) -> SearchResult {
    let line_index = match line_starts.binary_search(&byte_offset) {
        Ok(index) => index,
        Err(index) => index.saturating_sub(1),
    };
    let line_start = line_starts.get(line_index).copied().unwrap_or(0);
    let column_zero = byte_offset.saturating_sub(line_start);
    let raw_line = line_text_for_line_start(content, line_start);
    let line_text = compact_line_text(&raw_line, column_zero);
    SearchResult {
        path: virtual_path.to_string(),
        line: line_index + 1,
        column: column_zero + 1,
        line_text,
    }
}

fn line_text_for_line_start(content: &str, line_start: usize) -> String {
    let bytes = content.as_bytes();
    if line_start >= bytes.len() {
        return String::new();
    }
    let mut line_end = bytes.len();
    for (offset, byte) in bytes[line_start..].iter().enumerate() {
        if *byte == b'\n' {
            line_end = line_start + offset;
            break;
        }
    }
    String::from_utf8_lossy(&bytes[line_start..line_end]).to_string()
}

fn compact_line_text(line: &str, match_byte_offset: usize) -> String {
    const MAX_LINE_TEXT_BYTES: usize = 220;
    if line.len() <= MAX_LINE_TEXT_BYTES {
        return line.to_string();
    }

    let mut start = match_byte_offset.saturating_sub(MAX_LINE_TEXT_BYTES / 2);
    if start + MAX_LINE_TEXT_BYTES > line.len() {
        start = line.len().saturating_sub(MAX_LINE_TEXT_BYTES);
    }
    let mut end = (start + MAX_LINE_TEXT_BYTES).min(line.len());

    while start < line.len() && !line.is_char_boundary(start) {
        start += 1;
    }
    while end > start && !line.is_char_boundary(end) {
        end -= 1;
    }

    let mut out = String::new();
    if start > 0 {
        out.push_str("...");
    }
    out.push_str(&line[start..end]);
    if end < line.len() {
        out.push_str("...");
    }
    out
}

fn next_char_boundary(text: &str, pos: usize) -> usize {
    let mut p = pos.saturating_add(1);
    while p < text.len() && !text.is_char_boundary(p) {
        p += 1;
    }
    p
}

fn split_mount_and_rest(input: &str) -> Result<(&str, &str), VirtualFsError> {
    if input.is_empty() {
        return Err(VirtualFsError::InvalidVirtualPath(
            "empty virtual path".to_string(),
        ));
    }
    if let Some((mount, rest)) = input.split_once('/') {
        if mount.is_empty() {
            return Err(VirtualFsError::InvalidVirtualPath(input.to_string()));
        }
        Ok((mount, rest))
    } else {
        Ok((input, ""))
    }
}

fn split_head_tail(input: &str) -> Option<(&str, &str)> {
    if let Some((head, tail)) = input.split_once('/') {
        Some((head, tail))
    } else if !input.is_empty() {
        Some((input, ""))
    } else {
        None
    }
}

fn parse_branch_segment(segment: &str) -> Result<String, VirtualFsError> {
    let encoded = segment.strip_prefix('@').ok_or_else(|| {
        VirtualFsError::InvalidVirtualPath(format!("expected @branch segment, got {}", segment))
    })?;
    percent_decode(encoded)
}

fn scan_branch_roots(mount_root: &Path) -> Result<Vec<(String, PathBuf)>, VirtualFsError> {
    let branch_root = mount_root.join("branch");
    if !branch_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(branch_root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        out.push((name, path));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn git_status_from_file_status(status: GitFileStatus) -> GitStatus {
    match status {
        GitFileStatus::Modified => GitStatus::Modified,
        GitFileStatus::Deleted => GitStatus::Deleted,
        GitFileStatus::Untracked => GitStatus::Untracked,
        GitFileStatus::Staged => GitStatus::Staged,
        GitFileStatus::StagedDeleted => GitStatus::Deleted,
        GitFileStatus::StagedNew => GitStatus::Added,
    }
}

fn aggregate_root_git_status(ctx: &StatusContext) -> GitStatus {
    if !ctx.in_repo {
        return GitStatus::Unknown;
    }

    let mut has_modified = false;
    let mut has_deleted = false;
    let mut has_untracked = false;
    let mut has_added = false;
    let mut has_staged = false;
    let mut has_conflict = false;

    for status in ctx.status_map.values() {
        match status {
            GitStatus::Conflict => has_conflict = true,
            GitStatus::Deleted => has_deleted = true,
            GitStatus::Modified => has_modified = true,
            GitStatus::Staged => has_staged = true,
            GitStatus::Added => has_added = true,
            GitStatus::Untracked => has_untracked = true,
            GitStatus::Clean | GitStatus::Ignored | GitStatus::Unknown => {}
        }
    }

    if has_conflict {
        GitStatus::Conflict
    } else if has_deleted {
        GitStatus::Deleted
    } else if has_modified {
        GitStatus::Modified
    } else if has_staged {
        GitStatus::Staged
    } else if has_added {
        GitStatus::Added
    } else if has_untracked {
        GitStatus::Untracked
    } else {
        GitStatus::Clean
    }
}

fn slash_rel(root: &Path, path: &Path) -> String {
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();
    rel.replace('\\', "/")
}

fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for b in input.bytes() {
        let safe = b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.';
        if safe {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex((b >> 4) & 0x0F));
            out.push(hex(b & 0x0F));
        }
    }
    out
}

fn percent_decode(input: &str) -> Result<String, VirtualFsError> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(VirtualFsError::InvalidVirtualPath(input.to_string()));
            }
            let hi = from_hex(bytes[i + 1])
                .ok_or_else(|| VirtualFsError::InvalidVirtualPath(input.to_string()))?;
            let lo = from_hex(bytes[i + 2])
                .ok_or_else(|| VirtualFsError::InvalidVirtualPath(input.to_string()))?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| VirtualFsError::InvalidVirtualPath(input.to_string()))
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(10 + (b - b'a')),
        b'A'..=b'F' => Some(10 + (b - b'A')),
        _ => None,
    }
}

fn hex(v: u8) -> char {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    HEX[v as usize] as char
}
