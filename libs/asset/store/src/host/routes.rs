//! Shared route plumbing: the dispatch entry point both planes use, the
//! request outcome type, state-thread call bridging, and body helpers.
//!
//! Authorization discipline: every route that touches the core does its
//! authenticate + require + operate sequence inside ONE state-thread closure.
//! The state thread runs calls to completion in order, so a check can never
//! be separated from its act by a concurrent revocation.

use super::api::{fail_closes, fail_resp, Fail, RouteResult};
use super::config::ServerConfig;
use super::http::{BodyError, Conn, Head, Method, Resp};
use super::state::{StateCtx, StateHandle};
use super::util::log;
use crate::{ServerError, ServerResult};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plane {
    Control,
    Data,
}

/// What one request produced.
pub enum Outcome {
    /// Response to be framed and written by the connection loop.
    Resp(Resp),
    /// The handler already wrote the whole response (streamed body); `close`
    /// says whether it announced `Connection: close`.
    Streamed { close: bool },
    /// The connection is unusable (socket failure mid body or mid stream);
    /// close silently.
    Hangup,
}

/// Everything a route handler may reach, shared per worker thread.
#[derive(Clone)]
pub struct RouteCtx {
    /// Requests served on this plane since start. Paired with the accept
    /// counter it shows what a client's transport really costs: 30 blobs as
    /// 30 requests over 30 connections, or as 2 requests over 1.
    pub requests: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub state: StateHandle,
    pub cfg: std::sync::Arc<ServerConfig>,
    pub server_id: [u8; 16],
    /// Committed-catalog event journal + long-poll wakeups.
    pub events: std::sync::Arc<super::events::EventHub>,
    /// Live game rooms — who is playing what, right now, and how to reach
    /// them. In memory and leased: a room is a process on somebody's desk,
    /// not a catalog entry.
    pub rooms: std::sync::Arc<super::rooms::RoomRegistry>,
    /// Direct CAS handle for data-plane bulk writes: pure path/budget config
    /// over the same `<root>/cas` the core owns, so connection threads can
    /// hash, fsync and rename object files WITHOUT occupying the state
    /// thread. Catalog rows for those objects still commit on the state
    /// thread, after the bytes are durable — the admission ordering law is
    /// unchanged.
    pub cas: std::sync::Arc<crate::FsCas>,
}

/// Serve one parsed request head to completion.
pub fn serve_request(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    plane: Plane,
    force_close: bool,
) -> Outcome {
    let result = match plane {
        Plane::Control => super::routes_control::dispatch(conn, head, rc),
        Plane::Data => super::routes_data::dispatch(conn, head, rc, force_close),
    };
    match result {
        Ok(outcome) => outcome,
        Err(f) => {
            if let Fail::Srv(
                e @ (ServerError::Io { .. }
                | ServerError::Db { .. }
                | ServerError::UnsupportedSchema { .. }),
            ) = &f
            {
                // The client sees an opaque 500; the operator sees the cause.
                log(rc.cfg.log, &format!("internal error on {} {}: {e}", head.method.as_str(), head.target));
            }
            let mut resp = fail_resp(&f);
            if fail_closes(&f) {
                resp = resp.closing();
            }
            Outcome::Resp(resp)
        }
    }
}

/// Run a closure on the state thread, mapping loss of the state thread to a
/// 503 and core refusals to their route responses.
pub fn call_state<T: Send + 'static>(
    state: &StateHandle,
    f: impl FnOnce(&mut StateCtx) -> ServerResult<T> + Send + 'static,
) -> RouteResult<T> {
    match state.call(f) {
        None => Err(Fail::StateDown),
        Some(Ok(v)) => Ok(v),
        Some(Err(e)) => Err(Fail::Srv(e)),
    }
}

/// Map a body framing violation to its refusal outcome. All of these close
/// the connection: the body stream is in an unknown state.
pub fn body_err_outcome(e: BodyError) -> Outcome {
    match e {
        BodyError::Timeout => Outcome::Resp(Resp::error(408, "body timeout").closing()),
        BodyError::Malformed => Outcome::Resp(Resp::error(400, "malformed body").closing()),
        BodyError::TooLarge => Outcome::Resp(Resp::error(413, "body too large").closing()),
        BodyError::Io => Outcome::Hangup,
    }
}

/// Read a whole bounded request body, mapping framing violations to their
/// refusal outcomes.
///
/// On `TooLarge` specifically, drains whatever the client declared is still
/// coming before returning the refusal: the caller is about to answer 413
/// and close, and closing over an unread declared remainder risks an
/// abortive RST that can drop the 413 response itself — see
/// [`Conn::drain_remaining`]. The drain gets its own copy of `deadline_ms`
/// rather than whatever was left of the read attempt's budget: the read
/// side of an oversized body typically fails near-instantly (the length is
/// known from the declared Content-Length, before a single body byte is
/// read), so a shared budget would leave almost nothing to drain with.
pub fn read_body(
    conn: &mut Conn,
    head: &mut Head,
    max: u64,
    deadline_ms: u64,
) -> Result<Vec<u8>, Outcome> {
    match conn.read_body_full(head, max, deadline_ms) {
        Ok(b) => Ok(b),
        Err(e) => {
            if matches!(e, BodyError::TooLarge) {
                let deadline = Instant::now() + Duration::from_millis(deadline_ms.max(1));
                conn.drain_remaining(head, deadline);
            }
            Err(body_err_outcome(e))
        }
    }
}

/// Capability check with ROOT BYPASS: the transport-recorded bootstrap
/// admin can provision and exercise the whole server without ceremonial
/// self-grants (it could grant itself any capability anyway — the bypass
/// removes the foot-gun where the documented default token 403s).
/// Non-root principals go through the core grant store unchanged.
pub fn require_cap(
    ctx: &super::state::StateCtx,
    p: &crate::PrincipalId,
    cap: crate::Capability,
    ns: &str,
) -> ServerResult<()> {
    if ctx.is_root(p)? {
        return Ok(());
    }
    ctx.core.auth().require(p, cap, ns)
}

/// The bearer secret, shape-checked. Missing or malformed credentials produce
/// the same uniform refusal an unknown token does.
pub fn secret_of(head: &Head) -> RouteResult<String> {
    super::api::bearer_secret(head.authorization.as_deref())
        .ok_or(Fail::Srv(ServerError::Unauthenticated))
}

/// 405 for a known path with the wrong method.
pub fn method_not_allowed() -> RouteResult<Outcome> {
    Ok(Outcome::Resp(Resp::error(405, "method not allowed")))
}

pub fn not_found() -> RouteResult<Outcome> {
    Ok(Outcome::Resp(Resp::error(404, "not found")))
}

/// GET and HEAD share every read route; anything else on them is 405.
pub fn is_read(m: Method) -> bool {
    matches!(m, Method::Get | Method::Head)
}
