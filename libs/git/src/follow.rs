//! Fetch a checkout's configured upstream and prepare its changed files under the index lock.
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::http_sync;
use crate::index::{read_index_for_update, IndexLock};
use crate::worktree::checked_worktree_path;
use crate::{
    config, reach, refs, worktree, GitError, GitHttpRequest, GitHttpResponse, HttpSyncHooks, Index,
    ObjectId, ObjectKind, Repository, StatusOptions, TreeChange,
};

pub struct Upstream {
    pub branch: String,
    pub remote: String,
    pub remote_branch: String,
    /// The configured remote URL: the repository's identity ([`repo_key`]).
    pub url: String,
    /// Where fetches go: `url` after git's `url.<base>.insteadOf` (a mirror of the same repository).
    pub fetch_url: String,
    pub tracking_ref: String,
}

pub fn upstream(repo: &Repository) -> Result<Option<Upstream>, GitError> {
    let Some(branch) = repo.current_branch()? else { return Ok(None) };
    let config = config::read_file(&repo.common_dir.join("config"))?;
    let Some(remote) = config::get(&config, &format!("branch.{branch}.remote"))? else { return Ok(None) };
    if remote == "." || remote.is_empty() { return Ok(None); }
    let Some(merge) = config::get(&config, &format!("branch.{branch}.merge"))? else { return Ok(None) };
    let Some(remote_branch) = merge.strip_prefix("refs/heads/").filter(|s| !s.is_empty()) else { return Ok(None) };
    let Some(url) = config::get(&config, &format!("remote.{remote}.url"))?.filter(|s| !s.is_empty()) else { return Ok(None) };
    let tracking_ref = format!("refs/remotes/{remote}/{remote_branch}");
    let fetch_url = instead_of(&config, &url);
    Ok(Some(Upstream { branch, remote, remote_branch: remote_branch.into(), url, fetch_url, tracking_ref }))
}

/// git's `url.<base>.insteadOf`: the longest value that `url` starts with is replaced by its base.
fn instead_of(config: &str, url: &str) -> String {
    config::subsection_values(config, "url", "insteadOf").into_iter()
        .filter(|(_, prefix)| !prefix.is_empty() && url.starts_with(prefix.as_str()))
        .max_by_key(|(_, prefix)| prefix.len())
        .map_or_else(|| url.to_owned(), |(base, prefix)| format!("{base}{}", &url[prefix.len()..]))
}

#[derive(PartialEq, Eq)]
pub enum FetchFrom {
    Https(String),
    Local(PathBuf),
}

// Split off user info and port. Check the whole authority before deciding whether HTTP is local.
fn host_path(rest: &str) -> Option<(&str, &str)> {
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.contains(['\\', '?', '#']) || authority.chars().any(char::is_whitespace) {
        return None;
    }
    let server = authority.rsplit('@').next()?;
    let end = if server.starts_with('[') { server.find(']')? + 1 } else { server.find(':').unwrap_or(server.len()) };
    let (host, suffix) = server.split_at(end);
    if host.is_empty() || (!suffix.is_empty() && suffix.strip_prefix(':')?.parse::<u16>().is_err()) {
        return None;
    }
    Some((host, path))
}

fn invalid_url() -> GitError {
    GitError::InvalidRef("invalid upstream URL".into())
}

// Decode the path before handing it to the filesystem. A file URL can name a UNC share.
fn file_url_path(rest: &str) -> Result<PathBuf, GitError> {
    let (authority, path) = rest.split_once('/').ok_or_else(invalid_url)?;
    if authority.contains(['@', ':', '\\', '?', '#', '%']) { return Err(invalid_url()); }
    let mut bytes = Vec::new();
    if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
        bytes.extend_from_slice(b"//");
        bytes.extend_from_slice(authority.as_bytes());
    }
    bytes.push(b'/');
    let mut path = path.split(['?', '#']).next().unwrap_or_default();
    while let Some((prefix, escape)) = path.split_once('%') {
        bytes.extend_from_slice(prefix.as_bytes());
        let hex = escape.get(..2).ok_or_else(invalid_url)?;
        bytes.push(u8::from_str_radix(hex, 16).map_err(|_| invalid_url())?);
        path = &escape[2..];
    }
    bytes.extend_from_slice(path.as_bytes());
    if bytes.contains(&0) { return Err(invalid_url()); }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(std::ffi::OsString::from_vec(bytes).into())
    }
    #[cfg(not(unix))]
    {
        let path = String::from_utf8(bytes).map_err(|_| invalid_url())?;
        #[cfg(windows)]
        if path.as_bytes().get(2) == Some(&b':') && path.as_bytes()[1].is_ascii_alphabetic() {
            return Ok(path[1..].into());
        }
        Ok(path.into())
    }
}

pub fn fetch_from(url: &str) -> Result<FetchFrom, GitError> {
    if url.is_empty() || url.chars().any(char::is_control) { return Err(invalid_url()); }
    if let Some((scheme, rest)) = url.split_once("://") {
        match scheme {
            "https" | "http" | "ssh" => {
                let (host, path) = host_path(rest).ok_or_else(invalid_url)?;
                if scheme == "http" && !(host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1") {
                    return Err(GitError::InvalidRef("http upstream must be on loopback".into()));
                }
                return Ok(FetchFrom::Https(if scheme == "ssh" {
                    format!("https://{host}/{path}")
                } else { url.to_owned() }));
            }
            "file" => return file_url_path(rest).map(FetchFrom::Local),
            s if s.starts_with(|c: char| c.is_ascii_alphabetic())
                && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"+.-".contains(&c)) => {
                return Err(GitError::InvalidRef(format!("unsupported upstream scheme {s}")));
            }
            _ => return Err(invalid_url()),
        }
    }
    // As in git, a slash before the colon makes this a filesystem path.
    if let Some((server, path)) = url.split_once(':') {
        if !server.contains(['/', '\\']) && !(server.len() == 1 && server.as_bytes()[0].is_ascii_alphabetic()) {
            let (host, _) = host_path(server).ok_or_else(invalid_url)?;
            return Ok(FetchFrom::Https(format!("https://{host}/{}", path.trim_start_matches('/'))));
        }
    }
    Ok(FetchFrom::Local(url.into()))
}

/// Identifier a peer may share. Only the final component of a local path leaves the checkout.
pub fn repo_key(url: &str) -> Option<String> {
    let key = match fetch_from(url).ok()? {
        FetchFrom::Https(url) => {
            let (host, path) = host_path(url.split_once("://")?.1)?;
            let path = path.split(['?', '#']).next()?.trim_matches('/');
            let path = path.strip_suffix(".git").unwrap_or(path);
            if path.is_empty() { return None; }
            format!("{}/{path}", host.to_ascii_lowercase())
        }
        FetchFrom::Local(path) => {
            let name = path.file_name()?.to_str()?.rsplit('\\').next()?;
            let name = name.strip_suffix(".git").unwrap_or(name);
            if name.is_empty() { return None; }
            format!("local/{name}")
        }
    };
    Some(key)
}

#[derive(Debug, PartialEq, Eq)]
pub struct Fetched {
    pub old: Option<ObjectId>,
    pub tip: ObjectId,
}

fn optional_ref(result: Result<ObjectId, GitError>) -> Result<Option<ObjectId>, GitError> {
    match result {
        Ok(oid) => Ok(Some(oid)),
        Err(GitError::RefNotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

fn transport_error(mut reason: &str) -> GitError {
    let mut message = String::from("upstream HTTP request failed: ");
    while let Some((before, rest)) = reason.split_once("://") {
        message.push_str(before);
        message.push_str("://");
        let authority = rest.split(|c: char| c.is_whitespace() || matches!(c, '/' | '?' | '#')).next().unwrap();
        reason = match authority.rfind('@') {
            Some(at) => &rest[at + 1..],
            None => rest,
        };
    }
    message.push_str(reason);
    GitError::InvalidRef(message)
}

fn reflog_path(dir: &Path, name: &str) -> Result<PathBuf, GitError> {
    let path = checked_worktree_path(dir, &format!("logs/{name}"))?;
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() => Err(GitError::InvalidRef(format!("expected a regular reflog at {}", path.display()))),
        Err(e) if e.kind() != ErrorKind::NotFound => Err(GitError::io_at("stat reflog", &path, e)),
        _ => Ok(path),
    }
}

// Open all log destinations before changing the checkout or moving its ref.
struct ReflogUpdate {
    files: Vec<(PathBuf, fs::File)>,
    line: String,
}

impl ReflogUpdate {
    fn open(repo: &Repository, logs: &[(&Path, &str)], old: ObjectId, new: ObjectId, message: &str) -> Result<Self, GitError> {
        let config = config::read_file(&repo.common_dir.join("config"))?;
        let mut files = Vec::new();
        if config::get(&config, "core.logAllRefUpdates")?.as_deref().and_then(config::parse_bool) == Some(false) {
            return Ok(Self { files, line: String::new() });
        }
        let identity_field = |key| -> Result<Option<String>, GitError> {
            Ok(config::get(&config, key)?.map(|value| {
                value.trim().replace(|c: char| c.is_control() || matches!(c, '<' | '>'), "")
            }).filter(|value| !value.is_empty()))
        };
        let identity = match (identity_field("user.name")?, identity_field("user.email")?) {
            (Some(name), Some(email)) => format!("{name} <{email}>"),
            _ => "makepad <>".into(),
        };
        let seconds = crate::clock::unix_nanos() / 1_000_000_000;
        let message = message.replace(['\r', '\n', '\t'], " ");
        let line = format!("{old} {new} {identity} {seconds} +0000\t{message}\n");
        for &(dir, name) in logs {
            let path = reflog_path(dir, name)?;
            fs::create_dir_all(path.parent().unwrap())?;
            let file = fs::OpenOptions::new().create(true).append(true).open(&path)
                .map_err(|e| GitError::io_at("open reflog", &path, e))?;
            files.push((path, file));
        }
        Ok(Self { files, line })
    }

    fn append(self) -> Result<(), GitError> {
        for (path, mut file) in self.files {
            file.write_all(self.line.as_bytes()).and_then(|_| file.sync_all())
                .map_err(|e| GitError::io_at("append reflog", &path, e))?;
        }
        Ok(())
    }
}

fn reflog_saw_tip(repo: &Repository, tracking_ref: &str, tip: ObjectId) -> Result<bool, GitError> {
    let path = reflog_path(&repo.common_dir, tracking_ref)?;
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(GitError::io_at("open reflog", &path, e)),
    };
    // Git writes two 40-byte hex fields. The identity and message may use any encoding.
    let new_field = format!(" {tip} ");
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line)? != 0 {
        if line.ends_with(b"\n") && line.get(40..82) == Some(new_field.as_bytes())
            && line[..40].iter().all(u8::is_ascii_hexdigit) {
            return Ok(true);
        }
        line.clear();
    }
    Ok(false)
}

/// A local upstream as git's local transport takes it: a checkout, or the git directory of a
/// bare repository (how local mirrors are kept). Never a repository that merely encloses the path.
fn open_source(path: &Path) -> Result<Repository, GitError> {
    let path = path.canonicalize()?;
    if crate::repo::repository_paths(&path)?.is_some() { return Repository::open(&path); }
    if path.join("HEAD").is_file() && path.join("objects").is_dir() && path.join("refs").is_dir() {
        return Repository::open_git_dir(path.clone(), path);
    }
    Err(GitError::InvalidRef(format!("not a git repository: {}", path.display())))
}

pub fn fetch(
    repo: &mut Repository,
    up: &Upstream,
    http: &mut dyn FnMut(&GitHttpRequest) -> Result<GitHttpResponse, String>,
    hooks: &mut dyn HttpSyncHooks,
) -> Result<Fetched, GitError> {
    let branch = format!("refs/heads/{}", up.remote_branch);
    if up.tracking_ref != format!("refs/remotes/{}/{}", up.remote, up.remote_branch) {
        return Err(GitError::InvalidRef("unexpected upstream tracking ref".into()));
    }
    checked_worktree_path(&repo.common_dir, &up.tracking_ref)?;
    let old = optional_ref(repo.resolve_ref(&up.tracking_ref))?;
    let tip = match fetch_from(&up.fetch_url)? {
        FetchFrom::Local(path) => {
            let mut source = open_source(&repo.workdir.join(path))?;
            let tip = source.resolve_ref(&branch)?;
            copy_objects(&mut source, repo, tip)?;
            tip
        }
        FetchFrom::Https(url) => {
            let mut send = |req: &GitHttpRequest| http(req).map_err(|reason| transport_error(&reason));
            let resp = send(&http_sync::build_info_refs_request(&url, None))?;
            let head = http_sync::parse_info_refs_response(&resp, Some(&branch))
                .map_err(|_| GitError::InvalidRef(format!("no usable advertisement for {branch} (HTTP {})", resp.status_code)))?;
            match repo.read_commit(&head.oid) {
                Ok(_) => {}
                Err(GitError::ObjectNotFound(_)) => {
                    let mut have: Vec<_> = optional_ref(repo.head_oid())?.into_iter().chain(old).collect();
                    have.dedup();
                    let req = http_sync::build_upload_pack_request(&url, head.oid, &head.capabilities, &have, None)?;
                    let resp = send(&req)?;
                    let pack = http_sync::extract_pack_from_response(&resp)
                        .map_err(|_| GitError::CorruptPack(format!("invalid upload-pack response (HTTP {})", resp.status_code)))?
                        .ok_or_else(|| GitError::CorruptPack("empty upload-pack response".into()))?;
                    http_sync::import_fetch_pack(repo, &pack, hooks)?;
                }
                Err(e) => return Err(e),
            }
            head.oid
        }
    };
    repo.read_commit(&tip)?;
    if old != Some(tip) {
        let log = ReflogUpdate::open(repo, &[(&repo.common_dir, &up.tracking_ref)], old.unwrap_or(ObjectId::ZERO),
            tip, &format!("fetch: {} {}", up.remote, up.remote_branch))?;
        refs::write_ref_durable(&repo.common_dir, &up.tracking_ref, &tip)?;
        log.append()?;
    }
    Ok(Fetched { old, tip })
}

// Publish children first. After an interrupted copy a present tree or commit still closes its walk.
fn copy_objects(source: &mut Repository, repo: &mut Repository, tip: ObjectId) -> Result<(), GitError> {
    let mut stack = vec![(tip, None::<crate::Object>)];
    while let Some((oid, object)) = stack.pop() {
        if let Some(object) = object {
            if repo.write_object(object.kind, &object.data)? != oid {
                return Err(GitError::InvalidObject(format!("object hash mismatch for {oid}")));
            }
            continue;
        }
        match repo.read_object(&oid) {
            Ok(_) => continue,
            Err(GitError::ObjectNotFound(_)) => {}
            Err(e) => return Err(e),
        }
        let object = source.read_object(&oid)?;
        let children: Vec<_> = match object.kind {
            ObjectKind::Commit => {
                let commit = crate::commit::parse_commit(&object.data)?;
                commit.parents.into_iter().chain([commit.tree]).collect()
            }
            ObjectKind::Tree => crate::tree::parse_tree(&object.data)?.entries.into_iter()
                .filter(|entry| !entry.is_gitlink()).map(|entry| entry.oid).collect(),
            ObjectKind::Blob => Vec::new(),
            ObjectKind::Tag => return Err(GitError::InvalidObject("unexpected tag in branch history".into())),
        };
        stack.push((oid, Some(object)));
        stack.extend(children.into_iter().map(|child| (child, None)));
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    Current,
    FastForward { from: ObjectId, to: ObjectId },
    Mirror { from: ObjectId, to: ObjectId },
    Changes(Vec<String>),
    OwnCommits,
}

pub fn plan(repo: &mut Repository, up: &Upstream, fetched: &Fetched) -> Result<Plan, GitError> {
    let from = branch_head(repo, &up.branch)?;
    let to = fetched.tip;
    if reach::is_ancestor(repo, to, from)? { return Ok(Plan::Current); }
    let status = repo.status_with_options(StatusOptions { skip_untracked: true, skip_target_dirs: true, ..Default::default() })?;
    if !status.entries.is_empty() {
        let mut paths: Vec<_> = status.entries.into_iter().map(|e| e.path).collect();
        paths.dedup();
        paths.truncate(5);
        return Ok(Plan::Changes(paths));
    }
    Ok(if reach::is_ancestor(repo, from, to)? {
        Plan::FastForward { from, to }
    } else if fetched.old == Some(from) || reflog_saw_tip(repo, &up.tracking_ref, from)? {
        Plan::Mirror { from, to }
    } else {
        Plan::OwnCommits
    })
}

fn branch_head(repo: &Repository, branch: &str) -> Result<ObjectId, GitError> {
    if repo.current_branch()?.as_deref() != Some(branch) {
        return Err(GitError::InvalidRef("checked-out branch changed".into()));
    }
    let name = format!("refs/heads/{branch}");
    checked_worktree_path(&repo.common_dir, &name)?;
    repo.resolve_ref(&name)
}

pub struct Prepared {
    checkout: Checkout,
    branch: String,
    from: ObjectId,
    to: ObjectId,
}

pub fn prepare(
    repo: &mut Repository, branch: &str, from: ObjectId, to: ObjectId,
) -> Result<Prepared, GitError> {
    let lock = IndexLock::acquire(&repo.git_dir)?;
    if branch_head(repo, branch)? != from {
        return Err(GitError::InvalidRef("branch moved before prepare".into()));
    }
    let old = repo.read_commit(&from)?.tree;
    let new = repo.read_commit(&to)?.tree;
    let checkout = Checkout::prepare(repo, lock, &old, &new)?;
    Ok(Prepared { checkout, branch: branch.into(), from, to })
}

impl Prepared {
    pub fn apply(self, repo: &mut Repository) -> Result<(), GitError> {
        if repo.workdir != self.checkout.workdir || branch_head(repo, &self.branch)? != self.from {
            return Err(GitError::InvalidRef("checkout changed since prepare".into()));
        }
        // An editor can save while another repository is being prepared.
        self.checkout.check_paths()?;
        let branch_ref = format!("refs/heads/{}", self.branch);
        let log = ReflogUpdate::open(repo, &[(&repo.common_dir, &branch_ref), (&repo.git_dir, "HEAD")],
            self.from, self.to, "makepad run: follow upstream")?;
        self.checkout.apply()?;
        refs::write_ref_durable(&repo.common_dir, &branch_ref, &self.to)?;
        log.append()
    }
}

struct BlobWrite {
    path: String,
    oid: ObjectId,
    mode: u32,
    data: Vec<u8>,
}

pub(crate) struct Checkout {
    workdir: PathBuf,
    index: Index,
    lock: IndexLock,
    removed: BTreeMap<String, (ObjectId, u32)>,
    writes: Vec<BlobWrite>,
}

impl Checkout {
    pub(crate) fn prepare(repo: &mut Repository, lock: IndexLock, old: &ObjectId, new: &ObjectId) -> Result<Self, GitError> {
        let index = match read_index_for_update(&repo.git_dir) {
            Err(GitError::Io(e)) if e.kind() == ErrorKind::NotFound => Index { version: 2, entries: vec![] },
            other => other?,
        };
        let mut removed = BTreeMap::new();
        let mut writes = Vec::new();
        for change in repo.diff_trees(old, new)? {
            let (path, oid, mode) = match change {
                TreeChange::Deleted { path, oid, mode } => {
                    removed.insert(path, (oid, mode));
                    continue;
                }
                TreeChange::Modified { path, old_oid, old_mode, new_oid, new_mode } => {
                    removed.insert(path.clone(), (old_oid, old_mode));
                    (path, new_oid, new_mode)
                }
                TreeChange::Added { path, oid, mode } => (path, oid, mode),
            };
            let data = repo.read_blob(&oid)?;
            writes.push(BlobWrite { path, oid, mode, data });
        }
        let checkout = Self { workdir: repo.workdir.clone(), index, lock, removed, writes };
        checkout.check_paths()?;
        Ok(checkout)
    }

    fn check_paths(&self) -> Result<(), GitError> {
        let indexed = |path: &str| self.index.entries.binary_search_by(|e| e.path.as_str().cmp(path))
            .ok().map(|i| &self.index.entries[i]);
        for (path, &(oid, mode)) in &self.removed {
            let file = checked_worktree_path(&self.workdir, path)?;
            if !indexed(path).is_some_and(|entry| entry.stage() == 0 && entry.oid == oid && entry.mode == mode)
                || !matches_blob(&file, oid, mode)? {
                return Err(GitError::InvalidObject(format!("{path} was modified locally")));
            }
        }
        for write in &self.writes {
            if self.removed.contains_key(&write.path) { continue; }
            let path = &write.path;
            let file = checked_worktree_path(&self.workdir, path)?;
            if indexed(path).is_some() {
                return Err(GitError::InvalidIndex(format!("{path} already staged")));
            }
            // A tracked file may become a directory if this update removes it first.
            for (end, _) in path.match_indices('/') {
                let parent = &path[..end];
                if let Ok(meta) = fs::symlink_metadata(self.workdir.join(parent)) {
                    if !meta.is_dir() && !self.removed.contains_key(parent) {
                        return Err(GitError::InvalidObject(format!("untracked path in the way: {parent}")));
                    }
                }
            }
            match fs::symlink_metadata(file) {
                Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}
                Err(e) => return Err(e.into()),
                Ok(meta) if meta.is_dir() && self.directory_will_empty(path)? => {}
                Ok(_) => return Err(GitError::InvalidObject(format!("untracked path in the way: {path}"))),
            }
        }
        Ok(())
    }

    // Every leaf must be a tracked removal. Even an untracked empty directory is preserved.
    fn directory_will_empty(&self, path: &str) -> Result<bool, GitError> {
        let mut had_entry = false;
        for child in fs::read_dir(self.workdir.join(path))? {
            let child = child?;
            let Ok(name) = child.file_name().into_string() else { return Ok(false) };
            let child_path = format!("{path}/{name}");
            if !self.removed.contains_key(&child_path)
                && !(child.file_type()?.is_dir() && self.directory_will_empty(&child_path)?) {
                return Ok(false);
            }
            had_entry = true;
        }
        Ok(had_entry)
    }

    pub(crate) fn apply(mut self) -> Result<(), GitError> {
        for path in self.removed.keys() {
            let file = checked_worktree_path(&self.workdir, path)?;
            fs::remove_file(&file).map_err(|e| GitError::io_at("remove", &file, e))?;
            worktree::remove_empty_dirs(file.parent().unwrap(), &self.workdir);
        }
        self.index.entries.retain(|e| !self.removed.contains_key(&e.path));
        for write in self.writes {
            let file = checked_worktree_path(&self.workdir, &write.path)?;
            fs::create_dir_all(file.parent().unwrap())?;
            write.create(&file)?;
            self.index.entries.push(worktree::index_entry_from_metadata(
                write.path, write.oid, write.mode, &fs::symlink_metadata(&file)?,
            ));
        }
        self.index.entries.sort_by(|a, b| a.path.cmp(&b.path));
        self.lock.commit(&self.index, true)
    }
}

fn matches_blob(file: &Path, oid: ObjectId, mode: u32) -> Result<bool, GitError> {
    let meta = match fs::symlink_metadata(file) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
        if mode == 0o120000 {
            return Ok(meta.is_symlink() && crate::oid::hash_object("blob", fs::read_link(file)?.as_os_str().as_bytes()) == oid);
        }
        if (meta.permissions().mode() & 0o100 != 0) != (mode == 0o100755) { return Ok(false); }
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(meta.is_file() && worktree::hash_file_blob(file)? == oid)
}

impl BlobWrite {
    fn create(&self, path: &Path) -> Result<(), GitError> {
        #[cfg(unix)]
        if self.mode == 0o120000 {
            use std::os::unix::ffi::OsStrExt;
            return Ok(std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(&self.data), path)?);
        }
        // Exclusive creation also preserves an untracked file created after prepare.
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(path)
            .map_err(|e| GitError::io_at("create", path, e))?;
        file.write_all(&self.data)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(if self.mode == 0o100755 { 0o755 } else { 0o644 }))?;
        }
        Ok(())
    }
}
