//! Authenticated commercial releases. Credentials never enter Git configs or build commands.
use crate::{http, progress, sha256};
use makepad_git::{
    compute_status_with_options, flatten_tree,
    http_sync::{apply_pack_and_checkout, HttpSyncHooks},
    FileStatus, ObjectId, StatusOptions,
};
use makepad_strict_json::{self as json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const DEFAULT_SERVICE: &str = "https://makepad.nl/api/loader";

#[derive(Clone, Debug)]
pub struct Repository {
    pub name: String,
    pub path: String,
    pub commit: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub id: String,
    pub title: String,
    pub release: String,
    pub rust: String,
    pub package: String,
    pub binary: String,
    pub workspace: String,
    pub platforms: Vec<String>,
    pub cuda: bool,
    pub public: bool,
    pub features: Vec<String>,
    pub repositories: Vec<Repository>,
    raw: String,
}

pub fn platform() -> &'static str {
    if cfg!(all(windows, target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(target_os = "macos") {
        "x86_64-apple-darwin"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64-unknown-linux-gnu"
    } else {
        "x86_64-unknown-linux-gnu"
    }
}

fn text(v: &Value, key: &str) -> Result<String, String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() < 512)
        .map(str::to_owned)
        .ok_or_else(|| format!("Release is missing {key}"))
}
pub fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
pub fn relative(s: &str) -> bool {
    !s.is_empty() && s.len() < 512 && s.split('/').all(identifier)
}
fn hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Release {
    pub fn parse(v: &Value) -> Result<Self, String> {
        let mut r = Self {
            id: text(v, "id")?,
            title: text(v, "title")?,
            release: text(v, "release")?,
            rust: text(v, "rust")?,
            package: text(v, "package")?,
            binary: text(v, "binary")?,
            workspace: text(v, "workspace")?,
            platforms: Vec::new(),
            cuda: v.get("cuda").and_then(Value::as_bool).unwrap_or(false),
            public: v.get("public").and_then(Value::as_bool).unwrap_or(false),
            features: v.get("features").and_then(Value::as_arr).unwrap_or(&[]).iter().map(|f| f.as_str().filter(|s| identifier(s)).map(str::to_owned).ok_or("Invalid app feature")).collect::<Result<_, _>>()?,
            repositories: Vec::new(),
            raw: v.to_json(),
        };
        if ![&r.id, &r.release, &r.package, &r.binary]
            .into_iter()
            .all(|s| identifier(s))
            || !relative(&r.workspace)
            || r.rust.split('.').count() != 3
            || !r
                .rust
                .split('.')
                .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err("Invalid release identifier, workspace or pinned Rust version".into());
        }
        r.platforms = v
            .get("platforms")
            .and_then(Value::as_arr)
            .ok_or("Release has no platforms")?
            .iter()
            .map(|v| {
                v.as_str()
                    .filter(|s| identifier(s))
                    .map(str::to_owned)
                    .ok_or_else(|| "Invalid platform".to_string())
            })
            .collect::<Result<_, _>>()?;
        for entry in v
            .get("repositories")
            .and_then(Value::as_arr)
            .ok_or("Release has no repositories")?
        {
            let repo = Repository {
                name: text(entry, "name")?,
                path: text(entry, "path")?,
                commit: text(entry, "commit")?,
                sha256: text(entry, "sha256")?,
                bytes: entry
                    .get("bytes")
                    .and_then(Value::as_u64)
                    .ok_or("Missing repository size")?,
            };
            if !identifier(&repo.name)
                || !relative(&repo.path)
                || !hex(&repo.commit, 40)
                || !hex(&repo.sha256, 64)
                || repo.bytes == 0
                || repo.bytes > 2 * 1024 * 1024 * 1024
                || r.repositories
                    .iter()
                    .any(|x| x.name == repo.name || x.path == repo.path)
            {
                return Err("Invalid repository in release".into());
            }
            r.repositories.push(repo);
        }
        if r.repositories.is_empty() || r.repositories.len() > 8 {
            return Err("Invalid repository count".into());
        }
        // Parents must be checked out before nested app repositories.
        r.repositories.sort_by_key(|r| r.path.len());
        Ok(r)
    }
    pub fn supported(&self) -> bool {
        self.platforms.iter().any(|p| p == platform())
    }
    /// The snapshot directory this release builds from: its own label,
    /// `sources/<release>`, whenever that exists (complete, or still being
    /// completed). Otherwise an existing snapshot under another label whose
    /// receipts pin the same commit for every repository it already holds
    /// and that has room for the rest. A public app pinned to the same
    /// Makepad commit as the installed Scope then builds from Scope's
    /// checkout: the source paths, and with them Cargo's fingerprints in the
    /// shared target directory, are the same, so nothing is downloaded or
    /// compiled twice. A snapshot an app was already built from (see
    /// `mark_built`) wins, then complete matches over partial ones, then the
    /// last label. Nothing matching means a new snapshot under the label.
    pub fn directory(&self, root: &Path) -> PathBuf {
        let own = root.join("sources").join(&self.release);
        if own.exists() {
            return own;
        }
        let mut best: Option<((bool, usize, String), PathBuf)> = None;
        if let Ok(entries) = fs::read_dir(root.join("sources")) {
            for entry in entries.flatten() {
                let candidate = entry.path();
                let Some(label) = entry.file_name().to_str().map(str::to_owned) else { continue };
                if !candidate.is_dir() {
                    continue;
                }
                let mut held = 0;
                let mut usable = true;
                for repo in &self.repositories {
                    if repository_installed(&candidate, repo) {
                        held += 1;
                    } else if candidate.join(&repo.path).exists() {
                        usable = false;
                        break;
                    }
                }
                if !usable || held == 0 {
                    continue;
                }
                let rank = (candidate.join(BUILT_MARKER).is_file(), held, label);
                if best.as_ref().is_none_or(|(r, _)| rank > *r) {
                    best = Some((rank, candidate));
                }
            }
        }
        best.map_or(own, |(_, directory)| directory)
    }
    /// Record that an app was built from this release's snapshot, so later
    /// releases at the same commits prefer it (its artifacts are in the
    /// shared target directory) over an unbuilt copy such as the bootstrap's.
    pub fn mark_built(&self, root: &Path) -> Result<(), String> {
        fs::write(self.directory(root).join(BUILT_MARKER), format!("{} {}\n", self.id, self.release)).map_err(|e| e.to_string())
    }
    pub fn source(&self, root: &Path) -> PathBuf {
        self.directory(root).join(&self.workspace)
    }
    /// One line for the activity log saying where this release's sources
    /// come from, so a full recompile can be told apart from a shared build:
    /// ready under its own label; shared with another label (same commits,
    /// same artifacts); completing a shared snapshot; or a new snapshot whose
    /// Makepad commit differs from the ones already installed, which is the
    /// case where dependencies legitimately compile again.
    pub fn describe_sources(&self, root: &Path) -> String {
        let directory = self.directory(root);
        let own = root.join("sources").join(&self.release);
        let label = |directory: &Path| directory.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if self.installed(root) {
            return if directory == own {
                format!("Sources ready: {}", directory.display())
            } else {
                format!("Sources shared with release {} (same commits, same build artifacts): {}", label(&directory), directory.display())
            };
        }
        if directory != own {
            let missing: Vec<_> =
                self.repositories.iter().filter(|r| !repository_installed(&directory, r)).map(|r| r.name.as_str()).collect();
            return if missing.is_empty() {
                format!("Sources shared with release {}: {} lacks the workspace {}", label(&directory), directory.display(), self.workspace)
            } else {
                format!("Sources shared with release {}: adding {} to {}", label(&directory), missing.join(", "), directory.display())
            };
        }
        let Some(makepad) = self.repositories.iter().find(|r| r.name == "makepad") else {
            return format!("New source snapshot {}", self.release);
        };
        let mut others = Vec::new();
        if let Ok(entries) = fs::read_dir(root.join("sources")) {
            for entry in entries.flatten() {
                let Ok(receipt) = fs::read_to_string(receipt_path(&entry.path(), makepad)) else { continue };
                let commit = receipt.split_whitespace().next().unwrap_or_default();
                if commit != makepad.commit {
                    others.push(format!("{} ({})", entry.file_name().to_string_lossy(), &commit[..commit.len().min(12)]));
                }
            }
        }
        others.sort();
        if others.is_empty() {
            format!("New source snapshot {}", self.release)
        } else {
            format!(
                "New source snapshot {}: its Makepad commit {} differs from {}; dependencies compile again for it",
                self.release,
                &makepad.commit[..12],
                others.join(", ")
            )
        }
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        fs::write(path, &self.raw).map_err(|e| e.to_string())
    }
    pub fn installed(&self, root: &Path) -> bool {
        self.repositories.iter().all(|r| repository_installed(&self.directory(root), r))
            && self.source(root).join("Cargo.toml").is_file()
    }
    pub fn for_app(&self, app: &Value) -> Result<Self, String> {
        let Value::Obj(mut fields) = json::parse(self.raw.as_bytes()).map_err(str::to_owned)? else { return Err("Invalid release".into()); };
        let makepad = fields.iter().find(|(key, _)| key == "repositories")
            .and_then(|(_, value)| value.as_arr())
            .and_then(|repositories| repositories.iter().find(|repository| repository.get("name").and_then(Value::as_str) == Some("makepad")))
            .cloned().ok_or("Release has no public Makepad repository")?;
        fields.retain(|(key, _)| key != "repositories" && key != "public");
        fields.push(("repositories".into(), Value::Arr(vec![makepad])));
        fields.push(("public".into(), Value::Bool(true)));
        for name in ["id", "title", "package", "binary", "workspace", "features"] {
            let value = app.get(name).ok_or("Invalid app registry")?.clone();
            fields.retain(|(key, _)| key != name);
            fields.push((name.into(), value));
        }
        Self::parse(&Value::Obj(fields))
    }

}

fn service_url(service: &str, path: &str) -> Result<String, String> {
    // Development HTTP is restricted to loopback; email addresses always use TLS on the network.
    if !(service.starts_with("https://")
        || service.starts_with("http://127.0.0.1:")
        || service.starts_with("http://localhost:"))
        || service.contains(['?', '#', '@', '\r', '\n'])
    {
        return Err("Source service must use HTTPS".into());
    }
    Ok(format!("{}/{path}", service.trim_end_matches('/')))
}
fn request(
    service: &str,
    path: &str,
    key: &str,
    download: Option<&str>,
) -> Result<Vec<u8>, String> {
    let address = email(key)?;
    let url = service_url(service, path)?;
    let response = http::fetch_method_progress(
        if download.is_some() { "POST" } else { "GET" },
        &url,
        &[
            ("X-Makepad-Email".into(), address),
            ("Cache-Control".into(), "no-store".into()),
        ],
        &[],
        download,
    )?;
    Ok(response.body)
}
fn parse_catalog(bytes: &[u8]) -> Result<Vec<Release>, String> {
    if bytes.len() > 1024 * 1024 { return Err("Release catalog is too large".into()); }
    let v = json::parse(bytes).map_err(|e| format!("Invalid release catalog: {e}"))?;
    let entries = v.as_arr().ok_or("Catalog must contain an app list")?;
    if entries.len() > 128 { return Err("Too many catalog entries".into()); }
    entries.iter().map(Release::parse).collect()
}
pub fn fetch(service: &str, key: &str) -> Result<Vec<Release>, String> {
    progress::stage("Checking sources", "Contacting the source service", 0.0);
    let bytes = request(service, "catalog", key, None)?;
    progress::stage("Checking sources", "Reading the release catalog", 0.0);
    let releases = parse_catalog(&bytes)?;
    progress::stage("Checking sources", "Release catalog ready", 0.0);
    Ok(releases)
}
pub fn fetch_public(service: &str) -> Result<Release, String> {
    progress::stage("Checking sources", "Contacting the public source service", 0.0);
    let response = http::fetch_method_progress("GET", &service_url(service, "public/catalog")?, &[], &[], None)?;
    progress::stage("Checking sources", "Reading the release catalog", 0.0);
    let release = parse_catalog(&response.body)?.into_iter().find(|r| r.public).ok_or_else(|| "Public Makepad source is unavailable".to_owned())?;
    progress::stage("Checking sources", "Release catalog ready", 0.0);
    Ok(release)
}
pub fn apps() -> Result<Vec<Value>, String> {
    json::parse(include_bytes!("../apps.json")).map_err(str::to_owned)?.as_arr().map(<[Value]>::to_vec).ok_or("Invalid app registry".into())
}
/// Written into a snapshot directory once an app was built from it.
const BUILT_MARKER: &str = ".builder-built";
fn receipt(repo: &Repository) -> String { format!("{} {}", repo.commit, repo.sha256) }
fn receipt_path(dest: &Path, repo: &Repository) -> PathBuf { dest.join(".builder-repositories").join(&repo.name) }
/// The receipt records the verified commit and the pack it came from. The
/// commit alone identifies the tree, so a checkout of that commit made from
/// another catalog's pack (the public one against a private one) is the same
/// source and counts as installed.
fn repository_installed(dest: &Path, repo: &Repository) -> bool {
    fs::read_to_string(receipt_path(dest, repo)).is_ok_and(|s| s.split_whitespace().next() == Some(repo.commit.as_str()))
        && dest.join(&repo.path).join(".git").is_dir()
}

/// The public `makepad-git` hook deliberately stays small: older published
/// source packs only report checked-out paths. Keep the Builder compatible
/// with that API and publish a determinate summary once the sync returns its
/// report. During checkout the activity pane still advances for every file,
/// while the final event supplies the total used by the progress bar.
struct CheckoutProgress { files: u64 }
impl HttpSyncHooks for CheckoutProgress {
    fn on_checkout_file(&mut self, path: &str) {
        self.files += 1;
        progress::measured("Writing source files", path, self.files, 0, progress::Unit::Files);
    }
}

/// Stage beside the destination; a failed download never replaces the previous release.
pub fn checkout(
    service: &str,
    key: &str,
    root: &Path,
    release: &Release,
) -> Result<PathBuf, String> {
    let root = crate::validate_install_root(root)?;
    let root = root.as_path();
    if !release.supported() {
        return Err("This release does not support this platform".into());
    }
    if release.installed(root) {
        return Ok(release.source(root));
    }
    let dest = release.directory(root);
    fs::create_dir_all(dest.join(".builder-repositories")).map_err(|e| e.to_string())?;
    for (i, repo) in release.repositories.iter().enumerate() {
        if repository_installed(&dest, repo) { continue; }
        let checkout = dest.join(&repo.path);
        if checkout.exists() { return Err(format!("Existing source at {} has no matching receipt; it was left unchanged", checkout.display())); }
        progress::package("Source repositories", &repo.name, i + 1, release.repositories.len());
        let (url, headers) = if release.public {
            if repo.name != "makepad" || repo.path != "makepad" { return Err("Public releases may contain only Makepad".into()); }
            (service_url(service, &format!("public/source/{}/makepad.pack", release.release))?, vec![])
        } else {
            let address = email(key)?;
            let origin = service.trim_end_matches('/').strip_suffix("/api/loader").ok_or("Source service must end with /api/loader")?;
            (service_url(origin, &format!("{}/git?email={}&release={}&repository={}", release.id, makepad_loader_bundle::query_value(&address), release.release, repo.name))?, vec![("X-Makepad-Email".into(), address), ("Cache-Control".into(), "no-store".into())])
        };
        let bytes = http::fetch_method_progress("GET", &url, &headers, &[], Some(&repo.name))?.body;
        progress::stage("Verifying source", &repo.name, 0.0);
        if bytes.len() as u64 != repo.bytes || !sha256::sha256_hex(&bytes).eq_ignore_ascii_case(&repo.sha256) { return Err(format!("{} download failed size/hash verification", repo.name)); }
        let stage = dest.join(format!(".{}-{}-staging", repo.name, std::process::id()));
        fs::create_dir(&stage).map_err(|e| format!("Source staging: {e}"))?;
        let result = (|| {
            progress::stage("Unpacking Git objects", &repo.name, 0.0);
            let mut hooks = CheckoutProgress { files: 0 };
            let report = apply_pack_and_checkout(&stage, &service_url(service, &format!("repository/{}", repo.name))?, ObjectId::from_hex(&repo.commit).map_err(|e| e.to_string())?, None, &bytes, &mut hooks).map_err(|e| e.to_string())?;
            progress::measured("Unpacking Git objects", "Repository objects", report.imported_objects as u64, report.imported_objects as u64, progress::Unit::Objects);
            progress::measured("Saving Git objects", "Repository pack", bytes.len() as u64, bytes.len() as u64, progress::Unit::Bytes);
            progress::measured("Writing source files", "Repository files", report.checked_out_files as u64, report.checked_out_files as u64, progress::Unit::Files);
            fs::create_dir_all(checkout.parent().ok_or("Missing source parent")?).map_err(|e| e.to_string())?;
            fs::rename(&stage, &checkout).map_err(|e| e.to_string())?;
            fs::write(receipt_path(&dest, repo), receipt(repo)).map_err(|e| e.to_string())
        })();
        if result.is_err() { let _ = fs::remove_dir_all(&stage); }
        result?;
    }
    if !release.source(root).join("Cargo.toml").is_file() { return Err("Release is missing its Cargo workspace".into()); }
    release.save(&dest.join(format!("{}.json", release.id)))?;
    progress::stage("Ready", "Source repositories verified and checked out", 1.0);
    Ok(release.source(root))
}

/// Remove the source snapshots nothing refers to any more: neither the
/// releases in `keep` nor those recorded under `installed/` and
/// `available/`. Every checkout a snapshot's receipts name must be there,
/// with no local change, edited, deleted, new or staged: a snapshot the user
/// or an agent worked in stays, and so does one whose layout this Builder
/// does not recognise. Returns the labels removed, for the activity log.
pub fn prune_snapshots(root: &Path, keep: &[Release]) -> Result<Vec<String>, String> {
    let mut kept: Vec<PathBuf> = keep.iter().map(|release| release.directory(root)).collect();
    for directory in ["installed", "available"] {
        for entry in fs::read_dir(root.join(directory)).into_iter().flatten().flatten() {
            if let Some(release) = fs::read(entry.path()).ok().and_then(|bytes| Release::parse(&json::parse(&bytes).ok()?).ok()) {
                kept.push(release.directory(root));
            }
        }
    }
    let mut removed = Vec::new();
    for entry in fs::read_dir(root.join("sources")).into_iter().flatten().flatten() {
        let snapshot = entry.path();
        let Some(label) = entry.file_name().to_str().map(str::to_owned) else { continue };
        if !snapshot.is_dir() || kept.contains(&snapshot) || !snapshot_unchanged(&snapshot)? {
            continue;
        }
        fs::remove_dir_all(&snapshot).map_err(|e| format!("Remove {}: {e}", snapshot.display()))?;
        removed.push(label);
    }
    Ok(removed)
}

/// True when every repository the snapshot's receipts name is checked out
/// at a known path without local changes. The paths come from the release
/// files saved beside the checkouts; the bootstrap's own snapshot holds only
/// Makepad, which it always checks out under `makepad`.
fn snapshot_unchanged(snapshot: &Path) -> Result<bool, String> {
    let mut paths = vec![("makepad".to_owned(), "makepad".to_owned())];
    for entry in fs::read_dir(snapshot).map_err(|e| e.to_string())?.flatten() {
        if entry.path().extension().is_some_and(|extension| extension == "json") {
            if let Some(release) = fs::read(entry.path()).ok().and_then(|bytes| Release::parse(&json::parse(&bytes).ok()?).ok()) {
                paths.extend(release.repositories.into_iter().map(|repo| (repo.name, repo.path)));
            }
        }
    }
    for receipt in fs::read_dir(snapshot.join(".builder-repositories")).map_err(|e| e.to_string())?.flatten() {
        let name = receipt.file_name().to_string_lossy().into_owned();
        let Some((_, path)) = paths.iter().find(|(known, _)| *known == name) else { return Ok(false) };
        if !checkout_unchanged(&snapshot.join(path))? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn checkout_unchanged(checkout: &Path) -> Result<bool, String> {
    let describe = |error: makepad_git::GitError| format!("{}: {error}", checkout.display());
    let mut repository = makepad_git::Repository::open(checkout).map_err(describe)?;
    let head = repository.head_oid().map_err(describe)?;
    let commit = repository.read_commit(&head).map_err(describe)?;
    let tree = repository.read_tree(&commit.tree).map_err(describe)?;
    let files = flatten_tree(&tree, "", &mut |oid| repository.read_tree(oid)).map_err(describe)?;
    let index = repository.read_index().map_err(describe)?;
    let options = StatusOptions { skip_hidden: false, skip_target_dirs: true, skip_worktree_content_compare: false };
    let status = compute_status_with_options(&files, &index, &repository.workdir, options).map_err(describe)?;
    // Makepad tracks a few symbolic links. The Builder's checkout does not
    // create them, and Git's own checkout in the bootstrap leaves them
    // dangling, so their absence is how every snapshot starts, not an edit.
    let symbolic_link = |path: &str| index.entries.iter().any(|entry| entry.path == path && entry.mode & 0o170000 == 0o120000);
    Ok(status.entries.iter().all(|entry| entry.status == FileStatus::Deleted && symbolic_link(&entry.path)))
}

pub fn email(value: &str) -> Result<String, String> {
    makepad_loader_bundle::email(value)
}
