use makepad_studio_protocol::backend_protocol::{
    FileError, FileNode, FileNodeType, FileTreeData, GitCommitInfo, GitLog, GitStatus, SearchResult,
};
use makepad_git::{FileStatus as GitFileStatus, GitError, Repository as GitRepository};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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
    Io(std::io::Error),
    Git(String),
}

impl std::fmt::Display for VirtualFsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VirtualFsError::MissingMount(m) => write!(f, "missing mount: {}", m),
            VirtualFsError::InvalidVirtualPath(p) => write!(f, "invalid virtual path: {}", p),
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
            VirtualFsError::Io(err) => FileError::Io(err.to_string()),
            VirtualFsError::Git(err) => FileError::Git(err),
        }
    }
}

struct StatusContext {
    in_repo: bool,
    status_map: HashMap<String, GitStatus>,
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

pub(crate) fn protocol_search_results(paths: Vec<String>) -> Vec<SearchResult> {
    paths
        .into_iter()
        .map(|path| SearchResult {
            path,
            line: 0,
            column: 0,
            line_text: String::new(),
        })
        .collect()
}
