//! Receiver side of peer-assisted model distribution.
//!
//! `try_fetch_via_peers` runs INSIDE the downloader's per-artifact
//! transaction (artifact lock held, same `.part` file, same verification
//! tail) and tries the coordinator-provided peer list before the canonical
//! Hugging Face path:
//!
//! - Only digest-pinned files are peer-eligible (registry sha256 + size —
//!   there is nothing else a peer transfer could be verified against).
//! - Transfers resume: the existing `.part` prefix is hashed once, then every
//!   chunk streams through the same running SHA-256, so completing a file
//!   costs exactly one pass regardless of how many peers contributed.
//! - The blob endpoint answers bounded chunks (206 + `Content-Range`); the
//!   receiver loops ranged requests, validating offset continuity, the ETag
//!   content address and the declared total size on every response.
//! - A peer that fails (connect refused, HTTP error, mid-chunk EOF, stall)
//!   is dropped and the next peer resumes from the bytes already on disk.
//!   A completed transfer whose digest does not match quarantines the
//!   `.part` (deleted — poisoned bytes must not survive into a resume) and
//!   moves on. When every peer is exhausted the caller falls straight
//!   through to Hugging Face.
//! - Tickets ride in headers, never URLs, and are never logged.

use crate::backend::CancelToken;
use crate::download::DownloadProgress;
use crate::error::AssetAiError;
use crate::http_client::{http_fetch_no_redirect, parse_url, HttpClientRequest};
use crate::peer::{PeerPlan, now_unix};
use crate::registry::FileSpec;
use crate::sha256::{to_hex, Sha256};
use makepad_micro_serde::DeJson;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

/// TCP preflight before trusting a peer URL: a dead/filtered LAN box must
/// cost milliseconds, not a 60s client timeout per artifact.
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(3);
/// Chunk loop stall guard: this many consecutive zero-progress responses
/// drops the peer.
const MAX_STALLED_CHUNKS: u32 = 2;
/// Receiver-owned hard bound, independent of anything a peer advertises in a
/// response. Requests include an explicit range end and responses exceeding
/// it are refused before allocating or reading their body.
pub const MAX_RECEIVE_CHUNK: u64 = 32 * 1024 * 1024;

/// Outcome of the peer phase for one artifact: `Some(sha256)` = the `.part`
/// now holds the complete, digest-verified bytes (caller commits it);
/// `None` = peers could not supply the file — fall back to Hugging Face.
/// Only cancellation propagates as an error.
pub fn try_fetch_via_peers(
    plan: &PeerPlan,
    file: &FileSpec,
    part: &Path,
    progress: &mut dyn FnMut(DownloadProgress),
    cancel: &CancelToken,
    heartbeat: &dyn Fn() -> Result<(), AssetAiError>,
) -> Result<Option<String>, AssetAiError> {
    let (Some(digest), Some(total)) = (file.sha256.as_deref(), file.size) else {
        // Unpinned legacy entries have no digest to verify a peer against.
        return Ok(None);
    };
    if plan.sources.is_empty() {
        return Ok(None);
    }

    // Resume state shared across peers: offset + running hash of the .part.
    let mut offset;
    let mut hasher;
    match hash_part_prefix(part, cancel, heartbeat)? {
        Some((len, prefix_hasher)) if len <= total => {
            offset = len;
            hasher = prefix_hasher;
        }
        Some((len, _)) => {
            eprintln!(
                "peer-fetch {}: partial is {len} bytes, larger than pinned {total} — quarantined",
                file.cache_as
            );
            restore_part_prefix(part, 0)?;
            offset = 0;
            hasher = Sha256::new();
        }
        None => {
            offset = 0;
            hasher = Sha256::new();
        }
    }

    for base in &plan.sources {
        cancel.check()?;
        let Some(source_key) = source_node_key(base) else {
            eprintln!("peer-fetch {}: {base} unreachable — next source", file.cache_as);
            continue;
        };
        if source_key == plan.receiver_key {
            continue; // never fetch from ourselves
        }
        let Some(ticket) = plan.ticket_for(base, &source_key, digest, now_unix()) else {
            eprintln!(
                "peer-fetch {}: no transfer ticket for source {base} — next source",
                file.cache_as
            );
            continue;
        };
        eprintln!(
            "peer-fetch {}: trying {base} from offset {offset}/{total}",
            file.cache_as
        );

        let mut trusted_offset = offset;
        let mut trusted_hasher = hasher.clone();
        let mut stalled = 0u32;
        let mut retried_from_zero = false;
        'chunks: loop {
            cancel.check()?;
            crate::disk_space::check_download(part, Some(total))?;
            let appended = match fetch_one_chunk(
                base, &ticket, plan, file, digest, total, offset, part, progress, cancel,
            ) {
                Ok(ChunkOutcome::Appended { bytes, restarted }) => {
                    if restarted {
                        hasher = Sha256::new();
                        offset = 0;
                        // A 200 response deliberately replaced the old
                        // partial; no later rollback may recreate it by
                        // extending the new file with zeroes.
                        trusted_offset = 0;
                        trusted_hasher = Sha256::new();
                    }
                    bytes
                }
                Ok(ChunkOutcome::PeerFailed(reason)) => {
                    eprintln!(
                        "peer-fetch {}: {base}: {reason} — next source",
                        file.cache_as
                    );
                    restore_part_prefix(part, trusted_offset)?;
                    offset = trusted_offset;
                    hasher = trusted_hasher.clone();
                    break 'chunks;
                }
                Err(e @ (AssetAiError::Cancelled | AssetAiError::Unavailable(_))) => return Err(e),
                Err(e) => {
                    eprintln!("peer-fetch {}: {base}: {e} — next source", file.cache_as);
                    restore_part_prefix(part, trusted_offset)?;
                    offset = trusted_offset;
                    hasher = trusted_hasher.clone();
                    break 'chunks;
                }
            };
            if let Err(error) = heartbeat() {
                // The chunk reached disk before the lock heartbeat failed.
                // Do not let unauthenticated peer bytes survive an aborted
                // transaction and become the next run's resume prefix.
                restore_part_prefix(part, trusted_offset)?;
                return Err(error);
            }
            // The chunk on disk and the running hash advance together.
            hasher.update(&appended);
            offset += appended.len() as u64;
            if appended.is_empty() {
                stalled += 1;
            } else {
                stalled = 0;
            }
            if offset > total {
                eprintln!(
                    "peer-fetch {}: {base} overserved past the pinned size — quarantined",
                    file.cache_as
                );
                restore_part_prefix(part, 0)?;
                offset = 0;
                hasher = Sha256::new();
                break 'chunks;
            }
            if offset == total {
                let actual = to_hex(&hasher.clone().finish());
                if actual == digest {
                    return Ok(Some(actual));
                }
                // Poisoned bytes: quarantine so no later resume (peer OR
                // Hugging Face) builds on them, then try the next source
                // from zero.
                eprintln!(
                    "peer-fetch {}: sha256 mismatch after completing via {base} (got {actual}) — partial quarantined",
                    file.cache_as
                );
                restore_part_prefix(part, 0)?;
                offset = 0;
                hasher = Sha256::new();
                if !retried_from_zero {
                    trusted_offset = 0;
                    trusted_hasher = Sha256::new();
                    retried_from_zero = true;
                    stalled = 0;
                    eprintln!(
                        "peer-fetch {}: retrying {base} once from zero",
                        file.cache_as
                    );
                    continue 'chunks;
                }
                break 'chunks;
            }
            if stalled >= MAX_STALLED_CHUNKS {
                eprintln!(
                    "peer-fetch {}: {base} made no progress — next source",
                    file.cache_as
                );
                break 'chunks;
            }
        }
    }
    Ok(None)
}

enum ChunkOutcome {
    /// One FULL declared chunk appended to the `.part` (written + progress
    /// reported). `restarted` = the server answered 200 (whole file) and the
    /// `.part` was truncated first, so the caller resets its hash BEFORE
    /// folding `bytes` in. Partial chunks NEVER reach disk: a connection
    /// that dies mid-chunk fails the peer with nothing written, so a later
    /// peer can resume from a trustworthy full-chunk boundary instead of
    /// building on (and being blamed for) a dead peer's fragment.
    Appended { bytes: Vec<u8>, restarted: bool },
    /// This peer cannot (or refused to) serve; move on. The `.part` keeps
    /// the prefix it had before this request.
    PeerFailed(String),
}

#[allow(clippy::too_many_arguments)]
fn fetch_one_chunk(
    base: &str,
    ticket: &str,
    plan: &PeerPlan,
    file: &FileSpec,
    digest: &str,
    total: u64,
    offset: u64,
    part: &Path,
    progress: &mut dyn FnMut(DownloadProgress),
    cancel: &CancelToken,
) -> Result<ChunkOutcome, AssetAiError> {
    let url = format!("{base}{}{digest}", crate::peer_serve::BLOB_PATH_PREFIX);
    let requested_end = offset
        .saturating_add(MAX_RECEIVE_CHUNK - 1)
        .min(total.saturating_sub(1));
    let extra_headers = [
        ("Authorization".to_string(), format!("Bearer {ticket}")),
        ("X-Peer-Receiver".to_string(), plan.receiver_key.clone()),
    ];
    let request = HttpClientRequest {
        method: "GET",
        url: &url,
        range_from: Some(offset),
        range_to: Some(requested_end),
        bearer: None,
        body: None,
        extra_headers: &extra_headers,
    };
    let response = http_fetch_no_redirect(&request)?;
    let (chunk_len, restarted) = match response.status {
        206 => {
            let Some((from, to, of)) = response
                .header("content-range")
                .and_then(parse_content_range)
            else {
                return Ok(ChunkOutcome::PeerFailed("206 without a Content-Range".into()));
            };
            if of != total {
                return Ok(ChunkOutcome::PeerFailed(format!(
                    "peer reports size {of}, registry pins {total}"
                )));
            }
            if from != offset || to < from || to >= total || to > requested_end {
                return Ok(ChunkOutcome::PeerFailed(format!(
                    "Content-Range {from}-{to} exceeds requested {offset}-{requested_end}"
                )));
            }
            let len = to - from + 1;
            if len > MAX_RECEIVE_CHUNK {
                return Ok(ChunkOutcome::PeerFailed(format!(
                    "peer chunk {len} exceeds receiver cap {MAX_RECEIVE_CHUNK}"
                )));
            }
            (len, false)
        }
        200 => {
            // Whole-small-file answer; only acceptable when it is exactly
            // the pinned size and we restart the partial from zero.
            match response.content_length() {
                Some(len) if len == total && len <= MAX_RECEIVE_CHUNK => (total, true),
                other => {
                    return Ok(ChunkOutcome::PeerFailed(format!(
                        "200 with length {other:?}, registry pins {total}"
                    )));
                }
            }
        }
        status => {
            return Ok(ChunkOutcome::PeerFailed(format!("http {status}")));
        }
    };
    let expected_etag = format!("\"sha256:{digest}\"");
    if response.header("etag") != Some(expected_etag.as_str()) {
        return Ok(ChunkOutcome::PeerFailed(
            "missing or incorrect content-address ETag".to_string(),
        ));
    }
    if let Some(len) = response.content_length() {
        if len != chunk_len {
            return Ok(ChunkOutcome::PeerFailed(format!(
                "Content-Length {len} disagrees with the served range {chunk_len}"
            )));
        }
    }

    // Buffer the (bounded) chunk fully, then write it in ONE place, so the
    // running hash and the bytes on disk can never diverge and a dead
    // connection leaves the `.part` exactly as it was.
    let mut body = response.body;
    let mut collected = Vec::with_capacity(chunk_len as usize);
    let mut buf = [0u8; 65536];
    while (collected.len() as u64) < chunk_len {
        cancel.check()?;
        let want = ((chunk_len - collected.len() as u64) as usize).min(buf.len());
        match body.read(&mut buf[..want]) {
            Ok(0) => {
                return Ok(ChunkOutcome::PeerFailed(format!(
                    "connection ended {} bytes into a {chunk_len}-byte chunk",
                    collected.len()
                )));
            }
            Ok(n) => collected.extend_from_slice(&buf[..n]),
            Err(e) => {
                return Ok(ChunkOutcome::PeerFailed(format!("body read: {e}")));
            }
        }
    }
    {
        use std::io::Write;
        if !restarted {
            let actual_len = fs::metadata(part).map(|meta| meta.len()).unwrap_or(0);
            if actual_len != offset {
                return Ok(ChunkOutcome::PeerFailed(format!(
                    "partial changed on disk ({actual_len} bytes, expected {offset})"
                )));
            }
        }
        crate::disk_space::check_download(part, Some(total))?;
        let mut out = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(!restarted)
            .truncate(restarted)
            .open(part)
            .map_err(|e| AssetAiError::Download(format!("open {}: {e}", part.display())))?;
        out.write_all(&collected)
            .map_err(|e| AssetAiError::Download(format!("write {}: {e}", part.display())))?;
        out.flush()
            .map_err(|e| AssetAiError::Download(format!("flush {}: {e}", part.display())))?;
    }
    let base_offset = if restarted { 0 } else { offset };
    progress(DownloadProgress {
        file: file.path.clone(),
        done: base_offset + collected.len() as u64,
        total: Some(total),
    });
    Ok(ChunkOutcome::Appended {
        bytes: collected,
        restarted,
    })
}

/// `bytes <from>-<to>/<total>`.
fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let rest = value.trim().strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (from, to) = range.split_once('-')?;
    Some((
        from.trim().parse().ok()?,
        to.trim().parse().ok()?,
        total.trim().parse().ok()?,
    ))
}

/// Hashes an existing `.part` prefix (heartbeating the artifact lock through
/// multi-GB prefixes). `None` when there is no partial.
fn hash_part_prefix(
    part: &Path,
    cancel: &CancelToken,
    heartbeat: &dyn Fn() -> Result<(), AssetAiError>,
) -> Result<Option<(u64, Sha256)>, AssetAiError> {
    let mut file = match fs::File::open(part) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let mut hasher = Sha256::new();
    let mut len = 0u64;
    let mut buf = [0u8; 65536];
    let mut since_heartbeat = 0u64;
    loop {
        cancel.check()?;
        let n = file
            .read(&mut buf)
            .map_err(|e| AssetAiError::Download(format!("read {}: {e}", part.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        len += n as u64;
        since_heartbeat += n as u64;
        if since_heartbeat >= 64 * 1024 * 1024 {
            heartbeat()?;
            since_heartbeat = 0;
        }
    }
    Ok(Some((len, hasher)))
}

/// Restores the exact prefix that existed before trying a peer. Full chunks
/// are not authenticated individually, so bytes from a peer that later fails
/// must never contaminate the next peer or the canonical fallback.
fn restore_part_prefix(part: &Path, len: u64) -> Result<(), AssetAiError> {
    let mut options = fs::OpenOptions::new();
    options.write(true);
    if len == 0 {
        options.create(true);
    }
    let file = match options.open(part) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && len == 0 => return Ok(()),
        Err(error) => {
            return Err(AssetAiError::Download(format!(
                "open {} to roll back failed peer bytes: {error}",
                part.display()
            )))
        }
    };
    file.set_len(len).map_err(|error| {
        AssetAiError::Download(format!(
            "truncate {} to trusted prefix {len}: {error}",
            part.display()
        ))
    })
}

/// Cheap identity probe: TCP preflight, then `GET /health` for the box's
/// durable `node_key` (the ticket source scope). `None` = skip this peer.
fn source_node_key(base: &str) -> Option<String> {
    let url = parse_url(base).ok()?;
    if !url.https {
        use std::net::ToSocketAddrs;
        let addr = (url.host.as_str(), url.port)
            .to_socket_addrs()
            .ok()?
            .next()?;
        std::net::TcpStream::connect_timeout(&addr, PREFLIGHT_TIMEOUT).ok()?;
    }
    let health_url = format!("{base}/health");
    let response = http_fetch_no_redirect(&HttpClientRequest::get(&health_url)).ok()?;
    if response.status != 200 {
        return None;
    }
    let body = response.read_body_to_vec(64 * 1024).ok()?;
    let text = std::str::from_utf8(&body).ok()?;
    let health = crate::protocol::HealthJson::deserialize_json_lenient(text).ok()?;
    let node_key = health.node_key?;
    (node_key.len() == 32
        && node_key
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
    .then_some(node_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_parsing() {
        assert_eq!(
            parse_content_range("bytes 0-99/1000"),
            Some((0, 99, 1000))
        );
        assert_eq!(
            parse_content_range(" bytes 5-5/6 "),
            Some((5, 5, 6))
        );
        assert_eq!(parse_content_range("bytes */1000"), None);
        assert_eq!(parse_content_range("chairs 0-1/2"), None);
    }
}
