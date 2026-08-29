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
use std::sync::Arc;
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
        // Ordered batch pull. Must be matched BEFORE `["v1","blobs",b]` so
        // `fetch` is never parsed as a (malformed) blob id.
        ["v1", "blobs", "fetch"] if m == Method::Post => blob_batch(conn, head, rc, force_close),
        // Bulk upload: many blobs in ONE request, one catalog transaction.
        ["v1", "blobs", "batch"] if m == Method::Post => blob_upload_batch(conn, head, rc),
        // Admit a server-local file BY REFERENCE. Matched before the blob-id
        // arm for the same reason `fetch` is.
        ["v1", "blobs", "ref"] if m == Method::Post => blob_ref_admit(conn, head, rc),
        ["v1", "model-preview-sessions", session, "parts", part] if m == Method::Put => {
            model_preview_part_put(conn, head, rc, session, part)
        }
        ["v1", "model-preview-meshes", token] if m == Method::Get => {
            model_preview_mesh_get(head, rc, token)
        }
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

fn preview_session_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_'))
}

fn preview_part_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 24
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn preview_mesh_token_ok(value: &str) -> bool {
    value
        .strip_prefix("pmesh_")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
}

/// Store one changed part in the bounded event hub and emit its delta. The
/// body is intentionally never admitted to CAS or written to a temporary
/// file: preview transport is process-memory-only.
fn model_preview_part_put(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    session: &str,
    part: &str,
) -> RouteResult<Outcome> {
    if !preview_session_ok(session) {
        return Err(Fail::Http(400, "malformed model preview session"));
    }
    if !preview_part_ok(part) {
        return Err(Fail::Http(400, "malformed model preview part"));
    }
    let secret = secret_of(head)?;
    let session = session.to_string();
    let part = part.to_string();
    let namespace = rc
        .events
        .model_preview_namespace(&session)
        .ok_or(Fail::Http(404, "model preview session not found"))?;
    let now = now_ms();
    call_state(&rc.state, move |ctx| {
        let principal = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &principal, Capability::AssetPublish, &namespace)
    })?;
    let max = rc.cfg.budgets.max_blob_bytes.min(16 * 1024 * 1024);
    let bytes = match super::routes::read_body(conn, head, max, rc.cfg.data_body_deadline_ms) {
        Ok(bytes) => bytes,
        Err(outcome) => return Ok(outcome),
    };
    if bytes.is_empty() {
        return Err(Fail::Http(400, "empty model preview mesh"));
    }
    let token = format!("pmesh_{}", makepad_asset_data::sha256_hex(&bytes));
    rc.events
        .update_model_preview_part(&session, part, token.clone(), Arc::from(bytes), now_ms())
        .map_err(|what| ServerError::Conflict { what })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("mesh_token", s(token))]),
    )))
}

fn model_preview_mesh_get(head: &Head, rc: &RouteCtx, token: &str) -> RouteResult<Outcome> {
    if !preview_mesh_token_ok(token) {
        return Err(Fail::Http(400, "malformed model preview mesh token"));
    }
    let secret = secret_of(head)?;
    let now = now_ms();
    call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        Ok(())
    })?;
    let bytes = rc
        .events
        .model_preview_mesh(token)
        .ok_or(Fail::Http(404, "model preview mesh not found"))?;
    Ok(Outcome::Resp(Resp::bytes(
        200,
        "model/gltf-binary",
        bytes.as_ref().to_vec(),
    )))
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
// bulk upload (many blobs, one request, one catalog transaction)
// ---------------------------------------------------------------------------

/// Most blobs one upload batch may carry. Sized for a publish page (each
/// asset contributes a couple of small blobs); big media still travels one
/// blob per request.
pub const UPLOAD_BATCH_MAX_ITEMS: usize = 64;

/// `POST /v1/blobs/batch?ns=<ns>` — admit MANY blobs in one request.
///
/// Body framing: repeated `length(8, big-endian) | bytes[length]`. The
/// response answers `{"blobs": [{"blob_id", "size", "deduped"}, …]}` in
/// request order.
///
/// Why it exists: bulk publication (a compiled-in preset library seeding a
/// virgin store) used to pay one round trip AND one catalog commit per blob.
/// Here the connection thread hashes, fsyncs and renames every object into
/// the CAS itself — off the single state thread — and then ONE state-thread
/// visit records the whole set in ONE catalog transaction. The admission
/// ordering law is untouched: every byte is durable in the CAS before any
/// catalog row exists; a crash in between leaves only harmless unrecorded
/// objects that a retry dedups against.
fn blob_upload_batch(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let ns = head
        .query_get("ns")
        .ok_or(Fail::Http(400, "missing ns"))?
        .to_string();
    // Authorize BEFORE consuming the body: an unauthorized uploader costs
    // one head, not the whole batch stream.
    let now = now_ms();
    let auth_secret = secret.clone();
    let auth_ns = ns.clone();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(auth_secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::BlobWrite, &auth_ns)
    })?;
    let body = match super::routes::read_body(
        conn,
        head,
        rc.cfg.batch_max_bytes,
        rc.cfg.data_body_deadline_ms,
    ) {
        Ok(b) => b,
        Err(o) => return Ok(o),
    };
    // Parse the framing completely before touching the CAS.
    let mut frames: Vec<&[u8]> = Vec::new();
    let mut at = 0usize;
    while at < body.len() {
        if body.len() - at < 8 {
            return Err(Fail::Http(400, "malformed batch framing"));
        }
        let len = u64::from_be_bytes(body[at..at + 8].try_into().expect("8 bytes"));
        at += 8;
        if len == 0 || len > rc.cfg.budgets.max_blob_bytes {
            return Err(Fail::Http(400, "batch blob length"));
        }
        let end = at
            .checked_add(len as usize)
            .filter(|end| *end <= body.len())
            .ok_or(Fail::Http(400, "malformed batch framing"))?;
        frames.push(&body[at..end]);
        at = end;
    }
    if frames.is_empty() {
        return Err(Fail::Http(400, "empty batch"));
    }
    if frames.len() > UPLOAD_BATCH_MAX_ITEMS {
        return Err(Fail::Http(400, "batch too large"));
    }
    // CAS admission off the state thread, and PARALLEL: each object costs
    // two fsyncs (temp file + directory entry), ~10ms of pure disk barrier
    // on a laptop — serialized, a 64-blob page is most of a second of
    // nothing but fsync. A few workers overlap those barriers; every byte
    // is still durable before the catalog transaction below records it.
    let mut commits: Vec<Option<crate::BlobCommit>> = Vec::new();
    commits.resize_with(frames.len(), || None);
    {
        const CAS_BATCH_WORKERS: usize = 8;
        let per = frames.len().div_ceil(frames.len().min(CAS_BATCH_WORKERS));
        let mut first_err: Option<ServerError> = None;
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for (frame_chunk, out_chunk) in frames.chunks(per).zip(commits.chunks_mut(per)) {
                let cas = rc.cas.clone();
                handles.push(scope.spawn(move || -> Result<(), ServerError> {
                    for (bytes, slot) in frame_chunk.iter().zip(out_chunk.iter_mut()) {
                        let mut writer = cas.begin()?;
                        writer.write(bytes)?;
                        *slot = Some(cas.commit(writer, None)?);
                    }
                    Ok(())
                }));
            }
            for handle in handles {
                let outcome = handle.join().unwrap_or(Err(ServerError::InvalidState {
                    what: "cas batch worker",
                    state: "panicked",
                }));
                if let Err(e) = outcome {
                    first_err.get_or_insert(e);
                }
            }
        });
        if let Some(e) = first_err {
            return Err(Fail::Srv(e));
        }
    }
    let commits: Vec<crate::BlobCommit> = commits
        .into_iter()
        .map(|c| c.expect("every slot filled on success"))
        .collect();
    // ONE state-thread visit, ONE catalog transaction for every record.
    let rows: Vec<(BlobId, u64)> = commits.iter().map(|c| (c.blob_id, c.size)).collect();
    let record_now = now_ms();
    call_state(&rc.state, move |ctx| {
        ctx.core.record_blobs(&rows, record_now)
    })?;
    let blobs: Vec<Value> = commits
        .iter()
        .map(|c| {
            obj(vec![
                ("blob_id", s(c.blob_id.to_string())),
                ("size", Value::Int(c.size as i64)),
                ("deduped", Value::Bool(c.deduped)),
            ])
        })
        .collect();
    Ok(Outcome::Resp(Resp::json(
        201,
        &obj(vec![("blobs", Value::Arr(blobs))]),
    )))
}

// ---------------------------------------------------------------------------
// admission by reference (the store catalogues bytes it does not copy)
// ---------------------------------------------------------------------------

/// `POST /v1/blobs/ref?ns=<ns>` with `{"path": "/abs/path/to/file"}`.
///
/// The server hashes that file WHERE IT LIES and records it as a reference
/// blob: catalogued, addressable, servable, never copied. The answer is the
/// same shape an upload gives — `{blob_id, size, deduped}` — plus `owned`,
/// which says the store already had these exact bytes in its own CAS and so
/// recorded no reference at all.
///
/// Four gates, in cost order, before a single byte is read:
/// 1. the policy must be ON (otherwise the route does not exist: 404),
/// 2. the peer must be loopback when the policy says so — this is a
///    file-read privilege and it does not leave the machine,
/// 3. the caller must hold `BlobWrite` in the named namespace,
/// 4. the path must sit under the policy's prefix allowlist (if any).
fn blob_ref_admit(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let policy = rc.cfg.blob_refs.clone();
    if !policy.enabled {
        // Indistinguishable from "no such route" on purpose: a server that
        // does not do this should not advertise that it could.
        return not_found();
    }
    if policy.loopback_only && !conn.peer_is_loopback() {
        return Err(Fail::Http(403, "reference import is loopback-only"));
    }
    let secret = secret_of(head)?;
    let ns = head
        .query_get("ns")
        .ok_or(Fail::Http(400, "missing ns"))?
        .to_string();
    let bytes = match super::routes::read_body(
        conn,
        head,
        rc.cfg.max_json_body_bytes,
        rc.cfg.control_body_deadline_ms,
    ) {
        Ok(b) => b,
        Err(o) => return Ok(o),
    };
    let body = super::api::parse_json_body(&bytes)?;
    let path = body
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or(Fail::Http(400, "missing path"))?
        .to_string();
    if path.is_empty() || path.len() > crate::blobrefs::MAX_REF_PATH_BYTES {
        return Err(Fail::Http(400, "path length"));
    }
    let now = now_ms();
    let commit = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::BlobWrite, &ns)?;
        // Make it absolute BEFORE the allowlist check, or `../` would walk
        // straight out of an allowed root.
        let abs = crate::blobrefs::absolute_path(std::path::Path::new(&path))?;
        if !policy.path_allowed(&abs) {
            return Err(ServerError::Denied { capability: "blob_ref_path" });
        }
        ctx.core.put_blob_ref(&abs, now)
    })?;
    Ok(Outcome::Resp(Resp::json(
        201,
        &obj(vec![
            ("blob_id", s(commit.blob_id.to_string())),
            ("size", Value::Int(commit.size as i64)),
            ("deduped", Value::Bool(commit.deduped)),
            ("owned", Value::Bool(commit.owned)),
        ]),
    )))
}

// ---------------------------------------------------------------------------
// ordered batch pull
// ---------------------------------------------------------------------------

/// Frame status bytes. A frame ALWAYS appears for every requested blob, in
/// the requested order, so a client can match frames to its own queue
/// positionally and never has to guess what happened to an item.
pub const BATCH_OK: u8 = 0;
/// The store does not hold it (or it is not readable).
pub const BATCH_MISSING: u8 = 1;
/// Larger than the caller's declared per-item cap.
pub const BATCH_OVER_ITEM_CAP: u8 = 2;
/// The batch byte budget ran out before this item.
pub const BATCH_SKIPPED: u8 = 3;

/// Media type of the framed batch body. Versioned in the name: a client that
/// does not recognise it must not try to parse frames.
pub const BATCH_CONTENT_TYPE: &str = "application/vnd.makepad.blob-batch.v1";

/// `POST /v1/blobs/fetch` — pull many blobs in ONE request, in the order the
/// caller asked for.
///
/// Why it exists: a thumbnail grid asks for thirty small blobs. Thirty
/// round-trips (even keep-alive ones) pay per-request latency thirty times
/// and, worse, give the caller no way to say what matters FIRST. Here the
/// order of the request list is the order of the response bytes, so a client
/// that re-prioritises (the visible row changed) simply drops the connection
/// and asks again — everything already framed is already committed on its
/// side.
///
/// Framing: `status(1) | digest(32) | length(8, big-endian) | bytes[length]`
/// per item, with `length = 0` on every non-OK status, under an exact
/// `Content-Length` (this stack never chunks). Bytes come from the same
/// verified read path single GETs use.
fn blob_batch(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    force_close: bool,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let bytes = match super::routes::read_body(
        conn,
        head,
        rc.cfg.max_json_body_bytes,
        rc.cfg.control_body_deadline_ms,
    ) {
        Ok(b) => b,
        Err(o) => return Ok(o),
    };
    let body = super::api::parse_json_body(&bytes)?;
    let items = body
        .get("blobs")
        .and_then(Value::as_arr)
        .ok_or(Fail::Http(400, "missing blobs array"))?;
    if items.is_empty() {
        return Err(Fail::Http(400, "empty batch"));
    }
    if items.len() > rc.cfg.batch_max_items as usize {
        return Err(Fail::Http(400, "batch too large"));
    }
    let default_cap = match body.get("max_bytes") {
        None => rc.cfg.batch_max_bytes,
        Some(v) => v.as_u64().ok_or(Fail::Http(400, "malformed max_bytes"))?,
    };
    // Parse before touching the store: a malformed list costs nothing.
    let mut wanted: Vec<(BlobId, u64)> = Vec::with_capacity(items.len());
    for item in items {
        let (id_text, cap) = match item {
            Value::Str(t) => (t.as_str(), default_cap),
            Value::Obj(_) => {
                let t = item
                    .get("blob")
                    .and_then(Value::as_str)
                    .ok_or(Fail::Http(400, "malformed batch item"))?;
                let cap = match item.get("max_bytes") {
                    None => default_cap,
                    Some(v) => v.as_u64().ok_or(Fail::Http(400, "malformed max_bytes"))?,
                };
                (t, cap)
            }
            _ => return Err(Fail::Http(400, "malformed batch item")),
        };
        let id: BlobId = id_text.parse().map_err(|_| Fail::Http(400, "malformed blob id"))?;
        wanted.push((id, cap.min(rc.cfg.batch_max_bytes)));
    }

    // Plan the whole response first: the exact Content-Length has to be
    // known before a byte goes out, and planning on the state thread means
    // one authenticate + one size lookup per item, no reads yet.
    let budget = rc.cfg.batch_max_bytes;
    let plan_items = wanted.clone();
    let now = now_ms();
    let plan = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let mut out: Vec<(BlobId, u8, u64)> = Vec::with_capacity(plan_items.len());
        let mut total = 0u64;
        for (id, cap) in &plan_items {
            let mut size = ctx.core.catalog().blob_size(id)?;
            // A REFERENCE blob's bytes live in a file the store does not
            // own, so the recorded size is a claim about someone else's
            // disk. The frame commits to a length before any read, and a
            // wrong length would force a mid-stream hangup — so re-stat the
            // file here (one cheap syscall, no hashing) and report a
            // vanished or resized one as MISSING, which the client already
            // knows how to handle. Content drift still surfaces later, at
            // the read, as a refusal to serve.
            if size.is_some() {
                if let Some(entry) = ctx.core.blob_ref_of(id)? {
                    let live = std::fs::metadata(&entry.path)
                        .ok()
                        .filter(|m| m.is_file())
                        .map(|m| m.len());
                    if live != Some(entry.size) {
                        size = None;
                    }
                }
            }
            let (status, len) = match size {
                None => (BATCH_MISSING, 0),
                Some(size) if size > *cap => (BATCH_OVER_ITEM_CAP, 0),
                Some(size) if total.saturating_add(size) > budget => (BATCH_SKIPPED, 0),
                Some(size) => {
                    total += size;
                    (BATCH_OK, size)
                }
            };
            out.push((*id, status, len));
        }
        Ok(out)
    })?;

    let content_length: u64 = plan.iter().map(|(_, _, len)| 41 + *len).sum();
    let close = force_close || head.close;
    let headers: Vec<(&'static str, String)> = vec![
        ("Content-Type", BATCH_CONTENT_TYPE.to_string()),
        ("Cache-Control", "private, no-store".to_string()),
    ];
    if conn.write_stream_head(200, &headers, content_length, close).is_err() {
        return Ok(Outcome::Hangup);
    }
    if head.method == Method::Head {
        return Ok(Outcome::Streamed { close });
    }
    for (id, status, len) in &plan {
        let mut frame = Vec::with_capacity(41);
        frame.push(*status);
        frame.extend_from_slice(id.as_bytes());
        frame.extend_from_slice(&len.to_be_bytes());
        if conn.write_chunk(&frame).is_err() {
            return Ok(Outcome::Hangup);
        }
        if *status != BATCH_OK {
            continue;
        }
        // One blob in memory at a time, read through the same digest-verified
        // path a single GET uses.
        let blob = *id;
        let bytes = call_state(&rc.state, move |ctx| ctx.core.read_blob(&blob))?;
        if bytes.len() as u64 != *len {
            // The catalog size and the object disagree: the frame is already
            // committed to a length, so the only honest move is to drop the
            // connection rather than emit a lie.
            return Ok(Outcome::Hangup);
        }
        for chunk in bytes.chunks(STREAM_CHUNK) {
            if conn.write_chunk(chunk).is_err() {
                return Ok(Outcome::Hangup);
            }
        }
    }
    Ok(Outcome::Streamed { close })
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
