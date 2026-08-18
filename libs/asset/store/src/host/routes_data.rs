//! Data-plane routes: blob upload/download and thumbnails — the byte-heavy
//! traffic, kept off the control plane so a saturated download cannot starve
//! publication or the worker protocol.
//!
//! Serving discipline: bytes always come out of `AssetServerCore::read_blob`,
//! which verifies the full digest before anything is emitted (fail-closed;
//! a corrupt object serves nothing, never a prefix). Range and conditional
//! handling then slice the verified buffer:
//!   - strong ETags only — the content address itself, a perfect validator
//!   - `If-None-Match` -> 304, `If-Range` mismatch -> full 200
//!   - single byte ranges -> 206 with `Content-Range`; unsatisfiable -> 416
//!   - multi-range and malformed `Range` headers are ignored (full 200),
//!     as RFC 9110 permits — never guessed at

use super::api::{thumb_content_type, Fail, RouteResult};
use super::http::{
    etag_matches, if_range_matches, parse_range, Conn, Head, Method, RangeSpec, Resp,
};
use super::json::{obj, s, Value};
use super::routes::{require_cap,
    body_err_outcome, call_state, is_read, method_not_allowed, not_found, secret_of, Outcome,
    RouteCtx,
};
use super::util::now_ms;
use crate::{CandidateState, Capability, ServerError};
use makepad_asset_data::{AssetAlias, AssetManifest, AssetRevisionId, BlobId, ThumbnailMedia};
use std::time::{Duration, Instant};

const CACHE_IMMUTABLE: &str = "private, max-age=31536000, immutable";
/// Socket-facing copy chunk; independent of the CAS io chunk budget.
const STREAM_CHUNK: usize = 64 * 1024;

pub fn dispatch(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    force_close: bool,
) -> RouteResult<Outcome> {
    let segs = head.segs.clone();
    let seg: Vec<&str> = segs.iter().map(String::as_str).collect();
    let m = head.method;
    match seg.as_slice() {
        // Health answers on both planes so a discovered endpoint pair can be
        // probed symmetrically.
        ["v1", "health"] => {
            if is_read(m) {
                super::routes_control::health(rc)
            } else {
                method_not_allowed()
            }
        }
        ["v1", "blobs"] if m == Method::Post => blob_upload(conn, head, rc),
        ["v1", "blobs", b] if is_read(m) => {
            let id: BlobId = b.parse().map_err(|_| Fail::Http(400, "malformed blob id"))?;
            blob_get(conn, head, rc, force_close, id)
        }
        ["v1", "thumbnails", "alias", rest @ ..] if is_read(m) && !rest.is_empty() => {
            let alias = AssetAlias::new(rest.join("/"))
                .map_err(|_| Fail::Http(400, "malformed alias"))?;
            thumbnail_by_alias(conn, head, rc, force_close, alias)
        }
        ["v1", "thumbnails", "revision", r] if is_read(m) => {
            let rev: AssetRevisionId =
                r.parse().map_err(|_| Fail::Http(400, "malformed asset revision"))?;
            thumbnail_by_revision(conn, head, rc, force_close, rev)
        }
        _ => not_found(),
    }
}

// ---------------------------------------------------------------------------
// upload
// ---------------------------------------------------------------------------

fn blob_upload(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    // The namespace names the authorization context for this write; the blob
    // itself is content-addressed and deduplicated globally.
    let ns = head
        .query_get("ns")
        .ok_or(Fail::Http(400, "missing ns"))?
        .to_string();
    let expected: Option<BlobId> = match head.query_get("sha256") {
        None => None,
        Some(h) => Some(
            format!("sha256:{h}")
                .parse()
                .map_err(|_| Fail::Http(400, "malformed sha256"))?,
        ),
    };
    // Authorize BEFORE consuming the body: an unauthorized uploader costs
    // one head, not a quarter-gigabyte stream.
    let now = now_ms();
    let mut writer = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::BlobWrite, &ns)?;
        ctx.core.begin_blob()
    })?;
    // Stream the body straight into the hashing writer on this connection
    // thread; the state thread is only touched again to commit.
    let deadline = Instant::now() + Duration::from_millis(rc.cfg.data_body_deadline_ms.max(1));
    let max = rc.cfg.budgets.max_blob_bytes;
    let mut buf = vec![0u8; STREAM_CHUNK];
    loop {
        let n = match conn.body_read(head, &mut buf, max, deadline) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return Ok(body_err_outcome(e)),
        };
        writer.write(&buf[..n])?;
    }
    let commit_now = now_ms();
    let commit = call_state(&rc.state, move |ctx| {
        ctx.core.commit_blob(writer, expected, commit_now)
    })?;
    Ok(Outcome::Resp(Resp::json(
        201,
        &obj(vec![
            ("blob_id", s(commit.blob_id.to_string())),
            ("size", Value::Int(commit.size as i64)),
            ("deduped", Value::Bool(commit.deduped)),
        ]),
    )))
}

// ---------------------------------------------------------------------------
// verified byte serving (shared by blobs and thumbnails)
// ---------------------------------------------------------------------------

fn serve_bytes(
    conn: &mut Conn,
    head: &Head,
    force_close: bool,
    bytes: &[u8],
    content_type: &str,
    etag: String,
) -> RouteResult<Outcome> {
    if let Some(inm) = &head.if_none_match {
        if etag_matches(inm, &etag) {
            return Ok(Outcome::Resp(
                Resp::empty(304)
                    .with_header("ETag", etag)
                    .with_header("Cache-Control", CACHE_IMMUTABLE.to_string()),
            ));
        }
    }
    let size = bytes.len() as u64;
    // A Range only applies when If-Range is absent or validates against our
    // strong ETag; otherwise the full representation is served.
    let range = match &head.range {
        None => RangeSpec::None,
        Some(_) => {
            let honored = match &head.if_range {
                None => true,
                Some(ir) => if_range_matches(ir, &etag),
            };
            if honored {
                parse_range(head.range.as_deref(), size)
            } else {
                RangeSpec::None
            }
        }
    };
    let (status, slice, content_range) = match range {
        RangeSpec::Unsatisfiable => {
            return Ok(Outcome::Resp(
                Resp::error(416, "range not satisfiable")
                    .with_header("Content-Range", format!("bytes */{size}"))
                    .with_header("ETag", etag),
            ));
        }
        RangeSpec::None => (200, bytes, None),
        // `Single` implies a non-empty representation and in-bounds
        // positions by `parse_range` construction.
        RangeSpec::Single { start, end } => (
            206,
            &bytes[start as usize..=end as usize],
            Some(format!("bytes {start}-{end}/{size}")),
        ),
    };
    let close = force_close || head.close;
    let mut headers: Vec<(&'static str, String)> = vec![
        ("Content-Type", content_type.to_string()),
        ("ETag", etag),
        ("Accept-Ranges", "bytes".to_string()),
        ("Cache-Control", CACHE_IMMUTABLE.to_string()),
    ];
    if let Some(cr) = content_range {
        headers.push(("Content-Range", cr));
    }
    if conn
        .write_stream_head(status, &headers, slice.len() as u64, close)
        .is_err()
    {
        return Ok(Outcome::Hangup);
    }
    if head.method != Method::Head {
        for chunk in slice.chunks(STREAM_CHUNK) {
            if conn.write_chunk(chunk).is_err() {
                return Ok(Outcome::Hangup);
            }
        }
    }
    Ok(Outcome::Streamed { close })
}

fn blob_get(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    force_close: bool,
    id: BlobId,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let bytes = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        // Catalog-gated and digest-verified in full before a byte returns.
        ctx.core.read_blob(&id)
    })?;
    serve_bytes(
        conn,
        head,
        force_close,
        &bytes,
        "application/octet-stream",
        format!("\"{id}\""),
    )
}

// ---------------------------------------------------------------------------
// thumbnails
// ---------------------------------------------------------------------------

fn thumbnail_by_alias(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    force_close: bool,
    alias: AssetAlias,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let (bytes, media, blob) = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        // Aliases only ever point at published revisions (the catalog
        // maintains that, dropping heads on quarantine), so resolving is the
        // whole liveness check.
        let target = ctx
            .core
            .catalog()
            .resolve_asset_alias(&alias)?
            .ok_or(ServerError::NotFound { what: "alias" })?;
        thumbnail_of(ctx, &target.revision)
    })?;
    serve_bytes(conn, head, force_close, &bytes, thumb_content_type(media), format!("\"{blob}\""))
}

fn thumbnail_by_revision(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    force_close: bool,
    rev: AssetRevisionId,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let (bytes, media, blob) = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        // Revision-addressed reads must not serve pulled content.
        let manifest_bytes = ctx
            .core
            .catalog()
            .asset_revision_manifest(&rev)?
            .ok_or(ServerError::NotFound { what: "manifest" })?;
        let manifest = AssetManifest::from_canonical_bytes(&manifest_bytes)?;
        match ctx.core.catalog().asset_candidate_state(&manifest.asset_id, &rev)? {
            Some(CandidateState::Quarantined) | None => {
                return Err(ServerError::NotFound { what: "manifest" });
            }
            Some(_) => {}
        }
        thumbnail_of(ctx, &rev)
    })?;
    serve_bytes(conn, head, force_close, &bytes, thumb_content_type(media), format!("\"{blob}\""))
}

/// Resolve a revision's mandatory-typed thumbnail into verified bytes.
fn thumbnail_of(
    ctx: &super::state::StateCtx,
    rev: &AssetRevisionId,
) -> Result<(Vec<u8>, ThumbnailMedia, BlobId), ServerError> {
    let manifest_bytes = ctx
        .core
        .catalog()
        .asset_revision_manifest(rev)?
        .ok_or(ServerError::NotFound { what: "manifest" })?;
    let manifest = AssetManifest::from_canonical_bytes(&manifest_bytes)?;
    let thumb = manifest
        .thumbnail
        .ok_or(ServerError::NotFound { what: "thumbnail" })?;
    let bytes = ctx.core.read_blob(&thumb.blob)?;
    Ok((bytes, thumb.media, thumb.blob))
}
