//! On-demand HuggingFace model file downloader.
//!
//! - URL scheme: `<hf_base>/<repo>/resolve/<revision>/<path>` (legacy files
//!   default to `main`; release manifests pin immutable commits). hf_base defaults to
//!   https://huggingface.co and is configurable so tests can point it at a
//!   localhost fixture server, and so a LAN mirror can be used). A `path`
//!   that is itself an absolute http(s) URL is fetched verbatim — for
//!   weights that are not on HF (woosh-sfx's GitHub release zips); the
//!   bearer token stays restricted to the hf_base host either way.
//! - Resumable: bytes stream into `<dest>.part`; a restart sends
//!   `Range: bytes=<part-len>-` and appends. A server that ignores Range and
//!   answers 200 causes a clean restart from zero. A 416 only completes a
//!   `.part` whose length exactly matches its pinned manifest size.
//! - Atomic finish: verify size/hash, rename onto the destination, then write
//!   an atomic identity receipt. Existing files are not trusted by existence.
//! - Gated repos (flux1-dev): `HF_TOKEN` env is sent as a bearer token, but
//!   only to hosts under the hf_base host suffix — redirects to the CDN do
//!   not leak the token.

use crate::backend::CancelToken;
use crate::error::AssetAiError;
use crate::http_client::{http_fetch, parse_url, BearerAuth, HttpClientRequest};
use crate::registry::{ConversionSpec, FileSpec};
use crate::sha256::{to_hex, Sha256};
use makepad_micro_serde::*;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use std::time::{Duration, Instant};

pub const DEFAULT_HF_BASE: &str = "https://huggingface.co";

#[derive(Clone, Debug)]
pub struct DownloadProgress {
    /// Repo-relative file name being downloaded.
    pub file: String,
    /// Bytes on disk so far (includes the resumed prefix).
    pub done: u64,
    /// Total bytes when the server reports them (or the registry knows).
    pub total: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Downloader {
    pub hf_base: String,
    pub token: Option<String>,
    /// Host suffix the bearer token is restricted to; derived from hf_base.
    pub token_host_suffix: String,
    /// Peer-assisted transfer plan for the CURRENT job (coordinator source
    /// list + tickets); tried before the canonical `hf_base` path. `None` =
    /// straight to Hugging Face. See [`crate::peer_fetch`].
    pub peers: Option<Arc<crate::peer::PeerPlan>>,
    /// In-flight peer-serve leases of this service process: delete/replace
    /// steps below refuse to touch a path while a peer transfer reads it.
    pub leases: Option<crate::peer::ServeLeases>,
}

/// How long delete/replace steps wait for an in-flight peer serve of the
/// same path to finish before failing explicitly.
const LEASE_WAIT: Duration = Duration::from_secs(10);

impl Downloader {
    pub fn new(hf_base: &str, token: Option<String>) -> Result<Self, AssetAiError> {
        let parsed = parse_url(hf_base)?;
        Ok(Self {
            hf_base: hf_base.trim_end_matches('/').to_string(),
            token,
            token_host_suffix: parsed.host,
            peers: None,
            leases: None,
        })
    }

    /// Per-job peer plan (the server attaches the request's coordinator
    /// sources/tickets plus env-injected sources here).
    pub fn with_peer_plan(mut self, peers: Option<Arc<crate::peer::PeerPlan>>) -> Self {
        self.peers = peers;
        self
    }

    /// Wires the service's serve-lease registry into this downloader.
    pub fn with_serve_leases(mut self, leases: crate::peer::ServeLeases) -> Self {
        self.leases = Some(leases);
        self
    }

    /// Waits for any in-flight peer serve of `path`, then removes it. An
    /// unresolvable lease is an explicit error, never a clobbered source.
    fn guarded_remove(&self, path: &Path) -> Result<(), AssetAiError> {
        if let Some(leases) = &self.leases {
            leases.wait_unleased(path, LEASE_WAIT)?;
        }
        let _ = fs::remove_file(path);
        Ok(())
    }

    /// hf_base from `MAKEPAD_ASSET_AI_HF_BASE` (default huggingface.co),
    /// token from `HF_TOKEN`.
    pub fn from_env() -> Result<Self, AssetAiError> {
        let hf_base = std::env::var("MAKEPAD_ASSET_AI_HF_BASE")
            .unwrap_or_else(|_| DEFAULT_HF_BASE.to_string());
        let token = std::env::var("HF_TOKEN").ok().filter(|t| !t.is_empty());
        Self::new(&hf_base, token)
    }

    pub fn file_url(&self, file: &FileSpec) -> String {
        // Absolute URLs (GitHub release assets etc.) are used verbatim; the
        // token host restriction keeps HF_TOKEN off foreign hosts.
        if file.path.starts_with("https://") || file.path.starts_with("http://") {
            return file.path.clone();
        }
        format!(
            "{}/{}/resolve/{}/{}",
            self.hf_base,
            file.repo,
            file.resolved_revision(),
            file.path
        )
    }

    /// Makes sure `file` exists and matches its manifest identity; downloads
    /// (or resumes) it otherwise. Progress is reported after every chunk.
    pub fn ensure_file(
        &self,
        file: &FileSpec,
        cache_dir: &Path,
        progress: &mut dyn FnMut(DownloadProgress),
        cancel: &CancelToken,
    ) -> Result<PathBuf, AssetAiError> {
        cancel.check()?;
        let dest = file.dest_path(cache_dir);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AssetAiError::Download(format!("mkdir {}: {e}", parent.display())))?;
        }
        // Serialize all verify/download/commit activity for one cache path
        // across service processes. Waiters re-check the final receipt after
        // acquiring the lock and therefore do no duplicate network work.
        let artifact_lock = ArtifactLock::acquire(&dest, cancel)?;
        if dest.is_file() {
            if verify_source_file(file, cache_dir, cancel)? {
                // Already on disk and verified — do not report this as a
                // download. Chat/warm jobs were surfacing "download 100%".
                return Ok(dest);
            }
            // Keep a same-size/hash-mismatched final out of the resumable
            // `.part` path: a fresh request must replace it from byte zero.
            self.guarded_remove(&dest)?;
            let _ = fs::remove_file(verification_path(&dest));
        }
        if file.local {
            // Locally-converted weights (e.g. .mktts): there is nothing to
            // download — the file has to be placed in the cache by hand or by
            // the converter tooling.
            return Err(AssetAiError::Download(format!(
                "{} is a locally-converted file that cannot be downloaded; place it at {}",
                file.cache_as,
                dest.display()
            )));
        }
        let part = part_path(&dest);

        // Peer-assisted phase: try the coordinator-provided source boxes
        // before Hugging Face. On success the `.part` holds the complete,
        // digest-verified bytes and the same atomic commit + receipt tail
        // runs; on any peer failure this falls straight through to the
        // canonical path below (possibly resuming the same `.part`).
        if let Some(plan) = self.peers.clone() {
            if let Some(verified_sha256) = crate::peer_fetch::try_fetch_via_peers(
                &plan,
                file,
                &part,
                progress,
                cancel,
                &|| artifact_lock.heartbeat(),
            )? {
                let part_len = fs::metadata(&part)
                    .map_err(|e| {
                        AssetAiError::Download(format!("metadata {}: {e}", part.display()))
                    })?
                    .len();
                if file.size == Some(part_len) {
                    cancel.check()?;
                    self.commit_part_to_dest(&part, &dest)?;
                    write_source_receipt(file, &dest, Some(&verified_sha256))?;
                    progress(DownloadProgress {
                        file: file.path.clone(),
                        done: part_len,
                        total: Some(part_len),
                    });
                    return Ok(dest);
                }
                // Size disagreement after a hash match cannot happen unless
                // the file changed under us; quarantine and use the
                // canonical path.
                let _ = fs::remove_file(&part);
            }
        }

        let mut offset = match fs::metadata(&part) {
            Ok(meta) => meta.len(),
            Err(_) => 0,
        };
        if let Some(expected) = file.size {
            if offset > expected {
                return Err(AssetAiError::Download(format!(
                    "{}: partial file is {offset} bytes, larger than expected {expected}",
                    part.display()
                )));
            }
        }

        let url = self.file_url(file);
        let request = HttpClientRequest {
            method: "GET",
            url: &url,
            range_from: if offset > 0 { Some(offset) } else { None },
            range_to: None,
            bearer: self.token.as_deref().map(|token| BearerAuth {
                token,
                host_suffix: &self.token_host_suffix,
            }),
            body: None,
            extra_headers: &[],
        };
        let response = http_fetch(&request)?;

        let mut total;
        let mut download_body = true;
        match response.status {
            200 => {
                // Full body (fresh download, or the server ignored Range):
                // restart from zero.
                offset = 0;
                total = response.content_length().or(file.size);
            }
            206 => {
                total = response
                    .content_range_total()
                    .or_else(|| response.content_length().map(|len| offset + len))
                    .or(file.size);
            }
            416 => {
                if file.size != Some(offset) {
                    return Err(AssetAiError::Download(format!(
                        "{url}: 416 for {offset}-byte partial, expected exact manifest size {:?}",
                        file.size
                    )));
                }
                total = Some(offset);
                download_body = false;
            }
            401 | 403 => {
                return Err(AssetAiError::Download(format!(
                    "{url}: http {} — gated repo? set HF_TOKEN with access granted",
                    response.status
                )));
            }
            404 => {
                return Err(AssetAiError::Download(format!("{url}: not found (404)")));
            }
            status => {
                return Err(AssetAiError::Download(format!("{url}: http {status}")));
            }
        }

        if download_body {
            let mut out = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(offset > 0)
                .truncate(offset == 0)
                .open(&part)
                .map_err(|e| AssetAiError::Download(format!("open {}: {e}", part.display())))?;
            let mut done = offset;
            progress(DownloadProgress {
                file: file.path.clone(),
                done,
                total,
            });
            let mut body = response.body;
            let mut buf = [0u8; 65536];
            let mut since_heartbeat = 0u64;
            loop {
                cancel.check()?;
                let n = body
                    .read(&mut buf)
                    .map_err(|e| AssetAiError::Download(format!("{url}: body read: {e}")))?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])
                    .map_err(|e| AssetAiError::Download(format!("write {}: {e}", part.display())))?;
                done += n as u64;
                since_heartbeat += n as u64;
                if since_heartbeat >= 64 * 1024 * 1024 {
                    artifact_lock.heartbeat()?;
                    since_heartbeat = 0;
                }
                progress(DownloadProgress {
                    file: file.path.clone(),
                    done,
                    total,
                });
            }
            out.flush()
                .map_err(|e| AssetAiError::Download(format!("flush {}: {e}", part.display())))?;
            drop(out);
            artifact_lock.heartbeat()?;
            if let Some(total) = total {
                if done != total {
                    // Keep the .part for a later resume, but report failure.
                    return Err(AssetAiError::Download(format!(
                        "{url}: connection ended at {done} of {total} bytes (partial file kept for resume)"
                    )));
                }
            }
            if total.is_none() {
                total = Some(done);
            }
        }

        let actual_sha256 = if let Some(expected) = &file.sha256 {
            let actual = hash_file_with_heartbeat(&part, cancel, &artifact_lock)?;
            if actual != *expected {
                let _ = fs::remove_file(&part);
                return Err(AssetAiError::Download(format!(
                    "{url}: sha256 mismatch: expected {expected}, got {actual} (partial file discarded)"
                )));
            }
            Some(actual)
        } else {
            None
        };
        let part_len = fs::metadata(&part)
            .map_err(|e| AssetAiError::Download(format!("metadata {}: {e}", part.display())))?
            .len();
        if let Some(expected) = file.size {
            if part_len != expected {
                return Err(AssetAiError::Download(format!(
                    "{url}: downloaded file is {part_len} bytes, expected {expected} (partial kept for resume)"
                )));
            }
        }
        cancel.check()?;

        self.commit_part_to_dest(&part, &dest)?;
        write_source_receipt(file, &dest, actual_sha256.as_deref())?;
        let final_len = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        progress(DownloadProgress {
            file: file.path.clone(),
            done: final_len,
            total: total.or(Some(final_len)),
        });
        Ok(dest)
    }

    /// Atomic-enough finish shared by the peer and Hugging Face paths:
    /// rename within the same directory, only ever called on a fully
    /// verified `.part`. Windows refuses to rename onto an existing file, so
    /// the target is cleared first (waiting out any in-flight peer serve of
    /// it), and the rename retries briefly — antivirus/indexer handles hold
    /// fresh files for moments on real fleet boxes.
    fn commit_part_to_dest(&self, part: &Path, dest: &Path) -> Result<(), AssetAiError> {
        if let Some(leases) = &self.leases {
            leases.wait_unleased(dest, LEASE_WAIT)?;
        }
        let mut attempt = 0u32;
        loop {
            if dest.exists() {
                let _ = fs::remove_file(dest);
            }
            match fs::rename(part, dest) {
                Ok(()) => return Ok(()),
                Err(e) if attempt < 5 => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(40 * attempt as u64));
                    let _ = e;
                }
                Err(e) => {
                    return Err(AssetAiError::Download(format!(
                        "rename {} -> {}: {e}",
                        part.display(),
                        dest.display()
                    )));
                }
            }
        }
    }
}

pub fn part_path(dest: &Path) -> PathBuf {
    let mut os = dest.as_os_str().to_os_string();
    os.push(".part");
    PathBuf::from(os)
}

fn lock_path(dest: &Path) -> PathBuf {
    let mut os = dest.as_os_str().to_os_string();
    os.push(".lock");
    PathBuf::from(os)
}

const LOCK_STALE_AFTER: Duration = Duration::from_secs(6 * 60 * 60);

struct ArtifactLock {
    dir: PathBuf,
    owner: PathBuf,
}

impl ArtifactLock {
    fn acquire(dest: &Path, cancel: &CancelToken) -> Result<Self, AssetAiError> {
        let path = lock_path(dest);
        let started = Instant::now();
        loop {
            cancel.check()?;
            match fs::create_dir(&path) {
                Ok(()) => {
                    let owner = path.join(format!(
                        "owner-{}-{}",
                        std::process::id(),
                        unix_ns()
                    ));
                    match fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&owner)
                    {
                        Ok(mut file) => {
                            let contents = format!(
                                "pid={} acquired_unix_ms={}\n",
                                std::process::id(),
                                unix_ms()
                            );
                            file.write_all(contents.as_bytes()).map_err(|e| {
                                AssetAiError::Download(format!(
                                    "write lock owner {}: {e}",
                                    owner.display()
                                ))
                            })?;
                            return Ok(Self {
                                dir: path,
                                owner,
                            });
                        }
                        Err(error) => {
                            let _ = fs::remove_dir(&path);
                            return Err(AssetAiError::Download(format!(
                                "create lock owner {}: {error}",
                                owner.display()
                            )));
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if break_stale_lock(&path) {
                        continue;
                    }
                    if started.elapsed() > LOCK_STALE_AFTER {
                        return Err(AssetAiError::Download(format!(
                            "timed out waiting for artifact lock {}",
                            path.display()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => {
                    return Err(AssetAiError::Download(format!(
                        "create lock {}: {error}",
                        path.display()
                    )));
                }
            }
        }
    }

    fn heartbeat(&self) -> Result<(), AssetAiError> {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&self.owner)
            .map_err(|error| {
                AssetAiError::Download(format!(
                    "refresh artifact lock {}: {error}",
                    self.owner.display()
                ))
            })?;
        file.write_all(b".").map_err(|error| {
            AssetAiError::Download(format!(
                "refresh artifact lock {}: {error}",
                self.owner.display()
            ))
        })
    }
}

impl Drop for ArtifactLock {
    fn drop(&mut self) {
        // The unique owner name makes stale/release deletion compare-safe:
        // only this guard can remove its owner, and a new owner cannot enter
        // until the directory itself has been removed.
        let _ = fs::remove_file(&self.owner);
        let _ = fs::remove_dir(&self.dir);
    }
}

fn break_stale_lock(dir: &Path) -> bool {
    break_stale_lock_older_than(dir, LOCK_STALE_AFTER)
}

fn break_stale_lock_older_than(dir: &Path, stale_after: Duration) -> bool {
    let mut owners = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("owner-")
        {
            owners.push(entry.path());
        }
    }
    if owners.len() == 1 {
        let owner = &owners[0];
        let stale = fs::metadata(owner)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > stale_after);
        // Only one waiter can remove this exact unique owner. Losers do not
        // remove the directory, so they cannot accidentally delete a newly
        // acquired lock after the winner has cleaned it up.
        return stale && fs::remove_file(owner).is_ok() && fs::remove_dir(dir).is_ok();
    }
    if owners.is_empty() {
        let stale = fs::metadata(dir)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > stale_after);
        if stale {
            // Recover the tiny crash window between create_dir and owner
            // creation. create_new elects exactly one stale breaker.
            let breaker = dir.join(format!("breaker-{}-{}", std::process::id(), unix_ns()));
            if fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&breaker)
                .is_ok()
            {
                let _ = fs::remove_file(&breaker);
                return fs::remove_dir(dir).is_ok();
            }
        }
    }
    false
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn unix_ns() -> u128 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

struct ConversionHeartbeat {
    stop: Arc<std::sync::atomic::AtomicBool>,
    wake: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ConversionHeartbeat {
    fn start(artifact_lock: &ArtifactLock) -> Result<Self, AssetAiError> {
        artifact_lock.heartbeat()?;
        let owner = artifact_lock.owner.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = stop.clone();
        let wake = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let thread_wake = wake.clone();
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(std::sync::atomic::Ordering::Relaxed) {
                let (mutex, condvar) = &*thread_wake;
                let guard = mutex.lock().unwrap();
                let _ = condvar.wait_timeout(guard, Duration::from_secs(30));
                if thread_stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                if let Ok(mut file) = fs::OpenOptions::new().append(true).open(&owner) {
                    let _ = file.write_all(b".");
                } else {
                    break;
                }
            }
        });
        Ok(Self {
            stop,
            wake,
            thread: Some(thread),
        })
    }
}

impl Drop for ConversionHeartbeat {
    fn drop(&mut self) {
        self.stop
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.wake.1.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn hash_file_with_heartbeat(
    path: &Path,
    cancel: &CancelToken,
    artifact_lock: &ArtifactLock,
) -> Result<String, AssetAiError> {
    let mut file = fs::File::open(path)
        .map_err(|e| AssetAiError::Download(format!("open {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    let mut since_heartbeat = 0u64;
    loop {
        cancel.check()?;
        let n = file
            .read(&mut buf)
            .map_err(|e| AssetAiError::Download(format!("read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        since_heartbeat += n as u64;
        if since_heartbeat >= 64 * 1024 * 1024 {
            artifact_lock.heartbeat()?;
            since_heartbeat = 0;
        }
    }
    artifact_lock.heartbeat()?;
    Ok(to_hex(&hasher.finish()))
}

fn hash_file(path: &Path, cancel: &CancelToken) -> Result<String, AssetAiError> {
    let mut file = fs::File::open(path)
        .map_err(|e| AssetAiError::Download(format!("open {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        cancel.check()?;
        let n = file
            .read(&mut buf)
            .map_err(|e| AssetAiError::Download(format!("read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finish()))
}

const RECEIPT_VERSION: u32 = 1;

#[derive(Clone, Debug, SerJson, DeJson)]
struct VerificationReceipt {
    version: u32,
    kind: String,
    repo: String,
    path: String,
    revision: String,
    file_len: u64,
    modified_ns: u64,
    sha256: Option<String>,
    source_sha256: Option<String>,
    converter_id: Option<String>,
    converter_version: Option<String>,
}

pub fn verification_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".verified.json");
    PathBuf::from(value)
}

fn modified_ns(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos()
        .try_into()
        .ok()
}

fn metadata_identity(path: &Path) -> Option<(u64, u64)> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.len(), modified_ns(&metadata)?))
}

fn read_receipt(path: &Path) -> Option<VerificationReceipt> {
    let text = fs::read_to_string(verification_path(path)).ok()?;
    VerificationReceipt::deserialize_json(&text).ok()
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), AssetAiError> {
    let mut part = path.as_os_str().to_os_string();
    part.push(format!(".part-{}", std::process::id()));
    let part = PathBuf::from(part);
    fs::write(&part, bytes)
        .map_err(|e| AssetAiError::Download(format!("write {}: {e}", part.display())))?;
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&part, path).map_err(|e| {
        AssetAiError::Download(format!(
            "rename {} -> {}: {e}",
            part.display(),
            path.display()
        ))
    })
}

fn source_receipt_matches(file: &FileSpec, dest: &Path, receipt: &VerificationReceipt) -> bool {
    let Some((file_len, modified_ns)) = metadata_identity(dest) else {
        return false;
    };
    receipt.version == RECEIPT_VERSION
        && receipt.kind == "source"
        && receipt.repo == file.repo
        && receipt.path == file.path
        && receipt.revision == file.resolved_revision()
        && receipt.file_len == file_len
        && receipt.modified_ns == modified_ns
        && file.size.map_or(true, |expected| expected == file_len)
        && file.sha256.as_deref() == receipt.sha256.as_deref()
        && receipt.source_sha256.is_none()
        && receipt.converter_id.is_none()
        && receipt.converter_version.is_none()
}

fn write_source_receipt(
    file: &FileSpec,
    dest: &Path,
    actual_sha256: Option<&str>,
) -> Result<(), AssetAiError> {
    let (file_len, modified_ns) = metadata_identity(dest).ok_or_else(|| {
        AssetAiError::Download(format!("metadata {} after download", dest.display()))
    })?;
    let receipt = VerificationReceipt {
        version: RECEIPT_VERSION,
        kind: "source".to_string(),
        repo: file.repo.clone(),
        path: file.path.clone(),
        revision: file.resolved_revision().to_string(),
        file_len,
        modified_ns,
        sha256: actual_sha256.map(str::to_string).or_else(|| file.sha256.clone()),
        source_sha256: None,
        converter_id: None,
        converter_version: None,
    };
    atomic_write(
        &verification_path(dest),
        receipt.serialize_json().as_bytes(),
    )
}

fn verify_source_file(
    file: &FileSpec,
    cache_dir: &Path,
    cancel: &CancelToken,
) -> Result<bool, AssetAiError> {
    let dest = file.dest_path(cache_dir);
    if let Some(receipt) = read_receipt(&dest) {
        if source_receipt_matches(file, &dest, &receipt) {
            return Ok(true);
        }
    }
    let Some((file_len, _)) = metadata_identity(&dest) else {
        return Ok(false);
    };
    if file.size.is_some_and(|expected| expected != file_len) {
        return Ok(false);
    }
    let actual_sha256 = if let Some(expected) = &file.sha256 {
        let actual = hash_file(&dest, cancel)?;
        if &actual != expected {
            return Ok(false);
        }
        Some(actual)
    } else {
        None
    };
    write_source_receipt(file, &dest, actual_sha256.as_deref())?;
    Ok(true)
}

/// Cheap startup/readiness check. Preparation performs a first expensive
/// hash when needed and writes a receipt; startup only trusts a receipt whose
/// manifest identity and current length/mtime still match.
pub fn source_file_is_verified(file: &FileSpec, cache_dir: &Path) -> bool {
    let dest = file.dest_path(cache_dir);
    read_receipt(&dest)
        .as_ref()
        .is_some_and(|receipt| source_receipt_matches(file, &dest, receipt))
}

fn conversion_receipt_matches(
    file: &FileSpec,
    output: &Path,
    conversion: &ConversionSpec,
    receipt: &VerificationReceipt,
) -> bool {
    let Some((file_len, modified_ns)) = metadata_identity(output) else {
        return false;
    };
    receipt.version == RECEIPT_VERSION
        && receipt.kind == "conversion"
        && receipt.repo == file.repo
        && receipt.path == file.path
        && receipt.revision == file.resolved_revision()
        && receipt.file_len == file_len
        && receipt.modified_ns == modified_ns
        && file_len == conversion.size
        && receipt.sha256.as_deref() == Some(conversion.sha256.as_str())
        && receipt.source_sha256.as_deref() == file.sha256.as_deref()
        && receipt.converter_id.as_deref() == Some(conversion.converter_id.as_str())
        && receipt.converter_version.as_deref() == Some(conversion.converter_version.as_str())
}

pub fn converted_file_is_verified(file: &FileSpec, cache_dir: &Path) -> bool {
    let Some(conversion) = &file.conversion else {
        return false;
    };
    let Some(output) = file.converted_path(cache_dir) else {
        return false;
    };
    read_receipt(&output).as_ref().is_some_and(|receipt| {
        conversion_receipt_matches(file, &output, conversion, receipt)
    })
}

/// Verify a structured converter output and atomically record its complete
/// source/converter/output provenance. A backend's `prepare_artifacts` hook
/// calls this before it lets a pull job reach Ready.
pub fn verify_converted_file(
    file: &FileSpec,
    cache_dir: &Path,
    cancel: &CancelToken,
) -> Result<PathBuf, AssetAiError> {
    file.conversion.as_ref().ok_or_else(|| {
        AssetAiError::Download(format!("{} has no structured conversion", file.cache_as))
    })?;
    let output = file.converted_path(cache_dir).ok_or_else(|| {
        AssetAiError::Download(format!("{} has no converted output path", file.cache_as))
    })?;
    let artifact_lock = ArtifactLock::acquire(&output, cancel)?;
    verify_converted_file_locked(file, cache_dir, cancel, Some(&artifact_lock))
}

fn verify_converted_file_locked(
    file: &FileSpec,
    cache_dir: &Path,
    cancel: &CancelToken,
    artifact_lock: Option<&ArtifactLock>,
) -> Result<PathBuf, AssetAiError> {
    let conversion = file.conversion.as_ref().ok_or_else(|| {
        AssetAiError::Download(format!("{} has no structured conversion", file.cache_as))
    })?;
    let output = file.converted_path(cache_dir).ok_or_else(|| {
        AssetAiError::Download(format!("{} has no converted output path", file.cache_as))
    })?;
    if let Some(receipt) = read_receipt(&output) {
        if conversion_receipt_matches(file, &output, conversion, &receipt) {
            return Ok(output);
        }
    }
    let len = fs::metadata(&output)
        .map_err(|e| AssetAiError::Download(format!("metadata {}: {e}", output.display())))?
        .len();
    if len != conversion.size {
        return Err(AssetAiError::Download(format!(
            "converted {} is {len} bytes, expected {}",
            output.display(),
            conversion.size
        )));
    }
    let actual = match artifact_lock {
        Some(artifact_lock) => hash_file_with_heartbeat(&output, cancel, artifact_lock)?,
        None => hash_file(&output, cancel)?,
    };
    if actual != conversion.sha256 {
        return Err(AssetAiError::Download(format!(
            "converted {} sha256 mismatch: expected {}, got {actual}",
            output.display(),
            conversion.sha256
        )));
    }
    let (_, modified_ns) = metadata_identity(&output).ok_or_else(|| {
        AssetAiError::Download(format!("metadata {} after conversion", output.display()))
    })?;
    let receipt = VerificationReceipt {
        version: RECEIPT_VERSION,
        kind: "conversion".to_string(),
        repo: file.repo.clone(),
        path: file.path.clone(),
        revision: file.resolved_revision().to_string(),
        file_len: len,
        modified_ns,
        sha256: Some(actual),
        source_sha256: file.sha256.clone(),
        converter_id: Some(conversion.converter_id.clone()),
        converter_version: Some(conversion.converter_version.clone()),
    };
    atomic_write(
        &verification_path(&output),
        receipt.serialize_json().as_bytes(),
    )?;
    Ok(output)
}

/// Run one backend-specific deterministic conversion under the same portable
/// per-output inter-process lock as verification/receipt commit. The source
/// must already have been prepared by `BackendCtx::ensure_files`.
pub fn ensure_converted_file(
    file: &FileSpec,
    cache_dir: &Path,
    cancel: &CancelToken,
    convert: impl FnOnce(&Path, &Path) -> Result<(), AssetAiError>,
) -> Result<PathBuf, AssetAiError> {
    let output = file.converted_path(cache_dir).ok_or_else(|| {
        AssetAiError::Download(format!("{} has no converted output path", file.cache_as))
    })?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| AssetAiError::Download(format!("mkdir {}: {e}", parent.display())))?;
    }
    let artifact_lock = ArtifactLock::acquire(&output, cancel)?;
    if converted_file_is_verified(file, cache_dir) {
        return Ok(output);
    }
    let source = file.dest_path(cache_dir);
    if !source_file_is_verified(file, cache_dir) {
        return Err(AssetAiError::Download(format!(
            "conversion source {} is not prepared/verified",
            source.display()
        )));
    }
    // Never let a structurally plausible but provenance-invalid final make a
    // converter return early. A backend converter writes its own `.part` and
    // atomically commits; the outer lock prevents cross-process collisions.
    if output.exists() {
        fs::remove_file(&output).map_err(|e| {
            AssetAiError::Download(format!("remove stale {}: {e}", output.display()))
        })?;
    }
    let _ = fs::remove_file(verification_path(&output));
    cancel.check()?;
    let heartbeat = ConversionHeartbeat::start(&artifact_lock)?;
    let converted = convert(&source, &output);
    drop(heartbeat);
    converted?;
    cancel.check()?;
    verify_converted_file_locked(file, cache_dir, cancel, Some(&artifact_lock))
}

#[cfg(test)]
mod receipt_tests {
    use super::*;
    use crate::registry::{ConversionSpec, FileSpec};
    use crate::sha256::sha256_hex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "makepad-asset-ai-receipt-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn converted_spec(source_sha: String, output_sha: String, version: &str) -> FileSpec {
        FileSpec {
            role: Some("checkpoint_source".into()),
            repo: "org/repo".into(),
            path: "model.ckpt".into(),
            revision: Some("a".repeat(40)),
            cache_as: "upstream/model.ckpt".into(),
            size: Some(6),
            sha256: Some(source_sha),
            local: false,
            converts_to: None,
            conversion: Some(ConversionSpec {
                cache_as: "native/model.safetensors".into(),
                size: 7,
                sha256: output_sha,
                converter_id: "fixture".into(),
                converter_version: version.into(),
            }),
        }
    }

    #[test]
    fn structured_conversion_receipt_binds_source_converter_and_output() {
        let source = b"source";
        let output = b"weights";
        let cache = temp_dir("converted");
        let spec = converted_spec(sha256_hex(source), sha256_hex(output), "1");
        let source_path = spec.dest_path(&cache);
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        fs::write(&source_path, source).unwrap();
        write_source_receipt(&spec, &source_path, spec.sha256.as_deref()).unwrap();
        let output_path = spec.converted_path(&cache).unwrap();
        fs::create_dir_all(output_path.parent().unwrap()).unwrap();
        fs::write(&output_path, output).unwrap();

        assert!(!converted_file_is_verified(&spec, &cache));
        assert_eq!(
            verify_converted_file(&spec, &cache, &CancelToken::new()).unwrap(),
            output_path
        );
        assert!(converted_file_is_verified(&spec, &cache));

        let changed_converter = converted_spec(sha256_hex(source), sha256_hex(output), "2");
        assert!(!converted_file_is_verified(&changed_converter, &cache));
        let changed_source = converted_spec("11".repeat(32), sha256_hex(output), "1");
        assert!(!converted_file_is_verified(&changed_source, &cache));

        fs::write(&output_path, b"garbage").unwrap();
        assert!(!converted_file_is_verified(&spec, &cache));
    }

    #[test]
    fn conversion_hash_is_cancellable_and_never_writes_receipt() {
        let output = vec![9u8; 256 * 1024];
        let cache = temp_dir("converted-cancel");
        let mut spec = converted_spec("11".repeat(32), sha256_hex(&output), "1");
        spec.conversion.as_mut().unwrap().size = output.len() as u64;
        let output_path = spec.converted_path(&cache).unwrap();
        fs::create_dir_all(output_path.parent().unwrap()).unwrap();
        fs::write(&output_path, output).unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(
            verify_converted_file(&spec, &cache, &cancel).unwrap_err(),
            AssetAiError::Cancelled
        );
        assert!(!verification_path(&output_path).exists());
    }

    #[test]
    fn concurrent_conversion_runs_converter_once() {
        let source = b"source";
        let output = b"weights";
        let cache = temp_dir("converted-lock");
        let spec = converted_spec(sha256_hex(source), sha256_hex(output), "1");
        let source_path = spec.dest_path(&cache);
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        fs::write(&source_path, source).unwrap();
        write_source_receipt(&spec, &source_path, spec.sha256.as_deref()).unwrap();
        let runs = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(3));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let cache = cache.clone();
            let spec = spec.clone();
            let runs = runs.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                ensure_converted_file(
                    &spec,
                    &cache,
                    &CancelToken::new(),
                    |_, destination| {
                        runs.fetch_add(1, Ordering::SeqCst);
                        fs::write(destination, output).map_err(|error| {
                            AssetAiError::Download(format!("fixture convert: {error}"))
                        })
                    },
                )
                .unwrap()
            }));
        }
        barrier.wait();
        for thread in threads {
            assert_eq!(fs::read(thread.join().unwrap()).unwrap(), output);
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(converted_file_is_verified(&spec, &cache));
    }

    #[test]
    fn stale_owner_cleanup_is_compare_safe() {
        let dir = temp_dir("stale-lock").join("artifact.lock");
        fs::create_dir(&dir).unwrap();
        let owner = dir.join("owner-dead-1");
        fs::write(&owner, b"dead").unwrap();
        std::thread::sleep(Duration::from_millis(1));
        assert!(break_stale_lock_older_than(&dir, Duration::ZERO));
        assert!(!dir.exists());

        // Once one waiter has removed the exact old owner, a second waiter
        // cannot remove a newly-created owner's directory using stale state.
        fs::create_dir(&dir).unwrap();
        let fresh = dir.join("owner-live-2");
        fs::write(&fresh, b"live").unwrap();
        assert!(!break_stale_lock_older_than(&dir, Duration::from_secs(3600)));
        assert!(fresh.exists());
    }

    #[test]
    fn active_lock_heartbeat_prevents_stale_theft() {
        let root = temp_dir("active-heartbeat");
        let dest = root.join("artifact");
        let lock = ArtifactLock::acquire(&dest, &CancelToken::new()).unwrap();
        std::thread::sleep(Duration::from_millis(2));
        // With an artificial zero threshold the old, unrefreshed owner is
        // considered stale. A heartbeat refreshes it; use a threshold longer
        // than the tiny post-heartbeat interval to prove it cannot be stolen.
        lock.heartbeat().unwrap();
        assert!(!break_stale_lock_older_than(
            &lock.dir,
            Duration::from_secs(1)
        ));
        assert!(lock.owner.exists());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_url_hf_and_absolute() {
        let downloader = Downloader::new(DEFAULT_HF_BASE, None).unwrap();
        let hf = FileSpec {
            role: None,
            repo: "FacebookAI/roberta-large".into(),
            path: "tokenizer.json".into(),
            revision: None,
            cache_as: "audio/woosh/tokenizer.json".into(),
            size: None,
            sha256: None,
            local: false,
            converts_to: None,
            conversion: None,
        };
        assert_eq!(
            downloader.file_url(&hf),
            "https://huggingface.co/FacebookAI/roberta-large/resolve/main/tokenizer.json"
        );
        // Absolute URLs (GitHub release assets) pass through verbatim.
        let github = FileSpec {
            path: "https://github.com/SonyResearch/Woosh/releases/download/v1.0.0/Woosh-AE.zip"
                .into(),
            ..hf
        };
        assert_eq!(downloader.file_url(&github), github.path);
    }
}
