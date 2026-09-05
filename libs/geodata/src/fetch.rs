//! Polite bulk downloader.
//!
//! Rules, enforced here so no layer module can violate them:
//! - bulk file downloads only — never API paging, never tile scraping
//! - one transfer at a time, with a fixed pause after every network hit
//! - descriptive User-Agent with a contact address
//! - a source is never re-contacted while the cached copy is younger than its
//!   `recheck_days`; after that we revalidate with If-Modified-Since so an
//!   unchanged file costs the server a 304 and no bytes
//! - interrupted downloads resume (`.part` + curl `-C -`)
//!
//! Downloads shell out to `curl` (retries, TLS, resume for free). The same
//! functions are usable from the maps app later for live-ish sources: call
//! `fetch_source` with the source's `recheck_days` (e.g. 1-2 days for the NDW
//! charger file) and it does the right thing.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

pub const USER_AGENT: &str =
    "makepad-geodata/0.1 (bulk open-data fetcher; contact: rik@n4.io)";
const PAUSE_AFTER_TRANSFER: Duration = Duration::from_secs(1);

/// A bulk-downloadable source file. All fields static: the registry is code.
#[derive(Debug, Clone, Copy)]
pub struct SourceSpec {
    pub id: &'static str,
    pub url: &'static str,
    /// Filename inside the cache directory.
    pub filename: &'static str,
    pub license: &'static str,
    pub attribution: &'static str,
    /// Do not even revalidate more often than this.
    pub recheck_days: u32,
    /// Optional curl --limit-rate value (e.g. "10M") for small origin servers.
    pub limit_rate: Option<&'static str>,
}

pub struct FetchOptions {
    pub cache_dir: PathBuf,
    pub force: bool,
}

#[derive(Debug)]
pub enum FetchOutcome {
    /// Cache is fresh; no network contact was made.
    CachedFresh(PathBuf),
    /// Revalidated; server said unchanged.
    NotModified(PathBuf),
    /// Downloaded (new or updated).
    Downloaded(PathBuf),
}

impl FetchOutcome {
    pub fn path(&self) -> &Path {
        match self {
            FetchOutcome::CachedFresh(p)
            | FetchOutcome::NotModified(p)
            | FetchOutcome::Downloaded(p) => p,
        }
    }
}

fn meta_path(cache_dir: &Path, spec: &SourceSpec) -> PathBuf {
    cache_dir.join(format!("{}.meta.json", spec.filename))
}

fn read_fetched_unix(cache_dir: &Path, spec: &SourceSpec) -> Option<u64> {
    let text = std::fs::read_to_string(meta_path(cache_dir, spec)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("fetched_unix")?.as_u64()
}

fn write_meta(cache_dir: &Path, spec: &SourceSpec, bytes: u64) -> std::io::Result<()> {
    let meta = serde_json::json!({
        "id": spec.id,
        "url": spec.url,
        "license": spec.license,
        "attribution": spec.attribution,
        "fetched_unix": now_unix(),
        "bytes": bytes,
    });
    let mut file = std::fs::File::create(meta_path(cache_dir, spec))?;
    file.write_all(serde_json::to_string_pretty(&meta).unwrap().as_bytes())
}

fn now_unix() -> u64 {
    crate::clock::now_unix()
}

/// Fetch one source into the cache directory, politely. Returns the cached
/// file path. Never contacts the network when the cached copy is fresh.
pub fn fetch_source(opts: &FetchOptions, spec: &SourceSpec) -> std::io::Result<FetchOutcome> {
    std::fs::create_dir_all(&opts.cache_dir)?;
    let dest = opts.cache_dir.join(spec.filename);
    let part = opts.cache_dir.join(format!("{}.part", spec.filename));

    if dest.exists() && !opts.force {
        let fetched = read_fetched_unix(&opts.cache_dir, spec)
            .or_else(|| {
                dest.metadata()
                    .ok()?
                    .modified()
                    .ok()?
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs())
            })
            .unwrap_or(0);
        let age_days = (now_unix().saturating_sub(fetched)) / 86_400;
        if age_days < u64::from(spec.recheck_days) {
            return Ok(FetchOutcome::CachedFresh(dest));
        }
    }

    let mut cmd = Command::new("curl");
    cmd.arg("-fsSL")
        .arg("--retry")
        .arg("3")
        .arg("--retry-delay")
        .arg("2")
        .arg("--connect-timeout")
        .arg("20")
        .arg("-A")
        .arg(USER_AGENT)
        .arg("-o")
        .arg(&part);
    if let Some(rate) = spec.limit_rate {
        cmd.arg("--limit-rate").arg(rate);
    }
    if dest.exists() && !opts.force {
        // Revalidate: only transfer if newer than what we have.
        cmd.arg("-z").arg(&dest);
    } else if part.exists() {
        // Resume an interrupted download.
        cmd.arg("-C").arg("-");
    }
    cmd.arg(spec.url);

    let status = cmd.status()?;
    std::thread::sleep(PAUSE_AFTER_TRANSFER);
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "curl failed for {} ({})",
            spec.url, status
        )));
    }

    let part_len = part.metadata().map(|m| m.len()).unwrap_or(0);
    if part_len == 0 && dest.exists() {
        // 304 Not Modified: curl -z left us an empty output file.
        let _ = std::fs::remove_file(&part);
        write_meta(&opts.cache_dir, spec, dest.metadata()?.len())?;
        return Ok(FetchOutcome::NotModified(dest));
    }
    if part_len == 0 {
        return Err(std::io::Error::other(format!(
            "download of {} produced no data",
            spec.url
        )));
    }
    std::fs::rename(&part, &dest)?;
    write_meta(&opts.cache_dir, spec, part_len)?;
    Ok(FetchOutcome::Downloaded(dest))
}
